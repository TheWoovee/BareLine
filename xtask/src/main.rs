#[cfg(windows)]
mod bench;
#[cfg(windows)]
mod bench_diff;
#[cfg(windows)]
mod controller_fixture;
// SPDX-License-Identifier: MPL-2.0
#[cfg(windows)]
mod render;
mod capture;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if args.first().is_some_and(|a| a == "perf") && args.get(1).is_some_and(|a| a == "diff") {
        return bench_diff::run(&args[2..]);
    }
    #[cfg(windows)]
    if args.first().is_some_and(|a| a == "fixture") {
        return controller_fixture::run(&args[1..]);
    }
    #[cfg(windows)]
    if args.first().is_some_and(|a| a == "perf") && args.get(1).is_some_and(|a| a == "document") {
        return bench::run(&args[2..]);
    }
    #[cfg(windows)]
    if args.first().is_some_and(|arg| arg == "render") {
        return render::run(&args[1..]);
    }
    if args.len() < 2 || args[0] != "perf" || !["launch", "smoke"].contains(&args[1].as_str()) {
        return Err("Usage: cargo xtask perf launch|smoke [--release] [--samples N]".into());
    }
    let smoke = args[1] == "smoke";
    let release = args.iter().any(|arg| arg == "--release");
    let repetitions = args
        .iter()
        .position(|s| s == "--samples")
        .map(|i| {
            args.get(i + 1)
                .ok_or("missing sample count")?
                .parse::<usize>()
                .map_err(|_| "invalid sample count")
        })
        .transpose()?
        .unwrap_or(1);
    if !(1..=100).contains(&repetitions) {
        return Err("Sample count must be 1..100".into());
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let executable = root.join(if release {
        "target/release/bareline.exe"
    } else {
        "target/debug/bareline.exe"
    });
    if !executable.exists() {
        return Err(
            "Build the selected shell profile first; xtask never rebuilds it implicitly".into(),
        );
    }
    let mut hash = Sha256::new();
    let mut file = fs::File::open(&executable)?;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hash.update(&chunk[..read]);
    }
    let digest = format!("{:x}", hash.finalize());
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory = root.join("tests/perf/results");
    let isolated = directory.join(format!("launch-{stamp}"));
    fs::create_dir_all(isolated.join("data"))?;
    let measured_executable = isolated.join("bareline.exe");
    fs::copy(&executable, &measured_executable)?;
    let mut copied = fs::File::open(&measured_executable)?;
    let mut copied_hash = Sha256::new();
    loop {
        let read = copied.read(&mut chunk)?;
        if read == 0 { break; }
        copied_hash.update(&chunk[..read]);
    }
    if format!("{:x}", copied_hash.finalize()) != digest {
        return Err("Editor binary changed while preparing isolated measurement".into());
    }
    fs::write(isolated.join("bareline.portable"), [])?;
    let mut samples = Vec::new();
    let mut failures = 0usize;
    for repetition in 0..repetitions {
        // Alternate modes to reduce ordering bias; no claim that this clears the OS cache.
        for software in if repetition % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let started = Instant::now();
            let mut command = Command::new(&measured_executable);
            command
                .arg(if smoke { "--smoke" } else { "--perf" })
                .arg(if software { "--software" } else { "--hardware" })
                .args(["--no-session", "--no-extensions", "--new-instance"]);
            let result = match capture::run(&mut command, Duration::from_secs(30)) {
                Ok(result) => result,
                Err(error) => {
                    failures += 1;
                    samples.push(json!({"repetition": repetition, "requested_software": software,
                        "status": "spawn_or_capture_error", "error": error.to_string(), "frame": null, "idle": null}));
                    continue;
                }
            };
            let events: Vec<Value> = result.stdout
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            let frame = events
                .iter()
                .find(|e| e["event"] == "first_frame");
            let idle = events.iter().find(|e| e["event"] == "idle");
            let status = if result.status == "ok" && (frame.is_none() || (!smoke && idle.is_none())) { "missing_marker" } else { result.status };
            failures += usize::from(status != "ok");
            samples.push(json!({"repetition": repetition, "requested_software": software, "process_duration_us": started.elapsed().as_micros(), "frame": frame, "idle": idle,
                "status": status, "exit_code": result.exit_code, "stdout": result.stdout, "stderr": result.stderr}));
        }
    }
    let os_build = Command::new("cmd")
        .args(["/c", "ver"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    let result = json!({"schema_version": 2, "kind": if smoke { "hidden_render_smoke" } else { "visible_launch_idle" },
        "profile": if release { "release" } else { "debug" }, "os": std::env::consts::OS, "os_build": os_build,
        "arch": std::env::consts::ARCH, "machine": std::env::var("COMPUTERNAME").ok(), "logical_cpus": std::thread::available_parallelism().ok().map(|n| n.get()),
        "binary_sha256": digest, "cache_state": "uncontrolled", "isolated_directory": isolated, "failure_count": failures, "idle_delay_seconds": if smoke { 0 } else { 10 }, "samples": samples});
    let path = directory.join(format!("launch-{stamp}.json"));
    fs::write(&path, serde_json::to_vec_pretty(&result)?)?;
    println!("{}", path.display());
    if failures != 0 { return Err(format!("{failures} measurement trials failed; raw evidence preserved").into()); }
    Ok(())
}
