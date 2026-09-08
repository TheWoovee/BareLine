// SPDX-License-Identifier: MPL-2.0
//! Run through Cargo only: parent kills exclusively the child it spawned here.
use bareline_file_io::{cancellation::Cancellation, recovery::{self, Boundary, FaultInjector, RecoveryEdit, RecoveryMetadata, RecoveryWriter}};
use bareline_platform::{FileIdentity, LocalFileSystem};
use std::{fs::{self, File}, io, path::{Path, PathBuf}, process::{Child, Command, Stdio}, sync::atomic::{AtomicU64, Ordering}, time::{Duration, Instant}};
const POINTS: [Boundary; 6] = [Boundary::SegmentsWritten, Boundary::SegmentsFlushed, Boundary::JournalWritten, Boundary::JournalFlushed, Boundary::CheckpointWritten, Boundary::CheckpointCommitted];
struct Fs;
impl LocalFileSystem for Fs {
    fn identity(&self, _: &File) -> io::Result<FileIdentity> { Err(io::Error::other("unused fixture capability")) }
    fn validate_target(&self, _: &Path) -> io::Result<()> { Ok(()) }
    fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> { fs::rename(staged, target) }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("bareline-pr020-death-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap(); Self(path)
    }
}
impl Drop for Scratch { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
struct OwnedChild(Child);
impl Drop for OwnedChild { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
struct Stop { at: Boundary, marker: Option<PathBuf> }
impl FaultInjector for Stop {
    fn boundary(&mut self, at: Boundary) -> io::Result<()> {
        if at != self.at { return Ok(()); }
        if let Some(marker) = &self.marker {
            // Marker is closed before publication; parent never guesses a process ID.
            fs::write(marker, b"reached")?;
            loop { std::thread::sleep(Duration::from_millis(20)); }
        }
        Err(io::Error::new(io::ErrorKind::StorageFull, "injected storage boundary"))
    }
}
fn edit(removed: &[u8], inserted: &[u8]) -> RecoveryEdit { RecoveryEdit { offset: 1, removed: removed.to_vec(), inserted: inserted.to_vec() } }
fn exercise(root: &Path, at: Boundary, death: bool) {
    fs::write(root.join("original"), b"hello").unwrap();
    let mut writer = RecoveryWriter::create(&root.join("journal"), RecoveryMetadata { original_path: Some(root.join("original")), source_generation: "fixture".into(), codec_catalog_version: "utf8-v1".into(), original_len: 5 }, &Fs).unwrap();
    writer.seal_baseline(&mut &b"hello"[..], || Ok(true), &Cancellation::default(), &Fs).unwrap();
    writer.append(1, &[edit(b"ell", b"ipp")]).unwrap();
    writer.checkpoint(&Fs).unwrap();
    let mut stop = Stop { at, marker: death.then(|| root.join("reached")) };
    let result = if matches!(at, Boundary::CheckpointWritten | Boundary::CheckpointCommitted) {
        writer.append(2, &[edit(b"ipp", b"amm")]).unwrap();
        writer.checkpoint_with_faults(&Fs, &mut stop)
    } else { writer.append_with_faults(2, &[edit(b"ipp", b"amm")], &mut stop).map(|_| ()) };
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::StorageFull);
}
fn reconcile(root: &Path, at: Boundary) {
    let inspection = recovery::recover_to(&root.join("journal"), &root.join("recovered"), &Cancellation::default()).unwrap();
    let revision = inspection.last_durable.unwrap().revision;
    let bytes = fs::read(root.join("recovered")).unwrap();
    assert!(revision == 1 || revision == 2);
    assert_eq!(bytes, if revision == 1 { b"hippo" } else { b"hammo" });
    if matches!(at, Boundary::JournalFlushed | Boundary::CheckpointWritten | Boundary::CheckpointCommitted) { assert_eq!(revision, 2); }
    assert_eq!(fs::read(root.join("original")).unwrap(), b"hello");
    let mut count = 0;
    recovery::replay_transactions(&root.join("journal"), &Cancellation::default(), |receipt, edits| {
        count += 1; assert_eq!(receipt.revision, count);
        assert_eq!(edits.len(), 1); assert_eq!(edits[0].offset, 1);
        let (removed, inserted): (&[u8], &[u8]) = if count == 1 { (b"ell", b"ipp") } else { (b"ipp", b"amm") };
        assert_eq!(edits[0].removed, removed); assert_eq!(edits[0].inserted, inserted); Ok(())
    }).unwrap();
    assert_eq!(count, revision); // No byte/history prefix disagreement.
}
#[test]
fn recovery_storage_full_boundaries_preserve_matching_history() {
    for at in POINTS { let root = Scratch::new(); exercise(&root.0, at, false); reconcile(&root.0, at); }
}
#[test]
fn recovery_process_death_boundaries_preserve_matching_history() {
    for (index, at) in POINTS.into_iter().enumerate() {
        let root = Scratch::new();
        let mut child = OwnedChild(Command::new(std::env::current_exe().unwrap()).args(["--exact", "recovery_owned_child", "--nocapture"]).env("BARELINE_PR020_CHILD_ROOT", &root.0).env("BARELINE_PR020_BOUNDARY", index.to_string()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.0.join("reached").exists() {
            assert!(child.0.try_wait().unwrap().is_none(), "child exited before {at:?}");
            assert!(Instant::now() < deadline, "child boundary timeout {at:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
        child.0.kill().unwrap(); assert!(!child.0.wait().unwrap().success());
        reconcile(&root.0, at);
    }
}
#[test]
fn recovery_owned_child() {
    let Some(root) = std::env::var_os("BARELINE_PR020_CHILD_ROOT") else { return; };
    let index: usize = std::env::var("BARELINE_PR020_BOUNDARY").unwrap().parse().unwrap();
    exercise(Path::new(&root), POINTS[index], true);
    panic!("child must stop at requested boundary");
}
