// SPDX-License-Identifier: MPL-2.0
//! Seeded property tests (QA-05): Resident edits, undo/redo, selection metadata and
//! line mapping against a `String` oracle. Fixed seeds and bounded steps keep every
//! failure reproducible; no property-testing dependency is needed.
use bareline_document::{
    Budget, Document, DocumentSnapshot, Edit, EditTransaction, Error, Revision, TextOffset,
    history::{EditMetadata, EditOrigin, HistoryPolicy, Selection},
};

/// SplitMix64. Report the seed and step of a failure to reproduce it exactly.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const SAMPLES: &[&str] = &[
    "",
    "a",
    "xyz",
    "\r",
    "\n",
    "\r\n",
    "\n\r",
    "é",
    "ب",
    "👩🏽‍💻",
    "e\u{301}",
    "\u{feff}",
    "abc\r\ndef",
    "\r\r\n\n",
];

fn read_all(snapshot: &DocumentSnapshot) -> String {
    snapshot
        .read(TextOffset(0)..TextOffset(snapshot.len()), usize::MAX)
        .unwrap()
}
fn boundaries(text: &str) -> Vec<usize> {
    text.char_indices().map(|(i, _)| i).chain(Some(text.len())).collect()
}
/// Offsets after each CR, LF or CRLF terminator (a CRLF pair counts once).
fn line_starts(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = vec![0];
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i += 2;
            starts.push(i);
        } else {
            if bytes[i] == b'\r' || bytes[i] == b'\n' {
                starts.push(i + 1);
            }
            i += 1;
        }
    }
    starts
}
fn eol_label(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let cr = text.matches('\r').count() - crlf;
    let lf = text.matches('\n').count() - crlf;
    match (cr > 0, lf > 0, crlf > 0) {
        (false, _, false) => "LF",
        (true, false, false) => "CR",
        (false, false, true) => "CRLF",
        _ => "Mixed",
    }
}

#[derive(Clone)]
struct State {
    text: String,
    id: u64,
}
struct Step {
    before: State,
    after: State,
    metadata: EditMetadata,
}
/// Oracle history: content-state identities are modelled by `State::id`.
struct Model {
    current: State,
    saved: u64,
    undo: Vec<Step>,
    redo: Vec<Step>,
    revision: u64,
    next_id: u64,
    max_changes: usize,
}
impl Model {
    fn commit(&mut self, text: String, metadata: EditMetadata) {
        self.next_id += 1;
        let after = State { text, id: self.next_id };
        let before = std::mem::replace(&mut self.current, after.clone());
        self.undo.push(Step {
            before,
            after,
            metadata,
        });
        self.redo.clear();
        if self.undo.len() > self.max_changes {
            self.undo.remove(0);
        }
        self.revision += 1;
    }
}

fn check(document: &Document, model: &Model, rng: &mut Rng, context: &str) {
    let snapshot = document.snapshot();
    let text = &model.current.text;
    assert_eq!(read_all(&snapshot), *text, "{context}: text");
    assert_eq!(snapshot.len(), text.len(), "{context}: len");
    assert_eq!(snapshot.is_empty(), text.is_empty(), "{context}: is_empty");
    assert_eq!(snapshot.revision, Revision(model.revision), "{context}: revision");
    assert_eq!(snapshot.eol_label(), eol_label(text), "{context}: eol label");
    assert_eq!(document.dirty(), model.current.id != model.saved, "{context}: dirty");
    let stats = document.history_stats();
    assert_eq!(stats.undo_changes, model.undo.len(), "{context}: undo depth");
    assert_eq!(stats.redo_changes, model.redo.len(), "{context}: redo depth");
    assert_eq!(
        document.history_metadata(true),
        model.undo.last().map(|step| &step.metadata),
        "{context}: undo metadata"
    );
    assert_eq!(
        document.history_metadata(false),
        model.redo.last().map(|step| &step.metadata),
        "{context}: redo metadata"
    );
    let starts = line_starts(text);
    assert_eq!(snapshot.line_count(), starts.len(), "{context}: line count");
    for (line, start) in starts.iter().enumerate() {
        let end = starts.get(line + 1).copied().unwrap_or(text.len());
        assert_eq!(
            snapshot.line_range(line),
            Ok(TextOffset(*start)..TextOffset(end)),
            "{context}: line {line} range"
        );
    }
    assert_eq!(snapshot.line_range(starts.len()), Err(Error::OutOfBounds));
    for _ in 0..24 {
        let offset = rng.below(text.len() + 1);
        let expected = if text.is_char_boundary(offset) {
            Ok(starts.partition_point(|start| *start <= offset) - 1)
        } else {
            Err(Error::InvalidBoundary)
        };
        assert_eq!(
            snapshot.line_at(TextOffset(offset)),
            expected,
            "{context}: line_at({offset})"
        );
        assert_eq!(snapshot.is_boundary(TextOffset(offset)), text.is_char_boundary(offset));
    }
    assert_eq!(snapshot.line_at(TextOffset(text.len() + 1)), Err(Error::OutOfBounds));
    let points = boundaries(text);
    let (a, b) = (*rng.pick(&points), *rng.pick(&points));
    let range = a.min(b)..a.max(b);
    let joined: String = snapshot
        .chunks(TextOffset(range.start)..TextOffset(range.end))
        .unwrap()
        .collect();
    assert_eq!(joined, text[range.clone()], "{context}: chunks {range:?}");
    if !range.is_empty() {
        assert_eq!(
            snapshot.read(TextOffset(range.start)..TextOffset(range.end), range.len() - 1),
            Err(Error::BudgetExceeded)
        );
    }
}

