// SPDX-License-Identifier: MPL-2.0
#![cfg(windows)]
use bareline_extensions_protocol::*;
use bareline_platform_windows::{
    SandboxedProcessLauncher, extension_transport::PipeServer, sandbox_grant_restricted_qualification_write,
};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT_CASE_ID: AtomicU64 = AtomicU64::new(1);
const MAX_STAGED_EDIT_BYTES: usize = MEMORY_LIMIT / 2;

fn qualification_case_id(command: &str) -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        NEXT_CASE_ID.fetch_add(1, Ordering::Relaxed),
        command.replace('.', "_")
    )
}

/// Fixture launch remains below the production signature gate and uses only
/// locally built components. Every request crosses the real authenticated pipe.
struct CaseDiagnostics<'a> {
    case_id: &'a str,
    host_pid: u32,
    command: &'a str,
    child: &'a std::cell::RefCell<bareline_platform_windows::SandboxedChild>,
    started: Instant,
    complete: bool,
    requests: u64,
    requested_text_bytes: u64,
    requested_original_bytes: u64,
    last_text_end: u64,
    completed_replies: u64,
    completed_response_bytes: u64,
    read_wall: Duration,
    encode_wall: Duration,
    write_wall: Duration,
}
impl Drop for CaseDiagnostics<'_> {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        let status = match self.child.try_borrow_mut() {
            Ok(mut child) => match child.try_wait() {
                Ok(Some(status)) => status.to_string(),
                Ok(None) => "running".into(),
                Err(error) => format!("wait error: {error}"),
            },
            Err(_) => "child status borrowed during failure".into(),
        };
        eprintln!(
            "PR-T10 failure diagnostic: case_id={} host_pid={} command={} elapsed_ms={} requests={} requested_text_bytes={} requested_original_bytes={} last_text_end={} completed_replies={} completed_response_bytes={} read_wall_ms={} encode_wall_ms={} write_wall_ms={} child_status={}",
            self.case_id,
            self.host_pid,
            self.command,
            self.started.elapsed().as_millis(),
            self.requests,
            self.requested_text_bytes,
            self.requested_original_bytes,
            self.last_text_end,
            self.completed_replies,
            self.completed_response_bytes,
            self.read_wall.as_millis(),
            self.encode_wall.as_millis(),
            self.write_wall.as_millis(),
            status
        );
    }
}

struct CaseResult {
    output: String,
    elapsed: Duration,
    requests: u64,
    requested_text_bytes: u64,
    requested_original_bytes: u64,
    committed_edits: usize,
}

struct HostTelemetry {
    path: std::path::PathBuf,
    replayed: bool,
}
impl HostTelemetry {
    fn create(case_id: &str) -> Self {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/qualification-telemetry");
        std::fs::create_dir_all(&directory).unwrap();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = directory.join(format!("{case_id}-{nonce}.jsonl"));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        sandbox_grant_restricted_qualification_write(&path).unwrap();
        Self { path, replayed: false }
    }

