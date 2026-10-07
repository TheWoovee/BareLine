// SPDX-License-Identifier: MPL-2.0
//! Seam-wide checks; each concern's own tests live beside it.
use super::*;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

#[test]
fn missing_native_services_answer_in_plain_language() {
    let refusal = "This system does not support";
    assert!(resolve_program("sh").unwrap_err().starts_with(refusal));
    assert!(
        shell_integration::reveal(Path::new("/tmp"))
            .unwrap_err()
            .starts_with(refusal)
    );
    assert!(shell_integration::TrayIcon::new(0).is_err());
    assert_eq!(
        update::validate_install_root(Path::new("/opt/bareline"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::Unsupported
    );
}

#[test]
fn file_watching_reports_a_change_in_a_watched_folder() {
    let folder = std::env::temp_dir().join(format!("bareline-seam-watch-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let woken = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = woken.clone();
    let service = WatchService::start_notifying(
        vec![folder.clone()],
        Arc::new(move || flag.store(true, std::sync::atomic::Ordering::Release)),
    )
    .unwrap();
    std::fs::write(folder.join("changed.txt"), b"external").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut seen = false;
    while !seen && std::time::Instant::now() < deadline {
        while let Some(event) = service.try_recv() {
            seen |= format!("{event:?}").contains("changed.txt");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(service);
    let _ = std::fs::remove_dir_all(&folder);
    assert!(seen, "the watcher reports the new file");
    assert!(woken.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn a_second_launch_hands_its_files_to_the_first() {
    // The same handoff the editor runs, in a private runtime folder whose
    // socket path stays within the 104 bytes macOS allows.
    let runtime = PathBuf::from(format!("/tmp/bl-seam-{}", std::process::id()));
    let scope = runtime.join("settings.json");
    let request = |path: &str| instance::OpenRequest {
        paths: vec![PathBuf::from(path)],
        ..Default::default()
    };
    let first = bareline_platform_posix::instance::coordinate_in(
        &runtime,
        &scope,
        None,
        request("/tmp/a.txt"),
        false,
        Arc::new(|| {}),
    )
    .unwrap();
    let instance::Outcome::Primary(server) = first else {
        panic!("the first launch owns the profile");
    };
    server.set_accepting(true);
    let second = bareline_platform_posix::instance::coordinate_in(
        &runtime,
        &scope,
        None,
        request("/tmp/b.txt"),
        false,
        Arc::new(|| {}),
    )
    .unwrap();
    assert!(matches!(second, instance::Outcome::Forwarded));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut received = None;
    while received.is_none() && std::time::Instant::now() < deadline {
        received = server.try_recv();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        received.map(|request| request.paths),
        Some(vec![PathBuf::from("/tmp/b.txt")])
    );
    drop(server);
    let _ = std::fs::remove_dir_all(&runtime);
}
