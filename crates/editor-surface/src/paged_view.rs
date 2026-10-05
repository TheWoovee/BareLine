// SPDX-License-Identifier: MPL-2.0
//! Actual paged editing controller. File I/O and document mutation run on one bounded
//! shared worker; the UI renders an explicitly incomplete, bounded viewport adapter.
use crate::{EditorSurface, Input, Selection};
use bareline_document::{
    Budget, ContentStateId, DocumentBuilder, Edit, EditTransaction, TextOffset,
    paged::{PagedSnapshot, TextWindow, WindowPoll},
};
/// Compatibility export for the immutable read capability owned and implemented
/// by `bareline-file-io`; editor-surface adds no persistence authority to it.
pub use bareline_file_io::paged_service::PagedReadHandle;
use bareline_file_io::{
    cancellation::Cancellation,
    lifecycle::{Fingerprint, PagedOpened},
    paged_service::{PagedLifecycleError, PagedSession},
};
use bareline_platform::{
    LocalFileSystem,
    executor::{BoundedExecutor, SubmitError, WorkKind},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
};
const WINDOW: usize = 64 * 1024;
/// Number of shared I/O threads. A long-running job (e.g. a full-file Save) occupies
/// one thread, so other paged documents keep making progress on the remaining threads
/// instead of waiting behind a single global queue.
const WORKER_POOL: usize = 4;
#[path = "mapped_viewport.rs"]
mod mapped_viewport;
#[path = "paged_spill.rs"]
mod paged_spill;
#[path = "paged_transfer.rs"]
pub mod transfer;
pub use mapped_viewport::{SourceAffinity, ViewportSegment};
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlobalScrollPosition {
    Ready(u64, f64, f64),
    Pending,
}
#[derive(Clone, Debug)]
pub struct PagedFrameState {
    pub displayed: std::ops::Range<TextOffset>,
    pub requested: Option<TextOffset>,
    pub ready: bool,
}
/// All scroll quantities are UTF-8 bytes, independent of unknown line totals.
#[derive(Clone, Copy, Debug)]
pub struct PagedScrollMetrics {
    pub offset: f64,
    pub viewport: f64,
    pub total: f64,
    pub lines_known: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionRestoreStatus {
    Pending,
    Applied,
    Failed(String),
    Superseded,
}
struct SelectionValidation {
    cancellation: Cancellation,
    snapshot: PagedSnapshot,
    preserve_viewport: bool,
    /// Re-window even when the caret is already loaded (a window-edge move).
    recentre: bool,
    /// The validated selection and its centred, line-aligned window start.
    result: Receiver<Result<(Selection, usize), String>>,
    completed: Option<Result<(Selection, usize), String>>,
}
impl Drop for SelectionValidation {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
#[derive(Clone, Copy)]
struct ViewportMapping {
    offset: usize,
    line: u64,
    line_start: TextOffset,
    line_end: TextOffset,
}
type Job = Box<dyn FnOnce() + Send + 'static>;
/// The worker path's refusal of a linked history entry, reached only when the
/// linked-history probe found the actor briefly busy; the view replays the input.
const LINKED_HISTORY_BUSY: &str = "The linked documents are busy; try again in a moment.";
/// How long a busy reader sleeps before re-checking the actor.
const ACTOR_WAIT: std::time::Duration = std::time::Duration::from_millis(5);
const PAGED_QUEUE_DEPTH: usize = 64;
struct PagedWorker(BoundedExecutor);
impl PagedWorker {
    fn submit(&self, kind: WorkKind, job: Job) -> Result<(), SubmitError> {
        self.0.submit(kind, job)
    }
}
fn worker() -> &'static PagedWorker {
    static WORKER: OnceLock<PagedWorker> = OnceLock::new();
    WORKER.get_or_init(|| PagedWorker(BoundedExecutor::new(WORKER_POOL, PAGED_QUEUE_DEPTH, "paged-view-io")))
}
struct JobWake(Option<Arc<dyn Fn() + Send + Sync>>);
impl JobWake {
    fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self(Some(notify))
    }
    fn finish(mut self) {
        self.fire();
    }
    fn fire(&mut self) {
        if let Some(notify) = self.0.take() {
            // Unwind builds (tests) only; release panics abort via the fatal panic hook.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| notify()));
        }
    }
}
impl Drop for JobWake {
    fn drop(&mut self) {
        self.fire();
    }
}
struct JobCompletion<T, E: From<String> = PagedOperationError> {
    sender: Option<SyncSender<Result<T, E>>>,
    wake: JobWake,
}
impl<T, E: From<String>> JobCompletion<T, E> {
    fn new(sender: SyncSender<Result<T, E>>, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            sender: Some(sender),
            wake: JobWake::new(notify),
        }
    }
    fn complete(mut self, result: Result<T, E>) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.try_send(result);
        }
        self.wake.fire();
    }
}
impl<T, E: From<String>> Drop for JobCompletion<T, E> {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.try_send(Err("Paged worker failed".to_owned().into()));
        }
        self.wake.fire();
    }
}
#[derive(Debug)]
enum PagedOperationError {
    Lifecycle(PagedLifecycleError),
    Message(String),
}
impl From<String> for PagedOperationError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}
impl From<&str> for PagedOperationError {
    fn from(value: &str) -> Self {
        Self::Message(value.into())
    }
}
impl From<PagedLifecycleError> for PagedOperationError {
    fn from(value: PagedLifecycleError) -> Self {
        Self::Lifecycle(value)
    }
}
impl std::fmt::Display for PagedOperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lifecycle(error) => error.fmt(f),
            Self::Message(error) => f.write_str(error),
        }
    }
}
enum Action {
    Tail {
        platform: Arc<dyn LocalFileSystem>,
        request: bool,
        follow: bool,
    },
    UnlockTail,
    RetryRecovery,
    /// A materialized transaction and the history metadata it records.
    Prepared(
        EditTransaction,
        Option<crate::tracked_edit::TrackedEditCompletion>,
        bareline_document::history::EditMetadata,
    ),
    Source(
        bareline_document::paged::PreparedSourceTransaction,
        Option<crate::tracked_edit::TrackedEditCompletion>,
    ),
    Metadata(bareline_document::DocumentMetadata),
    Read(usize),
    Edit {
        range: std::ops::Range<TextOffset>,
        insert: String,
    },
    Undo,
    Redo,
    Save {
        owner: Option<PagedSaveOwner>,
        copy_only: bool,
        destination: bareline_file_io::lifecycle::PreparedDestination,
        platform: Arc<dyn LocalFileSystem>,
    },
}
/// Presentation owner for one admitted paged save. This is separate from the
/// file-layer operation generation so a delayed receipt cannot resolve a later
/// save submitted by the same view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PagedSaveOwner {
    pub document: (u64, u64),
    pub generation: u64,
}