/// Sorted, nonoverlapping edits with distinct starts, in oracle byte offsets.
fn random_edits(rng: &mut Rng, text: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let points = boundaries(text);
    let count = 1 + rng.below(3);
    let mut picks: Vec<usize> = (0..count * 2).map(|_| *rng.pick(&points)).collect();
    picks.sort_unstable();
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for pair in picks.chunks(2) {
        if edits.last().is_some_and(|(previous, _)| previous.start == pair[0]) {
            continue;
        }
        // Keep documents small: large texts favour deletion.
        let insert = if text.len() > 384 && rng.chance(70) {
            ""
        } else {
            *rng.pick(SAMPLES)
        };
        edits.push((pair[0]..pair[1], insert.to_owned()));
    }
    edits
}
fn apply_oracle(text: &str, edits: &[(std::ops::Range<usize>, String)]) -> String {
    let mut result = text.to_owned();
    for (range, insert) in edits.iter().rev() {
        result.replace_range(range.clone(), insert);
    }
    result
}
fn transaction(revision: u64, edits: &[(std::ops::Range<usize>, String)], rng: &mut Rng) -> EditTransaction {
    let mut edits: Vec<Edit> = edits
        .iter()
        .map(|(range, insert)| Edit {
            range: TextOffset(range.start)..TextOffset(range.end),
            insert: insert.clone(),
        })
        .collect();
    // Document::prepare sorts; submitting out of order must not change the result.
    if rng.chance(50) {
        edits.reverse();
    }
    EditTransaction {
        base_revision: Revision(revision),
        edits,
    }
}
fn selections(rng: &mut Rng, text: &str) -> Vec<Selection> {
    let points = boundaries(text);
    (0..1 + rng.below(3))
        .map(|_| Selection {
            anchor: TextOffset(*rng.pick(&points)),
            caret: TextOffset(*rng.pick(&points)),
        })
        .collect()
}
fn interior_offset(rng: &mut Rng, text: &str) -> Option<usize> {
    let interior: Vec<usize> = (0..text.len()).filter(|i| !text.is_char_boundary(*i)).collect();
    (!interior.is_empty()).then(|| *rng.pick(&interior))
}

