// SPDX-License-Identifier: MPL-2.0
//! Native consumer for background recovery discovery and exact-directory restore.
use super::*;
use bareline_platform::LocalFileSystem;
use bareline_renderer::DrawOp;
use bareline_ui::{rect, text};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
struct PendingCompare {
    directory: PathBuf,
    existing: Vec<super::lifecycle::Identity>,
    original: PathBuf,
    recovered: Option<super::lifecycle::Identity>,
}
struct PendingRestore {
    directory: PathBuf,
    token: DiscoveryToken,
    request_id: u64,
}
type RecoveryDiscovery = Result<Vec<(PathBuf, bareline_file_io::recovery::RecoveryInspection)>, String>;
fn inspect_recovery_root(
    root: &std::path::Path,
    excluded: &std::collections::HashSet<PathBuf>,
    cancel: &bareline_file_io::cancellation::Cancellation,
    platform: &dyn LocalFileSystem,
) -> RecoveryDiscovery {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|error| error.to_string())? {
        cancel.check().map_err(|error| format!("{error:?}"))?;
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.file_name().to_string_lossy().starts_with("paged-") {
            continue;
        }
        let directory = entry.path();
        if excluded.contains(&directory) {
            continue;
        }
        let Ok(_guard) = platform.guard_directory(&directory) else {
            continue;
        };
        if let Ok(inspection) = bareline_file_io::recovery::inspect(&directory, cancel)
            && inspection.status != bareline_file_io::recovery::RecoveryStatus::Discarded
        {
            entries.push((directory, inspection));
            if entries.len() == 256 {
                break;
            }
        }
    }
    entries.sort_by_key(|(_, inspection)| {
        std::cmp::Reverse(inspection.last_durable.map_or(0, |receipt| receipt.protected_unix_ms))
    });
    Ok(entries)
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DiscoveryToken {
    generation: u64,
    root: Option<PathBuf>,
}
#[derive(Clone)]
enum RecoveryContent {
    Discovering,
    Ready(Vec<RecoveryRow>),
    Failed(String),
}
impl Default for RecoveryContent {
    fn default() -> Self {
        Self::Discovering
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum RecoveryNoticeState {
    Idle,
    Preparing { directory: PathBuf },
    Failed { directory: Option<PathBuf>, error: String },
}
/// One document, collapsed from every checkpoint directory that recovers it.
/// The Recovery Center shows one row per document (UX-53) even when a document
/// left dozens of checkpoint directories behind after repeated crashes.
#[derive(Clone)]
pub(super) struct RecoveryRow {
    /// Newest checkpoint directory; drives restore, compare, export and preview.
    directory: PathBuf,
    /// Every checkpoint directory for this document, newest first.
    group: Vec<PathBuf>,
    name: String,
    original: Option<PathBuf>,
    status: bareline_file_io::recovery::RecoveryStatus,
    protected_unix_ms: u64,
    size: u64,
    count: usize,
    complete_baseline: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum RecoveryRowId {
    Original(PathBuf),
    Untitled(PathBuf),
}
impl RecoveryRow {
    fn identity(&self) -> RecoveryRowId {
        self.original.clone().map_or_else(
            || RecoveryRowId::Untitled(self.directory.clone()),
            RecoveryRowId::Original,
        )
    }
}
/// Collapse raw checkpoint directories into one row per document, keeping the
/// newest checkpoint as the representative. Directories sharing an original
/// path are the same document; path-less (untitled) recoveries stay separate.
fn group_documents(entries: &[(PathBuf, bareline_file_io::recovery::RecoveryInspection)]) -> Vec<RecoveryRow> {
    let mut rows: Vec<RecoveryRow> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for (directory, inspection) in entries {
        let key = inspection
            .metadata
            .original_path
            .as_ref()
            .map(|path| format!("path:{}", path.display()))
            .unwrap_or_else(|| format!("dir:{}", directory.display()));
        let ms = inspection.last_durable.map_or(0, |receipt| receipt.protected_unix_ms);
        let name = inspection
            .metadata
            .original_path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled document".into());
        if let Some(pos) = keys.iter().position(|existing| existing == &key) {
            let row = &mut rows[pos];
            row.group.push(directory.clone());
            row.count += 1;
            if ms > row.protected_unix_ms {
                row.directory = directory.clone();
                row.protected_unix_ms = ms;
                row.status = inspection.status;
                row.size = inspection.metadata.original_len;
                row.complete_baseline = inspection.complete_baseline;
                row.original = inspection.metadata.original_path.clone();
                row.name = name;
            }
        } else {
            keys.push(key);
            rows.push(RecoveryRow {
                directory: directory.clone(),
                group: vec![directory.clone()],
                name,
                original: inspection.metadata.original_path.clone(),
                status: inspection.status,
                protected_unix_ms: ms,
                size: inspection.metadata.original_len,
                count: 1,
                complete_baseline: inspection.complete_baseline,
            });
        }
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.protected_unix_ms));
    rows
}
/// A recovery checkpoint is stale once its newest protected time is older than
/// this. "Delete all older than 7 days" removes every stale checkpoint.
const STALE_MS: u64 = 7 * 24 * 60 * 60 * 1000;
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
/// Plain-language "time since" for a checkpoint stamp.
fn relative_time(protected_unix_ms: u64) -> String {
    if protected_unix_ms == 0 {
        return "no checkpoint yet".into();
    }
    let now = now_unix_ms();
    let secs = now.saturating_sub(protected_unix_ms) / 1000;
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        let m = secs / 60;
        format!("{m} minute{} ago", if m == 1 { "" } else { "s" })
    } else if secs < 86_400 {
        let h = secs / 3600;
        format!("{h} hour{} ago", if h == 1 { "" } else { "s" })
    } else {
        let d = secs / 86_400;
        format!("{d} day{} ago", if d == 1 { "" } else { "s" })
    }
}
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["bytes", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
fn should_auto_open(documents: usize, allow: bool, open: bool, another_modal: bool) -> bool {
    documents > 0 && allow && !open && !another_modal
}
#[derive(Default)]
pub(super) struct RecoveryRuntime {
    root: Option<PathBuf>,
    mutation_allowed: bool,
    started: bool,
    pending: Option<Receiver<(DiscoveryToken, RecoveryDiscovery)>>,
    entries: Vec<(PathBuf, bareline_file_io::recovery::RecoveryInspection)>,
    content: RecoveryContent,
    discovery_generation: u64,
    completion_notice_generation: Option<u64>,
    claimed_directories: std::collections::BTreeMap<PathBuf, u64>,
    selection_identity: Option<RecoveryRowId>,
    cancellation: bareline_file_io::cancellation::Cancellation,
    discovery_cancellation: bareline_file_io::cancellation::Cancellation,
    notice_states: std::collections::BTreeMap<toast::DocumentKey, RecoveryNoticeState>,
    active_notice_document: Option<toast::DocumentKey>,
    notice_revision: u64,
    open: bool,
    selected: usize,
    focus: Option<usize>,
    pending_compare: Option<PendingCompare>,
    pending_restore: Option<PendingRestore>,
    confirm_discard: Option<Vec<PathBuf>>,
    hits: Vec<(bareline_renderer::Rect, String)>,
    operation: Option<Receiver<Result<Vec<PathBuf>, String>>>,
    preview_path: Option<PathBuf>,
    preview: Option<Receiver<Result<String, String>>>,
    preview_text: String,
    preview_cancellation: bareline_file_io::cancellation::Cancellation,
    allow_auto_open: bool,
}
impl RecoveryRuntime {
    pub(super) fn forget_document(&mut self, document: toast::DocumentKey) {
        self.notice_states.remove(&document);
        if self.active_notice_document == Some(document) {
            self.active_notice_document = None;
        }
    }
    pub(super) fn has_input_focus(&self) -> bool {
        self.open || self.confirm_discard.is_some()
    }
    fn action_enabled(&self, command: &str) -> bool {
        let requires_ready_checkpoint = matches!(
            command,
            "recovery.restore_selected"
                | "recovery.compare"
                | "recovery.export"
                | "recovery.discard"
                | "recovery.confirm_discard"
                | "recovery.delete_old"
        );
        let checkpoint_busy =
            self.pending_restore.is_some() || self.pending_compare.is_some() || self.operation.is_some();
        if requires_ready_checkpoint && (checkpoint_busy || !matches!(&self.content, RecoveryContent::Ready(_))) {
            return false;
        }
        let selected = self.rows().get(self.selected);
        match command {
            "recovery.restore_selected" => {
                self.pending_restore.is_none() && selected.is_some_and(|row| row.complete_baseline)
            }
            "recovery.compare" => selected.is_some_and(|row| row.complete_baseline && row.original.is_some()),
            "recovery.export" => selected.is_some() && self.operation.is_none(),
            "recovery.discard" => self.mutation_allowed && selected.is_some() && self.operation.is_none(),
            "recovery.confirm_discard" => {
                self.mutation_allowed && self.confirm_discard.is_some() && self.operation.is_none()
            }
            "recovery.delete_old" => {
                self.mutation_allowed && self.operation.is_none() && self.stale_directories().next().is_some()
            }
            "recovery.discovery_retry" => matches!(&self.content, RecoveryContent::Failed(_)),
            _ => true,
        }
    }
    fn default_command(&self) -> &'static str {
        if self.confirm_discard.is_some() {
            return if self.action_enabled("recovery.confirm_discard") {
                "recovery.confirm_discard"
            } else {
                "recovery.keep"
            };
        }
        match &self.content {
            RecoveryContent::Failed(_) if self.action_enabled("recovery.discovery_retry") => "recovery.discovery_retry",
            RecoveryContent::Ready(rows) if !rows.is_empty() && self.action_enabled("recovery.restore_selected") => {
                "recovery.restore_selected"
            }
            RecoveryContent::Discovering | RecoveryContent::Ready(_) => "recovery.keep",
            RecoveryContent::Failed(_) => "recovery.keep",
        }
    }
    fn focused_command(&self) -> String {
        self.focus
            .and_then(|index| self.hits.get(index))
            .map_or_else(|| self.default_command().into(), |(_, command)| command.clone())
    }
    fn stale_directories(&self) -> impl Iterator<Item = PathBuf> + '_ {
        let cutoff = now_unix_ms().saturating_sub(STALE_MS);
        self.entries.iter().filter_map(move |(directory, inspection)| {
            let ms = inspection.last_durable.map_or(0, |receipt| receipt.protected_unix_ms);
            (ms != 0 && ms < cutoff).then(|| directory.clone())
        })
    }
    fn rows(&self) -> &[RecoveryRow] {
        match &self.content {
            RecoveryContent::Ready(rows) => rows,
            RecoveryContent::Discovering | RecoveryContent::Failed(_) => &[],
        }
    }
    fn remember_selection(&mut self) {
        let identity = self.rows().get(self.selected).map(RecoveryRow::identity);
        self.selection_identity = identity;
    }
    fn rebuild_rows(&mut self) {
        let previous_index = self.selected;
        let previous_path = self.rows().get(previous_index).map(|row| row.directory.clone());
        let identity = self
            .selection_identity
            .clone()
            .or_else(|| self.rows().get(previous_index).map(RecoveryRow::identity));
        let rows = group_documents(&self.entries);
        self.selected = identity
            .as_ref()
            .and_then(|identity| rows.iter().position(|row| &row.identity() == identity))
            .unwrap_or_else(|| previous_index.min(rows.len().saturating_sub(1)));
        self.selection_identity = rows.get(self.selected).map(RecoveryRow::identity);
        let selected_path = rows.get(self.selected).map(|row| row.directory.clone());
        if previous_path != selected_path {
            self.preview_cancellation.cancel();
            self.preview = None;
            self.preview_path = None;
            self.preview_text.clear();
        }
        self.content = RecoveryContent::Ready(rows);
        self.focus = None;
    }
    fn request_discovery(&mut self) {
        self.discovery_cancellation.cancel();
        self.discovery_cancellation = Default::default();
        self.pending = None;
        self.started = false;
        self.discovery_generation = self.discovery_generation.wrapping_add(1).max(1);
        self.content = RecoveryContent::Discovering;
    }
    fn release_closed_claims(&mut self, open_documents: &std::collections::BTreeSet<u64>) -> bool {
        let before = self.claimed_directories.len();
        self.claimed_directories
            .retain(|_, document| open_documents.contains(document));
        self.claimed_directories.len() != before
    }
    fn token(&self) -> DiscoveryToken {
        DiscoveryToken {
            generation: self.discovery_generation,
            root: self.root.clone(),
        }
    }
    fn accept_discovery(&mut self, token: &DiscoveryToken, result: RecoveryDiscovery) -> bool {
        if token != &self.token() {
            return false;
        }
        match result {
            Ok(entries) => {
                self.entries = entries;
                self.rebuild_rows();
            }
            Err(error) => {
                self.entries.clear();
                self.preview_cancellation.cancel();
                self.preview = None;
                self.preview_path = None;
                self.preview_text.clear();
                self.content = RecoveryContent::Failed(error);
                self.focus = None;
            }
        }
        true
    }
    fn refresh_after_operation(&mut self, removed: &[PathBuf]) {
        if !removed.is_empty() {
            self.entries.retain(|(entry, _)| !removed.contains(entry));
            self.rebuild_rows();
        }
        self.request_discovery();
    }
    pub(super) fn configure(&mut self, root: Option<PathBuf>, mutation_allowed: bool) {
        self.cancellation.cancel();
        self.discovery_cancellation.cancel();
        self.preview_cancellation.cancel();
        self.root = root;
        self.mutation_allowed = mutation_allowed;
        self.cancellation = Default::default();
        self.request_discovery();
        self.entries.clear();
        self.selection_identity = None;
        self.preview = None;
        self.preview_path = None;
        self.preview_text.clear();
        self.allow_auto_open = true;
    }
    pub(super) fn dismiss(&mut self) {
        self.open = false;
        self.preview_cancellation.cancel();
        self.preview = None;
        self.preview_path = None;
        self.preview_text.clear();
        self.confirm_discard = None;
        self.focus = None;
        self.allow_auto_open = false;
    }
}
impl Drop for RecoveryRuntime {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.discovery_cancellation.cancel();
        self.preview_cancellation.cancel();
    }
}
impl Shell {
    fn open_recovery_center(&mut self) {
        self.activate_modal(modal::ModalSurface::Recovery);
        self.recovery.open = true;
        self.recovery.allow_auto_open = true;
        self.toasts.resolve(&toast::NotificationId::from("recovery-discovery"));
    }

