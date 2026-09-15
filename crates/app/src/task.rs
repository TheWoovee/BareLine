// SPDX-License-Identifier: MPL-2.0
//! One-shot background jobs on a shared, bounded thread pool.
//!
//! Historically each feature spawned its own thread with a private
//! `sync_channel(1)` result channel, a private cancellation flag, and a private
//! `notify` wake — 60-odd near-identical copies of the same pattern, and every
//! spawn was one more unbounded OS thread. [`Task`] collapses that pattern onto a
//! single [`Pool`] of a fixed number of worker threads: the number of live
//! threads is bounded no matter how many jobs are outstanding, thread creation is
//! fallible (a failed spawn degrades to "busy" instead of aborting), and cancel /
//! receive / notify are provided once here.
//!
//! It also carries [`Wake`], the typed wake payload the event loop uses to pump
//! only the runtime whose worker actually completed, instead of every runtime on
//! every wake.

use bareline_platform::executor::{BoundedExecutor, SubmitError, WorkKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A boxed unit of work the pool runs to completion on one of its threads.
type Job = Box<dyn FnOnce() + Send + 'static>;

/// Worker threads in the shared pool. Small: these jobs are I/O bound and mostly
/// block on the filesystem, so a handful keeps the UI responsive without letting
/// thread count track document/worker count.
const POOL_THREADS: usize = 4;

/// Total pending queue depth shared by every worker. Submission refuses (returns
/// [`Busy`]) once it is full rather than blocking the caller — the event loop must
/// never stall waiting for a worker.
const QUEUE_DEPTH: usize = 32;

/// The pool had no capacity to accept a job: its shared pending queue was full,
/// admission was closed, or thread creation failed so it has no live workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("background work queue is full")
    }
}

impl std::error::Error for Busy {}

/// A shared cancellation flag. Cloning is cheap (an `Arc` bump) and every clone
/// observes the same cancelled state, so a caller keeps one clone and the running
/// job holds another.
#[derive(Clone, Default)]
pub struct Cancel(Arc<CancelState>);

#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    /// Serializes cancellation with result consumption. Once `cancel` returns,
    /// a concurrent or later poll cannot publish success.
    gate: Mutex<()>,
}

impl Cancel {
    /// Request cancellation. Long jobs poll [`Cancel::is_cancelled`] to stop early;
    /// on completion the pool drops a cancelled job's result instead of delivering
    /// it, so a cancelled [`Task`] never yields a value.
    pub fn cancel(&self) {
        let _gate = self.0.gate.lock().unwrap_or_else(|error| error.into_inner());
        self.0.cancelled.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }
}

struct CompletionSignal {
    notify: Mutex<Box<dyn Fn() + Send + 'static>>,
    fired: AtomicBool,
}

impl CompletionSignal {
    fn new(notify: impl Fn() + Send + 'static) -> Self {
        Self {
            notify: Mutex::new(Box::new(notify)),
            fired: AtomicBool::new(false),
        }
    }

    fn fire(&self) {
        if self.fired.swap(true, Ordering::SeqCst) {
            return;
        }
        let notify = self.notify.lock().unwrap_or_else(|error| error.into_inner());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| notify()));
    }
}

/// A fixed-size pool of worker threads that run [`Job`]s to completion.
///
/// Thread creation is fallible: [`Pool::new`] keeps only the workers that spawned
/// successfully, so under handle/address-space exhaustion the pool ends up with
/// fewer threads (or none, in which case every submission returns [`Busy`]) rather
/// than panicking.
pub struct Pool {
    executor: BoundedExecutor,
}

impl Pool {
    /// Build a pool with `threads` workers sharing one total queue `depth`.
    pub fn new(threads: usize, depth: usize) -> Self {
        Self {
            executor: BoundedExecutor::new(threads, depth, "bareline-task"),
        }
    }

    /// Submit a fire-and-forget job. Returns [`Busy`] if no worker can accept it.
    pub fn execute<F>(&self, job: F) -> Result<(), Busy>
    where
        F: FnOnce() + Send + 'static,
    {
        self.submit(Box::new(job))
    }

