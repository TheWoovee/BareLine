// SPDX-License-Identifier: MPL-2.0
use bareline_document::{Budget, Document, service::Scheduler};
use bareline_editor_surface::{EditorSurface, paged_view::PagedEditorSurface};
pub use bareline_editor_surface::Input;
use bareline_file_io::lifecycle::{
    FileError, Fingerprint, IoCompletion, IoRequest, IoService, IoTicket,
};
use bareline_platform::LocalFileSystem;
use bareline_renderer::{DrawOp, LayoutError, Rect, TextBackend};
use std::sync::Arc;
use std::{path::PathBuf, sync::mpsc::TryRecvError};

pub enum WorkspaceEditor {
    Resident(EditorSurface),
    Paged(PagedEditorSurface),
}
impl From<EditorSurface> for WorkspaceEditor { fn from(value: EditorSurface) -> Self { Self::Resident(value) } }
impl std::ops::Deref for WorkspaceEditor {
    type Target = EditorSurface;
    fn deref(&self) -> &EditorSurface { match self { Self::Resident(e) => e, Self::Paged(e) => &e.surface } }
}
impl std::ops::DerefMut for WorkspaceEditor {
    fn deref_mut(&mut self) -> &mut EditorSurface { match self { Self::Resident(e) => e, Self::Paged(e) => &mut e.surface } }
}
impl WorkspaceEditor {
    pub fn commit(&mut self, value: String) { match self { Self::Resident(editor) => editor.commit(value), Self::Paged(editor) => { editor.surface.cancel_composition(); editor.enqueue(Input::Insert(value)); } } }
    pub fn enqueue(&mut self, input: Input) { match self { Self::Resident(e) => e.enqueue(input), Self::Paged(e) => e.enqueue(input) } }
    pub fn pump(&mut self) -> bool { match self { Self::Resident(e) => e.pump(), Self::Paged(e) => { let changed = e.pump(); changed | e.surface.pump() } } }
    pub fn can_undo(&self) -> bool { match self { Self::Resident(editor)=>editor.can_undo(),Self::Paged(editor)=>editor.can_undo() } }
    pub fn can_redo(&self) -> bool { match self { Self::Resident(editor)=>editor.can_redo(),Self::Paged(editor)=>editor.can_redo() } }
    pub fn dirty(&self) -> bool { match self { Self::Resident(e) => e.dirty(), Self::Paged(e) => e.dirty() } }
    pub fn busy(&self) -> bool { match self { Self::Resident(e) => e.busy(), Self::Paged(e) => e.busy() } }
    pub fn read_only(&self) -> bool { match self { Self::Resident(e) => e.read_only(), Self::Paged(e) => e.surface.user_read_only } }
    pub fn set_read_only(&mut self, value: bool) { self.user_read_only = value; }
    pub fn recovery_status(&self) -> bareline_file_io::paged_recovery::PagedRecoveryStatus { match self { Self::Resident(editor)=>editor.recovery_status(), Self::Paged(editor)=>editor.recovery_status() } }
    pub fn retry_recovery(&mut self) -> Result<(),String> { match self { Self::Resident(editor)=>{editor.retry_recovery();Ok(())},Self::Paged(editor)=>editor.retry_recovery() } }
    pub fn paged(&self) -> bool { matches!(self, Self::Paged(_)) }
    /// Navigate a bounded text window without scanning the complete source.
    pub fn page_by(&mut self, forward: bool) -> bool {
        let Self::Paged(editor) = self else { return false; };
        let start = editor.viewport_start().0;
        let next = if forward { start.saturating_add(48 * 1024).min(editor.snapshot().len()) } else { start.saturating_sub(48 * 1024) };
        if let Err(error) = editor.request_viewport(bareline_document::TextOffset(next)) { editor.error = Some(error); }
        true
    }
}

