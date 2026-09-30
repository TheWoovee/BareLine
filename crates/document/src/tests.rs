// SPDX-License-Identifier: MPL-2.0
use super::*;
fn read(snapshot: &DocumentSnapshot) -> String {
    snapshot
        .read(TextOffset(0)..TextOffset(snapshot.len()), 1 << 20)
        .unwrap()
}
#[test]
fn new_file_eol_is_only_a_fallback_and_survives_undo() {
    let mut document = Document::from_utf8("", Budget::new(65536), Budget::new(65536)).unwrap();
    document
        .initialize_metadata(
            DocumentMetadata::new(std::collections::BTreeMap::from([(
                "file.new_document_eol".into(),
                "crlf".into(),
            )]))
            .unwrap(),
        )
        .unwrap();
    assert!(!document.dirty());
    assert_eq!(document.snapshot().insertion_eol(), "\r\n");
    edit(&mut document, 0, 0, "literal\nLF").unwrap();
    assert_eq!(document.snapshot().insertion_eol(), "\n");
    assert_eq!(read(&document.snapshot()), "literal\nLF");
    document.undo().unwrap();
    assert!(!document.dirty());
    assert_eq!(document.snapshot().insertion_eol(), "\r\n");
    edit(&mut document, 0, 0, "a\r\nb\nc\r").unwrap();
    assert_eq!(document.snapshot().eol_label(), "Mixed");
    assert_eq!(read(&document.snapshot()), "a\r\nb\nc\r");
}
#[test]
fn snapshot_fork_has_independent_identity_and_edits_without_copying_text() {
    let bytes = Budget::new(4096);
    let source = Document::from_utf8("saved", bytes.clone(), Budget::new(4096)).unwrap();
    let before = bytes.used();
    let mut fork = Document::fork_from_snapshot(&source.snapshot(), bytes.clone(), Budget::new(4096)).unwrap();
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
    // Every one of the 100k entries must survive the full undo/redo walk, so the history
    // allowance is the computed charge peak; a larger charge evicts and undo then fails.
    // Text is charged to the byte budget only. Structure: an edit adds at most 3 leaves,
    // and a balanced tree of at most 300k leaves is under 32 levels high. Slots: the
    // geometric peak of full undo, old half-size redo and replacement redo.
    let edits = 100_000;
    let structure = edits * ((3 * 32 + NODES_PER_EDIT) * NODE_BYTES + std::mem::size_of::<history::OwnedEdit>());
    let slot_peak = (131_072 + 65_536 + 131_072) * std::mem::size_of::<History>();
    let history = Budget::new(structure + slot_peak);
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
    let samples = ["", "a", "\r", "\n", "\r\n", "ب", "👩🏽‍💻", "e\u{301}", "abc\r\ndef"];
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
        assert_eq!(current.line_count(), lines(&expected), "newlines at edit {n}");
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
    // History pressure no longer refuses edits; this test is about the byte budget.
    let undo = Budget::new(64 * 1024);
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
    assert_eq!(edit(&mut doc, 0, 0, &"x".repeat(20)), Err(Error::BudgetExceeded));
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
fn typed(document: &mut Document, at: usize, text: &str, boundary: u64, monotonic_ms: u64) -> Result<Revision, Error> {
    let caret = |offset| history::Selection {
        anchor: TextOffset(offset),
        caret: TextOffset(offset),
    };
    document.apply_with_metadata(
        EditTransaction {
            base_revision: document.snapshot().revision,
            edits: vec![Edit {
                range: TextOffset(at)..TextOffset(at),
                insert: text.into(),
            }],
        },
        history::EditMetadata {
            before: vec![caret(at)],
            after: vec![caret(at + text.len())],
            origin: history::EditOrigin::Typing,
            boundary,
            monotonic_ms,
        },
    )
}
#[test]
fn typing_merge_decision_is_reported_and_a_caret_jump_starts_a_new_entry() {
    let mut doc = Document::from_utf8("", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
    typed(&mut doc, 0, "a", 1, 0).unwrap();
    assert!(!doc.last_edit_merged());
    typed(&mut doc, 1, "b", 1, 10).unwrap();
    assert!(doc.last_edit_merged());
    // Home, then type: same boundary and time window, but the caret moved.
    typed(&mut doc, 0, "c", 1, 20).unwrap();
    assert!(!doc.last_edit_merged());
    assert_eq!(doc.history_stats().undo_changes, 2);
    assert_eq!(read(&doc.snapshot()), "cab");
    doc.undo().unwrap();
    assert_eq!(read(&doc.snapshot()), "ab");
    doc.undo().unwrap();
    assert_eq!(read(&doc.snapshot()), "");
}
#[test]
fn save_point_stops_typing_merge_and_undo_redo_reach_it() {
    let mut doc = Document::from_utf8("", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
    typed(&mut doc, 0, "a", 1, 0).unwrap();
    doc.mark_saved_state(doc.snapshot().content_state);
    assert!(!doc.dirty());
    typed(&mut doc, 1, "b", 1, 10).unwrap();
    assert!(!doc.last_edit_merged());
    assert!(doc.dirty());
    doc.undo().unwrap();
    assert_eq!(read(&doc.snapshot()), "a");
    assert!(!doc.dirty());
    doc.undo().unwrap();
    assert!(doc.dirty());
    doc.redo().unwrap();
    assert_eq!(read(&doc.snapshot()), "a");
    assert!(!doc.dirty());
}
#[test]
fn history_pressure_evicts_oldest_entries_instead_of_refusing_edits() {
    let text = "x".repeat(1 << 20);
    let history = Budget::new(64 * 1024);
    let mut doc = Document::from_utf8(&text, Budget::new(4 << 20), history.clone()).unwrap();
    // Select All + Delete: the deleted megabyte is owned by the before-root already,
    // so it is not charged to the much smaller history budget again.
    edit(&mut doc, 0, text.len(), "").unwrap();
    assert!(doc.snapshot().is_empty());
    assert!(doc.dirty());
    // Keep typing well past the budget: the oldest entries give way.
    for offset in 0..2000 {
        edit(&mut doc, offset, offset, "y").unwrap();
    }
    assert!(history.used() <= history.limit());
    let undo = doc.history_stats().undo_changes;
    assert!(undo > 1 && undo < 2001, "{undo} entries retained");
    doc.undo().unwrap();
    assert_eq!(doc.snapshot().len(), 1999);
    doc.redo().unwrap();
    assert_eq!(doc.snapshot().len(), 2000);
}
#[test]
fn rejected_edits_under_history_pressure_evict_nothing() {
    let mut doc = Document::from_utf8("", Budget::new(4 << 20), Budget::new(64 * 1024)).unwrap();
    for offset in 0..2000 {
        edit(&mut doc, offset, offset, "y").unwrap();
    }
    let depth = doc.history_stats().undo_changes;
    let large = "z".repeat(16 * 1024);
    // Each edit below could only be charged by evicting, but each is rejected first.
    assert!(edit(&mut doc, 0, 5000, &large).is_err());
    let stale = EditTransaction {
        base_revision: Revision(0),
        edits: vec![Edit {
            range: TextOffset(0)..TextOffset(0),
            insert: large.clone(),
        }],
    };
    assert_eq!(doc.apply(stale), Err(Error::StaleRevision));
    let caret = |offset| history::Selection {
        anchor: TextOffset(offset),
        caret: TextOffset(offset),
    };
    let past_end = doc.apply_with_metadata(
        EditTransaction {
            base_revision: doc.snapshot().revision,
            edits: vec![Edit {
                range: TextOffset(0)..TextOffset(0),
                insert: large.clone(),
            }],
        },
        history::EditMetadata {
            before: vec![caret(0)],
            after: vec![caret(1 << 20)],
            origin: history::EditOrigin::Typing,
            boundary: 1,
            monotonic_ms: 0,
        },
    );
    assert!(past_end.is_err());
    assert_eq!(doc.history_stats().undo_changes, depth);
    assert_eq!(doc.snapshot().len(), 2000);
    // A valid edit whose many tree pieces need several entries' room is admitted by
    // evicting older entries.
    doc.apply(spread(&doc, 64)).unwrap();
    assert!(!doc.last_edit_untracked());
    assert!(doc.history_stats().undo_changes < depth);
    doc.undo().unwrap();
    assert_eq!(doc.snapshot().len(), 2000);
}
/// One-byte inserts at `count` separate offsets: a single entry charging many tree pieces.
fn spread(document: &Document, count: usize) -> EditTransaction {
    EditTransaction {
        base_revision: document.snapshot().revision,
        edits: (0..count)
            .map(|index| Edit {
                range: TextOffset(index * 16)..TextOffset(index * 16),
                insert: "z".into(),
            })
            .collect(),
    }
}
#[test]
fn replacing_everything_with_a_paste_larger_than_the_history_budget_stays_undoable() {
    let text = "x".repeat(64 * 1024);
    let history = Budget::new(16 * 1024);
    let mut doc = Document::from_utf8(&text, Budget::new(1 << 20), history.clone()).unwrap();
    // Select All + Paste: both the replaced and the pasted text are owned by tree
    // segments in the byte budget, so neither is charged to the history budget.
    let paste = "p".repeat(128 * 1024);
    edit(&mut doc, 0, text.len(), &paste).unwrap();
    assert!(!doc.last_edit_untracked());
    assert!(history.used() <= history.limit());
    assert_eq!(doc.snapshot().len(), paste.len());
    doc.undo().unwrap();
    assert_eq!(read(&doc.snapshot()), text);
    doc.redo().unwrap();
    assert_eq!(doc.snapshot().len(), paste.len());
}
#[test]
fn a_peer_taking_the_room_relief_made_is_retried_without_clearing_history() {
    let history = Budget::new(64 * 1024);
    let mut doc = Document::from_utf8("", Budget::new(1 << 20), history.clone()).unwrap();
    for offset in 0..8 {
        edit(&mut doc, offset, offset, "y").unwrap();
    }
    // Another worker takes whatever room is left right after the first relief.
    let mut taken = None;
    let mut racing = |document: &mut Document, demand: Demand| {
        document.relieve_history(demand);
        if taken.is_none() {
            taken = Some(history.claim(history.limit() - history.used()).unwrap());
        }
    };
    let transaction = EditTransaction {
        base_revision: doc.snapshot().revision,
        edits: vec![Edit {
            range: TextOffset(8)..TextOffset(8),
            insert: "z".into(),
        }],
    };
    doc.apply_relieved(transaction, None, &mut racing).unwrap();
    assert!(!doc.last_edit_untracked());
    // Relief ran again and evicted only the oldest entries; the rest stay undoable.
    assert!(doc.history_stats().undo_changes > 1);
    drop(taken);
    doc.undo().unwrap();
    assert_eq!(read(&doc.snapshot()), "yyyyyyyy");
    doc.undo().unwrap();
    assert_eq!(read(&doc.snapshot()), "yyyyyyy");
}
#[test]
fn when_relief_frees_nothing_the_documents_own_oldest_entries_admit_the_edit() {
    let history = Budget::new(64 * 1024);
    let mut doc = Document::from_utf8("", Budget::new(1 << 20), history.clone()).unwrap();
    for offset in 0..8 {
        edit(&mut doc, offset, offset, "y").unwrap();
    }
    let depth = doc.history_stats().undo_changes;
    // The budget is full with claims no relief can evict, and relief declines.
    let full = history.claim(history.limit() - history.used()).unwrap();
    let transaction = EditTransaction {
        base_revision: doc.snapshot().revision,
        edits: vec![Edit {
            range: TextOffset(8)..TextOffset(8),
            insert: "z".into(),
        }],
    };
    doc.apply_relieved(transaction, None, &mut |_: &mut Document, _: Demand| {})
        .unwrap();
    assert!(!doc.last_edit_untracked());
    let kept = doc.history_stats().undo_changes;
    assert!((1..=depth).contains(&kept));
    drop(full);
    doc.undo().unwrap();
    assert_eq!(read(&doc.snapshot()), "yyyyyyyy");
}
#[test]
fn an_edit_no_history_can_admit_is_applied_and_reported_untracked() {
    let history = Budget::new(64 * 1024);
    let mut doc = Document::from_utf8(&"x".repeat(4096), Budget::new(1 << 20), history.clone()).unwrap();
    edit(&mut doc, 0, 0, "y").unwrap();
    assert!(!doc.last_edit_untracked());
    // Claims no eviction can free fill the budget, and this edit needs more room than
    // the document's own single entry releases.
    let full = history.claim(history.limit() - history.used()).unwrap();
    doc.apply(spread(&doc, 128)).unwrap();
    assert_eq!(doc.snapshot().len(), 4096 + 1 + 128);
    assert!(doc.last_edit_untracked());
    assert_eq!(doc.history_stats().undo_changes, 0);
    assert_eq!(doc.undo(), Err(Error::EmptyHistory));
    drop(full);
    edit(&mut doc, 0, 0, "w").unwrap();
    assert!(!doc.last_edit_untracked());
    doc.undo().unwrap();
    assert_eq!(doc.snapshot().len(), 4096 + 1 + 128);
}
#[test]
fn repeated_select_all_paste_evicts_history_that_alone_keeps_replaced_text() {
    let size = 96 * 1024;
    let bytes = Budget::new(256 * 1024);
    let mut doc = Document::from_utf8(&"a".repeat(size), bytes.clone(), Budget::new(1 << 20)).unwrap();
    // From the second paste on, the byte budget cannot hold the paste next to the live
    // text and the text replaced two pastes ago, which only undo history keeps alive.
    // That entry gives way instead of the paste being refused.
    for letter in ["b", "c", "d", "e"] {
        edit(&mut doc, 0, size, &letter.repeat(size)).unwrap();
        assert!(!doc.last_edit_untracked());
        assert_eq!(doc.history_stats().undo_changes, 1);
        assert_eq!(read(&doc.snapshot()), letter.repeat(size));
    }
    assert!(bytes.used() <= bytes.limit());
    doc.undo().unwrap();
    assert_eq!(read(&doc.snapshot()), "d".repeat(size));
    doc.redo().unwrap();
    assert_eq!(read(&doc.snapshot()), "e".repeat(size));
}
#[test]
fn byte_shortfall_of_live_text_is_refused_and_keeps_history() {
    let bytes = Budget::new(256 * 1024);
    let mut doc = Document::from_utf8("", bytes.clone(), Budget::new(1 << 20)).unwrap();
    for offset in 0..3 {
        edit(&mut doc, offset, offset, "x").unwrap();
    }
    // Live text of another document fills most of the shared byte budget, and this
    // document's history keeps no text alive on its own.
    let _live = Document::from_utf8(&"w".repeat(200 * 1024), bytes.clone(), Budget::new(1 << 20)).unwrap();
    assert_eq!(
        edit(&mut doc, 3, 3, &"z".repeat(100 * 1024)),
        Err(Error::BudgetExceeded)
    );
    assert_eq!(doc.history_stats().undo_changes, 3);
    doc.undo().unwrap();
    assert_eq!(read(&doc.snapshot()), "xx");
}
#[test]
fn typed_text_coalesces_into_few_leaves() {
    let mut doc = Document::from_utf8("", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
    for (offset, ms) in (0..200).zip((0..).step_by(10)) {
        typed(&mut doc, offset, "z", 1, ms).unwrap();
    }
    assert_eq!(read(&doc.snapshot()), "z".repeat(200));
    // One leaf per keystroke would need a tree about eight levels high.
    assert!(tree::height(&doc.snapshot().root) <= 2);
    doc.undo().unwrap();
    assert!(doc.snapshot().is_empty());
}
fn linked_pair() -> (Document, Document, group::UndoGroup) {
    let bytes = Budget::new(1 << 20);
    let mut first = Document::from_utf8("first", bytes.clone(), Budget::new(1 << 20)).unwrap();
    let mut second = Document::from_utf8("second", bytes, Budget::new(1 << 20)).unwrap();
    let replace = |document: &Document, text: &str| EditTransaction {
        base_revision: document.snapshot().revision,
        edits: vec![Edit {
            range: TextOffset(0)..TextOffset(document.snapshot().len()),
            insert: text.into(),
        }],
    };
    let prepared = vec![
        first.prepare(replace(&first, "one")).unwrap(),
        second.prepare(replace(&second, "two")).unwrap(),
    ];
    let id = group::commit(&mut [&mut first, &mut second], prepared).unwrap();
    assert_eq!(second.undo(), Err(Error::LinkedUndoRequired));
    (first, second, id)
}
#[test]
fn losing_one_linked_entry_downgrades_partners_to_local_undo() {
    // Trimmed by the history limit.
    let (mut first, mut second, id) = linked_pair();
    first.set_history_policy(history::HistoryPolicy {
        max_changes: 0,
        ..Default::default()
    });
    assert!(group::undo(&mut [&mut first, &mut second], id).is_err());
    second.undo().unwrap();
    assert_eq!(read(&second.snapshot()), "second");
    second.redo().unwrap();
    assert_eq!(read(&second.snapshot()), "two");
    // Evicted under budget pressure.
    let (mut first, mut second, _) = linked_pair();
    assert!(first.evict_oldest_history((0, 0)));
    second.undo().unwrap();
    assert_eq!(read(&second.snapshot()), "second");
    // Closed with its partner still open.
    let (first, mut second, _) = linked_pair();
    drop(first);
    second.undo().unwrap();
    assert_eq!(read(&second.snapshot()), "second");
}
