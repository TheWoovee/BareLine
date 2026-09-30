// SPDX-License-Identifier: MPL-2.0
//! Panic-time sealing of recovery work. Release builds use `panic = "abort"`, so
//! nothing unwinds and no `catch_unwind` runs after a panic. The application's
//! panic hook calls [`seal`] instead: checkpoint and journal work that is already
//! queued or running on other threads gets a bounded chance to become durable
//! before the process aborts.
use std::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

static OUTSTANDING: AtomicUsize = AtomicUsize::new(0);
static SEALING: AtomicBool = AtomicBool::new(false);
thread_local! {
    /// Tracked work executing on this thread. A panicking thread cannot finish it.
    static RUNNING: Cell<usize> = const { Cell::new(0) };
}

/// One unit of queued or running recovery work. It is released when the work
/// finishes, or when a job that never ran is dropped (for example a full queue).
struct Outstanding;
impl Outstanding {
    fn new() -> Self {
        OUTSTANDING.fetch_add(1, Ordering::SeqCst);
        Self
    }
}
impl Drop for Outstanding {
    fn drop(&mut self) {
        OUTSTANDING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Tracked recovery work running on the current thread.
pub(crate) struct Active {
    _outstanding: Outstanding,
}
impl Active {
    fn enter(outstanding: Outstanding) -> Self {
        let _ = RUNNING.try_with(|running| running.set(running.get() + 1));
        Self {
            _outstanding: outstanding,
        }
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        let _ = RUNNING.try_with(|running| running.set(running.get().saturating_sub(1)));
    }
}

/// Track synchronous recovery work, such as a journal append, for its scope.
pub(crate) fn active() -> Active {
    Active::enter(Outstanding::new())
}

/// Wrap a queued recovery job so [`seal`] waits for it from the moment it is queued.
pub(crate) fn tracked(job: impl FnOnce() + Send + 'static) -> Box<dyn FnOnce() + Send> {
    let outstanding = Outstanding::new();
    Box::new(move || {
        let _active = Active::enter(outstanding);
        job();
    })
}

/// True once a panic hook has started sealing.
pub fn sealing() -> bool {
    SEALING.load(Ordering::SeqCst)
}

/// Signal sealing, then wait up to `budget` for queued and running recovery work
/// on other threads to finish. Work on the calling thread cannot finish and is
/// not awaited. Returns whether everything finished. Intended for the panic hook.
pub fn seal(budget: Duration) -> bool {
    SEALING.store(true, Ordering::SeqCst);
    let own = RUNNING.try_with(Cell::get).unwrap_or(0);
    let deadline = Instant::now().checked_add(budget);
    loop {
        if OUTSTANDING.load(Ordering::SeqCst) <= own {
            return true;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
