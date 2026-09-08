// SPDX-License-Identifier: MPL-2.0
//! Global logical navigation over a captured paged root. All Pending page
//! resolution occurs on one bounded worker; the UI only submits and polls.
use crate::paged_view::PagedReadHandle;
use bareline_document::{Budget, TextOffset, line_lookup::{LineLookupPoll, LineTarget}, paged::{PagedSnapshot, SparseLineIndex}};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, TryRecvError}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationTarget { Line(u64), Byte(TextOffset) }
pub struct NavigationResult {
    /// The caller must compare document and content state before applying.
    pub snapshot: PagedSnapshot,
    pub offset: TextOffset,
    /// True beginning of the line, even when offset is a mid-line viewport prefix.
    pub line_start: TextOffset,
    pub first_global_line: u64,
}
struct Job {
    handle: PagedReadHandle,
    target: NavigationTarget,
    budget: Budget,
    notify: Arc<dyn Fn() + Send + Sync>,
}
struct Active {
    snapshot: PagedSnapshot,
    target: NavigationTarget,
    cancel: Arc<AtomicBool>,
    result: Receiver<Result<NavigationResult, String>>,
}
#[derive(Default)]
pub struct GlobalNavigation { active: Option<Active>, queued: Option<Job> }
impl GlobalNavigation {
    pub fn new() -> Self { Self::default() }
    pub fn is_pending(&self) -> bool { self.active.is_some() || self.queued.is_some() }
    pub fn cancel(&mut self) {
        self.queued = None;
        if let Some(active) = &self.active { active.cancel.store(true, Ordering::Relaxed); }
    }
    pub fn request(&mut self, handle: PagedReadHandle, target: NavigationTarget, budget: Budget, notify: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        if matches!(target, NavigationTarget::Byte(offset) if offset.0 > handle.snapshot().len()) {
            return Err("Navigation offset is outside the captured document".into());
        }
        if let NavigationTarget::Line(line) = target { usize::try_from(line).map_err(|_| "Logical line exceeds this platform's range")?; }
        if let Some(active) = &self.active {
            if !active.cancel.load(Ordering::Relaxed) && active.target == target
                && active.snapshot.same_document(handle.snapshot()) && active.snapshot.content_state == handle.snapshot().content_state {
                return Ok(());
            }
            active.cancel.store(true, Ordering::Relaxed);
            self.queued = Some(Job { handle, target, budget, notify });
            return Ok(());
        }
        self.start(Job { handle, target, budget, notify })
    }
    fn start(&mut self, job: Job) -> Result<(), String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let snapshot = job.handle.snapshot().clone();
        let target = job.target;
        let (tx, result) = mpsc::sync_channel(1);
        std::thread::Builder::new().name("bareline-global-line".into()).spawn(move || {
            let outcome = resolve(&job.handle, job.target, job.budget, &worker_cancel);
            let _ = tx.send(outcome);
            (job.notify)();
        }).map_err(|e| format!("Cannot start global navigation: {e}"))?;
        self.active = Some(Active { snapshot, target, cancel, result });
        Ok(())
    }
    pub fn poll(&mut self) -> Option<Result<NavigationResult, String>> {
        let active = self.active.as_ref()?;
        let result = match active.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("Global navigation worker stopped".into()),
        };
        let cancelled = active.cancel.load(Ordering::Relaxed);
        self.active = None;
        if let Some(job) = self.queued.take() {
            return self.start(job).err().map(Err);
        }
        if cancelled { None } else { Some(result) }
    }
}
impl Drop for GlobalNavigation { fn drop(&mut self) { self.cancel(); } }

fn lookup(handle: &PagedReadHandle, index: &SparseLineIndex, target: LineTarget, budget: Budget, cancelled: &AtomicBool) -> Result<LineLookupPoll, String> {
    let mut lookup = index.lookup(target, budget).map_err(|e| format!("Global line lookup: {e:?}"))?;
    loop {
        if cancelled.load(Ordering::Relaxed) { lookup.cancel(); return Err("Global navigation cancelled".into()); }
        match lookup.poll() {
            result @ (LineLookupPoll::Line(_) | LineLookupPoll::Range(_)) => return Ok(result),
            LineLookupPoll::Pending(ticket) => {
                // The handle is immutable; historical view roots remain usable.
                // The applying view separately checks source identity/content state.
                if !handle.resolve_captured_page(ticket)? { std::thread::sleep(std::time::Duration::from_millis(1)); }
            }
            LineLookupPoll::Progress(_) => std::thread::yield_now(),
            LineLookupPoll::Unavailable(reason) => return Err(format!("Global navigation source unavailable: {reason:?}")),
            LineLookupPoll::Failed(error) => return Err(format!("Global navigation failed: {error:?}")),
            LineLookupPoll::Cancelled => return Err("Global navigation cancelled".into()),
            LineLookupPoll::Finished => return Err("Global navigation ended without a line".into()),
        }
    }
}
fn resolve(handle: &PagedReadHandle, target: NavigationTarget, budget: Budget, cancelled: &AtomicBool) -> Result<NavigationResult, String> {
    let snapshot = handle.snapshot().clone();
    let index = SparseLineIndex::new(snapshot.clone(), 16, 64 * 1024, &budget).map_err(|e| format!("Global line index: {e:?}"))?;
    let line = match target {
        NavigationTarget::Line(line) => usize::try_from(line).map_err(|_| "Logical line exceeds this platform's range")?,
        NavigationTarget::Byte(offset) => match lookup(handle, &index, LineTarget::Byte(offset), budget.clone(), cancelled)? {
            LineLookupPoll::Line(line) => line,
            _ => return Err("Global byte lookup returned no line".into()),
        },
    };
    let LineLookupPoll::Range(range) = lookup(handle, &index, LineTarget::Line(line), budget, cancelled)? else {
        return Err("Global line lookup returned no range".into());
    };
    let offset = match target { NavigationTarget::Line(_) => range.start, NavigationTarget::Byte(offset) => offset };
    Ok(NavigationResult { snapshot, offset, line_start: range.start, first_global_line: line as u64 })
}
