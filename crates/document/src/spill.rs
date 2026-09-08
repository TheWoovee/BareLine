// SPDX-License-Identifier: MPL-2.0
//! Two-phase immutable segment spill. Capture owns roots; only an exact actor may attach.
use crate::{
    Budget, BudgetClaim, ContentStateId, Document, DocumentSnapshot, Error, Revision,
    history::HistoryPolicy,
    paged::{PagedDocument, PagedHistory, PagedSnapshot},
    source::MemorySource,
    tree,
};
use std::{collections::BTreeMap, ops::Range, sync::Arc};
pub struct SpillSegment<'a> {
    pub id: u64,
    pub text: &'a str,
}
pub struct StoredSegment {
    pub id: u64,
    pub range: Range<u64>,
    /// Original decoded-text provenance for Resident baseline chunks; typed bytes stay None.
    pub original: Option<(MemorySource, Range<u64>)>,
}
#[derive(Clone, Copy)]
struct Stamp {
    document_id: u64,
    revision: Revision,
    state: ContentStateId,
    saved: ContentStateId,
    undo: usize,
    redo: usize,
}
pub struct SpillPlan {
    snapshot: PagedSnapshot,
    saved: ContentStateId,
    policy: HistoryPolicy,
    undo: Vec<PagedHistory>,
    redo: Vec<PagedHistory>,
    segments: BTreeMap<u64, Arc<tree::Segment>>,
    bytes: Budget,
    history: Budget,
    _claims: Vec<BudgetClaim>,
}
pub struct PreparedSpill {
    stamp: Stamp,
    document: PagedDocument,
}
impl SpillPlan {
    pub fn segments(&self) -> impl Iterator<Item = SpillSegment<'_>> {
        self.segments.iter().map(|(id, segment)| SpillSegment {
            id: *id,
            text: &segment.text,
        })
    }
    pub fn matches_resident(&self, snapshot: &DocumentSnapshot) -> bool {
        self.snapshot.document_id == snapshot.document_id
            && self.snapshot.revision == snapshot.revision
            && self.snapshot.content_state == snapshot.content_state
    }
    pub fn snapshot(&self) -> &PagedSnapshot {
        &self.snapshot
    }
    pub(crate) fn resident(document: &Document) -> Result<Self, Error> {
        if document
            .undo
            .iter()
            .chain(&document.redo)
            .any(|entry| entry.group.is_some())
        {
            return Err(Error::LinkedUndoRequired);
        }
        if !document.current.complete {
            return Err(Error::IncompleteSource);
        }
        let snapshot = PagedSnapshot {
            metadata: document.current.metadata.clone(),
            root: document.current.root.clone(),
            revision: document.current.revision,
            content_state: document.current.content_state,
            document_id: document.current.document_id,
            _structure: None,
        };
        let convert = |entry: &crate::History| PagedHistory {
            before_metadata: entry.before_metadata.clone(),
            after_metadata: entry.after_metadata.clone(),
            typing_insert: entry.typing_insert,
            metadata: entry.metadata.clone(),
            edits: entry.edits.clone(),
            before_state: entry.before_state,
            after_state: entry.after_state,
            _reservation: entry._undo_reservation.clone(),
        };
        Self::capture(
            snapshot,
            document.saved_state,
            document.history_policy,
            document.undo.iter().map(convert).collect(),
            document.redo.iter().map(convert).collect(),
            document.bytes.clone(),
            document.history.clone(),
        )
    }
    pub(crate) fn paged(document: &PagedDocument) -> Result<Self, Error> {
        Self::capture(
            document.current.clone(),
            document.saved_state,
            document.history_policy,
            document.undo.to_vec(),
            document.redo.to_vec(),
            document.bytes.clone(),
            document.history.clone(),
        )
    }
    fn capture(
        snapshot: PagedSnapshot,
        saved: ContentStateId,
        policy: HistoryPolicy,
        undo: Vec<PagedHistory>,
        redo: Vec<PagedHistory>,
        bytes: Budget,
        history: Budget,
    ) -> Result<Self, Error> {
        let entry_count = undo
            .len()
            .checked_add(redo.len())
            .ok_or(Error::BudgetExceeded)?;
        let charge = bytes.claim(
            entry_count
                .checked_mul(std::mem::size_of::<PagedHistory>())
                .ok_or(Error::BudgetExceeded)?,
        )?;
        let mut plan = Self {
            snapshot,
            saved,
            policy,
            undo,
            redo,
            segments: BTreeMap::new(),
            bytes,
            history,
            _claims: vec![charge],
        };
        let mut roots = vec![plan.snapshot.root.clone()];
        for entry in plan.undo.iter().chain(&plan.redo) {
            for edit in &entry.edits {
                roots.push(edit.inverse.clone());
                roots.push(edit.inserted.clone());
            }
        }
        for root in roots {
            let mut stack: Vec<_> = root.into_iter().collect();
            while let Some(node) = stack.pop() {
                match node.as_ref() {
                    tree::Node::Leaf(piece) => {
                        let id = Arc::as_ptr(&piece.segment) as usize as u64;
                        if !plan.segments.contains_key(&id) {
                            plan._claims.push(plan.bytes.claim(128)?);
                            plan.segments.insert(id, piece.segment.clone());
                        }
                    }
                    tree::Node::Branch { left, right, .. } => {
                        stack.push(right.clone());
                        stack.push(left.clone());
                    }
                    tree::Node::Source { .. } | tree::Node::OwnedSource { .. } => {}
                }
            }
        }
        Ok(plan)
    }
    /// The producer must have flushed every mapped segment and attached its owned loader.
    /// No actor state is touched during conversion; stale attachment simply drops this result.
    pub fn prepare(
        self,
        source: MemorySource,
        stored: Vec<StoredSegment>,
    ) -> Result<PreparedSpill, Error> {
        if !source.has_owned_loader() {
            return Err(Error::IncompleteSource);
        }
        if stored.len() != self.segments.len() {
            return Err(Error::IncompleteSource);
        }
        let mut map = BTreeMap::new();
        for segment in stored {
            let original = self.segments.get(&segment.id).ok_or(Error::WrongDocument)?;
            if segment.range.start > segment.range.end
                || segment.range.end > source.len()
                || segment.range.end - segment.range.start != original.text.len() as u64
                || segment.original.as_ref().is_some_and(|(source, range)| {
                    range.start > range.end
                        || range.end > source.len()
                        || range.end - range.start != original.text.len() as u64
                })
                || map.insert(segment.id, segment).is_some()
            {
                return Err(Error::OutOfBounds);
            }
        }
        fn convert(
            root: &tree::Root,
            source: &MemorySource,
            map: &BTreeMap<u64, StoredSegment>,
            budget: &Budget,
        ) -> Result<tree::Root, Error> {
            let Some(node) = root else {
                return Ok(None);
            };
            match node.as_ref() {
                tree::Node::Leaf(piece) => {
                    let stored = &map[&(Arc::as_ptr(&piece.segment) as usize as u64)];
                    let original = piece
                        .origin()
                        .map(|(source, range)| (source.clone(), range))
                        .or_else(|| {
                            stored.original.as_ref().map(|(source, range)| {
                                (
                                    source.clone(),
                                    range.start + piece.range.start as u64
                                        ..range.start + piece.range.end as u64,
                                )
                            })
                        });
                    Ok(Some(tree::charged_node(
                        tree::Node::OwnedSource {
                            _charge: None,
                            source: source.clone(),
                            range: stored.range.start + piece.range.start as u64
                                ..stored.range.start + piece.range.end as u64,
                            original,
                            summary: piece.summary,
                        },
                        budget,
                    )?))
                }
                tree::Node::Branch { left, right, .. } => tree::charged_concat(
                    convert(&Some(left.clone()), source, map, budget)?,
                    convert(&Some(right.clone()), source, map, budget)?,
                    budget,
                ),
                tree::Node::Source { .. } | tree::Node::OwnedSource { .. } => {
                    Ok(Some(node.clone()))
                }
            }
        }
        let stamp = Stamp {
            document_id: self.snapshot.document_id,
            revision: self.snapshot.revision,
            state: self.snapshot.content_state,
            saved: self.saved,
            undo: self.undo.len(),
            redo: self.redo.len(),
        };
        let mut snapshot = self.snapshot.clone();
        snapshot.root = convert(&snapshot.root, &source, &map, &self.bytes)?;
        let mut document = PagedDocument::new(snapshot, self.bytes.clone(), self.history.clone());
        document.saved_state = self.saved;
        document.history_policy = self.policy;
        let history = |entries: Vec<PagedHistory>| {
            entries
                .into_iter()
                .map(|mut entry| {
                    for edit in &mut entry.edits {
                        edit.inverse = convert(&edit.inverse, &source, &map, &self.bytes)?;
                        edit.inserted = convert(&edit.inserted, &source, &map, &self.bytes)?;
                    }
                    Ok(entry)
                })
                .collect::<Result<Vec<_>, Error>>()
        };
        document.undo =
            crate::history::HistoryStack::from_vec(history(self.undo)?, self.history.clone())?;
        document.redo =
            crate::history::HistoryStack::from_vec(history(self.redo)?, self.history.clone())?;
        Ok(PreparedSpill { stamp, document })
    }
}
impl PreparedSpill {
    pub(crate) fn attach_resident(self, document: &Document) -> Result<PagedDocument, Error> {
        self.validate(
            document.current.document_id,
            document.current.revision,
            document.current.content_state,
            document.saved_state,
            document.undo.len(),
            document.redo.len(),
        )?;
        Ok(self.document)
    }
    pub(crate) fn attach_paged(self, document: &PagedDocument) -> Result<PagedDocument, Error> {
        self.validate(
            document.current.document_id,
            document.current.revision,
            document.current.content_state,
            document.saved_state,
            document.undo.len(),
            document.redo.len(),
        )?;
        Ok(self.document)
    }
    fn validate(
        &self,
        document_id: u64,
        revision: Revision,
        state: ContentStateId,
        saved: ContentStateId,
        undo: usize,
        redo: usize,
    ) -> Result<(), Error> {
        if self.stamp.document_id != document_id {
            return Err(Error::WrongDocument);
        }
        if self.stamp.revision != revision
            || self.stamp.state != state
            || self.stamp.saved != saved
            || self.stamp.undo != undo
            || self.stamp.redo != redo
        {
            return Err(Error::StaleRevision);
        }
        Ok(())
    }
    pub fn matches_resident(&self, snapshot: &DocumentSnapshot) -> bool {
        self.stamp.document_id == snapshot.document_id
            && self.stamp.revision == snapshot.revision
            && self.stamp.state == snapshot.content_state
    }
}
