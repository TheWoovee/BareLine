// SPDX-License-Identifier: MPL-2.0
//! Global logical navigation over a captured paged root. All Pending page
//! resolution occurs on one bounded worker; the UI only submits and polls.
use crate::paged_view::PagedReadHandle;
use bareline_document::{Budget, TextOffset, line_lookup::{LineLookupPoll, LineTarget}, paged::{PagedSnapshot, SparseLineIndex}};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, TryRecvError}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationTarget { Line(u64), Byte(TextOffset) }
/// One speculative window. Superseding a seek cancels this work; it never installs
/// a viewport or changes selection, and all retained pages use the document budget.
#[derive(Default)]
pub struct ViewportPrefetch {
    active: Option<(Arc<AtomicBool>, Receiver<()>)>,
    last: Option<((u64, u64), usize)>,
}
impl ViewportPrefetch {
    pub fn cancel(&mut self) {
        if let Some((cancel, _)) = &self.active { cancel.store(true, Ordering::Relaxed); }
        self.last = None;
    }
    pub fn poll(&mut self) {
        if self.active.as_ref().is_some_and(|(_, receiver)| !matches!(receiver.try_recv(), Err(TryRecvError::Empty))) { self.active = None; }
    }
    pub fn request(&mut self, handle: PagedReadHandle, offset: TextOffset, budget: Budget, notify: Arc<dyn Fn() + Send + Sync>) {
        self.poll();
        let key = (handle.snapshot().identity_token(), offset.0);
        if self.active.is_some() || self.last == Some(key) || offset.0 >= handle.snapshot().len() { return; }
        let cancel = Arc::new(AtomicBool::new(false)); let worker_cancel = cancel.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        if std::thread::Builder::new().name("bareline-viewport-prefetch".into()).spawn(move || {
            if let Ok(mut request) = handle.snapshot().begin_viewport(offset, 64 * 1024, &budget) {
                while !worker_cancel.load(Ordering::Relaxed) {
                    match request.poll() {
                        bareline_document::paged::WindowPoll::Pending(ticket) => match handle.resolve_captured_page(ticket) { Ok(true) => {}, Ok(false) => std::thread::yield_now(), Err(_) => break },
                        _ => break,
                    }
                }
            }
            let _ = sender.try_send(()); notify();
        }).is_ok() { self.active = Some((cancel, receiver)); self.last = Some(key); }
    }
}
impl Drop for ViewportPrefetch { fn drop(&mut self) { self.cancel(); } }
/// Snap a shaped hit against the complete captured UTF-8 sequence, including a
/// grapheme whose combining context extends beyond the displayed window.
pub(crate) fn snap_grapheme(handle: &PagedReadHandle, offset: usize, budget: &Budget, cancellation: &bareline_file_io::cancellation::Cancellation) -> Result<usize, String> {
    use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};
    let _context_budget = budget.reserve(8192).map_err(|e| format!("Grapheme context: {e:?}"))?;
    fn chunk(handle: &PagedReadHandle, at: usize, backwards: bool, budget: &Budget, cancellation: &bareline_file_io::cancellation::Cancellation) -> Result<(usize, String), String> {
        let start = if backwards { at.saturating_sub(4096) } else { at };
        let size = if backwards { at - start } else { 4096 };
        let mut request = handle.snapshot().begin_viewport(TextOffset(start), size, budget).map_err(|e| format!("Grapheme window: {e:?}"))?;
        loop {
            cancellation.check().map_err(|e| format!("Grapheme hit: {e:?}"))?;
            match request.poll() {
                bareline_document::paged::WindowPoll::Ready(window) => return Ok((window.range().start.0, window.text().to_owned())),
                bareline_document::paged::WindowPoll::Pending(ticket) => { if !handle.resolve_captured_page(ticket)? { std::thread::yield_now(); } },
                _ => return Err("Grapheme source unavailable".into()),
            }
        }
    }
    let mut cursor = GraphemeCursor::new(offset, handle.snapshot().len(), true);
    let (mut start, mut text) = chunk(handle, offset, false, budget, cancellation)?;
    let mut previous = false;
    loop {
        cancellation.check().map_err(|e| format!("Grapheme hit: {e:?}"))?;
        let result = if previous { cursor.prev_boundary(&text, start) } else { cursor.is_boundary(&text, start).map(|boundary| boundary.then_some(offset)) };
        match result {
            Ok(Some(boundary)) => return Ok(boundary),
            Ok(None) if !previous => previous = true,
            Ok(None) => return Ok(0),
            Err(GraphemeIncomplete::PreContext(end)) => { let (from, context) = chunk(handle, end, true, budget, cancellation)?; cursor.provide_context(&context, from); },
            Err(GraphemeIncomplete::PrevChunk) => { (start, text) = chunk(handle, start, true, budget, cancellation)?; },
            Err(GraphemeIncomplete::NextChunk) => { (start, text) = chunk(handle, start + text.len(), false, budget, cancellation)?; },
            Err(GraphemeIncomplete::InvalidOffset) => return Err("Invalid shaped hit boundary".into()),
        }
    }
}
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
pub struct GlobalNavigation { active: Option<Active>, queued: Option<Job>, index: Arc<Mutex<Option<(PagedSnapshot, SparseLineIndex)>>> }
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
        let retained = self.index.clone();
        let (tx, result) = mpsc::sync_channel(1);
        std::thread::Builder::new().name("bareline-global-line".into()).spawn(move || {
            let outcome = (|| {
                let mut retained = retained.lock().map_err(|_| "Global line index lock failed".to_owned())?;
                if !retained.as_ref().is_some_and(|(snapshot, _)| snapshot.same_document(job.handle.snapshot()) && snapshot.content_state == job.handle.snapshot().content_state) {
                    let index = SparseLineIndex::new(job.handle.snapshot().clone(), 256, 64 * 1024, &job.budget).map_err(|e| format!("Global line index: {e:?}"))?;
                    *retained = Some((job.handle.snapshot().clone(), index));
                }
                resolve(&job.handle, &mut retained.as_mut().unwrap().1, job.target, job.budget, &worker_cancel)
            })();
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

fn lookup(handle: &PagedReadHandle, index: &mut SparseLineIndex, target: LineTarget, budget: Budget, cancelled: &AtomicBool) -> Result<LineLookupPoll, String> {
    let mut lookup = index.lookup(target, budget).map_err(|e| format!("Global line lookup: {e:?}"))?;
    loop {
        if cancelled.load(Ordering::Relaxed) { lookup.cancel(); return Err("Global navigation cancelled".into()); }
        let result = lookup.poll();
        index.retain_lookup_progress(&lookup).map_err(|e| format!("Global line checkpoint: {e:?}"))?;
        match result {
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
fn resolve(handle: &PagedReadHandle, index: &mut SparseLineIndex, target: NavigationTarget, budget: Budget, cancelled: &AtomicBool) -> Result<NavigationResult, String> {
    let snapshot = handle.snapshot().clone();
    let line = match target {
        NavigationTarget::Line(line) => usize::try_from(line).map_err(|_| "Logical line exceeds this platform's range")?,
        NavigationTarget::Byte(offset) => match lookup(handle, index, LineTarget::Byte(offset), budget.clone(), cancelled)? {
            LineLookupPoll::Line(line) => line,
            _ => return Err("Global byte lookup returned no line".into()),
        },
    };
    let LineLookupPoll::Range(range) = lookup(handle, index, LineTarget::Line(line), budget, cancelled)? else {
        return Err("Global line lookup returned no range".into());
    };
    let offset = match target { NavigationTarget::Line(_) => range.start, NavigationTarget::Byte(offset) => offset };
    Ok(NavigationResult { snapshot, offset, line_start: range.start, first_global_line: line as u64 })
}
