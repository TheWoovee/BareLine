// SPDX-License-Identifier: MPL-2.0
//! Logical document actors on a fixed worker pool. UI submits without blocking.
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
}
pub struct Completion {
    pub change: Option<Arc<crate::change::AppliedChange>>,
    pub result: Result<Revision, Error>,
    pub snapshot: DocumentSnapshot,
    pub metadata: Option<crate::history::EditMetadata>,
    pub undo_depth: usize,
    pub redo_depth: usize,
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
struct GroupRequest {
    mutation: GroupMutation,
    reply: SyncSender<GroupCompletion>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
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
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(_) => return Err((SubmitError::Saturated, work)),
        };
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
        let snapshot = self
            .publication
            .snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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
        let mut handles = Vec::new();
        for number in 0..count {
            let incoming = ready.clone();
            let result = thread::Builder::new()
                .name(format!("document-{number}"))
                .spawn(move || {
                    while let Some(work) = incoming.next() {
                        let job = match work {
                            Work::Actor(job) => job,
                            Work::HistoryPolicy(job, max_changes) => {
                                let mut actor =
                                    job.lock().unwrap_or_else(|error| error.into_inner());
                                if !actor.retired {
                                    let mut policy = actor.document.history_policy;
                                    // Multiple workers may acquire this actor out of queue
                                    // order; coalesce to the latest admitted setting.
                                    policy.max_changes =
                                        actor.configured_history_limit.unwrap_or(max_changes);
                                    actor.document.set_history_policy(policy);
                                }
                                drop(actor);
                                incoming.complete(None);
                                continue;
                            }
                            Work::Group(request) => {
                                run_group(request);
                                incoming.complete(None);
                                continue;
                            }
                        };
                        for _ in 0..ACTOR_QUANTUM {
                            let mut actor = job.lock().unwrap_or_else(|p| p.into_inner());
                            let Some(request) = actor.queue.pop_front() else {
                                break;
                            };
                            let applying = matches!(&request.mutation, Mutation::Apply(_));
                            let mut metadata = match &request.mutation {
                                Mutation::Apply(_) => request.metadata.clone(),
                                Mutation::Metadata { .. } => None,
                                Mutation::Undo => actor.document.history_metadata(true).cloned(),
                                Mutation::Redo => actor.document.history_metadata(false).cloned(),
                            };
                            let before_revision = actor.document.snapshot().revision;
                            let result = match request.mutation {
                                Mutation::Apply(edit) => match request.metadata {
                                    Some(metadata) => {
                                        actor.document.apply_with_metadata(edit, metadata)
                                    }
                                    None => actor.document.apply(edit),
                                },
                                Mutation::Metadata {
                                    base_revision,
                                    metadata,
                                } => actor.document.apply_metadata(base_revision, metadata),
                                Mutation::Undo => actor.document.undo(),
                                Mutation::Redo => actor.document.redo(),
                            };
                            if applying && result.is_ok() {
                                metadata = actor.document.history_metadata(true).cloned();
                            }
                            let depths = actor.document.history_stats();
                            let snapshot = actor.document.snapshot();
                            actor.published.update(snapshot.clone());
                            let change = if result.is_ok() && snapshot.revision != before_revision {
                                snapshot.applied_change().cloned()
                            } else {
                                None
                            };
                            let _ = request.reply.try_send(Completion {
                                change,
                                result,
                                snapshot,
                                metadata,
                                undo_depth: depths.undo_changes,
                                redo_depth: depths.redo_changes,
                            });
                            drop(actor);
                            if let Some(notify) = request.notify {
                                notify();
                            }
                        }
                        let mut actor = job.lock().unwrap_or_else(|p| p.into_inner());
                        if actor.queue.is_empty() {
                            actor.scheduled = false;
                            incoming.complete(None);
                        } else {
                            incoming.complete(Some(Work::Actor(job.clone())));
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
        DocumentService {
            actor: Arc::new(Mutex::new(Actor {
                document,
                configured_history_limit: None,
                queue: std::collections::VecDeque::new(),
                scheduled: false,
                retired: false,
                published: published.clone(),
            })),
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
        self.ready
            .submit(Work::Group(request))
            .map_err(|(kind, work)| {
                let Work::Group(request) = work else {
                    unreachable!()
                };
                (kind, request.mutation)
            })?;
        Ok(receiver)
    }
}
fn run_group(request: GroupRequest) {
    let (mut targets, action) = match request.mutation {
        GroupMutation::Apply(edits) => (
            edits
                .into_iter()
                .map(|edit| (edit.participant, Some(edit.transaction)))
                .collect::<Vec<_>>(),
            None,
        ),
        GroupMutation::Undo {
            group,
            participants,
        } => (
            participants
                .into_iter()
                .map(|participant| (participant, None))
                .collect(),
            Some((group, false)),
        ),
        GroupMutation::Redo {
            group,
            participants,
        } => (
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
            if !actor
                .document
                .snapshot()
                .same_document(&participant.snapshot)
            {
                return Err(Error::WrongDocument);
            }
            if actor.document.current.revision != participant.snapshot.revision {
                return Err(Error::StaleRevision);
            }
        }
        if let Some((group, redo)) = action {
            let mut documents: Vec<_> =
                actors.iter_mut().map(|actor| &mut actor.document).collect();
            if redo {
                crate::group::redo(&mut documents, group)?;
            } else {
                crate::group::undo(&mut documents, group)?;
            }
            return Ok(group);
        }
        let mut prepared = Vec::with_capacity(actors.len());
        for (actor, (_, transaction)) in actors.iter().zip(&mut targets) {
            prepared.push(
                actor
                    .document
                    .prepare(transaction.take().expect("apply transaction"))?,
            );
        }
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
    let _ = request
        .reply
        .try_send(GroupCompletion { result, snapshots });
    if let Some(notify) = request.notify {
        notify();
    }
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
        if !actor.document.current.same_document(captured)
            || actor.document.current.revision != captured.revision
        {
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
    pub fn migrate_spill(
        &self,
        prepared: crate::spill::PreparedSpill,
    ) -> Result<crate::paged::PagedDocument, Error> {
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
        let paged =
            crate::paged::PagedDocument::from_clean_spill(&actor.document, captured, source)?;
        actor.retired = true;
        Ok(paged)
    }
    /// Roll back a failed controller installation; the retired actor never changed content.
    pub fn cancel_clean_spill(&self, captured: &DocumentSnapshot) -> Result<(), Error> {
        let mut actor = self.actor.try_lock().map_err(|_| Error::ActorBusy)?;
        if !actor.document.current.same_document(captured)
            || actor.document.current.revision != captured.revision
        {
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
    pub fn submit(
        &self,
        mutation: Mutation,
    ) -> Result<Receiver<Completion>, (SubmitError, Mutation)> {
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
    ) -> Result<Receiver<Completion>, (SubmitError, EditTransaction, crate::history::EditMetadata)>
    {
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
        let mut actor = match self.actor.try_lock() {
            Ok(actor) => actor,
            Err(_) => return Err((SubmitError::Saturated, mutation)),
        };
        if actor.retired {
            return Err((SubmitError::Closed, mutation));
        }
        let mut state = match self.ready.state.try_lock() {
            Ok(state) => state,
            Err(_) => return Err((SubmitError::Saturated, mutation)),
        };
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
    fn group_edit(
        service: &DocumentService,
        snapshot: DocumentSnapshot,
        insert: &str,
    ) -> GroupEdit {
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
    // Completion receipt precedes scheduler slot release. Transient saturation is a
    // documented admission outcome; retain and retry the same non-droppable mutation.
    fn submit_group_retry(pool: &Scheduler, mut mutation: GroupMutation) -> GroupCompletion {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match pool.submit_group(mutation, None) {
                Ok(receiver) => {
                    return receiver
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                }
                Err((SubmitError::Saturated, returned)) if std::time::Instant::now() < deadline => {
                    mutation = returned;
                    std::thread::yield_now();
                }
                Err((error, _)) => panic!("group admission failed: {error:?}"),
            }
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
        let failed = submit_group_retry(
            &pool,
            GroupMutation::Apply(vec![
                group_edit(&first, first_snapshot.clone(), "changed"),
                bad,
            ]),
        );
        assert_eq!(failed.result, Err(Error::StaleRevision));
        assert!(
            failed
                .snapshots
                .iter()
                .all(|snapshot| snapshot.revision == Revision(0))
        );
        let completed = submit_group_retry(
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut mutation = Mutation::Undo;
        let single = loop {
            match first.submit(mutation) {
                Ok(receiver) => {
                    break receiver
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                }
                Err((SubmitError::Saturated, returned)) if std::time::Instant::now() < deadline => {
                    mutation = returned;
                    std::thread::yield_now();
                }
                Err((error, _)) => panic!("single undo admission failed: {error:?}"),
            }
        };
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
        let undone = submit_group_retry(
            &pool,
            GroupMutation::Undo {
                group,
                participants,
            },
        );
        assert_eq!(undone.result, Ok(group));
        assert_eq!(
            undone.snapshots[0]
                .read(TextOffset(0)..TextOffset(5), 5)
                .unwrap(),
            "first"
        );
        assert_eq!(
            undone.snapshots[1]
                .read(TextOffset(0)..TextOffset(6), 6)
                .unwrap(),
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
        let redone = submit_group_retry(
            &pool,
            GroupMutation::Redo {
                group,
                participants,
            },
        );
        assert_eq!(redone.result, Ok(group));
        assert_eq!(
            redone.snapshots[0]
                .read(TextOffset(0)..TextOffset(3), 3)
                .unwrap(),
            "one"
        );
    }
    #[test]
    fn configured_history_limit_retires_on_worker_without_changing_content() {
        let pool = Scheduler::new(1, 8).unwrap();
        let mut document =
            Document::from_utf8("", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            peer.configure_history_limit(1);
            if let Ok(actor) = service.actor.try_lock() {
                if actor.document.history_stats().undo_changes == 1 {
                    assert_eq!(
                        actor.document.snapshot().content_state,
                        captured.content_state
                    );
                    assert_eq!(actor.document.snapshot().revision, captured.revision);
                    break;
                }
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn many_documents_share_workers_and_stale_concurrent_edits_are_rejected() {
        let pool = Scheduler::new(2, 32).unwrap();
        assert!(pool.worker_count() <= 2);
        let budget = Budget::new(1 << 20);
        let history = Budget::new(1 << 20);
        let services: Vec<_> = (0..5000)
            .map(|_| {
                pool.document(
                    Document::from_utf8("", budget.clone(), history.clone()).unwrap(),
                    8,
                )
            })
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
        let second = loop {
            if let Ok(receiver) = services[0].submit(edit()) {
                break receiver;
            }
            thread::yield_now();
        };
        assert_eq!(second.recv().unwrap().result, Err(Error::StaleRevision));
        let receipt_bytes = result.snapshot.applied_change().unwrap().charged_bytes();
        assert_eq!(budget.used(), 1 + receipt_bytes);
        drop(services);
        drop(pool);
    }
}
