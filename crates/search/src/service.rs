// SPDX-License-Identifier: MPL-2.0
//! One lazy search worker with a coalescing mailbox: one running and one pending query.
use super::sources::{OpenDocumentResults, PagedOpenDocument, scan_mixed_open_documents};
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
type PageResolver = Box<dyn FnMut(bareline_document::source::PageTicket) -> Result<bool, String> + Send>;
enum Work {
    Operation {
        run: Box<dyn FnOnce(&SearchJob) + Send>,
        reject: Box<dyn FnOnce() + Send>,
    },
    Folder {
        scope: super::folders::FolderScope,
        query: SearchQuery,
        trust: Arc<dyn bareline_platform::PathTrustProvider + Send + Sync>,
        platform: Arc<dyn bareline_platform::LocalFileSystem>,
        reply: SyncSender<Result<super::folders::FolderResults, SearchError>>,
    },
    ReplacePaged {
        results: Arc<super::paged::PagedResults>,
        snapshot: bareline_document::paged::PagedSnapshot,
        replacement: String,
        scope: ReplaceScope,
        resolve: PageResolver,
        reply: SyncSender<Result<PreparedPagedReplacement, ReplaceError>>,
    },
    Paged {
        snapshot: bareline_document::paged::PagedSnapshot,
        query: SearchQuery,
        resolve: PageResolver,
        reply: SyncSender<Result<super::paged::PagedResults, SearchError>>,
    },
    OpenDocuments {
        paged: Vec<PagedOpenDocument>,
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
pub struct PreparedPagedReplacement {
    pub source: bareline_document::paged::PagedSnapshot,
    pub transaction: EditTransaction,
}
pub struct PreparedReplacement {
    pub source: DocumentSnapshot,
    pub transaction: EditTransaction,
}
impl Request {
    fn reject(self, error: SearchError) {
        self.job.cancel();
        match self.work {
            Work::Operation { reject, .. } => reject(),
            Work::Folder { reply, .. } => {
                let _ = reply.try_send(Err(error));
            }
            Work::ReplacePaged { reply, .. } => {
                let _ = reply.try_send(Err(ReplaceError::Cancelled));
            }
            Work::Paged { reply, .. } => {
                let _ = reply.try_send(Err(error));
            }
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
        self.job.acknowledge_terminal();
        (self.notify)();
    }
    fn execute(self) {
        match self.work {
            Work::Operation { run, .. } => run(&self.job),
            Work::Folder {
                scope,
                query,
                trust,
                platform,
                reply,
            } => {
                let _ = reply.try_send(Ok(super::folders::collect_folder(
                    &scope,
                    &query,
                    &self.job,
                    trust.as_ref(),
                    platform,
                )));
            }
            Work::ReplacePaged {
                results,
                snapshot,
                replacement,
                scope,
                resolve,
                reply,
            } => {
                let result = results
                    .prepare_replace(&snapshot, &replacement, scope, &self.job, resolve)
                    .map(|transaction| PreparedPagedReplacement {
                        source: snapshot,
                        transaction,
                    });
                let _ = reply.try_send(result);
            }
            Work::Paged {
                snapshot,
                query,
                resolve,
                reply,
            } => {
                let _ = reply.try_send(Ok(super::paged::scan_paged(
                    &snapshot,
                    &query,
                    &self.job,
                    resolve,
                    |_| {},
                )));
            }
            Work::OpenDocuments {
                paged,
                snapshots,
                query,
                reply,
            } => {
                let _ = reply.try_send(Ok(scan_mixed_open_documents(snapshots, paged, &query, &self.job)));
            }
            Work::Search { snapshot, query, reply } => {
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
        self.job.acknowledge_terminal();
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
pub struct PagedReplaceTicket {
    pub job: SearchJob,
    receiver: Receiver<Result<PreparedPagedReplacement, ReplaceError>>,
}
impl PagedReplaceTicket {
    pub fn try_recv(&self) -> Result<Result<PreparedPagedReplacement, ReplaceError>, TryRecvError> {
        self.receiver.try_recv()
    }
}
impl Drop for PagedReplaceTicket {
    fn drop(&mut self) {
        self.job.cancel();
    }
}
pub struct FolderSearchTicket {
    pub job: SearchJob,
    receiver: Receiver<Result<super::folders::FolderResults, SearchError>>,
}
impl FolderSearchTicket {
    pub fn try_recv(&self) -> Result<Result<super::folders::FolderResults, SearchError>, TryRecvError> {
        self.receiver.try_recv()
    }
}
impl Drop for FolderSearchTicket {
    fn drop(&mut self) {
        self.job.cancel();
    }
}
pub struct BackgroundTicket<T> {
    pub job: SearchJob,
    receiver: Receiver<Result<T, String>>,
}
impl<T> BackgroundTicket<T> {
    pub fn try_recv(&self) -> Result<Result<T, String>, TryRecvError> {
        self.receiver.try_recv()
    }
}
impl<T> Drop for BackgroundTicket<T> {
    fn drop(&mut self) {
        self.job.cancel();
    }
}
pub struct PagedSearchTicket {
    pub job: SearchJob,
    receiver: Receiver<Result<super::paged::PagedResults, SearchError>>,
}
impl PagedSearchTicket {
    pub fn try_recv(&self) -> Result<Result<super::paged::PagedResults, SearchError>, TryRecvError> {
        self.receiver.try_recv()
    }
}
impl Drop for PagedSearchTicket {
    fn drop(&mut self) {
        self.job.cancel();
    }
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
    /// Shares the same bounded/coalescing worker with search and preview preparation.
    pub fn operation<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&SearchJob) -> Result<T, String> + Send + 'static,
        notify: Notify,
    ) -> BackgroundTicket<T> {
        let job = SearchJob::default();
        let (reply, receiver) = mpsc::sync_channel(1);
        let rejected = reply.clone();
        self.enqueue(Request {
            work: Work::Operation {
                run: Box::new(move |job| {
                    let _ = reply.try_send(operation(job));
                }),
                reject: Box::new(move || {
                    let _ = rejected.try_send(Err("Operation superseded".into()));
                }),
            },
            job: job.clone(),
            notify,
        });
        BackgroundTicket { job, receiver }
    }

    pub fn submit_folder(
        &self,
        scope: super::folders::FolderScope,
        query: SearchQuery,
        trust: Arc<dyn bareline_platform::PathTrustProvider + Send + Sync>,
        platform: Arc<dyn bareline_platform::LocalFileSystem>,
        notify: Notify,
    ) -> FolderSearchTicket {
        let job = SearchJob::default();
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(Request {
            work: Work::Folder {
                scope,
                query,
                trust,
                platform,
                reply,
            },
            job: job.clone(),
            notify,
        });
        FolderSearchTicket { job, receiver }
    }

    pub fn replace_paged(
        &self,
        results: Arc<super::paged::PagedResults>,
        snapshot: bareline_document::paged::PagedSnapshot,
        replacement: String,
        scope: ReplaceScope,
        resolve: impl FnMut(bareline_document::source::PageTicket) -> Result<bool, String> + Send + 'static,
        notify: Notify,
    ) -> PagedReplaceTicket {
        let job = SearchJob::default();
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(Request {
            work: Work::ReplacePaged {
                results,
                snapshot,
                replacement,
                scope,
                resolve: Box::new(resolve),
                reply,
            },
            job: job.clone(),
            notify,
        });
        PagedReplaceTicket { job, receiver }
    }

    pub fn submit_paged(
        &self,
        snapshot: bareline_document::paged::PagedSnapshot,
        query: SearchQuery,
        resolve: impl FnMut(bareline_document::source::PageTicket) -> Result<bool, String> + Send + 'static,
        notify: Notify,
    ) -> PagedSearchTicket {
        let job = SearchJob::default();
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(Request {
            work: Work::Paged {
                snapshot,
                query,
                resolve: Box::new(resolve),
                reply,
            },
            job: job.clone(),
            notify,
        });
        PagedSearchTicket { job, receiver }
    }

    pub fn submit_open_documents(
        &self,
        snapshots: Vec<DocumentSnapshot>,
        query: SearchQuery,
        notify: Notify,
    ) -> OpenDocumentTicket {
        self.submit_mixed_open_documents(snapshots, Vec::new(), query, notify)
    }
    pub fn submit_mixed_open_documents(
        &self,
        snapshots: Vec<DocumentSnapshot>,
        paged: Vec<PagedOpenDocument>,
        query: SearchQuery,
        notify: Notify,
    ) -> OpenDocumentTicket {
        let job = SearchJob::default();
        let (reply, receiver) = mpsc::sync_channel(1);
        self.enqueue(Request {
            work: Work::OpenDocuments {
                paged,
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
        std::thread::Builder::new().name("search".into()).spawn(move || {
            loop {
                let request = {
                    let mut state = worker.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
                worker
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .running = None;
            }
        })?;
        Ok(Self { shared })
    }
    /// Replaces obsolete queued work and cancels the active scan. UI callers never wait
    /// for a scan; the short mailbox lock is the only synchronous coordination.
    pub fn submit(&self, snapshot: DocumentSnapshot, query: SearchQuery, notify: Notify) -> SearchTicket {
        let job = SearchJob::default();
        let (reply, receiver) = mpsc::sync_channel(1);
        let request = Request {
            work: Work::Search { snapshot, query, reply },
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
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        receiver.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let result = ticket.try_recv().unwrap().unwrap();
        assert_eq!(result.completeness(), super::super::Completeness::Complete);
        assert_eq!(result.count(), 3);
        assert!(result.documents()[0].source().same_document(&first));
    }
    #[test]
    fn rapid_queries_coalesce_and_latest_completion_notifies_without_polling() {
        let snapshot = Document::from_utf8(&"abc".repeat(100_000), Budget::new(1 << 20), Budget::new(1 << 20))
            .unwrap()
            .snapshot();
        let worker = SearchWorker::new().unwrap();
        let (notify, notified) = mpsc::channel();
        let notify: Notify = Arc::new(move || {
            let _ = notify.send(());
        });
        let mut tickets = Vec::new();
        for _ in 0..12 {
            tickets.push(worker.submit(snapshot.clone(), SearchQuery::literal("abc"), notify.clone()));
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
