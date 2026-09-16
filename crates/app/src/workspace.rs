// SPDX-License-Identifier: MPL-2.0
use bareline_document::{Budget, Document, service::Scheduler};
pub use bareline_editor_surface::Input;
use bareline_editor_surface::{EditorSurface, paged_view::PagedEditorSurface};
use bareline_file_io::lifecycle::{
    DestinationCondition, DestinationConsent, FileError, Fingerprint, IoCompletion, IoRequest, IoService, IoTicket,
    PreparedDestination, SaveCleanup, SaveConflict, SaveOperation,
};
use bareline_platform::LocalFileSystem;
use bareline_renderer::{DrawOp, LayoutError, Rect, TextBackend};
use std::sync::Arc;
use std::{path::PathBuf, sync::mpsc::TryRecvError};

mod encoding;
mod new_document;
mod remote;
pub use bareline_file_io::codecs::failure::EncodingFailure;
pub enum WorkspaceEditor {
    Resident(EditorSurface),
    Paged(PagedEditorSurface),
}
/// A tab the user closed and may reopen. A document that is saved and still on
/// disk is remembered by path only, so its paged source, spill store and
/// transcode directory are released as soon as the tab closes; documents with
/// unsaved work must keep their model because nothing on disk can rebuild it.
enum ClosedDocument {
    Retained(Box<WorkspaceEditor>, Option<FileState>, String),
    Reopen(PathBuf),
}
impl ClosedDocument {
    fn paged(&self) -> bool {
        matches!(self, Self::Retained(editor, ..) if editor.paged())
    }
}
impl From<EditorSurface> for WorkspaceEditor {
    fn from(value: EditorSurface) -> Self {
        Self::Resident(value)
    }
}
/// Raised when a whole-document operation that only a resident editor can
/// perform is requested on a paged (large-file) editor, whose text is never
/// fully in memory. Callers surface this to the status bar instead of silently
/// operating on the bounded viewport placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotSupportedForPaged {
    pub operation: &'static str,
}
impl NotSupportedForPaged {
    pub const fn new(operation: &'static str) -> Self {
        Self { operation }
    }
}
impl std::fmt::Display for NotSupportedForPaged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is not available for large files", self.operation)
    }
}
impl std::error::Error for NotSupportedForPaged {}
impl From<NotSupportedForPaged> for String {
    fn from(value: NotSupportedForPaged) -> Self {
        value.to_string()
    }
}

/// The read/state API shared by both editor variants. Whole-document editing
/// and query operations that only a resident editor supports are intentionally
/// absent; use `WorkspaceEditor`'s inherent methods for those, which report
/// [`NotSupportedForPaged`] on paged editors rather than touching the viewport
/// placeholder.
pub trait EditorView {
    fn can_undo(&self) -> bool;
    fn can_redo(&self) -> bool;
    fn dirty(&self) -> bool;
    fn busy(&self) -> bool;
    fn recovery_status(&self) -> bareline_file_io::paged_recovery::PagedRecoveryStatus;
}
impl EditorView for EditorSurface {
    fn can_undo(&self) -> bool {
        EditorSurface::can_undo(self)
    }
    fn can_redo(&self) -> bool {
        EditorSurface::can_redo(self)
    }
    fn dirty(&self) -> bool {
        EditorSurface::dirty(self)
    }
    fn busy(&self) -> bool {
        EditorSurface::busy(self)
    }
    fn recovery_status(&self) -> bareline_file_io::paged_recovery::PagedRecoveryStatus {
        EditorSurface::recovery_status(self)
    }
}
impl EditorView for PagedEditorSurface {
    fn can_undo(&self) -> bool {
        PagedEditorSurface::can_undo(self)
    }
    fn can_redo(&self) -> bool {
        PagedEditorSurface::can_redo(self)
    }
    fn dirty(&self) -> bool {
        PagedEditorSurface::dirty(self)
    }
    fn busy(&self) -> bool {
        PagedEditorSurface::busy(self)
    }
    fn recovery_status(&self) -> bareline_file_io::paged_recovery::PagedRecoveryStatus {
        PagedEditorSurface::recovery_status(self)
    }
}
impl EditorView for WorkspaceEditor {
    fn can_undo(&self) -> bool {
        self.can_undo()
    }
    fn can_redo(&self) -> bool {
        self.can_redo()
    }
    fn dirty(&self) -> bool {
        self.dirty()
    }
    fn busy(&self) -> bool {
        self.busy()
    }
    fn recovery_status(&self) -> bareline_file_io::paged_recovery::PagedRecoveryStatus {
        self.recovery_status()
    }
}
impl WorkspaceEditor {
    pub fn scroll_horizontal(&mut self, delta: f64) {
        match self {
            Self::Resident(editor) => editor.scroll_horizontal(delta),
            Self::Paged(editor) => editor.scroll_horizontal(delta),
        }
    }
    /// Route native shaped hit testing to the authoritative editor variant.
    pub fn click(
        &mut self,
        backend: &impl TextBackend,
        point: bareline_renderer::Point,
        extend: bool,
    ) -> Result<(), LayoutError> {
        match self {
            Self::Resident(editor) => editor.click(backend, point, extend),
            Self::Paged(editor) => editor.click(backend, point, extend),
        }
    }
    pub fn commit(&mut self, value: String) {
        self.commit_with_origin(value, bareline_document::history::EditOrigin::Command);
    }
    pub fn commit_with_origin(&mut self, value: String, origin: bareline_document::history::EditOrigin) {
        self.cancel_composition();
        self.enqueue_with_origin(Input::Insert(value), origin);
    }
    pub fn enqueue_with_origin(&mut self, input: Input, origin: bareline_document::history::EditOrigin) {
        match self {
            Self::Resident(editor) => editor.enqueue_with_origin(input, origin),
            Self::Paged(editor) => editor.enqueue(input),
        }
    }
    pub fn enqueue(&mut self, input: Input) {
        match self {
            Self::Resident(e) => e.enqueue(input),
            Self::Paged(e) => e.enqueue(input),
        }
    }
    pub fn pump(&mut self) -> bool {
        match self {
            Self::Resident(e) => e.pump(),
            Self::Paged(e) => e.pump_view(),
        }
    }
    /// Borrow the resident surface, or `None` for a paged editor.
    pub fn resident(&self) -> Option<&EditorSurface> {
        match self {
            Self::Resident(e) => Some(e),
            Self::Paged(_) => None,
        }
    }
    /// Mutably borrow the resident surface, or `None` for a paged editor.
    pub fn resident_mut(&mut self) -> Option<&mut EditorSurface> {
        match self {
            Self::Resident(e) => Some(e),
            Self::Paged(_) => None,
        }
    }
    /// Clear any in-flight IME composition on the bounded viewport surface.
    pub fn cancel_composition(&mut self) {
        match self {
            Self::Resident(e) => e.cancel_composition(),
            Self::Paged(e) => e.cancel_composition(),
        }
    }
    pub fn can_undo(&self) -> bool {
        match self {
            Self::Resident(editor) => editor.can_undo(),
            Self::Paged(editor) => editor.can_undo(),
        }
    }
    pub fn can_redo(&self) -> bool {
        match self {
            Self::Resident(editor) => editor.can_redo(),
            Self::Paged(editor) => editor.can_redo(),
        }
    }
    pub fn dirty(&self) -> bool {
        match self {
            Self::Resident(e) => e.dirty(),
            Self::Paged(e) => e.dirty(),
        }
    }
    pub fn busy(&self) -> bool {
        match self {
            Self::Resident(e) => e.busy(),
            Self::Paged(e) => e.busy(),
        }
    }
    pub fn read_only(&self) -> bool {
        match self {
            Self::Resident(e) => e.read_only(),
            Self::Paged(e) => e.user_read_only(),
        }
    }
    pub fn set_read_only(&mut self, value: bool) {
        match self {
            Self::Resident(e) => e.user_read_only = value,
            Self::Paged(e) => e.set_user_read_only(value),
        }
    }
    pub fn recovery_status(&self) -> bareline_file_io::paged_recovery::PagedRecoveryStatus {
        match self {
            Self::Resident(editor) => editor.recovery_status(),
            Self::Paged(editor) => editor.recovery_status(),
        }
    }
    pub fn retry_recovery(&mut self) -> Result<(), String> {
        match self {
            Self::Resident(editor) => {
                editor.retry_recovery();
                Ok(())
            }
            Self::Paged(editor) => editor.retry_recovery(),
        }
    }
    /// Request durable recovery retirement when the user discards on close or exit.
    pub fn discard_recovery(&mut self) -> bareline_file_io::recovery_retirement::DiscardPoll {
        match self {
            Self::Resident(editor) => editor.discard_recovery(),
            Self::Paged(editor) => editor.discard_recovery(),
        }
    }
    pub fn resume_recovery_after_discard(&mut self) {
        match self {
            Self::Resident(editor) => editor.resume_recovery_after_discard(),
            Self::Paged(editor) => editor.resume_recovery_after_discard(),
        }
    }
    /// Borrow the bounded viewport surface that both variants present and draw.
    /// This is the currently displayed text window — for a paged editor it is
    /// never the whole document, so use it only for presentation and per-view
    /// state, not for whole-document editing or queries.
    pub fn viewport(&self) -> &EditorSurface {
        match self {
            Self::Resident(e) => e,
            Self::Paged(e) => e.viewport(),
        }
    }
    /// Mutably borrow the bounded viewport surface. See [`Self::viewport`].
    pub fn viewport_mut(&mut self) -> &mut EditorSurface {
        match self {
            Self::Resident(e) => e,
            Self::Paged(e) => e.viewport_mut(),
        }
    }
    /// The snapshot of the bounded viewport window that is currently presented.
    pub fn snapshot(&self) -> &bareline_document::DocumentSnapshot {
        self.viewport().snapshot()
    }
    pub fn document_identity(&self) -> (u64, u64) {
        match self {
            Self::Resident(editor) => editor.snapshot().identity_token(),
            Self::Paged(editor) => editor.snapshot().identity_token(),
        }
    }
    /// Pending IME composition text on the presented viewport, if any.
    pub fn composition_text(&self) -> Option<&str> {
        self.viewport().composition_text()
    }
    /// Release cached shaped layouts for the presented viewport.
    pub fn release_layouts(&mut self, backend: &mut impl TextBackend) {
        self.viewport_mut().release_layouts(backend);
    }
    /// Route the external-scrollbar preference to the presented viewport.
    pub fn set_external_scrollbar(&mut self, external: bool) {
        self.viewport_mut().set_external_scrollbar(external);
    }
    /// Copy presentation state (theme, scroll, insets) from the presented
    /// viewport into another surface.
    pub fn copy_presentation_to(&self, view: &mut EditorSurface) {
        self.viewport().copy_presentation_to(view);
    }
    /// The selected text of a resident document. Reported as unsupported for a
    /// paged editor, whose selection may span text that is not resident.
    pub fn selected_text(&self) -> Result<String, String> {
        match self {
            Self::Resident(e) => e.selected_text().map_err(str::to_string),
            Self::Paged(_) => Err(NotSupportedForPaged::new("Copying the selection").into()),
        }
    }
    /// The active selection set. A resident editor reports its in-memory
    /// selections; a paged editor reports its whole-document (global) selection,
    /// which spans text outside the loaded viewport.
    pub fn selection_set(&self) -> bareline_editor_surface::power::SelectionSet {
        match self {
            Self::Resident(e) => e.selection_set(),
            Self::Paged(e) => e.global_selection_set(),
        }
    }
    /// Run a power (text-transform) command on a resident document. Paged
    /// documents route power through their own prepared/worker path, so a direct
    /// power command reports the operation as unsupported here.
    pub fn execute_power(&mut self, command: &str) -> Result<(), String> {
        match self {
            Self::Resident(e) => e.execute_power(command),
            Self::Paged(_) => Err(NotSupportedForPaged::new("This editing command").into()),
        }
    }
    /// Apply a prepared power edit to a resident document. Unsupported for a
    /// paged editor.
    pub fn apply_power(&mut self, prepared: bareline_editor_surface::power::PowerEdit) -> Result<(), String> {
        match self {
            Self::Resident(e) => e.apply_power(prepared),
            Self::Paged(_) => Err(NotSupportedForPaged::new("This editing command").into()),
        }
    }
    /// Replace the resident document's selection set. A paged editor manages its
    /// selection through the global paged path, so this reports as unsupported.
    pub fn set_selections(&mut self, selections: bareline_editor_surface::power::SelectionSet) -> Result<(), String> {
        match self {
            Self::Resident(e) => e.set_selections(selections),
            Self::Paged(_) => Err(NotSupportedForPaged::new("Setting the selection").into()),
        }
    }
    /// Rectangle-clipboard metadata for a resident selection. Unsupported for a
    /// paged editor.
    pub fn rectangle_clipboard_metadata(&self, text: &str) -> Result<Option<Vec<u8>>, String> {
        match self {
            Self::Resident(e) => e
                .rectangle_clipboard_metadata(text)
                .map_err(|error| format!("{error:?}")),
            Self::Paged(_) => Err(NotSupportedForPaged::new("Rectangle copy").into()),
        }
    }
    /// Accept worker-prepared reviewed-open edits. Only a resident editor can
    /// apply these; a paged editor reports the operation as unsupported.
    pub fn apply_prepared(
        &mut self,
        source: &bareline_document::DocumentSnapshot,
        transaction: bareline_document::EditTransaction,
    ) -> Result<(), String> {
        match self {
            Self::Resident(e) => e.apply_prepared(source, transaction).map_err(str::to_string),
            Self::Paged(_) => Err(NotSupportedForPaged::new("Replacing across open documents").into()),
        }
    }
    /// Attach a loaded document service to a resident preview. Paged documents
    /// are opened through their own path and never finish loading this way.
    pub fn finish_loading(
        &mut self,
        service: bareline_document::service::DocumentService,
        snapshot: bareline_document::DocumentSnapshot,
    ) {
        match self {
            Self::Resident(e) => e.finish_loading(service, snapshot),
            Self::Paged(_) => {}
        }
    }
    /// Record that a resident document was saved. Paged saves track their own
    /// state through the paged save path.
    pub fn mark_saved(&mut self, captured: &bareline_document::DocumentSnapshot) {
        match self {
            Self::Resident(e) => e.mark_saved(captured),
            Self::Paged(_) => {}
        }
    }
    /// The document actor service backing a resident editor, or `None` for a
    /// paged editor (which has no resident actor).
    pub fn document_service(&self) -> Option<bareline_document::service::DocumentService> {
        match self {
            Self::Resident(e) => e.document_service(),
            Self::Paged(_) => None,
        }
    }
    /// The saved content-state of a resident document, or `None` for a paged
    /// editor.
    pub fn saved_content_state(&self) -> Option<bareline_document::ContentStateId> {
        match self {
            Self::Resident(e) => Some(e.saved_content_state()),
            Self::Paged(_) => None,
        }
    }
    /// Migrate a clean resident document into paged storage. Unsupported for an
    /// already-paged editor.
    pub fn migrate_clean_spill(
        &self,
        captured: &bareline_document::DocumentSnapshot,
        source: bareline_document::source::MemorySource,
    ) -> Result<bareline_document::paged::PagedDocument, bareline_document::Error> {
        match self {
            Self::Resident(e) => e.migrate_clean_spill(captured, source),
            Self::Paged(_) => Err(bareline_document::Error::ActorBusy),
        }
    }
    /// Cancel an in-flight clean spill on a resident document. Unsupported for
    /// an already-paged editor.
    pub fn cancel_clean_spill(
        &self,
        captured: &bareline_document::DocumentSnapshot,
    ) -> Result<(), bareline_document::Error> {
        match self {
            Self::Resident(e) => e.cancel_clean_spill(captured),
            Self::Paged(_) => Err(bareline_document::Error::ActorBusy),
        }
    }
    // Presentation and per-view methods forwarded to the bounded viewport
    // surface that both variants draw. These act on the currently displayed
    // window, never on paged text outside it.
    pub fn scroll(&mut self, delta: f64, height: f32) {
        self.viewport_mut().scroll(delta, height);
    }
    pub fn zoom_by(&mut self, steps: f32) -> bool {
        self.viewport_mut().zoom_by(steps)
    }
    pub fn set_focused(&mut self, focused: bool) {
        self.viewport_mut().set_focused(focused);
    }
    pub fn tick_caret_blink(&mut self, now: std::time::Instant) -> bool {
        self.viewport_mut().tick_caret_blink(now)
    }
    pub fn blink_deadline(&self) -> Option<std::time::Instant> {
        self.viewport().blink_deadline()
    }
    pub fn reset_caret_blink(&mut self) {
        self.viewport_mut().reset_caret_blink();
    }
    pub fn take_ordered_receipts(&mut self) -> Vec<bareline_editor_surface::power::consumer::OrderedReceipt> {
        self.viewport_mut().take_ordered_receipts()
    }
    pub fn set_wrap(&mut self, wrap: bool) {
        self.viewport_mut().set_wrap(wrap);
    }
    pub fn set_font_family(&mut self, family: &str) -> Result<(), String> {
        self.viewport_mut().set_font_family(family)
    }
    pub fn font_family(&self) -> &str {
        self.viewport().font_family()
    }
    pub fn apply_visual_preferences(
        &mut self,
        font_size_pt: f64,
        tab_width: u8,
        line_numbers: bool,
        highlight_current_line: bool,
        whitespace: &str,
    ) {
        self.viewport_mut().apply_visual_preferences(
            font_size_pt,
            tab_width,
            line_numbers,
            highlight_current_line,
            whitespace,
        );
    }
    pub fn set_view_spacers(&mut self, rows: &[(u64, u64)]) -> Result<(), String> {
        self.viewport_mut().set_view_spacers(rows)
    }
    pub fn logical_scroll(&self) -> (u64, f64, f64) {
        self.viewport().logical_scroll()
    }
    pub fn set_logical_scroll(&mut self, line: u64, fraction: f64, x: f64) {
        self.viewport_mut().set_logical_scroll(line, fraction, x);
    }
    pub fn layout_range(
        &self,
        id: bareline_renderer::LayoutId,
    ) -> Option<std::ops::Range<bareline_document::TextOffset>> {
        self.viewport().layout_range(id)
    }
    pub fn accessibility_geometry(
        &self,
        backend: &impl TextBackend,
        width: f32,
        height: f32,
    ) -> Vec<(std::ops::Range<usize>, Rect)> {
        self.viewport().accessibility_geometry(backend, width, height)
    }
    pub fn power_hit_position(
        &self,
        backend: &impl TextBackend,
        point: bareline_renderer::Point,
    ) -> Option<(usize, usize, usize)> {
        self.viewport().power_hit_position(backend, point)
    }
    pub fn finish_column_measurement(&mut self) {
        self.viewport_mut().finish_column_measurement();
    }
    pub fn text_left(&self) -> f32 {
        self.viewport().text_left()
    }
    pub fn set_eol_status_override(&mut self, label: Option<String>) {
        self.viewport_mut().set_eol_status_override(label);
    }
    pub fn session_language_selection(&self) -> Option<bareline_file_io::session::LanguageSelection> {
        self.viewport().session_language_selection()
    }
    pub fn restore_session_language(&mut self, selection: Option<&bareline_file_io::session::LanguageSelection>) {
        self.viewport_mut().restore_session_language(selection);
    }
    pub fn persisted_folds(&self) -> Vec<std::ops::Range<u64>> {
        self.viewport().persisted_folds()
    }
    pub fn restore_folds(&mut self, ranges: &[std::ops::Range<u64>]) {
        self.viewport_mut().restore_folds(ranges);
    }
    pub fn unfold_all(&mut self) {
        self.viewport_mut().unfold_all();
    }
    pub fn toggle_current_fold(&mut self) {
        self.viewport_mut().toggle_current_fold();
    }
    pub fn preedit(&mut self, value: String, cursor: Option<(usize, usize)>) {
        self.viewport_mut().preedit(value, cursor);
    }
    pub fn set_search_marks(
        &mut self,
        style: u8,
        ranges: Vec<std::ops::Range<bareline_document::TextOffset>>,
    ) -> Result<(), String> {
        self.viewport_mut().set_search_marks(style, ranges)
    }
    pub fn clear_search_marks(&mut self, style: Option<u8>) {
        self.viewport_mut().clear_search_marks(style);
    }
    pub fn paged(&self) -> bool {
        matches!(self, Self::Paged(_))
    }
    /// Navigate a bounded text window without scanning the complete source.
    pub fn page_by(&mut self, forward: bool) -> bool {
        let Self::Paged(editor) = self else {
            return false;
        };
        let start = editor.viewport_start().0;
        let next = if forward {
            start.saturating_add(48 * 1024).min(editor.snapshot().len())
        } else {
            start.saturating_sub(48 * 1024)
        };
        if let Err(error) = editor.request_viewport(bareline_document::TextOffset(next)) {
            editor.error = Some(error);
        }
        true
    }
}

/// Paint a bounded pending viewport without presenting stale text or hit geometry.
pub fn paint_paged_pending(
    editor: &WorkspaceEditor,
    width: f32,
    height: f32,
    theme: bareline_ui::theme::UiTheme,
    ops: &mut Vec<bareline_renderer::DrawOp>,
) -> bool {
    let WorkspaceEditor::Paged(paged) = editor else {
        return false;
    };
    let frame = paged.paged_frame_state();
    if frame.ready {
        return false;
    }
    ops.push(bareline_renderer::DrawOp::Fill(
        bareline_ui::rect(
            0.0,
            bareline_ui::TAB_HEIGHT,
            width,
            (height - bareline_ui::TAB_HEIGHT).max(0.0),
        ),
        theme.editor,
    ));
    let hatch_top = bareline_ui::TAB_HEIGHT + 48.0;
    let region = bareline_ui::rect(
        0.0,
        hatch_top,
        width,
        (height - hatch_top - bareline_ui::STATUS_HEIGHT).max(0.0),
    );
    ops.push(bareline_renderer::DrawOp::PushClip(region));
    for column in 0..((width.max(0.0) / 24.0).ceil() as usize).min(1024) {
        let x = column as f32 * 24.0;
        ops.push(bareline_renderer::DrawOp::Line {
            from: bareline_renderer::Point { x, y: hatch_top },
            to: bareline_renderer::Point {
                x: x + region.height,
                y: hatch_top + region.height,
            },
            color: theme.border,
            width: 1.0,
        });
    }
    ops.push(bareline_renderer::DrawOp::PopClip);
    let label = paged.error.clone().unwrap_or_else(|| {
        frame.requested.map_or_else(
            || "Loading viewport…".to_owned(),
            |offset| format!("Loading viewport at byte {}…", offset.0),
        )
    });
    bareline_ui::text(ops, 16.0, bareline_ui::TAB_HEIGHT + 16.0, label, 13.0, theme.muted);
    true
}

