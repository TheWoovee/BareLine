// SPDX-License-Identifier: MPL-2.0
//! Global logical navigation over a captured paged root. All Pending page
//! resolution occurs on one bounded worker; the UI only submits and polls.
//! One persistent line index per document serves navigation, power commands
//! and fold projection, and follows edits instead of starting over.
use bareline_document::{
    Budget, ContentStateId, TextOffset,
    line_lookup::{LineLookupPoll, LineTarget},
    paged::{IndexError, LineCheckpoint, LineCount, PagedSnapshot, SparseLineIndex, WindowPoll},
};
use bareline_file_io::paged_service::PagedReadHandle;
use std::{
    sync::{
        Arc, Condvar, Mutex, RwLock, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationTarget {
    Line(u64),
    Byte(TextOffset),
}
/// One speculative window. Superseding a seek cancels this work; it never installs
/// a viewport or changes selection, and all retained pages use the document budget.
#[derive(Default)]
pub struct ViewportPrefetch {
    active: Option<(Arc<AtomicBool>, Receiver<()>)>,
    last: Option<((u64, u64), usize)>,
}
impl ViewportPrefetch {
    pub fn cancel(&mut self) {
        if let Some((cancel, _)) = &self.active {
            cancel.store(true, Ordering::Relaxed);
        }
        self.last = None;
    }
    pub fn poll(&mut self) {
        if self
            .active
            .as_ref()
            .is_some_and(|(_, receiver)| !matches!(receiver.try_recv(), Err(TryRecvError::Empty)))
        {
            self.active = None;
        }
    }
    pub fn request(
        &mut self,
        handle: PagedReadHandle,
        offset: TextOffset,
        budget: Budget,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.poll();
        let key = (handle.snapshot().identity_token(), offset.0);
        if self.active.is_some() || self.last == Some(key) || offset.0 >= handle.snapshot().len() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        if std::thread::Builder::new()
            .name("bareline-viewport-prefetch".into())
            .spawn(move || {
                if let Ok(mut request) = handle.snapshot().begin_viewport(offset, 64 * 1024, &budget) {
                    while !worker_cancel.load(Ordering::Relaxed) {
                        match request.poll() {
                            bareline_document::paged::WindowPoll::Pending(ticket) => {
                                match handle.resolve_captured_page(ticket) {
                                    Ok(true) => {}
                                    Ok(false) => std::thread::yield_now(),
                                    Err(_) => break,
                                }
                            }
                            _ => break,
                        }
                    }
                }
                let _ = sender.try_send(());
                notify();
            })
            .is_ok()
        {
            self.active = Some((cancel, receiver));
            self.last = Some(key);
        }
    }
}
impl Drop for ViewportPrefetch {
    fn drop(&mut self) {
        self.cancel();
    }
}
/// Snap a shaped hit against the complete captured UTF-8 sequence, including a
/// grapheme whose combining context extends beyond the displayed window.
pub(crate) fn snap_grapheme(
    handle: &PagedReadHandle,
    offset: usize,
    budget: &Budget,
    cancellation: &bareline_file_io::cancellation::Cancellation,
) -> Result<usize, String> {
    use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};
    let _context_budget = budget.claim(8192).map_err(|e| format!("Grapheme context: {e:?}"))?;
    fn chunk(
        handle: &PagedReadHandle,
        at: usize,
        backwards: bool,
        budget: &Budget,
        cancellation: &bareline_file_io::cancellation::Cancellation,
    ) -> Result<(usize, String), String> {
        let start = if backwards { at.saturating_sub(4096) } else { at };
        let size = if backwards { at - start } else { 4096 };
        let mut request = handle
            .snapshot()
            .begin_viewport(TextOffset(start), size, budget)
            .map_err(|e| format!("Grapheme window: {e:?}"))?;
        loop {
            cancellation.check().map_err(|e| format!("Grapheme hit: {e:?}"))?;
            match request.poll() {
                bareline_document::paged::WindowPoll::Ready(window) => {
                    return Ok((window.range().start.0, window.text().to_owned()));
                }
                bareline_document::paged::WindowPoll::Pending(ticket) => {
                    if !handle
                        .resolve_captured_page(ticket)
                        .map_err(|error| error.to_string())?
                    {
                        std::thread::yield_now();
                    }
                }
                _ => return Err("Grapheme source unavailable".into()),
            }
        }
    }
    let mut cursor = GraphemeCursor::new(offset, handle.snapshot().len(), true);
    let (mut start, mut text) = chunk(handle, offset, false, budget, cancellation)?;
    let mut previous = false;
    loop {
        cancellation.check().map_err(|e| format!("Grapheme hit: {e:?}"))?;
        let result = if previous {
            cursor.prev_boundary(&text, start)
        } else {
            cursor
                .is_boundary(&text, start)
                .map(|boundary| boundary.then_some(offset))
        };
        match result {
            Ok(Some(boundary)) => return Ok(boundary),
            Ok(None) if !previous => previous = true,
            Ok(None) => return Ok(0),
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (from, context) = chunk(handle, end, true, budget, cancellation)?;
                cursor.provide_context(&context, from);
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                (start, text) = chunk(handle, start, true, budget, cancellation)?;
            }
            Err(GraphemeIncomplete::NextChunk) => {
                (start, text) = chunk(handle, start + text.len(), false, budget, cancellation)?;
            }
            Err(GraphemeIncomplete::InvalidOffset) => return Err("Invalid shaped hit boundary".into()),
        }
    }
}
/// First line start in `start..=limit`, which is `start` itself when the byte before
/// it ends a line. Scans at most 4 KiB; a longer line keeps the unsnapped `start`.
pub(crate) fn snap_line_start(
    handle: &PagedReadHandle,
    start: usize,
    limit: usize,
    budget: &Budget,
    cancellation: &bareline_file_io::cancellation::Cancellation,
) -> Result<usize, String> {
    let length = handle.snapshot().len();
    if start == 0 || start >= limit || start > length {
        return Ok(start);
    }
    // One byte of context before `start`, and one past `limit` to see a CRLF's LF.
    let from = start - 1;
    let end = limit.saturating_add(1).min(start.saturating_add(4096)).min(length);
    let mut request = handle
        .snapshot()
        .begin_viewport(TextOffset(from), end - from, budget)
        .map_err(|e| format!("Line start window: {e:?}"))?;
    let window = loop {
        cancellation.check().map_err(|e| format!("Line start: {e:?}"))?;
        match request.poll() {
            bareline_document::paged::WindowPoll::Ready(window) => break window,
            bareline_document::paged::WindowPoll::Pending(ticket) => {
                if !handle
                    .resolve_captured_page(ticket)
                    .map_err(|error| error.to_string())?
                {
                    std::thread::yield_now();
                }
            }
            _ => return Err("Line start source unavailable".into()),
        }
    };
    let base = window.range().start.0;
    let bytes = window.text().as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        let next = base + index + 1;
        if next > limit {
            break;
        }
        let ends_line = *byte == b'\n'
            || (*byte == b'\r'
                && match bytes.get(index + 1) {
                    Some(following) => *following != b'\n',
                    None => next == length,
                });
        if ends_line && next >= start {
            return Ok(next);
        }
    }
    Ok(start)
}
pub struct NavigationResult {
    /// The caller must compare document and content state before applying.
    pub snapshot: PagedSnapshot,
    pub offset: TextOffset,
    /// True beginning of the line, even when offset is a mid-line viewport prefix.
    pub line_start: TextOffset,
    /// Start of the next line (or the end of the source), which bounds this
    /// line's width even when it runs past the viewport window.
    pub line_end: TextOffset,
    pub first_global_line: u64,
}
/// Checkpoints one document's line index keeps. They stay evenly spaced, so a
/// 300 MB text keeps one about every 512 KiB and no lookup scans more (PED-08).
const INDEX_CHECKPOINTS: usize = 1024;
const INDEX_WINDOW: usize = 64 * 1024;
/// Bytes one background count step reads before it reports progress.
const COUNT_SLICE: usize = 8 << 20;
/// How long an idle owner thread stays parked for the next request. Edits
/// arrive far more often, so typing never starts a thread per recount.
const OWNER_IDLE: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct IndexReceipt {
    snapshot: PagedSnapshot,
    scanned: usize,
    lines: Option<usize>,
}
impl IndexReceipt {
    fn of(index: &SparseLineIndex) -> Self {
        Self {
            snapshot: index.snapshot().clone(),
            scanned: index.scanned_to().0,
            lines: match index.line_count() {
                LineCount::Known(count) => Some(count),
                LineCount::Unknown => None,
            },
        }
    }
}
enum CountStep {
    Complete,
    Slice,
    Interrupted,
}

