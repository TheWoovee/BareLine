// SPDX-License-Identifier: MPL-2.0
//! Seeded property tests (QA-05, QA-16): paged windows, sparse line checkpoints and
//! line lookups over an evicting page cache, checked against a `String` oracle while
//! materialized edits, undo and redo change the piece tree.
use bareline_document::{
    Budget, Edit, EditTransaction, Error, Revision, TextOffset,
    line_lookup::{LineLookupPoll, LineLookupRequest, LineTarget},
    paged::{
        IndexError, LineCount, PagedDocument, PagedSnapshot, SparseLineIndex, TextWindow, WindowPoll, WindowRequest,
    },
    source::{Generation, MemorySource, PageTicket, SourceKind, SourcePublisher},
};
use std::ops::Range;

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

const PIECES: &[&str] = &[
    "",
    "a",
    "line",
    " ",
    "\r",
    "\n",
    "\r\n",
    "é",
    "中文",
    "👩🏽‍💻",
    "\u{feff}",
    "x\r\ny",
];

/// Owns the original bytes and plays the I/O worker: every Pending ticket is served.
struct Pages {
    bytes: Vec<u8>,
    publisher: SourcePublisher,
    generation: Generation,
    page_size: usize,
}
impl Pages {
    fn serve(&self, ticket: PageTicket) {
        assert_eq!(ticket.generation, self.generation, "ticket for a foreign source");
        let start = usize::try_from(ticket.page).unwrap() * self.page_size;
        let end = (start + self.page_size).min(self.bytes.len());
        self.publisher
            .publish(ticket, &self.bytes[start..end], self.generation)
            .unwrap();
    }
}
fn finish(request: &mut WindowRequest, pages: &Pages) -> TextWindow {
    for _ in 0..1_000_000 {
        match request.poll() {
            WindowPoll::Ready(window) => return window,
            WindowPoll::Pending(ticket) => pages.serve(ticket),
            WindowPoll::Unavailable(reason) => panic!("source unavailable: {reason:?}"),
            WindowPoll::InvalidUtf8 => panic!("window split a scalar"),
            WindowPoll::Finished => panic!("window request finished without text"),
        }
    }
    panic!("window request made no progress");
}
fn window(snapshot: &PagedSnapshot, pages: &Pages, range: Range<usize>, budget: &Budget) -> TextWindow {
    let mut request = snapshot
        .begin_read(TextOffset(range.start)..TextOffset(range.end), range.len(), budget)
        .unwrap();
    finish(&mut request, pages)
}
fn lookup(request: &mut LineLookupRequest, pages: &Pages) -> LineLookupPoll {
    for _ in 0..1_000_000 {
        match request.poll() {
            LineLookupPoll::Pending(ticket) => pages.serve(ticket),
            LineLookupPoll::Progress(_) => {}
            other => return other,
        }
    }
    panic!("line lookup made no progress");
}

fn boundaries(text: &str) -> Vec<usize> {
    text.char_indices().map(|(i, _)| i).chain(Some(text.len())).collect()
}
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
/// Terminators in `text[..end]` (CRLF counts once) and whether it ends with CR.
fn prefix_breaks(text: &str, end: usize) -> (usize, bool) {
    let mut breaks = 0;
    let mut previous_cr = false;
    for &byte in &text.as_bytes()[..end] {
        if byte == b'\r' || (byte == b'\n' && !previous_cr) {
            breaks += 1;
        }
        previous_cr = byte == b'\r';
    }
    (breaks, previous_cr)
}
fn common_boundary_prefix(old: &str, new: &str) -> usize {
    let mut at = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(at) || !new.is_char_boundary(at) {
        at -= 1;
    }
    at
}

