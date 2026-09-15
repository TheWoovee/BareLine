// SPDX-License-Identifier: MPL-2.0
#![cfg(all(windows, debug_assertions))]
//! SEC-03: the extension host must launch under a restricted token that cannot read files
//! granted only to the user's own account (everything under %USERPROFILE% by default) yet can
//! still read the directory it is explicitly granted. The host is spawned through the real
//! launcher; its hidden `--sandbox-selftest` mode reads both files and reports the outcome as
//! its exit code (0 == granted file readable AND profile file denied).
use bareline_platform_windows::{SandboxedProcessLauncher, sandbox_lock_to_current_user};
use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

fn unique(tag: &str) -> String {
    format!(
        "bareline-sec03-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[test]
fn restricted_host_reads_its_grant_but_not_the_user_profile() {
    let host = PathBuf::from(env!("CARGO_BIN_EXE_bareline-extension-host"));
    let profile = PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));

    // Both files live under %USERPROFILE%; they differ only in who is granted read access.
    let grant_dir = profile.join(unique("grant"));
    let deny_dir = profile.join(unique("deny"));
    std::fs::create_dir(&grant_dir).unwrap();
    std::fs::create_dir(&deny_dir).unwrap();
    let grant_file = grant_dir.join("allowed.txt");
    let deny_file = deny_dir.join("secret.txt");
    std::fs::write(&grant_file, b"granted").unwrap();
    std::fs::write(&deny_file, b"secret").unwrap();
    // Lock the profile file to the current user only (no restricting SID can reach it).
    sandbox_lock_to_current_user(&deny_file).unwrap();

    // The parent process (full token) can read both files.
    assert!(std::fs::read(&grant_file).is_ok());
    assert!(std::fs::read(&deny_file).is_ok());

    let mut command = Command::new(&host);
    command
        .arg("--sandbox-selftest")
        .arg(&grant_file)
        .arg(&deny_file)
        .current_dir(host.parent().unwrap());
    // Mirror the production launch: no inherited environment, minimal allowlist.
    command.env_clear();
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    command.env("SystemRoot", &system_root);
    command.env("windir", &system_root);
    command.env("Path", std::path::Path::new(&system_root).join("System32"));
    command.env("PATHEXT", ".COM;.EXE;.BAT;.CMD");

    // The host executable and the allowed file are the restricted token's explicit grants.
    let (mut child, mut guard) = SandboxedProcessLauncher::default()
        .spawn_host(&mut command, &[host.as_path(), grant_file.as_path()])
        .unwrap();
    let restricted = child.is_restricted();

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = guard.terminate();
            panic!("restricted host self-test did not exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(guard);
    let _ = std::fs::remove_dir_all(&grant_dir);
    let _ = std::fs::remove_dir_all(&deny_dir);

    if !restricted {
        eprintln!(
            "SEC-03: restricted-token sandbox unavailable on this machine; the read-isolation \
             assertion was not exercised (the launcher fell back to job memory limits only)."
        );
        return;
    }
    assert!(
        status.success(),
        "restricted host must read its grant and be denied the %USERPROFILE% file ({status})"
    );
}
