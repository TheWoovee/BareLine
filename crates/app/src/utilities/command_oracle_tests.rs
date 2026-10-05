// SPDX-License-Identifier: MPL-2.0
//! Shared, exact command fixtures; these core tests do not qualify native routing.
use super::*;
use bareline_document::{Budget, Document};
use serde::Deserialize;

#[derive(Deserialize)]
struct Catalog {
    schema_version: u32,
    commands: Vec<Command>,
}

#[derive(Deserialize)]
struct Command {
    command: String,
    success: Vec<Success>,
    failures: Vec<Failure>,
}

#[derive(Deserialize)]
struct Success {
    id: String,
    before: String,
    range: [usize; 2],
    after: String,
    max_bytes: usize,
}

#[derive(Deserialize)]
struct Failure {
    id: String,
    before: String,
    range: [usize; 2],
    error: String,
    max_bytes: usize,
    #[serde(default)]
    cancelled: bool,
}

fn document(text: &str) -> Document {
    Document::from_utf8(text, Budget::new(65536), Budget::new(65536)).unwrap()
}

fn text(snapshot: &DocumentSnapshot) -> String {
    snapshot.read(TextOffset(0)..TextOffset(snapshot.len()), 65536).unwrap()
}

fn verify(command: &str, kind: Transform) {
    let catalog: Catalog =
        serde_json::from_str(include_str!("../../../../tests/e2e/utility_command_oracles.json")).unwrap();
    assert_eq!(catalog.schema_version, 1);
    let fixtures = catalog.commands.iter().find(|row| row.command == command).unwrap();
    assert!(!fixtures.success.is_empty());
    assert!(!fixtures.failures.is_empty());
    for case in &fixtures.success {
        let mut doc = document(&case.before);
        let before = doc.snapshot();
        let transaction = transform(
            &before,
            TextOffset(case.range[0])..TextOffset(case.range[1]),
            kind,
            case.max_bytes,
            &CancelToken::default(),
        )
        .unwrap();
        assert_eq!(transaction.edits.len(), 1, "{}", case.id);
        assert_eq!(text(&doc.snapshot()), case.before, "staging mutated {}", case.id);
        assert!(!doc.dirty());
        doc.apply(transaction).unwrap();
        assert_eq!(text(&doc.snapshot()), case.after, "{}", case.id);
        assert_eq!(text(&before), case.before, "snapshot changed {}", case.id);
        assert!(doc.dirty());
        doc.undo().unwrap();
        assert_eq!(text(&doc.snapshot()), case.before, "Undo {}", case.id);
        assert!(!doc.dirty());
        doc.redo().unwrap();
        assert_eq!(text(&doc.snapshot()), case.after, "Redo {}", case.id);
        assert!(doc.dirty());

        let mut raced = document(&case.before);
        let original = raced.snapshot();
        let stale = transform(
            &original,
            TextOffset(case.range[0])..TextOffset(case.range[1]),
            kind,
            case.max_bytes,
            &CancelToken::default(),
        )
        .unwrap();
        raced
            .apply(EditTransaction {
                base_revision: original.revision,
                edits: vec![Edit {
                    range: TextOffset(original.len())..TextOffset(original.len()),
                    insert: "!".into(),
                }],
            })
            .unwrap();
        assert_eq!(raced.apply(stale), Err(bareline_document::Error::StaleRevision));
        assert_eq!(text(&raced.snapshot()), format!("{}!", case.before));
        raced.undo().unwrap();
        assert_eq!(text(&raced.snapshot()), case.before);
        assert!(!raced.dirty(), "stale operation added an Undo entry: {}", case.id);
    }
    for case in &fixtures.failures {
        let doc = document(&case.before);
        let snapshot = doc.snapshot();
        let cancel = CancelToken::default();
        if case.cancelled {
            cancel.cancel();
        }
        let error = transform(
            &snapshot,
            TextOffset(case.range[0])..TextOffset(case.range[1]),
            kind,
            case.max_bytes,
            &cancel,
        )
        .err()
        .expect("A rejected transform must not produce an edit transaction");
        assert_eq!(format!("{error:?}"), case.error, "{}", case.id);
        assert_eq!(doc.snapshot().revision, snapshot.revision);
        assert_eq!(text(&doc.snapshot()), case.before);
        assert!(!doc.dirty());
    }
}

#[test]
fn base64_encode_contract() {
    verify("utilities.base64Encode", Transform::Base64Encode);
}

#[test]
fn base64_decode_contract() {
    verify("utilities.base64Decode", Transform::Base64Decode);
}

#[test]
fn url_encode_contract() {
    verify("utilities.urlEncode", Transform::UrlEncode);
}

#[test]
fn url_decode_contract() {
    verify("utilities.urlDecode", Transform::UrlDecode);
}