pub struct Workspace {
    pub theme: bareline_ui::theme::UiTheme,
    pub editors: Vec<WorkspaceEditor>,
    scheduler: Scheduler,
    bytes: Budget,
    history: Budget,
    pub resident_max_bytes: u64,
    page_size_bytes: usize,
    page_cache_bytes: usize,
    undo_max_changes: usize,
    pub transcode_quota_bytes: u64,
    pub recovery_root: Option<PathBuf>,
    notify: Arc<dyn Fn() + Send + Sync>,
    last_drawn: Option<usize>,
    files: Vec<Option<FileState>>,
    untitled_labels: Vec<String>,
    next_untitled: u64,
    new_document_defaults: new_document::NewDocumentDefaults,
    recent: Vec<bareline_platform::SerializedPath>,
    file_system: Arc<dyn LocalFileSystem>,
    io: Option<IoService>,
    pending_io: Vec<PendingIo>,
    pending_paged_saves: std::collections::BTreeSet<bareline_editor_surface::paged_view::PagedSaveOwner>,
    open_outcomes: Vec<LaunchOpenOutcome>,
    next_recovery_restore_request: u64,
    recovery_restore_outcomes: Vec<RecoveryRestoreOutcome>,
    pending_recovery_restore_publications: Vec<PendingRecoveryRestorePublication>,
    pending_save_recovery: Vec<(PathBuf, IoTicket)>,
    pending_save_cleanup: Vec<(PathBuf, IoTicket)>,
    scanned_save_recovery: std::collections::BTreeSet<PathBuf>,
    failed_save_recovery: std::collections::BTreeSet<PathBuf>,
    replacement_registry: bareline_search::replace_disk::OpenFileRegistry,
    pub message: Option<String>,
    pub find: crate::find::FindController,
    pub search_panel: crate::search_panel::SearchPanel,
    pub search_focus: bool,
    /// Reserved by a platform shell that composes Search into a shared dock.
    pub bottom_panel_height: f32,
    pub external_search_panel: bool,
    pending_replace: Option<bareline_search::service::ReplaceTicket>,
    pending_paged_replace: Option<bareline_search::service::PagedReplaceTicket>,
    pending_search_navigation: Option<PendingSearchNavigation>,
    acknowledged_search_commands: Vec<bareline_editor_surface::power::consumer::OrderedReceipt>,
    styling: crate::styling::Styling,
    retired: Vec<WorkspaceEditor>,
    closed: Vec<ClosedDocument>,
    closed_documents: std::cell::RefCell<Vec<(u64, u64)>>,
    paused_transcode: Option<Box<bareline_file_io::lifecycle::PausedTranscode>>,
    paused_reload: Option<bareline_document::DocumentSnapshot>,
    eol_status: std::cell::RefCell<encoding::EolTracker>,
    encoding_failures: Vec<EncodingFailure>,
    save_conflicts: Vec<SaveConflict>,
    selected_save_conflict: Option<(PathBuf, (u64, u64))>,
    save_cleanups: Vec<SaveCleanup>,
    selected_save_cleanup: Option<PathBuf>,
    eol_job: Option<encoding::EolJob>,
    interpreting_paged: Option<(bareline_document::paged::PagedSnapshot, PathBuf)>,
    spill_pending: bool,
    promotion_target: Option<(u64, u64)>,
    spill_paused: bool,
    spill_selection: Option<(bareline_document::paged::PagedSnapshot, usize, usize, Option<u64>)>,
}
enum SearchNavigationSource {
    Resident(bareline_document::DocumentSnapshot),
    Paged(bareline_document::paged::PagedSnapshot),
}
struct PendingSearchNavigation {
    source: SearchNavigationSource,
    job: bareline_search::SearchJobId,
    query: bareline_search::SearchQuery,
    range: std::ops::Range<bareline_document::TextOffset>,
    backwards: bool,
}
struct FileState {
    binary_accepted: bool,
    _lease: Option<bareline_search::replace_disk::OpenFileLease>,
    path: PathBuf,
    fingerprint: Fingerprint,
    bom: bool,
    encoding: Option<bareline_file_io::codecs::resident::ResidentEncoding>,
}
struct PendingIo {
    completion: Option<IoCompletion>,
    receiver: IoTicket,
    save: Option<(usize, PathBuf, bool)>,
    copy_only: bool,
    open_path: Option<PathBuf>,
    launch_request: Option<u64>,
    recovery_restore_request: Option<u64>,
    allow_duplicate: bool,
    preview: Option<bareline_document::DocumentSnapshot>,
    reload: Option<bareline_document::DocumentSnapshot>,
}
struct PendingRecoveryRestorePublication {
    request_id: u64,
    document_id: u64,
    receipt: bareline_editor_surface::TrackedEditReceipt,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchOpenOutcome {
    Opened { request_id: u64, document: (u64, u64) },
    Failed { request_id: u64, error: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryRestoreOutcome {
    Restored { request_id: u64, document: (u64, u64) },
    Failed { request_id: u64, error: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseError {
    Busy,
    Unsaved,
    Missing,
    RecoveryPending,
    RecoveryFailed(String),
}
impl Workspace {
    fn new_paged_editor(
        &self,
        opened: Box<bareline_file_io::lifecycle::PagedOpened>,
    ) -> Result<PagedEditorSurface, String> {
        let recovery_root = self.recovery_root.clone().or_else(|| {
            opened
                .recovery_origin
                .as_deref()
                .and_then(std::path::Path::parent)
                .map(std::path::Path::to_path_buf)
        });
        let mut editor = PagedEditorSurface::new(opened, self.bytes.clone(), self.notify.clone())?;
        editor.configure_owned_spill(
            std::env::temp_dir().join("Bareline-owned-spill"),
            self.file_system.clone(),
            self.source_options(),
        );
        if let Some(root) = recovery_root {
            editor.enable_recovery(root, self.file_system.clone());
        }
        Ok(editor)
    }
    pub fn source_options(&self) -> bareline_file_io::source::SourceOptions {
        bareline_file_io::source::SourceOptions {
            resident_max_bytes: self.resident_max_bytes,
            page_size_bytes: self.page_size_bytes,
            page_cache_bytes: self.page_cache_bytes,
        }
    }
    pub fn apply_resource_settings(&mut self, settings: &bareline_settings::EffectiveSettings) {
        self.new_document_defaults
            .encoding
            .clone_from(&settings.default_encoding);
        self.new_document_defaults.eol.clone_from(&settings.default_eol);
        self.undo_max_changes = settings.undo_max_changes;
        self.resident_max_bytes = settings.resident_max_bytes;
        self.bytes.set_limit(settings.aggregate_cache_bytes);
        self.history.set_limit(settings.undo_aggregate_ram_bytes);
        self.page_cache_bytes = settings.page_cache_bytes.min(settings.aggregate_cache_bytes);
        self.page_size_bytes = settings.page_size_bytes.min(self.page_cache_bytes);
        let options = self.source_options();
        for editor in &mut self.editors {
            if let WorkspaceEditor::Paged(paged) = editor {
                paged.configure_owned_spill(
                    std::env::temp_dir().join("Bareline-owned-spill"),
                    self.file_system.clone(),
                    options,
                );
            }
        }
    }
    pub fn apply_reviewed_open(
        &mut self,
        mut prepared: Vec<(bareline_document::DocumentSnapshot, bareline_document::EditTransaction)>,
    ) -> Result<bareline_editor_surface::group_view::SurfaceGroup, String> {
        let mut views = Vec::new();
        let mut ordered = Vec::new();
        for editor in &mut self.editors {
            if let WorkspaceEditor::Resident(editor) = editor
                && let Some(index) = prepared
                    .iter()
                    .position(|(snapshot, _)| snapshot.same_document(editor.snapshot()))
            {
                let (snapshot, transaction) = prepared.remove(index);
                ordered.push((
                    snapshot,
                    bareline_editor_surface::power::PowerEdit {
                        transaction,
                        selections: bareline_editor_surface::Selection::default().into(),
                    },
                ));
                views.push(editor);
            }
        }
        if !prepared.is_empty() || views.is_empty() {
            return Err("A replacement document closed or changed".into());
        }
        bareline_editor_surface::group_view::SurfaceGroup::apply(&self.scheduler, &mut views, ordered)
    }
    pub fn pump_reviewed_open(
        &mut self,
        group: &mut bareline_editor_surface::group_view::SurfaceGroup,
        sources: &[bareline_document::DocumentSnapshot],
    ) -> Result<Option<bareline_document::group::UndoGroup>, String> {
        let mut views: Vec<_> = self
            .editors
            .iter_mut()
            .filter_map(|editor| match editor {
                WorkspaceEditor::Resident(editor)
                    if sources.iter().any(|source| source.same_document(editor.snapshot())) =>
                {
                    Some(editor)
                }
                _ => None,
            })
            .collect();
        group.pump(&mut views)
    }

    pub fn replacement_registry(&self) -> bareline_search::replace_disk::OpenFileRegistry {
        self.replacement_registry.clone()
    }

    /// Created lazily on the first document command, after the initial frame.
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>, file_system: Arc<dyn LocalFileSystem>) -> std::io::Result<Self> {
        Ok(Self {
            theme: bareline_ui::theme::UiTheme::default(),
            editors: Vec::new(),
            scheduler: Scheduler::new(2, 256)?,
            bytes: Budget::new(256 << 20),
            history: Budget::new(128 << 20),
            resident_max_bytes: 256 << 20,
            page_size_bytes: 1 << 20,
            page_cache_bytes: 64 << 20,
            undo_max_changes: 100_000,
            transcode_quota_bytes: 20u64 << 30,
            recovery_root: None,
            notify,
            last_drawn: None,
            files: Vec::new(),
            untitled_labels: Vec::new(),
            next_untitled: 1,
            new_document_defaults: Default::default(),
            recent: Vec::new(),
            file_system,
            io: None,
            pending_io: Vec::new(),
            pending_paged_saves: std::collections::BTreeSet::new(),
            open_outcomes: Vec::new(),
            next_recovery_restore_request: 1,
            recovery_restore_outcomes: Vec::new(),
            pending_recovery_restore_publications: Vec::new(),
            pending_save_recovery: Vec::new(),
            pending_save_cleanup: Vec::new(),
            scanned_save_recovery: std::collections::BTreeSet::new(),
            failed_save_recovery: std::collections::BTreeSet::new(),
            replacement_registry: Default::default(),
            message: None,
            find: crate::find::FindController::default(),
            search_panel: Default::default(),
            search_focus: false,
            bottom_panel_height: 0.0,
            external_search_panel: false,
            pending_replace: None,
            pending_paged_replace: None,
            pending_search_navigation: None,
            acknowledged_search_commands: Vec::new(),
            styling: crate::styling::Styling::default(),
            retired: Vec::new(),
            closed: Vec::new(),
            closed_documents: std::cell::RefCell::new(Vec::new()),
            paused_transcode: None,
            paused_reload: None,
            eol_status: Default::default(),
            encoding_failures: Vec::new(),
            save_conflicts: Vec::new(),
            selected_save_conflict: None,
            save_cleanups: Vec::new(),
            selected_save_cleanup: None,
            eol_job: None,
            interpreting_paged: None,
            spill_pending: false,
            promotion_target: None,
            spill_paused: false,
            spill_selection: None,
        })
    }
    pub fn new_document(&mut self) -> Result<(), String> {
        let metadata = self.new_document_defaults.metadata().inspect_err(|error| {
            self.message = Some(error.clone());
        })?;
        let mut document = Document::from_utf8("", self.bytes.clone(), self.history.clone())
            .map_err(|error| format!("new document: {error:?}"))?;
        document
            .initialize_metadata(metadata)
            .map_err(|error| format!("new document policy: {error:?}"))?;
        let snapshot = document.snapshot();
        self.editors
            .push(EditorSurface::new(self.scheduler.document(document, 32), snapshot, self.notify.clone()).into());
        self.files.push(None);
        self.untitled_labels.push(format!("Untitled {}", self.next_untitled));
        self.next_untitled += 1;
        Ok(())
    }
    /// Adopt an immutable compare/recovery view without copying the resident text.
    /// Its fresh document identity prevents source/target aliases during hunk apply.
    pub fn add_snapshot_preview(
        &mut self,
        snapshot: &bareline_document::DocumentSnapshot,
        label: String,
    ) -> Result<usize, bareline_document::Error> {
        let document = Document::fork_from_snapshot(snapshot, self.bytes.clone(), self.history.clone())?;
        let mut editor = EditorSurface::loading(document.snapshot(), self.notify.clone());
        editor.user_read_only = true;
        let index = self.editors.len();
        self.editors.push(editor.into());
        self.files.push(None);
        self.untitled_labels.push(label.chars().take(4096).collect());
        Ok(index)
    }
    /// A historical read-only pane owns an immutable captured paged root. It has
    /// no file target and cannot participate in Save or implicit recovery writes.
    pub fn add_paged_snapshot_preview(
        &mut self,
        source_index: usize,
        captured: &bareline_editor_surface::paged_view::PagedReadHandle,
        label: String,
    ) -> Result<usize, String> {
        let Some(WorkspaceEditor::Paged(source)) = self.editors.get(source_index) else {
            return Err("Paged source is no longer open".into());
        };
        let preview = source.clone_captured_view(captured)?;
        let index = self.editors.len();
        self.editors.push(WorkspaceEditor::Paged(preview));
        self.files.push(None);
        self.untitled_labels.push(label.chars().take(4096).collect());
        Ok(index)
    }
    pub fn pump(&mut self) -> bool {
        let mut changed = self.find.pump();
        let mut cleanup = 0;
        while cleanup < self.pending_save_cleanup.len() {
            match self.pending_save_cleanup[cleanup].1.try_recv() {
                Err(TryRecvError::Empty) => cleanup += 1,
                received => {
                    let (transaction, _) = self.pending_save_cleanup.remove(cleanup);
                    changed = true;
                    match received {
                        Ok(IoCompletion::SaveCleanupRetried { cleanup, result: Ok(true) }) => {
                            self.save_cleanups.retain(|known| known.transaction != transaction);
                            if self.selected_save_cleanup.as_ref() == Some(&transaction) {
                                self.selected_save_cleanup = self
                                    .save_cleanups
                                    .first()
                                    .map(|cleanup| cleanup.transaction.clone());
                            }
                            self.message = Some("Saved recovery files were cleaned up.".into());
                            drop(cleanup);
                        }
                        Ok(IoCompletion::SaveCleanupRetried { cleanup, result }) => {
                            if !self.save_cleanups.iter().any(|known| known.transaction == transaction) {
                                self.record_save_cleanup(cleanup);
                            }
                            self.message = Some(match result {
                                Ok(false) => "Save cleanup was already completed.".into(),
                                Err(error) => format!("Saved document remains committed; recovery cleanup failed: {}", file_error(error)),
                                Ok(true) => unreachable!(),
                            });
                        }
                        _ => self.message = Some("Saved document remains committed; recovery cleanup stopped. Retry cleanup from File commands.".into()),
                    }
                }
            }
        }
        let mut recovery = 0;
        while recovery < self.pending_save_recovery.len() {
            match self.pending_save_recovery[recovery].1.try_recv() {
                Err(TryRecvError::Empty) => recovery += 1,
                received => {
                    let (parent, _) = self.pending_save_recovery.remove(recovery);
                    changed = true;
                    match received {
                        Ok(IoCompletion::SaveRecoveryInspection { result: Ok(found), .. }) => {
                            self.failed_save_recovery.remove(&parent);
                            let count = self.merge_save_recovery(found.conflicts);
                            let cleanups = found.cleanups.len();
                            for cleanup in found.cleanups {
                                self.record_save_cleanup(cleanup);
                            }
                            if count != 0 {
                                self.message = Some(format!(
                                    "{count} interrupted save transaction(s) are available for compare, Save Elsewhere, or retention."
                                ));
                            } else if cleanups != 0 {
                                self.message = Some(format!(
                                    "{cleanups} interrupted cleanup transaction(s) are ready to retry."
                                ));
                            }
                        }
                        Ok(IoCompletion::SaveRecoveryInspection { result: Err(error), .. }) => {
                            self.scanned_save_recovery.remove(&parent);
                            self.failed_save_recovery.insert(parent.clone());
                            self.message = Some(format!(
                                "Save recovery discovery failed for {}: {}. Retry recovery discovery from File commands.",
                                parent.display(),
                                file_error(error)
                            ));
                        }
                        _ => {
                            self.scanned_save_recovery.remove(&parent);
                            self.failed_save_recovery.insert(parent.clone());
                            self.message = Some(format!(
                                "Save recovery discovery stopped for {}. Retry recovery discovery from File commands.",
                                parent.display()
                            ));
                        }
                    }
                }
            }
        }
        if let Some(warning) = bareline_file_io::recovery_retirement::take_cleanup_warning(&self.notify) {
            self.message = Some(format!("{warning}. Use Retry Recovery to try cleanup again."));
            changed = true;
        }
        changed |= self.search_panel.pump();
        changed |= self.styling.pump();
        for (index, editor) in self.editors.iter_mut().enumerate() {
            let policy_ready = match editor {
                WorkspaceEditor::Resident(surface) => surface
                    .document_service()
                    .is_none_or(|service| service.configure_history_limit(self.undo_max_changes)),
                WorkspaceEditor::Paged(surface) => surface.configure_history_limit(self.undo_max_changes),
            };
            if !policy_ready {
                (self.notify)();
            }
            if let Some(root) = &self.recovery_root {
                match editor {
                    WorkspaceEditor::Resident(surface) => surface.enable_recovery(
                        root.clone(),
                        self.file_system.clone(),
                        self.files[index].as_ref().and_then(|file| file.encoding.clone()),
                        self.files[index].as_ref().map(|file| file.path.clone()),
                        self.bytes.clone(),
                    ),
                    WorkspaceEditor::Paged(surface) => surface.enable_recovery(root.clone(), self.file_system.clone()),
                }
            }
            if let WorkspaceEditor::Paged(paged) = editor {
                paged.set_streaming_quota(self.transcode_quota_bytes);
            }
            changed |= editor.pump();
            if let WorkspaceEditor::Paged(paged) = editor {
                while let Some(terminal) = paged.take_save_terminal() {
                    let owned = terminal.receipt.requested.document == terminal.owner.document.0
                        && self.pending_paged_saves.remove(&terminal.owner);
                    if !owned {
                        continue;
                    }
                    changed = true;
                    match terminal.receipt.terminal {
                        Ok(bareline_file_io::paged_service::PagedTerminalOutcome::Saved { .. }) => {
                            let resident_save_pending = self.pending_io.iter().any(|pending| pending.save.is_some());
                            if self.pending_paged_saves.is_empty()
                                && !resident_save_pending
                                && self.message.as_deref() == Some("Saving…")
                            {
                                self.message = None;
                            }
                        }
                        Ok(_) => {}
                        Err(error) => self.message = Some(error.to_string()),
                    }
                }
                if let Some(error) = &paged.error {
                    self.message = Some(error.clone());
                }
                if let Some(conflict) = paged.take_save_conflict() {
                    self.save_conflicts.push(conflict);
                }
                if let Some(cleanup) = paged.take_save_cleanup() {
                    self.message = Some(format!(
                        "Saved, but recovery-file cleanup is pending: {}",
                        cleanup.error
                    ));
                    self.selected_save_cleanup = Some(cleanup.transaction.clone());
                    self.save_cleanups.push(cleanup);
                }
            }
        }
        let active_paged_saves: std::collections::BTreeSet<_> = self
            .editors
            .iter()
            .filter_map(|editor| match editor {
                WorkspaceEditor::Paged(paged) => paged.pending_save_owner(),
                WorkspaceEditor::Resident(_) => None,
            })
            .collect();
        self.pending_paged_saves
            .retain(|owner| active_paged_saves.contains(owner));
        let mut publication = 0;
        while publication < self.pending_recovery_restore_publications.len() {
            let terminal = self.pending_recovery_restore_publications[publication]
                .receipt
                .terminal();
            let Some(terminal) = terminal else {
                publication += 1;
                continue;
            };
            let pending = self.pending_recovery_restore_publications.remove(publication);
            match terminal {
                Ok(revision)
                    if self
                        .editors
                        .iter()
                        .any(|editor| editor.document_identity().0 == pending.document_id) =>
                {
                    self.record_recovery_restore(Some(pending.request_id), Ok((pending.document_id, revision.0)));
                }
                Ok(_) => self.record_recovery_restore(
                    Some(pending.request_id),
                    Err("Recovered document closed before its text was published.".into()),
                ),
                Err(error) => self.record_recovery_restore(Some(pending.request_id), Err(error)),
            }
            changed = true;
        }
        changed |= self.pump_encoding();
        changed |= self.pump_search_acknowledgment();
        let mut saved_paths = Vec::new();
        for (index, editor) in self.editors.iter().enumerate() {
            if let WorkspaceEditor::Paged(paged) = editor
                && let Some(file) = &mut self.files[index]
                && (file.path != paged.path() || file.fingerprint != paged.fingerprint())
            {
                let path = paged.path();
                let fingerprint = paged.fingerprint();
                match self
                    .replacement_registry
                    .try_register(path.clone(), &fingerprint.identity)
                {
                    Ok(lease) => {
                        if !paged.save_as_required() {
                            saved_paths.push(path.clone());
                        }
                        file._lease = Some(lease);
                        file.path = path;
                        file.fingerprint = fingerprint;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => self.message = Some(format!("File admission update failed: {error}")),
                }
            }
        }
        for path in saved_paths {
            self.note_recent(path);
        }
        if let Some(ticket) = &self.pending_paged_replace {
            match ticket.try_recv() {
                Err(TryRecvError::Empty) => {}
                received => {
                    let cancelled = ticket.job.is_cancelled();
                    self.pending_paged_replace = None;
                    changed = true;
                    self.message = Some(match received {
                        Ok(Ok(prepared)) if !cancelled => {
                            let target = self.editors.iter_mut().find_map(|editor| match editor {
                                WorkspaceEditor::Paged(paged) if paged.snapshot().same_document(&prepared.source) => {
                                    Some(paged)
                                }
                                _ => None,
                            });
                            match target {
                                Some(paged) => match paged.apply_prepared(&prepared.source, prepared.transaction) {
                                    Ok(()) => "Applying paged replacements…".into(),
                                    Err(error) => error,
                                },
                                None => "Document closed; replacement not applied".into(),
                            }
                        }
                        Ok(Err(error)) => format!("Replacement not applied: {error:?}"),
                        _ => "Replacement cancelled".into(),
                    });
                }
            }
        }
        if let Some(ticket) = &self.pending_replace {
            match ticket.try_recv() {
                Err(TryRecvError::Empty) => {}
                received => {
                    let cancelled = ticket.job.is_cancelled();
                    self.pending_replace = None;
                    changed = true;
                    self.message = Some(match received {
                        Ok(Ok(prepared)) if !cancelled => {
                            let count = prepared.transaction.edits.len();
                            match self
                                .editors
                                .iter_mut()
                                .find(|editor| editor.snapshot().same_document(&prepared.source))
                            {
                                Some(editor) => match editor.apply_prepared(&prepared.source, prepared.transaction) {
                                    Ok(()) => format!("Applying {count} replacements…"),
                                    Err(error) => error.into(),
                                },
                                None => "Document was closed; replacement was not applied.".into(),
                            }
                        }
                        Ok(Err(error)) => format!("Replacement was not applied: {error:?}"),
                        _ => "Replacement cancelled.".into(),
                    });
                }
            }
        }
        if self
            .message
            .as_deref()
            .is_some_and(|message| message.starts_with("Applying "))
            && !self.editors.iter().any(|editor| editor.busy())
        {
            self.message = Some("Replacement complete.".into());
        }
        let mut i = 0;
        while i < self.pending_io.len() {
            if let Ok(prefix) = self.pending_io[i].receiver.try_prefix()
                && self.pending_io[i].reload.is_none()
            {
                let label = self.pending_io[i]
                    .open_path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map_or_else(|| "File".into(), |name| name.to_string_lossy().into_owned());
                self.editors
                    .push(EditorSurface::loading(prefix.clone(), self.notify.clone()).into());
                self.files.push(None);
                self.untitled_labels.push(format!("{label} (loading)"));
                self.pending_io[i].preview = Some(prefix);
                changed = true;
            }
            let received = if let Some(completion) = self.pending_io[i].completion.take() {
                Ok(completion)
            } else {
                self.pending_io[i].receiver.try_recv()
            };
            let result = match received {
                Ok(result) => result,
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(TryRecvError::Disconnected) => {
                    let error = "File worker stopped.".to_string();
                    self.message = Some(error.clone());
                    if self.spill_pending {
                        self.spill_pending = false;
                        self.spill_paused = true;
                    }
                    let failed = self.pending_io.remove(i);
                    self.discard_preview(failed.preview.as_ref());
                    self.record_launch_open(failed.launch_request, Err(error));
                    self.record_recovery_restore(
                        failed.recovery_restore_request,
                        Err("Recovery restore worker stopped.".into()),
                    );
                    changed = true;
                    continue;
                }
            };
            let target = match &result {
                IoCompletion::Open(Ok(opened)) => Some((opened.path.clone(), opened.fingerprint.identity.clone())),
                IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Complete(opened)) => {
                    Some((opened.path.clone(), opened.fingerprint.identity.clone()))
                }
                IoCompletion::Save(Ok(saved)) if !self.pending_io[i].copy_only => self.pending_io[i]
                    .save
                    .as_ref()
                    .map(|(_, path, _)| (path.clone(), saved.fingerprint.identity.clone())),
                _ => None,
            };
            let mut admission = if self.pending_io[i].allow_duplicate {
                None
            } else if let Some((path, identity)) = target {
                match self.replacement_registry.try_register(path, &identity) {
                    Ok(lease) => Some(lease),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        self.pending_io[i].completion = Some(result);
                        i += 1;
                        continue;
                    }
                    Err(error) => {
                        let pending = self.pending_io.remove(i);
                        self.discard_preview(pending.preview.as_ref());
                        let error = format!("File admission failed: {error}");
                        self.message = Some(error.clone());
                        self.record_launch_open(pending.launch_request, Err(error.clone()));
                        self.record_recovery_restore(pending.recovery_restore_request, Err(error));
                        changed = true;
                        continue;
                    }
                }
            } else {
                None
            };
            let pending = self.pending_io.remove(i);
            let launch_request = pending.launch_request;
            let recovery_restore_request = pending.recovery_restore_request;
            changed = true;
            match result {
                IoCompletion::Open(Ok(opened)) => {
                    self.note_recent(opened.path.clone());
                    if let Some(captured) = &pending.reload {
                        let current = self
                            .editors
                            .iter()
                            .position(|editor| editor.snapshot().same_document(captured));
                        if let Some(index) = current
                            && self.editors[index].snapshot().revision == captured.revision
                            && !self.editors[index].busy()
                        {
                            let snapshot = opened.document.snapshot();
                            self.editors[index].finish_loading(self.scheduler.document(opened.document, 32), snapshot);
                            self.editors[index].enqueue(Input::SetCaret(0, false));
                            self.files[index] = Some(FileState {
                                binary_accepted: false,
                                _lease: admission.take(),
                                path: opened.path,
                                fingerprint: opened.fingerprint,
                                bom: opened.bom,
                                encoding: opened.encoding,
                            });
                            self.refresh_encoding_open(index);
                            self.find.clear_source();
                            self.message = Some("Reloaded from disk.".into());
                        } else {
                            self.message =
                                Some("Document changed while reloading; current edits were preserved.".into());
                        }
                        continue;
                    }
                    if !pending.allow_duplicate
                        && let Some(existing) = self.files.iter().position(|f| {
                            f.as_ref().is_some_and(|f| {
                                f.fingerprint.identity.volume == opened.fingerprint.identity.volume
                                    && f.fingerprint.identity.file == opened.fingerprint.identity.file
                            })
                        })
                    {
                        self.message = Some(format!("File is already open in tab {}.", existing + 1));
                        self.discard_preview(pending.preview.as_ref());
                        let document = self.editors[existing].document_identity();
                        self.record_launch_open(launch_request, Ok(document));
                        continue;
                    }
                    let snapshot = opened.document.snapshot();
                    let file = Some(FileState {
                        binary_accepted: false,
                        _lease: admission.take(),
                        path: opened.path,
                        fingerprint: opened.fingerprint,
                        bom: opened.bom,
                        encoding: opened.encoding,
                    });
                    let preview = pending.preview.as_ref().and_then(|source| {
                        self.editors
                            .iter()
                            .position(|editor| editor.snapshot().same_document(source))
                    });
                    if let Some(index) = preview {
                        self.editors[index].finish_loading(self.scheduler.document(opened.document, 32), snapshot);
                        self.files[index] = file;
                        self.untitled_labels[index].clear();
                    } else {
                        self.editors.push(
                            EditorSurface::new(
                                self.scheduler.document(opened.document, 32),
                                snapshot,
                                self.notify.clone(),
                            )
                            .into(),
                        );
                        self.files.push(file);
                        self.untitled_labels.push(String::new());
                    }
                    let index = preview.unwrap_or(self.editors.len() - 1);
                    self.refresh_encoding_open(index);
                    self.message = None;
                    let document = self.editors[index].document_identity();
                    self.record_launch_open(launch_request, Ok(document));
                }
                IoCompletion::Save(Ok(saved)) => {
                    let cleanup = saved.cleanup.clone();
                    if !pending.copy_only
                        && let Some((index, path, bom)) = pending.save
                        && let Some(editor) = self.editors.get_mut(index)
                    {
                        editor.mark_saved(&saved.captured);
                        self.note_recent(path.clone());
                        self.files[index] = Some(FileState {
                            binary_accepted: true,
                            _lease: admission.take(),
                            path,
                            fingerprint: saved.fingerprint,
                            bom,
                            encoding: self.files[index].as_ref().and_then(|file| file.encoding.clone()),
                        });
                    }
                    if let Some(cleanup) = cleanup {
                        self.message = Some(format!(
                            "Saved, but recovery-file cleanup is pending: {}",
                            cleanup.error
                        ));
                        self.record_save_cleanup(cleanup);
                    } else {
                        self.message = None;
                    }
                }
                IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Complete(mut opened)) => {
                    if self
                        .interpreting_paged
                        .as_ref()
                        .is_some_and(|(_, path)| *path == opened.path)
                    {
                        let (captured, _) = self.interpreting_paged.take().unwrap();
                        let index = self.editors.iter().position(|editor| matches!(editor, WorkspaceEditor::Paged(editor) if editor.snapshot().same_document(&captured) && editor.snapshot().revision == captured.revision));
                        if let Some(index) = index {
                            let file = FileState {
                                binary_accepted: false,
                                _lease: admission.take(),
                                path: opened.path.clone(),
                                fingerprint: opened.fingerprint.clone(),
                                bom: opened.transcoded.store.state.bom,
                                encoding: None,
                            };
                            match self.new_paged_editor(opened) {
                                Ok(mut editor) => {
                                    if let Some(root) = &self.recovery_root {
                                        editor.enable_recovery(root.clone(), self.file_system.clone());
                                    }
                                    let old =
                                        std::mem::replace(&mut self.editors[index], WorkspaceEditor::Paged(editor));
                                    self.retired.push(old);
                                    self.files[index] = Some(file);
                                    self.find.clear_source();
                                    self.message = Some("Original bytes reinterpreted.".into());
                                }
                                Err(error) => self.message = Some(error),
                            }
                        } else {
                            self.message = Some("Document changed while interpreting; current edits retained.".into());
                        }
                        continue;
                    }
                    if let Some(captured) = &pending.reload {
                        let current = self
                            .editors
                            .iter()
                            .position(|editor| editor.snapshot().same_document(captured));
                        if let Some(index) = current
                            && self.editors[index].snapshot().revision == captured.revision
                            && !self.editors[index].busy()
                        {
                            let read_only = self.editors[index].viewport().user_read_only;
                            let file = FileState {
                                binary_accepted: false,
                                _lease: admission.take(),
                                path: opened.path.clone(),
                                fingerprint: opened.fingerprint.clone(),
                                bom: opened.transcoded.store.state.bom,
                                encoding: None,
                            };
                            match self.new_paged_editor(opened) {
                                Ok(mut editor) => {
                                    self.editors[index].copy_presentation_to(editor.viewport_mut());
                                    editor.set_user_read_only(read_only);
                                    if let Some(root) = &self.recovery_root {
                                        editor.enable_recovery(root.clone(), self.file_system.clone());
                                    }
                                    let old =
                                        std::mem::replace(&mut self.editors[index], WorkspaceEditor::Paged(editor));
                                    self.retired.push(old);
                                    self.files[index] = Some(file);
                                    self.refresh_encoding_open(index);
                                    self.find.clear_source();
                                    self.message = Some("Reloaded from disk.".into());
                                }
                                Err(error) => self.message = Some(error),
                            }
                        } else {
                            self.message =
                                Some("Document changed while reloading; current edits were preserved.".into());
                        }
                        continue;
                    }
                    if opened.recovery_origin.is_none() {
                        self.note_recent(opened.path.clone());
                    }
                    if opened.recovery_origin.is_some() {
                        match self.adopt_recovered_resident(&mut opened) {
                            Ok(Some((document_id, receipt))) => {
                                self.discard_preview(pending.preview.as_ref());
                                if let Some(request_id) = recovery_restore_request {
                                    self.pending_recovery_restore_publications.push(
                                        PendingRecoveryRestorePublication {
                                            request_id,
                                            document_id,
                                            receipt,
                                        },
                                    );
                                }
                                continue;
                            }
                            Err(error) => {
                                self.discard_preview(pending.preview.as_ref());
                                self.message = Some(error.clone());
                                self.record_recovery_restore(recovery_restore_request, Err(error));
                                continue;
                            }
                            Ok(None) => {}
                        }
                    }
                    self.discard_preview(pending.preview.as_ref());
                    let file = FileState {
                        binary_accepted: false,
                        _lease: admission.take(),
                        path: opened.path.clone(),
                        fingerprint: opened.fingerprint.clone(),
                        bom: opened.transcoded.store.state.bom,
                        encoding: None,
                    };
                    match self.new_paged_editor(opened) {
                        Ok(mut editor) => {
                            if let Some(root) = &self.recovery_root {
                                editor.enable_recovery(root.clone(), self.file_system.clone());
                            }
                            self.editors.push(WorkspaceEditor::Paged(editor));
                            self.files.push(Some(file));
                            self.untitled_labels.push(String::new());
                            self.message = None;
                            let document = self.editors.last().unwrap().document_identity();
                            self.record_launch_open(launch_request, Ok(document));
                            self.record_recovery_restore(recovery_restore_request, Ok(document));
                        }
                        Err(error) => {
                            self.message = Some(error.clone());
                            self.record_launch_open(launch_request, Err(error.clone()));
                            self.record_recovery_restore(recovery_restore_request, Err(error));
                        }
                    }
                }
                IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Paused(paused)) => {
                    self.discard_preview(pending.preview.as_ref());
                    let error = format!(
                        "Transcode quota reached: {:?}. Resume after increasing the quota.",
                        paused.error
                    );
                    self.message = Some(error.clone());
                    self.record_launch_open(launch_request, Err(error.clone()));
                    self.record_recovery_restore(recovery_restore_request, Err(error));
                    self.paused_reload = pending.reload.clone();
                    self.paused_transcode = Some(paused);
                }
                IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Failed(error)) => {
                    self.interpreting_paged = None;
                    self.discard_preview(pending.preview.as_ref());
                    let error = file_error(error);
                    self.message = Some(error.clone());
                    self.record_launch_open(launch_request, Err(error.clone()));
                    self.record_recovery_restore(recovery_restore_request, Err(error));
                }
                IoCompletion::ResidentSpilled { captured, result } => {
                    self.spill_pending = false;
                    match result {
                        Err(error) => {
                            self.spill_paused = true;
                            self.message =
                                Some(format!("Memory spill paused; document retained: {}", file_error(error)));
                        }
                        Ok((mut transcoded, prepared)) => {
                            let preserving_history = prepared.is_some();
                            let index = self.editors.iter().position(|editor| {
                                matches!(editor, WorkspaceEditor::Resident(_))
                                    && editor.snapshot().same_document(&captured)
                            });
                            if let Some(index) = index
                                && !self.document_busy(index)
                                && self.editors[index].snapshot().revision == captured.revision
                                && self
                                    .editors
                                    .iter()
                                    .filter(|editor| editor.snapshot().same_document(&captured))
                                    .count()
                                    == 1
                                && (preserving_history
                                    || self.files[index].as_ref().is_some_and(|file| {
                                        transcoded.store.fingerprint.sha256 == file.fingerprint.sha256
                                    }))
                            {
                                let selection = self.editors[index].viewport().selection;
                                let migration = match prepared {
                                    Some(prepared) => self.editors[index]
                                        .document_service()
                                        .ok_or(bareline_document::Error::ActorBusy)
                                        .and_then(|service| service.migrate_spill(prepared)),
                                    None => {
                                        self.editors[index].migrate_clean_spill(&captured, transcoded.source.source())
                                    }
                                };
                                match migration {
                                    Err(error) => {
                                        self.spill_paused = true;
                                        self.message = Some(format!("Memory spill could not attach: {error:?}"));
                                    }
                                    Ok(document) => {
                                        transcoded.document = document;
                                        let opened = Box::new(bareline_file_io::lifecycle::PagedOpened {
                                            recovery_origin: None,
                                            recovered_resident: None,
                                            path: self.files[index]
                                                .as_ref()
                                                .map_or_else(|| PathBuf::from("Untitled"), |file| file.path.clone()),
                                            fingerprint: self.files[index].as_ref().map_or_else(
                                                || transcoded.store.fingerprint.clone(),
                                                |file| file.fingerprint.clone(),
                                            ),
                                            transcoded,
                                        });
                                        match self.new_paged_editor(opened) {
                                            Err(error) => {
                                                let _ = self.editors[index].cancel_clean_spill(&captured);
                                                self.spill_paused = true;
                                                self.message = Some(error);
                                            }
                                            Ok(mut paged) => {
                                                if self.files[index].is_none() {
                                                    paged.require_save_as();
                                                }
                                                paged.set_streaming_quota(self.transcode_quota_bytes);
                                                self.editors[index].copy_presentation_to(paged.viewport_mut());
                                                if let Some(root) = &self.recovery_root {
                                                    paged.enable_recovery(root.clone(), self.file_system.clone());
                                                }
                                                self.spill_selection = Some((
                                                    paged.snapshot().clone(),
                                                    selection.anchor,
                                                    selection.caret,
                                                    None,
                                                ));
                                                let old = std::mem::replace(
                                                    &mut self.editors[index],
                                                    WorkspaceEditor::Paged(paged),
                                                );
                                                self.retired.push(old);
                                                if let Some(file) = self.files[index].as_mut() {
                                                    file.encoding = None;
                                                }
                                                self.refresh_encoding_open(index);
                                                self.find.clear_source();
                                                self.last_drawn = None;
                                                // The move to large-file storage is an internal
                                                // detail; clear the transient status instead of
                                                // showing a banner about it (UX-04/UX-60).
                                                self.message = None;
                                            }
                                        }
                                    }
                                }
                            } else {
                                self.spill_paused = true;
                                self.message = Some("Memory spill was not attached because the document, original bytes, or view ownership changed; current data was retained.".into());
                            }
                        }
                    }
                }
                IoCompletion::Open(Err(FileError::StreamingRequired)) => {
                    self.discard_preview(pending.preview.as_ref());
                    if let Some(path) = pending.open_path {
                        self.open_paged(path, launch_request, pending.allow_duplicate);
                    }
                }
                IoCompletion::Open(Err(error)) | IoCompletion::Save(Err(error)) => {
                    if let FileError::EncodingAt(failure) = &error {
                        if self.encoding_failures.len() == 32 {
                            self.encoding_failures.remove(0);
                        }
                        self.encoding_failures.push(failure.clone());
                    }
                    if let Some(conflict) = error.save_conflict() {
                        self.record_save_conflict(conflict);
                    }
                    self.discard_preview(pending.preview.as_ref());
                    let error = file_error(error);
                    self.message = Some(error.clone());
                    self.record_launch_open(launch_request, Err(error));
                }
                IoCompletion::SaveRecoveryInspection { result, .. } => {
                    if let Ok(found) = result {
                        self.merge_save_recovery(found.conflicts);
                        for cleanup in found.cleanups {
                            self.record_save_cleanup(cleanup);
                        }
                    }
                }
                IoCompletion::SaveCleanupRetried { .. } => {}
            }
        }
        if let Some((snapshot, anchor, caret, token)) = self.spill_selection.take() {
            if let Some(WorkspaceEditor::Paged(editor)) = self.editors.iter_mut().find(
                |editor| matches!(editor, WorkspaceEditor::Paged(paged) if paged.snapshot().same_document(&snapshot)),
            ) {
                if editor.snapshot().content_state != snapshot.content_state {
                    self.message = Some("Document changed before its promoted selection was restored.".into());
                } else if editor.busy() {
                    self.spill_selection = Some((snapshot, anchor, caret, token));
                } else if let Some(token) = token {
                    use bareline_editor_surface::paged_view::SelectionRestoreStatus;
                    match editor.selection_restore_status(token) {
                        SelectionRestoreStatus::Pending => {
                            self.spill_selection = Some((snapshot, anchor, caret, Some(token)))
                        }
                        SelectionRestoreStatus::Applied => {}
                        SelectionRestoreStatus::Failed(error) => self.message = Some(error),
                        SelectionRestoreStatus::Superseded => {
                            self.message = Some("Promoted selection restoration was superseded.".into())
                        }
                    }
                } else {
                    match editor.restore_global_selection(
                        bareline_document::TextOffset(anchor),
                        bareline_document::TextOffset(caret),
                        false,
                    ) {
                        Ok(token) => self.spill_selection = Some((snapshot, anchor, caret, Some(token))),
                        Err(error) => self.message = Some(error),
                    }
                }
            }
        }
        // Event-driven pressure handling; failed storage waits for explicit retry.
        if !self.spill_pending
            && !self.spill_paused
            && self.pending_io.is_empty()
            && self.bytes.used() > self.bytes.limit().saturating_mul(3) / 4
        {
            self.migrate_clean_resident();
        }
        changed
    }
    /// Retry or initiate an owned spill. Only clean history-free single views qualify;
    /// dirty/history-bearing tabs remain owned until their history spill path is available.
    /// Idempotent targeted promotion. `false` means its worker is still pending;
    /// `true` means the exact captured document now owns a paged actor.
    pub fn source_edit_budget(&self) -> Budget {
        self.bytes.clone()
    }
    pub fn promote_resident_for_source_edit(
        &mut self,
        index: usize,
        captured_identity: (u64, u64),
    ) -> Result<bool, String> {
        let editor = self.editors.get(index).ok_or("Document closed")?;
        match editor {
            WorkspaceEditor::Paged(paged) => {
                return if paged.snapshot().identity_token() == captured_identity {
                    Ok(true)
                } else {
                    Err("Document changed during promotion".into())
                };
            }
            WorkspaceEditor::Resident(resident) => {
                if resident.snapshot().identity_token() != captured_identity {
                    return Err("Document changed during promotion".into());
                }
            }
        }
        if self.promotion_target == Some(captured_identity) {
            return if self.spill_pending {
                Ok(false)
            } else {
                Err(self
                    .message
                    .clone()
                    .unwrap_or_else(|| "Promotion did not attach; document retained".into()))
            };
        }
        if self.spill_pending || !self.pending_io.is_empty() || editor.busy() {
            return Err("Document I/O is busy; retry promotion".into());
        }
        let captured = editor.snapshot().clone();
        if !captured.is_complete()
            || self
                .editors
                .iter()
                .filter(|peer| peer.snapshot().same_document(&captured))
                .count()
                != 1
        {
            return Err("Promotion requires one complete document actor view".into());
        }
        let service = editor.document_service().ok_or("Document actor unavailable")?;
        let saved_state = editor.saved_content_state().ok_or("Document actor unavailable")?;
        let encoding = self.files[index].as_ref().and_then(|file| file.encoding.clone());
        let original = self.files[index]
            .as_ref()
            .map(|file| (file.path.clone(), file.fingerprint.clone()));
        if !self.ensure_io() {
            return Err("File I/O unavailable".into());
        }
        let request = IoRequest::SpillOwnedResident {
            saved_state,
            service,
            captured,
            encoding,
            original,
            cache: std::env::temp_dir().join("Bareline-owned-spill"),
            quota: self.transcode_quota_bytes,
            options: self.source_options(),
            bytes: self.bytes.clone(),
            history: self.history.clone(),
        };
        let receiver = self
            .io
            .as_ref()
            .unwrap()
            .submit(request, self.notify.clone())
            .map_err(|_| "Spill queue full")?;
        self.promotion_target = Some(captured_identity);
        self.spill_pending = true;
        self.spill_paused = false;
        self.pending_io.push(PendingIo {
            completion: None,
            receiver,
            save: None,
            copy_only: false,
            open_path: None,
            launch_request: None,
            recovery_restore_request: None,
            allow_duplicate: false,
            preview: None,
            reload: None,
        });
        Ok(false)
    }
    pub fn migrate_clean_resident(&mut self) -> bool {
        if self.spill_pending || !self.pending_io.is_empty() {
            return false;
        }
        let candidate = self
            .editors
            .iter()
            .enumerate()
            .filter(|(index, editor)| {
                matches!(editor, WorkspaceEditor::Resident(_))
                    && self.files[*index].is_some()
                    && !editor.busy()
                    && editor.snapshot().is_complete()
                    && editor.snapshot().len() >= 1024 * 1024
                    && editor
                        .viewport()
                        .selection
                        .anchor
                        .abs_diff(editor.viewport().selection.caret)
                        <= 64 * 1024 - 8
                    && self
                        .editors
                        .iter()
                        .filter(|peer| peer.snapshot().same_document(editor.snapshot()))
                        .count()
                        == 1
            })
            .max_by_key(|(_, editor)| editor.snapshot().len())
            .map(|(index, _)| index);
        let Some(index) = candidate else {
            return false;
        };
        if !self.ensure_io() {
            return false;
        }
        let file = self.files[index].as_ref().unwrap();
        let Some(service) = self.editors[index].document_service() else {
            return false;
        };
        let Some(saved_state) = self.editors[index].saved_content_state() else {
            return false;
        };
        let request = IoRequest::SpillOwnedResident {
            saved_state,
            service,
            captured: self.editors[index].snapshot().clone(),
            encoding: file.encoding.clone(),
            original: Some((file.path.clone(), file.fingerprint.clone())),
            cache: std::env::temp_dir().join("Bareline-owned-spill"),
            quota: self.transcode_quota_bytes,
            options: self.source_options(),
            bytes: self.bytes.clone(),
            history: self.history.clone(),
        };
        match self.io.as_ref().unwrap().submit(request, self.notify.clone()) {
            Ok(receiver) => {
                self.spill_pending = true;
                self.spill_paused = false;
                self.pending_io.push(PendingIo {
                    completion: None,
                    receiver,
                    save: None,
                    copy_only: false,
                    open_path: None,
                    launch_request: None,
                    recovery_restore_request: None,
                    allow_duplicate: false,
                    preview: None,
                    reload: None,
                });
                true
            }
            Err(_) => false,
        }
    }
    fn discard_preview(&mut self, source: Option<&bareline_document::DocumentSnapshot>) {
        if let Some(index) = source.and_then(|source| {
            self.editors
                .iter()
                .position(|editor| editor.snapshot().same_document(source))
        }) {
            self.retired.push(self.editors.remove(index));
            self.files.remove(index);
            self.untitled_labels.remove(index);
            for pending in &mut self.pending_io {
                if let Some((target, _, _)) = &mut pending.save
                    && *target > index
                {
                    *target -= 1;
                }
            }
            self.last_drawn = None;
            self.find.clear_source();
        }
    }
    fn ensure_io(&mut self) -> bool {
        if self.io.is_none() {
            match IoService::new(self.file_system.clone()) {
                Ok(io) => self.io = Some(io),
                Err(e) => {
                    self.message = Some(e.to_string());
                    return false;
                }
            }
        }
        true
    }
    pub fn open(&mut self, path: PathBuf) {
        let _ = self.open_for_launch(path, None, true, false);
    }
    pub fn open_tracked(&mut self, request_id: u64, path: PathBuf) -> Result<(), String> {
        self.open_for_launch(path, Some(request_id), true, false)
    }
    pub fn open_recovery_tracked(&mut self, request_id: u64, path: PathBuf) -> Result<(), String> {
        self.open_for_launch(path, Some(request_id), false, true)
    }
    fn open_for_launch(
        &mut self,
        path: PathBuf,
        launch_request: Option<u64>,
        discover_recovery: bool,
        allow_duplicate: bool,
    ) -> Result<(), String> {
        if !self.ensure_io() {
            return Err("File service unavailable".into());
        }
        let recovery_parent = path.parent().map(PathBuf::from);
        let request = IoRequest::OpenStreaming {
            path: path.clone(),
            bytes: self.bytes.clone(),
            history: self.history.clone(),
            resident_max_bytes: self.resident_max_bytes,
        };
        match self.io.as_ref().unwrap().submit(request, self.notify.clone()) {
            Ok(receiver) => {
                self.pending_io.push(PendingIo {
                    completion: None,
                    receiver,
                    save: None,
                    copy_only: false,
                    open_path: Some(path),
                    launch_request,
                    recovery_restore_request: None,
                    allow_duplicate,
                    preview: None,
                    reload: None,
                });
                self.message = Some("Opening…".into());
                if discover_recovery && let Some(parent) = recovery_parent {
                    self.discover_save_recovery(&parent);
                }
                Ok(())
            }
            Err(_) => {
                let error = "File queue is full. Try again after the pending operation.".to_string();
                self.message = Some(error.clone());
                Err(error)
            }
        }
    }
    pub fn save_conflicts(&self) -> &[SaveConflict] {
        &self.save_conflicts
    }
    pub fn select_save_conflict(&mut self, active: usize, transaction: &std::path::Path) -> bool {
        let Some(context) = self.editors.get(active).map(WorkspaceEditor::document_identity) else {
            return false;
        };
        if self
            .save_conflicts
            .iter()
            .any(|conflict| conflict.transaction == transaction)
        {
            self.selected_save_conflict = Some((transaction.to_path_buf(), context));
            true
        } else {
            false
        }
    }
    pub fn select_next_save_conflict(&mut self, active: usize) -> Option<&SaveConflict> {
        if self.save_conflicts.is_empty() {
            return None;
        }
        let context = self.editors.get(active)?.document_identity();
        let active_path = self.path(active);
        let mut candidates: Vec<_> = self
            .save_conflicts
            .iter()
            .enumerate()
            .filter_map(|(index, conflict)| {
                (active_path.is_some()
                    && (conflict.target.as_deref() == active_path
                        || Some(conflict.editor_version.as_path()) == active_path))
                    .then_some(index)
            })
            .collect();
        if candidates.is_empty() {
            candidates.extend(0..self.save_conflicts.len());
        }
        let current = self
            .selected_save_conflict
            .as_ref()
            .filter(|(_, selected_context)| *selected_context == context)
            .and_then(|(selected, _)| {
                candidates
                    .iter()
                    .position(|index| self.save_conflicts[*index].transaction == *selected)
            });
        let next = candidates[current.map_or(0, |position| (position + 1) % candidates.len())];
        self.selected_save_conflict = Some((self.save_conflicts[next].transaction.clone(), context));
        self.save_conflicts.get(next)
    }
    pub fn selected_save_conflict(&self, active: usize) -> Option<&SaveConflict> {
        let active_path = self.path(active);
        let active_document = self.editors.get(active).map(WorkspaceEditor::document_identity);
        self.selected_save_conflict
            .as_ref()
            .filter(|(_, context)| Some(*context) == active_document)
            .and_then(|(selected, _)| {
                self.save_conflicts
                    .iter()
                    .find(|conflict| &conflict.transaction == selected)
            })
            .or_else(|| {
                self.save_conflicts.iter().find(|conflict| {
                    active_path.is_some()
                        && (conflict.target.as_deref() == active_path
                            || Some(conflict.editor_version.as_path()) == active_path)
                })
            })
    }
    pub fn selected_actionable_save_conflict(&self, active: usize) -> Option<&SaveConflict> {
        self.selected_save_conflict(active).filter(|conflict| {
            conflict.verified
                && conflict.state != bareline_platform::CommitState::CleanupPending
                && conflict.state != bareline_platform::CommitState::Unverified
        })
    }
    pub fn save_cleanups(&self) -> &[SaveCleanup] {
        &self.save_cleanups
    }
    pub fn selected_save_cleanup(&self) -> Option<&SaveCleanup> {
        self.selected_save_cleanup
            .as_ref()
            .and_then(|selected| {
                self.save_cleanups
                    .iter()
                    .find(|cleanup| &cleanup.transaction == selected)
            })
            .or_else(|| self.save_cleanups.first())
    }
    fn record_save_cleanup(&mut self, cleanup: SaveCleanup) {
        self.selected_save_cleanup = Some(cleanup.transaction.clone());
        if let Some(existing) = self
            .save_cleanups
            .iter_mut()
            .find(|known| known.transaction == cleanup.transaction)
        {
            *existing = cleanup;
        } else {
            self.save_cleanups.push(cleanup);
        }
    }
    pub fn retry_save_cleanup(&mut self, transaction: &std::path::Path) -> bool {
        if self.pending_save_cleanup.iter().any(|(known, _)| known == transaction) {
            return false;
        }
        let Some(index) = self
            .save_cleanups
            .iter()
            .position(|cleanup| cleanup.transaction == transaction)
        else {
            return false;
        };
        if !self.ensure_io() {
            return false;
        }
        let cleanup = self.save_cleanups[index].clone();
        match self
            .io
            .as_ref()
            .unwrap()
            .submit(IoRequest::RetrySaveCleanup { cleanup }, self.notify.clone())
        {
            Ok(ticket) => {
                self.pending_save_cleanup.push((transaction.to_path_buf(), ticket));
                self.message = Some("Cleaning saved recovery files…".into());
                true
            }
            Err(_) => {
                self.message = Some("Save cleanup queue is full. Retry after the pending operation.".into());
                false
            }
        }
    }
    pub fn retain_save_conflict(&mut self, transaction: &std::path::Path) -> bool {
        let Some(index) = self
            .save_conflicts
            .iter()
            .position(|conflict| conflict.transaction == transaction)
        else {
            return false;
        };
        self.save_conflicts.remove(index);
        if self
            .selected_save_conflict
            .as_ref()
            .is_some_and(|(selected, _)| selected == transaction)
        {
            self.selected_save_conflict = None;
        }
        true
    }
    fn record_save_conflict(&mut self, conflict: SaveConflict) {
        if let Some(existing) = self
            .save_conflicts
            .iter_mut()
            .find(|known| known.transaction == conflict.transaction)
        {
            *existing = conflict;
        } else {
            self.save_conflicts.push(conflict);
        }
    }
    pub fn track_save_conflict(&mut self, conflict: SaveConflict) {
        self.record_save_conflict(conflict);
    }
    fn merge_save_recovery(&mut self, found: Vec<SaveConflict>) -> usize {
        let before = self.save_conflicts.len();
        for conflict in found {
            if !self
                .save_conflicts
                .iter()
                .any(|known| known.transaction == conflict.transaction)
            {
                self.record_save_conflict(conflict);
            }
        }
        self.save_conflicts.len() - before
    }
    pub fn discover_save_recovery(&mut self, parent: &std::path::Path) -> bool {
        let parent = parent.to_path_buf();
        if !self.scanned_save_recovery.insert(parent.clone()) {
            return false;
        }
        if !self.ensure_io() {
            self.scanned_save_recovery.remove(&parent);
            self.failed_save_recovery.insert(parent);
            return false;
        }
        match self.io.as_ref().unwrap().submit(
            IoRequest::InspectSaveRecovery { parent: parent.clone() },
            self.notify.clone(),
        ) {
            Ok(ticket) => {
                self.pending_save_recovery.push((parent, ticket));
                true
            }
            Err(_) => {
                self.scanned_save_recovery.remove(&parent);
                self.failed_save_recovery.insert(parent.clone());
                self.message = Some("Save recovery queue is full. Retry recovery discovery from File commands.".into());
                false
            }
        }
    }
    pub fn retry_save_recovery(&mut self, parent: &std::path::Path) -> bool {
        self.failed_save_recovery.remove(parent);
        self.scanned_save_recovery.remove(parent);
        self.discover_save_recovery(parent)
    }
    pub fn failed_save_recovery(&self) -> Option<&std::path::Path> {
        self.failed_save_recovery.iter().next().map(PathBuf::as_path)
    }
    fn open_paged(&mut self, path: PathBuf, launch_request: Option<u64>, allow_duplicate: bool) {
        let request = IoRequest::OpenPagedEncoded(bareline_file_io::lifecycle::PagedOpenRequest {
            path: path.clone(),
            bytes: self.bytes.clone(),
            history: self.history.clone(),
            cache: std::env::temp_dir().join("Bareline-transcode"),
            options: bareline_file_io::codecs::disk::DiskOptions {
                temp_quota_bytes: self.transcode_quota_bytes,
                interpret: None,
            },
            source_options: self.source_options(),
        });
        self.submit_paged(request, path, launch_request, allow_duplicate);
    }
    fn submit_paged(&mut self, request: IoRequest, path: PathBuf, launch_request: Option<u64>, allow_duplicate: bool) {
        if !self.ensure_io() {
            self.record_launch_open(launch_request, Err("File service unavailable".into()));
            return;
        }
        match self.io.as_ref().unwrap().submit(request, self.notify.clone()) {
            Ok(receiver) => {
                self.pending_io.push(PendingIo {
                    completion: None,
                    receiver,
                    save: None,
                    copy_only: false,
                    open_path: Some(path),
                    launch_request,
                    recovery_restore_request: None,
                    allow_duplicate,
                    preview: None,
                    reload: None,
                });
                self.message = Some("Preparing paged text…".into());
            }
            Err(request) => {
                if let IoRequest::ResumeTranscode { paused, .. } = *request {
                    self.paused_transcode = Some(paused);
                }
                let error = "File queue is full; retry opening or resuming.".to_string();
                self.message = Some(error.clone());
                self.record_launch_open(launch_request, Err(error));
            }
        }
    }
    /// A recovered document that fits in memory becomes an ordinary editable tab under
    /// its own name, with unsaved changes, instead of a read-only paged view that can
    /// never finish loading. The receipt owns the exact recovered insertion;
    /// callers must not publish restore success before that receipt is terminal.
    fn adopt_recovered_resident(
        &mut self,
        opened: &mut bareline_file_io::lifecycle::PagedOpened,
    ) -> Result<Option<(u64, bareline_editor_surface::TrackedEditReceipt)>, String> {
        let Some(text) = opened.recovered_resident.take() else {
            return Ok(None);
        };
        let recovery_origin = opened.recovery_origin.clone();
        let label = opened
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled 1".to_owned());
        let document = Document::from_utf8("", self.bytes.clone(), self.history.clone())
            .map_err(|error| format!("Recovered text could not create an editor: {error:?}"))?;
        let snapshot = document.snapshot();
        let document_id = snapshot.identity_token().0;
        let mut surface = EditorSurface::new(self.scheduler.document(document, 32), snapshot, self.notify.clone());
        // Inserting the recovered text is what marks the tab as having unsaved changes.
        let receipt = surface.enqueue_tracked(Input::Insert(text))?;
        self.editors.push(surface.into());
        self.files.push(None);
        self.untitled_labels.push(label);
        let index = self.editors.len() - 1;
        let recovery_root = self.recovery_root.clone().or_else(|| {
            recovery_origin
                .as_deref()
                .and_then(std::path::Path::parent)
                .map(std::path::Path::to_path_buf)
        });
        if let Some(root) = recovery_root
            && let WorkspaceEditor::Resident(surface) = &mut self.editors[index]
        {
            surface.enable_recovery(root, self.file_system.clone(), None, None, self.bytes.clone());
            if let Some(origin) = recovery_origin {
                surface.adopt_recovery_origin(origin);
            }
        }
        self.message = None;
        Ok(Some((document_id, receipt)))
    }
    pub fn restore_paged_recovery(&mut self, directory: PathBuf) {
        self.submit_paged_recovery(directory);
    }
    /// Submit a recovery restore whose exact terminal publication must be
    /// observed by an owning controller. At most 32 unfinished or unconsumed
    /// tracked restores are retained; untracked restore callers do not occupy
    /// this queue.
    pub fn restore_paged_recovery_tracked(&mut self, directory: PathBuf) -> Result<u64, String> {
        let retained = self.recovery_restore_outcomes.len()
            + self.pending_recovery_restore_publications.len()
            + self
                .pending_io
                .iter()
                .filter(|pending| pending.recovery_restore_request.is_some())
                .count();
        if retained >= 32 {
            return Err("Too many recovery restore results are waiting to be handled.".into());
        }
        let request_id = self.next_recovery_restore_request;
        self.next_recovery_restore_request = self.next_recovery_restore_request.wrapping_add(1).max(1);
        let before = self.pending_io.len();
        self.submit_paged_recovery(directory);
        if self.pending_io.len() > before {
            self.pending_io.last_mut().unwrap().recovery_restore_request = Some(request_id);
        } else {
            self.recovery_restore_outcomes.push(RecoveryRestoreOutcome::Failed {
                request_id,
                error: self
                    .message
                    .clone()
                    .unwrap_or_else(|| "Recovery restore could not be queued.".into()),
            });
        }
        Ok(request_id)
    }
    fn submit_paged_recovery(&mut self, directory: PathBuf) {
        self.submit_paged(
            IoRequest::RestorePagedRecovery {
                directory: directory.clone(),
                bytes: self.bytes.clone(),
                history: self.history.clone(),
                resident_max_bytes: self.resident_max_bytes.min(16 << 20),
            },
            directory,
            None,
            false,
        );
    }
    /// Take only the requested result, preserving every other controller's
    /// still-observable terminal outcome.
    pub fn take_recovery_restore_outcome(&mut self, request_id: u64) -> Option<RecoveryRestoreOutcome> {
        let index = self.recovery_restore_outcomes.iter().position(|outcome| {
            matches!(outcome,
                RecoveryRestoreOutcome::Restored { request_id: known, .. }
                    | RecoveryRestoreOutcome::Failed { request_id: known, .. }
                    if *known == request_id)
        })?;
        Some(self.recovery_restore_outcomes.remove(index))
    }
    pub fn paused_transcode_info(&self) -> Option<(&std::path::Path, u64, u64, u64)> {
        let paused = self.paused_transcode.as_ref()?;
        if let bareline_file_io::codecs::disk::DiskError::Quota { used, required, limit } = paused.error {
            Some((&paused.path, used, required, limit))
        } else {
            None
        }
    }
    pub fn resume_transcode(&mut self, quota_bytes: u64) {
        self.transcode_quota_bytes = quota_bytes;
        if let Some(paused) = self.paused_transcode.take() {
            let path = paused.path.clone();
            let before = self.pending_io.len();
            self.submit_paged(
                IoRequest::ResumeTranscode {
                    paused,
                    temp_quota_bytes: quota_bytes,
                },
                path,
                None,
                false,
            );
            if self.pending_io.len() > before {
                self.pending_io.last_mut().unwrap().reload = self.paused_reload.take();
            }
        }
    }
    /// Recent paths are metadata only; loading this list never opens a file.
    pub fn recent_paths(&self) -> &[bareline_platform::SerializedPath] {
        &self.recent
    }
    pub fn restore_recent_paths(&mut self, recent: &[bareline_platform::SerializedPath]) {
        for path in recent.iter().take(100) {
            if !self
                .recent
                .iter()
                .any(|existing| existing.encoding == path.encoding && existing.data == path.data)
            {
                self.recent.push(path.clone());
            }
        }
        self.recent.truncate(100);
    }
    fn note_recent(&mut self, path: PathBuf) {
        let path = bareline_platform::SerializedPath::from_native(&path);
        self.recent
            .retain(|existing| existing.encoding != path.encoding || existing.data != path.data);
        self.recent.insert(0, path);
        self.recent.truncate(100);
    }
    pub fn path(&self, index: usize) -> Option<&std::path::Path> {
        if matches!(self.editors.get(index),Some(WorkspaceEditor::Paged(editor)) if editor.save_as_required()) {
            return None;
        }
        self.files.get(index).and_then(|f| f.as_ref().map(|f| f.path.as_path()))
    }
    pub fn path_loading(&self, path: &std::path::Path) -> bool {
        self.pending_io
            .iter()
            .any(|pending| pending.open_path.as_deref() == Some(path))
    }
    pub fn take_launch_open_outcomes(&mut self) -> Vec<LaunchOpenOutcome> {
        std::mem::take(&mut self.open_outcomes)
    }
    pub fn take_tracked_open_outcomes(&mut self, request_ids: &[u64]) -> Vec<LaunchOpenOutcome> {
        let outcomes = std::mem::take(&mut self.open_outcomes);
        let (selected, retained) = outcomes.into_iter().partition(|outcome| {
            let request_id = match outcome {
                LaunchOpenOutcome::Opened { request_id, .. } | LaunchOpenOutcome::Failed { request_id, .. } => {
                    *request_id
                }
            };
            request_ids.contains(&request_id)
        });
        self.open_outcomes = retained;
        selected
    }
    fn record_launch_open(&mut self, request_id: Option<u64>, result: Result<(u64, u64), String>) {
        let Some(request_id) = request_id else {
            return;
        };
        self.open_outcomes.push(match result {
            Ok(document) => LaunchOpenOutcome::Opened { request_id, document },
            Err(error) => LaunchOpenOutcome::Failed { request_id, error },
        });
    }
    fn record_recovery_restore(&mut self, request_id: Option<u64>, result: Result<(u64, u64), String>) {
        let Some(request_id) = request_id else {
            return;
        };
        self.recovery_restore_outcomes.push(match result {
            Ok(document) => RecoveryRestoreOutcome::Restored { request_id, document },
            Err(error) => RecoveryRestoreOutcome::Failed { request_id, error },
        });
    }
    pub fn fingerprint(&self, index: usize) -> Option<&Fingerprint> {
        self.files
            .get(index)
            .and_then(|file| file.as_ref())
            .map(|file| &file.fingerprint)
    }
    pub fn reload(&mut self, index: usize, discard_confirmed: bool) -> Result<(), String> {
        let editor = self.editors.get(index).ok_or("Document is unavailable")?;
        if editor.busy() || (editor.dirty() && !discard_confirmed) {
            return Err("Confirm discard of current edits before reloading".into());
        }
        let captured = editor.snapshot().clone();
        let path = self
            .path(index)
            .ok_or("Save this document before reloading")?
            .to_path_buf();
        if self.path_loading(&path) {
            return Err("This file is already loading".into());
        }
        if !self.ensure_io() {
            return Err("File service unavailable".into());
        }
        let request = if matches!(self.editors[index], WorkspaceEditor::Paged(_)) {
            self.remote_open_request(path.clone())
        } else {
            IoRequest::OpenStreaming {
                path: path.clone(),
                bytes: self.bytes.clone(),
                history: self.history.clone(),
                resident_max_bytes: self.resident_max_bytes,
            }
        };
        let receiver = self
            .io
            .as_ref()
            .unwrap()
            .submit(request, self.notify.clone())
            .map_err(|_| "File queue is full")?;
        self.pending_io.push(PendingIo {
            completion: None,
            receiver,
            save: None,
            copy_only: false,
            open_path: Some(path),
            launch_request: None,
            recovery_restore_request: None,
            allow_duplicate: false,
            preview: None,
            reload: Some(captured),
        });
        self.message = Some("Reloading… current text remains available until complete.".into());
        Ok(())
    }
    pub fn reorder(&mut self, order: &[usize]) -> bool {
        if self.io_busy() || order.len() != self.editors.len() {
            return false;
        }
        let unique: std::collections::BTreeSet<_> = order.iter().copied().collect();
        if unique.len() != order.len() || unique.last().is_some_and(|last| *last >= order.len()) {
            return false;
        }
        let mut editors: Vec<_> = self.editors.drain(..).map(Some).collect();
        let mut files: Vec<_> = self.files.drain(..).map(Some).collect();
        let mut titles: Vec<_> = self.untitled_labels.drain(..).map(Some).collect();
        for index in order {
            self.editors.push(editors[*index].take().unwrap());
            self.files.push(files[*index].take().unwrap());
            self.untitled_labels.push(titles[*index].take().unwrap());
        }
        self.last_drawn = None;
        self.find.clear_source();
        true
    }
    pub fn document_busy(&self, index: usize) -> bool {
        self.editors.get(index).is_some_and(|editor| editor.busy())
            || self
                .pending_io
                .iter()
                .any(|pending| pending.save.as_ref().is_some_and(|(target, _, _)| *target == index))
    }
    pub fn discard_recoveries(&mut self, indexes: &[usize]) -> bareline_file_io::recovery_retirement::DiscardPoll {
        let mut outcome = bareline_file_io::recovery_retirement::DiscardPoll::Durable;
        for &index in indexes {
            let Some(editor) = self.editors.get_mut(index) else {
                continue;
            };
            match editor.discard_recovery() {
                bareline_file_io::recovery_retirement::DiscardPoll::TombstoneFailed(error) => {
                    return bareline_file_io::recovery_retirement::DiscardPoll::TombstoneFailed(error);
                }
                bareline_file_io::recovery_retirement::DiscardPoll::Pending => {
                    outcome = bareline_file_io::recovery_retirement::DiscardPoll::Pending;
                }
                bareline_file_io::recovery_retirement::DiscardPoll::CleanupPending(error)
                    if !matches!(outcome, bareline_file_io::recovery_retirement::DiscardPoll::Pending) =>
                {
                    outcome = bareline_file_io::recovery_retirement::DiscardPoll::CleanupPending(error);
                }
                _ => {}
            }
        }
        outcome
    }
    pub fn close(&mut self, index: usize, discard: bool, renderer: &mut impl TextBackend) -> Result<(), CloseError> {
        let editor = self.editors.get(index).ok_or(CloseError::Missing)?;
        if self.document_busy(index) {
            return Err(CloseError::Busy);
        }
        if editor.dirty() && !discard {
            return Err(CloseError::Unsaved);
        }
        let preview_source = editor.read_only().then(|| editor.snapshot().clone());
        if discard {
            match self.editors[index].discard_recovery() {
                bareline_file_io::recovery_retirement::DiscardPoll::Pending => {
                    return Err(CloseError::RecoveryPending);
                }
                bareline_file_io::recovery_retirement::DiscardPoll::TombstoneFailed(error) => {
                    self.message = Some(format!("Discard not yet durable: {error}. Retrying recovery cleanup."));
                    return Err(CloseError::RecoveryFailed(error));
                }
                bareline_file_io::recovery_retirement::DiscardPoll::CleanupPending(error) => {
                    self.message = Some(format!("Document discarded; recovery cleanup pending: {error}"));
                }
                bareline_file_io::recovery_retirement::DiscardPoll::Durable => {}
            }
        }
        if let Some(source) = preview_source {
            self.pending_io.retain(|pending| {
                !pending
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.same_document(&source))
            });
        }
        let mut closed = self.editors.remove(index);
        closed.release_layouts(renderer);
        let file = self.files.remove(index);
        let label = self.untitled_labels.remove(index);
        self.closed_documents
            .borrow_mut()
            .push(closed.snapshot().identity_token());
        let reopen = (!closed.dirty())
            .then(|| file.as_ref().map(|file| file.path.clone()))
            .flatten()
            .filter(|path| path.is_file());
        match reopen {
            // Release the paged source, spill store and transcode directory now.
            Some(path) => {
                drop(closed);
                self.closed.push(ClosedDocument::Reopen(path));
            }
            None => {
                // At most one closed document keeps a paged source alive.
                if closed.paged() {
                    self.closed.retain(|entry| !entry.paged());
                }
                self.closed
                    .push(ClosedDocument::Retained(Box::new(closed), file, label));
            }
        }
        if self.closed.len() > 20 {
            self.closed.remove(0);
        }
        self.find.clear_source();
        if self.editors.is_empty() {
            self.find.hide();
        }
        for pending in &mut self.pending_io {
            if let Some((target, _, _)) = &mut pending.save
                && *target > index
            {
                *target -= 1;
            }
        }
        self.last_drawn = match self.last_drawn {
            Some(previous) if previous == index => None,
            Some(previous) if previous > index => Some(previous - 1),
            other => other,
        };
        Ok(())
    }
    pub fn styling_receipt(&self) -> Option<crate::styling::StylingReceipt> {
        self.styling.receipt()
    }
    pub fn syntax_result(&self) -> Option<&bareline_syntax::SyntaxResult> {
        self.styling.result.as_ref()
    }
    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }
    pub fn scheduler_and_editors(&mut self) -> (&Scheduler, &mut Vec<WorkspaceEditor>) {
        (&self.scheduler, &mut self.editors)
    }
    pub fn can_restore_closed(&self) -> bool {
        !self.closed.is_empty()
    }
    pub fn set_last_closed_read_only(&mut self, read_only: bool) {
        if let Some(ClosedDocument::Retained(editor, _, _)) = self.closed.last_mut() {
            editor.set_read_only(read_only);
        }
    }
    /// Documents closed since the last call, so panels can drop cached views of them.
    pub fn take_closed_documents(&self) -> Vec<(u64, u64)> {
        std::mem::take(&mut self.closed_documents.borrow_mut())
    }
    /// Reattach the retained model, history and selection without reopening its path.
    pub fn restore_last_closed(&mut self) -> Option<usize> {
        let (mut editor, file, label) = match self.closed.pop()? {
            ClosedDocument::Retained(editor, file, label) => (*editor, file, label),
            ClosedDocument::Reopen(path) => {
                self.open(path);
                return None;
            }
        };
        editor.resume_recovery_after_discard();
        if !matches!(&editor,WorkspaceEditor::Paged(paged) if paged.save_as_required())
            && let Some(file) = &file
        {
            self.note_recent(file.path.clone());
        }
        let index = self.editors.len();
        self.editors.push(editor);
        self.files.push(file);
        self.untitled_labels.push(label);
        self.last_drawn = None;
        Some(index)
    }
    pub fn io_busy(&self) -> bool {
        self.eol_job.is_some()
            || !self.pending_io.is_empty()
            || self.editors.iter().any(|editor| editor.paged() && editor.busy())
    }
    pub fn cancel_file_operations(&mut self) {
        self.eol_job = None;
        self.paused_transcode = None;
        for pending in &self.pending_io {
            pending.receiver.cancel();
        }
        if self.io_busy() {
            self.message = Some("Cancelling file operations…".into());
        }
    }
    pub fn save(&mut self, index: usize, path: PathBuf) -> bool {
        let Some(editor) = self.editors.get(index) else {
            return false;
        };
        let document = editor.document_identity();
        let current = self
            .files
            .get(index)
            .and_then(Option::as_ref)
            .filter(|file| file.path == path);
        let operation = if current.is_some() {
            SaveOperation::Save
        } else {
            SaveOperation::SaveAs
        };
        let (condition, consent) = current.map_or(
            (DestinationCondition::MustBeAbsent, DestinationConsent::NotRequired),
            |file| {
                (
                    DestinationCondition::ReplaceCaptured(file.fingerprint.clone()),
                    DestinationConsent::ExistingDocument,
                )
            },
        );
        self.save_prepared(
            index,
            PreparedDestination {
                path,
                condition,
                consent,
                document,
                operation,
            },
        )
    }
    pub fn save_copy(&mut self, index: usize, path: PathBuf) -> bool {
        let Some(editor) = self.editors.get(index) else {
            return false;
        };
        self.save_prepared(
            index,
            PreparedDestination {
                path,
                condition: DestinationCondition::MustBeAbsent,
                consent: DestinationConsent::NotRequired,
                document: editor.document_identity(),
                operation: SaveOperation::SaveCopy,
            },
        )
    }
    pub fn save_prepared(&mut self, index: usize, destination: PreparedDestination) -> bool {
        self.save_internal(index, destination)
    }
    /// Dirty documents in stable tab order; the native caller prompts for untitled paths.
    pub fn save_all_targets(&self) -> Vec<(usize, Option<PathBuf>)> {
        self.editors
            .iter()
            .enumerate()
            .filter(|(_, editor)| editor.dirty())
            .map(|(index, _)| (index, self.path(index).map(PathBuf::from)))
            .collect()
    }
    fn save_internal(&mut self, index: usize, destination: PreparedDestination) -> bool {
        let Some(editor) = self.editors.get(index) else {
            return false;
        };
        let identity = editor.document_identity();
        if identity != destination.document {
            self.message = Some("Save destination expired because the document changed or closed.".into());
            return false;
        }
        let copy_only = destination.operation == SaveOperation::SaveCopy;
        let authorized = match (&destination.condition, destination.consent) {
            (DestinationCondition::MustBeAbsent, DestinationConsent::NotRequired) => true,
            (DestinationCondition::ReplaceCaptured(_), DestinationConsent::OverwriteConfirmed) => true,
            (DestinationCondition::ReplaceCaptured(captured), DestinationConsent::ExistingDocument) => {
                destination.operation == SaveOperation::Save
                    && self
                        .files
                        .get(index)
                        .and_then(Option::as_ref)
                        .is_some_and(|file| file.path == destination.path && &file.fingerprint == captured)
            }
            _ => false,
        };
        if !authorized {
            self.message = Some("Save destination has no matching overwrite consent.".into());
            return false;
        }
        self.encoding_failures
            .retain(|failure| !failure.same_document(identity));
        if copy_only && self.path(index).is_some_and(|source| source == destination.path) {
            self.message = Some("Save Copy needs a different destination from the document source.".into());
            return false;
        }
        if let Some(WorkspaceEditor::Paged(editor)) = self.editors.get_mut(index) {
            let submitted = if copy_only {
                editor.save_copy_prepared_tracked(destination, self.file_system.clone())
            } else {
                editor.save_prepared_tracked(destination, self.file_system.clone())
            };
            self.message = match &submitted {
                Ok(owner) => {
                    self.pending_paged_saves.insert(*owner);
                    Some("Saving…".into())
                }
                Err(error) => Some(error.clone()),
            };
            return submitted.is_ok();
        }
        if !self.ensure_io() {
            return false;
        }
        if self
            .pending_io
            .iter()
            .any(|p| p.save.as_ref().is_some_and(|(i, _, _)| *i == index))
        {
            self.message = Some("This document is already saving.".into());
            return false;
        }
        let Some(editor) = self.editors.get(index) else {
            return false;
        };
        if editor.busy() {
            self.message = Some("Wait for the pending edit before saving.".into());
            return false;
        }
        if !editor.snapshot().is_complete() || (editor.read_only() && !copy_only) {
            self.message = Some("Document is not ready or is read only; saving is unavailable.".into());
            return false;
        }
        let bom = self.files[index].as_ref().is_some_and(|file| file.bom);
        let path = destination.path.clone();
        let request = if copy_only {
            IoRequest::SaveCopy {
                snapshot: editor.snapshot().clone(),
                destination,
                source: self.files[index].as_ref().map(|file| file.path.clone()),
                bom,
                encoding: self.files[index].as_ref().and_then(|file| file.encoding.clone()),
            }
        } else if let Some(encoding) = self.files[index].as_ref().and_then(|file| file.encoding.clone()) {
            IoRequest::SaveEncoded {
                snapshot: editor.snapshot().clone(),
                destination,
                bom,
                encoding,
            }
        } else {
            IoRequest::Save {
                snapshot: editor.snapshot().clone(),
                destination,
                bom,
            }
        };
        match self.io.as_ref().unwrap().submit(request, self.notify.clone()) {
            Ok(receiver) => {
                self.pending_io.push(PendingIo {
                    completion: None,
                    receiver,
                    save: Some((index, path, bom)),
                    copy_only,
                    open_path: None,
                    launch_request: None,
                    recovery_restore_request: None,
                    allow_duplicate: false,
                    preview: None,
                    reload: None,
                });
                self.message = Some("Saving…".into());
                true
            }
            Err(_) => {
                self.message = Some("File queue is full. Try again after the pending operation.".into());
                false
            }
        }
    }
    pub fn titles(&self) -> Vec<String> {
        self.editors
            .iter()
            .enumerate()
            .map(|(i, editor)| {
                let mut title = self.files[i]
                    .as_ref()
                    .and_then(|file| file.path.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.untitled_labels[i].clone());
                if editor.dirty() {
                    title.push_str(" •");
                }
                title
            })
            .collect()
    }
    /// Drain only navigation commands whose target selection has actually been applied.
    pub fn take_acknowledged_commands(&mut self) -> Vec<(String, std::collections::BTreeMap<String, String>)> {
        self.pump_search_acknowledgment();
        self.take_ordered_search_receipts()
            .into_iter()
            .filter_map(|receipt| match receipt.event {
                bareline_editor_surface::power::consumer::ReceiptEvent::Command(id, arguments) => Some((id, arguments)),
                _ => None,
            })
            .collect()
    }
    pub fn take_ordered_search_receipts(&mut self) -> Vec<bareline_editor_surface::power::consumer::OrderedReceipt> {
        self.pump_search_acknowledgment();
        std::mem::take(&mut self.acknowledged_search_commands)
    }
    fn pump_search_acknowledgment(&mut self) -> bool {
        let Some(pending) = self.pending_search_navigation.take() else {
            return false;
        };
        let mut waiting = false;
        let mut acknowledged = false;
        for editor in &self.editors {
            let current = match (&pending.source, editor) {
                (SearchNavigationSource::Resident(source), WorkspaceEditor::Resident(editor))
                    if source.same_document(editor.snapshot()) =>
                {
                    let same = source.revision == editor.snapshot().revision;
                    let job = self.find.completed_results().map(|results| results.job);
                    Some((
                        same && job == Some(pending.job),
                        editor.busy(),
                        editor.selection.anchor,
                        editor.selection.caret,
                    ))
                }
                (SearchNavigationSource::Paged(source), WorkspaceEditor::Paged(editor))
                    if source.same_document(editor.snapshot()) =>
                {
                    let same = source.revision == editor.snapshot().revision
                        && source.content_state == editor.snapshot().content_state;
                    let job = self.find.completed_paged_results().map(|results| results.job);
                    Some((
                        same && job == Some(pending.job),
                        editor.busy(),
                        editor.global_selection().0.0,
                        editor.global_selection().1.0,
                    ))
                }
                _ => None,
            };
            if let Some((same, busy, anchor, caret)) = current {
                if !same {
                    break;
                }
                if busy {
                    waiting = true;
                    break;
                }
                acknowledged = anchor == pending.range.start.0 && caret == pending.range.end.0;
                break;
            }
        }
        if waiting {
            self.pending_search_navigation = Some(pending);
            return false;
        }
        if acknowledged {
            let id = if pending.backwards {
                "search.find_previous"
            } else {
                "search.find_next"
            };
            self.acknowledged_search_commands
                .push(bareline_editor_surface::power::consumer::OrderedReceipt {
                    sequence: bareline_editor_surface::power::consumer::next_receipt_sequence(),
                    event: bareline_editor_surface::power::consumer::ReceiptEvent::Command(
                        id.into(),
                        crate::macros::search_arguments(&pending.query),
                    ),
                });
        }
        acknowledged
    }
    pub fn find_next(&mut self, active: usize, backwards: bool) {
        self.bind_find_to(active);
        self.pump_search_acknowledgment();
        if self.pending_search_navigation.is_some() || self.acknowledged_search_commands.len() >= 256 {
            return;
        }
        let Some(editor) = self.editors.get_mut(active) else {
            return;
        };
        if editor.busy() {
            return;
        }
        let at = if backwards {
            editor
                .viewport()
                .selection
                .anchor
                .min(editor.viewport().selection.caret)
        } else {
            editor
                .viewport()
                .selection
                .anchor
                .max(editor.viewport().selection.caret)
        };
        let captured = match editor {
            WorkspaceEditor::Resident(resident) => self.find.completed_results().map(|results| {
                (
                    SearchNavigationSource::Resident(resident.snapshot().clone()),
                    results.job,
                )
            }),
            WorkspaceEditor::Paged(paged) => self
                .find
                .completed_paged_results()
                .map(|results| (SearchNavigationSource::Paged(paged.snapshot().clone()), results.job)),
        };
        let query = self.find.query();
        let found = match editor {
            WorkspaceEditor::Resident(resident) => self.find.next(resident.snapshot(), at, backwards),
            WorkspaceEditor::Paged(paged) => {
                let (anchor, caret) = paged.global_selection();
                let at = if backwards {
                    anchor.0.min(caret.0)
                } else {
                    anchor.0.max(caret.0)
                };
                self.find.next_paged(paged.snapshot(), at, backwards)
            }
        };
        if let Some(range) = found {
            let applied = match editor {
                WorkspaceEditor::Resident(resident) => {
                    resident.enqueue(Input::SetCaret(range.start.0, false));
                    resident.enqueue(Input::SetCaret(range.end.0, true));
                    resident.search_selection = true;
                    true
                }
                WorkspaceEditor::Paged(paged) => match paged.restore_selection(range.start, range.end) {
                    Ok(()) => true,
                    Err(error) => {
                        self.message = Some(error);
                        false
                    }
                },
            };
            if applied && let Some((source, job)) = captured {
                self.pending_search_navigation = Some(PendingSearchNavigation {
                    source,
                    job,
                    query,
                    range,
                    backwards,
                });
            }
        }
    }
    pub fn take_paged_search_activation(&mut self) -> Option<usize> {
        let (source, range) = self.search_panel.take_paged_activation()?;
        let index = self.editors.iter().position(|editor| {
            matches!(editor,
            WorkspaceEditor::Paged(editor) if source.same_document(editor.snapshot()))
        })?;
        let WorkspaceEditor::Paged(editor) = &mut self.editors[index] else {
            return None;
        };
        if editor.busy()
            || source.revision != editor.snapshot().revision
            || source.content_state != editor.snapshot().content_state
        {
            self.message = Some("Search result is stale; run the search again.".into());
            return None;
        }
        match editor.restore_selection(range.start, range.end) {
            Ok(()) => Some(index),
            Err(error) => {
                self.message = Some(error);
                None
            }
        }
    }
    pub fn activate_search(
        &mut self,
        source: bareline_document::DocumentSnapshot,
        range: std::ops::Range<bareline_document::TextOffset>,
    ) -> Option<usize> {
        let index = self
            .editors
            .iter()
            .position(|editor| editor.snapshot().same_document(&source))?;
        let editor = &mut self.editors[index];
        if editor.snapshot().revision != source.revision || editor.busy() {
            self.message = Some("Search result is stale; run the search again.".into());
            return None;
        }
        editor.enqueue(Input::SetCaret(range.start.0, false));
        editor.enqueue(Input::SetCaret(range.end.0, true));
        editor.viewport_mut().search_selection = true;
        Some(index)
    }
    pub fn replace(&mut self, active: usize, all: bool) {
        self.bind_find_to(active);
        if self.pending_replace.is_some() || self.pending_paged_replace.is_some() {
            return;
        }
        let Some(editor) = self.editors.get(active) else {
            return;
        };
        if editor.busy() {
            self.message = Some("Wait for the pending edit before replacing.".into());
            return;
        }
        if let WorkspaceEditor::Paged(paged) = editor {
            let (anchor, caret) = paged.global_selection();
            let selection = bareline_document::TextOffset(anchor.0.min(caret.0))
                ..bareline_document::TextOffset(anchor.0.max(caret.0));
            match self
                .find
                .start_replace_paged(paged.read_handle(), selection, all, self.notify.clone())
            {
                Ok(ticket) => {
                    self.pending_paged_replace = Some(ticket);
                    self.message = Some("Preparing paged replacement…".into());
                }
                Err(error) => self.message = Some(error.into()),
            }
            return;
        }
        let selection = bareline_document::TextOffset(
            editor
                .viewport()
                .selection
                .anchor
                .min(editor.viewport().selection.caret),
        )
            ..bareline_document::TextOffset(
                editor
                    .viewport()
                    .selection
                    .anchor
                    .max(editor.viewport().selection.caret),
            );
        match self
            .find
            .start_replace(editor.snapshot(), selection, all, self.notify.clone())
        {
            Ok(ticket) => self.pending_replace = Some(ticket),
            Err(error) => self.message = Some(error.into()),
        }
    }
    pub fn cancel_search(&mut self) {
        self.pending_paged_replace = None;
        self.pending_replace = None;
        self.find.cancel_search();
        self.message = Some("Search / replacement preparation cancelled.".into());
    }
    pub fn bind_find_to(&mut self, active: usize) {
        enum Source {
            Resident(bareline_document::DocumentSnapshot),
            Paged(bareline_document::paged::PagedSnapshot),
        }
        let source = self.editors.get(active).map(|editor| match editor {
            WorkspaceEditor::Resident(editor) => Source::Resident(editor.snapshot().clone()),
            WorkspaceEditor::Paged(editor) => Source::Paged(editor.snapshot().clone()),
        });
        match source {
            Some(Source::Resident(source)) => self.bind_find_resident(&source),
            Some(Source::Paged(source)) => self.bind_find_paged(&source),
            None => self.clear_find_source(),
        }
    }
    pub fn bind_find_resident(&mut self, source: &bareline_document::DocumentSnapshot) {
        if self.find.bind_resident(source) {
            self.detach_find_operations();
        }
    }
    pub fn bind_find_paged(&mut self, source: &bareline_document::paged::PagedSnapshot) {
        if self.find.bind_paged(source) {
            self.detach_find_operations();
        }
    }
    pub fn clear_find_source(&mut self) {
        self.find.clear_source();
        self.detach_find_operations();
    }
    fn detach_find_operations(&mut self) {
        self.pending_search_navigation = None;
        self.pending_replace = None;
        self.pending_paged_replace = None;
    }
    pub fn draw(
        &mut self,
        active: usize,
        renderer: &mut impl TextBackend,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        for mut editor in self.retired.drain(..) {
            editor.release_layouts(renderer);
        }
        if self.search_panel.take_search_requested() {
            let mut snapshots = Vec::new();
            let mut paged = Vec::new();
            for (index, editor) in self.editors.iter().enumerate() {
                match editor {
                    WorkspaceEditor::Resident(editor) => snapshots.push(editor.snapshot().clone()),
                    WorkspaceEditor::Paged(editor) => {
                        let handle = editor.read_handle();
                        let label = self
                            .files
                            .get(index)
                            .and_then(|file| file.as_ref())
                            .map(|file| file.path.display().to_string())
                            .unwrap_or_else(|| "Untitled".into());
                        paged.push(bareline_search::sources::PagedOpenDocument {
                            snapshot: handle.snapshot().clone(),
                            label,
                            resolve: Box::new(move |ticket| {
                                handle.resolve_page(ticket).map_err(|error| error.to_string())
                            }),
                        });
                    }
                }
            }
            self.search_panel
                .start_mixed(snapshots, paged, self.search_panel.query(), self.notify.clone());
        }
        if self.last_drawn != Some(active) {
            if let Some(previous) = self.last_drawn.and_then(|i| self.editors.get_mut(i)) {
                previous.release_layouts(renderer);
            }
            self.last_drawn = Some(active);
        }
        self.bind_find_to(active);
        let mut result = match self.editors.get_mut(active) {
            Some(editor) => {
                match editor {
                    WorkspaceEditor::Resident(resident) => self.find.refresh(resident.snapshot(), self.notify.clone()),
                    WorkspaceEditor::Paged(paged) => self.find.refresh_paged(paged.read_handle(), self.notify.clone()),
                }
                editor.viewport_mut().top_inset = if self.find.open { self.find.height() } else { 0.0 };
                editor.viewport_mut().bottom_inset = if self.external_search_panel {
                    self.bottom_panel_height
                } else {
                    self.search_panel.height()
                };
                let language = self
                    .files
                    .get(active)
                    .and_then(|file| file.as_ref())
                    .map_or(bareline_syntax::Language::PlainText, |file| {
                        bareline_syntax::Language::detect(&file.path)
                    });
                let definition = editor.viewport().udl.clone();
                let language = if definition.is_some() {
                    bareline_syntax::Language::PlainText
                } else {
                    editor
                        .viewport()
                        .language_override
                        .or(editor.viewport().detected_language)
                        .unwrap_or(language)
                };
                let label = definition.as_ref().map_or(language.label(), |d| d.name.as_str());
                editor.viewport_mut().language = language;
                if let WorkspaceEditor::Paged(paged) = editor {
                    let mut policy = bareline_settings::LanguagePolicy::default();
                    policy.lexer = match paged.viewport().syntax_preference {
                        bareline_syntax::LexerPreference::Lexilla => bareline_settings::LexerPreference::Primary,
                        bareline_syntax::LexerPreference::Native => bareline_settings::LexerPreference::Native,
                    };
                    self.styling.refresh_paged_mapped(
                        paged.read_handle(),
                        paged.viewport().snapshot(),
                        paged.viewport_start(),
                        paged.source_segments().to_vec(),
                        language,
                        crate::language::LanguageConfiguration {
                            policy,
                            definition: definition.clone(),
                        },
                        self.notify.clone(),
                    );
                    if self
                        .styling
                        .receipt()
                        .is_some_and(|receipt| receipt.identity == paged.snapshot().identity_token())
                        && self
                            .styling
                            .result
                            .as_ref()
                            .is_some_and(|result| result.is_current(&paged.viewport().snapshot()))
                        && let Some((folds, first_line, partial)) = self.styling.paged_folds.take()
                    {
                        if let Err(error) = paged.set_known_anchored_folds(folds, 0, partial, first_line) {
                            self.message = Some(error);
                        }
                    }
                }
                let paged = editor.paged();
                editor.set_external_scrollbar(paged);
                let frame_start = ops.len();
                let mut result = if paint_paged_pending(editor, width, height, self.theme, ops) {
                    Ok(None)
                } else {
                    editor.viewport_mut().draw_styled(
                        renderer,
                        width,
                        height,
                        ops,
                        bareline_editor_surface::SyntaxView {
                            result: self
                                .styling
                                .result
                                .as_ref()
                                .filter(|result| result.language == language),
                            language: label,
                            unavailable: self.styling.unavailable,
                        },
                    )
                };
                if let WorkspaceEditor::Paged(paged) = &mut *editor {
                    if let Err(error) = paged.refine_horizontal_viewport(renderer, width) {
                        paged.error = Some(error);
                    }
                }
                if matches!(&*editor, WorkspaceEditor::Paged(paged) if !paged.paged_frame_state().ready) {
                    ops.truncate(frame_start);
                    paint_paged_pending(editor, width, height, self.theme, ops);
                    result = Ok(None);
                }
                let result = result.map(|caret| {
                    if matches!(&*editor, WorkspaceEditor::Paged(paged) if !paged.caret_in_viewport()) {
                        if let Some(rect) = caret {
                            ops.retain(
                                |op| !matches!(op, bareline_renderer::DrawOp::Fill(bounds, _) if *bounds == rect),
                            );
                        }
                        None
                    } else {
                        caret
                    }
                });
                if let WorkspaceEditor::Resident(_) = editor {
                    if let Some(definition) = definition {
                        self.styling.refresh_udl(
                            editor.snapshot(),
                            definition,
                            editor.viewport().visible_text.clone(),
                            self.notify.clone(),
                        );
                    } else {
                        self.styling.refresh_preferred(
                            editor.snapshot(),
                            language,
                            editor.viewport().visible_text.clone(),
                            self.notify.clone(),
                            editor.viewport().syntax_preference,
                        );
                    }
                }
                result
            }
            None => Ok(None),
        };
        if let Some(caret) = self.find.draw_with_theme_in(renderer, width, height, self.theme, ops)? {
            result = Ok(Some(caret));
        }
        let labels: Vec<_> = self
            .editors
            .iter()
            .zip(self.titles())
            .map(|(editor, title)| (editor.snapshot().clone(), title))
            .collect();
        if !self.external_search_panel
            && let Some(caret) = self.search_panel.draw(renderer, width, height, &labels, ops)?
        {
            result = Ok(Some(caret));
        }
        if let Some(message) = &self.message {
            let y = (height - 58.0).max(34.0);
            ops.push(DrawOp::Fill(
                bareline_ui::rect(50.0, y, width - 66.0, 30.0),
                bareline_ui::ELEVATED,
            ));
            bareline_ui::text(ops, 64.0, y + 6.0, message, 13.0, bareline_ui::TEXT);
        }
        result
    }
}
fn file_error(error: FileError) -> String {
    match error {
        FileError::Transcode(error) => format!("File conversion stopped: {error:?}"),
        FileError::EncodingAt(failure) => format!(
            "{} at text bytes {}..{} (revision {}).",
            failure.reason, failure.range.start.0, failure.range.end.0, failure.revision
        ),
        FileError::Encoding(error) => format!("Encoding operation was not applied: {error:?}"),
        FileError::Cancelled => "File operation cancelled.".into(),
        FileError::IncompleteSource => "File is still loading; wait before saving.".into(),
        FileError::StreamingRequired => {
            "This file exceeds the configured resident limit; reopen with paged storage.".into()
        }
        FileError::UnsupportedEncoding => "This file needs an encoding that is not available in this build.".into(),
        FileError::Changed => "The file changed during the operation. Your edits remain in memory.".into(),
        FileError::Conflict { target, proposed, .. } => format!(
            "The destination changed before replacement. The current file is at {}; your editor version remains at {}.",
            target.display(),
            proposed.display()
        ),
        FileError::ConflictAfterCommit {
            target,
            proposed,
            displaced,
            ..
        } => format!(
            "The destination changed during replacement. Compare {} with the preserved other version at {}. Your editor version remains at {}; save it elsewhere or retain the other version.",
            target.display(),
            displaced.display(),
            proposed.display()
        ),
        FileError::ConflictAfterCreate { target, proposed, .. } => format!(
            "The new destination changed while the save completed. The current file is at {}; your editor version remains at {}.",
            target.display(),
            proposed.display()
        ),
        FileError::CancelledAfterCommit {
            target,
            proposed,
            displaced,
            ..
        } => match displaced {
            Some(displaced) => format!(
                "Save cancellation arrived after replacement. No success was recorded; inspect {}. Your editor version is at {}, and the displaced version is at {}.",
                target.display(),
                proposed.display(),
                displaced.display()
            ),
            None => format!(
                "Save cancellation arrived after the new file was created. No success was recorded; inspect {} or recover your editor version from {}.",
                target.display(),
                proposed.display()
            ),
        },
        FileError::VerificationAfterCommit {
            target,
            proposed,
            displaced,
            reason,
            ..
        } => match displaced {
            Some(displaced) => format!(
                "Save replacement needs recovery because verification failed ({reason}). Inspect {}; editor version: {}; displaced version: {}.",
                target.display(),
                proposed.display(),
                displaced.display()
            ),
            None => format!(
                "The created file needs recovery because verification failed ({reason}). Inspect {}; editor version: {}.",
                target.display(),
                proposed.display()
            ),
        },
        FileError::Commit {
            staged,
            proposed,
            displaced,
            error,
            ..
        } => match (proposed, displaced) {
            (Some(proposed), Some(displaced)) => format!(
                "Save could not finish the replacement ({error}). Retained transaction files: {}, {}, and {}",
                staged.display(),
                proposed.display(),
                displaced.display()
            ),
            (Some(proposed), None) => format!(
                "Save could not replace the destination ({error}). Staged copies: {} and {}",
                staged.display(),
                proposed.display()
            ),
            _ => format!(
                "Save could not replace the destination ({error}). Staged copy: {}",
                staged.display()
            ),
        },
        FileError::Io(error) => format!("File operation failed: {error}"),
        FileError::Budget => "Document memory budget reached.".into(),
    }
}

