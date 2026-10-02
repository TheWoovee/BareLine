// SPDX-License-Identifier: MPL-2.0
//! The surface's one shared bounded pool for scans too long for the UI thread:
//! exact column counts and grapheme boundaries past their synchronous budget.
//! Its worker starts on first use and is reused; a full queue refuses work
//! instead of spawning a thread or blocking input (EDT-20).
use bareline_platform::executor::{BoundedExecutor, Job, SubmitError, WorkKind};
use std::sync::OnceLock;

/// These jobs are cancellable CPU scans over resident text, and only
/// pathological lines or clusters reach them, so one worker is enough.
const WORKERS: usize = 1;
const QUEUE_DEPTH: usize = 8;
const THREAD_NAME: &str = "bareline-surface";

fn pool() -> &'static BoundedExecutor {
    static POOL: OnceLock<BoundedExecutor> = OnceLock::new();
    POOL.get_or_init(|| BoundedExecutor::new(WORKERS, QUEUE_DEPTH, THREAD_NAME))
}

pub(crate) fn submit(job: Job) -> Result<(), SubmitError> {
    #[cfg(test)]
    SUBMITTED.with(|submitted| submitted.set(submitted.get() + 1));
    pool().submit(WorkKind::Interactive, job)
}

#[cfg(test)]
thread_local! {
    /// Jobs this test thread handed to the pool. Per thread, so parallel tests
    /// sharing the process-wide pool cannot disturb a count.
    static SUBMITTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Jobs the calling thread has submitted so far.
#[cfg(test)]
pub(crate) fn submitted_here() -> usize {
    SUBMITTED.with(std::cell::Cell::get)
}

/// Whether the calling thread is one of the shared pool's workers (named
/// `bareline-surface-N`), not a thread spawned for one job.
#[cfg(test)]
pub(crate) fn on_pool_worker() -> bool {
    std::thread::current()
        .name()
        .is_some_and(|name| name.starts_with(THREAD_NAME))
}
