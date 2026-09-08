// SPDX-License-Identifier: MPL-2.0
//! Bounded validation and atomic splicing of sealed text stores. No disk I/O in commit.
use crate::{
    Budget, BudgetClaim, ContentStateId, Error, Revision, TextOffset,
    history::{EditMetadata, OwnedEdit},
    paged::{PagedDocument, PagedHistory, PagedSnapshot, TextWindow, WindowPoll, WindowRequest},
    source::{MemorySource, PageTicket, Unavailable},
    tree,
};
use std::ops::Range;
#[derive(Clone)]
pub struct OwnedTextRange {
    pub source: MemorySource,
    pub range: Range<u64>,
}
#[derive(Clone)]
pub struct SourceEdit {
    pub range: Range<TextOffset>,
    pub inverse: OwnedTextRange,
    pub inserted: OwnedTextRange,
}
pub enum SourceTransactionPoll {
    Ready(PreparedSourceTransaction),
    Pending(PageTicket),
    Progress,
    Unavailable(Unavailable),
    Failed(Error),
    Cancelled,
    Finished,
}
pub struct PreparedSourceTransaction {
    snapshot: PagedSnapshot,
    edits: Vec<SourceEdit>,
    metadata: EditMetadata,
    _claim: BudgetClaim,
}
pub struct SourceTransactionRequest {
    snapshot: PagedSnapshot,
    edits: Vec<SourceEdit>,
    metadata: EditMetadata,
    index: usize,
    cursor: usize,
    insertion: bool,
    boundary_checked: bool,
    old: Option<TextWindow>,
    window: Option<WindowRequest>,
    pending_source: Option<PagedSnapshot>,
    budget: Budget,
    claim: Option<BudgetClaim>,
    cancelled: bool,
    finished: bool,
}
fn owned_snapshot(template: &PagedSnapshot, range: &OwnedTextRange) -> PagedSnapshot {
    let mut snapshot = template.clone();
    snapshot.root = tree::from_owned_source(range.source.clone(), range.range.clone(), None);
    snapshot
}
impl PagedSnapshot {
    pub fn prepare_source_transaction(
        &self,
        mut edits: Vec<SourceEdit>,
        metadata: EditMetadata,
        budget: Budget,
    ) -> Result<SourceTransactionRequest, Error> {
        if edits.is_empty() || edits.len() > 4096 {
            return Err(Error::EmptyTransaction);
        }
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        let mut length = self.len();
        for (index, edit) in edits.iter().enumerate() {
            if edit.range.start > edit.range.end || edit.range.end.0 > self.len() {
                return Err(Error::OutOfBounds);
            }
            if index > 0
                && (edits[index - 1].range.end > edit.range.start
                    || edits[index - 1].range.start == edit.range.start)
            {
                return Err(Error::OverlappingEdits);
            }
            for owned in [&edit.inverse, &edit.inserted] {
                if !owned.source.has_owned_loader()
                    || owned.range.start > owned.range.end
                    || owned.range.end > owned.source.len()
                {
                    return Err(Error::IncompleteSource);
                }
                usize::try_from(owned.range.end - owned.range.start)
                    .map_err(|_| Error::BudgetExceeded)?;
            }
            if edit.inverse.range.end - edit.inverse.range.start
                != (edit.range.end.0 - edit.range.start.0) as u64
            {
                return Err(Error::OutOfBounds);
            }
            length = length
                .checked_sub(edit.range.end.0 - edit.range.start.0)
                .and_then(|length| {
                    length
                        .checked_add((edit.inserted.range.end - edit.inserted.range.start) as usize)
                })
                .ok_or(Error::BudgetExceeded)?;
        }
        metadata.validate(self.len(), length)?;
        let descriptors = product(
            edits.capacity(),
            std::mem::size_of::<SourceEdit>()
                + 2 * (std::mem::size_of::<tree::Node>() + 2 * std::mem::size_of::<usize>()),
        )?;
        let claim = budget.claim(sum(descriptors, selection_bytes(&metadata)?)?)?;
        Ok(SourceTransactionRequest {
            snapshot: self.clone(),
            edits,
            metadata,
            index: 0,
            cursor: 0,
            insertion: false,
            boundary_checked: false,
            old: None,
            window: None,
            pending_source: None,
            budget,
            claim: Some(claim),
            cancelled: false,
            finished: false,
        })
    }
}
impl SourceTransactionRequest {
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.window = None;
        self.old = None;
        self.pending_source = None;
        self.edits = Vec::new();
        self.claim = None;
    }
    pub fn matches_snapshot(&self, snapshot: &PagedSnapshot) -> bool {
        self.snapshot.same_document(snapshot)
            && self.snapshot.revision == snapshot.revision
            && self.snapshot.content_state == snapshot.content_state
    }
    pub fn resolve_owned(&self, ticket: PageTicket) -> Result<bool, Error> {
        self.pending_source
            .as_ref()
            .map_or(Ok(false), |snapshot| snapshot.resolve_owned(ticket))
    }
    pub fn poll(&mut self) -> SourceTransactionPoll {
        if self.cancelled {
            return SourceTransactionPoll::Cancelled;
        }
        if self.finished {
            return SourceTransactionPoll::Finished;
        }
        let result = self.step();
        if matches!(
            result,
            SourceTransactionPoll::Failed(_) | SourceTransactionPoll::Unavailable(_)
        ) {
            self.cancel();
        }
        result
    }
    fn window(
        &mut self,
        snapshot: PagedSnapshot,
        start: usize,
        length: usize,
        exact: bool,
    ) -> Result<Option<TextWindow>, SourceTransactionPoll> {
        if self.window.is_none() {
            let request = if exact {
                snapshot.begin_read(
                    TextOffset(start)..TextOffset(start + length),
                    length,
                    &self.budget,
                )
            } else {
                snapshot.begin_viewport(TextOffset(start), length, &self.budget)
            };
            self.window = Some(request.map_err(SourceTransactionPoll::Failed)?);
            self.pending_source = Some(snapshot);
        }
        match self.window.as_mut().expect("stage window").poll() {
            WindowPoll::Ready(window) => {
                self.window = None;
                self.pending_source = None;
                Ok(Some(window))
            }
            WindowPoll::Pending(ticket) => Err(SourceTransactionPoll::Pending(ticket)),
            WindowPoll::Unavailable(reason) => Err(SourceTransactionPoll::Unavailable(reason)),
            WindowPoll::InvalidUtf8 => Err(SourceTransactionPoll::Failed(Error::InvalidBoundary)),
            WindowPoll::Finished => Err(SourceTransactionPoll::Failed(Error::IncompleteSource)),
        }
    }
    fn step(&mut self) -> SourceTransactionPoll {
        let Some(edit) = self.edits.get(self.index).cloned() else {
            self.finished = true;
            return SourceTransactionPoll::Ready(PreparedSourceTransaction {
                snapshot: self.snapshot.clone(),
                edits: std::mem::take(&mut self.edits),
                metadata: self.metadata.clone(),
                _claim: self.claim.take().expect("stage metadata charge"),
            });
        };
        if !self.boundary_checked {
            let offset = edit.range.start.0;
            if self.snapshot.is_empty() {
                self.boundary_checked = true;
            } else {
                let start = offset.saturating_sub(4);
                let length = 8.min(self.snapshot.len() - start);
                match self.window(self.snapshot.clone(), start, length, false) {
                    Ok(Some(window)) => {
                        if offset < window.range().start.0
                            || offset > window.range().end.0
                            || !window
                                .text()
                                .is_char_boundary(offset - window.range().start.0)
                        {
                            return SourceTransactionPoll::Failed(Error::InvalidBoundary);
                        }
                        self.boundary_checked = true;
                    }
                    Ok(None) => return SourceTransactionPoll::Progress,
                    Err(result) => return result,
                }
            }
            return SourceTransactionPoll::Progress;
        }
        if !self.insertion {
            let length = edit.range.end.0 - edit.range.start.0;
            if self.cursor == length {
                self.insertion = true;
                self.cursor = 0;
                return SourceTransactionPoll::Progress;
            }
            if self.old.is_none() {
                match self.window(
                    self.snapshot.clone(),
                    edit.range.start.0 + self.cursor,
                    (length - self.cursor).min(64 * 1024),
                    false,
                ) {
                    Ok(Some(window)) => {
                        if window.range().start.0 != edit.range.start.0 + self.cursor
                            || window.text().is_empty()
                        {
                            return SourceTransactionPoll::Failed(Error::InvalidBoundary);
                        }
                        self.old = Some(window);
                    }
                    Ok(None) => return SourceTransactionPoll::Progress,
                    Err(result) => return result,
                }
                return SourceTransactionPoll::Progress;
            }
            let length = self.old.as_ref().expect("old bytes").text().len();
            match self.window(
                owned_snapshot(&self.snapshot, &edit.inverse),
                self.cursor,
                length,
                true,
            ) {
                Ok(Some(window)) => {
                    if window.text() != self.old.as_ref().expect("old bytes").text() {
                        return SourceTransactionPoll::Failed(Error::StaleRevision);
                    }
                    self.cursor += length;
                    self.old = None;
                }
                Ok(None) => {}
                Err(result) => return result,
            }
        } else {
            let length = (edit.inserted.range.end - edit.inserted.range.start) as usize;
            if self.cursor == length {
                self.index += 1;
                self.cursor = 0;
                self.insertion = false;
                self.boundary_checked = false;
                return SourceTransactionPoll::Progress;
            }
            match self.window(
                owned_snapshot(&self.snapshot, &edit.inserted),
                self.cursor,
                (length - self.cursor).min(64 * 1024),
                false,
            ) {
                Ok(Some(window)) => {
                    if window.range().start.0 != self.cursor || window.text().is_empty() {
                        return SourceTransactionPoll::Failed(Error::InvalidBoundary);
                    }
                    self.cursor = window.range().end.0;
                }
                Ok(None) => {}
                Err(result) => return result,
            }
        }
        SourceTransactionPoll::Progress
    }
}
impl PreparedSourceTransaction {
    pub fn edits(&self) -> &[SourceEdit] {
        &self.edits
    }
    pub fn metadata(&self) -> &EditMetadata {
        &self.metadata
    }
    pub fn base_revision(&self) -> Revision {
        self.snapshot.revision
    }
    pub fn next_revision(&self) -> Result<Revision, Error> {
        Ok(Revision(
            self.snapshot
                .revision
                .0
                .checked_add(1)
                .ok_or(Error::RevisionOverflow)?,
        ))
    }
}
fn inverse_root(
    root: &tree::Root,
    inverse: &OwnedTextRange,
    cursor: &mut u64,
    budget: &Budget,
) -> Result<tree::Root, Error> {
    let Some(node) = root else {
        return Ok(None);
    };
    match node.as_ref() {
        tree::Node::Branch { left, right, .. } => tree::charged_concat(
            inverse_root(&Some(left.clone()), inverse, cursor, budget)?,
            inverse_root(&Some(right.clone()), inverse, cursor, budget)?,
            budget,
        ),
        _ => {
            let bytes = node.summary().bytes as u64;
            let start = *cursor;
            *cursor += bytes;
            let original = match node.as_ref() {
                tree::Node::Source { source, range, .. } => Some((source.clone(), range.clone())),
                tree::Node::OwnedSource { original, .. } => original.clone(),
                tree::Node::Leaf(piece) => piece
                    .origin()
                    .map(|(source, range)| (source.clone(), range)),
                tree::Node::Branch { .. } => unreachable!(),
            };
            Ok(Some(tree::charged_node(
                tree::Node::OwnedSource {
                    _charge: None,
                    source: inverse.source.clone(),
                    range: inverse.range.start + start..inverse.range.start + start + bytes,
                    original,
                    summary: node.summary(),
                },
                budget,
            )?))
        }
    }
}
// Source transaction publication is deliberately separate from durable journal I/O.
/// An exclusive actor lease. All fallible work precedes construction. Drop aborts;
/// publish cannot allocate, read pages, or fail after the caller's durable append.
pub struct SourceCommitLease<'a> {
    document: &'a mut PagedDocument,
    next: PagedSnapshot,
    history: PagedHistory,
    prepared: PreparedSourceTransaction,
}
impl SourceCommitLease<'_> {
    pub fn edits(&self) -> &[SourceEdit] {
        self.prepared.edits()
    }
    pub fn metadata(&self) -> &EditMetadata {
        &self.history.metadata
    }
    pub fn snapshot(&self) -> &PagedSnapshot {
        &self.next
    }
    pub fn next_revision(&self) -> Revision {
        self.next.revision
    }
    pub fn publish(self) -> Revision {
        let Self {
            document,
            next,
            history,
            prepared: _,
        } = self;
        let revision = next.revision;
        document.undo.push(history); // capacity reserved before lease was returned
        document.redo.clear();
        document.current = next;
        document.trim_history();
        revision
    }
}
fn product(a: usize, b: usize) -> Result<usize, Error> {
    a.checked_mul(b).ok_or(Error::BudgetExceeded)
}
fn sum(a: usize, b: usize) -> Result<usize, Error> {
    a.checked_add(b).ok_or(Error::BudgetExceeded)
}
fn selection_bytes(metadata: &EditMetadata) -> Result<usize, Error> {
    product(
        sum(metadata.before.capacity(), metadata.after.capacity())?,
        std::mem::size_of::<crate::history::Selection>(),
    )
}
impl PagedDocument {
    pub fn lease_source_transaction(
        &mut self,
        prepared: PreparedSourceTransaction,
    ) -> Result<SourceCommitLease<'_>, Error> {
        if !self.current.same_document(&prepared.snapshot) {
            return Err(Error::WrongDocument);
        }
        if self.current.revision != prepared.snapshot.revision
            || self.current.content_state != prepared.snapshot.content_state
        {
            return Err(Error::StaleRevision);
        }
        let revision = prepared.next_revision()?;

