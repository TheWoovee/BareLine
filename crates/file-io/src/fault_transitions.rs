// SPDX-License-Identifier: MPL-2.0
//! Private test-only save transition seam. Never linked into production.
use super::*;
use std::{cell::RefCell, process::{Child, Command, Stdio}, time::{Duration, Instant}};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Point { StageCreated, StageWritten, BeforeStageFlush, StageFlushed, ExpectedFingerprintChecked, BeforeReplace, AfterReplace, TargetFingerprintChecked, BeforeReceipt }
const POINTS: [Point; 9] = [Point::StageCreated, Point::StageWritten, Point::BeforeStageFlush, Point::StageFlushed, Point::ExpectedFingerprintChecked, Point::BeforeReplace, Point::AfterReplace, Point::TargetFingerprintChecked, Point::BeforeReceipt];
thread_local! { static ACTIVE: RefCell<Option<(Point, Option<PathBuf>)>> = const { RefCell::new(None) }; }
pub(super) fn hit(point: Point) -> io::Result<()> {
    ACTIVE.with(|active| {
        let active = active.borrow();
        let Some((selected, marker)) = active.as_ref() else { return Ok(()); };
        if *selected != point { return Ok(()); }
        if let Some(marker) = marker {
            fs::write(marker, b"reached")?;
            loop { std::thread::sleep(Duration::from_millis(20)); }
        }
        Err(io::Error::new(io::ErrorKind::StorageFull, "injected save transition"))
    })
}
struct Reset;
impl Drop for Reset { fn drop(&mut self) { ACTIVE.with(|active| *active.borrow_mut() = None); } }
struct FixtureFs;
impl LocalFileSystem for FixtureFs {
    fn identity(&self, file: &File) -> io::Result<bareline_platform::FileIdentity> {
        let m = file.metadata()?;
        Ok(bareline_platform::FileIdentity { volume: 1, file: 1, length: m.len(), modified: m.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64 })
    }
    fn validate_target(&self, _: &Path) -> io::Result<()> { Ok(()) }
    fn commit(&self, stage: &Path, target: &Path, _: bool) -> io::Result<()> { fs::rename(stage, target) }
}
struct Scratch(PathBuf);
impl Scratch { fn new() -> Self {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!("bareline-pr020-save-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&path).unwrap(); Self(path)
} }
impl Drop for Scratch { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
struct OwnedChild(Child);
impl Drop for OwnedChild { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
fn exercise(root: &Path, point: Point, death: bool) {
    let target = root.join("target");
    fs::write(&target, b"OLD generation\n").unwrap();
    fs::write(root.join("backup"), b"OLD generation\n").unwrap();
    let expected = fingerprint(&target, &FixtureFs, &Cancellation::default()).unwrap();
    ACTIVE.with(|active| *active.borrow_mut() = Some((point, death.then(|| root.join("reached")))));
    let _reset = Reset;
    let result = save_bytes(&target, Some(&expected), &FixtureFs, &Cancellation::default(), |out| {
        out.write_all(b"NEW generation\n")?; Ok(())
    });
    assert!(result.is_err(), "fault was not reached: {point:?}");
}
fn reconcile(root: &Path, point: Point) {
    let target = fs::read(root.join("target")).unwrap();
    let after = matches!(point, Point::AfterReplace | Point::TargetFingerprintChecked | Point::BeforeReceipt);
    let expected: &[u8] = if after { b"NEW generation\n" } else { b"OLD generation\n" };
    assert_eq!(target, expected, "mixed or unexpected generation at {point:?}");
    let actual = fingerprint(&root.join("target"), &FixtureFs, &Cancellation::default()).unwrap();
    assert_eq!(actual.sha256, <[u8;32]>::from(Sha256::digest(expected)));
    assert_eq!(fs::read(root.join("backup")).unwrap(), b"OLD generation\n");
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().starts_with(".bareline-") {
            let bytes = fs::read(entry.path()).unwrap();
            assert!(bytes.is_empty() || bytes == b"NEW generation\n", "hybrid staged bytes");
        }
    }
}
#[test]
fn save_storage_full_transitions() {
    for point in POINTS { let root = Scratch::new(); exercise(&root.0, point, false); reconcile(&root.0, point); }
}
#[test]
fn save_process_death_transitions() {
    for (index, point) in POINTS.into_iter().enumerate() {
        let root = Scratch::new();
        let mut child = OwnedChild(Command::new(std::env::current_exe().unwrap()).args(["--exact", "lifecycle::fault_transitions::save_owned_child", "--nocapture"]).env("BARELINE_PR020_SAVE_ROOT", &root.0).env("BARELINE_PR020_SAVE_POINT", index.to_string()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.0.join("reached").exists() {
            assert!(child.0.try_wait().unwrap().is_none(), "child exited before {point:?}");
            assert!(Instant::now() < deadline, "child timeout at {point:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
        child.0.kill().unwrap(); assert!(!child.0.wait().unwrap().success());
        reconcile(&root.0, point);
    }
}
#[test]
fn save_owned_child() {
    let Some(root) = std::env::var_os("BARELINE_PR020_SAVE_ROOT") else { return; };
    let index: usize = std::env::var("BARELINE_PR020_SAVE_POINT").unwrap().parse().unwrap();
    exercise(Path::new(&root), POINTS[index], true);
    panic!("owned child did not stop");
}
