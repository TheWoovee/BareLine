// SPDX-License-Identifier: MPL-2.0
//! Small bounded executor with one bounded FIFO queue shared by every worker.
//! Workers start on first use, never up front (PERF-02).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

pub type Job = Box<dyn FnOnce() + Send + 'static>;

/// Privacy-safe classification used only for aggregate scheduling diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum WorkKind {
    General = 0,
    Interactive = 1,
    Bulk = 2,
    Maintenance = 3,
}

const WORK_KIND_COUNT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitError {
    Busy,
    Closed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KindStats {
    pub submitted: usize,
    pub completed: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExecutorStats {
    pub submitted: usize,
    pub running: usize,
    pub queued: usize,
    pub rejected: usize,
    pub completed: usize,
    pub peak_running: usize,
    pub workers: usize,
    pub by_kind: [KindStats; WORK_KIND_COUNT],
}

struct Queued {
    kind: WorkKind,
    job: Job,
}

#[derive(Default)]
struct QueueState {
    jobs: VecDeque<Queued>,
    closed: bool,
    /// Workers waiting for a job; each queued job is already promised to one.
    idle: usize,
    /// Workers started so far, which numbers the next one's thread name.
    started: usize,
}

#[derive(Default)]
struct Counters {
    submitted: AtomicUsize,
    running: AtomicUsize,
    rejected: AtomicUsize,
    completed: AtomicUsize,
    peak_running: AtomicUsize,
    kind_submitted: [AtomicUsize; WORK_KIND_COUNT],
    kind_completed: [AtomicUsize; WORK_KIND_COUNT],
}

struct Shared {
    queue: Mutex<QueueState>,
    ready: Condvar,
    queue_capacity: usize,
    counters: Counters,
    live_workers: AtomicUsize,
    worker_exited: Condvar,
}

struct WorkerExit(Arc<Shared>);

impl Drop for WorkerExit {
    fn drop(&mut self) {
        // Pair the exit predicate transition with the mutex used by waiters so
        // the final notification cannot land between their load and wait.
        let _queue = self.0.queue.lock().unwrap_or_else(|error| error.into_inner());
        self.0.live_workers.fetch_sub(1, Ordering::SeqCst);
        self.0.worker_exited.notify_all();
    }
}

struct Running {
    shared: Arc<Shared>,
    kind: WorkKind,
}

impl Running {
    fn begin(shared: Arc<Shared>, kind: WorkKind) -> Self {
        let running = shared.counters.running.fetch_add(1, Ordering::SeqCst) + 1;
        shared.counters.peak_running.fetch_max(running, Ordering::SeqCst);
        Self { shared, kind }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.shared.counters.running.fetch_sub(1, Ordering::SeqCst);
        self.shared.counters.completed.fetch_add(1, Ordering::SeqCst);
        self.shared.counters.kind_completed[self.kind as usize].fetch_add(1, Ordering::SeqCst);
    }
}

type Spawn = dyn Fn(String, Job) -> std::io::Result<std::thread::JoinHandle<()>> + Send + Sync;

/// At most `threads` workers consuming one total-capacity FIFO queue.
///
/// FIFO admission gives every accepted job a finite position without introducing
/// priority starvation. Submission never blocks. No thread starts before the first
/// submission, and another starts only when no idle worker is left for the new
/// job, so an idle process holds no pool threads it has not needed (PERF-02).
/// A failed thread start leaves the job to the workers already running, or is
/// reported as `Closed` when there are none. Closing drains accepted jobs and
/// rejects new work. Dropping the executor closes admission without blocking;
/// detached workers retain shared state until accepted jobs drain and they exit.
pub struct BoundedExecutor {
    shared: Arc<Shared>,
    max_workers: usize,
    thread_name: String,
    spawn: Box<Spawn>,
}

impl BoundedExecutor {
    pub fn new(threads: usize, queue_capacity: usize, thread_name: &str) -> Self {
        Self::with_spawner(threads, queue_capacity, thread_name, |name, run| {
            std::thread::Builder::new().name(name).spawn(run)
        })
    }

    fn with_spawner(
        threads: usize,
        queue_capacity: usize,
        thread_name: &str,
        spawn: impl Fn(String, Job) -> std::io::Result<std::thread::JoinHandle<()>> + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(QueueState::default()),
            ready: Condvar::new(),
            queue_capacity,
            counters: Counters::default(),
            live_workers: AtomicUsize::new(0),
            worker_exited: Condvar::new(),
        });
        Self {
            shared,
            max_workers: threads,
            thread_name: thread_name.to_owned(),
            spawn: Box::new(spawn),
        }
    }

    /// Starts one more worker. Runs under the queue lock, so the new worker cannot
    /// observe the queue before the job that needed it is pushed.
    fn start_worker(&self, queue: &mut QueueState) {
        let worker_shared = self.shared.clone();
        let spawned = (self.spawn)(
            format!("{}-{}", self.thread_name, queue.started),
            Box::new(move || worker_loop(worker_shared)),
        );
        // The handle is dropped: workers are detached and exit once closed and drained.
        if spawned.is_ok() {
            queue.started += 1;
            self.shared.live_workers.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn submit(&self, kind: WorkKind, job: Job) -> Result<(), SubmitError> {
        let mut queue = self.shared.queue.lock().unwrap_or_else(|error| error.into_inner());
        if queue.closed {
            self.shared.counters.rejected.fetch_add(1, Ordering::SeqCst);
            return Err(SubmitError::Closed);
        }
        if queue.jobs.len() >= self.shared.queue_capacity {
            self.shared.counters.rejected.fetch_add(1, Ordering::SeqCst);
            return Err(SubmitError::Busy);
        }
        // Each queued job already has an idle worker promised or waits for a busy
        // one; a worker starts only when none is left for this job.
        if queue.jobs.len() >= queue.idle && self.worker_count() < self.max_workers {
            self.start_worker(&mut queue);
        }
        if self.worker_count() == 0 {
            self.shared.counters.rejected.fetch_add(1, Ordering::SeqCst);
            return Err(SubmitError::Closed);
        }
        queue.jobs.push_back(Queued { kind, job });
        self.shared.counters.submitted.fetch_add(1, Ordering::SeqCst);
        self.shared.counters.kind_submitted[kind as usize].fetch_add(1, Ordering::SeqCst);
        drop(queue);
        self.shared.ready.notify_one();
        Ok(())
    }

    pub fn close(&self) {
        let mut queue = self.shared.queue.lock().unwrap_or_else(|error| error.into_inner());
        queue.closed = true;
        drop(queue);
        self.shared.ready.notify_all();
    }

    pub fn worker_count(&self) -> usize {
        self.shared.live_workers.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> ExecutorStats {
        let queued = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .jobs
            .len();
        let mut by_kind = [KindStats::default(); WORK_KIND_COUNT];
        for (index, stats) in by_kind.iter_mut().enumerate() {
            stats.submitted = self.shared.counters.kind_submitted[index].load(Ordering::SeqCst);
            stats.completed = self.shared.counters.kind_completed[index].load(Ordering::SeqCst);
        }
        ExecutorStats {
            submitted: self.shared.counters.submitted.load(Ordering::SeqCst),
            running: self.shared.counters.running.load(Ordering::SeqCst),
            queued,
            rejected: self.shared.counters.rejected.load(Ordering::SeqCst),
            completed: self.shared.counters.completed.load(Ordering::SeqCst),
            peak_running: self.shared.counters.peak_running.load(Ordering::SeqCst),
            workers: self.worker_count(),
            by_kind,
        }
    }
}

impl Drop for BoundedExecutor {
    fn drop(&mut self) {
        self.close();
    }
}

fn worker_loop(shared: Arc<Shared>) {
    let _exit = WorkerExit(shared.clone());
    loop {
        let queued = {
            let mut queue = shared.queue.lock().unwrap_or_else(|error| error.into_inner());
            while queue.jobs.is_empty() && !queue.closed {
                queue.idle += 1;
                queue = shared.ready.wait(queue).unwrap_or_else(|error| error.into_inner());
                queue.idle -= 1;
            }
            if queue.jobs.is_empty() && queue.closed {
                return;
            }
            queue.jobs.pop_front().expect("non-empty executor queue")
        };
        let _running = Running::begin(shared.clone(), queued.kind);
        // Unwind builds (tests) only; release panics abort via the fatal panic hook.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(queued.job));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn idle_worker_takes_next_fifo_job() {
        let executor = BoundedExecutor::new(2, 4, "shared-executor-test");
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (started_tx, started_rx) = mpsc::sync_channel(2);
        executor
            .submit(
                WorkKind::Bulk,
                Box::new(move || {
                    started_tx.send("long").unwrap();
                    release_rx.recv().unwrap();
                }),
            )
            .unwrap();
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(5)).unwrap(), "long");
        let (second_tx, second_rx) = mpsc::sync_channel(1);
        executor
            .submit(WorkKind::General, Box::new(move || second_tx.send(()).unwrap()))
            .unwrap();
        second_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (short_tx, short_rx) = mpsc::sync_channel(1);
        executor
            .submit(WorkKind::Interactive, Box::new(move || short_tx.send(()).unwrap()))
            .unwrap();
        short_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(executor.stats().peak_running, 2);
    }

    #[test]
    fn total_queue_cap_and_closed_pool_are_typed() {
        let executor = BoundedExecutor::new(1, 1, "bounded-executor-test");
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        executor
            .submit(
                WorkKind::Bulk,
                Box::new(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }),
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        executor.submit(WorkKind::General, Box::new(|| {})).unwrap();
        assert_eq!(
            executor.submit(WorkKind::General, Box::new(|| {})),
            Err(SubmitError::Busy)
        );
        release_tx.send(()).unwrap();
        executor.close();
        assert_eq!(
            executor.submit(WorkKind::General, Box::new(|| {})),
            Err(SubmitError::Closed)
        );
        assert_eq!(executor.stats().rejected, 2);
    }

    #[test]
    fn failed_worker_spawn_exposes_reduced_capacity() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counted = attempts.clone();
        let executor = BoundedExecutor::with_spawner(2, 4, "spawn-failure-test", move |name, run| {
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                std::thread::Builder::new().name(name).spawn(run)
            } else {
                Err(std::io::Error::other("controlled worker spawn failure"))
            }
        });
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        executor
            .submit(
                WorkKind::Bulk,
                Box::new(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }),
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // The second worker cannot start: the job waits for the one that did.
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        executor
            .submit(WorkKind::General, Box::new(move || done_tx.send(()).unwrap()))
            .unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(executor.worker_count(), 1);
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn pool_without_any_started_worker_rejects_as_closed() {
        let executor = BoundedExecutor::with_spawner(2, 4, "spawn-refused-test", |_name, _run| {
            Err(std::io::Error::other("controlled worker spawn failure"))
        });
        assert_eq!(
            executor.submit(WorkKind::General, Box::new(|| {})),
            Err(SubmitError::Closed)
        );
        assert_eq!(executor.stats().rejected, 1);
        assert_eq!(executor.stats().queued, 0, "a refused job is never queued");
    }

    /// Spins until `done`; a hang guard only, no assertion depends on timing.
    fn wait_until(mut done: impl FnMut() -> bool) {
        let watchdog = std::time::Instant::now() + Duration::from_secs(60);
        while !done() {
            assert!(std::time::Instant::now() < watchdog, "watchdog expired");
            std::thread::yield_now();
        }
    }

    /// PERF-02: an executor that was never used holds no threads, and a worker
    /// starts only when every started one is busy, up to the configured bound.
    #[test]
    fn workers_start_on_first_use_and_only_when_none_is_idle() {
        let spawned = Arc::new(AtomicUsize::new(0));
        let counted = spawned.clone();
        let executor = BoundedExecutor::with_spawner(2, 8, "lazy-executor-test", move |name, run| {
            counted.fetch_add(1, Ordering::SeqCst);
            std::thread::Builder::new().name(name).spawn(run)
        });
        assert_eq!(spawned.load(Ordering::SeqCst), 0, "no thread before first use");
        assert_eq!(executor.worker_count(), 0);
        assert_eq!(executor.stats().workers, 0);

        // Sequential work reuses the one idle worker.
        for _ in 0..3 {
            let (done_tx, done_rx) = mpsc::sync_channel(1);
            executor
                .submit(WorkKind::General, Box::new(move || done_tx.send(()).unwrap()))
                .unwrap();
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            wait_until(|| executor.shared.queue.lock().unwrap().idle == 1);
            assert_eq!(spawned.load(Ordering::SeqCst), 1);
        }

        // Concurrent work starts a second worker, never more than the bound.
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (started_tx, started_rx) = mpsc::sync_channel(3);
        for _ in 0..3 {
            let (started_tx, release_rx) = (started_tx.clone(), release_rx.clone());
            executor
                .submit(
                    WorkKind::Bulk,
                    Box::new(move || {
                        started_tx.send(()).unwrap();
                        let _ = release_rx.lock().unwrap().recv();
                    }),
                )
                .unwrap();
        }
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(spawned.load(Ordering::SeqCst), 2);
        assert_eq!(executor.worker_count(), 2);
        drop(release_tx);
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(executor.stats().peak_running, 2);
        assert_eq!(spawned.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn drop_closes_without_waiting_for_gated_work_then_worker_exits() {
        let executor = BoundedExecutor::new(1, 2, "nonblocking-drop-test");
        let shared = executor.shared.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        executor
            .submit(
                WorkKind::Bulk,
                Box::new(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }),
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let (dropped_tx, dropped_rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            drop(executor);
            dropped_tx.send(()).unwrap();
        });
        dropped_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("executor drop blocked behind gated work");
        assert_eq!(shared.live_workers.load(Ordering::SeqCst), 1);

        release_tx.send(()).unwrap();
        let mut queue = shared.queue.lock().unwrap();
        while shared.live_workers.load(Ordering::SeqCst) != 0 {
            let (next, timeout) = shared
                .worker_exited
                .wait_timeout(queue, Duration::from_secs(5))
                .unwrap();
            queue = next;
            assert!(!timeout.timed_out(), "detached worker did not exit after draining");
        }
    }

    #[test]
    fn fifo_order_prevents_bulk_starvation() {
        let executor = BoundedExecutor::new(1, 8, "fifo-executor-test");
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        executor
            .submit(
                WorkKind::Interactive,
                Box::new(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }),
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (order_tx, order_rx) = mpsc::sync_channel(3);
        for (kind, value) in [
            (WorkKind::Bulk, 1),
            (WorkKind::Interactive, 2),
            (WorkKind::Interactive, 3),
        ] {
            let order_tx = order_tx.clone();
            executor
                .submit(kind, Box::new(move || order_tx.send(value).unwrap()))
                .unwrap();
        }
        release_tx.send(()).unwrap();
        assert_eq!(order_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        assert_eq!(order_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
        assert_eq!(order_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 3);
        let stats = executor.stats();
        assert_eq!(stats.submitted, 4);
        assert_eq!(stats.by_kind[WorkKind::Bulk as usize].submitted, 1);
        assert_eq!(stats.by_kind[WorkKind::Interactive as usize].submitted, 3);
    }
}
