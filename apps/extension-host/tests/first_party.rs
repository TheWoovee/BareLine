// SPDX-License-Identifier: MPL-2.0
#![cfg(windows)]
use bareline_extensions_protocol::*;
use bareline_macros::process::ProcessLauncher;
use bareline_platform_windows::{WindowsProcessLauncher, extension_transport::PipeServer};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

// Run explicitly after scripts/test-first-party.ps1 builds actual WASI components.
#[test]
#[ignore = "requires wasm32-wasip2 release artifacts; use scripts/test-first-party.ps1"]
fn first_party_components_cross_authenticated_process_boundary() {
    for (crate_name, command, input, expected) in [
        (
            "json_tools",
            "ext.json.validate",
            b"{\"n\":12345678901234567890}".as_slice(),
            "Valid JSON",
        ),
        (
            "json_tools",
            "ext.json.minify",
            b"{ \"n\" : 12345678901234567890 }".as_slice(),
            "12345678901234567890",
        ),
        (
            "xml_tools",
            "ext.xml.validate",
            b"<r><n/></r>".as_slice(),
            "Valid XML",
        ),
        (
            "xml_tools",
            "ext.xml.format",
            b"<r xml:space='preserve'><a/> <b/></r>".as_slice(),
            "<r xml:space='preserve'><a/> <b/></r>",
        ),
        (
            "hex_view",
            "ext.hex.open",
            b"\xff\xfeA\0".as_slice(),
            "FF FE 41 00",
        ),
    ] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/wasm32-wasip2/release");
        let file = root.join(format!("bareline_{crate_name}.wasm"));
        let component = std::fs::read(&file).expect("build actual components first");
        assert_eq!(
            &component[..8],
            b"\0asm\x0d\0\x01\0",
            "must be a component, not a core module"
        );
        let server = PipeServer::create().unwrap();
        let mut process = Command::new(env!("CARGO_BIN_EXE_bareline-extension-host"));
        process
            .args([
                server.name(),
                &server.nonce_hex(),
                &std::process::id().to_string(),
            ])
            .arg(&file)
            .arg(format!("{:x}", Sha256::digest(&component)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let (mut child, mut job) = WindowsProcessLauncher.spawn(&mut process).unwrap();
        let mut pipe = server.accept(child.id(), Duration::from_secs(5)).unwrap();
        let context = Invocation {
            extension_id: format!("org.bareline.{}", crate_name.replace('_', "-")),
            command: command.into(),
            arguments: String::new(),
            document: 1,
            revision: 2,
            source_generation: 3,
            text_length: input.len() as u64,
            raw_length: input.len() as u64,
            grant_generation: 4,
        };
        let bytes = encode(&context).unwrap();
        pipe.write_all(&(bytes.len() as u32).to_le_bytes()).unwrap();
        pipe.write_all(&bytes).unwrap();
        let mut panel = None;
        let mut staged = Vec::new();
        while let Ok(message) = read_frame(&mut pipe) {
            assert_eq!(message.extension_id, context.extension_id);
            assert_eq!(message.context.grant_generation, 4);
            let result = match message.request {
                Request::ReadTextRange {
                    document,
                    revision,
                    range,
                } => {
                    assert_eq!((document, revision), (1, 2));
                    assert_ne!(crate_name, "hex_view");
                    BrokerValue::Bytes(input[range.start as usize..range.end as usize].to_vec())
                }
                Request::ReadOriginalBytes {
                    document,
                    generation,
                    range,
                } => {
                    assert_eq!((document, generation), (1, 3));
                    assert_eq!(crate_name, "hex_view");
                    BrokerValue::Bytes(input[range.start as usize..range.end as usize].to_vec())
                }
                Request::BeginEdits { document, revision } => {
                    assert_eq!((document, revision), (1, 2));
                    BrokerValue::Transaction(9)
                }
                Request::AppendChunk { transaction, chunk } => {
                    assert_eq!(transaction, 9);
                    staged.extend(chunk);
                    BrokerValue::Acknowledged
                }
                Request::CommitEdits {
                    transaction,
                    revision,
                } => {
                    assert_eq!((transaction, revision), (9, 2));
                    let edits: Vec<TextEdit> = decode(&staged).unwrap();
                    assert_eq!(edits.len(), 1);
                    assert_eq!(
                        edits[0].range,
                        TextRange {
                            start: 0,
                            end: input.len() as u64
                        }
                    );
                    panel = Some(edits[0].replacement.clone());
                    BrokerValue::Applied { revision: 3 }
                }
                Request::Panel { text, .. } => {
                    panel = Some(text);
                    BrokerValue::Acknowledged
                }
                other => panic!("unexpected request: {other:?}"),
            };
            let bytes = encode(&BrokerResponse {
                request_id: message.request_id,
                result: Ok(result),
            })
            .unwrap();
            pipe.write_all(&(bytes.len() as u32).to_le_bytes()).unwrap();
            pipe.write_all(&bytes).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "{crate_name}: {status}");
                break;
            }
            if Instant::now() >= deadline {
                job.terminate().unwrap();
                panic!("host did not terminate");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            panel
                .as_deref()
                .is_some_and(|text| if command == "ext.json.minify" {
                    text == "{\"n\":12345678901234567890}"
                } else if command == "ext.xml.format" {
                    text == expected
                } else {
                    text.contains(expected)
                }),
            "{crate_name}: {panel:?}"
        );
    }
}

