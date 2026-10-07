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
use std::path::PathBuf;
use std::sync::Arc;

mod encoding;
mod lifecycle;
mod new_document;
mod pump_editors;
mod pump_io;
mod pump_open;
mod pump_recovery;
mod pump_reload;
mod pump_replace;
mod pump_save;
mod pump_spill;
mod pump_transcode;
mod remote;
mod tabs;
pub use bareline_file_io::codecs::failure::EncodingFailure;
pub use lifecycle::{FileLifecycle, SaveOrigin};
use lifecycle::{LifecycleEvent, LifecycleRefusal, TabFacts};
use tabs::TabSlot;
pub use tabs::{TabEvent, TabId, TabKey};
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
    Reopen(Reopen),
}
/// A clean closed tab reopens from its path as a new document, which takes over
/// the closed document's tab (pin, position, view) and read-only choice (WSP-05).
struct Reopen {
    path: PathBuf,
    document: u64,
    read_only: bool,
}
/// Request ids of explicit opens whose tab becomes active when it appears
/// (APP-07). Launch requests count up from 1 and the shell's conflict opens
/// start at 2^63, so this range is never shared.
const ACTIVATION_REQUESTS: std::ops::Range<u64> = (1 << 62)..(1 << 63);
/// An explicit open waiting to show its tab.
struct ActivationRequest {
    request: u64,
    path: PathBuf,
    /// The document id of the tab already shown for it, loading or loaded.
    shown: Option<u64>,
    /// Whether that tab was removed so the result lands in another tab.
    displaced: bool,
    /// Closed document id and read-only choice of a restored closed tab.
    reopen: Option<(u64, bool)>,
}
/// The tab an explicit request asks the shell to activate.
struct Activation {
    document: u64,
    /// The request's loading tab that was already shown and has since been
    /// replaced by a different tab: the shell activates `document` only while
    /// the user still has that loading tab active (APP-07).
    from: Option<u64>,
}
impl ClosedDocument {
    fn paged(&self) -> bool {
        matches!(self, Self::Retained(editor, ..) if editor.paged())
    }
    /// Text a retained model keeps resident (a paged model counts its window).
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Retained(editor, ..) => editor.snapshot().len(),
            Self::Reopen(_) => 0,
        }
    }
    fn retains(&self, identity: (u64, u64)) -> bool {
        matches!(self, Self::Retained(editor, ..) if editor.snapshot().identity_token() == identity)
    }
}
/// A closed, saved document whose file is being checked on a worker. The tab
/// close never stats the path on the UI thread, where a disconnected share can
/// block for a minute (APP-19); the model is released once the file is found.
struct ClosedCheck {
    identity: (u64, u64),
    path: PathBuf,
    /// `None` while queued behind the check in flight. Checks run one at a
    /// time so a disconnected share cannot occupy every pool worker.
    task: Option<crate::task::Task<bool>>,
}
/// Whether a closed document's file is still on disk; runs on a worker only.
type ClosedPathProbe = Arc<dyn Fn(&std::path::Path) -> bool + Send + Sync>;
const MAX_CLOSED_DOCUMENTS: usize = 20;
/// The message bar's band above the status strip (or the bottom panel): a
/// 30-pixel bar and its gap. The text view ends above it.
const MESSAGE_BAND: f32 = 34.0;
const MAX_OPEN_OUTCOMES: usize = 256;
/// Unresolved save conflicts and cleanups stay listed (their files remain on disk),
/// but a burst of failures cannot grow the lists without bound.
const MAX_SAVE_ISSUES: usize = 256;
/// Replace the entry for the same transaction, or append and evict the oldest.
fn upsert_save_issue<T>(issues: &mut Vec<T>, issue: T, transaction: impl Fn(&T) -> &PathBuf) {
    if let Some(existing) = issues
        .iter_mut()
        .find(|known| transaction(&**known) == transaction(&issue))
    {
        *existing = issue;
        return;
    }
    if issues.len() >= MAX_SAVE_ISSUES {
        issues.remove(0);
    }
    issues.push(issue);
}
/// Collects the layout ids an editor releases outside a draw; it shapes nothing.
#[derive(Default)]
struct RetiredLayouts(Vec<bareline_renderer::LayoutId>);
impl TextBackend for RetiredLayouts {
    fn layout_size(&self, _: bareline_renderer::LayoutId) -> Result<(f32, f32), LayoutError> {
        Err(LayoutError::InvalidHandle)
    }
    fn shape(&mut self, _: &str, _: f32, _: f32) -> Result<bareline_renderer::LayoutId, LayoutError> {
        Err(LayoutError::BackendFailure)
    }
    fn set_styles(
        &mut self,
        _: bareline_renderer::LayoutId,
        _: &[bareline_renderer::TextStyle],
    ) -> Result<(), LayoutError> {
        Err(LayoutError::InvalidHandle)
    }
    fn hit_test(
        &self,
        _: bareline_renderer::LayoutId,
        _: bareline_renderer::Point,
    ) -> Result<bareline_renderer::TextHit, LayoutError> {
        Err(LayoutError::InvalidHandle)
    }
    fn caret(&self, _: bareline_renderer::LayoutId, _: usize) -> Result<Rect, LayoutError> {
        Err(LayoutError::InvalidHandle)
    }
    fn range_rects(&self, _: bareline_renderer::LayoutId, _: std::ops::Range<usize>) -> Result<Vec<Rect>, LayoutError> {
        Err(LayoutError::InvalidHandle)
    }
    fn release_layout(&mut self, layout: bareline_renderer::LayoutId) {
        self.0.push(layout);
    }
}
/// Byte ceiling for retained closed documents; a quarter of the budget when smaller.
const MAX_CLOSED_RETAINED_BYTES: usize = 64 * 1024 * 1024;
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
    /// True once nothing typed so far can be lost to a process exit.
    pub fn recovery_settled(&self) -> bool {
        match self {
            Self::Resident(editor) => editor.recovery_settled(),
            // Paged edits append to their journal inside the actor job, so an
            // idle paged editor holds no unprotected edit unless a typing burst
            // left a deferred batch, which its pump journals (PED-15). As for
            // resident documents, a journal restores only with its baseline copy,
            // so that copy must have landed or failed too.
            Self::Paged(editor) => {
                let status = editor.recovery_status();
                !editor.busy()
                    && !editor.recovery_batch_pending()
                    && (status.directory.is_none() || status.complete || status.error.is_some())
            }
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
    /// The selected text of a resident document, bounded by `limit` bytes.
    /// Reported as unsupported for a paged editor, whose selection may span
    /// text that is not resident.
    pub fn selected_text(&self, limit: usize) -> Result<String, String> {
        match self {
            Self::Resident(e) => e.selected_text(limit).map_err(str::to_string),
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
            Self::Resident(e) => e.rectangle_clipboard_metadata(text).map_err(|error| error.to_string()),
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
    /// End the typing run at the text a resident save is capturing, so undo can
    /// return to it even when typing continues while the save writes. Paged saves
    /// track their own state through the paged save path.
    pub fn seal_history(&mut self) {
        match self {
            Self::Resident(e) => e.seal_history(),
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
    pub fn reset_zoom(&mut self) -> bool {
        self.viewport_mut().reset_zoom()
    }
    pub fn set_view_guides(&mut self, guides: bareline_editor_surface::ViewGuides) {
        self.viewport_mut().set_view_guides(guides);
    }
    /// Go to, or select through, the bracket matching the one beside the caret
    /// (BIZ-07). False when the caret is not beside a bracket whose partner is
    /// within the bounded scan distance.
    pub fn jump_to_matching_brace(&mut self, select: bool) -> bool {
        let Some(inputs) = self.viewport().matching_brace_inputs(select) else {
            return false;
        };
        for input in inputs {
            self.enqueue(input);
        }
        true
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

/// Retry and large-file actions of a failed open, in workspace draw coordinates.
fn failed_open_buttons() -> [Rect; 2] {
    let y = bareline_ui::TAB_HEIGHT + 116.0;
    [
        bareline_ui::rect(16.0, y, 80.0, 30.0),
        bareline_ui::rect(108.0, y, 280.0, 30.0),
    ]
}

/// Paint a failed open's persistent error and actions instead of text (FIO-01).
/// Split panes paint the same panel, so `failed_open_pointer` targets match.
pub fn paint_failed_open(
    path: &std::path::Path,
    error: &str,
    width: f32,
    height: f32,
    theme: bareline_ui::theme::UiTheme,
    ops: &mut Vec<DrawOp>,
) {
    ops.push(DrawOp::Fill(
        bareline_ui::rect(
            0.0,
            bareline_ui::TAB_HEIGHT,
            width,
            (height - bareline_ui::TAB_HEIGHT).max(0.0),
        ),
        theme.editor,
    ));
    let top = bareline_ui::TAB_HEIGHT + 40.0;
    bareline_ui::text(
        ops,
        16.0,
        top,
        format!("Could not open {}", path.display()),
        15.0,
        theme.text,
    );
    bareline_ui::text(ops, 16.0, top + 32.0, error, 13.0, theme.muted);
    let [retry, large_file] = failed_open_buttons();
    for (bounds, label) in [(retry, "Retry"), (large_file, "Open read-only (large-file mode)")] {
        ops.push(DrawOp::FillRounded(bounds, theme.elevated, 4.0));
        ops.push(DrawOp::StrokeRounded(bounds, theme.border, 4.0, 1.0));
        bareline_ui::text(ops, bounds.x + 12.0, bounds.y + 7.0, label, 13.0, theme.text);
    }
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
    /// Each tab's identity, file and title, in the order of `editors` (P6-01).
    tabs: Vec<TabSlot>,
    next_tab: u64,
    tab_log: tabs::TabLog,
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
    /// Where the last `draw` put the message bar, in view coordinates; the
    /// shell keeps notifications above it.
    pub message_bar: Option<Rect>,
    pub find: crate::find::FindController,
    pub search_panel: crate::search_panel::SearchPanel,
    pub search_focus: bool,
    /// Reserved by a platform shell that composes Search into a shared dock.
    pub bottom_panel_height: f32,
    pub external_search_panel: bool,
    pending_replace: Option<bareline_search::service::ReplaceTicket>,
    pending_paged_replace: Option<bareline_search::service::PagedReplaceTicket>,
    /// Resident document whose submitted replacement is still being applied.
    applying_replace: Option<u64>,
    pending_search_navigation: Option<PendingSearchNavigation>,
    acknowledged_search_commands: Vec<bareline_editor_surface::power::consumer::OrderedReceipt>,
    styling: crate::styling::Styling,
    /// Spell checking of the resident views (BIZ-31).
    pub spelling: crate::spelling::Spelling,
    retired: Vec<WorkspaceEditor>,
    /// Native layouts of editors retired in `pump`, released by the next draw.
    retired_layouts: Vec<bareline_renderer::LayoutId>,
    closed: Vec<ClosedDocument>,
    /// Whether the last `close` remembered its document for Restore Closed Tab.
    last_close_remembered: bool,
    closed_checks: Vec<ClosedCheck>,
    closed_path_probe: ClosedPathProbe,
    /// Paths opened, saved or closed since the shell last took them, oldest
    /// first, for the Recent Files list (APP-12).
    recent_events: Vec<PathBuf>,
    closed_documents: std::cell::RefCell<Vec<(u64, u64)>>,
    activation_requests: Vec<ActivationRequest>,
    next_activation_request: u64,
    /// The tab an explicit open or restore asks to activate.
    activation: Option<Activation>,
    /// (previous document id, document id) for each restored closed tab.
    reopened: Vec<(u64, u64)>,
    /// Launch requests whose missing file becomes a new document (true, APP-09)
    /// or fails with only the plain not-found notice (false, APP-21).
    missing_launches: Vec<(u64, bool)>,
    /// Untitled documents that their first save creates at a launch path.
    create_targets: Vec<(u64, PathBuf)>,
    paused_transcode: Option<Box<bareline_file_io::lifecycle::PausedTranscode>>,
    paused_reload: Option<PendingReload>,
    /// The failed-open tab showing the pause; Resume reopens into it (FIO-01).
    paused_tab: Option<bareline_document::DocumentSnapshot>,
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
    /// Document id of the resident document the pending spill is migrating.
    spill_document: Option<u64>,
    spill_paused: bool,
    spill_selection: Option<(bareline_document::paged::PagedSnapshot, usize, usize, Option<u64>)>,
    failed_opens: Vec<FailedOpen>,
    /// Paths opened with a remote-read approval, which a plain open refuses (FIO-01).
    remote_open_paths: std::collections::BTreeSet<PathBuf>,
    /// Height of the band a platform shell reserves above a document's text for
    /// its external-change or follow banner, by document id. Views push their
    /// text down by it so a banner never covers tabs or text (UI-02).
    pub banner_bands: std::collections::BTreeMap<u64, f32>,
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
    /// The tab a save writes, its destination and BOM choice. The tab is named
    /// by id, so closing or moving other tabs never retargets it (P6-01).
    save: Option<(TabId, PathBuf, bool)>,
    copy_only: bool,
    open_path: Option<PathBuf>,
    launch_request: Option<u64>,
    recovery_restore_request: Option<u64>,
    allow_duplicate: bool,
    preview: Option<bareline_document::DocumentSnapshot>,
    reload: Option<PendingReload>,
    /// A user open whose failure keeps its tab as an error placeholder (FIO-01).
    keep_failed_tab: bool,
}
/// The exact document generation that a reload or Interpret As replaces. A
/// paged editor is captured by its whole-document snapshot, never by the
/// viewport projection that every scroll replaces (REC-15).
#[derive(Clone)]
enum ReloadTarget {
    Resident(bareline_document::DocumentSnapshot),
    Paged(bareline_document::paged::PagedSnapshot),
}
impl ReloadTarget {
    fn same_document(&self, editor: &WorkspaceEditor) -> bool {
        match (self, editor) {
            (Self::Resident(captured), WorkspaceEditor::Resident(editor)) => editor.snapshot().same_document(captured),
            (Self::Paged(captured), WorkspaceEditor::Paged(editor)) => editor.snapshot().same_document(captured),
            _ => false,
        }
    }
    /// The editor still holds exactly the captured text.
    fn unchanged(&self, editor: &WorkspaceEditor) -> bool {
        match (self, editor) {
            (Self::Resident(captured), WorkspaceEditor::Resident(editor)) => {
                editor.snapshot().same_document(captured) && editor.snapshot().revision == captured.revision
            }
            (Self::Paged(captured), WorkspaceEditor::Paged(editor)) => {
                let current = editor.snapshot();
                current.same_document(captured)
                    && current.revision == captured.revision
                    && current.content_state == captured.content_state
            }
            _ => false,
        }
    }
}
/// Reload and Interpret As replace a document the way close plus reopen does.
#[derive(Clone)]
struct PendingReload {
    target: ReloadTarget,
    /// The replaced text's recovery discard has started. If the replacement is
    /// then abandoned, recovery must resume for the text that stays open.
    discarding: bool,
    /// Interpret As target. A resident reinterpretation beyond the resident
    /// limits continues as a paged open of the same bytes (FIO-01).
    interpret: Option<bareline_file_io::codecs::Encoding>,
}
impl PendingReload {
    fn capture(editor: &WorkspaceEditor) -> Self {
        let target = match editor {
            WorkspaceEditor::Resident(editor) => ReloadTarget::Resident(editor.snapshot().clone()),
            WorkspaceEditor::Paged(editor) => ReloadTarget::Paged(editor.snapshot().clone()),
        };
        Self {
            target,
            discarding: false,
            interpret: None,
        }
    }
}
enum ReloadGate {
    Apply,
    Defer,
    Abandon,
}
/// A failed open keeps its tab: an empty, incomplete read-only placeholder whose
/// error stays visible with Retry and large-file actions (FIO-01).
struct FailedOpen {
    source: bareline_document::DocumentSnapshot,
    path: PathBuf,
    error: String,
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
/// The one plain notice for a requested file that does not exist (APP-21).
pub fn missing_file_message(path: &std::path::Path) -> String {
    format!("File not found: {}", path.display())
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
            tabs: Vec::new(),
            next_tab: 1,
            tab_log: Default::default(),
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
            message_bar: None,
            find: crate::find::FindController::default(),
            search_panel: Default::default(),
            search_focus: false,
            bottom_panel_height: 0.0,
            external_search_panel: false,
            pending_replace: None,
            pending_paged_replace: None,
            applying_replace: None,
            pending_search_navigation: None,
            acknowledged_search_commands: Vec::new(),
            styling: crate::styling::Styling::default(),
            spelling: crate::spelling::Spelling::default(),
            retired: Vec::new(),
            retired_layouts: Vec::new(),
            closed: Vec::new(),
            last_close_remembered: false,
            closed_checks: Vec::new(),
            closed_path_probe: Arc::new(|path: &std::path::Path| path.is_file()),
            recent_events: Vec::new(),
            closed_documents: std::cell::RefCell::new(Vec::new()),
            activation_requests: Vec::new(),
            next_activation_request: ACTIVATION_REQUESTS.start,
            activation: None,
            reopened: Vec::new(),
            missing_launches: Vec::new(),
            create_targets: Vec::new(),
            paused_transcode: None,
            paused_reload: None,
            paused_tab: None,
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
            spill_document: None,
            spill_paused: false,
            spill_selection: None,
            failed_opens: Vec::new(),
            remote_open_paths: std::collections::BTreeSet::new(),
            banner_bands: std::collections::BTreeMap::new(),
        })
    }
    pub fn new_document(&mut self) -> Result<(), String> {
        let metadata = self.new_document_defaults.metadata().inspect_err(|error| {
            self.message = Some(error.clone());
        })?;
        let mut document = Document::from_utf8("", self.bytes.clone(), self.history.clone())
            .map_err(|error| format!("A new document could not be created: {error}."))?;
        document
            .initialize_metadata(metadata)
            .map_err(|error| format!("The new-document settings could not be applied: {error}."))?;
        let snapshot = document.snapshot();
        let editor = EditorSurface::new(self.scheduler.document(document, 32), snapshot, self.notify.clone());
        let label = format!("Untitled {}", self.next_untitled);
        self.push_tab(editor.into(), None, label, LifecycleEvent::Created);
        self.next_untitled += 1;
        Ok(())
    }
    /// A new Untitled document holding `text`, such as standard input piped to
    /// the launch. The text is an unsaved edit, so closing asks to save it (APP-09).
    pub fn new_document_with_text(&mut self, text: String) -> Result<usize, String> {
        self.new_document()?;
        let index = self.editors.len() - 1;
        if !text.is_empty() {
            self.editors[index].commit(text);
        }
        Ok(index)
    }
    /// A launch path that does not exist opens as an empty document named after
    /// it, which its first save creates there, with one plain notice (APP-09, APP-21).
    fn new_document_for_missing(&mut self, path: PathBuf) -> Result<(u64, u64), String> {
        self.new_document()?;
        let index = self.editors.len() - 1;
        if let Some(name) = path.file_name() {
            self.tabs[index].label = name.to_string_lossy().into_owned();
        }
        let document = self.editors[index].document_identity();
        let live: Vec<_> = self.editors.iter().map(|editor| editor.document_identity().0).collect();
        self.create_targets.retain(|(id, _)| live.contains(id));
        self.message = Some(format!(
            "File not found: {}. It will be created when you save.",
            path.display()
        ));
        self.create_targets.push((document.0, path));
        Ok(document)
    }
    /// Where the first save of an unsaved document creates its file, when it was
    /// opened from a launch path that did not exist (APP-09).
    pub fn create_target(&self, index: usize) -> Option<&std::path::Path> {
        if self.tabs.get(index)?.file.is_some() {
            return None;
        }
        let document = self.editors.get(index)?.document_identity().0;
        self.create_targets
            .iter()
            .find(|(id, _)| *id == document)
            .map(|(_, path)| path.as_path())
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
        Ok(self.push_tab(
            editor.into(),
            None,
            label.chars().take(4096).collect(),
            LifecycleEvent::Previewed,
        ))
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
        Ok(self.push_tab(
            WorkspaceEditor::Paged(preview),
            None,
            label.chars().take(4096).collect(),
            LifecycleEvent::Previewed,
        ))
    }
    /// One event-loop turn of background work. Each kind of completion has
    /// its own handler; this only fixes the order they run in (ARC-01).
    pub fn pump(&mut self) -> bool {
        let mut changed = self.find.pump();
        self.pump_closed_checks();
        changed |= self.pump_save_cleanups();
        changed |= self.pump_save_recovery();
        if let Some(warning) = bareline_file_io::recovery_retirement::take_cleanup_warning(&self.notify) {
            self.message = Some(format!("{warning}. Use Retry Recovery to try cleanup again."));
            changed = true;
        }
        changed |= self.search_panel.pump();
        changed |= self.styling.pump();
        changed |= self.spelling.pump(&mut self.editors);
        if let Some(notice) = self.spelling.take_unavailable_notice() {
            self.message = Some(notice);
            changed = true;
        }
        changed |= self.pump_editors();
        changed |= self.pump_recovery_restore_publications();
        changed |= self.pump_encoding();
        changed |= self.pump_search_acknowledgment();
        self.sync_paged_files();
        changed |= self.pump_paged_replace();
        changed |= self.pump_replace();
        self.finish_applied_replace();
        changed |= self.pump_io();
        self.restore_spill_selection();
        // Closed-tab history gives back its retained text before open documents spill.
        self.trim_closed_history();
        // Event-driven pressure handling; failed storage waits for explicit retry.
        if !self.spill_pending
            && !self.spill_paused
            && self.pending_io.is_empty()
            && self.bytes.used() > self.bytes.limit().saturating_mul(3) / 4
        {
            self.migrate_clean_resident();
        }
        self.release_retired();
        changed
    }
    /// Drop retired editors, and the documents they hold, without waiting for a
    /// draw that never comes while minimized; only their native layout ids wait
    /// for the renderer at the next draw.
    fn release_retired(&mut self) {
        let mut layouts = RetiredLayouts::default();
        for mut editor in self.retired.drain(..) {
            editor.release_layouts(&mut layouts);
        }
        self.retired_layouts.append(&mut layouts.0);
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
            if self.spill_pending {
                return Ok(false);
            }
            // The failure is reported once; a later request starts a fresh promotion.
            self.promotion_target = None;
            return Err(self
                .message
                .clone()
                .unwrap_or_else(|| "Promotion did not attach; document retained".into()));
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
        let encoding = self.tabs[index].file.as_ref().and_then(|file| file.encoding.clone());
        let original = self.tabs[index]
            .file
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
        self.spill_document = Some(captured_identity.0);
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
            keep_failed_tab: false,
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
                    && self.tabs[*index].lifecycle.spill_eligible()
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
        let file = self.tabs[index].file.as_ref().unwrap();
        let Some(service) = self.editors[index].document_service() else {
            return false;
        };
        let Some(saved_state) = self.editors[index].saved_content_state() else {
            return false;
        };
        let spill_document = self.editors[index].snapshot().identity_token().0;
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
                self.spill_document = Some(spill_document);
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
                    keep_failed_tab: false,
                });
                true
            }
            Err(_) => false,
        }
    }
    fn discard_preview(&mut self, source: Option<&bareline_document::DocumentSnapshot>) {
        self.discard_preview_to(source, None);
    }
    /// Drop a loading or failed tab whose content `successor` takes over, if any.
    /// Pending saves name their tab by id, so no other tab's work moves (P6-01).
    fn discard_preview_to(&mut self, source: Option<&bareline_document::DocumentSnapshot>, successor: Option<TabId>) {
        if let Some(index) = source.and_then(|source| {
            self.editors
                .iter()
                .position(|editor| editor.snapshot().same_document(source))
        }) {
            let (removed, _) = self.remove_tab(index, successor);
            let document = removed.document_identity().0;
            self.retired.push(removed);
            // An explicit request's result now lands in another tab (APP-07).
            for entry in &mut self.activation_requests {
                if entry.shown == Some(document) {
                    entry.displaced = true;
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
    /// A user open: its tab becomes active once it appears (APP-07).
    pub fn open(&mut self, path: PathBuf) {
        self.open_requested(path, None);
    }
    /// Opens without ever moving the active tab, as session restore does (APP-07).
    pub fn open_in_background(&mut self, path: PathBuf) {
        let _ = self.open_for_launch(path, None, true, false);
    }
    fn open_requested(&mut self, path: PathBuf, reopen: Option<(u64, bool)>) {
        let request = self.request_activation(path.clone(), reopen);
        if self.open_for_launch(path, Some(request), true, false).is_err() {
            self.finish_activation(request, None);
        }
    }
    pub fn open_tracked(&mut self, request_id: u64, path: PathBuf) -> Result<(), String> {
        self.open_for_launch(path, Some(request_id), true, false)
    }
    /// `open_tracked`, except that a path that does not exist becomes a new
    /// document that its first save creates there (APP-09).
    pub fn open_tracked_or_create(&mut self, request_id: u64, path: PathBuf) -> Result<(), String> {
        self.open_tracked_missing(request_id, path, true)
    }
    /// `open_tracked`, except that a path that does not exist fails with only
    /// the plain `missing_file_message` and no failed-open tab (APP-21).
    pub fn open_tracked_or_report(&mut self, request_id: u64, path: PathBuf) -> Result<(), String> {
        self.open_tracked_missing(request_id, path, false)
    }
    fn open_tracked_missing(&mut self, request_id: u64, path: PathBuf, create: bool) -> Result<(), String> {
        if self.missing_launches.len() >= 256 {
            self.missing_launches.remove(0);
        }
        self.missing_launches.push((request_id, create));
        let result = self.open_for_launch(path, Some(request_id), true, false);
        if result.is_err() {
            self.missing_launches.retain(|(request, _)| *request != request_id);
        }
        result
    }
    fn request_activation(&mut self, path: PathBuf, reopen: Option<(u64, bool)>) -> u64 {
        let request = self.next_activation_request;
        self.next_activation_request = if request + 1 < ACTIVATION_REQUESTS.end {
            request + 1
        } else {
            ACTIVATION_REQUESTS.start
        };
        // An open cancelled without an outcome leaves its entry behind.
        if self.activation_requests.len() >= 64 {
            self.activation_requests.remove(0);
        }
        self.activation_requests.push(ActivationRequest {
            request,
            path,
            shown: None,
            displaced: false,
            reopen,
        });
        request
    }
    /// Shows an explicit request's tab (APP-07). Its first tab, loading or
    /// loaded, is activated. A document that later takes over that tab's slot in
    /// place (resident or paged completion, paged fallback, transcode) does not
    /// move focus again, so a user who has switched tabs meanwhile keeps them.
    /// A result that lands in another tab (already open, created, recovered) is
    /// activated only while the loading tab is still the active one. A restored
    /// closed tab hands its closed document's tab over to the new document (WSP-05).
    fn show_activation(&mut self, request: u64, document: u64) {
        let Some(entry) = self
            .activation_requests
            .iter_mut()
            .find(|entry| entry.request == request)
        else {
            return;
        };
        let previous = entry.shown.replace(document);
        if previous == Some(document) {
            return;
        }
        let displaced = std::mem::take(&mut entry.displaced);
        let reopen = entry.reopen;
        let previous_live = previous.is_some_and(|previous| {
            self.editors
                .iter()
                .any(|editor| editor.document_identity().0 == previous)
        });
        // A second document while the first tab is still open is not its replacement.
        if previous_live {
            return;
        }
        // A shown tab that finished in place (loaded, failed, paged fallback or
        // transcode, noted by `note_tab_replaced`) was activated when it appeared; the
        // active tab follows it through the pump, so a user who moved to another
        // tab while it loaded keeps that tab (PED-23). A result that lands in
        // another tab (`displaced`: already open, created, recovered) asks for
        // activation only from the loading tab it replaces (APP-07).
        if let Some(pending) = &mut self.activation
            && previous == Some(pending.document)
        {
            // The shell has not taken the earlier tab yet: activate its successor.
            pending.document = document;
        } else if previous.is_none() || displaced {
            self.activation = Some(Activation {
                document,
                from: previous,
            });
        }
        // Otherwise the document took the loading tab's slot in place, so an
        // active loading tab stays active without a new request.
        if let Some((closed, read_only)) = reopen {
            self.reopened.push((previous.unwrap_or(closed), document));
            if read_only
                && let Some(editor) = self
                    .editors
                    .iter_mut()
                    .find(|editor| editor.document_identity().0 == document)
            {
                editor.set_read_only(true);
            }
        }
    }
    /// Ends an explicit request with its document, or with the failed tab that
    /// says why it did not open.
    fn finish_activation(&mut self, request: u64, document: Option<u64>) {
        let Some(position) = self
            .activation_requests
            .iter()
            .position(|entry| entry.request == request)
        else {
            return;
        };
        let document = match document {
            Some(document) => Some(document),
            None => {
                // A failed tab does not take over a restored tab's pin and view.
                self.activation_requests[position].reopen = None;
                let path = &self.activation_requests[position].path;
                self.failed_opens
                    .iter()
                    .rev()
                    .find(|failed| &failed.path == path)
                    .map(|failed| failed.source.identity_token().0)
            }
        };
        if let Some(document) = document {
            self.show_activation(request, document);
        }
        self.activation_requests.remove(position);
    }
    fn forget_reopen(&mut self, request: Option<u64>) {
        if let Some(entry) = self
            .activation_requests
            .iter_mut()
            .find(|entry| Some(entry.request) == request)
        {
            entry.reopen = None;
        }
    }
    /// (previous document id, new document id) for each restored closed tab, so
    /// the shell moves the closed tab's pin, position and view to it (WSP-05).
    pub fn take_reopened_tabs(&mut self) -> Vec<(u64, u64)> {
        std::mem::take(&mut self.reopened)
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
        let keep_failed_tab = !allow_duplicate;
        // One user open per path at a time: a second one, even of a failed tab
        // whose Retry is in flight, would settle into a second tab (FIO-01).
        if keep_failed_tab
            && self
                .pending_io
                .iter()
                .any(|pending| pending.keep_failed_tab && pending.open_path.as_deref() == Some(path.as_path()))
        {
            let error = "File is already opening.".to_string();
            self.message = Some(error.clone());
            return Err(error);
        }
        if !self.ensure_io() {
            let error = self.message.take().unwrap_or_else(|| "File service unavailable".into());
            return Err(self.fail_open_submission(path, keep_failed_tab, error));
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
                // Opening a path again reuses its failed tab instead of adding one (FIO-01).
                let preview = keep_failed_tab
                    .then(|| self.failed_opens.iter().position(|failed| failed.path == path))
                    .flatten()
                    .map(|position| self.take_failed_open(position, false));
                self.pending_io.push(PendingIo {
                    completion: None,
                    receiver,
                    save: None,
                    copy_only: false,
                    open_path: Some(path),
                    launch_request,
                    recovery_restore_request: None,
                    allow_duplicate,
                    preview,
                    reload: None,
                    keep_failed_tab,
                });
                self.message = Some("Opening…".into());
                if discover_recovery && let Some(parent) = recovery_parent {
                    self.discover_save_recovery(&parent);
                }
                Ok(())
            }
            Err(_) => Err(self.fail_open_submission(
                path,
                keep_failed_tab,
                "File queue is full. Try again after the pending operation.".into(),
            )),
        }
    }
    /// An open that could not be submitted still leaves a visible error: the
    /// path's failed tab shows the new error, or a failed tab is added (FIO-01).
    fn fail_open_submission(&mut self, path: PathBuf, keep_failed_tab: bool, error: String) -> String {
        if keep_failed_tab {
            if let Some(failed) = self.failed_opens.iter_mut().find(|failed| failed.path == path) {
                failed.error.clone_from(&error);
            } else {
                self.settle_failed_open(None, Some(path), true, &error);
            }
        }
        self.message = Some(error.clone());
        error
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
        upsert_save_issue(&mut self.save_cleanups, cleanup, |issue| &issue.transaction);
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
        upsert_save_issue(&mut self.save_conflicts, conflict, |issue| &issue.transaction);
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
                self.message = Some("Too many file operations are waiting to check for interrupted saves. Use File > Document > Recovery > Retry Save Recovery Discovery in a moment.".into());
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
    fn paged_open_request(&self, path: PathBuf, interpret: Option<bareline_file_io::codecs::Encoding>) -> IoRequest {
        IoRequest::OpenPagedEncoded(bareline_file_io::lifecycle::PagedOpenRequest {
            path,
            bytes: self.bytes.clone(),
            history: self.history.clone(),
            cache: std::env::temp_dir().join("Bareline-transcode"),
            options: bareline_file_io::codecs::disk::DiskOptions {
                temp_quota_bytes: self.transcode_quota_bytes,
                interpret,
            },
            source_options: self.source_options(),
        })
    }
    fn submit_paged(&mut self, request: IoRequest, path: PathBuf, launch_request: Option<u64>, allow_duplicate: bool) {
        self.submit_paged_open(request, path, launch_request, allow_duplicate, None, false);
    }
    /// A loading tab handed to the paged fallback stays in place while it runs.
    fn submit_paged_open(
        &mut self,
        request: IoRequest,
        path: PathBuf,
        launch_request: Option<u64>,
        allow_duplicate: bool,
        preview: Option<bareline_document::DocumentSnapshot>,
        keep_failed_tab: bool,
    ) {
        if !self.ensure_io() {
            let error = "File service unavailable".to_string();
            self.settle_failed_open(preview.as_ref(), Some(path), keep_failed_tab, &error);
            self.record_launch_open(launch_request, Err(error));
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
                    preview,
                    reload: None,
                    keep_failed_tab,
                });
                self.message = Some("Preparing paged text…".into());
            }
            Err(request) => {
                let error = "File queue is full; retry opening or resuming.".to_string();
                let tab = self.settle_failed_open(preview.as_ref(), Some(path), keep_failed_tab, &error);
                if let IoRequest::ResumeTranscode { paused, .. } = *request {
                    self.paused_transcode = Some(paused);
                    self.paused_tab = tab;
                }
                self.message = Some(error.clone());
                self.record_launch_open(launch_request, Err(error));
            }
        }
    }
    fn preview_index(&self, source: Option<&bareline_document::DocumentSnapshot>) -> Option<usize> {
        let source = source?;
        self.editors
            .iter()
            .position(|editor| editor.snapshot().same_document(source))
    }
    /// The tab holding the file with this identity. Volume and file index are
    /// canonical, so a differently spelled path still finds it.
    fn open_file_index(&self, identity: &bareline_platform::FileIdentity) -> Option<usize> {
        self.tabs.iter().position(|tab| {
            tab.file.as_ref().is_some_and(|file| {
                file.fingerprint.identity.volume == identity.volume && file.fingerprint.identity.file == identity.file
            })
        })
    }
    /// An open of a file that is already open resolves to that tab: its own
    /// loading tab closes and hands focus to the existing tab, which is also
    /// the launch outcome (PED-24).
    fn settle_duplicate_open(
        &mut self,
        existing: usize,
        preview: Option<&bareline_document::DocumentSnapshot>,
        launch_request: Option<u64>,
    ) {
        let document = self.editors[existing].document_identity();
        // The duplicate's own tab hands its place to the existing tab.
        let existing_tab = self.tabs[existing].id;
        self.discard_preview_to(preview, Some(existing_tab));
        let tab = self
            .editors
            .iter()
            .position(|editor| editor.document_identity() == document)
            .unwrap_or(existing);
        self.message = Some(format!("File is already open in tab {}.", tab + 1));
        // Restoring a closed tab whose file is open again only shows that tab.
        self.forget_reopen(launch_request);
        self.record_launch_open(launch_request, Ok(document));
    }
    fn failed_open_position(&self, index: usize) -> Option<usize> {
        let editor = self.editors.get(index)?;
        self.failed_opens
            .iter()
            .position(|failed| failed.source.same_document(editor.snapshot()))
    }
    /// The path and error of a tab whose open failed (FIO-01).
    pub fn failed_open(&self, index: usize) -> Option<(&std::path::Path, &str)> {
        let failed = &self.failed_opens[self.failed_open_position(index)?];
        Some((failed.path.as_path(), failed.error.as_str()))
    }
    /// Settle the tab of a failed open. A user open keeps (or gains) an empty
    /// read-only placeholder carrying the error, never a partial preview that
    /// could pass for the whole file; other operations drop their preview.
    /// Returns the kept placeholder's snapshot.
    fn settle_failed_open(
        &mut self,
        preview: Option<&bareline_document::DocumentSnapshot>,
        path: Option<PathBuf>,
        keep_failed_tab: bool,
        error: &str,
    ) -> Option<bareline_document::DocumentSnapshot> {
        let Some(path) = path.filter(|_| keep_failed_tab) else {
            self.discard_preview(preview);
            return None;
        };
        // An empty document reserves nothing; its own budgets keep the
        // placeholder independent of an exhausted shared cap.
        let Ok(builder) = bareline_document::DocumentBuilder::new(Budget::new(0), Budget::new(0)) else {
            self.discard_preview(preview);
            return None;
        };
        let source = builder.prefix();
        let label = path
            .file_name()
            .map_or_else(|| "File".into(), |name| name.to_string_lossy().into_owned());
        let placeholder: WorkspaceEditor = EditorSurface::loading(source.clone(), self.notify.clone()).into();
        match self.preview_index(preview) {
            Some(index) => {
                let old = std::mem::replace(&mut self.editors[index], placeholder);
                self.note_tab_replaced(index, old.document_identity());
                self.retired.push(old);
                self.tabs[index].file = None;
                self.tabs[index].label = format!("{label} (failed)");
                let _ = self.apply_lifecycle(index, LifecycleEvent::LoadFailed);
            }
            None => {
                self.push_tab(
                    placeholder,
                    None,
                    format!("{label} (failed)"),
                    LifecycleEvent::LoadFailed,
                );
            }
        }
        self.find.clear_source();
        self.failed_opens.push(FailedOpen {
            source: source.clone(),
            path,
            error: error.to_owned(),
        });
        Some(source)
    }
    /// Hand a failed tab to a new open of its path: the tab shows loading and
    /// receives that open's document or next failure in place (FIO-01).
    fn take_failed_open(&mut self, position: usize, read_only: bool) -> bareline_document::DocumentSnapshot {
        let failed = self.failed_opens.remove(position);
        if self
            .paused_tab
            .as_ref()
            .is_some_and(|paused| paused.same_document(&failed.source))
        {
            // The new open replaces the pause shown in this tab; a later
            // Resume must not open the paused transcode into a second tab.
            self.paused_transcode = None;
            self.paused_reload = None;
            self.paused_tab = None;
        }
        if let Some(index) = self.preview_index(Some(&failed.source)) {
            // The placeholder carries the read-only choice to the replacing editor.
            self.editors[index].set_read_only(read_only);
            let label = failed
                .path
                .file_name()
                .map_or_else(|| "File".into(), |name| name.to_string_lossy().into_owned());
            self.tabs[index].label = format!("{label} (loading)");
            let _ = self.apply_lifecycle(index, LifecycleEvent::LoadStarted);
        }
        failed.source
    }
    /// Retry a failed open in its own tab (FIO-01).
    pub fn retry_failed_open(&mut self, index: usize) -> Result<(), String> {
        self.restart_failed_open(index, false)
    }
    /// Reopen a failed open read-only with paged (large-file) storage (FIO-01).
    pub fn open_failed_as_large_file(&mut self, index: usize) -> Result<(), String> {
        self.restart_failed_open(index, true)
    }
    fn restart_failed_open(&mut self, index: usize, paged: bool) -> Result<(), String> {
        let position = self
            .failed_open_position(index)
            .ok_or("This tab has no failed open to retry")?;
        if !self.ensure_io() {
            return Err("File service unavailable".into());
        }
        let path = self.failed_opens[position].path.clone();
        if self.remote_open_paths.contains(&path) {
            // A plain open refuses a remote path; only a new approval can retry it.
            let error = "Approve this remote file again with Open Remote File with Permission.".to_string();
            self.failed_opens[position].error.clone_from(&error);
            return Err(error);
        }
        let request = if paged {
            self.paged_open_request(path.clone(), None)
        } else {
            IoRequest::OpenStreaming {
                path: path.clone(),
                bytes: self.bytes.clone(),
                history: self.history.clone(),
                resident_max_bytes: self.resident_max_bytes,
            }
        };
        let receiver = match self.io.as_ref().unwrap().submit(request, self.notify.clone()) {
            Ok(receiver) => receiver,
            Err(_) => {
                // The tab keeps showing why this attempt did not start (FIO-01).
                let error = "File queue is full. Try again after the pending operation.".to_string();
                self.failed_opens[position].error.clone_from(&error);
                return Err(error);
            }
        };
        let source = self.take_failed_open(position, paged);
        self.pending_io.push(PendingIo {
            completion: None,
            receiver,
            save: None,
            copy_only: false,
            open_path: Some(path),
            launch_request: None,
            recovery_restore_request: None,
            allow_duplicate: false,
            preview: Some(source),
            reload: None,
            keep_failed_tab: true,
        });
        let message = if paged { "Preparing paged text…" } else { "Opening…" };
        self.message = Some(message.into());
        Ok(())
    }
    /// Route a press on a failed-open tab. Its only targets are the error panel's
    /// actions; `true` means the press was consumed.
    pub fn failed_open_pointer(&mut self, index: usize, point: bareline_renderer::Point) -> bool {
        if self.failed_open_position(index).is_none() {
            return false;
        }
        let [retry, large_file] = failed_open_buttons();
        let result = if retry.contains(point) {
            self.retry_failed_open(index)
        } else if large_file.contains(point) {
            self.open_failed_as_large_file(index)
        } else {
            return true;
        };
        if let Err(error) = result {
            self.message = Some(error);
        }
        true
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
            .map_err(|error| format!("Recovered text could not be opened: {error}."))?;
        let snapshot = document.snapshot();
        let document_id = snapshot.identity_token().0;
        let mut surface = EditorSurface::new(self.scheduler.document(document, 32), snapshot, self.notify.clone());
        // Inserting the recovered text is what marks the tab as having unsaved changes.
        let receipt = surface.enqueue_tracked(Input::Insert(text))?;
        let index = self.push_tab(surface.into(), None, label, LifecycleEvent::Created);
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
        self.submit_requested_recovery(directory);
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
        self.submit_requested_recovery(directory);
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
    /// A restore the user asked for shows its tab once it opens (APP-07).
    fn submit_requested_recovery(&mut self, directory: PathBuf) {
        let before = self.pending_io.len();
        self.submit_paged_recovery(directory.clone());
        if self.pending_io.len() > before {
            let request = self.request_activation(directory, None);
            self.pending_io.last_mut().unwrap().launch_request = Some(request);
        }
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
            // The tab kept for the paused open receives the resumed document in place.
            let kept = self
                .paused_tab
                .take()
                .and_then(|tab| {
                    self.failed_opens
                        .iter()
                        .position(|failed| failed.source.same_document(&tab))
                })
                .map(|position| self.failed_opens.remove(position).source);
            if let Some(index) = self.preview_index(kept.as_ref()) {
                let label = path
                    .file_name()
                    .map_or_else(|| "File".into(), |name| name.to_string_lossy().into_owned());
                self.tabs[index].label = format!("{label} (loading)");
                let _ = self.apply_lifecycle(index, LifecycleEvent::LoadStarted);
            }
            let keep_failed_tab = kept.is_some();
            // Resuming an open is an explicit command, so its tab becomes active
            // (APP-07); a resumed reload stays in its tab.
            let request = self
                .paused_reload
                .is_none()
                .then(|| self.request_activation(path.clone(), None));
            let before = self.pending_io.len();
            self.submit_paged_open(
                IoRequest::ResumeTranscode {
                    paused,
                    temp_quota_bytes: quota_bytes,
                },
                path,
                request,
                false,
                kept,
                keep_failed_tab,
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
    /// Paths opened, saved or closed since the last call, oldest first. Nothing
    /// else reorders the Recent Files list (APP-12).
    pub fn take_recent_events(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.recent_events)
    }
    fn note_recent(&mut self, path: PathBuf) {
        self.recent_events.push(path.clone());
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
        self.tabs
            .get(index)
            .and_then(|tab| tab.file.as_ref().map(|f| f.path.as_path()))
    }
    /// Point the resident document at `index` to `path` after its file moved
    /// there on disk (File ▸ Rename, WSP-01). The editor, its history and its
    /// captured fingerprint stay: a same-volume rename keeps the file identity,
    /// so reopening would only be deduplicated against this very document.
    /// Returns the path the document was bound to before.
    pub fn rebind_path(&mut self, index: usize, path: PathBuf) -> Result<PathBuf, String> {
        // A paged document's source path lives in its reader; it cannot move.
        if !matches!(self.editors.get(index), Some(WorkspaceEditor::Resident(_))) {
            return Err("Only a fully loaded document can be renamed".into());
        }
        let Some(Some(file)) = self.tabs.get_mut(index).map(|tab| &mut tab.file) else {
            return Err("Save the document before renaming it".into());
        };
        // Refresh the admission lease under the new path. While a disk
        // replacement owns admission the old lease stays; its file identity
        // still matches the renamed file, so it is still protected.
        if let Ok(lease) = self
            .replacement_registry
            .try_register(path.clone(), &file.fingerprint.identity)
        {
            file._lease = Some(lease);
        }
        let old = std::mem::replace(&mut file.path, path.clone());
        let stale = bareline_platform::SerializedPath::from_native(&old);
        self.recent
            .retain(|existing| existing.encoding != stale.encoding || existing.data != stale.data);
        self.note_recent(path);
        Ok(old)
    }
    /// Give the unsaved document at `index` a new tab title (File ▸ Rename on an
    /// Untitled tab, WSP-01). The document keeps its text, history and tab
    /// position; a launch path its first save would have created is dropped,
    /// so that save asks where to put the file. Saved documents rename their
    /// file and keep their editor through [`Self::rebind_path`] instead.
    pub fn rename_untitled(&mut self, index: usize, title: &str) -> Result<(), String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("Enter a name for the tab".into());
        }
        if title.chars().count() > 255 || title.chars().any(char::is_control) {
            return Err("Use a name of at most 255 characters, without line breaks or tabs".into());
        }
        if let Some(reason) = self.untitled_rename_blocked(index) {
            return Err(reason.into());
        }
        let document = self.editors[index].document_identity().0;
        self.create_targets.retain(|(id, _)| *id != document);
        self.tabs[index].label = title.to_owned();
        Ok(())
    }
    /// Why the tab at `index` cannot take a new title now, or `None` when it
    /// can: it must be an unsaved document that finished loading.
    pub fn untitled_rename_blocked(&self, index: usize) -> Option<&'static str> {
        if !matches!(self.tabs.get(index).map(|tab| &tab.file), Some(None)) {
            return Some("Only an unsaved document can be renamed without renaming its file");
        }
        let editor = &self.editors[index];
        if self.failed_open(index).is_some() || editor.busy() || !(editor.paged() || editor.snapshot().is_complete()) {
            return Some("Wait for the document to finish loading");
        }
        None
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
        self.missing_launches.retain(|(request, _)| *request != request_id);
        if ACTIVATION_REQUESTS.contains(&request_id) {
            self.finish_activation(request_id, result.ok().map(|document| document.0));
            return;
        }
        // Outcomes nobody collects must not accumulate; the oldest yield first.
        if self.open_outcomes.len() >= MAX_OPEN_OUTCOMES {
            self.open_outcomes.remove(0);
        }
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
        self.tabs
            .get(index)
            .and_then(|tab| tab.file.as_ref())
            .map(|file| &file.fingerprint)
    }
    /// Bytes of the document's file on disk as last opened or saved (UI-07).
    pub fn file_bytes(&self, index: usize) -> Option<u64> {
        self.fingerprint(index).map(|fingerprint| fingerprint.identity.length)
    }
    /// The banner band reserved above document `index`'s text (UI-02).
    pub fn banner_band(&self, index: usize) -> f32 {
        self.editors
            .get(index)
            .and_then(|editor| self.banner_bands.get(&editor.document_identity().0))
            .copied()
            .unwrap_or(0.0)
    }
    pub fn reload(&mut self, index: usize, discard_confirmed: bool) -> Result<(), String> {
        let editor = self.editors.get(index).ok_or("Document is unavailable")?;
        if editor.busy() || (editor.dirty() && !discard_confirmed) {
            return Err("Confirm discard of current edits before reloading".into());
        }
        let captured = PendingReload::capture(editor);
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
            keep_failed_tab: false,
        });
        self.message = Some("Reloading… current text remains available until complete.".into());
        Ok(())
    }
    fn reload_index(&self, target: &ReloadTarget) -> Option<usize> {
        self.editors.iter().position(|editor| target.unchanged(editor))
    }
    /// Reload and Interpret As replace a document the way close plus reopen
    /// does, so a dirty document's recovery is durably discarded, as close does,
    /// before its replacement attaches. Otherwise a crash would offer the
    /// discarded text instead of the reloaded one (REC-04). A busy target only
    /// delays the replacement; it is not a change to the document (REC-15).
    fn gate_reload(&mut self, i: usize, result: &IoCompletion) -> ReloadGate {
        if !matches!(
            result,
            IoCompletion::Open(Ok(_))
                | IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Complete(_))
        ) {
            return ReloadGate::Apply;
        }
        let Some(target) = self.pending_io[i].reload.as_ref().map(|reload| reload.target.clone()) else {
            return ReloadGate::Apply;
        };
        // A changed or closed document is reported by the completion itself.
        let Some(index) = self.reload_index(&target) else {
            return ReloadGate::Apply;
        };
        if self.editors[index].busy() {
            return ReloadGate::Defer;
        }
        if !self.editors[index].dirty() {
            return ReloadGate::Apply;
        }
        if let Some(reload) = self.pending_io[i].reload.as_mut() {
            reload.discarding = true;
        }
        match self.editors[index].discard_recovery() {
            bareline_file_io::recovery_retirement::DiscardPoll::Pending => ReloadGate::Defer,
            bareline_file_io::recovery_retirement::DiscardPoll::Durable
            | bareline_file_io::recovery_retirement::DiscardPoll::CleanupPending(_) => ReloadGate::Apply,
            bareline_file_io::recovery_retirement::DiscardPoll::TombstoneFailed(error) => {
                self.editors[index].resume_recovery_after_discard();
                self.message = Some(format!(
                    "Reload stopped because recovery could not be discarded: {error}. Current edits were kept."
                ));
                ReloadGate::Abandon
            }
        }
    }
    /// Recovery stays with the text that remains open when a replacement is
    /// abandoned after its discard started.
    fn resume_abandoned_reload(&mut self, reload: Option<&PendingReload>) {
        let Some(reload) = reload.filter(|reload| reload.discarding) else {
            return;
        };
        if let Some(editor) = self
            .editors
            .iter_mut()
            .find(|editor| reload.target.same_document(editor))
        {
            editor.resume_recovery_after_discard();
        }
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
        let mut tabs: Vec<_> = self.tabs.drain(..).map(Some).collect();
        for index in order {
            self.editors.push(editors[*index].take().unwrap());
            self.tabs.push(tabs[*index].take().unwrap());
        }
        self.last_drawn = None;
        self.find.clear_source();
        true
    }
    /// True once every open document's edits and recovery checkpoints have
    /// caught up. Logoff and shutdown pump the workspace until this holds or a
    /// deadline passes.
    pub fn recovery_settled(&self) -> bool {
        self.editors.iter().all(WorkspaceEditor::recovery_settled)
    }
    /// Whether the document at `index` has an edit in flight or a queued save.
    pub fn document_busy(&self, index: usize) -> bool {
        self.editors.get(index).is_some_and(|editor| editor.busy())
            || self.tabs.get(index).is_some_and(|tab| tab.lifecycle.saving())
    }
    pub fn discard_recoveries(&mut self, indexes: &[usize]) -> bareline_file_io::recovery_retirement::DiscardPoll {
        let mut outcome = bareline_file_io::recovery_retirement::DiscardPoll::Durable;
        for &index in indexes {
            // A preview's journal belongs to the document it shows (REC-13).
            if !self.tabs.get(index).is_some_and(|tab| tab.lifecycle.owns_text()) {
                continue;
            }
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
        // A queued save holds the file's lease, so the lifecycle keeps its tab
        // open until the save settles (P6-02).
        let closing = lifecycle::transition(
            self.tabs[index].lifecycle,
            LifecycleEvent::Closed,
            self.tab_facts(index),
        );
        if editor.busy() || closing.is_err() {
            return Err(CloseError::Busy);
        }
        if editor.dirty() && !discard {
            return Err(CloseError::Unsaved);
        }
        let preview_source = editor.read_only().then(|| editor.snapshot().clone());
        let closed_path = self.path(index).map(std::path::Path::to_path_buf);
        // A preview's journal belongs to the document it shows (REC-13).
        if discard && self.tabs[index].lifecycle.owns_text() {
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
        // Closing a loading tab cancels its open and settles the launch request;
        // the tab is remembered by path, never as its read-only loading preview.
        let mut loading_path = None;
        if let Some(source) = preview_source {
            let (abandoned, pending): (Vec<_>, Vec<_>) =
                std::mem::take(&mut self.pending_io).into_iter().partition(|pending| {
                    pending
                        .preview
                        .as_ref()
                        .is_some_and(|preview| preview.same_document(&source))
                });
            self.pending_io = pending;
            for abandoned in abandoned {
                abandoned.receiver.cancel();
                self.record_launch_open(
                    abandoned.launch_request,
                    Err("The tab was closed before the file finished opening.".into()),
                );
                self.record_recovery_restore(
                    abandoned.recovery_restore_request,
                    Err("The tab was closed before the recovered document finished opening.".into()),
                );
                if abandoned.reload.is_none() {
                    loading_path = loading_path.or(abandoned.open_path);
                }
            }
        }
        let (mut closed, TabSlot { file, label, .. }) = self.remove_tab(index, None);
        closed.release_layouts(renderer);
        self.closed_documents
            .borrow_mut()
            .push(closed.snapshot().identity_token());
        let failed_position = self
            .failed_opens
            .iter()
            .position(|failed| failed.source.same_document(closed.snapshot()));
        let failed_open = failed_position.map(|position| self.failed_opens.remove(position).path);
        if self.spill_pending && self.spill_document == Some(closed.snapshot().identity_token().0) {
            // Only an owned-resident spill has neither a save target nor a path.
            for pending in &self.pending_io {
                if pending.save.is_none() && pending.open_path.is_none() {
                    pending.receiver.cancel();
                }
            }
        }
        if failed_open.is_none()
            && let Some(path) = &closed_path
        {
            self.recent_events.push(path.clone());
        }
        // A failed-open tab or loading preview has no read-only choice of its own
        // to carry over.
        self.last_close_remembered = file.is_some() || (failed_open.is_none() && loading_path.is_none());
        let saved = (!closed.dirty())
            .then(|| file.as_ref().map(|file| file.path.clone()))
            .flatten();
        // A loading preview is never remembered as its partial read-only text.
        match (failed_open.or(loading_path), saved) {
            // A failed-open placeholder or loading preview holds no document:
            // remember its path only, and restoring it retries the open. Nothing
            // is stat'ed here (FIO-01, APP-19).
            (Some(path), _) => {
                let document = closed.document_identity().0;
                let read_only = file.is_some() && closed.read_only();
                drop(closed);
                self.closed.push(ClosedDocument::Reopen(Reopen {
                    path,
                    document,
                    read_only,
                }));
            }
            (None, saved) => {
                // At most one closed document keeps a paged source alive.
                if closed.paged() {
                    self.closed.retain(|entry| !entry.paged());
                }
                // A closed resident document keeps only its path and fingerprint, so
                // Replace in Files no longer reports it open; restore re-registers it.
                // A paged one still reads unloaded text from that file and keeps it.
                let mut file = file;
                if !closed.paged()
                    && let Some(file) = &mut file
                {
                    file._lease = None;
                }
                let identity = closed.snapshot().identity_token();
                self.closed
                    .push(ClosedDocument::Retained(Box::new(closed), file, label));
                // A saved document is kept only until a worker finds its file;
                // a paged one then releases its source, spill store and
                // transcode directory.
                if let Some(path) = saved {
                    self.check_closed_path(identity, path);
                }
            }
        }
        self.trim_closed_history();
        self.find.clear_source();
        if self.editors.is_empty() {
            self.find.hide();
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
    /// Retained closed tabs keep their text charged to the shared budget, so the
    /// history is bounded by bytes as well as count, and it yields entirely when
    /// open documents need the memory. The oldest entries are evicted first.
    fn trim_closed_history(&mut self) {
        let limit = self.bytes.limit();
        let cap = MAX_CLOSED_RETAINED_BYTES.min(limit / 4);
        loop {
            let pressure = self.bytes.used() > limit.saturating_mul(3) / 4;
            let retained: usize = self.closed.iter().map(ClosedDocument::retained_bytes).sum();
            let evict = if self.closed.len() > MAX_CLOSED_DOCUMENTS {
                Some(0)
            } else if retained > cap || (pressure && retained > 0) {
                self.closed.iter().position(|entry| entry.retained_bytes() > 0)
            } else {
                None
            };
            let Some(evict) = evict else {
                break;
            };
            let entry = self.closed.remove(evict);
            // A saved document still waiting for its file check (APP-19) yields its
            // model but keeps its path, as it would once the check found the file;
            // restoring it then reopens from disk (WSP-05). Nothing is stat'ed here.
            if self.closed.len() < MAX_CLOSED_DOCUMENTS
                && let ClosedDocument::Retained(editor, file, _) = &entry
                && let Some(check) = self
                    .closed_checks
                    .iter()
                    .find(|check| check.identity == editor.snapshot().identity_token())
            {
                let reopen = Reopen {
                    path: check.path.clone(),
                    document: editor.document_identity().0,
                    read_only: file.is_some() && editor.read_only(),
                };
                drop(entry);
                self.closed.insert(evict, ClosedDocument::Reopen(reopen));
            }
        }
        // A queued check whose model is gone has nothing left to decide; one in
        // flight finishes harmlessly because its entry no longer matches.
        let closed = &self.closed;
        self.closed_checks
            .retain(|check| check.task.is_some() || closed.iter().any(|entry| entry.retains(check.identity)));
    }
    pub fn can_restore_closed(&self) -> bool {
        !self.closed.is_empty()
    }
    /// Whether a closed tab's file check is still queued or running; until it
    /// finishes, that tab restores from its retained model (APP-19).
    pub fn closed_checks_pending(&self) -> bool {
        !self.closed_checks.is_empty()
    }
    fn check_closed_path(&mut self, identity: (u64, u64), path: PathBuf) {
        self.closed_checks.push(ClosedCheck {
            identity,
            path,
            task: None,
        });
        self.start_closed_check();
    }
    /// Start the oldest queued check unless one is already in flight.
    fn start_closed_check(&mut self) {
        if self.closed_checks.iter().any(|check| check.task.is_some()) {
            return;
        }
        while let Some(check) = self.closed_checks.first_mut() {
            let probe = self.closed_path_probe.clone();
            let notify = self.notify.clone();
            let target = check.path.clone();
            match crate::task::spawn(move || notify(), move |_| probe(&target)) {
                Ok(task) => {
                    check.task = Some(task);
                    return;
                }
                // A full pool keeps the model: restoring it then needs no file at all.
                Err(_) => {
                    self.closed_checks.remove(0);
                }
            }
        }
    }
    /// A closed document whose file is still on disk is remembered by path only,
    /// releasing its model; one whose file is gone keeps the model to restore.
    fn pump_closed_checks(&mut self) {
        let closed = &mut self.closed;
        self.closed_checks
            .retain(|check| match check.task.as_ref().map(|task| task.poll()) {
                None | Some(crate::task::TaskPoll::Pending) => true,
                Some(crate::task::TaskPoll::Complete(true)) => {
                    if let Some(entry) = closed.iter_mut().find(|entry| entry.retains(check.identity))
                        && let ClosedDocument::Retained(editor, file, _) = entry
                    {
                        // The reopened document takes over this tab and its
                        // read-only choice, as set since the close (WSP-05).
                        let document = editor.document_identity().0;
                        let read_only = file.is_some() && editor.read_only();
                        *entry = ClosedDocument::Reopen(Reopen {
                            path: check.path.clone(),
                            document,
                            read_only,
                        });
                    }
                    false
                }
                Some(_) => false,
            });
        self.start_closed_check();
    }
    pub fn set_last_closed_read_only(&mut self, read_only: bool) {
        if !self.last_close_remembered {
            return;
        }
        match self.closed.last_mut() {
            Some(ClosedDocument::Retained(editor, _, _)) => editor.set_read_only(read_only),
            Some(ClosedDocument::Reopen(reopen)) => reopen.read_only = read_only,
            None => {}
        }
    }
    /// Documents closed since the last call, so panels can drop cached views of them.
    pub fn take_closed_documents(&self) -> Vec<(u64, u64)> {
        std::mem::take(&mut self.closed_documents.borrow_mut())
    }
    /// Reattach the retained model, history and selection without reopening its path.
    /// A saved document reopens from disk and returns None; its tab is shown,
    /// with its pin, position, view and read-only state, once it loads (WSP-05).
    pub fn restore_last_closed(&mut self) -> Option<usize> {
        self.last_close_remembered = false;
        let (mut editor, mut file, label) = match self.closed.pop()? {
            ClosedDocument::Retained(editor, file, label) => (*editor, file, label),
            ClosedDocument::Reopen(reopen) => {
                self.open_requested(reopen.path, Some((reopen.document, reopen.read_only)));
                return None;
            }
        };
        // Closing released the open-file lease; the document is open again only once
        // Replace in Files can see it. Its saves still compare the kept fingerprint.
        if let Some(state) = &mut file
            && state._lease.is_none()
        {
            match self
                .replacement_registry
                .try_register(state.path.clone(), &state.fingerprint.identity)
            {
                Ok(lease) => state._lease = Some(lease),
                Err(error) => {
                    self.message = Some(if error.kind() == std::io::ErrorKind::WouldBlock {
                        "A replacement is changing files; reopen the closed document when it finishes.".into()
                    } else {
                        format!("File admission failed: {error}")
                    });
                    self.closed
                        .push(ClosedDocument::Retained(Box::new(editor), file, label));
                    return None;
                }
            }
        }
        // A check in flight finishes harmlessly: its entry is gone.
        let identity = editor.snapshot().identity_token();
        self.closed_checks
            .retain(|check| check.identity != identity || check.task.is_some());
        editor.resume_recovery_after_discard();
        if !matches!(&editor,WorkspaceEditor::Paged(paged) if paged.save_as_required())
            && let Some(file) = &file
        {
            self.note_recent(file.path.clone());
        }
        // A retained compare or recovery view comes back as a preview (REC-13).
        let view = match &editor {
            WorkspaceEditor::Resident(resident) => resident.document_service().is_none(),
            WorkspaceEditor::Paged(paged) => paged.historical(),
        };
        let index = self.push_tab(editor, file, label, LifecycleEvent::Restored { view });
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
        self.paused_tab = None;
        // The paused reload goes with its transcode: a later open must not inherit
        // it and replace that tab in place, and recovery stays with the open text.
        let reload = self.paused_reload.take();
        self.resume_abandoned_reload(reload.as_ref());
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
            .tabs
            .get(index)
            .and_then(|tab| tab.file.as_ref())
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
    /// A preview's text is never its own to save (REC-13).
    pub fn save_all_targets(&self) -> Vec<(usize, Option<PathBuf>)> {
        self.editors
            .iter()
            .enumerate()
            .filter(|(index, editor)| {
                editor.dirty() && self.tabs.get(*index).is_some_and(|tab| tab.lifecycle.owns_text())
            })
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
                        .tabs
                        .get(index)
                        .and_then(|tab| tab.file.as_ref())
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
        let Some(tab) = self.tab_id(index) else {
            return false;
        };
        // The lifecycle refuses a second save and any save of text still
        // loading; a preview saves only as a copy (P6-02).
        let started = lifecycle::transition(
            self.tabs[index].lifecycle,
            LifecycleEvent::SaveStarted { copy: copy_only },
            self.tab_facts(index),
        );
        if started == Err(LifecycleRefusal::AlreadySaving) {
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
        let saving = match started {
            Ok(saving) if editor.snapshot().is_complete() && (copy_only || !editor.read_only()) => saving,
            _ => {
                self.message = Some("Document is not ready or is read only; saving is unavailable.".into());
                return false;
            }
        };
        let file = self.tabs[index].file.as_ref();
        let bom = file.is_some_and(|file| file.bom);
        let path = destination.path.clone();
        let request = if copy_only {
            IoRequest::SaveCopy {
                snapshot: editor.snapshot().clone(),
                destination,
                source: file.map(|file| file.path.clone()),
                bom,
                encoding: file.and_then(|file| file.encoding.clone()),
            }
        } else if let Some(encoding) = file.and_then(|file| file.encoding.clone()) {
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
                if !copy_only {
                    // Typing during the save must not merge into the captured text.
                    self.editors[index].seal_history();
                }
                self.tabs[index].lifecycle = saving;
                self.pending_io.push(PendingIo {
                    completion: None,
                    receiver,
                    save: Some((tab, path, bom)),
                    copy_only,
                    open_path: None,
                    launch_request: None,
                    recovery_restore_request: None,
                    allow_duplicate: false,
                    preview: None,
                    reload: None,
                    keep_failed_tab: false,
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
                let mut title = self.tabs[i]
                    .file
                    .as_ref()
                    .and_then(|file| file.path.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.tabs[i].label.clone());
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
        for layout in self.retired_layouts.drain(..) {
            renderer.release_layout(layout);
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
                            .tabs
                            .get(index)
                            .and_then(|tab| tab.file.as_ref())
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
        // A pending binary notice (UI-01) gets its own band under the Find bar.
        let notice_band = if self.binary_warning_pending(active) {
            crate::encoding::BINARY_NOTICE_HEIGHT
        } else {
            0.0
        };
        let failed_open = self
            .failed_open(active)
            .map(|(path, error)| (path.to_path_buf(), error.to_owned()));
        let banner_band = self.banner_band(active);
        let file_bytes = self.file_bytes(active);
        let panel_inset = if self.external_search_panel {
            self.bottom_panel_height
        } else {
            self.search_panel.height()
        };
        // The message bar takes its own band above the bottom panel instead of
        // covering the last text lines, so the caret and a match revealed
        // there stay visible (LNX-EDIT-011).
        let message_band = if self.message.is_some() { MESSAGE_BAND } else { 0.0 };
        let mut result = match self.editors.get_mut(active) {
            Some(editor) => {
                match editor {
                    WorkspaceEditor::Resident(resident) => self.find.refresh(resident.snapshot(), self.notify.clone()),
                    WorkspaceEditor::Paged(paged) => self.find.refresh_paged(paged.read_handle(), self.notify.clone()),
                }
                let find_height = if self.find.open { self.find.height() } else { 0.0 };
                // Find bar, then any external-change banner (UI-02), then the
                // binary notice, all above the first text line.
                editor.viewport_mut().top_inset = find_height + banner_band + notice_band;
                editor.viewport_mut().file_bytes = file_bytes;
                editor.viewport_mut().not_loaded = failed_open.is_some();
                editor.viewport_mut().bottom_inset = panel_inset + message_band;
                let language = self
                    .tabs
                    .get(active)
                    .and_then(|tab| tab.file.as_ref())
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
                if let WorkspaceEditor::Resident(_) = editor {
                    // SRC-14: carry the previous colors to this revision before
                    // drawing, or the first frame after each edit draws plain.
                    self.styling.carry(
                        editor.snapshot(),
                        language,
                        definition.clone(),
                        editor.viewport().syntax_preference,
                    );
                }
                let paged = editor.paged();
                editor.set_external_scrollbar(paged);
                let frame_start = ops.len();
                let mut result = if paint_paged_pending(editor, width, height, self.theme, ops) {
                    Ok(None)
                } else if let Some((path, error)) = &failed_open {
                    paint_failed_open(path, error, width, height, self.theme, ops);
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
                if let WorkspaceEditor::Resident(surface) = editor {
                    self.spelling.refresh(
                        surface,
                        self.styling
                            .result
                            .as_ref()
                            .filter(|result| result.language == language),
                        self.notify.clone(),
                    );
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
        self.message_bar = None;
        if let Some(message) = &self.message {
            let y = (height - bareline_ui::STATUS_HEIGHT - panel_inset - MESSAGE_BAND).max(34.0);
            let bar = bareline_ui::rect(50.0, y, width - 66.0, 30.0);
            ops.push(DrawOp::Fill(bar, bareline_ui::ELEVATED));
            bareline_ui::text(ops, 64.0, y + 6.0, message, 13.0, bareline_ui::TEXT);
            self.message_bar = Some(bar);
        }
        result
    }
}
/// Status text for a failed file operation; the wording lives with the error
/// type so every surface shows the same sentence (UI-03).
fn file_error(error: FileError) -> String {
    error.to_string()
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
    /// A queued entry for driving one pump handler directly. Its ticket is a
    /// real, never-read save-recovery inspection; the handlers only read the
    /// fields a test sets (ARC-01).
    pub(super) fn pending_io(workspace: &mut Workspace) -> PendingIo {
        assert!(workspace.ensure_io());
        let request = IoRequest::InspectSaveRecovery {
            parent: std::env::temp_dir().join(format!("bareline-pump-handler-{}", std::process::id())),
        };
        let Ok(receiver) = workspace.io.as_ref().unwrap().submit(request, workspace.notify.clone()) else {
            panic!("the file queue accepts a handler fixture");
        };
        PendingIo {
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
            keep_failed_tab: false,
        }
    }
    struct DistinctOpenFileSystem;
    impl LocalFileSystem for DistinctOpenFileSystem {
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
            limit: usize,
            budget: std::time::Duration,
            cancelled: &dyn Fn() -> bool,
        ) -> bareline_platform::CacheRemovalOutcome {
            PagedFileSystem.remove_owned_cache_directory(
                root,
                candidate,
                root_identity,
                candidate_identity,
                proof_name,
                proof_bytes,
                limit,
                budget,
                cancelled,
            )
        }
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
        let (woke, wake) = std::sync::mpsc::channel();
        let mut workspace = Workspace::new(
            Arc::new(move || {
                let _ = woke.send(());
            }),
            platform,
        )
        .unwrap();
        // The worker waits on the closed gate, so returning at all proves discovery
        // never ran on this thread; no wall-clock budget is needed (QA-07).
        assert!(workspace.discover_save_recovery(&parent));
        assert!(!workspace.discover_save_recovery(&parent));
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        // Pump on each completion wake instead of spinning against a deadline.
        while !workspace.pending_save_recovery.is_empty() {
            wake.recv().unwrap();
            workspace.pump();
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
    fn failed_open_fixture(name: &str) -> (PathBuf, Workspace) {
        let directory = std::env::temp_dir().join(format!(
            "bareline-failed-open-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        (directory, workspace)
    }
    fn settle_open(workspace: &mut Workspace) {
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
    /// WSP-03: a closed dirty document drops its open-file lease, so Replace in Files
    /// no longer skips its file as open; restoring the document registers it again.
    #[test]
    fn closing_a_dirty_document_releases_its_replacement_lease_until_restore() {
        let root = std::env::temp_dir().join(format!(
            "bareline-lease-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("dirty.txt");
        std::fs::write(&path, "abc").unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.open(path);
        settle_open(&mut workspace);
        let canonical = workspace.path(0).unwrap().to_owned();
        let identity = workspace.fingerprint(0).unwrap().identity;
        let registry = workspace.replacement_registry();
        assert!(registry.is_registered(&canonical, &identity).unwrap());
        workspace.editors[0].enqueue(Input::Insert("X".into()));
        settle_open(&mut workspace);
        assert!(workspace.editors[0].dirty());
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(0, true, &mut renderer).unwrap();
        assert!(!registry.is_registered(&canonical, &identity).unwrap());
        assert_eq!(workspace.restore_last_closed(), Some(0));
        assert!(workspace.editors[0].dirty());
        assert!(registry.is_registered(&canonical, &identity).unwrap());
        drop(workspace);
        remove_test_directory(root);
    }
    /// SRC-14: the first frame after an edit in the main editor keeps the
    /// previous syntax colors instead of painting plain text until the worker
    /// replies. Split panes get the same through `Styling::prepare_view`.
    #[test]
    fn the_first_frame_after_an_edit_keeps_syntax_colors() {
        fn keyword_colored(
            ops: &[DrawOp],
            renderer: &bareline_renderer_recording::RecordingBackend,
            keyword: bareline_renderer::Color,
        ) -> bool {
            ops.iter().any(|op| {
                matches!(op, DrawOp::Layout { layout, .. }
                    if renderer.styles.get(layout).is_some_and(|styles| styles.iter().any(|style| style.color == keyword)))
            })
        }
        let (directory, mut workspace) = failed_open_fixture("syntax-carry");
        let path = directory.join("main.rs");
        std::fs::write(&path, "// note\nfn main() {}\n").unwrap();
        workspace.open(path);
        settle_open(&mut workspace);
        assert!(matches!(workspace.editors[0], WorkspaceEditor::Resident(_)));
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let mut ops = Vec::new();
            workspace.draw(0, &mut renderer, 800.0, 600.0, &mut ops).unwrap();
            let keyword = workspace.editors[0].viewport().theme.keyword;
            if keyword_colored(&ops, &renderer, keyword) && workspace.styling_receipt().is_some_and(|r| r.ready) {
                break;
            }
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "styling never settled");
            std::thread::yield_now();
        }
        workspace.editors[0].enqueue(Input::Insert("x".into()));
        settle_open(&mut workspace);
        assert!(workspace.editors[0].dirty());
        // No worker reply for the new revision has been pumped: this frame can
        // only be colored by the stand-in carried through the edit.
        let mut ops = Vec::new();
        workspace.draw(0, &mut renderer, 800.0, 600.0, &mut ops).unwrap();
        let keyword = workspace.editors[0].viewport().theme.keyword;
        let stand_in = workspace.syntax_result().unwrap();
        assert!(stand_in.is_current(workspace.editors[0].snapshot()));
        assert_eq!(stand_in.status, bareline_syntax::Status::Provisional);
        assert!(
            keyword_colored(&ops, &renderer, keyword),
            "the edited frame drew plain text"
        );
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// APP-19: closing a saved tab never stats its file on the UI thread, where
    /// a disconnected share blocks for a minute. The model stays restorable
    /// until a worker finds the file, and is then released.
    #[test]
    fn closing_a_saved_tab_checks_its_file_on_a_worker() {
        let (directory, mut workspace) = failed_open_fixture("close-check");
        let path = directory.join("saved.txt");
        std::fs::write(&path, "saved").unwrap();
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert!(workspace.failed_open(0).is_none(), "{:?}", workspace.message);
        let ui = std::thread::current().id();
        let (probed, probes) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let (probed, gate) = (std::sync::Mutex::new(probed), std::sync::Mutex::new(gate));
        workspace.closed_path_probe = Arc::new(move |path: &std::path::Path| {
            probed.lock().unwrap().send(std::thread::current().id()).unwrap();
            let _ = gate.lock().unwrap().recv_timeout(std::time::Duration::from_secs(10));
            path.is_file()
        });
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(0, false, &mut renderer).unwrap();
        // The probe is still held at its gate, so close cannot have waited for it.
        let probe_thread = probes.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_ne!(probe_thread, ui, "the closed file was checked on the UI thread");
        assert!(matches!(workspace.closed.last(), Some(ClosedDocument::Retained(..))));
        assert!(workspace.can_restore_closed());
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !matches!(workspace.closed.last(), Some(ClosedDocument::Reopen(reopen)) if reopen.path == path) {
            workspace.pump();
            assert!(
                std::time::Instant::now() < deadline,
                "the closed model was never released"
            );
            std::thread::yield_now();
        }
        assert!(workspace.closed_checks.is_empty());
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// APP-19: closing many tabs from an unreachable share keeps one file
    /// check in flight, so the shared pool keeps workers for other tasks.
    #[test]
    fn closed_file_checks_run_one_at_a_time() {
        let (directory, _) = failed_open_fixture("close-serial");
        // Distinct lengths give distinct test file identities, so the duplicate
        // open check (PED-24) never mistakes one file for another.
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(DistinctOpenFileSystem)).unwrap();
        let paths: Vec<_> = (0..3).map(|n| directory.join(format!("saved-{n}.txt"))).collect();
        for (n, path) in paths.iter().enumerate() {
            std::fs::write(path, "saved".repeat(n + 1)).unwrap();
            workspace.open(path.clone());
            settle_open(&mut workspace);
        }
        assert_eq!(workspace.editors.len(), 3, "{:?}", workspace.message);
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = std::sync::Mutex::new(gate);
        workspace.closed_path_probe = Arc::new(move |path: &std::path::Path| {
            let _ = gate.lock().unwrap().recv_timeout(std::time::Duration::from_secs(10));
            path.is_file()
        });
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        while !workspace.editors.is_empty() {
            workspace.close(0, false, &mut renderer).unwrap();
        }
        workspace.pump();
        assert_eq!(workspace.closed_checks.len(), 3);
        assert_eq!(
            workspace
                .closed_checks
                .iter()
                .filter(|check| check.task.is_some())
                .count(),
            1,
            "closed file checks must not fan out across the pool"
        );
        drop(release);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !workspace.closed_checks.is_empty() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "queued checks never ran");
            std::thread::yield_now();
        }
        assert!(
            workspace
                .closed
                .iter()
                .all(|entry| matches!(entry, ClosedDocument::Reopen(_)))
        );
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// A closed paged document is checked on a worker like any saved tab: if
    /// its file is gone the retained source stays restorable, as before.
    #[test]
    fn a_closed_paged_document_whose_file_is_gone_stays_restorable() {
        let (directory, mut workspace) = failed_open_fixture("close-paged");
        let path = directory.join("paged.txt");
        std::fs::write(&path, "line abc\r\n".repeat(20000)).unwrap();
        workspace.resident_max_bytes = 64 * 1024;
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert!(workspace.editors[0].paged(), "{:?}", workspace.message);
        workspace.closed_path_probe = Arc::new(|_: &std::path::Path| false);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(0, false, &mut renderer).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !workspace.closed_checks.is_empty() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "the check never finished");
            std::thread::yield_now();
        }
        assert!(matches!(workspace.closed.last(), Some(entry) if entry.paged()));
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// APP-12: the Recent Files list hears about opens, saves and closes only,
    /// oldest first; open tabs are never re-recorded by a pump. FIO-01: closing
    /// a failed-open placeholder records nothing and stats nothing.
    #[test]
    fn recent_events_follow_open_and_close_only() {
        let (directory, mut workspace) = failed_open_fixture("recent-events");
        let first = directory.join("first.txt");
        let second = directory.join("second.txt");
        std::fs::write(&first, "1").unwrap();
        std::fs::write(&second, "2").unwrap();
        workspace.open(first.clone());
        settle_open(&mut workspace);
        workspace.open(second.clone());
        settle_open(&mut workspace);
        assert_eq!(workspace.take_recent_events(), [first.clone(), second.clone()]);
        workspace.pump();
        assert!(
            workspace.take_recent_events().is_empty(),
            "open tabs are not re-recorded"
        );
        workspace.closed_path_probe = Arc::new(|_: &std::path::Path| true);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(0, false, &mut renderer).unwrap();
        assert_eq!(workspace.take_recent_events(), std::slice::from_ref(&first));
        let missing = directory.join("missing.txt");
        workspace.open(missing.clone());
        settle_open(&mut workspace);
        let failed = workspace.editors.len() - 1;
        assert!(workspace.failed_open(failed).is_some(), "{:?}", workspace.message);
        workspace.closed_path_probe =
            Arc::new(|_: &std::path::Path| -> bool { panic!("a placeholder close stats nothing") });
        workspace.close(failed, false, &mut renderer).unwrap();
        assert!(
            workspace.take_recent_events().is_empty(),
            "a failed open never enters the list"
        );
        assert!(matches!(workspace.closed.last(), Some(ClosedDocument::Reopen(reopen)) if reopen.path == missing));
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// WSP-01: Rename keeps the document itself. A saved document is retargeted
    /// in place, keeping its tab position, identity and undo history; an
    /// LNX-EDIT-011: the message bar covered the last text lines, so the caret
    /// after Ctrl+End or a match near the end was hidden behind it.
    #[test]
    fn the_message_bar_takes_its_own_band_below_the_text() {
        let (directory, mut workspace) = failed_open_fixture("message-band");
        workspace.new_document().unwrap();
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.draw(0, &mut renderer, 800.0, 600.0, &mut Vec::new()).unwrap();
        assert_eq!(workspace.editors[0].viewport().bottom_inset, 0.0);
        assert_eq!(workspace.message_bar, None);
        workspace.message = Some("This system does not support spell checking yet".into());
        workspace.draw(0, &mut renderer, 800.0, 600.0, &mut Vec::new()).unwrap();
        let bar = workspace.message_bar.expect("the message bar is drawn");
        let view = workspace.editors[0].viewport();
        assert_eq!(view.bottom_inset, MESSAGE_BAND);
        // The text ends at the bar's band; the bar sits on the status strip.
        let text_bottom = 600.0 - bareline_ui::STATUS_HEIGHT - view.bottom_inset;
        assert!(bar.y >= text_bottom && bar.y + bar.height <= 600.0 - bareline_ui::STATUS_HEIGHT);
        workspace.message = None;
        workspace.draw(0, &mut renderer, 800.0, 600.0, &mut Vec::new()).unwrap();
        assert_eq!(workspace.editors[0].viewport().bottom_inset, 0.0);
        assert_eq!(workspace.message_bar, None);
        drop(workspace);
        remove_test_directory(directory);
    }
    /// Untitled tab only changes its title, and loading or failed tabs refuse.
    #[test]
    fn rename_retargets_in_place_and_retitles_untitled_tabs() {
        let (directory, mut workspace) = failed_open_fixture("rename");
        let source = directory.join("before.txt");
        let target = directory.join("after.txt");
        std::fs::write(&source, "text").unwrap();
        workspace.new_document().unwrap();
        workspace.open(source.clone());
        settle_open(&mut workspace);
        workspace.new_document().unwrap();
        assert_eq!(workspace.path(1), Some(source.as_path()));
        workspace.editors[1].enqueue(Input::Insert("more ".into()));
        settle_open(&mut workspace);
        assert!(workspace.editors[1].can_undo());
        let identity = workspace.editors[1].document_identity();
        std::fs::rename(&source, &target).unwrap();
        assert_eq!(workspace.rebind_path(1, target.clone()), Ok(source.clone()));
        assert_eq!(workspace.path(1), Some(target.as_path()));
        assert_eq!(workspace.editors[1].document_identity(), identity);
        assert!(workspace.editors[1].can_undo() && workspace.editors[1].dirty());
        assert_eq!(workspace.titles()[1], "after.txt \u{2022}");
        let untitled = workspace.editors[0].document_identity();
        assert_eq!(workspace.rename_untitled(0, "  Notes  "), Ok(()));
        assert_eq!(workspace.titles(), ["Notes", "after.txt \u{2022}", "Untitled 2"]);
        assert_eq!(workspace.editors[0].document_identity(), untitled);
        assert!(workspace.rename_untitled(0, " ").is_err());
        assert!(workspace.rename_untitled(0, "a\tb").is_err());
        assert!(
            workspace.rename_untitled(1, "saved").is_err(),
            "a saved file renames on disk"
        );
        assert_eq!(workspace.titles()[0], "Notes");
        workspace.open(directory.join("missing.txt"));
        settle_open(&mut workspace);
        let failed = workspace.editors.len() - 1;
        assert!(workspace.failed_open(failed).is_some(), "{:?}", workspace.message);
        assert!(workspace.rename_untitled(failed, "x").is_err());
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// FIO-01 / MT-31: a forced open failure leaves a visible error tab, and
    /// Retry opens the file in that same tab.
    #[test]
    fn failed_open_keeps_an_error_tab_that_retries_in_place() {
        let (directory, mut workspace) = failed_open_fixture("retry");
        let path = directory.join("missing.txt");
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert_eq!(workspace.editors.len(), 1, "{:?}", workspace.message);
        assert_eq!(workspace.titles(), ["missing.txt (failed)"]);
        let (failed, error) = workspace.failed_open(0).unwrap();
        assert_eq!(failed, path.as_path());
        assert!(!error.is_empty());
        assert!(workspace.editors[0].read_only());
        std::fs::write(&path, "recovered").unwrap();
        workspace.retry_failed_open(0).unwrap();
        settle_open(&mut workspace);
        assert_eq!(workspace.editors.len(), 1, "{:?}", workspace.message);
        assert!(workspace.failed_open(0).is_none());
        assert_eq!(workspace.titles(), ["missing.txt"]);
        assert!(workspace.editors[0].snapshot().is_complete());
        assert_eq!(workspace.editors[0].snapshot().len(), 9);
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// FIO-01: a failed approved remote open keeps an error tab as a local open
    /// does, and approving that path again reuses the tab instead of adding one.
    #[test]
    fn failed_remote_open_keeps_one_error_tab() {
        let (directory, mut workspace) = failed_open_fixture("remote");
        let remote = directory.join("remote.txt");
        std::fs::write(&remote, "remote text\n").unwrap();
        for _ in 0..2 {
            let grant = bareline_platform::RemoteReadGrant::after_consent(
                remote.clone(),
                bareline_platform::RemoteReadAction::Open,
                std::time::Duration::from_secs(60),
            )
            .unwrap();
            workspace.open_authorized(remote.clone(), grant).unwrap();
            settle_open(&mut workspace);
            // This file system grants no remote reads, so the approved open fails.
            assert_eq!(workspace.titles(), ["remote.txt (failed)"], "{:?}", workspace.message);
            assert_eq!(workspace.failed_opens.len(), 1);
            assert_eq!(workspace.failed_open(0).unwrap().0, remote.as_path());
        }
        // A plain retry would be refused as remote; the tab asks for a new approval.
        for error in [workspace.retry_failed_open(0), workspace.open_failed_as_large_file(0)] {
            assert!(error.unwrap_err().contains("Open Remote File with Permission"));
        }
        assert!(!workspace.io_busy());
        assert_eq!(workspace.titles(), ["remote.txt (failed)"]);
        assert!(
            workspace
                .failed_open(0)
                .unwrap()
                .1
                .contains("Open Remote File with Permission")
        );
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// A paused reload goes with its paused transcode when file operations are
    /// cancelled: it no longer retains the replaced text for a later open.
    #[test]
    fn cancelling_file_operations_drops_a_paused_reload() {
        let (directory, mut workspace) = failed_open_fixture("paused-reload");
        let path = directory.join("reload.txt");
        std::fs::write(&path, "resident text\n").unwrap();
        workspace.open(path);
        settle_open(&mut workspace);
        // The reload needs paged storage, whose transcode pauses at its quota.
        workspace.resident_max_bytes = 0;
        workspace.transcode_quota_bytes = 8;
        workspace.reload(0, false).unwrap();
        settle_open(&mut workspace);
        assert!(workspace.paused_transcode.is_some(), "{:?}", workspace.message);
        assert!(workspace.paused_reload.is_some());
        workspace.cancel_file_operations();
        assert!(workspace.paused_transcode.is_none());
        assert!(workspace.paused_reload.is_none());
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// FIO-01: the failed tab's large-file action reopens it read-only and paged.
    #[test]
    fn failed_open_reopens_read_only_in_large_file_mode() {
        let (directory, mut workspace) = failed_open_fixture("paged");
        workspace.resident_max_bytes = 4;
        let path = directory.join("later.txt");
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert!(workspace.failed_open(0).is_some(), "{:?}", workspace.message);
        std::fs::write(&path, "paged text").unwrap();
        workspace.open_failed_as_large_file(0).unwrap();
        settle_open(&mut workspace);
        assert_eq!(workspace.editors.len(), 1, "{:?}", workspace.message);
        assert!(workspace.failed_open(0).is_none());
        assert!(matches!(&workspace.editors[0], WorkspaceEditor::Paged(editor) if editor.snapshot().len() == 10));
        assert!(workspace.editors[0].read_only());
        assert_eq!(workspace.titles(), ["later.txt"]);
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// FIO-01: opening a failed path again reuses its error tab instead of adding
    /// one per attempt, and the open that succeeds lands, editable, in that tab.
    #[test]
    fn reopening_a_failed_path_reuses_its_error_tab() {
        let (directory, mut workspace) = failed_open_fixture("reopen");
        let path = directory.join("absent.txt");
        for _ in 0..3 {
            workspace.open(path.clone());
            settle_open(&mut workspace);
            assert_eq!(workspace.titles(), ["absent.txt (failed)"], "{:?}", workspace.message);
            assert_eq!(workspace.failed_opens.len(), 1);
        }
        std::fs::write(&path, "present").unwrap();
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert_eq!(workspace.titles(), ["absent.txt"], "{:?}", workspace.message);
        assert!(workspace.failed_open(0).is_none());
        assert!(!workspace.editors[0].read_only());
        assert_eq!(workspace.editors[0].snapshot().len(), 7);
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// FIO-01: opening a path whose Retry is still in flight starts no second
    /// open, so a second failure cannot settle into a second error tab.
    #[test]
    fn opening_a_path_during_its_retry_keeps_one_tab() {
        let (directory, mut workspace) = failed_open_fixture("retrying");
        let path = directory.join("gone.txt");
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert!(workspace.failed_open(0).is_some(), "{:?}", workspace.message);
        workspace.retry_failed_open(0).unwrap();
        assert_eq!(
            workspace.open_for_launch(path.clone(), None, true, false),
            Err("File is already opening.".to_string())
        );
        assert_eq!(workspace.pending_io.len(), 1);
        settle_open(&mut workspace);
        assert_eq!(workspace.titles(), ["gone.txt (failed)"], "{:?}", workspace.message);
        assert_eq!(workspace.failed_opens.len(), 1);
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// FIO-01: an open that cannot be submitted still shows its error, on the
    /// path's failed tab when there is one, otherwise on a new failed tab.
    #[test]
    fn unsubmitted_open_shows_its_error_in_a_failed_tab() {
        let (directory, mut workspace) = failed_open_fixture("unsubmitted");
        let path = directory.join("first.txt");
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert!(workspace.failed_open(0).is_some(), "{:?}", workspace.message);
        let error = workspace.fail_open_submission(path.clone(), true, "queue full".into());
        assert_eq!(error, "queue full");
        assert_eq!(workspace.editors.len(), 1);
        assert_eq!(workspace.failed_open(0), Some((path.as_path(), "queue full")));
        let other = directory.join("second.txt");
        workspace.fail_open_submission(other.clone(), true, "queue full".into());
        assert_eq!(workspace.titles(), ["first.txt (failed)", "second.txt (failed)"]);
        assert_eq!(workspace.failed_open(1), Some((other.as_path(), "queue full")));
        assert!(workspace.editors[1].read_only());
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// FIO-01: a resident open that publishes its prefix and then exhausts the
    /// byte budget becomes the paged document in that same tab, and the paged
    /// editor keeps its binary read-only guard instead of the preview's flag.
    #[test]
    fn resident_prefix_falls_back_to_paged_in_place_keeping_binary_guard() {
        let (directory, mut workspace) = failed_open_fixture("fallback");
        // Resident needs a 2 MiB raw copy plus ~4 MiB of text, past this budget
        // well after the first 64 KiB prefix; paged needs its ~3.75 MiB transcode
        // scratch, then at most the bounded page cache.
        workspace.bytes = Budget::new(5 << 20);
        workspace.page_cache_bytes = 1 << 20;
        let path = directory.join("tool.bin");
        // Control bytes flag the sample binary; 0xe9 is not UTF-8, so Windows-1252
        // keeps a raw copy and decodes each 0xe9 to the two-byte é.
        let mut raw = vec![0x01; 32 << 10];
        raw.resize(2 << 20, 0xe9);
        std::fs::write(&path, &raw).unwrap();
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert_eq!(workspace.editors.len(), 1, "{:?}", workspace.message);
        assert!(workspace.failed_open(0).is_none(), "{:?}", workspace.message);
        assert_eq!(workspace.titles(), ["tool.bin"]);
        let WorkspaceEditor::Paged(editor) = &workspace.editors[0] else {
            panic!("expected the paged fallback: {:?}", workspace.message);
        };
        assert_eq!(editor.snapshot().len(), (32 << 10) + 2 * ((2 << 20) - (32 << 10)));
        assert!(editor.user_read_only(), "binary file opened editable");
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// FIO-01: a reload whose resident decode exhausts the byte budget takes the
    /// paged fallback and replaces its own tab instead of adding a duplicate.
    #[test]
    fn resident_reload_falls_back_to_paged_in_the_same_tab() {
        let (directory, mut workspace) = failed_open_fixture("reload");
        let path = directory.join("legacy.bin");
        let mut raw = vec![0x01; 32 << 10];
        raw.resize(2 << 20, 0xe9);
        std::fs::write(&path, &raw).unwrap();
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert_eq!(workspace.editors.len(), 1, "{:?}", workspace.message);
        assert!(matches!(&workspace.editors[0], WorkspaceEditor::Resident(_)));
        let resident = workspace.editors[0].document_identity();
        assert!(
            workspace.editors[0].viewport().user_read_only,
            "binary guard on the resident open"
        );
        // A resident view in OVR; the paged replacement cannot overwrite.
        workspace.editors[0].viewport_mut().overwrite = true;
        // The same budget as the open fallback test: too small for the resident
        // reload, enough for the paged one.
        workspace.bytes = Budget::new(5 << 20);
        workspace.page_cache_bytes = 1 << 20;
        workspace.reload(0, false).unwrap();
        settle_open(&mut workspace);
        assert_eq!(workspace.editors.len(), 1, "{:?}", workspace.message);
        let WorkspaceEditor::Paged(editor) = &workspace.editors[0] else {
            panic!("expected the paged reload: {:?}", workspace.message);
        };
        assert_ne!(workspace.editors[0].document_identity(), resident);
        assert!(!editor.viewport().overwrite, "a paged reload returns to Insert (UI-07)");
        // The control bytes make this a binary sample, so the reloaded file is
        // guarded read-only again and RO outranks the mode (FIO-01, UI-07).
        assert!(editor.user_read_only(), "reloaded binary file opened editable");
        assert_eq!(editor.viewport().status_segments("Plain text")[5], "RO");
        assert_eq!(editor.snapshot().len(), (32 << 10) + 2 * ((2 << 20) - (32 << 10)));
        assert_eq!(workspace.path(0), Some(path.as_path()));
        // Accepting the bytes as editable text leaves the paged tab in Insert,
        // not the OVR the resident view had.
        workspace.encoding_accept_binary(0, false).unwrap();
        assert!(!workspace.editors[0].read_only());
        assert_eq!(workspace.editors[0].viewport().status_segments("Plain text")[5], "INS");
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// Pump like the shell: follow the active document through each pump.
    fn settle_shown(workspace: &mut Workspace, active: &mut usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let before = workspace.tab_documents();
            workspace.pump();
            *active = workspace.active_after_pump(&before, *active);
            if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
    }
    /// PED-23: a large-file open finishes in its own tab while another tab is
    /// active: the tab keeps its position, the other tabs keep theirs, the
    /// active document does not change, and the old tab maps to the new one.
    #[test]
    fn finished_paged_open_keeps_its_tab_and_the_active_document() {
        let (directory, mut workspace) = failed_open_fixture("in-place");
        workspace.new_document().unwrap();
        let path = directory.join("large.txt");
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert!(workspace.failed_open(1).is_some(), "{:?}", workspace.message);
        // The shell showed the failed tab as the open asked (APP-07); the user
        // then moves to another tab.
        assert_eq!(workspace.take_activation(None), Some(1));
        workspace.new_document().unwrap();
        let first = workspace.editors[0].document_identity();
        let failed = workspace.editors[1].document_identity();
        let other = workspace.editors[2].document_identity();
        std::fs::write(&path, "line\n".repeat(64)).unwrap();
        workspace.open_failed_as_large_file(1).unwrap();
        let mut active = 2;
        settle_shown(&mut workspace, &mut active);
        assert_eq!(
            workspace.titles(),
            ["Untitled 1", "large.txt", "Untitled 2"],
            "{:?}",
            workspace.message
        );
        assert!(matches!(&workspace.editors[1], WorkspaceEditor::Paged(_)));
        assert_eq!(workspace.editors[0].document_identity(), first);
        assert_eq!(workspace.editors[2].document_identity(), other);
        assert_eq!(active, 2);
        assert_eq!(
            workspace.replacement_document(failed.0),
            workspace.editors[1].document_identity().0
        );
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// PED-23/WSP-11: Reload and Interpret As give a tab a fresh document in
    /// place. The replacement is recorded like a finished open's, so the shell
    /// keeps the tab's position, pin and colour (only its view starts over) and
    /// the active tab follows it, instead of closing it and adding one at the end.
    #[test]
    fn reload_and_interpret_record_the_tab_they_replace() {
        // (paged, interpret): resident reload, resident Interpret As, paged
        // reload, paged Interpret As.
        for (paged, interpret) in [(false, false), (false, true), (true, false), (true, true)] {
            let (directory, mut workspace) = failed_open_fixture(&format!("replaced-{paged}-{interpret}"));
            if paged {
                workspace.resident_max_bytes = 4;
            }
            let path = directory.join("reload.txt");
            std::fs::write(&path, "disk text").unwrap();
            workspace.new_document().unwrap();
            workspace.open(path.clone());
            settle_open(&mut workspace);
            assert_eq!(workspace.editors.len(), 2, "{:?}", workspace.message);
            assert_eq!(workspace.editors[1].paged(), paged);
            let first = workspace.editors[0].document_identity();
            let old = workspace.editors[1].document_identity();
            if interpret {
                workspace
                    .encoding_interpret(1, bareline_file_io::codecs::Encoding::Latin1, true)
                    .unwrap();
            } else {
                workspace.reload(1, true).unwrap();
            }
            let mut active = 1;
            settle_shown(&mut workspace, &mut active);
            let case = (paged, interpret, workspace.message.clone());
            assert_eq!(workspace.editors.len(), 2, "{case:?}");
            assert_eq!(workspace.editors[0].document_identity(), first, "{case:?}");
            let new = workspace.editors[1].document_identity();
            assert_ne!(new, old, "the tab was not replaced: {case:?}");
            assert_eq!(workspace.replacement_document(old.0), new.0, "{case:?}");
            assert_eq!(active, 1, "{case:?}");
            drop(workspace);
            let _ = std::fs::remove_dir_all(directory);
        }
    }
    /// PED-24: a large file opened again under another spelling (the fixture
    /// gives every file one identity) resolves to the tab already holding it:
    /// no second tab, the launch outcome names the existing document, a tab
    /// closed before the active one keeps the active document, and a
    /// duplicate open's own tab hands focus to the existing tab.
    #[test]
    fn duplicate_paged_open_resolves_to_the_existing_tab() {
        let (directory, mut workspace) = failed_open_fixture("duplicate");
        workspace.resident_max_bytes = 4;
        let path = directory.join("large.txt");
        std::fs::write(&path, "line\n".repeat(64)).unwrap();
        workspace.open(path.clone());
        settle_open(&mut workspace);
        assert!(
            matches!(&workspace.editors[0], WorkspaceEditor::Paged(_)),
            "{:?}",
            workspace.message
        );
        let existing = workspace.editors[0].document_identity();
        // The alias's failed tab sits before the active tab.
        let alias = directory.join("alias.txt");
        workspace.open(alias.clone());
        settle_open(&mut workspace);
        assert!(workspace.failed_open(1).is_some(), "{:?}", workspace.message);
        // The shell showed the failed tab as the open asked (APP-07); the user
        // then moves to another tab.
        assert_eq!(workspace.take_activation(None), Some(1));
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        let shown = workspace.editors[2].document_identity();
        std::fs::write(&alias, "line\n".repeat(64)).unwrap();
        workspace.open_tracked(9, alias.clone()).unwrap();
        let mut active = 2;
        settle_shown(&mut workspace, &mut active);
        assert_eq!(
            workspace.titles(),
            ["large.txt", "Untitled 1", "Untitled 2"],
            "{:?}",
            workspace.message
        );
        assert_eq!(
            workspace.take_tracked_open_outcomes(&[9]),
            [LaunchOpenOutcome::Opened {
                request_id: 9,
                document: existing,
            }]
        );
        assert_eq!(workspace.message.as_deref(), Some("File is already open in tab 1."));
        assert_eq!(workspace.editors[active].document_identity(), shown);
        // A duplicate shown in its own tab focuses the existing tab.
        let again = directory.join("again.txt");
        workspace.open(again.clone());
        settle_shown(&mut workspace, &mut active);
        assert_eq!(workspace.failed_open(3).map(|(path, _)| path), Some(again.as_path()));
        assert_eq!(active, 3);
        std::fs::write(&again, "line\n".repeat(64)).unwrap();
        workspace.open_failed_as_large_file(3).unwrap();
        settle_shown(&mut workspace, &mut active);
        assert_eq!(workspace.editors.len(), 3, "{:?}", workspace.message);
        assert_eq!(active, 0);
        assert_eq!(workspace.editors[0].document_identity(), existing);
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// PED-25: converting a paged selection's newlines under a fold converts
    /// the selected source line, not the text at its viewport offsets.
    #[test]
    fn paged_selection_eol_conversion_uses_source_offsets_under_folds() {
        use bareline_document::TextOffset;
        let (directory, mut workspace) = failed_open_fixture("eol-fold");
        workspace.resident_max_bytes = 4;
        let path = directory.join("folded.txt");
        let saved = directory.join("saved.txt");
        let content = format!("header\r\n{}tail one\r\ntail two\r\n", "interior\r\n".repeat(200));
        std::fs::write(&path, &content).unwrap();
        workspace.open(path);
        settle_open(&mut workspace);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !matches!(&workspace.editors[0], WorkspaceEditor::Paged(editor) if editor.viewport_ready()) {
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            workspace.pump();
            std::thread::yield_now();
        }
        let WorkspaceEditor::Paged(editor) = &mut workspace.editors[0] else {
            unreachable!()
        };
        // Lines 1..=200 fold under the header, so the tail follows it in the viewport.
        editor
            .set_known_global_folds(
                vec![bareline_syntax::folding::Fold {
                    header: 0,
                    end: 200,
                    level: 1,
                }],
                1,
                false,
                0,
            )
            .unwrap();
        editor.fold_all_known(1);
        assert_eq!(editor.persisted_global_folds(), vec![0..201]);
        loop {
            assert!(std::time::Instant::now() < deadline);
            workspace.pump();
            let WorkspaceEditor::Paged(editor) = &workspace.editors[0] else {
                unreachable!()
            };
            if editor.paged_frame_state().ready && editor.source_segments().len() > 1 {
                break;
            }
            std::thread::yield_now();
        }
        let tail = content.find("tail one").unwrap();
        let tail_end = tail + "tail one\r\n".len();
        let WorkspaceEditor::Paged(editor) = &mut workspace.editors[0] else {
            unreachable!()
        };
        let token = editor
            .restore_global_selection(TextOffset(tail), TextOffset(tail_end), true)
            .unwrap();
        loop {
            assert!(std::time::Instant::now() < deadline);
            workspace.pump();
            let WorkspaceEditor::Paged(editor) = &workspace.editors[0] else {
                unreachable!()
            };
            match editor.selection_restore_status(token) {
                bareline_editor_surface::paged_view::SelectionRestoreStatus::Pending => {}
                bareline_editor_surface::paged_view::SelectionRestoreStatus::Applied => break,
                status => panic!("selection restore: {status:?}"),
            }
            std::thread::yield_now();
        }
        let WorkspaceEditor::Paged(editor) = &workspace.editors[0] else {
            unreachable!()
        };
        assert_eq!(editor.global_selection(), (TextOffset(tail), TextOffset(tail_end)));
        let local = editor.viewport().selection;
        assert_ne!(
            editor.viewport_start().0 + local.anchor.min(local.caret),
            tail,
            "the fold must separate viewport offsets from source offsets"
        );
        workspace
            .encoding_eol(0, bareline_file_io::codecs::state::Eol::Lf, true)
            .unwrap();
        settle_open(&mut workspace);
        workspace.save(0, saved.clone());
        settle_open(&mut workspace);
        assert_eq!(
            std::fs::read_to_string(&saved).unwrap(),
            content.replacen("tail one\r\n", "tail one\n", 1),
            "{:?}",
            workspace.message
        );
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
    fn rename_rebinds_the_open_document_instead_of_reopening_it() {
        let root = std::env::temp_dir().join(format!(
            "bareline-rename-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("before.txt");
        let target = root.join("after.txt");
        std::fs::write(&source, b"kept text").unwrap();
        // The fixture reports one file identity for every path, as a
        // same-volume rename does for the moved file.
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.open(source.clone());
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
        assert_eq!(workspace.editors.len(), 1);
        std::fs::rename(&source, &target).unwrap();
        // Reopening the moved file is deduplicated against the open document.
        workspace.open(target.clone());
        settle(&mut workspace);
        assert_eq!(workspace.editors.len(), 1);
        assert_eq!(workspace.path(0), Some(source.as_path()));
        assert_eq!(workspace.rebind_path(0, target.clone()), Ok(source.clone()));
        assert_eq!(workspace.editors.len(), 1);
        assert_eq!(workspace.path(0), Some(target.as_path()));
        assert_eq!(workspace.titles(), vec!["after.txt".to_owned()]);
        let serialized = |path: &std::path::Path| bareline_platform::SerializedPath::from_native(path).data;
        assert_eq!(
            workspace.recent_paths().first().map(|path| path.data.clone()),
            Some(serialized(&target))
        );
        assert!(
            !workspace
                .recent_paths()
                .iter()
                .any(|path| path.data == serialized(&source))
        );
        // The next save writes under the new name; the old name stays gone.
        workspace.editors[0].enqueue(Input::Insert("more ".into()));
        settle(&mut workspace);
        assert!(workspace.save(0, target.clone()));
        settle(&mut workspace);
        assert!(!workspace.editors[0].dirty(), "{:?}", workspace.message);
        let saved = std::fs::read_to_string(&target).unwrap();
        assert!(saved.contains("more ") && saved.contains("kept text"), "{saved}");
        assert!(!source.exists());
        drop(workspace);
        remove_test_directory(root);
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
        // Distinct identities: a paged open of the same file is a duplicate (PED-24).
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(DistinctOpenFileSystem)).unwrap();
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
        assert!(workspace.editors[resident].selected_text(usize::MAX).is_ok());
        // The same operation on a paged editor reports NotSupportedForPaged
        // instead of silently reading the bounded viewport placeholder.
        let error = workspace.editors[paged]
            .selected_text(usize::MAX)
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
    fn selection_newline_conversion_never_splits_a_crlf_for_resident_and_paged() {
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
            "bareline-eol-edge-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        for paged in [false, true] {
            let source = root.join(if paged { "paged.txt" } else { "resident.txt" });
            let copy = root.join(if paged { "paged-copy.txt" } else { "resident-copy.txt" });
            // Bytes: a0 \r1 \n2 b3 \r4 \n5 c6 \r7 d8; the selection 2..5 splits both CRLFs.
            std::fs::write(&source, b"a\r\nb\r\nc\rd").unwrap();
            let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            if paged {
                workspace.resident_max_bytes = 4;
            }
            workspace.open(source);
            settle(&mut workspace);
            assert_eq!(workspace.editors[0].paged(), paged);
            match &mut workspace.editors[0] {
                WorkspaceEditor::Paged(editor) => editor
                    .restore_selection(bareline_document::TextOffset(2), bareline_document::TextOffset(5))
                    .unwrap(),
                editor => {
                    editor.enqueue(Input::SetCaret(2, false));
                    editor.enqueue(Input::SetCaret(5, true));
                }
            }
            settle(&mut workspace);
            workspace
                .encoding_eol(0, bareline_file_io::codecs::state::Eol::Lf, true)
                .unwrap();
            settle(&mut workspace);
            workspace.save_copy(0, copy.clone());
            settle(&mut workspace);
            assert_eq!(
                std::fs::read(&copy).unwrap(),
                b"a\nb\nc\rd",
                "paged={paged} {:?}",
                workspace.message
            );
        }
        remove_test_directory(root);
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
            // FIO-11: the user's spelling, never the `\\?\` canonical form.
            assert_eq!(prepared_path, target);
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
            // A restore the user asked for activates its tab, whether the recovered
            // text is adopted in memory (untitled) or reopened paged (APP-07).
            assert_eq!(restored.take_activation(None), Some(0));
            assert!(restored.activation_requests.is_empty());
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
    fn settle_reload(workspace: &mut Workspace) {
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
    /// Pump until the first tab's current revision is durably journaled in a
    /// checkpoint other than `previous`. Any recovery error, such as the
    /// `WrongDocument` of a recovery owner still bound to replaced text, fails.
    fn durable_checkpoint(workspace: &mut Workspace, previous: Option<&std::path::Path>) -> PathBuf {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            workspace.pump();
            let revision = match &workspace.editors[0] {
                WorkspaceEditor::Resident(editor) => editor.snapshot().revision.0,
                WorkspaceEditor::Paged(editor) => editor.snapshot().revision.0,
            };
            let status = workspace.editors[0].recovery_status();
            assert!(status.error.is_none(), "recovery unavailable: {:?}", status.error);
            if status.complete
                && status.durable.is_some_and(|durable| durable.revision == revision)
                && let Some(directory) = status.directory
                && previous != Some(directory.as_path())
            {
                return directory;
            }
            assert!(std::time::Instant::now() < deadline, "recovery never became durable");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    /// What the Recovery Center would offer: every journal that is not a
    /// durable discard tombstone.
    fn offered_recovery(directory: &std::path::Path) -> bool {
        bareline_file_io::recovery::inspect(directory, &Default::default())
            .is_ok_and(|inspection| inspection.status != bareline_file_io::recovery::RecoveryStatus::Discarded)
    }
    fn offered_recovery_texts(root: &std::path::Path) -> Vec<String> {
        let mut texts = Vec::new();
        for entry in std::fs::read_dir(root).unwrap() {
            let directory = entry.unwrap().path();
            if !directory.is_dir()
                || !directory
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("paged-"))
                || !offered_recovery(&directory)
            {
                continue;
            }
            let budget = bareline_document::Budget::new(256 << 20);
            let mut opened = bareline_file_io::paged_recovery::restore(
                &directory,
                Arc::new(PagedFileSystem),
                budget.clone(),
                bareline_document::Budget::new(0),
                &Default::default(),
            )
            .unwrap();
            texts.push(bareline_file_io::paged_recovery::preview(&mut opened, &budget, &Default::default()).unwrap());
        }
        texts
    }
    #[test]
    fn dirty_reload_and_interpret_recover_only_the_reloaded_text() {
        // (paged, interpret): resident reload, resident Interpret As, paged reload.
        for (paged, interpret) in [(false, false), (false, true), (true, false)] {
            let root = std::env::temp_dir().join(format!(
                "bareline-reload-recovery-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            let recovery = root.join("recovery");
            let path = root.join("reload.txt");
            std::fs::write(&path, "disk text").unwrap();
            let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            workspace.recovery_root = Some(recovery.clone());
            if paged {
                workspace.resident_max_bytes = 4;
            }
            workspace.open(path.clone());
            settle_reload(&mut workspace);
            assert_eq!(workspace.editors[0].paged(), paged);
            workspace.editors[0].enqueue(Input::SetCaret(0, false));
            workspace.editors[0].enqueue(Input::Insert("discarded ".into()));
            settle_reload(&mut workspace);
            let discarded = durable_checkpoint(&mut workspace, None);

            if interpret {
                workspace
                    .encoding_interpret(0, bareline_file_io::codecs::Encoding::Latin1, true)
                    .unwrap();
            } else {
                workspace.reload(0, true).unwrap();
            }
            settle_reload(&mut workspace);
            assert_eq!(workspace.message.as_deref(), Some("Reloaded from disk."));
            assert_eq!(workspace.editors.len(), 1);
            assert_eq!(workspace.editors[0].paged(), paged);
            assert!(!workspace.editors[0].dirty());
            assert!(
                !workspace.editors[0].can_undo(),
                "reload kept the discarded text's undo history"
            );

            workspace.editors[0].enqueue(Input::SetCaret(0, false));
            workspace.editors[0].enqueue(Input::Insert("kept ".into()));
            settle_reload(&mut workspace);
            durable_checkpoint(&mut workspace, Some(&discarded));
            // Simulated crash: nothing is dropped or cleaned up.
            std::mem::forget(workspace);

            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            // A resident discard purges at once; a paged one keeps its durable
            // tombstone while the retired editor still holds its source.
            while offered_recovery(&discarded) || (!paged && discarded.exists()) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the discarded pre-reload text is still offered for recovery"
                );
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert_eq!(offered_recovery_texts(&recovery), ["kept disk text"]);
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[test]
    fn paged_reload_survives_viewport_reads_while_it_runs() {
        let directory = std::env::temp_dir().join(format!(
            "bareline-paged-reload-scroll-{}-{}",
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
        settle_reload(&mut workspace);
        std::fs::write(&path, "b".repeat(12288)).unwrap();
        workspace.reload(0, false).unwrap();
        // Scrolling replaces the viewport projection; the document is unchanged.
        assert!(workspace.editors[0].page_by(false));
        settle_reload(&mut workspace);
        assert_eq!(workspace.message.as_deref(), Some("Reloaded from disk."));
        assert!(matches!(&workspace.editors[0], WorkspaceEditor::Paged(editor) if editor.snapshot().len() == 12288));
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
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
        let mut shown = Vec::new();
        loop {
            shown.extend(restored.tab_documents());
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
        // A loading tab the restore showed hands its place to the adopted
        // editor, so the shell keeps that tab and its focus (PED-23).
        let adopted = restored.editors[0].document_identity().0;
        for document in shown {
            assert_eq!(restored.replacement_document(document), adopted);
        }
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
        // The open runs on the bulk lane and the interrupted-save scan of its
        // folder on the save lane (FIO-14), so the first wake may be the scan's.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.editors.is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "no streaming prefix was published"
            );
            received.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
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
                .any(|op| matches!(op, DrawOp::Text { text, .. } if text.ends_with(" · 160001 lines")))
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
    fn settle_test_edits(workspace: &mut Workspace) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        // Queued input may not yet have been admitted to a worker, so it has no
        // completion notification. Keep driving admission until the edits settle.
        while workspace.editors.iter().any(WorkspaceEditor::busy) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "test edits did not settle");
            std::thread::yield_now();
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
        settle_test_edits(&mut workspace);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        assert_eq!(workspace.close(0, false, &mut renderer), Err(CloseError::Unsaved));
        let saving = workspace.editors[2].snapshot().clone();
        workspace.save(2, PathBuf::from("unused-fixture.txt"));
        assert_eq!(workspace.close(2, true, &mut renderer), Err(CloseError::Busy));
        workspace.close(0, true, &mut renderer).unwrap();
        assert_eq!(workspace.titles()[0], "Untitled 2");
        assert!(workspace.editors[1].snapshot().same_document(&saving));
        let target = workspace.pending_io[0].save.as_ref().unwrap().0;
        assert_eq!(workspace.tab_index(target), Some(1));
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
        settle_test_edits(&mut workspace);
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
        settle_test_edits(&mut workspace);
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
        settle_test_edits(&mut workspace);
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
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while workspace.pending_replace.is_some()
                || workspace.editors[0].busy()
                || workspace.find.status == "Searching…"
            {
                workspace.pump();
                if workspace.pending_replace.is_some()
                    || workspace.editors[0].busy()
                    || workspace.find.status == "Searching…"
                {
                    assert!(std::time::Instant::now() < deadline, "replace fixture did not settle");
                    match notified.recv_timeout(std::time::Duration::from_millis(1)) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(error) => panic!("{error}"),
                    }
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
    fn refused_replace_preparation_replaces_the_preparing_status_with_a_reason() {
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
        workspace.editors[0].enqueue(Input::Insert("cat dog".into()));
        let drain = |workspace: &mut Workspace| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while workspace.pending_replace.is_some()
                || workspace.editors[0].busy()
                || workspace.find.status == "Searching…"
            {
                workspace.pump();
                if workspace.pending_replace.is_some()
                    || workspace.editors[0].busy()
                    || workspace.find.status == "Searching…"
                {
                    assert!(std::time::Instant::now() < deadline, "replace fixture did not settle");
                    match notified.recv_timeout(std::time::Duration::from_millis(1)) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(error) => panic!("{error}"),
                    }
                }
            }
        };
        drain(&mut workspace);
        workspace.find.show_replace();
        workspace.find.field.insert("cat");
        workspace.find.replacement.insert("cow");
        workspace
            .find
            .refresh(workspace.editors[0].snapshot(), workspace.notify.clone());
        drain(&mut workspace);
        // Replace (one) with the caret after "dog": the worker refuses with NoMatch.
        workspace.replace(0, false);
        assert!(workspace.pending_replace.is_some());
        assert_eq!(workspace.find.status, "Preparing replacement…");
        drain(&mut workspace);
        assert_eq!(
            workspace.find.status,
            "Replacement was not applied: no matches to replace."
        );
        assert_eq!(workspace.message.as_deref(), Some(workspace.find.status.as_str()));
        assert_eq!(workspace.editors[0].snapshot().len(), "cat dog".len());
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
    #[test]
    fn binary_file_in_open_batch_gets_named_notice_without_blocking_other_opens() {
        let root = std::env::temp_dir().join(format!(
            "bareline-binary-batch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        // DistinctOpenFileSystem derives file identity from length, so every
        // fixture has a different length and none is treated as a duplicate.
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(DistinctOpenFileSystem)).unwrap();
        for index in 0..10 {
            let path = if index == 2 {
                let path = root.join("payload.bin");
                std::fs::write(&path, [0u8, 1, 2, 3, b'a'].repeat(40)).unwrap();
                path
            } else {
                let path = root.join(format!("text-{index}.txt"));
                std::fs::write(&path, "x".repeat(index + 1)).unwrap();
                path
            };
            workspace.open(path);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() || workspace.editors.iter().any(WorkspaceEditor::busy) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        assert_eq!(workspace.editors.len(), 10, "{:?}", workspace.message);
        let binary = (0..10)
            .find(|&index| {
                workspace.path(index).and_then(std::path::Path::file_name) == Some(std::ffi::OsStr::new("payload.bin"))
            })
            .expect("binary document opened");
        for index in 0..10 {
            assert_eq!(workspace.binary_notice(index).is_some(), index == binary, "{index}");
        }
        assert_eq!(
            workspace.binary_notice(binary).unwrap(),
            "payload.bin contains binary-like bytes. It is open read-only."
        );
        assert!(workspace.editors[binary].read_only());
        // The view reserves a band for the notice above the text, so the notice
        // never covers the file's first lines; other documents reserve nothing.
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut operations = Vec::new();
        let text = (binary + 1) % 10;
        for (index, inset) in [(binary, crate::encoding::BINARY_NOTICE_HEIGHT), (text, 0.0)] {
            workspace
                .draw(index, &mut renderer, 1100.0, 700.0, &mut operations)
                .unwrap();
            assert_eq!(workspace.editors[index].viewport().top_inset, inset, "{index}");
        }
        workspace.encoding_accept_binary(binary, false).unwrap();
        assert!(workspace.binary_notice(binary).is_none());
        assert!(!workspace.editors[binary].read_only());
        workspace
            .draw(binary, &mut renderer, 1100.0, 700.0, &mut operations)
            .unwrap();
        assert_eq!(workspace.editors[binary].viewport().top_inset, 0.0);
        drop(workspace);
        remove_test_directory(root);
    }
    #[test]
    fn ambiguous_legacy_open_names_likely_encodings() {
        let root = std::env::temp_dir().join(format!(
            "bareline-encoding-hint-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("short.txt");
        // "你好世界" in GBK is too short to decide, so it opens in the default
        // encoding with a non-blocking hint naming GBK (FIO-05).
        std::fs::write(&path, [0xc4, 0xe3, 0xba, 0xc3, 0xca, 0xc0, 0xbd, 0xe7]).unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.open(path);
        settle_reload(&mut workspace);
        assert_eq!(workspace.editors.len(), 1, "{:?}", workspace.message);
        let hint = workspace.encoding_hint(0).expect("ambiguous detection hint");
        assert!(hint.starts_with("short.txt: Encoding may be wrong"), "{hint}");
        assert!(hint.contains("GBK (Simplified Chinese)"), "{hint}");
        assert_eq!(workspace.message.as_deref(), Some(hint.as_str()));
        drop(workspace);
        remove_test_directory(root);
    }

    fn activation_fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bareline-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn only_explicit_opens_ask_for_their_tab_to_be_activated() {
        let root = activation_fixture("activation");
        let restored = root.join("restored.txt");
        let requested = root.join("requested.txt");
        std::fs::write(&restored, "restored in the background\n").unwrap();
        std::fs::write(&requested, "requested\n").unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(DistinctOpenFileSystem)).unwrap();
        workspace.new_document().unwrap();
        assert_eq!(workspace.take_activation(None), None);
        // A session file that finishes loading never moves the active tab (APP-07).
        workspace.open_in_background(restored.clone());
        settle_open(&mut workspace);
        assert_eq!(workspace.editors.len(), 2);
        assert_eq!(workspace.take_activation(None), None);
        workspace.open(requested.clone());
        settle_open(&mut workspace);
        let index = workspace
            .take_activation(None)
            .expect("an explicit open activates its tab");
        assert_eq!(workspace.path(index), Some(requested.as_path()));
        assert_eq!(workspace.take_activation(None), None);
        // Activation requests never leak into the launch outcome queue.
        assert!(workspace.activation_requests.is_empty());
        assert!(workspace.open_outcomes.is_empty());
        drop(workspace);
        remove_test_directory(root);
    }

    #[test]
    fn an_approved_remote_open_activates_its_tab() {
        /// Grants every approved remote read the ordinary test file system.
        struct RemoteFileSystem;
        impl LocalFileSystem for RemoteFileSystem {
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
                PagedFileSystem.identity(file)
            }
            fn validate_target(&self, path: &std::path::Path) -> std::io::Result<()> {
                PagedFileSystem.validate_target(path)
            }
            fn commit(&self, staged: &std::path::Path, target: &std::path::Path, existed: bool) -> std::io::Result<()> {
                PagedFileSystem.commit(staged, target, existed)
            }
            fn scoped_remote_read(
                &self,
                _: bareline_platform::RemoteReadAccess,
            ) -> std::io::Result<std::sync::Arc<dyn LocalFileSystem>> {
                Ok(Arc::new(PagedFileSystem))
            }
        }
        let root = activation_fixture("remote-open");
        let remote = root.join("remote.txt");
        std::fs::write(&remote, "approved remote text\n").unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(RemoteFileSystem)).unwrap();
        workspace.new_document().unwrap();
        let grant = bareline_platform::RemoteReadGrant::after_consent(
            remote.clone(),
            bareline_platform::RemoteReadAction::Open,
            std::time::Duration::from_secs(60),
        )
        .unwrap();
        workspace.open_authorized(remote.clone(), grant).unwrap();
        settle_open(&mut workspace);
        // The user approved this open, so its tab is shown (APP-07).
        let index = workspace
            .take_activation(None)
            .unwrap_or_else(|| panic!("remote tab not activated: {:?}", workspace.message));
        assert_eq!(workspace.path(index), Some(remote.as_path()));
        assert!(workspace.activation_requests.is_empty());
        assert!(workspace.open_outcomes.is_empty());
        drop(workspace);
        remove_test_directory(root);
    }

    #[test]
    fn restored_clean_tab_reopens_read_only_and_hands_over_its_tab() {
        let root = activation_fixture("restore-closed");
        let saved = root.join("saved.txt");
        std::fs::write(&saved, "alpha\nbeta\ngamma\n").unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(DistinctOpenFileSystem)).unwrap();
        workspace.new_document().unwrap();
        workspace.open(saved.clone());
        settle_open(&mut workspace);
        let index = workspace.take_activation(None).unwrap();
        let closed = workspace.editors[index].document_identity().0;
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(index, false, &mut renderer).unwrap();
        workspace.set_last_closed_read_only(true);
        // Once a worker finds its file (APP-19), a saved document is remembered by
        // path, keeping the read-only choice made meanwhile, and comes back as a
        // new document.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.closed_checks_pending() {
            workspace.pump();
            assert!(
                std::time::Instant::now() < deadline,
                "the closed file was never checked"
            );
            std::thread::yield_now();
        }
        assert_eq!(workspace.restore_last_closed(), None);
        settle_open(&mut workspace);
        let restored = workspace.take_activation(None).expect("the restored tab is activated");
        assert_eq!(workspace.path(restored), Some(saved.as_path()));
        assert!(workspace.editors[restored].read_only());
        let document = workspace.editors[restored].document_identity().0;
        assert_ne!(document, closed);
        // The shell moves the closed tab (pin, position, view) to the new document.
        let handovers = workspace.take_reopened_tabs();
        assert_eq!(handovers.first().map(|pair| pair.0), Some(closed));
        assert_eq!(handovers.last().map(|pair| pair.1), Some(document));
        assert!(workspace.activation_requests.is_empty());
        drop(workspace);
        remove_test_directory(root);
    }

    /// Plays the tab a request shows and the document that replaces it, the way
    /// the pump and its completions do, so the order is exact (APP-07).
    #[test]
    fn a_replaced_loading_tab_moves_focus_only_while_the_user_is_on_it() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(DistinctOpenFileSystem)).unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        let existing = workspace.editors[0].document_identity();
        let elsewhere = workspace.editors[1].document_identity().0;
        let loading_tab = |workspace: &mut Workspace, request: u64| {
            workspace.new_document().unwrap();
            let index = workspace.editors.len() - 1;
            let loading = workspace.editors[index].document_identity().0;
            workspace.show_activation(request, loading);
            // The first tab a request shows is always activated.
            assert_eq!(workspace.take_activation(Some(elsewhere)), Some(index));
            (index, loading)
        };
        // The finished document takes the loading tab's slot in place, as a paged
        // completion, paged fallback or transcode does, after the user has
        // switched to another tab: focus stays where the user put it.
        let request = workspace.request_activation(PathBuf::from("large.txt"), None);
        let (index, _) = loading_tab(&mut workspace, request);
        workspace.new_document().unwrap();
        let finished = workspace.editors.pop().unwrap();
        workspace.tabs.pop();
        let document = finished.document_identity();
        workspace
            .retired
            .push(std::mem::replace(&mut workspace.editors[index], finished));
        workspace.record_launch_open(Some(request), Ok(document));
        assert_eq!(workspace.take_activation(Some(elsewhere)), None);
        assert!(workspace.activation_requests.is_empty());
        // A result that lands in another tab (already open, created, recovered)
        // is shown only if the user is still on the loading tab it replaces.
        for kept in [false, true] {
            let request = workspace.request_activation(PathBuf::from("open.txt"), None);
            let (index, loading) = loading_tab(&mut workspace, request);
            let preview = workspace.editors[index].snapshot().clone();
            workspace.discard_preview(Some(&preview));
            workspace.record_launch_open(Some(request), Ok(existing));
            let active = if kept { loading } else { elsewhere };
            let expected = kept.then_some(0);
            assert_eq!(workspace.take_activation(Some(active)), expected, "kept: {kept}");
        }
        // Both steps within one pump: the loading tab was never shown as active,
        // so its successor is.
        let request = workspace.request_activation(PathBuf::from("quick.txt"), None);
        workspace.new_document().unwrap();
        let index = workspace.editors.len() - 1;
        let preview = workspace.editors[index].snapshot().clone();
        workspace.show_activation(request, preview.identity_token().0);
        workspace.discard_preview(Some(&preview));
        workspace.record_launch_open(Some(request), Ok(existing));
        assert_eq!(workspace.take_activation(Some(elsewhere)), Some(0));
        assert!(workspace.activation_requests.is_empty());
    }

    /// The real paged path: activating the loading tab, then switching away
    /// from it, must survive the completion that replaces it (APP-07).
    #[test]
    fn a_large_open_finishing_after_the_user_left_its_tab_keeps_their_tab() {
        let root = activation_fixture("paged-focus");
        let large = root.join("large.txt");
        std::fs::write(&large, "line abc\r\n".repeat(20000)).unwrap();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.resident_max_bytes = 4;
        workspace.new_document().unwrap();
        let elsewhere = workspace.editors[0].document_identity().0;
        workspace.open(large.clone());
        let mut activations = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            workspace.pump();
            // As the shell does after each pump; the user then switches back at once.
            if let Some(index) = workspace.take_activation(Some(elsewhere)) {
                activations += 1;
                assert_ne!(workspace.editors[index].document_identity().0, elsewhere);
            }
            if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        assert_eq!(activations, 1, "only the open's first tab is activated");
        assert_eq!(workspace.editors.len(), 2, "{:?}", workspace.message);
        assert!(workspace.editors[1].paged());
        assert_eq!(workspace.path(1), Some(large.as_path()));
        assert!(workspace.activation_requests.is_empty());
        drop(workspace);
        remove_test_directory(root);
    }

    #[test]
    fn missing_launch_path_opens_a_document_its_first_save_creates() {
        let root = activation_fixture("create-missing");
        let missing = root.join("new notes.txt");
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(DistinctOpenFileSystem)).unwrap();
        workspace.open_tracked_or_create(7, missing.clone()).unwrap();
        settle_open(&mut workspace);
        let outcomes = workspace.take_tracked_open_outcomes(&[7]);
        let [LaunchOpenOutcome::Opened { document, .. }] = outcomes.as_slice() else {
            panic!("{outcomes:?} {:?}", workspace.message);
        };
        assert_eq!(workspace.editors.len(), 1, "no failed tab besides the new document");
        assert_eq!(workspace.editors[0].document_identity(), *document);
        assert!(workspace.failed_open(0).is_none());
        assert_eq!(workspace.path(0), None);
        assert_eq!(workspace.create_target(0), Some(missing.as_path()));
        assert_eq!(workspace.titles()[0], "new notes.txt");
        assert!(!missing.exists(), "nothing is created before the first save");
        assert!(workspace.missing_launches.is_empty());
        // Other tracked opens of a missing file still fail as before.
        workspace.open_tracked(8, missing.clone()).unwrap();
        settle_open(&mut workspace);
        assert!(matches!(
            workspace.take_tracked_open_outcomes(&[8]).as_slice(),
            [LaunchOpenOutcome::Failed { request_id: 8, .. }]
        ));
        // A read-only or monitored launch of a missing file gets one plain notice
        // and no failed-open tab (APP-21).
        let absent = root.join("absent.txt");
        let tabs = workspace.editors.len();
        workspace.open_tracked_or_report(9, absent.clone()).unwrap();
        settle_open(&mut workspace);
        let expected = missing_file_message(&absent);
        assert_eq!(
            workspace.take_tracked_open_outcomes(&[9]),
            vec![LaunchOpenOutcome::Failed {
                request_id: 9,
                error: expected.clone(),
            }]
        );
        assert_eq!(workspace.editors.len(), tabs, "no failed-open tab for a missing file");
        assert_eq!(workspace.message.as_deref(), Some(expected.as_str()));
        assert!(!absent.exists() && workspace.missing_launches.is_empty());
        drop(workspace);
        remove_test_directory(root);
    }

    #[test]
    fn piped_text_becomes_an_unsaved_untitled_document() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(DistinctOpenFileSystem)).unwrap();
        let index = workspace
            .new_document_with_text("piped line\nsecond line\n".into())
            .unwrap();
        settle_open(&mut workspace);
        let editor = &workspace.editors[index];
        let snapshot = editor.snapshot();
        let text = snapshot
            .read(
                bareline_document::TextOffset(0)..bareline_document::TextOffset(snapshot.len()),
                1024,
            )
            .unwrap();
        assert!(text.contains("piped line") && text.contains("second line"), "{text:?}");
        assert!(editor.dirty(), "closing must offer to save the piped text");
        assert_eq!(workspace.path(index), None);
        assert!(workspace.titles()[index].starts_with("Untitled"));
    }
    /// WSP-02: retained closed documents are bounded by bytes as well as count.
    #[test]
    fn closed_history_is_bounded_by_retained_bytes_and_yields_under_pressure() {
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let mut workspace = Workspace::new(notify.clone(), Arc::new(PagedFileSystem)).unwrap();
        // A quarter of this budget (2 MiB) is the retained-history share.
        workspace.bytes = Budget::new(8 << 20);
        let retained = |text: &str| {
            let document = Document::from_utf8(text, Budget::new(16 << 20), Budget::new(1 << 20)).unwrap();
            let editor: WorkspaceEditor = EditorSurface::loading(document.snapshot(), notify.clone()).into();
            ClosedDocument::Retained(Box::new(editor), None, "Untitled".into())
        };
        let reopen_entry = |path: &str| Reopen {
            path: PathBuf::from(path),
            document: 0,
            read_only: false,
        };
        for _ in 0..3 {
            workspace.closed.push(retained(&"x".repeat(900 << 10)));
            workspace.trim_closed_history();
        }
        assert_eq!(workspace.closed.len(), 2, "the oldest retained text was not evicted");
        workspace
            .closed
            .insert(0, ClosedDocument::Reopen(reopen_entry("kept.txt")));
        let pressure = workspace.bytes.claim(7 << 20).unwrap();
        workspace.trim_closed_history();
        assert!(matches!(workspace.closed.as_slice(), [ClosedDocument::Reopen(_)]));
        drop(pressure);
        for _ in 0..25 {
            workspace
                .closed
                .push(ClosedDocument::Reopen(reopen_entry("reopen.txt")));
        }
        workspace.trim_closed_history();
        assert_eq!(workspace.closed.len(), MAX_CLOSED_DOCUMENTS);
    }
    /// WSP-02 with APP-19: a saved closed document still waiting for its file
    /// check yields its model under pressure but stays restorable by path.
    #[test]
    fn closed_history_trim_keeps_a_saved_document_awaiting_its_check_by_path() {
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let mut workspace = Workspace::new(notify.clone(), Arc::new(PagedFileSystem)).unwrap();
        workspace.bytes = Budget::new(8 << 20);
        let retained = |text: &str| {
            let document = Document::from_utf8(text, Budget::new(16 << 20), Budget::new(1 << 20)).unwrap();
            WorkspaceEditor::from(EditorSurface::loading(document.snapshot(), notify.clone()))
        };
        let saved = retained("saved text");
        let saved_identity = saved.snapshot().identity_token();
        let saved_document = saved.document_identity().0;
        workspace
            .closed
            .push(ClosedDocument::Retained(Box::new(saved), None, "saved.txt".into()));
        workspace.closed_checks.push(ClosedCheck {
            identity: saved_identity,
            path: PathBuf::from("saved.txt"),
            task: None,
        });
        workspace.closed.push(ClosedDocument::Retained(
            Box::new(retained("unsaved text")),
            None,
            "Untitled".into(),
        ));
        let pressure = workspace.bytes.claim(7 << 20).unwrap();
        workspace.trim_closed_history();
        drop(pressure);
        assert!(matches!(
            workspace.closed.as_slice(),
            [ClosedDocument::Reopen(reopen)]
                if reopen.path == std::path::Path::new("saved.txt") && reopen.document == saved_document
        ));
        assert!(workspace.closed_checks.is_empty(), "a queued check outlived its model");
    }
    /// WSP-04: closing a loading tab settles its launch request and leaves only
    /// a reopen-by-path entry, never the read-only loading preview.
    #[test]
    fn closing_a_loading_tab_settles_its_launch_and_remembers_only_the_path() {
        let (directory, mut workspace) = failed_open_fixture("close-loading");
        let path = directory.join("loading.txt");
        std::fs::write(&path, b"loading text\n").unwrap();
        workspace.open_tracked(7, path.clone()).unwrap();
        assert_eq!(workspace.pending_io.len(), 1);
        // Stand in for the published prefix; the completion stays unread.
        let prefix = Document::from_utf8("load", Budget::new(1 << 20), Budget::new(1 << 20))
            .unwrap()
            .snapshot();
        let loading = EditorSurface::loading(prefix.clone(), workspace.notify.clone());
        workspace.push_tab(
            loading.into(),
            None,
            "loading.txt (loading)".into(),
            LifecycleEvent::LoadStarted,
        );
        workspace.pending_io[0].preview = Some(prefix);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(0, false, &mut renderer).unwrap();
        assert!(workspace.pending_io.is_empty());
        assert!(matches!(
            workspace.take_launch_open_outcomes().as_slice(),
            [LaunchOpenOutcome::Failed { request_id: 7, .. }]
        ));
        assert!(matches!(workspace.closed.as_slice(), [ClosedDocument::Reopen(reopen)] if reopen.path == path));
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// WSP-12: a spill whose document was closed does not pause automatic
    /// spilling, and a failed promotion is reported once instead of forever.
    #[test]
    fn spill_of_a_closed_document_keeps_spilling_enabled_and_promotion_resets() {
        let (directory, mut workspace) = failed_open_fixture("spill-closed");
        workspace.new_document().unwrap();
        let identity = workspace.editors[0].snapshot().identity_token();
        workspace.promotion_target = Some(identity);
        workspace.message = Some("staging refused".into());
        assert_eq!(
            workspace.promote_resident_for_source_edit(0, identity),
            Err("staging refused".into())
        );
        assert_eq!(workspace.promotion_target, None);
        let captured = workspace.editors[0].snapshot().clone();
        workspace.spill_pending = true;
        workspace.spill_document = Some(identity.0);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(0, false, &mut renderer).unwrap();
        // Any real ticket can carry the injected spill completion.
        workspace.open(directory.join("missing.txt"));
        let pending = workspace.pending_io.len() - 1;
        workspace.pending_io[pending].completion = Some(IoCompletion::ResidentSpilled {
            captured,
            result: Err(FileError::Cancelled),
        });
        workspace.pump();
        assert!(!workspace.spill_pending);
        assert!(!workspace.spill_paused, "closing the spilled document paused spilling");
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// WSP-16: internal lists stay bounded and retired editors do not wait for a draw.
    #[test]
    fn internal_lists_are_bounded_and_retired_editors_release_in_pump() {
        struct Issue {
            transaction: PathBuf,
            attempt: usize,
        }
        let mut issues = Vec::new();
        for index in 0..300 {
            upsert_save_issue(
                &mut issues,
                Issue {
                    transaction: PathBuf::from(format!("t{index}")),
                    attempt: 0,
                },
                |issue| &issue.transaction,
            );
        }
        upsert_save_issue(
            &mut issues,
            Issue {
                transaction: PathBuf::from("t299"),
                attempt: 1,
            },
            |issue| &issue.transaction,
        );
        assert_eq!(issues.len(), MAX_SAVE_ISSUES);
        assert_eq!(issues[0].transaction, PathBuf::from("t44"));
        assert_eq!(issues.last().map(|issue| issue.attempt), Some(1));
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        for request in 0..300 {
            workspace.record_launch_open(Some(request), Err("unclaimed".into()));
        }
        assert_eq!(workspace.open_outcomes.len(), MAX_OPEN_OUTCOMES);
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        workspace.editors[1].enqueue(Input::Insert("retired text".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.editors[1].busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut operations = Vec::new();
        workspace.draw(1, &mut renderer, 800.0, 600.0, &mut operations).unwrap();
        // Retire the drawn editor as an in-place replacement does.
        let retired = workspace.editors.remove(1);
        workspace.tabs.remove(1);
        workspace.last_drawn = None;
        workspace.retired.push(retired);
        workspace.pump();
        assert!(workspace.retired.is_empty(), "retired editors waited for a draw");
        assert!(!workspace.retired_layouts.is_empty());
        workspace.draw(0, &mut renderer, 800.0, 600.0, &mut operations).unwrap();
        assert!(workspace.retired_layouts.is_empty());
    }
}

pub mod extensions;
