// SPDX-License-Identifier: MPL-2.0
//! The session-end signals in a real process: the first SIGTERM, SIGHUP or
//! SIGINT reaches the application instead of ending the process, a second one
//! ends it at once, a process that does not exit is ended at the deadline, and
//! a signal inherited as ignored stays ignored. Signal dispositions are
//! process-wide, so each case runs this test binary again as a helper.
#![cfg(unix)]
use bareline_platform_posix::session_end::SessionEndSignals;
use std::{
    io::{BufRead, BufReader},
    os::unix::process::{CommandExt, ExitStatusExt},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

const ROLE: &str = "BARELINE_SIGNAL_ROLE";

/// A helper process, killed if a test ends early so none outlives it.
struct Helper(Child);
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn helper(role: &str, ignore_interrupt: bool) -> Helper {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "helper_role", "--ignored", "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: only signal(2), which is async-signal-safe, runs between fork and exec.
    unsafe {
        command.pre_exec(move || {
            // The harness may itself run with SIGINT ignored; each case decides.
            let disposition = if ignore_interrupt { libc::SIG_IGN } else { libc::SIG_DFL };
            libc::signal(libc::SIGINT, disposition);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            Ok(())
        });
    }
    Helper(command.spawn().unwrap())
}
/// What the helper printed after `prefix`; the harness may print the test's
/// name on the same line first.
fn line(helper: &mut Helper, prefix: &str) -> String {
    let mut reader = BufReader::new(helper.0.stdout.as_mut().unwrap());
    let mut text = String::new();
    loop {
        text.clear();
        if reader.read_line(&mut text).unwrap() == 0 {
            panic!("the helper exited before printing {prefix}");
        }
        if let Some(index) = text.find(prefix) {
            return text[index + prefix.len()..].trim().to_string();
        }
    }
}
fn signal(helper: &Helper, signal: libc::c_int) {
    // SAFETY: a plain kill(2) on a child this test owns and has not reaped.
    assert_eq!(unsafe { libc::kill(helper.0.id() as libc::pid_t, signal) }, 0);
}
fn wait(helper: &mut Helper, within: Duration) -> ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = helper.0.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "the helper did not exit in time");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The helper: catches the signals, reports which, and acts out its role.
#[test]
#[ignore = "run by the other tests as a helper process"]
fn helper_role() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    let deadline = if role == "deadline" {
        Duration::from_millis(300)
    } else {
        Duration::from_secs(60)
    };
    let signals = SessionEndSignals::install(move |signal| sender.send(signal).unwrap(), deadline).unwrap();
    assert!(
        SessionEndSignals::install(|_| {}, deadline).is_err(),
        "installed once per process"
    );
    println!("caught={:?}", signals.caught());
    let signal = receiver.recv().unwrap();
    println!("ended-by={signal}");
    if role == "flush" {
        // The application flushed and exits on its own.
        std::process::exit(0);
    }
    // "second" and "deadline": the application never gets to exit.
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn the_first_signal_reaches_the_application_which_exits_on_its_own() {
    for (sent, name) in [(libc::SIGTERM, "15"), (libc::SIGHUP, "1"), (libc::SIGINT, "2")] {
        let mut helper = helper("flush", false);
        assert_eq!(line(&mut helper, "caught="), "[15, 1, 2]");
        signal(&helper, sent);
        assert_eq!(line(&mut helper, "ended-by="), name);
        let status = wait(&mut helper, Duration::from_secs(10));
        assert_eq!(status.code(), Some(0), "{status:?}");
    }
}

#[test]
fn a_second_signal_ends_the_process_at_once() {
    let mut helper = helper("second", false);
    line(&mut helper, "caught=");
    signal(&helper, libc::SIGTERM);
    line(&mut helper, "ended-by=");
    let started = Instant::now();
    signal(&helper, libc::SIGINT);
    let status = wait(&mut helper, Duration::from_secs(10));
    assert_eq!(status.signal(), Some(libc::SIGINT), "{status:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_process_still_running_at_the_deadline_is_ended_by_the_signal() {
    let mut helper = helper("deadline", false);
    line(&mut helper, "caught=");
    signal(&helper, libc::SIGHUP);
    line(&mut helper, "ended-by=");
    let status = wait(&mut helper, Duration::from_secs(10));
    assert_eq!(status.signal(), Some(libc::SIGHUP), "{status:?}");
}

#[test]
fn a_signal_inherited_as_ignored_stays_ignored() {
    let mut helper = helper("flush", true);
    assert_eq!(line(&mut helper, "caught="), "[15, 1]");
    // Ctrl+C in a background job's terminal does not reach it.
    signal(&helper, libc::SIGINT);
    std::thread::sleep(Duration::from_millis(200));
    assert!(helper.0.try_wait().unwrap().is_none(), "SIGINT stays ignored");
    signal(&helper, libc::SIGTERM);
    assert_eq!(line(&mut helper, "ended-by="), "15");
    assert_eq!(wait(&mut helper, Duration::from_secs(10)).code(), Some(0));
}