struct Harness {
    rng: Rng,
    pages: Pages,
    document: PagedDocument,
    index: SparseLineIndex,
    text: String,
    undo: Vec<(String, String)>,
    redo: Vec<(String, String)>,
    revision: u64,
    window_bytes: usize,
    windows: Budget,
    context: String,
}
impl Harness {
    fn new(seed: u64, bytes: &Budget, history: &Budget, windows: &Budget) -> Self {
        let mut rng = Rng(seed);
        let text: String = (0..rng.below(48)).map(|_| *rng.pick(PIECES)).collect();
        let page_size = *rng.pick(&[1usize, 3, 7, 16, 64]);
        let cache_limit = page_size * (1 + rng.below(3));
        let generation = Generation(seed);
        let (source, publisher) = MemorySource::new(
            text.len() as u64,
            generation,
            SourceKind::Paged,
            page_size,
            cache_limit,
            bytes.clone(),
        )
        .unwrap();
        let snapshot = PagedSnapshot::utf8(source, 0).unwrap();
        let window_bytes = 4 + rng.below(29);
        let index = SparseLineIndex::new(snapshot.clone(), 2 + rng.below(6), window_bytes, windows).unwrap();
        Self {
            rng,
            pages: Pages {
                bytes: text.clone().into_bytes(),
                publisher,
                generation,
                page_size,
            },
            document: PagedDocument::new(snapshot, bytes.clone(), history.clone()),
            index,
            text,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            window_bytes,
            windows: windows.clone(),
            context: String::new(),
        }
    }
    fn covering_window(&self, range: &Range<usize>) -> TextWindow {
        // Interior insertions need a nonempty surrounding window to prove the boundary.
        let points = boundaries(&self.text);
        let start = points
            .iter()
            .rev()
            .find(|p| **p < range.start)
            .copied()
            .unwrap_or(range.start);
        let end = points.iter().find(|p| **p > range.end).copied().unwrap_or(range.end);
        window(&self.document.snapshot(), &self.pages, start..end, &self.windows)
    }
    fn edit(&mut self) {
        let points = boundaries(&self.text);
        let mut picks: Vec<usize> = (0..2 + 2 * self.rng.below(2))
            .map(|_| *self.rng.pick(&points))
            .collect();
        picks.sort_unstable();
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        for pair in picks.chunks(2) {
            if edits.last().is_some_and(|(previous, _)| previous.start == pair[0]) {
                continue;
            }
            let insert = if self.text.len() > 320 && self.rng.chance(70) {
                String::new()
            } else {
                (0..self.rng.below(4)).map(|_| *self.rng.pick(PIECES)).collect()
            };
            edits.push((pair[0]..pair[1], insert));
        }
        let windows: Vec<TextWindow> = edits.iter().map(|(range, _)| self.covering_window(range)).collect();
        let mut expected = self.text.clone();
        for (range, insert) in edits.iter().rev() {
            expected.replace_range(range.clone(), insert);
        }
        let transaction = EditTransaction {
            base_revision: Revision(self.revision),
            edits: edits
                .iter()
                .map(|(range, insert)| Edit {
                    range: TextOffset(range.start)..TextOffset(range.end),
                    insert: insert.clone(),
                })
                .collect(),
        };
        let revision = self
            .document
            .apply_materialized(transaction, &windows)
            .unwrap_or_else(|error| panic!("{}: edit {edits:?} failed: {error:?}", self.context));
        self.revision += 1;
        assert_eq!(revision, Revision(self.revision), "{}", self.context);
        let first_changed = edits[0].0.start;
        let before = std::mem::replace(&mut self.text, expected.clone());
        self.undo.push((before, expected));
        self.redo.clear();
        self.reindex(first_changed);
    }
    fn history(&mut self, undo: bool) {
        let entry = if undo { self.undo.pop() } else { self.redo.pop() };
        let result = if undo {
            self.document.undo()
        } else {
            self.document.redo()
        };
        let Some((before, after)) = entry else {
            assert_eq!(result, Err(Error::EmptyHistory), "{}", self.context);
            return;
        };
        self.revision += 1;
        assert_eq!(result, Ok(Revision(self.revision)), "{}: undo={undo}", self.context);
        let next = if undo { before.clone() } else { after.clone() };
        let first_changed = common_boundary_prefix(&self.text, &next);
        self.text = next;
        if undo {
            self.redo.push((before, after));
        } else {
            self.undo.push((before, after));
        }
        self.reindex(first_changed);
    }
    fn reindex(&mut self, first_changed: usize) {
        self.index
            .invalidate_after_edit(self.document.snapshot(), TextOffset(first_changed))
            .unwrap();
        assert!(self.index.scanned_to().0 <= first_changed, "{}", self.context);
    }
    fn rejects_unproven_edits(&mut self) {
        let len = self.text.len();
        let edit = |range: Range<usize>| EditTransaction {
            base_revision: Revision(self.revision),
            edits: vec![Edit {
                range: TextOffset(range.start)..TextOffset(range.end),
                insert: "x".into(),
            }],
        };
        if len > 0 {
            // No window of this content state covers the edit.
            assert_eq!(
                self.document.apply_materialized(edit(0..len), &[]),
                Err(Error::IncompleteSource),
                "{}",
                self.context
            );
        }
        assert_eq!(
            self.document.apply_materialized(edit(0..len + 1), &[]),
            Err(Error::OutOfBounds),
            "{}",
            self.context
        );
        let interior: Vec<usize> = (0..len).filter(|i| !self.text.is_char_boundary(*i)).collect();
        if !interior.is_empty() {
            let at = *self.rng.pick(&interior);
            let proof = window(&self.document.snapshot(), &self.pages, 0..len, &self.windows);
            assert_eq!(
                self.document
                    .apply_materialized(edit(at..at), std::slice::from_ref(&proof)),
                Err(Error::InvalidBoundary),
                "{}",
                self.context
            );
        }
    }
    fn scan(&mut self) {
        let snapshot = self.document.snapshot();
        while self.index.scanned_to().0 < self.text.len() {
            let start = self.index.scanned_to().0;
            let mut end = (start + self.window_bytes).min(self.text.len());
            while !self.text.is_char_boundary(end) {
                end -= 1;
            }
            let next = window(&snapshot, &self.pages, start..end, &self.windows);
            assert_eq!(self.index.observe(&next), Ok(()), "{}", self.context);
        }
        let lines = line_starts(&self.text).len();
        assert_eq!(self.index.line_count(), LineCount::Known(lines), "{}", self.context);
    }
    fn lookups(&mut self) {
        let starts = line_starts(&self.text);
        let len = self.text.len();
        for _ in 0..3 {
            let offset = self.rng.below(len + 1);
            let mut request = self
                .index
                .lookup(LineTarget::Byte(TextOffset(offset)), self.windows.clone())
                .unwrap();
            match lookup(&mut request, &self.pages) {
                LineLookupPoll::Line(line) if self.text.is_char_boundary(offset) => assert_eq!(
                    line,
                    starts.partition_point(|start| *start <= offset) - 1,
                    "{}: byte {offset}",
                    self.context
                ),
                LineLookupPoll::Failed(Error::InvalidBoundary) if !self.text.is_char_boundary(offset) => {}
                other => panic!("{}: byte {offset} lookup returned {other:?}", self.context),
            }
            assert_eq!(self.index.retain_lookup_progress(&request), Ok(()), "{}", self.context);
            let line = self.rng.below(starts.len() + 1);
            let mut request = self.index.lookup(LineTarget::Line(line), self.windows.clone()).unwrap();
            match lookup(&mut request, &self.pages) {
                LineLookupPoll::Range(range) if line < starts.len() => {
                    let end = starts.get(line + 1).copied().unwrap_or(len);
                    assert_eq!(
                        range,
                        TextOffset(starts[line])..TextOffset(end),
                        "{}: line {line}",
                        self.context
                    );
                }
                LineLookupPoll::Failed(Error::OutOfBounds) if line == starts.len() => {}
                other => panic!("{}: line {line} lookup returned {other:?}", self.context),
            }
            assert_eq!(self.index.retain_lookup_progress(&request), Ok(()), "{}", self.context);
        }
        assert!(matches!(
            self.index
                .lookup(LineTarget::Byte(TextOffset(len + 1)), self.windows.clone()),
            Err(Error::OutOfBounds)
        ));
    }
    fn check(&mut self) {
        let snapshot = self.document.snapshot();
        let len = self.text.len();
        assert_eq!(snapshot.len(), len, "{}: len", self.context);
        assert_eq!(snapshot.revision, Revision(self.revision), "{}", self.context);
        let whole = window(&snapshot, &self.pages, 0..len, &self.windows);
        assert!(whole.matches_snapshot(&snapshot));
        assert_eq!(whole.text(), self.text, "{}: text", self.context);
        if let LineCount::Known(lines) = snapshot.line_count() {
            assert_eq!(lines, line_starts(&self.text).len(), "{}: line count", self.context);
        }
        assert_eq!(self.document.can_undo(), !self.undo.is_empty(), "{}", self.context);
        assert_eq!(self.document.can_redo(), !self.redo.is_empty(), "{}", self.context);
        // A viewport may start inside a scalar and may end inside one; it trims to text.
        let start = self.rng.below(len + 1);
        let max = 7 + self.rng.below(34);
        let mut request = snapshot.begin_viewport(TextOffset(start), max, &self.windows).unwrap();
        let view = finish(&mut request, &self.pages);
        let range = view.range();
        assert!(
            (start..=start + 3).contains(&range.start.0),
            "{}: {range:?}",
            self.context
        );
        assert!(range.end.0 <= (start + max).min(len), "{}: {range:?}", self.context);
        assert_eq!(
            self.text.get(range.start.0..range.end.0),
            Some(view.text()),
            "{}: viewport {range:?}",
            self.context
        );
        for _ in 0..8 {
            let offset = self.rng.below(len + 1);
            let checkpoint = self.index.checkpoint_before(TextOffset(offset)).unwrap();
            assert!(checkpoint.offset.0 <= offset, "{}", self.context);
            let (breaks, preceding_cr) = prefix_breaks(&self.text, checkpoint.offset.0);
            assert_eq!(
                (checkpoint.breaks, checkpoint.preceding_cr),
                (breaks, preceding_cr),
                "{}: checkpoint {checkpoint:?}",
                self.context
            );
        }
        assert_eq!(
            self.index.checkpoint_before(TextOffset(len + 1)).map(|c| c.offset),
            Err(Error::OutOfBounds)
        );
    }
}

