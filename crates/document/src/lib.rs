// SPDX-License-Identifier: MPL-2.0
//! Resident UTF-8 document core. All published bytes are owned and immutable.
pub mod group;
pub mod history;
pub mod line_lookup;
pub mod paged;
pub mod service;
pub mod source;
mod tree;
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};
pub use tree::Chunks;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextOffset(pub usize);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Revision(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentStateId(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    OutOfBounds,
    InvalidBoundary,
    OverlappingEdits,
    StaleRevision,
    BudgetExceeded,
    EmptyHistory,
    WrongDocument,
    RevisionOverflow,
    IncompleteSource,
    EmptyTransaction,
    LinkedUndoRequired,
    ActorBusy,
}

struct BudgetInner {
    limit: usize,
    used: AtomicUsize,
}
#[derive(Clone)]
pub struct Budget(Arc<BudgetInner>);
/// RAII charge for caller-owned buffers participating in the shared memory cap.
pub struct BudgetClaim {
    _reservation: Reservation,
}
impl Budget {
    pub fn claim(&self, bytes: usize) -> Result<BudgetClaim, Error> {
        Ok(BudgetClaim {
            _reservation: self.reserve(bytes)?,
        })
    }
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(BudgetInner {
            limit,
            used: AtomicUsize::new(0),
        }))
    }
    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Relaxed)
    }
    pub fn limit(&self) -> usize {
        self.0.limit
    }
    fn reserve(&self, bytes: usize) -> Result<Reservation, Error> {
        self.0
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|n| *n <= self.0.limit)
            })
            .map_err(|_| Error::BudgetExceeded)?;
        Ok(Reservation {
            budget: self.clone(),
            bytes,
        })
    }
}
struct Reservation {
    budget: Budget,
    bytes: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.0.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
fn unique() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
pub struct DocumentSnapshot {
    root: tree::Root,
    pub revision: Revision,
    pub content_state: ContentStateId,
    document_id: u64,
    complete: bool,
}
impl DocumentSnapshot {
    /// Opaque source token for validating queued external actions; forks have distinct identities.
    pub fn identity_token(&self) -> (u64, u64) { (self.document_id, self.revision.0) }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn same_document(&self, other: &Self) -> bool {
        self.document_id == other.document_id
    }
    pub fn eol_label(&self) -> &'static str {
        let summary = tree::summary(&self.root);
        match (summary.cr > 0, summary.lf > 0, summary.crlf > 0) {
            (false, false, false) | (false, true, false) => "LF",
            (true, false, false) => "CR",
            (false, false, true) => "CRLF",
            _ => "Mixed",
        }
    }
    pub fn line_range(&self, line: usize) -> Result<Range<TextOffset>, Error> {
        let start = tree::line_start(&self.root, line).ok_or(Error::OutOfBounds)?;
        let end = tree::line_start(&self.root, line + 1).unwrap_or(self.len());
        Ok(TextOffset(start)..TextOffset(end))
    }
    pub fn line_at(&self, offset: TextOffset) -> Result<usize, Error> {
        if offset.0 > self.len() {
            return Err(Error::OutOfBounds);
        }
        if !self.is_boundary(offset) {
            return Err(Error::InvalidBoundary);
        }
        Ok(tree::line_at(&self.root, offset.0))
    }
    pub fn len(&self) -> usize {
        tree::summary(&self.root).bytes
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn line_count(&self) -> usize {
        tree::summary(&self.root).breaks + 1
    }
    pub fn is_boundary(&self, offset: TextOffset) -> bool {
        offset.0 <= self.len() && tree::boundary(&self.root, offset.0)
    }
    fn validate_range(&self, range: &Range<TextOffset>) -> Result<(), Error> {
        if range.start > range.end || range.end.0 > self.len() {
            return Err(Error::OutOfBounds);
        }
        if !self.is_boundary(range.start) || !self.is_boundary(range.end) {
            return Err(Error::InvalidBoundary);
        }
        Ok(())
    }
    /// Caller chooses a bounded viewport range. Returned slices borrow immutable owned segments.
    pub fn chunks(&self, range: Range<TextOffset>) -> Result<Chunks<'_>, Error> {
        self.validate_range(&range)?;
        Ok(tree::chunks(&self.root, range.start.0..range.end.0))
    }
    pub fn read(&self, range: Range<TextOffset>, max_bytes: usize) -> Result<String, Error> {
        self.validate_range(&range)?;
        if range.end.0 - range.start.0 > max_bytes {
            return Err(Error::BudgetExceeded);
        }
        let mut output = String::with_capacity(range.end.0 - range.start.0);
        for chunk in self.chunks(range)? {
            output.push_str(chunk);
        }
        Ok(output)
    }
}