/// Fixture launch remains below the production signature gate and uses only
/// locally built components. Every request crosses the real authenticated pipe.
struct CaseDiagnostics<'a> {
    command: &'a str,
    child: &'a std::cell::RefCell<std::process::Child>,
    started: Instant,
    complete: bool,
    requests: u64,
    requested_text_bytes: u64,
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
            Ok(mut child) => format!("{:?}", child.try_wait()),
            Err(_) => "child status borrowed during failure".into(),
        };
        eprintln!(
            "PR027 failure diagnostic: command={} elapsed_ms={} requests={} requested_text_bytes={} last_text_end={} completed_replies={} completed_response_bytes={} read_wall_ms={} encode_wall_ms={} write_wall_ms={} child_status={}",
            self.command,
            self.started.elapsed().as_millis(),
            self.requests,
            self.requested_text_bytes,
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
fn component_case(
    crate_name: &str,
    command: &str,
    arguments: &str,
    text_length: u64,
    raw_length: u64,
    budget: ExecutionBudget,
    mut broker: impl FnMut(Envelope) -> Result<BrokerValue, String>,
) -> (String, Duration) {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-wasip2/release")
        .join(format!("bareline_{crate_name}.wasm"));
    let component = std::fs::read(&file).expect("build actual components first");
    assert!(component.starts_with(b"\0asm\x0d\0\x01\0"));
    let server = PipeServer::create().unwrap();
    let mut process = Command::new(env!("CARGO_BIN_EXE_bareline-extension-host"));
    process
        .args([
            server.name(),
            &server.nonce_hex(),
            &std::process::id().to_string(),
        ])
        .arg(&file)
        .arg(format!("{:x}", Sha256::digest(&component)))
        .arg(match budget {
            ExecutionBudget::Interactive => "interactive",
            ExecutionBudget::Background => "background",
        })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let started = Instant::now();
    let (child, mut job) = WindowsProcessLauncher.spawn(&mut process).unwrap();
    let child = std::cell::RefCell::new(child);
    let mut diagnostics = CaseDiagnostics {
        command,
        child: &child,
        started,
        complete: false,
        requests: 0,
        requested_text_bytes: 0,
        last_text_end: 0,
        completed_replies: 0,
        completed_response_bytes: 0,
        read_wall: Duration::ZERO,
        encode_wall: Duration::ZERO,
        write_wall: Duration::ZERO,
    };
    let mut pipe = server
        .accept(child.borrow().id(), Duration::from_secs(5))
        .unwrap();
    pipe.set_timeout(Duration::from_millis(budget.timeout_ms()) + Duration::from_secs(2));
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
        if let Request::ReadTextRange { range, .. } = &message.request {
            diagnostics.requested_text_bytes += range.end.saturating_sub(range.start);
            diagnostics.last_text_end = range.end;
        }
        assert_eq!(message.extension_id, invocation.extension_id);
        assert_eq!(
            message.context.grant_generation,
            invocation.grant_generation
        );
        let result = if let Request::Panel { ref text, .. } = message.request {
            assert_eq!(message.context.capability, Capability::UiPanel);
            assert!(
                panel.replace(text.clone()).is_none(),
                "expected exactly one result panel"
            );
            Ok(BrokerValue::Acknowledged)
        } else {
            broker(message.clone())
        };
        let encode_started = Instant::now();
        let encoded = encode(&BrokerResponse {
            request_id: message.request_id,
            result,
        });
        diagnostics.encode_wall += encode_started.elapsed();
        let bytes = encoded.unwrap();
        let write_started = Instant::now();
        let written = pipe
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .and_then(|_| pipe.write_all(&bytes));
        diagnostics.write_wall += write_started.elapsed();
        written.unwrap();
        diagnostics.completed_replies += 1;
        diagnostics.completed_response_bytes += bytes.len() as u64;
    }
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
    let panel = panel.expect("component result panel");
    diagnostics.complete = true;
    (panel, started.elapsed())
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
    let (panel, elapsed) = component_case(
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
    assert_eq!(panel, "Valid JSON");
    assert_eq!(next, GIB_JSON_LENGTH);
    assert_eq!(requests, GIB_JSON_LENGTH / 65536);
    assert!(
        elapsed
            < Duration::from_millis(ExecutionBudget::Background.timeout_ms())
                + Duration::from_secs(4)
    );
    eprintln!(
        "PR027 generated JSON: bytes={next}, requests={requests}, max_chunk=65536, elapsed_ms={}",
        elapsed.as_millis()
    );
    let mut read = 0;
    let (panel, _) = component_case(
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
            Ok(BrokerValue::Bytes(generated_json_range(
                range.start,
                range.end,
            )))
        },
    );
    assert_eq!(read, 65536);
    assert!(panel.contains("object at TextOffset 0"));
    assert!(panel.contains("Next page"));
    assert!(panel.contains("mode=2"));
}

