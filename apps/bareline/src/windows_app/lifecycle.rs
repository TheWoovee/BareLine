// SPDX-License-Identifier: MPL-2.0
//! Native file commands. Save All retains document identity while dialogs change focus.
use super::*;
use bareline_app::workspace::WorkspaceEditor;
use bareline_commands::{CommandContext, CommandId, CommandRegistry, CommandSpec, CommandState};
use bareline_file_io::cancellation::Cancellation;
use bareline_file_io::lifecycle::{DestinationPreflight, SaveOperation, preflight_destination};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
#[derive(Clone)]
pub(super) enum Identity {
    Resident(bareline_document::DocumentSnapshot),
    Paged(bareline_document::paged::PagedSnapshot),
}
impl Identity {
    pub(super) fn capture(editor: &WorkspaceEditor) -> Self {
        match editor {
            WorkspaceEditor::Resident(e) => Self::Resident(e.snapshot().clone()),
            WorkspaceEditor::Paged(e) => Self::Paged(e.snapshot().clone()),
        }
    }
    pub(super) fn matches(&self, editor: &WorkspaceEditor) -> bool {
        match (self, editor) {
            (Self::Resident(a), WorkspaceEditor::Resident(b)) => a.identity_token() == b.snapshot().identity_token(),
            (Self::Paged(a), WorkspaceEditor::Paged(b)) => a.identity_token() == b.snapshot().identity_token(),
            _ => false,
        }
    }
    fn token(&self) -> (u64, u64) {
        match self {
            Self::Resident(snapshot) => snapshot.identity_token(),
            Self::Paged(snapshot) => snapshot.identity_token(),
        }
    }
}
struct PendingDestination {
    identity: Identity,
    operation: SaveOperation,
    save_all: bool,
    cancellation: Cancellation,
    cancelled: bool,
    receiver: Receiver<Result<DestinationPreflight, bareline_file_io::lifecycle::FileError>>,
}
enum SaveAllStep {
    Waiting,
    NeedsDestination { identity: Identity, index: usize },
    Complete,
}
enum PendingConflictAction {
    Compare {
        transaction: PathBuf,
        editor: PathBuf,
        other: PathBuf,
        editor_request: u64,
        other_request: u64,
        editor_document: Option<(u64, u64)>,
        other_document: Option<(u64, u64)>,
    },
    SaveElsewhere {
        transaction: PathBuf,
        editor: PathBuf,
        request: u64,
        document: Option<(u64, u64)>,
    },
    DrainFailed {
        requests: Vec<u64>,
        message: String,
    },
}
impl Drop for PendingDestination {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
#[derive(Default)]
pub(super) struct LifecycleRuntime {
    queue: VecDeque<Identity>,
    pending: Option<Identity>,
    saved: usize,
    skipped: usize,
    failed: usize,
    running: bool,
    preflight: Option<PendingDestination>,
    last_save_directory: Option<PathBuf>,
    conflict_action: Option<PendingConflictAction>,
    next_conflict_open_request: u64,
    #[cfg(test)]
    save_destination_picker: Option<std::sync::Arc<dyn Fn(&str, Option<&Path>) -> Option<PathBuf> + Send + Sync>>,
    #[cfg(test)]
    conflict_open: Option<std::sync::Arc<dyn Fn(&mut Workspace, u64, PathBuf) -> Result<(), String> + Send + Sync>>,
    #[cfg(test)]
    conflict_open_outcomes: Vec<bareline_app::workspace::LaunchOpenOutcome>,
}
pub(super) fn register(registry: &mut CommandRegistry) {
    for (id, title, shortcut) in [
        ("file.save_copy", "Save Copy…", ""),
        ("file.save_all", "Save All", ""),
        ("file.restore_closed", "Restore Last Closed Tab", "Ctrl+Shift+T"),
        ("file.read_only", "Set Read-Only", ""),
        ("file.cancel_save_all", "Cancel Save All", ""),
        ("file.save_conflict_compare", "Compare Save Conflict Versions", ""),
        (
            "file.save_conflict_save_elsewhere",
            "Save Conflicted Editor Version Elsewhere",
            "",
        ),
        ("file.save_conflict_retain_other", "Retain Other Save Version", ""),
        ("file.save_conflict_next", "Select Next Save Conflict", ""),
        ("file.retry_save_cleanup", "Retry Saved Recovery Cleanup", ""),
        ("file.retry_save_recovery", "Retry Save Recovery Discovery", ""),
    ] {
        let id = CommandId(id);
        let _ = registry.register(CommandSpec {
            id,
            title,
            category: "File",
            shortcut,
            action: Action::Contributed(id),
        });
    }
}
impl LifecycleRuntime {
    fn open_conflict(&self, workspace: &mut Workspace, request: u64, path: PathBuf) -> Result<(), String> {
        #[cfg(test)]
        if let Some(open) = &self.conflict_open {
            return open(workspace, request, path);
        }
        workspace.open_recovery_tracked(request, path)
    }