    fn publish_closed_discovery(&mut self, token: &DiscoveryToken, documents: usize, failure: Option<String>) {
        if self.recovery.completion_notice_generation == Some(token.generation) {
            return;
        }
        self.recovery.completion_notice_generation = Some(token.generation);
        self.toasts.resolve(&toast::NotificationId::from("recovery-discovery"));
        if self.recovery.open {
            return;
        }
        if documents > 0 {
            self.toasts.push_typed(
                "recovery-discovery",
                token.generation,
                bareline_ui::theme::ToastLevel::Info,
                toast::NotificationKind::Outcome,
                format!("{documents} document(s) with recovery checkpoints available."),
                Some("Open Recovery Center to inspect the current checkpoint list.".into()),
                None,
                toast::NotificationLifetime::Transient,
                Instant::now(),
            );
        } else if let Some(error) = failure {
            self.toasts.push_typed(
                "recovery-discovery",
                token.generation,
                bareline_ui::theme::ToastLevel::Error,
                toast::NotificationKind::Outcome,
                "Recovery discovery unavailable.",
                Some(error),
                None,
                toast::NotificationLifetime::Persistent,
                Instant::now(),
            );
        }
    }

    fn recovery_auto_open_blocked(&self) -> bool {
        self.modal.is_some()
            || self.palette.open
            || self.search_modal()
            || self.settings.controller.open
            || self.shortcuts.open
            || self.macros.controller.manager.open
            || self.extensions.open
            || self.power.open
            || self.utilities.has_input_focus()
            || self.panels_accessibility_focus().is_some()
            || self.views_accessibility_focus().is_some()
            || self
                .workspace
                .as_ref()
                .is_some_and(|workspace| workspace.find.has_focus() || workspace.search_focus)
    }

