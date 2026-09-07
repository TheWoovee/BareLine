// SPDX-License-Identifier: MPL-2.0
//! Logical document actors on a fixed worker pool. UI submits without blocking.
use crate::{Document, DocumentSnapshot, EditTransaction, Error, Revision};
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, SyncSender, TrySendError},
};
use std::thread::{self, JoinHandle};

#[derive(Debug)]
pub enum SubmitError {
    Saturated,
    Closed,
    InvalidGroup,
}
pub enum Mutation {
    Apply(EditTransaction),
    Undo,
    Redo,
}
pub struct Completion {
    pub result: Result<Revision, Error>,
    pub snapshot: DocumentSnapshot,
}
struct Request {
    mutation: Mutation,
    reply: SyncSender<Completion>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}
struct Actor {
    document: Document,
    queue: std::collections::VecDeque<Request>,
    scheduled: bool,
}
type Job = Arc<Mutex<Actor>>;
enum Work {
    Actor(Job),
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
pub struct Scheduler {
    sender: Option<SyncSender<Work>>,
    workers: Vec<JoinHandle<()>>,
    id: u64,
}
#[derive(Clone)]
pub struct DocumentService {
    actor: Job,
    sender: SyncSender<Work>,
    mailbox_capacity: usize,
    scheduler_id: u64,
    document_id: u64,
}
impl Scheduler {
    pub fn new(workers: usize, ready_capacity: usize) -> std::io::Result<Self> {
        let count = workers.clamp(1, thread::available_parallelism().map_or(1, |n| n.get()));
        let (sender, receiver) = mpsc::sync_channel::<Work>(ready_capacity.max(1));
        let receiver = Arc::new(Mutex::new(receiver));
        let mut handles = Vec::new();
        for number in 0..count {
            let incoming = receiver.clone();
            let result = thread::Builder::new()
                .name(format!("document-{number}"))
                .spawn(move || {
                    loop {
                        // Only waiting for the next actor is serialized; work runs in parallel.
                        let job = {
                            let rx = incoming.lock().unwrap_or_else(|p| p.into_inner());
                            rx.recv()
                        };
                        let Ok(job) = job else {
                            break;
                        };
                        let job = match job {
                            Work::Actor(job) => job,
                            Work::Group(request) => {
                                run_group(request);
                                continue;
                            }
                        };
                        loop {
                            let mut actor = job.lock().unwrap_or_else(|p| p.into_inner());
                            let Some(request) = actor.queue.pop_front() else {
                                actor.scheduled = false;
                                break;
                            };
                            let result = match request.mutation {
                                Mutation::Apply(edit) => actor.document.apply(edit),
                                Mutation::Undo => actor.document.undo(),
                                Mutation::Redo => actor.document.redo(),
                            };
                            let snapshot = actor.document.snapshot();
                            // Receiver may be dropped after cancellation; never wait on the UI.
                            let _ = request.reply.try_send(Completion { result, snapshot });
                            drop(actor);
                            if let Some(notify) = request.notify {
                                notify();
                            }
                        }
                    }
                });
            match result {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    drop(sender);
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(error);
                }
            }
        }
        Ok(Self {
            sender: Some(sender),
            workers: handles,
            id: crate::unique(),
        })
    }
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }
    pub fn document(&self, document: Document, mailbox_capacity: usize) -> DocumentService {
        let document_id = document.current.document_id;
        DocumentService {
            actor: Arc::new(Mutex::new(Actor {
                document,
                queue: std::collections::VecDeque::new(),
                scheduled: false,
            })),
            sender: self.sender.as_ref().unwrap().clone(),
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
        let Some(sender) = &self.sender else {
            return Err((SubmitError::Closed, request.mutation));
        };
        sender.try_send(Work::Group(request)).map_err(|error| {
            let (kind, work) = match error {
                TrySendError::Full(work) => (SubmitError::Saturated, work),
                TrySendError::Disconnected(work) => (SubmitError::Closed, work),
            };
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
            if !actor.queue.is_empty() {
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
        .map(|actor| actor.document.snapshot())
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
        // Services must be dropped first to close the shared channel. Do not block a UI drop.
        self.sender.take();
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}
impl DocumentService {
    pub fn same_document(&self, snapshot: &DocumentSnapshot) -> bool { self.document_id == snapshot.document_id }
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
        let mut actor = match self.actor.try_lock() {
            Ok(actor) => actor,
            Err(_) => return Err((SubmitError::Saturated, mutation)),
        };
        if actor.queue.len() >= self.mailbox_capacity {
            return Err((SubmitError::Saturated, mutation));
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        actor.queue.push_back(Request {
            mutation,
            reply,
            notify,
        });
        if !actor.scheduled {
            match self.sender.try_send(Work::Actor(self.actor.clone())) {
                Ok(()) => actor.scheduled = true,
                Err(error) => {
                    let mutation = actor.queue.pop_back().unwrap().mutation;
                    let error = match error {
                        TrySendError::Full(_) => SubmitError::Saturated,
                        TrySendError::Disconnected(_) => SubmitError::Closed,
                    };
                    return Err((error, mutation));
                }
            }
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
        let failed = pool
            .submit_group(
                GroupMutation::Apply(vec![
                    group_edit(&first, first_snapshot.clone(), "changed"),
                    bad,
                ]),
                None,
            )
            .ok()
            .unwrap()
            .recv()
            .unwrap();
        assert_eq!(failed.result, Err(Error::StaleRevision));
        assert!(
            failed
                .snapshots
                .iter()
                .all(|snapshot| snapshot.revision == Revision(0))
        );
        let completed = pool
            .submit_group(
                GroupMutation::Apply(vec![
                    group_edit(&first, first_snapshot, "one"),
                    group_edit(&second, second_snapshot, "two"),
                ]),
                None,
            )
            .ok()
            .unwrap()
            .recv()
            .unwrap();
        let group = completed.result.unwrap();
        assert!(
            completed
                .snapshots
                .iter()
                .all(|snapshot| snapshot.revision == Revision(1))
        );
        let single = first.submit(Mutation::Undo).ok().unwrap().recv().unwrap();
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
        let undone = pool
            .submit_group(
                GroupMutation::Undo {
                    group,
                    participants,
                },
                None,
            )
            .ok()
            .unwrap()
            .recv()
            .unwrap();
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
        let redone = pool
            .submit_group(
                GroupMutation::Redo {
                    group,
                    participants,
                },
                None,
            )
            .ok()
            .unwrap()
            .recv()
            .unwrap();
        assert_eq!(redone.result, Ok(group));
        assert_eq!(
            redone.snapshots[0]
                .read(TextOffset(0)..TextOffset(3), 3)
                .unwrap(),
            "one"
        );
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
        assert_eq!(budget.used(), 1);
        drop(services);
        drop(pool);
    }
}
