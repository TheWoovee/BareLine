// SPDX-License-Identifier: MPL-2.0
//! Two-phase mixed workspace replacement. Preview owns exact reviewed ranges only.
use super::*;
use bareline_document::{DocumentSnapshot, Edit, EditTransaction, paged::PagedSnapshot};
use bareline_editor_surface::group_view::SurfaceGroup;
use bareline_renderer::{DrawOp, Rect};
use bareline_search::{
    Completeness, MAX_RESULT_BYTES, ReplaceScope, SearchJob,
    replace_disk::{
        BackupRetention, DiskApplySummary, DiskReplaceOptions, DiskReplacePreview, ReceiptJob, ReceiptScan,
        ReceiptState, ReplaceReceipt, apply_disk_files_with_paging, preview_disk_files_with_paging_options,
        remove_receipt_job, rollback_receipt_with_paging, scan_receipts,
    },
    replace_files::{OpenReplacePreview, preview_open_documents_options},
    service::{BackgroundTicket, SearchWorker},
};
use bareline_ui::{ACCENT, BORDER, CHROME, MUTED, TEXT, rect, text};
use std::{collections::VecDeque, path::Path, sync::mpsc::TryRecvError};
struct PagedChange {
    edit: Edit,
    before: String,
    after: String,
    included: bool,
}
struct PagedPreview {
    source: PagedSnapshot,
    label: String,
    included: bool,
    changes: Vec<PagedChange>,
}
struct Preview {
    open: Option<OpenReplacePreview>,
    labels: Vec<String>,
    paged: Vec<PagedPreview>,
    disk: DiskReplacePreview,
    skipped: usize,
    skip_reasons: String,
    open_skips: Vec<String>,
}
#[derive(Clone, Copy)]
enum Location {
    Open(usize, Option<usize>),
    Paged(usize, Option<usize>),
    Disk(usize, Option<usize>),
}
impl Preview {
    fn location(&self, mut row: usize) -> Option<Location> {
        if let Some(open) = &self.open {
            for (index, file) in open.documents().iter().enumerate() {
                if row <= file.changes.len() {
                    return Some(Location::Open(index, row.checked_sub(1)));
                }
                row -= file.changes.len() + 1;
            }
        }
        for (index, file) in self.paged.iter().enumerate() {
            if row <= file.changes.len() {
                return Some(Location::Paged(index, row.checked_sub(1)));
            }
            row -= file.changes.len() + 1;
        }
        for (index, file) in self.disk.files().iter().enumerate() {
            if row <= file.changes.len() {
                return Some(Location::Disk(index, row.checked_sub(1)));
            }
            row -= file.changes.len() + 1;
        }
        None
    }
    fn count_rows(&self) -> usize {
        self.open.as_ref().map_or(0, |open| {
            open.documents()
                .iter()
                .map(|file| file.changes.len() + 1)
                .sum::<usize>()
        }) + self.paged.iter().map(|file| file.changes.len() + 1).sum::<usize>()
            + self
                .disk
                .files()
                .iter()
                .map(|file| file.changes.len() + 1)
                .sum::<usize>()
    }
    fn selected(&self) -> usize {
        self.open.as_ref().map_or(0, OpenReplacePreview::selected_matches)
            + self
                .paged
                .iter()
                .filter(|file| file.included)
                .map(|file| file.changes.iter().filter(|change| change.included).count())
                .sum::<usize>()
            + self
                .disk
                .files()
                .iter()
                .filter(|file| file.included)
                .map(|file| file.changes.iter().filter(|change| change.included).count())
                .sum::<usize>()
    }
    fn selected_sources_current(&self, workspace: &bareline_app::workspace::Workspace) -> bool {
        let resident_current = self.open.as_ref().is_none_or(|open| {
            open.documents()
                .iter()
                .filter(|document| document.included && document.changes.iter().any(|change| change.included))
                .all(|document| {
                    workspace.editors.iter().any(|editor| {
                        matches!(editor, WorkspaceEditor::Resident(surface)
                            if !surface.read_only()
                                && !surface.busy()
                                && surface.snapshot().same_document(&document.snapshot)
                                && surface.snapshot().revision == document.snapshot.revision
                                && surface.snapshot().content_state == document.snapshot.content_state)
                    })
                })
        });
        let paged_current = self
            .paged
            .iter()
            .filter(|document| document.included && document.changes.iter().any(|change| change.included))
            .all(|document| {
                workspace.editors.iter().any(|editor| {
                    matches!(editor, WorkspaceEditor::Paged(surface)
                        if !surface.user_read_only()
                            && !surface.busy()
                            && surface.snapshot().same_document(&document.source)
                            && surface.snapshot().revision == document.source.revision
                            && surface.snapshot().content_state == document.source.content_state)
                })
            });
        resident_current && paged_current
    }
    fn toggle_all(&mut self, included: bool) {
        if let Some(open) = &mut self.open {
            open.toggle_all(included);
        }
        self.disk.toggle_all(included);
        for file in &mut self.paged {
            file.included = included;
            for change in &mut file.changes {
                change.included = included;
            }
        }
    }
    fn toggle(&mut self, row: usize) {
        match self.location(row) {
            Some(Location::Open(file, matched)) => {
                let open = self.open.as_mut().unwrap();
                if let Some(matched) = matched {
                    let included = !open.documents()[file].changes[matched].included;
                    open.set_match_included(file, matched, included);
                } else {
                    let included = !open.documents()[file].included;
                    open.set_document_included(file, included);
                }
            }
            Some(Location::Disk(file, matched)) => {
                if let Some(matched) = matched {
                    let included = !self.disk.files()[file].changes[matched].included;
                    self.disk.set_match_included(file, matched, included);
                } else {
                    let included = !self.disk.files()[file].included;
                    self.disk.set_file_included(file, included);
                }
            }
            Some(Location::Paged(file, matched)) => {
                if let Some(matched) = matched {
                    self.paged[file].changes[matched].included = !self.paged[file].changes[matched].included;
                } else {
                    self.paged[file].included = !self.paged[file].included;
                }
            }
            None => {}
        }
    }
    fn label(&self, row: usize) -> Option<String> {
        let (included, value) = match self.location(row)? {
            Location::Open(file, matched) => {
                let item = &self.open.as_ref()?.documents()[file];
                if let Some(matched) = matched {
                    let change = &item.changes[matched];
                    (
                        item.included && change.included,
                        format!("{}: {} → {}", change.range.start.0, change.before, change.after),
                    )
                } else {
                    (
                        item.included,
                        format!(
                            "Open document: {}",
                            self.labels.get(file).map(String::as_str).unwrap_or("Document")
                        ),
                    )
                }
            }
            Location::Paged(file, matched) => {
                let item = &self.paged[file];
                if let Some(matched) = matched {
                    let change = &item.changes[matched];
                    (
                        item.included && change.included,
                        format!("{}: {} → {}", change.edit.range.start.0, change.before, change.after),
                    )
                } else {
                    (item.included, format!("Paged document: {}", item.label))
                }
            }
            Location::Disk(file, matched) => {
                let item = &self.disk.files()[file];
                if let Some(matched) = matched {
                    let change = &item.changes[matched];
                    (
                        item.included && change.included,
                        format!("{}: {} → {}", change.range.start.0, change.before, change.after),
                    )
                } else {
                    (
                        item.included,
                        format!(
                            "{} [{}, BOM {}, EOL {}]",
                            item.path.display(),
                            bareline_app::encoding::label(item.encoding),
                            item.bom,
                            item.eol.label()
                        ),
                    )
                }
            }
        };
        Some(format!(
            "[{}] {}",
            if included { "✓" } else { " " },
            value
                .replace(['\r', '\n', '\t'], " ")
                .chars()
                .take(200)
                .collect::<String>()
        ))
    }
}
enum PendingOpen {
    Group(SurfaceGroup, Vec<DocumentSnapshot>, usize),
    Single(bareline_editor_surface::TrackedEditReceipt, usize),
}
struct PagedApply {
    source: PagedSnapshot,
    transaction: EditTransaction,
    matches: usize,
}
/// Replacement jobs under the recovery folder after any deletion or retention.
struct BackupListing {
    scan: ReceiptScan,
    retired: usize,
    released: u64,
}
/// One shown backup-listing row, aligned with `report`.
struct ListedJob {
    receipt: PathBuf,
    restorable: bool,
    /// Records still await reconciliation by hash; deleting would lose that answer.
    reconciling: bool,
    /// Records a retried Rollback might still restore; deleting discards them.
    retry: usize,
}
const NO_DURABLE_ROOT: &str =
    "Replace in Files needs the durable recovery folder for receipts and backups; they are never kept in %TEMP%.";
