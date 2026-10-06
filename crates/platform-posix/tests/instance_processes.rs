// SPDX-License-Identifier: MPL-2.0
//! The single-instance handoff between real processes: launches forwarded to the
//! owner exactly once, and an owner that crashed (leaving its socket behind) is
//! replaced by the next launch. The helper roles run this test binary again.
#![cfg(unix)]
use bareline_platform_posix::instance::{OpenRequest, Outcome, coordinate_in};
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

const ROLE: &str = "BARELINE_INSTANCE_ROLE";
const RUNTIME: &str = "BARELINE_INSTANCE_RUNTIME";
const SCOPE: &str = "BARELINE_INSTANCE_SCOPE";
const FILE: &str = "BARELINE_INSTANCE_FILE";

fn open(path: &str) -> OpenRequest {
    OpenRequest {
        paths: vec![PathBuf::from(path)],
        ..Default::default()
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // Short and under /tmp: a socket path must fit in 104 bytes on macOS.
        let root = PathBuf::from(format!(
            "/tmp/blp-{label}-{}-{}",
            std::process::id(),
            nanos % 1_000_000_000
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn runtime(&self) -> PathBuf {
        self.0.join("run")
    }
    fn scope(&self) -> PathBuf {
        self.0.join("profile").join("settings.toml")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
/// A helper process, killed if a test ends early so none outlives it.
struct Helper(Child);
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn helper(scratch: &Scratch, role: &str, file: &str) -> Helper {
    Helper(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "helper_role", "--ignored", "--nocapture", "--test-threads=1"])
            .env(ROLE, role)
            .env(RUNTIME, scratch.runtime())
            .env(SCOPE, scratch.scope())
            .env(FILE, file)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}
/// What the helper printed after `prefix`, waiting until it exits or prints it.
/// The test harness may print the test's name on the same line first.
fn line(helper: &mut Helper, prefix: &str) -> String {
    let stdout = helper.0.stdout.as_mut().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut text = String::new();
    loop {
        text.clear();
        if reader.read_line(&mut text).unwrap() == 0 {
            panic!("helper ended without printing {prefix}");
        }
        if let Some(at) = text.find(prefix) {
            return text[at + prefix.len()..].trim_end().to_owned();
        }
    }
}
fn drain(server: &bareline_platform_posix::instance::InstanceServer, count: usize) -> Vec<OpenRequest> {
    let watchdog = Instant::now() + Duration::from_secs(60);
    let mut received = Vec::new();
    while received.len() < count {
        assert!(Instant::now() < watchdog, "watchdog expired");
        match server.try_recv() {
            Some(request) => received.push(request),
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    received
}

/// The helper processes: `client` hands its file to the owner and prints the
/// outcome; `owner` becomes the owner, prints `ready` and serves until killed.
#[test]
#[ignore = "helper process for the two-process tests; started by them with --ignored"]
fn helper_role() {
    let (Some(role), Some(runtime), Some(scope), Some(file)) = (
        std::env::var_os(ROLE),
        std::env::var_os(RUNTIME),
        std::env::var_os(SCOPE),
        std::env::var(FILE).ok(),
    ) else {
        return;
    };
    let outcome = coordinate_in(
        Path::new(&runtime),
        Path::new(&scope),
        None,
        open(&file),
        false,
        Arc::new(|| {}),
    )
    .unwrap();
    match (role.to_str(), outcome) {
        (Some("client"), Outcome::Forwarded) => println!("outcome=forwarded"),
        (Some("client"), Outcome::Independent(reason)) => println!("outcome=independent {reason}"),
        (Some("client"), Outcome::Primary(_)) => println!("outcome=primary"),
        (Some("owner"), Outcome::Primary(_server)) => {
            println!("outcome=ready");
            // Served by the owner's own threads until the test kills this process.
            std::thread::sleep(Duration::from_secs(600));
        }
        (_, _) => println!("outcome=unexpected"),
    }
}

#[test]
fn launches_in_other_processes_are_forwarded_exactly_once() {
    let scratch = Scratch::new("forward");
    let Outcome::Primary(server) = coordinate_in(
        &scratch.runtime(),
        &scratch.scope(),
        None,
        OpenRequest::default(),
        false,
        Arc::new(|| {}),
    )
    .unwrap() else {
        panic!("the first launch owns the scope");
    };
    let files: Vec<String> = (0..4).map(|index| format!("/forward/{index}.txt")).collect();
    let mut clients: Vec<Helper> = files.iter().map(|file| helper(&scratch, "client", file)).collect();
    for client in &mut clients {
        assert_eq!(line(client, "outcome="), "forwarded");
        assert!(client.0.wait().unwrap().success());
    }
    let mut received = drain(&server, files.len());
    received.sort_by(|left, right| left.paths.cmp(&right.paths));
    assert_eq!(received, files.iter().map(|file| open(file)).collect::<Vec<_>>());
    // Each acknowledged request arrived once; nothing else is queued.
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.try_recv(), None);
    assert_eq!(server.pending(), 0);
}

#[test]
fn a_crashed_owner_is_replaced_by_the_next_launch() {
    let scratch = Scratch::new("crash");
    let mut owner = helper(&scratch, "owner", "/crash/owner.txt");
    assert_eq!(line(&mut owner, "outcome="), "ready");
    // While it runs, launches are forwarded to it, not owned here.
    let mut client = helper(&scratch, "client", "/crash/a.txt");
    assert_eq!(line(&mut client, "outcome="), "forwarded");
    client.0.wait().unwrap();
    // A crash: no cleanup runs, so the socket file stays behind.
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    let socket = std::fs::read_dir(scratch.runtime())
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| path.extension().is_some_and(|extension| extension == "sock"))
        .expect("the crashed owner left its socket");
    assert!(socket.exists());
    let Outcome::Primary(server) = coordinate_in(
        &scratch.runtime(),
        &scratch.scope(),
        None,
        OpenRequest::default(),
        false,
        Arc::new(|| {}),
    )
    .unwrap() else {
        panic!("the stale socket must not block the next owner");
    };
    let mut client = helper(&scratch, "client", "/crash/b.txt");
    assert_eq!(line(&mut client, "outcome="), "forwarded");
    client.0.wait().unwrap();
    assert_eq!(drain(&server, 1), vec![open("/crash/b.txt")]);
}
