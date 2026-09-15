// SPDX-License-Identifier: MPL-2.0
//! Private launch interface: an authenticated editor process supplies invocation
//! context. The executable accepts no online source and never grants capabilities.
mod build_capabilities {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../build-support/capability_assertion.rs"
    ));
}
#[cfg(windows)]
fn run() -> Result<(), String> {
    use bareline_extensions_protocol::{Invocation, MAX_CHUNK_BYTES, decode};
    use bareline_platform_windows::extension_transport::AuthenticatedPipe;
    use sha2::{Digest, Sha256};
    use std::{
        io::Read,
        sync::{Arc, atomic::AtomicBool},
        time::{Duration, Instant},
    };
    let qualification_telemetry =
        std::env::var_os("BARELINE_QUALIFICATION_TELEMETRY").is_some_and(|value| value == "1");
    let qualification_case_id =
        std::env::var("BARELINE_QUALIFICATION_CASE_ID").unwrap_or_else(|_| "uncorrelated".into());
    let host_pid = std::process::id();
    let report_phase_start = |phase: &str| {
        if qualification_telemetry {
            bareline_extension_host::report_qualification_event(format!(
                "{{\"schema\":\"bareline.first-party-runtime.v1\",\"event\":\"phase_start\",\"case_id\":\"{qualification_case_id}\",\"host_pid\":{host_pid},\"phase\":\"{phase}\"}}"
            ));
        }
    };
    // `args_os` so a non-UTF-8 argv is an error instead of a panic.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string().map_err(|_| "argument encoding"))
        .collect::<Result<_, _>>()?;
    if !matches!(args.len(), 4 | 5) {
        return Err("expected pipe, parent PID, component path, component SHA256".into());
    }
    let budget = match args.get(4).map(String::as_str).unwrap_or("interactive") {
        "interactive" => bareline_extensions_protocol::ExecutionBudget::Interactive,
        "background" => bareline_extensions_protocol::ExecutionBudget::Background,
        _ => return Err("unknown execution budget".into()),
    };
    let parent = args[1].parse().map_err(|_| "parent PID")?;
    let authentication_started = Instant::now();
    report_phase_start("authentication");
    let mut pipe = AuthenticatedPipe::connect(&args[0], parent, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    if qualification_telemetry {
        bareline_extension_host::report_qualification_event(format!(
            "{{\"schema\":\"bareline.first-party-runtime.v1\",\"event\":\"phase\",\"case_id\":\"{qualification_case_id}\",\"host_pid\":{host_pid},\"phase\":\"authentication\",\"elapsed_us\":{}}}",
            authentication_started.elapsed().as_micros()
        ));
    }
    // The operation watchdog owns the execution deadline. Keep broker I/O behind
    // its 50 ms trap grace so a handled pipe timeout cannot stop that watchdog.
    pipe.set_timeout(Duration::from_millis(budget.timeout_ms()) + Duration::from_secs(1));
    let mut prefix = [0; 4];
    pipe.read_exact(&mut prefix).map_err(|e| e.to_string())?;
    let size = u32::from_le_bytes(prefix) as usize;
    if size > MAX_CHUNK_BYTES {
        return Err("invocation size".into());
    }
    let mut invocation = vec![0; size];
    pipe.read_exact(&mut invocation).map_err(|e| e.to_string())?;
    let context: Invocation = decode(&invocation).map_err(|_| "invalid invocation")?;
    if context.arguments.len() > 4096 {
        return Err("invocation arguments limit".into());
    }
    let hash_read_started = Instant::now();
    report_phase_start("hash_read");
    let file = std::fs::File::open(&args[2]).map_err(|e| e.to_string())?;
    let mut component = Vec::new();
    file.take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut component)
        .map_err(|e| e.to_string())?;
    let observed_hash = format!("{:x}", Sha256::digest(&component));
    if component.len() > 32 * 1024 * 1024 || observed_hash != args[3] {
        return Err("component hash or size".into());
    }
    if qualification_telemetry {
        let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
        let hash_read = hash_read_started.elapsed();
        bareline_extension_host::report_qualification_event(format!(
            "{{\"schema\":\"bareline.first-party-runtime.v1\",\"event\":\"phase\",\"case_id\":\"{qualification_case_id}\",\"host_pid\":{host_pid},\"phase\":\"hash_read\",\"elapsed_us\":{}}}",
            hash_read.as_micros()
        ));
        bareline_extension_host::report_qualification_event(format!(
            "{{\"schema\":\"bareline.first-party-runtime.v1\",\"event\":\"component\",\"case_id\":\"{qualification_case_id}\",\"host_pid\":{host_pid},\"profile\":\"{profile}\",\"sha256\":\"{observed_hash}\",\"component_bytes\":{},\"host_deadline_ms\":{},\"hash_read_us\":{}}}",
            component.len(),
            budget.timeout_ms(),
            hash_read.as_micros()
        ));
    }
    let runtime_started = Instant::now();
    report_phase_start("runtime_init");
    let runtime = bareline_extension_host::Runtime::new().map_err(|e| e.to_string())?;
    if qualification_telemetry {
        bareline_extension_host::report_qualification_event(format!(
            "{{\"schema\":\"bareline.first-party-runtime.v1\",\"event\":\"phase\",\"case_id\":\"{qualification_case_id}\",\"host_pid\":{host_pid},\"phase\":\"runtime_init\",\"elapsed_us\":{}}}",
            runtime_started.elapsed().as_micros()
        ));
    }
    runtime
        .invoke_with_policy(
            &component,
            invocation,
            Box::new(move |request| {
                // Forward only the re-encoded envelope: trailing bytes after a valid
                // frame must never reach the parent as a length prefix (SEC-07).
                let mut rest = &request[..];
                let parsed =
                    bareline_extensions_protocol::read_frame(&mut rest).map_err(|_| "malformed request".to_owned())?;
                if !rest.is_empty() {
                    return Err("trailing request bytes".into());
                }
                bareline_extensions_protocol::write_frame(&mut pipe, &parsed)
                    .map_err(|_| "request encoding".to_owned())?;
                let mut prefix = [0; 4];
                pipe.read_exact(&mut prefix).map_err(|e| e.to_string())?;
                let count = u32::from_le_bytes(prefix) as usize;
                if count > MAX_CHUNK_BYTES {
                    return Err("reply size".into());
                }
                let mut response = vec![0; count];
                pipe.read_exact(&mut response).map_err(|e| e.to_string())?;
                Ok(response)
            }),
            Arc::new(AtomicBool::new(false)),
            budget,
        )
        .map_err(|e| format!("{e:#}"))
}
/// Hidden verification entry point, present only in debug builds: read the two paths given
/// after `--sandbox-selftest` and report — via the exit code — whether the restricted-token
/// sandbox (SEC-03) allowed the granted file and denied the %USERPROFILE% file. Exit 0 means
/// both expectations held. Never compiled into a release host.
#[cfg(all(windows, debug_assertions))]
fn sandbox_selftest() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|flag| flag != "--sandbox-selftest") {
        return;
    }
    let (Some(grant), Some(deny)) = (args.next(), args.next()) else {
        std::process::exit(64);
    };
    let grant_ok = std::fs::read(&grant).is_ok();
    let deny_blocked = std::fs::read(&deny).is_err();
    let code = if grant_ok && deny_blocked {
        0
    } else {
        (!grant_ok as i32) + 2 * (!deny_blocked as i32)
    };
    std::process::exit(code);
}
fn main() {
    build_capabilities::retain();
    #[cfg(all(windows, debug_assertions))]
    sandbox_selftest();
    #[cfg(windows)]
    if let Err(error) = run() {
        eprintln!("Extension host stopped: {error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        eprintln!("Authenticated native host transport is unavailable on this platform");
        std::process::exit(1);
    }
}
