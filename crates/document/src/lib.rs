// SPDX-License-Identifier: MPL-2.0
//! Resident UTF-8 document core. All published bytes are owned and immutable.
pub mod change;
pub mod group;
pub mod history;
pub mod line_lookup;
pub mod metadata;
pub mod paged;
pub mod paged_group;
pub mod service;
pub mod source;
pub mod source_transaction;
pub mod spill;
mod tree;
pub use metadata::DocumentMetadata;
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
    limit: std::sync::Mutex<usize>,
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
            limit: std::sync::Mutex::new(limit),
            used: AtomicUsize::new(0),
        }))
    }
    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Relaxed)
    }
    pub fn limit(&self) -> usize {
        *self.0.limit.lock().unwrap_or_else(|error| error.into_inner())
    }
    /// Retains every live claim. A lowered cap blocks new reservations until
    /// owners release enough bytes; all clones observe the same admission cap.
    pub fn set_limit(&self, limit: usize) {
        *self.0.limit.lock().unwrap_or_else(|error| error.into_inner()) = limit;
    }
    fn available(&self) -> usize {
        self.limit().saturating_sub(self.used())
    }
    /// Whether both handles meter the same shared allowance.
    pub(crate) fn same(&self, other: &Budget) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    fn reserve(&self, bytes: usize) -> Result<Reservation, Error> {
        let limit = self.0.limit.lock().unwrap_or_else(|error| error.into_inner());
        self.0
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|n| *n <= *limit)
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
#[cfg(test)]
mod budget_limit_tests {
    #[test]
    fn lowering_shared_limit_preserves_live_claims_and_reopens_after_release() {
        let budget = super::Budget::new(100);
        let peer = budget.clone();
        let claim = budget.claim(80).unwrap();
        peer.set_limit(40);
        assert_eq!(budget.used(), 80);
        assert_eq!(budget.limit(), 40);
        assert!(budget.claim(1).is_err());
        drop(claim);
        let retained = peer.claim(40).unwrap();
        assert!(budget.claim(1).is_err());
        drop(retained);
        assert_eq!(budget.used(), 0);
    }
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
    applied_change: Option<Arc<change::AppliedChange>>,
    metadata: DocumentMetadata,
    root: tree::Root,
    pub revision: Revision,
    pub content_state: ContentStateId,
    document_id: u64,
    complete: bool,
}
impl DocumentSnapshot {
    pub fn applied_change(&self) -> Option<&Arc<change::AppliedChange>> {
        self.applied_change.as_ref()
    }
    pub fn metadata(&self) -> &DocumentMetadata {
        &self.metadata
    }
    /// Opaque source token for validating queued external actions; forks have distinct identities.
    pub fn identity_token(&self) -> (u64, u64) {
        (self.document_id, self.revision.0)
    }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn same_document(&self, other: &Self) -> bool {
        self.document_id == other.document_id
    }
    pub fn eol_label(&self) -> &'static str {
        let summary = tree::summary(&self.root);
        match (summary.cr > 0, summary.lf > 0, summary.crlf > 0) {
            (false, false, false) => match self.metadata.get("file.new_document_eol") {
                Some("crlf") => "CRLF",
                Some("cr") => "CR",
                _ => "LF",
            },
            (false, true, false) => "LF",
            (true, false, false) => "CR",
            (false, false, true) => "CRLF",
            _ => "Mixed",
        }
    }
    /// Text inserted by Enter. Existing line endings take precedence over the
    /// creation-time fallback; callers inserting/pasting literal text bypass it.
    /// A Mixed document uses its most frequent ending (ties prefer CRLF, then LF),
    /// so edits do not spread a minority ending.
    pub fn insertion_eol(&self) -> &'static str {
        match self.eol_label() {
            "CRLF" => "\r\n",
            "CR" => "\r",
            "Mixed" => {
                let summary = tree::summary(&self.root);
                if summary.crlf >= summary.lf && summary.crlf >= summary.cr {
                    "\r\n"
                } else if summary.lf >= summary.cr {
                    "\n"
                } else {
                    "\r"
                }
            }
            _ => "\n",
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
/// Transactions up to this many edits keep one tree piece and receipt entry per edit;
/// larger ones (Replace All, huge multi-caret edits) coalesce clustered edits.
const PIECEWISE_EDITS: usize = 10_000;
/// Longest unchanged gap copied to join two neighboring edits into one range edit.
const COALESCE_GAP: usize = 256;
/// Bounds the replacement text staged for one coalesced range edit.
const COALESCE_BYTES: usize = 1024 * 1024;
/// Typed text joins the preceding small owned leaf up to this size, so a typing run
/// grows one leaf instead of adding a leaf and its tree path per keystroke.
const TYPED_LEAF_BYTES: usize = 256;
/// History charge per tree node an entry keeps alive: the node and its `Arc` counts.
const NODE_BYTES: usize = std::mem::size_of::<tree::Node>() + 2 * std::mem::size_of::<usize>();
/// Leaves and branches one edit adds besides the copied root-to-leaf paths.
const NODES_PER_EDIT: usize = 4;
/// Relief passes before a document's own entries give way to its edit: a peer that
/// another worker holds during one pass is usually free again in a later one.
const RELIEF_PASSES: usize = 4;
/// Room a validated mutation needs before it is charged, and the budget that meters it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Demand {
    /// A new undo entry charging `history` bytes to the history budget.
    Edit {
        history: usize,
    },
    Undo,
    Redo,
    /// Byte-budget room for the text an edit stages. Eviction frees only text that history
    /// alone keeps alive (replaced text of undo entries, undone text of redo entries); a
    /// shortfall of live text evicts nothing and the edit fails.
    Text {
        bytes: usize,
    },
}
impl Demand {
    /// Entries at the top of (undo, redo) that the mutation itself consumes.
    fn keep(self) -> (usize, usize) {
        match self {
            Self::Edit { .. } | Self::Text { .. } => (0, 0),
            Self::Undo => (1, 0),
            Self::Redo => (0, 1),
        }
    }
}
pub struct EditTransaction {
    pub base_revision: Revision,
    pub edits: Vec<Edit>,
}
struct History {
    before_metadata: DocumentMetadata,
    after_metadata: DocumentMetadata,
    before: tree::Root,
    after: tree::Root,
    before_state: ContentStateId,
    after_state: ContentStateId,
    /// Selection metadata; a typing merge replaces it. Inserted and deleted text is not
    /// charged here: the tree segments holding it are charged to the byte budget.
    _undo_reservation: history::Charge,
    /// Edit records and the tree nodes this entry's roots keep alive. A typing merge
    /// replaces it because the intermediate roots it covered are released. Shared so a
    /// spilled copy of this entry keeps the same charge.
    _structure_charge: Option<Arc<Reservation>>,
    /// Creation order across all documents; eviction drops the smallest first.
    sequence: u64,
    edits: Vec<history::OwnedEdit>,
    group: Option<group::GroupTag>,
    metadata: history::EditMetadata,
    typing_insert: bool,
}
impl History {
    /// Byte-budget text dropping this entry releases once the entries older than it are
    /// gone: the replaced text of an undo entry, or the undone text of a redo entry
    /// (`undone`). Text a live piece shares is never counted. Stops once `enough` is found.
    fn retained_text(&self, undone: bool, enough: usize) -> usize {
        let mut found = 0usize;
        for edit in &self.edits {
            if found >= enough {
                break;
            }
            let root = if undone { &edit.inserted } else { &edit.inverse };
            found = found.saturating_add(tree::exclusive_text(root, enough - found));
        }
        found
    }
}
/// Validated, budget-reserved roots; dropping this token leaves the document unchanged.
pub struct PreparedEdit {
    change: Arc<change::AppliedChange>,
    document_id: u64,
    base_revision: Revision,
    revision: Revision,
    entry: History,
    /// False when no history charge could be admitted even with this document's history
    /// evicted: the edit still applies, history is cleared and the completion says so.
    tracked: bool,
}
pub struct Document {
    current: DocumentSnapshot,
    saved_state: ContentStateId,
    history_policy: history::HistoryPolicy,
    undo: history::HistoryStack<History>,
    redo: history::HistoryStack<History>,
    bytes: Budget,
    history: Budget,
    last_merged: bool,
    last_untracked: bool,
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
    pub fn fork_from_snapshot(snapshot: &DocumentSnapshot, bytes: Budget, history: Budget) -> Result<Self, Error> {
        if !snapshot.is_complete() {
            return Err(Error::IncompleteSource);
        }
        let state = ContentStateId(unique());
        Ok(Self {
            current: DocumentSnapshot {
                applied_change: None,
                metadata: snapshot.metadata.clone(),
                root: snapshot.root.clone(),
                revision: Revision(0),
                content_state: state,
                document_id: unique(),
                complete: true,
            },
            saved_state: state,
            history_policy: history::HistoryPolicy::default(),
            undo: crate::history::HistoryStack::new(history.clone()),
            redo: crate::history::HistoryStack::new(history.clone()),
            bytes,
            history,
            last_merged: false,
            last_untracked: false,
        })
    }
    /// Budgets are shared across all documents created by the application.
    pub fn from_utf8(text: &str, bytes: Budget, history: Budget) -> Result<Self, Error> {
        let root = tree::from_text(text, &bytes)?;
        let state = ContentStateId(unique());
        Ok(Self {
            current: DocumentSnapshot {
                applied_change: None,
                metadata: DocumentMetadata::default(),
                root,
                revision: Revision(0),
                content_state: state,
                document_id: unique(),
                complete: true,
            },
            saved_state: state,
            history_policy: history::HistoryPolicy::default(),
            undo: crate::history::HistoryStack::new(history.clone()),
            redo: crate::history::HistoryStack::new(history.clone()),
            bytes,
            history,
            last_merged: false,
            last_untracked: false,
        })
    }
    /// Initialize opening policy before publishing this actor; does not create a user edit.
    pub fn initialize_metadata(&mut self, metadata: DocumentMetadata) -> Result<(), Error> {
        if self.dirty() || !self.undo.is_empty() || !self.redo.is_empty() {
            return Err(Error::ActorBusy);
        }
        self.current.metadata = metadata;
        Ok(())
    }
    pub fn apply_metadata(&mut self, base_revision: Revision, metadata: DocumentMetadata) -> Result<Revision, Error> {
        self.apply_metadata_relieved(base_revision, metadata, &mut Self::relieve_history)
    }
    /// `relieve` may evict history for the entry once the change is validated and staged.
    pub(crate) fn apply_metadata_relieved(
        &mut self,
        base_revision: Revision,
        metadata: DocumentMetadata,
        relieve: &mut dyn FnMut(&mut Self, Demand),
    ) -> Result<Revision, Error> {
        if base_revision != self.current.revision {
            return Err(Error::StaleRevision);
        }
        if metadata == self.current.metadata {
            return Ok(base_revision);
        }
        let revision = self.next_revision()?;
        let state = ContentStateId(unique());
        let change = change::AppliedChange::owned(
            self.current.document_id,
            self.current.revision,
            revision,
            self.current.content_state,
            state,
            change::ChangeDirection::Edit,
            &[],
            &self.bytes,
        )?;
        let entry_bytes = metadata.charge().saturating_add(128);
        relieve(&mut *self, Demand::Edit { history: entry_bytes });
        let charge = history::Charge::new(self.history.reserve(entry_bytes)?);
        self.undo.try_reserve(1).map_err(|_| Error::BudgetExceeded)?;
        let entry = History {
            before_metadata: self.current.metadata.clone(),
            after_metadata: metadata.clone(),
            before: self.current.root.clone(),
            after: self.current.root.clone(),
            before_state: self.current.content_state,
            after_state: state,
            _undo_reservation: charge,
            _structure_charge: None,
            sequence: unique(),
            edits: Vec::new(),
            group: None,
            metadata: history::EditMetadata::default(),
            typing_insert: false,
        };
        self.current.applied_change = Some(change);
        self.undo.push(entry);
        self.redo.clear();
        self.current.metadata = metadata;
        self.current.revision = revision;
        self.current.content_state = state;
        self.trim_history();
        Ok(revision)
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
    /// Record the content state a save wrote. Typing never merges across it, so undo
    /// and redo can return to exactly the saved text.
    pub fn mark_saved_state(&mut self, state: ContentStateId) {
        self.saved_state = state;
    }
    /// Whether the latest committed edit extended the previous undo entry instead of
    /// adding one. Views mirror this rather than guessing merges themselves.
    pub fn last_edit_merged(&self) -> bool {
        self.last_merged
    }
    /// Whether the latest committed edit applied without an undo entry because no
    /// history could be admitted even after evicting this document's own. Its undo
    /// history was cleared with it; views must tell the user.
    pub fn last_edit_untracked(&self) -> bool {
        self.last_untracked
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
        self.apply_relieved(transaction, None, &mut Self::relieve_history)
    }
    pub fn apply_with_metadata(
        &mut self,
        transaction: EditTransaction,
        metadata: history::EditMetadata,
    ) -> Result<Revision, Error> {
        self.apply_relieved(transaction, Some(metadata), &mut Self::relieve_history)
    }
    /// Validate and stage an edit, let `relieve` evict history for its text and its charge,
    /// then commit. Nothing is evicted for an edit that is rejected. An entry that cannot be
    /// charged even with this document's history evicted applies without history instead
    /// of refusing the user's edit, and `last_edit_untracked` reports it.
    pub(crate) fn apply_relieved(
        &mut self,
        mut transaction: EditTransaction,
        metadata: Option<history::EditMetadata>,
        relieve: &mut dyn FnMut(&mut Self, Demand),
    ) -> Result<Revision, Error> {
        let Some(metadata) = metadata else {
            if transaction.base_revision != self.current.revision {
                return Err(Error::StaleRevision);
            }
            if transaction.edits.is_empty() {
                return Ok(self.current.revision);
            }
            self.relieve_text(&mut transaction, None, relieve)?;
            let mut prepared = self.stage(transaction)?;
            self.admit(&mut prepared, 0, relieve);
            self.validate_prepared(&prepared)?;
            return Ok(self.commit_prepared_unchecked(prepared));
        };
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
        let selections = metadata.before.len().saturating_add(metadata.after.len());
        self.relieve_text(&mut transaction, Some(&metadata), relieve)?;
        let mut prepared = self.stage(transaction)?;
        metadata.validate(self.current.len(), tree::summary(&prepared.entry.after).bytes)?;
        for selection in &metadata.before {
            if !self.current.is_boundary(selection.anchor) || !self.current.is_boundary(selection.caret) {
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
        self.admit(&mut prepared, selections, relieve);
        prepared.entry.metadata = metadata;
        prepared.entry.typing_insert = typing_insert;
        self.validate_prepared(&prepared)?;
        Ok(self.commit_prepared_unchecked(prepared))
    }
    /// When the byte budget cannot hold the text `transaction` stages, validate it (and the
    /// cheap parts of `metadata`) and let `relieve` evict history that alone keeps replaced
    /// or undone text alive. A shortfall of live text evicts nothing, and staging then
    /// fails as before. Only a malformed after-selection, which needs the staged tree to
    /// check, can still be rejected after such relief.
    fn relieve_text(
        &mut self,
        transaction: &mut EditTransaction,
        metadata: Option<&history::EditMetadata>,
        relieve: &mut dyn FnMut(&mut Self, Demand),
    ) -> Result<(), Error> {
        let demand = Demand::Text {
            bytes: Self::text_need(transaction),
        };
        if !self.lacks_room(demand) {
            return Ok(());
        }
        self.check_transaction(transaction)?;
        if let Some(metadata) = metadata {
            let (removed, inserted) = transaction
                .edits
                .iter()
                .fold((0usize, 0usize), |(removed, inserted), edit| {
                    (
                        removed.saturating_add(edit.range.end.0 - edit.range.start.0),
                        inserted.saturating_add(edit.insert.len()),
                    )
                });
            let after_len = self.current.len().saturating_sub(removed).saturating_add(inserted);
            metadata.validate(self.current.len(), after_len)?;
            for selection in &metadata.before {
                if !self.current.is_boundary(selection.anchor) || !self.current.is_boundary(selection.caret) {
                    return Err(Error::InvalidBoundary);
                }
            }
        }
        relieve(&mut *self, demand);
        Ok(())
    }
    /// Byte-budget room staging `transaction` takes: its inserted text and its receipt.
    fn text_need(transaction: &EditTransaction) -> usize {
        transaction
            .edits
            .iter()
            .fold(0usize, |sum, edit| sum.saturating_add(edit.insert.len()))
            .saturating_add(
                transaction
                    .edits
                    .len()
                    .saturating_mul(std::mem::size_of::<change::CompactEdit>()),
            )
            .saturating_add(std::mem::size_of::<change::AppliedChange>() + 2 * std::mem::size_of::<usize>())
    }
    /// Admit a staged entry's undo slot and history charge. `relieve` evicts for it; when
    /// the charge still fails (another worker may have taken the room relief made, or held
    /// a peer relief had to skip), relief runs up to `RELIEF_PASSES` times in all before
    /// this document's own oldest entries give way, only as many as the charge needs, so
    /// the edit itself stays undoable. `prepared` stays untracked only when even an empty
    /// history cannot admit it; its history is then cleared anyway.
    fn admit(&mut self, prepared: &mut PreparedEdit, selections: usize, relieve: &mut dyn FnMut(&mut Self, Demand)) {
        let demand = self.edit_demand(prepared, selections);
        relieve(&mut *self, demand);
        let mut passes = 1;
        loop {
            if self.undo.try_reserve(1).is_ok() && self.charge(prepared, selections).is_ok() {
                return;
            }
            if passes < RELIEF_PASSES {
                passes += 1;
                // Let a worker holding a peer finish its step before the next pass.
                std::thread::yield_now();
                relieve(&mut *self, demand);
            } else if !self.evict_oldest_history((0, 0)) {
                return;
            }
        }
    }
    pub fn set_history_policy(&mut self, policy: history::HistoryPolicy) {
        self.history_policy = policy;
        self.trim_history();
    }
    /// Structure charge of one undo entry for `edits`: edit records and the tree nodes the
    /// entry keeps alive. Text is not charged to history at all: the segments holding the
    /// deleted text (before-root) and the inserted text (after-root) are already charged
    /// to the byte budget, so charging them again would refuse edits that fit.
    fn entry_charge(&self, edits: &[history::OwnedEdit]) -> usize {
        let nodes = tree::height(&self.current.root)
            .saturating_mul(3)
            .saturating_add(edits.len().saturating_mul(NODES_PER_EDIT));
        nodes
            .saturating_mul(NODE_BYTES)
            .saturating_add(edits.len().saturating_mul(std::mem::size_of::<history::OwnedEdit>()))
    }
    pub(crate) fn edit_demand(&self, prepared: &PreparedEdit, selections: usize) -> Demand {
        Demand::Edit {
            history: self
                .entry_charge(&prepared.entry.edits)
                .saturating_add(selections.saturating_mul(std::mem::size_of::<history::Selection>())),
        }
    }
    /// Charge a staged entry's history. On failure the entry stays untracked and holds
    /// no history charge.
    pub(crate) fn charge(&self, prepared: &mut PreparedEdit, selections: usize) -> Result<(), Error> {
        let mut charge = history::Charge::empty();
        if selections > 0 {
            charge.add(
                self.history
                    .reserve(selections.saturating_mul(std::mem::size_of::<history::Selection>()))?,
            );
        }
        let structure = self.history.reserve(self.entry_charge(&prepared.entry.edits))?;
        prepared.entry._undo_reservation = charge;
        prepared.entry._structure_charge = Some(Arc::new(structure));
        prepared.tracked = true;
        Ok(())
    }
    /// History bytes `demand` needs now: its entry plus any history slot growth.
    pub(crate) fn history_need(&self, demand: Demand) -> usize {
        match demand {
            Demand::Edit { history } => history.saturating_add(self.undo.growth_bytes(1)),
            Demand::Undo => self.redo.growth_bytes(1),
            Demand::Redo => self.undo.growth_bytes(1),
            Demand::Text { .. } => 0,
        }
    }
    /// The budget that meters `demand`: the byte budget for text, else the history budget.
    pub(crate) fn metered(&self, demand: Demand) -> &Budget {
        match demand {
            Demand::Text { .. } => &self.bytes,
            Demand::Edit { .. } | Demand::Undo | Demand::Redo => &self.history,
        }
    }
    /// Bytes `demand` currently lacks in the budget that meters it.
    fn shortfall(&self, demand: Demand) -> usize {
        match demand {
            Demand::Text { bytes } => bytes.saturating_sub(self.bytes.available()),
            Demand::Edit { .. } | Demand::Undo | Demand::Redo => {
                self.history_need(demand).saturating_sub(self.history.available())
            }
        }
    }
    /// Whether the budget that meters `demand` currently lacks room for it.
    pub(crate) fn lacks_room(&self, demand: Demand) -> bool {
        self.shortfall(demand) > 0
    }
    /// Bytes evicting evictable entries, oldest first, would release from the budget that
    /// meters `demand`; the scan stops once `enough` is found.
    pub(crate) fn freeable(&self, demand: Demand, keep: (usize, usize), enough: usize) -> usize {
        match demand {
            Demand::Text { .. } => self.freeable_text(keep, enough),
            Demand::Edit { .. } | Demand::Undo | Demand::Redo => self.freeable_history(keep, enough),
        }
    }
    /// Text bytes evicting evictable entries, oldest first, would release from the byte
    /// budget: text only history keeps alive. The scan stops once `enough` is found.
    fn freeable_text(&self, keep: (usize, usize), enough: usize) -> usize {
        let undo = &self.undo[..self.undo.len().saturating_sub(keep.0)];
        let redo = &self.redo[..self.redo.len().saturating_sub(keep.1)];
        let mut freeable = 0usize;
        for (entry, undone) in undo
            .iter()
            .map(|entry| (entry, false))
            .chain(redo.iter().map(|entry| (entry, true)))
        {
            if freeable >= enough {
                break;
            }
            freeable = freeable.saturating_add(entry.retained_text(undone, enough - freeable));
        }
        freeable
    }
    /// How much eviction for `demand` may count on, starting now: text relief stops once the
    /// entries it evicted account for the shortfall, so text that something outside this
    /// document's history still holds (a published snapshot, a fork) costs a bounded number of
    /// entries. History charges are exact and need no such bound.
    pub(crate) fn relief_bound(&self, demand: Demand) -> usize {
        match demand {
            Demand::Text { .. } => self.shortfall(demand),
            Demand::Edit { .. } | Demand::Undo | Demand::Redo => usize::MAX,
        }
    }
    /// History bytes evicting evictable entries, oldest first, would release; the scan
    /// stops once `enough` is found, so its cost follows the demand rather than the
    /// history size. Slot capacity stays charged, and claims shared with a spilled copy
    /// stay held.
    pub(crate) fn freeable_history(&self, keep: (usize, usize), enough: usize) -> usize {
        let undo = &self.undo[..self.undo.len().saturating_sub(keep.0)];
        let redo = &self.redo[..self.redo.len().saturating_sub(keep.1)];
        let mut freeable = 0usize;
        for entry in undo.iter().chain(redo) {
            if freeable >= enough {
                break;
            }
            freeable = freeable
                .saturating_add(entry._undo_reservation.exclusive_bytes())
                .saturating_add(
                    entry
                        ._structure_charge
                        .as_ref()
                        .filter(|charge| Arc::strong_count(charge) == 1)
                        .map_or(0, |charge| charge.bytes),
                );
        }
        freeable
    }
    /// History bytes `demand` would still lack after evicting all of this document's
    /// evictable history; other documents must release that much for it to fit.
    /// Evicting when nothing can close the gap only loses history.
    pub(crate) fn relief_missing(&self, demand: Demand) -> usize {
        // Evicting any entry of a stack leaves room in its existing slot allocation.
        let growth = |stack: &history::HistoryStack<History>, keep: usize| {
            if stack.len() > keep { 0 } else { stack.growth_bytes(1) }
        };
        let (keep_undo, keep_redo) = demand.keep();
        let floor = match demand {
            Demand::Edit { history } => history.saturating_add(growth(&self.undo, keep_undo)),
            Demand::Undo => growth(&self.redo, keep_redo),
            Demand::Redo => growth(&self.undo, keep_undo),
            Demand::Text { bytes } => bytes,
        };
        let missing = floor.saturating_sub(self.metered(demand).available());
        missing.saturating_sub(self.freeable(demand, demand.keep(), missing))
    }
    /// Evict this document's oldest history until `demand` fits, when eviction can.
    pub(crate) fn relieve_history(&mut self, demand: Demand) {
        if !self.lacks_room(demand) || self.relief_missing(demand) > 0 {
            return;
        }
        let mut uncovered = self.relief_bound(demand);
        uncovered = uncovered.saturating_sub(self.relieve_redo(demand));
        while self.lacks_room(demand) && uncovered > 0 {
            let Some(released) = self.evict_oldest_entry(demand.keep(), matches!(demand, Demand::Text { .. })) else {
                break;
            };
            uncovered = uncovered.saturating_sub(released);
        }
    }
    /// A new edit discards redo anyway, so under pressure it goes first. Returns the text
    /// the dropped entries alone kept alive when `demand` is for text (zero otherwise).
    pub(crate) fn relieve_redo(&mut self, demand: Demand) -> usize {
        let text = matches!(demand, Demand::Text { .. });
        let mut released = 0usize;
        if matches!(demand, Demand::Edit { .. } | Demand::Text { .. }) {
            while self.lacks_room(demand) && !self.redo.is_empty() {
                if text {
                    released = released.saturating_add(self.redo[0].retained_text(true, usize::MAX));
                }
                self.redo.drain(..1);
            }
        }
        released
    }
    /// Creation order of the oldest entry eviction may drop, keeping `keep` (undo, redo)
    /// entries at the top of each stack.
    pub(crate) fn oldest_history(&self, keep: (usize, usize)) -> Option<u64> {
        let (undo, redo) = self.evictable(keep);
        undo.into_iter().chain(redo).min()
    }
    fn evictable(&self, keep: (usize, usize)) -> (Option<u64>, Option<u64>) {
        let first = |stack: &history::HistoryStack<History>, keep: usize| {
            stack.first().filter(|_| stack.len() > keep).map(|entry| entry.sequence)
        };
        (first(&self.undo, keep.0), first(&self.redo, keep.1))
    }
    /// Drop the oldest evictable entry: the start of undo or the far end of redo. Other
    /// entries own complete roots and stay valid; linked partners fall back to local undo.
    pub(crate) fn evict_oldest_history(&mut self, keep: (usize, usize)) -> bool {
        self.evict_oldest_entry(keep, false).is_some()
    }
    /// `evict_oldest_history`, returning `None` when nothing is evictable, else the text
    /// the dropped entry alone kept alive when `text` is set (zero otherwise).
    pub(crate) fn evict_oldest_entry(&mut self, keep: (usize, usize), text: bool) -> Option<usize> {
        let from_redo = match self.evictable(keep) {
            (Some(undo), Some(redo)) => redo < undo,
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (None, None) => return None,
        };
        let stack = if from_redo { &mut self.redo } else { &mut self.undo };
        let released = if text {
            stack[0].retained_text(from_redo, usize::MAX)
        } else {
            0
        };
        stack.drain(..1);
        Some(released)
    }
    fn trim_history(&mut self) {
        let excess = self.undo.len().saturating_sub(self.history_policy.max_changes);
        self.undo.drain(..excess);
        let excess = self
            .redo
            .len()
            .saturating_sub(self.history_policy.max_changes.saturating_sub(self.undo.len()));
        // The end is the next redo; discard the furthest future first.
        self.redo.drain(..excess);
    }
    pub fn history_metadata(&self, undo: bool) -> Option<&history::EditMetadata> {
        (if undo { self.undo.last() } else { self.redo.last() }).map(|entry| &entry.metadata)
    }
    pub fn history_stats(&self) -> history::HistoryStats {
        history::HistoryStats {
            charged_capacity_bytes: self.undo.capacity_bytes() + self.redo.capacity_bytes(),
            undo_changes: self.undo.len(),
            redo_changes: self.redo.len(),
            charged_payload_bytes: self
                .undo
                .iter()
                .chain(&self.redo)
                .map(|entry| {
                    entry._undo_reservation.bytes() + entry._structure_charge.as_ref().map_or(0, |charge| charge.bytes)
                })
                .sum(),
        }
    }
    pub fn prepare(&self, transaction: EditTransaction) -> Result<PreparedEdit, Error> {
        let mut prepared = self.stage(transaction)?;
        self.charge(&mut prepared, 0)?;
        Ok(prepared)
    }
    /// Validate a transaction and stage its roots and text without charging history;
    /// the result is untracked until `charge` admits its history entry.
    pub(crate) fn stage(&self, mut transaction: EditTransaction) -> Result<PreparedEdit, Error> {
        self.check_transaction(&mut transaction)?;
        let revision = self.next_revision()?;
        if transaction.edits.len() > PIECEWISE_EDITS {
            transaction.edits = self.coalesce(transaction.edits);
        }
        // Any failure drops staged segments and leaves the document unchanged.
        let inserts = transaction
            .edits
            .iter()
            .map(|e| tree::from_inserted_text(&e.insert, &self.bytes))
            .collect::<Result<Vec<_>, _>>()?;
        let mut owned_edits = Vec::with_capacity(transaction.edits.len());
        let mut before_cursor = 0usize;
        let mut after_cursor = 0usize;
        for (edit, inserted) in transaction.edits.iter().zip(&inserts) {
            let start = after_cursor
                .checked_add(edit.range.start.0 - before_cursor)
                .ok_or(Error::BudgetExceeded)?;
            let end = start.checked_add(edit.insert.len()).ok_or(Error::BudgetExceeded)?;
            let (prefix, _) = tree::split(self.current.root.clone(), edit.range.end.0);
            let (_, inverse) = tree::split(prefix, edit.range.start.0);
            owned_edits.push(history::OwnedEdit {
                before_range: edit.range.start.0..edit.range.end.0,
                after_range: start..end,
                inverse,
                inserted: inserted.clone(),
            });
            before_cursor = edit.range.end.0;
            after_cursor = end;
        }
        let mut root = self.current.root.clone();
        let single = transaction.edits.len() == 1;
        for (edit, inserted) in transaction.edits.iter().zip(inserts).rev() {
            let (left_and_deleted, right) = tree::split(root, edit.range.end.0);
            let (left, _deleted) = tree::split(left_and_deleted, edit.range.start.0);
            let left = if single && edit.range.is_empty() {
                tree::concat_coalesced(left, inserted, TYPED_LEAF_BYTES, &self.bytes)
            } else {
                tree::concat(left, inserted)
            };
            root = tree::concat(left, right);
        }
        let state = ContentStateId(unique());
        let change = change::AppliedChange::owned(
            self.current.document_id,
            self.current.revision,
            revision,
            self.current.content_state,
            state,
            change::ChangeDirection::Edit,
            &owned_edits,
            &self.bytes,
        )?;
        Ok(PreparedEdit {
            change,
            document_id: self.current.document_id,
            base_revision: self.current.revision,
            revision,
            entry: History {
                before_metadata: self.current.metadata.clone(),
                after_metadata: self.current.metadata.clone(),
                before: self.current.root.clone(),
                after: root.clone(),
                before_state: self.current.content_state,
                after_state: state,
                _undo_reservation: history::Charge::empty(),
                _structure_charge: None,
                sequence: unique(),
                edits: owned_edits,
                group: None,
                metadata: history::EditMetadata::default(),
                typing_insert: false,
            },
            tracked: false,
        })
    }
    /// Sort a transaction's edits and validate them against the current text.
    fn check_transaction(&self, transaction: &mut EditTransaction) -> Result<(), Error> {
        if transaction.base_revision != self.current.revision {
            return Err(Error::StaleRevision);
        }
        if transaction.edits.is_empty() {
            return Err(Error::EmptyTransaction);
        }
        transaction.edits.sort_by_key(|e| (e.range.start, e.range.end));
        for (i, edit) in transaction.edits.iter().enumerate() {
            self.current.validate_range(&edit.range)?;
            if i > 0 {
                let previous = &transaction.edits[i - 1].range;
                if previous.end > edit.range.start || previous.start == edit.range.start {
                    return Err(Error::OverlappingEdits);
                }
            }
        }
        self.next_revision().map(|_| ())
    }
    /// Stage clustered edits of a large, sorted and validated transaction as range edits.
    /// Copying a short unchanged gap costs less than the tree pieces, history entry and
    /// receipt of a separate edit, so a Replace All stays one compact undo step.
    fn coalesce(&self, edits: Vec<Edit>) -> Vec<Edit> {
        let mut output: Vec<Edit> = Vec::new();
        for edit in edits {
            if let Some(last) = output.last_mut() {
                let gap = last.range.end.0..edit.range.start.0;
                if gap.len() <= COALESCE_GAP && last.insert.len() + gap.len() + edit.insert.len() <= COALESCE_BYTES {
                    let staged = last.insert.len();
                    for chunk in tree::chunks(&self.current.root, gap.clone()) {
                        last.insert.push_str(chunk);
                    }
                    if last.insert.len() - staged == gap.len() {
                        last.insert.push_str(&edit.insert);
                        last.range.end = edit.range.end;
                        continue;
                    }
                    // Unavailable source bytes: keep the edit separate.
                    last.insert.truncate(staged);
                }
            }
            output.push(edit);
        }
        output
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
    /// An entry whose undo slot or history charge cannot be admitted is refused; the
    /// document and its history stay unchanged.
    pub fn commit_prepared(&mut self, prepared: PreparedEdit) -> Result<Revision, Error> {
        self.validate_prepared(&prepared)?;
        if !prepared.tracked {
            return Err(Error::BudgetExceeded);
        }
        self.undo.try_reserve(1).map_err(|_| Error::BudgetExceeded)?;
        Ok(self.commit_prepared_unchecked(prepared))
    }
    fn commit_prepared_unchecked(&mut self, prepared: PreparedEdit) -> Revision {
        self.current.applied_change = Some(prepared.change);
        self.redo.clear();
        self.current.root = prepared.entry.after.clone();
        self.current.content_state = prepared.entry.after_state;
        self.current.revision = prepared.revision;
        self.last_untracked = !prepared.tracked;
        if !prepared.tracked {
            // Older entries would undo straight across this unrecorded edit. The caller
            // reports this through `last_edit_untracked`.
            self.undo.clear();
            self.last_merged = false;
            return self.current.revision;
        }
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
            last.edits[0].inserted = tree::concat_coalesced(
                last.edits[0].inserted.clone(),
                prepared.entry.edits[0].inserted.clone(),
                TYPED_LEAF_BYTES,
                &self.bytes,
            );
            last.edits[0].after_range.end = prepared.entry.edits[0].after_range.end;
            // The merged entry keeps one before and one after caret, as the new one does.
            last._undo_reservation = prepared.entry._undo_reservation;
            last._structure_charge = prepared.entry._structure_charge;
        } else {
            self.undo.push(prepared.entry);
        }
        self.last_merged = merge;
        self.trim_history();
        self.current.revision
    }
    pub fn undo(&mut self) -> Result<Revision, Error> {
        self.undo_relieved(&mut Self::relieve_history)
    }
    /// `relieve` may evict history for the slot this step needs, once it is valid.
    pub(crate) fn undo_relieved(&mut self, relieve: &mut dyn FnMut(&mut Self, Demand)) -> Result<Revision, Error> {
        if self
            .undo
            .last()
            .and_then(|entry| entry.group.as_ref())
            .is_some_and(group::GroupTag::linked)
        {
            return Err(Error::LinkedUndoRequired);
        }
        let revision = self.next_revision()?;
        if self.undo.is_empty() {
            return Err(Error::EmptyHistory);
        }
        relieve(&mut *self, Demand::Undo);
        self.redo.try_reserve_exact(1)?;
        let entry = self.undo.last().ok_or(Error::EmptyHistory)?;
        let change = change::AppliedChange::owned(
            self.current.document_id,
            self.current.revision,
            revision,
            self.current.content_state,
            entry.before_state,
            change::ChangeDirection::Undo,
            &entry.edits,
            &self.bytes,
        )?;
        let mut entry = self.undo.pop().ok_or(Error::EmptyHistory)?;
        self.current.applied_change = Some(change);
        entry.typing_insert = false;
        if let Some(previous) = self.undo.last_mut() {
            previous.typing_insert = false;
        }
        self.current.metadata = entry.before_metadata.clone();
        self.current.root = entry.before.clone();
        self.current.content_state = entry.before_state;
        self.current.revision = revision;
        self.redo.push(entry);
        Ok(revision)
    }
    pub fn redo(&mut self) -> Result<Revision, Error> {
        self.redo_relieved(&mut Self::relieve_history)
    }
    /// `relieve` may evict history for the slot this step needs, once it is valid.
    pub(crate) fn redo_relieved(&mut self, relieve: &mut dyn FnMut(&mut Self, Demand)) -> Result<Revision, Error> {
        if self
            .redo
            .last()
            .and_then(|entry| entry.group.as_ref())
            .is_some_and(group::GroupTag::linked)
        {
            return Err(Error::LinkedUndoRequired);
        }
        let revision = self.next_revision()?;
        if self.redo.is_empty() {
            return Err(Error::EmptyHistory);
        }
        relieve(&mut *self, Demand::Redo);
        self.undo.try_reserve_exact(1)?;
        let entry = self.redo.last().ok_or(Error::EmptyHistory)?;
        let change = change::AppliedChange::owned(
            self.current.document_id,
            self.current.revision,
            revision,
            self.current.content_state,
            entry.after_state,
            change::ChangeDirection::Redo,
            &entry.edits,
            &self.bytes,
        )?;
        let mut entry = self.redo.pop().ok_or(Error::EmptyHistory)?;
        self.current.applied_change = Some(change);
        entry.typing_insert = false;
        self.current.metadata = entry.after_metadata.clone();
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
