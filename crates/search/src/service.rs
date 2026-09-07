// SPDX-License-Identifier: MPL-2.0
//! One lazy search worker with a coalescing mailbox: one running and one pending query.
use super::sources::{OpenDocumentResults, scan_open_documents};
use super::{ReplaceError, ReplaceScope, SearchJob, SearchQuery, SearchResults, scan};
use bareline_document::{DocumentSnapshot, EditTransaction};
use std::sync::{
    Arc, Condvar, Mutex,
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};

type Notify = Arc<dyn Fn() + Send + Sync>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchError {
    Superseded,
    Stopped,
}
type ResultMessage = Result<SearchResults, SearchError>;
struct Request {
    work: Work,
    job: SearchJob,
    notify: Notify,
}
enum Work {
    OpenDocuments {
        snapshots: Vec<DocumentSnapshot>,
        query: SearchQuery,
        reply: SyncSender<Result<OpenDocumentResults, SearchError>>,
    },
    Search {
        snapshot: DocumentSnapshot,
        query: SearchQuery,
        reply: SyncSender<ResultMessage>,
    },
    Replace {
        results: Arc<SearchResults>,
        snapshot: DocumentSnapshot,
        replacement: String,
        scope: ReplaceScope,
        limit: usize,
        reply: SyncSender<Result<PreparedReplacement, ReplaceError>>,
    },
}
pub struct PreparedReplacement {
    pub source: DocumentSnapshot,
    pub transaction: EditTransaction,
}
impl Request {
    fn reject(self, error: SearchError) {
        self.job.cancel();
        match self.work {
            Work::OpenDocuments { reply, .. } => {
                let _ = reply.try_send(Err(error));
            }
            Work::Search { reply, .. } => {
                let _ = reply.try_send(Err(error));
            }
            Work::Replace { reply, .. } => {
                let _ = reply.try_send(Err(ReplaceError::Cancelled));
            }
        }
        (self.notify)();
    }
    fn execute(self) {
        match self.work {
            Work::OpenDocuments {
                snapshots,
                query,
                reply,
            } => {
                let _ = reply.try_send(Ok(scan_open_documents(
                    snapshots,
                    &query,
                    &self.job,
                    |_| {},
                )));
            }
            Work::Search {
                snapshot,
                query,
                reply,
            } => {
                let _ = reply.try_send(Ok(scan(&snapshot, &query, &self.job, |_| {})));
            }
            Work::Replace {
                results,
                snapshot,
                replacement,
                scope,
                limit,
                reply,
            } => {
                let result = results
                    .prepare_replace_scoped(&snapshot, &replacement, limit, scope, &self.job)
                    .map(|transaction| PreparedReplacement {
                        source: snapshot,
                        transaction,
                    });
                let _ = reply.try_send(result);
            }
        }
        (self.notify)();
    }
}
struct State {
    pending: Option<Request>,
    running: Option<SearchJob>,
    stop: bool,
}
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}
pub struct SearchWorker {
    shared: Arc<Shared>,
}
pub struct SearchTicket {
    pub job: SearchJob,
    receiver: Receiver<ResultMessage>,
}
pub struct OpenDocumentTicket {
    pub job: SearchJob,
    receiver: Receiver<Result<OpenDocumentResults, SearchError>>,
}
impl OpenDocumentTicket {
    pub fn try_recv(&self) -> Result<Result<OpenDocumentResults, SearchError>, TryRecvError> {
        self.receiver.try_recv()
    }
}
impl Drop for OpenDocumentTicket {
    fn drop(&mut self) {
        self.job.cancel();
    }
}
pub struct ReplaceTicket {
    pub job: SearchJob,
    receiver: Receiver<Result<PreparedReplacement, ReplaceError>>,
}
impl ReplaceTicket {
    pub fn try_recv(&self) -> Result<Result<PreparedReplacement, ReplaceError>, TryRecvError> {
        self.receiver.try_recv()
    }
}
impl Drop for ReplaceTicket {
    fn drop(&mut self) {
        self.job.cancel();
    }
}
impl SearchTicket {
    pub fn try_recv(&self) -> Result<ResultMessage, TryRecvError> {
        self.receiver.try_recv()
    }
}
impl Drop for SearchTicket {
    fn drop(&mut self) {
        self.job.cancel();
    }
}
impl SearchWorker {
    pub fn submit_open_documents(
        &self,
        snapshots: Vec<DocumentSnapshot>,
        query: SearchQuery,
        notify: Notify,
    ) -> OpenDocumentTicket {
        let job = SearchJob::default();
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(Request {
            work: Work::OpenDocuments {
                snapshots,
                query,
                reply,
            },
            job: job.clone(),
            notify,
        });
        OpenDocumentTicket { job, receiver }
    }
    pub fn new() -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                pending: None,
                running: None,
                stop: false,
            }),
            ready: Condvar::new(),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("search".into())
            .spawn(move || {
                loop {
                    let request = {
                        let mut state = worker.state.lock().unwrap();
                        while state.pending.is_none() && !state.stop {
                            state = worker.ready.wait(state).unwrap();
                        }
                        if state.stop {
                            break;
                        }
                        let request = state.pending.take().unwrap();
                        state.running = Some(request.job.clone());
                        request
                    };
                    request.execute();
                    worker.state.lock().unwrap().running = None;
                }
            })?;
        Ok(Self { shared })
    }
    /// Replaces obsolete queued work and cancels the active scan. UI callers never wait
    /// for a scan; the short mailbox lock is the only synchronous coordination.
    pub fn submit(
        &self,
        snapshot: DocumentSnapshot,
        query: SearchQuery,
        notify: Notify,
    ) -> SearchTicket {
        let job = SearchJob::default();
        let (reply, receiver) = mpsc::sync_channel(1);
        let request = Request {
            work: Work::Search {
                snapshot,
                query,
                reply,
            },
            job: job.clone(),
            notify,
        };
        self.enqueue(request);
        SearchTicket { job, receiver }
    }
    pub fn replace(
        &self,
        results: Arc<SearchResults>,
        snapshot: DocumentSnapshot,
        replacement: String,
        scope: ReplaceScope,
        notify: Notify,
    ) -> ReplaceTicket {
        let job = SearchJob::default();
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(Request {
            work: Work::Replace {
                results,
                snapshot,
                replacement,
                scope,
                limit: super::MAX_RESULT_BYTES,
                reply,
            },
            job: job.clone(),
            notify,
        });
        ReplaceTicket { job, receiver }
    }
    fn enqueue(&self, request: Request) {
        let previous = {
            let mut state = self.shared.state.lock().unwrap();
            if let Some(running) = &state.running {
                running.cancel();
            }
            state.pending.replace(request)
        };
        self.shared.ready.notify_one();
        if let Some(previous) = previous {
            previous.reject(SearchError::Superseded);
        }
    }
}
impl Drop for SearchWorker {
    fn drop(&mut self) {
        let pending = {
            let mut state = self.shared.state.lock().unwrap();
            state.stop = true;
            if let Some(running) = &state.running {
                running.cancel();
            }
            state.pending.take()
        };
        self.shared.ready.notify_one();
        if let Some(pending) = pending {
            pending.reject(SearchError::Stopped);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    #[test]
    fn open_document_ticket_notifies_with_grouped_results() {
        let first = Document::from_utf8("x x", Budget::new(4096), Budget::new(4096))
            .unwrap()
            .snapshot();
        let second = Document::from_utf8("x", Budget::new(4096), Budget::new(4096))
            .unwrap()
            .snapshot();
        let worker = SearchWorker::new().unwrap();
        let (sender, receiver) = mpsc::channel();
        let ticket = worker.submit_open_documents(
            vec![first.clone(), second],
            SearchQuery::literal("x"),
            Arc::new(move || {
                let _ = sender.send(());
            }),
        );
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let result = ticket.try_recv().unwrap().unwrap();
        assert_eq!(result.completeness(), super::super::Completeness::Complete);
        assert_eq!(result.count(), 3);
        assert!(result.documents()[0].source().same_document(&first));
    }
    #[test]
    fn rapid_queries_coalesce_and_latest_completion_notifies_without_polling() {
        let snapshot = Document::from_utf8(
            &"abc".repeat(100_000),
            Budget::new(1 << 20),
            Budget::new(1 << 20),
        )
        .unwrap()
        .snapshot();
        let worker = SearchWorker::new().unwrap();
        let (notify, notified) = mpsc::channel();
        let notify: Notify = Arc::new(move || {
            let _ = notify.send(());
        });
        let mut tickets = Vec::new();
        for _ in 0..12 {
            tickets.push(worker.submit(
                snapshot.clone(),
                SearchQuery::literal("abc"),
                notify.clone(),
            ));
        }
        let latest = worker.submit(snapshot, SearchQuery::literal("xyz"), notify);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            notified
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .unwrap();
            if let Ok(result) = latest.try_recv() {
                let result = result.unwrap();
                assert_eq!(result.completeness(), super::super::Completeness::Complete);
                assert_eq!(result.count(), 0);
                break;
            }
        }
        drop(tickets);
    }
}
