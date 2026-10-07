// SPDX-License-Identifier: MPL-2.0
#![cfg(all(target_os = "linux", debug_assertions))]
//! SEC-03 on Linux: the extension host runs under Landlock, so it can read the
//! file it is granted but nothing else of the user's, starting with $HOME. The
//! host is spawned through the real Linux sandbox; its hidden `--sandbox-selftest`
//! mode reads both paths and reports the outcome as its exit code (0 == granted
//! file readable AND denied path refused). Where the kernel has no Landlock the
//! default sandbox must refuse to start the host and say why (SEC-05).
//!
//! The verified launch is exercised end to end as well: hash checks, the
//! authenticated socket, the invocation frame, and the watchdog that stops a
//! runaway component.
use bareline_extensions_protocol::{BrokerResponse, ExecutionBudget, Invocation};
use bareline_platform_linux::{
    LinuxHostSandbox,
    extension_transport::{HostLaunch, HostLifecycle, Isolation, run_verified_host_observed},
    sandbox::probe,
};
use bareline_platform_posix::extension_transport::{HostSandbox, HostSpawn};
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bareline-sec03-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn host() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bareline-extension-host"))
}
/// Whether this kernel enforces Landlock. The branch taken is printed (visible
/// with `--nocapture`), and with `BARELINE_REQUIRE_LANDLOCK=1` a kernel without
/// Landlock fails the test instead of exercising only the refusal.
fn landlock() -> bool {
    let isolation = probe();
    eprintln!("SEC-03 sandbox probe: {isolation}");
    if std::env::var_os("BARELINE_REQUIRE_LANDLOCK").is_some_and(|value| value == "1") {
        assert!(
            matches!(isolation, Isolation::Enforced(_)),
            "BARELINE_REQUIRE_LANDLOCK=1 but {isolation}"
        );
    }
    matches!(isolation, Isolation::Enforced(_))
}
/// Runs the self-test through the sandbox and waits for its exit status.
fn sandboxed_selftest(sandbox: &LinuxHostSandbox, grant: &Path, deny: &Path) -> io::Result<(ExitStatus, Isolation)> {
    let executable = host();
    let file = std::fs::File::open(&executable)?;
    let mut spawned = sandbox.spawn_host(&HostSpawn {
        executable: &executable,
        executable_file: &file,
        // The host's component grant is the file it must be able to read.
        component: grant,
        arguments: vec!["--sandbox-selftest".into(), grant.into(), deny.into()],
        budget: ExecutionBudget::Interactive,
        socket: None,
    })?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = spawned.child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = spawned.guard.terminate();
            panic!("sandboxed self-test did not exit");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok((status, spawned.isolation))
}

#[test]
fn landlocked_host_reads_its_grant_but_not_the_home_folder() {
    let scratch = Scratch::new("selftest");
    let (grant_dir, deny_dir) = (scratch.0.join("grant"), scratch.0.join("deny"));
    std::fs::create_dir(&grant_dir).unwrap();
    std::fs::create_dir(&deny_dir).unwrap();
    let (grant, deny) = (grant_dir.join("allowed.txt"), deny_dir.join("secret.txt"));
    std::fs::write(&grant, b"granted").unwrap();
    std::fs::write(&deny, b"secret").unwrap();
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    // Unconfined, the host reads everything: the self-test fails (exit 2, the
    // denied file was readable), which is what makes the sandboxed run meaningful.
    let unconfined = Command::new(host())
        .args(["--sandbox-selftest".as_ref(), grant.as_os_str(), deny.as_os_str()])
        .status()
        .unwrap();
    assert_eq!(unconfined.code(), Some(2));
    assert!(std::fs::read_dir(&home).is_ok());
    if !landlock() {
        // SEC-05: no sandbox, no host; the refusal names the missing isolation.
        let error = sandboxed_selftest(&LinuxHostSandbox::default(), &grant, &deny).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().starts_with("isolation=Unsupported"), "{error}");
        return;
    }
    let sandbox = LinuxHostSandbox::default();
    let (status, isolation) = sandboxed_selftest(&sandbox, &grant, &deny).unwrap();
    assert!(matches!(isolation, Isolation::Enforced(_)), "{isolation}");
    assert!(
        status.success(),
        "granted file must be readable and the other refused ({status})"
    );
    let (status, _) = sandboxed_selftest(&sandbox, &grant, &home).unwrap();
    assert!(status.success(), "$HOME must not be listable from the host ({status})");
    // An explicit read grant opens exactly that folder.
    let granted = LinuxHostSandbox::default().with_read_grant(&deny_dir);
    let (status, _) = sandboxed_selftest(&granted, &grant, &deny).unwrap();
    assert_eq!(status.code(), Some(2), "a granted folder is readable");
    eprintln!("SEC-03 Landlock assertions ran: grant readable, other folder and $HOME refused");
}

