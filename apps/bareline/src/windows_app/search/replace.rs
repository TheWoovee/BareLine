// SPDX-License-Identifier: MPL-2.0
//! Two-phase mixed workspace replacement. Preview owns exact reviewed ranges only.
use super::*;
use bareline_document::{DocumentSnapshot, Edit, EditTransaction, paged::PagedSnapshot};
use bareline_editor_surface::group_view::SurfaceGroup;
use bareline_renderer::{DrawOp, Rect};
use bareline_search::{
    Completeness, MAX_RESULT_BYTES, ReplaceScope, SearchJob,
    replace_disk::{
        DiskApplySummary, DiskReplaceOptions, DiskReplacePreview, ReplaceReceipt,
        apply_disk_files_with_paging, preview_disk_files_with_paging_options,
        rollback_receipt_with_paging,
    },
    replace_files::{OpenReplacePreview, preview_open_documents_options},
    service::{BackgroundTicket, SearchWorker},
};
use bareline_ui::{ACCENT, BORDER, CHROME, MUTED, TEXT, rect, text};
use std::{collections::VecDeque, sync::mpsc::TryRecvError};
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
        }) + self
            .paged
            .iter()
            .map(|file| file.changes.len() + 1)
            .sum::<usize>()
            + self
                .disk
                .files()
                .iter()
                .map(|file| file.changes.len() + 1)
                .sum::<usize>()
    }
    fn selected(&self) -> usize {
        self.open
            .as_ref()
            .map_or(0, OpenReplacePreview::selected_matches)
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
                    self.paged[file].changes[matched].included =
                        !self.paged[file].changes[matched].included;
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
                        format!(
                            "{}: {} → {}",
                            change.range.start.0, change.before, change.after
                        ),
                    )
                } else {
                    (
                        item.included,
                        format!(
                            "Open document: {}",
                            self.labels
                                .get(file)
                                .map(String::as_str)
                                .unwrap_or("Document")
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
                        format!(
                            "{}: {} → {}",
                            change.edit.range.start.0, change.before, change.after
                        ),
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
                        format!(
                            "{}: {} → {}",
                            change.range.start.0, change.before, change.after
                        ),
                    )
                } else {
                    (
                        item.included,
                        format!(
                            "{} [{:?}, BOM {}, EOL {:?}]",
                            item.path.display(),
                            item.encoding,
                            item.bom,
                            item.eol
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
#[derive(Clone)]
enum Hit {
    Row(usize),
    Command(&'static str),
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
    skipped_preview: usize,
    skip_reasons: String,
    row: usize,
    top: usize,
    hits: Vec<(Rect, Hit)>,
    cancel_requested: bool,
    replacement_options: bareline_search::ReplacementOptions,
    disable_backup: bool,
    workspace_scope: bool,
}
pub(super) fn register(registry: &mut CommandRegistry) {
    for (id, title) in [
        (
            "search.replacePreview.preserveCase",
            "Toggle Preserve Replacement Case",
        ),
        (
            "search.replacePreview.includeBinary",
            "Toggle Binary Replacement Inclusion",
        ),
        (
            "search.replacePreview.backups",
            "Toggle Backups for This Replacement Job",
        ),
        (
            "search.replacePreview.refresh",
            "Refresh Replacement Preview",
        ),
        ("search.replaceInFiles", "Replace in Files…"),
        ("search.replaceInWorkspace", "Replace in Workspace…"),
        ("search.replacePreview.apply", "Apply Reviewed Replacements"),
        (
            "search.replacePreview.toggleAll",
            "Toggle All Preview Changes",
        ),
        (
            "search.replacePreview.cancel",
            "Cancel Workspace Replacement",
        ),
        ("search.replacePreview.close", "Close Replacement Preview"),
        (
            "search.replacePreview.rollback",
            "Restore Last Replacement Backups",
        ),
    ] {
        let id = CommandId(id);
        let _ = registry.register(CommandSpec {
            id,
            title,
            category: "Search",
            shortcut: "",
            action: Action::Contributed(id),
        });
        let _ = registry.set_presentation(
            id,
            CommandPresentation {
                menu_path: format!("Search > {title}"),
                accessible_name: Some(title.into()),
                ..Default::default()
            },
        );
    }
}
impl ReplaceRuntime {
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
    }
    pub(super) fn draw(&mut self, width: f32, height: f32, ops: &mut Vec<DrawOp>) {
        if !self.open {
            return;
        }
        self.hits.clear();
        let bounds = rect(
            8.0,
            (height - 340.0).max(0.0),
            (width - 16.0).max(100.0),
            320.0,
        );
        ops.push(DrawOp::Fill(bounds, CHROME));
        ops.push(DrawOp::Stroke(bounds, BORDER, 1.0));
        text(
            ops,
            24.0,
            bounds.y + 12.0,
            "Replace preview · Review included files and matches",
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
                self.hits.push((
                    rect(option_x, bounds.y + 30.0, 180.0, 24.0),
                    Hit::Command(id),
                ));
            }
            option_x += 190.0;
        }
        let rows = self.preview.as_ref().map_or(0, Preview::count_rows);
        self.top = self.top.min(rows.saturating_sub(1));
        for visible in 0..6 {
            let row = self.top + visible;
            if row >= rows {
                break;
            }
            let y = bounds.y + 65.0 + visible as f32 * 28.0;
            let rect = rect(20.0, y, width - 40.0, 27.0);
            if row == self.row {
                ops.push(DrawOp::Fill(rect, BORDER));
            }
            if let Some(label) = self.preview.as_ref().and_then(|preview| preview.label(row)) {
                text(ops, 26.0, y + 5.0, label, 12.0, TEXT);
                self.hits.push((rect, Hit::Row(row)));
            }
        }
        text(
            ops,
            24.0,
            bounds.y + 246.0,
            self.status.chars().take(180).collect::<String>(),
            12.0,
            MUTED,
        );
        let selected = self.preview.as_ref().map_or(0, Preview::selected);
        let mut x = 24.0;
        for (label, id, enabled, w) in [
            (
                "Refresh",
                "search.replacePreview.refresh",
                !self.busy(),
                80.0,
            ),
            (
                "Toggle all",
                "search.replacePreview.toggleAll",
                !self.busy() && self.preview.is_some(),
                100.0,
            ),
            (
                "Apply",
                "search.replacePreview.apply",
                !self.busy() && selected > 0,
                76.0,
            ),
            ("Cancel", "search.replacePreview.cancel", self.busy(), 76.0),
            (
                "Rollback",
                "search.replacePreview.rollback",
                !self.busy() && self.receipt.is_some(),
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

impl Shell {
    pub(super) fn search_replace_command(&mut self, id: &str) -> bool {
        if !id.starts_with("search.replaceIn") && !id.starts_with("search.replacePreview.") {
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
                self.search.replace.status =
                    "Options changed. Refresh and review before applying.".into();
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
                self.search
                    .replace
                    .preparing
                    .as_ref()
                    .map(|ticket| &ticket.job),
                self.search
                    .replace
                    .staging_paged
                    .as_ref()
                    .map(|ticket| &ticket.job),
                self.search
                    .replace
                    .applying
                    .as_ref()
                    .map(|ticket| &ticket.job),
                self.search
                    .replace
                    .rolling_back
                    .as_ref()
                    .map(|ticket| &ticket.job),
            ]
            .into_iter()
            .flatten()
            {
                job.cancel();
            }
            self.search.replace.paged_queue.clear();
            self.search.replace.disk_queue = None;
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
            self.search_apply_preview();
            return true;
        }
        if self.search.replace.worker.is_none() {
            match SearchWorker::new() {
                Ok(worker) => self.search.replace.worker = Some(worker),
                Err(error) => {
                    self.search.replace.status = error.to_string();
                    return true;
                }
            }
        }
        if id == "search.replacePreview.rollback" {
            if let (Some(receipt), Some(workspace)) =
                (self.search.replace.receipt.clone(), self.workspace.as_ref())
            {
                let registry = workspace.replacement_registry();
                self.search.replace.rolling_back =
                    Some(self.search.replace.worker.as_ref().unwrap().operation(
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
                self.search.replace.status =
                    "Restoring unchanged targets from retained backups…".into();
            }
            return true;
        }
        self.search.replace.workspace_scope = id == "search.replaceInWorkspace";
        let replacement_options = self.search.replace.replacement_options;
        let root = match self
            .platform
            .as_ref()
            .map(|platform| platform.pick_folder())
        {
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
            self.search.replace.status =
                "Wait for file operations to finish before previewing".into();
            return true;
        }
        let mut query = workspace.find.query();
        query.selection = None;
        let replacement = workspace.find.replacement.value().to_owned();
        let mut resident = Vec::new();
        let mut labels = Vec::new();
        let mut paged = Vec::new();
        let mut identities = Vec::new();
        for (index, editor) in workspace.editors.iter().enumerate() {
            let in_scope = workspace
                .path(index)
                .is_some_and(|path| path.starts_with(&root))
                || (id == "search.replaceInWorkspace" && workspace.path(index).is_none());
            if !in_scope {
                continue;
            }
            if editor.busy() || editor.read_only() {
                self.search.replace.status =
                    "An in-scope open document is busy or read-only; review after it is available"
                        .into();
                return true;
            }
            if let Some(fingerprint) = workspace.fingerprint(index) {
                identities.push(fingerprint.identity.clone());
            }
            let label = workspace
                .path(index)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| format!("Untitled {}", index + 1));
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
        self.search.replace.row = 0;
        self.search.replace.top = 0;
        self.search.replace.cancel_requested = false;
        self.search.replace.preparing =
            Some(self.search.replace.worker.as_ref().unwrap().operation(
                move |job| {
                    let platform: Arc<dyn bareline_platform::LocalFileSystem> =
                        Arc::new(bareline_platform_windows::WindowsFileSystem);
                    let trust = bareline_platform_windows::WindowsPathTrustProvider;
                    let mut folder_scope = FolderScope::user(root);
                    folder_scope.include_binary = replacement_options.include_binary;
                    let found = bareline_search::folders::collect_folder(
                        &folder_scope,
                        &query,
                        job,
                        &trust,
                        platform.clone(),
                    );
                    if found.summary.completeness != Completeness::Complete {
                        return Err(format!(
                            "Preview incomplete: {:?}; {} files skipped",
                            found.summary.completeness, found.summary.skipped_files
                        ));
                    }
                    let skipped = found.summary.skipped_files;
                    let skip_reasons = found
                        .skips
                        .iter()
                        .take(4)
                        .map(|(path, reason)| format!("{}: {reason:?}", path.display()))
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
                            .map_err(|error| format!("{error:?}"))?,
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
                                        .find(|(snapshot, _)| {
                                            snapshot.same_document(&file.snapshot)
                                        })
                                        .map(|(_, label)| label.clone())
                                        .unwrap_or_else(|| "Document".into())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut paged_preview = Vec::new();
                    let mut remaining = MAX_RESULT_BYTES / 3;
                    for (handle, label) in paged {
                        let source = handle.snapshot().clone();
                        let mut scoped = query.clone();
                        scoped.results_ram_bytes = remaining;
                        let found = bareline_search::paged::scan_paged(
                            &source,
                            &scoped,
                            job,
                            |ticket| handle.resolve_page(ticket),
                            |_| {},
                        );
                        if found.completeness != Completeness::Complete {
                            return Err(format!(
                                "Paged preview incomplete: {:?}",
                                found.completeness
                            ));
                        }
                        if found.matches.is_empty() {
                            continue;
                        }
                        let mut transaction = found
                            .prepare_replace_streaming(
                                &source,
                                &replacement,
                                ReplaceScope::All,
                                job,
                                |ticket| handle.resolve_page(ticket),
                            )
                            .map_err(|error| format!("{error:?}"))?;
                        if replacement_options.preserve_case {
                            found
                                .preserve_case(&mut transaction, job, |ticket| {
                                    handle.resolve_page(ticket)
                                })
                                .map_err(|error| format!("{error:?}"))?;
                        }
                        let mut changes = Vec::new();
                        for edit in transaction.edits {
                            let before = bareline_search::paged::excerpt(
                                &source,
                                edit.range.start,
                                job,
                                |ticket| handle.resolve_page(ticket),
                            )
                            .map_err(|error| format!("{error:?}"))?;
                            let mut end = edit.insert.len().min(160);
                            while !edit.insert.is_char_boundary(end) {
                                end -= 1;
                            }
                            let after = edit.insert[..end].to_owned();
                            remaining = remaining
                                .checked_sub(
                                    std::mem::size_of::<PagedChange>()
                                        + edit.insert.len()
                                        + before.len()
                                        + after.len(),
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
                    })
                },
                self.notify.clone(),
            ));
        self.search.replace.status =
            "Preparing read-only preview; no files are being changed…".into();
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
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        self.search.replace.skipped_preview = preview.skipped;
        self.search.replace.skip_reasons = preview.skip_reasons.clone();
        self.search.replace.changed_open = 0;
        self.search.replace.changed_disk = 0;
        self.search.replace.replaced_disk = 0;
        self.search.replace.replaced_open = 0;
        self.search.replace.failed_open = 0;
        self.search.replace.cancel_requested = false;
        if let Some(open) = preview.open {
            let count = open.selected_matches();
            if count > 0 {
                match open.prepare(&SearchJob::default()) {
                    Ok(prepared) => {
                        let mut transactions = prepared.into_transactions();
                        if transactions.len() == 1 {
                            let (source, transaction) = transactions.remove(0);
                            let target =
                                workspace
                                    .editors
                                    .iter_mut()
                                    .find_map(|editor| match editor {
                                        WorkspaceEditor::Resident(editor)
                                            if source.same_document(editor.snapshot()) =>
                                        {
                                            Some(editor)
                                        }
                                        _ => None,
                                    });
                            match target
                                .map(|editor| editor.apply_prepared_tracked(&source, transaction))
                            {
                                Some(Ok(receipt)) => {
                                    self.search.replace.pending_open =
                                        Some(PendingOpen::Single(receipt, count))
                                }
                                _ => self.search.replace.failed_open += 1,
                            }
                        } else {
                            let sources = transactions
                                .iter()
                                .map(|(source, _)| source.clone())
                                .collect();
                            match workspace.apply_reviewed_open(transactions) {
                                Ok(group) => {
                                    self.search.replace.pending_open =
                                        Some(PendingOpen::Group(group, sources, count))
                                }
                                Err(error) => {
                                    self.search.replace.failed_open += 1;
                                    self.search.replace.status = error;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        self.search.replace.failed_open += 1;
                        self.search.replace.status = format!("Open replacement failed: {error:?}");
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
        self.search.replace.status =
            "Applying reviewed changes; open documents remain unsaved…".into();
    }
    pub(super) fn search_replace_pump(&mut self) -> bool {
        let mut changed = false;
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
                        Ok(None) => {
                            self.search.replace.pending_open =
                                Some(PendingOpen::Group(group, sources, count))
                        }
                        Ok(Some(_)) => {
                            self.search.replace.changed_open += sources.len();
                            self.search.replace.replaced_open += count;
                            changed = true;
                        }
                        Err(error) => {
                            self.search.replace.failed_open += sources.len();
                            self.search.replace.status = error;
                            changed = true;
                        }
                    }
                }
                PendingOpen::Single(receipt, count) => match receipt.terminal() {
                    None => {
                        self.search.replace.pending_open = Some(PendingOpen::Single(receipt, count))
                    }
                    Some(Ok(_)) => {
                        self.search.replace.changed_open += 1;
                        self.search.replace.replaced_open += count;
                        changed = true;
                    }
                    Some(Err(error)) => {
                        self.search.replace.failed_open += 1;
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
                    match result {
                        Ok(Ok((source, prepared, count)))
                            if !self.search.replace.cancel_requested =>
                        {
                            let target = self.workspace.as_mut().and_then(|workspace| {
                                workspace
                                    .editors
                                    .iter_mut()
                                    .find_map(|editor| match editor {
                                        WorkspaceEditor::Paged(editor)
                                            if source.same_document(editor.snapshot()) =>
                                        {
                                            Some(editor)
                                        }
                                        _ => None,
                                    })
                            });
                            match target.map(|editor| {
                                editor.apply_prepared_source_tracked(&source, prepared)
                            }) {
                                Some(Ok(receipt)) => {
                                    self.search.replace.paged_pending = Some((receipt, count))
                                }
                                _ => {
                                    self.search.replace.failed_open += 1;
                                    self.search.replace.status =
                                        "Paged replacement source changed before apply".into();
                                }
                            }
                        }
                        Ok(Err(error)) => {
                            self.search.replace.failed_open += 1;
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
                    workspace
                        .editors
                        .iter_mut()
                        .find_map(|editor| match editor {
                            WorkspaceEditor::Paged(editor)
                                if next.source.same_document(editor.snapshot()) =>
                            {
                                Some(editor)
                            }
                            _ => None,
                        })
                });
                if let Some(editor) = target {
                    let handle = editor.read_handle();
                    let cache = self
                        .recovery_root
                        .clone()
                        .unwrap_or_else(std::env::temp_dir);
                    self.search.replace.staging_paged =
                        Some(self.search.replace.worker.as_ref().unwrap().operation(
                            move |job| {
                                let prepared = bareline_search::paged::stage_source_replacement(
                                    &next.source,
                                    next.transaction,
                                    job,
                                    |ticket| handle.resolve_page(ticket),
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
                }
                changed = true;
            } else if let Some(disk) = self.search.replace.disk_queue.take() {
                if let Some(workspace) = &self.workspace {
                    let registry = workspace.replacement_registry();
                    let directory = self
                        .recovery_root
                        .clone()
                        .unwrap_or_else(std::env::temp_dir);
                    let disable_backup = std::mem::take(&mut self.search.replace.disable_backup);
                    self.search.replace.applying =
                        Some(self.search.replace.worker.as_ref().unwrap().operation(
                            move |job| {
                                std::fs::create_dir_all(&directory)
                                    .map_err(|error| error.to_string())?;
                                let mut options = DiskReplaceOptions::new(directory);
                                if disable_backup { options.backup = bareline_search::replace_disk::BackupPolicy::DisabledForThisJob; }
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
                            self.search.replace.changed_disk = summary.changed_files();
                            self.search.replace.replaced_disk = summary.replaced_matches();
                            let failed=summary.receipt.files.iter().filter(|file|matches!(file.state,bareline_search::replace_disk::ReceiptState::Failed(_)|bareline_search::replace_disk::ReceiptState::Uncertain(_)|bareline_search::replace_disk::ReceiptState::Conflict)).count();
                            let skipped = summary
                                .receipt
                                .files
                                .iter()
                                .filter(|file| {
                                    matches!(
                                        file.state,
                                        bareline_search::replace_disk::ReceiptState::Skipped(_)
                                    )
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
                                self.search.replace.status.push_str(&format!(
                                    " Skipped examples: {}",
                                    self.search.replace.skip_reasons
                                ));
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
                        Ok(Ok(receipt)) => format!(
                            "Restored {} files; {} conflicts",
                            receipt
                                .files
                                .iter()
                                .filter(|file| file.state
                                    == bareline_search::replace_disk::ReceiptState::RolledBack)
                                .count(),
                            receipt
                                .files
                                .iter()
                                .filter(|file| file.state
                                    == bareline_search::replace_disk::ReceiptState::Conflict)
                                .count()
                        ),
                        Ok(Err(error)) => error,
                        _ => "Rollback worker stopped; reconcile before retrying".into(),
                    };
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
            .map_or(0, Preview::count_rows);
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
            Key::Down => {
                self.search.replace.row = (self.search.replace.row + 1).min(rows.saturating_sub(1))
            }
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