    /// Spawn `work` on the pool and return a [`Task`] that receives its result.
    ///
    /// `work` is handed a [`Cancel`] to poll. When it finishes, its value is sent
    /// to the returned task and `notify` is called. Cancellation publishes its own
    /// terminal outcome and wakes the owner immediately. Returns [`Busy`] if no
    /// worker can accept the job.
    pub fn spawn<T, F, N>(&self, notify: N, work: F) -> Result<Task<T>, Busy>
    where
        T: Send + 'static,
        F: FnOnce(&Cancel) -> T + Send + 'static,
        N: Fn() + Send + 'static,
    {
        let cancel = Cancel::default();
        let (sender, receiver) = mpsc::sync_channel::<Outcome<T>>(1);
        let completion = Arc::new(CompletionSignal::new(notify));
        let job_completion = completion.clone();
        let job_cancel = cancel.clone();
        self.submit(Box::new(move || {
            let outcome = if job_cancel.is_cancelled() {
                Outcome::Cancelled
            } else {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&job_cancel))) {
                    Ok(_value) if job_cancel.is_cancelled() => Outcome::Cancelled,
                    Ok(value) => Outcome::Complete(value),
                    Err(_) => Outcome::Failed(TaskFailure::Panicked),
                }
            };
            if sender.send(outcome).is_ok() {
                job_completion.fire();
            }
        }))?;
        Ok(Task {
            receiver,
            cancel,
            consumed: AtomicBool::new(false),
            completion,
        })
    }

    fn submit(&self, job: Job) -> Result<(), Busy> {
        self.executor
            .submit(WorkKind::General, job)
            .map_err(|_error: SubmitError| Busy)
    }

    /// Number of live worker threads — the ceiling on concurrent jobs.
    pub fn worker_count(&self) -> usize {
        self.executor.worker_count()
    }

    /// High-water mark of jobs that have run concurrently. Bounded by
    /// [`Pool::worker_count`]; used by tests to prove the pool does not spawn a
    /// thread per job.
    pub fn peak_concurrency(&self) -> usize {
        self.executor.stats().peak_running
    }

    /// Jobs executing right now.
    pub fn running(&self) -> usize {
        self.executor.stats().running
    }
}

/// A running background job whose result is delivered on completion.
///
/// Poll [`Task::poll`] each frame (non-blocking) for the value. [`Task::cancel`]
/// both signals the running job and guarantees the result is never delivered even
/// if the job was already finishing.
pub struct Task<T> {
    // `Receiver` is deliberately not synchronized. Its `!Sync` boundary makes
    // the one-shot task a single-consumer API, so `poll` cannot block behind a
    // concurrent `wait_timeout` call.
    receiver: Receiver<Outcome<T>>,
    cancel: Cancel,
    consumed: AtomicBool,
    completion: Arc<CompletionSignal>,
}

enum Outcome<T> {
    Complete(T),
    Cancelled,
    Failed(TaskFailure),
}

/// The observable state of a one-shot background task.
#[derive(Debug, PartialEq, Eq)]
pub enum TaskPoll<T> {
    Pending,
    Complete(T),
    Cancelled,
    Failed(TaskFailure),
    /// The terminal outcome was already returned to this task's owner.
    Consumed,
}

/// Sanitized task infrastructure failures. Panic payloads are never exposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskFailure {
    Panicked,
    Disconnected,
}

impl std::fmt::Display for TaskFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Panicked => f.write_str("background task failed"),
            Self::Disconnected => f.write_str("background worker disconnected"),
        }
    }
}

impl<T> Task<T> {
    /// Request cancellation and suppress delivery of any result.
    pub fn cancel(&self) {
        self.cancel.cancel();
        self.completion.fire();
    }

    /// Whether this task has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// A clone of this task's cancellation handle, for a job that wants to observe
    /// cancellation through a separate path.
    pub fn cancel_handle(&self) -> Cancel {
        self.cancel.clone()
    }

