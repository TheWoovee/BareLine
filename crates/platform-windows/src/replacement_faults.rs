// SPDX-License-Identifier: MPL-2.0
//! Owned child death at the boundaries of actual native replacement calls.
use super::*;
use bareline_file_io::cancellation::Cancellation;
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
thread_local! { static ACTIVE: RefCell<Option<(bool, bool, PathBuf)>> = const { RefCell::new(None) }; }
pub(super) fn hit(existed: bool, after: bool) -> io::Result<()> {
    ACTIVE.with(|active| {
        let active = active.borrow();
        let Some((wanted_existed, wanted_after, marker)) = active.as_ref() else {
            return Ok(());
        };
        if existed != *wanted_existed || after != *wanted_after {
            return Ok(());
        }
        fs::write(marker, b"reached")?;
        loop {
            std::thread::sleep(Duration::from_millis(20));
        }
    })
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bareline-pr020-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn native_replacement_process_death() {
    for existed in [false, true] {
        for after in [false, true] {
            let root = Scratch::new();
            let mut child = OwnedChild(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "files::replacement_faults::native_owned_child",
                        "--nocapture",
                    ])
                    .env("BARELINE_PR020_NATIVE_ROOT", &root.0)
                    .env("BARELINE_PR020_NATIVE_EXISTED", existed.to_string())
                    .env("BARELINE_PR020_NATIVE_AFTER", after.to_string())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(30);
            while !root.0.join("reached").exists() {
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "native child exited before boundary"
                );
                assert!(Instant::now() < deadline, "native child timed out");
                std::thread::sleep(Duration::from_millis(10));
            }
            child.0.kill().unwrap();
            assert!(!child.0.wait().unwrap().success());
            let target = root.0.join("target");
            if !existed && !after {
                assert!(!target.exists());
            } else {
                let bytes = fs::read(&target).unwrap();
                let expected: &[u8] = if after {
                    b"new complete generation"
                } else {
                    b"old complete generation"
                };
                assert_eq!(Sha256::digest(&bytes), Sha256::digest(expected));
                assert_eq!(bytes, expected);
            }
            assert_eq!(root.0.join("stage").exists(), !after);
            let recovered = bareline_file_io::lifecycle::inspect_save_transactions(
                &root.0,
                &super::WindowsFileSystem,
                &Cancellation::default(),
            )
            .unwrap();
            assert_eq!(recovered.len(), 1);
            assert_eq!(recovered[0].state, CommitState::Precommit);
            assert_eq!(
                fs::read(&recovered[0].editor_version).unwrap(),
                b"new complete generation"
            );
            if existed && after {
                assert_eq!(
                    fs::read(recovered[0].other_version.as_ref().unwrap()).unwrap(),
                    b"old complete generation"
                );
            }
        }
    }
}
#[test]
fn native_owned_child() {
    let Some(root) = std::env::var_os("BARELINE_PR020_NATIVE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let existed: bool = std::env::var("BARELINE_PR020_NATIVE_EXISTED").unwrap().parse().unwrap();
    let after: bool = std::env::var("BARELINE_PR020_NATIVE_AFTER").unwrap().parse().unwrap();
    if existed {
        fs::write(root.join("target"), b"old complete generation").unwrap();
    }
    fs::write(root.join("stage"), b"new complete generation").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(root.join("stage"))
        .unwrap()
        .sync_all()
        .unwrap();
    ACTIVE.with(|active| *active.borrow_mut() = Some((existed, after, root.join("reached"))));
    let transaction = WindowsFileSystem
        .prepare_commit(
            &root.join("stage"),
            &root.join("target"),
            if existed {
                CommitMode::Replace
            } else {
                CommitMode::CreateNew
            },
            &bareline_file_io::cancellation::Cancellation::default(),
        )
        .unwrap();
    WindowsFileSystem.commit_transaction(transaction).unwrap();
    panic!("native child did not stop");
}