#[cfg(test)]
mod tests {
    fn remove_test_directory(path: impl AsRef<std::path::Path>) {
        let path = path.as_ref();
        assert!(
            path.starts_with(std::env::temp_dir()),
            "expected an owned temporary fixture"
        );
        // Workspace drop cancels workers asynchronously. Wait for sealed handles
        // and in-progress recovery writes before declaring fixture cleanup done.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match std::fs::remove_dir_all(path) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::DirectoryNotEmpty
                    ) && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(error) => panic!(
                    "fixture workers did not release {} before cleanup: {error}",
                    path.display()
                ),
            }
        }
    }
    use super::*;
    pub(super) struct PagedFileSystem;
    impl LocalFileSystem for PagedFileSystem {
        fn cache_directory_guard(
            &self,
            path: &std::path::Path,
        ) -> std::io::Result<Option<bareline_platform::CacheDirectoryLease>> {
            let metadata = match std::fs::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Ok(None);
            }
            use std::hash::{Hash, Hasher};
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut hash);
            Ok(Some(bareline_platform::CacheDirectoryLease {
                path: path.to_path_buf(),
                identity: bareline_platform::CacheDirectoryIdentity {
                    volume: 1,
                    file: hash.finish(),
                },
                guard: Arc::new(()),
                migration_publisher: None,
            }))
        }
        fn remove_owned_cache_directory(
            &self,
            root: &std::path::Path,
            candidate: &std::path::Path,
            _: bareline_platform::CacheDirectoryIdentity,
            _: bareline_platform::CacheDirectoryIdentity,
            proof_name: &str,
            proof_bytes: &[u8],
            _: usize,
            _: std::time::Duration,
            _: &dyn Fn() -> bool,
        ) -> bareline_platform::CacheRemovalOutcome {
            let result = (|| {
                if candidate.parent() != Some(root) || std::fs::read(candidate.join(proof_name))? != proof_bytes {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "test cleanup proof changed",
                    ));
                }
                std::fs::remove_dir_all(candidate)?;
                Ok(1)
            })();
            bareline_platform::CacheRemovalOutcome {
                visited: 1,
                retry_authority_retained: true,
                result,
            }
        }
        fn guard_directory(&self, _: &std::path::Path) -> std::io::Result<std::sync::Arc<dyn Send + Sync>> {
            Ok(std::sync::Arc::new(()))
        }
        fn available_space(&self, _: &std::path::Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        fn open_sealed_read(&self, path: &std::path::Path) -> std::io::Result<std::fs::File> {
            std::fs::File::open(path)
        }
        fn identity(&self, file: &std::fs::File) -> std::io::Result<bareline_platform::FileIdentity> {
            let meta = file.metadata()?;
            Ok(bareline_platform::FileIdentity {
                volume: 1,
                file: 1,
                length: meta.len(),
                modified: meta
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
            })
        }
        fn validate_target(&self, _: &std::path::Path) -> std::io::Result<()> {
            Ok(())
        }
        fn prepare_commit(
            &self,
            staged: &std::path::Path,
            target: &std::path::Path,
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
        fn commit(&self, staged: &std::path::Path, target: &std::path::Path, _: bool) -> std::io::Result<()> {
            if target.exists() {
                std::fs::remove_file(target)?;
            }
            std::fs::rename(staged, target)
        }
    }
    struct DistinctOpenFileSystem;
    impl LocalFileSystem for DistinctOpenFileSystem {
        fn guard_directory(&self, path: &std::path::Path) -> std::io::Result<std::sync::Arc<dyn Send + Sync>> {
            PagedFileSystem.guard_directory(path)
        }
        fn available_space(&self, path: &std::path::Path) -> std::io::Result<u64> {
            PagedFileSystem.available_space(path)
        }
        fn open_sealed_read(&self, path: &std::path::Path) -> std::io::Result<std::fs::File> {
            PagedFileSystem.open_sealed_read(path)
        }
        fn identity(&self, file: &std::fs::File) -> std::io::Result<bareline_platform::FileIdentity> {
            let mut identity = PagedFileSystem.identity(file)?;
            identity.file = identity.length;
            Ok(identity)
        }
        fn validate_target(&self, path: &std::path::Path) -> std::io::Result<()> {
            PagedFileSystem.validate_target(path)
        }
        fn commit(&self, staged: &std::path::Path, target: &std::path::Path, existed: bool) -> std::io::Result<()> {
            PagedFileSystem.commit(staged, target, existed)
        }
    }
    struct CleanupFailureFileSystem(std::sync::atomic::AtomicBool);
    impl LocalFileSystem for CleanupFailureFileSystem {
        fn guard_directory(&self, path: &std::path::Path) -> std::io::Result<std::sync::Arc<dyn Send + Sync>> {
            PagedFileSystem.guard_directory(path)
        }
        fn identity(&self, file: &std::fs::File) -> std::io::Result<bareline_platform::FileIdentity> {
            PagedFileSystem.identity(file)
        }
        fn validate_target(&self, path: &std::path::Path) -> std::io::Result<()> {
            PagedFileSystem.validate_target(path)
        }
        fn prepare_commit(
            &self,
            staged: &std::path::Path,
            target: &std::path::Path,
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
        fn cleanup_commit(&self, receipt: &mut bareline_platform::CommitReceipt) -> std::io::Result<()> {
            if self.0.swap(false, std::sync::atomic::Ordering::SeqCst) {
                Err(std::io::Error::other("injected cleanup failure"))
            } else {
                PagedFileSystem.cleanup_commit(receipt)
            }
        }
        fn commit(&self, staged: &std::path::Path, target: &std::path::Path, existed: bool) -> std::io::Result<()> {
            PagedFileSystem.commit(staged, target, existed)
        }
    }
    struct GatedRecoveryFileSystem {
        gate: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
        recovery: bareline_platform::CommitRecovery,
    }
    impl LocalFileSystem for GatedRecoveryFileSystem {
        fn identity(&self, file: &std::fs::File) -> std::io::Result<bareline_platform::FileIdentity> {
            PagedFileSystem.identity(file)
        }
        fn validate_target(&self, path: &std::path::Path) -> std::io::Result<()> {
            PagedFileSystem.validate_target(path)
        }
        fn inspect_commit_transactions(
            &self,
            _: &std::path::Path,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> std::io::Result<Vec<bareline_platform::CommitRecovery>> {
            let (ready, wake) = &*self.gate;
            let mut ready = ready.lock().unwrap();
            while !*ready {
                cancellation.check()?;
                let waited = wake.wait_timeout(ready, std::time::Duration::from_millis(10)).unwrap();
                ready = waited.0;
            }
            Ok(vec![self.recovery.clone()])
        }
        fn commit(&self, staged: &std::path::Path, target: &std::path::Path, existed: bool) -> std::io::Result<()> {
            PagedFileSystem.commit(staged, target, existed)
        }
    }
    #[test]
    fn recovery_discovery_is_async_deduplicated_and_worker_owned() {
        let parent = std::env::temp_dir().join(format!(
            "bareline-gated-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&parent).unwrap();
        let transaction = parent.join("state");
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let platform = Arc::new(GatedRecoveryFileSystem {
            gate: gate.clone(),
            recovery: bareline_platform::CommitRecovery {
                target: Some(parent.join("target.txt")),
                proposed: parent.join("editor-version"),
                displaced: Some(parent.join("displaced-version")),
                journal: transaction.clone(),
                state: bareline_platform::CommitState::Conflict,
                verified: true,
            },
        });
        let mut workspace = Workspace::new(Arc::new(|| {}), platform).unwrap();
        let started = std::time::Instant::now();
        assert!(workspace.discover_save_recovery(&parent));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(!workspace.discover_save_recovery(&parent));
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while workspace.pending_save_recovery.len() != 0 {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(workspace.save_conflicts().len(), 1);
        assert_eq!(workspace.save_conflicts()[0].transaction, transaction);
        workspace.new_document().unwrap();
        assert!(workspace.selected_save_conflict(0).is_none());
        assert!(workspace.select_save_conflict(0, &transaction));
        assert_eq!(workspace.selected_save_conflict(0).unwrap().transaction, transaction);
        drop(workspace);
        remove_test_directory(parent);
    }

    #[test]
    fn conflict_selection_cycles_within_active_document_and_invalidates_on_tab_change() {
        let root = std::env::temp_dir().join(format!(
            "bareline-conflict-selection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(DistinctOpenFileSystem)).unwrap();
        workspace.open(first.clone());
        workspace.open(second.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while workspace.io_busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(workspace.editors.len(), 2, "fixture must open two distinct documents");
        assert_ne!(
            workspace.editors[0].document_identity(),
            workspace.editors[1].document_identity(),
            "selection context requires distinct document identities"
        );
        for (transaction, target) in [
            (root.join("first-a"), first.clone()),
            (root.join("first-b"), first.clone()),
            (root.join("second-a"), second.clone()),
        ] {
            workspace.record_save_conflict(SaveConflict {
                target: Some(target),
                editor_version: transaction.with_extension("editor"),
                other_version: Some(transaction.with_extension("other")),
                transaction,
                state: bareline_platform::CommitState::Conflict,
                verified: true,
            });
        }
        assert_eq!(
            workspace.select_next_save_conflict(0).unwrap().transaction,
            root.join("first-a")
        );
        assert_eq!(
            workspace.select_next_save_conflict(0).unwrap().transaction,
            root.join("first-b")
        );
        assert_eq!(
            workspace.selected_save_conflict(1).unwrap().transaction,
            root.join("second-a")
        );
        remove_test_directory(root);
    }
    #[test]
    fn verified_save_marks_clean_while_cleanup_retry_remains_owned() {
        let root = std::env::temp_dir().join(format!(
            "bareline-cleanup-warning-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let file_system = Arc::new(CleanupFailureFileSystem(std::sync::atomic::AtomicBool::new(true)));
        let mut workspace = Workspace::new(Arc::new(|| {}), file_system).unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("saved bytes".into()));
        while workspace.editors[0].busy() {
            workspace.pump();
            std::thread::yield_now();
        }
        workspace.save(0, root.join("saved.txt"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        workspace.pump();
        assert!(
            !workspace.editors[0].dirty(),
            "verified target must be recorded as saved"
        );
        assert_eq!(workspace.save_cleanups().len(), 1);
        let transaction = workspace.save_cleanups()[0].transaction.clone();
        assert!(workspace.retry_save_cleanup(&transaction));
        while workspace.io_busy() || !workspace.pending_save_cleanup.is_empty() {
            workspace.pump();
            std::thread::yield_now();
        }
        assert!(workspace.save_cleanups().is_empty());
        remove_test_directory(root);
    }
    #[test]
    fn tracked_missing_opens_emit_terminal_receipts_without_accumulating() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        let missing = std::env::temp_dir().join(format!(
            "bareline-launch-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for request_id in 1..=257 {
            workspace.open_tracked(request_id, missing.clone()).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let outcome = loop {
                workspace.pump();
                if let Some(outcome) = workspace.take_launch_open_outcomes().pop() {
                    break outcome;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            };
            assert!(matches!(
                outcome,
                LaunchOpenOutcome::Failed {
                    request_id: actual,
                    ..
                } if actual == request_id
            ));
            assert!(!workspace.path_loading(&missing));
        }
        assert!(workspace.pending_io.is_empty());
        assert!(workspace.open_outcomes.is_empty());
        std::fs::write(&missing, b"valid\n").unwrap();
        workspace.open_tracked(258, missing.clone()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let outcome = loop {
            workspace.pump();
            if let Some(outcome) = workspace.take_launch_open_outcomes().pop() {
                break outcome;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert!(matches!(
            outcome,
            LaunchOpenOutcome::Opened {
                request_id: 258,
                document
            } if document == workspace.editors[0].document_identity()
        ));
        drop(workspace);
        std::fs::remove_file(missing).unwrap();
    }

    #[test]
    fn readonly_paged_reload_replaces_same_tab_and_preserves_policy() {
        let directory = std::env::temp_dir().join(format!(
            "bareline-readonly-reload-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("source.txt");
        std::fs::write(&path, "a".repeat(8192)).unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.resident_max_bytes = 4096;
        workspace.open(path.clone());
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
                std::thread::yield_now();
            }
        }
        settle(&mut workspace);
        workspace.editors[0].set_read_only(true);
        std::fs::write(&path, "b".repeat(12288)).unwrap();
        workspace.reload(0, false).unwrap();
        settle(&mut workspace);
        assert_eq!(workspace.editors.len(), 1);
        assert!(workspace.editors[0].viewport().user_read_only);
        assert!(matches!(&workspace.editors[0],WorkspaceEditor::Paged(editor) if editor.snapshot().len() == 12288));
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    #[test]
    fn paged_eol_counts_whole_file_and_rejects_stale_scan_results() {
        let directory = std::env::temp_dir().join(format!(
            "bareline-eol-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("lines.txt");
        std::fs::write(&path, format!("{}\r\n{}\r\n", "x".repeat(65535), "x".repeat(65535))).unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.resident_max_bytes = 4096;
        workspace.open(path);
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        settle(&mut workspace);
        assert_eq!(workspace.encoding_eol_label(0), "CRLF");
        workspace.editors[0].enqueue(Input::Insert("\n".into()));
        settle(&mut workspace);
        assert_eq!(workspace.encoding_eol_label(0), "Computing");
        workspace.editors[0].enqueue(Input::Undo);
        settle(&mut workspace);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let label = workspace.encoding_eol_label(0);
            assert_ne!(label, "Mixed", "stale edited-root count escaped");
            if label != "Computing" {
                assert_eq!(label, "CRLF");
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    #[test]
    fn encoding_failure_reveal_rejects_offsets_after_document_changes() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.new_document().unwrap();
        let identity = workspace.editors[0].snapshot().identity_token();
        workspace
            .encoding_failures
            .push(EncodingFailure::new(identity, 0..0, "fixture failure"));
        workspace.encoding_reveal_failure(0).unwrap();
        workspace.editors[0].enqueue(Input::Insert("x".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.editors[0].busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(
            workspace
                .encoding_reveal_failure(0)
                .unwrap_err()
                .contains("Document changed")
        );
    }
    #[test]
    fn encoding_policy_undo_redo_and_untitled_save() {
        use bareline_file_io::codecs::Encoding;
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.new_document().unwrap();
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        workspace.encoding_convert(0, Encoding::Utf16Le, true).unwrap();
        settle(&mut workspace);
        assert!(workspace.editors[0].dirty());
        assert_eq!(workspace.encoding_state(0).unwrap().save_target, Encoding::Utf16Le);
        workspace.editors[0].enqueue(Input::Undo);
        settle(&mut workspace);
        assert!(!workspace.editors[0].dirty());
        assert_eq!(workspace.encoding_state(0).unwrap().save_target, Encoding::Utf8);
        workspace.editors[0].enqueue(Input::Redo);
        settle(&mut workspace);
        let path = std::env::temp_dir().join(format!(
            "bareline-policy-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"replace me").unwrap();
        let prepared = bareline_file_io::lifecycle::preflight_destination(
            &path,
            None,
            workspace.editors[0].document_identity(),
            SaveOperation::SaveAs,
            &PagedFileSystem,
            &bareline_file_io::cancellation::Cancellation::default(),
        )
        .unwrap()
        .approve(true)
        .unwrap();
        workspace.save_prepared(0, prepared);
        settle(&mut workspace);
        assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0xfe]);
        assert!(!workspace.editors[0].dirty());
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn paged_workspace_edits_undoes_navigates_and_saves() {
        let directory = std::env::temp_dir().join(format!(
            "bareline-paged-ui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("source.txt");
        let saved = directory.join("saved.txt");
        let content = "line abc\r\n".repeat(20000);
        std::fs::write(&path, &content).unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.resident_max_bytes = 64 * 1024;
        workspace.open(path);
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        settle(&mut workspace);
        assert_eq!(workspace.editors.len(), 1, "{:?}", workspace.message);
        assert!(workspace.editors[0].paged());
        assert!(workspace.editors[0].snapshot().len() <= 65536);
        workspace.editors[0].enqueue(Input::Insert("edited ".into()));
        settle(&mut workspace);
        assert!(workspace.editors[0].dirty());
        workspace.editors[0].enqueue(Input::Undo);
        settle(&mut workspace);
        assert!(!workspace.editors[0].dirty());
        workspace.editors[0].enqueue(Input::Redo);
        settle(&mut workspace);
        workspace.editors[0].page_by(true);
        settle(&mut workspace);
        workspace.save(0, saved.clone());
        settle(&mut workspace);
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), format!("edited {content}"));
        assert_eq!(
            workspace.message, None,
            "paged save banner did not reach terminal state"
        );
        assert_eq!(workspace.path(0), Some(saved.as_path()));
        assert!(!workspace.editors[0].dirty());
        drop(workspace);
        remove_test_directory(directory);
    }
    #[test]
    fn resident_only_operation_is_reported_unsupported_for_a_paged_editor() {
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        let directory = std::env::temp_dir().join(format!(
            "bareline-notsupported-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let small = directory.join("small.txt");
        let large = directory.join("large.txt");
        std::fs::write(&small, "hello resident").unwrap();
        std::fs::write(&large, "line abc\r\n".repeat(20000)).unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.resident_max_bytes = 64 * 1024;
        workspace.open(small);
        workspace.open(large);
        settle(&mut workspace);
        let resident = workspace
            .editors
            .iter()
            .position(|editor| !editor.paged())
            .expect("a resident editor");
        let paged = workspace
            .editors
            .iter()
            .position(WorkspaceEditor::paged)
            .expect("a paged editor");
        // A resident editor answers the resident-only operation.
        assert!(workspace.editors[resident].selected_text().is_ok());
        // The same operation on a paged editor reports NotSupportedForPaged
        // instead of silently reading the bounded viewport placeholder.
        let error = workspace.editors[paged]
            .selected_text()
            .expect_err("paged is unsupported");
        assert_eq!(
            error,
            super::NotSupportedForPaged::new("Copying the selection").to_string()
        );
        assert!(error.contains("not available for large files"), "{error}");
        drop(workspace);
        remove_test_directory(directory);
    }
    #[test]
    fn save_copy_and_restore_closed_preserve_document_identity_and_history() {
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() && !workspace.editors.iter().any(|editor| editor.busy()) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        let root = std::env::temp_dir().join(format!(
            "bareline-copy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        for paged in [false, true] {
            let original = root.join(if paged { "paged.txt" } else { "resident.txt" });
            let original_text = if paged {
                "abcdef\n".repeat(1_000)
            } else {
                "abcdef".into()
            };
            std::fs::write(&original, &original_text).unwrap();
            let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            if paged {
                workspace.resident_max_bytes = 4;
            }
            workspace.open(original.clone());
            settle(&mut workspace);
            if paged {
                assert_ne!(
                    workspace.editors[0].document_identity(),
                    workspace.editors[0].snapshot().identity_token(),
                    "the paged viewport must not supply save-preflight identity"
                );
            }
            workspace.editors[0].enqueue(Input::Insert("X".into()));
            settle(&mut workspace);
            let copy = root.join(if paged { "paged-copy.txt" } else { "resident-copy.txt" });
            let copy_document = workspace.editors[0].document_identity();
            if paged {
                workspace.editors[0].scroll(10_000.0, 400.0);
                settle(&mut workspace);
                assert_eq!(workspace.editors[0].document_identity(), copy_document);
            }
            workspace.save_prepared(
                0,
                PreparedDestination {
                    path: copy.clone(),
                    condition: DestinationCondition::MustBeAbsent,
                    consent: DestinationConsent::NotRequired,
                    document: copy_document,
                    operation: SaveOperation::SaveCopy,
                },
            );
            settle(&mut workspace);
            assert_eq!(std::fs::read_to_string(copy).unwrap(), format!("X{original_text}"));
            assert_eq!(std::fs::read_to_string(&original).unwrap(), original_text);
            assert!(workspace.editors[0].dirty());
            assert_eq!(workspace.path(0), Some(original.as_path()));
            let alias = root.join(if paged { "paged-alias.txt" } else { "resident-alias.txt" });
            std::fs::hard_link(&original, &alias).unwrap();
            workspace.save_copy(0, alias);
            settle(&mut workspace);
            assert_eq!(std::fs::read_to_string(&original).unwrap(), original_text);
            assert!(workspace.editors[0].dirty());
            let selection = workspace.editors[0].viewport().selection;
            let mut renderer = bareline_renderer_recording::RecordingBackend::default();
            workspace.close(0, true, &mut renderer).unwrap();
            assert_eq!(
                workspace.recent_paths()[0],
                bareline_platform::SerializedPath::from_native(&original)
            );
            let mut next = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            next.restore_recent_paths(workspace.recent_paths());
            assert_eq!(next.recent_paths(), workspace.recent_paths());
            assert!(workspace.can_restore_closed());
            assert_eq!(workspace.restore_last_closed(), Some(0));
            assert_eq!(workspace.editors[0].viewport().selection, selection);
            workspace.editors[0].enqueue(Input::Undo);
            settle(&mut workspace);
            assert!(!workspace.editors[0].dirty());
        }
        for paged in [false, true] {
            let source = root.join(if paged {
                "paged-overwrite-source.txt"
            } else {
                "resident-overwrite-source.txt"
            });
            let target = root.join(if paged {
                "paged-overwrite-target.txt"
            } else {
                "resident-overwrite-target.txt"
            });
            std::fs::write(&source, "abcdef").unwrap();
            std::fs::write(&target, "older destination").unwrap();
            let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            if paged {
                workspace.resident_max_bytes = 4;
            }
            workspace.open(source);
            settle(&mut workspace);
            workspace.editors[0].enqueue(Input::Insert("X".into()));
            settle(&mut workspace);
            let prepared = bareline_file_io::lifecycle::preflight_destination(
                &target,
                None,
                workspace.editors[0].document_identity(),
                SaveOperation::SaveAs,
                &PagedFileSystem,
                &bareline_file_io::cancellation::Cancellation::default(),
            )
            .unwrap()
            .approve(true)
            .unwrap();
            let prepared_path = prepared.path.clone();
            assert_eq!(prepared_path, std::fs::canonicalize(&target).unwrap());
            workspace.save_prepared(0, prepared);
            settle(&mut workspace);
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "Xabcdef");
            assert_eq!(workspace.path(0), Some(prepared_path.as_path()));
            assert!(!workspace.editors[0].dirty());
        }
        remove_test_directory(root);
    }
    #[test]
    fn stale_prepared_destination_rejects_content_revision_for_resident_and_paged() {
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() && !workspace.editors.iter().any(|editor| editor.busy()) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }

        let root = std::env::temp_dir().join(format!(
            "bareline-stale-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        for paged in [false, true] {
            let source = root.join(if paged { "paged.txt" } else { "resident.txt" });
            let target = root.join(if paged { "paged-copy.txt" } else { "resident-copy.txt" });
            let source_text = if paged {
                "abcdef\n".repeat(1_000)
            } else {
                "abcdef".into()
            };
            std::fs::write(&source, &source_text).unwrap();
            let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            if paged {
                workspace.resident_max_bytes = 4;
            }
            workspace.open(source.clone());
            settle(&mut workspace);
            let captured_document = workspace.editors[0].document_identity();
            let stale = PreparedDestination {
                path: target.clone(),
                condition: DestinationCondition::MustBeAbsent,
                consent: DestinationConsent::NotRequired,
                document: captured_document,
                operation: SaveOperation::SaveCopy,
            };

            workspace.editors[0].enqueue(Input::Insert("Y".into()));
            settle(&mut workspace);
            assert_ne!(workspace.editors[0].document_identity(), captured_document);
            assert!(workspace.editors[0].dirty());
            assert!(!workspace.save_prepared(0, stale));
            assert!(!target.exists());
            assert_eq!(std::fs::read_to_string(source).unwrap(), source_text);
            assert!(
                workspace
                    .message
                    .as_deref()
                    .is_some_and(|message| message.contains("expired"))
            );
        }
        remove_test_directory(root);
    }
    #[test]
    fn streamed_large_edit_undo_redo_restart_uses_compact_recipe_and_terminal_receipt() {
        use bareline_document::paged::{OwnedTextRange, SourceEdit, SourceTransactionPoll};
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "bareline-large-actor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let original = root.join("input.txt");
        std::fs::write(&original, b"base").unwrap();
        let platform: Arc<dyn LocalFileSystem> = Arc::new(PagedFileSystem);
        let mut workspace = Workspace::new(Arc::new(|| {}), platform.clone()).unwrap();
        workspace.resident_max_bytes = 1;
        workspace.recovery_root = Some(root.join("recovery"));
        workspace.open(original);
        fn settle(workspace: &mut Workspace) {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                workspace.pump();
                if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                    break;
                }
                assert!(std::time::Instant::now() < until, "{:?}", workspace.message);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        settle(&mut workspace);
        let (captured, handle) = match &workspace.editors[0] {
            WorkspaceEditor::Paged(paged) => (paged.snapshot().clone(), paged.read_handle()),
            _ => panic!("paged expected"),
        };
        let cancel = bareline_file_io::cancellation::Cancellation::default();
        let options = bareline_file_io::source::SourceOptions {
            resident_max_bytes: 1,
            page_size_bytes: 65536,
            page_cache_bytes: 262144,
        };
        let mut stage = bareline_file_io::owned_store::StreamingStoreBuilder::new(
            &root,
            64 * 1024 * 1024,
            platform,
            options,
            workspace.source_edit_budget(),
            cancel.clone(),
        )
        .unwrap();
        let inverse = stage.append_utf8("base").unwrap();
        let chunk = [b'z'; 65536];
        for _ in 0..288 {
            stage.write_all(&chunk).unwrap();
        }
        let length = 18 * 1024 * 1024;
        let source = stage.finish().unwrap();
        let edits = vec![SourceEdit {
            range: bareline_document::TextOffset(0)..bareline_document::TextOffset(4),
            inverse: OwnedTextRange {
                source: source.clone(),
                range: inverse,
            },
            inserted: OwnedTextRange {
                source,
                range: 4..4 + length as u64,
            },
        }];
        let mut prepare = captured
            .prepare_source_transaction(edits, Default::default(), workspace.source_edit_budget())
            .unwrap();
        let prepared = loop {
            match prepare.poll() {
                SourceTransactionPoll::Ready(prepared) => break prepared,
                SourceTransactionPoll::Progress => {}
                SourceTransactionPoll::Pending(ticket) => {
                    if !prepare.resolve_owned(ticket).unwrap() {
                        assert!(handle.resolve_page(ticket).unwrap());
                    }
                }
                _ => panic!("source preparation failed"),
            }
        };
        let receipt = match &mut workspace.editors[0] {
            WorkspaceEditor::Paged(paged) => paged.apply_prepared_source_tracked(&captured, prepared).unwrap(),
            _ => unreachable!(),
        };
        settle(&mut workspace);
        assert_eq!(receipt.terminal(), Some(Ok(bareline_document::Revision(1))));
        assert_eq!(receipt.terminal(), Some(Ok(bareline_document::Revision(1))));
        workspace.editors[0].enqueue(Input::Undo);
        settle(&mut workspace);
        assert_eq!(
            workspace.editors[0]
                .snapshot()
                .chunks(bareline_document::TextOffset(0)..bareline_document::TextOffset(4))
                .unwrap()
                .collect::<String>(),
            "base"
        );
        workspace.editors[0].enqueue(Input::Redo);
        settle(&mut workspace);
        let directory = workspace.editors[0].recovery_status().directory.unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !workspace.editors[0].recovery_status().complete {
            workspace.pump();
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(std::fs::metadata(directory.join("root-3.json")).unwrap().len() < 4096);
        drop(prepare);
        drop(handle);
        drop(captured);
        drop(workspace);
        let mut restored = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        restored.restore_paged_recovery(directory);
        settle(&mut restored);
        assert_eq!(restored.editors.len(), 1, "{:?}", restored.message);
        let output = root.join("restored.txt");
        restored.save(0, output.clone());
        settle(&mut restored);
        assert_eq!(std::fs::metadata(output).unwrap().len(), length as u64);
        drop(restored);
        remove_test_directory(root);
    }
    #[test]
    fn paged_recovery_restart_preserves_opaque_undo_and_recovers_stale_pointer() {
        let root = std::env::temp_dir().join(format!(
            "bareline-paged-recovery-ui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let original = root.join("original.txt");
        let saved = root.join("restored.txt");
        let raw = [255, 254, 65, 0, 0, 216, 66, 0];
        std::fs::write(&original, raw).unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.resident_max_bytes = 4;
        workspace.recovery_root = Some(root.join("recovery"));
        workspace.open(original.clone());
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        settle(&mut workspace);
        workspace.editors[0].enqueue(Input::SetCaret(1, false));
        workspace.editors[0].enqueue(Input::SetCaret(4, true));
        workspace.editors[0].enqueue(Input::Insert("X".into()));
        settle(&mut workspace);
        let directory = match &workspace.editors[0] {
            WorkspaceEditor::Paged(editor) => editor.recovery_status().directory.unwrap(),
            _ => panic!("expected paged"),
        };
        let earlier_root = std::fs::read(directory.join("paged-root.json")).unwrap();
        workspace.editors[0].enqueue(Input::Undo);
        settle(&mut workspace);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let status = match &workspace.editors[0] {
                WorkspaceEditor::Paged(editor) => editor.recovery_status(),
                _ => unreachable!(),
            };
            assert!(status.error.is_none(), "{:?}", status.error);
            if status.complete {
                assert_eq!(status.durable.unwrap().revision, 2);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        drop(workspace);
        std::fs::remove_file(&original).unwrap();
        // Published recovery sources must survive cancellation of the completed
        // restore operation (IoTicket::drop), before any original page is cached.
        let restore_cancel = bareline_file_io::cancellation::Cancellation::default();
        let preview_budget = bareline_document::Budget::new(256 << 20);
        let mut preview = bareline_file_io::paged_recovery::restore(
            &directory,
            Arc::new(PagedFileSystem),
            preview_budget.clone(),
            bareline_document::Budget::new(0),
            &restore_cancel,
        )
        .unwrap();
        restore_cancel.cancel();
        assert_eq!(
            bareline_file_io::paged_recovery::preview(&mut preview, &preview_budget, &Default::default()).unwrap(),
            "A\u{fffd}B"
        );
        drop(preview);
        let mut restored = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        let restore_request = restored.restore_paged_recovery_tracked(directory.clone()).unwrap();
        settle(&mut restored);
        assert_eq!(restored.editors.len(), 1, "{:?}", restored.message);
        let restored_document = restored.editors[0].document_identity();
        assert_eq!(
            restored.take_recovery_restore_outcome(restore_request),
            Some(RecoveryRestoreOutcome::Restored {
                request_id: restore_request,
                document: restored_document,
            })
        );
        assert!(restored.editors[0].dirty());
        assert!(restored.path(0).is_none());
        assert!(restored.recent_paths().is_empty(), "recovery pseudo-path entered MRU");
        restored.save(0, saved.clone());
        settle(&mut restored);
        assert_eq!(std::fs::read(&saved).unwrap(), raw);
        drop(restored);
        let journal = std::fs::read(directory.join("journal.bin")).unwrap();
        let mut corrupt = journal.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        std::fs::write(directory.join("journal.bin"), corrupt).unwrap();
        let mut prefix = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        prefix.restore_paged_recovery(directory.clone());
        settle(&mut prefix);
        assert_eq!(prefix.editors.len(), 1, "{:?}", prefix.message);
        let prefix_path = root.join("valid-prefix.txt");
        prefix.save(0, prefix_path.clone());
        settle(&mut prefix);
        assert_eq!(std::fs::read(prefix_path).unwrap(), [255, 254, 65, 0, 88, 0, 66, 0]);
        drop(prefix);
        std::fs::write(directory.join("journal.bin"), journal).unwrap();
        std::fs::write(directory.join("paged-root.json"), earlier_root).unwrap();
        let retired_directory = directory.clone();
        let mut stale = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        stale.restore_paged_recovery(directory);
        settle(&mut stale);
        assert_eq!(stale.editors.len(), 1, "{:?}", stale.message);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match stale.close(0, true, &mut renderer) {
                Ok(()) => break,
                Err(CloseError::RecoveryPending) => {
                    stale.pump();
                }
                Err(error) => panic!("recovered close failed: {error:?}"),
            }
            assert!(std::time::Instant::now() < deadline, "{:?}", stale.message);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(stale.restore_last_closed(), Some(0));
        assert!(stale.recent_paths().is_empty(), "recovery pseudo-path entered MRU");
        stale.editors[0].enqueue(Input::Insert("Q".into()));
        settle(&mut stale);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let status = stale.editors[0].recovery_status();
            assert!(status.error.is_none(), "{:?}", status.error);
            if status.complete
                && status
                    .directory
                    .as_ref()
                    .is_some_and(|directory| directory != &retired_directory)
            {
                break;
            }
            stale.pump();
            assert!(
                std::time::Instant::now() < deadline,
                "reopened document did not establish recovery ownership"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        stale.editors[0].enqueue(Input::Undo);
        settle(&mut stale);
        let latest = root.join("latest-from-stale-pointer.txt");
        stale.save(0, latest.clone());
        settle(&mut stale);
        assert_eq!(std::fs::read(latest).unwrap(), raw);
        drop(stale);
        remove_test_directory(root);
    }
    #[test]
    fn resident_and_untitled_automatic_recovery_restore_current_text() {
        let root = std::env::temp_dir().join(format!(
            "bareline-resident-recovery-ui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() && !workspace.editors.iter().any(|editor| editor.busy()) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        for resident in [false, true] {
            let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            workspace.recovery_root = Some(root.join(if resident { "resident" } else { "untitled" }));
            let original = root.join("resident.txt");
            if resident {
                std::fs::write(&original, b"original").unwrap();
                workspace.open(original.clone());
                settle(&mut workspace);
            } else {
                workspace.new_document().unwrap();
                workspace.pump();
            }
            workspace.editors[0].enqueue(Input::Insert("draft ".into()));
            settle(&mut workspace);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let directory = loop {
                workspace.pump();
                let status = workspace.editors[0].recovery_status();
                assert!(status.error.is_none(), "{:?}", status.error);
                if status.complete {
                    assert!(status.durable.is_some());
                    break status.directory.unwrap();
                }
                assert!(std::time::Instant::now() < deadline, "recovery never completed");
                std::thread::sleep(std::time::Duration::from_millis(2));
            };
            drop(workspace);
            if resident {
                std::fs::remove_file(&original).unwrap();
            }
            let preview_budget = bareline_document::Budget::new(256 << 20);
            let mut preview = bareline_file_io::paged_recovery::restore(
                &directory,
                Arc::new(PagedFileSystem),
                preview_budget.clone(),
                bareline_document::Budget::new(0),
                &Default::default(),
            )
            .unwrap();
            assert_eq!(
                bareline_file_io::paged_recovery::preview(&mut preview, &preview_budget, &Default::default()).unwrap(),
                if resident { "draft original" } else { "draft " }
            );
            drop(preview);
            let mut restored = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            let restore_request = restored.restore_paged_recovery_tracked(directory).unwrap();
            assert!(
                restored.take_recovery_restore_outcome(restore_request).is_none(),
                "submission is not a recovered-text terminal"
            );
            settle(&mut restored);
            assert_eq!(restored.editors.len(), 1, "{:?}", restored.message);
            let restored_document = restored.editors[0].document_identity();
            assert_eq!(
                restored.take_recovery_restore_outcome(restore_request),
                Some(RecoveryRestoreOutcome::Restored {
                    request_id: restore_request,
                    document: restored_document,
                })
            );
            let saved = root.join(if resident {
                "resident-restored.txt"
            } else {
                "untitled-restored.txt"
            });
            restored.save(0, saved.clone());
            settle(&mut restored);
            assert_eq!(
                std::fs::read_to_string(saved).unwrap(),
                if resident { "draft original" } else { "draft " }
            );
            drop(restored);
        }
        remove_test_directory(root);
    }
    #[derive(Default)]
    struct PurgeGateFileSystem {
        armed: std::sync::atomic::AtomicBool,
        released: std::sync::atomic::AtomicBool,
        entered: std::sync::Mutex<bool>,
        wake: std::sync::Condvar,
    }
    impl PurgeGateFileSystem {
        fn arm(&self) {
            self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        fn wait_entered(&self) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut entered = self.entered.lock().unwrap();
            while !*entered {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                assert!(!remaining.is_zero(), "old recovery purge never reached the gate");
                let (next, timeout) = self.wake.wait_timeout(entered, remaining).unwrap();
                entered = next;
                assert!(!timeout.timed_out() || *entered);
            }
        }
        fn release(&self) {
            self.released.store(true, std::sync::atomic::Ordering::SeqCst);
            self.wake.notify_all();
        }
    }
    impl LocalFileSystem for PurgeGateFileSystem {
        fn cache_directory_guard(
            &self,
            path: &std::path::Path,
        ) -> std::io::Result<Option<bareline_platform::CacheDirectoryLease>> {
            PagedFileSystem.cache_directory_guard(path)
        }
        fn remove_owned_cache_directory(
            &self,
            root: &std::path::Path,
            candidate: &std::path::Path,
            root_identity: bareline_platform::CacheDirectoryIdentity,
            candidate_identity: bareline_platform::CacheDirectoryIdentity,
            proof_name: &str,
            proof_bytes: &[u8],
            max_entries: usize,
            max_time: std::time::Duration,
            cancelled: &dyn Fn() -> bool,
        ) -> bareline_platform::CacheRemovalOutcome {
            if self.armed.load(std::sync::atomic::Ordering::SeqCst) {
                let mut entered = self.entered.lock().unwrap();
                *entered = true;
                self.wake.notify_all();
                while !self.released.load(std::sync::atomic::Ordering::SeqCst) {
                    entered = self.wake.wait(entered).unwrap();
                }
            }
            PagedFileSystem.remove_owned_cache_directory(
                root,
                candidate,
                root_identity,
                candidate_identity,
                proof_name,
                proof_bytes,
                max_entries,
                max_time,
                cancelled,
            )
        }
        fn available_space(&self, path: &std::path::Path) -> std::io::Result<u64> {
            PagedFileSystem.available_space(path)
        }
        fn guard_directory(&self, path: &std::path::Path) -> std::io::Result<Arc<dyn Send + Sync>> {
            PagedFileSystem.guard_directory(path)
        }
        fn open_sealed_read(&self, path: &std::path::Path) -> std::io::Result<std::fs::File> {
            PagedFileSystem.open_sealed_read(path)
        }
        fn identity(&self, file: &std::fs::File) -> std::io::Result<bareline_platform::FileIdentity> {
            PagedFileSystem.identity(file)
        }
        fn validate_target(&self, path: &std::path::Path) -> std::io::Result<()> {
            PagedFileSystem.validate_target(path)
        }
        fn commit(&self, staged: &std::path::Path, target: &std::path::Path, existed: bool) -> std::io::Result<()> {
            PagedFileSystem.commit(staged, target, existed)
        }
    }
    #[test]
    fn small_recovery_restores_an_editable_untitled_document() {
        let root = std::env::temp_dir().join(format!(
            "bareline-recovered-resident-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.recovery_root = Some(root.join("recovery"));
        workspace.new_document().unwrap();
        workspace.pump();
        workspace.editors[0].enqueue(Input::Insert("recovered draft".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let directory = loop {
            workspace.pump();
            let status = workspace.editors[0].recovery_status();
            assert!(status.error.is_none(), "{:?}", status.error);
            if status.complete
                && status.durable.is_some()
                && let Some(directory) = status.directory
            {
                break directory;
            }
            assert!(std::time::Instant::now() < deadline, "recovery never became durable");
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        std::mem::forget(workspace);
        let old_directory = directory.clone();
        let gate = Arc::new(PurgeGateFileSystem::default());
        let mut restored = Workspace::new(Arc::new(|| {}), gate.clone()).unwrap();
        restored.restore_paged_recovery(directory);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            restored.pump();
            if restored.editors.first().is_some_and(|editor| editor.dirty()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "restore never produced an edited document: {:?}",
                restored.message
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(
            matches!(restored.editors[0], WorkspaceEditor::Resident(_)),
            "small journal did not restore into an in-memory editor"
        );
        assert!(!restored.editors[0].read_only());
        assert_eq!(restored.titles()[0], "Untitled 1 \u{2022}");
        assert!(restored.path(0).is_none(), "restored document must ask where to save");
        gate.arm();
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match restored.close(0, true, &mut renderer) {
                Ok(()) => break,
                Err(CloseError::RecoveryPending) => {
                    restored.pump();
                }
                Err(error) => panic!("recovered close failed: {error:?}"),
            }
            assert!(std::time::Instant::now() < deadline, "{:?}", restored.message);
            std::thread::yield_now();
        }
        gate.wait_entered();
        assert_eq!(restored.restore_last_closed(), Some(0));
        assert!(!restored.editors[0].read_only());
        restored.editors[0].enqueue(Input::Insert(" again".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let replacement = loop {
            restored.pump();
            let status = restored.editors[0].recovery_status();
            assert!(status.error.is_none(), "{:?}", status.error);
            if status.complete
                && let Some(directory) = status.directory
                && directory != old_directory
            {
                break directory;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "reopened resident document did not establish fresh recovery ownership"
            );
            std::thread::yield_now();
        };
        assert!(replacement.exists());
        gate.release();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while old_directory.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "old recovery cleanup remained blocked"
            );
            std::thread::yield_now();
        }
        assert!(
            replacement.exists(),
            "old cleanup deleted the reopened document checkpoint"
        );
        drop(restored);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn resident_clean_undo_and_save_retire_obsolete_recovery() {
        let root = std::env::temp_dir().join(format!(
            "bareline-clean-recovery-ui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.recovery_root = Some(root.join("recovery"));
        workspace.new_document().unwrap();
        workspace.pump();
        fn wait(workspace: &mut Workspace, empty: bool) -> Option<PathBuf> {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                let state = workspace.editors[0].recovery_status();
                assert!(state.error.is_none(), "{:?}", state.error);
                if (empty && state.directory.is_none()) || (!empty && state.complete) {
                    return state.directory;
                }
                assert!(std::time::Instant::now() < deadline, "recovery state did not settle");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        workspace.editors[0].enqueue(Input::Insert("abandoned".into()));
        let first = wait(&mut workspace, false).unwrap();
        workspace.editors[0].enqueue(Input::Undo);
        wait(&mut workspace, true);
        assert!(!first.exists(), "retired checkpoint directory was left on disk");
        workspace.editors[0].enqueue(Input::Insert("saved".into()));
        let second = wait(&mut workspace, false).unwrap();
        workspace.save(0, root.join("saved.txt"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        wait(&mut workspace, true);
        assert!(!second.exists(), "retired checkpoint directory was left on disk");
        drop(workspace);
        remove_test_directory(root);
    }
    struct RetirementGate {
        entered: std::sync::mpsc::SyncSender<()>,
        released: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    }
    impl RetirementGate {
        fn wait(&self) {
            let _ = self.entered.try_send(());
            let (released, wake) = &*self.released;
            let mut released = released.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
        }
    }
    struct RetirementRelease(std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);
    impl Drop for RetirementRelease {
        fn drop(&mut self) {
            let (released, wake) = &*self.0;
            *released.lock().unwrap_or_else(|error| error.into_inner()) = true;
            wake.notify_all();
        }
    }
    struct RetirementFileSystem {
        deny: std::sync::Arc<std::sync::atomic::AtomicBool>,
        retirement_gate: Option<RetirementGate>,
        save_gate: Option<(std::ffi::OsString, RetirementGate)>,
    }
    impl LocalFileSystem for RetirementFileSystem {
        fn cache_directory_guard(
            &self,
            path: &std::path::Path,
        ) -> std::io::Result<Option<bareline_platform::CacheDirectoryLease>> {
            PagedFileSystem.cache_directory_guard(path)
        }
        fn remove_owned_cache_directory(
            &self,
            root: &std::path::Path,
            candidate: &std::path::Path,
            root_identity: bareline_platform::CacheDirectoryIdentity,
            candidate_identity: bareline_platform::CacheDirectoryIdentity,
            proof_name: &str,
            proof_bytes: &[u8],
            max_entries: usize,
            max_time: std::time::Duration,
            cancelled: &dyn Fn() -> bool,
        ) -> bareline_platform::CacheRemovalOutcome {
            PagedFileSystem.remove_owned_cache_directory(
                root,
                candidate,
                root_identity,
                candidate_identity,
                proof_name,
                proof_bytes,
                max_entries,
                max_time,
                cancelled,
            )
        }
        fn available_space(&self, path: &std::path::Path) -> std::io::Result<u64> {
            PagedFileSystem.available_space(path)
        }
        fn guard_directory(&self, path: &std::path::Path) -> std::io::Result<Arc<dyn Send + Sync>> {
            PagedFileSystem.guard_directory(path)
        }
        fn open_sealed_read(&self, path: &std::path::Path) -> std::io::Result<std::fs::File> {
            PagedFileSystem.open_sealed_read(path)
        }
        fn identity(&self, file: &std::fs::File) -> std::io::Result<bareline_platform::FileIdentity> {
            PagedFileSystem.identity(file)
        }
        fn validate_target(&self, path: &std::path::Path) -> std::io::Result<()> {
            PagedFileSystem.validate_target(path)
        }
        fn prepare_commit(
            &self,
            staged: &std::path::Path,
            target: &std::path::Path,
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
        fn commit(&self, staged: &std::path::Path, target: &std::path::Path, existed: bool) -> std::io::Result<()> {
            if let Some((name, gate)) = &self.save_gate
                && target.file_name() == Some(name.as_os_str())
            {
                gate.wait();
            }
            if target.file_name().is_some_and(|name| name == "retired.json") {
                if let Some(gate) = &self.retirement_gate {
                    gate.wait();
                }
                if self.deny.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(std::io::Error::other("injected retirement failure"));
                }
            }
            PagedFileSystem.commit(staged, target, existed)
        }
    }
    #[test]
    fn paged_save_publishes_terminal_before_recovery_retirement_finishes() {
        let root = std::env::temp_dir().join(format!(
            "bareline-save-terminal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.txt");
        let saved = root.join("saved.txt");
        std::fs::write(&source, b"abcde").unwrap();
        let (entered, retirement_entered) = std::sync::mpsc::sync_channel(1);
        let released = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let release_retirement = RetirementRelease(released.clone());
        let platform = Arc::new(RetirementFileSystem {
            deny: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            retirement_gate: Some(RetirementGate {
                entered,
                released: released.clone(),
            }),
            save_gate: None,
        });
        let mut workspace = Workspace::new(Arc::new(|| {}), platform).unwrap();
        workspace.resident_max_bytes = 4;
        workspace.recovery_root = Some(root.join("recovery"));
        workspace.open(source);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            workspace.pump();
            if workspace.editors.len() == 1 && !workspace.io_busy() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        workspace.editors[0].enqueue(Input::Insert("X".into()));
        loop {
            workspace.pump();
            let status = workspace.editors[0].recovery_status();
            assert!(status.error.is_none(), "{:?}", status.error);
            if status.complete && !workspace.editors[0].busy() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(workspace.save(0, saved.clone()));
        assert_eq!(workspace.message.as_deref(), Some("Saving…"));
        retirement_entered
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("save committed but retirement did not reach its controlled hold");

        workspace.message = Some("A newer status".into());
        workspace.pump();
        assert_eq!(std::fs::read(&saved).unwrap(), b"Xabcde");
        assert_eq!(workspace.message.as_deref(), Some("A newer status"));
        assert!(
            workspace.editors[0].busy(),
            "retirement hold must keep close safety busy"
        );

        drop(release_retirement);
        while workspace.io_busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        drop(workspace);
        remove_test_directory(root);
    }
    #[test]
    fn late_paged_terminal_does_not_clear_resident_save_banner() {
        let root = std::env::temp_dir().join(format!(
            "bareline-save-terminal-owner-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.txt");
        let paged_target = root.join("paged.txt");
        let resident_target = root.join("resident.txt");
        std::fs::write(&source, b"abcde").unwrap();
        let (entered, save_entered) = std::sync::mpsc::sync_channel(1);
        let released = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let release_save = RetirementRelease(released.clone());
        let platform = Arc::new(RetirementFileSystem {
            deny: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            retirement_gate: None,
            save_gate: Some((
                resident_target.file_name().unwrap().to_os_string(),
                RetirementGate { entered, released },
            )),
        });
        let mut workspace = Workspace::new(Arc::new(|| {}), platform).unwrap();
        workspace.resident_max_bytes = 4;
        workspace.recovery_root = Some(root.join("recovery"));
        workspace.open(source);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            workspace.pump();
            if workspace.editors.len() == 1 && !workspace.io_busy() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        workspace.editors[0].enqueue(Input::Insert("X".into()));
        loop {
            workspace.pump();
            let status = workspace.editors[0].recovery_status();
            if status.complete && !workspace.editors[0].busy() {
                break;
            }
            assert!(status.error.is_none(), "{:?}", status.error);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        workspace.new_document().unwrap();
        workspace.editors[1].enqueue(Input::Insert("resident".into()));
        while workspace.editors[1].busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }

        assert!(workspace.save(1, resident_target));
        save_entered
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("resident save did not reach its controlled hold");
        assert!(workspace.save(0, paged_target.clone()));
        while !workspace.pending_paged_saves.is_empty() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        assert_eq!(std::fs::read(&paged_target).unwrap(), b"Xabcde");
        assert_eq!(workspace.message.as_deref(), Some("Saving…"));
        assert!(workspace.pending_io.iter().any(|pending| pending.save.is_some()));

        drop(release_save);
        while workspace.io_busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(workspace.message, None);
        drop(workspace);
        remove_test_directory(root);
    }
    #[test]
    fn paged_recovery_failed_retirement_is_retryable() {
        let root = std::env::temp_dir().join(format!(
            "bareline-retire-retry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let original = root.join("original.txt");
        std::fs::write(&original, "abcde").unwrap();
        let deny = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut workspace = Workspace::new(
            Arc::new(|| {}),
            Arc::new(RetirementFileSystem {
                deny: deny.clone(),
                retirement_gate: None,
                save_gate: None,
            }),
        )
        .unwrap();
        workspace.resident_max_bytes = 4;
        workspace.recovery_root = Some(root.join("recovery"));
        workspace.open(original);
        fn settle(workspace: &mut Workspace) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                workspace.pump();
                if !workspace.io_busy() {
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        settle(&mut workspace);
        workspace.editors[0].enqueue(Input::Insert("X".into()));
        settle(&mut workspace);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let directory = loop {
            let status = workspace.editors[0].recovery_status();
            assert!(status.error.is_none(), "{:?}", status.error);
            if status.complete {
                break status.directory.unwrap();
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        deny.store(true, std::sync::atomic::Ordering::SeqCst);
        let saved = root.join("saved.txt");
        workspace.save(0, saved.clone());
        settle(&mut workspace);
        assert_eq!(std::fs::read_to_string(saved).unwrap(), "Xabcde");
        assert!(workspace.editors[0].recovery_status().error.is_some());
        assert_ne!(
            bareline_file_io::recovery::inspect(&directory, &Default::default())
                .unwrap()
                .status,
            bareline_file_io::recovery::RecoveryStatus::Discarded
        );
        deny.store(false, std::sync::atomic::Ordering::SeqCst);
        workspace.editors[0].retry_recovery().unwrap();
        settle(&mut workspace);
        assert!(workspace.editors[0].recovery_status().error.is_none());
        assert_eq!(
            bareline_file_io::recovery::inspect(&directory, &Default::default())
                .unwrap()
                .status,
            bareline_file_io::recovery::RecoveryStatus::Discarded
        );
        drop(workspace);
        remove_test_directory(root);
    }
    struct StreamingFileSystem {
        calls: std::sync::atomic::AtomicUsize,
        gate: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl LocalFileSystem for StreamingFileSystem {
        fn identity(&self, file: &std::fs::File) -> std::io::Result<bareline_platform::FileIdentity> {
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 3 {
                self.gate
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
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
        fn validate_target(&self, _: &std::path::Path) -> std::io::Result<()> {
            Ok(())
        }
        fn commit(&self, _: &std::path::Path, _: &std::path::Path, _: bool) -> std::io::Result<()> {
            unreachable!()
        }
    }
    #[test]
    fn streaming_prefix_is_read_only_then_same_tab_becomes_complete() {
        let path = std::env::temp_dir().join(format!(
            "bareline-ui-stream-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let content = "line 🦀\n".repeat(160_000);
        std::fs::write(&path, &content).unwrap();
        let (release, gate) = std::sync::mpsc::channel();
        let (sent, received) = std::sync::mpsc::channel();
        let mut workspace = Workspace::new(
            Arc::new(move || {
                let _ = sent.send(());
            }),
            Arc::new(StreamingFileSystem {
                calls: Default::default(),
                gate: std::sync::Mutex::new(gate),
            }),
        )
        .unwrap();
        workspace.open(path.clone());
        received.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        workspace.pump();
        assert_eq!(workspace.editors.len(), 1);
        let prefix = workspace.editors[0].snapshot().clone();
        assert!(!prefix.is_complete());
        assert!(workspace.editors[0].read_only());
        workspace.editors[0].enqueue(Input::Insert("forbidden".into()));
        workspace.editors[0].enqueue(Input::Undo);
        assert_eq!(workspace.editors[0].snapshot().revision, prefix.revision);
        assert!(!workspace.editors[0].busy());
        workspace.save(0, path.clone());
        assert_eq!(workspace.pending_io.len(), 1);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut ops = Vec::new();
        workspace.draw(0, &mut renderer, 1100.0, 700.0, &mut ops).unwrap();
        assert!(
            ops.iter().any(
                |op| matches!(op, DrawOp::Text { text, .. } if text.contains("Line numbers estimated · indexing"))
            )
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, DrawOp::Text { text, .. } if text == "~"))
        );
        release.send(()).unwrap();
        while workspace.io_busy() {
            received.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        assert_eq!(workspace.editors.len(), 1);
        assert!(workspace.editors[0].snapshot().is_complete());
        assert!(!workspace.editors[0].read_only());
        assert!(workspace.editors[0].snapshot().same_document(&prefix));
        assert_eq!(workspace.editors[0].snapshot().len(), content.len());
        assert_eq!(workspace.path(0), Some(path.as_path()));
        ops.clear();
        workspace.draw(0, &mut renderer, 1100.0, 700.0, &mut ops).unwrap();
        assert!(
            ops.iter()
                .all(|op| !matches!(op, DrawOp::Text { text, .. } if text.contains("estimated") || text == "~"))
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, DrawOp::Text { text, .. } if text.starts_with("Tab: ")))
        );
        std::fs::remove_file(path).unwrap();
    }
    struct GatedFileSystem(std::sync::Mutex<std::sync::mpsc::Receiver<()>>);
    impl LocalFileSystem for GatedFileSystem {
        fn identity(&self, _: &std::fs::File) -> std::io::Result<bareline_platform::FileIdentity> {
            unreachable!()
        }
        fn validate_target(&self, _: &std::path::Path) -> std::io::Result<()> {
            self.0
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            Err(std::io::ErrorKind::PermissionDenied.into())
        }
        fn commit(&self, _: &std::path::Path, _: &std::path::Path, _: bool) -> std::io::Result<()> {
            unreachable!()
        }
    }
    #[test]
    fn close_requires_discard_and_preserves_pending_save_target_after_tab_removal() {
        let (release, gate) = std::sync::mpsc::channel();
        let (notifier, notified) = std::sync::mpsc::channel();
        let mut workspace = Workspace::new(
            Arc::new(move || {
                let _ = notifier.send(());
            }),
            Arc::new(GatedFileSystem(std::sync::Mutex::new(gate))),
        )
        .unwrap();
        for _ in 0..3 {
            workspace.new_document().unwrap();
        }
        workspace.editors[0].enqueue(Input::Insert("unsaved".into()));
        while workspace.editors[0].busy() {
            notified.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        assert_eq!(workspace.close(0, false, &mut renderer), Err(CloseError::Unsaved));
        let saving = workspace.editors[2].snapshot().clone();
        workspace.save(2, PathBuf::from("unused-fixture.txt"));
        assert_eq!(workspace.close(2, true, &mut renderer), Err(CloseError::Busy));
        workspace.close(0, true, &mut renderer).unwrap();
        assert_eq!(workspace.titles()[0], "Untitled 2");
        assert!(workspace.editors[1].snapshot().same_document(&saving));
        assert_eq!(workspace.pending_io[0].save.as_ref().unwrap().0, 1);
        assert_eq!(workspace.close(1, true, &mut renderer), Err(CloseError::Busy));
        release.send(()).unwrap();
        while workspace.io_busy() {
            notified.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        assert!(workspace.message.as_ref().unwrap().contains("failed"));
        assert!(workspace.editors[1].snapshot().same_document(&saving));
    }
    #[test]
    fn find_is_worker_backed_and_navigation_preserves_document_revision() {
        let (_release, gate) = std::sync::mpsc::channel();
        let (notifier, notified) = std::sync::mpsc::channel();
        let mut workspace = Workspace::new(
            Arc::new(move || {
                let _ = notifier.send(());
            }),
            Arc::new(GatedFileSystem(std::sync::Mutex::new(gate))),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("one Straße STRASSE".into()));
        while workspace.editors[0].busy() {
            notified.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        let revision = workspace.editors[0].snapshot().revision;
        workspace.find.show();
        workspace.find.field.insert("strasse");
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut operations = Vec::new();
        workspace
            .draw(0, &mut renderer, 1100.0, 700.0, &mut operations)
            .unwrap();
        assert!(bareline_renderer::balanced_clips(&operations));
        assert_eq!(workspace.editors[0].viewport().top_inset, crate::find::HEIGHT);
        while workspace.find.status == "Searching…" {
            notified.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        workspace.editors[0].enqueue(Input::SetCaret(0, false));
        workspace.find_next(0, false);
        assert_eq!(
            (
                workspace.editors[0].viewport().selection.anchor,
                workspace.editors[0].viewport().selection.caret
            ),
            (4, 11)
        );
        assert_eq!(workspace.editors[0].snapshot().revision, revision);
        assert!(
            workspace.acknowledged_search_commands.is_empty(),
            "dispatch is not acknowledgment"
        );
        let commands = workspace.take_acknowledged_commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "search.find_next");
        assert_eq!(commands[0].1.get("pattern").map(String::as_str), Some("strasse"));
        workspace.find_next(0, false);
        assert_eq!(
            (
                workspace.editors[0].viewport().selection.anchor,
                workspace.editors[0].viewport().selection.caret
            ),
            (12, 19)
        );
        workspace.find.hide();
        workspace.find_next(0, false);
        assert_eq!(workspace.editors[0].viewport().selection.anchor, 4);
        assert_eq!(workspace.editors[0].viewport().selection.caret, 11);
        assert_eq!(workspace.take_acknowledged_commands().len(), 2);
        workspace.find_next(0, false);
        workspace.editors[0].enqueue(Input::SetCaret(0, false));
        assert!(
            workspace.take_acknowledged_commands().is_empty(),
            "superseded selection cannot record success"
        );
        operations.clear();
        workspace
            .draw(0, &mut renderer, 1100.0, 700.0, &mut operations)
            .unwrap();
        assert_eq!(workspace.editors[0].viewport().top_inset, 0.0);
    }

    #[test]
    fn find_results_and_replace_actions_follow_the_bound_document_generation() {
        let (_release, gate) = std::sync::mpsc::channel();
        let (notifier, notified) = std::sync::mpsc::channel();
        let mut workspace = Workspace::new(
            Arc::new(move || {
                let _ = notifier.send(());
            }),
            Arc::new(GatedFileSystem(std::sync::Mutex::new(gate))),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("needle needle".into()));
        workspace.editors[1].enqueue(Input::Insert("nothing here".into()));
        while workspace.editors.iter().any(WorkspaceEditor::busy) {
            notified.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        workspace.find.show_replace();
        workspace.find.field.insert("needle");
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut operations = Vec::new();
        workspace
            .draw(0, &mut renderer, 1100.0, 700.0, &mut operations)
            .unwrap();
        while workspace.find.searching() {
            notified.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        assert_eq!(workspace.find.completed_results().unwrap().count(), 2);
        workspace.find.case_sensitive = true;
        assert!(workspace.find.completed_results().is_none());
        assert!(
            workspace
                .find
                .semantics(1100.0)
                .into_iter()
                .find(|node| node.name == "Replace All")
                .unwrap()
                .disabled
        );
        workspace.find.case_sensitive = false;
        assert_eq!(workspace.find.completed_results().unwrap().count(), 2);
        assert!(
            !workspace
                .find
                .semantics(1100.0)
                .into_iter()
                .find(|node| node.name == "Replace All")
                .unwrap()
                .disabled
        );

        workspace.bind_find_to(1);
        assert_eq!(workspace.find.status, "Searching…");
        assert!(workspace.find.completed_results().is_none());
        assert!(
            workspace
                .find
                .semantics(1100.0)
                .into_iter()
                .find(|node| node.name == "Replace All")
                .unwrap()
                .disabled
        );

        operations.clear();
        workspace
            .draw(1, &mut renderer, 1100.0, 700.0, &mut operations)
            .unwrap();
        workspace.bind_find_to(0);
        operations.clear();
        workspace
            .draw(0, &mut renderer, 1100.0, 700.0, &mut operations)
            .unwrap();
        while workspace.find.searching() {
            notified.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        assert_eq!(workspace.find.completed_results().unwrap().count(), 2);

        let source_before_detached_replace = workspace.editors[0].snapshot().content_state;
        workspace.find.replacement.insert("replacement");
        workspace.replace(0, true);
        assert!(workspace.pending_replace.is_some());
        workspace.bind_find_to(1);
        assert!(workspace.pending_replace.is_none());
        workspace.pump();
        assert_eq!(
            workspace.editors[0].snapshot().content_state,
            source_before_detached_replace
        );
        workspace.bind_find_to(0);
        operations.clear();
        workspace
            .draw(0, &mut renderer, 1100.0, 700.0, &mut operations)
            .unwrap();
        while workspace.find.searching() {
            notified.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }

        workspace.editors[0].enqueue(Input::Insert(" changed".into()));
        while workspace.editors[0].busy() {
            notified.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        workspace.bind_find_to(0);
        assert_eq!(workspace.find.status, "Results changed; search again");
        assert!(workspace.find.completed_results().is_none());
        assert!(
            workspace
                .find
                .semantics(1100.0)
                .into_iter()
                .find(|node| node.name == "Replace All")
                .unwrap()
                .disabled
        );
    }
    #[test]
    fn replace_all_is_one_undo_and_cancelled_preparation_cannot_mutate() {
        let (_release, gate) = std::sync::mpsc::channel();
        let (notifier, notified) = std::sync::mpsc::channel();
        let mut workspace = Workspace::new(
            Arc::new(move || {
                let _ = notifier.send(());
            }),
            Arc::new(GatedFileSystem(std::sync::Mutex::new(gate))),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("Straße STRASSE".into()));
        let drain = |workspace: &mut Workspace| {
            while workspace.pending_replace.is_some()
                || workspace.editors[0].busy()
                || workspace.find.status == "Searching…"
            {
                workspace.pump();
                if workspace.pending_replace.is_some()
                    || workspace.editors[0].busy()
                    || workspace.find.status == "Searching…"
                {
                    notified.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
                }
            }
        };
        drain(&mut workspace);
        workspace.find.show_replace();
        workspace.find.field.insert("strasse");
        workspace.find.replacement.insert("road");
        workspace
            .find
            .refresh(workspace.editors[0].snapshot(), workspace.notify.clone());
        drain(&mut workspace);
        workspace.replace(0, true);
        workspace.cancel_search();
        workspace.pump();
        assert_eq!(workspace.editors[0].snapshot().len(), "Straße STRASSE".len());
        workspace.find.show_replace();
        workspace
            .find
            .refresh(workspace.editors[0].snapshot(), workspace.notify.clone());
        drain(&mut workspace);
        workspace.replace(0, true);
        drain(&mut workspace);
        let snapshot = workspace.editors[0].snapshot();
        assert_eq!(
            snapshot
                .read(
                    bareline_document::TextOffset(0)..bareline_document::TextOffset(snapshot.len()),
                    1024
                )
                .unwrap(),
            "road road"
        );
        workspace.editors[0].enqueue(Input::Undo);
        drain(&mut workspace);
        let snapshot = workspace.editors[0].snapshot();
        assert_eq!(
            snapshot
                .read(
                    bareline_document::TextOffset(0)..bareline_document::TextOffset(snapshot.len()),
                    1024
                )
                .unwrap(),
            "Straße STRASSE"
        );
        workspace.find.field.select_all();
        workspace.find.field.insert("changed query");
        workspace.replace(0, true);
        assert!(workspace.pending_replace.is_none());
    }

    #[test]
    fn tracked_recovery_restore_results_are_selective_and_bounded() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        for request_id in 1..=32 {
            workspace.record_recovery_restore(Some(request_id), Err(format!("failure {request_id}")));
        }
        assert_eq!(
            workspace.take_recovery_restore_outcome(17),
            Some(RecoveryRestoreOutcome::Failed {
                request_id: 17,
                error: "failure 17".into(),
            })
        );
        assert!(workspace.take_recovery_restore_outcome(17).is_none());
        assert_eq!(workspace.recovery_restore_outcomes.len(), 31);
        workspace.record_recovery_restore(Some(33), Err("failure 33".into()));
        assert!(
            workspace
                .restore_paged_recovery_tracked(PathBuf::from("checkpoint-over-capacity"))
                .is_err()
        );
        assert_eq!(workspace.recovery_restore_outcomes.len(), 32);
    }

    #[test]
    fn tracked_recovery_restore_worker_failure_publishes_a_consumable_terminal() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        let request_id = workspace
            .restore_paged_recovery_tracked(std::env::temp_dir().join(format!(
                "bareline-missing-recovery-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let outcome = loop {
            workspace.pump();
            if let Some(outcome) = workspace.take_recovery_restore_outcome(request_id) {
                break outcome;
            }
            assert!(std::time::Instant::now() < deadline, "tracked restore stayed pending");
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        assert!(matches!(
            outcome,
            RecoveryRestoreOutcome::Failed {
                request_id: failed,
                ref error,
            } if failed == request_id && !error.is_empty()
        ));
        assert!(workspace.take_recovery_restore_outcome(request_id).is_none());
    }
}

pub mod extensions;
