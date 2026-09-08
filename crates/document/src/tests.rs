// SPDX-License-Identifier: MPL-2.0
use super::*;
fn read(snapshot: &DocumentSnapshot) -> String {
    snapshot
        .read(TextOffset(0)..TextOffset(snapshot.len()), 1 << 20)
        .unwrap()
}
#[test]
fn snapshot_fork_has_independent_identity_and_edits_without_copying_text() {
    let bytes = Budget::new(4096);
    let source = Document::from_utf8("saved", bytes.clone(), Budget::new(4096)).unwrap();
    let before = bytes.used();
    let mut fork =
        Document::fork_from_snapshot(&source.snapshot(), bytes.clone(), Budget::new(4096)).unwrap();
    assert_eq!(before, bytes.used());
    assert!(!fork.snapshot().same_document(&source.snapshot()));
    let initial_token = fork.snapshot().identity_token();
    assert_ne!(initial_token, source.snapshot().identity_token());
    edit(&mut fork, 0, 5, "changed").unwrap();
    assert_eq!(read(&source.snapshot()), "saved");
    assert_eq!(read(&fork.snapshot()), "changed");
    fork.undo().unwrap();
    assert_eq!(read(&fork.snapshot()), "saved");
    assert_ne!(initial_token, fork.snapshot().identity_token());
}
fn edit(document: &mut Document, start: usize, end: usize, text: &str) -> Result<Revision, Error> {
    document.apply(EditTransaction {
        base_revision: document.snapshot().revision,
        edits: vec![Edit {
            range: TextOffset(start)..TextOffset(end),
            insert: text.into(),
        }],
    })
}
fn lines(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut count = 1;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' {
            count += 1;
            if bytes.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
        } else if bytes[i] == b'\n' {
            count += 1;
        }
        i += 1;
    }
    count
}
#[test]
fn random_edit_oracle_covers_newline_boundaries_snapshots_and_undo_redo() {
    let budget = Budget::new(64 << 20);
    // Payload allowance plus the charged geometric container peak: full undo,
    // old half-size redo, and replacement redo during the unchanged100k oracle.
    let slot_peak = (131_072 + 65_536 + 131_072) * std::mem::size_of::<History>();
    let history = Budget::new((64 << 20) + slot_peak);
    let mut doc = Document::from_utf8("", budget.clone(), history).unwrap();
    let original = doc.snapshot();
    let mut expected = String::new();
    let mut rng = 0x123456789abcdefu64;
    fn next(seed: &mut u64) -> usize {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed as usize
    }
    let samples = [
        "",
        "a",
        "\r",
        "\n",
        "\r\n",
        "ب",
        "👩🏽‍💻",
        "e\u{301}",
        "abc\r\ndef",
    ];
    for n in 0..100_000 {
        let boundaries: Vec<_> = expected
            .char_indices()
            .map(|(i, _)| i)
            .chain(Some(expected.len()))
            .collect();
        let a = boundaries[next(&mut rng) % boundaries.len()];
        let b = boundaries[next(&mut rng) % boundaries.len()];
        let (a, b) = (a.min(b), a.max(b));
        let insert = samples[next(&mut rng) % samples.len()];
        let before = doc.snapshot();
        edit(&mut doc, a, b, insert).unwrap();
        assert_eq!(read(&before), expected, "snapshot changed at edit {n}");
        expected.replace_range(a..b, insert);
        let current = doc.snapshot();
        assert_eq!(read(&current), expected, "edit {n}");
        assert_eq!(
            current.line_count(),
            lines(&expected),
            "newlines at edit {n}"
        );
        for line in 0..current.line_count() {
            let range = current.line_range(line).unwrap();
            assert_eq!(current.line_at(range.start).unwrap(), line);
            assert!(range.end.0 <= expected.len());
        }
        if n % 1000 == 0 {
            tree::assert_balanced(&current.root);
        }
    }
    for _ in 0..100_000 {
        doc.undo().unwrap();
    }
    assert_eq!(read(&doc.snapshot()), "");
    assert!(!doc.dirty());
    assert_eq!(read(&original), "");
    for _ in 0..100_000 {
        doc.redo().unwrap();
    }
    assert_eq!(read(&doc.snapshot()), expected);
    tree::assert_balanced(&doc.snapshot().root);
    drop(doc);
    drop(original);
    assert_eq!(budget.used(), 0);
}
#[test]
fn failed_batch_and_exhausted_shared_budget_do_not_commit() {
    let budget = Budget::new(16);
    // Keep the payload allowance tiny, while admitting one undo and one redo slot.
    let undo = Budget::new(32 + 2 * std::mem::size_of::<History>());
    let mut doc = Document::from_utf8("aب\r\n", budget.clone(), undo.clone()).unwrap();
    let saved = doc.snapshot();
    assert_eq!(edit(&mut doc, 2, 2, "x"), Err(Error::InvalidBoundary));
    let result = doc.apply(EditTransaction {
        base_revision: saved.revision,
        edits: vec![
            Edit {
                range: TextOffset(0)..TextOffset(3),
                insert: "x".into(),
            },
            Edit {
                range: TextOffset(1)..TextOffset(3),
                insert: "y".into(),
            },
        ],
    });
    assert_eq!(result, Err(Error::OverlappingEdits));
    assert_eq!(read(&doc.snapshot()), read(&saved));
    assert_eq!(
        edit(&mut doc, 0, 0, &"x".repeat(20)),
        Err(Error::BudgetExceeded)
    );
    assert_eq!(doc.snapshot().revision, saved.revision);
    assert_eq!(undo.used(), 0);
    assert_eq!(budget.used(), saved.len());
    // Failed phase above retains its16-byte cap. Success/undo additionally own
    // both the preceding receipt and the newly prepared receipt until publication.
    let receipt_bytes = std::mem::size_of::<crate::change::AppliedChange>()
        + std::mem::size_of::<crate::change::CompactEdit>()
        + 2 * std::mem::size_of::<usize>();
    budget.set_limit(16 + 2 * receipt_bytes);
    edit(&mut doc, 0, 0, "z").unwrap();
    doc.mark_saved(&saved).unwrap();
    assert!(doc.dirty());
    doc.undo().unwrap();
    assert!(!doc.dirty());
    assert_eq!(
        doc.apply(EditTransaction {
            base_revision: saved.revision,
            edits: Vec::new()
        }),
        Err(Error::StaleRevision)
    );
}
#[test]
fn crlf_split_at_chunk_boundary_counts_once_and_undo_owns_deleted_bytes() {
    let text = format!("{}\r\nend\r", "x".repeat(65535));
    let mut doc = Document::from_utf8(&text, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
    assert_eq!(doc.snapshot().line_count(), 3);
    edit(&mut doc, 65535, 65536, "").unwrap();
    assert_eq!(doc.snapshot().line_count(), 3);
    let length = doc.snapshot().len();
    edit(&mut doc, 0, length, "").unwrap();
    assert_eq!(doc.snapshot().line_count(), 1);
    doc.undo().unwrap();
    doc.undo().unwrap();
    assert_eq!(read(&doc.snapshot()), text);
}