    pub(super) fn recovery_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        if !self.recovery.action_enabled(id) {
            if let Some(workspace) = &mut self.workspace {
                workspace.message=Some("This recovery action needs a selected available checkpoint; incomplete snapshots can export saved edits with a gap report.".into());
            }
            return true;
        }
        match id {
            "recovery.open" => {
                if !self.ensure_workspace(el) {
                    return true;
                }
                self.open_recovery_center();
            }
            "recovery.keep" => {
                self.dismiss_modal(modal::ModalSurface::Recovery);
            }
            "recovery.restore_selected" => {
                if let Some(directory) = self
                    .recovery
                    .rows()
                    .get(self.recovery.selected)
                    .map(|row| row.directory.clone())
                    && self.ensure_workspace(el)
                {
                    let token = self.recovery.token();
                    match self
                        .workspace
                        .as_mut()
                        .unwrap()
                        .restore_paged_recovery_tracked(directory.clone())
                    {
                        Ok(request_id) => {
                            self.recovery.pending_restore = Some(PendingRestore {
                                directory,
                                token,
                                request_id,
                            });
                            (self.notify)();
                            self.dismiss_modal(modal::ModalSurface::Recovery);
                        }
                        Err(error) => self.publish_restore_failure(&directory, error),
                    }
                }
            }
            "recovery.compare" => {
                if self.recovery.pending_compare.is_some() {
                    return true;
                }
                if let Some(row) = self.recovery.rows().get(self.recovery.selected).cloned()
                    && self.ensure_workspace(el)
                {
                    let directory = row.directory.clone();
                    let workspace = self.workspace.as_mut().unwrap();
                    if let Some(original) = row.original {
                        self.recovery.pending_compare = Some(PendingCompare {
                            directory: directory.clone(),
                            existing: workspace
                                .editors
                                .iter()
                                .map(super::lifecycle::Identity::capture)
                                .collect(),
                            original,
                            recovered: None,
                        });
                        workspace.restore_paged_recovery(directory);
                        self.dismiss_modal(modal::ModalSurface::Recovery);
                    } else {
                        workspace.message = Some(
                            "This recovery has no original disk path. Open the recovered copy or export saved edits."
                                .into(),
                        );
                    }
                }
            }
            "recovery.discard" => {
                self.recovery.confirm_discard = self
                    .recovery
                    .rows()
                    .get(self.recovery.selected)
                    .map(|row| row.group.clone());
            }
            "recovery.delete_old" => {
                let stale: Vec<PathBuf> = self.recovery.stale_directories().collect();
                if stale.is_empty() {
                    if let Some(workspace) = &mut self.workspace {
                        workspace.message = Some("No recovery checkpoints are older than 7 days.".into());
                    }
                } else {
                    self.recovery.confirm_discard = Some(stale);
                }
            }
            "recovery.confirm_discard" | "recovery.export" => {
                if self.recovery.operation.is_some() {
                    return true;
                }
                let discard = id == "recovery.confirm_discard";
                let directories: Vec<PathBuf> = if discard {
                    self.recovery.confirm_discard.take().unwrap_or_default()
                } else {
                    self.recovery
                        .rows()
                        .get(self.recovery.selected)
                        .map(|row| vec![row.directory.clone()])
                        .unwrap_or_default()
                };
                let destination = if discard {
                    None
                } else {
                    self.platform.as_ref().and_then(|p| p.pick_folder().ok().flatten())
                };
                if !directories.is_empty() && (discard || destination.is_some()) {
                    let (tx, rx) = mpsc::sync_channel(1);
                    let notify = self.notify.clone();
                    let cancel = self.recovery.cancellation.clone();
                    if std::thread::Builder::new()
                        .name("recovery-action".into())
                        .spawn(move || {
                            let result = (|| -> Result<Vec<PathBuf>, String> {
                                let platform = bareline_platform_windows::WindowsFileSystem;
                                if discard {
                                    let mut removed = Vec::new();
                                    for directory in &directories {
                                        let _guard = platform.guard_directory(directory).map_err(|e| e.to_string())?;
                                        bareline_file_io::recovery::discard(directory, &platform)
                                            .map_err(|e| e.to_string())?;
                                        removed.push(directory.clone());
                                    }
                                    Ok(removed)
                                } else {
                                    let directory = &directories[0];
                                    let _guard = platform.guard_directory(directory).map_err(|e| e.to_string())?;
                                    let parent = destination.unwrap();
                                    let _destination_guard =
                                        platform.guard_directory(&parent).map_err(|e| e.to_string())?;
                                    let export = parent.join(format!(
                                        "recovery-edits-{}",
                                        std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_nanos()
                                    ));
                                    bareline_file_io::recovery::export_edits(directory, &export, &cancel)
                                        .map_err(|e| e.to_string())?;
                                    Ok(Vec::new())
                                }
                            })();
                            let _ = tx.send(result);
                            notify();
                        })
                        .is_ok()
                    {
                        self.recovery.operation = Some(rx);
                    }
                }
            }
            "recovery.open_folder" => {
                let selected = self
                    .platform
                    .as_ref()
                    .and_then(|platform| platform.pick_folder().ok().flatten());
                if let Some(directory) = selected
                    && self.ensure_workspace(el)
                {
                    self.workspace.as_mut().unwrap().restore_paged_recovery(directory);
                }
            }
            "recovery.restore_latest" => {
                if let Some((directory, _)) = self
                    .recovery
                    .entries
                    .iter()
                    .find(|(_, inspection)| inspection.complete_baseline)
                    .cloned()
                    && self.ensure_workspace(el)
                {
                    self.workspace.as_mut().unwrap().restore_paged_recovery(directory);
                } else if let Some(workspace) = &mut self.workspace {
                    workspace.message =
                        Some("No complete recovery checkpoint was found. Open a recovery folder to inspect it.".into());
                }
            }
            "recovery.retry" => {
                let cleanup_retries = bareline_file_io::recovery_retirement::retry_pending_cleanup();
                let mut retry_failure = None;
                if let Some(workspace) = &mut self.workspace {
                    retry_failure = workspace.editors.get_mut(self.app.active).and_then(|editor| {
                        let document = editor.snapshot().identity_token();
                        editor.retry_recovery().err().map(|error| (document, error))
                    });
                    workspace.message = if retry_failure.is_none() {
                        (cleanup_retries > 0)
                            .then(|| format!("Retrying {cleanup_retries} pending recovery cleanup(s)…"))
                    } else {
                        None
                    };
                }
                if let Some((document, error)) = retry_failure {
                    self.toasts.enqueue_owned(
                        toast::Notification::new(
                            format!("recovery-preparation-{}", document.0),
                            toast::next_revision(),
                            bareline_ui::theme::ToastLevel::Error,
                            toast::NotificationKind::Outcome,
                            "Recovery retry could not start.",
                            Some(error),
                            Some(document),
                            toast::NotificationLifetime::Persistent,
                        ),
                        Instant::now(),
                    );
                }
            }
            "recovery.discovery_retry" => {
                self.toasts.resolve(&toast::NotificationId::from("recovery-discovery"));
                self.recovery.request_discovery();
                (self.notify)();
            }
            "recovery.save_as" => self.dispatch(el, Action::SaveAs),
            _ => {
                if let Some(index) = id
                    .strip_prefix("recovery.select.")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    let last = self.recovery.rows().len().saturating_sub(1);
                    self.recovery.selected = index.min(last);
                    self.recovery.remember_selection();
                    self.recovery.confirm_discard = None;
                } else {
                    return false;
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    fn publish_recovery_notice(&mut self, document: toast::DocumentKey, state: RecoveryNoticeState) {
        let id = toast::NotificationId(format!("recovery-preparation-{}", document.0));
        self.recovery.notice_revision = self.recovery.notice_revision.wrapping_add(1);
        let notification = match state {
            RecoveryNoticeState::Idle => {
                self.toasts.resolve(&id);
                None
            }
            RecoveryNoticeState::Preparing { directory } => Some(toast::Notification::new(
                id,
                self.recovery.notice_revision,
                bareline_ui::theme::ToastLevel::Info,
                toast::NotificationKind::Progress,
                "Recovery snapshot preparing; editing remains available.",
                Some(format!("Recovery directory: {}", directory.display())),
                Some(document),
                toast::NotificationLifetime::Scoped,
            )),
            RecoveryNoticeState::Failed { directory, error } => Some(toast::Notification::new(
                id,
                self.recovery.notice_revision,
                bareline_ui::theme::ToastLevel::Error,
                toast::NotificationKind::Outcome,
                "Recovery unavailable. Retry Recovery or Save As.",
                Some(directory.map_or(error.clone(), |path| {
                    format!("{error}\nRecovery directory: {}", path.display())
                })),
                Some(document),
                toast::NotificationLifetime::Persistent,
            )),
        };
        if let Some(notification) = notification {
            self.toasts.enqueue_owned(notification, Instant::now());
        }
        if self
            .modal
            .is_some_and(|modal| modal.surface == modal::ModalSurface::NotificationDetails)
            && !self.toasts.details_open()
        {
            self.dismiss_modal(modal::ModalSurface::NotificationDetails);
        }
    }

    fn publish_restore_failure(&mut self, directory: &std::path::Path, error: String) {
        self.recovery.notice_revision = self.recovery.notice_revision.wrapping_add(1);
        self.toasts.push_typed(
            "recovery-restore",
            self.recovery.notice_revision,
            bareline_ui::theme::ToastLevel::Error,
            toast::NotificationKind::Outcome,
            "Recovered copy could not be opened; checkpoint retained.",
            Some(format!("{error}\nRecovery directory: {}", directory.display())),
            None,
            toast::NotificationLifetime::Persistent,
            Instant::now(),
        );
    }

    fn finish_pending_restore(
        &mut self,
        pending: PendingRestore,
        outcome: bareline_app::workspace::RecoveryRestoreOutcome,
    ) {
        if pending.token != self.recovery.token() {
            return;
        }
        match outcome {
            bareline_app::workspace::RecoveryRestoreOutcome::Restored { request_id, document }
                if request_id == pending.request_id =>
            {
                self.recovery
                    .claimed_directories
                    .insert(pending.directory.clone(), document.0);
                self.recovery.refresh_after_operation(&[pending.directory]);
                (self.notify)();
            }
            bareline_app::workspace::RecoveryRestoreOutcome::Failed { request_id, error }
                if request_id == pending.request_id =>
            {
                self.publish_restore_failure(&pending.directory, error);
            }
            _ => {}
        }
    }

    pub(super) fn recovery_pump(&mut self, _el: &ActiveEventLoop) {
        if let Some(request_id) = self.recovery.pending_restore.as_ref().map(|pending| pending.request_id)
            && let Some(outcome) = self
                .workspace
                .as_mut()
                .and_then(|workspace| workspace.take_recovery_restore_outcome(request_id))
        {
            let pending = self.recovery.pending_restore.take().unwrap();
            self.finish_pending_restore(pending, outcome);
        }
        let open_document_ids: std::collections::BTreeSet<u64> = self
            .workspace
            .as_ref()
            .into_iter()
            .flat_map(|workspace| workspace.editors.iter().map(|editor| editor.document_identity().0))
            .collect();
        if self.recovery.release_closed_claims(&open_document_ids) {
            self.recovery.request_discovery();
        }
        if self.recovery.pending_compare.is_some() && self.workspace.as_ref().is_some_and(|w| !w.io_busy()) {
            let mut pending = self.recovery.pending_compare.take().unwrap();
            let workspace = self.workspace.as_mut().unwrap();
            let new = workspace
                .editors
                .iter()
                .enumerate()
                .find(|(index, editor)| !pending.existing.iter().any(|id| id.matches(editor)) && if pending.recovered.is_none() {
                    matches!(editor, bareline_app::workspace::WorkspaceEditor::Paged(paged) if paged.recovery_origin_path().as_deref()==Some(pending.directory.as_path()))
                } else { workspace.path(*index)==Some(pending.original.as_path()) && !editor.dirty() })
                .map(|(index, _)| index);
            if let Some(index) = new {
                workspace.editors[index].set_read_only(true);
                if let Some(recovered) = pending.recovered {
                    let left = workspace.editors.iter().position(|editor| recovered.matches(editor));
                    if workspace.path(index) == Some(pending.original.as_path()) {
                        if let Some(left) = left {
                            self.compare_recovery_pair(left, index);
                        }
                    } else {
                        self.toasts.enqueue_owned(
                            toast::Notification::new(
                                format!("recovery-compare:{}", pending.directory.display()),
                                toast::next_revision(),
                                bareline_ui::theme::ToastLevel::Error,
                                toast::NotificationKind::Outcome,
                                "Current disk compare source did not open.",
                                Some(format!("Recovered copy retained at {}", pending.directory.display())),
                                None,
                                toast::NotificationLifetime::Persistent,
                            ),
                            Instant::now(),
                        );
                    }
                } else {
                    pending.recovered = Some(super::lifecycle::Identity::capture(&workspace.editors[index]));
                    pending.existing = workspace
                        .editors
                        .iter()
                        .map(super::lifecycle::Identity::capture)
                        .collect();
                    workspace.open(pending.original.clone());
                    self.recovery.pending_compare = Some(pending);
                }
                self.app.tabs = self.workspace.as_ref().unwrap().titles();
            } else {
                self.toasts.enqueue_owned(
                    toast::Notification::new(
                        format!("recovery-compare:{}", pending.directory.display()),
                        toast::next_revision(),
                        bareline_ui::theme::ToastLevel::Error,
                        toast::NotificationKind::Outcome,
                        "Recovery comparison could not open its source.",
                        Some(format!("Recovered copy retained at {}", pending.directory.display())),
                        None,
                        toast::NotificationLifetime::Persistent,
                    ),
                    Instant::now(),
                );
            }
        }
        let selected_preview = self
            .recovery
            .rows()
            .get(self.recovery.selected)
            .map(|row| row.directory.clone());
        if !self.recovery.open || selected_preview != self.recovery.preview_path {
            self.recovery.preview_cancellation.cancel();
            self.recovery.preview = None;
            self.recovery.preview_path = None;
        }
        if self.recovery.open && self.recovery.preview.is_none() {
            let path = self
                .recovery
                .rows()
                .get(self.recovery.selected)
                .map(|row| row.directory.clone());
            if path != self.recovery.preview_path {
                self.recovery.preview_path = path.clone();
                if let Some(path) = path {
                    self.recovery.preview_text = "Preparing bounded preview…".into();
                    let (tx, rx) = mpsc::sync_channel(1);
                    let notify = self.notify.clone();
                    self.recovery.preview_cancellation = Default::default();
                    let cancel = self.recovery.preview_cancellation.clone();
                    if std::thread::Builder::new()
                        .name("recovery-preview".into())
                        .spawn(move || {
                            let result = (|| {
                                let bytes = bareline_document::Budget::new(256 << 20);
                                let mut opened = bareline_file_io::paged_recovery::restore(
                                    &path,
                                    Arc::new(bareline_platform_windows::WindowsFileSystem),
                                    bytes.clone(),
                                    bareline_document::Budget::new(0),
                                    &cancel,
                                )?;
                                bareline_file_io::paged_recovery::preview(&mut opened, &bytes, &cancel)
                            })();
                            let _ = tx.send(result);
                            notify();
                        })
                        .is_ok()
                    {
                        self.recovery.preview = Some(rx);
                    }
                }
            }
        }
        if let Some(receiver) = &self.recovery.preview {
            match receiver.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.recovery.preview = None;
                    self.recovery.preview_text = match result {
                        Ok(Ok(text)) => text,
                        Ok(Err(error)) => format!("Preview unavailable: {error}"),
                        Err(_) => "Preview worker stopped.".into(),
                    };
                }
            }
        }
        if let Some(receiver) = &self.recovery.operation {
            match receiver.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.recovery.operation = None;
                    let (level, lifetime, message, details) = match result {
                        Ok(Ok(removed)) if !removed.is_empty() => {
                            self.recovery.refresh_after_operation(&removed);
                            if removed.len() == 1 {
                                (
                                    bareline_ui::theme::ToastLevel::Info,
                                    toast::NotificationLifetime::Transient,
                                    "Recovery discarded.".into(),
                                    None,
                                )
                            } else {
                                (
                                    bareline_ui::theme::ToastLevel::Info,
                                    toast::NotificationLifetime::Transient,
                                    format!("{} recovery checkpoints discarded.", removed.len()),
                                    None,
                                )
                            }
                        }
                        Ok(Ok(_)) => {
                            self.recovery.refresh_after_operation(&[]);
                            (
                                bareline_ui::theme::ToastLevel::Info,
                                toast::NotificationLifetime::Transient,
                                "Saved edits exported with gaps.json in the selected folder.".into(),
                                None,
                            )
                        }
                        Ok(Err(error)) => (
                            bareline_ui::theme::ToastLevel::Error,
                            toast::NotificationLifetime::Persistent,
                            "Recovery action failed.".into(),
                            Some(error),
                        ),
                        Err(_) => (
                            bareline_ui::theme::ToastLevel::Error,
                            toast::NotificationLifetime::Persistent,
                            "Recovery worker stopped; checkpoint retained.".into(),
                            None,
                        ),
                    };
                    self.recovery.notice_revision = self.recovery.notice_revision.wrapping_add(1);
                    self.toasts.push_typed(
                        "recovery-action",
                        self.recovery.notice_revision,
                        level,
                        toast::NotificationKind::Outcome,
                        message,
                        details,
                        None,
                        lifetime,
                        Instant::now(),
                    );
                    let last = self.recovery.rows().len().saturating_sub(1);
                    self.recovery.selected = self.recovery.selected.min(last);
                }
            }
        }
        if !self.recovery.started {
            self.recovery.started = true;
            let token = self.recovery.token();
            if let Some(root) = token.root.clone() {
                let (tx, rx) = mpsc::sync_channel(1);
                let cancel = self.recovery.discovery_cancellation.clone();
                let notify = self.notify.clone();
                let worker_token = token.clone();
                let mut referenced: std::collections::HashSet<PathBuf> = self
                    .workspace
                    .as_ref()
                    .map(|workspace| {
                        workspace
                            .editors
                            .iter()
                            .filter_map(|editor| editor.recovery_status().directory)
                            .collect()
                    })
                    .unwrap_or_default();
                let excluded: std::collections::HashSet<PathBuf> =
                    self.recovery.claimed_directories.keys().cloned().collect();
                referenced.extend(excluded.iter().cloned());
                let launched = std::thread::Builder::new()
                    .name("recovery-discovery".into())
                    .spawn(move || {
                        let result = (|| -> Result<Vec<_>, String> {
                            let platform = bareline_platform_windows::WindowsFileSystem;
                            let _guard = match platform.guard_directory(&root) {
                                Ok(guard) => guard,
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                    return Ok(Vec::new());
                                }
                                Err(error) => return Err(error.to_string()),
                            };
                            // Reclaim journals left behind by processes that are gone.
                            // A bounded number of the newest ones survive so a user can
                            // still recover by hand after an unusual crash.
                            let _ = bareline_file_io::paged_recovery::sweep(
                                &root,
                                &referenced,
                                &process_alive,
                                20,
                                &platform,
                            );
                            inspect_recovery_root(&root, &excluded, &cancel, &platform)
                        })();
                        let _ = tx.send((worker_token, result));
                        notify();
                    });
                match launched {
                    Ok(_) => self.recovery.pending = Some(rx),
                    Err(error) => {
                        let error = error.to_string();
                        self.recovery.content = RecoveryContent::Failed(error.clone());
                        self.publish_closed_discovery(&token, 0, Some(error));
                    }
                }
            } else {
                let _ = self.recovery.accept_discovery(&token, Ok(Vec::new()));
            }
        }
        if let Some(receiver) = &self.recovery.pending {
            match receiver.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.recovery.pending = None;
                    match result {
                        Ok((token, discovery)) => {
                            if self.recovery.accept_discovery(&token, discovery) {
                                let (documents, failure) = match &self.recovery.content {
                                    RecoveryContent::Ready(rows) => (rows.len(), None),
                                    RecoveryContent::Failed(error) => (0, Some(error.clone())),
                                    RecoveryContent::Discovering => (0, None),
                                };
                                let open_center = should_auto_open(
                                    documents,
                                    self.recovery.allow_auto_open,
                                    self.recovery.open,
                                    self.recovery_auto_open_blocked(),
                                );
                                if open_center {
                                    self.ensure_workspace(_el);
                                    self.open_recovery_center();
                                }
                                self.publish_closed_discovery(&token, documents, failure);
                                if self.recovery.open && documents > 0 {
                                    // Discovery completion occurs after the preview
                                    // phase in this pump; schedule one deterministic
                                    // pass to attach the selected row's preview.
                                    (self.notify)();
                                }
                            }
                        }
                        Err(_) => {
                            let token = self.recovery.token();
                            let error = "Recovery discovery worker stopped before reporting a result.".to_string();
                            let _ = self.recovery.accept_discovery(&token, Err(error.clone()));
                            self.publish_closed_discovery(&token, 0, Some(error));
                        }
                    }
                }
            }
        }
        let open_documents: std::collections::BTreeSet<_> = self
            .workspace
            .as_ref()
            .into_iter()
            .flat_map(|workspace| workspace.editors.iter().map(|editor| editor.document_identity()))
            .collect();
        self.recovery
            .notice_states
            .retain(|document, _| open_documents.contains(document));
        let mut recovery_notice = None;
        if let Some(workspace) = &mut self.workspace
            && let Some(editor) = workspace.editors.get(self.app.active)
        {
            let document = editor.snapshot().identity_token();
            let status = editor.recovery_status();
            let state = if let Some(error) = status.error {
                RecoveryNoticeState::Failed {
                    directory: status.directory,
                    error,
                }
            } else if let Some(directory) = status.directory.filter(|_| !status.complete) {
                RecoveryNoticeState::Preparing { directory }
            } else {
                RecoveryNoticeState::Idle
            };
            let changed = self.recovery.notice_states.get(&document) != Some(&state);
            if changed {
                self.recovery.notice_states.insert(document, state.clone());
                recovery_notice = Some((document, state));
            }
            self.recovery.active_notice_document = Some(document);
        }
        if let Some((document, state)) = recovery_notice {
            self.publish_recovery_notice(document, state);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    pub(super) fn recovery_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if !self.recovery.open || !center_consumes_event(event) {
            return false;
        }
        match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let pointer = self.editor_pointer();
                if let Some((index, (_, id))) = self
                    .recovery
                    .hits
                    .iter()
                    .enumerate()
                    .find(|(_, (r, _))| r.contains(pointer))
                {
                    let id = id.clone();
                    self.recovery.focus = Some(index);
                    self.recovery_dispatch(el, &id);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        self.dismiss_modal(modal::ModalSurface::Recovery);
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        self.recovery.focus = None;
                        let last = self.recovery.rows().len().saturating_sub(1);
                        self.recovery.selected = (self.recovery.selected + 1).min(last);
                        self.recovery.remember_selection();
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        self.recovery.focus = None;
                        self.recovery.selected = self.recovery.selected.saturating_sub(1);
                        self.recovery.remember_selection();
                    }
                    Key::Named(NamedKey::Tab) => {
                        let enabled: Vec<_> = self
                            .recovery
                            .hits
                            .iter()
                            .map(|(_, command)| self.recovery.action_enabled(command))
                            .collect();
                        self.recovery.focus =
                            next_recovery_focus(self.recovery.focus, &enabled, self.modifiers.shift_key());
                    }
                    key if activates_recovery_command(key) => {
                        let id = self.recovery.focused_command();
                        self.recovery_dispatch(el, &id);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

impl RecoveryRuntime {
    pub(super) fn draw(&mut self, theme: bareline_ui::theme::UiTheme, width: f32, height: f32, ops: &mut Vec<DrawOp>) {
        if !self.open {
            return;
        }
        self.hits.clear();
        ops.push(DrawOp::Fill(rect(0.0, 0.0, width, height), theme.editor));
        text(ops, 24.0, 20.0, "Recovery Center", 24.0, theme.text);
        text(
            ops,
            24.0,
            55.0,
            "Review a recovery copy before replacing any file. Arrow keys select; Enter opens a copy.",
            13.0,
            theme.text,
        );
        match &self.content {
            RecoveryContent::Discovering => text(
                ops,
                24.0,
                78.0,
                "Searching for recovery checkpoints… Actions are available when discovery completes.",
                13.0,
                theme.muted,
            ),
            RecoveryContent::Failed(error) => {
                text(ops, 24.0, 78.0, "Recovery checkpoint search failed.", 13.0, theme.text);
                let error: String = error.chars().take(180).collect();
                text(ops, 24.0, 100.0, &error, 12.0, theme.muted);
            }
            RecoveryContent::Ready(rows) if rows.is_empty() => {
                text(ops, 24.0, 100.0, "No recovery checkpoints found.", 14.0, theme.text);
            }
            RecoveryContent::Ready(rows) => text(
                ops,
                24.0,
                78.0,
                &format!("{} recoverable document(s) found.", rows.len()),
                12.0,
                theme.muted,
            ),
        }
        let rows = match &self.content {
            RecoveryContent::Ready(rows) => rows.as_slice(),
            RecoveryContent::Discovering | RecoveryContent::Failed(_) => &[],
        };
        let visible = ((height - 412.0).max(40.0) / 42.0) as usize;
        let start = self.selected.saturating_sub(visible.saturating_sub(1));
        for (row, (index, entry)) in rows.iter().enumerate().skip(start).take(visible).enumerate() {
            let bounds = rect(20.0, 122.0 + row as f32 * 42.0, width - 40.0, 38.0);
            if index == self.selected {
                ops.push(DrawOp::Stroke(bounds, theme.focus, 1.0));
            }
            let original = entry
                .original
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Not yet saved to disk".into());
            let extra = if entry.count > 1 {
                format!(" · {} checkpoints", entry.count)
            } else {
                String::new()
            };
            text(
                ops,
                30.0,
                bounds.y + 3.0,
                &format!("{} · {}", entry.name, recovery_state_label(entry.status)),
                14.0,
                theme.text,
            );
            text(
                ops,
                30.0,
                bounds.y + 21.0,
                &format!(
                    "{original} · {} · {}{extra}",
                    relative_time(entry.protected_unix_ms),
                    format_size(entry.size)
                ),
                11.0,
                theme.muted,
            );
            self.hits.push((bounds, format!("recovery.select.{index}")));
        }
        if !rows.is_empty() {
            let preview_y = (height - 270.0).max(145.0);
            text(ops, 24.0, preview_y, "Recovered copy — first 8 KiB", 14.0, theme.text);
            for (index, line) in self.preview_text.lines().take(6).enumerate() {
                let line: String = line.chars().take(120).collect();
                text(
                    ops,
                    24.0,
                    preview_y + 24.0 + index as f32 * 18.0,
                    &line,
                    12.0,
                    theme.text,
                );
            }
        }
        let y = (height - 70.0).max(140.0);
        if let Some(paths) = &self.confirm_discard {
            let message = if paths.len() == 1 {
                "Permanently discard this recovery? This cannot be undone.".to_string()
            } else {
                format!(
                    "Permanently discard {} recovery checkpoints? This cannot be undone.",
                    paths.len()
                )
            };
            text(ops, 24.0, y - 30.0, &message, 13.0, theme.text);
        } else if let Some(entry) = rows.get(self.selected) {
            let warning = match entry.status {
                bareline_file_io::recovery::RecoveryStatus::Complete => {
                    "Open recovered copy preserves the original file. Save As chooses a new destination."
                }
                _ => "Only validated data is available. Export saved edits includes an explicit gap report.",
            };
            text(ops, 24.0, y - 30.0, warning, 13.0, theme.text);
        }
        let actions = if matches!(&self.content, RecoveryContent::Failed(_)) {
            vec![("Retry", "recovery.discovery_retry"), ("Close", "recovery.keep")]
        } else if self.confirm_discard.is_some() {
            vec![
                ("Confirm irreversible discard", "recovery.confirm_discard"),
                ("Keep recovery", "recovery.keep"),
            ]
        } else {
            vec![
                ("Restore", "recovery.restore_selected"),
                ("Compare", "recovery.compare"),
                ("Export saved edits", "recovery.export"),
                ("Delete", "recovery.discard"),
                ("Delete all older than 7 days", "recovery.delete_old"),
                ("Close", "recovery.keep"),
            ]
        };
        for (index, (label, id)) in actions.iter().enumerate() {
            let button_width = ((width - 48.0) / actions.len() as f32).max(80.0);
            let bounds = rect(24.0 + index as f32 * button_width, y, button_width - 6.0, 34.0);
            let enabled = self.action_enabled(id);
            ops.push(DrawOp::Stroke(
                bounds,
                if enabled { theme.focus } else { theme.muted },
                1.0,
            ));
            text(
                ops,
                bounds.x + 8.0,
                bounds.y + 8.0,
                *label,
                12.0,
                if enabled { theme.text } else { theme.muted },
            );
            self.hits.push((bounds, (*id).into()));
        }
        if let Some((bounds, _)) = self.focus.and_then(|index| self.hits.get(index)) {
            ops.push(DrawOp::Stroke(*bounds, theme.focus, 2.0));
        }
        if matches!(&self.content, RecoveryContent::Ready(_)) && self.entries.len() == 256 {
            text(
                ops,
                24.0,
                75.0,
                "Showing at most 256 recoverable checkpoints; Open Recovery Folder accesses an exact additional item.",
                12.0,
                theme.muted,
            );
        }
    }
}

fn next_recovery_focus(current: Option<usize>, enabled: &[bool], backwards: bool) -> Option<usize> {
    if enabled.is_empty() || !enabled.iter().any(|enabled| *enabled) {
        return None;
    }
    let mut index = current.unwrap_or(if backwards { 0 } else { enabled.len() - 1 });
    for _ in 0..enabled.len() {
        index = if backwards {
            (index + enabled.len() - 1) % enabled.len()
        } else {
            (index + 1) % enabled.len()
        };
        if enabled[index] {
            return Some(index);
        }
    }
    None
}
fn center_consumes_event(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::KeyboardInput { .. }
            | WindowEvent::Ime(_)
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
    )
}
fn activates_recovery_command(key: &Key) -> bool {
    matches!(key, Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space))
        || matches!(key, Key::Character(value) if value == " ")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_discovery_excludes_live_adopted_checkpoint_and_keeps_other_data() {
        let root = std::env::temp_dir().join(format!(
            "bareline-recovery-center-claimed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let platform = bareline_platform_windows::WindowsFileSystem;
        let cancel = bareline_file_io::cancellation::Cancellation::default();
        let mut paths = Vec::new();
        for (name, byte) in [("paged-live", b'U'), ("paged-other", b'V')] {
            let directory = root.join(name);
            let mut writer = bareline_file_io::recovery::RecoveryWriter::create(
                &directory,
                bareline_file_io::recovery::RecoveryMetadata {
                    original_path: None,
                    source_generation: format!("fixture-{name}"),
                    codec_catalog_version: "utf8-v1".into(),
                    original_len: 0,
                },
                &platform,
            )
            .unwrap();
            writer
                .seal_baseline(&mut std::io::empty(), || Ok(true), &cancel, &platform)
                .unwrap();
            writer
                .append(
                    1,
                    &[bareline_file_io::recovery::RecoveryEdit {
                        offset: 0,
                        removed: Vec::new(),
                        inserted: vec![byte],
                    }],
                )
                .unwrap();
            writer.checkpoint(&platform).unwrap();
            drop(writer);
            paths.push(directory);
        }
        let excluded = std::collections::HashSet::from([paths[0].clone()]);
        let found = inspect_recovery_root(&root, &excluded, &cancel, &platform).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, paths[1]);
        assert_eq!(found[0].1.status, bareline_file_io::recovery::RecoveryStatus::Complete);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn center_actions_share_keyboard_and_accessibility_targets() {
        let mut runtime = RecoveryRuntime::default();
        runtime.open = true;
        let mut ops = Vec::new();
        runtime.draw(Default::default(), 1000.0, 600.0, &mut ops);
        let enabled: Vec<_> = runtime
            .hits
            .iter()
            .map(|(_, command)| runtime.action_enabled(command))
            .collect();
        let focus = next_recovery_focus(None, &enabled, false);
        assert_eq!(runtime.hits[focus.unwrap()].1, "recovery.keep");
        assert!(runtime.hits.iter().any(|(_, command)| command == "recovery.compare"));
        assert_eq!(next_recovery_focus(focus, &enabled, false), focus);
        let mut ids: Vec<_> = runtime
            .hits
            .iter()
            .map(|(_, command)| recovery_action_id(command))
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
        runtime.content = RecoveryContent::Ready(Vec::new());
        runtime.mutation_allowed = true;
        runtime.confirm_discard = Some(vec![PathBuf::from("named-recovery")]);
        runtime.draw(Default::default(), 1000.0, 600.0, &mut ops);
        assert!(
            runtime
                .hits
                .iter()
                .any(|(_, command)| command == "recovery.confirm_discard")
        );
        assert_eq!(runtime.default_command(), "recovery.confirm_discard");
        assert_eq!(runtime.focused_command(), "recovery.confirm_discard");
        assert_eq!(
            runtime.accessibility_focus(),
            Some(recovery_action_id("recovery.confirm_discard"))
        );
        assert!(activates_recovery_command(&Key::Named(NamedKey::Enter)));

        runtime.mutation_allowed = false;
        assert_eq!(runtime.default_command(), "recovery.keep");
        assert_eq!(runtime.focused_command(), "recovery.keep");
        assert_eq!(runtime.accessibility_focus(), Some(recovery_action_id("recovery.keep")));
        runtime.mutation_allowed = true;
        let (_operation_tx, operation_rx) = mpsc::sync_channel(1);
        runtime.operation = Some(operation_rx);
        assert_eq!(runtime.default_command(), "recovery.keep");
        assert_eq!(runtime.focused_command(), "recovery.keep");
        assert_eq!(runtime.accessibility_focus(), Some(recovery_action_id("recovery.keep")));
    }
    #[test]
    fn center_preserves_window_lifecycle_and_consumes_text_input() {
        for event in [
            WindowEvent::RedrawRequested,
            WindowEvent::CloseRequested,
            WindowEvent::Focused(true),
            WindowEvent::Resized(winit::dpi::PhysicalSize::new(800, 600)),
        ] {
            assert!(!center_consumes_event(&event));
        }
        assert!(center_consumes_event(&WindowEvent::Ime(winit::event::Ime::Commit(
            "x".into()
        ))));
    }
    #[test]
    fn open_transition_and_initial_command_match_keyboard_and_accessibility() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        assert!(shell.recovery.action_enabled("recovery.open"));
        shell.open_recovery_center();
        shell.recovery.draw(Default::default(), 900.0, 600.0, &mut Vec::new());
        assert!(shell.recovery.open);
        assert!(
            shell
                .modal
                .is_some_and(|modal| modal.surface == modal::ModalSurface::Recovery)
        );
        assert_eq!(shell.recovery.focused_command(), "recovery.keep");
        assert_eq!(
            shell.recovery.accessibility_focus(),
            Some(recovery_action_id(&shell.recovery.focused_command()))
        );
        assert!(activates_recovery_command(&Key::Named(NamedKey::Enter)));
        assert!(activates_recovery_command(&Key::Character(" ".into())));

        let token = shell.recovery.token();
        assert!(shell.recovery.accept_discovery(&token, Err("unavailable".into())));
        assert!(shell.recovery.action_enabled("recovery.open"));
        assert!(shell.recovery.action_enabled("recovery.keep"));
        assert!(shell.recovery.action_enabled("recovery.discovery_retry"));
        assert!(!shell.recovery.action_enabled("recovery.restore_selected"));
        shell.recovery.draw(Default::default(), 900.0, 600.0, &mut Vec::new());
        assert_eq!(shell.recovery.focused_command(), "recovery.discovery_retry");
        assert_eq!(
            shell.recovery.accessibility_focus(),
            Some(recovery_action_id("recovery.discovery_retry"))
        );
    }
    fn inspection(
        original: Option<&str>,
        protected_unix_ms: u64,
        size: u64,
    ) -> bareline_file_io::recovery::RecoveryInspection {
        use bareline_file_io::recovery::{DurableReceipt, RecoveryInspection, RecoveryMetadata, RecoveryStatus};
        RecoveryInspection {
            status: RecoveryStatus::Complete,
            metadata: RecoveryMetadata {
                original_path: original.map(PathBuf::from),
                source_generation: "test".into(),
                codec_catalog_version: "utf8-v1".into(),
                original_len: size,
            },
            last_durable: Some(DurableReceipt {
                revision: protected_unix_ms,
                protected_unix_ms,
            }),
            checkpoint_durable: None,
            validated_records: 0,
            complete_baseline: true,
            document_metadata: None,
        }
    }
    #[test]
    fn ninety_nine_directories_collapse_to_one_row_with_the_newest_checkpoint() {
        // 99 checkpoint directories for the same document, plus one for another.
        let mut entries = Vec::new();
        for i in 0..99u64 {
            entries.push((
                PathBuf::from(format!("paged-{i}")),
                inspection(Some("/docs/report.txt"), i * 10, 4096),
            ));
        }
        entries.push((PathBuf::from("paged-other"), inspection(Some("/docs/notes.txt"), 5, 10)));
        let rows = group_documents(&entries);
        assert_eq!(rows.len(), 2, "two distinct documents");
        // Newest checkpoint wins and rows sort newest-first.
        let report = &rows[0];
        assert_eq!(report.name, "report.txt");
        assert_eq!(report.count, 99);
        assert_eq!(report.group.len(), 99);
        assert_eq!(report.protected_unix_ms, 980);
        assert_eq!(report.directory, PathBuf::from("paged-98"));
        assert_eq!(rows[1].name, "notes.txt");
        assert_eq!(rows[1].count, 1);
        // A path-less (untitled) recovery is never merged into a titled document.
        let untitled = vec![
            (PathBuf::from("paged-a"), inspection(None, 1, 0)),
            (PathBuf::from("paged-b"), inspection(None, 2, 0)),
        ];
        let rows = group_documents(&untitled);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.name == "Untitled document" && row.count == 1));
    }

    #[test]
    fn delayed_discovery_is_loading_until_a_tagged_terminal_result() {
        let mut runtime = RecoveryRuntime::default();
        runtime.configure(Some(PathBuf::from("root-a")), true);
        runtime.open = true;
        runtime.draw(Default::default(), 900.0, 600.0, &mut Vec::new());
        let loading = runtime.accessibility_nodes(rect(0.0, 0.0, 900.0, 600.0));
        assert!(
            loading
                .iter()
                .any(|node| node.id == 100_902 && node.name.contains("Searching"))
        );
        assert!(!loading.iter().any(|node| node.name.contains("No recovery checkpoints")));

        let token = runtime.token();
        assert!(runtime.accept_discovery(
            &token,
            Ok(vec![(
                PathBuf::from("checkpoint-a"),
                inspection(Some("/docs/a.txt"), 10, 4)
            )])
        ));
        runtime.draw(Default::default(), 900.0, 600.0, &mut Vec::new());
        let ready = runtime.accessibility_nodes(rect(0.0, 0.0, 900.0, 600.0));
        assert_eq!(runtime.rows().len(), 1);
        assert_eq!(ready.iter().filter(|node| node.id == 100_902).count(), 1);
        assert!(
            ready
                .iter()
                .any(|node| node.id == 100_902 && node.name.contains("1 recoverable"))
        );

        runtime.request_discovery();
        let empty_token = runtime.token();
        assert!(runtime.accept_discovery(&empty_token, Ok(Vec::new())));
        runtime.draw(Default::default(), 900.0, 600.0, &mut Vec::new());
        assert!(
            runtime
                .accessibility_nodes(rect(0.0, 0.0, 900.0, 600.0))
                .iter()
                .any(|node| node.id == 100_902 && node.name == "No recovery checkpoints found.")
        );
    }

    #[test]
    fn discovery_rejects_obsolete_root_and_preserves_selection_identity() {
        let mut runtime = RecoveryRuntime::default();
        runtime.configure(Some(PathBuf::from("root-a")), true);
        let obsolete = runtime.token();
        runtime.configure(Some(PathBuf::from("root-b")), true);
        assert!(!runtime.accept_discovery(&obsolete, Ok(Vec::new())));
        assert!(matches!(&runtime.content, RecoveryContent::Discovering));

        let current = runtime.token();
        assert!(runtime.accept_discovery(
            &current,
            Ok(vec![
                (PathBuf::from("a-old"), inspection(Some("/docs/a.txt"), 20, 4)),
                (PathBuf::from("b-old"), inspection(Some("/docs/b.txt"), 10, 4)),
            ])
        ));
        runtime.selected = 1;
        runtime.remember_selection();
        runtime.request_discovery();
        let refresh = runtime.token();
        assert!(runtime.accept_discovery(
            &refresh,
            Ok(vec![
                (PathBuf::from("b-new"), inspection(Some("/docs/b.txt"), 40, 4)),
                (PathBuf::from("a-old"), inspection(Some("/docs/a.txt"), 20, 4)),
            ])
        ));
        assert_eq!(
            runtime.rows()[runtime.selected].identity(),
            RecoveryRowId::Original(PathBuf::from("/docs/b.txt"))
        );
    }

    #[test]
    fn restore_refresh_waits_for_matching_terminal_publication_and_fences_stale_results() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        shell.recovery.configure(Some(PathBuf::from("root-a")), true);
        let token = shell.recovery.token();
        let generation = token.generation;
        assert!(shell.recovery.accept_discovery(
            &token,
            Ok(vec![(
                PathBuf::from("checkpoint-a"),
                inspection(Some("/docs/a.txt"), 10, 4),
            )]),
        ));
        shell.recovery.pending_restore = Some(PendingRestore {
            directory: PathBuf::from("checkpoint-a"),
            token: token.clone(),
            request_id: 41,
        });
        assert_eq!(
            shell.recovery.discovery_generation, generation,
            "submission does not refresh"
        );
        for command in [
            "recovery.restore_selected",
            "recovery.compare",
            "recovery.export",
            "recovery.discard",
            "recovery.delete_old",
        ] {
            assert!(!shell.recovery.action_enabled(command), "{command} raced restore");
        }
        assert!(shell.recovery.action_enabled("recovery.open"));
        assert!(shell.recovery.action_enabled("recovery.keep"));
        assert!(shell.recovery.action_enabled("recovery.select.0"));
        assert_eq!(shell.recovery.default_command(), "recovery.keep");
        let pending = shell.recovery.pending_restore.take().unwrap();
        shell.finish_pending_restore(
            pending,
            bareline_app::workspace::RecoveryRestoreOutcome::Restored {
                request_id: 41,
                document: (9, 3),
            },
        );
        assert_eq!(shell.recovery.discovery_generation, generation + 1);
        assert!(matches!(&shell.recovery.content, RecoveryContent::Discovering));
        assert!(
            shell.recovery.entries.is_empty(),
            "claimed row is removed before rescan"
        );
        assert_eq!(
            shell.recovery.claimed_directories.get(&PathBuf::from("checkpoint-a")),
            Some(&9)
        );
        assert!(
            !shell
                .recovery
                .release_closed_claims(&std::collections::BTreeSet::from([9]))
        );
        assert!(shell.recovery.release_closed_claims(&std::collections::BTreeSet::new()));

        shell.recovery.configure(Some(PathBuf::from("root-b")), true);
        let stale = PendingRestore {
            directory: PathBuf::from("checkpoint-a"),
            token,
            request_id: 42,
        };
        let current_generation = shell.recovery.discovery_generation;
        shell.finish_pending_restore(
            stale,
            bareline_app::workspace::RecoveryRestoreOutcome::Failed {
                request_id: 42,
                error: "old root failed".into(),
            },
        );
        assert_eq!(shell.recovery.discovery_generation, current_generation);
        assert!(
            shell.toasts.is_empty(),
            "stale failures do not escape their root/generation"
        );

        let current = PendingRestore {
            directory: PathBuf::from("checkpoint-current"),
            token: shell.recovery.token(),
            request_id: 43,
        };
        shell.finish_pending_restore(
            current,
            bareline_app::workspace::RecoveryRestoreOutcome::Failed {
                request_id: 43,
                error: "full restore failure".into(),
            },
        );
        assert_eq!(shell.recovery.discovery_generation, current_generation);
        assert_eq!(
            shell.toasts.persistent_len(),
            1,
            "failure retains its checkpoint and details"
        );
    }

    #[test]
    fn checkpoint_operations_are_mutually_exclusive_and_default_focus_is_enabled() {
        let mut runtime = RecoveryRuntime::default();
        runtime.configure(Some(PathBuf::from("root")), true);
        let token = runtime.token();
        assert!(runtime.accept_discovery(
            &token,
            Ok(vec![(
                PathBuf::from("checkpoint"),
                inspection(Some("/docs/a.txt"), 10, 4),
            )]),
        ));
        let (_operation_tx, operation_rx) = mpsc::sync_channel(1);
        runtime.operation = Some(operation_rx);
        for command in [
            "recovery.restore_selected",
            "recovery.compare",
            "recovery.export",
            "recovery.discard",
            "recovery.delete_old",
        ] {
            assert!(!runtime.action_enabled(command), "{command} raced an operation");
        }
        assert_eq!(runtime.default_command(), "recovery.keep");
        runtime.operation = None;
        runtime.pending_compare = Some(PendingCompare {
            directory: PathBuf::from("checkpoint"),
            existing: Vec::new(),
            original: PathBuf::from("/docs/a.txt"),
            recovered: None,
        });
        assert!(!runtime.action_enabled("recovery.restore_selected"));
        assert!(!runtime.action_enabled("recovery.export"));
        runtime.pending_compare = None;

        let RecoveryContent::Ready(rows) = &mut runtime.content else {
            unreachable!()
        };
        rows[0].complete_baseline = false;
        runtime.open = true;
        runtime.draw(Default::default(), 900.0, 600.0, &mut Vec::new());
        assert!(!runtime.action_enabled("recovery.restore_selected"));
        assert_eq!(runtime.default_command(), "recovery.keep");
        assert_eq!(runtime.focused_command(), "recovery.keep");
        assert_eq!(runtime.accessibility_focus(), Some(recovery_action_id("recovery.keep")));
    }

    #[test]
    fn failed_discovery_exposes_full_error_retry_and_keeps_global_worker_on_close() {
        let mut runtime = RecoveryRuntime::default();
        runtime.configure(Some(PathBuf::from("root")), true);
        runtime.open = true;
        let token = runtime.token();
        let full_error = "access denied at C:\\recovery\\private";
        assert!(runtime.accept_discovery(&token, Err(full_error.into())));
        assert!(runtime.rows().is_empty(), "failure cannot reuse stale ready rows");
        runtime.draw(Default::default(), 900.0, 600.0, &mut Vec::new());
        let nodes = runtime.accessibility_nodes(rect(0.0, 0.0, 900.0, 600.0));
        assert!(
            nodes
                .iter()
                .any(|node| node.id == 100_902 && node.value.as_deref() == Some(full_error))
        );
        assert!(
            runtime
                .hits
                .iter()
                .any(|(_, command)| command == "recovery.discovery_retry")
        );

        runtime.request_discovery();
        let (_tx, rx) = mpsc::sync_channel(1);
        runtime.pending = Some(rx);
        runtime.started = true;
        runtime.preview_path = Some(PathBuf::from("preview"));
        runtime.dismiss();
        assert!(
            runtime.pending.is_some(),
            "closing only detaches preview, not global discovery"
        );
        assert!(
            runtime.preview_path.is_none(),
            "closing detaches the preview worker and its result"
        );
    }

    #[test]
    fn closed_discovery_completion_notifies_once_per_generation() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        let token = DiscoveryToken {
            generation: 12,
            root: Some(PathBuf::from("root")),
        };
        shell.publish_closed_discovery(&token, 2, None);
        shell.publish_closed_discovery(&token, 2, None);
        assert_eq!(shell.toasts.persistent_len(), 1);
    }

    #[test]
    fn removed_rows_select_the_nearest_survivor_and_closed_discovery_stays_closed() {
        let mut runtime = RecoveryRuntime::default();
        runtime.entries = vec![
            (PathBuf::from("one"), inspection(Some("/docs/one.txt"), 1, 10)),
            (PathBuf::from("two"), inspection(Some("/docs/two.txt"), 2, 10)),
        ];
        runtime.rebuild_rows();
        runtime.selected = 1;
        runtime.remember_selection();
        runtime.focus = Some(0);
        let generation = runtime.discovery_generation;
        runtime.refresh_after_operation(&[PathBuf::from("two")]);
        assert_eq!(runtime.selected, 0);
        assert_eq!(runtime.entries.len(), 1);
        assert_eq!(runtime.discovery_generation, generation + 1);
        assert!(matches!(&runtime.content, RecoveryContent::Discovering));
        assert_eq!(runtime.focus, None);

        runtime.dismiss();
        assert!(!should_auto_open(1, runtime.allow_auto_open, runtime.open, false));
        assert!(!should_auto_open(1, true, false, true));
        assert!(should_auto_open(1, true, false, false));
    }

    #[test]
    fn pump_notice_transition_is_generation_fenced_and_closed_failure_is_retained() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        let document = (17, 4);
        shell.publish_recovery_notice(
            document,
            RecoveryNoticeState::Preparing {
                directory: PathBuf::from("recovery-17"),
            },
        );
        assert!(shell.toasts.scoped_for(document).is_some());
        assert!(shell.toasts.scoped_for((17, 3)).is_none());

        shell.publish_recovery_notice(
            document,
            RecoveryNoticeState::Failed {
                directory: Some(PathBuf::from("recovery-17")),
                error: "checkpoint failed".into(),
            },
        );
        assert!(shell.toasts.scoped_for(document).is_none());
        assert_eq!(shell.toasts.persistent_len(), 1);
        shell.toasts.clear_document(document);
        assert_eq!(shell.toasts.persistent_len(), 1);

        shell.publish_recovery_notice(document, RecoveryNoticeState::Idle);
        assert!(shell.toasts.is_empty());
    }
}