fn run_edit_history(seed: u64, steps: usize, max_changes: Option<usize>) {
    let mut rng = Rng(seed);
    let bytes = Budget::new(64 << 20);
    let history = Budget::new(64 << 20);
    let initial = "seed\r\nline é\rend".to_owned();
    let mut document = Document::from_utf8(&initial, bytes.clone(), history.clone()).unwrap();
    let mut model = Model {
        current: State { text: initial, id: 0 },
        saved: 0,
        undo: Vec::new(),
        redo: Vec::new(),
        revision: 0,
        next_id: 0,
        max_changes: HistoryPolicy::default().max_changes,
    };
    if let Some(max_changes) = max_changes {
        document.set_history_policy(HistoryPolicy {
            max_changes,
            ..HistoryPolicy::default()
        });
        model.max_changes = max_changes;
    }
    // Retained snapshots must stay immutable and keep their content identity.
    let mut retained: Vec<(DocumentSnapshot, State)> = vec![(document.snapshot(), model.current.clone())];
    for step in 0..steps {
        let context = format!("seed {seed:#x} step {step}");
        let text = model.current.text.clone();
        match rng.below(100) {
            0..=39 => {
                let edits = random_edits(&mut rng, &text);
                let expected = apply_oracle(&text, &edits);
                let revision = document
                    .apply(transaction(model.revision, &edits, &mut rng))
                    .unwrap_or_else(|error| panic!("{context}: edit {edits:?} failed: {error:?}"));
                model.commit(expected, EditMetadata::default());
                assert_eq!(revision, Revision(model.revision), "{context}");
            }
            40..=49 => {
                let edits = random_edits(&mut rng, &text);
                let expected = apply_oracle(&text, &edits);
                let metadata = EditMetadata {
                    before: selections(&mut rng, &text),
                    after: selections(&mut rng, &expected),
                    origin: *rng.pick(&[
                        EditOrigin::Paste,
                        EditOrigin::Macro,
                        EditOrigin::MultiCursor,
                        EditOrigin::ReplaceAll,
                        EditOrigin::Extension,
                        EditOrigin::Command,
                    ]),
                    boundary: rng.next_u64(),
                    monotonic_ms: rng.next_u64(),
                };
                document
                    .apply_with_metadata(transaction(model.revision, &edits, &mut rng), metadata.clone())
                    .unwrap_or_else(|error| panic!("{context}: selection edit failed: {error:?}"));
                model.commit(expected, metadata);
            }
            50..=61 => match model.undo.pop() {
                Some(entry) => {
                    assert_eq!(document.undo(), Ok(Revision(model.revision + 1)), "{context}: undo");
                    model.revision += 1;
                    model.current = entry.before.clone();
                    model.redo.push(entry);
                }
                None => assert_eq!(document.undo(), Err(Error::EmptyHistory), "{context}"),
            },
            62..=73 => match model.redo.pop() {
                Some(entry) => {
                    assert_eq!(document.redo(), Ok(Revision(model.revision + 1)), "{context}: redo");
                    model.revision += 1;
                    model.current = entry.after.clone();
                    model.undo.push(entry);
                }
                None => assert_eq!(document.redo(), Err(Error::EmptyHistory), "{context}"),
            },
            74..=78 => {
                let (snapshot, state) = rng.pick(&retained);
                document.mark_saved(snapshot).unwrap();
                model.saved = state.id;
            }
            79..=81 => {
                // An empty transaction at the current revision is a no-op, not a history entry.
                let revision = document.apply(EditTransaction {
                    base_revision: Revision(model.revision),
                    edits: Vec::new(),
                });
                assert_eq!(revision, Ok(Revision(model.revision)), "{context}");
            }
            choice => {
                let len = text.len();
                let (edits, expected) = match choice % 6 {
                    0 => (
                        vec![Edit {
                            range: TextOffset(0)..TextOffset(len + 1),
                            insert: "x".into(),
                        }],
                        Error::OutOfBounds,
                    ),
                    1 if len > 0 => (
                        vec![Edit {
                            range: TextOffset(len)..TextOffset(0),
                            insert: String::new(),
                        }],
                        Error::OutOfBounds,
                    ),
                    2 => {
                        let at = *rng.pick(&boundaries(&text));
                        (
                            vec![
                                Edit {
                                    range: TextOffset(at)..TextOffset(at),
                                    insert: "a".into(),
                                },
                                Edit {
                                    range: TextOffset(at)..TextOffset(at),
                                    insert: "b".into(),
                                },
                            ],
                            Error::OverlappingEdits,
                        )
                    }
                    3 => match interior_offset(&mut rng, &text) {
                        Some(at) => (
                            vec![Edit {
                                range: TextOffset(at)..TextOffset(len),
                                insert: String::new(),
                            }],
                            Error::InvalidBoundary,
                        ),
                        None => continue,
                    },
                    4 => {
                        let stale = EditTransaction {
                            base_revision: Revision(model.revision.wrapping_add(1 + rng.below(3) as u64)),
                            edits: Vec::new(),
                        };
                        assert_eq!(document.apply(stale), Err(Error::StaleRevision), "{context}");
                        check(&document, &model, &mut rng, &context);
                        continue;
                    }
                    _ => {
                        // Selection metadata is validated against the post-edit text.
                        let edits = random_edits(&mut rng, &text);
                        let expected_text = apply_oracle(&text, &edits);
                        let mut metadata = EditMetadata {
                            before: selections(&mut rng, &text),
                            after: selections(&mut rng, &expected_text),
                            ..EditMetadata::default()
                        };
                        let error = match interior_offset(&mut rng, &expected_text) {
                            Some(at) if rng.chance(50) => {
                                metadata.after.push(Selection {
                                    anchor: TextOffset(at),
                                    caret: TextOffset(at),
                                });
                                Error::InvalidBoundary
                            }
                            _ => {
                                metadata.before.push(Selection {
                                    anchor: TextOffset(0),
                                    caret: TextOffset(len + 1),
                                });
                                Error::OutOfBounds
                            }
                        };
                        let result =
                            document.apply_with_metadata(transaction(model.revision, &edits, &mut rng), metadata);
                        assert_eq!(result, Err(error), "{context}: invalid selection");
                        check(&document, &model, &mut rng, &context);
                        continue;
                    }
                };
                let result = document.apply(EditTransaction {
                    base_revision: Revision(model.revision),
                    edits,
                });
                assert_eq!(result, Err(expected), "{context}: invalid edit");
            }
        }
        check(&document, &model, &mut rng, &context);
        for (snapshot, state) in &retained {
            assert_eq!(read_all(snapshot), state.text, "{context}: retained snapshot changed");
            assert_eq!(
                snapshot.content_state == document.snapshot().content_state,
                state.id == model.current.id,
                "{context}: content identity"
            );
        }
        if rng.chance(20) {
            if retained.len() == 8 {
                retained.remove(rng.below(8));
            }
            retained.push((document.snapshot(), model.current.clone()));
        }
    }
    drop(retained);
    drop(document);
    assert_eq!(bytes.used(), 0, "seed {seed:#x}: text budget leaked");
    assert_eq!(history.used(), 0, "seed {seed:#x}: history budget leaked");
}