pub struct Edit {
    pub range: Range<TextOffset>,
    pub insert: String,
}
pub struct EditTransaction {
    pub base_revision: Revision,
    pub edits: Vec<Edit>,
}
struct History {
    before: tree::Root,
    after: tree::Root,
    before_state: ContentStateId,
    after_state: ContentStateId,
    _undo_reservation: Reservation,
    group: Option<group::GroupTag>,
    metadata: history::EditMetadata,
    typing_insert: bool,
}
/// Validated, budget-reserved roots; dropping this token leaves the document unchanged.
pub struct PreparedEdit {
    document_id: u64,
    base_revision: Revision,
    revision: Revision,
    entry: History,
}
pub struct Document {
    current: DocumentSnapshot,
    saved_state: ContentStateId,
    history_policy: history::HistoryPolicy,
    undo: Vec<History>,
    redo: Vec<History>,
    bytes: Budget,
    history: Budget,
}
/// Incremental Resident construction. Prefix snapshots share immutable chunks, and
/// appended source data does not create user edits or consume undo history.
pub struct DocumentBuilder {
    document: Document,
}
impl DocumentBuilder {
    pub fn new(bytes: Budget, history: Budget) -> Result<Self, Error> {
        Ok(Self {
            document: Document::from_utf8("", bytes, history)?,
        })
    }
    pub fn append(&mut self, text: &str) -> Result<(), Error> {
        let suffix = tree::from_text(text, &self.document.bytes)?;
        let revision = self.document.next_revision()?;
        self.document.current.root = tree::concat(self.document.current.root.clone(), suffix);
        self.document.current.revision = revision;
        self.document.current.content_state = ContentStateId(unique());
        Ok(())
    }
    /// This snapshot is a loaded prefix, not an editable or complete document.
    pub fn prefix(&self) -> DocumentSnapshot {
        let mut snapshot = self.document.snapshot();
        snapshot.complete = false;
        snapshot
    }
    pub fn finish(mut self) -> Document {
        self.document.saved_state = self.document.current.content_state;
        self.document
    }
}
impl Document {
    /// Share immutable text storage with a complete snapshot while assigning a fresh
    /// document and content identity. Future edits and undo histories are independent.
    pub fn fork_from_snapshot(
        snapshot: &DocumentSnapshot,
        bytes: Budget,
        history: Budget,
    ) -> Result<Self, Error> {
        if !snapshot.is_complete() {
            return Err(Error::IncompleteSource);
        }
        let state = ContentStateId(unique());
        Ok(Self {
            current: DocumentSnapshot {
                root: snapshot.root.clone(),
                revision: Revision(0),
                content_state: state,
                document_id: unique(),
                complete: true,
            },
            saved_state: state,
            history_policy: history::HistoryPolicy::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            bytes,
            history,
        })
    }
    /// Budgets are shared across all documents created by the application.
    pub fn from_utf8(text: &str, bytes: Budget, history: Budget) -> Result<Self, Error> {
        let root = tree::from_text(text, &bytes)?;
        let state = ContentStateId(unique());
        Ok(Self {
            current: DocumentSnapshot {
                root,
                revision: Revision(0),
                content_state: state,
                document_id: unique(),
                complete: true,
            },
            saved_state: state,
            history_policy: history::HistoryPolicy::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            bytes,
            history,
        })
    }
    pub fn snapshot(&self) -> DocumentSnapshot {
        self.current.clone()
    }
    pub fn dirty(&self) -> bool {
        self.current.content_state != self.saved_state
    }
    /// Save may finish after newer edits. Only the captured content state becomes clean.
    pub fn mark_saved(&mut self, captured: &DocumentSnapshot) -> Result<(), Error> {
        if !captured.complete {
            return Err(Error::IncompleteSource);
        }
        if captured.document_id != self.current.document_id {
            return Err(Error::WrongDocument);
        }
        self.saved_state = captured.content_state;
        Ok(())
    }
    fn next_revision(&self) -> Result<Revision, Error> {
        self.current
            .revision
            .0
            .checked_add(1)
            .map(Revision)
            .ok_or(Error::RevisionOverflow)
    }
    pub fn apply(&mut self, transaction: EditTransaction) -> Result<Revision, Error> {
        if transaction.base_revision != self.current.revision {
            return Err(Error::StaleRevision);
        }
        if transaction.edits.is_empty() {
            return Ok(self.current.revision);
        }
        let prepared = self.prepare(transaction)?;
        self.commit_prepared(prepared)
    }
    pub fn apply_with_metadata(
        &mut self,
        transaction: EditTransaction,
        metadata: history::EditMetadata,
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
        let mut prepared = self.prepare(transaction)?;
        metadata.validate(
            self.current.len(),
            tree::summary(&prepared.entry.after).bytes,
        )?;
        for selection in &metadata.before {
            if !self.current.is_boundary(selection.anchor)
                || !self.current.is_boundary(selection.caret)
            {
                return Err(Error::InvalidBoundary);
            }
        }
        for selection in &metadata.after {
            if !tree::boundary(&prepared.entry.after, selection.anchor.0)
                || !tree::boundary(&prepared.entry.after, selection.caret.0)
            {
                return Err(Error::InvalidBoundary);
            }
        }
        let mut metadata_charge = self.history.reserve(
            (metadata.before.len() + metadata.after.len())
                * std::mem::size_of::<history::Selection>(),
        )?;
        prepared.entry._undo_reservation.bytes += metadata_charge.bytes;
        metadata_charge.bytes = 0;
        prepared.entry.metadata = metadata;
        prepared.entry.typing_insert = typing_insert;
        self.commit_prepared(prepared)
    }
    pub fn set_history_policy(&mut self, policy: history::HistoryPolicy) {
        self.history_policy = policy;
        self.trim_history();
    }
    fn trim_history(&mut self) {
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
        // The end is the next redo; discard the furthest future first.
        self.redo.drain(..excess);
    }
    pub fn history_metadata(&self, undo: bool) -> Option<&history::EditMetadata> {
        (if undo {
            self.undo.last()
        } else {
            self.redo.last()
        })
        .map(|entry| &entry.metadata)
    }
    pub fn history_stats(&self) -> history::HistoryStats {
        history::HistoryStats {
            undo_changes: self.undo.len(),
            redo_changes: self.redo.len(),
            charged_payload_bytes: self
                .undo
                .iter()
                .chain(&self.redo)
                .map(|entry| entry._undo_reservation.bytes)
                .sum(),
        }
    }
    pub fn prepare(&self, mut transaction: EditTransaction) -> Result<PreparedEdit, Error> {
        if transaction.base_revision != self.current.revision {
            return Err(Error::StaleRevision);
        }
        if transaction.edits.is_empty() {
            return Err(Error::EmptyTransaction);
        }
        transaction
            .edits
            .sort_by_key(|e| (e.range.start, e.range.end));
        let mut undo_bytes = 0usize;
        for (i, edit) in transaction.edits.iter().enumerate() {
            self.current.validate_range(&edit.range)?;
            if i > 0 {
                let previous = &transaction.edits[i - 1].range;
                if previous.end > edit.range.start || previous.start == edit.range.start {
                    return Err(Error::OverlappingEdits);
                }
            }
            undo_bytes = undo_bytes
                .checked_add(edit.range.end.0 - edit.range.start.0)
                .and_then(|n| n.checked_add(edit.insert.len()))
                .ok_or(Error::BudgetExceeded)?;
        }
        let revision = self.next_revision()?;
        // Reserve ownership before modifying roots; any failure drops staged segments.
        let reservation = self.history.reserve(undo_bytes.max(1))?;
        let inserts = transaction
            .edits
            .iter()
            .map(|e| tree::from_text(&e.insert, &self.bytes))
            .collect::<Result<Vec<_>, _>>()?;
        let mut root = self.current.root.clone();
        for (edit, inserted) in transaction.edits.iter().zip(inserts).rev() {
            let (left_and_deleted, right) = tree::split(root, edit.range.end.0);
            let (left, _deleted) = tree::split(left_and_deleted, edit.range.start.0);
            root = tree::concat(tree::concat(left, inserted), right);
        }
        let state = ContentStateId(unique());
        Ok(PreparedEdit {
            document_id: self.current.document_id,
            base_revision: self.current.revision,
            revision,
            entry: History {
                before: self.current.root.clone(),
                after: root.clone(),
                before_state: self.current.content_state,
                after_state: state,
                _undo_reservation: reservation,
                group: None,
                metadata: history::EditMetadata::default(),
                typing_insert: false,
            },
        })
    }
    fn validate_prepared(&self, prepared: &PreparedEdit) -> Result<(), Error> {
        if prepared.document_id != self.current.document_id {
            return Err(Error::WrongDocument);
        }
        if prepared.base_revision != self.current.revision {
            return Err(Error::StaleRevision);
        }
        Ok(())
    }
    pub fn commit_prepared(&mut self, prepared: PreparedEdit) -> Result<Revision, Error> {
        self.validate_prepared(&prepared)?;
        self.undo
            .try_reserve(1)
            .map_err(|_| Error::BudgetExceeded)?;
        Ok(self.commit_prepared_unchecked(prepared))
    }
    fn commit_prepared_unchecked(&mut self, mut prepared: PreparedEdit) -> Revision {
        self.redo.clear();
        self.current.root = prepared.entry.after.clone();
        self.current.content_state = prepared.entry.after_state;
        self.current.revision = prepared.revision;
        let merge = self.undo.last().is_some_and(|last| {
            last.group.is_none()
                && prepared.entry.group.is_none()
                && last.typing_insert
                && prepared.entry.typing_insert
                && last.after_state == prepared.entry.before_state
                && last.after_state != self.saved_state
                && prepared
                    .entry
                    .metadata
                    .follows(&last.metadata, self.history_policy.typing_interval_ms)
        });
        if merge {
            let last = self.undo.last_mut().expect("checked history");
            last.after = prepared.entry.after;
            last.after_state = prepared.entry.after_state;
            last.metadata.after = prepared.entry.metadata.after;
            last.metadata.monotonic_ms = prepared.entry.metadata.monotonic_ms;
            last._undo_reservation.bytes += prepared.entry._undo_reservation.bytes;
            prepared.entry._undo_reservation.bytes = 0;
        } else {
            self.undo.push(prepared.entry);
        }
        self.trim_history();
        self.current.revision
    }
    pub fn undo(&mut self) -> Result<Revision, Error> {
        if self.undo.last().is_some_and(|entry| entry.group.is_some()) {
            return Err(Error::LinkedUndoRequired);
        }
        let revision = self.next_revision()?;
        let mut entry = self.undo.pop().ok_or(Error::EmptyHistory)?;
        entry.typing_insert = false;
        if let Some(previous) = self.undo.last_mut() {
            previous.typing_insert = false;
        }
        self.current.root = entry.before.clone();
        self.current.content_state = entry.before_state;
        self.current.revision = revision;
        self.redo.push(entry);
        Ok(revision)
    }
    pub fn redo(&mut self) -> Result<Revision, Error> {
        if self.redo.last().is_some_and(|entry| entry.group.is_some()) {
            return Err(Error::LinkedUndoRequired);
        }
        let revision = self.next_revision()?;
        let mut entry = self.redo.pop().ok_or(Error::EmptyHistory)?;
        entry.typing_insert = false;
        self.current.root = entry.after.clone();
        self.current.content_state = entry.after_state;
        self.current.revision = revision;
        self.undo.push(entry);
        Ok(revision)
    }
    pub fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests;
