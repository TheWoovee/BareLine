#[cfg(windows)]
mod bench;
#[cfg(windows)]
mod bench_diff;
#[cfg(windows)]
mod controller_fixture;
// SPDX-License-Identifier: MPL-2.0
#[cfg(windows)]
mod render;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    process::{Command, Stdio},
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
    let mut samples = Vec::new();
    for repetition in 0..repetitions {
        // Alternate modes to reduce ordering bias; no claim that this clears the OS cache.
        for software in if repetition % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let started = Instant::now();
            let mut command = Command::new(&executable);
            command
                .arg(if smoke { "--smoke" } else { "--perf" })
                .arg(if software { "--software" } else { "--hardware" });
            let mut child = command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            loop {
                if child.try_wait()?.is_some() {
                    break;
                }
                if started.elapsed() > Duration::from_secs(30) {
                    child.kill()?;
                    child.wait()?;
                    return Err(
                        "Native measurement timed out after 30 seconds; child reaped".into(),
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let result = child.wait_with_output()?;
            if !result.status.success() {
                return Err(
                    format!("Shell failed: {}", String::from_utf8_lossy(&result.stderr)).into(),
                );
            }
            let output = String::from_utf8(result.stdout)?;
            let events: Vec<Value> = output
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            let frame = events
                .iter()
                .find(|e| e["event"] == "first_frame")
                .ok_or("No first-frame marker")?;
            let idle = events.iter().find(|e| e["event"] == "idle");
            if !smoke && idle.is_none() {
                return Err("No 10-second idle sample".into());
            }
            samples.push(json!({"repetition": repetition, "requested_software": software, "process_duration_us": started.elapsed().as_micros(), "frame": frame, "idle": idle}));
        }
    }
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let os_build = Command::new("cmd")
        .args(["/c", "ver"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    let result = json!({"schema_version": 2, "kind": if smoke { "hidden_render_smoke" } else { "visible_launch_idle" },
        "profile": if release { "release" } else { "debug" }, "os": std::env::consts::OS, "os_build": os_build,
        "arch": std::env::consts::ARCH, "machine": std::env::var("COMPUTERNAME").ok(), "logical_cpus": std::thread::available_parallelism().ok().map(|n| n.get()),
        "binary_sha256": digest, "cache_state": "uncontrolled", "idle_delay_seconds": if smoke { 0 } else { 10 }, "samples": samples});
    let directory = root.join("tests/perf/results");
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("launch-{timestamp}.json"));
    fs::write(&path, serde_json::to_vec_pretty(&result)?)?;
    println!("{}", path.display());
    Ok(())
}
