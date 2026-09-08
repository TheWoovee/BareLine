// SPDX-License-Identifier: MPL-2.0
//! Actual paged editing controller. File I/O and document mutation run on one bounded
//! shared worker; the UI renders an explicitly incomplete, bounded viewport adapter.
use crate::{EditorSurface, Input, Selection};
use bareline_document::{
    Budget, ContentStateId, DocumentBuilder, Edit, EditTransaction, TextOffset,
    paged::{PagedSnapshot, TextWindow, WindowPoll},
};
use bareline_file_io::{
    cancellation::Cancellation,
    lifecycle::{Fingerprint, PagedOpened, PagedSavePolicy, save_paged_cancellable},
};
use bareline_platform::LocalFileSystem;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
};
const WINDOW: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlobalScrollPosition { Ready(u64, f64, f64), Pending }
#[derive(Clone, Debug)]
pub struct PagedFrameState { pub displayed: std::ops::Range<TextOffset>, pub requested: Option<TextOffset>, pub ready: bool }
/// All scroll quantities are UTF-8 bytes, independent of unknown line totals.
#[derive(Clone, Copy, Debug)]
pub struct PagedScrollMetrics { pub offset: f64, pub viewport: f64, pub total: f64, pub lines_known: bool }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionRestoreStatus { Pending, Applied, Failed(String), Superseded }
struct SelectionValidation {
    cancellation: Cancellation,
    snapshot: PagedSnapshot,
    selection: Selection,
    preserve_viewport: bool,
    result: Receiver<Result<Selection, String>>,
}
impl Drop for SelectionValidation { fn drop(&mut self) { self.cancellation.cancel(); } }
#[derive(Clone, Copy)]
struct ViewportMapping { offset: usize, line: u64, line_start: TextOffset }
type Job = Box<dyn FnOnce() + Send>;
fn worker() -> &'static SyncSender<Job> {
    static WORKER: OnceLock<SyncSender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Job>(16);
        std::thread::Builder::new()
            .name("paged-view-io".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    job();
                }
            })
            .expect("paged worker creation");
        sender
    })
}
enum Action {
    Tail { platform: Arc<dyn LocalFileSystem>, request: bool, follow: bool },
    UnlockTail,
    RetryRecovery,
    Prepared(EditTransaction, Option<crate::tracked_edit::TrackedEditCompletion>),
    Source(bareline_document::paged::PreparedSourceTransaction, Option<crate::tracked_edit::TrackedEditCompletion>),
    Metadata(bareline_document::DocumentMetadata),
    Read(usize),
    Edit {
        range: std::ops::Range<TextOffset>,
        insert: String,
    },
    Undo,
    Redo,
    Save {
        copy_only: bool,
        target: PathBuf,
        expected: Option<Fingerprint>,
        platform: Arc<dyn LocalFileSystem>,
    },
}
struct Completed {
    selection: Option<Selection>,
    append_receipt: Option<bareline_file_io::tail::AppendReceipt>,
    generation_owner: Arc<()>,
    peer_epoch: u64,
    saved_state: Option<ContentStateId>,
    save_as_required: bool,
    following: bool,
    tail_pending: bool,
    source_changed: bool,
    fingerprint: Fingerprint,
    can_undo: bool,
    can_redo: bool,
    snapshot: PagedSnapshot,
    window: Result<TextWindow, String>,
    path: PathBuf,
    caret: usize,
    saved: Option<Fingerprint>,
}
struct PeerState {
    epoch: u64,
    saved_state: Option<ContentStateId>,
    save_as_required: bool,
}
struct RetiredGeneration {
    owner: Arc<()>,
    opened: Box<PagedOpened>,
    tail: Option<bareline_file_io::tail::TailSession>,
}
/// Immutable snapshot plus generation-checked page resolver for background consumers.
/// Never resolve pages on the UI thread. Busy returns without waiting for the actor.
#[derive(Clone)]
pub struct PagedReadHandle {
    _generation: Arc<()>,
    retired: Arc<Mutex<Vec<RetiredGeneration>>>,
    _views: Arc<()>,
    path: PathBuf,
    fingerprint: Fingerprint,
    tail: Arc<Mutex<Option<bareline_file_io::tail::TailSession>>>,
    actor: Arc<Mutex<Box<PagedOpened>>>,
    snapshot: PagedSnapshot,
}
impl PagedReadHandle {
    /// Captures provenance without opening files. Sealed readers are acquired by
    /// the consumer's worker, never by the UI thread.
    pub fn original_store(&self) -> Result<bareline_file_io::codecs::disk::DiskDecoded, String> {
        let opened = self.actor.try_lock().map_err(|_| "Paged source is busy")?;
        let current = opened.transcoded.document.snapshot();
        if !current.same_document(&self.snapshot) || current.content_state != self.snapshot.content_state {
            return Err("Paged source changed".into());
        }
        Ok(opened.transcoded.store.clone())
    }
    pub fn snapshot(&self) -> &PagedSnapshot { &self.snapshot }
    pub fn resolve_page(&self, ticket: bareline_document::source::PageTicket) -> Result<bool, String> {
        self.resolve(ticket, false)
    }
    /// Historical comparisons may retain an immutable root through later edits.
    /// Resolve only tickets referenced by that captured root and still owned by this
    /// actor's original/append generation. Reopen/unlock to another document fails closed.
    pub fn resolve_captured_page(&self, ticket: bareline_document::source::PageTicket) -> Result<bool, String> {
        self.resolve(ticket, true)
    }
    fn resolve(&self, ticket: bareline_document::source::PageTicket, historical: bool) -> Result<bool, String> {
        let referenced = self.snapshot.pieces().any(|piece| {
            let (source, range) = match piece {
                bareline_document::paged::PagedPiece::Original { source, range } | bareline_document::paged::PagedPiece::OriginalOwned { source, range, .. } | bareline_document::paged::PagedPiece::OwnedSource { source, range, .. } => (source, range),
                bareline_document::paged::PagedPiece::Inserted(_) => return false,
            };
            ticket.generation == source.generation() && ticket.page.checked_mul(source.page_size() as u64).is_some_and(|start| start < range.end && start.saturating_add(source.page_size() as u64) > range.start)
        });
        if !referenced { return Err("Page is not part of the captured source generation".into()); }
        let mut opened = match self.actor.try_lock() {
            Ok(opened) => opened,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(false),
            Err(std::sync::TryLockError::Poisoned(_)) => return Err("Paged source worker failed".into()),
        };
        let current = opened.transcoded.document.snapshot();
        if historical && !current.same_document(&self.snapshot) {
            drop(opened);
            let mut retired = match self.retired.try_lock() { Ok(retired) => retired, Err(std::sync::TryLockError::WouldBlock) => return Ok(false), Err(_) => return Err("Captured generation owner failed".into()) };
            let generation = retired.iter_mut().find(|generation| generation.opened.transcoded.document.snapshot().same_document(&self.snapshot)).ok_or("Captured generation is unavailable")?;
            if self.snapshot.resolve_owned(ticket).map_err(|error| format!("Captured owned source unavailable: {error:?}"))? { return Ok(true); }
            let handled = match generation.tail.as_mut() { Some(tail) => tail.read_page(ticket).map_err(|e| format!("{e:?}"))?, None => false };
            if !handled { generation.opened.transcoded.source.read_page(ticket).map_err(|e| format!("Captured source unavailable: {e:?}"))?; }
            return Ok(true);
        }
        if !current.same_document(&self.snapshot) || (!historical && current.content_state != self.snapshot.content_state) {
            return Err("Paged source changed; recompare or search again".into());
        }
        if self.snapshot.resolve_owned(ticket).map_err(|error| format!("Owned source unavailable: {error:?}"))? { return Ok(true); }
        let mut tail = match self.tail.try_lock() { Ok(tail) => tail, Err(std::sync::TryLockError::WouldBlock) => return Ok(false), Err(_) => return Err("Tail worker failed".into()) };
        let handled = match tail.as_mut() { Some(tail) => tail.read_page(ticket).map_err(|e| format!("{e:?}"))?, None => false };
        if !handled { opened.transcoded.source.read_page(ticket).map_err(|error| format!("Paged source unavailable: {error:?}"))?; }
        Ok(true)
    }
}
pub struct PagedEditorSurface {
    viewport_request: Option<TextOffset>,
    queued_viewport: Option<TextOffset>,
    queued_wheel: (f64, f32),
    bottom_scroll: Option<f32>,
    horizontal_anchor: Option<(usize, f32)>,
    queued_horizontal: f64,
    prefetch: crate::paged_navigation::ViewportPrefetch,
    global_selection: Selection,
    projected_selection: Selection,
    selection_token: u64,
    selection_status: SelectionRestoreStatus,
    selection_validation: Option<SelectionValidation>,
    pending_moves_selection: bool,
    deferred_input: Option<Input>,
    navigation_anchor: Option<usize>,
    initial_eol: Option<((u64,u64),bareline_file_io::codecs::state::EolState)>,
    encoding_failure: Arc<Mutex<Option<bareline_file_io::codecs::failure::EncodingFailure>>>,
    global_folds: Vec<bareline_syntax::folding::Fold>,
    global_fold_state: bareline_syntax::folding::FoldState,
    global_fold_overrides: std::collections::BTreeMap<usize, bool>,
    global_folds_partial: bool,
    global_fold_initialized: bool,
    pending_global_folds: Vec<std::ops::Range<u64>>,
    fold_viewport_line: Option<usize>,
    navigation: crate::paged_navigation::GlobalNavigation,
    navigation_ready: Option<crate::paged_navigation::NavigationResult>,
    requested_scroll: Option<(f64, f64)>,
    pending_scroll_mapping: Option<(ViewportMapping, f64, f64)>,
    viewport_mapping: Option<ViewportMapping>,
    global_spacers: Vec<(u64, u64)>,
    search_marks: crate::search_marks::SearchMarks,
    pending_marks: Option<crate::search_marks::SearchMarks>,
    append_receipt: Option<bareline_file_io::tail::AppendReceipt>,
    generation_owner: Arc<Mutex<Arc<()>>>,
    view_generation: Arc<()>,
    retired: Arc<Mutex<Vec<RetiredGeneration>>>,
    captured: Option<PagedReadHandle>,
    peer: Arc<Mutex<PeerState>>,
    peer_epoch: u64,
    views: Arc<()>,
    tail: Arc<Mutex<Option<bareline_file_io::tail::TailSession>>>,
    following: bool,
    follow_paused: bool,
    tail_pending: bool,
    tail_changed: bool,
    pub surface: EditorSurface,
    actor: Arc<Mutex<Box<PagedOpened>>>,
    snapshot: PagedSnapshot,
    saved_state: Option<ContentStateId>,
    recovery_config: Option<(PathBuf, Arc<dyn LocalFileSystem>)>,
    streaming_quota: u64,
    tracked_acknowledged: std::collections::VecDeque<u64>,
    pub save_as_required: bool,
    can_undo: bool,
    can_redo: bool,
    recovery: Arc<Mutex<Option<bareline_file_io::paged_recovery::PagedRecovery>>>,
    recovery_status: Arc<Mutex<bareline_file_io::paged_recovery::PagedRecoveryStatus>>,
    failed_retirements: Arc<Mutex<Vec<PathBuf>>>,
    budget: Budget,
    pending: Option<Receiver<Result<Completed, String>>>,
    pending_input: Option<Input>,
    cancellation: Cancellation,
    notify: Arc<dyn Fn() + Send + Sync>,
    viewport_start: usize,
    viewport_valid: bool,
    pub fingerprint: Fingerprint,
    pub path: PathBuf,
    recovery_origin: Option<PathBuf>,
    pub error: Option<String>,
}
impl PagedEditorSurface {
    pub fn initial_eol_label(&self)->Option<&'static str> {self.initial_eol.filter(|(identity,_)|*identity==self.snapshot.identity_token()).map(|(_,eol)|eol.label())}
    pub fn encoding_failure(&self)->Option<bareline_file_io::codecs::failure::EncodingFailure>{self.encoding_failure.try_lock().ok().and_then(|failure|failure.clone())}
    pub fn encoding_state(&self) -> Option<bareline_file_io::codecs::state::EncodingState> { bareline_file_io::codecs::state::metadata_encoding(self.snapshot.metadata()).or_else(||self.actor.try_lock().ok().map(|opened| opened.transcoded.store.state.clone())) }
    pub fn apply_document_metadata(&mut self, metadata: bareline_document::DocumentMetadata) -> Result<(),String> { if self.surface.user_read_only {return Err("Document is read only".into());} self.submit(Action::Metadata(metadata)) }
    pub fn read_handle(&self) -> PagedReadHandle {
        if let Some(captured) = &self.captured { return captured.clone(); }
        PagedReadHandle { _generation: self.view_generation.clone(), retired: self.retired.clone(), _views: self.views.clone(), path: self.path.clone(), fingerprint: self.fingerprint.clone(), actor: self.actor.clone(), tail: self.tail.clone(), snapshot: self.snapshot.clone() }
    }
    pub fn new(
        opened: Box<PagedOpened>,
        budget: Budget,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let snapshot = opened.transcoded.document.snapshot();
        let prefix = DocumentBuilder::new(budget.clone(), Budget::new(0))
            .map_err(|error| format!("{error:?}"))?
            .prefix();
        let mut surface = EditorSurface::loading(prefix, notify.clone());
        surface.set_eol_status_override(Some("Computing".into()));
        surface.encoding_label = format!("{:?}", opened.transcoded.store.state.save_target);
        surface.user_read_only = opened.transcoded.store.state.binary_warning;
        let generation_owner = Arc::new(());
        let mut view = Self {
            viewport_request: None, queued_viewport: None, queued_wheel: (0.0, 0.0), bottom_scroll: None, horizontal_anchor: None, queued_horizontal: 0.0, prefetch: Default::default(),
            initial_eol: (opened.recovery_origin.is_none() && snapshot.revision.0==0 && snapshot.content_state==opened.transcoded.document.saved_content_state()).then_some((snapshot.identity_token(),opened.transcoded.store.eol)),
            encoding_failure: Arc::new(Mutex::new(None)),
            navigation: crate::paged_navigation::GlobalNavigation::new(), navigation_ready: None, requested_scroll: None, pending_scroll_mapping: None, viewport_mapping: None, global_spacers: Vec::new(),
            global_folds: Vec::new(), global_fold_state: Default::default(), global_fold_overrides: Default::default(), global_folds_partial: true, global_fold_initialized: false, pending_global_folds: Vec::new(), fold_viewport_line: None,
            global_selection: Selection::default(), projected_selection: Selection::default(), selection_token: 0, selection_status: SelectionRestoreStatus::Superseded, selection_validation: None, pending_moves_selection: false, deferred_input: None, navigation_anchor: None,
            generation_owner: Arc::new(Mutex::new(generation_owner.clone())), view_generation: generation_owner,
            search_marks: Default::default(), pending_marks: None,
            append_receipt: None,
            retired: Arc::new(Mutex::new(Vec::new())), captured: None,
            peer: Arc::new(Mutex::new(PeerState { epoch: 0, saved_state: opened.recovery_origin.is_none().then_some(opened.transcoded.document.saved_content_state()), save_as_required: opened.recovery_origin.is_some() })),
            peer_epoch: 0,
            views: Arc::new(()),
            tail: Arc::new(Mutex::new(None)),
            following: false, follow_paused: false, tail_pending: false, tail_changed: false,
            saved_state: opened
                .recovery_origin
                .is_none()
                .then_some(opened.transcoded.document.saved_content_state()),
            save_as_required: opened.recovery_origin.is_some(),
            can_undo: opened.transcoded.document.can_undo(),
            can_redo: opened.transcoded.document.can_redo(),
            recovery_config: None,
            streaming_quota: 20 * 1024 * 1024 * 1024,
            tracked_acknowledged: Default::default(),
            recovery: Arc::new(Mutex::new(None)),
            recovery_status: Arc::new(Mutex::new(Default::default())),
            failed_retirements: Arc::new(Mutex::new(Vec::new())),
            snapshot,
            fingerprint: opened.fingerprint.clone(),
            path: opened.path.clone(),
            recovery_origin: opened.recovery_origin.clone(),
            surface,
            actor: Arc::new(Mutex::new(opened)),
            budget,
            pending: None,
            pending_input: None,
            cancellation: Cancellation::default(),
            notify,
            viewport_start: 0,
            viewport_valid: false,
            error: None,
        };
        view.request_viewport(TextOffset(0))?;
        Ok(view)
    }
    /// A real peer of the same full paged actor. Only viewport, selection and scrolling
    /// belong to the new view; no resident prefix is substituted for the document.
    pub fn clone_view(&self) -> Result<Self, String> {
        self.clone_view_inner(self.captured.clone())
    }
    pub fn clone_captured_view(&self, handle: &PagedReadHandle) -> Result<Self, String> {
        if !Arc::ptr_eq(&self.actor, &handle.actor) { return Err("Captured view belongs to another document actor".into()); }
        self.clone_view_inner(Some(handle.clone()))
    }
    fn clone_view_inner(&self, captured: Option<PagedReadHandle>) -> Result<Self, String> {
        let prefix = DocumentBuilder::new(self.budget.clone(), Budget::new(0)).map_err(|e| format!("{e:?}"))?.prefix();
        let mut surface = EditorSurface::loading(prefix, self.notify.clone());
        surface.encoding_label = self.surface.encoding_label.clone();
        surface.set_eol_status_override(Some("Computing".into()));
        surface.user_read_only = captured.is_some() || self.surface.user_read_only;
        surface.theme = self.surface.theme;
        surface.language = self.surface.language;
        surface.language_override = self.surface.language_override;
        surface.detected_language = self.surface.detected_language;
        surface.syntax_preference = self.surface.syntax_preference;
        surface.font_pixels = self.surface.font_pixels;
        surface.font_family = self.surface.font_family.clone();
        surface.tab_width = self.surface.tab_width;
        surface.line_numbers = self.surface.line_numbers;
        surface.highlight_current_line = self.surface.highlight_current_line;
        surface.whitespace = self.surface.whitespace.clone();
        let mut view = Self {
            navigation: crate::paged_navigation::GlobalNavigation::new(), navigation_ready: None, requested_scroll: None, pending_scroll_mapping: None, viewport_mapping: None, global_spacers: self.global_spacers.clone(),
            viewport_request: None, queued_viewport: None, queued_wheel: (0.0, 0.0), bottom_scroll: None, horizontal_anchor: None, queued_horizontal: 0.0, prefetch: Default::default(),
            retired: self.retired.clone(), captured: captured.clone(),
            global_selection: if captured.is_some() { Selection::default() } else { let (anchor, caret) = self.global_selection(); Selection { anchor: anchor.0, caret: caret.0 } }, projected_selection: Selection::default(), selection_token: 0, selection_status: SelectionRestoreStatus::Superseded, selection_validation: None, pending_moves_selection: false, deferred_input: None, navigation_anchor: None,
            global_folds: self.global_folds.clone(), global_fold_state: self.global_fold_state.clone(), global_fold_overrides: self.global_fold_overrides.clone(), global_folds_partial: self.global_folds_partial, global_fold_initialized: self.global_fold_initialized, pending_global_folds: self.pending_global_folds.clone(), fold_viewport_line: None,
            search_marks: self.search_marks.clone(), pending_marks: None,
            append_receipt: self.append_receipt,
            generation_owner: self.generation_owner.clone(), view_generation: captured.as_ref().map_or_else(|| self.view_generation.clone(), |h| h._generation.clone()),
            peer: self.peer.clone(), peer_epoch: self.peer_epoch, views: self.views.clone(),
            tail: self.tail.clone(), following: captured.is_none() && self.following, follow_paused: self.follow_paused,
            tail_pending: self.tail_pending, tail_changed: self.tail_changed, surface,
            initial_eol:self.initial_eol, encoding_failure:self.encoding_failure.clone(),
            actor: self.actor.clone(), snapshot: captured.as_ref().map_or_else(|| self.snapshot.clone(), |h| h.snapshot.fork_identity()), saved_state: captured.as_ref().map_or(self.saved_state, |h| Some(h.snapshot.content_state)),
            streaming_quota: self.streaming_quota,
            tracked_acknowledged: self.tracked_acknowledged.clone(),
            recovery_config: self.recovery_config.clone(), save_as_required: self.save_as_required,
            can_undo: self.can_undo, can_redo: self.can_redo, recovery: self.recovery.clone(),
            recovery_status: self.recovery_status.clone(), failed_retirements: self.failed_retirements.clone(),
            budget: self.budget.clone(), pending: None, pending_input: None, cancellation: self.cancellation.clone(),
            notify: self.notify.clone(), viewport_start: self.viewport_start, viewport_valid: false,
            fingerprint: captured.as_ref().map_or_else(|| self.fingerprint.clone(), |h| h.fingerprint.clone()), path: captured.as_ref().map_or_else(|| self.path.clone(), |h| h.path.clone()), recovery_origin: self.recovery_origin.clone(), error: None,
        };
        view.request_viewport(TextOffset(self.viewport_start))?;
        Ok(view)
    }
    /// Nonblocking peer publication check; the next viewport is fetched on the worker.
    pub fn refresh_peer(&mut self) -> bool {
        if self.busy() || self.captured.is_some() { return false; }
        let changed = self.peer.try_lock().is_ok_and(|peer| peer.epoch != self.peer_epoch);
        if !changed { return false; }
        self.sync_global_selection();
        self.viewport_valid = false;
        match self.submit(Action::Read(self.viewport_start)) { Ok(()) => true, Err(error) => { self.error = Some(error); false } }
    }
    pub fn enable_recovery(&mut self, root: PathBuf, platform: Arc<dyn LocalFileSystem>) {
        self.recovery_config = Some((root, platform));
    }
    pub fn retry_recovery(&mut self) -> Result<(), String> {
        self.submit(Action::RetryRecovery)
    }
    pub fn recovery_status(&self) -> bareline_file_io::paged_recovery::PagedRecoveryStatus {
        self.recovery_status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }
    pub fn mark_recovered(&mut self) {
        self.saved_state = None;
        if let Ok(mut peer) = self.peer.lock() { peer.saved_state = None; peer.epoch = peer.epoch.wrapping_add(1); }
    }
    pub fn snapshot(&self) -> &PagedSnapshot {
        &self.snapshot
    }
    pub fn can_undo(&self) -> bool {
        self.can_undo
    }
    pub fn can_redo(&self) -> bool {
        self.can_redo
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.selection_validation.is_some()
    }
    pub fn recovery_origin_path(&self) -> Option<&std::path::Path> { self.recovery_origin.as_deref() }
    pub fn viewport_ready(&self) -> bool { self.viewport_valid && !self.busy() }
    pub fn paged_frame_state(&self) -> PagedFrameState {
        PagedFrameState { displayed: TextOffset(self.viewport_start)..TextOffset(self.viewport_start + self.surface.snapshot.len()), requested: self.queued_viewport.or(self.viewport_request), ready: self.viewport_ready() && self.viewport_request.is_none() && self.queued_viewport.is_none() && self.requested_scroll.is_none() }
    }
    pub fn paged_scroll_metrics(&self, height: f32) -> PagedScrollMetrics {
        let snapshot = self.surface.snapshot();
        let (line, _, _) = self.surface.logical_scroll();
        let line = (line as usize).min(snapshot.line_count().saturating_sub(1));
        let start = snapshot.line_range(line).map_or(0, |range| range.start.0);
        let rows = ((height.max(0.0) as f64 / self.surface.line_height() as f64).ceil() as usize).max(1);
        let end = snapshot.line_range(line.saturating_add(rows)).map_or(snapshot.len(), |range| range.start.0);
        let viewport = end.saturating_sub(start).max(1).min(self.snapshot.len().max(1)) as f64;
        let total = self.snapshot.len().max(1) as f64;
        let offset = self.queued_viewport.or(self.viewport_request).map_or(self.viewport_start.saturating_add(start), |offset| offset.0) as f64;
        PagedScrollMetrics { offset: offset.min((total - viewport).max(0.0)), viewport, total, lines_known: self.viewport_start == 0 && snapshot.len() == self.snapshot.len() }
    }
    pub fn set_known_global_folds(&mut self, mut folds: Vec<bareline_syntax::folding::Fold>, level: usize, partial: bool, viewport_first_line: usize) -> Result<(), String> {
        if !self.viewport_valid { return Err("Wait for the paged viewport before projecting folds".into()); }
        if folds.iter().any(|fold| fold.header >= fold.end) { return Err("Invalid global fold range".into()); }
        let truncated = folds.len() > 8192; folds.truncate(8192); folds.sort_by_key(|fold| (fold.header, std::cmp::Reverse(fold.end))); folds.dedup_by_key(|fold| (fold.header, fold.end));
        self.global_folds = folds; self.global_folds_partial = partial || truncated; self.fold_viewport_line = Some(viewport_first_line);
        if !self.global_fold_initialized { if level > 0 { self.global_fold_state.apply_level(&self.global_folds, level); } self.global_fold_initialized = true; }
        else { self.global_fold_state.refresh(&self.global_folds); }
        for (&header, &collapsed) in &self.global_fold_overrides { if collapsed { self.global_fold_state.collapsed.insert(header); } else { self.global_fold_state.collapsed.remove(&header); } }
        for range in &self.pending_global_folds { if let Some(fold) = self.global_folds.iter().find(|fold| fold.header as u64 == range.start && fold.end as u64 + 1 == range.end) { self.global_fold_state.collapsed.insert(fold.header); } }
        if !self.global_folds_partial { self.pending_global_folds.clear(); }
        self.project_global_folds(); Ok(())
    }
    pub fn persisted_global_folds(&self) -> Vec<std::ops::Range<u64>> {
        if !self.pending_global_folds.is_empty() { return self.pending_global_folds.clone(); }
        self.global_folds.iter().filter(|fold| self.global_fold_state.collapsed.contains(&fold.header)).map(|fold| fold.header as u64..fold.end as u64 + 1).collect()
    }
    pub fn restore_global_folds(&mut self, ranges: &[std::ops::Range<u64>]) {
        self.pending_global_folds = ranges.iter().filter(|range| range.start < range.end && usize::try_from(range.end).is_ok()).take(8192).cloned().collect();
        self.global_fold_initialized = true; self.global_fold_state.unfold_all();
        for range in &self.pending_global_folds { if let Some(fold) = self.global_folds.iter().find(|fold| fold.header as u64 == range.start && fold.end as u64 + 1 == range.end) { self.global_fold_state.collapsed.insert(fold.header); } }
        self.project_global_folds();
    }
    pub fn fold_all_known(&mut self, level: usize) { self.global_fold_initialized = true; self.global_fold_overrides.clear(); self.global_fold_state.apply_level(&self.global_folds, level); self.project_global_folds(); }
    pub fn unfold_all_known(&mut self) { self.global_fold_initialized = true; self.global_fold_overrides.clear(); self.global_fold_state.unfold_all(); self.project_global_folds(); }
    pub fn toggle_current_known(&mut self) -> Result<(), String> {
        let first = self.fold_viewport_line.ok_or("Global line mapping is pending")?;
        let local = self.surface.snapshot.line_at(TextOffset(self.surface.selection.caret)).map_err(|_| "Caret line unavailable")?;
        let line = first.saturating_add(local);
        let header = self.global_folds.iter().filter(|fold| fold.header <= line && line <= fold.end).max_by_key(|fold| fold.header).map(|fold| fold.header).ok_or("No verified fold at the caret")?;
        self.global_fold_state.toggle(header); self.global_fold_overrides.insert(header, self.global_fold_state.collapsed.contains(&header)); self.project_global_folds(); Ok(())
    }
    fn project_global_folds(&mut self) {
        let Some(first) = self.fold_viewport_line else { self.surface.set_known_folds(Vec::new(), 8, true); return; };
        let end = first.saturating_add(self.surface.snapshot.line_count());
        let length = self.surface.snapshot.len();
        let at_eof = self.viewport_start.saturating_add(length) == self.snapshot.len();
        let complete_line = length != 0 && self.surface.snapshot.read(TextOffset(length - 1)..TextOffset(length), 1).is_ok_and(|last| last == "\n" || last == "\r");
        let complete_end = if at_eof || complete_line { end } else { end.saturating_sub(1) };
        let first_complete = self.viewport_start == 0 || self.viewport_first_line_start().is_some_and(|start| start.0 == self.viewport_start);
        let folds = self.global_folds.iter().filter(|fold| fold.header >= first && (fold.header != first || first_complete) && fold.end < complete_end).map(|fold| bareline_syntax::folding::Fold { header: fold.header - first, end: fold.end - first, level: fold.level }).collect();
        self.surface.set_known_folds(folds, 8, self.global_folds_partial);
        self.surface.fold_state.collapsed = self.global_fold_state.collapsed.iter().filter(|header| **header >= first && **header < end).map(|header| header - first).collect();
        self.surface.refresh_hidden_lines();
    }
    pub fn viewport_first_global_line(&self) -> Option<u64> { self.viewport_mapping.filter(|mapping| self.viewport_valid && !self.busy() && mapping.offset == self.viewport_start).map(|mapping| mapping.line) }
    pub fn viewport_first_line_start(&self) -> Option<TextOffset> { self.viewport_mapping.filter(|mapping| self.viewport_valid && !self.busy() && mapping.offset == self.viewport_start).map(|mapping| mapping.line_start) }
    pub fn global_logical_scroll(&mut self) -> GlobalScrollPosition {
        if self.busy() || self.requested_scroll.is_some() || self.pending_scroll_mapping.is_some() { return GlobalScrollPosition::Pending; }
        if let Some(first) = self.viewport_first_global_line() {
            let (line, fraction, x) = self.surface.logical_scroll();
            // A clipped first line has no measured absolute horizontal origin.
            if line == 0 && self.viewport_first_line_start().is_some_and(|start| start.0 != self.viewport_start) { return GlobalScrollPosition::Pending; }
            GlobalScrollPosition::Ready(first.saturating_add(line), fraction, x)
        } else { self.ensure_viewport_mapping(); GlobalScrollPosition::Pending }
    }
    pub fn request_global_scroll(&mut self, line: u64, fraction: f64, x: f64) -> Result<(), String> {
        if let Some(first) = self.viewport_first_global_line() && line >= first && line - first < self.surface.snapshot.line_count() as u64
            && (line > first || self.viewport_first_line_start() == Some(TextOffset(self.viewport_start))) {
            self.surface.set_logical_scroll(line - first, fraction, x); return Ok(());
        }
        self.requested_scroll = Some((fraction, x)); self.navigation_ready = None;
        self.navigation.request(self.read_handle(), crate::paged_navigation::NavigationTarget::Line(line), self.budget.clone(), self.notify.clone())
    }
    /// User wheel/trackpad scrolling. Crossing a loaded window requests an adjacent
    /// bounded viewport; the byte position remains useful while exact lines are pending.
    pub fn scroll_viewport(&mut self, delta: f64, height: f32) -> Result<(), String> {
        if !delta.is_finite() { return Ok(()); }
        if self.following && delta < 0.0 { self.follow_paused = true; }
        if self.busy() || self.viewport_request.is_some() || self.requested_scroll.is_some() {
            self.queued_wheel.0 = (self.queued_wheel.0 + delta).clamp(-1.0e9, 1.0e9); self.queued_wheel.1 = height; return Ok(());
        }
        let line_height = self.surface.line_height() as f64;
        let end = self.viewport_start.saturating_add(self.surface.snapshot.len());
        let at_top = self.surface.scroll_y + delta < 0.0;
        let at_bottom = self.surface.scroll_y + delta + height as f64 >= self.surface.snapshot.line_count() as f64 * line_height;
        if (delta < 0.0 && at_top && self.viewport_start != 0) || (delta > 0.0 && at_bottom && end < self.snapshot.len()) {
            if let GlobalScrollPosition::Ready(line, fraction, x) = self.global_logical_scroll() {
                let position = fraction + delta / line_height;
                let rows = position.floor() as i64;
                let target = if rows < 0 { line.saturating_sub(rows.unsigned_abs()) } else { line.saturating_add(rows as u64) };
                return self.request_global_scroll(target, position - position.floor(), x);
            }
            let start = if delta < 0.0 { self.viewport_start.saturating_sub(WINDOW / 2) } else { self.viewport_start.saturating_add(WINDOW / 2).min(self.snapshot.len()) };
            return self.request_viewport(TextOffset(start));
        }
        self.surface.scroll(delta, height);
        let margin = (height as f64 * 2.0).max(line_height * 8.0);
        if delta > 0.0 && self.surface.scroll_y + margin >= self.surface.snapshot.line_count() as f64 * line_height {
            self.prefetch.request(self.read_handle(), TextOffset(end), self.budget.clone(), self.notify.clone());
        } else if delta < 0.0 && self.surface.scroll_y < margin && self.viewport_start > 0 {
            self.prefetch.request(self.read_handle(), TextOffset(self.viewport_start.saturating_sub(WINDOW)), self.budget.clone(), self.notify.clone());
        }
        Ok(())
    }
    pub fn request_byte_scroll(&mut self, fraction: f64) -> Result<(), String> {
        self.request_byte_scroll_in_view(fraction, 600.0)
    }
    pub fn scroll_horizontal(&mut self, delta: f64) {
        if !delta.is_finite() { return; }
        if !self.paged_frame_state().ready { self.queued_horizontal = (self.queued_horizontal + delta).clamp(-1.0e9, 1.0e9); }
        else { self.surface.scroll_horizontal(delta); }
    }
    pub fn refine_horizontal_viewport(&mut self, backend: &impl bareline_renderer::TextBackend, width: f32) -> Result<bool, String> {
        if !self.paged_frame_state().ready { return Ok(false); }
        let Some(anchor) = self.surface.horizontal_window_anchor(backend, width) else { return Ok(false); };
        let end = self.viewport_start.saturating_add(self.surface.snapshot.len());
        if (anchor.direction < 0 && self.viewport_start == 0) || (anchor.direction > 0 && end >= self.snapshot.len()) { return Ok(false); }
        let offset = if anchor.direction < 0 { self.viewport_start.saturating_sub(WINDOW / 2) } else { self.viewport_start.saturating_add(WINDOW / 2).min(self.snapshot.len()) };
        let global_anchor = self.viewport_start.saturating_add(anchor.offset.0);
        if global_anchor < offset || global_anchor > offset.saturating_add(WINDOW).min(self.snapshot.len()) { return Ok(false); }
        self.request_viewport(TextOffset(offset))?;
        self.horizontal_anchor = Some((global_anchor, anchor.screen_x));
        Ok(true)
    }
    pub fn request_byte_scroll_in_view(&mut self, fraction: f64, height: f32) -> Result<(), String> {
        if !fraction.is_finite() { return Err("Invalid scrollbar position".into()); }
        if self.following && fraction < 1.0 { self.follow_paused = true; }
        self.queued_wheel = (0.0, height);
        self.navigation.cancel(); self.navigation_ready = None; self.requested_scroll = None; self.pending_scroll_mapping = None;
        self.bottom_scroll = (fraction >= 1.0).then_some(height);
        let extent = self.paged_scroll_metrics(height);
        let offset = if fraction >= 1.0 { self.snapshot.len().saturating_sub(WINDOW) } else { ((extent.total - extent.viewport).max(0.0) * fraction.clamp(0.0, 1.0)) as usize };
        self.request_viewport(TextOffset(offset))
    }
    pub fn byte_scroll_fraction(&self) -> f64 { if self.snapshot.is_empty() { 0.0 } else { self.viewport_start as f64 / self.snapshot.len() as f64 } }
    pub fn set_global_spacers(&mut self, rows: &[(u64, u64)]) -> Result<(), String> {
        if rows.len() > 8192 { return Err("Too many comparison spacer rows".into()); }
        self.global_spacers = rows.to_vec(); self.project_global_spacers()
    }
    fn project_global_spacers(&mut self) -> Result<(), String> {
        let Some(first) = self.viewport_first_global_line() else { return self.surface.set_view_spacers(&[]); };
        let end = first.saturating_add(self.surface.snapshot.line_count() as u64);
        let rows: Vec<_> = self.global_spacers.iter().filter(|(line, _)| *line >= first && *line <= end).map(|(line, count)| (line - first, *count)).collect();
        self.surface.set_view_spacers(&rows)
    }
    fn ensure_viewport_mapping(&mut self) {
        if self.viewport_valid && self.viewport_mapping.is_none() && !self.navigation.is_pending() && self.navigation_ready.is_none() && self.requested_scroll.is_none() {
            if let Err(error) = self.navigation.request(self.read_handle(), crate::paged_navigation::NavigationTarget::Byte(TextOffset(self.viewport_start)), self.budget.clone(), self.notify.clone()) { self.error = Some(error); }
        }
    }
    fn pump_navigation(&mut self) -> bool {
        let mut changed = false;
        if let Some(result) = self.navigation.poll() {
            match result { Ok(result) => self.navigation_ready = Some(result), Err(error) => { self.requested_scroll = None; self.error = Some(error); } }
            changed = true;
        }
        if self.busy() { return changed; }
        let Some(result) = self.navigation_ready.take() else { return changed; };
        let handle = self.read_handle();
        if !result.snapshot.same_document(handle.snapshot()) || result.snapshot.content_state != handle.snapshot().content_state { self.requested_scroll = None; return true; }
        let mapping = ViewportMapping { offset: result.offset.0, line: result.first_global_line, line_start: result.line_start };
        if let Some((fraction, x)) = self.requested_scroll.take() {
            self.pending_scroll_mapping = Some((mapping, fraction, x));
            if let Err(error) = self.request_viewport(result.offset) { self.pending_scroll_mapping = None; self.error = Some(error); }
        } else if result.offset.0 == self.viewport_start {
            self.viewport_mapping = Some(mapping); self.fold_viewport_line = usize::try_from(mapping.line).ok(); let _ = self.project_global_spacers(); self.project_global_folds();
        }
        true
    }
    pub fn set_search_marks(&mut self, style: u8, ranges: Vec<std::ops::Range<TextOffset>>) -> Result<(), String> {
        if ranges.iter().any(|range| range.end.0 > self.snapshot.len()) { return Err("Mark is outside this paged generation".into()); }
        self.search_marks.set(style, ranges)?; self.project_search_marks(); Ok(())
    }
    pub fn clear_search_marks(&mut self, style: Option<u8>) { self.search_marks.clear(style); self.project_search_marks(); }
    fn project_search_marks(&mut self) {
        self.surface.clear_search_marks(None);
        let start = self.viewport_start;
        let end = start.saturating_add(self.surface.snapshot.len());
        for style in 1..=5 {
            let ranges = self.search_marks.iter().filter(|(s, range)| *s == style && range.start.0 < end && range.end.0 > start).map(|(_, range)| TextOffset(range.start.0.max(start) - start)..TextOffset(range.end.0.min(end) - start)).collect();
            let _ = self.surface.set_search_marks(style, ranges);
        }
    }
    pub fn append_receipt(&self) -> Option<bareline_file_io::tail::AppendReceipt> { self.append_receipt }
    pub fn follow_status(&self) -> Option<(bool, bool)> { self.following.then_some((self.follow_paused, self.tail_changed)) }
    pub fn follow_banner_text(&self) -> Option<String> {
        self.follow_status().map(|(paused, changed)| {
            let name = self.path.file_name().unwrap_or_default().to_string_lossy();
            if changed { format!("{name} · Source changed (rotated, truncated or rewritten) · Reopen and follow") }
            else if paused { format!("Following {name} · Paused (scrolled up) · Resume ↓ · Unlock to edit") }
            else { format!("Following {name} · Following new content · Pause · Unlock to edit") }
        })
    }
    pub fn start_follow(&mut self, platform: Arc<dyn LocalFileSystem>) -> Result<(), String> {
        if self.dirty() { return Err("Save or discard edits before monitoring.".into()); }
        self.submit(Action::Tail { platform, request: true, follow: true })?;
        self.following = true; self.surface.user_read_only = true; self.follow_paused = false;
        Ok(())
    }
    pub fn set_follow_paused(&mut self, paused: bool) { self.follow_paused = paused; }
    pub fn follow_tick(&mut self, platform: Arc<dyn LocalFileSystem>, request: bool) -> Result<(), String> {
        if self.following && !self.busy() && (request || self.tail_pending) && !self.tail_changed {
            self.submit(Action::Tail { platform, request, follow: !self.follow_paused })?;
        }
        Ok(())
    }
    pub fn unlock_follow(&mut self, confirmed: bool) -> Result<(), String> {
        if !confirmed { return Ok(()); }
        self.submit(Action::UnlockTail)
    }
    pub fn dirty(&self) -> bool {
        Some(self.snapshot.content_state) != self.saved_state
    }
    pub fn viewport_start(&self) -> TextOffset {
        TextOffset(self.viewport_start)
    }
    /// Canonical endpoints are independent of the displayed, possibly clipped range.
    pub fn global_selection(&self) -> (TextOffset, TextOffset) {
        let selection = if self.viewport_valid && self.surface.selection != self.projected_selection
            && self.surface.selection.anchor <= self.surface.snapshot.len() && self.surface.selection.caret <= self.surface.snapshot.len()
            && self.surface.snapshot.is_boundary(TextOffset(self.surface.selection.anchor)) && self.surface.snapshot.is_boundary(TextOffset(self.surface.selection.caret)) {
            Selection { anchor: self.navigation_anchor.unwrap_or(self.viewport_start + self.surface.selection.anchor), caret: self.viewport_start + self.surface.selection.caret }
        } else { self.global_selection };
        (TextOffset(selection.anchor), TextOffset(selection.caret))
    }
    fn sync_global_selection(&mut self) {
        let moved = self.surface.selection != self.projected_selection;
        let (anchor, caret) = self.global_selection();
        self.global_selection = Selection { anchor: anchor.0, caret: caret.0 };
        self.projected_selection = self.surface.selection;
        if moved { self.navigation_anchor = None; }
    }
    pub fn selection_fully_in_viewport(&self) -> bool {
        let (anchor, caret) = self.global_selection();
        let end = self.viewport_start.saturating_add(self.surface.snapshot.len());
        self.viewport_valid && anchor.0 >= self.viewport_start && caret.0 >= self.viewport_start && anchor.0 <= end && caret.0 <= end
            && self.surface.snapshot.is_boundary(TextOffset(anchor.0 - self.viewport_start)) && self.surface.snapshot.is_boundary(TextOffset(caret.0 - self.viewport_start))
    }
    pub fn caret_in_viewport(&self) -> bool {
        let (_, caret) = self.global_selection();
        self.viewport_valid && caret.0 >= self.viewport_start && caret.0 <= self.viewport_start.saturating_add(self.surface.snapshot.len())
            && self.surface.snapshot.is_boundary(TextOffset(caret.0 - self.viewport_start))
    }
    fn project_global_selection(&mut self) {
        let length = self.surface.snapshot.len();
        let local = |offset: usize| offset.saturating_sub(self.viewport_start).min(length);
        // Clipping affects decoration only; it never writes back into the canonical endpoints.
        self.projected_selection = Selection { anchor: local(self.global_selection.anchor), caret: local(self.global_selection.caret) };
        if !self.surface.snapshot.is_boundary(TextOffset(self.projected_selection.anchor)) || !self.surface.snapshot.is_boundary(TextOffset(self.projected_selection.caret)) {
            self.projected_selection = Selection::default();
        }
        self.surface.selection = self.projected_selection;
        self.surface.selections = self.projected_selection.into();
        if !self.caret_in_viewport() { self.surface.reveal_caret = false; }
    }
    pub fn selection_restore_status(&self, token: u64) -> SelectionRestoreStatus {
        if token == self.selection_token { self.selection_status.clone() } else { SelectionRestoreStatus::Superseded }
    }
    pub fn restore_global_selection(&mut self, anchor: TextOffset, caret: TextOffset, preserve_viewport: bool) -> Result<u64, String> {
        self.restore_global_selection_mode(anchor, caret, preserve_viewport, false, false)
    }
    fn restore_global_selection_mode(&mut self, anchor: TextOffset, caret: TextOffset, preserve_viewport: bool, snap_hit: bool, extend_hit: bool) -> Result<u64, String> {
        if self.pending.is_some() { return Err("Wait for the pending paged operation.".into()); }
        if anchor.0 > self.snapshot.len() || caret.0 > self.snapshot.len() { return Err("Selection exceeds the document.".into()); }
        let handle = self.read_handle(); let snapshot = self.snapshot.clone();
        let budget = self.budget.clone(); let cancellation = Cancellation::default(); let request_cancellation = cancellation.clone(); let notify = self.notify.clone();
        let historical = self.captured.is_some();
        let (sender, result) = mpsc::sync_channel(1);
        worker().try_send(Box::new(move || {
            let validation = (|| {
                if !historical { let _ = handle.original_store()?; }
                for offset in [anchor, caret] {
                    let mut request = handle.snapshot().begin_viewport(TextOffset(offset.0.saturating_sub(4)), 12, &budget).map_err(|e| format!("Selection validation: {e:?}"))?;
                    loop {
                        cancellation.check().map_err(|e| format!("Selection validation: {e:?}"))?;
                        match request.poll() {
                            WindowPoll::Ready(window) => {
                                let local = offset.0.checked_sub(window.range().start.0).ok_or("Selection endpoint is unavailable")?;
                                if local > window.text().len() || !window.text().is_char_boundary(local) { return Err("Selection endpoint is not a UTF-8 boundary".into()); }
                                break;
                            }
                            WindowPoll::Pending(ticket) => { let ready = if historical { handle.resolve_captured_page(ticket)? } else { handle.resolve_page(ticket)? }; if !ready { std::thread::yield_now(); } }
                            _ => return Err("Selection endpoint is unavailable".into()),
                        }
                    }
                }
                if !historical { let _ = handle.original_store()?; }
                let snapped = if snap_hit { crate::paged_navigation::snap_grapheme(&handle, caret.0, &budget, &cancellation)? } else { caret.0 };
                Ok(Selection { anchor: if snap_hit && !extend_hit { snapped } else { anchor.0 }, caret: snapped })
            })();
            let _ = sender.try_send(validation); notify();
        })).map_err(|_| "Paged worker queue is full; retry.".to_owned())?;
        self.selection_token = self.selection_token.wrapping_add(1);
        self.selection_status = SelectionRestoreStatus::Pending;
        self.selection_validation = Some(SelectionValidation { cancellation: request_cancellation, snapshot, selection: Selection { anchor: anchor.0, caret: caret.0 }, preserve_viewport, result });
        Ok(self.selection_token)
    }
    fn pump_selection_validation(&mut self) -> bool {
        let Some(pending) = &self.selection_validation else { return false; };
        let result = match pending.result.try_recv() { Ok(result) => result, Err(TryRecvError::Empty) => return false, Err(TryRecvError::Disconnected) => Err("Selection validation worker stopped".into()) };
        let pending = self.selection_validation.take().unwrap();
        let result = if !pending.snapshot.same_document(&self.snapshot) || pending.snapshot.content_state != self.snapshot.content_state { Err("Document changed while validating selection".into()) }
            else if self.captured.is_none() { result.and_then(|selection| self.read_handle().original_store().map(|_| selection)) } else { result };
        match result {
            Err(error) => { self.deferred_input = None; self.selection_status = SelectionRestoreStatus::Failed(error.clone()); self.error = Some(error); }
            Ok(selection) => {
                self.error = None;
                self.global_selection = selection; self.project_global_selection();
                if pending.preserve_viewport { self.surface.reveal_caret = false; }
                self.selection_status = SelectionRestoreStatus::Applied;
                if !pending.preserve_viewport {
                    let start = if pending.selection.anchor.abs_diff(pending.selection.caret) <= WINDOW.saturating_sub(8) { pending.selection.anchor.min(pending.selection.caret) } else { pending.selection.caret };
                    if let Err(error) = self.request_viewport(TextOffset(start.saturating_sub(4))) { self.deferred_input = None; self.selection_status = SelectionRestoreStatus::Failed(error.clone()); self.error = Some(error); }
                }
            }
        }
        true
    }
    /// Restore selection inside the current authoritative viewport without moving it.
    pub fn set_viewport_selection(&mut self, anchor: TextOffset, caret: TextOffset) -> Result<(), String> {
        if !self.viewport_ready() { return Err("Wait for the paged viewport to finish loading.".into()); }
        let local = |offset: TextOffset| -> Result<usize, String> {
            if offset.0 > self.snapshot.len() { return Err("Selection exceeds the document.".into()); }
            let value = offset.0.checked_sub(self.viewport_start).ok_or("Selection precedes the viewport")?;
            if value > self.surface.snapshot().len() || !self.surface.snapshot().is_boundary(TextOffset(value)) {
                return Err("Selection is outside the available UTF-8 viewport.".into());
            }
            Ok(value)
        };
        let anchor = local(anchor)?;
        let caret = local(caret)?;
        self.surface.selection.anchor = anchor;
        self.surface.selection.caret = caret;
        self.surface.selections = self.surface.selection.into();
        self.global_selection = Selection { anchor: self.viewport_start + anchor, caret: self.viewport_start + caret };
        self.projected_selection = self.surface.selection;
        Ok(())
    }
    pub fn restore_selection(
        &mut self,
        anchor: TextOffset,
        caret: TextOffset,
    ) -> Result<(), String> {
        self.restore_global_selection(anchor, caret, false).map(|_| ())
    }
    pub fn request_viewport(&mut self, start: TextOffset) -> Result<(), String> {
        let start = TextOffset(start.0.min(self.snapshot.len()));
        self.horizontal_anchor = None;
        self.prefetch.cancel();
        if self.busy() { self.queued_viewport = Some(start); return Ok(()); }
        self.submit(Action::Read(start.0))?;
        self.error = None;
        self.viewport_request = Some(start);
        Ok(())
    }
    fn pump_viewport_requests(&mut self) -> bool {
        self.prefetch.poll();
        if self.busy() { return false; }
        self.viewport_request = None;
        if let Some(offset) = self.queued_viewport.take() {
            if let Err(error) = self.request_viewport(offset) { self.error = Some(error); }
            return true;
        }
        if let Some(height) = self.bottom_scroll.take() && self.viewport_valid && self.error.is_none() { self.surface.scroll(f64::MAX, height); }
        if let Some((offset, screen_x)) = self.horizontal_anchor.take() && self.viewport_valid && self.error.is_none() {
            if let Some(local) = offset.checked_sub(self.viewport_start) && local <= self.surface.snapshot.len() {
                if let Err(error) = self.surface.restore_horizontal_anchor(TextOffset(local), screen_x) { self.error = Some(error); }
            }
        }
        if self.queued_horizontal != 0.0 {
            let delta = std::mem::take(&mut self.queued_horizontal); self.surface.scroll_horizontal(delta);
        }
        if self.requested_scroll.is_none() && self.queued_wheel.0 != 0.0 {
            let (delta, height) = std::mem::replace(&mut self.queued_wheel, (0.0, 0.0));
            if let Err(error) = self.scroll_viewport(delta, height) { self.error = Some(error); }
            return true;
        }
        false
    }
    pub fn save(
        &mut self,
        target: PathBuf,
        expected: Option<Fingerprint>,
        platform: Arc<dyn LocalFileSystem>,
    ) -> Result<(), String> {
        if self.surface.user_read_only {
            return Err("Document is read only.".into());
        }
        self.submit(Action::Save {
            copy_only: false,
            target,
            expected,
            platform,
        })
    }
    /// Export the captured document without changing its save identity or recovery.
    pub fn save_copy(&mut self, target: PathBuf, platform: Arc<dyn LocalFileSystem>) -> Result<(), String> {
        self.submit(Action::Save { copy_only: true, target, expected: None, platform })
    }
    pub fn require_save_as(&mut self){self.save_as_required=true;if let Ok(mut peer)=self.peer.lock(){peer.save_as_required=true;}}
    pub fn acknowledge_tracked_power(&mut self,receipt:&crate::TrackedEditReceipt,id:&str,args:&crate::power::consumer::Arguments)->Result<(),String> {
        let revision=receipt.terminal().ok_or("Edit is still pending")??;
        if receipt.captured_identity_token.0!=self.snapshot.identity_token().0 || revision!=self.snapshot.revision {return Err("Power receipt is not the current document revision".into());}
        if self.tracked_acknowledged.contains(&receipt.operation_id){return Ok(());}
        if self.tracked_acknowledged.len()==256{self.tracked_acknowledged.pop_front();}
        self.tracked_acknowledged.push_back(receipt.operation_id);
        self.surface.acknowledge_command((id.into(),args.clone()));
        Ok(())
    }
    pub fn set_streaming_quota(&mut self, quota:u64) {self.streaming_quota=quota;}
    pub fn apply_prepared_source(&mut self,source:&PagedSnapshot,prepared:bareline_document::paged::PreparedSourceTransaction)->Result<(),String> {
        if self.following || self.surface.user_read_only {return Err("Document is read-only".into());}
        if source.identity_token()!=self.snapshot.identity_token() || source.content_state!=self.snapshot.content_state || prepared.base_revision()!=source.revision {return Err("Prepared source changed".into());}
        self.submit(Action::Source(prepared,None))
    }
    pub fn apply_prepared_source_tracked(&mut self,source:&PagedSnapshot,prepared:bareline_document::paged::PreparedSourceTransaction)->Result<crate::TrackedEditReceipt,String> {
        if self.following || self.surface.user_read_only {return Err("Document is read-only".into());}
        if source.identity_token()!=self.snapshot.identity_token() || source.content_state!=self.snapshot.content_state || prepared.base_revision()!=source.revision {return Err("Prepared source changed".into());}
        let receipt=crate::TrackedEditReceipt::new(source.identity_token());
        self.submit(Action::Source(prepared,Some(crate::tracked_edit::TrackedEditCompletion(receipt.clone()))))?;
        Ok(receipt)
    }
    pub fn apply_prepared_tracked(&mut self,source:&PagedSnapshot,transaction:EditTransaction)->Result<crate::TrackedEditReceipt,String> {
        if self.following || self.surface.user_read_only {return Err("Document is read-only".into());}
        if source.identity_token()!=self.snapshot.identity_token() || source.content_state!=self.snapshot.content_state || transaction.base_revision!=source.revision {return Err("Replacement source changed".into());}
        let receipt=crate::TrackedEditReceipt::new(source.identity_token());
        self.submit(Action::Prepared(transaction,Some(crate::tracked_edit::TrackedEditCompletion(receipt.clone()))))?;
        Ok(receipt)
    }
    /// Apply one reviewed multi-edit transaction through the paged actor and recovery journal.
    pub fn apply_prepared(&mut self, source: &PagedSnapshot, transaction: EditTransaction) -> Result<(), String> {
        if self.following || self.surface.user_read_only { return Err("Document is read-only".into()); }
        if !source.same_document(&self.snapshot) || source.revision != self.snapshot.revision || source.content_state != self.snapshot.content_state || transaction.base_revision != source.revision {
            return Err("Replacement source changed; search again".into());
        }
        self.submit(Action::Prepared(transaction,None))
    }
    pub fn enqueue(&mut self, input: Input) {
        if (!self.paged_frame_state().ready || self.surface.horizontal_anchor_pending()) && matches!(input, Input::SetCaret(..)) {
            self.error = Some("Wait for the requested viewport before hit testing.".into()); return;
        }
        if self.surface.user_read_only
            && matches!(
                input,
                Input::Insert(_) | Input::Backspace | Input::Delete | Input::Undo | Input::Redo
            )
        {
            self.error = Some("Document is read only.".into());
            return;
        }
        if !self.viewport_valid
            && matches!(input, Input::Insert(_) | Input::Backspace | Input::Delete)
        {
            self.error = Some("Load an available viewport before editing.".into());
            return;
        }
        if self.busy() {
            self.error = Some("Wait for the pending page or edit.".into());
            return;
        }
        self.sync_global_selection();
        if let Input::SetCaret(local, extend) = input {
            if local > self.surface.snapshot.len() || !self.surface.snapshot.is_boundary(TextOffset(local)) { self.error = Some("Caret is outside the available viewport".into()); return; }
            let caret = self.viewport_start + local;
            self.global_selection = Selection { anchor: if extend { self.global_selection.anchor } else { caret }, caret };
            self.project_global_selection(); self.surface.acknowledge(Input::SetCaret(local, extend)); return;
        }
        if matches!(input, Input::SelectAll) {
            if let Err(error) = self.restore_global_selection(TextOffset(0), TextOffset(self.snapshot.len()), true) { self.error = Some(error); }
            return;
        }
        if matches!(input, Input::Left(false) | Input::Right(false)) && self.global_selection.anchor != self.global_selection.caret {
            let range = self.global_selection.range(); let offset = if matches!(input, Input::Left(_)) { range.start } else { range.end };
            if let Err(error) = self.restore_global_selection(TextOffset(offset), TextOffset(offset), false) { self.error = Some(error); }
            return;
        }
        let needs_caret = matches!(input, Input::Left(_) | Input::Right(_) | Input::Up(_) | Input::Down(_) | Input::Home(_) | Input::End(_)) || (matches!(input, Input::Backspace | Input::Delete) && self.global_selection.anchor == self.global_selection.caret);
        if needs_caret && !self.caret_in_viewport() {
            let (anchor, caret) = self.global_selection();
            match self.restore_global_selection(anchor, caret, false) { Ok(_) => self.deferred_input = Some(input), Err(error) => self.error = Some(error) }
            return;
        }
        let acknowledged = input.clone();
        let selected = self.global_selection.range();
        let action = match input {
            Input::Insert(insert) => Some(Action::Edit {
                range: TextOffset(selected.start)..TextOffset(selected.end),
                insert,
            }),
            Input::Backspace | Input::Delete => {
                let backward = matches!(input, Input::Backspace);
                let range = if !selected.is_empty() {
                    selected
                } else if backward {
                    self.surface
                        .previous_grapheme(self.surface.selection.caret)
                        .map(|start| self.viewport_start + start..self.global_selection.caret)
                        .unwrap_or(selected)
                } else {
                    self.surface
                        .next_grapheme(self.surface.selection.caret)
                        .map(|end| self.global_selection.caret..self.viewport_start + end)
                        .unwrap_or(selected)
                };
                Some(Action::Edit {
                    range: TextOffset(range.start)..TextOffset(range.end),
                    insert: String::new(),
                })
            }
            Input::Undo => Some(Action::Undo),
            Input::Redo => Some(Action::Redo),
            navigation => {
                if matches!(navigation, Input::Left(true) | Input::Right(true) | Input::Up(true) | Input::Down(true) | Input::Home(true) | Input::End(true)) { self.navigation_anchor = Some(self.global_selection.anchor); }
                self.surface.enqueue(navigation);
                None
            }
        };
        if let Some(action) = action {
            match self.submit(action) {
                Err(error) => self.error = Some(error),
                Ok(()) => self.pending_input = Some(acknowledged),
            }
        }
    }
    pub fn click(&mut self, backend: &impl bareline_renderer::TextBackend, point: bareline_renderer::Point, extend: bool) -> Result<(), bareline_renderer::LayoutError> {
        if !self.paged_frame_state().ready || self.surface.horizontal_anchor_pending() {
            self.error = Some("Wait for the requested viewport before hit testing.".into()); return Ok(());
        }
        if self.surface.composition.is_some() || point.x < crate::LEFT || point.y < self.surface.top() { return Ok(()); }
        let row = ((point.y - self.surface.top()) as f64 + self.surface.scroll_y) / self.surface.line_height() as f64;
        let line = self.surface.logical_line(row.floor() as usize);
        let Some(layout) = self.surface.layouts.get(&line) else { return Ok(()); };
        let hit = backend.hit_test(layout.id, bareline_renderer::Point {
            x: point.x - crate::LEFT + (self.surface.scroll_x - layout.x_origin) as f32,
            y: ((row - (self.surface.visual_line(line) + layout.row_origin) as f64) * self.surface.line_height() as f64) as f32 + layout.context_y,
        })?;
        let local = layout.start.saturating_add(hit.byte_offset).min(layout.end);
        let caret = TextOffset(self.viewport_start.saturating_add(local));
        let anchor = if extend { self.global_selection().0 } else { caret };
        if let Err(error) = self.restore_global_selection_mode(anchor, caret, true, true, extend) { self.error = Some(error); }
        Ok(())
    }
    fn submit(&mut self, action: Action) -> Result<(), String> {
        if self.busy() {
            return Err("A paged operation is already pending.".into());
        }
        self.sync_global_selection();
        let moves_selection = matches!(&action, Action::Edit { .. } | Action::Prepared(..) | Action::Source(..) | Action::Undo | Action::Redo | Action::Tail { follow: true, .. });
        let mapped_marks = match &action {
            Action::Prepared(transaction,_) => Some(self.search_marks.mapped(transaction)),
            Action::Edit { range, insert } => Some(self.search_marks.mapped(&EditTransaction { base_revision: self.snapshot.revision, edits: vec![Edit { range: range.clone(), insert: insert.clone() }] })),
            _ => None,
        };
        if let Some(captured) = self.captured.clone() {
            let displayed_snapshot = self.snapshot.clone();
            let Action::Read(start) = action else { return Err("This is a read-only captured generation.".into()); };
            let budget = self.budget.clone();
            let cancellation = self.cancellation.clone();
            let notify = self.notify.clone();
            let (sender, receiver) = mpsc::sync_channel(1);
            worker().try_send(Box::new(move || {
                let result = (|| {
                    let snapshot = displayed_snapshot;
                    let start = start.min(snapshot.len());
                    let mut request = snapshot.begin_viewport(TextOffset(start), WINDOW, &budget).map_err(|e| format!("{e:?}"))?;
                    let window = loop {
                        cancellation.check().map_err(|e| format!("{e:?}"))?;
                        match request.poll() {
                            WindowPoll::Ready(window) => break Ok(window),
                            WindowPoll::Pending(ticket) => { if !captured.resolve_captured_page(ticket)? { std::thread::yield_now(); } }
                            WindowPoll::Unavailable(reason) => break Err(format!("Captured source unavailable: {reason:?}")),
                            WindowPoll::InvalidUtf8 => break Err("Captured source contains invalid UTF-8".into()),
                            WindowPoll::Finished => break Err("Captured viewport already finished".into()),
                        }
                    };
                    Ok(Completed { selection: None, append_receipt: None, generation_owner: captured._generation.clone(), peer_epoch: 0, saved_state: Some(snapshot.content_state), save_as_required: false, following: false, tail_pending: false, source_changed: false, fingerprint: captured.fingerprint.clone(), can_undo: false, can_redo: false, snapshot, window, path: captured.path.clone(), caret: start, saved: None })
                })();
                let _ = sender.try_send(result); notify();
            })).map_err(|_| "Paged worker queue is full; retry.".to_owned())?;
            self.pending = Some(receiver);
            return Ok(());
        }
        let encoding_failure=self.encoding_failure.clone();
        let actor = self.actor.clone();
        let retired = self.retired.clone();
        let generation_owner = self.generation_owner.clone();
        let peer = self.peer.clone();
        let tail = self.tail.clone();
        let recovery = self.recovery.clone();
        let recovery_config = self.recovery_config.clone();
        let streaming_quota=self.streaming_quota;
        let recovery_status = self.recovery_status.clone();
        let failed_retirements = self.failed_retirements.clone();
        let budget = self.budget.clone();
        let cancellation = self.cancellation.clone();
        let notify = self.notify.clone();
        let revision = self.snapshot.revision;
        let current_start = self.viewport_start;
        let current_caret = self.global_selection.caret;
        let (sender, receiver) = mpsc::sync_channel(1);
        worker()
            .try_send(Box::new(move || {
                let result = (|| {
                    cancellation.check().map_err(|error| format!("{error:?}"))?;
                    let mut opened = actor.lock().map_err(|_| "Paged actor stopped.")?;
                    let mut tail = tail.lock().map_err(|_| "Tail actor stopped.")?;
                    if tail.is_some() && matches!(&action, Action::Edit { .. } | Action::Prepared(..) | Action::Source(..) | Action::Metadata(_) | Action::Undo | Action::Redo | Action::Save { .. }) {
                        return Err("Unlock and capture a fixed generation before editing or saving monitored content.".into());
                    }
                    let saved_state = peer.lock().map_err(|_| "Peer state stopped")?.saved_state;
                    if opened.transcoded.document.snapshot().revision != revision && !matches!(&action, Action::Read(_)) {
                        return Err("Document changed; retry the operation.".into());
                    }
                    let mut start = current_start;
                    let mut caret = current_caret;
                    let mut saved = None;
                    let mut committed_selection=None;
                    let baseline = opened.transcoded.document.snapshot();
                    let previous_path = opened.path.clone();
                    let previous_fingerprint = opened.fingerprint.clone();
                    let previously_following = tail.is_some();
                    let mut retry_recovery = false;
                    let mut recovery_edits = Vec::new();
                    let mut streaming_protected=false;
                    match action {
                        Action::Tail { platform, request, follow } => {
                            if tail.is_none() {
                                *tail = Some(bareline_file_io::tail::TailSession::new(&opened, platform, budget.clone(), cancellation.clone()).map_err(|e| format!("{e:?}"))?);
                                if follow { caret = opened.transcoded.document.snapshot().len(); start = caret.saturating_sub(WINDOW / 2); }
                            }
                            let session = tail.as_mut().unwrap();
                            if request { session.request(&opened.path).map_err(|e| format!("{e:?}"))?; }
                            if session.step(&mut opened).map_err(|e| format!("{e:?}"))? && follow {
                                caret = opened.transcoded.document.snapshot().len(); start = caret.saturating_sub(WINDOW / 2);
                            }
                        }
                        Action::UnlockTail => {
                            let session = tail.as_ref().ok_or("Monitoring is not active")?;
                            let fixed = session.freeze(&opened).map_err(|e| format!("Cannot capture fixed generation: {e:?}"))?;
                            let previous = std::mem::replace(&mut *opened, fixed);
                            let old_owner = std::mem::replace(&mut *generation_owner.lock().map_err(|_| "Generation owner failed")?, Arc::new(()));
                            let mut retained = retired.lock().map_err(|_| "Captured generation owner failed")?;
                            retained.retain(|generation| Arc::strong_count(&generation.owner) > 1);
                            retained.push(RetiredGeneration { owner: old_owner, opened: previous, tail: tail.take() });
                            saved = Some(opened.fingerprint.clone());
                        }
                        Action::RetryRecovery => {
                            if let Some((_, platform)) = &recovery_config {
                                let paths = std::mem::take(
                                    &mut *failed_retirements
                                        .lock()
                                        .map_err(|_| "Recovery retirement state stopped")?,
                                );
                                let mut failed = Vec::new();
                                let mut failure = None;
                                for path in paths {
                                    if let Err(error) = bareline_file_io::recovery::discard(
                                        &path,
                                        platform.as_ref(),
                                    ) {
                                        failed.push(path);
                                        failure = Some(error.to_string());
                                    }
                                }
                                *failed_retirements
                                    .lock()
                                    .map_err(|_| "Recovery retirement state stopped")? = failed;
                                if let Some(error) = failure {
                                    if let Ok(mut status) = recovery_status.lock() {
                                        status.error = Some(error.clone());
                                    }
                                    return Err(error);
                                }
                            }
                            if Some(baseline.content_state) == saved_state {
                                if let Ok(mut status) = recovery_status.lock() {
                                    *status = Default::default();
                                }
                            } else {
                                retry_recovery = true;
                                recovery_edits.push(bareline_file_io::recovery::RecoveryEdit {
                                    offset: 0,
                                    removed: Vec::new(),
                                    inserted: Vec::new(),
                                });
                            }
                        }
                        Action::Read(offset) => {
                            start = offset;
                            caret = offset;
                        }
                        Action::Metadata(metadata) => { let revision=opened.transcoded.document.snapshot().revision; opened.transcoded.document.apply_metadata(revision,metadata).map_err(|error|format!("{error:?}"))?; }
                        Action::Source(prepared,completion) => {
                            ensure_stream_recovery(&opened,&baseline,&recovery_config,&recovery,&recovery_status,&notify)?;
                            let lease=opened.transcoded.document.lease_source_transaction(prepared).map_err(|e|format!("{e:?}"))?;
                            if let Some(journal)=recovery.lock().map_err(|_|"Recovery actor stopped")?.as_mut() {journal.append_sources(lease.snapshot(),lease.edits(),streaming_quota)?;}
                            if let Some(selection)=lease.metadata().after.first(){caret=selection.caret.0;start=caret.saturating_sub(WINDOW/2);committed_selection=Some(Selection{anchor:selection.anchor.0,caret});}
                            let revision=lease.publish();
                            if let Some(completion)=completion {completion.complete_once(Ok(revision));}
                            streaming_protected=true;
                        }
                        Action::Prepared(transaction,completion) => {
                            if tail.is_some() { return Err("Monitoring document is read-only".into()); }
                            let snapshot = opened.transcoded.document.snapshot();
                            if transaction.base_revision != snapshot.revision { return Err("Replacement source changed".into()); }
                            let mut windows = Vec::new();
                            let mut used = 0usize;
                            for edit in &transaction.edits {
                                cancellation.check().map_err(|error| format!("{error:?}"))?;
                                let length = edit.range.end.0.checked_sub(edit.range.start.0).ok_or("Invalid replacement range")?;
                                used = used.checked_add(length).and_then(|value| value.checked_add(edit.insert.len() + 128)).ok_or("Replacement staging limit")?;
                                if used > 16 * 1024 * 1024 { return Err("Replacement staging limit".into()); }
                                let window = read_window(&mut opened, &mut tail, &snapshot, edit.range.start.0.saturating_sub(4), length + 8, &budget, &cancellation)?;
                                let from = edit.range.start.0.checked_sub(window.range().start.0).ok_or("Invalid replacement boundary")?;
                                let to = edit.range.end.0.checked_sub(window.range().start.0).ok_or("Invalid replacement boundary")?;
                                let removed = window.text().get(from..to).ok_or("Invalid replacement boundary")?;
                                recovery_edits.push(bareline_file_io::recovery::RecoveryEdit { offset: edit.range.start.0 as u64, removed: removed.as_bytes().to_vec(), inserted: edit.insert.as_bytes().to_vec() });
                                windows.push(window);
                            }
                            let revision=opened.transcoded.document.apply_materialized(transaction, &windows).map_err(|error| format!("{error:?}"))?;
                            if let Some(completion)=completion {completion.complete_once(Ok(revision));}
                            caret = caret.min(opened.transcoded.document.snapshot().len());
                            start = start.min(caret);
                        }
                        Action::Edit { range, insert } => {
                            if insert.len() > WINDOW || range.end.0 - range.start.0 > WINDOW {
                                return Err("Edit exceeds the bounded viewport budget.".into());
                            }
                            let snapshot = opened.transcoded.document.snapshot();
                            let window = read_window(
                                &mut opened,
                                &mut tail,
                                &snapshot,
                                range.start.0.saturating_sub(4),
                                (range.end.0 - range.start.0 + 8).min(WINDOW + 8),
                                &budget,
                                &cancellation,
                            )?;
                            let local_start = range.start.0 - window.range().start.0;
                            let local_end = range.end.0 - window.range().start.0;
                            recovery_edits.push(bareline_file_io::recovery::RecoveryEdit {
                                offset: range.start.0 as u64,
                                removed: window.text().as_bytes()[local_start..local_end].to_vec(),
                                inserted: insert.as_bytes().to_vec(),
                            });
                            caret = range.start.0 + insert.len();
                            opened
                                .transcoded
                                .document
                                .apply_materialized(
                                    EditTransaction {
                                        base_revision: revision,
                                        edits: vec![Edit { range, insert }],
                                    },
                                    &[window],
                                )
                                .map_err(|error| format!("{error:?}"))?;
                            start = start.min(caret);
                        }
                        Action::Undo | Action::Redo => {
                            let undo=matches!(action,Action::Undo);
                            ensure_stream_recovery(&opened,&baseline,&recovery_config,&recovery,&recovery_status,&notify)?;
                            let prepared=opened.transcoded.document.prepare_source_history(undo,&budget).map_err(|e|format!("{e:?}"))?;
                            let lease=opened.transcoded.document.lease_source_history(prepared).map_err(|e|format!("{e:?}"))?;
                            if let Some(journal)=recovery.lock().map_err(|_|"Recovery actor stopped")?.as_mut() {journal.append_source_history(lease.snapshot(),lease.edits(),streaming_quota)?;}
                            let selections=if undo{&lease.metadata().before}else{&lease.metadata().after};
                            if let Some(selection)=selections.first(){caret=selection.caret.0;start=caret.saturating_sub(WINDOW/2);committed_selection=Some(Selection{anchor:selection.anchor.0,caret});}
                            lease.publish();
                            streaming_protected=true;
                        }
                        Action::Save {
                            copy_only,
                            target,
                            expected,
                            platform,
                        } => {
                            let snapshot = opened.transcoded.document.snapshot();
                            let _copy_source = if copy_only && opened.recovery_origin.is_none() { bareline_file_io::lifecycle::guard_copy_source(&opened.path,&target,platform.as_ref()).map_err(|e|format!("{e:?}"))? } else { None };
                            let policy = PagedSavePolicy {
                                store: opened.transcoded.store.clone(),
                                generation: opened.transcoded.source.source().generation(),
                                encoding: opened.transcoded.store.state.save_target,
                                bom: opened.transcoded.store.state.bom,
                            };
                            if let Ok(mut slot)=encoding_failure.lock(){*slot=None;}
                            let result = save_paged_cancellable(
                                snapshot,
                                &target,
                                expected.as_ref(),
                                &policy,
                                platform.as_ref(),
                                &cancellation,
                            )
                            .map_err(|error| { if let bareline_file_io::lifecycle::FileError::EncodingAt(failure)=&error {if let Ok(mut slot)=encoding_failure.lock(){*slot=Some(failure.clone());}} format!("{error:?}") })?;
                            if let Ok(mut slot)=encoding_failure.lock(){*slot=None;}
                            if !copy_only {
                                opened.fingerprint = result.fingerprint.clone();
                                opened.path = target;
                                saved = Some(result.fingerprint);
                            }
                        }
                    }
                    let snapshot = opened.transcoded.document.snapshot();
                    let (peer_epoch, shared_saved_state, shared_save_as_required) = {
                        let mut peer = peer.lock().map_err(|_| "Peer state stopped")?;
                        if saved.is_some() { peer.saved_state = Some(snapshot.content_state); peer.save_as_required = false; }
                        if tail.is_some() { peer.saved_state = Some(snapshot.content_state); }
                        if snapshot.content_state != baseline.content_state || snapshot.revision != baseline.revision || opened.path != previous_path || opened.fingerprint != previous_fingerprint || saved.is_some() || previously_following != tail.is_some() { peer.epoch = peer.epoch.wrapping_add(1); }
                        (peer.epoch, peer.saved_state, peer.save_as_required)
                    };
                    if saved.is_some() {
                        let retired = recovery
                            .lock()
                            .map_err(|_| "Recovery actor stopped")?
                            .take();
                        if let Some(retired) = retired {
                            let path = retired.directory().to_path_buf();
                            match retired.retire() {
                                Ok(_) => {
                                    if let Ok(mut status) = recovery_status.lock() {
                                        *status = Default::default();
                                    }
                                }
                                Err(error) => {
                                    failed_retirements
                                        .lock()
                                        .map_err(|_| "Recovery retirement state stopped")?
                                        .push(path);
                                    if let Ok(mut status) = recovery_status.lock() {
                                        status.error = Some(error);
                                    }
                                }
                            }
                        }
                    }
                    if !streaming_protected && (!recovery_edits.is_empty() || snapshot.metadata() != baseline.metadata()) {
                        if let Some((root, platform)) = recovery_config {
                            let protected = (|| -> Result<(), String> {
                                let mut journal =
                                    recovery.lock().map_err(|_| "Recovery actor stopped")?;
                                if retry_recovery {
                                    *journal = None;
                                }
                                if journal.is_none() {
                                    *journal = Some(
                                        bareline_file_io::paged_recovery::PagedRecovery::create(
                                            &root,
                                            opened.transcoded.store.clone(),
                                            opened.recovery_origin.is_none().then(||opened.path.clone()),
                                            baseline,
                                            platform,
                                            recovery_status.clone(),
                                            notify.clone(),
                                        )?,
                                    );
                                }
                                journal.as_mut().unwrap().append(&snapshot, &recovery_edits)
                            })();
                            if let Err(error) = protected {
                                if let Ok(mut status) = recovery_status.lock() {
                                    status.error = Some(error);
                                }
                            }
                        }
                    }
                    start = start.min(snapshot.len());
                    caret = caret.min(snapshot.len());
                    let window = read_window(
                        &mut opened,
                        &mut tail,
                        &snapshot,
                        start,
                        WINDOW,
                        &budget,
                        &cancellation,
                    );
                    Ok(Completed {
                        selection: committed_selection,
                        append_receipt: tail.as_ref().and_then(|tail| tail.append_receipt()),
                        generation_owner: generation_owner.lock().map_err(|_| "Generation owner failed")?.clone(),
                        peer_epoch, saved_state: shared_saved_state, save_as_required: shared_save_as_required,
                        following: tail.is_some(),
                        tail_pending: tail.as_ref().is_some_and(|s| s.pending()),
                        source_changed: tail.as_ref().is_some_and(|s| s.source_changed),
                        fingerprint: opened.fingerprint.clone(),
                        can_undo: opened.transcoded.document.can_undo(),
                        can_redo: opened.transcoded.document.can_redo(),
                        snapshot,
                        window,
                        path: opened.path.clone(),
                        caret,
                        saved,
                    })
                })();
                let _ = sender.try_send(result);
                notify();
            }))
            .map_err(|_| "Paged worker queue is full; retry.".to_owned())?;
        self.pending = Some(receiver);
        self.pending_moves_selection = moves_selection;
        self.pending_marks = mapped_marks;
        Ok(())
    }
    pub fn pump(&mut self) -> bool {
        self.sync_global_selection();
        let viewport_changed = self.pump_viewport_requests();
        let selection_changed = self.pump_selection_validation();
        let navigation_changed = self.pump_navigation();
        let Some(receiver) = &self.pending else {
            self.ensure_viewport_mapping();
            return self.refresh_peer() || navigation_changed || selection_changed || viewport_changed;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return selection_changed || navigation_changed || viewport_changed,
            Err(TryRecvError::Disconnected) => Err("Paged worker stopped.".into()),
        };
        self.pending = None;
        let moves_selection = std::mem::take(&mut self.pending_moves_selection);
        match result {
            Ok(completed) => {
                self.viewport_mapping = None;
                self.fold_viewport_line = None;
                if completed.snapshot.content_state != self.snapshot.content_state {
                    self.global_folds.clear(); self.global_fold_overrides.clear(); self.global_fold_state.unfold_all(); self.global_fold_initialized = false;
                }
                if completed.snapshot.content_state != self.snapshot.content_state {
                    if let Some(marks) = self.pending_marks.take() { self.search_marks = marks; } else { self.search_marks.clear(None); }
                } else { self.pending_marks = None; }
                self.append_receipt = completed.append_receipt;
                self.peer_epoch = completed.peer_epoch;
                self.view_generation = completed.generation_owner;
                self.saved_state = completed.saved_state;
                self.save_as_required = completed.save_as_required;
                self.fingerprint = completed.fingerprint.clone();
                let was_following = self.following;
                self.following = completed.following;
                if self.following { self.surface.user_read_only = true; }
                self.tail_pending = completed.tail_pending;
                self.tail_changed = completed.source_changed;
                if was_following && !self.following { self.surface.user_read_only = false; }
                if self.following { self.saved_state = Some(completed.snapshot.content_state); self.fingerprint = completed.fingerprint; }
                self.can_undo = completed.can_undo;
                self.can_redo = completed.can_redo;
                if let Some(input) = self.pending_input.take() {
                    self.surface.acknowledge(input);
                }
                self.viewport_valid = false;
                self.surface.set_eol_status_override(Some("Computing".into()));
                self.snapshot = completed.snapshot;
                self.path = completed.path;
                if let Some(fingerprint) = completed.saved {
                    self.fingerprint = fingerprint;
                    self.save_as_required = false;
                    self.saved_state = Some(self.snapshot.content_state);
                }
                let window = match completed.window {
                    Ok(window) => window,
                    Err(error) => {
                        self.error = Some(error);
                        return true;
                    }
                };
                let displayed = (|| {
                    let mut builder = DocumentBuilder::new(self.budget.clone(), Budget::new(0))?;
                    builder.append(window.text())?;
                    Ok::<_, bareline_document::Error>(builder.prefix())
                })();
                match displayed {
                    Ok(snapshot) => {
                        self.viewport_valid = true;
                        self.viewport_start = window.range().start.0;
                        self.surface.snapshot = snapshot;
                        self.surface.layout_revision = None;
                        self.surface.scroll_y = 0.0;
                        if moves_selection {
                            self.global_selection = completed.selection.unwrap_or(Selection { anchor: completed.caret, caret: completed.caret });
                        }
                        self.project_global_selection();
                        if let Some((mapping, fraction, x)) = self.pending_scroll_mapping.take() && mapping.offset == self.viewport_start {
                            self.viewport_mapping = Some(mapping); self.fold_viewport_line = usize::try_from(mapping.line).ok(); self.surface.set_logical_scroll(0, fraction, x);
                        }
                        let _ = self.project_global_spacers();
                        self.project_global_folds();
                        self.project_search_marks();
                        self.surface.error = Some(format!(
                            "Paged · bytes {}–{} of {} · Lines: indexing…",
                            self.viewport_start,
                            window.range().end.0,
                            self.snapshot.len()
                        ));
                        self.error = None;
                    }
                    Err(error) => self.error = Some(format!("Viewport unavailable: {error:?}")),
                }
            }
            Err(error) => {
                self.pending_marks = None;
                self.pending_input = None;
                self.error = Some(error);
            }
        }
        self.ensure_viewport_mapping();
        self.pump_viewport_requests();
        if !self.busy() && let Some(input) = self.deferred_input.take() { self.enqueue(input); }
        true
    }
}
impl Drop for PagedEditorSurface {
    fn drop(&mut self) {
        if Arc::strong_count(&self.views) == 1 { self.cancellation.cancel(); }
    }
}
fn read_window(
    opened: &mut PagedOpened,
    tail: &mut Option<bareline_file_io::tail::TailSession>,
    snapshot: &PagedSnapshot,
    start: usize,
    count: usize,
    budget: &Budget,
    cancellation: &Cancellation,
) -> Result<TextWindow, String> {
    let mut request = snapshot
        .begin_viewport(TextOffset(start), count, budget)
        .map_err(|error| format!("{error:?}"))?;
    loop {
        cancellation.check().map_err(|error| format!("{error:?}"))?;
        match request.poll() {
            WindowPoll::Ready(window) => return Ok(window),
            WindowPoll::Pending(ticket) => {
                let handled = match tail.as_mut() { Some(tail) => tail.read_page(ticket).map_err(|e| format!("{e:?}"))?, None => false };
                if !handled { opened.transcoded.source.read_page(ticket).map_err(|error| format!("{error:?}"))?; }
            }
            WindowPoll::Unavailable(reason) => {
                return Err(format!("Source unavailable: {reason:?}"));
            }
            WindowPoll::InvalidUtf8 => {
                return Err("Source contains invalid UTF-8; interpret its encoding again.".into());
            }
            WindowPoll::Finished => return Err("Viewport request already finished.".into()),
        }
    }
}

