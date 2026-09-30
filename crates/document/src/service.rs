// SPDX-License-Identifier: MPL-2.0
//! Logical document actors on a fixed worker pool. UI submits without blocking on
//! queued work: workers release the actor and its scheduler slot before they reply.
use crate::{Document, DocumentSnapshot, EditTransaction, Error, Revision};
use std::sync::{
    Arc, Condvar, Mutex,
    mpsc::{self, Receiver, SyncSender},
};
use std::thread::{self, JoinHandle};

#[derive(Debug)]
pub enum SubmitError {
    Saturated,
    Closed,
    InvalidGroup,
}
pub enum Mutation {
    Metadata {
        base_revision: Revision,
        metadata: crate::DocumentMetadata,
    },
    Apply(EditTransaction),
    Undo,
    Redo,
    /// A save wrote this content state. Typing stops merging across it so undo and
    /// redo can return to the saved text.
    MarkSaved(crate::ContentStateId),
}
pub struct Completion {
    pub change: Option<Arc<crate::change::AppliedChange>>,
    pub result: Result<Revision, Error>,
    pub snapshot: DocumentSnapshot,
    pub metadata: Option<crate::history::EditMetadata>,
    pub undo_depth: usize,
    pub redo_depth: usize,
    /// The applied edit extended the previous undo entry instead of adding one.
    pub merged: bool,
    /// The applied edit has no undo entry: no history could be admitted for it even
    /// after evicting this document's own, so its undo history was cleared too.
    pub untracked: bool,
}
struct Request {
    mutation: Mutation,
    metadata: Option<crate::history::EditMetadata>,
    reply: SyncSender<Completion>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}