const EXIT_WAIT_ID: i32 = 1301;
const EXIT_CANCEL_ID: i32 = 1302;
const DELETE_BACKUP_ID: i32 = 1303;
/// The task dialog's always-present Cancel button (`IDCANCEL`).
const DIALOG_CANCEL_ID: i32 = 2;
/// Whether `root` lies under `%TEMP%`. Both paths are resolved to their final form
/// (junctions, 8.3 names) as far as they exist and compared case-insensitively.
fn under_temp(root: &Path) -> bool {
    same_or_under(root, &std::env::temp_dir())
}
fn same_or_under(path: &Path, base: &Path) -> bool {
    comparable_path(path).starts_with(comparable_path(base))
}
/// `path` resolved through its deepest existing ancestor, then case-folded.
fn comparable_path(path: &Path) -> PathBuf {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut existing = path;
    let resolved = loop {
        if let Ok(resolved) = std::fs::canonicalize(existing) {
            break rest
                .iter()
                .rev()
                .fold(resolved, |resolved: PathBuf, name| resolved.join(name));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => break path.to_path_buf(),
        }
    };
    PathBuf::from(resolved.to_string_lossy().to_lowercase())
}
/// Another running process still owns (and may be writing) this job directory.
fn replace_job_owner_running(owner: u32, created_unix_nanos: u128) -> bool {
    owner != std::process::id()
        && crate::shell::recovery::process_started(owner).is_some_and(|start| {
            start <= created_unix_nanos.saturating_add(crate::shell::recovery::OWNER_START_SLACK_NANOS)
        })
}
fn backup_job_label(job: &ReceiptJob, now_unix_nanos: u128) -> String {
    let count = |state: fn(&ReceiptState) -> bool| job.receipt.files.iter().filter(|file| state(&file.state)).count();
    let days = now_unix_nanos.saturating_sub(job.created_unix_nanos) / (24 * 60 * 60 * 1_000_000_000);
    let first = job
        .receipt
        .files
        .first()
        .and_then(|file| file.path.to_native().ok())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "no files".into());
    format!(
        "{}{} · {} files, {} restorable, {} restored, {} retry, {} conflicts · first: {first}",
        if job.interrupted { "Interrupted replace · " } else { "" },
        match days {
            0 => "today".to_owned(),
            1 => "1 day old".to_owned(),
            days => format!("{days} days old"),
        },
        job.receipt.files.len(),
        job.restorable(),
        count(|state| *state == ReceiptState::RolledBack),
        count(|state| matches!(state, ReceiptState::RollbackFailed(_))),
        count(|state| *state == ReceiptState::Conflict),
    )
}
fn unix_nanos_now() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}
#[derive(Clone)]
enum Hit {
    Row(usize),
    Command(&'static str),
}

#[derive(Clone, Copy)]
struct ReplacementMenuFacts {
    open: bool,
    busy: bool,
    selected: Option<usize>,
    current: bool,
    has_receipt: bool,
}

fn replacement_command_state(id: &str, facts: ReplacementMenuFacts) -> bareline_commands::CommandState {
    use bareline_commands::CommandState;
    match id {
        "search.replacePreview.apply" if facts.busy => CommandState::disabled("Wait for replacement work to stop"),
        "search.replacePreview.apply" if facts.selected.is_none() => {
            CommandState::disabled("Create and review a replacement preview first")
        }
        "search.replacePreview.apply" if facts.selected == Some(0) => {
            CommandState::disabled("Select at least one reviewed replacement")
        }
        "search.replacePreview.apply" if !facts.current => {
            CommandState::disabled("Reviewed replacements are stale; refresh the preview")
        }
        "search.replacePreview.apply" => CommandState::default(),
        "search.replacePreview.toggleAll" if !facts.busy && facts.selected.is_some() => CommandState::default(),
        "search.replacePreview.toggleAll" => CommandState::disabled("Create and review a replacement preview first"),
        "search.replacePreview.refresh" if facts.open && !facts.busy => CommandState::default(),
        "search.replacePreview.refresh" => CommandState::disabled(if facts.open {
            "Wait for replacement work to stop"
        } else {
            "Open Replacement Preview first"
        }),
        "search.replacePreview.cancel" if facts.busy => CommandState::default(),
        "search.replacePreview.cancel" => CommandState::disabled("No workspace replacement is running"),
        "search.replacePreview.rollback" if !facts.busy && facts.has_receipt => CommandState::default(),
        "search.replacePreview.rollback" => CommandState::disabled("No completed replacement backup is available"),
        "search.replacePreview.close" if facts.open && !facts.busy => CommandState::default(),
        "search.replacePreview.close" => CommandState::disabled(if facts.open {
            "Cancel or finish replacement work before closing"
        } else {
            "Replacement Preview is closed"
        }),
        _ => CommandState::default(),
    }
}
#[derive(Default)]
pub(super) struct ReplaceRuntime {
    open: bool,
    worker: Option<SearchWorker>,
    preparing: Option<BackgroundTicket<Preview>>,
    applying: Option<BackgroundTicket<DiskApplySummary>>,
    rolling_back: Option<BackgroundTicket<ReplaceReceipt>>,
    preview: Option<Preview>,
    pending_open: Option<PendingOpen>,
    paged_queue: VecDeque<PagedApply>,
    paged_pending: Option<(bareline_editor_surface::TrackedEditReceipt, usize)>,
    staging_paged: Option<
        BackgroundTicket<(
            PagedSnapshot,
            bareline_document::paged::PreparedSourceTransaction,
            usize,
        )>,
    >,
    disk_queue: Option<DiskReplacePreview>,
    receipt: Option<PathBuf>,
    status: String,
    changed_open: usize,
    changed_disk: usize,
    replaced_disk: usize,
    replaced_open: usize,
    failed_open: usize,
    staging_token: Option<(u64, u64)>,
    open_outcomes: Vec<String>,
    report: Vec<String>,
    open_labels: Vec<((u64, u64), String)>,
    skipped_preview: usize,
    skip_reasons: String,
    row: usize,
    top: usize,
    hits: Vec<(Rect, Hit)>,
    cancel_requested: bool,
    replacement_options: bareline_search::ReplacementOptions,
    disable_backup: bool,
    workspace_scope: bool,
    /// Startup reconciliation, Manage Replace Backups, or a backup deletion.
    listing: Option<BackgroundTicket<BackupListing>>,
    listing_startup: bool,
    /// Status that the pending listing keeps ahead of its own summary.
    listing_note: Option<String>,
    startup_listed: bool,
    /// Receipt jobs aligned with `report` rows while a backup listing is shown.
    listed: Vec<ListedJob>,
    /// Exit request (trace ticket) already asked about a running disk replacement.
    exit_prompted: Option<u64>,
}
pub(super) fn register(registry: &mut CommandRegistry) {
    for (id, title) in [
        ("search.replacePreview.preserveCase", "Toggle Preserve Replacement Case"),
        (
            "search.replacePreview.includeBinary",
            "Toggle Binary Replacement Inclusion",
        ),
        (
            "search.replacePreview.backups",
            "Toggle Backups for This Replacement Job",
        ),
        ("search.replacePreview.refresh", "Refresh Replacement Preview"),
        ("search.replaceInFiles", "Replace in Files…"),
        ("search.replaceInWorkspace", "Replace in Workspace…"),
        ("search.replacePreview.apply", "Apply Reviewed Replacements"),
        ("search.replacePreview.toggleAll", "Toggle All Preview Changes"),
        ("search.replacePreview.cancel", "Cancel Workspace Replacement"),
        ("search.replacePreview.close", "Close Replacement Preview"),
        ("search.replacePreview.rollback", "Restore Last Replacement Backups"),
        ("search.replaceBackups.manage", "Manage Replace Backups…"),
        ("search.replaceBackups.delete", "Delete Selected Replace Backup"),
        ("search.replaceBackups.prune", "Delete Old Replace Backups"),
    ] {
        let id = CommandId(id);
        let registered = registry.register(CommandSpec {
            id,
            title,
            category: "Search",
            shortcut: "",
            action: Action::Contributed(id),
        });
        debug_assert!(registered.is_ok(), "duplicate command ID {id:?}");
        let _ = registry.set_presentation(
            id,
            CommandPresentation {
                menu_path: "Search".into(),
                accessible_name: Some(title.into()),
                ..Default::default()
            },
        );
    }
}
impl ReplaceRuntime {
    fn record_open(&mut self, token: (u64, u64), reason: &str) {
        let label = self
            .open_labels
            .iter()
            .find(|(identity, _)| *identity == token)
            .map(|(_, label)| label.as_str())
            .unwrap_or("Open document");
        self.open_outcomes.push(format!(
            "{label} [document {}, revision {}]: {reason}",
            token.0, token.1
        ));
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }
    fn busy(&self) -> bool {
        self.preparing.is_some()
            || self.applying.is_some()
            || self.rolling_back.is_some()
            || self.pending_open.is_some()
            || self.paged_pending.is_some()
            || self.staging_paged.is_some()
            || !self.paged_queue.is_empty()
            || self.disk_queue.is_some()
            || self.listing.is_some()
    }
    /// A job that is changing files on disk; exit waits for it (APP-10).
    fn disk_busy(&self) -> bool {
        self.disk_queue.is_some() || self.applying.is_some() || self.rolling_back.is_some()
    }
    fn selected_listed(&self) -> Option<&ListedJob> {
        if self.preview.is_some() {
            return None;
        }
        self.listed.get(self.row)
    }
    /// The selected listed job while backups are shown, else this session's last job.
    fn rollback_target(&self) -> Option<PathBuf> {
        if self.preview.is_none() && !self.listed.is_empty() {
            return self
                .selected_listed()
                .filter(|job| job.restorable)
                .map(|job| job.receipt.clone());
        }
        self.receipt.clone()
    }
    /// Shows receipt jobs as report rows. Startup shows them only when a job was
    /// interrupted; its retention then runs silently. `note` (a finished rollback's
    /// outcome) leads the status so the re-listing does not hide it.
    fn show_backups(&mut self, listing: BackupListing, startup: bool, note: Option<String>, now_unix_nanos: u128) {
        let interrupted = listing.scan.jobs.iter().filter(|job| job.interrupted).count();
        if startup && interrupted == 0 {
            return;
        }
        self.preview = None;
        self.report.clear();
        self.listed.clear();
        for job in &listing.scan.jobs {
            self.report.push(backup_job_label(job, now_unix_nanos));
            self.listed.push(ListedJob {
                receipt: job.receipt_path.clone(),
                restorable: job.restorable() > 0,
                reconciling: job.receipt.files.iter().any(|file| file.state.unresolved()),
                retry: job
                    .receipt
                    .files
                    .iter()
                    .filter(|file| matches!(file.state, ReceiptState::RollbackFailed(_)))
                    .count(),
            });
        }
        for (receipt, error) in &listing.scan.unreadable {
            self.report
                .push(format!("Unreadable receipt {}: {error}", receipt.display()));
            self.listed.push(ListedJob {
                receipt: receipt.clone(),
                restorable: false,
                reconciling: false,
                retry: 0,
            });
        }
        self.open = true;
        self.row = listing.scan.jobs.iter().position(|job| job.interrupted).unwrap_or(0);
        self.top = self.row;
        self.status = if interrupted > 0 {
            format!(
                "Interrupted replace: {interrupted} job(s) stopped before finishing and were reconciled by hash. Select one and choose Rollback to restore its backups."
            )
        } else {
            format!(
                "{} replace backup jobs; {} can be rolled back. Deleted {} old jobs ({} KiB).",
                listing.scan.jobs.len(),
                listing.scan.jobs.iter().filter(|job| job.restorable() > 0).count(),
                listing.retired,
                listing.released.div_ceil(1024)
            )
        };
        if let Some(note) = note {
            self.status = format!("{note}. {}", self.status);
        }
    }