#[test]
#[ignore = "coordinated release component gate; requires actual WASI artifacts"]
fn xml_xpath_security_and_hex_original_generation_cross_the_host() {
    let xml =
        b"<r xmlns:a='urn:x' id='root'><a:n id='one'>first</a:n><a:n id='two'>second</a:n></r>";
    for (arguments, expected) in [
        ("/r/a:n[2]/text()\na=urn:x", "second"),
        ("/r//@id", "root\none\ntwo"),
        ("/r/@missing", ""),
    ] {
        let (panel, _) = component_case(
            "xml_tools",
            "ext.xml.xpath",
            arguments,
            xml.len() as u64,
            0,
            ExecutionBudget::Interactive,
            |message| {
                let Request::ReadTextRange {
                    document,
                    revision,
                    range,
                } = message.request
                else {
                    panic!("XPath requested non-read operation");
                };
                assert_eq!((document, revision), (11, 23));
                Ok(BrokerValue::Bytes(
                    xml[range.start as usize..range.end as usize].to_vec(),
                ))
            },
        );
        assert_eq!(panel, expected);
    }
    let xml = b"<!DOCTYPE r SYSTEM 'https://example.invalid/forbidden'><r/>";
    let (panel, _) = component_case(
        "xml_tools",
        "ext.xml.validate",
        "",
        xml.len() as u64,
        0,
        ExecutionBudget::Interactive,
        |message| {
            let Request::ReadTextRange { range, .. } = message.request else {
                panic!("external XML attempted non-read operation");
            };
            Ok(BrokerValue::Bytes(
                xml[range.start as usize..range.end as usize].to_vec(),
            ))
        },
    );
    assert!(panel.contains("DTD and external entities are disabled"));
    for unavailable in [false, true] {
        let (panel, _) = component_case(
            "hex_view",
            "ext.hex.goto",
            "offset=0\nrows=1",
            999,
            4,
            ExecutionBudget::Interactive,
            |message| {
                let Request::ReadOriginalBytes {
                    document,
                    generation,
                    range,
                } = message.request
                else {
                    panic!("Hex must never read dirty decoded text");
                };
                assert_eq!((document, generation), (11, 37));
                assert_eq!((range.start, range.end), (0, 4));
                if unavailable {
                    Err("original source generation unavailable".into())
                } else {
                    Ok(BrokerValue::Bytes(vec![0xff, 0xfe, 0x41, 0]))
                }
            },
        );
        assert!(panel.contains("disk generation 37"));
        assert!(panel.contains("Unsaved text edits are excluded"));
        assert!(panel.contains(if unavailable {
            "?? ?? ?? ??"
        } else {
            "FF FE 41 00"
        }));
    }
}

#[test]
#[ignore = "coordinated release component gate; actual bounded 5 GiB Hex source"]
fn hex_five_gib_goto_reads_only_visible_original_range() {
    let length = 5u64 * 1024 * 1024 * 1024;
    let requested_offset = 4u64 * 1024 * 1024 * 1024 + 3;
    let aligned = requested_offset / 16 * 16;
    let mut calls = 0;
    let mut bytes_read = 0;
    let (panel, _) = component_case(
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
                (range.start..range.end)
                    .map(|offset| (offset & 255) as u8)
                    .collect(),
            ))
        },
    );
    assert_eq!(calls, 1);
    assert_eq!(bytes_read, 36 * 16);
    assert!(panel.contains(&format!("{aligned:016X}")));
    assert!(panel.contains("disk generation 37"));
    assert!(panel.contains("Unsaved text edits are excluded"));
    assert!(panel.contains("00 01 02 03 04 05 06 07"));
}