/// The exact file-layer terminal paired with the presentation request that
/// admitted it. Publication does not release the surface worker: recovery
/// retirement and viewport refresh remain protected by `busy()`.
#[derive(Debug)]
pub struct PagedSaveTerminal {
    pub owner: PagedSaveOwner,
    pub receipt: bareline_file_io::paged_service::PagedLifecycleReceipt,
}
struct Completed {
    selections: Option<crate::power::SelectionSet>,
    append_receipt: Option<bareline_file_io::tail::AppendReceipt>,
    generation_owner: Arc<()>,
    peer_epoch: u64,
    following: bool,
    tail_pending: bool,
    source_changed: bool,
    can_undo: bool,
    can_redo: bool,
    snapshot: PagedSnapshot,
    window: Result<TextWindow, String>,
    caret: usize,
    /// Requested window start, retried when the window read fails after a commit.
    start: usize,
}
struct PeerState {
    epoch: u64,
    /// Receipts of recent commits, so a view several commits behind maps its
    /// selection and window through all of them (PED-11, PED-13).
    changes: crate::change_log::ChangeLog,
}
pub struct PagedEditorSurface {
    manual_hidden: Vec<std::ops::Range<usize>>,
    mapping_dirty: bool,
    rebased_folds: Vec<mapped_viewport::FoldAnchor>,
    known_fold_anchors: Vec<mapped_viewport::FoldAnchor>,
    fold_history: std::collections::VecDeque<(
        ContentStateId,
        Vec<bareline_syntax::folding::Fold>,
        bareline_syntax::folding::FoldState,
        std::collections::BTreeMap<usize, bool>,
        Vec<mapped_viewport::FoldAnchor>,
        // Carried folds no mapping had resolved in that text (PED-07).
        Vec<mapped_viewport::FoldAnchor>,
    )>,
    mapped: Option<mapped_viewport::MappedViewport>,
    mapping_job: Option<mapped_viewport::MappingJob>,
    /// The pending mapping reveals a fold at a selection endpoint; input waits
    /// for it (PED-12). Any other pending mapping lets input through (PED-07).
    reveal_mapping: bool,
    mapping_generation: u64,
    mapping_folds: Vec<std::ops::Range<usize>>,
    viewport_request: Option<TextOffset>,
    queued_viewport: Option<TextOffset>,
    queued_wheel: (f64, f32),
    bottom_scroll: Option<f32>,
    horizontal_anchor: Option<(usize, f32)>,
    /// A horizontal bar jump's target, keyed by its source anchor byte (EDT-28).
    horizontal_target: Option<(usize, f64)>,
    queued_horizontal: f64,
    prefetch: crate::paged_navigation::ViewportPrefetch,
    global_selections: crate::power::SelectionSet,
    power_state: crate::paged_power::PowerViewState,
    power_state_history: crate::paged_power::PowerStateHistory,
    power_inputs: std::collections::VecDeque<Input>,
    power_preparing: bool,
    /// An edit deferred its recovery append behind queued input (PED-15); the batch
    /// is journaled once the view is idle unless a later edit journaled it.
    recovery_deferred: bool,
    /// Typing history boundary for staged input; any other input renews it.
    power_history_boundary: u64,
    power_input_enabled: bool,
    power_hidden_refresh: bool,
    /// A macro playback run; edits made while it is open are tagged with it in
    /// the actor history, and undo or redo them as one step afterwards (WSP-09).
    undo_run: Option<u64>,
    /// The selection was clamped through changes the view could not map; the
    /// next window moves endpoints it holds back to a boundary (PED-11).
    resnap_selection: bool,
    projected_selection: Selection,
    selection_token: u64,
    selection_status: SelectionRestoreStatus,
    selection_validation: Option<SelectionValidation>,
    pending_moves_selection: bool,
    deferred_input: Option<Input>,
    /// The deferred input follows a window-edge re-centre.
    deferred_edge: bool,
    /// Set while that input is replayed, so it cannot re-centre again.
    edge_replay: bool,
    navigation_anchor: Option<usize>,
    initial_eol: Option<((u64, u64), bareline_file_io::codecs::state::EolState)>,
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
    /// Scroll the caret into view once the window requested for it arrives.
    reveal_after_read: bool,
    append_receipt: Option<bareline_file_io::tail::AppendReceipt>,
    view_generation: Arc<()>,
    captured: Option<PagedReadHandle>,
    peer: Arc<Mutex<PeerState>>,
    peer_epoch: u64,
    /// Held by this view, its clones and every read handle taken from them.
    views: Arc<()>,
    /// Held only by open views of this actor, never by read handles that background
    /// jobs still hold, so a closed view releases linked history at once (QA-07).
    open_views: Arc<()>,
    following: bool,
    follow_paused: bool,
    tail_pending: bool,
    tail_changed: bool,
    /// A follow request that arrived while the view was busy; the next idle tick
    /// sends it, so an append signalled meanwhile is never dropped (QA-08).
    follow_requested: bool,
    pub surface: EditorSurface,
    actor: PagedSession,
    snapshot: PagedSnapshot,
    streaming_quota: u64,
    configured_history_limit: Option<usize>,
    tracked_acknowledged: std::collections::VecDeque<u64>,
    can_undo: bool,
    can_redo: bool,
    budget: Budget,
    pending: Option<Receiver<Result<Completed, PagedOperationError>>>,
    save_generation: Arc<std::sync::atomic::AtomicU64>,
    pending_save_owner: Option<PagedSaveOwner>,
    save_terminals: Arc<Mutex<std::collections::VecDeque<PagedSaveTerminal>>>,
    pending_input: Option<Input>,
    cancellation: Cancellation,
    notify: Arc<dyn Fn() + Send + Sync>,
    viewport_start: usize,
    viewport_valid: bool,
    pub error: Option<String>,
}
impl PagedEditorSurface {
    /// Retryable, nonblocking policy update on the same worker as paged edits.
    pub fn configure_history_limit(&mut self, max_changes: usize) -> bool {
        if self.configured_history_limit == Some(max_changes) {
            return true;
        }
        let actor = self.actor.clone();
        let notify = self.notify.clone();
        if worker()
            .submit(
                WorkKind::Maintenance,
                Box::new(move || {
                    let completion = JobWake::new(notify);
                    let mut opened = match actor.lock_document() {
                        Ok(opened) => opened,
                        Err(_) => return,
                    };
                    opened.document_mut().set_history_limit(max_changes);
                    drop(opened);
                    completion.finish();
                }),
            )
            .is_err()
        {
            return false;
        }
        self.configured_history_limit = Some(max_changes);
        true
    }
    pub fn initial_eol_label(&self) -> Option<&'static str> {
        self.initial_eol
            .filter(|(identity, _)| *identity == self.snapshot.identity_token())
            .map(|(_, eol)| eol.label())
    }
    pub fn encoding_failure(&self) -> Option<bareline_file_io::codecs::failure::EncodingFailure> {
        self.actor.encoding_failure()
    }
    pub fn take_save_conflict(&self) -> Option<bareline_file_io::lifecycle::SaveConflict> {
        self.actor.take_save_conflict()
    }
    pub fn take_save_cleanup(&self) -> Option<bareline_file_io::lifecycle::SaveCleanup> {
        self.actor.take_save_cleanup()
    }
    pub fn encoding_state(&self) -> Option<bareline_file_io::codecs::state::EncodingState> {
        bareline_file_io::codecs::state::metadata_encoding(self.snapshot.metadata())
            .or_else(|| self.actor.state().ok().map(|state| state.encoding))
    }
    pub fn apply_document_metadata(&mut self, metadata: bareline_document::DocumentMetadata) -> Result<(), String> {
        if self.surface.user_read_only {
            return Err("Document is read only".into());
        }
        self.submit(Action::Metadata(metadata))
    }
    pub fn read_handle(&self) -> PagedReadHandle {
        if let Some(captured) = &self.captured {
            return captured.clone();
        }
        self.actor
            .read_handle(self.snapshot.clone(), self.view_generation.clone(), self.views.clone())
    }
    pub fn new(opened: Box<PagedOpened>, budget: Budget, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let snapshot = opened.transcoded.document.snapshot();
        let prefix = DocumentBuilder::new(budget.clone(), Budget::new(0))
            .map_err(|error| error.to_string())?
            .prefix();
        let mut surface = EditorSurface::loading(prefix, notify.clone());
        surface.set_gutter_lines_estimated(true);
        surface.set_eol_status_override(Some("Computing".into()));
        surface.encoding_label = opened
            .transcoded
            .store
            .state
            .save_target
            .status_label(opened.transcoded.store.state.bom);
        surface.user_read_only = opened.transcoded.store.state.binary_warning;
        let initial_eol = (opened.recovery_origin.is_none()
            && snapshot.revision.0 == 0
            && snapshot.content_state == opened.transcoded.document.saved_content_state())
        .then_some((snapshot.identity_token(), opened.transcoded.store.eol));
        let can_undo = opened.transcoded.document.can_undo();
        let can_redo = opened.transcoded.document.can_redo();
        let actor = PagedSession::new(opened);
        let generation_owner = actor.current_generation_owner();
        let mut view = Self {
            mapped: None,
            mapping_job: None,
            reveal_mapping: false,
            mapping_generation: 0,
            mapping_folds: Vec::new(),
            manual_hidden: Vec::new(),
            mapping_dirty: false,
            rebased_folds: Vec::new(),
            known_fold_anchors: Vec::new(),
            fold_history: Default::default(),
            viewport_request: None,
            queued_viewport: None,
            queued_wheel: (0.0, 0.0),
            bottom_scroll: None,
            horizontal_anchor: None,
            horizontal_target: None,
            queued_horizontal: 0.0,
            prefetch: Default::default(),
            initial_eol,
            navigation: crate::paged_navigation::GlobalNavigation::new(),
            navigation_ready: None,
            requested_scroll: None,
            pending_scroll_mapping: None,
            viewport_mapping: None,
            global_spacers: Vec::new(),
            global_folds: Vec::new(),
            global_fold_state: Default::default(),
            global_fold_overrides: Default::default(),
            global_folds_partial: true,
            global_fold_initialized: false,
            pending_global_folds: Vec::new(),
            fold_viewport_line: None,
            power_hidden_refresh: false,
            power_input_enabled: false,
            power_inputs: std::collections::VecDeque::new(),
            power_preparing: false,
            recovery_deferred: false,
            power_history_boundary: crate::power::consumer::next_receipt_sequence(),
            undo_run: None,
            resnap_selection: false,
            power_state_history: Default::default(),
            power_state: crate::paged_power::PowerViewState::default(),
            global_selections: Selection::default().into(),
            projected_selection: Selection::default(),
            selection_token: 0,
            selection_status: SelectionRestoreStatus::Superseded,
            selection_validation: None,
            pending_moves_selection: false,
            deferred_input: None,
            deferred_edge: false,
            edge_replay: false,
            navigation_anchor: None,
            view_generation: generation_owner,
            search_marks: Default::default(),
            reveal_after_read: false,
            append_receipt: None,
            captured: None,
            peer: Arc::new(Mutex::new(PeerState {
                epoch: 0,
                changes: Default::default(),
            })),
            peer_epoch: 0,
            views: Arc::new(()),
            open_views: Arc::new(()),
            following: false,
            follow_paused: false,
            tail_pending: false,
            tail_changed: false,
            follow_requested: false,
            can_undo,
            can_redo,
            streaming_quota: 20 * 1024 * 1024 * 1024,
            configured_history_limit: None,
            tracked_acknowledged: Default::default(),
            snapshot,
            surface,
            actor,
            budget,
            pending: None,
            save_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            pending_save_owner: None,
            save_terminals: Arc::new(Mutex::new(Default::default())),
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
        if !handle.belongs_to(&self.actor) {
            return Err("Captured view belongs to another document actor".into());
        }
        self.clone_view_inner(Some(handle.clone()))
    }
    fn clone_view_inner(&self, captured: Option<PagedReadHandle>) -> Result<Self, String> {
        let prefix = DocumentBuilder::new(self.budget.clone(), Budget::new(0))
            .map_err(|e| e.to_string())?
            .prefix();
        let mut surface = EditorSurface::loading(prefix, self.notify.clone());
        surface.encoding_label = self.surface.encoding_label.clone();
        surface.file_bytes = self.surface.file_bytes;
        surface.line_status = self.surface.line_status.clone();
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
        surface.set_gutter_lines_estimated(self.surface.gutter_lines_estimated());
        surface.highlight_current_line = self.surface.highlight_current_line;
        surface.whitespace = self.surface.whitespace.clone();
        surface.guides = self.surface.guides;
        let mut view = Self {
            // Peers of one actor share the document's line index; a captured
            // (historical) view indexes its own text.
            navigation: if captured.is_some() {
                crate::paged_navigation::GlobalNavigation::new()
            } else {
                crate::paged_navigation::GlobalNavigation::sharing(self.navigation.line_index().clone())
            },
            navigation_ready: None,
            requested_scroll: None,
            pending_scroll_mapping: None,
            viewport_mapping: None,
            global_spacers: self.global_spacers.clone(),
            mapped: None,
            mapping_job: None,
            reveal_mapping: false,
            mapping_generation: 0,
            mapping_folds: Vec::new(),
            manual_hidden: self.manual_hidden.clone(),
            mapping_dirty: true,
            // A peer of the same text keeps the carried folds no mapping has
            // resolved yet (PED-07).
            rebased_folds: if captured.is_none() {
                self.rebased_folds.clone()
            } else {
                Vec::new()
            },
            known_fold_anchors: self.known_fold_anchors.clone(),
            fold_history: Default::default(),
            viewport_request: None,
            queued_viewport: None,
            queued_wheel: (0.0, 0.0),
            bottom_scroll: None,
            horizontal_anchor: None,
            horizontal_target: None,
            queued_horizontal: 0.0,
            prefetch: Default::default(),
            captured: captured.clone(),
            power_hidden_refresh: false,
            power_input_enabled: self.power_input_enabled,
            power_inputs: std::collections::VecDeque::new(),
            power_preparing: false,
            recovery_deferred: false,
            power_history_boundary: crate::power::consumer::next_receipt_sequence(),
            undo_run: None,
            resnap_selection: false,
            power_state_history: self.power_state_history.clone(),
            power_state: self.power_state.clone(),
            global_selections: if captured.is_some() {
                Selection::default().into()
            } else {
                self.global_selection_set()
            },
            projected_selection: Selection::default(),
            selection_token: 0,
            selection_status: SelectionRestoreStatus::Superseded,
            selection_validation: None,
            pending_moves_selection: false,
            deferred_input: None,
            deferred_edge: false,
            edge_replay: false,
            navigation_anchor: None,
            global_folds: self.global_folds.clone(),
            global_fold_state: self.global_fold_state.clone(),
            global_fold_overrides: self.global_fold_overrides.clone(),
            global_folds_partial: self.global_folds_partial,
            global_fold_initialized: self.global_fold_initialized,
            pending_global_folds: self.pending_global_folds.clone(),
            fold_viewport_line: None,
            search_marks: self.search_marks.clone(),
            reveal_after_read: false,
            append_receipt: self.append_receipt,
            view_generation: captured
                .as_ref()
                .map_or_else(|| self.view_generation.clone(), PagedReadHandle::generation_owner),
            peer: self.peer.clone(),
            peer_epoch: self.peer_epoch,
            views: self.views.clone(),
            open_views: self.open_views.clone(),
            following: captured.is_none() && self.following,
            follow_paused: self.follow_paused,
            tail_pending: self.tail_pending,
            tail_changed: self.tail_changed,
            follow_requested: false,
            surface,
            initial_eol: self.initial_eol,
            actor: self.actor.clone(),
            snapshot: captured
                .as_ref()
                .map_or_else(|| self.snapshot.clone(), |h| h.snapshot().fork_identity()),
            streaming_quota: self.streaming_quota,
            configured_history_limit: self.configured_history_limit,
            tracked_acknowledged: self.tracked_acknowledged.clone(),
            can_undo: self.can_undo,
            can_redo: self.can_redo,
            budget: self.budget.clone(),
            pending: None,
            save_generation: self.save_generation.clone(),
            pending_save_owner: None,
            save_terminals: Arc::new(Mutex::new(Default::default())),
            pending_input: None,
            cancellation: self.cancellation.clone(),
            notify: self.notify.clone(),
            viewport_start: self.viewport_start,
            viewport_valid: false,
            error: None,
        };
        view.request_viewport(TextOffset(self.viewport_start))?;
        Ok(view)
    }
    /// Nonblocking peer publication check; the next viewport is fetched on the worker.
    pub fn refresh_peer(&mut self) -> bool {
        // A completed linked-history receipt still keeps public busy true until
        // installation; it must not prevent scheduling its own peer viewport.
        if self.power_preparing || !self.power_inputs.is_empty() || self.power_actor_busy() || self.captured.is_some() {
            return false;
        }
        let changed = self.peer.try_lock().is_ok_and(|peer| peer.epoch != self.peer_epoch);
        if !changed {
            return false;
        }
        self.sync_global_selection();
        self.viewport_valid = false;
        match self.submit(Action::Read(self.viewport_start)) {
            Ok(()) => true,
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }
    pub fn enable_recovery(&mut self, root: PathBuf, platform: Arc<dyn LocalFileSystem>) {
        self.actor.configure_recovery(root, platform);
    }
    pub fn retry_recovery(&mut self) -> Result<(), String> {
        self.submit(Action::RetryRecovery)
    }
    /// A captured historical/preview view (e.g. "compare with last saved"). It shares
    /// the live document's actor but owns neither its save state nor its recovery
    /// journal: it is never dirty, never saved by Save All, and never retires
    /// recovery on close (REC-13).
    pub fn historical(&self) -> bool {
        self.captured.is_some()
    }
    /// Begin durable discard off the UI thread and report when the tombstone permits close.
    pub fn discard_recovery(&mut self) -> bareline_file_io::recovery_retirement::DiscardPoll {
        if self.historical() {
            // The journal belongs to the live document, which stays open.
            return bareline_file_io::recovery_retirement::DiscardPoll::Durable;
        }
        self.actor.poll_recovery_discard(self.notify.clone())
    }
    pub fn resume_recovery_after_discard(&mut self) {
        if self.historical() {
            return;
        }
        // The retired recovery can still finish a cancelled baseline job. Give
        // reopened editing a new status owner so that late completion cannot
        // publish an error into the replacement journal generation.
        self.actor.resume_recovery_after_discard();
    }
    pub fn recovery_status(&self) -> bareline_file_io::paged_recovery::PagedRecoveryStatus {
        self.actor.recovery_status()
    }
    pub fn mark_recovered(&mut self) {
        self.actor.mark_recovered();
        if let Ok(mut peer) = self.peer.lock() {
            peer.epoch = peer.epoch.wrapping_add(1);
        }
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
        self.input_busy() || self.mapping_job.is_some()
    }
    /// `busy` without a pending fold mapping, which input does not wait for.
    fn input_busy(&self) -> bool {
        transfer::history_pending(self)
            || self.power_preparing
            || !self.power_inputs.is_empty()
            || self.edit_actor_busy()
    }
    pub fn power_actor_busy(&self) -> bool {
        self.edit_actor_busy() || self.mapping_job.is_some()
    }
    /// Work the next edit waits for. A pending fold mapping is not part of it:
    /// the edit supersedes the mapping, which is queued again for the committed
    /// text, while the previous mapping stays on screen (PED-07). Only a mapping
    /// queued by a fold reveal still holds input back, so typing never lands in
    /// text that is drawn as hidden (PED-12).
    pub fn edit_actor_busy(&self) -> bool {
        transfer::history_busy(self)
            || self.pending.is_some()
            || self.selection_validation.is_some()
            || (self.reveal_mapping && self.mapping_job.is_some())
    }
    pub fn take_power_input(&mut self) -> Option<Input> {
        if self.edit_actor_busy() || self.power_preparing {
            return None;
        }
        self.sync_global_selection();
        let input = self.power_inputs.pop_front()?;
        self.power_preparing = true;
        Some(input)
    }
    /// Enable the composition root's staged input worker before accepting input.
    pub fn enable_power_input(&mut self) {
        self.power_input_enabled = true;
    }
    pub fn finish_power_preparation(&mut self) {
        self.power_preparing = false;
        // A burst's last input can make no edit (or fail to prepare), leaving the
        // view idle with the batch earlier inputs deferred: wake it so its idle pump
        // journals that batch without waiting for unrelated activity (PED-15).
        if self.recovery_deferred && !self.busy() {
            (self.notify)();
        }
    }
    /// Returns an input taken by [`Self::take_power_input`] whose preparation
    /// could not start (the shared pool was busy) to the front of the queue, so
    /// the keystroke is retried in order instead of lost (PED-17).
    pub fn requeue_power_input(&mut self, input: Input) {
        self.power_preparing = false;
        self.power_inputs.push_front(input);
    }
    /// Tag every edit committed from now on with `run`, so that once the run ends
    /// one Undo or Redo moves all of its adjacent entries (one macro playback,
    /// one step), as on a resident editor (WSP-09).
    pub fn begin_undo_run(&mut self, run: u64) {
        self.undo_run = Some(run);
    }
    pub fn end_undo_run(&mut self) {
        self.undo_run = None;
    }
    pub fn undo_run(&self) -> Option<u64> {
        self.undo_run
    }
    pub fn take_power_hidden_refresh(&mut self) -> bool {
        if self.busy() {
            false
        } else {
            std::mem::take(&mut self.power_hidden_refresh)
        }
    }
    pub fn acknowledge_power_input(
        &mut self,
        receipt: Option<&crate::TrackedEditReceipt>,
        source: &PagedSnapshot,
        input: Input,
    ) -> Result<(), String> {
        let revision = if let Some(receipt) = receipt {
            receipt.terminal().ok_or("Input is still pending")??
        } else {
            source.revision
        };
        if source.identity_token().0 != self.snapshot.identity_token().0 || revision != self.snapshot.revision {
            return Err("Input receipt source changed".into());
        }
        self.surface.acknowledge(input);
        Ok(())
    }
    pub fn source_segments(&self) -> &[ViewportSegment] {
        self.mapped.as_ref().map_or(&[], |map| map.segments.as_slice())
    }
    pub fn source_changed(&self) -> bool {
        self.tail_changed || self.actor.source_changed()
    }
    pub fn source_offset(&self, local: TextOffset, affinity: SourceAffinity) -> Option<TextOffset> {
        if local.0 > self.surface.snapshot.len() {
            return None;
        }
        if let Some(map) = &self.mapped {
            map.source_offset(local, affinity)
        } else {
            Some(TextOffset(self.viewport_start + local.0))
        }
    }
    pub fn local_offset(&self, source: TextOffset) -> Option<TextOffset> {
        if let Some(map) = &self.mapped {
            map.local_offset(source)
        } else {
            source
                .0
                .checked_sub(self.viewport_start)
                .filter(|offset| *offset <= self.surface.snapshot.len())
                .map(TextOffset)
        }
    }
    fn local_line_for_global(&self, global: u64) -> Option<u64> {
        if self.mapped.is_none() {
            return global
                .checked_sub(self.viewport_first_global_line()?)
                .filter(|line| *line < self.surface.snapshot.line_count() as u64);
        }
        for segment in self.source_segments() {
            let relative = global.checked_sub(segment.first_global_line?)?;
            let base = self.surface.snapshot.line_at(segment.local.start).ok()?;
            let line = base.checked_add(usize::try_from(relative).ok()?)?;
            if let Ok(range) = self.surface.snapshot.line_range(line)
                && range.start >= segment.local.start
                && range.start < segment.local.end
            {
                return Some(line as u64);
            }
        }
        None
    }
    fn queue_fold_projection(&mut self) {
        if self.pending.is_some() || !self.viewport_valid {
            return;
        }
        let folds: Vec<_> = self
            .global_folds
            .iter()
            .filter(|fold| self.global_fold_state.collapsed.contains(&fold.header))
            .map(|fold| fold.header + 1..fold.end + 1)
            .collect();
        if !self.mapping_dirty
            && folds == self.mapping_folds
            && (self.mapping_job.is_some()
                || self.mapped.as_ref().is_some_and(|map| {
                    map.source.same_document(self.read_handle().snapshot())
                        && map.source.content_state == self.snapshot.content_state
                }))
        {
            return;
        }
        if folds.is_empty()
            && self.manual_hidden.is_empty()
            && self.rebased_folds.is_empty()
            && self.mapped.is_none()
            && self.mapping_job.is_none()
        {
            return;
        }
        self.mapping_generation = self.mapping_generation.wrapping_add(1);
        self.mapping_folds = folds.clone();
        let collapsed: Vec<_> = self
            .global_folds
            .iter()
            .filter(|fold| self.global_fold_state.collapsed.contains(&fold.header))
            .cloned()
            .collect();
        // The mapping looks up only folds near the window, except collapsed
        // ones the view has no byte anchor for (PED-07).
        let anchored: std::collections::HashSet<(usize, usize)> = self
            .known_fold_anchors
            .iter()
            .chain(self.mapped.iter().flat_map(|map| map.anchors.iter()))
            .map(|anchor| (anchor.fold.header, anchor.fold.end))
            .collect();
        let unanchored = collapsed
            .iter()
            .map(|fold| (fold.header, fold.end))
            .filter(|fold| !anchored.contains(fold))
            .collect();
        match mapped_viewport::request(
            self.read_handle(),
            TextOffset(self.viewport_start),
            collapsed,
            self.manual_hidden.clone(),
            self.rebased_folds.clone(),
            mapped_viewport::MappedText {
                generation: self.mapping_generation,
                line_index: self.navigation.line_index().clone(),
                unanchored,
            },
            self.budget.clone(),
            self.notify.clone(),
        ) {
            Ok(job) => {
                self.mapping_job = Some(job);
                self.mapping_dirty = false;
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn set_global_hidden_ranges(&mut self, ranges: &[std::ops::Range<u64>]) -> Result<(), String> {
        if ranges.len() > 8192 {
            return Err("Too many hidden line ranges".into());
        }
        let hidden = ranges
            .iter()
            .map(|range| {
                if range.start >= range.end {
                    return Err("Invalid hidden line range".to_owned());
                }
                Ok(
                    usize::try_from(range.start).map_err(|_| "Hidden line exceeds platform")?
                        ..usize::try_from(range.end).map_err(|_| "Hidden line exceeds platform")?,
                )
            })
            .collect::<Result<Vec<_>, String>>()?;
        if self.manual_hidden != hidden {
            self.manual_hidden = hidden;
            self.mapping_dirty = true;
            self.queue_fold_projection();
        }
        Ok(())
    }
    /// Moves the fold state to `next`, the text a commit installs; `window` is
    /// the window of it the commit read.
    fn transition_fold_anchors(&mut self, next: &PagedSnapshot, window: Option<&TextWindow>) {
        if next.content_state == self.snapshot.content_state {
            return;
        }
        self.manual_hidden.clear();
        self.fold_history.retain(|entry| entry.0 != self.snapshot.content_state);
        self.fold_history.push_back((
            self.snapshot.content_state,
            self.global_folds.clone(),
            self.global_fold_state.clone(),
            self.global_fold_overrides.clone(),
            self.known_fold_anchors.clone(),
            self.rebased_folds.clone(),
        ));
        while self.fold_history.len() > 16 {
            self.fold_history.pop_front();
        }
        // Carried folds no mapping has resolved since the last change move again;
        // their collapsed state lives only in the anchor (PED-07).
        let carried = std::mem::take(&mut self.rebased_folds);
        self.mapping_dirty = true;
        let change = next
            .applied_change()
            .filter(|change| change.matches_before(self.snapshot.identity_token(), self.snapshot.content_state))
            .cloned();
        let shift = change
            .as_ref()
            .and_then(|change| self.change_line_shift(change, next, window));
        if let Some((_, folds, state, overrides, anchors, rebased)) =
            self.fold_history.iter().find(|entry| entry.0 == next.content_state)
        {
            self.global_folds = folds.clone();
            self.global_fold_state = state.clone();
            self.global_fold_overrides = overrides.clone();
            self.known_fold_anchors = anchors.clone();
            self.rebased_folds = rebased.clone();
            // Carried collapsed folds the restored text does not hold join it,
            // moved through the change, so an undo or redo never expands them.
            if let Some(change) = &change {
                let held: std::collections::HashSet<(usize, usize)> = self
                    .known_fold_anchors
                    .iter()
                    .chain(&self.rebased_folds)
                    .map(|anchor| (anchor.header.0, anchor.end.0))
                    .collect();
                let moved: Vec<_> = carried
                    .iter()
                    .filter(|anchor| anchor.collapsed)
                    .filter_map(|anchor| rebase_fold_anchor(change, shift, anchor, true, next.len()))
                    .filter(|anchor| !held.contains(&(anchor.header.0, anchor.end.0)))
                    .collect();
                self.rebased_folds.extend(moved);
                self.rebased_folds
                    .sort_by_key(|anchor| (anchor.header, anchor.end, !anchor.collapsed));
                self.rebased_folds.dedup_by_key(|anchor| (anchor.header, anchor.end));
                self.rebased_folds.truncate(8192);
            }
            return;
        }
        if let Some(change) = &change {
            let anchors = self
                .known_fold_anchors
                .iter()
                .chain(self.mapped.iter().flat_map(|map| map.anchors.iter()))
                .map(|anchor| (anchor, self.global_fold_state.collapsed.contains(&anchor.fold.header)))
                .chain(carried.iter().map(|anchor| (anchor, anchor.collapsed)));
            for (anchor, collapsed) in anchors {
                if let Some(moved) = rebase_fold_anchor(change, shift, anchor, collapsed, next.len()) {
                    self.rebased_folds.push(moved);
                }
            }
        }
        // Of duplicates, a carried collapsed fold wins over a known anchor whose
        // state the last change already reset.
        self.rebased_folds
            .sort_by_key(|anchor| (anchor.header, anchor.end, !anchor.collapsed));
        self.rebased_folds.dedup_by_key(|anchor| (anchor.header, anchor.end));
        self.rebased_folds.truncate(8192);
        self.known_fold_anchors.clear();
        self.global_folds.clear();
        self.global_fold_overrides.clear();
        self.global_fold_state.unfold_all();
        self.global_fold_initialized = false;
    }
    /// How `change`, from this view's text to `next`, moves the lines past its
    /// edits, counted from the bytes it replaced and inserted (PED-07). `None`
    /// when its edits are out of order or those bytes are not in memory: the
    /// replaced ones in the window this view shows, the inserted ones and their
    /// neighbours in `window`, a window of `next`.
    fn change_line_shift(
        &self,
        change: &bareline_document::change::AppliedChange,
        next: &PagedSnapshot,
        window: Option<&TextWindow>,
    ) -> Option<LineShift> {
        let edits = change.edits();
        let mut cursor = 0;
        let mut grown = 0isize;
        for edit in edits {
            if edit.before.start.0 < cursor || edit.before.start > edit.before.end {
                return None;
            }
            cursor = edit.before.end.0;
            grown += edit.inserted_len as isize - (edit.before.end.0 - edit.before.start.0) as isize;
        }
        let (start, end) = (edits.first()?.before.start.0, edits.last()?.before.end.0);
        let inserted_end = end.checked_add_signed(grown)?;
        if end - start > LINE_SHIFT_BYTES || inserted_end.saturating_sub(start) > LINE_SHIFT_BYTES {
            return None;
        }
        // The bytes before and after the edits are the same in both texts.
        let window = window?;
        let range = window.range();
        let text = window.text().as_bytes();
        let byte = |offset: usize| text.get(offset.checked_sub(range.start.0)?).copied();
        if start < range.start.0 || inserted_end < start || inserted_end > range.end.0 {
            return None;
        }
        let before_cr = if start == 0 { false } else { byte(start - 1)? == b'\r' };
        let after = if inserted_end == next.len() {
            None
        } else {
            Some(byte(inserted_end)?)
        };
        let inserted = text.get(start - range.start.0..inserted_end - range.start.0)?;
        let replaced = if start == end {
            String::new()
        } else {
            // The window this view shows is a window of its own text.
            if !self.viewport_valid
                || self
                    .mapped
                    .as_ref()
                    .is_some_and(|map| map.source.content_state != self.snapshot.content_state)
            {
                return None;
            }
            let (from, to) = (
                self.local_offset(TextOffset(start))?,
                self.local_offset(TextOffset(end))?,
            );
            // Equal distances mean no hidden text lies between them.
            if to.0.checked_sub(from.0)? != end - start {
                return None;
            }
            self.surface.snapshot.read(from..to, end - start).ok()?
        };
        let breaks = |core: &[u8]| {
            let mut previous_cr = before_cr;
            let mut count = 0isize;
            for &byte in core {
                count += isize::from(byte == b'\r' || (byte == b'\n' && !previous_cr));
                previous_cr = byte == b'\r';
            }
            let past = count + isize::from(after.is_some_and(|byte| byte == b'\r' || (byte == b'\n' && !previous_cr)));
            (count, past)
        };
        let (old, new) = (breaks(replaced.as_bytes()), breaks(inserted));
        Some(LineShift {
            start,
            end,
            at: new.0 - old.0,
            past: new.1 - old.1,
        })
    }
    fn pump_fold_projection(&mut self) -> bool {
        let Some(job) = &self.mapping_job else {
            return false;
        };
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(_) => Err("Fold viewport worker stopped".into()),
        };
        self.mapping_job = None;
        match result {
            Ok(map)
                if map.generation == self.mapping_generation
                    && map.source.same_document(self.read_handle().snapshot())
                    && map.source.content_state == self.snapshot.content_state =>
            {
                self.surface.snapshot = map.projection.clone();
                self.surface.layout_revision = None;
                self.surface.set_source_segments(&map.segments);
                let mut present: std::collections::HashSet<(usize, usize)> =
                    self.global_folds.iter().map(|fold| (fold.header, fold.end)).collect();
                for anchor in &map.anchors {
                    if present.insert((anchor.fold.header, anchor.fold.end)) {
                        self.global_folds.push(anchor.fold.clone());
                    }
                    if anchor.collapsed {
                        self.global_fold_state.collapsed.insert(anchor.fold.header);
                        self.global_fold_overrides.insert(anchor.fold.header, true);
                    }
                }
                self.merge_known_fold_anchors(map.anchors.clone());
                self.global_folds
                    .sort_by_key(|fold| (fold.header, std::cmp::Reverse(fold.end)));
                // Carried folds too far from this window wait for a mapping that
                // reaches them (PED-07).
                self.rebased_folds = map.deferred.clone();
                self.reveal_mapping = false;
                self.mapping_folds = self
                    .global_folds
                    .iter()
                    .filter(|fold| self.global_fold_state.collapsed.contains(&fold.header))
                    .map(|fold| fold.header + 1..fold.end + 1)
                    .collect();
                self.mapped = Some(map);
                self.project_mapped_fold_gutter();
                self.project_global_selection();
                self.project_search_marks();
                let _ = self.project_global_spacers();
                self.error = None;
            }
            Ok(_) => {
                self.mapping_dirty = true;
            }
            Err(error) => {
                self.viewport_valid = false;
                self.mapping_dirty = true;
                self.reveal_mapping = false;
                self.error = Some(error);
            }
        }
        true
    }
    pub fn recovery_origin_path(&self) -> Option<PathBuf> {
        self.actor.recovery_origin()
    }
    pub fn viewport_ready(&self) -> bool {
        self.viewport_valid && !self.busy()
    }
    pub fn paged_frame_state(&self) -> PagedFrameState {
        PagedFrameState {
            displayed: self
                .source_offset(TextOffset(0), SourceAffinity::After)
                .unwrap_or(TextOffset(self.viewport_start))
                ..self
                    .source_offset(TextOffset(self.surface.snapshot.len()), SourceAffinity::Before)
                    .unwrap_or(TextOffset(self.viewport_start)),
            requested: self.queued_viewport.or(self.viewport_request),
            ready: self.viewport_ready()
                && self.viewport_request.is_none()
                && self.queued_viewport.is_none()
                && self.requested_scroll.is_none(),
        }
    }
    pub fn paged_scroll_metrics(&self, height: f32) -> PagedScrollMetrics {
        let snapshot = self.surface.snapshot();
        let (line, _, _) = self.surface.logical_scroll();
        let line = (line as usize).min(snapshot.line_count().saturating_sub(1));
        let start = snapshot.line_range(line).map_or(0, |range| range.start.0);
        let rows = ((height.max(0.0) as f64 / self.surface.line_height() as f64).ceil() as usize).max(1);
        let end = snapshot
            .line_range(line.saturating_add(rows))
            .map_or(snapshot.len(), |range| range.start.0);
        let viewport = end.saturating_sub(start).max(1).min(self.snapshot.len().max(1)) as f64;
        let total = self.snapshot.len().max(1) as f64;
        let offset = self
            .queued_viewport
            .or(self.viewport_request)
            .or_else(|| self.source_offset(TextOffset(start), SourceAffinity::After))
            .map_or(self.viewport_start, |offset| offset.0) as f64;
        PagedScrollMetrics {
            offset: offset.min((total - viewport).max(0.0)),
            viewport,
            total,
            lines_known: self.viewport_start == 0 && snapshot.len() == self.snapshot.len(),
        }
    }
    pub fn set_known_global_folds(
        &mut self,
        mut folds: Vec<bareline_syntax::folding::Fold>,
        level: usize,
        partial: bool,
        viewport_first_line: usize,
    ) -> Result<(), String> {
        if !self.viewport_valid {
            return Err("Wait for the paged viewport before projecting folds".into());
        }
        if folds.iter().any(|fold| fold.header >= fold.end) {
            return Err("Invalid global fold range".into());
        }
        if partial {
            folds.extend(self.global_folds.iter().cloned());
        }
        let truncated = folds.len() > 8192;
        folds.truncate(8192);
        folds.sort_by_key(|fold| (fold.header, std::cmp::Reverse(fold.end)));
        folds.dedup_by_key(|fold| (fold.header, fold.end));
        self.global_folds = folds;
        self.global_folds_partial = partial || truncated;
        self.fold_viewport_line = Some(viewport_first_line);
        if !self.global_fold_initialized {
            if level > 0 {
                self.global_fold_state.apply_level(&self.global_folds, level);
            }
            self.global_fold_initialized = true;
        } else {
            self.global_fold_state.refresh(&self.global_folds);
        }
        for (&header, &collapsed) in &self.global_fold_overrides {
            if collapsed {
                self.global_fold_state.collapsed.insert(header);
            } else {
                self.global_fold_state.collapsed.remove(&header);
            }
        }
        for range in &self.pending_global_folds {
            if let Some(fold) = self
                .global_folds
                .iter()
                .find(|fold| fold.header as u64 == range.start && fold.end as u64 + 1 == range.end)
            {
                self.global_fold_state.collapsed.insert(fold.header);
            }
        }
        if !self.global_folds_partial {
            self.pending_global_folds.clear();
        }
        self.project_global_folds();
        Ok(())
    }
    pub fn persisted_global_folds(&self) -> Vec<std::ops::Range<u64>> {
        if !self.pending_global_folds.is_empty() {
            return self.pending_global_folds.clone();
        }
        let mut folds: Vec<_> = self
            .global_folds
            .iter()
            .filter(|fold| self.global_fold_state.collapsed.contains(&fold.header))
            .map(|fold| fold.header as u64..fold.end as u64 + 1)
            .collect();
        // Collapsed folds still carried by their bytes, at the lines each change
        // moved them to (PED-07); one whose shift was not counted waits for a
        // mapping to look it up.
        let mut seen: std::collections::HashSet<_> = folds.iter().map(|range| (range.start, range.end)).collect();
        for anchor in &self.rebased_folds {
            let range = anchor.fold.header as u64..anchor.fold.end as u64 + 1;
            if anchor.collapsed
                && anchor.lines_known
                && anchor.fold.header < anchor.fold.end
                && seen.insert((range.start, range.end))
            {
                folds.push(range);
            }
        }
        folds
    }
    pub fn restore_global_folds(&mut self, ranges: &[std::ops::Range<u64>]) {
        self.pending_global_folds = ranges
            .iter()
            .filter(|range| range.start < range.end && usize::try_from(range.end).is_ok())
            .take(8192)
            .cloned()
            .collect();
        self.global_fold_initialized = true;
        self.global_fold_state.unfold_all();
        for range in &self.pending_global_folds {
            if let Some(fold) = self
                .global_folds
                .iter()
                .find(|fold| fold.header as u64 == range.start && fold.end as u64 + 1 == range.end)
            {
                self.global_fold_state.collapsed.insert(fold.header);
            }
        }
        self.project_global_folds();
    }
    pub fn fold_all_known(&mut self, level: usize) {
        self.global_fold_initialized = true;
        self.global_fold_overrides.clear();
        self.global_fold_state.apply_level(&self.global_folds, level);
        let level = level.clamp(1, 8);
        self.set_carried_folds(|anchor| anchor.collapsed || anchor.fold.level >= level);
        self.project_global_folds();
    }
    /// Real "Fold All": collapse every known region at every nesting level,
    /// independent of a level threshold (ARCH-20).
    pub fn fold_all_regions(&mut self) {
        self.global_fold_initialized = true;
        self.global_fold_overrides.clear();
        self.global_fold_state.fold_all(&self.global_folds);
        self.set_carried_folds(|_| true);
        self.project_global_folds();
    }
    pub fn unfold_all_known(&mut self) {
        self.global_fold_initialized = true;
        self.global_fold_overrides.clear();
        self.global_fold_state.unfold_all();
        self.set_carried_folds(|_| false);
        self.project_global_folds();
    }
    /// Sets the collapsed state of every fold still carried by its bytes, so
    /// Fold All and Unfold All reach folds no mapping has resolved yet (PED-07).
    fn set_carried_folds(&mut self, collapsed: impl Fn(&mapped_viewport::FoldAnchor) -> bool) {
        for anchor in &mut self.rebased_folds {
            let state = collapsed(&*anchor);
            if anchor.collapsed != state {
                anchor.collapsed = state;
                self.mapping_dirty = true;
            }
        }
    }
    pub fn toggle_current_known(&mut self) -> Result<(), String> {
        let caret = self.global_selection().1;
        let innermost = |anchors: &[mapped_viewport::FoldAnchor]| {
            anchors
                .iter()
                .enumerate()
                .filter(|(_, anchor)| anchor.header <= caret && caret < anchor.end)
                .max_by_key(|(_, anchor)| anchor.header)
                .map(|(index, anchor)| (index, anchor.header))
        };
        // A fold still carried by its bytes toggles in place (PED-07).
        let known = innermost(self.known_fold_anchors.as_slice());
        if let Some((index, header)) = innermost(self.rebased_folds.as_slice())
            && known.is_none_or(|(_, known)| known < header)
        {
            let anchor = &mut self.rebased_folds[index];
            anchor.collapsed = !anchor.collapsed;
            self.mapping_dirty = true;
            self.project_global_folds();
            return Ok(());
        }
        if let Some(header) = known.map(|(index, _)| self.known_fold_anchors[index].fold.header) {
            self.global_fold_state.toggle(header);
            self.global_fold_overrides
                .insert(header, self.global_fold_state.collapsed.contains(&header));
            self.project_global_folds();
            return Ok(());
        }
        let first = self.fold_viewport_line.ok_or("Global line mapping is pending")?;
        let local = self
            .surface
            .snapshot
            .line_at(TextOffset(self.surface.selection.caret))
            .map_err(|_| "Caret line unavailable")?;
        let line = if self.mapped.is_some() {
            self.surface
                .source_line_at(TextOffset(self.surface.selection.caret))
                .and_then(|line| usize::try_from(line).ok())
                .ok_or("Fold source line is unavailable")?
        } else {
            first.saturating_add(local)
        };
        let header = self
            .global_folds
            .iter()
            .filter(|fold| fold.header <= line && line <= fold.end)
            .max_by_key(|fold| fold.header)
            .map(|fold| fold.header)
            .ok_or("No verified fold at the caret")?;
        self.global_fold_state.toggle(header);
        self.global_fold_overrides
            .insert(header, self.global_fold_state.collapsed.contains(&header));
        self.project_global_folds();
        Ok(())
    }
    fn project_global_folds(&mut self) {
        self.queue_fold_projection();
        if self.mapped.is_some() || self.mapping_job.is_some() {
            self.project_mapped_fold_gutter();
            return;
        }
        let Some(first) = self.fold_viewport_line else {
            self.surface.set_known_folds(Vec::new(), 8, true);
            return;
        };
        let end = first.saturating_add(self.surface.snapshot.line_count());
        let length = self.surface.snapshot.len();
        let at_eof = self.viewport_start.saturating_add(length) == self.snapshot.len();
        let complete_line = length != 0
            && self
                .surface
                .snapshot
                .read(TextOffset(length - 1)..TextOffset(length), 1)
                .is_ok_and(|last| last == "\n" || last == "\r");
        let complete_end = if at_eof || complete_line {
            end
        } else {
            end.saturating_sub(1)
        };
        let first_complete = self.viewport_start == 0
            || self
                .viewport_first_line_start()
                .is_some_and(|start| start.0 == self.viewport_start);
        let folds = self
            .global_folds
            .iter()
            .filter(|fold| fold.header >= first && (fold.header != first || first_complete) && fold.end < complete_end)
            .map(|fold| bareline_syntax::folding::Fold {
                header: fold.header - first,
                end: fold.end - first,
                level: fold.level,
            })
            .collect();
        self.surface.set_known_folds(folds, 8, self.global_folds_partial);
        self.surface.fold_state.collapsed = self
            .global_fold_state
            .collapsed
            .iter()
            .filter(|header| **header >= first && **header < end)
            .map(|header| header - first)
            .collect();
        self.surface.refresh_hidden_lines();
    }
    pub fn set_known_anchored_folds(
        &mut self,
        anchors: Vec<bareline_syntax::folding::AnchoredFold>,
        level: usize,
        partial: bool,
        viewport_first_line: usize,
    ) -> Result<(), String> {
        if anchors.len() > 8192
            || anchors.iter().any(|anchor| {
                anchor.header >= anchor.body.start
                    || anchor.body.start > anchor.body.end
                    || anchor.body.end.0 > self.snapshot.len()
            })
        {
            return Err("Invalid verified fold anchors".into());
        }
        self.set_known_global_folds(
            anchors.iter().map(|anchor| anchor.fold.clone()).collect(),
            level,
            partial,
            viewport_first_line,
        )?;
        if !partial {
            self.known_fold_anchors.clear();
        }
        let anchors: Vec<_> = anchors
            .into_iter()
            .map(|anchor| mapped_viewport::FoldAnchor {
                header: anchor.header,
                body: anchor.body.start,
                end: anchor.body.end,
                collapsed: self.global_fold_state.collapsed.contains(&anchor.fold.header),
                fold: anchor.fold,
                lines_known: true,
            })
            .collect();
        self.merge_known_fold_anchors(anchors);
        Ok(())
    }
    /// Newer anchors replace known ones with the same header; the last duplicate
    /// wins. Set lookups keep Fold All installs linear (PED-19).
    fn merge_known_fold_anchors(&mut self, anchors: Vec<mapped_viewport::FoldAnchor>) {
        let latest: std::collections::HashMap<usize, usize> = anchors
            .iter()
            .enumerate()
            .map(|(index, anchor)| (anchor.header.0, index))
            .collect();
        self.known_fold_anchors
            .retain(|known| !latest.contains_key(&known.header.0));
        self.known_fold_anchors.extend(
            anchors
                .into_iter()
                .enumerate()
                .filter(|(index, anchor)| latest.get(&anchor.header.0) == Some(index))
                .map(|(_, anchor)| anchor),
        );
        self.known_fold_anchors.truncate(8192);
    }
    /// Ensure-visible: expand every collapsed fold whose hidden body contains a
    /// selection endpoint, so navigation and Find never leave the caret (and the
    /// next edit) inside hidden text. Returns true when a fold was expanded.
    ///
    /// Only endpoints count. An explicit selection that spans a whole collapsed
    /// fold (Select All, or a drag across it) deliberately replaces the folded
    /// lines with it, as in other editors; the edit then starts and ends in
    /// visible text.
    fn expand_folds_at_selection(&mut self) -> bool {
        if self.global_fold_state.collapsed.is_empty() && !self.rebased_folds.iter().any(|anchor| anchor.collapsed) {
            return false;
        }
        let length = self.snapshot.len();
        let mut offsets: Vec<usize> = self
            .global_selection_set()
            .selections
            .iter()
            .flat_map(|selection| [selection.anchor, selection.caret])
            .collect();
        offsets.sort_unstable();
        let holds = |anchor: &mapped_viewport::FoldAnchor| {
            let first = offsets.partition_point(|offset| *offset < anchor.body.0);
            anchor.body < anchor.end
                && offsets
                    .get(first)
                    .is_some_and(|offset| *offset < anchor.end.0 || anchor.end.0 == length)
        };
        let headers: std::collections::BTreeSet<usize> = self
            .known_fold_anchors
            .iter()
            .chain(self.mapped.iter().flat_map(|map| map.anchors.iter()))
            .filter(|anchor| self.global_fold_state.collapsed.contains(&anchor.fold.header) && holds(anchor))
            .map(|anchor| anchor.fold.header)
            .collect();
        for header in &headers {
            self.global_fold_state.collapsed.remove(header);
            self.global_fold_overrides.insert(*header, false);
        }
        // Folds still carried by their bytes reveal in place (PED-07).
        let mut carried = false;
        for anchor in &mut self.rebased_folds {
            if anchor.collapsed && holds(&*anchor) {
                anchor.collapsed = false;
                carried = true;
            }
        }
        if headers.is_empty() && !carried {
            return false;
        }
        if carried {
            self.mapping_dirty = true;
        }
        let generation = self.mapping_generation;
        self.project_global_folds();
        // Input waits for the projection this queued, so it never lands while
        // the revealed text is still drawn as hidden (PED-12). A projection
        // that could not be queued now (an edit or read is pending) follows the
        // commit's window, which shows every line.
        if self.mapping_generation != generation && self.mapping_job.is_some() {
            self.reveal_mapping = true;
        }
        true
    }
    fn project_mapped_fold_gutter(&mut self) {
        let headers: Vec<_> = self
            .global_folds
            .iter()
            .filter_map(|fold| {
                self.local_line_for_global(fold.header as u64).map(|local| {
                    (
                        local as usize,
                        fold.level,
                        self.global_fold_state.collapsed.contains(&fold.header),
                    )
                })
            })
            .collect();
        // These are gutter-only zero-body markers: the mapped source already omitted
        // the body. Feeding its old line count into hidden_lines would hide the suffix.
        self.surface.known_folds = headers
            .iter()
            .map(|(header, level, _)| bareline_syntax::folding::Fold {
                header: *header,
                end: *header,
                level: *level,
            })
            .collect();
        self.surface.fold_revision = Some(self.surface.snapshot.revision.0);
        self.surface.fold_state.collapsed = headers
            .into_iter()
            .filter(|(_, _, collapsed)| *collapsed)
            .map(|(header, _, _)| header)
            .collect();
        self.surface.refresh_hidden_lines();
    }
    pub fn viewport_first_global_line(&self) -> Option<u64> {
        self.viewport_mapping
            .filter(|mapping| self.viewport_valid && !self.busy() && mapping.offset == self.viewport_start)
            .map(|mapping| mapping.line)
    }
    /// While the fold map is still pending, install an exact single-window gutter
    /// mapping so the gutter shows real global line numbers (the window's first line
    /// plus the local index) instead of restarting at 1 on every scrolled window.
    /// Within one contiguous window the global line is exactly the first line of the
    /// window plus the local line index, so no estimate is involved once the window's
    /// first global line is known.
    fn project_window_gutter(&mut self) {
        if self.mapped.is_some() {
            return;
        }
        let Some(base) = self.viewport_first_global_line() else {
            return;
        };
        let len = self.surface.snapshot.len();
        self.surface.set_source_segments(&[ViewportSegment {
            local: TextOffset(0)..TextOffset(len),
            source: TextOffset(self.viewport_start)..TextOffset(self.viewport_start + len),
            first_global_line: Some(base),
            source_line_start: Some(TextOffset(self.viewport_start)),
        }]);
    }
    pub fn viewport_first_line_start(&self) -> Option<TextOffset> {
        self.viewport_mapping
            .filter(|mapping| self.viewport_valid && !self.busy() && mapping.offset == self.viewport_start)
            .map(|mapping| mapping.line_start)
    }
    /// Deterministic progress for the sparse line-number index the view keeps
    /// across navigation requests, or `None` before any index exists.
    pub fn index_fraction(&self) -> Option<f32> {
        self.navigation.index_progress(&self.snapshot)
    }
    fn refresh_gutter_accuracy(&mut self) -> bool {
        let indexed = self.navigation.indexed_line_count(&self.snapshot);
        let estimated = indexed.is_none();
        // The status bar's size group reports whole-document line completeness,
        // with scan progress while the index is still being built (UI-06/UI-07).
        self.surface.line_status = Some(match (self.snapshot.line_count(), indexed) {
            (bareline_document::paged::LineCount::Known(lines), _) | (_, Some(lines)) => crate::line_count_label(lines),
            (bareline_document::paged::LineCount::Unknown, None)
                if self.navigation.line_count_stopped(&self.snapshot) =>
            {
                "Line numbers estimated · count stopped".into()
            }
            (bareline_document::paged::LineCount::Unknown, None) => match self.index_fraction() {
                Some(fraction) if fraction < 0.999 => format!(
                    "Line numbers estimated · indexing {}%",
                    (fraction * 100.0).round() as u32
                ),
                _ => "Line numbers estimated · indexing".into(),
            },
        });
        self.surface.set_gutter_lines_estimated(estimated)
    }
    pub fn gutter_lines_estimated(&self) -> bool {
        self.surface.gutter_lines_estimated()
    }
    pub fn indexed_line_count(&self) -> Option<usize> {
        self.navigation.indexed_line_count(&self.snapshot)
    }
    pub fn active_gutter_line(&self) -> Option<u64> {
        self.surface
            .source_line_at(TextOffset(self.surface.selection.caret))
            .map(|line| line.saturating_add(1))
    }
    pub fn global_logical_scroll(&mut self) -> GlobalScrollPosition {
        if self.busy() || self.requested_scroll.is_some() || self.pending_scroll_mapping.is_some() {
            return GlobalScrollPosition::Pending;
        }
        if let Some(first) = self.viewport_first_global_line() {
            let (line, fraction, x) = self.surface.logical_scroll();
            if self.mapped.is_some() {
                let local = self
                    .surface
                    .snapshot
                    .line_range(line as usize)
                    .ok()
                    .map(|range| range.start);
                if local.is_some_and(|offset| {
                    self.source_segments().iter().any(|segment| {
                        segment.local.start == offset && segment.source_line_start != Some(segment.source.start)
                    })
                }) {
                    return GlobalScrollPosition::Pending;
                }
                return local
                    .and_then(|offset| self.surface.source_line_at(offset))
                    .map_or(GlobalScrollPosition::Pending, |global| {
                        GlobalScrollPosition::Ready(global, fraction, x)
                    });
            }
            // A clipped first line has no measured absolute horizontal origin.
            if line == 0
                && self
                    .viewport_first_line_start()
                    .is_some_and(|start| start.0 != self.viewport_start)
            {
                return GlobalScrollPosition::Pending;
            }
            GlobalScrollPosition::Ready(first.saturating_add(line), fraction, x)
        } else {
            self.ensure_viewport_mapping();
            GlobalScrollPosition::Pending
        }
    }
    pub fn request_global_scroll(&mut self, line: u64, fraction: f64, x: f64) -> Result<(), String> {
        if self.mapped.is_some()
            && let Some(local) = self.local_line_for_global(line)
        {
            self.surface.set_logical_scroll(local, fraction, x);
            return Ok(());
        }
        if let Some(first) = self.viewport_first_global_line()
            && line >= first
            && line - first < self.surface.snapshot.line_count() as u64
            && (line > first || self.viewport_first_line_start() == Some(TextOffset(self.viewport_start)))
        {
            self.surface.set_logical_scroll(line - first, fraction, x);
            return Ok(());
        }
        self.requested_scroll = Some((fraction, x));
        self.navigation_ready = None;
        self.navigation.request(
            self.read_handle(),
            crate::paged_navigation::NavigationTarget::Line(line),
            self.budget.clone(),
            self.notify.clone(),
        )
    }
    /// User wheel/trackpad scrolling. Crossing a loaded window requests an adjacent
    /// bounded viewport; the byte position remains useful while exact lines are pending.
    pub fn scroll_viewport(&mut self, delta: f64, height: f32) -> Result<(), String> {
        if !delta.is_finite() {
            return Ok(());
        }
        if self.following && delta < 0.0 {
            self.follow_paused = true;
        }
        if self.busy() || self.viewport_request.is_some() || self.requested_scroll.is_some() {
            self.queued_wheel.0 = (self.queued_wheel.0 + delta).clamp(-1.0e9, 1.0e9);
            self.queued_wheel.1 = height;
            return Ok(());
        }
        let line_height = self.surface.line_height() as f64;
        let end = self
            .source_offset(TextOffset(self.surface.snapshot.len()), SourceAffinity::Before)
            .map_or(self.viewport_start, |offset| offset.0);
        let at_top = self.surface.scroll_y + delta < 0.0;
        let at_bottom =
            self.surface.scroll_y + delta + height as f64 >= self.surface.snapshot.line_count() as f64 * line_height;
        if (delta < 0.0 && at_top && self.viewport_start != 0)
            || (delta > 0.0 && at_bottom && end < self.snapshot.len())
        {
            if let GlobalScrollPosition::Ready(line, fraction, x) = self.global_logical_scroll() {
                let position = fraction + delta / line_height;
                let rows = position.floor() as i64;
                let mut target = if rows < 0 {
                    line.saturating_sub(rows.unsigned_abs())
                } else {
                    line.saturating_add(rows as u64)
                };
                for fold in &self.global_folds {
                    if self.global_fold_state.collapsed.contains(&fold.header)
                        && (fold.header as u64) < target
                        && target <= fold.end as u64
                    {
                        target = if delta < 0.0 {
                            fold.header as u64
                        } else {
                            fold.end as u64 + 1
                        };
                    }
                }
                for range in &self.manual_hidden {
                    if range.start as u64 <= target && target < range.end as u64 {
                        target = if delta < 0.0 {
                            (range.start as u64).saturating_sub(1)
                        } else {
                            range.end as u64
                        };
                    }
                }
                return self.request_global_scroll(target, position - position.floor(), x);
            }
            let start = if delta < 0.0 {
                self.viewport_start.saturating_sub(WINDOW / 2)
            } else {
                self.viewport_start.saturating_add(WINDOW / 2).min(self.snapshot.len())
            };
            return self.request_viewport(TextOffset(start));
        }
        self.surface.scroll(delta, height);
        let margin = (height as f64 * 2.0).max(line_height * 8.0);
        if delta > 0.0 && self.surface.scroll_y + margin >= self.surface.snapshot.line_count() as f64 * line_height {
            self.prefetch.request(
                self.read_handle(),
                TextOffset(end),
                self.budget.clone(),
                self.notify.clone(),
            );
        } else if delta < 0.0 && self.surface.scroll_y < margin && self.viewport_start > 0 {
            self.prefetch.request(
                self.read_handle(),
                TextOffset(self.viewport_start.saturating_sub(WINDOW)),
                self.budget.clone(),
                self.notify.clone(),
            );
        }
        Ok(())
    }
    pub fn request_byte_scroll(&mut self, fraction: f64) -> Result<(), String> {
        self.request_byte_scroll_in_view(fraction, 600.0)
    }
    pub fn scroll_horizontal(&mut self, delta: f64) {
        if !delta.is_finite() {
            return;
        }
        if !self.paged_frame_state().ready {
            self.queued_horizontal = (self.queued_horizontal + delta).clamp(-1.0e9, 1.0e9);
        } else {
            self.surface.scroll_horizontal(delta);
        }
    }
    pub fn refine_horizontal_viewport(
        &mut self,
        backend: &impl bareline_renderer::TextBackend,
        width: f32,
    ) -> Result<bool, String> {
        // The bar spans the whole source line, so it can show while the loaded
        // window alone fits; keep its height clear below the last row (EDT-28).
        let bar = self.horizontal_scrollbar(width, 0.0);
        self.surface.horizontal_bar_reserved =
            self.paged_frame_state().ready && bar.total.is_some_and(|total| total > bar.viewport + 0.5);
        if !self.paged_frame_state().ready {
            return Ok(false);
        }
        let Some(anchor) = self.surface.horizontal_window_anchor(backend, width) else {
            return Ok(false);
        };
        let first = self
            .source_offset(TextOffset(0), SourceAffinity::After)
            .map_or(self.viewport_start, |offset| offset.0);
        let end = self
            .source_offset(TextOffset(self.surface.snapshot.len()), SourceAffinity::Before)
            .map_or(first, |offset| offset.0);
        if (anchor.direction < 0 && first == 0) || (anchor.direction > 0 && end >= self.snapshot.len()) {
            return Ok(false);
        }
        let Some(global_anchor) = self
            .source_offset(anchor.offset, SourceAffinity::After)
            .map(|offset| offset.0)
        else {
            return Ok(false);
        };
        let offset = if anchor.direction < 0 {
            first.saturating_sub(WINDOW / 2)
        } else {
            global_anchor.saturating_sub(WINDOW / 2)
        };
        if offset == self.viewport_start {
            return Ok(false);
        }
        if global_anchor < offset || global_anchor > offset.saturating_add(WINDOW).min(self.snapshot.len()) {
            return Ok(false);
        }
        self.request_viewport(TextOffset(offset))?;
        self.horizontal_anchor = Some((global_anchor, anchor.screen_x));
        Ok(true)
    }
    /// The loaded window's widest long line placed within its whole source
    /// line (EDT-28): the line's source start, and the source bytes of that
    /// line before and after the window. Only the window's first line can
    /// start before the window, and only its last can continue past it; the
    /// hidden parts are known once the window's line mapping has landed and
    /// count as zero until then.
    fn horizontal_source(&self) -> Option<(crate::HorizontalLine, usize, usize, usize)> {
        let line = self.surface.horizontal_line()?;
        let first = self.source_offset(TextOffset(line.start), SourceAffinity::After)?.0;
        let last = self.source_offset(TextOffset(line.end), SourceAffinity::Before)?.0;
        let mapping = self
            .viewport_mapping
            .filter(|mapping| self.viewport_valid && mapping.offset == self.viewport_start && line.start == 0);
        let before = mapping.map_or(0, |mapping| first.saturating_sub(mapping.line_start.0));
        let after = mapping
            .filter(|_| line.end == self.surface.snapshot.len())
            .map_or(0, |mapping| mapping.line_end.0.saturating_sub(last));
        Some((line, first - before, before, after))
    }
    fn horizontal_target_pending(&self, anchor: usize) -> bool {
        self.horizontal_anchor.is_some_and(|(offset, _)| offset == anchor)
            || self.surface.pending_horizontal_anchor.is_some_and(|(local, _, _)| {
                // Restoring snaps the anchor back onto a character boundary.
                self.source_offset(TextOffset(local), SourceAffinity::After)
                    .is_some_and(|global| global.0 <= anchor && anchor - global.0 < 4)
            })
    }
    /// The horizontal bar for this view, measured over whole source lines
    /// rather than the loaded window (EDT-28), so its thumb keeps its place
    /// when the window moves along a long line.
    pub fn horizontal_scrollbar(&self, width: f32, body_height: f32) -> bareline_ui::controls::HorizontalScrollbar {
        let mut bar = self.surface.horizontal_scrollbar(width, body_height);
        let Some(mut total) = bar.total else {
            return bar;
        };
        if let Some((line, _, before, after)) = self.horizontal_source() {
            let hidden = before as f64 * line.per_byte;
            bar.offset += hidden;
            total = total.max(hidden + line.width + after as f64 * line.per_byte);
        }
        if let Some((anchor, target)) = self.horizontal_target
            && self.horizontal_target_pending(anchor)
        {
            bar.offset = target;
        }
        bar.total = Some(total.max(bar.offset + bar.viewport));
        bar
    }
    pub fn needs_horizontal_scrollbar(&self, width: f32, body_height: f32) -> bool {
        crate::horizontal_bar_needed(&self.horizontal_scrollbar(width, body_height))
    }
    /// Pans so [`Self::horizontal_scrollbar`] reads `target` (EDT-28). A view
    /// that stays inside the loaded window pans the surface; one that would
    /// reach past it loads the window around the estimated source byte and
    /// anchors that byte at the left edge, as window refinement does.
    pub fn scroll_horizontal_to(&mut self, target: f64, viewport: f64) -> Result<(), String> {
        if !target.is_finite() || !self.paged_frame_state().ready {
            return Ok(());
        }
        let target = target.max(0.0);
        let Some((line, line_start, before, after)) = self.horizontal_source() else {
            self.surface.scroll_horizontal_to(target);
            return Ok(());
        };
        let local = target - before as f64 * line.per_byte;
        let outside = (local < 0.0 && before > 0) || (local + viewport > line.width && after > 0);
        if !outside || line.per_byte <= 0.0 {
            self.surface.scroll_horizontal_to(local);
            return Ok(());
        }
        let line_end = self
            .source_offset(TextOffset(line.end), SourceAffinity::Before)
            .map_or(line_start, |offset| offset.0)
            .saturating_add(after);
        let anchor = line_start
            .saturating_add((target / line.per_byte) as usize)
            .min(line_end)
            .min(self.snapshot.len());
        self.request_viewport(TextOffset(anchor.saturating_sub(WINDOW / 2)))?;
        self.horizontal_anchor = Some((anchor, 0.0));
        self.horizontal_target = Some((anchor, target));
        Ok(())
    }
    pub fn request_byte_scroll_in_view(&mut self, fraction: f64, height: f32) -> Result<(), String> {
        if !fraction.is_finite() {
            return Err("Invalid scrollbar position".into());
        }
        if self.following && fraction < 1.0 {
            self.follow_paused = true;
        }
        self.queued_wheel = (0.0, height);
        self.navigation.cancel();
        self.navigation_ready = None;
        self.requested_scroll = None;
        self.pending_scroll_mapping = None;
        self.bottom_scroll = (fraction >= 1.0).then_some(height);
        let extent = self.paged_scroll_metrics(height);
        let offset = if fraction >= 1.0 {
            self.snapshot.len().saturating_sub(WINDOW)
        } else {
            ((extent.total - extent.viewport).max(0.0) * fraction.clamp(0.0, 1.0)) as usize
        };
        self.request_viewport(TextOffset(offset))
    }
    pub fn byte_scroll_fraction(&self) -> f64 {
        if self.snapshot.is_empty() {
            0.0
        } else {
            self.viewport_start as f64 / self.snapshot.len() as f64
        }
    }
    pub fn set_global_spacers(&mut self, rows: &[(u64, u64)]) -> Result<(), String> {
        if rows.len() > 8192 {
            return Err("Too many comparison spacer rows".into());
        }
        self.global_spacers = rows.to_vec();
        self.project_global_spacers()
    }
    fn project_global_spacers(&mut self) -> Result<(), String> {
        let rows: Vec<_> = self
            .global_spacers
            .iter()
            .filter_map(|(line, count)| self.local_line_for_global(*line).map(|line| (line, *count)))
            .collect();
        self.surface.set_view_spacers(&rows)
    }
    fn ensure_viewport_mapping(&mut self) {
        if self.viewport_valid
            && self.viewport_mapping.is_none()
            && !self.navigation.is_pending()
            && self.navigation_ready.is_none()
            && self.requested_scroll.is_none()
        {
            if let Err(error) = self.navigation.request(
                self.read_handle(),
                crate::paged_navigation::NavigationTarget::Byte(TextOffset(self.viewport_start)),
                self.budget.clone(),
                self.notify.clone(),
            ) {
                self.error = Some(error);
            }
        }
    }
    /// Counts this text's lines in the background once, so the exact count
    /// arrives without a navigation to the end of the file (PERF-04).
    fn ensure_line_count(&mut self) {
        if self.captured.is_some() {
            return;
        }
        // The count's handle is not a view, so it never delays the last view's
        // cancellation.
        self.navigation.count_lines(
            &self.snapshot,
            || {
                self.actor
                    .read_handle(self.snapshot.clone(), self.view_generation.clone(), Arc::new(()))
            },
            self.budget.clone(),
            self.notify.clone(),
        );
    }
    fn pump_navigation(&mut self) -> bool {
        let mut changed = false;
        if let Some(result) = self.navigation.poll() {
            match result {
                Ok(result) => self.navigation_ready = Some(result),
                Err(error) => {
                    self.requested_scroll = None;
                    self.error = Some(error);
                }
            }
            changed = true;
        }
        if self.busy() {
            return changed;
        }
        let Some(result) = self.navigation_ready.take() else {
            return changed;
        };
        let handle = self.read_handle();
        if !result.snapshot.same_document(handle.snapshot())
            || result.snapshot.content_state != handle.snapshot().content_state
        {
            self.requested_scroll = None;
            return true;
        }
        let mapping = ViewportMapping {
            offset: result.offset.0,
            line: result.first_global_line,
            line_start: result.line_start,
            line_end: result.line_end,
        };
        if let Some((fraction, x)) = self.requested_scroll.take() {
            self.pending_scroll_mapping = Some((mapping, fraction, x));
            if let Err(error) = self.request_viewport(result.offset) {
                self.pending_scroll_mapping = None;
                self.error = Some(error);
            }
        } else if result.offset.0 == self.viewport_start {
            self.viewport_mapping = Some(mapping);
            self.fold_viewport_line = usize::try_from(mapping.line).ok();
            let _ = self.project_global_spacers();
            self.project_global_folds();
            self.project_window_gutter();
        }
        true
    }
    pub fn set_search_marks(&mut self, style: u8, ranges: Vec<std::ops::Range<TextOffset>>) -> Result<(), String> {
        if ranges.iter().any(|range| range.end.0 > self.snapshot.len()) {
            return Err("Mark is outside this paged generation".into());
        }
        self.search_marks.set(style, ranges)?;
        self.project_search_marks();
        Ok(())
    }
    pub fn clear_search_marks(&mut self, style: Option<u8>) {
        self.search_marks.clear(style);
        self.project_search_marks();
    }
    fn project_search_marks(&mut self) {
        self.surface.clear_search_marks(None);
        let fallback = [ViewportSegment {
            local: TextOffset(0)..TextOffset(self.surface.snapshot.len()),
            source: TextOffset(self.viewport_start)..TextOffset(self.viewport_start + self.surface.snapshot.len()),
            first_global_line: None,
            source_line_start: None,
        }];
        let segments = if self.mapped.is_some() {
            self.source_segments()
        } else {
            &fallback
        };
        let mut projected = Vec::new();
        for style in 1..=5 {
            let ranges = segments
                .iter()
                .flat_map(|segment| {
                    self.search_marks
                        .iter()
                        .filter(move |(s, range)| {
                            *s == style && range.start < segment.source.end && range.end > segment.source.start
                        })
                        .map(move |(_, range)| {
                            TextOffset(
                                segment.local.start.0 + range.start.0.max(segment.source.start.0)
                                    - segment.source.start.0,
                            )
                                ..TextOffset(
                                    segment.local.start.0 + range.end.0.min(segment.source.end.0)
                                        - segment.source.start.0,
                                )
                        })
                })
                .collect();
            projected.push((style, ranges));
        }
        for (style, ranges) in projected {
            let _ = self.surface.set_search_marks(style, ranges);
        }
    }
    pub fn append_receipt(&self) -> Option<bareline_file_io::tail::AppendReceipt> {
        self.append_receipt
    }
    pub fn follow_status(&self) -> Option<(bool, bool)> {
        self.following.then_some((self.follow_paused, self.tail_changed))
    }
    pub fn follow_banner_text(&self) -> Option<String> {
        self.follow_status().map(|(paused, changed)| {
            let path = self.actor.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if changed {
                format!("{name} · Source changed (rotated, truncated or rewritten) · Reopen and follow")
            } else if paused {
                format!("Following {name} · Paused (scrolled up) · Resume ↓ · Unlock to edit")
            } else {
                format!("Following {name} · Following new content · Pause · Unlock to edit")
            }
        })
    }
    pub fn start_follow(&mut self, platform: Arc<dyn LocalFileSystem>) -> Result<(), String> {
        if self.dirty() {
            return Err("Save or discard edits before monitoring.".into());
        }
        self.submit(Action::Tail {
            platform,
            request: true,
            follow: true,
        })?;
        self.following = true;
        self.surface.user_read_only = true;
        self.follow_paused = false;
        Ok(())
    }
    pub fn set_follow_paused(&mut self, paused: bool) {
        self.follow_paused = paused;
    }
    pub fn follow_tick(&mut self, platform: Arc<dyn LocalFileSystem>, request: bool) -> Result<(), String> {
        let request = request || self.follow_requested;
        if !self.following || self.tail_changed {
            return Ok(());
        }
        if self.busy() {
            // An append signalled during a check, a copy or the view's own reads is
            // requested again once the view is idle, not left for a later event (QA-08).
            self.follow_requested = request;
        } else if request || self.tail_pending {
            self.submit(Action::Tail {
                platform,
                request,
                follow: !self.follow_paused,
            })?;
            self.follow_requested = false;
        }
        Ok(())
    }
    pub fn unlock_follow(&mut self, confirmed: bool) -> Result<(), String> {
        if !confirmed {
            return Ok(());
        }
        self.submit(Action::UnlockTail)
    }
    pub fn dirty(&self) -> bool {
        !self.historical() && Some(self.snapshot.content_state) != self.actor.saved_state()
    }
    pub fn path(&self) -> PathBuf {
        self.actor.path()
    }
    pub fn fingerprint(&self) -> Fingerprint {
        self.actor.fingerprint()
    }
    pub fn save_as_required(&self) -> bool {
        self.actor.save_as_required()
    }
    pub fn viewport_start(&self) -> TextOffset {
        TextOffset(self.viewport_start)
    }
    /// Per-view read-only flag mirrored on the bounded viewport surface.
    pub fn user_read_only(&self) -> bool {
        self.surface.user_read_only
    }
    /// Set the per-view read-only flag on the bounded viewport surface.
    pub fn set_user_read_only(&mut self, value: bool) {
        self.surface.user_read_only = value;
    }
    /// Clear any in-flight IME composition held by the bounded viewport surface.
    pub fn cancel_composition(&mut self) {
        self.surface.cancel_composition();
    }
    /// Pump the paged actor together with its bounded viewport surface.
    pub fn pump_view(&mut self) -> bool {
        let changed = self.pump();
        changed | self.surface.pump()
    }
    /// Borrow the bounded viewport surface that presents the current window.
    pub fn viewport(&self) -> &EditorSurface {
        &self.surface
    }
    /// Mutably borrow the bounded viewport surface that presents the current window.
    pub fn viewport_mut(&mut self) -> &mut EditorSurface {
        &mut self.surface
    }
    /// Canonical endpoints are independent of the displayed, possibly clipped range.
    pub fn capture_power(&self) -> crate::paged_power::Capture {
        let selections = self.global_selection_set();
        let literal_contexts = selections
            .selections
            .iter()
            .map(|selection| {
                let local = self.local_offset(TextOffset(selection.caret))?;
                if self.source_offset(local, SourceAffinity::After) != Some(TextOffset(selection.caret)) {
                    return None;
                }
                let syntax = self.surface.typing_syntax.as_ref().filter(|syntax| {
                    syntax.is_current(&self.surface.snapshot)
                        && matches!(syntax.status, bareline_syntax::Status::Complete)
                        && (local.0 == 0 || syntax.range.start < local)
                        && local <= syntax.range.end
                })?;
                Some(syntax.spans.iter().any(|span| {
                    span.range.start.0 < local.0
                        && local.0 <= span.range.end.0
                        && matches!(
                            span.kind,
                            bareline_syntax::StyleKind::Comment | bareline_syntax::StyleKind::String
                        )
                }))
            })
            .collect();
        let mut state = self.power_state.clone();
        state.retain_rectangle_for(&selections);
        // Pairing and overtype follow the resident editor: plain text without a
        // user-defined language types closers verbatim.
        let pairs = self.surface.smart_typing
            && self.surface.smart_pairs
            && (self.surface.language != bareline_syntax::Language::PlainText || self.surface.udl.is_some());
        crate::paged_power::Capture {
            source: self.read_handle(),
            line_index: self.navigation.line_index().clone(),
            selections,
            literal_contexts,
            state,
            language: self.surface.language,
            definition: self.surface.udl.clone(),
            tab_width: self.surface.configured_tab_width(),
            column_maps: None,
            history_boundary: self.power_history_boundary,
            undo_run: self.undo_run,
            typing: crate::paged_typing::TypingConfig {
                language: self.surface.language,
                definition: self.surface.udl.clone(),
                smart_pairs: pairs,
                smart_indent: self.surface.smart_typing && self.surface.smart_indent,
                tab_width: self.surface.configured_tab_width(),
                literal_context: None,
                overtype: true,
            },
        }
    }
    pub fn install_power_state(
        &mut self,
        source: &PagedSnapshot,
        revision: bareline_document::Revision,
        selections: crate::power::SelectionSet,
        state: crate::paged_power::PowerViewState,
        hidden: &[std::ops::Range<u64>],
    ) -> Result<(), String> {
        if source.identity_token().0 != self.snapshot.identity_token().0 || revision != self.snapshot.revision {
            return Err("Power result source changed".into());
        }
        if selections.selections.is_empty()
            || selections.primary >= selections.selections.len()
            || selections
                .selections
                .iter()
                .any(|selection| selection.anchor > self.snapshot.len() || selection.caret > self.snapshot.len())
        {
            return Err("Invalid prepared power selections".into());
        }
        self.navigation_anchor = None;
        self.global_selections = crate::paged_power::normalize_selections(&selections).0;
        self.power_state = state;
        self.project_global_selection();
        if source.revision != revision && !self.power_state.hidden.is_empty() {
            self.power_hidden_refresh = true;
        }
        if !self.caret_in_viewport() {
            self.request_viewport(TextOffset(self.global_selections.primary().caret))?;
        }
        self.set_global_hidden_ranges(hidden)?;
        Ok(())
    }
    pub fn acknowledge_power_view(
        &mut self,
        source: &PagedSnapshot,
        id: &str,
        args: &crate::power::consumer::Arguments,
    ) -> Result<(), String> {
        if source.identity_token() != self.snapshot.identity_token()
            || source.content_state != self.snapshot.content_state
        {
            return Err("Power view source changed".into());
        }
        self.surface.acknowledge_command((id.into(), args.clone()));
        Ok(())
    }
    pub fn global_selection_set(&self) -> crate::power::SelectionSet {
        if self.surface.selection != self.projected_selection {
            let (anchor, caret) = self.global_selection();
            Selection {
                anchor: anchor.0,
                caret: caret.0,
            }
            .into()
        } else {
            self.global_selections.clone()
        }
    }
    pub fn global_selection(&self) -> (TextOffset, TextOffset) {
        let selection = if self.viewport_valid
            && self.surface.selection != self.projected_selection
            && self.surface.selection.anchor <= self.surface.snapshot.len()
            && self.surface.selection.caret <= self.surface.snapshot.len()
            && self
                .surface
                .snapshot
                .is_boundary(TextOffset(self.surface.selection.anchor))
            && self
                .surface
                .snapshot
                .is_boundary(TextOffset(self.surface.selection.caret))
        {
            match (
                self.source_offset(TextOffset(self.surface.selection.anchor), SourceAffinity::After),
                self.source_offset(TextOffset(self.surface.selection.caret), SourceAffinity::After),
            ) {
                (Some(anchor), Some(caret)) => Selection {
                    anchor: self.navigation_anchor.unwrap_or(anchor.0),
                    caret: caret.0,
                },
                _ => self.global_selections.primary(),
            }
        } else {
            self.global_selections.primary()
        };
        (TextOffset(selection.anchor), TextOffset(selection.caret))
    }
    fn sync_global_selection(&mut self) {
        let moved = self.surface.selection != self.projected_selection;
        let (anchor, caret) = self.global_selection();
        if moved {
            self.global_selections = Selection {
                anchor: anchor.0,
                caret: caret.0,
            }
            .into();
        }
        self.projected_selection = self.surface.selection;
        if moved {
            self.forget_selection_context();
        }
    }
    /// A caret or selection change ends a pending Shift-navigation anchor and
    /// any rectangle; either would otherwise silently redirect the next edit.
    fn forget_selection_context(&mut self) {
        self.navigation_anchor = None;
        self.power_state.clear_rectangle();
    }
    pub fn selection_fully_in_viewport(&self) -> bool {
        self.viewport_valid
            && self.global_selection_set().selections.iter().all(|selection| {
                self.local_offset(TextOffset(selection.anchor)).is_some()
                    && self.local_offset(TextOffset(selection.caret)).is_some()
            })
    }
    pub fn caret_in_viewport(&self) -> bool {
        let (_, caret) = self.global_selection();
        self.viewport_valid && self.local_offset(caret).is_some()
    }
    /// A restored selection needs no new window when its caret, and its anchor when
    /// the whole selection fits one window, are already displayed.
    fn selection_loaded(&self, selection: Selection) -> bool {
        self.viewport_valid
            && self.local_offset(TextOffset(selection.caret)).is_some()
            && (selection.anchor.abs_diff(selection.caret) > WINDOW.saturating_sub(8)
                || self.local_offset(TextOffset(selection.anchor)).is_some())
    }
    /// Moves selection endpoints inside the loaded window back to a character
    /// boundary, after clamping through changes the view could not map left
    /// them wherever the old offsets fell (PED-11).
    fn snap_selection_to_window(&mut self) {
        let start = self.viewport_start;
        let text = &self.surface.snapshot;
        let snap = |offset: usize| {
            let Some(mut local) = offset.checked_sub(start).filter(|local| *local <= text.len()) else {
                return offset;
            };
            while local > 0 && !text.is_boundary(TextOffset(local)) {
                local -= 1;
            }
            start + local
        };
        for selection in &mut self.global_selections.selections {
            selection.anchor = snap(selection.anchor);
            selection.caret = snap(selection.caret);
        }
    }
    fn project_global_selection(&mut self) {
        let length = self.surface.snapshot.len();
        let local = |offset: usize| {
            self.local_offset(TextOffset(offset)).map_or_else(
                || if offset < self.viewport_start { 0 } else { length },
                |offset| offset.0,
            )
        };
        // Clipping affects decoration only; it never writes back into the canonical endpoints.
        let primary = self.global_selections.primary();
        let projected = self
            .global_selections
            .selections
            .iter()
            .map(|selection| Selection {
                anchor: local(selection.anchor),
                caret: local(selection.caret),
            })
            .collect();
        self.projected_selection = Selection {
            anchor: local(primary.anchor),
            caret: local(primary.caret),
        };
        if !self
            .surface
            .snapshot
            .is_boundary(TextOffset(self.projected_selection.anchor))
            || !self
                .surface
                .snapshot
                .is_boundary(TextOffset(self.projected_selection.caret))
        {
            self.projected_selection = Selection::default();
        }
        self.surface.selection = self.projected_selection;
        self.surface.selections = crate::power::SelectionSet {
            selections: projected,
            primary: self.global_selections.primary,
        };
        if !self.caret_in_viewport() {
            self.surface.reveal_caret = false;
        }
    }
    pub fn selection_restore_status(&self, token: u64) -> SelectionRestoreStatus {
        if token == self.selection_token {
            self.selection_status.clone()
        } else {
            SelectionRestoreStatus::Superseded
        }
    }
    pub fn restore_global_selection(
        &mut self,
        anchor: TextOffset,
        caret: TextOffset,
        preserve_viewport: bool,
    ) -> Result<u64, String> {
        self.restore_global_selection_mode(anchor, caret, preserve_viewport, false, false)
    }
    fn restore_global_selection_mode(
        &mut self,
        anchor: TextOffset,
        caret: TextOffset,
        preserve_viewport: bool,
        snap_hit: bool,
        extend_hit: bool,
    ) -> Result<u64, String> {
        if self.pending.is_some() {
            return Err("Wait for the pending paged operation.".into());
        }
        if anchor.0 > self.snapshot.len() || caret.0 > self.snapshot.len() {
            return Err("Selection exceeds the document.".into());
        }
        let handle = self.read_handle();
        let snapshot = self.snapshot.clone();
        let budget = self.budget.clone();
        let cancellation = Cancellation::default();
        let request_cancellation = cancellation.clone();
        let notify = self.notify.clone();
        let historical = self.captured.is_some();
        let (sender, result) = mpsc::sync_channel(1);
        worker()
            .submit(
                WorkKind::Interactive,
                Box::new(move || {
                    let completion = JobCompletion::new(sender, notify);
                    let validation = (|| {
                        if !historical {
                            loop {
                                cancellation.check().map_err(|e| format!("Selection validation: {e}"))?;
                                let observed = handle.actor_generation();
                                match handle.try_original_store() {
                                    Ok(_) => break,
                                    Err(PagedLifecycleError::Busy) => {
                                        handle.wait_after(observed, ACTOR_WAIT);
                                    }
                                    Err(error) => return Err(error.to_string()),
                                }
                            }
                        }
                        for offset in [anchor, caret] {
                            let mut request = handle
                                .snapshot()
                                .begin_viewport(TextOffset(offset.0.saturating_sub(4)), 12, &budget)
                                .map_err(|e| format!("Selection validation: {e}"))?;
                            loop {
                                cancellation.check().map_err(|e| format!("Selection validation: {e}"))?;
                                let observed = handle.actor_generation();
                                match request.poll() {
                                    WindowPoll::Ready(window) => {
                                        let local = offset
                                            .0
                                            .checked_sub(window.range().start.0)
                                            .ok_or("Selection endpoint is unavailable")?;
                                        if local > window.text().len() || !window.text().is_char_boundary(local) {
                                            return Err("Selection endpoint is not a UTF-8 boundary".into());
                                        }
                                        break;
                                    }
                                    WindowPoll::Pending(ticket) => {
                                        let ready = if historical {
                                            handle
                                                .resolve_captured_page(ticket)
                                                .map_err(|error| error.to_string())?
                                        } else {
                                            handle.resolve_page(ticket).map_err(|error| error.to_string())?
                                        };
                                        if !ready {
                                            handle.wait_after(observed, ACTOR_WAIT);
                                        }
                                    }
                                    _ => return Err("Selection endpoint is unavailable".into()),
                                }
                            }
                        }
                        if !historical {
                            loop {
                                cancellation.check().map_err(|e| format!("Selection validation: {e}"))?;
                                let observed = handle.actor_generation();
                                match handle.try_original_store() {
                                    Ok(_) => break,
                                    Err(PagedLifecycleError::Busy) => {
                                        handle.wait_after(observed, ACTOR_WAIT);
                                    }
                                    Err(error) => return Err(error.to_string()),
                                }
                            }
                        }
                        let snapped = if snap_hit {
                            crate::paged_navigation::snap_grapheme(&handle, caret.0, &budget, &cancellation)?
                        } else {
                            caret.0
                        };
                        let selection = Selection {
                            anchor: if snap_hit && !extend_hit { snapped } else { anchor.0 },
                            caret: snapped,
                        };
                        let window = if preserve_viewport {
                            0
                        } else {
                            restore_window_start(&handle, selection, &budget, &cancellation)?
                        };
                        Ok((selection, window))
                    })();
                    completion.complete(validation);
                }),
            )
            .map_err(|_| "Paged worker queue is full; retry.".to_owned())?;
        self.selection_token = self.selection_token.wrapping_add(1);
        self.selection_status = SelectionRestoreStatus::Pending;
        self.selection_validation = Some(SelectionValidation {
            cancellation: request_cancellation,
            snapshot,
            preserve_viewport,
            recentre: false,
            result,
            completed: None,
        });
        Ok(self.selection_token)
    }
    fn pump_selection_validation(&mut self) -> bool {
        let Some(pending) = &mut self.selection_validation else {
            return false;
        };
        if pending.completed.is_none() {
            pending.completed = Some(match pending.result.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => Err("Selection validation worker stopped".into()),
            });
        }
        let same = pending.snapshot.same_document(&self.snapshot)
            && pending.snapshot.content_state == self.snapshot.content_state;
        // A peer may own the actor briefly. Retain the received validation and
        // token instead of converting temporary contention into a lost restore.
        if same && self.captured.is_none() && pending.completed.as_ref().is_some_and(Result::is_ok) {
            match self.read_handle().try_original_store() {
                Err(PagedLifecycleError::Busy) => {
                    (self.notify)();
                    return false;
                }
                Err(error) => {
                    self.selection_validation.as_mut().unwrap().completed = Some(Err(error.to_string()));
                }
                Ok(_) => {}
            }
        }
        let mut pending = self.selection_validation.take().unwrap();
        let result = if same {
            pending.completed.take().unwrap()
        } else {
            Err("Document changed while validating selection".into())
        };
        match result {
            Err(error) => {
                self.deferred_input = None;
                self.selection_status = SelectionRestoreStatus::Failed(error.clone());
                self.error = Some(error);
            }
            Ok((selection, window_start)) => {
                self.error = None;
                // A window-edge re-centre restores the selection it already had, so a
                // Shift-extended move keeps its sticky anchor across the edge.
                let anchor = if pending.recentre { self.navigation_anchor } else { None };
                self.forget_selection_context();
                self.navigation_anchor = anchor;
                self.global_selections = selection.into();
                self.project_global_selection();
                if pending.preserve_viewport {
                    self.surface.reveal_caret = false;
                }
                self.selection_status = SelectionRestoreStatus::Applied;
                if !pending.preserve_viewport {
                    // Navigation or Find landing in a collapsed body reveals it (PED-12).
                    self.expand_folds_at_selection();
                    // Keep the window when the selection is already loaded; otherwise
                    // centre a line-aligned window on it (PED-09).
                    if pending.recentre || !self.selection_loaded(selection) {
                        match self.request_viewport(TextOffset(window_start)) {
                            Ok(()) => self.reveal_after_read = true,
                            Err(error) => {
                                self.deferred_input = None;
                                self.selection_status = SelectionRestoreStatus::Failed(error.clone());
                                self.error = Some(error);
                            }
                        }
                    } else {
                        self.surface.reveal_caret = true;
                    }
                }
            }
        }
        true
    }
    /// Restore selection inside the current authoritative viewport without moving it.
    pub fn set_viewport_selection(&mut self, anchor: TextOffset, caret: TextOffset) -> Result<(), String> {
        if !self.viewport_ready() {
            return Err("Wait for the paged viewport to finish loading.".into());
        }
        let local = |offset: TextOffset| -> Result<usize, String> {
            if offset.0 > self.snapshot.len() {
                return Err("Selection exceeds the document.".into());
            }
            let value = self
                .local_offset(offset)
                .ok_or("Selection is outside the visible source segments")?
                .0;
            if value > self.surface.snapshot().len() || !self.surface.snapshot().is_boundary(TextOffset(value)) {
                return Err("Selection is outside the available UTF-8 viewport.".into());
            }
            Ok(value)
        };
        let anchor = local(anchor)?;
        let caret = local(caret)?;
        self.forget_selection_context();
        self.surface.selection.anchor = anchor;
        self.surface.selection.caret = caret;
        self.surface.selections = self.surface.selection.into();
        self.global_selections = Selection {
            anchor: self
                .source_offset(TextOffset(anchor), SourceAffinity::After)
                .ok_or("Invalid source anchor")?
                .0,
            caret: self
                .source_offset(TextOffset(caret), SourceAffinity::After)
                .ok_or("Invalid source caret")?
                .0,
        }
        .into();
        self.projected_selection = self.surface.selection;
        Ok(())
    }
    pub fn restore_selection(&mut self, anchor: TextOffset, caret: TextOffset) -> Result<(), String> {
        self.restore_global_selection(anchor, caret, false).map(|_| ())
    }
    pub fn request_viewport(&mut self, start: TextOffset) -> Result<(), String> {
        let start = TextOffset(start.0.min(self.snapshot.len()));
        self.horizontal_anchor = None;
        self.horizontal_target = None;
        self.prefetch.cancel();
        if self.busy() {
            self.queued_viewport = Some(start);
            return Ok(());
        }
        self.submit(Action::Read(start.0))?;
        self.error = None;
        self.viewport_request = Some(start);
        Ok(())
    }
    fn pump_viewport_requests(&mut self) -> bool {
        self.prefetch.poll();
        if self.busy() {
            return false;
        }
        self.viewport_request = None;
        if let Some(offset) = self.queued_viewport.take() {
            // Any horizontal anchor was set with this request after it was
            // queued (EDT-28); resubmitting must not drop it.
            let anchor = (self.horizontal_anchor, self.horizontal_target);
            if let Err(error) = self.request_viewport(offset) {
                self.error = Some(error);
            } else {
                (self.horizontal_anchor, self.horizontal_target) = anchor;
            }
            return true;
        }
        if let Some((anchor, _)) = self.horizontal_target
            && !self.horizontal_target_pending(anchor)
        {
            // The bar jump landed or was abandoned.
            self.horizontal_target = None;
        }
        if let Some(height) = self.bottom_scroll.take()
            && self.viewport_valid
            && self.error.is_none()
        {
            self.surface.scroll(f64::MAX, height);
        }
        if let Some((offset, screen_x)) = self.horizontal_anchor.take()
            && self.viewport_valid
            && self.error.is_none()
        {
            if let Some(mut local) = self.local_offset(TextOffset(offset)) {
                // A scroll-bar jump estimates its byte; land on a character.
                while local.0 > 0 && !self.surface.snapshot.is_boundary(local) {
                    local.0 -= 1;
                }
                if let Err(error) = self.surface.restore_horizontal_anchor(local, screen_x) {
                    self.error = Some(error);
                }
            }
        }
        if self.queued_horizontal != 0.0 {
            let delta = std::mem::take(&mut self.queued_horizontal);
            self.surface.scroll_horizontal(delta);
        }
        if self.requested_scroll.is_none() && self.queued_wheel.0 != 0.0 {
            let (delta, height) = std::mem::replace(&mut self.queued_wheel, (0.0, 0.0));
            if let Err(error) = self.scroll_viewport(delta, height) {
                self.error = Some(error);
            }
            return true;
        }
        false
    }
    pub fn save_prepared(
        &mut self,
        destination: bareline_file_io::lifecycle::PreparedDestination,
        platform: Arc<dyn LocalFileSystem>,
    ) -> Result<(), String> {
        if self.surface.user_read_only || self.historical() {
            return Err("Document is read only.".into());
        }
        self.submit(Action::Save {
            owner: None,
            copy_only: false,
            destination,
            platform,
        })
    }
    pub fn save_prepared_tracked(
        &mut self,
        destination: bareline_file_io::lifecycle::PreparedDestination,
        platform: Arc<dyn LocalFileSystem>,
    ) -> Result<PagedSaveOwner, String> {
        if self.surface.user_read_only || self.historical() {
            return Err("Document is read only.".into());
        }
        let owner = self.next_save_owner();
        self.submit(Action::Save {
            owner: Some(owner),
            copy_only: false,
            destination,
            platform,
        })?;
        self.pending_save_owner = Some(owner);
        Ok(owner)
    }
    pub fn save(
        &mut self,
        target: PathBuf,
        expected: Option<Fingerprint>,
        platform: Arc<dyn LocalFileSystem>,
    ) -> Result<(), String> {
        let condition = expected
            .map(bareline_file_io::lifecycle::DestinationCondition::ReplaceCaptured)
            .unwrap_or(bareline_file_io::lifecycle::DestinationCondition::MustBeAbsent);
        self.save_prepared(
            bareline_file_io::lifecycle::PreparedDestination {
                path: target,
                condition,
                consent: bareline_file_io::lifecycle::DestinationConsent::ExistingDocument,
                document: self.snapshot().identity_token(),
                operation: bareline_file_io::lifecycle::SaveOperation::Save,
            },
            platform,
        )
    }
    /// Export the captured document without changing its save identity or recovery.
    pub fn save_copy_prepared(
        &mut self,
        destination: bareline_file_io::lifecycle::PreparedDestination,
        platform: Arc<dyn LocalFileSystem>,
    ) -> Result<(), String> {
        self.submit(Action::Save {
            owner: None,
            copy_only: true,
            destination,
            platform,
        })
    }
    pub fn save_copy_prepared_tracked(
        &mut self,
        destination: bareline_file_io::lifecycle::PreparedDestination,
        platform: Arc<dyn LocalFileSystem>,
    ) -> Result<PagedSaveOwner, String> {
        let owner = self.next_save_owner();
        self.submit(Action::Save {
            owner: Some(owner),
            copy_only: true,
            destination,
            platform,
        })?;
        self.pending_save_owner = Some(owner);
        Ok(owner)
    }
    pub fn save_copy(&mut self, target: PathBuf, platform: Arc<dyn LocalFileSystem>) -> Result<(), String> {
        self.save_copy_prepared(
            bareline_file_io::lifecycle::PreparedDestination {
                path: target,
                condition: bareline_file_io::lifecycle::DestinationCondition::MustBeAbsent,
                consent: bareline_file_io::lifecycle::DestinationConsent::NotRequired,
                document: self.snapshot().identity_token(),
                operation: bareline_file_io::lifecycle::SaveOperation::SaveCopy,
            },
            platform,
        )
    }
    pub fn require_save_as(&mut self) {
        self.actor.require_save_as();
        if let Ok(mut peer) = self.peer.lock() {
            peer.epoch = peer.epoch.wrapping_add(1);
        }
    }
    fn next_save_owner(&mut self) -> PagedSaveOwner {
        PagedSaveOwner {
            document: self.snapshot.identity_token(),
            generation: self
                .save_generation
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .wrapping_add(1),
        }
    }
    pub fn pending_save_owner(&self) -> Option<PagedSaveOwner> {
        self.pending_save_owner
    }
    pub fn take_save_terminal(&self) -> Option<PagedSaveTerminal> {
        self.save_terminals.lock().ok()?.pop_front()
    }
    pub fn acknowledge_tracked_power(
        &mut self,
        receipt: &crate::TrackedEditReceipt,
        id: &str,
        args: &crate::power::consumer::Arguments,
    ) -> Result<(), String> {
        let revision = receipt.terminal().ok_or("Edit is still pending")??;
        if receipt.captured_identity_token.0 != self.snapshot.identity_token().0 || revision != self.snapshot.revision {
            return Err("Power receipt is not the current document revision".into());
        }
        if self.tracked_acknowledged.contains(&receipt.operation_id) {
            return Ok(());
        }
        if self.tracked_acknowledged.len() == 256 {
            self.tracked_acknowledged.pop_front();
        }
        self.tracked_acknowledged.push_back(receipt.operation_id);
        self.surface.acknowledge_command((id.into(), args.clone()));
        Ok(())
    }
    pub fn set_streaming_quota(&mut self, quota: u64) {
        self.streaming_quota = quota;
    }
    pub fn apply_prepared_source(
        &mut self,
        source: &PagedSnapshot,
        prepared: bareline_document::paged::PreparedSourceTransaction,
    ) -> Result<(), String> {
        if self.following || self.surface.user_read_only {
            return Err("Document is read-only".into());
        }
        if source.identity_token() != self.snapshot.identity_token()
            || source.content_state != self.snapshot.content_state
            || prepared.base_revision() != source.revision
        {
            return Err("Prepared source changed".into());
        }
        self.submit(Action::Source(prepared, None))
    }
    pub fn apply_prepared_source_tracked(
        &mut self,
        source: &PagedSnapshot,
        prepared: bareline_document::paged::PreparedSourceTransaction,
    ) -> Result<crate::TrackedEditReceipt, String> {
        if self.following || self.surface.user_read_only {
            return Err("Document is read-only".into());
        }
        if source.identity_token() != self.snapshot.identity_token()
            || source.content_state != self.snapshot.content_state
            || prepared.base_revision() != source.revision
        {
            return Err("Prepared source changed".into());
        }
        let receipt = crate::TrackedEditReceipt::new(source.identity_token());
        self.submit(Action::Source(
            prepared,
            Some(crate::tracked_edit::TrackedEditCompletion(receipt.clone())),
        ))?;
        Ok(receipt)
    }
    pub fn apply_prepared_tracked(
        &mut self,
        source: &PagedSnapshot,
        transaction: EditTransaction,
    ) -> Result<crate::TrackedEditReceipt, String> {
        if self.following || self.surface.user_read_only {
            return Err("Document is read-only".into());
        }
        if source.identity_token() != self.snapshot.identity_token()
            || source.content_state != self.snapshot.content_state
            || transaction.base_revision != source.revision
        {
            return Err("Replacement source changed".into());
        }
        let receipt = crate::TrackedEditReceipt::new(source.identity_token());
        self.submit(Action::Prepared(
            transaction,
            Some(crate::tracked_edit::TrackedEditCompletion(receipt.clone())),
            Default::default(),
        ))?;
        Ok(receipt)
    }
    /// Apply a small staged power input from memory. Its history metadata lets
    /// the actor merge consecutive single-caret typing into one undo step, and
    /// it needs no staging store.
    pub fn apply_materialized_power_tracked(
        &mut self,
        source: &PagedSnapshot,
        edit: crate::paged_power::MaterializedEdit,
    ) -> Result<crate::TrackedEditReceipt, String> {
        if self.following || self.surface.user_read_only {
            return Err("Document is read-only".into());
        }
        if source.identity_token() != self.snapshot.identity_token()
            || source.content_state != self.snapshot.content_state
            || edit.transaction.base_revision != source.revision
        {
            return Err("Prepared source changed".into());
        }
        let receipt = crate::TrackedEditReceipt::new(source.identity_token());
        self.submit(Action::Prepared(
            edit.transaction,
            Some(crate::tracked_edit::TrackedEditCompletion(receipt.clone())),
            edit.metadata,
        ))?;
        Ok(receipt)
    }
    /// Apply one reviewed multi-edit transaction through the paged actor and recovery journal.
    pub fn apply_prepared(&mut self, source: &PagedSnapshot, transaction: EditTransaction) -> Result<(), String> {
        if self.following || self.surface.user_read_only {
            return Err("Document is read-only".into());
        }
        if !source.same_document(&self.snapshot)
            || source.revision != self.snapshot.revision
            || source.content_state != self.snapshot.content_state
            || transaction.base_revision != source.revision
        {
            return Err("Replacement source changed; search again".into());
        }
        self.submit(Action::Prepared(transaction, None, Default::default()))
    }
    pub fn enqueue(&mut self, input: Input) {
        if (!self.paged_frame_state().ready || self.surface.horizontal_anchor_pending())
            && matches!(input, Input::SetCaret(..))
        {
            self.error = Some("Wait for the requested viewport before hit testing.".into());
            return;
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
        if !matches!(&input, Input::Insert(text) if text.chars().count() == 1) {
            // Navigation, deletion and every other input end the typing run.
            self.power_history_boundary = crate::power::consumer::next_receipt_sequence();
        }
        // Typing never edits hidden text: a collapsed body holding a selection
        // endpoint is revealed first (PED-12); a selection spanning a whole fold
        // replaces it by design. Staged power input already waits for the new
        // projection; the direct path replays the input once it is installed.
        if matches!(input, Input::Insert(_) | Input::Backspace | Input::Delete)
            && self.expand_folds_at_selection()
            && !self.power_input_enabled
        {
            self.deferred_input = Some(input);
            self.deferred_edge = false;
            return;
        }
        if self.power_input_enabled && matches!(input, Input::Insert(_) | Input::Backspace | Input::Delete) {
            if self.power_inputs.len() >= 256 {
                self.error = Some("Paged input queue is full".into());
                return;
            }
            self.power_inputs.push_back(input);
            (self.notify)();
            return;
        }
        if !self.viewport_valid && matches!(input, Input::Insert(_) | Input::Backspace | Input::Delete) {
            self.error = Some("Load an available viewport before editing.".into());
            return;
        }
        // A pending fold mapping does not hold input back; the previous mapping
        // stays until it lands, and an edit supersedes it (PED-07).
        if self.input_busy() {
            self.error = Some("Wait for the pending page or edit.".into());
            return;
        }
        self.sync_global_selection();
        if let Input::DocumentHome(extend) | Input::DocumentEnd(extend) = input {
            let caret = if matches!(input, Input::DocumentEnd(_)) {
                self.snapshot.len()
            } else {
                0
            };
            let anchor = if extend {
                self.global_selections.primary().anchor
            } else {
                caret
            };
            if let Err(error) = self.restore_global_selection(TextOffset(anchor), TextOffset(caret), false) {
                self.error = Some(error);
            }
            return;
        }
        if let Input::SetCaret(local, extend) = input {
            if local > self.surface.snapshot.len() || !self.surface.snapshot.is_boundary(TextOffset(local)) {
                self.error = Some("Caret is outside the available viewport".into());
                return;
            }
            let Some(caret) = self
                .source_offset(TextOffset(local), SourceAffinity::After)
                .map(|offset| offset.0)
            else {
                self.error = Some("Caret is outside visible source segments".into());
                return;
            };
            self.forget_selection_context();
            self.global_selections = Selection {
                anchor: if extend {
                    self.global_selections.primary().anchor
                } else {
                    caret
                },
                caret,
            }
            .into();
            self.project_global_selection();
            self.surface.acknowledge(Input::SetCaret(local, extend));
            return;
        }
        if matches!(input, Input::SelectAll) {
            if let Err(error) = self.restore_global_selection(TextOffset(0), TextOffset(self.snapshot.len()), true) {
                self.error = Some(error);
            }
            return;
        }
        if matches!(input, Input::Left(false) | Input::Right(false))
            && self.global_selections.primary().anchor != self.global_selections.primary().caret
        {
            let range = self.global_selections.primary().range();
            let offset = if matches!(input, Input::Left(_)) {
                range.start
            } else {
                range.end
            };
            if let Err(error) = self.restore_global_selection(TextOffset(offset), TextOffset(offset), false) {
                self.error = Some(error);
            }
            return;
        }
        let needs_caret = matches!(
            input,
            Input::Left(_)
                | Input::Right(_)
                | Input::Up(_)
                | Input::Down(_)
                | Input::Home(_)
                | Input::End(_)
                | Input::WordLeft(_)
                | Input::WordRight(_)
        ) || (matches!(input, Input::Backspace | Input::Delete)
            && self.global_selections.primary().anchor == self.global_selections.primary().caret);
        if needs_caret && !self.caret_in_viewport() {
            let (anchor, caret) = self.global_selection();
            match self.restore_global_selection(anchor, caret, false) {
                Ok(_) => {
                    self.deferred_input = Some(input);
                    self.deferred_edge = false;
                }
                Err(error) => self.error = Some(error),
            }
            return;
        }
        if !self.edge_replay && self.navigation_blocked_at_edge(&input) {
            self.recentre_and_replay(input);
            return;
        }
        let acknowledged = input.clone();
        let selected = self.global_selections.primary().range();
        let action = match input {
            Input::Insert(insert) => Some(Action::Edit {
                range: TextOffset(selected.start)..TextOffset(selected.end),
                insert,
            }),
            Input::Backspace | Input::Delete => {
                let backward = matches!(input, Input::Backspace);
                let range = if !selected.is_empty() {
                    selected
                } else {
                    let local = self.surface.selection.caret;
                    let caret = self.global_selections.primary().caret;
                    let target = if backward {
                        self.surface.previous_grapheme(local)
                    } else {
                        self.surface.next_grapheme(local)
                    };
                    let global = target.and_then(|target| {
                        let range = if backward {
                            self.source_offset(TextOffset(target), SourceAffinity::After)?.0..caret
                        } else {
                            caret..self.source_offset(TextOffset(target), SourceAffinity::Before)?.0
                        };
                        Some((range, local.abs_diff(target)))
                    });
                    match global {
                        // One grapheme is contiguous in the source; a longer global
                        // range crosses a fold seam and would delete its hidden body
                        // (PED-22).
                        Some((range, length)) if range.end.checked_sub(range.start) != Some(length) => {
                            self.error = Some("Unfold the hidden lines before deleting across them.".into());
                            return;
                        }
                        Some((range, _)) => range,
                        None => selected,
                    }
                };
                Some(Action::Edit {
                    range: TextOffset(range.start)..TextOffset(range.end),
                    insert: String::new(),
                })
            }
            Input::Undo => Some(Action::Undo),
            Input::Redo => Some(Action::Redo),
            navigation => {
                // A non-extending move collapses the selection; a stale anchor
                // would silently re-extend it once the surface caret moves.
                self.forget_selection_context();
                if matches!(
                    navigation,
                    Input::Left(true)
                        | Input::Right(true)
                        | Input::Up(true)
                        | Input::Down(true)
                        | Input::Home(true)
                        | Input::End(true)
                        | Input::WordLeft(true)
                        | Input::WordRight(true)
                ) {
                    self.navigation_anchor = Some(self.global_selections.primary().anchor);
                }
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
    pub fn click(
        &mut self,
        backend: &impl bareline_renderer::TextBackend,
        point: bareline_renderer::Point,
        extend: bool,
    ) -> Result<(), bareline_renderer::LayoutError> {
        if !self.paged_frame_state().ready || self.surface.horizontal_anchor_pending() {
            self.error = Some("Wait for the requested viewport before hit testing.".into());
            return Ok(());
        }
        if self.surface.composition.is_some() || point.y < self.surface.top() {
            return Ok(());
        }
        if (self.surface.text_left() - 26.0..self.surface.text_left()).contains(&point.x) {
            let row =
                ((point.y - self.surface.top()) as f64 + self.surface.scroll_y) / self.surface.line_height() as f64;
            let local = self.surface.logical_line(row.floor() as usize);
            let global = if self.mapped.is_some() {
                self.surface
                    .snapshot
                    .line_range(local)
                    .ok()
                    .and_then(|range| self.surface.source_line_at(range.start))
            } else {
                self.viewport_first_global_line().map(|first| first + local as u64)
            };
            if let Some(header) = global.and_then(|line| usize::try_from(line).ok())
                && self.global_folds.iter().any(|fold| fold.header == header)
            {
                self.global_fold_state.toggle(header);
                self.global_fold_overrides
                    .insert(header, self.global_fold_state.collapsed.contains(&header));
                self.project_global_folds();
            }
            return Ok(());
        }
        if point.x < self.surface.text_left() {
            return Ok(());
        }
        let row = ((point.y - self.surface.top()) as f64 + self.surface.scroll_y) / self.surface.line_height() as f64;
        let line = self.surface.logical_line(row.floor() as usize);
        let Some(layout) = self.surface.layouts.get(&line) else {
            return Ok(());
        };
        let hit = backend.hit_test(
            layout.id,
            bareline_renderer::Point {
                x: point.x - self.surface.text_left() + (self.surface.scroll_x - layout.x_origin) as f32,
                y: ((row - (self.surface.visual_line(line) + layout.row_origin) as f64)
                    * self.surface.line_height() as f64) as f32
                    + layout.context_y,
            },
        )?;
        let local = layout.start.saturating_add(hit.byte_offset).min(layout.end);
        let caret = self
            .source_offset(TextOffset(local), SourceAffinity::After)
            .ok_or(bareline_renderer::LayoutError::InvalidOffset)?;
        let anchor = if extend { self.global_selection().0 } else { caret };
        if let Err(error) = self.restore_global_selection_mode(anchor, caret, true, true, extend) {
            self.error = Some(error);
        }
        Ok(())
    }
    /// A submitted edit or read supersedes a pending fold mapping: its result
    /// would describe text or a window the completion replaces. The previous
    /// mapping stays on screen until then, and the completion queues the mapping
    /// again for what it installs (PED-07).
    fn supersede_mapping(&mut self) {
        if self.mapping_job.take().is_some() {
            self.mapping_dirty = true;
        }
        self.reveal_mapping = false;
    }
    fn submit(&mut self, action: Action) -> Result<(), String> {
        if matches!(action, Action::Undo | Action::Redo)
            && let Some(result) = transfer::try_history(self, matches!(action, Action::Undo))
        {
            return result;
        }
        if self.edit_actor_busy() {
            return Err("A paged operation is already pending.".into());
        }
        self.sync_global_selection();
        let moves_selection = matches!(
            &action,
            Action::Edit { .. }
                | Action::Prepared(..)
                | Action::Source(..)
                | Action::Undo
                | Action::Redo
                | Action::Tail { follow: true, .. }
        );
        if let Some(captured) = self.captured.clone() {
            let displayed_snapshot = self.snapshot.clone();
            let Action::Read(start) = action else {
                return Err("This is a read-only captured generation.".into());
            };
            let budget = self.budget.clone();
            let cancellation = self.cancellation.clone();
            let notify = self.notify.clone();
            let (sender, receiver) = mpsc::sync_channel(1);
            worker()
                .submit(
                    WorkKind::Interactive,
                    Box::new(move || {
                        let completion = JobCompletion::new(sender, notify);
                        let result = (|| {
                            let snapshot = displayed_snapshot;
                            let start = start.min(snapshot.len());
                            let mut request = snapshot
                                .begin_line_viewport(TextOffset(start), WINDOW, &budget)
                                .map_err(|e| e.to_string())?;
                            let window = loop {
                                cancellation.check().map_err(|e| e.to_string())?;
                                let observed = captured.actor_generation();
                                match request.poll() {
                                    WindowPoll::Ready(window) => break Ok(window),
                                    WindowPoll::Pending(ticket) => {
                                        if !captured
                                            .resolve_captured_page(ticket)
                                            .map_err(|error| error.to_string())?
                                        {
                                            captured.wait_after(observed, ACTOR_WAIT);
                                        }
                                    }
                                    WindowPoll::Unavailable(reason) => {
                                        break Err(format!("Captured source unavailable: {reason}"));
                                    }
                                    WindowPoll::InvalidUtf8 => {
                                        break Err("Captured source contains invalid UTF-8".into());
                                    }
                                    WindowPoll::Finished => break Err("Captured viewport already finished".into()),
                                }
                            };
                            Ok(Completed {
                                selections: None,
                                append_receipt: None,
                                generation_owner: captured.generation_owner(),
                                peer_epoch: 0,
                                following: false,
                                tail_pending: false,
                                source_changed: false,
                                can_undo: false,
                                can_redo: false,
                                snapshot,
                                window,
                                caret: start,
                                start,
                            })
                        })();
                        completion.complete(result);
                    }),
                )
                .map_err(|_| "Paged worker queue is full; retry.".to_owned())?;
            self.supersede_mapping();
            self.pending = Some(receiver);
            return Ok(());
        }
        // An edit with more edits already queued behind it defers its recovery append:
        // the burst's last edit journals the whole batch as one record, one durability
        // point instead of one per keystroke (PED-15). The composition root stages
        // every paged keystroke through this queue (`enable_power_input`), so typing
        // faster than an edit and its fsync forms the queue and is batched; the direct
        // path takes no input while an edit is pending and has nothing to batch.
        let defer_recovery =
            matches!(&action, Action::Edit { .. } | Action::Prepared(..)) && !self.power_inputs.is_empty();
        self.recovery_deferred |= defer_recovery;
        let actor = self.actor.clone();
        let source_owner = self.actor.clone();
        let peer = self.peer.clone();
        let streaming_quota = self.streaming_quota;
        let budget = self.budget.clone();
        let cancellation = self.cancellation.clone();
        let notify = self.notify.clone();
        let save_terminals = self.save_terminals.clone();
        let revision = self.snapshot.revision;
        let current_start = self.viewport_start;
        let current_caret = self.global_selections.primary().caret;
        let current_selections = self.global_selections.clone();
        let view_identity = self.snapshot.identity_token();
        let view_state = self.snapshot.content_state;
        let transforms_selection = matches!(&action, Action::Source(..) | Action::Undo | Action::Redo);
        let undo_run = self.undo_run;
        let work_kind = if matches!(&action, Action::Save { .. }) {
            WorkKind::Bulk
        } else {
            WorkKind::General
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        worker()
            .submit(
                work_kind,
                Box::new(move || {
                    let completion = JobCompletion::new(sender, notify.clone());
                    let result = (|| {
                        cancellation
                            .check()
                            .map_err(|_| PagedOperationError::Lifecycle(PagedLifecycleError::Cancelled))?;
                        let mut opened = actor.lock_document()?;
                        let mut tail = actor.lock_tail()?;
                        if tail.is_active()
                            && matches!(
                                &action,
                                Action::Edit { .. }
                                    | Action::Prepared(..)
                                    | Action::Source(..)
                                    | Action::Metadata(_)
                                    | Action::Undo
                                    | Action::Redo
                                    | Action::Save { .. }
                            )
                        {
                            return Err(
                                "Unlock and capture a fixed generation before editing or saving monitored content."
                                    .into(),
                            );
                        }
                        if opened.document().snapshot().revision != revision && !matches!(&action, Action::Read(_)) {
                            return Err("Document changed; retry the operation.".into());
                        }
                        let mut start = current_start;
                        let mut caret = current_caret;
                        let mut saved = None;
                        let mut committed_selection = None;
                        let baseline = opened.document().snapshot();
                        let previous_path = actor.path();
                        let previous_fingerprint = actor.fingerprint();
                        let previously_following = tail.is_active();
                        let mut retry_recovery = false;
                        let mut recovery_edits = Vec::new();
                        let mut streaming_protected = false;
                        match action {
                            Action::Tail {
                                platform,
                                request,
                                follow,
                            } => {
                                if !tail.is_active() {
                                    tail.start(&opened, platform, budget.clone(), cancellation.clone())?;
                                    if follow {
                                        caret = opened.document().snapshot().len();
                                        start = caret.saturating_sub(WINDOW / 2);
                                    }
                                }
                                if actor.step_tail(&mut opened, &mut tail, request)? && follow {
                                    caret = opened.document().snapshot().len();
                                    start = caret.saturating_sub(WINDOW / 2);
                                }
                            }
                            Action::UnlockTail => {
                                saved = Some(actor.unlock_tail(&mut opened, &mut tail)?);
                            }
                            Action::RetryRecovery => {
                                let lifecycle_stamp = actor.stamp_for(&opened);
                                drop(tail);
                                drop(opened);
                                actor.signal_document_released();
                                let receipt = actor.execute(
                                    bareline_file_io::paged_service::PagedLifecycleCommand::RetryRecovery {
                                        requested: lifecycle_stamp,
                                    },
                                    &cancellation,
                                );
                                let receipt_stamp = receipt.requested;
                                let rebuild = match receipt.terminal.map_err(PagedOperationError::Lifecycle)? {
                                    bareline_file_io::paged_service::PagedTerminalOutcome::RecoveryRetried {
                                        rebuild,
                                    } => rebuild,
                                    _ => return Err("Unexpected recovery receipt".into()),
                                };
                                opened = actor.lock_document()?;
                                if actor.stamp_for(&opened) != receipt_stamp {
                                    return Err(PagedOperationError::Lifecycle(
                                        bareline_file_io::paged_service::PagedLifecycleError::Changed,
                                    )
                                    .into());
                                }
                                tail = actor.lock_tail()?;
                                if rebuild {
                                    retry_recovery = true;
                                    recovery_edits.push(bareline_file_io::recovery::RecoveryEdit {
                                        offset: 0,
                                        removed: Vec::new(),
                                        inserted: Vec::new(),
                                    });
                                }
                            }
                            Action::Read(offset) => {
                                // Peers may have committed since this view's snapshot; keep
                                // the requested window over the same text through every
                                // commit (PED-13).
                                let chain = peer
                                    .lock()
                                    .ok()
                                    .and_then(|peer| {
                                        peer.changes.chain(
                                            (view_identity, view_state),
                                            (baseline.identity_token(), baseline.content_state),
                                        )
                                    })
                                    .or_else(|| {
                                        baseline
                                            .applied_change()
                                            .filter(|change| change.matches_before(view_identity, view_state))
                                            .map(|change| vec![change.clone()])
                                    });
                                start = chain.map_or(offset, |chain| {
                                    chain
                                        .iter()
                                        .fold(offset, |offset, change| map_offset_through(change, offset, false))
                                });
                                caret = start;
                            }
                            Action::Metadata(metadata) => {
                                let revision = opened.document().snapshot().revision;
                                opened
                                    .document_mut()
                                    .apply_metadata(revision, metadata)
                                    .map_err(|error| error.to_string())?;
                            }
                            Action::Source(prepared, completion) => {
                                actor.ensure_recovery(&opened, &baseline, notify.clone())?;
                                let lease = opened
                                    .document_mut()
                                    .lease_source_transaction(prepared)
                                    .map_err(|e| e.to_string())?;
                                actor.append_recovery_sources(lease.snapshot(), lease.edits(), streaming_quota)?;
                                if let Some(selection) = lease.metadata().after.first() {
                                    caret = selection.caret.0;
                                    start = caret.saturating_sub(WINDOW / 2);
                                    committed_selection = Some(crate::power::SelectionSet {
                                        selections: lease
                                            .metadata()
                                            .after
                                            .iter()
                                            .map(|selection| Selection {
                                                anchor: selection.anchor.0,
                                                caret: selection.caret.0,
                                            })
                                            .collect(),
                                        primary: 0,
                                    });
                                }
                                let revision = lease.publish();
                                if let Some(completion) = completion {
                                    completion.complete_once(Ok(revision));
                                }
                                streaming_protected = true;
                            }
                            Action::Prepared(transaction, completion, metadata) => {
                                if tail.is_active() {
                                    return Err("Monitoring document is read-only".into());
                                }
                                let snapshot = opened.document().snapshot();
                                if transaction.base_revision != snapshot.revision {
                                    return Err("Replacement source changed".into());
                                }
                                let mut windows = Vec::new();
                                let mut used = 0usize;
                                for edit in &transaction.edits {
                                    cancellation.check().map_err(|error| error.to_string())?;
                                    let length = edit
                                        .range
                                        .end
                                        .0
                                        .checked_sub(edit.range.start.0)
                                        .ok_or("Invalid replacement range")?;
                                    used = used
                                        .checked_add(length)
                                        .and_then(|value| value.checked_add(edit.insert.len() + 128))
                                        .ok_or("Replacement staging limit")?;
                                    if used > 16 * 1024 * 1024 {
                                        return Err("Replacement staging limit".into());
                                    }
                                    let window = read_window(
                                        &mut opened,
                                        &mut tail,
                                        &snapshot,
                                        edit.range.start.0.saturating_sub(4),
                                        length + 8,
                                        false,
                                        &budget,
                                        &cancellation,
                                        &source_owner,
                                    )?;
                                    let from = edit
                                        .range
                                        .start
                                        .0
                                        .checked_sub(window.range().start.0)
                                        .ok_or("Invalid replacement boundary")?;
                                    let to = edit
                                        .range
                                        .end
                                        .0
                                        .checked_sub(window.range().start.0)
                                        .ok_or("Invalid replacement boundary")?;
                                    let removed = window.text().get(from..to).ok_or("Invalid replacement boundary")?;
                                    recovery_edits.push(bareline_file_io::recovery::RecoveryEdit {
                                        offset: edit.range.start.0 as u64,
                                        removed: removed.as_bytes().to_vec(),
                                        inserted: edit.insert.as_bytes().to_vec(),
                                    });
                                    windows.push(window);
                                }
                                // Transform the view through the transaction so the caret
                                // never lands on an untransformed (possibly mid-scalar)
                                // offset, and record it for undo/redo (PED-11).
                                let edits: Vec<_> = transaction
                                    .edits
                                    .iter()
                                    .map(|edit| (edit.range.start.0..edit.range.end.0, edit.insert.len()))
                                    .collect();
                                let after = map_selections(&current_selections, |offset| {
                                    map_offset(edits.iter().cloned(), offset, true)
                                });
                                // Staged power input supplies its own history metadata
                                // (typing origin, boundary and selections, PED-15); other
                                // prepared transactions record the transformed view.
                                let mut metadata = metadata;
                                if metadata.before.is_empty() && metadata.after.is_empty() {
                                    metadata.before = history_selections(&current_selections);
                                    metadata.after = history_selections(&after);
                                }
                                crate::paged_power::tag_undo_run(&mut metadata, undo_run);
                                let revision = opened
                                    .document_mut()
                                    .apply_materialized_with_metadata(transaction, &windows, metadata)
                                    .map_err(|error| error.to_string())?;
                                if let Some(completion) = completion {
                                    completion.complete_once(Ok(revision));
                                }
                                caret = after.primary().caret;
                                start = map_offset(edits.iter().cloned(), start, false).min(caret);
                                committed_selection = Some(after);
                            }
                            Action::Edit { range, insert } => {
                                let length = range
                                    .end
                                    .0
                                    .checked_sub(range.start.0)
                                    .ok_or("Invalid replacement range")?;
                                if insert.len() > WINDOW || length > WINDOW {
                                    return Err("Edit exceeds the bounded viewport budget.".into());
                                }
                                let snapshot = opened.document().snapshot();
                                let window = read_window(
                                    &mut opened,
                                    &mut tail,
                                    &snapshot,
                                    range.start.0.saturating_sub(4),
                                    (length + 8).min(WINDOW + 8),
                                    false,
                                    &budget,
                                    &cancellation,
                                    &source_owner,
                                )?;
                                // Mirror the `Prepared` path: never index the read
                                // window with unchecked arithmetic.
                                let local_start = range
                                    .start
                                    .0
                                    .checked_sub(window.range().start.0)
                                    .ok_or("Invalid replacement boundary")?;
                                let local_end = range
                                    .end
                                    .0
                                    .checked_sub(window.range().start.0)
                                    .ok_or("Invalid replacement boundary")?;
                                let removed = window
                                    .text()
                                    .get(local_start..local_end)
                                    .ok_or("Invalid replacement boundary")?;
                                recovery_edits.push(bareline_file_io::recovery::RecoveryEdit {
                                    offset: range.start.0 as u64,
                                    removed: removed.as_bytes().to_vec(),
                                    inserted: insert.as_bytes().to_vec(),
                                });
                                caret = range.start.0 + insert.len();
                                // Undo and redo restore these selections (PED-11).
                                let primary = current_selections.primary();
                                let mut metadata = bareline_document::history::EditMetadata {
                                    before: vec![bareline_document::history::Selection {
                                        anchor: TextOffset(primary.anchor),
                                        caret: TextOffset(primary.caret),
                                    }],
                                    after: vec![bareline_document::history::Selection {
                                        anchor: TextOffset(caret),
                                        caret: TextOffset(caret),
                                    }],
                                    ..Default::default()
                                };
                                crate::paged_power::tag_undo_run(&mut metadata, undo_run);
                                opened
                                    .document_mut()
                                    .apply_materialized_with_metadata(
                                        EditTransaction {
                                            base_revision: revision,
                                            edits: vec![Edit { range, insert }],
                                        },
                                        &[window],
                                        metadata,
                                    )
                                    .map_err(|error| error.to_string())?;
                                start = start.min(caret);
                            }
                            Action::Undo | Action::Redo => {
                                let undo = matches!(action, Action::Undo);
                                // The entries of one finished macro run move as one step.
                                // The open run steps singly, so a macro's own recorded
                                // Undo stays a single step (WSP-09).
                                let run = opened
                                    .document()
                                    .history_metadata(undo)
                                    .and_then(crate::paged_power::undo_run_of)
                                    .filter(|run| Some(*run) != undo_run);
                                let mut first = true;
                                loop {
                                    // A journal failure degrades recovery; it never refuses the
                                    // user's Undo or Redo (FIO-03). After one, Undo and Redo stay
                                    // unjournaled until an edit starts a journal or the user
                                    // retries, so a lasting failure never creates and copies a
                                    // journal per keypress. Checked per step, so the rest of a
                                    // macro run stays unjournaled once one step missed it.
                                    let suspended = actor.recovery_suspended();
                                    let ensured = if suspended {
                                        Ok(())
                                    } else {
                                        actor.ensure_recovery(&opened, &baseline, notify.clone())
                                    };
                                    let step = (|| -> Result<_, PagedOperationError> {
                                        let prepared = opened
                                            .document()
                                            .prepare_source_history(undo, &budget)
                                            .map_err(|e| e.to_string())?;
                                        let lease = opened.document_mut().lease_source_history(prepared).map_err(
                                            |e| match e {
                                                // Reached when the linked-history probe found the
                                                // actor briefly busy (PED-21); the group path owns it.
                                                bareline_document::Error::LinkedUndoRequired => {
                                                    LINKED_HISTORY_BUSY.to_owned()
                                                }
                                                e => e.to_string(),
                                            },
                                        )?;
                                        let journaled = if suspended {
                                            Ok(())
                                        } else {
                                            ensured.and_then(|()| {
                                                actor.append_recovery_history(
                                                    lease.snapshot(),
                                                    lease.edits(),
                                                    streaming_quota,
                                                )
                                            })
                                        };
                                        let selections = if undo {
                                            &lease.metadata().before
                                        } else {
                                            &lease.metadata().after
                                        };
                                        let selections = (!selections.is_empty()).then(|| crate::power::SelectionSet {
                                            selections: selections
                                                .iter()
                                                .map(|selection| Selection {
                                                    anchor: selection.anchor.0,
                                                    caret: selection.caret.0,
                                                })
                                                .collect(),
                                            primary: 0,
                                        });
                                        lease.publish();
                                        if let Err(error) = journaled {
                                            // The journal missed this revision, so it is retired
                                            // rather than extended; the next edit starts a fresh one.
                                            actor.abandon_recovery(error.to_string());
                                        }
                                        Ok(selections)
                                    })();
                                    match step {
                                        Ok(Some(selections)) => {
                                            caret = selections.primary().caret;
                                            start = caret.saturating_sub(WINDOW / 2);
                                            committed_selection = Some(selections);
                                        }
                                        Ok(None) => {}
                                        Err(error) if first => return Err(error),
                                        // The steps before are committed (and journaled unless
                                        // recovery was retired); the group stops there.
                                        Err(_) => break,
                                    }
                                    first = false;
                                    // Log every step, so views map through all of them.
                                    let stepped = opened.document().snapshot();
                                    if let Some(change) = stepped.applied_change()
                                        && let Ok(mut peer) = peer.lock()
                                    {
                                        peer.changes.record(change);
                                    }
                                    if run.is_none()
                                        || opened
                                            .document()
                                            .history_metadata(undo)
                                            .and_then(crate::paged_power::undo_run_of)
                                            != run
                                        || cancellation.check().is_err()
                                    {
                                        break;
                                    }
                                }
                                streaming_protected = true;
                            }
                            Action::Save {
                                owner,
                                copy_only,
                                destination,
                                platform,
                            } => {
                                // Extract the save inputs while the actor is still
                                // locked; the save/fingerprint policy lives in
                                // `file-io`'s `paged_service`.
                                let snapshot = opened.document().snapshot();
                                if destination.document != snapshot.identity_token() {
                                    return Err("Save destination expired because the document changed.".into());
                                }
                                let lifecycle_stamp = actor.stamp_for(&opened);
                                // Release the actor for the duration of the full-file write.
                                // A Save is only reached when monitoring is off, so `tail`
                                // is `None`; both guards drop in reverse lock order (tail
                                // then actor) and are re-acquired in order afterwards, so
                                // peers and the read window keep serving this document
                                // instead of spinning on the actor throughout the write.
                                drop(tail);
                                drop(opened);
                                actor.signal_document_released();
                                let receipt = actor.execute(
                                    bareline_file_io::paged_service::PagedLifecycleCommand::Save {
                                        requested: lifecycle_stamp,
                                        destination,
                                        copy_only,
                                        platform,
                                    },
                                    &cancellation,
                                );
                                let saved_stamp = receipt.requested;
                                let applies = actor.receipt_applies(&receipt);
                                let disposition = match &receipt.terminal {
                                    Ok(bareline_file_io::paged_service::PagedTerminalOutcome::Saved {
                                        fingerprint,
                                        copy_only,
                                    }) => Ok((fingerprint.clone(), *copy_only)),
                                    Ok(_) => Err("Unexpected save receipt".to_owned()),
                                    Err(error) => Err(error.to_string()),
                                };
                                if let Some(owner) = owner {
                                    if let Ok(mut terminals) = save_terminals.lock() {
                                        terminals.push_back(PagedSaveTerminal { owner, receipt });
                                    }
                                    notify();
                                }
                                let (fingerprint, receipt_copy_only) =
                                    disposition.map_err(PagedOperationError::Message)?;
                                if !receipt_copy_only && applies {
                                    saved = Some(fingerprint);
                                    let _ = actor.execute(
                                        bareline_file_io::paged_service::PagedLifecycleCommand::RetireRecovery {
                                            requested: saved_stamp,
                                        },
                                        &cancellation,
                                    );
                                }
                                opened = actor.lock_document()?;
                                tail = actor.lock_tail()?;
                            }
                        }
                        let snapshot = opened.document().snapshot();
                        // History or source edits without recorded selections still move
                        // the caret through the committed change, never leaving it on a
                        // stale, possibly mid-scalar offset (PED-11).
                        if transforms_selection
                            && committed_selection.is_none()
                            && let Some(change) = snapshot.applied_change().filter(|change| {
                                change.matches_before(baseline.identity_token(), baseline.content_state)
                            })
                        {
                            let mapped =
                                map_selections(&current_selections, |offset| map_offset_through(change, offset, true));
                            caret = mapped.primary().caret;
                            start = caret.saturating_sub(WINDOW / 2);
                            committed_selection = Some(mapped);
                        }
                        let peer_epoch = {
                            let mut peer = peer.lock().map_err(|_| "Peer state stopped")?;
                            if snapshot.revision != baseline.revision
                                && let Some(change) = snapshot.applied_change()
                            {
                                peer.changes.record(change);
                            }
                            if snapshot.content_state != baseline.content_state
                                || snapshot.revision != baseline.revision
                                || actor.path() != previous_path
                                || actor.fingerprint() != previous_fingerprint
                                || saved.is_some()
                                || previously_following != tail.is_active()
                            {
                                peer.epoch = peer.epoch.wrapping_add(1);
                            }
                            peer.epoch
                        };
                        if !streaming_protected
                            && (!recovery_edits.is_empty() || snapshot.metadata() != baseline.metadata())
                        {
                            let _ = if defer_recovery && !retry_recovery {
                                actor.defer_recovery_edits(
                                    &opened,
                                    &baseline,
                                    &snapshot,
                                    &recovery_edits,
                                    notify.clone(),
                                )
                            } else {
                                actor.protect_recovery_edits(
                                    &opened,
                                    &baseline,
                                    &snapshot,
                                    &recovery_edits,
                                    retry_recovery,
                                    notify.clone(),
                                )
                            };
                        }
                        start = start.min(snapshot.len());
                        caret = caret.min(snapshot.len());
                        let window = read_window(
                            &mut opened,
                            &mut tail,
                            &snapshot,
                            start,
                            WINDOW,
                            true,
                            &budget,
                            &cancellation,
                            &source_owner,
                        );
                        Ok(Completed {
                            start,
                            selections: committed_selection,
                            append_receipt: tail.append_receipt(),
                            generation_owner: actor.current_generation_owner(),
                            peer_epoch,
                            following: tail.is_active(),
                            tail_pending: tail.pending(),
                            source_changed: tail.source_changed(),
                            can_undo: opened.document().can_undo(),
                            can_redo: opened.document().can_redo(),
                            snapshot,
                            window,
                            caret,
                        })
                    })();
                    // The actor guard has dropped with the inner closure; wake any reader
                    // waiting on the gate so it retries immediately instead of sleeping out
                    // its timeout.
                    actor.signal_document_released();
                    completion.complete(result);
                }),
            )
            .map_err(|_| "Paged worker queue is full; retry.".to_owned())?;
        self.supersede_mapping();
        self.pending = Some(receiver);
        self.pending_moves_selection = moves_selection;
        Ok(())
    }
    pub fn pump(&mut self) -> bool {
        let _ = transfer::pump_history(self);
        self.pump_owned_spill();
        self.sync_global_selection();
        let fold_changed = self.pump_fold_projection();
        let viewport_changed = self.pump_viewport_requests();
        let selection_changed = self.pump_selection_validation();
        let navigation_changed = self.pump_navigation();
        self.ensure_line_count();
        let gutter_accuracy_changed = self.refresh_gutter_accuracy();
        let Some(receiver) = &self.pending else {
            self.ensure_viewport_mapping();
            self.flush_deferred_recovery();
            // A restore that kept its window, or a fold reveal, still owes its input.
            let replayed = self.replay_deferred_input();
            return self.refresh_peer()
                || replayed
                || gutter_accuracy_changed
                || navigation_changed
                || selection_changed
                || viewport_changed
                || fold_changed;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                return gutter_accuracy_changed
                    || selection_changed
                    || navigation_changed
                    || viewport_changed
                    || fold_changed;
            }
            Err(TryRecvError::Disconnected) => Err("Paged worker stopped.".into()),
        };
        self.pending = None;
        self.pending_save_owner = None;
        let moves_selection = std::mem::take(&mut self.pending_moves_selection);
        match result {
            Ok(completed) => {
                self.viewport_mapping = None;
                self.fold_viewport_line = None;
                self.transition_fold_anchors(&completed.snapshot, completed.window.as_ref().ok());
                if completed.snapshot.content_state != self.snapshot.content_state {
                    // Every commit since this view's snapshot, from the shared log;
                    // peers may have committed several times before this refresh
                    // (PED-11, PED-13).
                    let from = (self.snapshot.identity_token(), self.snapshot.content_state);
                    let to = (completed.snapshot.identity_token(), completed.snapshot.content_state);
                    let chain = self
                        .peer
                        .lock()
                        .ok()
                        .and_then(|peer| peer.changes.chain(from, to))
                        .filter(|chain| !chain.is_empty())
                        .or_else(|| {
                            completed
                                .snapshot
                                .applied_change()
                                .filter(|change| change.matches_before(from.0, from.1))
                                .map(|change| vec![change.clone()])
                        });
                    if let Some(chain) = chain {
                        for change in &chain {
                            self.power_state_history.transition(
                                change.before_state,
                                change.after_state,
                                &mut self.power_state,
                                change,
                            );
                            // Marks follow every committed change, including typing,
                            // prepared source transactions, undo and redo (PED-20).
                            self.search_marks = self.search_marks.mapped_change(change);
                            if !moves_selection {
                                // A peer's commit shifts this view's selection (PED-13).
                                self.global_selections = map_selections(&self.global_selections, |offset| {
                                    map_offset_through(change, offset, false)
                                });
                                if let Some(anchor) = self.navigation_anchor {
                                    self.navigation_anchor = Some(map_offset_through(change, anchor, false));
                                }
                            }
                        }
                    } else {
                        self.power_state = crate::paged_power::PowerViewState::default();
                        self.power_state_history = Default::default();
                        self.search_marks.clear(None);
                        if !moves_selection {
                            let length = completed.snapshot.len();
                            self.navigation_anchor = None;
                            self.global_selections =
                                map_selections(&self.global_selections, |offset| offset.min(length));
                            // A clamped endpoint may split a character (PED-11).
                            self.resnap_selection = true;
                        }
                    }
                    self.power_hidden_refresh = !self.power_state.hidden.is_empty();
                }
                self.append_receipt = completed.append_receipt;
                self.peer_epoch = completed.peer_epoch;
                self.view_generation = completed.generation_owner;
                let was_following = self.following;
                self.following = completed.following;
                if self.following {
                    self.surface.user_read_only = true;
                }
                self.tail_pending = completed.tail_pending;
                self.tail_changed = completed.source_changed;
                if was_following && !self.following {
                    self.surface.user_read_only = false;
                }
                // Peer refreshes carry the actor's new disk identity after another
                // clone saves. Every live view must acknowledge that same identity.
                self.can_undo = completed.can_undo;
                self.can_redo = completed.can_redo;
                if let Some(input) = self.pending_input.take() {
                    self.surface.acknowledge(input);
                }
                self.viewport_valid = false;
                self.surface.set_eol_status_override(Some("Computing".into()));
                if self.captured.is_none() {
                    // The shared index keeps every checkpoint this change left
                    // intact instead of starting over (PED-07, PED-08). When it
                    // trails by several commits, the shared change log carries
                    // it through each of them.
                    let to = (completed.snapshot.identity_token(), completed.snapshot.content_state);
                    self.navigation
                        .line_index()
                        .follow_through(&completed.snapshot, |from| {
                            self.peer.lock().ok()?.changes.chain(from, to)
                        });
                }
                self.snapshot = completed.snapshot;
                self.refresh_gutter_accuracy();
                if moves_selection {
                    // The commit already happened: its selection is authoritative even
                    // when the following window read fails (PED-21).
                    // Power state follows the history transition above.
                    self.navigation_anchor = None;
                    // History lists the primary first; restore document order.
                    self.global_selections = completed.selections.map_or_else(
                        || {
                            Selection {
                                anchor: completed.caret,
                                caret: completed.caret,
                            }
                            .into()
                        },
                        |selections| crate::paged_power::normalize_selections(&selections).0,
                    );
                }
                let window = match completed.window {
                    Ok(window) => window,
                    Err(error) => {
                        self.error = Some(error);
                        if moves_selection {
                            // One retry of the window over the committed text; a failed
                            // plain read does not queue another.
                            self.queued_viewport = Some(TextOffset(completed.start));
                        }
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
                        // The commit's window shows its text without folds until
                        // the projection queued below lands; carried folds keep
                        // their state meanwhile (PED-07).
                        self.mapped = None;
                        self.mapping_job = None;
                        self.reveal_mapping = false;
                        self.surface.set_source_segments(&[]);
                        self.viewport_valid = true;
                        self.viewport_start = window.range().start.0;
                        self.surface.snapshot = snapshot;
                        self.surface.layout_revision = None;
                        self.surface.scroll_y = 0.0;
                        if std::mem::take(&mut self.resnap_selection) {
                            self.snap_selection_to_window();
                        }
                        self.project_global_selection();
                        if std::mem::take(&mut self.reveal_after_read) && self.caret_in_viewport() {
                            self.surface.reveal_caret = true;
                        }
                        if let Some((mapping, fraction, x)) = self.pending_scroll_mapping.take()
                            && mapping.offset == self.viewport_start
                        {
                            self.viewport_mapping = Some(mapping);
                            self.fold_viewport_line = usize::try_from(mapping.line).ok();
                            self.surface.set_logical_scroll(0, fraction, x);
                        }
                        self.project_window_gutter();
                        let _ = self.project_global_spacers();
                        self.project_global_folds();
                        self.project_search_marks();
                        // Line-count progress belongs to the status bar's size group
                        // (refresh_gutter_accuracy), never to text painted over the
                        // document (UI-06); a fresh window clears stale notices.
                        self.surface.error = None;
                        self.error = None;
                    }
                    Err(error) => self.error = Some(format!("Viewport unavailable: {error}")),
                }
            }
            Err(error) => {
                let input = self.pending_input.take();
                let error = error.to_string();
                // The linked-history probe met a briefly held document lock, and the
                // worker then found a linked entry: replay the same Undo or Redo once
                // idle so the group path runs it, instead of failing it (PED-21, QA-07).
                if error == LINKED_HISTORY_BUSY
                    && self.deferred_input.is_none()
                    && let Some(input @ (Input::Undo | Input::Redo)) = input
                {
                    self.deferred_input = Some(input);
                    self.deferred_edge = false;
                } else {
                    self.error = Some(error);
                }
            }
        }
        self.ensure_viewport_mapping();
        self.pump_viewport_requests();
        self.replay_deferred_input();
        // The result just consumed may have left the view idle with a batch deferred.
        self.flush_deferred_recovery();
        true
    }
    /// A burst's last edit journals the recovery batch its earlier edits deferred.
    /// When the queued input made no edit after all, the batch is journaled once the
    /// view is idle (PED-15), on a worker, which wakes the view when it is durable.
    fn flush_deferred_recovery(&mut self) {
        if !self.recovery_deferred || self.busy() {
            return;
        }
        if !self.actor.recovery_deferred() {
            // The burst's last edit journaled it.
            self.recovery_deferred = false;
            return;
        }
        let actor = self.actor.clone();
        let notify = self.notify.clone();
        self.recovery_deferred = worker()
            .submit(
                WorkKind::General,
                Box::new(move || {
                    let _ = actor.flush_deferred_recovery();
                    notify();
                }),
            )
            .is_err();
    }
    /// Recovery appends wait in a deferred batch (PED-15): the latest text is not
    /// yet durable even when the view is idle.
    pub fn recovery_batch_pending(&self) -> bool {
        self.recovery_deferred || self.actor.recovery_deferred()
    }
    /// Replay an input deferred behind a selection restore, window read or fold
    /// reveal once the view is idle.
    fn replay_deferred_input(&mut self) -> bool {
        if self.busy() {
            return false;
        }
        let Some(input) = self.deferred_input.take() else {
            return false;
        };
        // A replay after a window-edge re-centre never re-centres again, so a line
        // longer than a window cannot loop (PED-14).
        self.edge_replay = std::mem::take(&mut self.deferred_edge);
        self.enqueue(input);
        self.edge_replay = false;
        true
    }
    /// Navigation that cannot move inside the loaded window, although the document
    /// continues past that edge (PED-14).
    fn navigation_blocked_at_edge(&self, input: &Input) -> bool {
        let snapshot = &self.surface.snapshot;
        let length = snapshot.len();
        let caret = self.surface.selection.caret;
        if !self.viewport_valid || caret > length {
            return false;
        }
        let first = self
            .source_offset(TextOffset(0), SourceAffinity::After)
            .map_or(self.viewport_start, |offset| offset.0);
        let end = self
            .source_offset(TextOffset(length), SourceAffinity::Before)
            .map_or(first, |offset| offset.0);
        let more_before = first > 0;
        let more_after = end < self.snapshot.len();
        let line = snapshot.line_at(TextOffset(caret)).unwrap_or(0);
        let last_line = line + 1 >= snapshot.line_count();
        match input {
            Input::Left(_) | Input::WordLeft(_) => more_before && caret == 0,
            Input::Right(_) | Input::WordRight(_) => more_after && caret == length,
            Input::Up(_) => more_before && line == 0,
            Input::Down(_) | Input::End(_) => more_after && last_line,
            Input::Home(_) => more_before && line == 0 && self.viewport_first_line_start() != Some(TextOffset(first)),
            _ => false,
        }
    }
    /// Request a window centred on the caret, then replay `input` there (PED-14).
    fn recentre_and_replay(&mut self, input: Input) {
        let (anchor, caret) = self.global_selection();
        match self.restore_global_selection(anchor, caret, false) {
            Ok(_) => {
                if let Some(pending) = &mut self.selection_validation {
                    pending.recentre = true;
                }
                self.deferred_input = Some(input);
                self.deferred_edge = true;
            }
            Err(error) => self.error = Some(error),
        }
    }
}
impl Drop for PagedEditorSurface {
    fn drop(&mut self) {
        if Arc::strong_count(&self.views) == 1 {
            self.cancellation.cancel();
            // A deferred recovery batch (PED-15) is journaled on a worker that holds
            // the session until then, so closing the last view never waits on its
            // fsync and the journal's own drop finds nothing left to write.
            if self.recovery_batch_pending() {
                let actor = self.actor.clone();
                let _ = worker().submit(
                    WorkKind::General,
                    Box::new(move || {
                        let _ = actor.flush_deferred_recovery();
                    }),
                );
            }
        }
    }
}
/// Map one offset through non-overlapping edits given in pre-edit coordinates as
/// `(replaced range, inserted length)`. An offset inside a replaced range moves to
/// the end of its replacement, so the result is always a boundary of the new text.
/// A pure insertion exactly at the offset moves it only when `after` is set (the
/// editing view's own caret); peers and window starts stay before it.
fn map_offset(edits: impl Iterator<Item = (std::ops::Range<usize>, usize)>, offset: usize, after: bool) -> usize {
    let mut delta: i128 = 0;
    let mut inside = None;
    for (range, inserted) in edits {
        if range.end < offset || (range.end == offset && (range.start < offset || after)) {
            delta += inserted as i128 - (range.end - range.start) as i128;
        } else if range.start < offset {
            inside = Some(range.start + inserted);
        }
    }
    usize::try_from(inside.unwrap_or(offset) as i128 + delta).unwrap_or(0)
}
/// Most bytes a change may replace or insert for a view to count the lines it
/// moves; past this, carried folds keep only their bytes (PED-07).
const LINE_SHIFT_BYTES: usize = 1024 * 1024;
/// How one change moves lines: its edits span `start..end` of the old text; an
/// offset at `end` moves `at` lines and one past it `past` lines.
#[derive(Clone, Copy)]
struct LineShift {
    start: usize,
    end: usize,
    at: isize,
    past: isize,
}
/// `anchor` moved through `change` into a text of `len` bytes, or `None` when an
/// edit overlaps it. Its lines move with `shift` when it lies wholly before or
/// after the edits; otherwise they are no longer known (PED-07).
fn rebase_fold_anchor(
    change: &bareline_document::change::AppliedChange,
    shift: Option<LineShift>,
    anchor: &mapped_viewport::FoldAnchor,
    collapsed: bool,
    len: usize,
) -> Option<mapped_viewport::FoldAnchor> {
    let overlaps = change.edits().iter().any(|edit| {
        if edit.before.is_empty() {
            anchor.header < edit.before.start && edit.before.start < anchor.end
        } else {
            edit.before.start < anchor.end && edit.before.end > anchor.header
        }
    });
    if overlaps {
        return None;
    }
    let moved = |offset: TextOffset, before: bool| -> Option<TextOffset> {
        let mut result = offset.0 as i128;
        for edit in change.edits() {
            if edit.before.end < offset || (edit.before.end == offset && !(before && edit.before.is_empty())) {
                result += edit.inserted_len as i128 - (edit.before.end.0 - edit.before.start.0) as i128;
            }
        }
        usize::try_from(result)
            .ok()
            .filter(|offset| *offset <= len)
            .map(TextOffset)
    };
    let mut fold = anchor.fold.clone();
    let lines_known = anchor.lines_known
        && match shift {
            _ if change.edits().is_empty() => true,
            Some(shift) if anchor.end.0 < shift.start => true,
            Some(shift) if anchor.header.0 >= shift.end => {
                let header = if anchor.header.0 == shift.end {
                    shift.at
                } else {
                    shift.past
                };
                match (
                    fold.header.checked_add_signed(header),
                    fold.end.checked_add_signed(shift.past),
                ) {
                    (Some(header), Some(end)) => {
                        fold.header = header;
                        fold.end = end;
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        };
    Some(mapped_viewport::FoldAnchor {
        header: moved(anchor.header, false)?,
        body: moved(anchor.body, false)?,
        end: moved(anchor.end, true)?,
        fold,
        collapsed,
        lines_known,
    })
}
pub(crate) fn map_offset_through(
    change: &bareline_document::change::AppliedChange,
    offset: usize,
    after: bool,
) -> usize {
    map_offset(
        change
            .edits()
            .iter()
            .map(|edit| (edit.before.start.0..edit.before.end.0, edit.inserted_len)),
        offset,
        after,
    )
}
fn map_selections(set: &crate::power::SelectionSet, map: impl Fn(usize) -> usize) -> crate::power::SelectionSet {
    crate::power::SelectionSet {
        selections: set
            .selections
            .iter()
            .map(|selection| Selection {
                anchor: map(selection.anchor),
                caret: map(selection.caret),
            })
            .collect(),
        primary: set.primary,
    }
}
/// History metadata keeps at most 1,024 selections; larger sets record the primary.
fn history_selections(set: &crate::power::SelectionSet) -> Vec<bareline_document::history::Selection> {
    let selections = if set.selections.len() > 1024 {
        vec![set.primary()]
    } else {
        set.selections.clone()
    };
    selections
        .into_iter()
        .map(|selection| bareline_document::history::Selection {
            anchor: TextOffset(selection.anchor),
            caret: TextOffset(selection.caret),
        })
        .collect()
}
/// Window start for a restored selection (PED-09): centred on the whole selection
/// when it fits one window, otherwise on the caret; never short of a full window at
/// EOF; then advanced to the next line start so the first displayed line is whole.
fn restore_window_start(
    handle: &PagedReadHandle,
    selection: Selection,
    budget: &Budget,
    cancellation: &Cancellation,
) -> Result<usize, String> {
    let length = handle.snapshot().len();
    let range = selection.range();
    let (centre, limit) = if range.end - range.start <= WINDOW.saturating_sub(8) {
        (range.start + (range.end - range.start) / 2, range.start)
    } else {
        (selection.caret, selection.caret)
    };
    let start = centre
        .saturating_sub(WINDOW / 2)
        .min(length.saturating_sub(WINDOW))
        .min(limit);
    crate::paged_navigation::snap_line_start(handle, start, limit, budget, cancellation)
}
fn read_window(
    opened: &mut bareline_file_io::paged_service::PagedDocumentGuard<'_>,
    tail: &mut bareline_file_io::paged_service::PagedTailGuard<'_>,
    snapshot: &PagedSnapshot,
    start: usize,
    count: usize,
    display: bool,
    budget: &Budget,
    cancellation: &Cancellation,
    session: &PagedSession,
) -> Result<TextWindow, String> {
    // Display windows never start or end inside a CRLF (PED-10); edit windows keep
    // their exact edges so the edited range stays covered.
    let mut request = if display {
        snapshot.begin_line_viewport(TextOffset(start), count, budget)
    } else {
        snapshot.begin_viewport(TextOffset(start), count, budget)
    }
    .map_err(|error| error.to_string())?;
    loop {
        cancellation.check().map_err(|error| error.to_string())?;
        match request.poll() {
            WindowPoll::Ready(window) => return Ok(window),
            WindowPoll::Pending(ticket) => {
                let owned = snapshot
                    .resolve_owned(ticket)
                    .map_err(|error| format!("Owned page unavailable: {error}"))?;
                let handled = owned || tail.read_page(ticket).map_err(|error| error.to_string())?;
                if !handled {
                    opened.read_source_page(ticket).map_err(|error| {
                        if matches!(error, bareline_file_io::paged_service::PagedLifecycleError::Changed) {
                            session.mark_source_changed();
                        }
                        error.to_string()
                    })?;
                }
            }
            WindowPoll::Unavailable(reason) => {
                if reason == bareline_document::source::Unavailable::SourceChanged {
                    session.mark_source_changed();
                }
                return Err(format!("Source unavailable: {reason}"));
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
    use std::{
        fs::File,
        ops::Range,
        path::Path,
        sync::Arc,
        time::{Duration, Instant},
    };
    #[test]
    fn paged_adapter_reports_terminal_failure_and_wakes_after_unwind() {
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let (wake_tx, wake_rx) = mpsc::sync_channel(1);
        worker()
            .submit(
                WorkKind::Interactive,
                Box::new(move || {
                    let _completion = JobCompletion::<()>::new(result_tx, Arc::new(move || wake_tx.send(()).unwrap()));
                    panic!("controlled paged adapter failure");
                }),
            )
            .unwrap();
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(PagedOperationError::Message(error)) if error == "Paged worker failed"
        ));
    }
    struct Platform;
    impl LocalFileSystem for Platform {
        fn validate_target(&self, _: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn available_space(&self, _: &Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        fn guard_directory(&self, _: &Path) -> std::io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn open_sealed_read(&self, path: &Path) -> std::io::Result<File> {
            File::open(path)
        }
        fn identity(&self, file: &File) -> std::io::Result<bareline_platform::FileIdentity> {
            let metadata = file.metadata()?;
            Ok(bareline_platform::FileIdentity {
                volume: 1,
                file: 1,
                length: metadata.len(),
                modified: metadata
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
            })
        }
        fn prepare_commit(
            &self,
            staged: &Path,
            target: &Path,
            mode: bareline_platform::CommitMode,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> std::io::Result<bareline_platform::PreparedCommit> {
            bareline_platform::prepare_simulated_commit(self, staged, target, mode, cancellation)
        }
        fn commit_transaction(
            &self,
            transaction: bareline_platform::PreparedCommit,
        ) -> std::io::Result<bareline_platform::CommitReceipt> {
            bareline_platform::simulate_commit_transaction(self, transaction)
        }
        fn commit(&self, staged: &Path, target: &Path, existed: bool) -> std::io::Result<()> {
            assert!(!existed, "fixture only supports Save As to a new file");
            std::fs::rename(staged, target)
        }
        fn open_follow_read(&self, path: &Path) -> std::io::Result<(File, Arc<dyn Send + Sync>)> {
            Ok((File::open(path)?, Arc::new(())))
        }
    }
    /// `Platform` for a recovery journal, which republishes its manifest in place;
    /// `Platform` itself only supports Save As to a new file.
    struct JournalPlatform;
    impl LocalFileSystem for JournalPlatform {
        fn validate_target(&self, path: &Path) -> std::io::Result<()> {
            Platform.validate_target(path)
        }
        fn available_space(&self, path: &Path) -> std::io::Result<u64> {
            Platform.available_space(path)
        }
        fn guard_directory(&self, path: &Path) -> std::io::Result<Arc<dyn Send + Sync>> {
            Platform.guard_directory(path)
        }
        fn open_sealed_read(&self, path: &Path) -> std::io::Result<File> {
            Platform.open_sealed_read(path)
        }
        fn identity(&self, file: &File) -> std::io::Result<bareline_platform::FileIdentity> {
            Platform.identity(file)
        }
        fn prepare_commit(
            &self,
            staged: &Path,
            target: &Path,
            mode: bareline_platform::CommitMode,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> std::io::Result<bareline_platform::PreparedCommit> {
            bareline_platform::prepare_simulated_commit(self, staged, target, mode, cancellation)
        }
        fn commit_transaction(
            &self,
            transaction: bareline_platform::PreparedCommit,
        ) -> std::io::Result<bareline_platform::CommitReceipt> {
            bareline_platform::simulate_commit_transaction(self, transaction)
        }
        fn commit(&self, staged: &Path, target: &Path, _existed: bool) -> std::io::Result<()> {
            std::fs::rename(staged, target)
        }
    }
    fn drain(view: &mut PagedEditorSurface) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            view.pump();
            if !view.busy() && !view.navigation.is_pending() && view.navigation_ready.is_none() {
                assert!(view.error.is_none(), "{:?}", view.error);
                break;
            }
            assert!(Instant::now() < deadline, "paged worker timed out");
            std::thread::yield_now();
        }
    }
    struct GatedPlatform {
        started: std::sync::mpsc::SyncSender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl LocalFileSystem for GatedPlatform {
        fn validate_target(&self, path: &Path) -> std::io::Result<()> {
            Platform.validate_target(path)
        }
        fn available_space(&self, path: &Path) -> std::io::Result<u64> {
            Platform.available_space(path)
        }
        fn guard_directory(&self, path: &Path) -> std::io::Result<Arc<dyn Send + Sync>> {
            Platform.guard_directory(path)
        }
        fn open_sealed_read(&self, path: &Path) -> std::io::Result<File> {
            Platform.open_sealed_read(path)
        }
        fn identity(&self, file: &File) -> std::io::Result<bareline_platform::FileIdentity> {
            Platform.identity(file)
        }
        fn prepare_commit(
            &self,
            staged: &Path,
            target: &Path,
            mode: bareline_platform::CommitMode,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> std::io::Result<bareline_platform::PreparedCommit> {
            bareline_platform::prepare_simulated_commit(self, staged, target, mode, cancellation)
        }
        fn commit_transaction(
            &self,
            transaction: bareline_platform::PreparedCommit,
        ) -> std::io::Result<bareline_platform::CommitReceipt> {
            bareline_platform::simulate_commit_transaction(self, transaction)
        }
        fn commit(&self, staged: &Path, target: &Path, existed: bool) -> std::io::Result<()> {
            self.started.send(()).map_err(std::io::Error::other)?;
            self.release
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .recv()
                .map_err(std::io::Error::other)?;
            Platform.commit(staged, target, existed)
        }
    }
    struct ReleaseOnDrop(Option<std::sync::mpsc::SyncSender<()>>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            if let Some(release) = self.0.take() {
                let _ = release.send(());
            }
        }
    }
    #[test]
    fn paged_save_does_not_strand_another_documents_edit_and_undo() {
        use bareline_file_io::{
            codecs::disk::DiskOptions,
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
            source::SourceOptions,
        };
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "bareline-shared-paged-worker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let open = |name: &str, text: &str| {
            let path = root.join(name);
            std::fs::write(&path, text).unwrap();
            let budget = Budget::new(16 << 20);
            let TranscodeOutcome::Complete(opened) = open_paged_encoded(
                PagedOpenRequest {
                    path,
                    bytes: budget.clone(),
                    history: Budget::new(4 << 20),
                    cache: root.join(format!("{name}-cache")),
                    options: DiskOptions {
                        temp_quota_bytes: 4 << 20,
                        interpret: None,
                    },
                    source_options: SourceOptions {
                        resident_max_bytes: 0,
                        ..Default::default()
                    },
                },
                Arc::new(Platform),
                Cancellation::default(),
                |_| {},
            ) else {
                panic!("paged fixture open failed")
            };
            PagedEditorSurface::new(opened, budget, Arc::new(|| {})).unwrap()
        };
        let first_text = "first document\n";
        let second_text = "second document\n";
        let mut first = open("first.txt", first_text);
        let mut second = open("second.txt", second_text);
        drain(&mut first);
        drain(&mut second);

        let (save_started_tx, save_started_rx) = mpsc::sync_channel(1);
        let (save_release_tx, save_release_rx) = mpsc::sync_channel(1);
        let mut release = ReleaseOnDrop(Some(save_release_tx));
        let saved = root.join("saved.txt");
        first
            .save(
                saved.clone(),
                None,
                Arc::new(GatedPlatform {
                    started: save_started_tx,
                    release: Mutex::new(save_release_rx),
                }),
            )
            .unwrap();
        save_started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        // Advance three queue positions. With the former four private queues,
        // the following real edit was assigned behind the blocked save.
        for _ in 0..3 {
            let (done_tx, done_rx) = mpsc::sync_channel(1);
            worker()
                .submit(WorkKind::General, Box::new(move || done_tx.send(()).unwrap()))
                .unwrap();
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        second.enqueue(Input::Insert("X".into()));
        drain(&mut second);
        assert_eq!(
            second
                .surface
                .snapshot()
                .read(TextOffset(0)..TextOffset(second.surface.snapshot().len()), WINDOW)
                .unwrap(),
            format!("X{second_text}")
        );
        second.enqueue(Input::Undo);
        drain(&mut second);
        assert_eq!(
            second
                .surface
                .snapshot()
                .read(TextOffset(0)..TextOffset(second.surface.snapshot().len()), WINDOW)
                .unwrap(),
            second_text
        );

        release.0.take().unwrap().send(()).unwrap();
        drain(&mut first);
        assert_eq!(std::fs::read(&saved).unwrap(), first_text.as_bytes());
        drop(first);
        drop(second);
        remove_fixture_root(&root);
    }
    #[test]
    fn staged_multicaret_input_uses_global_ranges_and_one_history_entry() {
        use bareline_file_io::{
            codecs::disk::DiskOptions,
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
            source::SourceOptions,
        };
        let root = std::env::temp_dir().join(format!(
            "bareline-global-power-{}-{}",
            std::process::id(),
            crate::power::consumer::next_receipt_sequence()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("source.txt");
        let original = format!("\"a\"\n{}", "abc\n".repeat(49999));
        std::fs::write(&path, &original).unwrap();
        let budget = Budget::new(64 << 20);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(16 << 20),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 16 << 20,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    ..Default::default()
                },
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("open failed")
        };
        let mut view = PagedEditorSurface::new(opened, budget.clone(), Arc::new(|| {})).unwrap();
        drain(&mut view);
        view.surface.typing_syntax = Some(
            bareline_syntax::lex(
                view.surface.snapshot.clone(),
                bareline_syntax::Language::Rust,
                TextOffset(0)..TextOffset(view.surface.snapshot.len()),
                None,
                &bareline_syntax::Cancellation::default(),
            )
            .unwrap(),
        );
        view.global_selections = crate::power::SelectionSet {
            selections: vec![
                Selection { anchor: 1, caret: 1 },
                Selection { anchor: 5, caret: 5 },
                Selection {
                    anchor: 180001,
                    caret: 180001,
                },
            ],
            primary: 0,
        };
        view.project_global_selection();
        assert_eq!(
            view.capture_power().literal_contexts,
            vec![Some(true), Some(false), None]
        );
        let set = crate::power::SelectionSet {
            selections: vec![
                Selection { anchor: 1, caret: 1 },
                Selection {
                    anchor: 180001,
                    caret: 180001,
                },
            ],
            primary: 1,
        };
        view.global_selections = set.clone();
        view.project_global_selection();
        assert!(!view.selection_fully_in_viewport());
        let before = view.snapshot().clone();
        let options = crate::power::captured::StagingOptions {
            cache: root.clone(),
            quota: 16 << 20,
            platform: Arc::new(Platform),
            source_options: Default::default(),
            budget: budget.clone(),
            memory: 8 << 20,
            cancellation: Cancellation::default(),
        };
        let mut prepared =
            crate::paged_power::prepare_input(view.capture_power(), Input::Insert("Z".into()), &options).unwrap();
        apply_prepared_input(&mut view, &before, &mut prepared);
        view.install_power_state(
            &before,
            view.snapshot().revision,
            prepared.selections,
            prepared.state,
            &prepared.hidden_lines,
        )
        .unwrap();
        drain(&mut view);
        assert_eq!(
            view.global_selection_set().selections,
            vec![
                Selection { anchor: 2, caret: 2 },
                Selection {
                    anchor: 180003,
                    caret: 180003
                }
            ]
        );
        let actual = crate::power::captured::clipboard_text(
            view.read_handle(),
            &[TextOffset(0)..TextOffset(view.snapshot().len())],
            1 << 20,
            budget.clone(),
            Cancellation::default(),
        )
        .unwrap();
        let mut expected = original.clone();
        expected.insert(180001, 'Z');
        expected.insert(1, 'Z');
        assert_eq!(actual, expected);
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(view.snapshot().len(), original.len());
        assert!(!view.can_undo());
        let restored = view.global_selection_set();
        assert_eq!(restored.primary(), set.primary());
        assert!(
            set.selections
                .iter()
                .all(|selection| restored.selections.contains(selection))
        );
        drop(view);
        drop(before);
        drop(prepared.source);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn global_scroll_crosses_windows_and_keeps_midline_coordinates_exact() {
        use bareline_file_io::{
            codecs::disk::DiskOptions,
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
            source::SourceOptions,
        };
        let root = std::env::temp_dir().join(format!("bareline-paged-global-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("source.txt");
        std::fs::write(&path, "abc\n".repeat(40000)).unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 4 * 1024 * 1024,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    ..SourceOptions::default()
                },
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("open failed")
        };
        let mut view = PagedEditorSurface::new(opened, budget, Arc::new(|| {})).unwrap();
        drain(&mut view);
        let initial_selection = view.global_selection();
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        view.surface.draw(&mut backend, 800.0, 400.0, &mut Vec::new()).unwrap();
        let point = bareline_renderer::Point {
            x: view.surface.text_left() + 10.0,
            y: view.surface.top() + 2.0,
        };
        view.click(&backend, point, false).unwrap();
        drain(&mut view);
        let clicked = view.global_selection();
        assert_eq!(clicked.0, clicked.1);
        assert!(clicked.1.0 > 0);
        view.click(
            &backend,
            bareline_renderer::Point {
                x: view.surface.text_left() + 25.0,
                ..point
            },
            true,
        )
        .unwrap();
        drain(&mut view);
        assert_eq!(view.global_selection().0, clicked.0);
        assert!(view.global_selection().1.0 > clicked.1.0);
        view.restore_global_selection(initial_selection.0, initial_selection.1, true)
            .unwrap();
        drain(&mut view);
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
        view.request_byte_scroll_in_view(1.0, 400.0).unwrap();
        drain(&mut view);
        assert_eq!(
            view.viewport_start().0 + view.surface.snapshot().len(),
            view.snapshot().len()
        );
        assert!(view.surface.scroll_y > 0.0);
        let scroll_source = view.snapshot().identity_token();
        view.request_global_scroll(30000, 0.25, 17.0).unwrap();
        match view.global_logical_scroll() {
            GlobalScrollPosition::Pending => {
                assert!(view.requested_scroll.is_some() || view.busy() || view.pending_scroll_mapping.is_some())
            }
            GlobalScrollPosition::Ready(line, fraction, x) => {
                // The validated current window may already contain the requested line.
                assert_eq!((line, fraction, x), (30000, 0.25, 17.0));
                assert!(view.paged_frame_state().ready);
                assert_eq!(view.snapshot().identity_token(), scroll_source);
                let first = view.viewport_first_global_line().unwrap();
                assert!(first <= 30000);
                let local = view.surface.snapshot().line_range((30000 - first) as usize).unwrap();
                assert_eq!(view.viewport_start().0 + local.start.0, 120000);
                assert_eq!(view.global_selection(), initial_selection);
            }
        }
        drain(&mut view);
        assert_eq!(
            view.global_logical_scroll(),
            GlobalScrollPosition::Ready(30000, 0.25, 17.0)
        );
        view.request_viewport(TextOffset(120002)).unwrap();
        assert_eq!(view.global_logical_scroll(), GlobalScrollPosition::Pending);
        drain(&mut view);
        assert_eq!(view.viewport_first_global_line(), Some(30000));
        assert_eq!(view.viewport_first_line_start(), Some(TextOffset(120000)));
        let viewport = view.viewport_start();
        view.surface.scroll_y = 37.5;
        view.surface.scroll_x = 19.0;
        let token = view
            .restore_global_selection(TextOffset(2), TextOffset(9), true)
            .unwrap();
        assert_eq!(view.selection_restore_status(token), SelectionRestoreStatus::Pending);
        drain(&mut view);
        assert_eq!(view.selection_restore_status(token), SelectionRestoreStatus::Applied);
        assert_eq!(view.global_selection(), (TextOffset(2), TextOffset(9)));
        assert_eq!(view.viewport_start(), viewport);
        assert_eq!((view.surface.scroll_y, view.surface.scroll_x), (37.5, 19.0));
        assert!(!view.caret_in_viewport());
        let mut peer = view.clone_view().unwrap();
        drain(&mut peer);
        assert_eq!(peer.global_selection(), (TextOffset(2), TextOffset(9)));
        assert_eq!(peer.viewport_start(), viewport);
        drop(peer);
        view.set_known_global_folds(
            vec![bareline_syntax::folding::Fold {
                header: 30001,
                end: 30003,
                level: 1,
            }],
            0,
            false,
            30000,
        )
        .unwrap();
        assert!(view.global_fold_state.collapsed.is_empty());
        view.fold_all_known(1);
        assert!(view.global_fold_state.collapsed.contains(&30001));
        drain(&mut view);
        view.set_global_spacers(&[(30002, 3)]).unwrap();
        view.enqueue(Input::Insert("€".into()));
        drain(&mut view);
        assert_eq!(view.global_selection(), (TextOffset(5), TextOffset(5)));
        let handle = view.read_handle();
        let mut read = handle
            .snapshot()
            .begin_read(TextOffset(0)..TextOffset(8), 16, &view.budget)
            .unwrap();
        loop {
            match read.poll() {
                WindowPoll::Ready(window) => {
                    assert_eq!(window.text(), "ab€bc\n");
                    break;
                }
                WindowPoll::Pending(ticket) => {
                    handle.resolve_page(ticket).unwrap();
                }
                _ => panic!("edited prefix unavailable"),
            }
        }
        let before = view.global_selection();
        let before_viewport = view.viewport_start();
        let superseded = view
            .restore_global_selection(TextOffset(2), TextOffset(5), true)
            .unwrap();
        let invalid = view
            .restore_global_selection(TextOffset(3), TextOffset(5), true)
            .unwrap();
        assert_eq!(
            view.selection_restore_status(superseded),
            SelectionRestoreStatus::Superseded
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while view.busy() {
            view.pump();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(matches!(
            view.selection_restore_status(invalid),
            SelectionRestoreStatus::Failed(_)
        ));
        assert_eq!(view.global_selection(), before);
        assert_eq!(view.viewport_start(), before_viewport);
        assert!(
            view.restore_global_selection(TextOffset(view.snapshot().len() + 1), TextOffset(0), true)
                .is_err()
        );
        drop(handle);
        drop(read);
        let entire = view.snapshot().len();
        view.restore_global_selection(TextOffset(0), TextOffset(entire), true)
            .unwrap();
        drain(&mut view);
        let content = view.snapshot().content_state;
        view.enqueue(Input::Delete);
        let deadline = Instant::now() + Duration::from_secs(10);
        while view.busy() {
            view.pump();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(view.snapshot().content_state, content);
        assert_eq!(view.global_selection(), (TextOffset(0), TextOffset(entire)));
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("bounded viewport"))
        );
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn go_to_end_resolves_to_the_last_line_once_the_index_is_complete() {
        use bareline_file_io::{
            codecs::disk::DiskOptions,
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
            source::SourceOptions,
        };
        let root = std::env::temp_dir().join(format!(
            "bareline-paged-goto-end-{}-{}",
            std::process::id(),
            crate::power::consumer::next_receipt_sequence()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("source.txt");
        std::fs::write(&path, "abc\n".repeat(40_000)).unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 4 * 1024 * 1024,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    ..SourceOptions::default()
                },
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("open failed")
        };
        let mut view = PagedEditorSurface::new(opened, budget, Arc::new(|| {})).unwrap();
        // Keep the estimate: only navigation below may complete the index.
        view.navigation.pause_line_count();
        drain(&mut view);
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        let mut estimated_ops = Vec::new();
        view.surface
            .draw(&mut backend, 800.0, 400.0, &mut estimated_ops)
            .unwrap();
        let estimated_marker = estimated_ops
            .iter()
            .find_map(|op| match op {
                bareline_renderer::DrawOp::Text { origin, text, .. }
                    if (origin.x - 2.0).abs() < f32::EPSILON && text == "~" =>
                {
                    Some(text.clone())
                }
                _ => None,
            })
            .expect("incomplete index gutter approximation marker");
        assert_eq!(estimated_marker, "~");
        let estimated_label = estimated_ops
            .iter()
            .find_map(|op| match op {
                bareline_renderer::DrawOp::Text { origin, text, .. } if (origin.x - 14.0).abs() < f32::EPSILON => {
                    Some(text.clone())
                }
                _ => None,
            })
            .expect("incomplete index gutter label");
        assert!(view.gutter_lines_estimated());
        assert!(
            view.surface
                .status_segments("Plain text")
                .iter()
                .any(|segment| segment.contains("estimated"))
        );
        // UI-06: indexing progress lives in the status bar's size group; no
        // progress text is painted over the document without a background.
        assert!(view.surface.error.is_none(), "{:?}", view.surface.error);
        let status_y = 400.0 - bareline_ui::STATUS_HEIGHT + 4.0;
        assert!(estimated_ops.iter().any(|op| matches!(
            op,
            bareline_renderer::DrawOp::Text { origin, text, .. }
                if origin.y == status_y && text.contains("Line numbers estimated")
        )));
        assert!(estimated_ops.iter().all(|op| !matches!(
            op,
            bareline_renderer::DrawOp::Text { origin, text, .. }
                if origin.y != status_y && (text.contains("indexing") || text.contains("Large file"))
        )));

        // Hold a real scan on the navigation worker after it prepared the shared
        // sparse index. UI pump, status reads and cancellation must all complete
        // before the worker is released; the coordinator releases on timeout so a
        // regression fails instead of hanging the test process.
        let baseline_progress = view.index_fraction().unwrap_or(0.0);
        let (held_tx, held_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        view.navigation.hold_next_scan(held_tx, release_rx);
        let handle = view.read_handle();
        let navigation_budget = view.budget.clone();
        view.navigation
            .request(
                handle,
                crate::paged_navigation::NavigationTarget::Byte(TextOffset(view.snapshot().len() / 2)),
                navigation_budget,
                Arc::new(|| {}),
            )
            .unwrap();
        held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (ui_done_tx, ui_done_rx) = mpsc::sync_channel(1);
        let coordinator = std::thread::spawn(move || {
            let completed_before_release = ui_done_rx.recv_timeout(Duration::from_secs(2)).is_ok();
            let _ = release_tx.send(());
            completed_before_release
        });
        view.pump();
        let held_progress = view.index_fraction().expect("held scan retained a matching receipt");
        assert!(held_progress >= baseline_progress && held_progress < 1.0);
        assert_eq!(view.indexed_line_count(), None);
        assert!(
            view.surface
                .status_segments("Plain text")
                .iter()
                .any(|segment| segment.contains("estimated"))
        );
        view.request_byte_scroll_in_view(0.0, 400.0).unwrap();
        ui_done_tx.send(()).unwrap();
        assert!(
            coordinator.join().unwrap(),
            "UI pump/status/cancel waited for the held line scan"
        );
        drain(&mut view);

        // Scan almost all of the real retained sparse index. Approximate progress
        // is deliberately above the former 99.9% threshold, but EOF has not been
        // observed and the line count must remain estimated.
        let handle = view.read_handle();
        let end = TextOffset(view.snapshot().len());
        let navigation_budget = view.budget.clone();
        view.navigation
            .request(
                handle,
                crate::paged_navigation::NavigationTarget::Byte(TextOffset(end.0 - 64)),
                navigation_budget,
                Arc::new(|| {}),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match view.navigation.poll() {
                Some(Ok(_)) => break,
                Some(Err(error)) => panic!("line index completion failed: {error}"),
                None => {
                    assert!(Instant::now() < deadline, "line index completion timed out");
                    std::thread::yield_now();
                }
            }
        }
        assert!(view.index_fraction().is_some_and(|fraction| fraction >= 0.999));
        assert_eq!(view.indexed_line_count(), None);
        assert!(!view.refresh_gutter_accuracy());
        assert!(view.gutter_lines_estimated());

        // Only terminal EOF knowledge makes the same generation exact.
        let handle = view.read_handle();
        let navigation_budget = view.budget.clone();
        view.navigation
            .request(
                handle,
                crate::paged_navigation::NavigationTarget::Byte(end),
                navigation_budget,
                Arc::new(|| {}),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match view.navigation.poll() {
                Some(Ok(_)) => break,
                Some(Err(error)) => panic!("line index completion failed: {error}"),
                None => {
                    assert!(Instant::now() < deadline, "line index completion timed out");
                    std::thread::yield_now();
                }
            }
        }
        assert_eq!(view.index_fraction(), Some(1.0));
        assert_eq!(view.indexed_line_count(), Some(40_001));
        assert!(view.refresh_gutter_accuracy());
        assert!(
            !view.refresh_gutter_accuracy(),
            "exact transition repeated without a state change"
        );
        let (receipt_held_tx, receipt_held_rx) = mpsc::sync_channel(1);
        let (receipt_release_tx, receipt_release_rx) = mpsc::sync_channel(1);
        let receipt_guard = view.navigation.hold_receipt_write(receipt_held_tx, receipt_release_rx);
        receipt_held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!view.refresh_gutter_accuracy());
        assert!(!view.gutter_lines_estimated());
        receipt_release_tx.send(()).unwrap();
        receipt_guard.join().unwrap();
        let mut exact_ops = Vec::new();
        view.surface.draw(&mut backend, 800.0, 400.0, &mut exact_ops).unwrap();
        let exact_label = exact_ops
            .iter()
            .find_map(|op| match op {
                bareline_renderer::DrawOp::Text { origin, text, .. }
                    if (origin.x - 14.0).abs() < f32::EPSILON && text == &estimated_label =>
                {
                    Some(text.clone())
                }
                _ => None,
            })
            .expect("completed index exact gutter label for the same row");
        assert_eq!(estimated_label, exact_label);
        assert!(!exact_ops.iter().any(|op| matches!(
            op,
            bareline_renderer::DrawOp::Text { origin, text, .. }
                if (origin.x - 2.0).abs() < f32::EPSILON && text == "~"
        )));
        assert!(!view.gutter_lines_estimated());
        assert!(
            view.surface
                .status_segments("Plain text")
                .iter()
                .all(|segment| !segment.contains("estimated"))
        );
        // A forced-paged document (resident_max_bytes = 0) keeps its source tree lazy,
        // so `snapshot().line_count()` stays Unknown until the whole file is
        // materialised, and window reads never materialise it. The fixture is 40_000
        // "abc\n" lines, so the final (empty) line is index 40_000 and the total is
        // 40_001; go-to-end must land the gutter on that last line.
        let total_lines = 40_000usize + 1;
        // Ctrl+End jumps the byte scrollbar to the very bottom of the file.
        view.request_byte_scroll_in_view(1.0, 400.0).unwrap();
        drain(&mut view);
        assert!(
            !view.gutter_lines_estimated(),
            "installing a complete resident viewport slice reset paged index accuracy"
        );
        // The last window sits at the end of the document, and once the index is
        // complete its first global line is resolved so the gutter shows true numbers
        // (window start line + local index) rather than restarting at 1.
        assert_eq!(
            view.viewport_start().0 + view.surface.snapshot().len(),
            view.snapshot().len()
        );
        let base = view.viewport_first_global_line().expect("window first line resolved");
        let last = view
            .surface
            .source_line_at(TextOffset(view.surface.snapshot().len()))
            .expect("last gutter line");
        assert_eq!(
            last,
            (total_lines - 1) as u64,
            "go-to-end must land on the true last line"
        );
        assert!(base <= last);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn large_fold_projection_maps_seams_and_preserves_anchors_through_undo() {
        use bareline_file_io::{
            codecs::disk::DiskOptions,
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
            source::SourceOptions,
        };
        let root = std::env::temp_dir().join(format!("bareline-paged-fold-map-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let text = format!(
            "intro\nheader\n{}suffix\n",
            "body with enough bytes for a large fold\n".repeat(12_000)
        );
        let suffix = text.find("suffix").unwrap();
        assert!(suffix > 256 * 1024);
        let path = root.join("fold.txt");
        std::fs::write(&path, &text).unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 1024 * 1024,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    ..SourceOptions::default()
                },
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("open failed")
        };
        let mut view = PagedEditorSurface::new(opened, budget, Arc::new(|| {})).unwrap();
        drain(&mut view);
        view.set_known_anchored_folds(
            vec![bareline_syntax::folding::AnchoredFold {
                fold: bareline_syntax::folding::Fold {
                    header: 1,
                    end: 12001,
                    level: 1,
                },
                header: TextOffset(6),
                body: TextOffset(13)..TextOffset(suffix),
            }],
            0,
            false,
            0,
        )
        .unwrap();
        view.fold_all_known(1);
        drain(&mut view);
        assert_eq!(
            view.surface
                .snapshot()
                .read(TextOffset(0)..TextOffset(view.surface.snapshot().len()), WINDOW)
                .unwrap(),
            "intro\nheader\nsuffix\n"
        );
        assert_eq!(
            view.source_offset(TextOffset(13), SourceAffinity::Before),
            Some(TextOffset(13))
        );
        assert_eq!(
            view.source_offset(TextOffset(13), SourceAffinity::After),
            Some(TextOffset(suffix))
        );
        assert_eq!(view.local_offset(TextOffset(suffix)), Some(TextOffset(13)));
        assert!(view.local_offset(TextOffset(100_000)).is_none());
        assert_eq!(view.surface.source_line_at(TextOffset(13)), Some(12002));
        view.enqueue(Input::Insert("new\n".into()));
        drain(&mut view);
        assert!(view.global_fold_state.collapsed.contains(&2));
        assert_eq!(
            view.surface
                .snapshot()
                .read(TextOffset(0)..TextOffset(view.surface.snapshot().len()), WINDOW)
                .unwrap(),
            "new\nintro\nheader\nsuffix\n"
        );
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert!(view.global_fold_state.collapsed.contains(&1));
        assert_eq!(view.local_offset(TextOffset(suffix)), Some(TextOffset(13)));
        // Type on the visible header line: the edit overlaps the fold anchor without
        // touching its hidden body (typing into the body first reveals it, PED-12).
        view.restore_global_selection(TextOffset(8), TextOffset(8), true)
            .unwrap();
        drain(&mut view);
        view.enqueue(Input::Insert("X".into()));
        drain(&mut view);
        assert!(
            !view.global_fold_state.collapsed.contains(&1),
            "overlapping anchor must await reverification"
        );
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert!(view.global_fold_state.collapsed.contains(&1));
        let mut peer = view.clone_view().unwrap();
        drain(&mut peer);
        peer.unfold_all_known();
        drain(&mut peer);
        assert_eq!(peer.source_segments().len(), 1);
        assert_eq!(view.source_segments().len(), 2);
        drop(peer);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn horizontal_paged_line_crosses_source_windows_with_a_shaped_anchor() {
        use bareline_file_io::{
            codecs::disk::DiskOptions,
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
            source::SourceOptions,
        };
        let root = std::env::temp_dir().join(format!("bareline-paged-horizontal-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("line.txt");
        std::fs::write(&path, "x".repeat(WINDOW * 4)).unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 1024 * 1024,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    ..SourceOptions::default()
                },
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("open failed")
        };
        let mut view = PagedEditorSurface::new(opened, budget, Arc::new(|| {})).unwrap();
        drain(&mut view);
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        view.scroll_horizontal(1_000_000.0);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            view.pump();
            view.surface.draw(&mut backend, 800.0, 400.0, &mut Vec::new()).unwrap();
            view.refine_horizontal_viewport(&backend, 800.0).unwrap();
            if view.viewport_start().0 > 0
                && view.paged_frame_state().ready
                && !view.surface.horizontal_anchor_pending()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "horizontal paged window did not advance: {:?}",
                view.error
            );
            std::thread::yield_now();
        }
        assert_eq!(view.global_selection(), (TextOffset(0), TextOffset(0)));
        assert!(view.surface.snapshot().len() <= WINDOW);
        drain(&mut view);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn undo_and_redo_survive_a_failing_recovery_journal() {
        // FIO-03: a recovery failure degrades recovery and is reported; it never
        // refuses the user's Undo or Redo. The journal that missed a step is retired,
        // and a lasting failure never creates a new journal on every keypress.
        let (root, mut view, budget) = paged_fixture("undo-journal-failure", "abc\n");
        let journals = |root: &Path| -> Vec<PathBuf> {
            std::fs::read_dir(root.join("recovery"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.is_dir()
                        && path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with("paged-"))
                })
                .collect()
        };
        view.enable_recovery(root.join("recovery"), Arc::new(JournalPlatform));
        view.restore_global_selection(TextOffset(1), TextOffset(1), true)
            .unwrap();
        drain(&mut view);
        view.enqueue(Input::Insert("X".into()));
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "aXbc\n");
        let journal = view.recovery_status().directory.expect("the edit is journaled");
        assert_eq!(journals(&root), vec![journal.clone()]);
        // Every history append now fails on the pointer quota.
        view.set_streaming_quota(1);
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "abc\n");
        assert_eq!(view.global_selection(), (TextOffset(1), TextOffset(1)));
        assert!(view.can_redo());
        assert!(
            journal.join("retired.json").exists(),
            "the journal that missed the Undo is retired, not left for crash recovery"
        );
        let status = view.recovery_status();
        assert!(status.directory.is_none());
        assert!(status.error.is_some(), "the failure is reported");
        // Repeated Undo and Redo stay unjournaled: no journal per keypress.
        for (input, text, caret) in [
            (Input::Redo, "aXbc\n", 2),
            (Input::Undo, "abc\n", 1),
            (Input::Redo, "aXbc\n", 2),
        ] {
            view.enqueue(input);
            drain(&mut view);
            assert_eq!(document_text(&view, &budget), text);
            assert_eq!(view.global_selection(), (TextOffset(caret), TextOffset(caret)));
            assert_eq!(journals(&root), vec![journal.clone()]);
            assert!(view.recovery_status().error.is_some());
        }
        // The next edit starts one fresh journal from the published text. It is
        // typed away from the caret, so it never merges into the "X" Undo step.
        view.set_streaming_quota(20 * 1024 * 1024 * 1024);
        view.restore_global_selection(TextOffset(0), TextOffset(0), true)
            .unwrap();
        drain(&mut view);
        view.enqueue(Input::Insert("Y".into()));
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "YaXbc\n");
        let fresh = view.recovery_status().directory.expect("the edit starts a journal");
        assert_ne!(fresh, journal);
        assert_eq!(journals(&root).len(), 2);
        // From here on no journal can be created: its root is under a file.
        let blocker = root.join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        view.enable_recovery(blocker.join("recovery"), Arc::new(JournalPlatform));
        view.set_streaming_quota(1);
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "aXbc\n");
        assert_eq!(view.global_selection(), (TextOffset(0), TextOffset(0)));
        assert!(fresh.join("retired.json").exists());
        // A user retry ends the suspension; the failed rebuild is reported, and
        // Undo and Redo still succeed while the journal cannot be created.
        view.retry_recovery().unwrap();
        drain(&mut view);
        assert!(view.recovery_status().error.is_some());
        view.enqueue(Input::Redo);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "YaXbc\n");
        assert_eq!(view.global_selection(), (TextOffset(1), TextOffset(1)));
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "aXbc\n");
        assert!(view.error.is_none());
        assert!(view.recovery_status().error.is_some());
        assert_eq!(journals(&root).len(), 2);
        drop(view);
        let _ = std::fs::remove_dir_all(root);
    }
    /// Removes a fixture directory. A closed view's line owner thread is
    /// detached (PED-08), so it can still hold the store's files for a moment
    /// after the view drops; retry briefly instead of joining it.
    fn remove_fixture_root(root: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match std::fs::remove_dir_all(root) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error) if Instant::now() >= deadline => {
                    panic!("could not remove {}: {error}", root.display())
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
    /// Opens `text` as a forced-paged document in its own temporary directory.
    fn paged_fixture(name: &str, text: &str) -> (std::path::PathBuf, PagedEditorSurface, Budget) {
        paged_fixture_with(
            name,
            text,
            bareline_file_io::source::SourceOptions {
                resident_max_bytes: 0,
                ..Default::default()
            },
        )
    }
    fn paged_fixture_with(
        name: &str,
        text: &str,
        source_options: bareline_file_io::source::SourceOptions,
    ) -> (std::path::PathBuf, PagedEditorSurface, Budget) {
        use bareline_file_io::{
            codecs::disk::DiskOptions,
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
        };
        let root = std::env::temp_dir().join(format!(
            "bareline-paged-{name}-{}-{}",
            std::process::id(),
            crate::power::consumer::next_receipt_sequence()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("source.txt");
        std::fs::write(&path, text).unwrap();
        let budget = Budget::new(64 << 20);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(4 << 20),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 16 << 20,
                    interpret: None,
                },
                source_options,
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("paged fixture open failed")
        };
        let mut view = PagedEditorSurface::new(opened, budget.clone(), Arc::new(|| {})).unwrap();
        drain(&mut view);
        (root, view, budget)
    }
    /// Pumps until the background count reports `lines` for the view's text.
    fn wait_for_line_count(view: &mut PagedEditorSurface, lines: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while view.indexed_line_count() != Some(lines) {
            view.pump();
            assert!(view.error.is_none(), "{:?}", view.error);
            assert!(
                Instant::now() < deadline,
                "line count stayed {:?}",
                view.indexed_line_count()
            );
            std::thread::yield_now();
        }
    }
    #[test]
    fn background_line_count_completes_and_follows_an_edit_without_a_rescan() {
        // PERF-04: the exact count arrives without any navigation to the end.
        let text = "abc\r\n".repeat(40_000);
        let (root, mut view, _budget) = paged_fixture("line-count", &text);
        wait_for_line_count(&mut view, 40_001);
        let index = view.navigation.line_index().clone();
        let rebuilds = index.rebuilds();
        let scanned = index.scanned_bytes();
        // PED-08: an edit near the start keeps the checkpoints after it, so the
        // recount reads up to the first moved checkpoint, not the whole file.
        view.enqueue(Input::Insert("new\n".into()));
        drain(&mut view);
        wait_for_line_count(&mut view, 40_002);
        assert_eq!(index.rebuilds(), rebuilds, "the edit restarted the index at byte zero");
        let rescanned = index.scanned_bytes() - scanned;
        assert!(rescanned < text.len() / 2, "recount read {rescanned} bytes");
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn power_lookups_start_from_the_shared_index_not_byte_zero() {
        use bareline_document::line_lookup::{LineLookupPoll, LineTarget};
        let text = "abc\n".repeat(40_000);
        let (root, mut view, budget) = paged_fixture("shared-index", &text);
        wait_for_line_count(&mut view, 40_001);
        let capture = view.capture_power();
        let index = view.navigation.line_index().clone();
        let rebuilds = index.rebuilds();
        let scanned = index.scanned_bytes();
        // PED-06: a command's lookup near the end starts at a checkpoint the
        // count retained; a private index would read from byte zero.
        let result = capture
            .line_index
            .lookup(
                &capture.source,
                LineTarget::Byte(TextOffset(text.len() - 8)),
                &budget,
                &mut None,
                &mut || Ok(()),
            )
            .unwrap();
        assert!(matches!(result, LineLookupPoll::Line(39_998)), "{result:?}");
        let read = index.scanned_bytes() - scanned;
        assert!(read < 2 * 64 * 1024, "lookup read {read} bytes");
        assert_eq!(index.rebuilds(), rebuilds);
        drop(capture);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn line_wise_transforms_plan_through_the_shared_index() {
        // PED-06: the transform planner finds the selected lines from the
        // document's retained checkpoints. A private index would leave the shared
        // one untouched and read about 800 KB from byte zero for each lookup.
        let text = "abc\n".repeat(200_000);
        let (root, mut view, budget) = paged_fixture("transform-index", &text);
        wait_for_line_count(&mut view, 200_001);
        let options = staging(&root, &budget);
        let index = view.navigation.line_index().clone();
        let rebuilds = index.rebuilds();
        let scanned = index.scanned_bytes();
        let caret = text.len() - 8;
        let transaction = paged_transform(
            &view,
            &options,
            &[Range {
                start: caret,
                end: caret,
            }],
            crate::power::Transform::Indent,
        )
        .expect("indent changes text");
        let read = index.scanned_bytes() - scanned;
        assert!(read > 0, "the plan did not look its lines up in the shared index");
        // Two byte and two line lookups, each from a checkpoint within one
        // 64 KiB spacing of its target.
        assert!(read < 4 * 64 * 1024, "planning read {read} bytes");
        assert_eq!(index.rebuilds(), rebuilds);
        drop(transaction);
        drop(options);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn peer_views_share_one_background_count() {
        use crate::paged_navigation::{GlobalNavigation, NavigationTarget};
        let text = "abc\n".repeat(40_000);
        let (root, view, budget) = paged_fixture("shared-count", &text);
        let snapshot = view.snapshot().clone();
        let mut first = GlobalNavigation::new();
        let mut second = GlobalNavigation::sharing(first.line_index().clone());
        // Hold the first view's worker on a navigation, so its count stays queued.
        let (held_tx, held_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        first.hold_next_scan(held_tx, release_rx);
        first
            .request(
                view.read_handle(),
                NavigationTarget::Byte(TextOffset(4)),
                budget.clone(),
                Arc::new(|| {}),
            )
            .unwrap();
        held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        first.count_lines(&snapshot, || view.read_handle(), budget.clone(), Arc::new(|| {}));
        // PERF-04: the peer leaves the shared index to the count already queued.
        second.count_lines(&snapshot, || view.read_handle(), budget.clone(), Arc::new(|| {}));
        assert_eq!(second.spawned_threads(), 0);
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while second.indexed_line_count(&snapshot) != Some(40_001) {
            assert!(Instant::now() < deadline, "the shared count did not complete");
            std::thread::yield_now();
        }
        assert_eq!(first.spawned_threads(), 1);
        assert_eq!(second.spawned_threads(), 0);
        drop(first);
        drop(second);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn superseding_navigation_requests_reuse_the_owner_thread() {
        use crate::paged_navigation::NavigationTarget;
        let text = "abc\n".repeat(40_000);
        let (root, mut view, budget) = paged_fixture("navigation-owner", &text);
        let before = view.navigation.spawned_threads();
        let (held_tx, held_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        view.navigation.hold_next_scan(held_tx, release_rx);
        let request = |view: &mut PagedEditorSurface, target| {
            let handle = view.read_handle();
            view.navigation
                .request(handle, target, budget.clone(), Arc::new(|| {}))
                .unwrap();
        };
        request(&mut view, NavigationTarget::Byte(TextOffset(4)));
        held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        for line in 1..=16 {
            request(&mut view, NavigationTarget::Line(line * 1000));
        }
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let result = loop {
            if let Some(result) = view.navigation.poll() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline, "navigation timed out");
            std::thread::yield_now();
        };
        assert_eq!(result.first_global_line, 16_000);
        assert_eq!(result.line_start, TextOffset(64_000));
        // PED-08: queued requests replace each other on the running worker; none
        // starts a thread of its own.
        assert!(view.navigation.spawned_threads() - before <= 1);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn recounts_after_edits_reuse_the_parked_owner_thread() {
        let text = "abc\n".repeat(40_000);
        let (root, mut view, _budget) = paged_fixture("owner-recount", &text);
        wait_for_line_count(&mut view, 40_001);
        let spawned = view.navigation.spawned_threads();
        assert_eq!(spawned, 1);
        // PED-08: each edit's recount runs on the view's parked owner thread;
        // typing never starts a thread per edit.
        for edit in 1..=5 {
            view.enqueue(Input::Insert("new\n".into()));
            drain(&mut view);
            wait_for_line_count(&mut view, 40_001 + edit);
            assert_eq!(
                view.navigation.spawned_threads(),
                spawned,
                "edit {edit} started a thread"
            );
        }
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn closing_a_view_never_waits_for_its_owner_thread() {
        use crate::paged_navigation::NavigationTarget;
        let text = "abc\n".repeat(40_000);
        let (root, mut view, budget) = paged_fixture("owner-close", &text);
        // Hold the owner thread inside a navigation, where a page read from a
        // disconnected share would stall it (APP-19).
        let (held_tx, held_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        view.navigation.hold_next_scan(held_tx, release_rx);
        let handle = view.read_handle();
        view.navigation
            .request(handle, NavigationTarget::Line(39_000), budget.clone(), Arc::new(|| {}))
            .unwrap();
        held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let exited = view.navigation.owner_exited();
        // PED-08: closing the tab on the UI thread signals the owner and returns;
        // it never joins a thread that is still reading.
        let closing = Instant::now();
        drop(view);
        let closed = closing.elapsed();
        assert!(!exited(), "the close waited for the owner thread");
        assert!(closed < Duration::from_secs(2), "closing took {closed:?}");
        // Released, the owner sees the stop at its next check and exits with
        // its read handle.
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !exited() {
            assert!(Instant::now() < deadline, "the stopped owner thread did not exit");
            std::thread::yield_now();
        }
        remove_fixture_root(&root);
    }
    #[test]
    fn failed_background_counts_retry_then_report_stopped() {
        use crate::paged_navigation::GlobalNavigation;
        let text = "abc\n".repeat(1_000);
        let (root, view, _budget) = paged_fixture("count-failure", &text);
        let snapshot = view.snapshot().clone();
        let mut navigation = GlobalNavigation::new();
        // An empty budget cannot hold the line index, so every count fails. A
        // failure is retried instead of leaving the text marked as counted.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !navigation.line_count_stopped(&snapshot) {
            navigation.count_lines(&snapshot, || view.read_handle(), Budget::new(0), Arc::new(|| {}));
            assert!(Instant::now() < deadline, "the failed count was not retried");
            std::thread::yield_now();
        }
        assert_eq!(navigation.indexed_line_count(&snapshot), None);
        // The retries ran on the parked owner thread.
        assert_eq!(navigation.spawned_threads(), 1);
        drop(navigation);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn fold_mapping_after_an_edit_reads_near_the_edit_and_the_hidden_lines() {
        // PED-07: fold projection looks its lines up in the shared index. After an
        // edit near the start, it reads up to the first moved checkpoint and then
        // only near the window; a rescan from byte zero would read about 600 KB
        // to reach line 150,000.
        let text = "abc\n".repeat(200_000);
        let (root, mut view, _budget) = paged_fixture("fold-index", &text);
        wait_for_line_count(&mut view, 200_001);
        // Only the mapping job reads through the index from here on.
        view.navigation.pause_line_count();
        view.enqueue(Input::Insert("new\n".into()));
        drain(&mut view);
        let index = view.navigation.line_index().clone();
        let rebuilds = index.rebuilds();
        let scanned = index.scanned_bytes();
        view.set_global_hidden_ranges(&[Range {
            start: 150_000,
            end: 150_010,
        }])
        .unwrap();
        drain(&mut view);
        let map = view.mapped.as_ref().expect("the fold mapping completed");
        assert_eq!(map.source.content_state, view.snapshot().content_state);
        let read = index.scanned_bytes() - scanned;
        assert!(read > 0, "the mapping did not look its lines up in the shared index");
        // One 64 KiB window to the first moved checkpoint, then at most one
        // checkpoint spacing to the end of the window's reach; the hidden lines
        // lie past it and are not looked up.
        assert!(read < 3 * 64 * 1024, "fold mapping read {read} bytes");
        assert_eq!(index.rebuilds(), rebuilds, "the edit restarted the index at byte zero");
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn fold_mapping_looks_up_only_hidden_lines_near_the_window() {
        // PED-07: 200 hidden ranges spread over 800 KB. The mapping of the first
        // window looks up those within the window and its margin, about 132 KB
        // of text; looking every range up would read nearly the whole file.
        let text = "abc\n".repeat(200_000);
        let (root, mut view, _budget) = paged_fixture("fold-near", &text);
        wait_for_line_count(&mut view, 200_001);
        view.navigation.pause_line_count();
        let index = view.navigation.line_index().clone();
        let scanned = index.scanned_bytes();
        let ranges: Vec<_> = (0..200u64).map(|k| k * 1000 + 1..k * 1000 + 5).collect();
        view.set_global_hidden_ranges(&ranges).unwrap();
        drain(&mut view);
        let map = view.mapped.as_ref().expect("the fold mapping completed");
        // Lines 1..5 (bytes 4..20) are hidden after the first line.
        assert_eq!(map.segments[0].source, TextOffset(0)..TextOffset(4));
        assert_eq!(map.segments[1].source.start, TextOffset(20));
        assert_eq!(map.segments[1].first_global_line, Some(5));
        assert_eq!(map.segments[1].source_line_start, Some(TextOffset(20)));
        let read = index.scanned_bytes() - scanned;
        assert!(read > 0, "the mapping did not look its lines up");
        assert!(read < 3 * 64 * 1024, "fold mapping read {read} bytes");
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn collapsed_folds_far_from_the_window_survive_edits_until_a_mapping_reaches_them() {
        // PED-07: a collapsed fold 600 KB away is carried through edits by its
        // bytes; no mapping near the start looks its lines up. Moving the
        // window there resolves it, collapsed, at its new lines.
        let text = "abc\n".repeat(200_000);
        let (root, mut view, _budget) = paged_fixture("fold-far", &text);
        view.set_known_anchored_folds(
            vec![bareline_syntax::folding::AnchoredFold {
                fold: bareline_syntax::folding::Fold {
                    header: 150_000,
                    end: 150_010,
                    level: 1,
                },
                header: TextOffset(600_000),
                body: TextOffset(600_004)..TextOffset(600_044),
            }],
            0,
            false,
            0,
        )
        .unwrap();
        view.fold_all_known(1);
        drain(&mut view);
        assert!(view.global_fold_state.collapsed.contains(&150_000));
        view.enqueue(Input::Insert("new\n".into()));
        drain(&mut view);
        view.enqueue(Input::Insert("more\n".into()));
        drain(&mut view);
        // Two edits later the fold is still carried, collapsed, at its new bytes.
        let carried: Vec<_> = view
            .rebased_folds
            .iter()
            .map(|anchor| (anchor.header, anchor.body, anchor.end, anchor.collapsed))
            .collect();
        assert_eq!(
            carried,
            vec![(TextOffset(600_009), TextOffset(600_013), TextOffset(600_053), true)]
        );
        view.request_viewport(TextOffset(600_009)).unwrap();
        drain(&mut view);
        assert!(view.global_fold_state.collapsed.contains(&150_002));
        assert!(view.rebased_folds.is_empty());
        assert!(view.local_offset(TextOffset(600_020)).is_none(), "the body is shown");
        drop(view);
        remove_fixture_root(&root);
    }
    /// 200,000 lines of "abc" with one fold, lines 150,000..=150,010 at byte
    /// 600,000, collapsed while the window shows the start of the file.
    fn far_fold_fixture(name: &str) -> (std::path::PathBuf, PagedEditorSurface, Budget) {
        let (root, mut view, budget) = paged_fixture(name, &"abc\n".repeat(200_000));
        view.set_known_anchored_folds(
            vec![bareline_syntax::folding::AnchoredFold {
                fold: bareline_syntax::folding::Fold {
                    header: 150_000,
                    end: 150_010,
                    level: 1,
                },
                header: TextOffset(600_000),
                body: TextOffset(600_004)..TextOffset(600_044),
            }],
            0,
            false,
            0,
        )
        .unwrap();
        view.fold_all_known(1);
        drain(&mut view);
        assert!(view.global_fold_state.collapsed.contains(&150_000));
        (root, view, budget)
    }
    /// Header byte, collapsed state and whether the lines are known of each
    /// fold the view still carries by its bytes.
    fn carried_folds(view: &PagedEditorSurface) -> Vec<(usize, bool, bool)> {
        view.rebased_folds
            .iter()
            .map(|anchor| (anchor.header.0, anchor.collapsed, anchor.lines_known))
            .collect()
    }
    #[test]
    fn carried_collapsed_folds_survive_undo_and_redo() {
        // PED-07: the fold history keeps the folds no mapping had resolved, so
        // undo and redo after edits far from a collapsed fold never expand it.
        let (root, mut view, _budget) = far_fold_fixture("fold-far-history");
        view.enqueue(Input::Insert("new\n".into()));
        drain(&mut view);
        view.enqueue(Input::Insert("more\n".into()));
        drain(&mut view);
        assert_eq!(carried_folds(&view), vec![(600_009, true, true)]);
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(
            carried_folds(&view),
            vec![(600_004, true, true)],
            "the undo expanded a far collapsed fold"
        );
        assert_eq!(view.persisted_global_folds(), vec![150_001..150_012]);
        view.enqueue(Input::Redo);
        drain(&mut view);
        assert_eq!(carried_folds(&view), vec![(600_009, true, true)]);
        view.enqueue(Input::Undo);
        drain(&mut view);
        view.enqueue(Input::Undo);
        drain(&mut view);
        // The opened text knows the fold by line again.
        assert!(view.global_fold_state.collapsed.contains(&150_000));
        assert_eq!(view.persisted_global_folds(), vec![150_000..150_011]);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn unfold_all_reaches_carried_folds() {
        // PED-07: Unfold All expands a fold carried by its bytes; a mapping that
        // reaches it later shows its body instead of collapsing it again.
        let (root, mut view, _budget) = far_fold_fixture("fold-far-unfold");
        view.enqueue(Input::Insert("new\n".into()));
        drain(&mut view);
        assert_eq!(carried_folds(&view), vec![(600_004, true, true)]);
        view.unfold_all_known();
        drain(&mut view);
        assert_eq!(carried_folds(&view), vec![(600_004, false, true)]);
        assert!(view.persisted_global_folds().is_empty());
        view.request_viewport(TextOffset(600_004)).unwrap();
        drain(&mut view);
        assert!(!view.global_fold_state.collapsed.contains(&150_001));
        assert!(view.local_offset(TextOffset(600_020)).is_some(), "the body is hidden");
        // Fold All collapses it again while it is carried.
        view.request_viewport(TextOffset(0)).unwrap();
        drain(&mut view);
        view.enqueue(Input::Insert("x".into()));
        drain(&mut view);
        view.fold_all_regions();
        drain(&mut view);
        assert_eq!(carried_folds(&view), vec![(600_005, true, true)]);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn session_folds_include_carried_folds_at_their_moved_lines() {
        // PED-07: a collapsed fold carried by its bytes is persisted at the lines
        // each change moved it to, counted from the bytes around the edit:
        // inserted line breaks from the commit's window, a removed one from the
        // window the view showed before it.
        let (root, mut view, _budget) = far_fold_fixture("fold-far-persist");
        assert_eq!(view.persisted_global_folds(), vec![150_000..150_011]);
        view.enqueue(Input::Insert("new\n".into()));
        drain(&mut view);
        view.enqueue(Input::Insert("more\n".into()));
        drain(&mut view);
        assert_eq!(carried_folds(&view), vec![(600_009, true, true)]);
        assert_eq!(view.persisted_global_folds(), vec![150_002..150_013]);
        view.enqueue(Input::Backspace);
        drain(&mut view);
        assert_eq!(carried_folds(&view), vec![(600_008, true, true)]);
        assert_eq!(view.persisted_global_folds(), vec![150_001..150_012]);
        // A mapping that reaches the fold agrees with the counted lines.
        view.request_viewport(TextOffset(600_008)).unwrap();
        drain(&mut view);
        assert!(view.rebased_folds.is_empty());
        assert_eq!(view.persisted_global_folds(), vec![150_001..150_012]);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn navigation_into_a_carried_fold_reveals_it() {
        // PED-07, PED-12: a selection placed in the body of a fold carried by its
        // bytes expands it, as for a fold the view knows by line.
        let (root, mut view, _budget) = far_fold_fixture("fold-far-reveal");
        view.enqueue(Input::Insert("new\n".into()));
        drain(&mut view);
        assert_eq!(carried_folds(&view), vec![(600_004, true, true)]);
        view.restore_global_selection(TextOffset(600_020), TextOffset(600_020), false)
            .unwrap();
        drain(&mut view);
        assert!(!view.global_fold_state.collapsed.contains(&150_001));
        assert!(
            view.local_offset(TextOffset(600_020)).is_some(),
            "the caret is in hidden text"
        );
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn typing_proceeds_while_a_fold_mapping_is_pending() {
        // PED-07: a pending fold mapping no longer holds a keystroke back. The
        // edit supersedes the mapping, which the commit queues again.
        let (root, mut view, budget) = paged_fixture("mapping-input", &"abc\n".repeat(1_000));
        view.set_global_hidden_ranges(&[Range { start: 100, end: 110 }])
            .unwrap();
        assert!(view.mapping_job.is_some());
        view.enqueue(Input::Insert("x".into()));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.pending.is_some(), "the keystroke waited for the mapping");
        assert!(view.mapping_job.is_none());
        drain(&mut view);
        assert!(document_text(&view, &budget).starts_with("xabc\n"));
        // Staged input is taken while a mapping is pending too.
        view.set_global_hidden_ranges(&[Range { start: 100, end: 110 }])
            .unwrap();
        assert!(view.mapping_job.is_some());
        view.enable_power_input();
        view.enqueue(Input::Insert("y".into()));
        assert!(matches!(view.take_power_input(), Some(Input::Insert(text)) if text == "y"));
        view.finish_power_preparation();
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn a_shared_index_several_commits_behind_follows_the_change_log() {
        // PED-08: an index that missed two commits follows their receipts from
        // the shared change log; it keeps its checkpoints instead of starting
        // over, and the recount reads up to the first moved one.
        use crate::paged_navigation::{GlobalNavigation, SharedLineIndex};
        let text = "abc\r\n".repeat(40_000);
        let (root, mut view, budget) = paged_fixture("skipped-revisions", &text);
        let opened = view.snapshot().clone();
        // A second index, counted at the opened text, stands in for a view that
        // never installed the commits in between.
        let index = SharedLineIndex::default();
        let mut counter = GlobalNavigation::sharing(index.clone());
        let wait_for = |counter: &mut GlobalNavigation, view: &PagedEditorSurface, lines: usize| {
            let snapshot = view.snapshot().clone();
            let deadline = Instant::now() + Duration::from_secs(10);
            while counter.indexed_line_count(&snapshot) != Some(lines) {
                counter.count_lines(&snapshot, || view.read_handle(), budget.clone(), Arc::new(|| {}));
                assert!(Instant::now() < deadline, "the count did not complete");
                std::thread::yield_now();
            }
        };
        wait_for(&mut counter, &view, 40_001);
        view.enqueue(Input::Insert("one\n".into()));
        drain(&mut view);
        view.enqueue(Input::Insert("two\n".into()));
        drain(&mut view);
        let latest = view.snapshot().clone();
        assert_ne!(latest.content_state, opened.content_state);
        let rebuilds = index.rebuilds();
        let scanned = index.scanned_bytes();
        let to = (latest.identity_token(), latest.content_state);
        index.follow_through(&latest, |from| view.peer.lock().ok()?.changes.chain(from, to));
        assert_eq!(
            index.rebuilds(),
            rebuilds,
            "the skipped commits restarted the index at byte zero"
        );
        wait_for(&mut counter, &view, 40_003);
        assert_eq!(index.rebuilds(), rebuilds);
        let rescanned = index.scanned_bytes() - scanned;
        assert!(rescanned < text.len() / 2, "recount read {rescanned} bytes");
        drop(counter);
        drop(view);
        remove_fixture_root(&root);
    }
    fn staging(root: &Path, budget: &Budget) -> crate::power::captured::StagingOptions {
        crate::power::captured::StagingOptions {
            cache: root.to_path_buf(),
            quota: 16 << 20,
            platform: Arc::new(Platform),
            source_options: Default::default(),
            budget: budget.clone(),
            memory: 8 << 20,
            cancellation: Cancellation::default(),
        }
    }
    fn document_text(view: &PagedEditorSurface, budget: &Budget) -> String {
        crate::power::captured::clipboard_text(
            view.read_handle(),
            &[TextOffset(0)..TextOffset(view.snapshot().len())],
            1 << 20,
            budget.clone(),
            Cancellation::default(),
        )
        .unwrap()
    }
    fn apply_staged(
        view: &mut PagedEditorSurface,
        before: &PagedSnapshot,
        transaction: bareline_document::paged::PreparedSourceTransaction,
    ) {
        let receipt = view.apply_prepared_source_tracked(before, transaction).unwrap();
        drain(view);
        assert!(receipt.terminal().unwrap().is_ok());
    }
    /// Applies a prepared input the way the composition root does: from memory
    /// when it is keystroke-sized, otherwise through its staged transaction.
    fn apply_prepared_input(
        view: &mut PagedEditorSurface,
        before: &PagedSnapshot,
        prepared: &mut crate::paged_power::PreparedPower,
    ) {
        let receipt = if let Some(edit) = prepared.materialized.take() {
            view.apply_materialized_power_tracked(before, edit).unwrap()
        } else {
            view.apply_prepared_source_tracked(before, prepared.transaction.take().expect("prepared edit"))
                .unwrap()
        };
        drain(view);
        assert!(receipt.terminal().unwrap().is_ok());
    }
    #[test]
    fn stale_rectangle_never_captures_typing_after_a_click_elsewhere() {
        let original = "abcd\n".repeat(6);
        let (root, mut view, budget) = paged_fixture("rectangle", &original);
        let options = staging(&root, &budget);
        let args = [
            ("first_line", 1),
            ("last_line", 3),
            ("start_column", 1),
            ("end_column", 2),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<crate::power::consumer::Arguments>();
        let prepared =
            crate::paged_power::prepare(view.capture_power(), "editor.rectangle.select", &args, &options).unwrap();
        assert!(prepared.transaction.is_none());
        view.install_power_state(
            &prepared.source,
            prepared.source.revision,
            prepared.selections,
            prepared.state,
            &prepared.hidden_lines,
        )
        .unwrap();
        drain(&mut view);
        assert_eq!(view.global_selection_set().selections.len(), 3);
        assert!(view.capture_power().state.rectangle.is_some());
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        view.surface.draw(&mut backend, 800.0, 400.0, &mut Vec::new()).unwrap();
        let point = bareline_renderer::Point {
            x: view.surface.text_left() + 10.0,
            y: view.surface.top() + 2.0,
        };
        view.click(&backend, point, false).unwrap();
        drain(&mut view);
        let (anchor, caret) = view.global_selection();
        assert_eq!(anchor, caret);
        assert!(caret.0 <= 4, "the click lands on the first line, outside the rectangle");
        assert!(view.capture_power().state.rectangle.is_none());
        let before = view.snapshot().clone();
        let mut typed =
            crate::paged_power::prepare_input(view.capture_power(), Input::Insert("X".into()), &options).unwrap();
        apply_prepared_input(&mut view, &before, &mut typed);
        let mut expected = original.clone();
        expected.insert(caret.0, 'X');
        assert_eq!(document_text(&view, &budget), expected);
        drop(view);
        drop(before);
        drop(prepared.source);
        drop(typed);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn paged_rectangle_backspace_and_cut_never_pad_short_rows() {
        let (root, mut view, budget) = paged_fixture("rectangle-delete", "abcd\nab\n\nabcd\n");
        let options = staging(&root, &budget);
        let rectangle = |start_column: usize, end_column: usize| {
            [
                ("first_line", 0),
                ("last_line", 3),
                ("start_column", start_column),
                ("end_column", end_column),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<crate::power::consumer::Arguments>()
        };
        let selected = crate::paged_power::prepare(
            view.capture_power(),
            "editor.rectangle.select",
            &rectangle(3, 3),
            &options,
        )
        .unwrap();
        view.install_power_state(
            &selected.source,
            selected.source.revision,
            selected.selections,
            selected.state,
            &selected.hidden_lines,
        )
        .unwrap();
        drain(&mut view);
        // A zero-width Backspace removes one grapheme per row that reaches the column.
        let before_delete = view.snapshot().clone();
        let mut deleted = crate::paged_power::prepare_input(view.capture_power(), Input::Backspace, &options).unwrap();
        assert_eq!(deleted.arguments.get("direction").map(String::as_str), Some("backward"));
        apply_staged(&mut view, &before_delete, deleted.transaction.take().unwrap());
        assert_eq!(document_text(&view, &budget), "abd\nab\n\nabd\n");
        // Cut removes the block without padding the empty row.
        let before_cut = view.snapshot().clone();
        let mut cut =
            crate::paged_power::prepare(view.capture_power(), "editor.rectangle.cut", &rectangle(1, 2), &options)
                .unwrap();
        apply_staged(&mut view, &before_cut, cut.transaction.take().unwrap());
        assert_eq!(document_text(&view, &budget), "ad\na\n\nad\n");
        drop(view);
        drop(before_delete);
        drop(before_cut);
        drop(selected.source);
        drop(deleted);
        drop(cut);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn paged_move_up_keeps_the_moved_line_selected() {
        let (root, mut view, budget) = paged_fixture("move-up", "a\r\nb\r\nc\r\n");
        let options = staging(&root, &budget);
        // The whole "c" line, including its line break.
        let mut selection = (TextOffset(6), TextOffset(9));
        let mut snapshots = Vec::new();
        for expected in ["a\r\nc\r\nb\r\n", "c\r\na\r\nb\r\n"] {
            let before = view.snapshot().clone();
            let transaction = crate::power::captured::prepare_transform(
                view.read_handle(),
                view.navigation.line_index(),
                &[selection.0..selection.1],
                crate::power::Transform::MoveUp,
                4,
                bareline_document::history::EditMetadata {
                    before: vec![bareline_document::history::Selection {
                        anchor: selection.0,
                        caret: selection.1,
                    }],
                    boundary: crate::power::consumer::next_receipt_sequence(),
                    ..Default::default()
                },
                &options,
            )
            .unwrap()
            .expect("the move changes text");
            let after = transaction.metadata().after.clone();
            apply_staged(&mut view, &before, transaction);
            snapshots.push(before);
            assert_eq!(document_text(&view, &budget), expected);
            assert_eq!(after.len(), 1);
            selection = (after[0].anchor, after[0].caret);
            assert_eq!(&expected[selection.0.0..selection.1.0], "c\r\n");
        }
        drop(view);
        drop(snapshots);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn paged_indent_keeps_adjacent_carets_on_their_characters() {
        // Carets between 日 and 本, and between 語 and x: one merged two-row range.
        let (root, mut view, budget) = paged_fixture("indent-carets", "日本\n語x\n");
        let options = staging(&root, &budget);
        let caret = |offset| bareline_document::history::Selection {
            anchor: TextOffset(offset),
            caret: TextOffset(offset),
        };
        let before = view.snapshot().clone();
        let transaction = crate::power::captured::prepare_transform(
            view.read_handle(),
            view.navigation.line_index(),
            &[TextOffset(3)..TextOffset(3), TextOffset(10)..TextOffset(10)],
            crate::power::Transform::Indent,
            4,
            bareline_document::history::EditMetadata {
                before: vec![caret(3), caret(10)],
                boundary: crate::power::consumer::next_receipt_sequence(),
                ..Default::default()
            },
            &options,
        )
        .unwrap()
        .expect("indent changes text");
        let after = transaction.metadata().after.clone();
        apply_staged(&mut view, &before, transaction);
        let text = document_text(&view, &budget);
        assert_eq!(text, "    日本\n    語x\n");
        for selection in &after {
            assert!(text.is_char_boundary(selection.anchor.0) && text.is_char_boundary(selection.caret.0));
        }
        // Each caret moves by its own row's indent, not the whole range's.
        assert_eq!(after, vec![caret(7), caret(18)]);
        drop(view);
        drop(before);
        drop(options);
        remove_fixture_root(&root);
    }
    fn paged_transform(
        view: &PagedEditorSurface,
        options: &crate::power::captured::StagingOptions,
        ranges: &[std::ops::Range<usize>],
        action: crate::power::Transform,
    ) -> Option<bareline_document::paged::PreparedSourceTransaction> {
        let ranges = ranges
            .iter()
            .map(|range| TextOffset(range.start)..TextOffset(range.end))
            .collect::<Vec<_>>();
        let before = ranges
            .iter()
            .map(|range| bareline_document::history::Selection {
                anchor: range.start,
                caret: range.end,
            })
            .collect();
        crate::power::captured::prepare_transform(
            view.read_handle(),
            view.navigation.line_index(),
            &ranges,
            action,
            4,
            bareline_document::history::EditMetadata {
                before,
                boundary: crate::power::consumer::next_receipt_sequence(),
                ..Default::default()
            },
            options,
        )
        .unwrap()
    }
    #[test]
    fn paged_line_break_for_an_unterminated_last_row_follows_the_document() {
        // EDT-24: the last row has no line break of its own, so the CRLF of the
        // document is used, never a hard-coded LF that would make it Mixed.
        for (name, text, range, action, expected) in [
            (
                "duplicate-crlf",
                "a\r\nb",
                3..4,
                crate::power::Transform::Duplicate,
                "a\r\nb\r\nb",
            ),
            (
                "split-crlf",
                "ab\r\ncd",
                4..6,
                crate::power::Transform::Split { column: 1 },
                "ab\r\nc\r\nd",
            ),
            (
                "duplicate-lf",
                "a\nb",
                2..3,
                crate::power::Transform::Duplicate,
                "a\nb\nb",
            ),
            ("duplicate-alone", "b", 0..1, crate::power::Transform::Duplicate, "b\nb"),
            // The line before ends in a multi-byte character: the probe for its
            // ending must not start inside that character.
            (
                "duplicate-lf-accent",
                "café\nlast",
                6..10,
                crate::power::Transform::Duplicate,
                "café\nlast\nlast",
            ),
            (
                "split-lf-accent",
                "café\nlast",
                6..10,
                crate::power::Transform::Split { column: 2 },
                "café\nla\nst",
            ),
            (
                "duplicate-crlf-accent",
                "café\r\nlast",
                7..11,
                crate::power::Transform::Duplicate,
                "café\r\nlast\r\nlast",
            ),
            (
                "split-crlf-accent",
                "café\r\nlast",
                7..11,
                crate::power::Transform::Split { column: 2 },
                "café\r\nla\r\nst",
            ),
            (
                "duplicate-cr-accent",
                "café\rlast",
                6..10,
                crate::power::Transform::Duplicate,
                "café\rlast\rlast",
            ),
            (
                "duplicate-lf-cjk",
                "日本\nlast",
                7..11,
                crate::power::Transform::Duplicate,
                "日本\nlast\nlast",
            ),
        ] {
            let (root, mut view, budget) = paged_fixture(name, text);
            let options = staging(&root, &budget);
            let before = view.snapshot().clone();
            let transaction = paged_transform(&view, &options, &[range], action).expect("changes text");
            apply_staged(&mut view, &before, transaction);
            assert_eq!(document_text(&view, &budget), expected, "{name}");
            drop(view);
            drop(before);
            drop(options);
            remove_fixture_root(&root);
        }
    }
    #[test]
    fn paged_transform_that_changes_nothing_prepares_no_edit() {
        // EDT-23: already trimmed or sorted lines submit nothing (no dirty flag,
        // no undo step).
        let (root, mut view, budget) = paged_fixture("no-op", "b\r\nx\r\n c\r\n");
        let options = staging(&root, &budget);
        let sort = crate::power::Transform::Sort {
            descending: false,
            case_sensitive: true,
            numeric: false,
        };
        assert!(
            paged_transform(
                &view,
                &options,
                &[Range { start: 0, end: 5 }],
                crate::power::Transform::Trim
            )
            .is_none()
        );
        assert!(paged_transform(&view, &options, &[Range { start: 0, end: 5 }], sort).is_none());
        // Of two ranges, only the one that changes is edited; the other keeps its caret.
        let before = view.snapshot().clone();
        let transaction =
            paged_transform(&view, &options, &[0..0, 7..7], crate::power::Transform::Trim).expect("changes text");
        let after = transaction.metadata().after.clone();
        apply_staged(&mut view, &before, transaction);
        assert_eq!(document_text(&view, &budget), "b\r\nx\r\nc\r\n");
        assert_eq!(after[0].caret, TextOffset(0));
        drop(view);
        drop(before);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn shift_navigation_that_did_not_move_leaves_no_hidden_selection() {
        let (root, mut view, budget) = paged_fixture("anchor", "abc\ndefgh\n");
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        view.surface.draw(&mut backend, 800.0, 400.0, &mut Vec::new()).unwrap();
        view.enqueue(Input::End(false));
        drain(&mut view);
        // Already at the line end: Shift+End moves nothing but records an anchor.
        view.enqueue(Input::End(true));
        drain(&mut view);
        assert_eq!(view.global_selection(), (TextOffset(3), TextOffset(3)));
        view.enqueue(Input::Down(false));
        let deadline = Instant::now() + Duration::from_secs(10);
        while view.surface.virtual_navigation_pending() {
            view.surface.draw(&mut backend, 800.0, 400.0, &mut Vec::new()).unwrap();
            view.pump_view();
            assert!(Instant::now() < deadline, "vertical navigation timed out");
            std::thread::yield_now();
        }
        drain(&mut view);
        let (anchor, caret) = view.global_selection();
        assert!(caret.0 > 4, "Down moves to the second line");
        assert_eq!(anchor, caret, "a plain arrow key leaves no hidden selection");
        view.enqueue(Input::Insert("X".into()));
        drain(&mut view);
        let text = document_text(&view, &budget);
        assert!(text.starts_with("abc\n"), "{text:?}");
        assert_eq!(text.matches('\n').count(), 2, "{text:?}");
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn plain_text_closer_inserts_and_overtype_is_all_or_nothing_across_carets() {
        let (root, mut view, budget) = paged_fixture("overtype", "a)\nb)\nc\n");
        let options = staging(&root, &budget);
        view.surface.language = bareline_syntax::Language::PlainText;
        view.global_selections = Selection { anchor: 1, caret: 1 }.into();
        view.project_global_selection();
        let before = view.snapshot().clone();
        let mut plain =
            crate::paged_power::prepare_input(view.capture_power(), Input::Insert(")".into()), &options).unwrap();
        assert!(
            plain.materialized.is_some() || plain.transaction.is_some(),
            "plain text inserts the closer"
        );
        apply_prepared_input(&mut view, &before, &mut plain);
        assert_eq!(document_text(&view, &budget), "a))\nb)\nc\n");
        // With pairing active, a caret before `)` and one elsewhere both insert.
        view.surface.language = bareline_syntax::Language::Rust;
        view.global_selections = crate::power::SelectionSet {
            selections: vec![Selection { anchor: 5, caret: 5 }, Selection { anchor: 8, caret: 8 }],
            primary: 0,
        };
        view.project_global_selection();
        let typed = view.snapshot().clone();
        let mut paired =
            crate::paged_power::prepare_input(view.capture_power(), Input::Insert(")".into()), &options).unwrap();
        apply_prepared_input(&mut view, &typed, &mut paired);
        assert_eq!(document_text(&view, &budget), "a))\nb))\nc)\n");
        drop(view);
        drop(before);
        drop(typed);
        drop(plain);
        drop(paired);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn line_commands_accept_a_selection_ending_after_a_multibyte_character() {
        let (root, mut view, budget) = paged_fixture("boundary", "aü\ncü\n");
        let options = staging(&root, &budget);
        // Hide lines: the second line is selected up to, not including, its newline.
        view.global_selections = Selection { anchor: 4, caret: 7 }.into();
        view.project_global_selection();
        let hidden = crate::paged_power::prepare(
            view.capture_power(),
            "editor.lines.hide",
            &crate::power::consumer::Arguments::new(),
            &options,
        )
        .unwrap();
        assert_eq!(hidden.hidden_lines, vec![1..2]);
        // Tab with the first line selected up to `ü` indents exactly that line.
        let before = view.snapshot().clone();
        let transaction = crate::power::captured::prepare_transform(
            view.read_handle(),
            view.navigation.line_index(),
            &[TextOffset(0)..TextOffset(3)],
            crate::power::Transform::Indent,
            4,
            bareline_document::history::EditMetadata {
                before: vec![bareline_document::history::Selection {
                    anchor: TextOffset(0),
                    caret: TextOffset(3),
                }],
                boundary: crate::power::consumer::next_receipt_sequence(),
                ..Default::default()
            },
            &options,
        )
        .unwrap()
        .expect("indent changes text");
        apply_staged(&mut view, &before, transaction);
        let text = document_text(&view, &budget);
        let (first, rest) = text.split_once('\n').unwrap();
        assert_ne!(first, "aü");
        assert_eq!(first.trim_start(), "aü");
        assert_eq!(rest, "cü\n");
        drop(view);
        drop(before);
        drop(hidden);
        drop(options);
        remove_fixture_root(&root);
    }
    /// Installs a prepared power result the way the composition root does.
    fn install_prepared(view: &mut PagedEditorSurface, prepared: crate::paged_power::PreparedPower) {
        view.install_power_state(
            &prepared.source,
            prepared.source.revision,
            prepared.selections,
            prepared.state,
            &prepared.hidden_lines,
        )
        .unwrap();
        drain(view);
    }
    /// Stages one input, applies it and installs its selections.
    fn type_input(view: &mut PagedEditorSurface, input: Input, options: &crate::power::captured::StagingOptions) {
        let before = view.snapshot().clone();
        let mut prepared = crate::paged_power::prepare_input(view.capture_power(), input, options).unwrap();
        apply_prepared_input(view, &before, &mut prepared);
        view.install_power_state(
            &before,
            view.snapshot().revision,
            prepared.selections,
            prepared.state,
            &prepared.hidden_lines,
        )
        .unwrap();
        drain(view);
    }
    #[test]
    fn carets_merged_by_backspace_then_typing_edits_once() {
        let (root, mut view, budget) = paged_fixture("merged-carets", "abcdef\n");
        let options = staging(&root, &budget);
        view.global_selections = crate::power::SelectionSet {
            selections: vec![Selection { anchor: 3, caret: 3 }, Selection { anchor: 4, caret: 4 }],
            primary: 1,
        };
        view.project_global_selection();
        // Both carets delete one character and land on the same offset.
        type_input(&mut view, Input::Backspace, &options);
        assert_eq!(document_text(&view, &budget), "abef\n");
        assert_eq!(
            view.global_selection_set().selections,
            vec![Selection { anchor: 2, caret: 2 }]
        );
        type_input(&mut view, Input::Insert("X".into()), &options);
        assert_eq!(document_text(&view, &budget), "abXef\n");
        drop(view);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn all_occurrences_select_disjoint_matches_that_accept_typing() {
        let (root, mut view, budget) = paged_fixture("occurrences-overlap", "aaaaa\n");
        let options = staging(&root, &budget);
        view.global_selections = Selection { anchor: 0, caret: 2 }.into();
        view.project_global_selection();
        let prepared = crate::paged_power::prepare(
            view.capture_power(),
            "editor.selection.allOccurrences",
            &crate::power::consumer::Arguments::new(),
            &options,
        )
        .unwrap();
        assert_eq!(
            prepared.selections.selections,
            vec![Selection { anchor: 0, caret: 2 }, Selection { anchor: 2, caret: 4 }]
        );
        install_prepared(&mut view, prepared);
        type_input(&mut view, Input::Insert("b".into()), &options);
        assert_eq!(document_text(&view, &budget), "bba\n");
        drop(view);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn next_occurrence_stops_at_the_first_free_match_and_wraps() {
        let (root, mut view, budget) = paged_fixture("occurrences-next", "foo x foo y foo\n");
        let options = staging(&root, &budget);
        let args = crate::power::consumer::Arguments::new();
        // An empty selection first selects the word under the caret.
        view.global_selections = Selection { anchor: 7, caret: 7 }.into();
        view.project_global_selection();
        let word =
            crate::paged_power::prepare(view.capture_power(), "editor.selection.nextOccurrence", &args, &options)
                .unwrap();
        assert_eq!(word.selections.selections, vec![Selection { anchor: 6, caret: 9 }]);
        install_prepared(&mut view, word);
        let next =
            crate::paged_power::prepare(view.capture_power(), "editor.selection.nextOccurrence", &args, &options)
                .unwrap();
        assert_eq!(
            next.selections.selections,
            vec![Selection { anchor: 6, caret: 9 }, Selection { anchor: 12, caret: 15 }]
        );
        assert_eq!(next.selections.primary, 1);
        install_prepared(&mut view, next);
        let wrapped =
            crate::paged_power::prepare(view.capture_power(), "editor.selection.nextOccurrence", &args, &options)
                .unwrap();
        assert_eq!(wrapped.selections.selections.len(), 3);
        assert_eq!(wrapped.selections.primary(), Selection { anchor: 0, caret: 3 });
        install_prepared(&mut view, wrapped);
        let exhausted =
            crate::paged_power::prepare(view.capture_power(), "editor.selection.nextOccurrence", &args, &options)
                .unwrap();
        assert_eq!(exhausted.selections.selections.len(), 3);
        view.global_selections = Selection { anchor: 6, caret: 9 }.into();
        view.project_global_selection();
        let skipped =
            crate::paged_power::prepare(view.capture_power(), "editor.selection.skipOccurrence", &args, &options)
                .unwrap();
        assert_eq!(skipped.selections.selections, vec![Selection { anchor: 12, caret: 15 }]);
        drop(view);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn word_selection_resolves_pages_that_are_not_resident() {
        // Small pages and a small cache: the word sits far from the viewport,
        // so its page is not resident and the worker must load it itself.
        let filler = format!("{}\n", "-".repeat(63)).repeat(16 * 1024);
        let text = format!("{filler}needle{filler}");
        let (root, mut view, budget) = paged_fixture_with(
            "occurrences-pending",
            &text,
            bareline_file_io::source::SourceOptions {
                resident_max_bytes: 0,
                page_size_bytes: 4096,
                page_cache_bytes: 256 * 1024,
            },
        );
        let options = staging(&root, &budget);
        let word = filler.len();
        view.global_selections = Selection {
            anchor: word + 2,
            caret: word + 2,
        }
        .into();
        view.project_global_selection();
        // A lookup that only waits on a pending page never finishes; the
        // watchdog turns that into a cancelled error instead of a hung test.
        let cancellation = options.cancellation.clone();
        let (done, finished) = mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            if finished.recv_timeout(Duration::from_secs(60)).is_err() {
                cancellation.cancel();
            }
        });
        let selected = crate::paged_power::prepare(
            view.capture_power(),
            "editor.selection.nextOccurrence",
            &crate::power::consumer::Arguments::new(),
            &options,
        );
        let _ = done.send(());
        watchdog.join().unwrap();
        let selected = selected.unwrap();
        assert_eq!(
            selected.selections.selections,
            vec![Selection {
                anchor: word,
                caret: word + "needle".len(),
            }]
        );
        drop(view);
        drop(selected);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn bookmarks_on_one_line_act_on_that_line_once() {
        let (root, mut view, budget) = paged_fixture("bookmark-lines", "ab\ncd\nef\n");
        let options = staging(&root, &budget);
        let args = crate::power::consumer::Arguments::new();
        // An edit left a second bookmark inside the first line.
        view.power_state.bookmarks = vec![0, 1, 3];
        let selected =
            crate::paged_power::prepare(view.capture_power(), "editor.bookmark.selectLines", &args, &options).unwrap();
        assert_eq!(
            selected.selections.selections,
            vec![Selection { anchor: 0, caret: 3 }, Selection { anchor: 3, caret: 6 }]
        );
        assert_eq!(selected.state.bookmarks, vec![0, 3]);
        // Toggling that line removes every bookmark on it and keeps the rest.
        view.power_state.bookmarks = vec![0, 1, 3];
        view.global_selections = Selection { anchor: 1, caret: 1 }.into();
        view.project_global_selection();
        let toggled =
            crate::paged_power::prepare(view.capture_power(), "editor.bookmark.toggle", &args, &options).unwrap();
        assert_eq!(toggled.state.bookmarks, vec![3]);
        view.power_state.bookmarks = vec![0, 1];
        let before = view.snapshot().clone();
        let mut deleted =
            crate::paged_power::prepare(view.capture_power(), "editor.bookmark.deleteLines", &args, &options).unwrap();
        apply_staged(
            &mut view,
            &before,
            deleted.transaction.take().expect("one line deletion"),
        );
        assert_eq!(document_text(&view, &budget), "cd\nef\n");
        assert!(deleted.state.bookmarks.is_empty());
        drop(view);
        drop(before);
        drop(deleted);
        drop(selected);
        drop(toggled);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn selections_beyond_the_paged_edit_limit_are_refused_with_a_clear_message() {
        let (root, mut view, budget) = paged_fixture("selection-limit", &"a\n".repeat(1100));
        let options = staging(&root, &budget);
        view.global_selections = Selection { anchor: 0, caret: 1 }.into();
        view.project_global_selection();
        let error = crate::paged_power::prepare(
            view.capture_power(),
            "editor.selection.allOccurrences",
            &crate::power::consumer::Arguments::new(),
            &options,
        )
        .err()
        .expect("more matches than one paged edit can record");
        assert!(error.contains("1024"), "{error}");
        view.global_selections = crate::power::SelectionSet {
            selections: (0..1100)
                .map(|line| Selection {
                    anchor: line * 2,
                    caret: line * 2,
                })
                .collect(),
            primary: 0,
        };
        view.project_global_selection();
        let error = crate::paged_power::prepare_input(view.capture_power(), Input::Insert("x".into()), &options)
            .err()
            .expect("too many carets for one paged edit");
        assert!(error.contains("1024"), "{error}");
        assert_eq!(document_text(&view, &budget), "a\n".repeat(1100));
        drop(view);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn keyboard_rectangle_extension_grows_left_and_up_from_a_fixed_anchor() {
        let (root, mut view, budget) = paged_fixture("rectangle-extend", &"abcd\n".repeat(6));
        let options = staging(&root, &budget);
        // Line 3, column 2.
        view.global_selections = Selection { anchor: 17, caret: 17 }.into();
        view.project_global_selection();
        let maps = (0..6)
            .map(|line| (line, crate::power::DisplayColumnMap::new("abcd", 4)))
            .collect::<std::collections::BTreeMap<_, _>>();
        let extend = |view: &mut PagedEditorSurface, dx: isize, dy: isize| {
            let mut capture = view.capture_power();
            capture.column_maps = Some(maps.clone());
            let args = [("dx".to_string(), dx.to_string()), ("dy".to_string(), dy.to_string())]
                .into_iter()
                .collect::<crate::power::consumer::Arguments>();
            let prepared = crate::paged_power::prepare(capture, "editor.rectangle.extend", &args, &options).unwrap();
            install_prepared(view, prepared);
            let rectangle = view.capture_power().state.rectangle.expect("rectangle kept");
            (
                rectangle.first_line,
                rectangle.last_line,
                rectangle.start_column,
                rectangle.end_column,
            )
        };
        assert_eq!(extend(&mut view, -1, 0), (3, 3, 1, 2));
        assert_eq!(extend(&mut view, 0, -1), (2, 3, 1, 2));
        assert_eq!(extend(&mut view, -1, -1), (1, 3, 0, 2));
        // Moving back toward the anchor shrinks the block again.
        assert_eq!(extend(&mut view, 1, 1), (2, 3, 1, 2));
        drop(view);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn staged_typing_is_one_undo_step_without_staging_stores() {
        let (root, mut view, budget) = paged_fixture("typing-run", "x\n");
        let options = staging(&root, &budget);
        // Merging needs keystrokes within the typing interval; lift it so the
        // test never depends on how quickly the actor answers.
        view.actor.lock_document().unwrap().document_mut().set_history_policy(
            bareline_document::history::HistoryPolicy {
                typing_interval_ms: u64::MAX,
                ..Default::default()
            },
        );
        let stores = || {
            std::fs::read_dir(&root)
                .unwrap()
                .filter(|entry| {
                    entry
                        .as_ref()
                        .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with("owned-stream"))
                })
                .count()
        };
        let initial = stores();
        for character in "hello".chars() {
            let before = view.snapshot().clone();
            let mut typed =
                crate::paged_power::prepare_input(view.capture_power(), Input::Insert(character.into()), &options)
                    .unwrap();
            assert!(typed.transaction.is_none(), "a keystroke stages no store");
            apply_prepared_input(&mut view, &before, &mut typed);
            view.install_power_state(
                &before,
                view.snapshot().revision,
                typed.selections,
                typed.state,
                &typed.hidden_lines,
            )
            .unwrap();
            drain(&mut view);
        }
        assert_eq!(document_text(&view, &budget), "hellox\n");
        assert_eq!(stores(), initial);
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "x\n");
        assert!(!view.can_undo(), "the typed word is one undo step");
        drop(view);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn large_selection_delete_stays_on_the_staged_path() {
        let (root, mut view, budget) = paged_fixture("large-delete", &format!("{}\n", "a".repeat(256 * 1024)));
        let options = staging(&root, &budget);
        // A keystroke-sized delete is applied from memory.
        view.global_selections = Selection { anchor: 0, caret: 1024 }.into();
        view.project_global_selection();
        let before = view.snapshot().clone();
        let mut small = crate::paged_power::prepare_input(view.capture_power(), Input::Delete, &options).unwrap();
        assert!(small.materialized.is_some() && small.transaction.is_none());
        apply_prepared_input(&mut view, &before, &mut small);
        // A large selection delete keeps its undo text in a staging store
        // instead of the shared in-memory byte and history budgets.
        view.global_selections = Selection {
            anchor: 0,
            caret: 200 * 1024,
        }
        .into();
        view.project_global_selection();
        let staged_before = view.snapshot().clone();
        let mut large = crate::paged_power::prepare_input(view.capture_power(), Input::Delete, &options).unwrap();
        assert!(large.transaction.is_some(), "a large delete is staged");
        assert!(
            large.materialized.is_none(),
            "a large delete is not applied from memory"
        );
        apply_prepared_input(&mut view, &staged_before, &mut large);
        assert_eq!(document_text(&view, &budget), format!("{}\n", "a".repeat(55 * 1024)));
        drop(view);
        drop(before);
        drop(staged_before);
        drop(small);
        drop(large);
        drop(options);
        remove_fixture_root(&root);
    }
    #[test]
    fn peer_owns_full_document_and_survives_other_view_close() {
        use bareline_file_io::{
            codecs::disk::DiskOptions,
            lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
            source::SourceOptions,
        };
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "bareline-paged-peer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("source.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 1024 * 1024,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    ..SourceOptions::default()
                },
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("open failed")
        };
        let mut first = PagedEditorSurface::new(opened, budget, Arc::new(|| {})).unwrap();
        drain(&mut first);
        let saved = first.read_handle();
        let mut second = first.clone_view().unwrap();
        drain(&mut second);
        let stale_lifecycle = first.actor.state().unwrap().stamp;
        assert!(first.snapshot().same_document(second.snapshot()));
        second.enqueue(Input::Insert("peer ".into()));
        drain(&mut second);
        let rejected_path = root.join("stale-save.txt");
        let rejected = first.actor.execute(
            bareline_file_io::paged_service::PagedLifecycleCommand::Save {
                requested: stale_lifecycle,
                destination: bareline_file_io::lifecycle::PreparedDestination {
                    path: rejected_path.clone(),
                    condition: bareline_file_io::lifecycle::DestinationCondition::MustBeAbsent,
                    consent: bareline_file_io::lifecycle::DestinationConsent::NotRequired,
                    document: (stale_lifecycle.document, stale_lifecycle.revision.0),
                    operation: bareline_file_io::lifecycle::SaveOperation::Save,
                },
                copy_only: false,
                platform: Arc::new(Platform),
            },
            &Cancellation::default(),
        );
        assert!(matches!(
            rejected.terminal,
            Err(bareline_file_io::paged_service::PagedLifecycleError::Changed)
        ));
        assert!(!rejected_path.exists());
        assert!(first.refresh_peer());
        drain(&mut first);
        assert_eq!(first.snapshot().content_state, second.snapshot().content_state);
        assert!(first.dirty() && second.dirty());
        assert_eq!(first.surface.snapshot.len(), second.surface.snapshot.len());
        let saved_path = root.join("saved-peer.txt");
        second.save(saved_path.clone(), None, Arc::new(Platform)).unwrap();
        drain(&mut second);
        assert!(first.refresh_peer());
        drain(&mut first);
        assert_eq!(first.path(), saved_path);
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert!(!first.dirty() && !second.dirty());
        assert!(!first.source_changed() && !second.source_changed());
        let mut captured = first.clone_captured_view(&saved).unwrap();
        drain(&mut captured);
        assert!(captured.surface.user_read_only);
        // REC-13: the pre-save capture differs from the saved state, yet a historical
        // view is never dirty, never saved in place, and never retires the live
        // document's recovery when it is closed.
        assert!(captured.historical() && !first.historical());
        assert_ne!(Some(captured.snapshot().content_state), first.actor.saved_state());
        assert!(!captured.dirty());
        captured.set_user_read_only(false);
        assert!(
            captured
                .save(root.join("historical-save.txt"), None, Arc::new(Platform))
                .is_err()
        );
        captured.set_user_read_only(true);
        assert_eq!(
            captured.discard_recovery(),
            bareline_file_io::recovery_retirement::DiscardPoll::Durable
        );
        assert_eq!(captured.snapshot().content_state, saved.snapshot().content_state);
        assert_eq!(captured.surface.snapshot.len(), 14);
        drop(first);
        second.enqueue(Input::Insert("alive ".into()));
        drain(&mut second);
        assert!(second.surface.snapshot.len() > 14);
        drop(second);
        captured.request_viewport(TextOffset(0)).unwrap();
        drain(&mut captured);
        assert_eq!(captured.surface.snapshot.len(), 14);
        drop(captured);
        drop(saved);
        remove_fixture_root(&root);
    }
    /// QA-08: a file event that arrives while the view is busy, here while a follow
    /// check that found nothing new waits to be pumped, is followed once the view is
    /// idle instead of being dropped until some later event.
    #[test]
    fn follow_request_while_busy_is_sent_once_the_view_is_idle() {
        use std::io::Write as _;
        let (root, mut view, _budget) = paged_fixture("follow-busy", "first\n");
        let path = view.path();
        let platform: Arc<dyn LocalFileSystem> = Arc::new(Platform);
        let settle = |view: &mut PagedEditorSurface| loop {
            drain(view);
            view.follow_tick(platform.clone(), false).unwrap();
            if !view.busy() {
                break;
            }
        };
        view.start_follow(platform.clone()).unwrap();
        settle(&mut view);
        assert_eq!(view.follow_status(), Some((false, false)));
        // A check that finds nothing new completes; the view has not pumped it yet.
        view.follow_tick(platform.clone(), true).unwrap();
        let completion = view.pending.take().unwrap().recv().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(completion).unwrap();
        view.pending = Some(receiver);
        assert!(view.busy());
        // The file grows, and its event reaches the still busy view.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"second\n")
            .unwrap();
        view.follow_tick(platform.clone(), true).unwrap();
        settle(&mut view);
        assert_eq!(view.snapshot().len(), "first\nsecond\n".len());
        assert_eq!(view.follow_status(), Some((false, false)));
        drop(view);
        let _ = std::fs::remove_dir_all(root);
    }
    /// Pump, drawing each turn, until a vertical move (which needs layouts) settles.
    fn settle_with_layout(view: &mut PagedEditorSurface, backend: &mut bareline_renderer_recording::RecordingBackend) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while view.busy() || view.surface.virtual_navigation_pending() {
            view.surface.draw(backend, 800.0, 400.0, &mut Vec::new()).unwrap();
            view.pump_view();
            assert!(Instant::now() < deadline, "paged navigation timed out");
            std::thread::yield_now();
        }
        drain(view);
    }
    #[test]
    fn restore_centres_a_line_aligned_window_and_keeps_a_loaded_one() {
        let (root, mut view, _budget) = paged_fixture("restore-window", &"abc\n".repeat(40_000));
        let length = view.snapshot().len();
        // Ctrl+End shows a full window of text ending at the last byte, not just
        // the last few bytes.
        view.enqueue(Input::DocumentEnd(false));
        drain(&mut view);
        assert_eq!(view.global_selection(), (TextOffset(length), TextOffset(length)));
        assert_eq!(view.viewport_start(), TextOffset(length - WINDOW));
        assert_eq!(view.viewport_start().0 + view.surface.snapshot().len(), length);
        assert!(view.caret_in_viewport());
        // A target already in the loaded window keeps that window.
        view.restore_global_selection(TextOffset(length - 100), TextOffset(length - 100), false)
            .unwrap();
        drain(&mut view);
        assert_eq!(view.viewport_start(), TextOffset(length - WINDOW));
        assert_eq!(view.global_selection().1, TextOffset(length - 100));
        // A distant Find hit is centred, with the window snapped to a line start.
        view.restore_global_selection(TextOffset(80_001), TextOffset(80_003), false)
            .unwrap();
        drain(&mut view);
        assert_eq!(view.global_selection(), (TextOffset(80_001), TextOffset(80_003)));
        assert_eq!(view.viewport_start(), TextOffset(47_236));
        assert!(view.selection_fully_in_viewport());
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn windows_never_start_or_end_inside_a_crlf() {
        let (root, mut view, budget) = paged_fixture("crlf-window", &"ab\r\n".repeat(40_000));
        // 4_003 is the LF of a CRLF; the window would also end between CR and LF.
        view.request_viewport(TextOffset(4_003)).unwrap();
        drain(&mut view);
        assert_eq!(view.viewport_start(), TextOffset(4_004));
        let local = view.surface.snapshot().len();
        assert_eq!(view.viewport_start().0 + local, 69_538);
        // The window ends before a whole CRLF: its last byte is the preceding "b",
        // never a stranded CR.
        assert_eq!(
            view.surface
                .snapshot()
                .read(TextOffset(local - 1)..TextOffset(local), 1)
                .unwrap(),
            "b"
        );
        view.enqueue(Input::SetCaret(0, false));
        view.enqueue(Input::Insert("x".into()));
        drain(&mut view);
        let text = document_text(&view, &budget);
        // Source bytes 69_538..69_540 (69_539..69_541 after the insert) are one CRLF.
        assert_eq!(&text[69_539..69_541], "\r\n");
        assert_eq!(&text[4_000..4_009], "ab\r\nxab\r\n");
        assert!(!text.contains("\rx"));
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn navigation_at_a_window_edge_requests_the_adjacent_window_and_replays() {
        let (root, mut view, _budget) = paged_fixture("edge-navigation", &"abc\n".repeat(40_000));
        let end = view.viewport_start().0 + view.surface.snapshot().len();
        assert_eq!(end, WINDOW);
        view.restore_global_selection(TextOffset(end), TextOffset(end), true)
            .unwrap();
        drain(&mut view);
        view.enqueue(Input::Right(false));
        drain(&mut view);
        assert_eq!(view.global_selection(), (TextOffset(end + 1), TextOffset(end + 1)));
        assert_eq!(view.viewport_start(), TextOffset(end - WINDOW / 2));
        // Down on the last line of a window continues into the next one.
        let end = view.viewport_start().0 + view.surface.snapshot().len();
        view.restore_global_selection(TextOffset(end), TextOffset(end), true)
            .unwrap();
        drain(&mut view);
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        view.surface.draw(&mut backend, 800.0, 400.0, &mut Vec::new()).unwrap();
        view.enqueue(Input::Down(false));
        settle_with_layout(&mut view, &mut backend);
        assert_eq!(view.global_selection(), (TextOffset(end + 4), TextOffset(end + 4)));
        assert!(view.viewport_start().0 > end - WINDOW);
        // Shift+Down across the edge keeps the selection's anchor.
        let end = view.viewport_start().0 + view.surface.snapshot().len();
        view.restore_global_selection(TextOffset(end - 8), TextOffset(end), true)
            .unwrap();
        drain(&mut view);
        view.surface.draw(&mut backend, 800.0, 400.0, &mut Vec::new()).unwrap();
        view.enqueue(Input::Down(true));
        settle_with_layout(&mut view, &mut backend);
        assert_eq!(view.global_selection(), (TextOffset(end - 8), TextOffset(end + 4)));
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn prepared_edit_and_its_undo_transform_the_caret() {
        let (root, mut view, budget) = paged_fixture("prepared-caret", "abcd\u{e9}\nnext\n");
        // The caret sits after the two-byte `é`.
        view.restore_global_selection(TextOffset(6), TextOffset(6), true)
            .unwrap();
        drain(&mut view);
        let before = view.snapshot().clone();
        view.apply_prepared(
            &before,
            EditTransaction {
                base_revision: before.revision,
                edits: vec![Edit {
                    range: TextOffset(0)..TextOffset(1),
                    insert: "xy".into(),
                }],
            },
        )
        .unwrap();
        drain(&mut view);
        // Untransformed, offset 6 would now split the `é`.
        assert_eq!(view.global_selection(), (TextOffset(7), TextOffset(7)));
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(view.global_selection(), (TextOffset(6), TextOffset(6)));
        view.enqueue(Input::Redo);
        drain(&mut view);
        assert_eq!(view.global_selection(), (TextOffset(7), TextOffset(7)));
        view.enqueue(Input::Insert("!".into()));
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "xybcd\u{e9}!\nnext\n");
        drop(view);
        drop(before);
        remove_fixture_root(&root);
    }
    #[test]
    fn clone_view_selection_and_window_follow_a_peer_edit() {
        let (root, mut view, _budget) = paged_fixture("peer-selection", "one\ntwo\nthree\n");
        let mut peer = view.clone_view().unwrap();
        drain(&mut peer);
        peer.restore_global_selection(TextOffset(8), TextOffset(8), true)
            .unwrap();
        drain(&mut peer);
        view.enqueue(Input::Insert("XYZ".into()));
        drain(&mut view);
        assert!(peer.refresh_peer());
        drain(&mut peer);
        assert_eq!(peer.global_selection(), (TextOffset(11), TextOffset(11)));
        // Text inserted at the top of the peer's window stays in view.
        assert_eq!(peer.viewport_start(), TextOffset(0));
        assert_eq!(peer.surface.snapshot().len(), view.snapshot().len());
        drop(peer);
        drop(view);
        remove_fixture_root(&root);
    }
    /// PED-13: a peer that refreshes only after several commits maps its
    /// selection through every one of them, not only the last.
    #[test]
    fn clone_view_selection_follows_several_peer_commits() {
        let (root, mut view, _budget) = paged_fixture("peer-selection-chain", "one\ntwo\nthree\n");
        let mut peer = view.clone_view().unwrap();
        drain(&mut peer);
        peer.restore_global_selection(TextOffset(8), TextOffset(8), true)
            .unwrap();
        drain(&mut peer);
        // "XYZ" lands at 0 and "AB" after it, both before the peer's caret.
        for insert in ["XYZ", "AB"] {
            view.enqueue(Input::Insert(insert.into()));
            drain(&mut view);
        }
        assert!(peer.refresh_peer());
        drain(&mut peer);
        assert_eq!(peer.global_selection(), (TextOffset(13), TextOffset(13)));
        drop(peer);
        drop(view);
        remove_fixture_root(&root);
    }
    /// PED-13: the refreshed window starts over the same text after several
    /// commits above it.
    #[test]
    fn clone_view_window_follows_several_peer_commits() {
        let (root, mut view, _budget) = paged_fixture("peer-window-chain", &"x\n".repeat(100_000));
        let mut peer = view.clone_view().unwrap();
        drain(&mut peer);
        peer.request_viewport(TextOffset(100_000)).unwrap();
        drain(&mut peer);
        assert_eq!(peer.viewport_start(), TextOffset(100_000));
        for insert in ["XYZ", "AB"] {
            view.enqueue(Input::Insert(insert.into()));
            drain(&mut view);
        }
        assert!(peer.refresh_peer());
        drain(&mut peer);
        assert_eq!(peer.viewport_start(), TextOffset(100_005));
        drop(peer);
        drop(view);
        remove_fixture_root(&root);
    }
    /// PED-11: without the receipts (a trimmed log) the peer clamps its caret,
    /// then snaps it back to a character boundary so typing still works.
    #[test]
    fn clone_view_without_receipts_snaps_a_clamped_caret_to_a_character() {
        let (root, mut view, budget) = paged_fixture("peer-resnap", "a\u{e9}\n");
        let mut peer = view.clone_view().unwrap();
        drain(&mut peer);
        peer.restore_global_selection(TextOffset(3), TextOffset(3), true)
            .unwrap();
        drain(&mut peer);
        for _ in 0..2 {
            view.enqueue(Input::Insert("\u{e9}".into()));
            drain(&mut view);
        }
        assert_eq!(document_text(&view, &budget), "\u{e9}\u{e9}a\u{e9}\n");
        view.peer.lock().unwrap().changes.clear();
        assert!(peer.refresh_peer());
        drain(&mut peer);
        // Offset 3 is inside the second "é"; the caret moves to its start.
        assert_eq!(peer.global_selection(), (TextOffset(2), TextOffset(2)));
        peer.enqueue(Input::Insert("z".into()));
        drain(&mut peer);
        assert_eq!(document_text(&peer, &budget), "\u{e9}z\u{e9}a\u{e9}\n");
        drop(peer);
        drop(view);
        remove_fixture_root(&root);
    }
    /// WSP-09: the paged edits of one ended macro run undo and redo as one
    /// step; while the run is open its own Undo steps one entry.
    #[test]
    fn one_undo_and_redo_move_a_whole_ended_paged_undo_run() {
        let (root, mut view, budget) = paged_fixture("paged-undo-run", "text\n");
        view.begin_undo_run(7);
        for insert in ["a", "b"] {
            view.enqueue(Input::Insert(insert.into()));
            drain(&mut view);
        }
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "atext\n");
        view.enqueue(Input::Redo);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "abtext\n");
        view.end_undo_run();
        view.enqueue(Input::Insert("c".into()));
        drain(&mut view);
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "abtext\n");
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "text\n");
        assert_eq!(view.global_selection(), (TextOffset(0), TextOffset(0)));
        view.enqueue(Input::Redo);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "abtext\n");
        assert_eq!(view.global_selection(), (TextOffset(2), TextOffset(2)));
        drop(view);
        remove_fixture_root(&root);
    }
    /// PED-17: an input whose preparation could not start goes back to the
    /// front of the queue and is taken again before later keystrokes.
    #[test]
    fn a_requeued_power_input_is_taken_again_in_order() {
        let (root, mut view, _budget) = paged_fixture("power-requeue", "text\n");
        view.enable_power_input();
        view.enqueue(Input::Insert("a".into()));
        view.enqueue(Input::Insert("b".into()));
        let first = view.take_power_input().unwrap();
        assert!(view.take_power_input().is_none());
        view.requeue_power_input(first);
        assert!(view.busy());
        assert!(matches!(view.take_power_input(), Some(Input::Insert(text)) if text == "a"));
        view.finish_power_preparation();
        assert!(matches!(view.take_power_input(), Some(Input::Insert(text)) if text == "b"));
        view.finish_power_preparation();
        assert!(view.take_power_input().is_none());
        drop(view);
        remove_fixture_root(&root);
    }
    /// PED-15: keystrokes applied while more input is queued defer their recovery
    /// appends, and the burst's last keystroke journals them all as one record: one
    /// durability point for the burst instead of one per keystroke.
    #[test]
    fn queued_typing_is_journaled_as_one_recovery_record() {
        let (root, mut view, budget) = paged_fixture("deferred-journal", "text\n");
        view.enable_recovery(root.join("recovery"), Arc::new(JournalPlatform));
        let options = staging(&root, &budget);
        view.enable_power_input();
        for text in ["a", "b", "c"] {
            view.enqueue(Input::Insert(text.into()));
        }
        // The queued keystrokes keep the view busy, so drive them like the composition
        // root (power_stream.rs) does: take one whenever the actor is free, finish its
        // preparation, apply it, and pump.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut receipts = Vec::new();
        while !view.power_inputs.is_empty() || view.power_actor_busy() {
            if let Some(input) = view.take_power_input() {
                let before = view.snapshot().clone();
                let mut prepared = crate::paged_power::prepare_input(view.capture_power(), input, &options).unwrap();
                let edit = prepared
                    .materialized
                    .take()
                    .expect("a keystroke is applied from memory");
                view.finish_power_preparation();
                receipts.push(view.apply_materialized_power_tracked(&before, edit).unwrap());
            }
            view.pump();
            assert!(view.error.is_none(), "{:?}", view.error);
            assert!(Instant::now() < deadline, "paged typing timed out");
            std::thread::yield_now();
        }
        drain(&mut view);
        assert_eq!(receipts.len(), 3);
        assert!(receipts.iter().all(|receipt| receipt.terminal().unwrap().is_ok()));
        assert_eq!(document_text(&view, &budget), "abctext\n");
        let directory = view.recovery_status().directory.expect("the burst is journaled");
        let inspection = bareline_file_io::recovery::inspect(&directory, &Cancellation::default()).unwrap();
        assert_eq!(inspection.validated_records, 1, "one record for three keystrokes");
        assert_eq!(
            inspection.last_durable.map(|receipt| receipt.revision),
            Some(view.snapshot().revision.0)
        );
        assert!(view.recovery_status().error.is_none());
        drop(view);
        let _ = std::fs::remove_dir_all(root);
    }
    /// PED-15: when a burst's last queued input makes no edit (its preparation is
    /// dropped, as the composition root does when the document or selection changed),
    /// the batch the earlier keystrokes deferred has no edit left to journal it. The
    /// view wakes itself for that: an event loop that pumps only when woken still sees
    /// the batch become durable, as one record, and reports it pending until then.
    #[test]
    fn deferred_batch_is_journaled_when_the_last_queued_input_makes_no_edit() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (root, mut view, budget) = paged_fixture("deferred-idle", "text\n");
        view.enable_recovery(root.join("recovery"), Arc::new(JournalPlatform));
        let options = staging(&root, &budget);
        let woken = Arc::new(AtomicBool::new(false));
        let notify: Arc<dyn Fn() + Send + Sync> = {
            let woken = woken.clone();
            Arc::new(move || woken.store(true, Ordering::SeqCst))
        };
        view.notify = notify;
        view.enable_power_input();
        for text in ["a", "b", "c"] {
            view.enqueue(Input::Insert(text.into()));
        }
        // Apply the first two keystrokes; both defer behind the input queued after them.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut applied = 0;
        while applied < 2 || view.power_actor_busy() {
            if applied < 2
                && let Some(input) = view.take_power_input()
            {
                let before = view.snapshot().clone();
                let mut prepared = crate::paged_power::prepare_input(view.capture_power(), input, &options).unwrap();
                let edit = prepared
                    .materialized
                    .take()
                    .expect("a keystroke is applied from memory");
                view.finish_power_preparation();
                let _receipt = view.apply_materialized_power_tracked(&before, edit).unwrap();
                applied += 1;
            }
            view.pump();
            assert!(view.error.is_none(), "{:?}", view.error);
            assert!(Instant::now() < deadline, "paged typing timed out");
            std::thread::yield_now();
        }
        assert!(view.recovery_batch_pending(), "the keystrokes deferred their appends");
        // The last input is taken and dropped without an edit.
        woken.store(false, Ordering::SeqCst);
        assert!(view.take_power_input().is_some());
        view.finish_power_preparation();
        while view.recovery_batch_pending()
            || view.recovery_status().durable.map(|receipt| receipt.revision) != Some(view.snapshot().revision.0)
        {
            if woken.swap(false, Ordering::SeqCst) {
                view.pump();
            }
            assert!(Instant::now() < deadline, "the deferred batch was never journaled");
            std::thread::yield_now();
        }
        assert_eq!(document_text(&view, &budget), "abtext\n");
        let directory = view.recovery_status().directory.expect("the batch is journaled");
        let inspection = bareline_file_io::recovery::inspect(&directory, &Cancellation::default()).unwrap();
        assert_eq!(inspection.validated_records, 1, "one record for the two keystrokes");
        assert!(view.recovery_status().error.is_none());
        drop(view);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn undo_while_the_document_lock_is_held_waits_instead_of_failing() {
        let (root, mut view, budget) = paged_fixture("busy-undo", "text\n");
        view.enqueue(Input::Insert("A".into()));
        drain(&mut view);
        let actor = view.actor.clone();
        let guard = actor.lock_document().unwrap();
        view.enqueue(Input::Undo);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert!(view.busy());
        drop(guard);
        drain(&mut view);
        assert_eq!(document_text(&view, &budget), "text\n");
        assert_eq!(view.global_selection(), (TextOffset(0), TextOffset(0)));
        drop(actor);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn a_failed_window_read_after_a_commit_keeps_the_selection_and_retries() {
        let (root, mut view, budget) = paged_fixture("commit-window-retry", "text\nmore\n");
        view.enqueue(Input::Insert("AB".into()));
        // Intercept the committed result and fail its window read.
        let receiver = view.pending.take().expect("typing submits an edit");
        let mut completed = receiver.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        completed.window = Err("injected window failure".into());
        let start = completed.start;
        let (sender, injected) = mpsc::sync_channel(1);
        sender.send(Ok(completed)).unwrap();
        view.pending = Some(injected);
        assert!(view.pump());
        // The commit's selection is installed although no window could be shown.
        assert_eq!(view.global_selection(), (TextOffset(2), TextOffset(2)));
        assert_eq!(view.error.as_deref(), Some("injected window failure"));
        assert!(!view.viewport_valid);
        assert_eq!(view.queued_viewport, Some(TextOffset(start)));
        // The queued retry reads the committed text and clears the error.
        drain(&mut view);
        assert!(view.viewport_valid);
        assert_eq!(view.queued_viewport, None);
        assert_eq!(view.global_selection(), (TextOffset(2), TextOffset(2)));
        assert_eq!(view.surface.snapshot().len(), view.snapshot().len());
        assert_eq!(document_text(&view, &budget), "ABtext\nmore\n");
        drop(view);
        remove_fixture_root(&root);
    }
    /// A forced-paged document with one collapsed fold over lines 2..=51.
    fn folded_fixture(name: &str) -> (std::path::PathBuf, PagedEditorSurface, Budget, usize) {
        let text = format!("intro\nheader\n{}suffix\n", "body line\n".repeat(50));
        let suffix = text.find("suffix").unwrap();
        let (root, mut view, budget) = paged_fixture(name, &text);
        fold_body(&mut view, suffix);
        (root, view, budget, suffix)
    }
    fn fold_body(view: &mut PagedEditorSurface, body_end: usize) {
        view.set_known_anchored_folds(
            vec![bareline_syntax::folding::AnchoredFold {
                fold: bareline_syntax::folding::Fold {
                    header: 1,
                    end: 51,
                    level: 1,
                },
                header: TextOffset(6),
                body: TextOffset(13)..TextOffset(body_end),
            }],
            0,
            false,
            0,
        )
        .unwrap();
        view.fold_all_known(1);
        drain(view);
        assert_eq!(
            view.surface
                .snapshot()
                .read(TextOffset(0)..TextOffset(view.surface.snapshot().len()), WINDOW)
                .unwrap(),
            "intro\nheader\nsuffix\n"
        );
    }
    #[test]
    fn find_inside_a_collapsed_fold_expands_it_and_typing_never_edits_hidden_text() {
        let (root, mut view, budget, suffix) = folded_fixture("fold-reveal");
        // A Find hit or Go To landing in the hidden body reveals it.
        view.restore_global_selection(TextOffset(20), TextOffset(20), false)
            .unwrap();
        drain(&mut view);
        assert!(!view.global_fold_state.collapsed.contains(&1));
        assert!(view.local_offset(TextOffset(20)).is_some());
        assert!(view.caret_in_viewport());
        view.enqueue(Input::Insert("X".into()));
        drain(&mut view);
        assert_eq!(&document_text(&view, &budget)[20..21], "X");
        // A caret left inside a collapsed body is revealed before typing lands.
        fold_body(&mut view, suffix + 1);
        view.restore_global_selection(TextOffset(20), TextOffset(20), true)
            .unwrap();
        drain(&mut view);
        assert!(view.global_fold_state.collapsed.contains(&1));
        view.enqueue(Input::Insert("Y".into()));
        drain(&mut view);
        assert!(!view.global_fold_state.collapsed.contains(&1));
        assert_eq!(&document_text(&view, &budget)[20..22], "YX");
        assert!(view.local_offset(TextOffset(21)).is_some());
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn backspace_at_a_fold_seam_never_deletes_the_hidden_body() {
        let (root, mut view, budget, suffix) = folded_fixture("fold-seam");
        let original = document_text(&view, &budget);
        // The caret starts the line after the collapsed body (local seam 13).
        view.restore_global_selection(TextOffset(suffix), TextOffset(suffix), true)
            .unwrap();
        drain(&mut view);
        assert_eq!(view.local_offset(TextOffset(suffix)), Some(TextOffset(13)));
        view.enqueue(Input::Backspace);
        assert!(!view.busy());
        assert!(view.error.as_ref().is_some_and(|error| error.contains("Unfold")));
        assert_eq!(document_text(&view, &budget), original);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn fold_anchor_install_keeps_the_newest_anchor_per_header() {
        let (root, mut view, _budget) = paged_fixture("fold-anchors", "a\nb\nc\n");
        let anchor = |header: usize, end: usize| mapped_viewport::FoldAnchor {
            header: TextOffset(header),
            body: TextOffset(header + 1),
            end: TextOffset(end),
            fold: bareline_syntax::folding::Fold { header, end, level: 1 },
            collapsed: false,
            lines_known: true,
        };
        view.merge_known_fold_anchors(vec![anchor(0, 4), anchor(2, 6)]);
        view.merge_known_fold_anchors(vec![anchor(2, 8), anchor(4, 9), anchor(2, 10)]);
        let known: Vec<_> = view
            .known_fold_anchors
            .iter()
            .map(|anchor| (anchor.header.0, anchor.end.0))
            .collect();
        assert_eq!(known, vec![(0, 4), (4, 9), (2, 10)]);
        drop(view);
        remove_fixture_root(&root);
    }
    #[test]
    fn search_marks_follow_source_typing_undo_and_redo() {
        let (root, mut view, budget) = paged_fixture("marks", "one two three\n");
        let options = staging(&root, &budget);
        view.set_search_marks(1, vec![TextOffset(8)..TextOffset(13)]).unwrap();
        let marks = |view: &PagedEditorSurface| view.search_marks.iter().collect::<Vec<_>>();
        let before = view.snapshot().clone();
        let mut typed =
            crate::paged_power::prepare_input(view.capture_power(), Input::Insert("X".into()), &options).unwrap();
        // A keystroke is applied from memory (PED-15); marks follow it the same way.
        apply_prepared_input(&mut view, &before, &mut typed);
        assert_eq!(marks(&view), vec![(1, TextOffset(9)..TextOffset(14))]);
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(marks(&view), vec![(1, TextOffset(8)..TextOffset(13))]);
        view.enqueue(Input::Redo);
        drain(&mut view);
        assert_eq!(marks(&view), vec![(1, TextOffset(9)..TextOffset(14))]);
        drop(view);
        drop(before);
        drop(typed);
        drop(options);
        remove_fixture_root(&root);
    }
}