    fn command_state(
        &self,
        id: &str,
        workspace: Option<&bareline_app::workspace::Workspace>,
    ) -> bareline_commands::CommandState {
        let facts = ReplacementMenuFacts {
            open: self.open,
            busy: self.busy(),
            selected: self.preview.as_ref().map(Preview::selected),
            current: self
                .preview
                .as_ref()
                .zip(workspace)
                .is_some_and(|(preview, workspace)| preview.selected_sources_current(workspace)),
            has_receipt: self.rollback_target().is_some(),
        };
        replacement_command_state(id, facts)
    }

    pub(super) fn annotate_context(
        &self,
        context: &mut bareline_commands::CommandContext,
        workspace: Option<&bareline_app::workspace::Workspace>,
    ) {
        use bareline_commands::{CommandId, CommandState};
        let available = workspace.is_some();
        for id in ["search.replaceInFiles", "search.replaceInWorkspace"] {
            if !available {
                context
                    .states
                    .insert(CommandId(id), CommandState::disabled("Open a document first"));
            }
        }
        for (id, checked) in [
            (
                "search.replacePreview.preserveCase",
                self.replacement_options.preserve_case,
            ),
            (
                "search.replacePreview.includeBinary",
                self.replacement_options.include_binary,
            ),
            ("search.replacePreview.backups", !self.disable_backup),
        ] {
            context.states.insert(
                CommandId(id),
                CommandState {
                    enabled: self.open && !self.busy(),
                    checked,
                    disabled_reason: (!self.open || self.busy()).then(|| {
                        if self.open {
                            "Wait for replacement work to stop".into()
                        } else {
                            "Open Replacement Preview first".into()
                        }
                    }),
                    ..Default::default()
                },
            );
        }

        for id in [
            "search.replacePreview.apply",
            "search.replacePreview.toggleAll",
            "search.replacePreview.refresh",
            "search.replacePreview.cancel",
            "search.replacePreview.rollback",
            "search.replacePreview.close",
        ] {
            context.states.insert(CommandId(id), self.command_state(id, workspace));
        }
        let busy = self.busy().then_some("Wait for replacement work to stop");
        for (id, reason) in [
            ("search.replaceBackups.manage", busy),
            ("search.replaceBackups.prune", busy),
            (
                "search.replaceBackups.delete",
                busy.or_else(|| {
                    self.selected_listed()
                        .is_none()
                        .then_some("Select a job in Manage Replace Backups first")
                }),
            ),
        ] {
            let state = match reason {
                Some(reason) => CommandState::disabled(reason),
                None => CommandState::default(),
            };
            context.states.insert(CommandId(id), state);
        }
    }
    pub(super) fn draw(
        &mut self,
        workspace: Option<&bareline_app::workspace::Workspace>,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) {
        if !self.open {
            return;
        }
        self.hits.clear();
        let bounds = rect(8.0, (height - 340.0).max(0.0), (width - 16.0).max(100.0), 320.0);
        ops.push(DrawOp::Fill(bounds, CHROME));
        ops.push(DrawOp::Stroke(bounds, BORDER, 1.0));
        text(
            ops,
            24.0,
            bounds.y + 12.0,
            if self.preview.is_none() && !self.listed.is_empty() {
                "Replace backups · Select a job, then Rollback or Delete Selected Replace Backup"
            } else {
                "Replace preview · Review included files and matches"
            },
            15.0,
            TEXT,
        );
        let mut option_x = 24.0;
        for (label, id) in [
            (
                format!(
                    "[{}] Preserve case",
                    if self.replacement_options.preserve_case {
                        "x"
                    } else {
                        " "
                    }
                ),
                "search.replacePreview.preserveCase",
            ),
            (
                format!(
                    "[{}] Include binary",
                    if self.replacement_options.include_binary {
                        "x"
                    } else {
                        " "
                    }
                ),
                "search.replacePreview.includeBinary",
            ),
            (
                format!(
                    "[{}] Backups in receipt folder",
                    if self.disable_backup { " " } else { "x" }
                ),
                "search.replacePreview.backups",
            ),
        ] {
            text(ops, option_x, bounds.y + 35.0, label, 12.0, TEXT);
            if !self.busy() {
                self.hits
                    .push((rect(option_x, bounds.y + 30.0, 180.0, 24.0), Hit::Command(id)));
            }
            option_x += 190.0;
        }
        let rows = self.preview.as_ref().map_or(self.report.len(), Preview::count_rows);
        self.top = self.top.min(rows.saturating_sub(1));
        for (row, label, rect) in self.visible_rows(width, height) {
            if row == self.row {
                ops.push(DrawOp::Fill(rect, BORDER));
            }
            text(ops, 26.0, rect.y + 5.0, label, 12.0, TEXT);
            self.hits.push((rect, Hit::Row(row)));
        }
        text(
            ops,
            24.0,
            bounds.y + 246.0,
            self.status.chars().take(180).collect::<String>(),
            12.0,
            MUTED,
        );
        let apply_enabled = self.command_state("search.replacePreview.apply", workspace).enabled;
        let mut x = 24.0;
        for (label, id, enabled, w) in [
            ("Refresh", "search.replacePreview.refresh", !self.busy(), 80.0),
            (
                "Toggle all",
                "search.replacePreview.toggleAll",
                !self.busy() && self.preview.is_some(),
                100.0,
            ),
            ("Apply", "search.replacePreview.apply", apply_enabled, 76.0),
            ("Cancel", "search.replacePreview.cancel", self.busy(), 76.0),
            (
                "Rollback",
                "search.replacePreview.rollback",
                !self.busy() && self.rollback_target().is_some(),
                90.0,
            ),
            ("Close", "search.replacePreview.close", !self.busy(), 70.0),
        ] {
            let rect = rect(x, bounds.y + 275.0, w, 28.0);
            text(
                ops,
                x + 8.0,
                bounds.y + 281.0,
                label,
                12.0,
                if enabled { ACCENT } else { MUTED },
            );
            if enabled {
                self.hits.push((rect, Hit::Command(id)));
            }
            x += w + 8.0;
        }
    }
}

impl ReplaceRuntime {
    /// Share the rendered viewport with read-only accessibility publication.
    /// Never materialize the full replacement result list on the UI thread.
    fn visible_rows(&self, width: f32, height: f32) -> Vec<(usize, String, Rect)> {
        let rows = self.preview.as_ref().map_or(self.report.len(), Preview::count_rows);
        let top = self.top.min(rows.saturating_sub(1));
        (0..6)
            .filter_map(|visible| {
                let row = top + visible;
                if row >= rows {
                    return None;
                }
                let label = self
                    .preview
                    .as_ref()
                    .and_then(|preview| preview.label(row))
                    .or_else(|| self.report.get(row).cloned())?;
                Some((
                    row,
                    label,
                    rect(
                        20.0,
                        (height - 340.0).max(0.0) + 65.0 + visible as f32 * 28.0,
                        width - 40.0,
                        27.0,
                    ),
                ))
            })
            .collect()
    }
}
impl Shell {
    pub(in crate::shell) fn search_replace_accessibility_nodes(
        &self,
        width: f32,
        height: f32,
    ) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        let preview = &self.search.replace;
        if !preview.open {
            return Vec::new();
        }
        let y = (height - 340.0).max(0.0);
        let mut nodes = vec![AccessibilityNode {
            id: 78_000,
            parent: 1,
            role: AccessibilityRole::Status,
            name: "Replacement preview status".into(),
            value: Some(preview.status.chars().take(180).collect()),
            bounds: [24.0, (y + 246.0) as f64, (width - 48.0).max(0.0) as f64, 18.0],
            disabled: false,
            selected: false,
            expanded: None,
            focusable: false,
            invokable: false,
            position_in_set: None,
            size_of_set: None,
        }];
        for (slot, (_, label, bounds)) in preview.visible_rows(width, height).into_iter().enumerate() {
            nodes.push(AccessibilityNode {
                id: 78_001 + slot as u64,
                parent: 1,
                role: AccessibilityRole::ListItem,
                name: label,
                value: None,
                bounds: [
                    bounds.x as f64,
                    bounds.y as f64,
                    bounds.width as f64,
                    bounds.height as f64,
                ],
                disabled: false,
                selected: false,
                expanded: None,
                focusable: false,
                invokable: false,
                position_in_set: None,
                size_of_set: None,
            });
        }
        nodes
    }
    #[allow(clippy::too_many_lines)]
    pub(super) fn search_replace_command(&mut self, id: &str) -> bool {
        if !id.starts_with("search.replaceIn")
            && !id.starts_with("search.replacePreview.")
            && !id.starts_with("search.replaceBackups.")
        {
            return false;
        }
        if matches!(
            id,
            "search.replacePreview.preserveCase"
                | "search.replacePreview.includeBinary"
                | "search.replacePreview.backups"
        ) {
            if !self.search.replace.busy() {
                match id {
                    "search.replacePreview.preserveCase" => {
                        self.search.replace.replacement_options.preserve_case ^= true
                    }
                    "search.replacePreview.includeBinary" => {
                        self.search.replace.replacement_options.include_binary ^= true
                    }
                    _ => self.search.replace.disable_backup ^= true,
                }
                self.search.replace.preview = None;
                self.search.replace.status = "Options changed. Refresh and review before applying.".into();
            }
            return true;
        }
        let id = if id == "search.replacePreview.refresh" {
            if self.search.replace.workspace_scope {
                "search.replaceInWorkspace"
            } else {
                "search.replaceInFiles"
            }
        } else {
            id
        };
        if id == "search.replacePreview.close" {
            if !self.search.replace.busy() {
                self.search.replace.open = false;
            }
            return true;
        }
        self.search.replace.open = true;
        if id == "search.replacePreview.cancel" {
            self.search.replace.cancel_requested = true;
            for job in [
                self.search.replace.preparing.as_ref().map(|ticket| &ticket.job),
                self.search.replace.staging_paged.as_ref().map(|ticket| &ticket.job),
                self.search.replace.applying.as_ref().map(|ticket| &ticket.job),
                self.search.replace.rolling_back.as_ref().map(|ticket| &ticket.job),
                self.search.replace.listing.as_ref().map(|ticket| &ticket.job),
            ]
            .into_iter()
            .flatten()
            {
                job.cancel();
            }
            let cancelled: Vec<_> = self
                .search
                .replace
                .paged_queue
                .drain(..)
                .map(|file| file.source.identity_token())
                .collect();
            for token in cancelled {
                self.search
                    .replace
                    .record_open(token, "Skipped: cancelled before submission");
                self.search.replace.skipped_preview += 1;
            }
            self.search.replace.status = "Stopping at the next safe commit boundary…".into();
            return true;
        }
        if self.search.replace.busy() {
            return true;
        }
        if id == "search.replacePreview.toggleAll" {
            if let Some(preview) = &mut self.search.replace.preview {
                preview.toggle_all(preview.selected() == 0);
            }
            return true;
        }
        if id == "search.replacePreview.apply" {
            let state = self.search.replace.command_state(id, self.workspace.as_ref());
            if !state.enabled {
                self.search.replace.status = state
                    .disabled_reason
                    .unwrap_or_else(|| "Reviewed replacements cannot be applied yet".into());
                return true;
            }
            self.search_apply_preview();
            return true;
        }
        if !self.ensure_replace_worker() {
            return true;
        }
        match id {
            "search.replaceBackups.manage" => {
                self.search_replace_list_backups(None, None, false);
                return true;
            }
            "search.replaceBackups.prune" => {
                self.search_replace_list_backups(Some(BackupRetention::default()), None, false);
                return true;
            }
            "search.replaceBackups.delete" => {
                let selected = self
                    .search
                    .replace
                    .selected_listed()
                    .map(|job| (job.receipt.clone(), job.reconciling, job.retry));
                match selected {
                    Some((receipt, false, retry)) => {
                        // A retryable rollback never pins its backups forever: the user
                        // may give it up here, told exactly what is discarded.
                        let retry = match retry {
                            0 => String::new(),
                            1 => " 1 file could still be restored by retrying Rollback.".into(),
                            retry => format!(" {retry} files could still be restored by retrying Rollback."),
                        };
                        let confirmed = self.platform.as_ref().is_none_or(|platform| {
                            platform.task_dialog(
                                "Bareline",
                                "Delete this replace backup?",
                                &format!(
                                    "{}\n\nIts receipt and original-file backups are deleted permanently; this job can no longer be rolled back.{retry}",
                                    receipt.display()
                                ),
                                &[(DELETE_BACKUP_ID, "&Delete")],
                                DIALOG_CANCEL_ID,
                            ) == DELETE_BACKUP_ID
                        });
                        if confirmed {
                            // The session receipt is canonical; the listed one is not.
                            if self.search.replace.receipt.as_ref().is_some_and(|current| {
                                comparable_path(current) == comparable_path(&receipt)
                            }) {
                                self.search.replace.receipt = None;
                            }
                            self.search_replace_list_backups(None, Some(receipt), false);
                        }
                    }
                    Some((_, true, _)) => {
                        self.search.replace.status =
                            "Let this job finish reconciling (open Manage Replace Backups again) before deleting its backups".into()
                    }
                    None => self.search.replace.status = "Select a job in Manage Replace Backups first".into(),
                }
                return true;
            }
            _ => {}
        }
        if id == "search.replacePreview.rollback" {
            if let Some(receipt) = self.search.replace.rollback_target() {
                // No workspace means no open document can hold a target.
                let registry = self
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.replacement_registry())
                    .unwrap_or_default();
                self.search.replace.rolling_back = Some(self.search.replace.worker.as_ref().unwrap().operation(
                    move |job| {
                        rollback_receipt_with_paging(
                            &receipt,
                            &registry,
                            job,
                            &bareline_platform_windows::WindowsPathTrustProvider,
                            Arc::new(bareline_platform_windows::WindowsFileSystem),
                        )
                        .map_err(|error| error.to_string())
                    },
                    self.notify.clone(),
                ));
                self.search.replace.status = "Restoring unchanged targets from retained backups…".into();
            }
            return true;
        }
        self.search.replace.workspace_scope = id == "search.replaceInWorkspace";
        let replacement_options = self.search.replace.replacement_options;
        let root = match self.platform.as_ref().map(|platform| platform.pick_folder()) {
            Some(Ok(Some(root))) => root,
            Some(Err(error)) => {
                self.search.replace.status = error;
                return true;
            }
            _ => return true,
        };
        let Some(workspace) = &self.workspace else {
            return true;
        };
        if workspace.io_busy() {
            self.search.replace.status = "Wait for file operations to finish before previewing".into();
            return true;
        }
        let mut query = workspace.find.query();
        query.selection = None;
        let replacement = workspace.find.replacement.value().to_owned();
        let mut resident = Vec::new();
        let mut labels = Vec::new();
        let mut paged = Vec::new();
        let mut identities = Vec::new();
        let mut open_skips = Vec::new();
        for (index, editor) in workspace.editors.iter().enumerate() {
            let in_scope = workspace.path(index).is_some_and(|path| path.starts_with(&root))
                || (id == "search.replaceInWorkspace" && workspace.path(index).is_none());
            if !in_scope {
                continue;
            }
            if let Some(fingerprint) = workspace.fingerprint(index) {
                identities.push(fingerprint.identity.clone());
            }
            let label = workspace
                .path(index)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| format!("Untitled {}", index + 1));
            if editor.busy() || editor.read_only() {
                open_skips.push(format!(
                    "{label}: {}",
                    if editor.read_only() {
                        "Read-only; unlock and refresh preview"
                    } else {
                        "Busy; wait and refresh preview"
                    }
                ));
                continue;
            }
            match editor {
                WorkspaceEditor::Resident(editor) => {
                    if let Some(service) = editor.document_service() {
                        let snapshot = editor.snapshot().clone();
                        labels.push((snapshot.clone(), label));
                        resident.push((service, snapshot));
                    }
                }
                WorkspaceEditor::Paged(editor) => paged.push((editor.read_handle(), label)),
            }
        }
        self.search.replace.preview = None;
        self.search.replace.listed.clear();
        self.search.replace.row = 0;
        self.search.replace.top = 0;
        self.search.replace.cancel_requested = false;
        self.search.replace.preparing = Some(self.search.replace.worker.as_ref().unwrap().operation(
            move |job| {
                let platform: Arc<dyn bareline_platform::LocalFileSystem> =
                    Arc::new(bareline_platform_windows::WindowsFileSystem);
                let trust = bareline_platform_windows::WindowsPathTrustProvider;
                let mut folder_scope = FolderScope::user(root);
                folder_scope.include_binary = replacement_options.include_binary;
                let found =
                    bareline_search::folders::collect_folder(&folder_scope, &query, job, &trust, platform.clone());
                if found.summary.completeness != Completeness::Complete {
                    return Err(format!(
                        "Preview incomplete: {}; {} files skipped",
                        found.summary.completeness, found.summary.skipped_files
                    ));
                }
                let skipped = found.summary.skipped_files + open_skips.len();
                let disk_skip_reasons = found
                    .skips
                    .iter()
                    .take(4)
                    .map(|(path, reason)| format!("{}: {reason}", path.display()))
                    .collect::<Vec<_>>()
                    .join("; ");
                let skip_reasons = open_skips
                    .iter()
                    .take(4)
                    .cloned()
                    .chain(std::iter::once(disk_skip_reasons))
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join("; ");
                let paths = found
                    .groups
                    .into_iter()
                    .filter(|group| {
                        !identities.iter().any(|identity| {
                            identity.volume == group.fingerprint.identity.volume
                                && identity.file == group.fingerprint.identity.file
                        })
                    })
                    .map(|group| group.path);
                let disk = preview_disk_files_with_paging_options(
                    paths,
                    &query,
                    &replacement,
                    job,
                    &trust,
                    platform,
                    MAX_RESULT_BYTES / 3,
                    replacement_options,
                )
                .map_err(|error| error.to_string())?;
                let open = if resident.is_empty() {
                    None
                } else {
                    Some(
                        preview_open_documents_options(
                            resident,
                            &query,
                            &replacement,
                            job,
                            MAX_RESULT_BYTES / 3,
                            replacement_options,
                        )
                        .map_err(|error| error.to_string())?,
                    )
                };
                let labels = open
                    .as_ref()
                    .map(|open| {
                        open.documents()
                            .iter()
                            .map(|file| {
                                labels
                                    .iter()
                                    .find(|(snapshot, _)| snapshot.same_document(&file.snapshot))
                                    .map(|(_, label)| label.clone())
                                    .unwrap_or_else(|| "Document".into())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut paged_preview = Vec::new();
                let mut remaining = MAX_RESULT_BYTES / 3;
                let template = bareline_search::ReplacementTemplate::decode(&replacement, query.mode)
                    .map_err(|error| error.to_string())?;
                for (handle, label) in paged {
                    let source = handle.snapshot().clone();
                    let mut scoped = query.clone();
                    scoped.results_ram_bytes = remaining;
                    let found = bareline_search::paged::scan_paged(
                        &source,
                        &scoped,
                        job,
                        |ticket| handle.resolve_page(ticket).map_err(|error| error.to_string()),
                        |_| {},
                    );
                    if found.completeness != Completeness::Complete {
                        return Err(format!("Paged preview incomplete: {}", found.completeness));
                    }
                    if found.matches.is_empty() {
                        continue;
                    }
                    let mut transaction = found
                        .prepare_replace_streaming(&source, &template, ReplaceScope::All, job, |ticket| {
                            handle.resolve_page(ticket).map_err(|error| error.to_string())
                        })
                        .map_err(|error| error.to_string())?;
                    if replacement_options.preserve_case {
                        found
                            .preserve_case(&mut transaction, job, |ticket| {
                                handle.resolve_page(ticket).map_err(|error| error.to_string())
                            })
                            .map_err(|error| error.to_string())?;
                    }
                    let mut changes = Vec::new();
                    for edit in transaction.edits {
                        let before =
                            bareline_search::paged::match_excerpt(&source, edit.range.clone(), job, |ticket| {
                                handle.resolve_page(ticket).map_err(|error| error.to_string())
                            })
                            .map_err(|error| error.to_string())?;
                        let mut end = edit.insert.len().min(160);
                        while !edit.insert.is_char_boundary(end) {
                            end -= 1;
                        }
                        let after = edit.insert[..end].to_owned();
                        remaining = remaining
                            .checked_sub(
                                std::mem::size_of::<PagedChange>() + edit.insert.len() + before.len() + after.len(),
                            )
                            .ok_or("Preview budget reached")?;
                        changes.push(PagedChange {
                            edit,
                            before,
                            after,
                            included: true,
                        });
                    }
                    paged_preview.push(PagedPreview {
                        source,
                        label,
                        included: true,
                        changes,
                    });
                }
                Ok(Preview {
                    open,
                    labels,
                    paged: paged_preview,
                    disk,
                    skipped,
                    skip_reasons,
                    open_skips,
                })
            },
            self.notify.clone(),
        ));
        self.search.replace.status = "Preparing read-only preview; no files are being changed…".into();
        true
    }
    fn search_apply_preview(&mut self) {
        let Some(preview) = self.search.replace.preview.take() else {
            return;
        };
        if preview.selected() == 0 {
            self.search.replace.preview = Some(preview);
            return;
        }
        // Refuse before any document changes when disk receipts and backups have no
        // durable home.
        let disk_selected = preview
            .disk
            .files()
            .iter()
            .any(|file| file.included && file.changes.iter().any(|change| change.included));
        if disk_selected && self.replace_receipt_root().is_none() {
            self.search.replace.status = format!("{NO_DURABLE_ROOT} No files were changed.");
            self.search.replace.preview = Some(preview);
            return;
        }
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        self.search.replace.report.clear();
        self.search.replace.listed.clear();
        self.search.replace.open_outcomes = preview.open_skips.clone();
        self.search.replace.open_labels = preview
            .open
            .as_ref()
            .map(|open| {
                open.documents()
                    .iter()
                    .zip(&preview.labels)
                    .map(|(document, label)| (document.snapshot.identity_token(), label.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.search.replace.open_labels.extend(
            preview
                .paged
                .iter()
                .map(|file| (file.source.identity_token(), file.label.clone())),
        );
        self.search.replace.skipped_preview = preview.skipped;
        self.search.replace.skip_reasons = preview.skip_reasons.clone();
        self.search.replace.changed_open = 0;
        self.search.replace.changed_disk = 0;
        self.search.replace.replaced_disk = 0;
        self.search.replace.replaced_open = 0;
        self.search.replace.failed_open = 0;
        self.search.replace.cancel_requested = false;
        if let Some(mut open) = preview.open {
            for index in 0..open.documents().len() {
                let source = &open.documents()[index].snapshot;
                let reason = match workspace
                    .editors
                    .iter()
                    .find(|editor| editor.snapshot().same_document(source))
                {
                    Some(editor) if editor.read_only() => {
                        Some("Skipped: document became read-only; unlock and refresh preview")
                    }
                    Some(editor) if editor.busy() => Some("Skipped: document is busy; wait and refresh preview"),
                    None => Some("Skipped: document closed; refresh preview"),
                    _ => None,
                };
                if let Some(reason) = reason {
                    self.search.replace.record_open(source.identity_token(), reason);
                    self.search.replace.skipped_preview += 1;
                    open.set_document_included(index, false);
                }
            }
            let count = open.selected_matches();
            let selected_tokens: Vec<_> = open
                .documents()
                .iter()
                .filter(|document| document.included && document.changes.iter().any(|change| change.included))
                .map(|document| document.snapshot.identity_token())
                .collect();
            if count > 0 {
                match open.prepare(&SearchJob::default()) {
                    Ok(prepared) => {
                        let mut transactions = prepared.into_transactions();
                        if transactions.len() == 1 {
                            let (source, transaction) = transactions.remove(0);
                            let target = workspace.editors.iter_mut().find_map(|editor| match editor {
                                WorkspaceEditor::Resident(editor) if source.same_document(editor.snapshot()) => {
                                    Some(editor)
                                }
                                _ => None,
                            });
                            match target.map(|editor| editor.apply_prepared_tracked(&source, transaction)) {
                                Some(Ok(receipt)) => {
                                    self.search.replace.pending_open = Some(PendingOpen::Single(receipt, count))
                                }
                                error => {
                                    self.search.replace.failed_open += 1;
                                    let reason = match error {
                                        Some(Err(error)) => error,
                                        _ => "Document unavailable",
                                    };
                                    self.search.replace.record_open(source.identity_token(), reason);
                                }
                            }
                        } else {
                            let sources: Vec<_> = transactions.iter().map(|(source, _)| source.clone()).collect();
                            let failed_sources = sources.clone();
                            match workspace.apply_reviewed_open(transactions) {
                                Ok(group) => {
                                    self.search.replace.pending_open = Some(PendingOpen::Group(group, sources, count))
                                }
                                Err(error) => {
                                    self.search.replace.failed_open += failed_sources.len();
                                    for source in failed_sources {
                                        self.search.replace.record_open(source.identity_token(), &error);
                                    }
                                    self.search.replace.status = error;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        self.search.replace.failed_open += selected_tokens.len();
                        self.search.replace.status = format!("Open replacement failed: {error}.");
                        for token in selected_tokens {
                            self.search
                                .replace
                                .record_open(token, &format!("Preparation failed: {error}."));
                        }
                    }
                }
            }
        }
        for file in preview.paged.into_iter().filter(|file| file.included) {
            let edits: Vec<_> = file
                .changes
                .into_iter()
                .filter(|change| change.included)
                .map(|change| change.edit)
                .collect();
            if !edits.is_empty() {
                let matches = edits.len();
                self.search.replace.paged_queue.push_back(PagedApply {
                    transaction: EditTransaction {
                        base_revision: file.source.revision,
                        edits,
                    },
                    source: file.source,
                    matches,
                });
            }
        }
        self.search.replace.disk_queue = Some(preview.disk);
        self.search.replace.status = "Applying reviewed changes; open documents remain unsaved…".into();
    }
    #[allow(clippy::too_many_lines)]
    pub(super) fn search_replace_pump(&mut self) -> bool {
        let mut changed = false;
        // SRC-09: once per launch, reconcile jobs an earlier process left unfinished and
        // apply backup retention. Interrupted jobs surface with Rollback.
        if !self.search.replace.startup_listed
            && self.profile.settled()
            && !self.smoke
            && !self.perf
            && !self.search.replace.busy()
        {
            self.search.replace.startup_listed = true;
            if self.replace_receipt_root().is_some() && self.ensure_replace_worker() {
                self.search_replace_list_backups(Some(BackupRetention::default()), None, true);
            }
        }
        if let Some(ticket) = &self.search.replace.listing {
            match ticket.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.search.replace.listing = None;
                    let startup = std::mem::take(&mut self.search.replace.listing_startup);
                    let note = self.search.replace.listing_note.take();
                    changed = true;
                    match result {
                        Ok(Ok(listing)) => self
                            .search
                            .replace
                            .show_backups(listing, startup, note, unix_nanos_now()),
                        Ok(Err(error)) if startup => eprintln!("event=replace_receipts_scan_failed error={error:?}"),
                        Ok(Err(error)) => self.search.replace.status = error,
                        _ if startup => {}
                        _ => {
                            self.search.replace.status =
                                "Backup listing stopped; open Manage Replace Backups again".into()
                        }
                    }
                }
            }
        }
        if let Some(ticket) = &self.search.replace.preparing {
            match ticket.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.search.replace.preparing = None;
                    changed = true;
                    match result {
                        Ok(Ok(preview)) => {
                            self.search.replace.status = format!(
                                "{} selected matches; {} skipped. {} Review before applying.",
                                preview.selected(),
                                preview.skipped,
                                preview.skip_reasons
                            );
                            self.search.replace.preview = Some(preview);
                        }
                        Ok(Err(error)) => self.search.replace.status = error,
                        _ => self.search.replace.status = "Preview worker stopped".into(),
                    }
                }
            }
        }
        if let Some(pending) = self.search.replace.pending_open.take() {
            let Some(workspace) = &mut self.workspace else {
                return changed;
            };
            match pending {
                PendingOpen::Group(mut group, sources, count) => {
                    match workspace.pump_reviewed_open(&mut group, &sources) {
                        Ok(None) => self.search.replace.pending_open = Some(PendingOpen::Group(group, sources, count)),
                        Ok(Some(_)) => {
                            self.search.replace.changed_open += sources.len();
                            self.search.replace.replaced_open += count;
                            changed = true;
                        }
                        Err(error) => {
                            self.search.replace.failed_open += sources.len();
                            for source in &sources {
                                self.search.replace.record_open(source.identity_token(), &error);
                            }
                            self.search.replace.status = error;
                            changed = true;
                        }
                    }
                }
                PendingOpen::Single(receipt, count) => match receipt.terminal() {
                    None => self.search.replace.pending_open = Some(PendingOpen::Single(receipt, count)),
                    Some(Ok(_)) => {
                        self.search.replace.changed_open += 1;
                        self.search.replace.replaced_open += count;
                        changed = true;
                    }
                    Some(Err(error)) => {
                        self.search.replace.failed_open += 1;
                        self.search.replace.record_open(receipt.captured_identity_token, &error);
                        self.search.replace.status = error;
                        changed = true;
                    }
                },
            }
        }
        if let Some(ticket) = &self.search.replace.staging_paged {
            match ticket.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.search.replace.staging_paged = None;
                    changed = true;
                    let staging_token = self.search.replace.staging_token.take();
                    match result {
                        Ok(Ok((source, prepared, count))) if !self.search.replace.cancel_requested => {
                            let target = self.workspace.as_mut().and_then(|workspace| {
                                workspace.editors.iter_mut().find_map(|editor| match editor {
                                    WorkspaceEditor::Paged(editor) if source.same_document(editor.snapshot()) => {
                                        Some(editor)
                                    }
                                    _ => None,
                                })
                            });
                            match target.map(|editor| editor.apply_prepared_source_tracked(&source, prepared)) {
                                Some(Ok(receipt)) => self.search.replace.paged_pending = Some((receipt, count)),
                                _ => {
                                    self.search.replace.failed_open += 1;
                                    self.search.replace.record_open(
                                        source.identity_token(),
                                        "Source changed or became read-only before apply; refresh preview",
                                    );
                                    self.search.replace.status = "Paged replacement source changed before apply".into();
                                }
                            }
                        }
                        Ok(Err(error)) => {
                            self.search.replace.failed_open += 1;
                            if let Some(token) = staging_token {
                                self.search.replace.record_open(token, &error);
                            }
                            self.search.replace.status = error;
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some((receipt, count)) = self.search.replace.paged_pending.take() {
            match receipt.terminal() {
                None => self.search.replace.paged_pending = Some((receipt, count)),
                Some(Ok(_)) => {
                    self.search.replace.changed_open += 1;
                    self.search.replace.replaced_open += count;
                    changed = true;
                }
                Some(Err(error)) => {
                    self.search.replace.failed_open += 1;
                    self.search.replace.record_open(receipt.captured_identity_token, &error);
                    self.search.replace.status = error;
                    changed = true;
                }
            }
        }
        if self.search.replace.pending_open.is_none()
            && self.search.replace.paged_pending.is_none()
            && self.search.replace.staging_paged.is_none()
        {
            if let Some(next) = self.search.replace.paged_queue.pop_front() {
                let target = self.workspace.as_mut().and_then(|workspace| {
                    workspace.editors.iter_mut().find_map(|editor| match editor {
                        WorkspaceEditor::Paged(editor) if next.source.same_document(editor.snapshot()) => Some(editor),
                        _ => None,
                    })
                });
                if let Some(editor) = target {
                    if editor.viewport().read_only() || editor.busy() {
                        self.search.replace.record_open(
                            next.source.identity_token(),
                            "Skipped: document became read-only or busy; refresh preview",
                        );
                        self.search.replace.skipped_preview += 1;
                        return true;
                    }
                    self.search.replace.staging_token = Some(next.source.identity_token());
                    let handle = editor.read_handle();
                    let cache = self.recovery_root.clone().unwrap_or_else(std::env::temp_dir);
                    self.search.replace.staging_paged = Some(self.search.replace.worker.as_ref().unwrap().operation(
                        move |job| {
                            let prepared = bareline_search::paged::stage_source_replacement(
                                &next.source,
                                next.transaction,
                                job,
                                |ticket| handle.resolve_page(ticket).map_err(|error| error.to_string()),
                                Arc::new(bareline_platform_windows::WindowsFileSystem),
                                &cache,
                                20u64 << 30,
                            )?;
                            Ok((next.source, prepared, next.matches))
                        },
                        self.notify.clone(),
                    ));
                } else {
                    self.search.replace.failed_open += 1;
                    self.search.replace.record_open(
                        next.source.identity_token(),
                        "Document closed before apply; refresh preview",
                    );
                }
                changed = true;
            } else if let Some(disk) = self.search.replace.disk_queue.take() {
                let receipt_root = self.replace_receipt_root();
                if let (Some(workspace), Some(directory)) = (&self.workspace, receipt_root) {
                    let registry = workspace.replacement_registry();
                    let disable_backup = std::mem::take(&mut self.search.replace.disable_backup);
                    let open_outcomes = self.search.replace.open_outcomes.clone();
                    let cancelled = self.search.replace.cancel_requested;
                    self.search.replace.applying = Some(self.search.replace.worker.as_ref().unwrap().operation(
                        move |job| {
                            std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
                            let mut options = DiskReplaceOptions::new(directory);
                            options.open_outcomes = open_outcomes;
                            if cancelled {
                                job.cancel();
                            }
                            if disable_backup {
                                options.backup = bareline_search::replace_disk::BackupPolicy::DisabledForThisJob;
                            }
                            apply_disk_files_with_paging(
                                disk,
                                &options,
                                &registry,
                                job,
                                &bareline_platform_windows::WindowsPathTrustProvider,
                                Arc::new(bareline_platform_windows::WindowsFileSystem),
                            )
                            .map_err(|error| error.to_string())
                        },
                        self.notify.clone(),
                    ));
                    changed = true;
                } else if self.workspace.is_some() {
                    // Apply already refused every selected disk file without this root.
                    drop(disk);
                    self.search.replace.status = format!(
                        "Changed {} open documents; replaced {} matches. No disk receipt: {NO_DURABLE_ROOT}",
                        self.search.replace.changed_open, self.search.replace.replaced_open
                    );
                    changed = true;
                }
            }
        }
        if let Some(ticket) = &self.search.replace.applying {
            match ticket.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.search.replace.applying = None;
                    changed = true;
                    match result {
                        Ok(Ok(summary)) => {
                            self.search.replace.listed.clear();
                            self.search.replace.report = summary.receipt.open_outcomes.clone();
                            self.search
                                .replace
                                .report
                                .extend(summary.receipt.files.iter().map(|file| {
                                    let label = file
                                        .path
                                        .to_native()
                                        .map(|path| path.display().to_string())
                                        .unwrap_or_else(|_| "Disk file".into());
                                    format!("{label}: {} ({} matches)", file.state, file.matches)
                                }));
                            self.search.replace.top = 0;
                            self.search.replace.row = 0;
                            self.search.replace.changed_disk = summary.changed_files();
                            self.search.replace.replaced_disk = summary.replaced_matches();
                            let failed = summary
                                .receipt
                                .files
                                .iter()
                                .filter(|file| {
                                    matches!(
                                        file.state,
                                        bareline_search::replace_disk::ReceiptState::Failed(_)
                                            | bareline_search::replace_disk::ReceiptState::Uncertain(_)
                                            | bareline_search::replace_disk::ReceiptState::Conflict
                                    )
                                })
                                .count();
                            let skipped = summary
                                .receipt
                                .files
                                .iter()
                                .filter(|file| {
                                    matches!(file.state, bareline_search::replace_disk::ReceiptState::Skipped(_))
                                })
                                .count();
                            self.search.replace.status = format!(
                                "Changed {} open and {} disk files; replaced {} matches; skipped {}; failed {}. Receipt: {}",
                                self.search.replace.changed_open,
                                summary.changed_files(),
                                self.search.replace.replaced_open + summary.replaced_matches(),
                                skipped + self.search.replace.skipped_preview,
                                self.search.replace.failed_open + failed,
                                summary.receipt_path.display()
                            );
                            if !self.search.replace.skip_reasons.is_empty() {
                                self.search
                                    .replace
                                    .status
                                    .push_str(&format!(" Skipped examples: {}", self.search.replace.skip_reasons));
                            }
                            if !summary.receipt.open_outcomes.is_empty() {
                                self.search
                                    .replace
                                    .status
                                    .push_str(&format!(" Open outcomes: {}", summary.receipt.open_outcomes.join("; ")));
                            }
                            self.search.replace.receipt = Some(summary.receipt_path);
                        }
                        Ok(Err(error)) => {
                            self.search.replace.status = format!(
                                "Disk apply stopped: {error}; {} open documents already changed",
                                self.search.replace.changed_open
                            )
                        }
                        _ => {
                            self.search.replace.status =
                                "Apply worker stopped; reconcile the receipt before retrying".into()
                        }
                    }
                }
            }
        }
        if let Some(ticket) = &self.search.replace.rolling_back {
            match ticket.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.search.replace.rolling_back = None;
                    changed = true;
                    self.search.replace.status = match result {
                        Ok(Ok(receipt)) => {
                            let count = |state: fn(&ReceiptState) -> bool| {
                                receipt.files.iter().filter(|file| state(&file.state)).count()
                            };
                            format!(
                                "Restored {} files; {} conflicts; {} could not be restored now (retry Rollback); {} had no backup",
                                count(|state| *state == ReceiptState::RolledBack),
                                count(|state| *state == ReceiptState::Conflict),
                                count(|state| matches!(
                                    state,
                                    ReceiptState::RollbackFailed(_) | ReceiptState::RollbackStaged
                                )),
                                count(|state| matches!(
                                    state,
                                    ReceiptState::Skipped(reason)
                                        if reason.starts_with("No backup") || reason.starts_with("Backup missing")
                                )),
                            )
                        }
                        Ok(Err(error)) => error,
                        _ => "Rollback worker stopped; reconcile before retrying".into(),
                    };
                    // Keep a shown backup listing current; its rows follow the receipts.
                    if !self.search.replace.listed.is_empty() && self.search.replace.preview.is_none() {
                        let status = self.search.replace.status.clone();
                        self.search_replace_list_backups(None, None, false);
                        if self.search.replace.listing.is_some() {
                            self.search.replace.listing_note = Some(status.clone());
                        }
                        self.search.replace.status = status;
                    }
                }
            }
        }
        if changed && !self.search.replace.busy() && self.search.replace.cancel_requested {
            self.search.replace.cancel_requested = false;
            self.search.replace.status = format!(
                "Cancelled. {} open documents and {} disk files changed; {} matches applied before cancellation. {}",
                self.search.replace.changed_open,
                self.search.replace.changed_disk,
                self.search.replace.replaced_open + self.search.replace.replaced_disk,
                self.search.replace.status
            );
        }
        changed
    }
    pub(super) fn search_replace_pointer(&mut self, point: Point) -> bool {
        if !self.search.replace.open {
            return false;
        }
        let hit = self
            .search
            .replace
            .hits
            .iter()
            .find(|(bounds, _)| bounds.contains(point))
            .map(|(_, hit)| hit.clone());
        match hit {
            Some(Hit::Row(row)) => {
                self.search.replace.row = row;
                if !self.search.replace.busy() {
                    if let Some(preview) = &mut self.search.replace.preview {
                        preview.toggle(row);
                    }
                }
                true
            }
            Some(Hit::Command(id)) => self.search_replace_command(id),
            None => false,
        }
    }
    pub(super) fn search_replace_key(&mut self, key: bareline_ui::controls::Key) -> bool {
        use bareline_ui::controls::Key;
        if !self.search.replace.open {
            return false;
        }
        let rows = self
            .search
            .replace
            .preview
            .as_ref()
            .map_or(self.search.replace.report.len(), Preview::count_rows);
        match key {
            Key::Escape => {
                let id = if self.search.replace.busy() {
                    "search.replacePreview.cancel"
                } else {
                    "search.replacePreview.close"
                };
                self.search_replace_command(id);
            }
            Key::Up => self.search.replace.row = self.search.replace.row.saturating_sub(1),
            Key::Down => self.search.replace.row = (self.search.replace.row + 1).min(rows.saturating_sub(1)),
            Key::Home => self.search.replace.row = 0,
            Key::End => self.search.replace.row = rows.saturating_sub(1),
            Key::Space => {
                if !self.search.replace.busy() {
                    if let Some(preview) = &mut self.search.replace.preview {
                        preview.toggle(self.search.replace.row);
                    }
                }
            }
            Key::Enter => {
                self.search_replace_command("search.replacePreview.apply");
            }
            _ => return false,
        }
        if self.search.replace.row < self.search.replace.top {
            self.search.replace.top = self.search.replace.row;
        } else if self.search.replace.row >= self.search.replace.top + 6 {
            self.search.replace.top = self.search.replace.row.saturating_sub(5);
        }
        true
    }
}

impl Shell {
    /// Durable home of replace receipts and backups. Never `%TEMP%`, which Storage
    /// Sense may empty and turn every later rollback into a missing backup.
    fn replace_receipt_root(&self) -> Option<PathBuf> {
        self.recovery_root.clone().filter(|root| !under_temp(root))
    }
    fn ensure_replace_worker(&mut self) -> bool {
        if self.search.replace.worker.is_none() {
            match SearchWorker::new() {
                Ok(worker) => self.search.replace.worker = Some(worker),
                Err(error) => {
                    self.search.replace.status = error.to_string();
                    return false;
                }
            }
        }
        true
    }
    /// Deletes `delete`, lists every job (reconciling interrupted ones by hash) and
    /// then retires what `retention` expires. Runs on the replacement worker.
    fn search_replace_list_backups(
        &mut self,
        retention: Option<BackupRetention>,
        delete: Option<PathBuf>,
        startup: bool,
    ) {
        let Some(root) = self.replace_receipt_root() else {
            if !startup {
                self.search.replace.status = NO_DURABLE_ROOT.into();
            }
            return;
        };
        let Some(worker) = self.search.replace.worker.as_ref() else {
            return;
        };
        self.search.replace.listing_startup = startup;
        self.search.replace.listing_note = None;
        self.search.replace.listing = Some(worker.operation(
            move |job| {
                let platform = bareline_platform_windows::WindowsFileSystem;
                let trust = bareline_platform_windows::WindowsPathTrustProvider;
                let mut retired = 0;
                let mut released = 0;
                if let Some(receipt) = delete {
                    released += remove_receipt_job(&receipt, &platform)
                        .map_err(|error| format!("Could not delete the replace backup: {error}"))?;
                    retired += 1;
                }
                let mut scan = scan_receipts(&root, &replace_job_owner_running, job, &trust, &platform)
                    .map_err(|error| format!("Could not read replace backups: {error}"))?;
                if let Some(retention) = retention {
                    for receipt in retention.expired(&scan.jobs, unix_nanos_now()) {
                        if job.is_cancelled() {
                            break;
                        }
                        // A job that cannot be deleted now stays listed and is retried later.
                        if let Ok(bytes) = remove_receipt_job(&receipt, &platform) {
                            retired += 1;
                            released += bytes;
                            scan.jobs.retain(|listed| listed.receipt_path != receipt);
                        }
                    }
                }
                Ok(BackupListing {
                    scan,
                    retired,
                    released,
                })
            },
            self.notify.clone(),
        ));
        if !startup {
            self.search.replace.open = true;
            self.search.replace.status = "Reading replace backups…".into();
        }
    }
    /// APP-10: exit neither abandons nor silently cancels a disk replacement. Returns
    /// the request when exit may continue; otherwise it is retained (Wait, or Cancel
    /// and exit until the worker stops) or retired (the prompt itself was cancelled).
    pub(in crate::shell) fn search_replace_exit_gate(
        &mut self,
        pending: crate::shell::PendingClose,
    ) -> Option<crate::shell::PendingClose> {
        if !self.search.replace.disk_busy() {
            self.search.replace.exit_prompted = None;
            return Some(pending);
        }
        let ticket = self.pending_close_trace_ticket.unwrap_or(0);
        if self.search.replace.exit_prompted != Some(ticket) {
            self.search.replace.exit_prompted = Some(ticket);
            let choice = self.platform.as_ref().map(|platform| {
                platform.task_dialog(
                    "Bareline",
                    "A replacement is running",
                    "Replace in Files is changing files on disk. Wait for it to finish, or cancel it at the next safe file boundary and then exit. Every finished file stays in its receipt for Rollback.",
                    &[(EXIT_WAIT_ID, "&Wait"), (EXIT_CANCEL_ID, "&Cancel and exit")],
                    EXIT_WAIT_ID,
                )
            });
            match choice {
                Some(EXIT_CANCEL_ID) => {
                    self.search_replace_command("search.replacePreview.cancel");
                }
                Some(EXIT_WAIT_ID) | None => {
                    self.search.replace.open = true;
                    self.search.replace.status = "Exit continues when the replacement finishes…".into();
                }
                Some(_) => {
                    // The prompt's own Cancel keeps both the replacement and the window.
                    self.search.replace.exit_prompted = None;
                    self.qa_command_trace
                        .transition(ticket, "cancelled", "replacement-busy");
                    return None;
                }
            }
        }
        self.pending_close = Some(pending);
        self.qa_command_trace.transition(ticket, "deferred", "replacement-busy");
        None
    }
}

#[cfg(test)]
mod menu_state_tests {
    use super::*;
    use bareline_app::workspace::{Input, Workspace};

    fn facts(open: bool, busy: bool, selected: Option<usize>, current: bool) -> ReplacementMenuFacts {
        ReplacementMenuFacts {
            open,
            busy,
            selected,
            current,
            has_receipt: false,
        }
    }

    #[test]
    fn replacement_preview_accessibility_tracks_only_rendered_rows_and_terminal_status() {
        let mut shell = crate::shell::accessibility::tests::headless_shell();
        assert!(shell.search_replace_accessibility_nodes(1000.0, 700.0).is_empty());
        shell.search.replace.open = true;
        shell.search.replace.report = (0..20).map(|i| format!("Outcome {i}")).collect();
        shell.search.replace.top = 3;
        shell.search.replace.status = "Incomplete: regex resource limit".into();
        let nodes = shell.search_replace_accessibility_nodes(1000.0, 700.0);
        assert_eq!(nodes.len(), 7);
        assert_eq!(nodes[0].value.as_deref(), Some("Incomplete: regex resource limit"));
        for (slot, node) in nodes[1..].iter().enumerate() {
            assert_eq!(node.name, format!("Outcome {}", slot + 3));
            assert_eq!(node.bounds, [20.0, 425.0 + slot as f64 * 28.0, 960.0, 27.0]);
            assert!(!node.focusable && !node.invokable);
        }
        shell.search.replace.report.truncate(2);
        let nodes = shell.search_replace_accessibility_nodes(1000.0, 700.0);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1].name, "Outcome 1");
        shell.search.replace.open = false;
        assert!(shell.search_replace_accessibility_nodes(1000.0, 700.0).is_empty());
    }

    #[test]
    fn replacement_menu_state_covers_idle_running_cancelling_complete_and_stale() {
        let idle = facts(false, false, None, false);
        assert!(!replacement_command_state("search.replacePreview.apply", idle).enabled);
        assert!(!replacement_command_state("search.replacePreview.cancel", idle).enabled);

        let running = facts(true, true, None, false);
        assert!(!replacement_command_state("search.replacePreview.apply", running).enabled);
        assert!(replacement_command_state("search.replacePreview.cancel", running).enabled);

        // Cancellation remains an in-flight/busy state until the worker acknowledges it.
        let cancelling = facts(true, true, Some(3), true);
        assert!(replacement_command_state("search.replacePreview.cancel", cancelling).enabled);
        assert!(!replacement_command_state("search.replacePreview.close", cancelling).enabled);

        let complete = facts(true, false, Some(3), true);
        assert!(replacement_command_state("search.replacePreview.apply", complete).enabled);

        let stale = facts(true, false, Some(3), false);
        let stale_apply = replacement_command_state("search.replacePreview.apply", stale);
        assert!(!stale_apply.enabled);
        assert_eq!(
            stale_apply.disabled_reason.as_deref(),
            Some("Reviewed replacements are stale; refresh the preview")
        );
    }

    fn settle(workspace: &mut Workspace) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.editors.iter().any(WorkspaceEditor::busy) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn apply_route_preserves_preview_when_a_selected_source_changed() {
        let mut workspace =
            Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        for _ in 0..2 {
            workspace.new_document().unwrap();
        }
        for editor in &mut workspace.editors {
            editor.enqueue(Input::Insert("needle".into()));
        }
        settle(&mut workspace);

        let query = bareline_search::SearchQuery::literal("needle");
        let targets = workspace.editors.iter().map(|editor| match editor {
            WorkspaceEditor::Resident(surface) => (surface.document_service().unwrap(), surface.snapshot().clone()),
            WorkspaceEditor::Paged(_) => unreachable!(),
        });
        let mut open = preview_open_documents_options(
            targets,
            &query,
            "replacement",
            &SearchJob::default(),
            MAX_RESULT_BYTES,
            Default::default(),
        )
        .unwrap();
        assert!(open.set_document_included(1, false));
        let disk = bareline_search::replace_disk::preview_disk_files(
            Vec::<PathBuf>::new(),
            &query,
            "replacement",
            &SearchJob::default(),
            &bareline_platform_windows::WindowsPathTrustProvider,
            &bareline_platform_windows::WindowsFileSystem,
            MAX_RESULT_BYTES,
        )
        .unwrap();
        let preview = Preview {
            open: Some(open),
            labels: vec!["selected".into(), "deselected".into()],
            paged: Vec::new(),
            disk,
            skipped: 0,
            skip_reasons: String::new(),
            open_skips: Vec::new(),
        };

        workspace.editors[1].enqueue(Input::Insert(" changed".into()));
        settle(&mut workspace);
        assert!(preview.selected_sources_current(&workspace));

        workspace.editors[0].enqueue(Input::Insert(" changed".into()));
        settle(&mut workspace);
        assert!(!preview.selected_sources_current(&workspace));

        let mut shell = crate::shell::accessibility::tests::headless_shell();
        shell.workspace = Some(workspace);
        shell.search.replace.open = true;
        shell.search.replace.preview = Some(preview);
        assert!(shell.search_replace_command("search.replacePreview.apply"));
        assert!(shell.search.replace.preview.is_some());
        assert!(shell.search.replace.status.contains("stale"));
    }

    fn empty_disk_preview() -> DiskReplacePreview {
        bareline_search::replace_disk::preview_disk_files(
            Vec::<PathBuf>::new(),
            &bareline_search::SearchQuery::literal("x"),
            "y",
            &SearchJob::default(),
            &bareline_platform_windows::WindowsPathTrustProvider,
            &bareline_platform_windows::WindowsFileSystem,
            MAX_RESULT_BYTES,
        )
        .unwrap()
    }

    /// APP-10: exit waits for a disk replacement instead of cancelling it mid-job.
    #[test]
    fn exit_waits_for_a_running_disk_replacement() {
        let mut shell = crate::shell::accessibility::tests::headless_shell();
        shell.search.replace.disk_queue = Some(empty_disk_preview());
        shell.queue_application_close();
        let pending = shell.pending_close.take().unwrap();
        assert!(!shell.application_close_ready(pending));
        assert!(matches!(
            shell.pending_close,
            Some(crate::shell::PendingClose::Application)
        ));
        assert!(shell.search.replace.status.contains("replacement finishes"));
        let pending = shell.pending_close.take().unwrap();
        assert!(!shell.application_close_ready(pending), "still running");
        assert!(shell.pending_close.is_some());
        // The worker finished: the retained exit request now proceeds.
        shell.search.replace.disk_queue = None;
        let pending = shell.pending_close.take().unwrap();
        assert!(shell.application_close_ready(pending));
        assert!(shell.pending_close.is_none());
        assert!(shell.search.replace.exit_prompted.is_none());
    }

    fn receipt_job(name: &str, created_unix_nanos: u128, state: ReceiptState, interrupted: bool) -> ReceiptJob {
        use bareline_platform::SerializedPath;
        use bareline_search::replace_disk::{FileReceipt, ReceiptFingerprint};
        ReceiptJob {
            receipt_path: PathBuf::from(name).join("receipt.json"),
            receipt: ReplaceReceipt {
                version: 1,
                files: vec![FileReceipt {
                    path: SerializedPath::from_native(&PathBuf::from(name).join("target.txt")),
                    original: ReceiptFingerprint {
                        volume: 0,
                        file: 0,
                        length: 1,
                        modified: 0,
                        sha256: [0; 32],
                    },
                    after_hash: Some([1; 32]),
                    backup: Some(SerializedPath::from_native(&PathBuf::from(name).join("original-0.bak"))),
                    matches: 1,
                    state,
                }],
                open_outcomes: Vec::new(),
            },
            interrupted,
            owner: 1,
            created_unix_nanos,
        }
    }

    /// SRC-09: a job an earlier process left unfinished surfaces at startup with Rollback.
    #[test]
    fn startup_listing_surfaces_interrupted_replace_with_rollback() {
        let day = 24 * 60 * 60 * 1_000_000_000u128;
        let mut runtime = ReplaceRuntime::default();
        let quiet = BackupListing {
            scan: ReceiptScan {
                jobs: vec![receipt_job("done", day, ReceiptState::Committed, false)],
                unreadable: Vec::new(),
            },
            retired: 0,
            released: 0,
        };
        runtime.show_backups(quiet, true, None, 2 * day);
        assert!(!runtime.open, "startup stays quiet without an interrupted job");
        assert!(runtime.report.is_empty());

        let listing = BackupListing {
            scan: ReceiptScan {
                jobs: vec![
                    receipt_job("restored", 3 * day, ReceiptState::RolledBack, false),
                    receipt_job("killed", 2 * day, ReceiptState::ReconciledCommitted, true),
                ],
                unreadable: Vec::new(),
            },
            retired: 0,
            released: 0,
        };
        runtime.show_backups(listing, true, None, 3 * day);
        assert!(runtime.open);
        assert!(runtime.status.starts_with("Interrupted replace"), "{}", runtime.status);
        assert!(runtime.report[1].starts_with("Interrupted replace"));
        assert_eq!(runtime.row, 1);
        assert_eq!(
            runtime.rollback_target(),
            Some(PathBuf::from("killed").join("receipt.json"))
        );
        assert!(runtime.command_state("search.replacePreview.rollback", None).enabled);
        // A fully restored job offers no rollback.
        runtime.row = 0;
        assert_eq!(runtime.rollback_target(), None);
        assert!(!runtime.command_state("search.replacePreview.rollback", None).enabled);
    }

    /// SRC-07/SRC-11: a re-listing after Rollback keeps its outcome, and a job whose
    /// rollback can only be retried stays deletable; only reconciling jobs are held.
    #[test]
    fn relisting_keeps_the_rollback_outcome_and_retry_jobs_stay_deletable() {
        let day = 24 * 60 * 60 * 1_000_000_000u128;
        let mut runtime = ReplaceRuntime::default();
        let listing = BackupListing {
            scan: ReceiptScan {
                jobs: vec![
                    receipt_job("retry", day, ReceiptState::RollbackFailed("open".into()), false),
                    receipt_job("staged", day, ReceiptState::RollbackStaged, false),
                ],
                unreadable: Vec::new(),
            },
            retired: 0,
            released: 0,
        };
        runtime.show_backups(listing, false, Some("Restored 3 files; 1 conflicts".into()), 2 * day);
        assert!(
            runtime
                .status
                .starts_with("Restored 3 files; 1 conflicts. 2 replace backup jobs"),
            "{}",
            runtime.status
        );
        assert!(!runtime.listed[0].reconciling);
        assert_eq!(runtime.listed[0].retry, 1);
        assert!(runtime.listed[1].reconciling);
    }

    /// SRC-11: listing and session receipts match whatever their spelling.
    #[test]
    fn receipt_paths_compare_in_canonical_case_folded_form() {
        let temp = std::env::temp_dir();
        let upper = PathBuf::from(temp.to_string_lossy().to_uppercase());
        assert_eq!(comparable_path(&temp), comparable_path(&upper));
        assert_eq!(
            comparable_path(&temp.join("missing").join("receipt.json")),
            comparable_path(&upper.join("MISSING").join("Receipt.json"))
        );
        assert!(under_temp(&upper.join("nested")));
        assert!(!under_temp(temp.parent().unwrap()));
    }

    /// SRC-11: receipts and backups are never kept in %TEMP%; Apply refuses first.
    #[test]
    fn replace_receipts_and_backups_never_live_under_temp() {
        let mut shell = crate::shell::accessibility::tests::headless_shell();
        let durable = std::env::temp_dir().parent().unwrap().join("Bareline").join("recovery");
        shell.recovery_root = Some(durable.clone());
        assert_eq!(shell.replace_receipt_root(), Some(durable));
        shell.recovery_root = Some(std::env::temp_dir().join("bareline-recovery"));
        assert_eq!(shell.replace_receipt_root(), None);
        // Spelling %TEMP% in another case does not escape the check.
        let shouted = PathBuf::from(std::env::temp_dir().to_string_lossy().to_uppercase()).join("bareline-recovery");
        shell.recovery_root = Some(shouted);
        assert_eq!(shell.replace_receipt_root(), None);

        let root = std::env::temp_dir().join(format!(
            "bareline-replace-root-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let target = root.join("a.txt");
        std::fs::write(&target, "x").unwrap();
        let disk = bareline_search::replace_disk::preview_disk_files(
            vec![target.clone()],
            &bareline_search::SearchQuery::literal("x"),
            "y",
            &SearchJob::default(),
            &bareline_platform_windows::WindowsPathTrustProvider,
            &bareline_platform_windows::WindowsFileSystem,
            MAX_RESULT_BYTES,
        )
        .unwrap();
        shell.search.replace.preview = Some(Preview {
            open: None,
            labels: Vec::new(),
            paged: Vec::new(),
            disk,
            skipped: 0,
            skip_reasons: String::new(),
            open_skips: Vec::new(),
        });
        shell.search_apply_preview();
        assert!(shell.search.replace.preview.is_some(), "the reviewed preview is kept");
        assert!(shell.search.replace.disk_queue.is_none());
        assert!(
            shell.search.replace.status.contains("%TEMP%"),
            "{}",
            shell.search.replace.status
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"x");
        drop(shell);
        std::fs::remove_dir_all(root).unwrap();
    }
}