struct Actor {
    document: Document,
    configured_history_limit: Option<usize>,
    queue: std::collections::VecDeque<Request>,
    scheduled: bool,
    retired: bool,
    published: Arc<Publication>,
}
type Job = Arc<Mutex<Actor>>;
/// Every actor of one scheduler, for evicting history across documents under pressure.
type Registry = Mutex<Vec<std::sync::Weak<Mutex<Actor>>>>;
enum Work {
    Actor(Job),
    HistoryPolicy(Job, usize),
    Group(GroupRequest),
}
pub struct GroupParticipant {
    pub service: DocumentService,
    pub snapshot: DocumentSnapshot,
}
pub struct GroupEdit {
    pub participant: GroupParticipant,
    pub transaction: EditTransaction,
}
pub enum GroupMutation {
    Apply(Vec<GroupEdit>),
    Undo {
        group: crate::group::UndoGroup,
        participants: Vec<GroupParticipant>,
    },
    Redo {
        group: crate::group::UndoGroup,
        participants: Vec<GroupParticipant>,
    },
}
pub struct GroupCompletion {
    pub result: Result<crate::group::UndoGroup, Error>,
    pub snapshots: Vec<DocumentSnapshot>,
}
type Notify = Option<Arc<dyn Fn() + Send + Sync>>;
struct GroupRequest {
    mutation: GroupMutation,
    reply: SyncSender<GroupCompletion>,
    notify: Notify,
}
const ACTOR_QUANTUM: usize = 8;
struct ReadyState {
    queue: std::collections::VecDeque<Work>,
    // Includes running jobs: their reserved slot makes yielding infallible.
    admitted: usize,
    closed: bool,
}
struct ReadyQueue {
    state: Mutex<ReadyState>,
    wake: Condvar,
    capacity: usize,
}
impl ReadyQueue {
    fn close(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).closed = true;
        self.wake.notify_all();
    }
    fn submit(&self, work: Work) -> Result<(), (SubmitError, Work)> {
        // Every holder of this lock does O(1) queue work, so waiting for it is
        // bounded; losing that race to a worker is not saturation (QA-06).
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return Err((SubmitError::Closed, work));
        }
        if state.admitted >= self.capacity {
            return Err((SubmitError::Saturated, work));
        }
        state.admitted += 1;
        state.queue.push_back(work);
        self.wake.notify_one();
        Ok(())
    }
    fn next(&self) -> Option<Work> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(work) = state.queue.pop_front() {
                return Some(work);
            }
            if state.closed && state.admitted == 0 {
                return None;
            }
            state = self.wake.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }
    fn complete(&self, again: Option<Work>) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(work) = again {
            state.queue.push_back(work);
        } else {
            state.admitted -= 1;
        }
        self.wake.notify_all();
    }
}
struct Publication {
    snapshot: Mutex<DocumentSnapshot>,
}
impl Publication {
    fn update(&self, snapshot: DocumentSnapshot) {
        *self.snapshot.lock().unwrap_or_else(|p| p.into_inner()) = snapshot;
    }
}
/// Coalesced revisions: a slow reader retains only the latest immutable snapshot.
/// Completion notifications wake the UI; this receiver never polls in the background.
pub struct RevisionReceiver {
    publication: Arc<Publication>,
    seen: Revision,
}
impl RevisionReceiver {
    pub fn latest(&mut self) -> Option<DocumentSnapshot> {
        let snapshot = self.publication.snapshot.lock().unwrap_or_else(|p| p.into_inner());
        if snapshot.revision == self.seen {
            return None;
        }
        self.seen = snapshot.revision;
        Some(snapshot.clone())
    }
}
pub struct Scheduler {
    ready: Arc<ReadyQueue>,
    workers: Vec<JoinHandle<()>>,
    id: u64,
    actors: Arc<Registry>,
}
#[derive(Clone)]
pub struct DocumentService {
    actor: Job,
    ready: Arc<ReadyQueue>,
    published: Arc<Publication>,
    mailbox_capacity: usize,
    scheduler_id: u64,
    document_id: u64,
}
impl Scheduler {
    pub fn new(workers: usize, ready_capacity: usize) -> std::io::Result<Self> {
        let count = workers.clamp(1, thread::available_parallelism().map_or(1, |n| n.get()));
        let ready = Arc::new(ReadyQueue {
            state: Mutex::new(ReadyState {
                queue: std::collections::VecDeque::new(),
                admitted: 0,
                closed: false,
            }),
            wake: Condvar::new(),
            capacity: ready_capacity.max(1),
        });
        let actors: Arc<Registry> = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for number in 0..count {
            let incoming = ready.clone();
            let registry = actors.clone();
            let result = thread::Builder::new()
                .name(format!("document-{number}"))
                .spawn(move || {
                    while let Some(work) = incoming.next() {
                        let job = match work {
                            Work::Actor(job) => job,
                            Work::HistoryPolicy(job, max_changes) => {
                                let mut actor = job.lock().unwrap_or_else(|error| error.into_inner());
                                if !actor.retired {
                                    let mut policy = actor.document.history_policy;
                                    // Multiple workers may acquire this actor out of queue
                                    // order; coalesce to the latest admitted setting.
                                    policy.max_changes = actor.configured_history_limit.unwrap_or(max_changes);
                                    actor.document.set_history_policy(policy);
                                }
                                drop(actor);
                                incoming.complete(None);
                                continue;
                            }
                            Work::Group(request) => {
                                let (reply, completion, notify) = run_group(request, &registry);
                                // Release the slot before replying, so a caller acting on
                                // this completion is admitted at once (QA-06).
                                incoming.complete(None);
                                let _ = reply.try_send(completion);
                                if let Some(notify) = notify {
                                    notify();
                                }
                                continue;
                            }
                        };
                        let mut served = 0;
                        loop {
                            let mut actor = job.lock().unwrap_or_else(|p| p.into_inner());
                            let Some(request) = actor.queue.pop_front() else {
                                // A scheduled actor always holds a request; release it anyway.
                                actor.scheduled = false;
                                incoming.complete(None);
                                break;
                            };
                            let applying = matches!(&request.mutation, Mutation::Apply(_));
                            let mut metadata = match &request.mutation {
                                Mutation::Apply(_) => request.metadata.clone(),
                                Mutation::Metadata { .. } | Mutation::MarkSaved(_) => None,
                                Mutation::Undo => actor.document.history_metadata(true).cloned(),
                                Mutation::Redo => actor.document.history_metadata(false).cloned(),
                            };
                            // Validated mutations evict history across documents sharing
                            // the budget before they are charged; rejected ones evict nothing.
                            let mut relieve = |document: &mut Document, demand: crate::Demand| {
                                relieve_shared_history(&registry, &job, document, demand)
                            };
                            let before_revision = actor.document.snapshot().revision;
                            let result = match request.mutation {
                                Mutation::Apply(edit) => {
                                    actor.document.apply_relieved(edit, request.metadata, &mut relieve)
                                }
                                Mutation::Metadata {
                                    base_revision,
                                    metadata,
                                } => actor
                                    .document
                                    .apply_metadata_relieved(base_revision, metadata, &mut relieve),
                                Mutation::Undo => actor.document.undo_relieved(&mut relieve),
                                Mutation::Redo => actor.document.redo_relieved(&mut relieve),
                                Mutation::MarkSaved(state) => {
                                    actor.document.mark_saved_state(state);
                                    Ok(before_revision)
                                }
                            };
                            if applying && result.is_ok() {
                                metadata = actor.document.history_metadata(true).cloned();
                            }
                            let depths = actor.document.history_stats();
                            let snapshot = actor.document.snapshot();
                            let committed = applying && result.is_ok() && snapshot.revision != before_revision;
                            let merged = committed && actor.document.last_edit_merged();
                            let untracked = committed && actor.document.last_edit_untracked();
                            actor.published.update(snapshot.clone());
                            let change = if result.is_ok() && snapshot.revision != before_revision {
                                snapshot.applied_change().cloned()
                            } else {
                                None
                            };
                            served += 1;
                            // Release the actor, and with its last request the admission
                            // slot, before replying: a caller acting on this completion
                            // never finds the worker still holding either (QA-06).
                            let last = actor.queue.is_empty();
                            if last {
                                actor.scheduled = false;
                                incoming.complete(None);
                            }
                            drop(actor);
                            let _ = request.reply.try_send(Completion {
                                change,
                                result,
                                snapshot,
                                metadata,
                                undo_depth: depths.undo_changes,
                                redo_depth: depths.redo_changes,
                                merged,
                                untracked,
                            });
                            if let Some(notify) = request.notify {
                                notify();
                            }
                            if last {
                                break;
                            }
                            if served == ACTOR_QUANTUM {
                                // Yield to other actors after this reply, keeping the
                                // slot for the queued requests and their order.
                                incoming.complete(Some(Work::Actor(job.clone())));
                                break;
                            }
                        }
                    }
                });
            match result {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    ready.close();
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(error);
                }
            }
        }
        Ok(Self {
            ready,
            workers: handles,
            id: crate::unique(),
            actors,
        })
    }
    /// Reject new work and drain all already accepted mutations without blocking.
    pub fn close(&self) {
        self.ready.close();
    }
    /// Wait for accepted work to finish. Call on a shutdown worker, never the UI thread.
    pub fn shutdown(mut self) {
        self.close();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }
    pub fn document(&self, document: Document, mailbox_capacity: usize) -> DocumentService {
        let document_id = document.current.document_id;
        let published = Arc::new(Publication {
            snapshot: Mutex::new(document.snapshot()),
        });
        let actor = Arc::new(Mutex::new(Actor {
            document,
            configured_history_limit: None,
            queue: std::collections::VecDeque::new(),
            scheduled: false,
            retired: false,
            published: published.clone(),
        }));
        let mut actors = self.actors.lock().unwrap_or_else(|p| p.into_inner());
        if actors.len() == actors.capacity() {
            actors.retain(|actor| actor.strong_count() > 0);
        }
        actors.push(Arc::downgrade(&actor));
        drop(actors);
        DocumentService {
            actor,
            ready: self.ready.clone(),
            published,
            mailbox_capacity: mailbox_capacity.max(1),
            scheduler_id: self.id,
            document_id,
        }
    }
    /// Nonblocking submission; workers acquire actor guards in stable document-id order.
    pub fn submit_group(
        &self,
        mutation: GroupMutation,
        notify: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<Receiver<GroupCompletion>, (SubmitError, GroupMutation)> {
        let participants: Vec<_> = match &mutation {
            GroupMutation::Apply(edits) => edits.iter().map(|edit| &edit.participant).collect(),
            GroupMutation::Undo { participants, .. } | GroupMutation::Redo { participants, .. } => {
                participants.iter().collect()
            }
        };
        let mut ids: Vec<_> = participants
            .iter()
            .map(|participant| participant.service.document_id)
            .collect();
        ids.sort_unstable();
        if participants.len() < 2
            || participants.len() > crate::group::MAX_GROUP_DOCUMENTS
            || ids.windows(2).any(|ids| ids[0] == ids[1])
            || participants
                .iter()
                .any(|participant| participant.service.scheduler_id != self.id)
        {
            return Err((SubmitError::InvalidGroup, mutation));
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        let request = GroupRequest {
            mutation,
            reply,
            notify,
        };
        self.ready.submit(Work::Group(request)).map_err(|(kind, work)| {
            let Work::Group(request) = work else { unreachable!() };
            (kind, request.mutation)
        })?;
        Ok(receiver)
    }
}
/// Runs the group and returns its reply, completion and wake, which the worker
/// delivers only after releasing the group's scheduler slot.
fn run_group(request: GroupRequest, registry: &Registry) -> (SyncSender<GroupCompletion>, GroupCompletion, Notify) {
    let (mut targets, action) = match request.mutation {
        GroupMutation::Apply(edits) => (
            edits
                .into_iter()
                .map(|edit| (edit.participant, Some(edit.transaction)))
                .collect::<Vec<_>>(),
            None,
        ),
        GroupMutation::Undo { group, participants } => (
            participants
                .into_iter()
                .map(|participant| (participant, None))
                .collect(),
            Some((group, false)),
        ),
        GroupMutation::Redo { group, participants } => (
            participants
                .into_iter()
                .map(|participant| (participant, None))
                .collect(),
            Some((group, true)),
        ),
    };
    targets.sort_by_key(|(participant, _)| participant.service.document_id);
    let jobs: Vec<_> = targets
        .iter()
        .map(|(participant, _)| participant.service.actor.clone())
        .collect();
    let mut actors: Vec<_> = jobs
        .iter()
        .map(|job| job.lock().unwrap_or_else(|poison| poison.into_inner()))
        .collect();
    let result = (|| {
        for (actor, (participant, _)) in actors.iter().zip(&targets) {
            if actor.retired || !actor.queue.is_empty() {
                return Err(Error::ActorBusy);
            }
            if !participant.snapshot.complete {
                return Err(Error::IncompleteSource);
            }
            if !actor.document.snapshot().same_document(&participant.snapshot) {
                return Err(Error::WrongDocument);
            }
            if actor.document.current.revision != participant.snapshot.revision {
                return Err(Error::StaleRevision);
            }
        }
        if let Some((group, redo)) = action {
            let mut documents: Vec<_> = actors.iter_mut().map(|actor| &mut actor.document).collect();
            if redo {
                crate::group::redo(&mut documents, group)?;
            } else {
                crate::group::undo(&mut documents, group)?;
            }
            return Ok(group);
        }
        // Validate every member before any member evicts history for its charge.
        let mut prepared = Vec::with_capacity(actors.len());
        for (actor, (_, transaction)) in actors.iter().zip(&mut targets) {
            prepared.push(actor.document.stage(transaction.take().expect("apply transaction"))?);
        }
        let demands: Vec<_> = actors
            .iter()
            .zip(&prepared)
            .map(|(actor, prepared)| actor.document.edit_demand(prepared, 0))
            .collect();
        let pressured = actors
            .iter()
            .zip(&demands)
            .any(|(actor, demand)| actor.document.lacks_room(*demand));
        // Members are locked by this worker; peers are only try-locked, never awaited.
        let peers: Vec<Job> = if pressured {
            registered(registry)
                .into_iter()
                .filter(|peer| !jobs.iter().any(|job| Arc::ptr_eq(job, peer)))
                .collect()
        } else {
            Vec::new()
        };
        let mut guards: Vec<_> = peers
            .iter()
            .filter_map(|peer| peer.try_lock().ok())
            .filter(|peer| !peer.retired)
            .collect();
        let mut documents: Vec<&mut Document> = actors
            .iter_mut()
            .map(|actor| &mut actor.document)
            .chain(guards.iter_mut().map(|peer| &mut peer.document))
            .collect();
        // Evict only when the whole group can then be admitted together, so a refused
        // group edit costs no document its history. Members come first in `documents`.
        let relieve = pressured && group_admissible(&documents, &demands);
        for (member, prepared) in prepared.iter_mut().enumerate() {
            let demand = demands[member];
            if relieve {
                let history = documents[member].history.clone();
                documents[member].relieve_redo(demand);
                while documents[member].lacks_room(demand)
                    && evict_oldest(&mut documents, &[], demand, &history).is_some()
                {}
            }
            documents[member]
                .undo
                .try_reserve(1)
                .map_err(|_| Error::BudgetExceeded)?;
            documents[member].charge(prepared, 0)?;
        }
        drop(documents);
        drop(guards);
        let mut documents: Vec<_> = actors.iter_mut().map(|actor| &mut actor.document).collect();
        crate::group::commit(&mut documents, prepared)
    })();
    let snapshots = actors
        .iter()
        .map(|actor| {
            let snapshot = actor.document.snapshot();
            actor.published.update(snapshot.clone());
            snapshot
        })
        .collect();
    drop(actors);
    (request.reply, GroupCompletion { result, snapshots }, request.notify)
}
/// Evict the oldest history across every document sharing the budget that meters `demand`
/// until it fits, so budget pressure evicts history instead of refusing a user's edit.
/// For text, only history that alone keeps replaced or undone text alive can help, and
/// eviction stops once the evicted entries account for the shortfall. Busy or retiring
/// peers are skipped; this never blocks on another actor.
fn relieve_shared_history(registry: &Registry, own: &Job, document: &mut Document, demand: crate::Demand) {
    if !document.lacks_room(demand) {
        return;
    }
    let peers: Vec<Job> = registered(registry)
        .into_iter()
        .filter(|peer| !Arc::ptr_eq(peer, own))
        .collect();
    let mut guards: Vec<_> = peers
        .iter()
        .filter_map(|peer| peer.try_lock().ok())
        .filter(|peer| !peer.retired && peer.document.metered(demand).same(document.metered(demand)))
        .collect();
    // Evict nothing when even all evictable history could not admit the demand. Each
    // scan stops once the gap is covered, so this does not walk every peer's history.
    let mut missing = document.relief_missing(demand);
    for peer in &guards {
        if missing == 0 {
            break;
        }
        missing = missing.saturating_sub(peer.document.freeable(demand, (0, 0), missing));
    }
    if missing > 0 {
        return;
    }
    let mut uncovered = document.relief_bound(demand);
    uncovered = uncovered.saturating_sub(document.relieve_redo(demand));
    let budget = document.metered(demand).clone();
    let keep = [demand.keep()];
    let mut documents: Vec<&mut Document> = std::iter::once(document)
        .chain(guards.iter_mut().map(|peer| &mut peer.document))
        .collect();
    while documents[0].lacks_room(demand) && uncovered > 0 {
        let Some(released) = evict_oldest(&mut documents, &keep, demand, &budget) else {
            break;
        };
        uncovered = uncovered.saturating_sub(released);
    }
}
/// Every live actor of this scheduler. Collected under the registry lock, used after it.
fn registered(registry: &Registry) -> Vec<Job> {
    registry
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .collect()
}
/// Evict the oldest evictable entry among the documents whose `demand` budget is `budget`,
/// keeping `keep[index]` (undo, redo) entries at the top of document `index` (none past
/// `keep`). `None` when none of them has an evictable entry left; else the text the entry
/// alone kept alive when `demand` is for text (zero otherwise).
fn evict_oldest(
    documents: &mut [&mut Document],
    keep: &[(usize, usize)],
    demand: crate::Demand,
    budget: &crate::Budget,
) -> Option<usize> {
    let keep_of = |index: usize| keep.get(index).copied().unwrap_or((0, 0));
    let (_, index) = documents
        .iter()
        .enumerate()
        .filter(|(_, document)| document.metered(demand).same(budget))
        .filter_map(|(index, document)| {
            document
                .oldest_history(keep_of(index))
                .map(|sequence| (sequence, index))
        })
        .min()?;
    documents[index].evict_oldest_entry(keep_of(index), matches!(demand, crate::Demand::Text { .. }))
}
/// Whether evicting history among `documents` can admit every group member's demand at
/// once. Members come first in `documents`, one per demand; members that share a history
/// budget are checked together against it.
fn group_admissible(documents: &[&mut Document], demands: &[crate::Demand]) -> bool {
    (0..demands.len()).all(|member| {
        let history = &documents[member].history;
        let need = demands
            .iter()
            .enumerate()
            .filter(|(other, _)| documents[*other].history.same(history))
            .fold(0usize, |sum, (other, demand)| {
                sum.saturating_add(documents[other].history_need(*demand))
            });
        let mut missing = need.saturating_sub(history.available());
        for document in documents.iter().filter(|document| document.history.same(history)) {
            if missing == 0 {
                break;
            }
            missing = missing.saturating_sub(document.freeable_history((0, 0), missing));
        }
        missing == 0
    })
}
impl Drop for Scheduler {
    fn drop(&mut self) {
        // Explicit close wakes workers even when document services outlive the scheduler.
        self.close();
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}
impl DocumentService {
    /// Nonblocking policy admission. The caller retries when the bounded queue
    /// is full; retirement of retained roots happens on the document worker.
    pub fn configure_history_limit(&self, max_changes: usize) -> bool {
        let Ok(mut actor) = self.actor.try_lock() else {
            return false;
        };
        if actor.retired {
            return false;
        }
        if actor.configured_history_limit == Some(max_changes) {
            return true;
        }
        if self
            .ready
            .submit(Work::HistoryPolicy(self.actor.clone(), max_changes))
            .is_err()
        {
            return false;
        }
        actor.configured_history_limit = Some(max_changes);
        true
    }
    pub fn same_document(&self, snapshot: &DocumentSnapshot) -> bool {
        self.document_id == snapshot.document_id
    }
    /// Capture on a worker: clones immutable current/history roots without performing I/O.
    pub fn capture_spill_with_saved(
        &self,
        captured: &DocumentSnapshot,
        saved: crate::ContentStateId,
    ) -> Result<crate::spill::SpillPlan, Error> {
        let mut actor = self.actor.try_lock().map_err(|_| Error::ActorBusy)?;
        if actor.retired || actor.scheduled || !actor.queue.is_empty() {
            return Err(Error::ActorBusy);
        }
        if !actor.document.current.same_document(captured) || actor.document.current.revision != captured.revision {
            return Err(Error::StaleRevision);
        }
        // The caller owns the UI's opaque savepoint for this exact document snapshot.
        actor.document.saved_state = saved;
        crate::spill::SpillPlan::resident(&actor.document)
    }
    pub fn capture_spill(&self) -> Result<crate::spill::SpillPlan, Error> {
        let actor = self.actor.try_lock().map_err(|_| Error::ActorBusy)?;
        if actor.retired || actor.scheduled || !actor.queue.is_empty() {
            return Err(Error::ActorBusy);
        }
        crate::spill::SpillPlan::resident(&actor.document)
    }
    pub fn migrate_spill(&self, prepared: crate::spill::PreparedSpill) -> Result<crate::paged::PagedDocument, Error> {
        let mut actor = self.actor.try_lock().map_err(|_| Error::ActorBusy)?;
        if actor.retired || actor.scheduled || !actor.queue.is_empty() {
            return Err(Error::ActorBusy);
        }
        let document = prepared.attach_resident(&actor.document)?;
        actor.retired = true;
        Ok(document)
    }
    /// Attach an independently sealed copy to a clean, history-free actor. Retires this
    /// service atomically; drop it after installing the returned Paged actor to release RAM.
    pub fn migrate_clean_spill(
        &self,
        captured: &DocumentSnapshot,
        source: crate::source::MemorySource,
    ) -> Result<crate::paged::PagedDocument, Error> {
        let mut actor = self.actor.try_lock().map_err(|_| Error::ActorBusy)?;
        if actor.retired || actor.scheduled || !actor.queue.is_empty() {
            return Err(Error::ActorBusy);
        }
        let paged = crate::paged::PagedDocument::from_clean_spill(&actor.document, captured, source)?;
        actor.retired = true;
        Ok(paged)
    }
    /// Roll back a failed controller installation; the retired actor never changed content.
    pub fn cancel_clean_spill(&self, captured: &DocumentSnapshot) -> Result<(), Error> {
        let mut actor = self.actor.try_lock().map_err(|_| Error::ActorBusy)?;
        if !actor.document.current.same_document(captured) || actor.document.current.revision != captured.revision {
            return Err(Error::StaleRevision);
        }
        actor.retired = false;
        Ok(())
    }
    /// Reads only the publication slot, never the live actor or file storage.
    pub fn snapshot(&self) -> DocumentSnapshot {
        self.published
            .snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    pub fn subscribe(&self) -> RevisionReceiver {
        RevisionReceiver {
            publication: self.published.clone(),
            seen: self.snapshot().revision,
        }
    }
    /// Saturation returns the mutation so non-droppable edits can be retried unchanged.
    pub fn submit(&self, mutation: Mutation) -> Result<Receiver<Completion>, (SubmitError, Mutation)> {
        self.submit_with_notify(mutation, None)
    }
    /// `notify` wakes an event loop after completion; no idle polling is needed.
    pub fn submit_with_notify(
        &self,
        mutation: Mutation,
        notify: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<Receiver<Completion>, (SubmitError, Mutation)> {
        self.submit_context(mutation, None, notify)
    }
    pub fn submit_metadata(
        &self,
        base_revision: Revision,
        metadata: crate::DocumentMetadata,
        notify: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<Receiver<Completion>, (SubmitError, Mutation)> {
        self.submit_with_notify(
            Mutation::Metadata {
                base_revision,
                metadata,
            },
            notify,
        )
    }
    /// Metadata remains owned by the caller when admission fails, just like the edit.
    pub fn submit_with_metadata(
        &self,
        transaction: EditTransaction,
        metadata: crate::history::EditMetadata,
        notify: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<Receiver<Completion>, (SubmitError, EditTransaction, crate::history::EditMetadata)> {
        self.submit_context(Mutation::Apply(transaction), Some(metadata.clone()), notify)
            .map_err(|(error, mutation)| {
                let Mutation::Apply(transaction) = mutation else {
                    unreachable!()
                };
                (error, transaction, metadata)
            })
    }
    fn submit_context(
        &self,
        mutation: Mutation,
        metadata: Option<crate::history::EditMetadata>,
        notify: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<Receiver<Completion>, (SubmitError, Mutation)> {
        // Workers release the actor before they reply, so this waits at most for one
        // mutation, group or history policy of this resident document that is already
        // running, never for queued work. A lost race is not saturation (QA-06).
        let mut actor = self.actor.lock().unwrap_or_else(|p| p.into_inner());
        if actor.retired {
            return Err((SubmitError::Closed, mutation));
        }
        // Held only for O(1) queue work, never across a mutation.
        let mut state = self.ready.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return Err((SubmitError::Closed, mutation));
        }
        if !actor.scheduled && state.admitted >= self.ready.capacity {
            return Err((SubmitError::Saturated, mutation));
        }
        if actor.queue.len() >= self.mailbox_capacity {
            return Err((SubmitError::Saturated, mutation));
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        actor.queue.push_back(Request {
            mutation,
            metadata,
            reply,
            notify,
        });
        if !actor.scheduled {
            state.admitted += 1;
            state.queue.push_back(Work::Actor(self.actor.clone()));
            actor.scheduled = true;
            self.ready.wake.notify_one();
        }
        Ok(receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Budget, Edit, TextOffset};
    fn group_edit(service: &DocumentService, snapshot: DocumentSnapshot, insert: &str) -> GroupEdit {
        GroupEdit {
            participant: GroupParticipant {
                service: service.clone(),
                snapshot: snapshot.clone(),
            },
            transaction: EditTransaction {
                base_revision: snapshot.revision,
                edits: vec![Edit {
                    range: TextOffset(0)..TextOffset(snapshot.len()),
                    insert: insert.into(),
                }],
            },
        }
    }
    // Workers release the scheduler slot before they reply, so a caller acting on a
    // completion is admitted at once: no test retries a refused submission (QA-06).
    fn submit_group(pool: &Scheduler, mutation: GroupMutation) -> GroupCompletion {
        match pool.submit_group(mutation, None) {
            Ok(receiver) => receiver.recv().unwrap(),
            Err((error, _)) => panic!("group admission failed: {error:?}"),
        }
    }
    #[test]
    fn grouped_worker_commit_failure_and_linked_undo_are_atomic() {
        let pool = Scheduler::new(2, 16).unwrap();
        let budget = Budget::new(4096);
        let first = Document::from_utf8("first", budget.clone(), Budget::new(4096)).unwrap();
        let first_snapshot = first.snapshot();
        let first = pool.document(first, 8);
        let second = Document::from_utf8("second", budget.clone(), Budget::new(4096)).unwrap();
        let second_snapshot = second.snapshot();
        let second = pool.document(second, 8);
        let mut bad = group_edit(&second, second_snapshot.clone(), "changed");
        bad.transaction.base_revision = Revision(99);
        let failed = submit_group(
            &pool,
            GroupMutation::Apply(vec![group_edit(&first, first_snapshot.clone(), "changed"), bad]),
        );
        assert_eq!(failed.result, Err(Error::StaleRevision));
        assert!(failed.snapshots.iter().all(|snapshot| snapshot.revision == Revision(0)));
        let completed = submit_group(
            &pool,
            GroupMutation::Apply(vec![
                group_edit(&first, first_snapshot, "one"),
                group_edit(&second, second_snapshot, "two"),
            ]),
        );
        let group = completed.result.unwrap();
        assert!(
            completed
                .snapshots
                .iter()
                .all(|snapshot| snapshot.revision == Revision(1))
        );
        let single = submit(&first, Mutation::Undo);
        assert_eq!(single.result, Err(Error::LinkedUndoRequired));
        let participants = vec![
            GroupParticipant {
                service: first.clone(),
                snapshot: completed.snapshots[0].clone(),
            },
            GroupParticipant {
                service: second.clone(),
                snapshot: completed.snapshots[1].clone(),
            },
        ];
        let undone = submit_group(&pool, GroupMutation::Undo { group, participants });
        assert_eq!(undone.result, Ok(group));
        assert_eq!(
            undone.snapshots[0].read(TextOffset(0)..TextOffset(5), 5).unwrap(),
            "first"
        );
        assert_eq!(
            undone.snapshots[1].read(TextOffset(0)..TextOffset(6), 6).unwrap(),
            "second"
        );
        let participants = vec![
            GroupParticipant {
                service: first.clone(),
                snapshot: undone.snapshots[0].clone(),
            },
            GroupParticipant {
                service: second.clone(),
                snapshot: undone.snapshots[1].clone(),
            },
        ];
        let redone = submit_group(&pool, GroupMutation::Redo { group, participants });
        assert_eq!(redone.result, Ok(group));
        assert_eq!(
            redone.snapshots[0].read(TextOffset(0)..TextOffset(3), 3).unwrap(),
            "one"
        );
    }
    fn submit(service: &DocumentService, mutation: Mutation) -> Completion {
        match service.submit(mutation) {
            Ok(receiver) => receiver.recv().unwrap(),
            Err((error, _)) => panic!("admission failed: {error:?}"),
        }
    }
    fn append(service: &DocumentService, text: &str) -> Completion {
        let snapshot = service.snapshot();
        submit(
            service,
            Mutation::Apply(EditTransaction {
                base_revision: snapshot.revision,
                edits: vec![Edit {
                    range: TextOffset(snapshot.len())..TextOffset(snapshot.len()),
                    insert: text.into(),
                }],
            }),
        )
    }
    /// One-byte inserts at `count` separate offsets: one entry charging many tree pieces.
    fn spread_transaction(snapshot: &DocumentSnapshot, count: usize) -> EditTransaction {
        EditTransaction {
            base_revision: snapshot.revision,
            edits: (0..count)
                .map(|index| Edit {
                    range: TextOffset(index * 16)..TextOffset(index * 16),
                    insert: "y".into(),
                })
                .collect(),
        }
    }
    fn spread(service: &DocumentService, count: usize) -> Mutation {
        Mutation::Apply(spread_transaction(&service.snapshot(), count))
    }
    fn stats(service: &DocumentService) -> crate::history::HistoryStats {
        // The worker releases the actor before it replies, and no request is in flight.
        let actor = service.actor.try_lock().expect("actor released before its reply");
        actor.document.history_stats()
    }
    #[test]
    fn history_pressure_evicts_the_oldest_history_across_documents() {
        let pool = Scheduler::new(1, 16).unwrap();
        let bytes = Budget::new(1 << 20);
        let history = Budget::new(32 * 1024);
        let first = pool.document(Document::from_utf8("", bytes.clone(), history.clone()).unwrap(), 8);
        let second = pool.document(
            Document::from_utf8(&"s".repeat(4096), bytes.clone(), history.clone()).unwrap(),
            8,
        );
        // Fill the shared budget until the first document evicts its own oldest entries.
        let mut depth = 0;
        for _ in 0..10_000 {
            let completion = append(&first, "x");
            assert!(completion.result.is_ok());
            assert!(!completion.merged);
            if completion.undo_depth <= depth {
                break;
            }
            depth = completion.undo_depth;
        }
        assert!(depth > 1);
        let filled = stats(&first).undo_changes;
        // The other document's edit, whose tree pieces need several entries' room, is
        // admitted by evicting the first document's older history, not refused and not
        // paid for with its own.
        let completion = submit(&second, spread(&second, 16));
        assert!(completion.result.is_ok());
        assert!(!completion.untracked);
        assert_eq!(completion.undo_depth, 1);
        assert!(stats(&first).undo_changes < filled);
        assert!(stats(&first).undo_changes > 0);
        let completion = submit(&first, Mutation::Undo);
        assert!(completion.result.is_ok());
    }
    #[test]
    fn group_edits_evict_only_when_every_member_can_then_be_admitted() {
        let pool = Scheduler::new(1, 16).unwrap();
        let bytes = Budget::new(1 << 20);
        let history = Budget::new(64 * 1024);
        let document = || Document::from_utf8(&"s".repeat(4096), bytes.clone(), history.clone()).unwrap();
        let first = pool.document(document(), 8);
        let second = pool.document(document(), 8);
        for _ in 0..12 {
            assert!(append(&first, "y").result.is_ok());
        }
        // Claims no eviction can free fill the rest of the shared budget.
        let full = history.claim(history.limit() - history.used()).unwrap();
        let first_snapshot = first.snapshot();
        let second_snapshot = second.snapshot();
        // The first member's entry would fit once some of its own history went, but the
        // second member's needs more than both documents' history releases. The group is
        // refused before either member evicts anything.
        let refused = submit_group(
            &pool,
            GroupMutation::Apply(vec![
                group_edit(&first, first_snapshot.clone(), "one"),
                GroupEdit {
                    participant: GroupParticipant {
                        service: second.clone(),
                        snapshot: second_snapshot.clone(),
                    },
                    transaction: spread_transaction(&second_snapshot, 128),
                },
            ]),
        );
        assert_eq!(refused.result, Err(Error::BudgetExceeded));
        assert_eq!(stats(&first).undo_changes, 12);
        // A group that fits once the first member's oldest entries go is admitted.
        let admitted = submit_group(
            &pool,
            GroupMutation::Apply(vec![
                group_edit(&first, first_snapshot, "one"),
                group_edit(&second, second_snapshot, "two"),
            ]),
        );
        assert!(admitted.result.is_ok());
        let kept = stats(&first).undo_changes;
        assert!((2..=12).contains(&kept), "{kept} entries kept");
        assert_eq!(stats(&second).undo_changes, 1);
        drop(full);
    }
    #[test]
    fn an_edit_no_history_can_admit_is_reported_untracked() {
        let pool = Scheduler::new(1, 16).unwrap();
        let history = Budget::new(64 * 1024);
        let service = pool.document(
            Document::from_utf8(&"x".repeat(256), Budget::new(1 << 20), history.clone()).unwrap(),
            8,
        );
        let completion = append(&service, "a");
        assert!(!completion.untracked);
        assert_eq!(completion.undo_depth, 1);
        let _full = history.claim(history.limit() - history.used()).unwrap();
        // Even with its one older entry evicted, nothing can make room for this entry's
        // many tree pieces: the edit applies, and the completion says it has no undo.
        let completion = submit(&service, spread(&service, 8));
        assert!(completion.result.is_ok());
        assert!(completion.untracked);
        assert!(!completion.merged);
        assert_eq!(completion.undo_depth, 0);
        assert_eq!(completion.snapshot.len(), 256 + 1 + 8);
    }
    fn replace_all(service: &DocumentService, text: &str) -> Completion {
        let snapshot = service.snapshot();
        submit(
            service,
            Mutation::Apply(EditTransaction {
                base_revision: snapshot.revision,
                edits: vec![Edit {
                    range: TextOffset(0)..TextOffset(snapshot.len()),
                    insert: text.into(),
                }],
            }),
        )
    }
    #[test]
    fn byte_shortfall_evicts_history_that_alone_keeps_replaced_text_across_documents() {
        let pool = Scheduler::new(1, 16).unwrap();
        let bytes = Budget::new(256 * 1024);
        let history = Budget::new(1 << 20);
        let first = pool.document(Document::from_utf8("", bytes.clone(), history.clone()).unwrap(), 8);
        let second = pool.document(
            Document::from_utf8(&"s".repeat(128 * 1024), bytes.clone(), history.clone()).unwrap(),
            8,
        );
        // Select All + Delete: only that undo entry still keeps the deleted text alive.
        assert!(replace_all(&second, "").result.is_ok());
        for _ in 0..3 {
            assert!(append(&first, "x").result.is_ok());
        }
        // The paste needs more room than is left. The other document's entry, the oldest
        // and the one keeping text no document shows, gives way instead of the paste
        // being refused; this document keeps its own history.
        let completion = append(&first, &"z".repeat(160 * 1024));
        assert!(completion.result.is_ok());
        assert!(!completion.untracked);
        assert_eq!(completion.undo_depth, 4);
        assert_eq!(completion.snapshot.len(), 3 + 160 * 1024);
        assert_eq!(stats(&second).undo_changes, 0);
        let completion = submit(&first, Mutation::Undo);
        assert!(completion.result.is_ok());
        assert_eq!(completion.snapshot.len(), 3);
    }
    #[test]
    fn byte_shortfall_of_live_text_is_refused_without_evicting_history() {
        let pool = Scheduler::new(1, 16).unwrap();
        let bytes = Budget::new(256 * 1024);
        let history = Budget::new(1 << 20);
        let first = pool.document(Document::from_utf8("", bytes.clone(), history.clone()).unwrap(), 8);
        let second = pool.document(Document::from_utf8("", bytes.clone(), history.clone()).unwrap(), 8);
        for _ in 0..3 {
            assert!(append(&first, "x").result.is_ok());
            assert!(append(&second, "y").result.is_ok());
        }
        // Live text, which no history eviction can free, fills most of the byte budget;
        // the history of both documents keeps no text alive on its own.
        let _live = Document::from_utf8(&"w".repeat(200 * 1024), bytes.clone(), history.clone()).unwrap();
        // The paste fits the whole budget but not the room left. It fails, and neither
        // this document nor its peer loses undo history for it.
        let completion = append(&first, &"z".repeat(100 * 1024));
        assert!(matches!(completion.result, Err(crate::Error::BudgetExceeded)));
        assert_eq!(completion.undo_depth, 3);
        assert_eq!(stats(&first).undo_changes, 3);
        assert_eq!(stats(&second).undo_changes, 3);
        assert!(submit(&second, Mutation::Undo).result.is_ok());
    }
    #[test]
    fn configured_history_limit_retires_on_worker_without_changing_content() {
        let pool = Scheduler::new(1, 8).unwrap();
        let mut document = Document::from_utf8("", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        for _ in 0..4 {
            let snapshot = document.snapshot();
            document
                .apply(EditTransaction {
                    base_revision: snapshot.revision,
                    edits: vec![Edit {
                        range: TextOffset(snapshot.len())..TextOffset(snapshot.len()),
                        insert: "x".into(),
                    }],
                })
                .unwrap();
        }
        let captured = document.snapshot();
        let service = pool.document(document, 8);
        let peer = service.clone();
        assert!(peer.configure_history_limit(1));
        // One worker runs admitted work in order, so this reply follows the policy.
        let marked = submit(&service, Mutation::MarkSaved(captured.content_state));
        assert_eq!(marked.result, Ok(captured.revision));
        assert_eq!(stats(&service).undo_changes, 1);
        assert_eq!(marked.snapshot.content_state, captured.content_state);
        assert_eq!(marked.snapshot.revision, captured.revision);
    }
    /// QA-06: a caller woken by a completion finds the actor and the scheduler slot
    /// already released, so its next edit is never refused as busy. The wake runs on
    /// the worker after the reply; it records what a woken caller would see.
    #[test]
    fn completions_release_actor_and_scheduler_slot_before_waking_the_caller() {
        let pool = Scheduler::new(1, 1).unwrap();
        let budget = Budget::new(1 << 20);
        let history = Budget::new(1 << 20);
        let first = pool.document(Document::from_utf8("one", budget.clone(), history.clone()).unwrap(), 8);
        let second = pool.document(Document::from_utf8("two", budget.clone(), history.clone()).unwrap(), 8);
        let (seen, observed) = mpsc::sync_channel(1);
        let wake: Arc<dyn Fn() + Send + Sync> = {
            let ready = pool.ready.clone();
            let actors = [first.actor.clone(), second.actor.clone()];
            Arc::new(move || {
                let admitted = ready.state.lock().unwrap().admitted;
                let free = actors.iter().all(|actor| actor.try_lock().is_ok());
                let _ = seen.try_send((admitted, free));
            })
        };
        for _ in 0..3 {
            let snapshot = first.snapshot();
            let receiver = first
                .submit_with_notify(
                    Mutation::Apply(EditTransaction {
                        base_revision: snapshot.revision,
                        edits: vec![Edit {
                            range: TextOffset(snapshot.len())..TextOffset(snapshot.len()),
                            insert: "x".into(),
                        }],
                    }),
                    Some(wake.clone()),
                )
                .ok()
                .unwrap();
            assert!(receiver.recv().unwrap().result.is_ok());
            assert_eq!(observed.recv().unwrap(), (0, true));
        }
        let receiver = pool
            .submit_group(
                GroupMutation::Apply(vec![
                    group_edit(&first, first.snapshot(), "1"),
                    group_edit(&second, second.snapshot(), "2"),
                ]),
                Some(wake.clone()),
            )
            .ok()
            .unwrap();
        assert!(receiver.recv().unwrap().result.is_ok());
        assert_eq!(observed.recv().unwrap(), (0, true));
        // The single slot is free again: the next request is admitted, not saturated.
        assert_eq!(submit(&second, Mutation::Undo).result, Err(Error::LinkedUndoRequired));
    }
    #[test]
    fn many_documents_share_workers_and_stale_concurrent_edits_are_rejected() {
        let pool = Scheduler::new(2, 32).unwrap();
        assert!(pool.worker_count() <= 2);
        let budget = Budget::new(1 << 20);
        let history = Budget::new(1 << 20);
        let services: Vec<_> = (0..5000)
            .map(|_| pool.document(Document::from_utf8("", budget.clone(), history.clone()).unwrap(), 8))
            .collect();
        let edit = || {
            Mutation::Apply(EditTransaction {
                base_revision: Revision(0),
                edits: vec![Edit {
                    range: TextOffset(0)..TextOffset(0),
                    insert: "x".into(),
                }],
            })
        };
        let first = services[0].submit(edit()).ok().unwrap();
        let result = first.recv().unwrap();
        assert_eq!(result.result, Ok(Revision(1)));
        // Admitted at once: the slot was released before the first reply (QA-06).
        let second = services[0].submit(edit()).ok().unwrap();
        assert_eq!(second.recv().unwrap().result, Err(Error::StaleRevision));
        let receipt_bytes = result.snapshot.applied_change().unwrap().charged_bytes();
        assert_eq!(budget.used(), 1 + receipt_bytes);
        drop(services);
        drop(pool);
    }
}