pub struct Workspace {
    pub theme: bareline_ui::theme::UiTheme,
    pub editors: Vec<WorkspaceEditor>,
    scheduler: Scheduler,
    bytes: Budget,
    history: Budget,
    pub resident_max_bytes: u64,
    pub transcode_quota_bytes: u64,
    pub recovery_root: Option<PathBuf>,
    notify: Arc<dyn Fn() + Send + Sync>,
    last_drawn: Option<usize>,
    files: Vec<Option<FileState>>,
    untitled_labels: Vec<String>,
    next_untitled: u64,
    file_system: Arc<dyn LocalFileSystem>,
    io: Option<IoService>,
    pending_io: Vec<PendingIo>,
    pub message: Option<String>,
    pub find: crate::find::FindController,
    pub search_panel: crate::search_panel::SearchPanel,
    pub search_focus: bool,
    pending_replace: Option<bareline_search::service::ReplaceTicket>,
    styling: crate::styling::Styling,
    retired: Vec<WorkspaceEditor>,
    closed: Vec<(WorkspaceEditor, Option<FileState>, String)>,
    paused_transcode: Option<Box<bareline_file_io::lifecycle::PausedTranscode>>,
}
struct FileState {
    path: PathBuf,
    fingerprint: Fingerprint,
    bom: bool,
    encoding: Option<bareline_file_io::codecs::resident::ResidentEncoding>,
}
struct PendingIo {
    receiver: IoTicket,
    save: Option<(usize, PathBuf, bool)>,
    copy_only: bool,
    open_path: Option<PathBuf>,
    preview: Option<bareline_document::DocumentSnapshot>,
    reload: Option<bareline_document::DocumentSnapshot>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseError {
    Busy,
    Unsaved,
    Missing,
}
impl Workspace {
    /// Created lazily on the first document command, after the initial frame.
    pub fn new(
        notify: Arc<dyn Fn() + Send + Sync>,
        file_system: Arc<dyn LocalFileSystem>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            theme: bareline_ui::theme::UiTheme::default(),
            editors: Vec::new(),
            scheduler: Scheduler::new(2, 256)?,
            bytes: Budget::new(256 << 20),
            history: Budget::new(128 << 20),
            resident_max_bytes: 256 << 20,
            transcode_quota_bytes: 20u64 << 30,
            recovery_root: None,
            notify,
            last_drawn: None,
            files: Vec::new(),
            untitled_labels: Vec::new(),
            next_untitled: 1,
            file_system,
            io: None,
            pending_io: Vec::new(),
            message: None,
            find: crate::find::FindController::default(),
            search_panel: Default::default(),
            search_focus: false,
            pending_replace: None,
            styling: crate::styling::Styling::default(),
            retired: Vec::new(),
            closed: Vec::new(),
            paused_transcode: None,
        })
    }
    pub fn new_document(&mut self) -> Result<(), bareline_document::Error> {
        let document = Document::from_utf8("", self.bytes.clone(), self.history.clone())?;
        let snapshot = document.snapshot();
        self.editors.push(EditorSurface::new(
            self.scheduler.document(document, 32),
            snapshot,
            self.notify.clone(),
        ).into());
        self.files.push(None);
        self.untitled_labels
            .push(format!("Untitled {}", self.next_untitled));
        self.next_untitled += 1;
        Ok(())
    }
    /// Adopt an immutable compare/recovery view without copying the resident text.
    /// Its fresh document identity prevents source/target aliases during hunk apply.
    pub fn add_snapshot_preview(&mut self, snapshot: &bareline_document::DocumentSnapshot, label: String) -> Result<usize, bareline_document::Error> {
        let document = Document::fork_from_snapshot(snapshot, self.bytes.clone(), self.history.clone())?;
        let mut editor = EditorSurface::loading(document.snapshot(), self.notify.clone());
        editor.user_read_only = true;
        let index = self.editors.len();
        self.editors.push(editor.into());
        self.files.push(None);
        self.untitled_labels.push(label.chars().take(4096).collect());
        Ok(index)
    }
    pub fn pump(&mut self) -> bool {
        let mut changed = self.find.pump();
        changed |= self.search_panel.pump();
        changed |= self.styling.pump();
        for (index, editor) in self.editors.iter_mut().enumerate() {
            if let Some(root) = &self.recovery_root {
                match editor {
                    WorkspaceEditor::Resident(surface) => surface.enable_recovery(root.clone(), self.file_system.clone(), self.files[index].as_ref().and_then(|file|file.encoding.clone()), self.files[index].as_ref().map(|file|file.path.clone()), self.bytes.clone()),
                    WorkspaceEditor::Paged(surface) => surface.enable_recovery(root.clone(), self.file_system.clone()),
                }
            }
            changed |= editor.pump();
            if let WorkspaceEditor::Paged(paged) = editor { if let Some(error) = &paged.error { self.message = Some(error.clone()); } }
        }
        for (index, editor) in self.editors.iter().enumerate() { if let WorkspaceEditor::Paged(paged) = editor { if let Some(file) = &mut self.files[index] { file.path = paged.path.clone(); file.fingerprint = paged.fingerprint.clone(); } } }
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
                                Some(editor) => match editor
                                    .apply_prepared(&prepared.source, prepared.transaction)
                                {
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
        if self.message.as_deref().is_some_and(|message| message.starts_with("Applying ")) && !self.editors.iter().any(|editor| editor.busy()) { self.message = Some("Replacement complete.".into()); }
        let mut i = 0;
        while i < self.pending_io.len() {
            if let Ok(prefix) = self.pending_io[i].receiver.try_prefix() && self.pending_io[i].reload.is_none() {
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
            let result = match self.pending_io[i].receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(TryRecvError::Disconnected) => {
                    self.message = Some("File worker stopped.".into());
                    let failed = self.pending_io.remove(i);
                    self.discard_preview(failed.preview.as_ref());
                    changed = true;
                    continue;
                }
            };
            let pending = self.pending_io.remove(i);
            changed = true;
            match result {
                IoCompletion::Open(Ok(opened)) => {
                    if let Some(captured) = &pending.reload {
                        let current = self.editors.iter().position(|editor| editor.snapshot().same_document(captured));
                        if let Some(index) = current && self.editors[index].snapshot().revision == captured.revision && !self.editors[index].busy() {
                            let snapshot = opened.document.snapshot();
                            self.editors[index].finish_loading(self.scheduler.document(opened.document, 32), snapshot);
                            self.editors[index].enqueue(Input::SetCaret(0,false));
                            self.files[index] = Some(FileState {path:opened.path,fingerprint:opened.fingerprint,bom:opened.bom,encoding:opened.encoding});
                            self.find.clear_source();
                            self.message = Some("Reloaded from disk.".into());
                        } else { self.message = Some("Document changed while reloading; current edits were preserved.".into()); }
                        continue;
                    }
                    if let Some(existing) = self.files.iter().position(|f| {
                        f.as_ref().is_some_and(|f| {
                            f.fingerprint.identity.volume == opened.fingerprint.identity.volume
                                && f.fingerprint.identity.file == opened.fingerprint.identity.file
                        })
                    }) {
                        self.message =
                            Some(format!("File is already open in tab {}.", existing + 1));
                        self.discard_preview(pending.preview.as_ref());
                        continue;
                    }
                    let snapshot = opened.document.snapshot();
                    let file = Some(FileState {
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
                        self.editors[index]
                            .finish_loading(self.scheduler.document(opened.document, 32), snapshot);
                        self.files[index] = file;
                        self.untitled_labels[index].clear();
                    } else {
                        self.editors.push(EditorSurface::new(
                            self.scheduler.document(opened.document, 32),
                            snapshot,
                            self.notify.clone(),
                        ).into());
                        self.files.push(file);
                        self.untitled_labels.push(String::new());
                    }
                    self.message = None;
                }
                IoCompletion::Save(Ok(saved)) => {
                    if !pending.copy_only
                        && let Some((index, path, bom)) = pending.save
                        && let Some(editor) = self.editors.get_mut(index)
                    {
                        editor.mark_saved(&saved.captured);
                        self.files[index] = Some(FileState {
                            path,
                            fingerprint: saved.fingerprint,
                            bom,
                            encoding: self.files[index]
                                .as_ref()
                                .and_then(|file| file.encoding.clone()),
                        });
                    }
                    self.message = None;
                }
                IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Complete(opened)) => {
                    self.discard_preview(pending.preview.as_ref());
                    let file = FileState { path: opened.path.clone(), fingerprint: opened.fingerprint.clone(), bom: opened.transcoded.store.state.bom, encoding: None };
                    match PagedEditorSurface::new(opened, self.bytes.clone(), self.notify.clone()) {
                        Ok(mut editor) => { if let Some(root) = &self.recovery_root { editor.enable_recovery(root.clone(), self.file_system.clone()); } self.editors.push(WorkspaceEditor::Paged(editor)); self.files.push(Some(file)); self.untitled_labels.push(String::new()); self.message = None; }
                        Err(error) => self.message = Some(error),
                    }
                }
                IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Paused(paused)) => {
                    self.discard_preview(pending.preview.as_ref());
                    self.message = Some(format!("Transcode quota reached: {:?}. Resume after increasing the quota.", paused.error));
                    self.paused_transcode = Some(paused);
                }
                IoCompletion::Transcode(bareline_file_io::lifecycle::TranscodeOutcome::Failed(error)) => {
                    self.discard_preview(pending.preview.as_ref()); self.message = Some(file_error(error));
                }
                IoCompletion::ResidentSpilled { captured: _, result } => {
                    self.message = Some(match result {
                        Ok(_) => "Resident migration is not attached; the current document was retained.".into(),
                        Err(error) => file_error(error),
                    });
                }
                IoCompletion::Open(Err(FileError::StreamingRequired)) => {
                    self.discard_preview(pending.preview.as_ref());
                    if let Some(path) = pending.open_path { self.open_paged(path); }
                }
                IoCompletion::Open(Err(error)) | IoCompletion::Save(Err(error)) => {
                    self.discard_preview(pending.preview.as_ref());
                    self.message = Some(file_error(error))
                }
            }
        }
        changed
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
        if !self.ensure_io() {
            return;
        }
        let request = IoRequest::OpenStreaming {
            path: path.clone(),
            bytes: self.bytes.clone(),
            history: self.history.clone(),
            resident_max_bytes: self.resident_max_bytes,
        };
        match self
            .io
            .as_ref()
            .unwrap()
            .submit(request, self.notify.clone())
        {
            Ok(receiver) => {
                self.pending_io.push(PendingIo {
                    receiver,
                    save: None, copy_only: false,
                    open_path: Some(path),
                    preview: None,
                    reload: None,
                });
                self.message = Some("Opening…".into());
            }
            Err(_) => {
                self.message =
                    Some("File queue is full. Try again after the pending operation.".into())
            }
        }
    }
    fn open_paged(&mut self, path: PathBuf) {
        let request = IoRequest::OpenPagedEncoded(bareline_file_io::lifecycle::PagedOpenRequest {
            path: path.clone(), bytes: self.bytes.clone(), history: self.history.clone(),
            cache: std::env::temp_dir().join("Bareline-transcodes"),
            options: bareline_file_io::codecs::disk::DiskOptions { temp_quota_bytes: self.transcode_quota_bytes, interpret: None },
            source_options: bareline_file_io::source::SourceOptions::default(),
        });
        self.submit_paged(request, path);
    }
    fn submit_paged(&mut self, request: IoRequest, path: PathBuf) {
        if !self.ensure_io() { return; }
        match self.io.as_ref().unwrap().submit(request, self.notify.clone()) {
            Ok(receiver) => { self.pending_io.push(PendingIo { receiver, save: None, copy_only: false, open_path: Some(path), preview: None, reload: None }); self.message = Some("Preparing paged text…".into()); }
            Err(request) => { if let IoRequest::ResumeTranscode { paused, .. } = *request { self.paused_transcode = Some(paused); } self.message = Some("File queue is full; retry opening or resuming.".into()); },
        }
    }
    pub fn restore_paged_recovery(&mut self, directory: PathBuf) {
        self.submit_paged(IoRequest::RestorePagedRecovery { directory: directory.clone(), bytes: self.bytes.clone(), history: self.history.clone() }, directory);
    }
    pub fn paused_transcode_info(&self) -> Option<(&std::path::Path, u64, u64, u64)> {
        let paused = self.paused_transcode.as_ref()?;
        if let bareline_file_io::codecs::disk::DiskError::Quota { used, required, limit } = paused.error { Some((&paused.path, used, required, limit)) } else { None }
    }
    pub fn resume_transcode(&mut self, quota_bytes: u64) {
        self.transcode_quota_bytes = quota_bytes;
        if let Some(paused) = self.paused_transcode.take() { let path = paused.path.clone(); self.submit_paged(IoRequest::ResumeTranscode { paused, temp_quota_bytes: quota_bytes }, path); }
    }
    pub fn path(&self, index: usize) -> Option<&std::path::Path> {
        if matches!(self.editors.get(index),Some(WorkspaceEditor::Paged(editor)) if editor.save_as_required) { return None; }
        self.files
            .get(index)
            .and_then(|f| f.as_ref().map(|f| f.path.as_path()))
    }
    pub fn path_loading(&self, path: &std::path::Path) -> bool {
        self.pending_io.iter().any(|pending| pending.open_path.as_deref() == Some(path))
    }
    pub fn fingerprint(&self, index: usize) -> Option<&Fingerprint> {
        self.files.get(index).and_then(|file| file.as_ref()).map(|file| &file.fingerprint)
    }
    pub fn reload(&mut self, index: usize, discard_confirmed: bool) -> Result<(), String> {
        let editor = self.editors.get(index).ok_or("Document is unavailable")?;
        if editor.busy() || editor.read_only() || (editor.dirty() && !discard_confirmed) { return Err("Confirm discard of current edits before reloading".into()); }
        let captured = editor.snapshot().clone();
        let path = self.path(index).ok_or("Save this document before reloading")?.to_path_buf();
        if self.path_loading(&path) { return Err("This file is already loading".into()); }
        if !self.ensure_io() { return Err("File service unavailable".into()); }
        let receiver = self.io.as_ref().unwrap().submit(IoRequest::OpenStreaming {path:path.clone(),bytes:self.bytes.clone(),history:self.history.clone(),resident_max_bytes:self.resident_max_bytes}, self.notify.clone()).map_err(|_| "File queue is full")?;
        self.pending_io.push(PendingIo {receiver,save:None,copy_only:false,open_path:Some(path),preview:None,reload:Some(captured)});
        self.message = Some("Reloading… current text remains available until complete.".into());
        Ok(())
    }
    pub fn reorder(&mut self, order: &[usize]) -> bool {
        if self.io_busy() || order.len() != self.editors.len() { return false; }
        let unique: std::collections::BTreeSet<_> = order.iter().copied().collect();
        if unique.len() != order.len() || unique.last().is_some_and(|last| *last >= order.len()) { return false; }
        let mut editors: Vec<_> = self.editors.drain(..).map(Some).collect();
        let mut files: Vec<_> = self.files.drain(..).map(Some).collect();
        let mut titles: Vec<_> = self.untitled_labels.drain(..).map(Some).collect();
        for index in order { self.editors.push(editors[*index].take().unwrap()); self.files.push(files[*index].take().unwrap()); self.untitled_labels.push(titles[*index].take().unwrap()); }
        self.last_drawn = None;
        self.find.clear_source();
        true
    }
    pub fn document_busy(&self, index: usize) -> bool {
        self.editors.get(index).is_some_and(|editor| editor.busy())
            || self.pending_io.iter().any(|pending| {
                pending
                    .save
                    .as_ref()
                    .is_some_and(|(target, _, _)| *target == index)
            })
    }
    pub fn close(
        &mut self,
        index: usize,
        discard: bool,
        renderer: &mut impl TextBackend,
    ) -> Result<(), CloseError> {
        let editor = self.editors.get(index).ok_or(CloseError::Missing)?;
        if self.document_busy(index) {
            return Err(CloseError::Busy);
        }
        if editor.dirty() && !discard {
            return Err(CloseError::Unsaved);
        }
        if editor.read_only() {
            let source = editor.snapshot().clone();
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
        self.closed.push((closed, file, label));
        if self.closed.len() > 10 { self.closed.remove(0); }
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
    pub fn scheduler(&self) -> &Scheduler { &self.scheduler }
    pub fn scheduler_and_editors(&mut self) -> (&Scheduler, &mut Vec<WorkspaceEditor>) { (&self.scheduler, &mut self.editors) }
    pub fn can_restore_closed(&self) -> bool { !self.closed.is_empty() }
    /// Reattach the retained model, history and selection without reopening its path.
    pub fn restore_last_closed(&mut self) -> Option<usize> {
        let (editor, file, label) = self.closed.pop()?;
        let index = self.editors.len();
        self.editors.push(editor);
        self.files.push(file);
        self.untitled_labels.push(label);
        self.last_drawn = None;
        Some(index)
    }
    pub fn io_busy(&self) -> bool {
        !self.pending_io.is_empty() || self.editors.iter().any(|editor| editor.paged() && editor.busy())
    }
    pub fn cancel_file_operations(&mut self) {
        self.paused_transcode = None;
        for pending in &self.pending_io {
            pending.receiver.cancel();
        }
        if self.io_busy() {
            self.message = Some("Cancelling file operations…".into());
        }
    }
    pub fn save(&mut self, index: usize, path: PathBuf) { self.save_internal(index, path, false); }
    pub fn save_copy(&mut self, index: usize, path: PathBuf) { self.save_internal(index, path, true); }
    /// Dirty documents in stable tab order; the native caller prompts for untitled paths.
    pub fn save_all_targets(&self) -> Vec<(usize, Option<PathBuf>)> {
        self.editors.iter().enumerate().filter(|(_, editor)| editor.dirty()).map(|(index, _)| (index, self.path(index).map(PathBuf::from))).collect()
    }
    fn save_internal(&mut self, index: usize, path: PathBuf, copy_only: bool) {
        if copy_only && self.path(index).is_some_and(|source| source == path) {
            self.message = Some("Save Copy needs a different destination from the document source.".into());
            return;
        }
        if let Some(WorkspaceEditor::Paged(editor)) = self.editors.get_mut(index) {
            let expected = (editor.path == path && !editor.save_as_required).then(|| editor.fingerprint.clone());
            self.message = match if copy_only { editor.save_copy(path, self.file_system.clone()) } else { editor.save(path, expected, self.file_system.clone()) } { Ok(()) => Some("Saving…".into()), Err(error) => Some(error) };
            return;
        }
        if !self.ensure_io() {
            return;
        }
        if self
            .pending_io
            .iter()
            .any(|p| p.save.as_ref().is_some_and(|(i, _, _)| *i == index))
        {
            self.message = Some("This document is already saving.".into());
            return;
        }
        let Some(editor) = self.editors.get(index) else {
            return;
        };
        if editor.busy() {
            self.message = Some("Wait for the pending edit before saving.".into());
            return;
        }
        if !editor.snapshot().is_complete() || (editor.read_only() && !copy_only) {
            self.message = Some("Document is not ready or is read only; saving is unavailable.".into());
            return;
        }
        let existing = self.files[index].as_ref().filter(|file| file.path == path);
        let expected = existing.filter(|_| !copy_only).map(|file| file.fingerprint.clone());
        let bom = self.files[index].as_ref().is_some_and(|file| file.bom);
        let request = if copy_only {
            IoRequest::SaveCopy { snapshot:editor.snapshot().clone(), target:path.clone(), source:self.files[index].as_ref().map(|file|file.path.clone()), bom, encoding:self.files[index].as_ref().and_then(|file|file.encoding.clone()) }
        } else if let Some(encoding) = self.files[index]
            .as_ref()
            .and_then(|file| file.encoding.clone())
        {
            IoRequest::SaveEncoded {
                snapshot: editor.snapshot().clone(),
                target: path.clone(),
                expected,
                bom,
                encoding,
            }
        } else {
            IoRequest::Save {
                snapshot: editor.snapshot().clone(),
                target: path.clone(),
                expected,
                bom,
            }
        };
        match self
            .io
            .as_ref()
            .unwrap()
            .submit(request, self.notify.clone())
        {
            Ok(receiver) => {
                self.pending_io.push(PendingIo {
                    receiver,
                    save: Some((index, path, bom)),
                    copy_only,
                    open_path: None,
                    preview: None,
                    reload: None,
                });
                self.message = Some("Saving…".into());
            }
            Err(_) => {
                self.message =
                    Some("File queue is full. Try again after the pending operation.".into())
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
    pub fn find_next(&mut self, active: usize, backwards: bool) {
        let Some(editor) = self.editors.get_mut(active) else {
            return;
        };
        if editor.busy() {
            return;
        }
        let at = if backwards {
            editor.selection.anchor.min(editor.selection.caret)
        } else {
            editor.selection.anchor.max(editor.selection.caret)
        };
        let found = match editor {
            WorkspaceEditor::Resident(resident) => self.find.next(resident.snapshot(), at, backwards),
            WorkspaceEditor::Paged(paged) => self.find.next_paged(paged.snapshot(), paged.viewport_start().0 + at, backwards),
        };
        if let Some(range) = found {
            match editor {
                WorkspaceEditor::Resident(resident) => {
                    resident.enqueue(Input::SetCaret(range.start.0, false));
                    resident.enqueue(Input::SetCaret(range.end.0, true));
                    resident.search_selection = true;
                }
                WorkspaceEditor::Paged(paged) => {
                    if let Err(error) = paged.restore_selection(range.start, range.end) { self.message = Some(error); }
                }
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
        editor.search_selection = true;
        Some(index)
    }
    pub fn replace(&mut self, active: usize, all: bool) {
        if self.pending_replace.is_some() {
            return;
        }
        let Some(editor) = self.editors.get(active) else {
            return;
        };
        if editor.busy() {
            self.message = Some("Wait for the pending edit before replacing.".into());
            return;
        }
        let selection =
            bareline_document::TextOffset(editor.selection.anchor.min(editor.selection.caret))
                ..bareline_document::TextOffset(
                    editor.selection.anchor.max(editor.selection.caret),
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
        self.pending_replace = None;
        self.find.cancel_search();
        self.message = Some("Search / replacement preparation cancelled.".into());
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
            let snapshots = self
                .editors
                .iter()
                .filter(|editor| !editor.read_only())
                .map(|editor| editor.snapshot().clone())
                .collect();
            self.search_panel
                .start(snapshots, self.search_panel.query(), self.notify.clone());
        }
        if self.last_drawn != Some(active) {
            if let Some(previous) = self.last_drawn.and_then(|i| self.editors.get_mut(i)) {
                previous.release_layouts(renderer);
            }
            self.last_drawn = Some(active);
        }
        let mut result = match self.editors.get_mut(active) {
            Some(editor) => {
                match editor {
                    WorkspaceEditor::Resident(resident) => self.find.refresh(resident.snapshot(), self.notify.clone()),
                    WorkspaceEditor::Paged(paged) => self.find.refresh_paged(paged.read_handle(), self.notify.clone()),
                }
                editor.top_inset = if self.find.open {
                    self.find.height()
                } else {
                    0.0
                };
                editor.bottom_inset = self.search_panel.height();
                let language = self
                    .files
                    .get(active)
                    .and_then(|file| file.as_ref())
                    .map_or(bareline_syntax::Language::PlainText, |file| {
                        bareline_syntax::Language::detect(&file.path)
                    });
                let definition = editor.udl.clone();
                let language = if definition.is_some() { bareline_syntax::Language::PlainText } else { editor.language_override.or(editor.detected_language).unwrap_or(language) };
                let label = definition.as_ref().map_or(language.label(), |d| d.name.as_str());
                editor.language = language;
                let result = editor.draw_styled(
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
                );
                if let Some(definition) = definition {
                    self.styling.refresh_udl(editor.snapshot(), definition, editor.visible_text.clone(), self.notify.clone());
                } else { self.styling.refresh_preferred(
                    editor.snapshot(),
                    language,
                    editor.visible_text.clone(),
                    self.notify.clone(),
                    editor.syntax_preference,
                ); }
                result
            }
            None => Ok(None),
        };
        if let Some(caret) = self.find.draw_with_theme(renderer, width, self.theme, ops)? {
            result = Ok(Some(caret));
        }
        let labels: Vec<_> = self
            .editors
            .iter()
            .zip(self.titles())
            .map(|(editor, title)| (editor.snapshot().clone(), title))
            .collect();
        if let Some(caret) = self
            .search_panel
            .draw(renderer, width, height, &labels, ops)?
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
        FileError::Encoding(error) => format!("Encoding operation was not applied: {error:?}"),
        FileError::Cancelled => "File operation cancelled.".into(),
        FileError::IncompleteSource => "File is still loading; wait before saving.".into(),
        FileError::StreamingRequired => {
            "This file exceeds the configured resident limit; reopen with paged storage."
                .into()
        }
        FileError::UnsupportedEncoding => {
            "This file needs an encoding that is not available in this build.".into()
        }
        FileError::Changed => {
            "The file changed during the operation. Your edits remain in memory.".into()
        }
        FileError::Conflict { staged } => format!(
            "File changed; destination was preserved. Your staged copy is at {}",
            staged.display()
        ),
        FileError::Commit { staged, error } => format!(
            "Save could not replace the destination ({error}). Staged copy: {}",
            staged.display()
        ),
        FileError::Io(error) => format!("File operation failed: {error}"),
        FileError::Budget => "Document memory budget reached.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct PagedFileSystem;
    impl LocalFileSystem for PagedFileSystem {
        fn guard_directory(&self, _: &std::path::Path) -> std::io::Result<std::sync::Arc<dyn Send + Sync>> { Ok(std::sync::Arc::new(())) }
        fn available_space(&self, _: &std::path::Path) -> std::io::Result<u64> { Ok(u64::MAX) }
        fn open_sealed_read(&self, path: &std::path::Path) -> std::io::Result<std::fs::File> { std::fs::File::open(path) }
        fn identity(&self, file: &std::fs::File) -> std::io::Result<bareline_platform::FileIdentity> {
            let meta = file.metadata()?;
            Ok(bareline_platform::FileIdentity { volume: 1, file: 1, length: meta.len(), modified: meta.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64 })
        }
        fn validate_target(&self, _: &std::path::Path) -> std::io::Result<()> { Ok(()) }
        fn commit(&self, staged: &std::path::Path, target: &std::path::Path, _: bool) -> std::io::Result<()> { if target.exists() { std::fs::remove_file(target)?; } std::fs::rename(staged, target) }
    }
    #[test]
    fn paged_workspace_edits_undoes_navigates_and_saves() {
        let directory = std::env::temp_dir().join(format!("bareline-paged-ui-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
                if !workspace.io_busy() { break; }
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
        assert_eq!(workspace.path(0), Some(saved.as_path()));
        assert!(!workspace.editors[0].dirty());
        drop(workspace);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn save_copy_and_restore_closed_preserve_document_identity_and_history() {
        fn settle(workspace:&mut Workspace) {
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
            loop { workspace.pump(); if !workspace.io_busy() && !workspace.editors.iter().any(|editor|editor.busy()) { break; } assert!(std::time::Instant::now()<deadline,"{:?}",workspace.message); std::thread::sleep(std::time::Duration::from_millis(2)); }
        }
        let root=std::env::temp_dir().join(format!("bareline-copy-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&root).unwrap();
        for paged in [false,true] {
            let original=root.join(if paged {"paged.txt"} else {"resident.txt"}); std::fs::write(&original,"abcdef").unwrap();
            let mut workspace=Workspace::new(Arc::new(||{}),Arc::new(PagedFileSystem)).unwrap(); if paged {workspace.resident_max_bytes=4;}
            workspace.open(original.clone());settle(&mut workspace);
            workspace.editors[0].enqueue(Input::Insert("X".into()));settle(&mut workspace);
            let copy=root.join(if paged {"paged-copy.txt"} else {"resident-copy.txt"});workspace.save_copy(0,copy.clone());settle(&mut workspace);
            assert_eq!(std::fs::read_to_string(copy).unwrap(),"Xabcdef");assert_eq!(std::fs::read_to_string(&original).unwrap(),"abcdef");assert!(workspace.editors[0].dirty());assert_eq!(workspace.path(0),Some(original.as_path()));
            let alias=root.join(if paged {"paged-alias.txt"} else {"resident-alias.txt"});std::fs::hard_link(&original,&alias).unwrap();workspace.save_copy(0,alias);settle(&mut workspace);assert_eq!(std::fs::read_to_string(&original).unwrap(),"abcdef");assert!(workspace.editors[0].dirty());
            let selection=workspace.editors[0].selection;
            let mut renderer=bareline_renderer_recording::RecordingBackend::default();workspace.close(0,true,&mut renderer).unwrap();assert!(workspace.can_restore_closed());assert_eq!(workspace.restore_last_closed(),Some(0));assert_eq!(workspace.editors[0].selection,selection);
            workspace.editors[0].enqueue(Input::Undo);settle(&mut workspace);assert!(!workspace.editors[0].dirty());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn paged_recovery_restart_preserves_opaque_undo_provenance_and_refuses_stale_root() {
        let root=std::env::temp_dir().join(format!("bareline-paged-recovery-ui-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&root).unwrap();
        let original=root.join("original.txt"); let saved=root.join("restored.txt");
        let raw=[255,254,65,0,0,216,66,0]; std::fs::write(&original,raw).unwrap();
        let mut workspace=Workspace::new(Arc::new(||{}),Arc::new(PagedFileSystem)).unwrap();
        workspace.resident_max_bytes=4; workspace.recovery_root=Some(root.join("recovery")); workspace.open(original.clone());
        fn settle(workspace:&mut Workspace) { let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10); loop {workspace.pump();if !workspace.io_busy(){break;}assert!(std::time::Instant::now()<deadline,"{:?}",workspace.message);std::thread::sleep(std::time::Duration::from_millis(2));} }
        settle(&mut workspace);
        workspace.editors[0].enqueue(Input::SetCaret(1,false));
        workspace.editors[0].enqueue(Input::SetCaret(4,true));
        workspace.editors[0].enqueue(Input::Insert("X".into())); settle(&mut workspace);
        let directory=match &workspace.editors[0] {WorkspaceEditor::Paged(editor)=>editor.recovery_status().directory.unwrap(),_=>panic!("expected paged")};
        let earlier_root=std::fs::read(directory.join("paged-root.json")).unwrap();
        workspace.editors[0].enqueue(Input::Undo);settle(&mut workspace);
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
        loop { let status=match &workspace.editors[0]{WorkspaceEditor::Paged(editor)=>editor.recovery_status(),_=>unreachable!()}; assert!(status.error.is_none(),"{:?}",status.error); if status.complete {assert_eq!(status.durable.unwrap().revision,2);break;} assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(2)); }
        drop(workspace); std::fs::remove_file(&original).unwrap();
        let mut restored=Workspace::new(Arc::new(||{}),Arc::new(PagedFileSystem)).unwrap();restored.restore_paged_recovery(directory.clone());settle(&mut restored);
        assert_eq!(restored.editors.len(),1,"{:?}",restored.message);assert!(restored.editors[0].dirty());assert!(restored.path(0).is_none());
        restored.save(0,saved.clone());settle(&mut restored);assert_eq!(std::fs::read(&saved).unwrap(),raw);
        drop(restored);
        let journal=std::fs::read(directory.join("journal.bin")).unwrap();let mut corrupt=journal.clone();*corrupt.last_mut().unwrap()^=1;std::fs::write(directory.join("journal.bin"),corrupt).unwrap();
        let mut prefix=Workspace::new(Arc::new(||{}),Arc::new(PagedFileSystem)).unwrap();prefix.restore_paged_recovery(directory.clone());settle(&mut prefix);assert_eq!(prefix.editors.len(),1,"{:?}",prefix.message);let prefix_path=root.join("valid-prefix.txt");prefix.save(0,prefix_path.clone());settle(&mut prefix);assert_eq!(std::fs::read(prefix_path).unwrap(),[255,254,65,0,88,0,66,0]);drop(prefix);std::fs::write(directory.join("journal.bin"),journal).unwrap();
        std::fs::write(directory.join("paged-root.json"),earlier_root).unwrap();
        let mut stale=Workspace::new(Arc::new(||{}),Arc::new(PagedFileSystem)).unwrap();stale.restore_paged_recovery(directory);settle(&mut stale);
        assert!(stale.editors.is_empty());assert!(stale.message.as_deref().is_some_and(|text|text.contains("stale")),"{:?}",stale.message);
        drop(stale);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn resident_and_untitled_automatic_recovery_restore_current_text() {
        let root=std::env::temp_dir().join(format!("bareline-resident-recovery-ui-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));std::fs::create_dir(&root).unwrap();
        fn settle(workspace:&mut Workspace){let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);loop{workspace.pump();if !workspace.io_busy()&&!workspace.editors.iter().any(|editor|editor.busy()){break;}assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(2));}}
        for resident in [false,true] {
            let mut workspace=Workspace::new(Arc::new(||{}),Arc::new(PagedFileSystem)).unwrap();workspace.recovery_root=Some(root.join(if resident{"resident"}else{"untitled"}));
            let original=root.join("resident.txt");
            if resident {std::fs::write(&original,b"original").unwrap();workspace.open(original.clone());settle(&mut workspace);}else{workspace.new_document().unwrap();workspace.pump();}
            workspace.editors[0].enqueue(Input::Insert("draft ".into()));settle(&mut workspace);
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
            let directory=loop{workspace.pump();let status=workspace.editors[0].recovery_status();assert!(status.error.is_none(),"{:?}",status.error);if status.complete{assert!(status.durable.is_some());break status.directory.unwrap();}assert!(std::time::Instant::now()<deadline,"recovery never completed");std::thread::sleep(std::time::Duration::from_millis(2));};
            drop(workspace);if resident{std::fs::remove_file(&original).unwrap();}
            let mut restored=Workspace::new(Arc::new(||{}),Arc::new(PagedFileSystem)).unwrap();restored.restore_paged_recovery(directory);settle(&mut restored);assert_eq!(restored.editors.len(),1,"{:?}",restored.message);
            let saved=root.join(if resident{"resident-restored.txt"}else{"untitled-restored.txt"});restored.save(0,saved.clone());settle(&mut restored);assert_eq!(std::fs::read_to_string(saved).unwrap(),if resident{"draft original"}else{"draft "});drop(restored);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn resident_clean_undo_and_save_retire_obsolete_recovery() {
        let root=std::env::temp_dir().join(format!("bareline-clean-recovery-ui-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));std::fs::create_dir(&root).unwrap();
        let mut workspace=Workspace::new(Arc::new(||{}),Arc::new(PagedFileSystem)).unwrap();workspace.recovery_root=Some(root.join("recovery"));workspace.new_document().unwrap();workspace.pump();
        fn wait(workspace:&mut Workspace,empty:bool)->Option<PathBuf>{let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);loop{workspace.pump();let state=workspace.editors[0].recovery_status();assert!(state.error.is_none(),"{:?}",state.error);if (empty&&state.directory.is_none())||(!empty&&state.complete){return state.directory;}assert!(std::time::Instant::now()<deadline,"recovery state did not settle");std::thread::sleep(std::time::Duration::from_millis(2));}}
        workspace.editors[0].enqueue(Input::Insert("abandoned".into()));let first=wait(&mut workspace,false).unwrap();workspace.editors[0].enqueue(Input::Undo);wait(&mut workspace,true);
        assert_eq!(bareline_file_io::recovery::inspect(&first,&Default::default()).unwrap().status,bareline_file_io::recovery::RecoveryStatus::Discarded);
        workspace.editors[0].enqueue(Input::Insert("saved".into()));let second=wait(&mut workspace,false).unwrap();workspace.save(0,root.join("saved.txt"));
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);while workspace.io_busy(){workspace.pump();assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(2));}wait(&mut workspace,true);
        assert_eq!(bareline_file_io::recovery::inspect(&second,&Default::default()).unwrap().status,bareline_file_io::recovery::RecoveryStatus::Discarded);
        drop(workspace);std::fs::remove_dir_all(root).unwrap();
    }
    struct RetirementFileSystem(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl LocalFileSystem for RetirementFileSystem {
        fn available_space(&self,path:&std::path::Path)->std::io::Result<u64>{PagedFileSystem.available_space(path)}
        fn guard_directory(&self,path:&std::path::Path)->std::io::Result<Arc<dyn Send+Sync>>{PagedFileSystem.guard_directory(path)}
        fn open_sealed_read(&self,path:&std::path::Path)->std::io::Result<std::fs::File>{PagedFileSystem.open_sealed_read(path)}
        fn identity(&self,file:&std::fs::File)->std::io::Result<bareline_platform::FileIdentity>{PagedFileSystem.identity(file)}
        fn validate_target(&self,path:&std::path::Path)->std::io::Result<()>{PagedFileSystem.validate_target(path)}
        fn commit(&self,staged:&std::path::Path,target:&std::path::Path,existed:bool)->std::io::Result<()>{if self.0.load(std::sync::atomic::Ordering::SeqCst)&&target.file_name().is_some_and(|name|name=="retired.json"){return Err(std::io::Error::other("injected retirement failure"));}PagedFileSystem.commit(staged,target,existed)}
    }
    #[test]
    fn paged_recovery_failed_retirement_is_retryable() {
        let root=std::env::temp_dir().join(format!("bareline-retire-retry-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));std::fs::create_dir(&root).unwrap();let original=root.join("original.txt");std::fs::write(&original,"abcde").unwrap();
        let deny=Arc::new(std::sync::atomic::AtomicBool::new(false));let mut workspace=Workspace::new(Arc::new(||{}),Arc::new(RetirementFileSystem(deny.clone()))).unwrap();workspace.resident_max_bytes=4;workspace.recovery_root=Some(root.join("recovery"));workspace.open(original);
        fn settle(workspace:&mut Workspace){let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);loop{workspace.pump();if !workspace.io_busy(){break;}assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(2));}}
        settle(&mut workspace);workspace.editors[0].enqueue(Input::Insert("X".into()));settle(&mut workspace);
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);let directory=loop{let status=workspace.editors[0].recovery_status();assert!(status.error.is_none(),"{:?}",status.error);if status.complete{break status.directory.unwrap();}assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(2));};
        deny.store(true,std::sync::atomic::Ordering::SeqCst);let saved=root.join("saved.txt");workspace.save(0,saved.clone());settle(&mut workspace);assert_eq!(std::fs::read_to_string(saved).unwrap(),"Xabcde");assert!(workspace.editors[0].recovery_status().error.is_some());assert_ne!(bareline_file_io::recovery::inspect(&directory,&Default::default()).unwrap().status,bareline_file_io::recovery::RecoveryStatus::Discarded);
        deny.store(false,std::sync::atomic::Ordering::SeqCst);workspace.editors[0].retry_recovery().unwrap();settle(&mut workspace);assert!(workspace.editors[0].recovery_status().error.is_none());assert_eq!(bareline_file_io::recovery::inspect(&directory,&Default::default()).unwrap().status,bareline_file_io::recovery::RecoveryStatus::Discarded);
        drop(workspace);std::fs::remove_dir_all(root).unwrap();
    }
    struct StreamingFileSystem {
        calls: std::sync::atomic::AtomicUsize,
        gate: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl LocalFileSystem for StreamingFileSystem {
        fn identity(
            &self,
            file: &std::fs::File,
        ) -> std::io::Result<bareline_platform::FileIdentity> {
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
        received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        workspace.pump();
        assert_eq!(workspace.editors.len(), 1);
        let prefix = workspace.editors[0].snapshot().clone();
        assert!(!prefix.is_complete());
        workspace.editors[0].enqueue(Input::Insert("forbidden".into()));
        workspace.editors[0].enqueue(Input::Undo);
        assert_eq!(workspace.editors[0].snapshot().revision, prefix.revision);
        assert!(!workspace.editors[0].busy());
        workspace.save(0, path.clone());
        assert_eq!(workspace.pending_io.len(), 1);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut ops = Vec::new();
        workspace
            .draw(0, &mut renderer, 1100.0, 700.0, &mut ops)
            .unwrap();
        assert!(
            ops.iter()
                .any(|op| matches!(op, DrawOp::Text { text, .. } if text.contains("indexing…")))
        );
        release.send(()).unwrap();
        while workspace.io_busy() {
            received
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            workspace.pump();
        }
        assert_eq!(workspace.editors.len(), 1);
        assert!(workspace.editors[0].snapshot().is_complete());
        assert!(!workspace.editors[0].read_only());
        assert!(workspace.editors[0].snapshot().same_document(&prefix));
        assert_eq!(workspace.editors[0].snapshot().len(), content.len());
        assert_eq!(workspace.path(0), Some(path.as_path()));
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
            notified
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            workspace.pump();
        }
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        assert_eq!(
            workspace.close(0, false, &mut renderer),
            Err(CloseError::Unsaved)
        );
        let saving = workspace.editors[2].snapshot().clone();
        workspace.save(2, PathBuf::from("unused-fixture.txt"));
        assert_eq!(
            workspace.close(2, true, &mut renderer),
            Err(CloseError::Busy)
        );
        workspace.close(0, true, &mut renderer).unwrap();
        assert_eq!(workspace.titles()[0], "Untitled 2");
        assert!(workspace.editors[1].snapshot().same_document(&saving));
        assert_eq!(workspace.pending_io[0].save.as_ref().unwrap().0, 1);
        assert_eq!(
            workspace.close(1, true, &mut renderer),
            Err(CloseError::Busy)
        );
        release.send(()).unwrap();
        while workspace.io_busy() {
            notified
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
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
            notified
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
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
        assert_eq!(workspace.editors[0].top_inset, crate::find::HEIGHT);
        while workspace.find.status == "Searching…" {
            notified
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            workspace.pump();
        }
        workspace.editors[0].enqueue(Input::SetCaret(0, false));
        workspace.find_next(0, false);
        assert_eq!(
            (
                workspace.editors[0].selection.anchor,
                workspace.editors[0].selection.caret
            ),
            (4, 11)
        );
        assert_eq!(workspace.editors[0].snapshot().revision, revision);
        workspace.find_next(0, false);
        assert_eq!(
            (
                workspace.editors[0].selection.anchor,
                workspace.editors[0].selection.caret
            ),
            (12, 19)
        );
        workspace.find.hide();
        workspace.find_next(0, false);
        assert_eq!(workspace.editors[0].selection.anchor, 4);
        assert_eq!(workspace.editors[0].selection.caret, 11);
        operations.clear();
        workspace
            .draw(0, &mut renderer, 1100.0, 700.0, &mut operations)
            .unwrap();
        assert_eq!(workspace.editors[0].top_inset, 0.0);
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
                    notified
                        .recv_timeout(std::time::Duration::from_secs(3))
                        .unwrap();
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
        assert_eq!(
            workspace.editors[0].snapshot().len(),
            "Straße STRASSE".len()
        );
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
}