    fn replay(&mut self, case_id: &str, host_pid: u32, require_complete: bool) -> String {
        let telemetry = std::fs::read_to_string(&self.path).unwrap();
        assert!(telemetry.len() <= 64 * 1024, "host telemetry exceeded its bound");
        for line in telemetry.lines() {
            eprintln!("{line}");
        }
        let _ = std::fs::remove_file(&self.path);
        self.replayed = true;
        let lines: Vec<_> = telemetry.lines().collect();
        assert!(!lines.is_empty(), "host emitted no qualification telemetry");
        let case = format!("\"case_id\":\"{case_id}\"");
        let pid = format!("\"host_pid\":{host_pid}");
        assert!(lines.iter().all(|line| {
            line.starts_with('{')
                && line.ends_with('}')
                && line.contains("\"schema\":\"bareline.first-party-runtime.v1\"")
                && line.contains(&case)
                && line.contains(&pid)
        }));
        assert!(telemetry.contains("\"event\":\"phase_start\""));
        assert!(telemetry.contains("\"event\":\"phase\""));
        assert!(telemetry.contains("\"event\":\"component\""));
        if require_complete {
            for phase in [
                "authentication",
                "hash_read",
                "runtime_init",
                "prepare",
                "compile",
                "instantiate",
                "first_broker",
                "response",
                "execute",
            ] {
                assert!(
                    telemetry.contains(&format!("\"phase\":\"{phase}\"")),
                    "host telemetry omitted {phase}: {telemetry}"
                );
            }
        }
        telemetry
    }
}
impl Drop for HostTelemetry {
    fn drop(&mut self) {
        if self.replayed {
            return;
        }
        if let Ok(telemetry) = std::fs::read_to_string(&self.path) {
            for line in telemetry.lines() {
                eprintln!("{line}");
            }
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn configure_qualification_process(
    process: &mut Command,
    executable: &std::path::Path,
    case_id: &str,
    telemetry_path: &std::path::Path,
) {
    process.env_clear();
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    process
        .env("SystemRoot", &system_root)
        .env("windir", &system_root)
        .env("Path", std::path::Path::new(&system_root).join("System32"))
        .env("PATHEXT", ".COM;.EXE;.BAT;.CMD")
        .env("NoDefaultCurrentDirectoryInExePath", "1")
        .env("BARELINE_QUALIFICATION_TELEMETRY", "1")
        .env("BARELINE_QUALIFICATION_CASE_ID", case_id)
        .env("BARELINE_QUALIFICATION_TELEMETRY_PATH", telemetry_path);
    for name in ["SystemDrive", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE"] {
        if let Some(value) = std::env::var_os(name) {
            process.env(name, value);
        }
    }
    if let Some(runtime) = executable.parent() {
        process.current_dir(runtime);
    }
}

fn selected_host() -> std::path::PathBuf {
    std::env::var_os("BARELINE_T10_HOST_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_BIN_EXE_bareline-extension-host")))
}

fn selected_component(crate_name: &str) -> std::path::PathBuf {
    let variable = match crate_name {
        "json_tools" => "BARELINE_T10_JSON_COMPONENT_PATH",
        "xml_tools" => "BARELINE_T10_XML_COMPONENT_PATH",
        "hex_view" => "BARELINE_T10_HEX_COMPONENT_PATH",
        _ => panic!("unknown first-party component {crate_name}"),
    };
    std::env::var_os(variable)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-wasip2/release")
                .join(format!("bareline_{crate_name}.wasm"))
        })
}