        // Nodes own separate RAII charges; this covers selection and history metadata.
        let history_bytes = sum(
            selection_bytes(&prepared.metadata)?,
            product(prepared.edits.len(), std::mem::size_of::<OwnedEdit>())?,
        )?;
        let reservation = crate::history::Charge::new(self.history.reserve(history_bytes)?);
        let mut edits = Vec::new();
        edits
            .try_reserve_exact(prepared.edits.len())
            .map_err(|_| Error::BudgetExceeded)?;
        let mut before = 0usize;
        let mut after = 0usize;
        for edit in &prepared.edits {
            let start = sum(after, edit.range.start.0 - before)?;
            let end = sum(
                start,
                (edit.inserted.range.end - edit.inserted.range.start) as usize,
            )?;
            let (prefix, _) =
                tree::charged_split(self.current.root.clone(), edit.range.end.0, &self.bytes)?;
            let (_, removed) = tree::charged_split(prefix, edit.range.start.0, &self.bytes)?;
            edits.push(OwnedEdit {
                before_range: edit.range.start.0..edit.range.end.0,
                after_range: start..end,
                inverse: inverse_root(&removed, &edit.inverse, &mut 0, &self.bytes)?,
                inserted: tree::charged_owned(
                    edit.inserted.source.clone(),
                    edit.inserted.range.clone(),
                    None,
                    &self.bytes,
                )?,
            });
            before = edit.range.end.0;
            after = end;
        }
        let mut root = self.current.root.clone();
        for edit in edits.iter().rev() {
            let (prefix, right) = tree::charged_split(root, edit.before_range.end, &self.bytes)?;
            let (left, _) = tree::charged_split(prefix, edit.before_range.start, &self.bytes)?;
            root = tree::charged_concat(
                tree::charged_concat(left, edit.inserted.clone(), &self.bytes)?,
                right,
                &self.bytes,
            )?;
        }
        self.undo
            .try_reserve_exact(1)
            .map_err(|_| Error::BudgetExceeded)?;
        let state = ContentStateId(crate::unique());
        let history = PagedHistory {
            before_metadata: self.current.metadata.clone(),
            after_metadata: self.current.metadata.clone(),
            typing_insert: false,
            metadata: prepared.metadata.clone(),
            edits,
            before_state: self.current.content_state,
            after_state: state,
            _reservation: reservation,
        };
        let mut next = self.current.clone();
        next.root = root;
        next.revision = revision;
        next.content_state = state;
        next.applied_change = Some(crate::change::AppliedChange::owned(
            self.current.document_id,
            self.current.revision,
            revision,
            self.current.content_state,
            state,
            crate::change::ChangeDirection::Edit,
            &history.edits,
            &self.bytes,
        )?);
        Ok(SourceCommitLease {
            document: self,
            next,
            history,
            prepared,
        })
    }
    /// Convenience for callers without a journal. Journal owners must use the lease.
    pub fn commit_source_transaction(
        &mut self,
        prepared: PreparedSourceTransaction,
    ) -> Result<Revision, Error> {
        Ok(self.lease_source_transaction(prepared)?.publish())
    }
}
/// Immutable history payload views. Window reads remain bounded and Pending-aware;
/// they never concatenate a transaction into a String. Both roots own their stores.
pub struct HistorySourceEdit {
    pub range: Range<TextOffset>,
    pub removed: PagedSnapshot,
    pub inserted: PagedSnapshot,
}
pub struct PreparedSourceHistory {
    snapshot: PagedSnapshot,
    undo: bool,
    before_state: ContentStateId,
    after_state: ContentStateId,
    edits: Vec<HistorySourceEdit>,
    _claim: BudgetClaim,
    _history_charge: crate::history::Charge,
}
impl PreparedSourceHistory {
    pub fn edits(&self) -> &[HistorySourceEdit] {
        &self.edits
    }
    pub fn base_revision(&self) -> Revision {
        self.snapshot.revision
    }
    pub fn next_revision(&self) -> Result<Revision, Error> {
        self.snapshot
            .revision
            .0
            .checked_add(1)
            .map(Revision)
            .ok_or(Error::RevisionOverflow)
    }
}
pub struct HistoryCommitLease<'a> {
    document: &'a mut PagedDocument,
    next: PagedSnapshot,
    prepared: PreparedSourceHistory,
}
impl HistoryCommitLease<'_> {
    pub fn edits(&self) -> &[HistorySourceEdit] {
        self.prepared.edits()
    }
    pub fn undo(&self) -> bool {
        self.prepared.undo
    }
    pub fn metadata(&self) -> &EditMetadata {
        &((if self.prepared.undo {
            self.document.undo.last()
        } else {
            self.document.redo.last()
        })
        .expect("exclusive history lease"))
        .metadata
    }
    pub fn snapshot(&self) -> &PagedSnapshot {
        &self.next
    }
    pub fn next_revision(&self) -> Revision {
        self.next.revision
    }
    pub fn publish(self) -> Revision {
        let Self {
            document,
            next,
            prepared,
        } = self;
        let (source, destination) = if prepared.undo {
            (&mut document.undo, &mut document.redo)
        } else {
            (&mut document.redo, &mut document.undo)
        };
        let mut entry = source.pop().expect("exclusive history lease");
        entry.typing_insert = false;
        if prepared.undo {
            if let Some(previous) = source.last_mut() {
                previous.typing_insert = false;
            }
        }
        destination.push(entry);
        let revision = next.revision;
        document.current = next;
        revision
    }
}
impl PagedDocument {
    pub fn prepare_source_history(
        &self,
        undo: bool,
        budget: &Budget,
    ) -> Result<PreparedSourceHistory, Error> {
        let entry = (if undo {
            self.undo.last()
        } else {
            self.redo.last()
        })
        .ok_or(Error::EmptyHistory)?;
        let claim = budget.claim(sum(
            product(entry.edits.len(), std::mem::size_of::<HistorySourceEdit>())?,
            entry._reservation.reference_bytes(),
        )?)?;
        let mut edits = Vec::new();
        edits
            .try_reserve_exact(entry.edits.len())
            .map_err(|_| Error::BudgetExceeded)?;
        for edit in &entry.edits {
            let (range, removed, inserted) = if undo {
                (&edit.after_range, &edit.inserted, &edit.inverse)
            } else {
                (&edit.before_range, &edit.inverse, &edit.inserted)
            };
            let mut removed_snapshot = self.current.clone();
            removed_snapshot.root = removed.clone();
            let mut inserted_snapshot = self.current.clone();
            inserted_snapshot.root = inserted.clone();
            edits.push(HistorySourceEdit {
                range: TextOffset(range.start)..TextOffset(range.end),
                removed: removed_snapshot,
                inserted: inserted_snapshot,
            });
        }
        Ok(PreparedSourceHistory {
            snapshot: self.current.clone(),
            undo,
            before_state: entry.before_state,
            after_state: entry.after_state,
            edits,
            _claim: claim,
            _history_charge: entry._reservation.clone(),
        })
    }
    pub fn lease_source_history(
        &mut self,
        prepared: PreparedSourceHistory,
    ) -> Result<HistoryCommitLease<'_>, Error> {
        if !self.current.same_document(&prepared.snapshot) {
            return Err(Error::WrongDocument);
        }
        if self.current.revision != prepared.snapshot.revision
            || self.current.content_state != prepared.snapshot.content_state
        {
            return Err(Error::StaleRevision);
        }
        let entry = (if prepared.undo {
            self.undo.last()
        } else {
            self.redo.last()
        })
        .ok_or(Error::EmptyHistory)?;
        if entry.before_state != prepared.before_state || entry.after_state != prepared.after_state
        {
            return Err(Error::StaleRevision);
        }
        let revision = prepared.next_revision()?;
        let mut root = self.current.root.clone();
        for edit in entry.edits.iter().rev() {
            let (range, inserted) = if prepared.undo {
                (&edit.after_range, &edit.inverse)
            } else {
                (&edit.before_range, &edit.inserted)
            };
            let (prefix, suffix) = tree::charged_split(root, range.end, &self.bytes)?;
            let (prefix, _) = tree::charged_split(prefix, range.start, &self.bytes)?;
            root = tree::charged_concat(
                tree::charged_concat(prefix, inserted.clone(), &self.bytes)?,
                suffix,
                &self.bytes,
            )?;
        }
        let mut next = self.current.clone();
        next.root = root;
        next.revision = revision;
        next.metadata = if prepared.undo {
            entry.before_metadata.clone()
        } else {
            entry.after_metadata.clone()
        };
        next.content_state = if prepared.undo {
            entry.before_state
        } else {
            entry.after_state
        };
        next.applied_change = Some(crate::change::AppliedChange::owned(
            self.current.document_id,
            self.current.revision,
            revision,
            self.current.content_state,
            next.content_state,
            if prepared.undo {
                crate::change::ChangeDirection::Undo
            } else {
                crate::change::ChangeDirection::Redo
            },
            &entry.edits,
            &self.bytes,
        )?);
        (if prepared.undo {
            &mut self.redo
        } else {
            &mut self.undo
        })
        .try_reserve_exact(1)
        .map_err(|_| Error::BudgetExceeded)?;
        Ok(HistoryCommitLease {
            document: self,
            next,
            prepared,
        })
    }
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    use crate::{
        paged::RestoredPiece,
        source::{Generation, OwnedPageLoader, SourceKind},
    };
    use std::sync::Arc;
    struct Repeat(u8);
    impl OwnedPageLoader for Repeat {
        fn read(&self, _: u64, output: &mut [u8]) -> std::io::Result<()> {
            output.fill(self.0);
            Ok(())
        }
    }
    fn source(length: u64, byte: u8, budget: &Budget) -> MemorySource {
        let generation = Generation(crate::unique());
        let (source, _publisher) = MemorySource::new(
            length,
            generation,
            SourceKind::Paged,
            4096,
            8192,
            budget.clone(),
        )
        .unwrap();
        source.attach_owned_loader(Arc::new(Repeat(byte))).unwrap();
        // Paged ownership is the immutable loader; Resident sealing requires every page.
        source
    }
    fn document(length: u64, bytes: &Budget, history: &Budget) -> PagedDocument {
        let base = source(length, b'a', bytes);
        PagedDocument::restore_pieces(
            base.clone(),
            vec![RestoredPiece::OwnedSource {
                source: base,
                range: 0..length,
                original: None,
            }],
            bytes.clone(),
            history.clone(),
            Revision(0),
        )
        .unwrap()
    }
    // Small tests construct a validated token directly, isolating the publication
    // contract from page validation. The large test below drives the real validator.
    fn token(document: &PagedDocument, bytes: &Budget) -> PreparedSourceTransaction {
        PreparedSourceTransaction {
            snapshot: document.snapshot(),
            edits: vec![SourceEdit {
                range: TextOffset(0)..TextOffset(document.snapshot().len()),
                inverse: OwnedTextRange {
                    source: source(document.snapshot().len() as u64, b'a', bytes),
                    range: 0..document.snapshot().len() as u64,
                },
                inserted: OwnedTextRange {
                    source: source(7, b'b', bytes),
                    range: 0..7,
                },
            }],
            metadata: EditMetadata::default(),
            _claim: bytes.claim(1024).unwrap(),
        }
    }
    #[test]
    fn lease_abort_and_quota_failure_leave_revision_and_history_unchanged() {
        let bytes = Budget::new(1024 * 1024);
        let history = Budget::new(1024 * 1024);
        let mut doc = document(20, &bytes, &history);
        let before = doc.snapshot();
        let prepared = token(&doc, &bytes);
        let initial = bytes.used() - 1024;
        drop(doc.lease_source_transaction(prepared).unwrap());
        assert_eq!(doc.snapshot().revision, before.revision);
        assert_eq!(doc.snapshot().content_state, before.content_state);
        assert_eq!(doc.history_stats().undo_changes, 0);
        assert_eq!(bytes.used(), initial);
        let prepared = token(&doc, &bytes);
        let full = bytes.claim(bytes.limit() - bytes.used()).unwrap();
        assert!(matches!(
            doc.lease_source_transaction(prepared),
            Err(Error::BudgetExceeded)
        ));
        drop(full);
        assert_eq!(doc.snapshot().revision, before.revision);
        assert_eq!(doc.history_stats().undo_changes, 0);
        assert_eq!(
            history.used(),
            doc.undo.capacity_bytes() + doc.redo.capacity_bytes()
        );
    }
    #[test]
    fn publish_and_history_publish_need_no_remaining_budget() {
        let bytes = Budget::new(1024 * 1024);
        let history = Budget::new(1024 * 1024);
        let mut doc = document(20, &bytes, &history);
        let prepared = token(&doc, &bytes);
        let lease = doc.lease_source_transaction(prepared).unwrap();
        let full = bytes.claim(bytes.limit() - bytes.used()).unwrap();
        let full_history = history.claim(history.limit() - history.used()).unwrap();
        assert_eq!(lease.publish(), Revision(1));
        drop(full);
        drop(full_history);
        let retained = doc.snapshot();
        let prepared = doc.prepare_source_history(true, &bytes).unwrap();
        let lease = doc.lease_source_history(prepared).unwrap();
        assert_eq!(lease.edits()[0].inserted.len(), 20);
        let full = bytes.claim(bytes.limit() - bytes.used()).unwrap();
        let full_history = history.claim(history.limit() - history.used()).unwrap();
        assert_eq!(lease.publish(), Revision(2));
        drop(full);
        drop(full_history);
        assert_eq!(doc.snapshot().len(), 20);
        assert_eq!(retained.len(), 7);
        doc.set_history_policy(crate::history::HistoryPolicy {
            max_changes: 0,
            ..Default::default()
        });
        drop(doc);
        let retained_usage = bytes.used();
        assert!(retained_usage > 0);
        drop(retained);
        assert_eq!(bytes.used(), 0);
        assert_eq!(history.used(), 0);
    }
    #[test]
    fn large_validated_splice_undo_retains_ranges_without_payload_materialization() {
        let bytes = Budget::new(2 * 1024 * 1024);
        let history = Budget::new(64 * 1024);
        let mut doc = document(8, &bytes, &history);
        let old = doc.snapshot();
        let inserted = source(20 * 1024 * 1024, b'z', &bytes);
        let edit = SourceEdit {
            range: TextOffset(0)..TextOffset(8),
            inverse: OwnedTextRange {
                source: source(8, b'a', &bytes),
                range: 0..8,
            },
            inserted: OwnedTextRange {
                source: inserted,
                range: 0..20 * 1024 * 1024,
            },
        };
        let mut request = old
            .prepare_source_transaction(vec![edit], EditMetadata::default(), bytes.clone())
            .unwrap();
        let prepared = loop {
            match request.poll() {
                SourceTransactionPoll::Ready(prepared) => break prepared,
                SourceTransactionPoll::Pending(ticket) => {
                    assert!(request.resolve_owned(ticket).unwrap())
                }
                SourceTransactionPoll::Progress => {}
                _ => panic!("unexpected validation outcome"),
            }
        };
        doc.lease_source_transaction(prepared).unwrap().publish();
        assert_eq!(doc.snapshot().len(), 20 * 1024 * 1024);
        let prepared = doc.prepare_source_history(true, &bytes).unwrap();
        assert_eq!(prepared.edits()[0].removed.len(), 20 * 1024 * 1024);
        assert!(bytes.used() < 1024 * 1024);
        doc.lease_source_history(prepared).unwrap().publish();
        assert_eq!(doc.snapshot().len(), 8);
        doc.redo().unwrap();
        assert_eq!(doc.snapshot().len(), 20 * 1024 * 1024);
    }
}