#[cfg(test)]
mod peer_tests {
    use super::*;
    use std::{fs::File, path::Path, time::{Duration, Instant}};
    struct Platform;
    impl LocalFileSystem for Platform {
        fn validate_target(&self, _: &Path) -> std::io::Result<()> { Ok(()) }
        fn available_space(&self, _: &Path) -> std::io::Result<u64> { Ok(u64::MAX) }
        fn guard_directory(&self, _: &Path) -> std::io::Result<Arc<dyn Send + Sync>> { Ok(Arc::new(())) }
        fn open_sealed_read(&self, path: &Path) -> std::io::Result<File> { File::open(path) }
        fn identity(&self, file: &File) -> std::io::Result<bareline_platform::FileIdentity> {
            let metadata = file.metadata()?;
            Ok(bareline_platform::FileIdentity { volume: 1, file: 1, length: metadata.len(), modified: metadata.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64 })
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> std::io::Result<()> { Err(std::io::Error::other("unused")) }
    }
    fn drain(view: &mut PagedEditorSurface) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            view.pump();
            if !view.busy() && !view.navigation.is_pending() && view.navigation_ready.is_none() { assert!(view.error.is_none(), "{:?}", view.error); break; }
            assert!(Instant::now() < deadline, "paged worker timed out");
            std::thread::yield_now();
        }
    }
    #[test]
    fn global_scroll_crosses_windows_and_keeps_midline_coordinates_exact() {
        use bareline_file_io::{codecs::disk::DiskOptions, lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded}, source::SourceOptions};
        let root = std::env::temp_dir().join(format!("bareline-paged-global-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("source.txt"); std::fs::write(&path, "abc\n".repeat(40000)).unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(PagedOpenRequest { path, bytes: budget.clone(), history: Budget::new(1024 * 1024), cache: root.clone(), options: DiskOptions { temp_quota_bytes: 4 * 1024 * 1024, interpret: None }, source_options: SourceOptions { resident_max_bytes: 0, ..SourceOptions::default() } }, Arc::new(Platform), Cancellation::default(), |_| {}) else { panic!("open failed") };
        let mut view = PagedEditorSurface::new(opened, budget, Arc::new(|| {})).unwrap(); drain(&mut view);
        let initial_selection = view.global_selection();
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        view.surface.draw(&mut backend, 800.0, 400.0, &mut Vec::new()).unwrap();
        let point = bareline_renderer::Point { x: crate::LEFT + 10.0, y: view.surface.top() + 2.0 };
        view.click(&backend, point, false).unwrap(); drain(&mut view);
        let clicked = view.global_selection();
        assert_eq!(clicked.0, clicked.1); assert!(clicked.1.0 > 0);
        view.click(&backend, bareline_renderer::Point { x: crate::LEFT + 25.0, ..point }, true).unwrap(); drain(&mut view);
        assert_eq!(view.global_selection().0, clicked.0);
        assert!(view.global_selection().1.0 > clicked.1.0);
        view.restore_global_selection(initial_selection.0, initial_selection.1, true).unwrap(); drain(&mut view);
        view.request_byte_scroll_in_view(0.5, 400.0).unwrap();
        assert!(!view.paged_frame_state().ready);
        view.click(&backend, point, false).unwrap();
        assert_eq!(view.global_selection(), initial_selection);
        view.request_byte_scroll_in_view(0.75, 400.0).unwrap();
        assert!(view.paged_frame_state().requested.unwrap().0 > 80_000);
        view.scroll_viewport(0.25, 400.0).unwrap();
        view.scroll_viewport(0.5, 400.0).unwrap();
        assert_eq!(view.queued_wheel.0, 0.75);
        drain(&mut view);
        assert!(view.paged_frame_state().ready);
        assert_eq!(view.global_selection(), initial_selection);
        assert_eq!(view.queued_wheel.0, 0.0);
        assert_eq!(view.surface.scroll_y, 0.75);
        let metrics = view.paged_scroll_metrics(400.0);
        assert_eq!(metrics.total, 160_000.0);
        assert!(!metrics.lines_known);
        assert!(metrics.offset > 80_000.0 && metrics.viewport < metrics.total);
        view.request_byte_scroll_in_view(1.0, 400.0).unwrap(); drain(&mut view);
        assert_eq!(view.viewport_start().0 + view.surface.snapshot().len(), view.snapshot().len());
        assert!(view.surface.scroll_y > 0.0);
        view.request_global_scroll(30000, 0.25, 17.0).unwrap();
        assert_eq!(view.global_logical_scroll(), GlobalScrollPosition::Pending);
        drain(&mut view);
        assert_eq!(view.global_logical_scroll(), GlobalScrollPosition::Ready(30000, 0.25, 17.0));
        view.request_viewport(TextOffset(120002)).unwrap();
        assert_eq!(view.global_logical_scroll(), GlobalScrollPosition::Pending);
        drain(&mut view);
        assert_eq!(view.viewport_first_global_line(), Some(30000));
        assert_eq!(view.viewport_first_line_start(), Some(TextOffset(120000)));
        let viewport = view.viewport_start();
        view.surface.scroll_y = 37.5; view.surface.scroll_x = 19.0;
        let token = view.restore_global_selection(TextOffset(2), TextOffset(9), true).unwrap();
        assert_eq!(view.selection_restore_status(token), SelectionRestoreStatus::Pending);
        drain(&mut view);
        assert_eq!(view.selection_restore_status(token), SelectionRestoreStatus::Applied);
        assert_eq!(view.global_selection(), (TextOffset(2), TextOffset(9)));
        assert_eq!(view.viewport_start(), viewport);
        assert_eq!((view.surface.scroll_y, view.surface.scroll_x), (37.5, 19.0));
        assert!(!view.caret_in_viewport());
        let mut peer = view.clone_view().unwrap(); drain(&mut peer);
        assert_eq!(peer.global_selection(), (TextOffset(2), TextOffset(9)));
        assert_eq!(peer.viewport_start(), viewport);
        drop(peer);
        view.set_known_global_folds(vec![bareline_syntax::folding::Fold { header: 30001, end: 30003, level: 1 }], 0, false, 30000).unwrap();
        assert!(view.global_fold_state.collapsed.is_empty());
        view.fold_all_known(1);
        assert!(view.global_fold_state.collapsed.contains(&30001));
        view.set_global_spacers(&[(30002, 3)]).unwrap();
        view.enqueue(Input::Insert("€".into())); drain(&mut view);
        assert_eq!(view.global_selection(), (TextOffset(5), TextOffset(5)));
        let handle = view.read_handle();
        let mut read = handle.snapshot().begin_read(TextOffset(0)..TextOffset(8), 16, &view.budget).unwrap();
        loop { match read.poll() {
            WindowPoll::Ready(window) => { assert_eq!(window.text(), "ab€bc\n"); break; }
            WindowPoll::Pending(ticket) => { handle.resolve_page(ticket).unwrap(); }
            _ => panic!("edited prefix unavailable"),
        } }
        let before = view.global_selection(); let before_viewport = view.viewport_start();
        let superseded = view.restore_global_selection(TextOffset(2), TextOffset(5), true).unwrap();
        let invalid = view.restore_global_selection(TextOffset(3), TextOffset(5), true).unwrap();
        assert_eq!(view.selection_restore_status(superseded), SelectionRestoreStatus::Superseded);
        let deadline = Instant::now() + Duration::from_secs(10);
        while view.busy() { view.pump(); assert!(Instant::now() < deadline); std::thread::yield_now(); }
        assert!(matches!(view.selection_restore_status(invalid), SelectionRestoreStatus::Failed(_)));
        assert_eq!(view.global_selection(), before); assert_eq!(view.viewport_start(), before_viewport);
        assert!(view.restore_global_selection(TextOffset(view.snapshot().len() + 1), TextOffset(0), true).is_err());
        drop(handle); drop(read);
        let entire = view.snapshot().len();
        view.restore_global_selection(TextOffset(0), TextOffset(entire), true).unwrap(); drain(&mut view);
        let content = view.snapshot().content_state;
        view.enqueue(Input::Delete);
        let deadline = Instant::now() + Duration::from_secs(10);
        while view.busy() { view.pump(); assert!(Instant::now() < deadline); std::thread::yield_now(); }
        assert_eq!(view.snapshot().content_state, content);
        assert_eq!(view.global_selection(), (TextOffset(0), TextOffset(entire)));
        assert!(view.error.as_ref().is_some_and(|error| error.contains("bounded viewport")));
        drop(view); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn horizontal_paged_line_crosses_source_windows_with_a_shaped_anchor() {
        use bareline_file_io::{codecs::disk::DiskOptions, lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded}, source::SourceOptions};
        let root = std::env::temp_dir().join(format!("bareline-paged-horizontal-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("line.txt"); std::fs::write(&path, "x".repeat(WINDOW * 4)).unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(PagedOpenRequest { path, bytes: budget.clone(), history: Budget::new(1024 * 1024), cache: root.clone(), options: DiskOptions { temp_quota_bytes: 1024 * 1024, interpret: None }, source_options: SourceOptions { resident_max_bytes: 0, ..SourceOptions::default() } }, Arc::new(Platform), Cancellation::default(), |_| {}) else { panic!("open failed") };
        let mut view = PagedEditorSurface::new(opened, budget, Arc::new(|| {})).unwrap(); drain(&mut view);
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        view.scroll_horizontal(1_000_000.0);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            view.pump();
            view.surface.draw(&mut backend, 800.0, 400.0, &mut Vec::new()).unwrap();
            view.refine_horizontal_viewport(&backend, 800.0).unwrap();
            if view.viewport_start().0 > 0 && view.paged_frame_state().ready && !view.surface.horizontal_anchor_pending() { break; }
            assert!(Instant::now() < deadline, "horizontal paged window did not advance: {:?}", view.error);
            std::thread::yield_now();
        }
        assert_eq!(view.global_selection(), (TextOffset(0), TextOffset(0)));
        assert!(view.surface.snapshot().len() <= WINDOW);
        drain(&mut view);
        drop(view); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn peer_owns_full_document_and_survives_other_view_close() {
        use bareline_file_io::{codecs::disk::DiskOptions, lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded}, source::SourceOptions};
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!("bareline-paged-peer-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("source.txt"); std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(PagedOpenRequest { path, bytes: budget.clone(), history: Budget::new(1024 * 1024), cache: root.clone(), options: DiskOptions { temp_quota_bytes: 1024 * 1024, interpret: None }, source_options: SourceOptions { resident_max_bytes: 0, ..SourceOptions::default() } }, Arc::new(Platform), Cancellation::default(), |_| {}) else { panic!("open failed") };
        let mut first = PagedEditorSurface::new(opened, budget, Arc::new(|| {})).unwrap(); drain(&mut first);
        let saved = first.read_handle();
        let mut second = first.clone_view().unwrap(); drain(&mut second);
        assert!(first.snapshot().same_document(second.snapshot()));
        second.enqueue(Input::Insert("peer ".into())); drain(&mut second);
        assert!(first.refresh_peer()); drain(&mut first);
        assert_eq!(first.snapshot().content_state, second.snapshot().content_state);
        assert!(first.dirty() && second.dirty());
        assert_eq!(first.surface.snapshot.len(), second.surface.snapshot.len());
        let mut captured = first.clone_captured_view(&saved).unwrap(); drain(&mut captured);
        assert!(captured.surface.user_read_only);
        assert_eq!(captured.snapshot().content_state, saved.snapshot().content_state);
        assert_eq!(captured.surface.snapshot.len(), 14);
        drop(first);
        second.enqueue(Input::Insert("alive ".into())); drain(&mut second);
        assert!(second.surface.snapshot.len() > 14);
        drop(second);
        captured.request_viewport(TextOffset(0)).unwrap(); drain(&mut captured);
        assert_eq!(captured.surface.snapshot.len(), 14);
        drop(captured); drop(saved); std::fs::remove_dir_all(root).unwrap();
    }
}

/// Materialize the complete bounded transaction before changing document/history.
/// Keep the returned reservation alive until the journal attempt has completed.
fn ensure_stream_recovery(opened:&PagedOpened,baseline:&PagedSnapshot,config:&Option<(PathBuf,Arc<dyn LocalFileSystem>)>,recovery:&Arc<Mutex<Option<bareline_file_io::paged_recovery::PagedRecovery>>>,status:&Arc<Mutex<bareline_file_io::paged_recovery::PagedRecoveryStatus>>,notify:&Arc<dyn Fn()+Send+Sync>)->Result<(),String> {
    let Some((root,platform))=config else{return Ok(());};
    let mut journal=recovery.lock().map_err(|_|"Recovery actor stopped")?;
    if journal.is_none(){*journal=Some(bareline_file_io::paged_recovery::PagedRecovery::create(root,opened.transcoded.store.clone(),opened.recovery_origin.is_none().then(||opened.path.clone()),baseline.clone(),platform.clone(),status.clone(),notify.clone())?);}
    Ok(())
}