const SAFE: &str = "(component (core module $m (func (export \"run\"))) (core instance $i (instantiate $m)) (func (export \"run\") (canon lift (core func $i \"run\"))))";
const LOOP: &str = "(component (core module $m (func (export \"run\") (loop $l br $l))) (core instance $i (instantiate $m)) (func (export \"run\") (canon lift (core func $i \"run\"))))";

fn invocation() -> Invocation {
    Invocation {
        extension_id: "fixture".into(),
        command: "fixture.run".into(),
        arguments: String::new(),
        document: 1,
        revision: 1,
        source_generation: 1,
        text_length: 0,
        raw_length: 0,
        grant_generation: 1,
    }
}
fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
/// The host as a release would install it: a debug build carries over 256 MiB
/// of debug information, beyond the size the verified launch hashes.
fn installed_host(scratch: &Scratch) -> PathBuf {
    let installed = scratch.0.join("bareline-extension-host");
    if !installed.exists() {
        let status = Command::new("strip")
            .arg("--strip-debug")
            .arg("-o")
            .arg(&installed)
            .arg(host())
            .status()
            .expect("binutils `strip` installs the test host");
        assert!(status.success());
    }
    installed
}
/// One verified launch of a fixture component; returns the outcome and the
/// lifecycle the editor observed. `cancel_after_auth` cancels it once connected.
fn launch(scratch: &Scratch, component: &str, cancel_after_auth: bool) -> (io::Result<()>, Vec<String>) {
    let bytes = wat::parse_str(component).unwrap_or_else(|_| component.as_bytes().to_vec());
    let file = scratch.0.join("fixture.wasm");
    std::fs::write(&file, &bytes).unwrap();
    let executable = installed_host(scratch);
    let signer = bareline_distribution::update::PublisherPin {
        subject: String::new(),
        issuers: Vec::new(),
    };
    let invocation = invocation();
    let cancelled = Arc::new(AtomicBool::new(false));
    let canceller = cancelled.clone();
    let mut phases = Vec::new();
    let result = run_verified_host_observed(
        HostLaunch {
            executable: &executable,
            executable_sha256: digest(&std::fs::read(&executable).unwrap()),
            signer: &signer,
            component: &file,
            component_sha256: digest(&bytes),
            invocation: &invocation,
            budget: ExecutionBudget::Interactive,
        },
        cancelled,
        |phase| {
            if cancel_after_auth && matches!(phase, HostLifecycle::Authenticated(_)) {
                canceller.store(true, Ordering::Release);
            }
            phases.push(format!("{phase:?}").split('(').next().unwrap_or_default().to_owned());
        },
        |_| BrokerResponse {
            request_id: 0,
            result: Err("no broker in this test".into()),
        },
    );
    (result, phases)
}

#[test]
fn verified_host_runs_a_component_under_the_sandbox_and_stops_a_runaway() {
    let scratch = Scratch::new("launch");
    if !landlock() {
        let (result, phases) = launch(&scratch, SAFE, false);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert!(phases.is_empty(), "nothing starts without the sandbox");
        return;
    }
    let (result, phases) = launch(&scratch, SAFE, false);
    result.unwrap();
    assert_eq!(phases, ["Started", "Authenticated", "Drained"]);
    // A component that never returns is stopped by the watchdog once cancelled.
    let started = Instant::now();
    let (result, phases) = launch(&scratch, LOOP, true);
    assert!(result.is_err());
    assert_eq!(phases, ["Started", "Authenticated", "Drained"]);
    assert!(started.elapsed() < Duration::from_secs(30), "the runaway was stopped");
    // Bytes that are not a component end the host with a failure.
    let (result, _) = launch(&scratch, "not a component", false);
    assert!(result.is_err());
    // A changed host binary is refused before anything starts.
    let file = scratch.0.join("fixture.wasm");
    let invocation = invocation();
    let signer = bareline_distribution::update::PublisherPin {
        subject: String::new(),
        issuers: Vec::new(),
    };
    let error = run_verified_host_observed(
        HostLaunch {
            executable: &installed_host(&scratch),
            executable_sha256: [0; 32],
            signer: &signer,
            component: &file,
            component_sha256: digest(&std::fs::read(&file).unwrap()),
            invocation: &invocation,
            budget: ExecutionBudget::Interactive,
        },
        Arc::new(AtomicBool::new(false)),
        |_| panic!("nothing starts"),
        |_| panic!("nothing is brokered"),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "runtime hash");
    eprintln!("SEC-03 verified launch ran under Landlock: component, runaway, bad component, bad hash");
}