fn recovery_action_id(command: &str) -> u64 {
    if let Some(index) = command
        .strip_prefix("recovery.select.")
        .and_then(|value| value.parse::<u64>().ok())
    {
        return 100_000 + index;
    }
    100_500
        + match command {
            "recovery.restore_selected" => 0,
            "recovery.compare" => 1,
            "recovery.export" => 2,
            "recovery.discard" => 3,
            "recovery.keep" => 4,
            "recovery.confirm_discard" => 5,
            "recovery.delete_old" => 6,
            "recovery.discovery_retry" => 7,
            _ => 99,
        }
}
impl Shell {
    pub(super) fn recovery_accessibility_nodes(&self) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        self.recovery.accessibility_nodes(self.editor_bounds())
    }
    pub(super) fn recovery_accessibility_focus(&self) -> Option<u64> {
        self.recovery.accessibility_focus()
    }
    pub(super) fn recovery_accessibility(
        &mut self,
        el: &ActiveEventLoop,
        action: &bareline_platform::accessibility::AccessibilityAction,
    ) -> bool {
        use bareline_platform::accessibility::AccessibilityAction;
        if !self.recovery.open {
            return false;
        }
        let (id, invoke) = match action {
            AccessibilityAction::Focus(id) => (*id, false),
            AccessibilityAction::Invoke(id) => (*id, true),
            _ => return false,
        };
        let Some((index, (_, command))) = self
            .recovery
            .hits
            .iter()
            .enumerate()
            .find(|(_, (_, command))| recovery_action_id(command) == id)
        else {
            return false;
        };
        let command = command.clone();
        if !self.recovery.action_enabled(&command) {
            return false;
        }
        self.recovery.focus = Some(index);
        if invoke {
            self.recovery_dispatch(el, &command);
        } else if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

fn recovery_state_label(status: bareline_file_io::recovery::RecoveryStatus) -> &'static str {
    use bareline_file_io::recovery::RecoveryStatus;
    match status {
        RecoveryStatus::Complete => "Complete",
        RecoveryStatus::EditsOnly => "Edits only",
        RecoveryStatus::CorruptTail => "Corrupt tail",
        RecoveryStatus::SourceUnavailable => "Source unavailable",
        RecoveryStatus::Discarded => "Discarded",
    }
}

