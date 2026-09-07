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
