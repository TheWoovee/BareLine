// SPDX-License-Identifier: MPL-2.0
//! Bounded UTF-8 windows over a generation-aware source. Raw legacy bytes must first
//! pass through a transcoder; byte offsets here address the UTF-8 text view only.
use crate::{
    Budget, ContentStateId, EditTransaction, Error, Reservation, Revision, TextOffset,
    source::{MemorySource, PageTicket, SourceRead, Unavailable},
    tree,
};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineCount {
    Known(usize),
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineCheckpoint {
    pub offset: TextOffset,
    /// Terminators in the scanned prefix; a CR followed by the next LF counts once.
    pub breaks: usize,
    pub preceding_cr: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexError {
    StaleSnapshot,
    OutOfOrder,
    Cancelled,
    WindowTooLarge,
}
/// A bounded checkpoint index populated by sequential window reads on a worker.
/// Sparse navigation starts at the nearest retained checkpoint and refines via Pending reads.
pub struct SparseLineIndex {
    snapshot: PagedSnapshot,
    checkpoints: Vec<LineCheckpoint>,
    capacity: usize,
    max_window_bytes: usize,
    progress: LineCheckpoint,
    cancelled: bool,
    _reservation: Reservation,
}
impl SparseLineIndex {
    pub fn new(
        snapshot: PagedSnapshot,
        max_checkpoints: usize,
        max_window_bytes: usize,
        budget: &Budget,
    ) -> Result<Self, Error> {
        if max_checkpoints < 2 || max_window_bytes == 0 {
            return Err(Error::BudgetExceeded);
        }
        let reservation = budget.reserve(
            max_checkpoints
                .checked_mul(std::mem::size_of::<LineCheckpoint>())
                .ok_or(Error::BudgetExceeded)?,
        )?;
        let progress = LineCheckpoint {
            offset: TextOffset(0),
            breaks: 0,
            preceding_cr: false,
        };
        let mut checkpoints = Vec::with_capacity(max_checkpoints);
        checkpoints.push(progress);
        Ok(Self {
            snapshot,
            checkpoints,
            capacity: max_checkpoints,
            max_window_bytes,
            progress,
            cancelled: false,
            _reservation: reservation,
        })
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }
    pub fn reset(&mut self, snapshot: PagedSnapshot) {
        self.snapshot = snapshot;
        self.progress = LineCheckpoint {
            offset: TextOffset(0),
            breaks: 0,
            preceding_cr: false,
        };
        self.checkpoints.clear();
        self.checkpoints.push(self.progress);
        self.cancelled = false;
    }
    pub fn scanned_to(&self) -> TextOffset {
        self.progress.offset
    }
    pub fn line_count(&self) -> LineCount {
        if self.progress.offset.0 == self.snapshot.len() {
            LineCount::Known(self.progress.breaks + 1)
        } else {
            LineCount::Unknown
        }
    }
    pub fn checkpoint_before(&self, offset: TextOffset) -> Result<LineCheckpoint, Error> {
        if offset.0 > self.snapshot.len() {
            return Err(Error::OutOfBounds);
        }
        let at = self
            .checkpoints
            .partition_point(|checkpoint| checkpoint.offset <= offset);
        Ok(self.checkpoints[at.saturating_sub(1)])
    }
    /// At most max_window_bytes are inspected, and no per-line allocations occur.
    pub fn observe(&mut self, window: &TextWindow) -> Result<(), IndexError> {
        if self.cancelled {
            return Err(IndexError::Cancelled);
        }
        if window.document_id != self.snapshot.document_id
            || window.content_state != self.snapshot.content_state
        {
            return Err(IndexError::StaleSnapshot);
        }
        if window.range.start != self.progress.offset {
            return Err(IndexError::OutOfOrder);
        }
        if window.text.len() > self.max_window_bytes {
            return Err(IndexError::WindowTooLarge);
        }
        for byte in window.text.bytes() {
            if byte == b'\r' || (byte == b'\n' && !self.progress.preceding_cr) {
                self.progress.breaks += 1;
            }
            self.progress.preceding_cr = byte == b'\r';
        }
        self.progress.offset = window.range.end;
        if self
            .checkpoints
            .last()
            .is_some_and(|checkpoint| checkpoint.offset == self.progress.offset)
        {
            return Ok(());
        }
        if self.checkpoints.len() == self.capacity {
            self.checkpoints.remove(1);
        }
        self.checkpoints.push(self.progress);
        Ok(())
    }
}
#[derive(Clone)]
pub struct PagedSnapshot {
    root: tree::Root,
    pub revision: Revision,
    pub content_state: ContentStateId,
    document_id: u64,
    _structure: Option<std::sync::Arc<crate::BudgetClaim>>,
}
impl PagedSnapshot {
    pub fn same_document(&self, other: &Self) -> bool { self.document_id == other.document_id }
    pub fn pieces(&self) -> Pieces<'_> {
        Pieces {
            stack: self.root.as_deref().into_iter().collect(),
        }
    }
    /// `text_start` omits an already-detected UTF-8 BOM. Every returned window is
    /// strictly validated; malformed input reports InvalidUtf8, never replacement text.
    pub fn utf8(source: MemorySource, text_start: u64) -> Result<Self, Error> {
        let length = source
            .len()
            .checked_sub(text_start)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(Error::OutOfBounds)?;
        let end = source.len();
        let _ = length;
        Ok(Self {
            root: tree::from_source(source, text_start..end),
            revision: Revision(0),
            content_state: ContentStateId(crate::unique()),
            document_id: crate::unique(),
            _structure: None,
        })
    }
    pub fn len(&self) -> usize {
        tree::summary(&self.root).bytes
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn line_count(&self) -> LineCount {
        let summary = tree::summary(&self.root);
        if !summary.unknown {
            LineCount::Known(summary.breaks + 1)
        } else {
            LineCount::Unknown
        }
    }
    /// Owns one bounded window allocation, including while waiting for source pages.
    /// Keep the request across Pending responses; this permits single-page caches.
    pub fn begin_read(
        &self,
        range: Range<TextOffset>,
        max_bytes: usize,
        budget: &Budget,
    ) -> Result<WindowRequest, Error> {
        if range.start > range.end || range.end.0 > self.len() {
            return Err(Error::OutOfBounds);
        }
        let length = range.end.0 - range.start.0;
        if length > max_bytes {
            return Err(Error::BudgetExceeded);
        }
        let reservation = budget.reserve(length)?;
        Ok(WindowRequest {
            snapshot: self.clone(),
            cursor: range.start.0,
            end: range.end.0,
            range,
            bytes: Some(Vec::with_capacity(length)),
            reservation: Some(reservation),
            align_edges: false,
        })
    }
    /// A display window may trim at most three continuation bytes at either edge.
    /// Interior malformed input and truncated scalars at actual EOF still fail.
    pub fn begin_viewport(
        &self,
        start: TextOffset,
        max_bytes: usize,
        budget: &Budget,
    ) -> Result<WindowRequest, Error> {
        let end = start
            .0
            .checked_add(max_bytes)
            .unwrap_or(self.len())
            .min(self.len());
        let mut request = self.begin_read(start..TextOffset(end), max_bytes, budget)?;
        request.align_edges = true;
        Ok(request)
    }
}
pub enum PagedPiece<'a> {
    Original {
        source: &'a MemorySource,
        range: Range<u64>,
    },
    Inserted(&'a str),
    OriginalOwned {
        source: &'a MemorySource,
        range: Range<u64>,
        text: &'a str,
    },
}
/// Streams leaf provenance with only tree-height scratch storage, without source reads.
pub struct Pieces<'a> {
    stack: Vec<&'a tree::Node>,
}
impl<'a> Iterator for Pieces<'a> {
    type Item = PagedPiece<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.stack.pop() {
            match node {
                tree::Node::Source { source, range } => {
                    return Some(PagedPiece::Original {
                        source,
                        range: range.clone(),
                    });
                }
                tree::Node::Leaf(piece) => {
                    return Some(match piece.origin() {
                        Some((source, range)) => PagedPiece::OriginalOwned {
                            source,
                            range,
                            text: piece.text(),
                        },
                        None => PagedPiece::Inserted(piece.text()),
                    });
                }
                tree::Node::Branch { left, right, .. } => {
                    self.stack.push(right);
                    self.stack.push(left);
                }
            }
        }
        None
    }
}
pub struct TextWindow {
    range: Range<TextOffset>,
    text: String,
    content_state: ContentStateId,
    document_id: u64,
    _reservation: Reservation,
}
impl TextWindow {
    pub fn matches_snapshot(&self, snapshot: &PagedSnapshot) -> bool {
        self.document_id == snapshot.document_id && self.content_state == snapshot.content_state
    }
    pub fn range(&self) -> Range<TextOffset> {
        self.range.clone()
    }
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Owned bytes for journal consumers; never depends on a live source page.
pub enum RestoredPiece { Original(Range<u64>), Inserted(String) }
pub struct OwnedDelta {
    pub range: Range<TextOffset>,
    pub removed: String,
    pub inserted: String,
}
struct PagedHistory {
    edits: Vec<OwnedEdit>,
    before_state: ContentStateId,
    after_state: ContentStateId,
    _reservation: Reservation,
}
struct OwnedEdit {
    before_range: Range<usize>,
    after_range: Range<usize>,
    inverse: tree::Root,
    inserted: tree::Root,
}
/// Source-backed edits share the same balanced piece tree as Resident documents.
/// Callers materialize bounded windows before submitting edits; no actor lock spans I/O.
pub struct PagedDocument {
    current: PagedSnapshot,
    bytes: Budget,
    history: Budget,
    undo: Vec<PagedHistory>,
    redo: Vec<PagedHistory>,
}
impl PagedDocument {
    pub fn new(snapshot: PagedSnapshot, bytes: Budget, history: Budget) -> Self {
        Self {
            current: snapshot,
            bytes,
            history,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
    pub fn snapshot(&self) -> PagedSnapshot {
        self.current.clone()
    }
    /// Every edited range must be covered by an owned window of this content state.
    /// An interior insertion needs a nonempty surrounding window to prove its boundary.
    pub fn apply_materialized(
        &mut self,
        mut transaction: EditTransaction,
        windows: &[TextWindow],
    ) -> Result<Revision, Error> {
        if transaction.base_revision != self.current.revision {
            return Err(Error::StaleRevision);
        }
        if transaction.edits.is_empty() {
            return Ok(self.current.revision);
        }
        transaction
            .edits
            .sort_by_key(|edit| (edit.range.start, edit.range.end));
        let mut inverse = Vec::with_capacity(transaction.edits.len());
        let mut undo_bytes = 0usize;
        for (index, edit) in transaction.edits.iter().enumerate() {
            if edit.range.start > edit.range.end || edit.range.end.0 > self.current.len() {
                return Err(Error::OutOfBounds);
            }
            if index > 0 {
                let previous = &transaction.edits[index - 1].range;
                if previous.end > edit.range.start || previous.start == edit.range.start {
                    return Err(Error::OverlappingEdits);
                }
            }
            let window = windows
                .iter()
                .find(|window| {
                    window.document_id == self.current.document_id
                        && window.content_state == self.current.content_state
                        && window.range.start <= edit.range.start
                        && window.range.end >= edit.range.end
                })
                .ok_or(Error::IncompleteSource)?;
            let start = edit.range.start.0 - window.range.start.0;
            let end = edit.range.end.0 - window.range.start.0;
            if !window.text.is_char_boundary(start)
                || !window.text.is_char_boundary(end)
                || (window.text.is_empty()
                    && edit.range.start.0 != 0
                    && edit.range.start.0 != self.current.len())
            {
                return Err(Error::InvalidBoundary);
            }
            undo_bytes = undo_bytes
                .checked_add(end - start)
                .and_then(|n| n.checked_add(edit.insert.len()))
                .ok_or(Error::BudgetExceeded)?;
            inverse.push(tree::own_inverse(
                &self.current.root,
                edit.range.start.0..edit.range.end.0,
                &window.text[start..end],
                &self.bytes,
            )?);
        }
        let revision = Revision(
            self.current
                .revision
                .0
                .checked_add(1)
                .ok_or(Error::RevisionOverflow)?,
        );
        let reservation = self.history.reserve(undo_bytes.max(1))?;
        let inserts = transaction
            .edits
            .iter()
            .map(|edit| tree::from_text(&edit.insert, &self.bytes))
            .collect::<Result<Vec<_>, _>>()?;
        let mut after = self.current.root.clone();
        let mut owned_edits = Vec::with_capacity(transaction.edits.len());
        let mut before_cursor = 0;
        let mut after_cursor: usize = 0;
        for ((edit, inverse), inserted) in transaction.edits.iter().zip(inverse).zip(inserts) {
            let start = after_cursor
                .checked_add(edit.range.start.0 - before_cursor)
                .ok_or(Error::BudgetExceeded)?;
            let end = start
                .checked_add(edit.insert.len())
                .ok_or(Error::BudgetExceeded)?;
            owned_edits.push(OwnedEdit {
                before_range: edit.range.start.0..edit.range.end.0,
                after_range: start..end,
                inverse,
                inserted,
            });
            before_cursor = edit.range.end.0;
            after_cursor = end;
        }
        for edit in owned_edits.iter().rev() {
            after = replace_root(after, edit.before_range.clone(), edit.inserted.clone());
        }
        let state = ContentStateId(crate::unique());
        self.undo.push(PagedHistory {
            edits: owned_edits,
            before_state: self.current.content_state,
            after_state: state,
            _reservation: reservation,
        });
        self.redo.clear();
        self.current.root = after;
        self.current.revision = revision;
        self.current.content_state = state;
        Ok(revision)
    }
    /// Rebuild a validated recovery recipe with source provenance intact.
    pub fn restore_pieces(source: MemorySource, pieces: Vec<RestoredPiece>, bytes: Budget, history: Budget, revision: Revision) -> Result<Self, Error> {
        let mut snapshot = PagedSnapshot::utf8(source.clone(), 0)?;
        snapshot._structure = Some(std::sync::Arc::new(bytes.claim(pieces.len().checked_mul(2 * std::mem::size_of::<tree::Node>()).ok_or(Error::BudgetExceeded)?)?));
        let mut root = None;
        for piece in pieces {
            let next = match piece {
                RestoredPiece::Original(range) => { if range.start > range.end || range.end > source.len() { return Err(Error::OutOfBounds); } tree::from_source(source.clone(), range) },
                RestoredPiece::Inserted(text) => tree::from_text(&text, &bytes)?,
            };
            root = tree::concat(root, next);
        }
        snapshot.root = root;
        snapshot.revision = revision;
        Ok(Self::new(snapshot, bytes, history))
    }
    pub fn can_undo(&self) -> bool { !self.undo.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo.is_empty() }
    pub fn history_delta(&self, undo: bool) -> Result<Vec<OwnedDelta>, Error> {
        let entry = if undo { self.undo.last() } else { self.redo.last() }.ok_or(Error::EmptyHistory)?;
        let owned_text = |root: &tree::Root| tree::chunks(root, 0..tree::summary(root).bytes).collect::<String>();
        Ok(entry.edits.iter().map(|edit| {
            let range = if undo { &edit.after_range } else { &edit.before_range };
            OwnedDelta { range: TextOffset(range.start)..TextOffset(range.end), removed: owned_text(if undo { &edit.inserted } else { &edit.inverse }), inserted: owned_text(if undo { &edit.inverse } else { &edit.inserted }) }
        }).collect())
    }
    pub fn undo(&mut self) -> Result<Revision, Error> {
        let revision = Revision(
            self.current
                .revision
                .0
                .checked_add(1)
                .ok_or(Error::RevisionOverflow)?,
        );
        let entry = self.undo.pop().ok_or(Error::EmptyHistory)?;
        for edit in entry.edits.iter().rev() {
            self.current.root = replace_root(
                self.current.root.clone(),
                edit.after_range.clone(),
                edit.inverse.clone(),
            );
        }
        self.current.content_state = entry.before_state;
        self.current.revision = revision;
        self.redo.push(entry);
        Ok(revision)
    }
    pub fn redo(&mut self) -> Result<Revision, Error> {
        let revision = Revision(
            self.current
                .revision
                .0
                .checked_add(1)
                .ok_or(Error::RevisionOverflow)?,
        );
        let entry = self.redo.pop().ok_or(Error::EmptyHistory)?;
        for edit in entry.edits.iter().rev() {
            self.current.root = replace_root(
                self.current.root.clone(),
                edit.before_range.clone(),
                edit.inserted.clone(),
            );
        }
        self.current.content_state = entry.after_state;
        self.current.revision = revision;
        self.undo.push(entry);
        Ok(revision)
    }
}
fn replace_root(root: tree::Root, range: Range<usize>, inserted: tree::Root) -> tree::Root {
    let (prefix, suffix) = tree::split(root, range.end);
    let (prefix, _) = tree::split(prefix, range.start);
    tree::concat(tree::concat(prefix, inserted), suffix)
}
pub enum WindowPoll {
    Ready(TextWindow),
    Pending(PageTicket),
    Unavailable(Unavailable),
    /// The requested endpoints split a scalar, or source bytes are not valid UTF-8.
    InvalidUtf8,
    Finished,
}
pub struct WindowRequest {
    snapshot: PagedSnapshot,
    cursor: usize,
    end: usize,
    range: Range<TextOffset>,
    bytes: Option<Vec<u8>>,
    reservation: Option<Reservation>,
    align_edges: bool,
}
impl WindowRequest {
    /// Nonblocking, bounded by the requested byte count; never reads from disk.
    pub fn poll(&mut self) -> WindowPoll {
        let Some(bytes) = self.bytes.as_mut() else {
            return WindowPoll::Finished;
        };
        while self.cursor < self.end {
            let Some(span) = tree::span_at(&self.snapshot.root, self.cursor) else {
                return WindowPoll::Finished;
            };
            let (source, range) = match span {
                tree::Span::Owned(owned) => {
                    let count = owned.len().min(self.end - self.cursor);
                    bytes.extend_from_slice(&owned[..count]);
                    self.cursor += count;
                    continue;
                }
                tree::Span::Source(source, range) => (source, range),
            };
            let page_remaining =
                source.page_size() as u64 - range.start % source.page_size() as u64;
            let count = page_remaining
                .min(range.end - range.start)
                .min((self.end - self.cursor) as u64);
            match source.read(range.start..range.start + count) {
                SourceRead::Ready(page) => {
                    bytes.extend_from_slice(page.bytes());
                    self.cursor += count as usize;
                }
                SourceRead::Pending(ticket) => return WindowPoll::Pending(ticket),
                SourceRead::Unavailable(reason) => {
                    self.bytes = None;
                    self.reservation = None;
                    return WindowPoll::Unavailable(reason);
                }
            }
        }
        let mut bytes = self.bytes.take().expect("active request");
        if self.align_edges {
            if self.range.start.0 != 0 {
                let skip = bytes
                    .iter()
                    .take(3)
                    .take_while(|byte| **byte & 0xc0 == 0x80)
                    .count();
                bytes.drain(..skip);
                self.range.start.0 += skip;
            }
            if self.range.end.0 < self.snapshot.len()
                && let Err(error) = std::str::from_utf8(&bytes)
                && error.error_len().is_none()
            {
                self.range.end.0 -= bytes.len() - error.valid_up_to();
                bytes.truncate(error.valid_up_to());
            }
        }
        match String::from_utf8(bytes) {
            Ok(text) => WindowPoll::Ready(TextWindow {
                range: self.range.clone(),
                text,
                content_state: self.snapshot.content_state,
                document_id: self.snapshot.document_id,
                _reservation: self.reservation.take().expect("window budget"),
            }),
            Err(_) => {
                self.reservation = None;
                WindowPoll::InvalidUtf8
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{Generation, SourceKind};
    fn ready(snapshot: &PagedSnapshot, start: usize, end: usize, budget: &Budget) -> TextWindow {
        let mut request = snapshot
            .begin_read(TextOffset(start)..TextOffset(end), end - start, budget)
            .unwrap();
        match request.poll() {
            WindowPoll::Ready(window) => window,
            _ => panic!("expected owned/ready bytes"),
        }
    }
    #[test]
    fn sparse_index_is_bounded_crlf_aware_cancellable_and_state_scoped() {
        let budget = Budget::new(2048);
        let (source, publisher) =
            MemorySource::new(8, Generation(9), SourceKind::Paged, 8, 8, budget.clone()).unwrap();
        publisher
            .publish(
                PageTicket {
                    generation: Generation(9),
                    page: 0,
                },
                b"a\r\nb\nc\r\n",
                Generation(9),
            )
            .unwrap();
        let snapshot = PagedSnapshot::utf8(source, 0).unwrap();
        let mut index = SparseLineIndex::new(snapshot.clone(), 2, 4, &budget).unwrap();
        assert_eq!(index.line_count(), LineCount::Unknown);
        index.observe(&ready(&snapshot, 0, 2, &budget)).unwrap();
        index.observe(&ready(&snapshot, 2, 5, &budget)).unwrap();
        index.observe(&ready(&snapshot, 5, 8, &budget)).unwrap();
        assert_eq!(index.line_count(), LineCount::Known(4));
        assert_eq!(index.checkpoints.len(), 2);
        index.cancel();
        assert_eq!(
            index.observe(&ready(&snapshot, 8, 8, &budget)),
            Err(IndexError::Cancelled)
        );
        let mut document = PagedDocument::new(snapshot.clone(), budget.clone(), Budget::new(1024));
        let window = ready(&snapshot, 0, 2, &budget);
        document
            .apply_materialized(
                EditTransaction {
                    base_revision: Revision(0),
                    edits: vec![crate::Edit {
                        range: TextOffset(0)..TextOffset(1),
                        insert: "x".into(),
                    }],
                },
                std::slice::from_ref(&window),
            )
            .unwrap();
        index.reset(document.snapshot());
        assert_eq!(index.observe(&window), Err(IndexError::StaleSnapshot));
    }
    #[test]
    fn deleted_inverse_survives_eviction_change_and_multiple_undo_redo() {
        let budget = Budget::new(1024);
        let (source, publisher) =
            MemorySource::new(8, Generation(1), SourceKind::Paged, 4, 4, budget.clone()).unwrap();
        let mut document = PagedDocument::new(
            PagedSnapshot::utf8(source, 0).unwrap(),
            budget.clone(),
            Budget::new(1024),
        );
        publisher
            .publish(
                PageTicket {
                    generation: Generation(1),
                    page: 0,
                },
                b"abcd",
                Generation(1),
            )
            .unwrap();
        let window = ready(&document.snapshot(), 0, 4, &budget);
        document
            .apply_materialized(
                EditTransaction {
                    base_revision: Revision(0),
                    edits: vec![crate::Edit {
                        range: TextOffset(0)..TextOffset(4),
                        insert: String::new(),
                    }],
                },
                &[window],
            )
            .unwrap();
        publisher
            .publish(
                PageTicket {
                    generation: Generation(1),
                    page: 1,
                },
                b"efgh",
                Generation(1),
            )
            .unwrap();
        let window = ready(&document.snapshot(), 0, 4, &budget);
        document
            .apply_materialized(
                EditTransaction {
                    base_revision: Revision(1),
                    edits: vec![crate::Edit {
                        range: TextOffset(0)..TextOffset(4),
                        insert: String::new(),
                    }],
                },
                &[window],
            )
            .unwrap();
        publisher.mark_changed();
        document.undo().unwrap();
        assert_eq!(ready(&document.snapshot(), 0, 4, &budget).text(), "efgh");
        document.undo().unwrap();
        assert_eq!(
            ready(&document.snapshot(), 0, 8, &budget).text(),
            "abcdefgh"
        );
        assert_eq!(document.snapshot().line_count(), LineCount::Known(1));
        document.redo().unwrap();
        document.redo().unwrap();
        assert!(document.snapshot().is_empty());
        tree::assert_balanced(&document.snapshot().root);
    }
    #[test]
    fn materialized_multi_edit_is_atomic_and_rejects_stale_or_split_boundaries() {
        let budget = Budget::new(2048);
        let (source, publisher) =
            MemorySource::new(8, Generation(2), SourceKind::Paged, 8, 8, budget.clone()).unwrap();
        publisher
            .publish(
                PageTicket {
                    generation: Generation(2),
                    page: 0,
                },
                "a😀b\r\n".as_bytes(),
                Generation(2),
            )
            .unwrap();
        let mut document = PagedDocument::new(
            PagedSnapshot::utf8(source, 0).unwrap(),
            budget.clone(),
            Budget::new(1024),
        );
        let window = ready(&document.snapshot(), 0, 8, &budget);
        assert_eq!(
            document.apply_materialized(
                EditTransaction {
                    base_revision: Revision(0),
                    edits: vec![crate::Edit {
                        range: TextOffset(2)..TextOffset(3),
                        insert: "bad".into()
                    }]
                },
                std::slice::from_ref(&window)
            ),
            Err(Error::InvalidBoundary)
        );
        assert_eq!(document.snapshot().revision, Revision(0));
        document
            .apply_materialized(
                EditTransaction {
                    base_revision: Revision(0),
                    edits: vec![
                        crate::Edit {
                            range: TextOffset(1)..TextOffset(5),
                            insert: "z".into(),
                        },
                        crate::Edit {
                            range: TextOffset(6)..TextOffset(8),
                            insert: "\n".into(),
                        },
                    ],
                },
                std::slice::from_ref(&window),
            )
            .unwrap();
        assert_eq!(ready(&document.snapshot(), 0, 4, &budget).text(), "azb\n");
        assert_eq!(
            document.apply_materialized(
                EditTransaction {
                    base_revision: Revision(1),
                    edits: vec![crate::Edit {
                        range: TextOffset(0)..TextOffset(1),
                        insert: "x".into()
                    }]
                },
                &[window]
            ),
            Err(Error::IncompleteSource)
        );
        document.undo().unwrap();
        assert_eq!(
            ready(&document.snapshot(), 0, 8, &budget).text(),
            "a😀b\r\n"
        );
        document.redo().unwrap();
        assert_eq!(ready(&document.snapshot(), 0, 4, &budget).text(), "azb\n");
    }
    #[test]
    fn window_keeps_owned_prefix_across_eviction_and_split_scalar() {
        let budget = Budget::new(32);
        let (source, publisher) =
            MemorySource::new(8, Generation(1), SourceKind::Paged, 4, 4, budget.clone()).unwrap();
        let snapshot = PagedSnapshot::utf8(source, 0).unwrap();
        assert_eq!(snapshot.line_count(), LineCount::Unknown);
        let mut request = snapshot
            .begin_read(TextOffset(0)..TextOffset(8), 8, &budget)
            .unwrap();
        let WindowPoll::Pending(ticket) = request.poll() else {
            panic!()
        };
        publisher
            .publish(ticket, &[b'a', b'b', 0xf0, 0x9f], Generation(1))
            .unwrap();
        let WindowPoll::Pending(ticket) = request.poll() else {
            panic!()
        };
        publisher
            .publish(ticket, &[0x98, 0x80, b'\r', b'\n'], Generation(1))
            .unwrap();
        let WindowPoll::Ready(window) = request.poll() else {
            panic!()
        };
        assert_eq!(window.text(), "ab😀\r\n");
        assert_eq!(budget.used(), 12);
        assert!(matches!(request.poll(), WindowPoll::Finished));
        drop(window);
        assert_eq!(budget.used(), 4);
    }
    #[test]
    fn missing_generation_and_invalid_utf8_never_yield_a_complete_window() {
        let budget = Budget::new(16);
        let (source, publisher) =
            MemorySource::new(8, Generation(1), SourceKind::Paged, 4, 4, budget.clone()).unwrap();
        let snapshot = PagedSnapshot::utf8(source, 0).unwrap();
        let mut request = snapshot
            .begin_read(TextOffset(0)..TextOffset(8), 8, &budget)
            .unwrap();
        let WindowPoll::Pending(ticket) = request.poll() else {
            panic!()
        };
        publisher.publish(ticket, b"good", Generation(1)).unwrap();
        assert!(matches!(request.poll(), WindowPoll::Pending(_)));
        publisher.mark_changed();
        assert!(matches!(
            request.poll(),
            WindowPoll::Unavailable(Unavailable::SourceChanged)
        ));
        assert_eq!(budget.used(), 4);
        let (source, publisher) =
            MemorySource::new(1, Generation(2), SourceKind::Paged, 1, 1, budget.clone()).unwrap();
        publisher
            .publish(
                PageTicket {
                    generation: Generation(2),
                    page: 0,
                },
                &[255],
                Generation(2),
            )
            .unwrap();
        let mut request = PagedSnapshot::utf8(source, 0)
            .unwrap()
            .begin_read(TextOffset(0)..TextOffset(1), 1, &budget)
            .unwrap();
        assert!(matches!(request.poll(), WindowPoll::InvalidUtf8));
    }
}