impl RecoveryRuntime {
    pub(super) fn accessibility_nodes(
        &self,
        origin: bareline_renderer::Rect,
    ) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        if !self.open {
            return Vec::new();
        }
        let mut nodes = vec![AccessibilityNode {
            id: 100_900,
            parent: 1,
            role: AccessibilityRole::Group,
            name: "Recovery Center".into(),
            value: None,
            bounds: [
                origin.x as f64,
                origin.y as f64,
                origin.width as f64,
                origin.height as f64,
            ],
            disabled: false,
            selected: false,
            expanded: None,
            focusable: false,
            invokable: false,
        }];
        let (status_name, status_value) = match &self.content {
            RecoveryContent::Discovering => ("Searching for recovery checkpoints…".into(), None),
            RecoveryContent::Ready(rows) if rows.is_empty() => ("No recovery checkpoints found.".into(), None),
            RecoveryContent::Ready(rows) => (format!("{} recoverable document(s) found.", rows.len()), None),
            RecoveryContent::Failed(error) => ("Recovery checkpoint search failed.".into(), Some(error.clone())),
        };
        nodes.push(AccessibilityNode {
            id: 100_902,
            parent: 100_900,
            role: AccessibilityRole::Status,
            name: status_name,
            value: status_value,
            bounds: [origin.x as f64, (origin.y + 70.0) as f64, origin.width as f64, 48.0],
            disabled: false,
            selected: false,
            expanded: None,
            focusable: false,
            invokable: false,
        });
        for (bounds, command) in &self.hits {
            let row = command
                .strip_prefix("recovery.select.")
                .and_then(|s| s.parse::<usize>().ok());
            let name = if let Some(index) = row {
                self.rows()
                    .get(index)
                    .map(|entry| {
                        let original = entry
                            .original
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "not yet saved to disk".into());
                        format!(
                            "{}; {}; {}; {}; {}",
                            entry.name,
                            original,
                            recovery_state_label(entry.status),
                            relative_time(entry.protected_unix_ms),
                            format_size(entry.size)
                        )
                    })
                    .unwrap_or_else(|| "Recovery checkpoint".into())
            } else {
                match command.as_str() {
                    "recovery.restore_selected" => "Restore recovered copy",
                    "recovery.compare" => "Compare with current disk",
                    "recovery.export" => "Export saved edits and gap report",
                    "recovery.discard" => "Delete this recovery",
                    "recovery.delete_old" => "Delete all recoveries older than 7 days",
                    "recovery.keep" => "Close Recovery Center",
                    "recovery.confirm_discard" => "Confirm irreversible discard",
                    "recovery.discovery_retry" => "Retry recovery checkpoint search",
                    _ => "Recovery action",
                }
                .into()
            };
            nodes.push(AccessibilityNode {
                id: recovery_action_id(command),
                parent: 100_900,
                role: if row.is_some() {
                    AccessibilityRole::ListItem
                } else {
                    AccessibilityRole::Button
                },
                name,
                value: None,
                bounds: [
                    (bounds.x + origin.x) as f64,
                    (bounds.y + origin.y) as f64,
                    bounds.width as f64,
                    bounds.height as f64,
                ],
                disabled: !self.action_enabled(command),
                selected: row == Some(self.selected),
                expanded: None,
                focusable: self.action_enabled(command),
                invokable: self.action_enabled(command),
            });
        }
        if matches!(&self.content, RecoveryContent::Ready(_)) {
            nodes.push(AccessibilityNode {
                id: 100_901,
                parent: 100_900,
                role: AccessibilityRole::Status,
                name: if let Some(paths) = &self.confirm_discard {
                    format!(
                        "Permanently discard {} recovery checkpoint(s)? This cannot be undone.",
                        paths.len()
                    )
                } else {
                    "Recovered copy preview".into()
                },
                value: Some(self.preview_text.clone()),
                bounds: [
                    origin.x as f64,
                    (origin.y + origin.height - 270.0).max(origin.y) as f64,
                    origin.width as f64,
                    150.0,
                ],
                disabled: false,
                selected: false,
                expanded: None,
                focusable: false,
                invokable: false,
            });
        }
        nodes
    }
    pub(super) fn accessibility_focus(&self) -> Option<u64> {
        self.open.then(|| {
            self.focus
                .and_then(|index| self.hits.get(index))
                .map_or(recovery_action_id(self.default_command()), |(_, command)| {
                    recovery_action_id(command)
                })
        })
    }
}