/// One persistent sparse line index per paged document, shared by global
/// navigation, power commands and fold projection (PED-06, PED-07). It follows
/// each published change instead of restarting at byte zero. Workers hold its
/// lock only between windows, never across a page read, so the UI thread can
/// follow an edit without waiting for a scan.
#[derive(Clone, Default)]
pub struct SharedLineIndex(Arc<SharedIndex>);
#[derive(Default)]
struct SharedIndex {
    index: Mutex<Option<SparseLineIndex>>,
    receipt: RwLock<Option<IndexReceipt>>,
    /// Text bytes lookups and counts have inspected.
    scanned: AtomicUsize,
    /// Times the index started over at byte zero.
    rebuilds: AtomicUsize,
    /// The text being counted and the navigation owner counting it, so peer
    /// views of one document run a single background count (PERF-04).
    counter: Mutex<Option<(CountKey, Weak<Owner>)>>,
}
type CountKey = ((u64, u64), ContentStateId);
impl SharedLineIndex {
    /// Moves the index to `snapshot`, the text a view just installed, keeping
    /// every checkpoint the change did not touch.
    pub fn follow(&self, snapshot: &PagedSnapshot) {
        if let Ok(mut slot) = self.0.index.lock()
            && let Some(index) = slot.as_mut()
        {
            self.advance(index, snapshot);
        }
    }
    pub fn scanned_bytes(&self) -> usize {
        self.0.scanned.load(Ordering::Relaxed)
    }
    pub fn rebuilds(&self) -> usize {
        self.0.rebuilds.load(Ordering::Relaxed)
    }
    /// Whether `index` describes `snapshot` afterwards. A snapshot older than the
    /// indexed text leaves the index alone; a skipped change starts it over.
    fn advance(&self, index: &mut SparseLineIndex, snapshot: &PagedSnapshot) -> bool {
        let current = index.snapshot();
        if current.same_document(snapshot) {
            if snapshot.revision.0 < current.revision.0 {
                return current.content_state == snapshot.content_state;
            }
            if index.invalidate_after_change(snapshot.clone()) == Ok(true) {
                return true;
            }
            // A direct successor whose edits are out of order (an undo or redo
            // can publish those) still leaves the text before its first edit.
            let current = index.snapshot();
            let first = snapshot
                .applied_change()
                .filter(|change| change.matches_before(current.identity_token(), current.content_state))
                .and_then(|change| change.edits().iter().map(|edit| edit.before.start.0).min())
                .map(|first| first.min(current.len()).min(snapshot.len()));
            if let Some(first) = first
                && index.invalidate_after_edit(snapshot.clone(), TextOffset(first)).is_ok()
            {
                return true;
            }
        }
        index.reset(snapshot.clone());
        self.0.rebuilds.fetch_add(1, Ordering::Relaxed);
        true
    }
    /// Runs `f` with the shared index moved to `snapshot`, or with `None` when
    /// `snapshot` is older than the text the index already follows.
    fn with_index<R>(
        &self,
        snapshot: &PagedSnapshot,
        budget: &Budget,
        f: impl FnOnce(Option<&mut SparseLineIndex>) -> R,
    ) -> Result<R, String> {
        let mut slot = self.0.index.lock().map_err(|_| "Line index lock failed".to_owned())?;
        if slot.is_none() {
            *slot = Some(
                SparseLineIndex::new(snapshot.clone(), INDEX_CHECKPOINTS, INDEX_WINDOW, budget)
                    .map_err(|e| format!("Line index: {e:?}"))?,
            );
            self.0.rebuilds.fetch_add(1, Ordering::Relaxed);
        }
        let index = slot.as_mut().expect("line index");
        let current = self.advance(index, snapshot);
        Ok(f(current.then_some(index)))
    }
    fn publish(&self, receipt: IndexReceipt) {
        if let Ok(mut slot) = self.0.receipt.write() {
            *slot = Some(receipt);
        }
    }
    /// Creates or advances the index for `snapshot` and publishes its progress.
    fn prepare(&self, snapshot: &PagedSnapshot, budget: &Budget) -> Result<(), String> {
        if let Some(receipt) = self.with_index(snapshot, budget, |index| index.map(|index| IndexReceipt::of(index)))? {
            self.publish(receipt);
        }
        Ok(())
    }
    /// Resolves one lookup on the calling worker, resolving Pending pages through
    /// `handle`, and retains its progress in the shared index. `hint` carries the
    /// previous lookup's verified checkpoint in the same text, so ordered lookups
    /// read each byte once; `check` cancels. Returns the terminal poll.
    pub fn lookup(
        &self,
        handle: &PagedReadHandle,
        target: LineTarget,
        budget: &Budget,
        hint: &mut Option<LineCheckpoint>,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<LineLookupPoll, String> {
        let snapshot = handle.snapshot();
        // A capture older than the shared text scans with a private index (the
        // old cost) and never moves the shared one backwards.
        let mut private = None;
        let mut request = self
            .with_index(snapshot, budget, |index| match index {
                Some(index) => index.lookup_from(snapshot, target, budget.clone(), *hint),
                None => SparseLineIndex::new(snapshot.clone(), 16, INDEX_WINDOW, budget).and_then(|index| {
                    let request = index.lookup_from(snapshot, target, budget.clone(), *hint);
                    private = Some(index);
                    request
                }),
            })?
            .map_err(|e| format!("Line index: {e:?}"))?;
        let mut seen = 0;
        loop {
            if let Err(error) = check() {
                request.cancel();
                return Err(error);
            }
            let result = request.poll();
            self.0
                .scanned
                .fetch_add(request.scanned_bytes() - seen, Ordering::Relaxed);
            seen = request.scanned_bytes();
            if let Some(index) = private.as_mut() {
                let _ = index.retain_lookup_progress(&request);
            } else {
                let mut restart = None;
                let receipt = {
                    let mut slot = self.0.index.lock().map_err(|_| "Line index lock failed".to_owned())?;
                    match slot.as_mut() {
                        Some(index) if request.matches_snapshot(index.snapshot()) => {
                            index
                                .retain_lookup_progress(&request)
                                .map_err(|e| format!("Line checkpoint: {e:?}"))?;
                            // A scan that reached the checkpoints an edit moved
                            // continues from the one nearest the target.
                            if matches!(result, LineLookupPoll::Progress(_))
                                && let (Ok(start), Some(verified)) =
                                    (index.start_for(target), request.verified_checkpoint())
                                && start.offset.0 > verified.offset.0.saturating_add(INDEX_WINDOW)
                            {
                                restart = Some(
                                    index
                                        .lookup_from(snapshot, target, budget.clone(), None)
                                        .map_err(|e| format!("Line index: {e:?}"))?,
                                );
                            }
                            Some(IndexReceipt::of(index))
                        }
                        _ => None,
                    }
                };
                if let Some(next) = restart {
                    request = next;
                    seen = 0;
                }
                if let Some(receipt) = receipt {
                    self.publish(receipt);
                }
            }
            match result {
                LineLookupPoll::Pending(ticket) => {
                    if !handle
                        .resolve_captured_page(ticket)
                        .map_err(|error| error.to_string())?
                    {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
                LineLookupPoll::Progress(_) => {}
                result => {
                    *hint = request.verified_checkpoint();
                    return Ok(result);
                }
            }
        }
    }
    /// Extends the index toward the end of `handle`'s text, reading at most about
    /// `slice` bytes. `interrupted` is polled between windows.
    fn count(
        &self,
        handle: &PagedReadHandle,
        budget: &Budget,
        slice: usize,
        interrupted: &dyn Fn() -> bool,
    ) -> Result<CountStep, String> {
        let snapshot = handle.snapshot();
        let mut observed = 0;
        loop {
            if interrupted() {
                return Ok(CountStep::Interrupted);
            }
            let next = self
                .with_index(snapshot, budget, |index| match index {
                    Some(index) => index.next_window(snapshot, budget),
                    // A newer text replaced this one; its own count follows it.
                    None => Ok(None),
                })?
                .map_err(|e| format!("Line count: {e:?}"))?;
            let Some(mut request) = next else {
                self.prepare(snapshot, budget)?;
                return Ok(CountStep::Complete);
            };
            let window = loop {
                if interrupted() {
                    return Ok(CountStep::Interrupted);
                }
                match request.poll() {
                    WindowPoll::Ready(window) => break window,
                    WindowPoll::Pending(ticket) => {
                        if !handle
                            .resolve_captured_page(ticket)
                            .map_err(|error| error.to_string())?
                        {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                    WindowPoll::Unavailable(_) => return Err("Line count source unavailable".into()),
                    WindowPoll::InvalidUtf8 | WindowPoll::Finished => {
                        return Err("Line count window unavailable".into());
                    }
                }
            };
            if window.text().is_empty() {
                return Err("Line count made no progress".into());
            }
            observed += window.text().len();
            self.0.scanned.fetch_add(window.text().len(), Ordering::Relaxed);
            let receipt = {
                let mut slot = self.0.index.lock().map_err(|_| "Line index lock failed".to_owned())?;
                match slot.as_mut() {
                    Some(index) => match index.observe(&window) {
                        // A lookup or an edit moved the index meanwhile; the next
                        // window starts from its new state.
                        Ok(()) | Err(IndexError::OutOfOrder | IndexError::StaleSnapshot) => {
                            Some(IndexReceipt::of(index))
                        }
                        Err(error) => return Err(format!("Line count: {error:?}")),
                    },
                    None => None,
                }
            };
            if let Some(receipt) = receipt {
                self.publish(receipt);
            }
            if observed >= slice {
                return Ok(CountStep::Slice);
            }
        }
    }
}

#[cfg(test)]
struct ScanHook {
    entered: std::sync::mpsc::SyncSender<()>,
    release: Receiver<()>,
}
struct Job {
    handle: PagedReadHandle,
    target: NavigationTarget,
    budget: Budget,
    notify: Arc<dyn Fn() + Send + Sync>,
    cancel: Arc<AtomicBool>,
    result: SyncSender<Result<NavigationResult, String>>,
    #[cfg(test)]
    hook: Option<ScanHook>,
}
#[derive(Clone)]
struct CountJob {
    handle: PagedReadHandle,
    budget: Budget,
    notify: Arc<dyn Fn() + Send + Sync>,
}
#[derive(Default)]
struct Queue {
    running: bool,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The newest navigation request; a newer one replaces it unstarted.
    job: Option<Job>,
    count: Option<CountJob>,
}
/// The view's one navigation worker (PED-08). It serves superseding requests and
/// the background line count in turn, instead of a new thread per request, and
/// parks on `wake` between them. It exits when the view drops, or after
/// `OWNER_IDLE` without work.
#[derive(Default)]
struct Owner {
    queue: Mutex<Queue>,
    /// Signalled with `queue` held when work arrives or the view drops.
    wake: Condvar,
    stop: AtomicBool,
    spawned: AtomicUsize,
}
struct Active {
    snapshot: PagedSnapshot,
    target: NavigationTarget,
    cancel: Arc<AtomicBool>,
    result: Receiver<Result<NavigationResult, String>>,
}

#[derive(Default)]
pub struct GlobalNavigation {
    active: Option<Active>,
    owner: Arc<Owner>,
    line_index: SharedLineIndex,
    observed_receipt: std::cell::RefCell<Option<IndexReceipt>>,
    /// The text the latest background count was requested for.
    counted: Option<CountKey>,
    #[cfg(test)]
    count_paused: bool,
    #[cfg(test)]
    scan_hook: Option<ScanHook>,
}
impl GlobalNavigation {
    pub fn new() -> Self {
        Self::default()
    }
    /// A navigation over a peer view's document, sharing its line index.
    pub fn sharing(line_index: SharedLineIndex) -> Self {
        Self {
            active: None,
            owner: Arc::default(),
            line_index,
            observed_receipt: Default::default(),
            counted: None,
            #[cfg(test)]
            count_paused: false,
            #[cfg(test)]
            scan_hook: None,
        }
    }
    pub fn line_index(&self) -> &SharedLineIndex {
        &self.line_index
    }
    pub fn is_pending(&self) -> bool {
        self.active.is_some()
    }
    /// Fraction of the document the retained sparse line index has scanned, for
    /// the status strip's determinate progress while line numbers are resolving.
    pub fn index_progress(&self, expected: &PagedSnapshot) -> Option<f32> {
        let receipt = self.observed_index_receipt(expected)?;
        let total = receipt.snapshot.len();
        if total == 0 {
            return Some(1.0);
        }
        Some((receipt.scanned.min(total) as f32 / total as f32).clamp(0.0, 1.0))
    }
    pub fn indexed_line_count(&self, expected: &PagedSnapshot) -> Option<usize> {
        self.observed_index_receipt(expected)?.lines
    }
    fn observed_index_receipt(&self, expected: &PagedSnapshot) -> Option<IndexReceipt> {
        if let Ok(receipt) = self.line_index.0.receipt.try_read() {
            *self.observed_receipt.borrow_mut() = receipt.clone();
        }
        self.observed_receipt.borrow().clone().filter(|receipt| {
            receipt.snapshot.same_document(expected) && receipt.snapshot.content_state == expected.content_state
        })
    }
    #[cfg(test)]
    pub(crate) fn hold_next_scan(&mut self, entered: std::sync::mpsc::SyncSender<()>, release: Receiver<()>) {
        self.scan_hook = Some(ScanHook { entered, release });
    }
    #[cfg(test)]
    pub(crate) fn hold_receipt_write(
        &self,
        entered: std::sync::mpsc::SyncSender<()>,
        release: Receiver<()>,
    ) -> std::thread::JoinHandle<()> {
        let index = self.line_index.clone();
        std::thread::spawn(move || {
            let _receipt = index.0.receipt.write().unwrap_or_else(|error| error.into_inner());
            let _ = entered.send(());
            let _ = release.recv_timeout(std::time::Duration::from_secs(5));
        })
    }
    /// Keeps line counts estimated until a navigation reaches the end.
    #[cfg(test)]
    pub(crate) fn pause_line_count(&mut self) {
        self.count_paused = true;
    }
    #[cfg(test)]
    pub(crate) fn spawned_threads(&self) -> usize {
        self.owner.spawned.load(Ordering::Relaxed)
    }
    pub fn cancel(&mut self) {
        if let Ok(mut queue) = self.owner.queue.lock() {
            queue.job = None;
        }
        if let Some(active) = &self.active {
            active.cancel.store(true, Ordering::Relaxed);
        }
    }
    pub fn request(
        &mut self,
        handle: PagedReadHandle,
        target: NavigationTarget,
        budget: Budget,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        if matches!(target, NavigationTarget::Byte(offset) if offset.0 > handle.snapshot().len()) {
            return Err("Navigation offset is outside the captured document".into());
        }
        if let NavigationTarget::Line(line) = target {
            usize::try_from(line).map_err(|_| "Logical line exceeds this platform's range")?;
        }
        if let Some(active) = &self.active {
            if !active.cancel.load(Ordering::Relaxed)
                && active.target == target
                && active.snapshot.same_document(handle.snapshot())
                && active.snapshot.content_state == handle.snapshot().content_state
            {
                return Ok(());
            }
            active.cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, result) = mpsc::sync_channel(1);
        let snapshot = handle.snapshot().clone();
        let job = Job {
            handle,
            target,
            budget,
            notify,
            cancel: cancel.clone(),
            result: sender,
            #[cfg(test)]
            hook: self.scan_hook.take(),
        };
        self.submit(Some(job), None)?;
        self.active = Some(Active {
            snapshot,
            target,
            cancel,
            result,
        });
        Ok(())
    }
    /// Keeps one background count running toward the end of the view's text, so
    /// the exact line count arrives without a navigation to the end (PERF-04).
    /// Each text is requested once; the count follows the index across edits.
    pub fn count_lines(
        &mut self,
        snapshot: &PagedSnapshot,
        handle: impl FnOnce() -> PagedReadHandle,
        budget: Budget,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        #[cfg(test)]
        if self.count_paused {
            return;
        }
        let key = (snapshot.identity_token(), snapshot.content_state);
        if self.counted == Some(key) || self.indexed_line_count(snapshot).is_some() {
            return;
        }
        // A peer view of this document already counts this text into the shared
        // index; a second count would only repeat its reads. Should that view
        // close or its count stop first, this view takes the count over.
        if let Ok(mut counter) = self.line_index.0.counter.lock() {
            if let Some((counting, owner)) = counter.as_ref()
                && *counting == key
                && !std::ptr::eq(owner.as_ptr(), Arc::as_ptr(&self.owner))
                && owner
                    .upgrade()
                    .is_some_and(|owner| owner.queue.lock().is_ok_and(|queue| queue.count.is_some()))
            {
                return;
            }
            *counter = Some((key, Arc::downgrade(&self.owner)));
        }
        self.counted = Some(key);
        let _ = self.submit(
            None,
            Some(CountJob {
                handle: handle(),
                budget,
                notify,
            }),
        );
    }
    /// Queues work for the owner thread, waking it when parked and starting it
    /// only when it is not running.
    fn submit(&mut self, job: Option<Job>, count: Option<CountJob>) -> Result<(), String> {
        let mut queue = self
            .owner
            .queue
            .lock()
            .map_err(|_| "Global navigation queue failed".to_owned())?;
        if let Some(job) = job {
            queue.job = Some(job);
        }
        if let Some(count) = count {
            queue.count = Some(count);
        }
        if queue.running {
            self.owner.wake.notify_all();
            return Ok(());
        }
        let owner = self.owner.clone();
        let index = self.line_index.clone();
        match std::thread::Builder::new()
            .name("bareline-global-line".into())
            .spawn(move || run(&owner, &index))
        {
            Ok(thread) => {
                queue.running = true;
                // A previous owner thread already left its loop after idling.
                queue.thread = Some(thread);
                self.owner.spawned.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(error) => {
                queue.job = None;
                queue.count = None;
                Err(format!("Cannot start global navigation: {error}"))
            }
        }
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
        if cancelled { None } else { Some(result) }
    }
}
impl Drop for GlobalNavigation {
    fn drop(&mut self) {
        self.cancel();
        self.owner.stop.store(true, Ordering::Relaxed);
        let thread = self.owner.queue.lock().ok().and_then(|mut queue| {
            queue.count = None;
            self.owner.wake.notify_all();
            queue.thread.take()
        });
        // The worker stops between windows. Joining releases its read handle,
        // and with it the document's source files, before the view is gone.
        // This is a deliberate wait on the closing thread, bounded by the one
        // window (at most INDEX_WINDOW bytes) or page read already in flight; a
        // stalled storage read delays the close by that read alone.
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// The owner thread: navigation requests first, then the background count.
fn run(owner: &Owner, index: &SharedLineIndex) {
    struct Exit<'a>(&'a Owner);
    impl Drop for Exit<'_> {
        fn drop(&mut self) {
            if std::thread::panicking()
                && let Ok(mut queue) = self.0.queue.lock()
            {
                queue.running = false;
            }
        }
    }
    let _exit = Exit(owner);
    loop {
        let work = {
            let Ok(mut queue) = owner.queue.lock() else {
                return;
            };
            let idle = Instant::now();
            loop {
                if owner.stop.load(Ordering::Relaxed) {
                    queue.running = false;
                    return;
                }
                if let Some(job) = queue.job.take() {
                    break Ok(job);
                }
                if let Some(count) = queue.count.clone() {
                    break Err(count);
                }
                // Parked, the thread holds no read handle; the next request or
                // the view's drop wakes it.
                let Some(left) = OWNER_IDLE.checked_sub(idle.elapsed()).filter(|left| !left.is_zero()) else {
                    queue.running = false;
                    return;
                };
                queue = match owner.wake.wait_timeout(queue, left) {
                    Ok((queue, _)) => queue,
                    Err(_) => return,
                };
            }
        };
        match work {
            Ok(job) => navigate(owner, index, job),
            Err(count) => {
                let interrupted =
                    || owner.stop.load(Ordering::Relaxed) || !owner.queue.lock().is_ok_and(|queue| queue.job.is_none());
                let step = index.count(&count.handle, &count.budget, COUNT_SLICE, &interrupted);
                if matches!(step, Ok(CountStep::Interrupted)) {
                    continue;
                }
                if !matches!(step, Ok(CountStep::Slice))
                    && let Ok(mut queue) = owner.queue.lock()
                    && queue.count.as_ref().is_some_and(|queued| {
                        queued.handle.snapshot().same_document(count.handle.snapshot())
                            && queued.handle.snapshot().content_state == count.handle.snapshot().content_state
                    })
                {
                    // Complete, or failed quietly: the estimate stays in place.
                    queue.count = None;
                }
                (count.notify)();
            }
        }
    }
}
fn navigate(owner: &Owner, index: &SharedLineIndex, job: Job) {
    let cancelled = || job.cancel.load(Ordering::Relaxed) || owner.stop.load(Ordering::Relaxed);
    let outcome = (|| {
        index.prepare(job.handle.snapshot(), &job.budget)?;
        #[cfg(test)]
        if let Some(hook) = &job.hook {
            let _ = hook.entered.send(());
            let _ = hook.release.recv_timeout(std::time::Duration::from_secs(5));
        }
        resolve(&job.handle, index, job.target, &job.budget, &cancelled)
    })();
    let _ = job.result.send(outcome);
    (job.notify)();
}

fn lookup(
    handle: &PagedReadHandle,
    index: &SharedLineIndex,
    target: LineTarget,
    budget: &Budget,
    cancelled: &dyn Fn() -> bool,
) -> Result<LineLookupPoll, String> {
    let result = index.lookup(handle, target, budget, &mut None, &mut || {
        if cancelled() {
            Err("Global navigation cancelled".into())
        } else {
            Ok(())
        }
    })?;
    match result {
        result @ (LineLookupPoll::Line(_) | LineLookupPoll::Range(_)) => Ok(result),
        LineLookupPoll::Unavailable(reason) => Err(format!("Global navigation source unavailable: {reason:?}")),
        LineLookupPoll::Failed(error) => Err(format!("Global navigation failed: {error:?}")),
        LineLookupPoll::Cancelled => Err("Global navigation cancelled".into()),
        _ => Err("Global navigation ended without a line".into()),
    }
}
fn resolve(
    handle: &PagedReadHandle,
    index: &SharedLineIndex,
    target: NavigationTarget,
    budget: &Budget,
    cancelled: &dyn Fn() -> bool,
) -> Result<NavigationResult, String> {
    let snapshot = handle.snapshot().clone();
    let line = match target {
        NavigationTarget::Line(line) => {
            usize::try_from(line).map_err(|_| "Logical line exceeds this platform's range")?
        }
        NavigationTarget::Byte(offset) => match lookup(handle, index, LineTarget::Byte(offset), budget, cancelled)? {
            LineLookupPoll::Line(line) => line,
            _ => return Err("Global byte lookup returned no line".into()),
        },
    };
    let LineLookupPoll::Range(range) = lookup(handle, index, LineTarget::Line(line), budget, cancelled)? else {
        return Err("Global line lookup returned no range".into());
    };
    let offset = match target {
        NavigationTarget::Line(_) => range.start,
        NavigationTarget::Byte(offset) => offset,
    };
    Ok(NavigationResult {
        snapshot,
        offset,
        line_start: range.start,
        line_end: range.end,
        first_global_line: line as u64,
    })
}