    /// Non-blocking check for an explicit outcome. A terminal outcome is returned
    /// once; later calls return [`TaskPoll::Consumed`].
    pub fn poll(&self) -> TaskPoll<T> {
        let _gate = self.cancel.0.gate.lock().unwrap_or_else(|error| error.into_inner());
        if self.consumed.load(Ordering::SeqCst) {
            return TaskPoll::Consumed;
        }
        if self.cancel.is_cancelled() {
            self.consumed.store(true, Ordering::SeqCst);
            return TaskPoll::Cancelled;
        }
        let outcome = self.receiver.try_recv();
        match outcome {
            Ok(outcome) => self.finish(outcome),
            Err(TryRecvError::Empty) => TaskPoll::Pending,
            Err(TryRecvError::Disconnected) => {
                self.consumed.store(true, Ordering::SeqCst);
                if self.cancel.is_cancelled() {
                    TaskPoll::Cancelled
                } else {
                    TaskPoll::Failed(TaskFailure::Disconnected)
                }
            }
        }
    }

    /// Block up to `timeout` for the result. Intended for tests and synchronous
    /// call sites; the event loop uses [`Task::poll`] instead.
    pub fn wait_timeout(&self, timeout: Duration) -> TaskPoll<T> {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            {
                let _gate = self.cancel.0.gate.lock().unwrap_or_else(|error| error.into_inner());
                if self.consumed.load(Ordering::SeqCst) {
                    return TaskPoll::Consumed;
                }
                if self.cancel.is_cancelled() {
                    self.consumed.store(true, Ordering::SeqCst);
                    return TaskPoll::Cancelled;
                }
            }
            let remaining = deadline.map_or(timeout, |deadline| deadline.saturating_duration_since(Instant::now()));
            if remaining.is_zero() {
                return TaskPoll::Pending;
            }
            let slice = remaining.min(Duration::from_millis(10));
            match self.receiver.recv_timeout(slice) {
                Ok(outcome) => {
                    let _gate = self.cancel.0.gate.lock().unwrap_or_else(|error| error.into_inner());
                    if self.consumed.swap(true, Ordering::SeqCst) {
                        return TaskPoll::Consumed;
                    }
                    return self.map_outcome(outcome);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _gate = self.cancel.0.gate.lock().unwrap_or_else(|error| error.into_inner());
                    if self.consumed.swap(true, Ordering::SeqCst) {
                        return TaskPoll::Consumed;
                    }
                    return if self.cancel.is_cancelled() {
                        TaskPoll::Cancelled
                    } else {
                        TaskPoll::Failed(TaskFailure::Disconnected)
                    };
                }
            }
        }
    }

    fn finish(&self, outcome: Outcome<T>) -> TaskPoll<T> {
        self.consumed.store(true, Ordering::SeqCst);
        self.map_outcome(outcome)
    }

    fn map_outcome(&self, outcome: Outcome<T>) -> TaskPoll<T> {
        if self.cancel.is_cancelled() {
            return TaskPoll::Cancelled;
        }
        match outcome {
            Outcome::Complete(value) => TaskPoll::Complete(value),
            Outcome::Cancelled => TaskPoll::Cancelled,
            Outcome::Failed(error) => TaskPoll::Failed(error),
        }
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// The process-wide pool used by [`spawn`] and [`execute`].
fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(POOL_THREADS, QUEUE_DEPTH))
}

/// Spawn `work` on the shared pool. See [`Pool::spawn`].
pub fn spawn<T, F, N>(notify: N, work: F) -> Result<Task<T>, Busy>
where
    T: Send + 'static,
    F: FnOnce(&Cancel) -> T + Send + 'static,
    N: Fn() + Send + 'static,
{
    pool().spawn(notify, work)
}

/// Run a fire-and-forget job on the shared pool. See [`Pool::execute`].
pub fn execute<F>(job: F) -> Result<(), Busy>
where
    F: FnOnce() + Send + 'static,
{
    pool().execute(job)
}

/// A runtime whose background worker can complete and needs pumping.
///
/// Each variant maps one-to-one to a `*_pump` method on the shell. A wake carries
/// the source that woke the loop so only that runtime is pumped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Search,
    Shortcuts,
    Instance,
    Launch,
    Session,
    Recovery,
    Lifecycle,
    Encoding,
    Migration,
    ShellRecent,
    Performance,
    Power,
    Utilities,
    Macros,
    Panels,
    Watch,
    Language,
    Extensions,
    Inventory,
    Compare,
    Update,
    Settings,
}