/// Golden fixtures use the same layout, enabled-state policy, node builder and
/// focus mapping as the native Center; no window or disk access is required.
#[cfg(test)]
pub(super) fn accessibility_test_cases() -> Vec<(
    &'static str,
    Vec<bareline_platform::accessibility::AccessibilityNode>,
    Option<u64>,
)> {
    use bareline_file_io::recovery::{DurableReceipt, RecoveryInspection, RecoveryMetadata, RecoveryStatus};
    let mut result = Vec::new();
    for scenario in [
        "closed",
        "open_empty",
        "populated",
        "discard_confirmation",
        "focus_forward",
        "focus_backward",
    ] {
        let mut runtime = RecoveryRuntime::default();
        runtime.open = scenario != "closed";
        runtime.mutation_allowed = true;
        if scenario == "open_empty" {
            runtime.content = RecoveryContent::Ready(Vec::new());
        }
        if !matches!(scenario, "closed" | "open_empty") {
            runtime.entries.push((
                PathBuf::from("checkpoint-1"),
                RecoveryInspection {
                    status: RecoveryStatus::Complete,
                    metadata: RecoveryMetadata {
                        original_path: Some(PathBuf::from("document.txt")),
                        source_generation: "fixture-generation".into(),
                        codec_catalog_version: "bareline-codecs-v1".into(),
                        original_len: 12,
                    },
                    last_durable: Some(DurableReceipt {
                        revision: 3,
                        protected_unix_ms: 123456,
                    }),
                    checkpoint_durable: Some(DurableReceipt {
                        revision: 3,
                        protected_unix_ms: 123456,
                    }),
                    validated_records: 3,
                    complete_baseline: true,
                    document_metadata: None,
                },
            ));
            runtime.rebuild_rows();
            runtime.preview_text = "First line\nSecond line".into();
        }
        if scenario == "discard_confirmation" {
            runtime.confirm_discard = Some(vec![PathBuf::from("checkpoint-1")]);
        }
        runtime.draw(Default::default(), 1000.0, 800.0, &mut Vec::new());
        let enabled: Vec<_> = runtime
            .hits
            .iter()
            .map(|(_, command)| runtime.action_enabled(command))
            .collect();
        if scenario == "focus_forward" {
            runtime.focus = next_recovery_focus(None, &enabled, false);
        }
        if scenario == "focus_backward" {
            runtime.focus = next_recovery_focus(None, &enabled, true);
        }
        result.push((
            scenario,
            runtime.accessibility_nodes(rect(0.0, 0.0, 1000.0, 800.0)),
            runtime.accessibility_focus(),
        ));
    }
    result
}

#[cfg(test)]
pub(super) fn accessibility_modal_test_setup(shell: &mut Shell) {
    shell.activate_modal(modal::ModalSurface::Recovery);
    shell.recovery.open = true;
    shell.recovery.draw(Default::default(), 1000.0, 800.0, &mut Vec::new());
}

#[cfg(windows)]
mod alive {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, id: u32) -> isize;
        fn GetExitCodeProcess(process: isize, code: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }
    /// True when a process with this id is still running. Unknown ids are reported as
    /// running so a doubtful case never deletes someone else's recovery data.
    pub fn running(id: u32) -> bool {
        if id == std::process::id() {
            return true;
        }
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, id);
            if process == 0 {
                return false;
            }
            let mut code = 0u32;
            let queried = GetExitCodeProcess(process, &mut code) != 0;
            CloseHandle(process);
            !queried || code == STILL_ACTIVE
        }
    }
}
#[cfg(windows)]
fn process_alive(id: u32) -> bool {
    alive::running(id)
}
#[cfg(not(windows))]
fn process_alive(id: u32) -> bool {
    id == std::process::id()
}
