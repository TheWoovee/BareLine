// SPDX-License-Identifier: MPL-2.0
//! Bounded UTF-8 windows over a generation-aware source. Raw legacy bytes must first
//! pass through a transcoder; byte offsets here address the UTF-8 text view only.
pub use crate::source_transaction::{
    HistoryCommitLease, HistorySourceEdit, OwnedTextRange, PreparedSourceHistory,
    PreparedSourceTransaction, SourceCommitLease, SourceEdit, SourceTransactionPoll,
    SourceTransactionRequest,
};
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
    /// Keep the unchanged prefix after an edit; later checkpoints must be rediscovered.
    pub fn invalidate_after_edit(
        &mut self,
        snapshot: PagedSnapshot,
        first_changed: TextOffset,
    ) -> Result<(), Error> {
        if !self.snapshot.same_document(&snapshot) {
            return Err(Error::WrongDocument);
        }
        if first_changed.0 > self.snapshot.len() || first_changed.0 > snapshot.len() {
            return Err(Error::OutOfBounds);
        }
        self.checkpoints
            .retain(|checkpoint| checkpoint.offset <= first_changed);
        self.progress = *self
            .checkpoints
            .last()
            .expect("initial checkpoint retained");
        self.snapshot = snapshot;
        self.cancelled = false;
        Ok(())
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
    /// Start cancellable refinement from the nearest safe retained checkpoint.
    pub fn lookup(
        &self,
        target: crate::line_lookup::LineTarget,
        budget: Budget,
    ) -> Result<crate::line_lookup::LineLookupRequest, Error> {
        let checkpoint = match target {
            crate::line_lookup::LineTarget::Byte(offset) => self.checkpoint_before(offset)?,
            crate::line_lookup::LineTarget::Line(line) => *self
                .checkpoints
                .iter()
                .rev()
                .find(|c| c.breaks < line)
                .unwrap_or(&self.checkpoints[0]),
        };
        crate::line_lookup::LineLookupRequest::new(
            self.snapshot.clone(),
            checkpoint,
            target,
            self.max_window_bytes,
            budget,
        )
    }
    /// Retain a worker lookup's verified prefix without unbounded index growth.
    pub fn retain_lookup_progress(
        &mut self,
        request: &crate::line_lookup::LineLookupRequest,
    ) -> Result<(), IndexError> {
        if self.cancelled {
            return Err(IndexError::Cancelled);
        }
        if !request.matches_snapshot(&self.snapshot) {
            return Err(IndexError::StaleSnapshot);
        }
        let checkpoint = request.verified_checkpoint().ok_or(IndexError::Cancelled)?;
        if checkpoint.offset.0 > self.snapshot.len() {
            return Err(IndexError::OutOfOrder);
        }
        match self
            .checkpoints
            .binary_search_by_key(&checkpoint.offset, |value| value.offset)
        {
            Ok(index) => {
                if self.checkpoints[index] != checkpoint {
                    return Err(IndexError::OutOfOrder);
                }
            }
            Err(_) => {
                if self.checkpoints.len() == self.capacity {
                    self.checkpoints.remove(1);
                }
                let index = self
                    .checkpoints
                    .partition_point(|value| value.offset < checkpoint.offset);
                self.checkpoints.insert(index, checkpoint);
            }
        }
        if checkpoint.offset > self.progress.offset {
            self.progress = checkpoint;
        }
        Ok(())
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
    pub(crate) applied_change: Option<std::sync::Arc<crate::change::AppliedChange>>,
    pub(crate) metadata: crate::DocumentMetadata,
    pub(crate) root: tree::Root,
    pub revision: Revision,
    pub content_state: ContentStateId,
    pub(crate) document_id: u64,
    pub(crate) _structure: Option<std::sync::Arc<crate::BudgetClaim>>,
}
impl PagedSnapshot {
    pub fn applied_change(&self) -> Option<&std::sync::Arc<crate::change::AppliedChange>> {
        self.applied_change.as_ref()
    }
    pub fn metadata(&self) -> &crate::DocumentMetadata {
        &self.metadata
    }
    /// Opaque source token for validating queued external actions; forks have distinct identities.
    pub fn identity_token(&self) -> (u64, u64) {
        (self.document_id, self.revision.0)
    }
    /// A historical/read-only presentation owns a distinct identity while retaining bytes.
    pub fn fork_identity(&self) -> Self {
        let mut snapshot = self.clone();
        snapshot.document_id = crate::unique();
        snapshot.applied_change = None;
        snapshot
    }
    pub fn same_document(&self, other: &Self) -> bool {
        self.document_id == other.document_id
    }
    /// Explicit worker-only owned-page resolution; false routes to the original producer.
    pub fn resolve_owned(&self, ticket: PageTicket) -> Result<bool, Error> {
        for piece in self.pieces() {
            if let PagedPiece::OwnedSource { source, .. } = piece
                && source.generation() == ticket.generation
            {
                return source.resolve_owned(ticket);
            }
        }
        Ok(false)
    }
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
            applied_change: None,
            root: tree::from_source(source, text_start..end),
            revision: Revision(0),
            content_state: ContentStateId(crate::unique()),
            document_id: crate::unique(),
            _structure: None,
            metadata: crate::DocumentMetadata::default(),
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
    OwnedSource {
        source: &'a MemorySource,
        range: Range<u64>,
        original: Option<(&'a MemorySource, Range<u64>)>,
    },
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
                tree::Node::OwnedSource {
                    source,
                    range,
                    original,
                    ..
                } => {
                    return Some(PagedPiece::OwnedSource {
                        source,
                        range: range.clone(),
                        original: original
                            .as_ref()
                            .map(|(source, range)| (source, range.clone())),
                    });
                }
                tree::Node::Source { source, range, .. } => {
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
pub enum RestoredPiece {
    OwnedSource {
        source: MemorySource,
        range: Range<u64>,
        original: Option<(MemorySource, Range<u64>)>,
    },
    Original(Range<u64>),
    Inserted(String),
}
pub struct OwnedDelta {
    pub range: Range<TextOffset>,
    pub removed: String,
    pub inserted: String,
}
#[derive(Clone)]
pub(crate) struct PagedHistory {
    pub(crate) before_metadata: crate::DocumentMetadata,
    pub(crate) after_metadata: crate::DocumentMetadata,
    pub(crate) typing_insert: bool,
    pub(crate) metadata: crate::history::EditMetadata,
    pub(crate) edits: Vec<OwnedEdit>,
    pub(crate) before_state: ContentStateId,
    pub(crate) after_state: ContentStateId,
    pub(crate) _reservation: crate::history::Charge,
}
use crate::history::OwnedEdit;
/// Source-backed edits share the same balanced piece tree as Resident documents.
/// Callers materialize bounded windows before submitting edits; no actor lock spans I/O.
pub struct PagedDocument {
    pub(crate) current: PagedSnapshot,
    pub(crate) saved_state: ContentStateId,
    pub(crate) bytes: Budget,
    pub(crate) history: Budget,
    pub(crate) history_policy: crate::history::HistoryPolicy,
    pub(crate) undo: crate::history::HistoryStack<PagedHistory>,
    pub(crate) redo: crate::history::HistoryStack<PagedHistory>,
}
impl PagedDocument {
    pub fn new(snapshot: PagedSnapshot, bytes: Budget, history: Budget) -> Self {
        Self {
            saved_state: snapshot.content_state,
            current: snapshot,
            bytes,
            history: history.clone(),
            history_policy: crate::history::HistoryPolicy::default(),
            undo: crate::history::HistoryStack::new(history.clone()),
            redo: crate::history::HistoryStack::new(history.clone()),
        }
    }
    /// Storage owner has sealed an exact copy of `captured`. Refuse dirty/history state
    /// rather than dropping undo. The old service must be retired before using this actor.
    pub(crate) fn from_clean_spill(
        document: &crate::Document,
        captured: &crate::DocumentSnapshot,
        source: MemorySource,
    ) -> Result<Self, Error> {
        if !document.current.same_document(captured) {
            return Err(Error::WrongDocument);
        }
        if document.current.revision != captured.revision {
            return Err(Error::StaleRevision);
        }
        if document.dirty() || !document.undo.is_empty() || !document.redo.is_empty() {
            return Err(Error::ActorBusy);
        }
        if !captured.is_complete() || source.len() != captured.len() as u64 {
            return Err(Error::IncompleteSource);
        }
        let snapshot = PagedSnapshot {
            applied_change: captured.applied_change().cloned(),
            root: tree::from_source(source.clone(), 0..source.len()),
            revision: captured.revision,
            content_state: captured.content_state,
            document_id: captured.document_id,
            _structure: None,
            metadata: captured.metadata.clone(),
        };
        let mut paged = Self::new(snapshot, document.bytes.clone(), document.history.clone());
        paged.history_policy = document.history_policy;
        Ok(paged)
    }
    /// Restore policy from a validated recovery recipe before exposing this actor.
    pub fn restore_metadata(&mut self, metadata: crate::DocumentMetadata) -> Result<(), Error> {
        if !self.undo.is_empty() || !self.redo.is_empty() {
            return Err(Error::ActorBusy);
        }
        self.current.metadata = metadata;
        Ok(())
    }
    pub fn initialize_metadata(&mut self, metadata: crate::DocumentMetadata) -> Result<(), Error> {
        if self.saved_state != self.current.content_state
            || !self.undo.is_empty()
            || !self.redo.is_empty()
        {
            return Err(Error::ActorBusy);
        }
        self.current.metadata = metadata;
        Ok(())
    }
    pub fn apply_metadata(
        &mut self,
        base_revision: Revision,
        metadata: crate::DocumentMetadata,
    ) -> Result<Revision, Error> {
        if base_revision != self.current.revision {
            return Err(Error::StaleRevision);
        }
        if metadata == self.current.metadata {
            return Ok(base_revision);
        }
        let revision = Revision(
            base_revision
                .0
                .checked_add(1)
                .ok_or(Error::RevisionOverflow)?,
        );
        let charge = crate::history::Charge::new(
            self.history
                .reserve(metadata.charge().saturating_add(128))?,
        );
        self.undo
            .try_reserve(1)
            .map_err(|_| Error::BudgetExceeded)?;
        let state = ContentStateId(crate::unique());
        let change = crate::change::AppliedChange::owned(
            self.current.document_id,
            self.current.revision,
            revision,
            self.current.content_state,
            state,
            crate::change::ChangeDirection::Edit,
            &[],
            &self.bytes,
        )?;
        self.current.applied_change = Some(change);
        self.undo.push(PagedHistory {
            before_metadata: self.current.metadata.clone(),
            after_metadata: metadata.clone(),
            typing_insert: false,
            metadata: crate::history::EditMetadata::default(),
            edits: Vec::new(),
            before_state: self.current.content_state,
            after_state: state,
            _reservation: charge,
        });
        self.redo.clear();
        self.current.metadata = metadata;
        self.current.revision = revision;
        self.current.content_state = state;
        self.trim_history();
        Ok(revision)
    }
    pub fn snapshot(&self) -> PagedSnapshot {
        self.current.clone()
    }
    /// Every edited range must be covered by an owned window of this content state.
    /// An interior insertion needs a nonempty surrounding window to prove its boundary.
    pub fn apply_materialized(
        &mut self,
        transaction: EditTransaction,
        windows: &[TextWindow],
    ) -> Result<Revision, Error> {
        self.apply_materialized_with_metadata(
            transaction,
            windows,
            crate::history::EditMetadata::default(),
        )
    }
    pub fn apply_materialized_with_metadata(
        &mut self,
        mut transaction: EditTransaction,
        windows: &[TextWindow],
        metadata: crate::history::EditMetadata,
    ) -> Result<Revision, Error> {
        let typing_insert = transaction.edits.len() == 1
            && transaction.edits[0].range.is_empty()
            && !transaction.edits[0].insert.is_empty()
            && metadata.before.len() == 1
            && metadata.after.len() == 1
            && metadata.before[0].anchor == metadata.before[0].caret
            && metadata.before[0].caret == transaction.edits[0].range.start
            && metadata.after[0].anchor == metadata.after[0].caret
            && metadata.after[0].caret.0
                == transaction.edits[0]
                    .range
                    .start
                    .0
                    .saturating_add(transaction.edits[0].insert.len());
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
            .map(|edit| tree::charged_text(&edit.insert, &self.bytes))
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
            after = tree::charged_replace(
                after,
                edit.before_range.clone(),
                edit.inserted.clone(),
                &self.bytes,
            )?;
        }
        let state = ContentStateId(crate::unique());
        metadata.validate(self.current.len(), tree::summary(&after).bytes)?;
        let mut metadata_charge = self.history.reserve(
            (metadata.before.len() + metadata.after.len())
                * std::mem::size_of::<crate::history::Selection>(),
        )?;
        let mut reservation = reservation;
        reservation.bytes += metadata_charge.bytes;
        metadata_charge.bytes = 0;
        self.undo
            .try_reserve(1)
            .map_err(|_| Error::BudgetExceeded)?;
        let entry = PagedHistory {
            before_metadata: self.current.metadata.clone(),
            after_metadata: self.current.metadata.clone(),
            typing_insert,
            metadata,
            edits: owned_edits,
            before_state: self.current.content_state,
            after_state: state,
            _reservation: crate::history::Charge::new(reservation),
        };
        let change = crate::change::AppliedChange::owned(
            self.current.document_id,
            self.current.revision,
            revision,
            self.current.content_state,
            state,
            crate::change::ChangeDirection::Edit,
            &entry.edits,
            &self.bytes,
        )?;
        let merge = self.undo.last().is_some_and(|last| {
            last.typing_insert
                && entry.typing_insert
                && last.after_state != self.saved_state
                && last.after_state == entry.before_state
                && entry
                    .metadata
                    .follows(&last.metadata, self.history_policy.typing_interval_ms)
        });
        if merge {
            let last = self.undo.last_mut().expect("checked history");
            last.edits[0].inserted = tree::charged_concat(
                last.edits[0].inserted.clone(),
                entry.edits[0].inserted.clone(),
                &self.bytes,
            )?;
            last.edits[0].after_range.end = entry.edits[0].after_range.end;
            last.after_state = entry.after_state;
            last.metadata.after = entry.metadata.after;
            last.metadata.monotonic_ms = entry.metadata.monotonic_ms;
            last._reservation.merge(entry._reservation);
        } else {
            self.undo.push(entry);
        }
        self.redo.clear();
        self.current.applied_change = Some(change);
        self.current.root = after;
        self.current.revision = revision;
        self.current.content_state = state;
        self.trim_history();
        Ok(revision)
    }
    /// Rebuild a validated recovery recipe with source provenance intact.
    pub fn restore_pieces(
        source: MemorySource,
        pieces: Vec<RestoredPiece>,
        bytes: Budget,
        history: Budget,
        revision: Revision,
    ) -> Result<Self, Error> {
        let mut snapshot = PagedSnapshot::utf8(source.clone(), 0)?;
        let mut root = None;
        for piece in pieces {
            let next = match piece {
                RestoredPiece::OwnedSource {
                    source,
                    range,
                    original,
                } => {
                    if !source.has_owned_loader()
                        || range.start > range.end
                        || range.end > source.len()
                        || original.as_ref().is_some_and(|(source, original)| {
                            original.start > original.end
                                || original.end > source.len()
                                || original.end - original.start != range.end - range.start
                        })
                    {
                        return Err(Error::OutOfBounds);
                    }
                    tree::charged_owned(source, range, original, &bytes)?
                }
                RestoredPiece::Original(range) => {
                    if range.start > range.end || range.end > source.len() {
                        return Err(Error::OutOfBounds);
                    }
                    tree::charged_source(source.clone(), range, &bytes)?
                }
                RestoredPiece::Inserted(text) => tree::charged_text(&text, &bytes)?,
            };
            root = tree::charged_concat(root, next, &bytes)?;
        }
        snapshot.root = root;
        snapshot.revision = revision;
        Ok(Self::new(snapshot, bytes, history))
    }
    pub fn saved_content_state(&self) -> ContentStateId {
        self.saved_state
    }
    pub fn capture_spill(&self) -> Result<crate::spill::SpillPlan, Error> {
        crate::spill::SpillPlan::paged(self)
    }
    pub fn attach_spill(&mut self, prepared: crate::spill::PreparedSpill) -> Result<(), Error> {
        let next = prepared.attach_paged(self)?;
        *self = next;
        Ok(())
    }
    pub fn mark_saved(&mut self, snapshot: &PagedSnapshot) -> Result<(), Error> {
        if !self.current.same_document(snapshot) {
            return Err(Error::WrongDocument);
        }
        self.saved_state = snapshot.content_state;
        Ok(())
    }
    /// Tail owner supplies a verified decoded suffix (source byte zero corresponds to `from`).
    /// Existing snapshots retain their old immutable prefix/suffix. Continuity and scalar
    /// boundary validation belong to the tail decoder before this publication step.
    pub fn replace_tail_source(
        &mut self,
        from: TextOffset,
        source: MemorySource,
    ) -> Result<Revision, Error> {
        if !self.undo.is_empty() || !self.redo.is_empty() {
            return Err(Error::ActorBusy);
        }
        if from.0 > self.current.len() {
            return Err(Error::OutOfBounds);
        }
        let suffix_len = usize::try_from(source.len()).map_err(|_| Error::BudgetExceeded)?;
        from.0
            .checked_add(suffix_len)
            .ok_or(Error::BudgetExceeded)?;
        let revision = Revision(
            self.current
                .revision
                .0
                .checked_add(1)
                .ok_or(Error::RevisionOverflow)?,
        );
        let (prefix, _) = tree::charged_split(self.current.root.clone(), from.0, &self.bytes)?;
        let suffix = tree::charged_source(source.clone(), 0..source.len(), &self.bytes)?;
        let root = tree::charged_concat(prefix, suffix, &self.bytes)?;
        let state = ContentStateId(crate::unique());
        let change = crate::change::AppliedChange::build(
            self.current.document_id,
            self.current.revision,
            revision,
            self.current.content_state,
            state,
            crate::change::ChangeDirection::Edit,
            1,
            std::iter::once(crate::change::CompactEdit {
                before: from..TextOffset(self.current.len()),
                inserted_len: suffix_len,
            }),
            &self.bytes,
        )?;
        self.current.applied_change = Some(change);
        self.current.root = root;
        self.current.revision = revision;
        self.current.content_state = state;
        Ok(revision)
    }
    pub fn set_history_policy(&mut self, policy: crate::history::HistoryPolicy) {
        self.history_policy = policy;
        self.trim_history();
    }
    pub fn set_history_limit(&mut self, max_changes: usize) {
        let mut policy = self.history_policy;
        policy.max_changes = max_changes;
        self.set_history_policy(policy);
    }
    pub(crate) fn trim_history(&mut self) {
        let excess = self
            .undo
            .len()
            .saturating_sub(self.history_policy.max_changes);
        self.undo.drain(..excess);
        let excess = self.redo.len().saturating_sub(
            self.history_policy
                .max_changes
                .saturating_sub(self.undo.len()),
        );
        self.redo.drain(..excess);
    }
    pub fn history_metadata(&self, undo: bool) -> Option<&crate::history::EditMetadata> {
        (if undo {
            self.undo.last()
        } else {
            self.redo.last()
        })
        .map(|entry| &entry.metadata)
    }
    pub fn history_stats(&self) -> crate::history::HistoryStats {
        crate::history::HistoryStats {
            charged_capacity_bytes: self.undo.capacity_bytes() + self.redo.capacity_bytes(),
            undo_changes: self.undo.len(),
            redo_changes: self.redo.len(),
            charged_payload_bytes: self
                .undo
                .iter()
                .chain(&self.redo)
                .map(|entry| entry._reservation.bytes())
                .sum(),
        }
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn history_delta_request(
        &self,
        undo: bool,
        max_bytes: usize,
        budget: Budget,
    ) -> Result<HistoryDeltaRequest, Error> {
        let entry = (if undo {
            self.undo.last()
        } else {
            self.redo.last()
        })
        .ok_or(Error::EmptyHistory)?;
        let total = entry
            .edits
            .iter()
            .try_fold(0usize, |total, edit| {
                total
                    .checked_add(tree::summary(&edit.inverse).bytes)?
                    .checked_add(tree::summary(&edit.inserted).bytes)
            })
            .ok_or(Error::BudgetExceeded)?;
        if total > max_bytes {
            return Err(Error::BudgetExceeded);
        }
        let charge = budget.claim(
            total
                .checked_add(
                    entry
                        .edits
                        .len()
                        .checked_mul(std::mem::size_of::<OwnedDelta>())
                        .ok_or(Error::BudgetExceeded)?,
                )
                .ok_or(Error::BudgetExceeded)?,
        )?;
        Ok(HistoryDeltaRequest {
            snapshot: self.current.clone(),
            edits: entry.edits.clone(),
            undo,
            index: 0,
            inserted_phase: false,
            cursor: 0,
            removed: String::new(),
            text: String::new(),
            output: Vec::with_capacity(entry.edits.len()),
            window: None,
            budget,
            charge: Some(charge),
            cancelled: false,
            finished: false,
        })
    }
    pub fn history_delta(&self, undo: bool) -> Result<Vec<OwnedDelta>, Error> {
        let entry = if undo {
            self.undo.last()
        } else {
            self.redo.last()
        }
        .ok_or(Error::EmptyHistory)?;
        if entry
            .edits
            .iter()
            .any(|edit| tree::has_source(&edit.inverse) || tree::has_source(&edit.inserted))
        {
            return Err(Error::IncompleteSource);
        }
        let owned_text = |root: &tree::Root| {
            tree::chunks(root, 0..tree::summary(root).bytes).collect::<String>()
        };
        Ok(entry
            .edits
            .iter()
            .map(|edit| {
                let range = if undo {
                    &edit.after_range
                } else {
                    &edit.before_range
                };
                OwnedDelta {
                    range: TextOffset(range.start)..TextOffset(range.end),
                    removed: owned_text(if undo { &edit.inserted } else { &edit.inverse }),
                    inserted: owned_text(if undo { &edit.inverse } else { &edit.inserted }),
                }
            })
            .collect())
    }
    pub fn undo(&mut self) -> Result<Revision, Error> {
        let prepared = self.prepare_source_history(true, &self.bytes)?;
        Ok(self.lease_source_history(prepared)?.publish())
    }
    pub fn redo(&mut self) -> Result<Revision, Error> {
        let prepared = self.prepare_source_history(false, &self.bytes)?;
        Ok(self.lease_source_history(prepared)?.publish())
    }
}
/// The payload budget stays charged until the recovery journal consumer releases it.
pub struct MaterializedHistory {
    pub deltas: Vec<OwnedDelta>,
    _charge: crate::BudgetClaim,
}
pub enum HistoryDeltaPoll {
    Ready(MaterializedHistory),
    Pending(PageTicket),
    Progress,
    Unavailable(Unavailable),
    Failed(Error),
    Cancelled,
    Finished,
}
pub struct HistoryDeltaRequest {
    snapshot: PagedSnapshot,
    edits: Vec<OwnedEdit>,
    undo: bool,
    index: usize,
    inserted_phase: bool,
    cursor: usize,
    removed: String,
    text: String,
    output: Vec<OwnedDelta>,
    window: Option<WindowRequest>,
    budget: Budget,
    charge: Option<crate::BudgetClaim>,
    cancelled: bool,
    finished: bool,
}
impl HistoryDeltaRequest {
    pub fn matches_snapshot(&self, snapshot: &PagedSnapshot) -> bool {
        self.snapshot.same_document(snapshot)
            && self.snapshot.revision == snapshot.revision
            && self.snapshot.content_state == snapshot.content_state
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.window = None;
        self.output = Vec::new();
        self.text = String::new();
        self.removed = String::new();
        self.charge = None;
    }
    pub fn resolve_owned(&self, ticket: PageTicket) -> Result<bool, Error> {
        let Some(edit) = self.edits.get(self.index) else {
            return Ok(false);
        };
        let mut snapshot = self.snapshot.clone();
        snapshot.root = if self.inserted_phase != self.undo {
            edit.inserted.clone()
        } else {
            edit.inverse.clone()
        };
        snapshot.resolve_owned(ticket)
    }
    pub fn poll(&mut self) -> HistoryDeltaPoll {
        if self.cancelled {
            return HistoryDeltaPoll::Cancelled;
        }
        if self.finished {
            return HistoryDeltaPoll::Finished;
        }
        let result = self.step();
        if matches!(
            result,
            HistoryDeltaPoll::Failed(_) | HistoryDeltaPoll::Unavailable(_)
        ) {
            self.cancel();
            self.finished = true;
        }
        result
    }
    fn step(&mut self) -> HistoryDeltaPoll {
        let Some(edit) = self.edits.get(self.index) else {
            self.finished = true;
            return HistoryDeltaPoll::Ready(MaterializedHistory {
                deltas: std::mem::take(&mut self.output),
                _charge: self.charge.take().expect("history payload charge"),
            });
        };
        let root = if self.inserted_phase != self.undo {
            &edit.inserted
        } else {
            &edit.inverse
        };
        let len = tree::summary(root).bytes;
        if self.cursor == len {
            if !self.inserted_phase {
                self.removed = std::mem::take(&mut self.text);
                self.inserted_phase = true;
                self.cursor = 0;
            } else {
                let range = if self.undo {
                    &edit.after_range
                } else {
                    &edit.before_range
                };
                self.output.push(OwnedDelta {
                    range: TextOffset(range.start)..TextOffset(range.end),
                    removed: std::mem::take(&mut self.removed),
                    inserted: std::mem::take(&mut self.text),
                });
                self.index += 1;
                self.inserted_phase = false;
                self.cursor = 0;
            }
            return HistoryDeltaPoll::Progress;
        }
        if self.window.is_none() {
            if self.cursor == 0 {
                self.text = String::with_capacity(len);
            }
            let mut snapshot = self.snapshot.clone();
            snapshot.root = root.clone();
            match snapshot.begin_viewport(TextOffset(self.cursor), 64 * 1024, &self.budget) {
                Ok(window) => self.window = Some(window),
                Err(error) => return HistoryDeltaPoll::Failed(error),
            }
        }
        match self.window.as_mut().expect("history window").poll() {
            WindowPoll::Ready(window) => {
                if window.range.start.0 != self.cursor || window.text.is_empty() {
                    return HistoryDeltaPoll::Failed(Error::InvalidBoundary);
                }
                self.cursor = window.range.end.0;
                self.text.push_str(&window.text);
                self.window = None;
                HistoryDeltaPoll::Progress
            }
            WindowPoll::Pending(ticket) => HistoryDeltaPoll::Pending(ticket),
            WindowPoll::Unavailable(reason) => HistoryDeltaPoll::Unavailable(reason),
            WindowPoll::InvalidUtf8 => HistoryDeltaPoll::Failed(Error::InvalidBoundary),
            WindowPoll::Finished => HistoryDeltaPoll::Failed(Error::IncompleteSource),
        }
    }
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
                tree::Span::Source(source, range) | tree::Span::OwnedSource(source, range) => {
                    (source, range)
                }
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

#[cfg(test)]
mod lookup_feedback_tests {
    use super::*;
    use crate::{
        line_lookup::{LineLookupPoll, LineTarget},
        source::{Generation, SourceKind},
    };
    #[test]
    fn lookup_feedback_retains_cr_boundary_and_rejects_stale_without_growing() {
        let budget = Budget::new(8192);
        let (source, publisher) =
            MemorySource::new(8, Generation(991), SourceKind::Paged, 8, 8, budget.clone()).unwrap();
        publisher
            .publish(
                PageTicket {
                    generation: Generation(991),
                    page: 0,
                },
                b"abc\r\nx\nz",
                Generation(991),
            )
            .unwrap();
        let snapshot = PagedSnapshot::utf8(source, 0).unwrap();
        let mut index = SparseLineIndex::new(snapshot.clone(), 2, 4, &budget).unwrap();
        let mut request = index
            .lookup(LineTarget::Byte(TextOffset(8)), budget.clone())
            .unwrap();
        assert!(matches!(
            request.poll(),
            LineLookupPoll::Progress(TextOffset(4))
        ));
        index.retain_lookup_progress(&request).unwrap();
        let checkpoint = index.checkpoint_before(TextOffset(4)).unwrap();
        assert!(checkpoint.preceding_cr);
        assert_eq!(checkpoint.breaks, 1);
        assert!(matches!(
            request.poll(),
            LineLookupPoll::Progress(TextOffset(8))
        ));
        index.retain_lookup_progress(&request).unwrap();
        assert_eq!(index.checkpoints.len(), 2);
        assert_eq!(index.line_count(), LineCount::Known(3));
        assert!(matches!(request.poll(), LineLookupPoll::Line(2)));
        let earlier = index
            .lookup(LineTarget::Byte(TextOffset(2)), budget.clone())
            .unwrap();
        index.retain_lookup_progress(&earlier).unwrap();
        assert!(
            index
                .checkpoints
                .windows(2)
                .all(|pair| pair[0].offset < pair[1].offset)
        );
        assert_eq!(index.scanned_to(), TextOffset(8));
        let mut changed = snapshot;
        changed.content_state = ContentStateId(crate::unique());
        index.reset(changed);
        assert_eq!(
            index.retain_lookup_progress(&request),
            Err(IndexError::StaleSnapshot)
        );
        request.cancel();
        assert!(request.verified_checkpoint().is_none());
    }
}
