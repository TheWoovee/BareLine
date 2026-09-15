// SPDX-License-Identifier: MPL-2.0
#![cfg(windows)]
use bareline_extensions_protocol::{Invocation, encode};
use bareline_macros::process::ProcessLauncher;
use bareline_platform_windows::{WindowsProcessLauncher, extension_transport::PipeServer};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn run_fixture(component: &str) -> bool {
    let root = std::env::temp_dir().join(format!(
        "bareline-host-fixture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    // The production host accepts binary components only (SEC-08), so text fixtures
    // are assembled by the test rather than by the host.
    let file = root.join("fixture.wasm");
    let bytes = wat::parse_str(component).unwrap_or_else(|_| component.as_bytes().to_vec());
    std::fs::write(&file, &bytes).unwrap();
    let server = PipeServer::create().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_bareline-extension-host"));
    command
        .args([server.name(), &std::process::id().to_string()])
        .arg(&file)
        .arg(format!("{:x}", Sha256::digest(&bytes)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // This test intentionally exercises locally built fixtures below the publisher
    // verification gate; no production trust bypass exists in run_verified_host.
    let (mut child, mut job) = WindowsProcessLauncher.spawn(&mut command).unwrap();
    let mut pipe = server.accept(child.id(), Duration::from_secs(5)).unwrap();
    let context = Invocation {
        extension_id: "fixture".into(),
        command: "fixture.run".into(),
        arguments: String::new(),
        document: 1,
        revision: 1,
        source_generation: 1,
        text_length: 0,
        raw_length: 0,
        grant_generation: 1,
    };
    let bytes = encode(&context).unwrap();
    pipe.write_all(&(bytes.len() as u32).to_le_bytes()).unwrap();
    pipe.write_all(&bytes).unwrap();
    let deadline = Instant::now() + Duration::from_secs(7);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            job.terminate().unwrap();
            panic!("host exceeded process isolation deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(pipe);
    drop(job);
    std::fs::remove_file(file).unwrap();
    std::fs::remove_dir(root).unwrap();
    status.success()
}
#[test]
fn real_authenticated_host_safe_and_cpu_loop_leave_parent_alive() {
    assert!(run_fixture(
        "(component (core module $m (func (export \"run\"))) (core instance $i (instantiate $m)) (func (export \"run\") (canon lift (core func $i \"run\"))))"
    ));
    assert!(!run_fixture(
        "(component (core module $m (func (export \"run\") (loop $l br $l))) (core instance $i (instantiate $m)) (func (export \"run\") (canon lift (core func $i \"run\"))))"
    ));
    assert!(!run_fixture("not a component"));
}
