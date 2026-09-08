// SPDX-License-Identifier: MPL-2.0
//! Bounded validation and atomic splicing of sealed text stores. No disk I/O in commit.
use crate::{
    Budget, BudgetClaim, ContentStateId, Error, Revision, TextOffset,
    history::{EditMetadata, OwnedEdit},
    paged::{PagedDocument, PagedHistory, PagedSnapshot, TextWindow, WindowPoll, WindowRequest},
    source::{MemorySource, PageTicket, Unavailable},
    tree,
};
use std::{ops::Range, sync::Arc};
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
        let claim = budget.claim(
            edits
                .len()
                .checked_mul(std::mem::size_of::<SourceEdit>() + 128)
                .ok_or(Error::BudgetExceeded)?,
        )?;
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
        self.edits.clear();
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
fn inverse_root(root: &tree::Root, inverse: &OwnedTextRange, cursor: &mut u64) -> tree::Root {
    let Some(node) = root else {
        return None;
    };
    match node.as_ref() {
        tree::Node::Branch { left, right, .. } => tree::concat(
            inverse_root(&Some(left.clone()), inverse, cursor),
            inverse_root(&Some(right.clone()), inverse, cursor),
        ),
        _ => {
            let bytes = node.summary().bytes as u64;
            let start = *cursor;
            *cursor += bytes;
            let original = match node.as_ref() {
                tree::Node::Source { source, range } => Some((source.clone(), range.clone())),
                tree::Node::OwnedSource { original, .. } => original.clone(),
                tree::Node::Leaf(piece) => piece
                    .origin()
                    .map(|(source, range)| (source.clone(), range)),
                tree::Node::Branch { .. } => unreachable!(),
            };
            Some(Arc::new(tree::Node::OwnedSource {
                source: inverse.source.clone(),
                range: inverse.range.start + start..inverse.range.start + start + bytes,
                original,
                summary: node.summary(),
            }))
        }
    }
}
impl PagedDocument {
    pub fn commit_source_transaction(
        &mut self,
        prepared: PreparedSourceTransaction,
    ) -> Result<Revision, Error> {
        if !self.current.same_document(&prepared.snapshot) {
            return Err(Error::WrongDocument);
        }
        if self.current.revision != prepared.snapshot.revision
            || self.current.content_state != prepared.snapshot.content_state
        {
            return Err(Error::StaleRevision);
        }
        let revision = prepared.next_revision()?;
        let reservation = crate::history::Charge::new(
            self.history.reserve(
                prepared
                    .edits
                    .len()
                    .checked_mul(std::mem::size_of::<OwnedEdit>() + 256)
                    .ok_or(Error::BudgetExceeded)?,
            )?,
        );
        let mut edits = Vec::with_capacity(prepared.edits.len());
        let mut before = 0usize;
        let mut after = 0usize;
        for edit in &prepared.edits {
            let start = after
                .checked_add(edit.range.start.0 - before)
                .ok_or(Error::BudgetExceeded)?;
            let end = start
                .checked_add((edit.inserted.range.end - edit.inserted.range.start) as usize)
                .ok_or(Error::BudgetExceeded)?;
            let (prefix, _) = tree::split(self.current.root.clone(), edit.range.end.0);
            let (_, removed) = tree::split(prefix, edit.range.start.0);
            edits.push(OwnedEdit {
                before_range: edit.range.start.0..edit.range.end.0,
                after_range: start..end,
                inverse: inverse_root(&removed, &edit.inverse, &mut 0),
                inserted: tree::from_owned_source(
                    edit.inserted.source.clone(),
                    edit.inserted.range.clone(),
                    None,
                ),
            });
            before = edit.range.end.0;
            after = end;
        }
        let mut root = self.current.root.clone();
        for edit in edits.iter().rev() {
            let (prefix, right) = tree::split(root, edit.before_range.end);
            let (left, _) = tree::split(prefix, edit.before_range.start);
            root = tree::concat(tree::concat(left, edit.inserted.clone()), right);
        }
        self.undo
            .try_reserve(1)
            .map_err(|_| Error::BudgetExceeded)?;
        let state = ContentStateId(crate::unique());
        self.undo.push(PagedHistory {
            before_metadata: self.current.metadata.clone(),
            after_metadata: self.current.metadata.clone(),
            typing_insert: false,
            metadata: prepared.metadata,
            edits,
            before_state: self.current.content_state,
            after_state: state,
            _reservation: reservation,
        });
        self.redo.clear();
        self.current.root = root;
        self.current.revision = revision;
        self.current.content_state = state;
        self.trim_history();
        Ok(revision)
    }
}