impl Source {
    /// Every pump source, in the order the event loop runs them for a full wake.
    pub const ALL: [Source; 22] = [
        Source::Search,
        Source::Shortcuts,
        Source::Instance,
        Source::Launch,
        Source::Session,
        Source::Recovery,
        Source::Lifecycle,
        Source::Encoding,
        Source::Migration,
        Source::ShellRecent,
        Source::Performance,
        Source::Power,
        Source::Utilities,
        Source::Macros,
        Source::Panels,
        Source::Watch,
        Source::Language,
        Source::Extensions,
        Source::Inventory,
        Source::Compare,
        Source::Update,
        Source::Settings,
    ];
}

/// The typed user-event payload the event loop wakes on.
///
/// [`Wake::All`] pumps every runtime (the conservative default used by the generic
/// notify and any worker not yet routed to its own source). [`Wake::One`] pumps
/// exactly the one runtime whose worker completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wake {
    All,
    One(Source),
}

impl Wake {
    /// Whether a full wake should run the pump for `source`.
    pub fn runs(self, source: Source) -> bool {
        match self {
            Wake::All => true,
            Wake::One(one) => one == source,
        }
    }

    /// Invoke `pump` once for each source this wake selects: every source for
    /// [`Wake::All`], exactly one for [`Wake::One`].
    pub fn dispatch(self, mut pump: impl FnMut(Source)) {
        match self {
            Wake::All => Source::ALL.iter().for_each(|&source| pump(source)),
            Wake::One(source) => pump(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Condvar, Mutex};

    /// A gate a test opens once to release every job blocked on it.
    #[derive(Default)]
    struct Gate {
        open: Mutex<bool>,
        cond: Condvar,
    }

    impl Gate {
        fn wait(&self) {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.cond.wait(open).unwrap();
            }
        }

        fn release(&self) {
            *self.open.lock().unwrap() = true;
            self.cond.notify_all();
        }
    }

    /// A counter jobs bump on entry, so the test can wait for exactly N starts.
    #[derive(Default)]
    struct Started {
        count: Mutex<usize>,
        cond: Condvar,
    }

    impl Started {
        fn mark(&self) {
            *self.count.lock().unwrap() += 1;
            self.cond.notify_all();
        }

        fn wait_for(&self, target: usize) {
            let mut count = self.count.lock().unwrap();
            while *count < target {
                let (guard, timeout) = self.cond.wait_timeout(count, Duration::from_secs(5)).unwrap();
                count = guard;
                assert!(!timeout.timed_out(), "timed out waiting for {target} starts");
            }
        }

        fn get(&self) -> usize {
            *self.count.lock().unwrap()
        }
    }

    #[test]
    fn task_completes_and_delivers_its_result() {
        let pool = Pool::new(2, 8);
        let woke = Arc::new(AtomicUsize::new(0));
        let (notified_tx, notified_rx) = mpsc::sync_channel(1);
        let notify = {
            let woke = woke.clone();
            move || {
                woke.fetch_add(1, Ordering::SeqCst);
                notified_tx.send(()).unwrap();
            }
        };
        let task = pool.spawn(notify, |_cancel| 40 + 2).expect("accepted");
        assert_eq!(task.wait_timeout(Duration::from_secs(5)), TaskPoll::Complete(42));
        notified_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(task.poll(), TaskPoll::Consumed);
        assert_eq!(woke.load(Ordering::SeqCst), 1, "notify fires once on delivery");
    }

    #[test]
    fn task_cancel_prevents_delivery() {
        let pool = Pool::new(2, 8);
        let gate = Arc::new(Gate::default());
        let woke = Arc::new(AtomicUsize::new(0));
        let notify = {
            let woke = woke.clone();
            move || {
                woke.fetch_add(1, Ordering::SeqCst);
            }
        };
        let job_gate = gate.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let task = pool
            .spawn(notify, move |_cancel| {
                started_tx.send(()).unwrap();
                job_gate.wait();
                99
            })
            .expect("accepted");
        // Cancel while the job is blocked, then let it finish. The finished value
        // must not be delivered.
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        task.cancel();
        assert_eq!(task.poll(), TaskPoll::Cancelled);
        assert_eq!(woke.load(Ordering::SeqCst), 1, "terminal cancellation wakes its owner");
        gate.release();
        assert!(task.is_cancelled());
    }

    #[test]
    fn pool_bound_is_respected_when_more_tasks_than_threads_are_queued() {
        // Two worker threads, deep queues. Submitting three blocking jobs must never
        // run more than two at once: the third queues rather than spawning a thread.
        let pool = Pool::new(2, 8);
        let gate = Arc::new(Gate::default());
        let started = Arc::new(Started::default());
        let mut tasks = Vec::new();
        for _ in 0..3 {
            let job_gate = gate.clone();
            let job_started = started.clone();
            let task = pool
                .spawn(
                    || {},
                    move |_cancel| {
                        job_started.mark();
                        job_gate.wait();
                        1u8
                    },
                )
                .expect("accepted");
            tasks.push(task);
        }
        // Both threads are busy; the third job is queued and cannot have started.
        started.wait_for(2);
        assert_eq!(started.get(), 2, "only worker_count jobs run before release");
        assert_eq!(pool.running(), 2);
        assert_eq!(pool.worker_count(), 2);
        // Release everyone and drain.
        gate.release();
        for task in &tasks {
            assert_eq!(task.wait_timeout(Duration::from_secs(5)), TaskPoll::Complete(1));
        }
        assert_eq!(started.get(), 3, "all three eventually ran");
        assert_eq!(pool.peak_concurrency(), 2, "never exceeded the pool bound");
    }

    #[test]
    fn short_job_uses_idle_worker_while_long_job_remains_blocked() {
        let pool = Pool::new(2, 4);
        let (long_started_tx, long_started_rx) = mpsc::sync_channel(1);
        let (long_release_tx, long_release_rx) = mpsc::sync_channel(1);
        let long = pool
            .spawn(
                || {},
                move |_| {
                    long_started_tx.send(()).unwrap();
                    long_release_rx.recv().unwrap();
                    1
                },
            )
            .unwrap();
        long_started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let free = pool.spawn(|| {}, |_| 2).unwrap();
        assert_eq!(free.wait_timeout(Duration::from_secs(5)), TaskPoll::Complete(2));
        let short = pool.spawn(|| {}, |_| 3).unwrap();
        assert_eq!(
            short.wait_timeout(Duration::from_secs(5)),
            TaskPoll::Complete(3),
            "an idle worker must take the next shared-queue job"
        );
        assert_eq!(pool.worker_count(), 2);
        long_release_tx.send(()).unwrap();
        assert_eq!(long.wait_timeout(Duration::from_secs(5)), TaskPoll::Complete(1));
    }

    #[test]
    fn cancel_before_dequeue_skips_work_and_reports_terminal() {
        let pool = Pool::new(1, 4);
        let gate = Arc::new(Gate::default());
        let started = Arc::new(Started::default());
        let first_gate = gate.clone();
        let first_started = started.clone();
        let first = pool
            .spawn(
                || {},
                move |_| {
                    first_started.mark();
                    first_gate.wait();
                },
            )
            .unwrap();
        started.wait_for(1);
        let ran = Arc::new(AtomicBool::new(false));
        let job_ran = ran.clone();
        let (woke_tx, woke_rx) = mpsc::sync_channel(1);
        let second = pool
            .spawn(
                move || woke_tx.send(()).unwrap(),
                move |_| job_ran.swap(true, Ordering::SeqCst),
            )
            .unwrap();
        second.cancel();
        woke_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            second.poll(),
            TaskPoll::Cancelled,
            "cancellation is terminal before the queued job can start"
        );
        gate.release();
        assert_eq!(first.wait_timeout(Duration::from_secs(5)), TaskPoll::Complete(()));
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn cancel_after_publication_suppresses_buffered_success() {
        let pool = Pool::new(1, 4);
        let (woke, wake) = mpsc::sync_channel(1);
        let task = pool.spawn(move || woke.send(()).unwrap(), |_| 42).unwrap();
        wake.recv_timeout(Duration::from_secs(5)).unwrap();
        task.cancel();
        assert_eq!(task.poll(), TaskPoll::Cancelled);
        assert_eq!(task.poll(), TaskPoll::Consumed);
    }

    #[test]
    fn dropping_owner_signals_running_work_and_send_failure_is_harmless() {
        let pool = Pool::new(1, 4);
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (observed_tx, observed_rx) = mpsc::sync_channel(1);
        let task = pool
            .spawn(
                || {},
                move |cancel| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    observed_tx.send(cancel.is_cancelled()).unwrap();
                },
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(task);
        release_tx.send(()).unwrap();
        assert!(observed_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        let next = pool.spawn(|| {}, |_| 7).unwrap();
        assert_eq!(next.wait_timeout(Duration::from_secs(5)), TaskPoll::Complete(7));
    }

    #[test]
    fn panicked_work_is_sanitized_and_worker_metrics_recover() {
        let pool = Pool::new(1, 4);
        let failed = pool
            .spawn(|| {}, |_| -> usize { panic!("secret panic payload") })
            .unwrap();
        assert_eq!(
            failed.wait_timeout(Duration::from_secs(5)),
            TaskPoll::Failed(TaskFailure::Panicked)
        );
        let gate = Arc::new(Gate::default());
        let next_gate = gate.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let next = pool
            .spawn(
                || {},
                move |_| {
                    started_tx.send(()).unwrap();
                    next_gate.wait();
                    9
                },
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(pool.running(), 1, "failed job was removed from running metrics");
        gate.release();
        assert_eq!(next.wait_timeout(Duration::from_secs(5)), TaskPoll::Complete(9));
        assert_eq!(pool.worker_count(), 1);
    }

    #[test]
    fn panicked_notification_does_not_kill_worker_or_lose_result() {
        let pool = Pool::new(1, 4);
        let task = pool.spawn(|| panic!("notify payload"), |_| 5).unwrap();
        assert_eq!(task.wait_timeout(Duration::from_secs(5)), TaskPoll::Complete(5));
        let next = pool.spawn(|| {}, |_| 6).unwrap();
        assert_eq!(next.wait_timeout(Duration::from_secs(5)), TaskPoll::Complete(6));
        assert_eq!(pool.worker_count(), 1);
    }

    #[test]
    fn disconnected_result_is_a_one_shot_failure() {
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(sender);
        let task = Task::<usize> {
            receiver,
            cancel: Cancel::default(),
            consumed: AtomicBool::new(false),
            completion: Arc::new(CompletionSignal::new(|| {})),
        };
        assert_eq!(task.poll(), TaskPoll::Failed(TaskFailure::Disconnected));
        assert_eq!(task.poll(), TaskPoll::Consumed);
    }

    #[test]
    fn closed_pool_rejects_new_work() {
        let pool = Pool::new(1, 1);
        pool.executor.close();
        assert!(pool.execute(|| {}).is_err());
    }

    #[test]
    fn wake_variant_routes_to_exactly_one_pump() {
        // A per-source counter the dispatch increments.
        let count = |wake: Wake| {
            let mut hits = std::collections::HashMap::new();
            wake.dispatch(|source| *hits.entry(source).or_insert(0usize) += 1);
            hits
        };

        // Every specific wake routes to exactly one pump, and it is the right one.
        for &source in &Source::ALL {
            let hits = count(Wake::One(source));
            let total: usize = hits.values().sum();
            assert_eq!(total, 1, "{source:?} must pump exactly one runtime");
            assert_eq!(hits.get(&source), Some(&1));
            assert!(Wake::One(source).runs(source));
            // ...and no other source is pumped or considered run.
            for &other in &Source::ALL {
                if other != source {
                    assert!(!Wake::One(source).runs(other));
                    assert_eq!(hits.get(&other), None);
                }
            }
        }

        // A full wake routes to every pump exactly once.
        let hits = count(Wake::All);
        assert_eq!(hits.len(), Source::ALL.len());
        assert!(hits.values().all(|&n| n == 1));
    }
}