    fn allocate_conflict_open_request(&mut self) -> u64 {
        if self.next_conflict_open_request < (1u64 << 63) {
            self.next_conflict_open_request = 1u64 << 63;
        }
        let request = self.next_conflict_open_request;
        self.next_conflict_open_request = self.next_conflict_open_request.saturating_add(1);
        request
    }
    pub(super) fn busy(&self) -> bool {
        self.running || self.preflight.is_some()
    }
    pub(super) fn preparing(&self, identity: (u64, u64)) -> bool {
        self.preflight
            .as_ref()
            .is_some_and(|pending| pending.identity.token() == identity)
    }
    pub(super) fn cancel_preflight(&mut self, identity: (u64, u64)) {
        if let Some(pending) = &mut self.preflight
            && pending.identity.token() == identity
        {
            pending.cancellation.cancel();
            pending.cancelled = true;
        }
    }
    fn record_save_all_submission(&mut self, identity: Identity, accepted: bool) -> bool {
        if accepted {
            self.pending = Some(identity);
            true
        } else {
            self.failed += 1;
            false
        }
    }
    pub(super) fn annotate_context(&self, context: &mut CommandContext, workspace: Option<&Workspace>, active: usize) {
        let save_all_targets = workspace.map_or(0, |workspace| workspace.save_all_targets().len());
        for id in ["file.save_all", "file.cancel_save_all"] {
            context.states.insert(
                CommandId(id),
                save_all_command_state(id, self.running, save_all_targets),
            );
        }
        context.states.insert(
            CommandId("file.read_only"),
            CommandState {
                checked: workspace
                    .and_then(|w| w.editors.get(active))
                    .is_some_and(|e| e.read_only()),
                ..Default::default()
            },
        );
        for (id, disabled) in [
            (
                "file.restore_closed",
                !workspace.is_some_and(|w| w.can_restore_closed()),
            ),
            (
                "file.save_conflict_compare",
                self.conflict_action.is_some()
                    || !workspace.is_some_and(|w| {
                        w.selected_actionable_save_conflict(active)
                            .is_some_and(|conflict| conflict.other_version.is_some())
                    }),
            ),
            (
                "file.save_conflict_save_elsewhere",
                self.conflict_action.is_some()
                    || !workspace.is_some_and(|w| w.selected_actionable_save_conflict(active).is_some()),
            ),
            (
                "file.save_conflict_retain_other",
                !workspace.is_some_and(|w| w.selected_save_conflict(active).is_some()),
            ),
            (
                "file.save_conflict_next",
                !workspace.is_some_and(|w| !w.save_conflicts().is_empty()),
            ),
            (
                "file.retry_save_cleanup",
                !workspace.is_some_and(|w| !w.save_cleanups().is_empty()),
            ),
            (
                "file.retry_save_recovery",
                !workspace.is_some_and(|w| w.failed_save_recovery().is_some()),
            ),
        ] {
            context.states.insert(
                CommandId(id),
                if disabled {
                    CommandState::disabled("No applicable file operation")
                } else {
                    CommandState::default()
                },
            );
        }
    }
}

fn save_all_command_state(id: &str, running: bool, targets: usize) -> CommandState {
    match id {
        "file.save_all" if running => CommandState::disabled("Save All is already running"),
        "file.save_all" if targets == 0 => CommandState::disabled("No modified documents to save"),
        "file.cancel_save_all" if !running => CommandState::disabled("Save All is not running"),
        _ => CommandState::default(),
    }
}

fn advance_existing_save_all(runtime: &mut LifecycleRuntime, workspace: &mut Workspace) -> SaveAllStep {
    if let Some(identity) = &runtime.pending {
        if let Some(index) = workspace.editors.iter().position(|editor| identity.matches(editor)) {
            if workspace.document_busy(index) {
                return SaveAllStep::Waiting;
            }
            if workspace.editors[index].dirty() {
                runtime.failed += 1;
            } else {
                runtime.saved += 1;
            }
        } else {
            runtime.skipped += 1;
        }
        runtime.pending = None;
    }
    while let Some(identity) = runtime.queue.pop_front() {
        let Some(index) = workspace.editors.iter().position(|editor| identity.matches(editor)) else {
            runtime.skipped += 1;
            continue;
        };
        if workspace.document_busy(index) {
            runtime.queue.push_front(identity);
            return SaveAllStep::Waiting;
        }
        let Some(path) = workspace.path(index).map(PathBuf::from) else {
            return SaveAllStep::NeedsDestination { identity, index };
        };
        let accepted = workspace.save(index, path);
        if runtime.record_save_all_submission(identity, accepted) {
            return SaveAllStep::Waiting;
        }
    }
    SaveAllStep::Complete
}
impl Shell {
    fn choose_save_document(&self, name: &str, directory: Option<&Path>) -> Result<Option<PathBuf>, String> {
        #[cfg(test)]
        if let Some(picker) = &self.lifecycle.save_destination_picker {
            return Ok(picker(name, directory));
        }
        self.platform
            .as_ref()
            .map(|platform| platform.save_document_file_at(name, directory))
            .unwrap_or(Ok(None))
    }
    pub(super) fn start_save_all(&mut self) {
        if self.lifecycle.running {
            return;
        }
        if let Some(workspace) = &self.workspace {
            self.lifecycle.queue = workspace
                .save_all_targets()
                .iter()
                .map(|(index, _)| Identity::capture(&workspace.editors[*index]))
                .collect();
            self.lifecycle.pending = None;
            self.lifecycle.saved = 0;
            self.lifecycle.skipped = 0;
            self.lifecycle.failed = 0;
            self.lifecycle.running = true;
        }
        (self.notify)();
    }
    fn save_dialog_defaults(&self, index: usize) -> (String, Option<PathBuf>) {
        let title = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.titles().get(index).cloned())
            .unwrap_or_else(|| format!("Untitled {}", index + 1));
        let trimmed = title.trim_end_matches(['\u{2022}', '*', '\u{25cf}', ' ']);
        let name = if Path::new(trimmed).extension().is_some() {
            trimmed.to_owned()
        } else {
            format!("{trimmed}.txt")
        };
        let directory = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.path(index))
            .and_then(Path::parent)
            .filter(|path| path.is_dir())
            .map(PathBuf::from)
            .or_else(|| self.lifecycle.last_save_directory.clone().filter(|path| path.is_dir()));
        (name, directory)
    }

    pub(super) fn request_document_save(&mut self, index: usize, operation: SaveOperation) -> bool {
        let existing = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.path(index))
            .map(PathBuf::from);
        if operation == SaveOperation::Save
            && let Some(path) = existing
        {
            return self
                .workspace
                .as_mut()
                .is_some_and(|workspace| workspace.save(index, path));
        }
        if self.lifecycle.preflight.is_some() {
            if let Some(workspace) = &mut self.workspace {
                workspace.message = Some("Wait for the pending save destination check.".into());
            }
            return false;
        }
        let (mut name, directory) = self.save_dialog_defaults(index);
        if operation == SaveOperation::SaveCopy {
            let path = Path::new(&name);
            let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("Untitled");
            let extension = path.extension().and_then(|extension| extension.to_str());
            name = extension.map_or_else(
                || format!("{stem} - Copy"),
                |extension| format!("{stem} - Copy.{extension}"),
            );
        }
        let path = match self.choose_save_document(&name, directory.as_deref()) {
            Ok(Some(path)) => path,
            Ok(None) => return false,
            Err(error) => {
                self.toasts.push_typed(
                    "save-dialog",
                    toast::next_revision(),
                    bareline_ui::theme::ToastLevel::Error,
                    toast::NotificationKind::Outcome,
                    "The save destination could not be chosen.",
                    Some(error),
                    None,
                    toast::NotificationLifetime::Persistent,
                    Instant::now(),
                );
                return false;
            }
        };
        self.begin_destination_preflight(index, path, operation, false)
    }

    fn begin_destination_preflight(
        &mut self,
        index: usize,
        path: PathBuf,
        operation: SaveOperation,
        save_all: bool,
    ) -> bool {
        let Some(workspace) = &self.workspace else {
            return false;
        };
        let Some(editor) = workspace.editors.get(index) else {
            return false;
        };
        let identity = Identity::capture(editor);
        let document = identity.token();
        let source = workspace.path(index).map(PathBuf::from);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let cancellation = Cancellation::default();
        let worker_cancellation = cancellation.clone();
        let wake = self.wake.clone();
        let spawned = std::thread::Builder::new()
            .name("save-destination-preflight".into())
            .spawn(move || {
                let result = preflight_destination(
                    &path,
                    source.as_deref(),
                    document,
                    operation,
                    &bareline_platform_windows::WindowsFileSystem,
                    &worker_cancellation,
                );
                let _ = sender.send(result);
                wake(Wake::One(Source::Lifecycle));
            });
        match spawned {
            Ok(_) => {
                self.lifecycle.preflight = Some(PendingDestination {
                    identity,
                    operation,
                    save_all,
                    cancellation,
                    cancelled: false,
                    receiver,
                });
                if let Some(workspace) = &mut self.workspace {
                    workspace.message = Some("Checking save destination…".into());
                }
                true
            }
            Err(error) => {
                self.toasts.push_typed(
                    format!("save-preflight-{}", document.0),
                    toast::next_revision(),
                    bareline_ui::theme::ToastLevel::Error,
                    toast::NotificationKind::Outcome,
                    "Could not check the save destination.",
                    Some(error.to_string()),
                    Some(document),
                    toast::NotificationLifetime::Persistent,
                    Instant::now(),
                );
                if save_all {
                    self.lifecycle.failed += 1;
                    (self.notify)();
                }
                false
            }
        }
    }

    pub(super) fn lifecycle_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        self.lifecycle_dispatch_inner(id)
    }
    fn lifecycle_dispatch_inner(&mut self, id: &str) -> bool {
        match id {
            "file.save_copy" => {
                self.request_document_save(self.app.active, SaveOperation::SaveCopy);
            }
            "file.save_all" => {
                self.start_save_all();
                self.lifecycle_pump_inner();
            }
            "file.cancel_save_all" => {
                self.lifecycle.queue.clear();
                self.lifecycle.pending = None;
                if self
                    .lifecycle
                    .preflight
                    .as_ref()
                    .is_some_and(|pending| pending.save_all)
                {
                    if let Some(pending) = &mut self.lifecycle.preflight {
                        pending.cancellation.cancel();
                        pending.cancelled = true;
                    }
                }
                self.lifecycle.running = false;
                if let Some(workspace) = &mut self.workspace {
                    workspace.message = Some("Save All cancelled; an already submitted save will finish.".into());
                }
            }
            "file.restore_closed" => {
                if let Some(workspace) = &mut self.workspace
                    && let Some(index) = workspace.restore_last_closed()
                {
                    self.app.active = index;
                    self.app.tabs = workspace.titles();
                }
            }
            "file.read_only" => {
                if let Some(editor) = self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active)) {
                    editor.set_read_only(!editor.read_only());
                }
            }
            "file.save_conflict_compare" => {
                if self.lifecycle.conflict_action.is_some() {
                    return true;
                }
                let conflict = self
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.selected_actionable_save_conflict(self.app.active).cloned());
                if let Some(conflict) = conflict
                    && let Some(other) = conflict.other_version
                {
                    let editor_request = self.lifecycle.allocate_conflict_open_request();
                    let other_request = self.lifecycle.allocate_conflict_open_request();
                    let workspace = self.workspace.as_mut().unwrap();
                    workspace.select_save_conflict(self.app.active, &conflict.transaction);
                    let editor_open =
                        self.lifecycle
                            .open_conflict(workspace, editor_request, conflict.editor_version.clone());
                    let other_open = self.lifecycle.open_conflict(workspace, other_request, other.clone());
                    match (editor_open, other_open) {
                        (Ok(()), Ok(())) => {
                            self.lifecycle.conflict_action = Some(PendingConflictAction::Compare {
                                transaction: conflict.transaction,
                                editor: conflict.editor_version,
                                other,
                                editor_request,
                                other_request,
                                editor_document: None,
                                other_document: None,
                            });
                            workspace.message = Some("Opening the preserved save versions for comparison…".into());
                        }
                        (left, right) => {
                            let mut requests = Vec::new();
                            if left.is_ok() {
                                requests.push(editor_request);
                            }
                            if right.is_ok() {
                                requests.push(other_request);
                            }
                            self.lifecycle.conflict_action = Some(PendingConflictAction::DrainFailed {
                                requests,
                                message: "The exact comparison could not be opened; recovery files remain retained."
                                    .into(),
                            });
                        }
                    }
                }
            }
            "file.save_conflict_save_elsewhere" => {
                if self.lifecycle.conflict_action.is_some() {
                    return true;
                }
                let conflict = self
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.selected_actionable_save_conflict(self.app.active).cloned());
                if let Some(conflict) = conflict {
                    let request = self.lifecycle.allocate_conflict_open_request();
                    let workspace = self.workspace.as_mut().unwrap();
                    workspace.select_save_conflict(self.app.active, &conflict.transaction);
                    match self
                        .lifecycle
                        .open_conflict(workspace, request, conflict.editor_version.clone())
                    {
                        Ok(()) => {
                            self.lifecycle.conflict_action = Some(PendingConflictAction::SaveElsewhere {
                                transaction: conflict.transaction,
                                editor: conflict.editor_version,
                                request,
                                document: None,
                            });
                            workspace.message = Some("Opening the preserved editor version for Save Elsewhere…".into());
                        }
                        Err(_) => self.toasts.enqueue_owned(
                            toast::Notification::new(
                                format!("data-safety:save-conflict-open:{}", conflict.transaction.display()),
                                toast::next_revision(),
                                bareline_ui::theme::ToastLevel::Error,
                                toast::NotificationKind::Outcome,
                                "The preserved editor version could not be queued.",
                                Some(format!(
                                    "Recovery transaction retained at {}",
                                    conflict.transaction.display()
                                )),
                                None,
                                toast::NotificationLifetime::Persistent,
                            ),
                            Instant::now(),
                        ),
                    }
                }
            }
            "file.save_conflict_retain_other" => {
                if let Some(workspace) = &mut self.workspace
                    && let Some(transaction) = workspace
                        .selected_save_conflict(self.app.active)
                        .map(|conflict| conflict.transaction.clone())
                {
                    workspace.retain_save_conflict(&transaction);
                    workspace.message =
                        Some("The other version and transaction files were retained for later recovery.".into());
                }
            }
            "file.save_conflict_next" => {
                if let Some(workspace) = &mut self.workspace
                    && let Some(conflict) = workspace.select_next_save_conflict(self.app.active)
                {
                    let transaction = conflict.transaction.display().to_string();
                    let target = conflict
                        .target
                        .as_ref()
                        .map_or_else(|| "unverified target".into(), |path| path.display().to_string());
                    let action =
                        if conflict.verified && conflict.state != bareline_platform::CommitState::CleanupPending {
                            "Compare and Save Elsewhere are available"
                        } else {
                            "content actions are unavailable; retain or inspect this transaction"
                        };
                    workspace.message = Some(format!(
                        "Selected save transaction {transaction} for {target}; {action}."
                    ));
                }
            }
            "file.retry_save_cleanup" => {
                if let Some(workspace) = &mut self.workspace
                    && let Some(transaction) = workspace
                        .selected_save_cleanup()
                        .map(|cleanup| cleanup.transaction.clone())
                {
                    workspace.retry_save_cleanup(&transaction);
                }
            }
            "file.retry_save_recovery" => {
                if let Some(workspace) = &mut self.workspace
                    && let Some(parent) = workspace.failed_save_recovery().map(PathBuf::from)
                {
                    workspace.retry_save_recovery(&parent);
                }
            }
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn lifecycle_pump(&mut self, _el: &ActiveEventLoop) {
        self.lifecycle_pump_inner();
    }
    fn lifecycle_pump_inner(&mut self) {
        if let Some(action) = self.lifecycle.conflict_action.take() {
            let resolve = |workspace: &Workspace, document: (u64, u64), path: &Path| {
                workspace.editors.iter().enumerate().find_map(|(index, editor)| {
                    (editor.document_identity() == document && workspace.path(index) == Some(path)).then_some(index)
                })
            };
            match action {
                PendingConflictAction::Compare {
                    transaction,
                    editor,
                    other,
                    editor_request,
                    other_request,
                    mut editor_document,
                    mut other_document,
                } => {
                    let Some(workspace) = &mut self.workspace else {
                        return;
                    };
                    let outcomes = workspace.take_tracked_open_outcomes(&[editor_request, other_request]);
                    #[cfg(test)]
                    let outcomes = {
                        let mut outcomes = outcomes;
                        let mut retained = Vec::new();
                        for outcome in self.lifecycle.conflict_open_outcomes.drain(..) {
                            let request_id = match &outcome {
                                bareline_app::workspace::LaunchOpenOutcome::Opened { request_id, .. }
                                | bareline_app::workspace::LaunchOpenOutcome::Failed { request_id, .. } => *request_id,
                            };
                            if request_id == editor_request || request_id == other_request {
                                outcomes.push(outcome);
                            } else {
                                retained.push(outcome);
                            }
                        }
                        self.lifecycle.conflict_open_outcomes = retained;
                        outcomes
                    };
                    let mut failed = None;
                    let mut pending_requests = Vec::new();
                    if editor_document.is_none() {
                        pending_requests.push(editor_request);
                    }
                    if other_document.is_none() {
                        pending_requests.push(other_request);
                    }
                    for outcome in outcomes {
                        match outcome {
                            bareline_app::workspace::LaunchOpenOutcome::Opened { request_id, document } => {
                                pending_requests.retain(|pending| *pending != request_id);
                                if request_id == editor_request {
                                    editor_document = Some(document);
                                } else if request_id == other_request {
                                    other_document = Some(document);
                                }
                            }
                            bareline_app::workspace::LaunchOpenOutcome::Failed { request_id, error } => {
                                pending_requests.retain(|pending| *pending != request_id);
                                failed = Some(error);
                            }
                        }
                    }
                    if let Some(error) = failed {
                        let message = format!(
                            "The exact save comparison could not open ({error}); recovery files remain retained."
                        );
                        if pending_requests.is_empty() {
                            self.toasts.enqueue_owned(
                                toast::Notification::new(
                                    format!("data-safety:save-compare:{}", transaction.display()),
                                    toast::next_revision(),
                                    bareline_ui::theme::ToastLevel::Error,
                                    toast::NotificationKind::Outcome,
                                    "The exact save comparison could not open.",
                                    Some(message),
                                    None,
                                    toast::NotificationLifetime::Persistent,
                                ),
                                Instant::now(),
                            );
                        } else {
                            self.lifecycle.conflict_action = Some(PendingConflictAction::DrainFailed {
                                requests: pending_requests,
                                message,
                            });
                        }
                    } else if let (Some(editor_document), Some(other_document)) = (editor_document, other_document) {
                        let pair = Some((
                            resolve(workspace, editor_document, &editor),
                            resolve(workspace, other_document, &other),
                        ));
                        if let Some((Some(left), Some(right))) = pair {
                            if !self.compare_start_pair(left, right) {
                                self.toasts.enqueue_owned(
                                    toast::Notification::new(
                                        format!("data-safety:save-compare:{}", transaction.display()),
                                        toast::next_revision(),
                                        bareline_ui::theme::ToastLevel::Error,
                                        toast::NotificationKind::Outcome,
                                        "The exact recovery documents closed before comparison.",
                                        Some(format!("Recovery files remain retained at {}", transaction.display())),
                                        None,
                                        toast::NotificationLifetime::Persistent,
                                    ),
                                    Instant::now(),
                                );
                            }
                        } else {
                            self.toasts.enqueue_owned(
                                toast::Notification::new(
                                    format!("data-safety:save-compare:{}", transaction.display()),
                                    toast::next_revision(),
                                    bareline_ui::theme::ToastLevel::Error,
                                    toast::NotificationKind::Outcome,
                                    "A tracked recovery document changed or closed before comparison.",
                                    Some(format!("Recovery files remain retained at {}", transaction.display())),
                                    None,
                                    toast::NotificationLifetime::Persistent,
                                ),
                                Instant::now(),
                            );
                        }
                    } else {
                        self.lifecycle.conflict_action = Some(PendingConflictAction::Compare {
                            transaction,
                            editor,
                            other,
                            editor_request,
                            other_request,
                            editor_document,
                            other_document,
                        });
                    }
                }
                PendingConflictAction::SaveElsewhere {
                    transaction,
                    editor,
                    request,
                    mut document,
                } => {
                    let Some(workspace) = &mut self.workspace else {
                        return;
                    };
                    let outcomes = workspace.take_tracked_open_outcomes(&[request]);
                    let mut failed = None;
                    for outcome in outcomes {
                        match outcome {
                            bareline_app::workspace::LaunchOpenOutcome::Opened { document: opened, .. } => {
                                document = Some(opened)
                            }
                            bareline_app::workspace::LaunchOpenOutcome::Failed { error, .. } => failed = Some(error),
                        }
                    }
                    if let Some(error) = failed {
                        self.toasts.enqueue_owned(
                            toast::Notification::new(
                                format!("data-safety:save-elsewhere:{}", transaction.display()),
                                toast::next_revision(),
                                bareline_ui::theme::ToastLevel::Error,
                                toast::NotificationKind::Outcome,
                                "The preserved editor version could not open.",
                                Some(format!(
                                    "{error}\nRecovery transaction retained at {}",
                                    transaction.display()
                                )),
                                None,
                                toast::NotificationLifetime::Persistent,
                            ),
                            Instant::now(),
                        );
                    } else if let Some(document) = document {
                        if let Some(index) = resolve(workspace, document, &editor) {
                            self.app.active = index;
                            self.request_document_save(index, SaveOperation::SaveAs);
                        } else {
                            self.toasts.enqueue_owned(
                                toast::Notification::new(
                                    format!("data-safety:save-elsewhere:{}", transaction.display()),
                                    toast::next_revision(),
                                    bareline_ui::theme::ToastLevel::Error,
                                    toast::NotificationKind::Outcome,
                                    "The tracked recovery document changed or closed before Save Elsewhere.",
                                    Some(format!(
                                        "Retained bytes were not reused. Recovery transaction: {}",
                                        transaction.display()
                                    )),
                                    None,
                                    toast::NotificationLifetime::Persistent,
                                ),
                                Instant::now(),
                            );
                        }
                    } else {
                        self.lifecycle.conflict_action = Some(PendingConflictAction::SaveElsewhere {
                            transaction,
                            editor,
                            request,
                            document,
                        });
                    }
                }
                PendingConflictAction::DrainFailed { mut requests, message } => {
                    if let Some(workspace) = &mut self.workspace {
                        for outcome in workspace.take_tracked_open_outcomes(&requests) {
                            let request_id = match outcome {
                                bareline_app::workspace::LaunchOpenOutcome::Opened { request_id, .. }
                                | bareline_app::workspace::LaunchOpenOutcome::Failed { request_id, .. } => request_id,
                            };
                            requests.retain(|request| *request != request_id);
                        }
                        if requests.is_empty() {
                            self.toasts.enqueue_owned(
                                toast::Notification::new(
                                    "data-safety:save-conflict-open-drain",
                                    toast::next_revision(),
                                    bareline_ui::theme::ToastLevel::Error,
                                    toast::NotificationKind::Outcome,
                                    "Save conflict versions could not be opened.",
                                    Some(message),
                                    None,
                                    toast::NotificationLifetime::Persistent,
                                ),
                                Instant::now(),
                            );
                        } else {
                            self.lifecycle.conflict_action =
                                Some(PendingConflictAction::DrainFailed { requests, message });
                        }
                    }
                }
            }
        }
        if let Some(pending) = &mut self.lifecycle.preflight
            && !pending.cancelled
            && !self
                .workspace
                .as_ref()
                .is_some_and(|workspace| workspace.editors.iter().any(|editor| pending.identity.matches(editor)))
        {
            pending.cancellation.cancel();
            pending.cancelled = true;
            let document = pending.identity.token();
            self.toasts.enqueue_owned(
                toast::Notification::new(
                    format!("data-safety:save-preflight-{}", document.0),
                    toast::next_revision(),
                    bareline_ui::theme::ToastLevel::Warning,
                    toast::NotificationKind::Outcome,
                    "Save destination expired because the document changed or closed.",
                    None,
                    Some(document),
                    toast::NotificationLifetime::Persistent,
                ),
                Instant::now(),
            );
        }
        if let Some(pending) = self.lifecycle.preflight.take() {
            match pending.receiver.try_recv() {
                Err(TryRecvError::Empty) => {
                    self.lifecycle.preflight = Some(pending);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    if pending.cancelled {
                        if pending.save_all && self.lifecycle.running {
                            self.lifecycle.skipped += 1;
                            (self.notify)();
                        }
                        return;
                    }
                    let document = pending.identity.token();
                    self.toasts.push_typed(
                        format!("save-preflight-{}", document.0),
                        toast::next_revision(),
                        bareline_ui::theme::ToastLevel::Error,
                        toast::NotificationKind::Outcome,
                        "Save destination check stopped.",
                        None,
                        Some(document),
                        toast::NotificationLifetime::Persistent,
                        Instant::now(),
                    );
                    if pending.save_all {
                        self.lifecycle.failed += 1;
                    }
                }
                Ok(Err(error)) => {
                    if pending.cancelled {
                        if pending.save_all && self.lifecycle.running {
                            self.lifecycle.skipped += 1;
                            (self.notify)();
                        }
                        return;
                    }
                    let document = pending.identity.token();
                    self.toasts.push_typed(
                        format!("save-preflight-{}", document.0),
                        toast::next_revision(),
                        bareline_ui::theme::ToastLevel::Error,
                        toast::NotificationKind::Outcome,
                        "Save destination rejected.",
                        Some(format!("{error:?}")),
                        Some(document),
                        toast::NotificationLifetime::Persistent,
                        Instant::now(),
                    );
                    if pending.save_all {
                        self.lifecycle.failed += 1;
                    }
                }
                Ok(Ok(preflight)) => {
                    if pending.cancelled {
                        if pending.save_all && self.lifecycle.running {
                            self.lifecycle.skipped += 1;
                            (self.notify)();
                        }
                        return;
                    }
                    let Some(index) = self.workspace.as_ref().and_then(|workspace| {
                        workspace
                            .editors
                            .iter()
                            .position(|editor| pending.identity.matches(editor))
                    }) else {
                        if pending.save_all {
                            self.lifecycle.skipped += 1;
                            (self.notify)();
                        }
                        return;
                    };
                    let confirmed = !preflight.requires_overwrite_consent()
                        || self
                            .platform
                            .as_ref()
                            .is_some_and(|platform| platform.confirm_overwrite(preflight.path()));
                    let Some(destination) = preflight.approve(confirmed) else {
                        if pending.save_all {
                            self.lifecycle.skipped += 1;
                        }
                        if let Some(workspace) = &mut self.workspace {
                            workspace.message = Some("Save cancelled; destination was not changed.".into());
                        }
                        if pending.save_all {
                            (self.notify)();
                        }
                        return;
                    };
                    if destination.operation != pending.operation {
                        if let Some(workspace) = &mut self.workspace {
                            workspace.message = Some("Save destination expired because the operation changed.".into());
                        }
                        if pending.save_all {
                            self.lifecycle.failed += 1;
                            (self.notify)();
                        }
                        return;
                    }
                    self.lifecycle.last_save_directory = destination.path.parent().map(PathBuf::from);
                    let accepted = self
                        .workspace
                        .as_mut()
                        .is_some_and(|workspace| workspace.save_prepared(index, destination));
                    if pending.save_all {
                        if self
                            .lifecycle
                            .record_save_all_submission(pending.identity.clone(), accepted)
                        {
                            return;
                        }
                    } else {
                        return;
                    }
                }
            }
        }
        if !self.lifecycle.running {
            return;
        }
        if self.workspace.is_none() {
            self.lifecycle.running = false;
            return;
        }
        loop {
            let step = advance_existing_save_all(&mut self.lifecycle, self.workspace.as_mut().unwrap());
            match step {
                SaveAllStep::Waiting => return,
                SaveAllStep::Complete => break,
                SaveAllStep::NeedsDestination { identity, index } => {
                    let (name, directory) = self.save_dialog_defaults(index);
                    let path = match self
                        .platform
                        .as_ref()
                        .map(|p| p.save_document_file_at(&name, directory.as_deref()))
                    {
                        Some(Ok(Some(path))) => path,
                        Some(Ok(None)) => {
                            self.lifecycle.skipped += 1;
                            continue;
                        }
                        Some(Err(error)) => {
                            self.lifecycle.failed += 1;
                            self.toasts.push_typed(
                                "save-all-dialog",
                                toast::next_revision(),
                                bareline_ui::theme::ToastLevel::Error,
                                toast::NotificationKind::Outcome,
                                "Save All could not choose a destination.",
                                Some(error),
                                None,
                                toast::NotificationLifetime::Persistent,
                                Instant::now(),
                            );
                            continue;
                        }
                        None => {
                            self.lifecycle.failed += 1;
                            continue;
                        }
                    };
                    if !self.workspace.as_ref().is_some_and(|workspace| {
                        workspace
                            .editors
                            .get(index)
                            .is_some_and(|editor| identity.matches(editor))
                    }) {
                        self.lifecycle.skipped += 1;
                        continue;
                    }
                    if self.begin_destination_preflight(index, path, SaveOperation::SaveAs, true) {
                        return;
                    }
                    // A spawn failure was counted by begin_destination_preflight.
                }
            }
        }
        self.lifecycle.running = false;
        let level = if self.lifecycle.failed == 0 {
            bareline_ui::theme::ToastLevel::Info
        } else {
            bareline_ui::theme::ToastLevel::Error
        };
        let lifetime = if self.lifecycle.failed == 0 {
            toast::NotificationLifetime::Transient
        } else {
            toast::NotificationLifetime::Persistent
        };
        self.toasts.push_typed(
            "save-all-outcome",
            toast::next_revision(),
            level,
            toast::NotificationKind::Outcome,
            format!(
                "Save All: {} saved; {} cancelled or closed; {} failed or changed during save.",
                self.lifecycle.saved, self.lifecycle.skipped, self.lifecycle.failed
            ),
            None,
            None,
            lifetime,
            Instant::now(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Identity, LifecycleRuntime, PendingConflictAction, SaveAllStep, advance_existing_save_all,
        save_all_command_state,
    };
    use bareline_app::workspace::{Input, Workspace};
    use bareline_file_io::lifecycle::SaveConflict;
    use std::sync::Arc;

    fn recovery_shell(target: &std::path::Path, conflict: SaveConflict) -> crate::windows_app::Shell {
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        let mut workspace =
            Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.open(target.to_path_buf());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        workspace.editors[0].enqueue(Input::Insert(" dirty".into()));
        while workspace.editors[0].busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        workspace.track_save_conflict(conflict);
        shell.workspace = Some(workspace);
        shell
    }

    fn settle_conflict_action(shell: &mut crate::windows_app::Shell) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while shell.lifecycle.conflict_action.is_some() {
            shell.workspace.as_mut().unwrap().pump();
            shell.lifecycle_pump_inner();
            assert!(
                std::time::Instant::now() < deadline,
                "{:?}",
                shell.workspace.as_ref().unwrap().message
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn save_all_menu_state_covers_idle_ready_and_running() {
        assert!(!save_all_command_state("file.save_all", false, 0).enabled);
        assert!(save_all_command_state("file.save_all", false, 2).enabled);
        assert!(!save_all_command_state("file.save_all", true, 2).enabled);
        assert!(save_all_command_state("file.cancel_save_all", true, 2).enabled);
        assert!(!save_all_command_state("file.cancel_save_all", false, 2).enabled);
    }

    #[test]
    fn tracked_conflict_compare_starts_the_exact_open_pair() {
        let root = std::env::temp_dir().join(format!(
            "bareline-conflict-compare-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let editor = root.join("editor-version");
        let other = root.join("target.txt");
        std::fs::write(&editor, b"editor bytes").unwrap();
        std::fs::write(&other, b"other bytes").unwrap();
        let mut shell = recovery_shell(
            &other,
            SaveConflict {
                target: Some(other.clone()),
                editor_version: editor,
                other_version: Some(other.clone()),
                transaction: root.join("state"),
                state: bareline_platform::CommitState::Conflict,
                verified: true,
            },
        );
        assert!(shell.lifecycle_dispatch_inner("file.save_conflict_compare"));
        settle_conflict_action(&mut shell);
        assert!(shell.compare_is_active());
        drop(shell);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compare_peer_failure_after_a_separate_success_pump_drains_exact_requests() {
        let root = std::env::temp_dir().join(format!(
            "bareline-conflict-drain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let editor = root.join("editor-version");
        let other = root.join("target.txt");
        std::fs::write(&editor, b"editor bytes").unwrap();
        std::fs::write(&other, b"other bytes").unwrap();
        let mut shell = recovery_shell(
            &other,
            SaveConflict {
                target: Some(other.clone()),
                editor_version: editor,
                other_version: Some(other.clone()),
                transaction: root.join("state"),
                state: bareline_platform::CommitState::Conflict,
                verified: true,
            },
        );
        shell.lifecycle.conflict_open = Some(Arc::new(|_, _, _| Ok(())));
        assert!(shell.lifecycle_dispatch_inner("file.save_conflict_compare"));
        let (editor_request, other_request) = match shell.lifecycle.conflict_action.as_ref().unwrap() {
            PendingConflictAction::Compare {
                editor_request,
                other_request,
                ..
            } => (*editor_request, *other_request),
            _ => unreachable!(),
        };
        shell
            .lifecycle
            .conflict_open_outcomes
            .push(bareline_app::workspace::LaunchOpenOutcome::Opened {
                request_id: editor_request,
                document: (7, 9),
            });
        shell.lifecycle_pump_inner();
        assert!(matches!(
            shell.lifecycle.conflict_action.as_ref(),
            Some(PendingConflictAction::Compare {
                editor_document: Some((7, 9)),
                other_document: None,
                ..
            })
        ));
        shell
            .lifecycle
            .conflict_open_outcomes
            .push(bareline_app::workspace::LaunchOpenOutcome::Failed {
                request_id: other_request,
                error: "injected peer failure".into(),
            });
        shell.lifecycle_pump_inner();
        assert!(shell.lifecycle.conflict_action.is_none());
        assert!(shell.lifecycle.conflict_open_outcomes.is_empty());
        shell.toasts.draw(
            &mut bareline_renderer_recording::RecordingBackend::default(),
            800.0,
            600.0,
            Default::default(),
            &mut Vec::new(),
        );
        let notices = shell.toasts.accessibility();
        let failure = notices
            .iter()
            .find(|notice| {
                notice
                    .details
                    .as_deref()
                    .is_some_and(|details| details.contains("injected peer failure"))
            })
            .expect("the consumed peer failure must remain available as a typed notification");
        assert_eq!(failure.level, bareline_ui::theme::ToastLevel::Error);
        assert_eq!(failure.kind, crate::windows_app::toast::NotificationKind::Outcome);
        assert!(
            failure
                .details
                .as_deref()
                .is_some_and(|details| details.contains("recovery files remain retained"))
        );
        drop(shell);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tracked_save_elsewhere_reaches_picker_and_preflight() {
        let root = std::env::temp_dir().join(format!(
            "bareline-conflict-save-elsewhere-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let editor = root.join("editor-version");
        let target = root.join("target.txt");
        let destination = root.join("saved-elsewhere.txt");
        std::fs::write(&editor, b"editor bytes").unwrap();
        std::fs::write(&target, b"other bytes").unwrap();
        let mut shell = recovery_shell(
            &target,
            SaveConflict {
                target: Some(target.clone()),
                editor_version: editor,
                other_version: Some(target.clone()),
                transaction: root.join("state"),
                state: bareline_platform::CommitState::Conflict,
                verified: true,
            },
        );
        let picked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let picked_for_callback = picked.clone();
        let destination_for_callback = destination.clone();
        shell.lifecycle.save_destination_picker = Some(Arc::new(move |_, _| {
            picked_for_callback.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(destination_for_callback.clone())
        }));
        assert!(shell.lifecycle_dispatch_inner("file.save_conflict_save_elsewhere"));
        settle_conflict_action(&mut shell);
        assert_eq!(picked.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(shell.lifecycle.preflight.is_some());
        drop(shell);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejected_save_all_submission_does_not_wait_before_next_entry() {
        let root = std::env::temp_dir().join(format!(
            "bareline-save-all-rejection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        std::fs::write(&first, "first").unwrap();
        std::fs::write(&second, "second").unwrap();
        let mut workspace =
            Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.open(first);
        workspace.open(second);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        for editor in &mut workspace.editors {
            editor.enqueue(Input::Insert("X".into()));
        }
        while workspace.editors.iter().any(|editor| editor.busy()) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        workspace.editors[0].set_read_only(true);
        let mut runtime = LifecycleRuntime {
            running: true,
            queue: workspace.editors.iter().map(Identity::capture).collect(),
            ..Default::default()
        };
        assert!(matches!(
            advance_existing_save_all(&mut runtime, &mut workspace),
            SaveAllStep::Waiting
        ));
        assert_eq!(runtime.failed, 1);
        assert!(runtime.pending.is_some());
        assert!(runtime.queue.is_empty());
        while workspace.io_busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        drop(workspace);
        std::fs::remove_dir_all(root).unwrap();
    }
}