#[test]
fn paged_windows_checkpoints_and_lookups_match_string_oracle() {
    for seed in [0x9a6e_0001u64, 0x9a6e_0002, 0x9a6e_0003, 0x9a6e_0004] {
        let bytes = Budget::new(16 << 20);
        let history = Budget::new(16 << 20);
        let windows = Budget::new(16 << 20);
        let mut harness = Harness::new(seed, &bytes, &history, &windows);
        for step in 0..120 {
            harness.context = format!("seed {seed:#x} step {step}");
            match harness.rng.below(100) {
                0..=39 => harness.edit(),
                40..=54 => harness.history(true),
                55..=69 => harness.history(false),
                70..=79 => harness.rejects_unproven_edits(),
                80..=89 => harness.scan(),
                _ => harness.lookups(),
            }
            harness.check();
        }
        harness.scan();
        harness.lookups();
        // A cancelled index refuses further windows without losing its checkpoints.
        harness.index.cancel();
        let snapshot = harness.document.snapshot();
        let empty = window(&snapshot, &harness.pages, 0..0, &harness.windows);
        assert_eq!(harness.index.observe(&empty), Err(IndexError::Cancelled));
        drop(empty);
        drop(snapshot);
        drop(harness);
        assert_eq!(bytes.used(), 0, "seed {seed:#x}: text/page budget leaked");
        assert_eq!(history.used(), 0, "seed {seed:#x}: history budget leaked");
        assert_eq!(windows.used(), 0, "seed {seed:#x}: window budget leaked");
    }
}