#[test]
fn edits_undo_redo_and_selections_match_string_oracle() {
    for seed in [0x5eed_0001, 0x5eed_0002, 0x5eed_0003] {
        run_edit_history(seed, 300, None);
    }
}

#[test]
fn bounded_history_trims_oldest_changes_like_the_oracle() {
    for seed in [0x7121_0001, 0x7121_0002] {
        run_edit_history(seed, 200, Some(1 + (seed as usize % 7)));
    }
}

#[test]
fn typing_bursts_merge_only_contiguous_unsaved_keystrokes() {
    const INTERVAL: u64 = 1000;
    const KEYS: &[&str] = &["a", "Z", " ", "\r", "\n", "é", "ب", "👩", "\u{301}"];
    for seed in [0x7e57_0001u64, 0x7e57_0002, 0x7e57_0003, 0x7e57_0004] {
        let mut rng = Rng(seed);
        let mut document = Document::from_utf8("start\r\nend", Budget::new(1 << 20), Budget::new(8 << 20)).unwrap();
        let mut text = "start\r\nend".to_owned();
        // Text before each expected undo step, plus the final text.
        let mut step_starts: Vec<String> = Vec::new();
        let mut caret = text.len();
        let mut boundary = 1u64;
        let mut now = 10_000u64;
        let mut last_after: Option<usize> = None;
        let mut saved_at_last = false;
        for key in 0..120 {
            let context = format!("seed {seed:#x} key {key}");
            // A boundary change, a pause, a caret move or a save point ends the burst.
            match rng.below(10) {
                0 => {
                    boundary += 1;
                    last_after = None;
                }
                1 => {
                    now += INTERVAL + 1 + rng.below(50) as u64;
                    last_after = None;
                }
                2 => caret = *rng.pick(&boundaries(&text)),
                3 => {
                    document.mark_saved(&document.snapshot()).unwrap();
                    saved_at_last = true;
                }
                _ => now += rng.below(INTERVAL as usize + 1) as u64,
            }
            let insert = *rng.pick(KEYS);
            let merges = last_after == Some(caret) && !saved_at_last;
            if !merges {
                step_starts.push(text.clone());
            }
            let metadata = EditMetadata {
                before: vec![Selection {
                    anchor: TextOffset(caret),
                    caret: TextOffset(caret),
                }],
                after: vec![Selection {
                    anchor: TextOffset(caret + insert.len()),
                    caret: TextOffset(caret + insert.len()),
                }],
                origin: EditOrigin::Typing,
                boundary,
                monotonic_ms: now,
            };
            document
                .apply_with_metadata(
                    EditTransaction {
                        base_revision: document.snapshot().revision,
                        edits: vec![Edit {
                            range: TextOffset(caret)..TextOffset(caret),
                            insert: insert.into(),
                        }],
                    },
                    metadata,
                )
                .unwrap_or_else(|error| panic!("{context}: {error:?}"));
            text.insert_str(caret, insert);
            caret += insert.len();
            last_after = Some(caret);
            saved_at_last = false;
            assert_eq!(read_all(&document.snapshot()), text, "{context}");
            assert_eq!(
                document.history_stats().undo_changes,
                step_starts.len(),
                "{context}: merged steps"
            );
        }
        let finished = text.clone();
        for expected in step_starts.iter().rev() {
            document.undo().unwrap();
            assert_eq!(read_all(&document.snapshot()), *expected, "seed {seed:#x}: undo");
        }
        assert_eq!(document.undo(), Err(Error::EmptyHistory));
        for _ in 0..step_starts.len() {
            document.redo().unwrap();
        }
        assert_eq!(read_all(&document.snapshot()), finished, "seed {seed:#x}: redo");
    }
}