fn component_case(
    crate_name: &str,
    command: &str,
    arguments: &str,
    text_length: u64,
    raw_length: u64,
    budget: ExecutionBudget,
    mut broker: impl FnMut(Envelope) -> Result<BrokerValue, String>,
) -> CaseResult {
    let file = selected_component(crate_name);
    let hash_read_started = Instant::now();
    let component = std::fs::read(&file).expect("build actual components first");
    assert!(component.starts_with(b"\0asm\x0d\0\x01\0"));
    let component_hash = format!("{:x}", Sha256::digest(&component));
    let hash_read = hash_read_started.elapsed();
    let case_id = qualification_case_id(command);
    let mut telemetry = HostTelemetry::create(&case_id);
    let server = PipeServer::create().unwrap();
    let executable = selected_host();
    let mut process = Command::new(&executable);
    process
        .args([server.name(), &std::process::id().to_string()])
        .arg(&file)
        .arg(&component_hash)
        .arg(match budget {
            ExecutionBudget::Interactive => "interactive",
            ExecutionBudget::Background => "background",
        })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    configure_qualification_process(&mut process, &executable, &case_id, &telemetry.path);
    let started = Instant::now();
    let (child, mut job) = SandboxedProcessLauncher::default()
        .spawn_host(&mut process, &[executable.as_path(), file.as_path()])
        .unwrap();
    let host_pid = child.id();
    let restricted = child.is_restricted();
    let child = std::cell::RefCell::new(child);
    let mut diagnostics = CaseDiagnostics {
        case_id: &case_id,
        host_pid,
        command,
        child: &child,
        started,
        complete: false,
        requests: 0,
        requested_text_bytes: 0,
        requested_original_bytes: 0,
        last_text_end: 0,
        completed_replies: 0,
        completed_response_bytes: 0,
        read_wall: Duration::ZERO,
        encode_wall: Duration::ZERO,
        write_wall: Duration::ZERO,
    };
    let mut pipe = server.accept(child.borrow().id(), Duration::from_secs(5)).unwrap();
    let authentication = started.elapsed();
    let parent_deadline = Duration::from_millis(budget.timeout_ms()) + Duration::from_secs(2);
    pipe.set_timeout(parent_deadline);
    let invocation = Invocation {
        extension_id: format!("org.bareline.{}", crate_name.replace('_', "-")),
        command: command.into(),
        arguments: arguments.into(),
        document: 11,
        revision: 23,
        source_generation: 37,
        text_length,
        raw_length,
        grant_generation: 41,
    };
    let bytes = encode(&invocation).unwrap();
    pipe.write_all(&(bytes.len() as u32).to_le_bytes()).unwrap();
    pipe.write_all(&bytes).unwrap();
    let mut panel = None;
    let mut result_was_panel = false;
    let mut staged = Vec::new();
    let mut transaction_open = false;
    let mut committed_edits = 0usize;
    let execute_started = Instant::now();
    let mut last_response_completed = execute_started;
    let mut first_broker = None;
    loop {
        let read_started = Instant::now();
        let received = read_frame(&mut pipe);
        diagnostics.read_wall += read_started.elapsed();
        let message = match received {
            Ok(message) => message,
            Err(ProtocolError::Io) => break,
            Err(error) => panic!("malformed child frame: {error:?}"),
        };
        diagnostics.requests += 1;
        first_broker.get_or_insert_with(|| execute_started.elapsed());
        if let Request::ReadTextRange { range, .. } = &message.request {
            diagnostics.requested_text_bytes += range.end.saturating_sub(range.start);
            diagnostics.last_text_end = range.end;
        }
        if let Request::ReadOriginalBytes { range, .. } = &message.request {
            diagnostics.requested_original_bytes += range.end.saturating_sub(range.start);
        }
        assert_eq!(message.extension_id, invocation.extension_id);
        assert_eq!(message.context.grant_generation, invocation.grant_generation);
        let result = match &message.request {
            Request::Panel { text, .. } => {
                assert_eq!(message.context.capability, Capability::UiPanel);
                assert_eq!(message.context.scope, Scope::Extension);
                assert!(text.len() <= 1024 * 1024, "guest bypassed the panel output limit");
                assert!(panel.replace(text.clone()).is_none(), "expected exactly one result");
                result_was_panel = true;
                Ok(BrokerValue::Acknowledged)
            }
            Request::BeginEdits { document, revision } => {
                assert_eq!(message.context.capability, Capability::DocumentEdit);
                assert_eq!(message.context.scope, Scope::Document(11));
                assert_eq!((*document, *revision), (11, 23));
                assert!(!transaction_open, "one invocation opened multiple edit transactions");
                transaction_open = true;
                Ok(BrokerValue::Transaction(9))
            }
            Request::AppendChunk { transaction, chunk } => {
                assert_eq!(message.context.capability, Capability::DocumentEdit);
                assert_eq!(message.context.scope, Scope::Document(11));
                assert_eq!(*transaction, 9);
                assert!(transaction_open);
                assert!(chunk.len() <= MAX_CHUNK_BYTES, "guest exceeded the edit chunk limit");
                let aggregate = staged
                    .len()
                    .checked_add(chunk.len())
                    .expect("staged edit size overflow");
                assert!(
                    aggregate <= MAX_STAGED_EDIT_BYTES,
                    "guest exceeded the aggregate staged-edit limit"
                );
                staged.extend(chunk);
                Ok(BrokerValue::Acknowledged)
            }
            Request::CommitEdits { transaction, revision } => {
                assert_eq!(message.context.capability, Capability::DocumentEdit);
                assert_eq!(message.context.scope, Scope::Document(11));
                assert_eq!((*transaction, *revision), (9, 23));
                assert!(transaction_open);
                let edits: Vec<TextEdit> = decode(&staged).unwrap();
                assert_eq!(edits.len(), 1, "formatting must remain one undo transaction");
                assert_eq!(
                    edits[0].range,
                    TextRange {
                        start: 0,
                        end: text_length
                    }
                );
                assert!(panel.replace(edits[0].replacement.clone()).is_none());
                transaction_open = false;
                committed_edits += edits.len();
                Ok(BrokerValue::Applied { revision: 24 })
            }
            _ => broker(message.clone()),
        };
        let encode_started = Instant::now();
        let encoded = encode(&BrokerResponse {
            request_id: message.request_id,
            result,
        });
        diagnostics.encode_wall += encode_started.elapsed();
        let bytes = encoded.unwrap();
        assert!(
            bytes.len() <= MAX_CHUNK_BYTES,
            "broker response exceeded the host reply limit"
        );
        let write_started = Instant::now();
        let written = pipe
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .and_then(|_| pipe.write_all(&bytes));
        diagnostics.write_wall += write_started.elapsed();
        written.unwrap();
        last_response_completed = Instant::now();
        diagnostics.completed_replies += 1;
        diagnostics.completed_response_bytes += bytes.len() as u64;
    }
    assert!(!transaction_open, "component exited with an open edit transaction");
    let execution = execute_started.elapsed();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.borrow_mut().try_wait().unwrap() {
            assert!(status.success(), "component process: {status}");
            break;
        }
        if Instant::now() >= deadline {
            job.terminate().unwrap();
            panic!("component did not exit");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let shutdown = last_response_completed.elapsed();
    telemetry.replay(&case_id, host_pid, true);
    let panel = panel.expect("component result panel");
    if result_was_panel {
        assert!(panel.len() <= 1024 * 1024, "component result exceeded the panel limit");
    }
    diagnostics.complete = true;
    let elapsed = started.elapsed();
    eprintln!(
        "{{\"schema\":\"bareline.first-party-runtime.v1\",\"event\":\"case\",\"case_id\":\"{case_id}\",\"host_pid\":{host_pid},\"command\":\"{command}\",\"profile\":\"{}\",\"sandbox\":\"{}\",\"sha256\":\"{component_hash}\",\"component_bytes\":{},\"text_bytes\":{text_length},\"original_bytes\":{raw_length},\"output_bytes\":{},\"host_deadline_ms\":{},\"parent_deadline_ms\":{},\"hash_read_us\":{},\"authentication_us\":{},\"first_broker_us\":{},\"execute_us\":{},\"parent_response_write_us\":{},\"shutdown_us\":{},\"requests\":{},\"requested_text_bytes\":{},\"requested_original_bytes\":{},\"response_bytes\":{}}}",
        if cfg!(debug_assertions) { "debug" } else { "release" },
        if restricted {
            "restricted_token"
        } else {
            "job_limited_fallback"
        },
        component.len(),
        panel.len(),
        budget.timeout_ms(),
        parent_deadline.as_millis(),
        hash_read.as_micros(),
        authentication.as_micros(),
        first_broker.unwrap_or(execution).as_micros(),
        execution.as_micros(),
        diagnostics.write_wall.as_micros(),
        shutdown.as_micros(),
        diagnostics.requests,
        diagnostics.requested_text_bytes,
        diagnostics.requested_original_bytes,
        diagnostics.completed_response_bytes,
    );
    CaseResult {
        output: panel,
        elapsed,
        requests: diagnostics.requests,
        requested_text_bytes: diagnostics.requested_text_bytes,
        requested_original_bytes: diagnostics.requested_original_bytes,
        committed_edits,
    }
}

fn text_case(crate_name: &str, command: &str, arguments: &str, input: &[u8]) -> CaseResult {
    component_case(
        crate_name,
        command,
        arguments,
        input.len() as u64,
        0,
        ExecutionBudget::Interactive,
        |message| {
            assert_eq!(message.context.capability, Capability::DocumentRead);
            assert_eq!(message.context.scope, Scope::Document(11));
            let Request::ReadTextRange {
                document,
                revision,
                range,
            } = message.request
            else {
                panic!("{command} requested unexpected broker operation");
            };
            assert_eq!((document, revision), (11, 23));
            assert!(range.start <= range.end && range.end <= input.len() as u64);
            Ok(BrokerValue::Bytes(
                input[range.start as usize..range.end as usize].to_vec(),
            ))
        },
    )
}

// The artifacts are an external precondition, so the workspace test remains ignored;
// scripts/test-first-party.ps1 selects this exact fast release-profile gate in CI.
#[test]
#[ignore = "requires current wasm32-wasip2 release artifacts; use scripts/test-first-party.ps1 -Suite Fast"]
fn fast_release_components_cover_semantics_permissions_and_provenance() {
    assert!(
        !cfg!(debug_assertions),
        "first-party qualification requires the release host profile"
    );

    let valid = text_case(
        "json_tools",
        "ext.json.validate",
        "",
        r#"{"number":12345678901234567890,"text":"é🦀"}"#.as_bytes(),
    );
    assert_eq!(valid.output, "Valid JSON");
    assert!(valid.requests > 0);

    let minified = text_case("json_tools", "ext.json.minify", "", b"{ \"n\" : 12345678901234567890 }");
    assert_eq!(minified.output, "{\"n\":12345678901234567890}");
    assert_eq!(minified.committed_edits, 1);

    let formatted = text_case(
        "json_tools",
        "ext.json.format",
        "",
        r#"{"number":12345678901234567890,"text":"é🦀"}"#.as_bytes(),
    );
    assert!(formatted.output.contains("12345678901234567890"));
    assert!(formatted.output.contains("é🦀"));
    assert_eq!(formatted.committed_edits, 1);

    let tree = text_case("json_tools", "ext.json.tree", "", br#"{"a":[1,2]}"#);
    assert!(tree.output.contains("object at TextOffset 0"));
    assert!(tree.requested_text_bytes <= br#"{"a":[1,2]}"#.len() as u64);

    let xml_valid = text_case("xml_tools", "ext.xml.validate", "", b"<r><n/></r>");
    assert_eq!(xml_valid.output, "Valid XML; external resolution disabled");
    let preserved = b"<r xml:space='preserve'><a/> <b/></r>";
    let xml_preserved = text_case("xml_tools", "ext.xml.format", "", preserved);
    assert_eq!(xml_preserved.output.as_bytes(), preserved);
    assert_eq!(xml_preserved.committed_edits, 1);

    let xml_source = b"<?xml version='1.0' encoding='UTF-16'?><r xml:space='preserve'><n>\xc3\xa9</n> <b/></r>";
    let xml_format = text_case("xml_tools", "ext.xml.format", "", xml_source);
    assert!(xml_format.output.contains("encoding='UTF-16'"));
    assert!(xml_format.output.contains("xml:space='preserve'"));
    assert_eq!(xml_format.committed_edits, 1);

    let xml = b"<r xmlns:a='urn:x' id='root'><a:n id='one'>first</a:n><a:n id='two'>second</a:n></r>";
    for (arguments, expected) in [
        ("/r/a:n[2]/text()\na=urn:x", "second"),
        ("/r//@id", "root\none\ntwo"),
        ("/r/@missing", ""),
    ] {
        assert_eq!(text_case("xml_tools", "ext.xml.xpath", arguments, xml).output, expected);
    }
    let xml_error = text_case("xml_tools", "ext.xml.validate", "", b"<r><unclosed></r>");
    assert!(xml_error.output.contains("TextOffset"));
    let external = text_case(
        "xml_tools",
        "ext.xml.validate",
        "",
        b"<!DOCTYPE r SYSTEM 'https://example.invalid/forbidden'><r/>",
    );
    assert!(external.output.contains("DTD and external entities are disabled"));

    let original = [0xff, 0xfe, 0x41, 0x00];
    let hex = component_case(
        "hex_view",
        "ext.hex.goto",
        "offset=0\nrows=1",
        999,
        original.len() as u64,
        ExecutionBudget::Interactive,
        |message| {
            assert_eq!(message.context.capability, Capability::DocumentRead);
            let Request::ReadOriginalBytes {
                document,
                generation,
                range,
            } = message.request
            else {
                panic!("Hex must navigate original bytes rather than decoded dirty text");
            };
            assert_eq!((document, generation), (11, 37));
            assert_eq!((range.start, range.end), (0, 4));
            Ok(BrokerValue::Bytes(original.to_vec()))
        },
    );
    assert!(hex.output.contains("FF FE 41 00"));
    assert!(hex.output.contains("disk generation 37"));
    assert!(hex.output.contains("Unsaved text edits are excluded"));
    assert_eq!(hex.requested_original_bytes, 4);
    let unavailable = component_case(
        "hex_view",
        "ext.hex.goto",
        "offset=0\nrows=1",
        999,
        original.len() as u64,
        ExecutionBudget::Interactive,
        |message| {
            assert!(matches!(message.request, Request::ReadOriginalBytes { .. }));
            Err("original source generation unavailable".into())
        },
    );
    assert!(unavailable.output.contains("?? ?? ?? ??"));
    assert!(unavailable.output.contains("disk generation 37"));
    assert!(unavailable.output.contains("Unsaved text edits are excluded"));

    // This is a simulated broker denial through the actual guest/host transport.
    // Production authorization policy is covered by extensions-protocol broker tests.
    let mut denial_observed = false;
    let denied = component_case(
        "json_tools",
        "ext.json.validate",
        "",
        2,
        0,
        ExecutionBudget::Interactive,
        |message| {
            assert_eq!(message.context.capability, Capability::DocumentRead);
            assert_eq!(message.context.scope, Scope::Document(11));
            assert!(matches!(message.request, Request::ReadTextRange { .. }));
            denial_observed = true;
            Err("permission denied by qualification broker".into())
        },
    );
    assert!(denial_observed);
    assert!(denied.output.contains("permission denied by qualification broker"));
}

fn interrupt_actual_component(cancel: bool) -> String {
    let file = selected_component("json_tools");
    let component = std::fs::read(&file).expect("build actual components first");
    let executable = selected_host();
    let case_id = qualification_case_id(if cancel { "cancel" } else { "timeout" });
    let mut telemetry = HostTelemetry::create(&case_id);
    let server = PipeServer::create().unwrap();
    let mut process = Command::new(&executable);
    process
        .args([server.name(), &std::process::id().to_string()])
        .arg(&file)
        .arg(format!("{:x}", Sha256::digest(&component)))
        .arg("interactive")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    configure_qualification_process(&mut process, &executable, &case_id, &telemetry.path);
    let (mut child, mut job) = SandboxedProcessLauncher::default()
        .spawn_host(&mut process, &[executable.as_path(), file.as_path()])
        .unwrap();
    let host_pid = child.id();
    let sandbox = if child.is_restricted() {
        "restricted_token"
    } else {
        "job_limited_fallback"
    };
    let mut pipe = server.accept(child.id(), Duration::from_secs(5)).unwrap();
    pipe.set_timeout(Duration::from_secs(7));
    let invocation = Invocation {
        extension_id: "org.bareline.json-tools".into(),
        command: "ext.json.validate".into(),
        arguments: String::new(),
        document: 11,
        revision: 23,
        source_generation: 37,
        text_length: 2,
        raw_length: 0,
        grant_generation: 41,
    };
    let bytes = encode(&invocation).unwrap();
    pipe.write_all(&(bytes.len() as u32).to_le_bytes()).unwrap();
    pipe.write_all(&bytes).unwrap();
    let request = read_frame(&mut pipe).expect("actual component must reach its first broker request");
    assert!(matches!(request.request, Request::ReadTextRange { .. }));
    if cancel {
        job.terminate().unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            job.terminate().unwrap();
            panic!("interrupted qualification component did not stop");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let telemetry = telemetry.replay(&case_id, host_pid, false);
    assert!(!status.success());
    eprintln!(
        "{{\"schema\":\"bareline.first-party-runtime.v1\",\"event\":\"interruption\",\"case_id\":\"{case_id}\",\"host_pid\":{host_pid},\"kind\":\"{}\",\"sandbox\":\"{sandbox}\",\"host_deadline_ms\":{},\"outcome\":\"{status}\"}}",
        if cancel { "cancel" } else { "timeout" },
        ExecutionBudget::Interactive.timeout_ms()
    );
    assert!(telemetry.contains("\"phase\":\"first_broker\""));
    assert!(
        telemetry
            .lines()
            .next_back()
            .is_some_and(|line| line.contains("\"event\":\"phase_start\"") && line.contains("\"phase\":\"response\"")),
        "interrupted host must retain its last started phase: {telemetry}"
    );
    status.to_string()
}

#[test]
#[ignore = "requires current wasm32-wasip2 release artifacts; use scripts/test-first-party.ps1 -Suite Fast"]
fn fast_release_components_cancel_and_timeout_without_poisoning_the_next_launch() {
    assert!(
        !cfg!(debug_assertions),
        "first-party qualification requires the release host profile"
    );
    let cancelled = interrupt_actual_component(true);
    assert!(!cancelled.is_empty());
    let timed_out = interrupt_actual_component(false);
    assert!(
        timed_out.contains("124"),
        "watchdog deadline must remain explicit: {timed_out}"
    );
    assert_eq!(
        text_case("json_tools", "ext.json.validate", "", b"{}").output,
        "Valid JSON"
    );
}

#[test]
#[ignore = "requires current wasm32-wasip2 release artifacts; use scripts/test-first-party.ps1 -Suite Fast"]
fn fast_release_components_run_under_controlled_concurrency() {
    assert!(
        !cfg!(debug_assertions),
        "first-party qualification requires the release host profile"
    );
    std::thread::scope(|scope| {
        let json = scope.spawn(|| text_case("json_tools", "ext.json.validate", "", b"{\"n\":1}"));
        let xml = scope.spawn(|| text_case("xml_tools", "ext.xml.xpath", "/r/n/text()", b"<r><n>x</n></r>"));
        assert_eq!(json.join().unwrap().output, "Valid JSON");
        assert_eq!(xml.join().unwrap().output, "x");
    });
}

const GIB_JSON_LENGTH: u64 = 1 << 30;
fn generated_json_range(start: u64, end: u64) -> Vec<u8> {
    assert!(start <= end && end <= GIB_JSON_LENGTH && end - start <= 65536);
    let mut bytes = vec![b'a'; (end - start) as usize];
    for (index, byte) in b"{\"data\":\"".iter().enumerate() {
        let position = index as u64;
        if (start..end).contains(&position) {
            bytes[(position - start) as usize] = *byte;
        }
    }
    for (position, byte) in [(GIB_JSON_LENGTH - 2, b'"'), (GIB_JSON_LENGTH - 1, b'}')] {
        if (start..end).contains(&position) {
            bytes[(position - start) as usize] = byte;
        }
    }
    bytes
}

#[test]
#[ignore = "coordinated release component gate; actual generated 1 GiB background workload"]
fn generated_one_gib_json_validates_and_tree_reads_only_one_page() {
    let mut next = 0u64;
    let mut requests = 0u64;
    let result = component_case(
        "json_tools",
        "ext.json.validate",
        "",
        GIB_JSON_LENGTH,
        0,
        ExecutionBudget::Background,
        |message| {
            assert_eq!(message.context.capability, Capability::DocumentRead);
            let Request::ReadTextRange {
                document,
                revision,
                range,
            } = message.request
            else {
                panic!("validation requested non-read capability");
            };
            assert_eq!((document, revision), (11, 23));
            assert_eq!(range.start, next);
            let bytes = generated_json_range(range.start, range.end);
            next = range.end;
            requests += 1;
            Ok(BrokerValue::Bytes(bytes))
        },
    );
    assert_eq!(result.output, "Valid JSON");
    assert_eq!(next, GIB_JSON_LENGTH);
    assert_eq!(requests, GIB_JSON_LENGTH / 65536);
    assert!(result.elapsed < Duration::from_millis(ExecutionBudget::Background.timeout_ms()) + Duration::from_secs(4));
    eprintln!(
        "PR-T10 generated JSON: bytes={next}, requests={requests}, max_chunk=65536, synthetic_source=true, elapsed_ms={}",
        result.elapsed.as_millis()
    );
    let mut read = 0;
    let result = component_case(
        "json_tools",
        "ext.json.tree",
        "",
        GIB_JSON_LENGTH,
        0,
        ExecutionBudget::Interactive,
        |message| {
            let Request::ReadTextRange { range, .. } = message.request else {
                panic!("tree requested non-read operation");
            };
            read += range.end - range.start;
            Ok(BrokerValue::Bytes(generated_json_range(range.start, range.end)))
        },
    );
    assert_eq!(read, 65536);
    assert!(result.output.contains("object at TextOffset 0"));
    assert!(result.output.contains("Next page"));
    assert!(result.output.contains("mode=2"));
}

#[test]
#[ignore = "coordinated release component gate; actual bounded 5 GiB Hex source"]
fn hex_five_gib_goto_reads_only_visible_original_range() {
    let length = 5u64 * 1024 * 1024 * 1024;
    let requested_offset = 4u64 * 1024 * 1024 * 1024 + 3;
    let aligned = requested_offset / 16 * 16;
    let mut calls = 0;
    let mut bytes_read = 0;
    let result = component_case(
        "hex_view",
        "ext.hex.goto",
        &format!("offset={requested_offset}\nrows=32"),
        7,
        length,
        ExecutionBudget::Interactive,
        |message| {
            assert_eq!(message.context.capability, Capability::DocumentRead);
            assert_eq!(message.context.scope, Scope::Document(11));
            let Request::ReadOriginalBytes {
                document,
                generation,
                range,
            } = message.request
            else {
                panic!("large Hex must request only original bytes");
            };
            assert_eq!((document, generation), (11, 37));
            assert_eq!((range.start, range.end), (aligned - 32, aligned + 34 * 16));
            assert!(range.end - range.start <= 36 * 16);
            calls += 1;
            bytes_read += range.end - range.start;
            Ok(BrokerValue::Bytes(
                (range.start..range.end).map(|offset| (offset & 255) as u8).collect(),
            ))
        },
    );
    assert_eq!(calls, 1);
    assert_eq!(bytes_read, 36 * 16);
    assert!(result.output.contains(&format!("{aligned:016X}")));
    assert!(result.output.contains("disk generation 37"));
    assert!(result.output.contains("Unsaved text edits are excluded"));
    assert!(result.output.contains("00 01 02 03 04 05 06 07"));
    eprintln!(
        "PR-T10 synthetic Hex: source_bytes={length}, requested_offset={requested_offset}, broker_calls={calls}, broker_bytes={bytes_read}, synthetic_source=true"
    );
}
