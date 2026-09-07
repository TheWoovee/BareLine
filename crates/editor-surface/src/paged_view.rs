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
    RetryRecovery,
    Read(usize),
    Edit {
        range: std::ops::Range<TextOffset>,
        insert: String,
    },
    Undo,
    Redo,
    Save {
        target: PathBuf,
        expected: Option<Fingerprint>,
        platform: Arc<dyn LocalFileSystem>,
    },
}
struct Completed {
    can_undo: bool,
    can_redo: bool,
    snapshot: PagedSnapshot,
    window: Result<TextWindow, String>,
    path: PathBuf,
    caret: usize,
    saved: Option<Fingerprint>,
}
pub struct PagedEditorSurface {
    pub surface: EditorSurface,
    actor: Arc<Mutex<Box<PagedOpened>>>,
    snapshot: PagedSnapshot,
    saved_state: Option<ContentStateId>,
    recovery_config: Option<(PathBuf, Arc<dyn LocalFileSystem>)>,
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
    restoring_selection: Option<(usize, usize)>,
    pub fingerprint: Fingerprint,
    pub path: PathBuf,
    pub error: Option<String>,
}
impl PagedEditorSurface {
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
        surface.encoding_label = format!("{:?}", opened.transcoded.store.state.save_target);
        let mut view = Self {
            saved_state: opened
                .recovery_origin
                .is_none()
                .then_some(snapshot.content_state),
            save_as_required: opened.recovery_origin.is_some(),
            can_undo: false,
            can_redo: false,
            recovery_config: None,
            recovery: Arc::new(Mutex::new(None)),
            recovery_status: Arc::new(Mutex::new(Default::default())),
            failed_retirements: Arc::new(Mutex::new(Vec::new())),
            snapshot,
            fingerprint: opened.fingerprint.clone(),
            path: opened.path.clone(),
            surface,
            actor: Arc::new(Mutex::new(opened)),
            budget,
            pending: None,
            pending_input: None,
            cancellation: Cancellation::default(),
            notify,
            viewport_start: 0,
            viewport_valid: false,
            restoring_selection: None,
            error: None,
        };
        view.request_viewport(TextOffset(0))?;
        Ok(view)
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
        self.pending.is_some()
    }
    pub fn dirty(&self) -> bool {
        Some(self.snapshot.content_state) != self.saved_state
    }
    pub fn viewport_start(&self) -> TextOffset {
        TextOffset(self.viewport_start)
    }
    pub fn restore_selection(
        &mut self,
        anchor: TextOffset,
        caret: TextOffset,
    ) -> Result<(), String> {
        let anchor = anchor.0.min(self.snapshot.len());
        let caret = caret.0.min(self.snapshot.len());
        if anchor.abs_diff(caret) > WINDOW.saturating_sub(8) {
            return Err("Selection exceeds the bounded paged viewport.".into());
        }
        self.request_viewport(TextOffset(anchor.min(caret).saturating_sub(4)))?;
        self.restoring_selection = Some((anchor, caret));
        Ok(())
    }
    pub fn request_viewport(&mut self, start: TextOffset) -> Result<(), String> {
        self.submit(Action::Read(start.0.min(self.snapshot.len())))
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
            target,
            expected,
            platform,
        })
    }
    pub fn enqueue(&mut self, input: Input) {
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
        let acknowledged = input.clone();
        let selected = self.surface.selection.range();
        let action = match input {
            Input::Insert(insert) => Some(Action::Edit {
                range: TextOffset(self.viewport_start + selected.start)
                    ..TextOffset(self.viewport_start + selected.end),
                insert,
            }),
            Input::Backspace | Input::Delete => {
                let backward = matches!(input, Input::Backspace);
                let range = if !selected.is_empty() {
                    selected
                } else if backward {
                    self.surface
                        .previous_grapheme(self.surface.selection.caret)
                        .map(|start| start..self.surface.selection.caret)
                        .unwrap_or(selected)
                } else {
                    self.surface
                        .next_grapheme(self.surface.selection.caret)
                        .map(|end| self.surface.selection.caret..end)
                        .unwrap_or(selected)
                };
                Some(Action::Edit {
                    range: TextOffset(self.viewport_start + range.start)
                        ..TextOffset(self.viewport_start + range.end),
                    insert: String::new(),
                })
            }
            Input::Undo => Some(Action::Undo),
            Input::Redo => Some(Action::Redo),
            navigation => {
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
    fn submit(&mut self, action: Action) -> Result<(), String> {
        if self.busy() {
            return Err("A paged operation is already pending.".into());
        }
        let actor = self.actor.clone();
        let recovery = self.recovery.clone();
        let recovery_config = self.recovery_config.clone();
        let recovery_status = self.recovery_status.clone();
        let failed_retirements = self.failed_retirements.clone();
        let saved_state = self.saved_state;
        let budget = self.budget.clone();
        let cancellation = self.cancellation.clone();
        let notify = self.notify.clone();
        let revision = self.snapshot.revision;
        let current_start = self.viewport_start;
        let current_caret = current_start + self.surface.selection.caret;
        let (sender, receiver) = mpsc::sync_channel(1);
        worker()
            .try_send(Box::new(move || {
                let result = (|| {
                    cancellation.check().map_err(|error| format!("{error:?}"))?;
                    let mut opened = actor.lock().map_err(|_| "Paged actor stopped.")?;
                    if opened.transcoded.document.snapshot().revision != revision {
                        return Err("Document changed; retry the operation.".into());
                    }
                    let mut start = current_start;
                    let mut caret = current_caret;
                    let mut saved = None;
                    let baseline = opened.transcoded.document.snapshot();
                    let mut retry_recovery = false;
                    let mut recovery_edits = Vec::new();
                    match action {
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
                        Action::Edit { range, insert } => {
                            if insert.len() > WINDOW || range.end.0 - range.start.0 > WINDOW {
                                return Err("Edit exceeds the bounded viewport budget.".into());
                            }
                            let snapshot = opened.transcoded.document.snapshot();
                            let window = read_window(
                                &mut opened,
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
                        Action::Undo => {
                            recovery_edits = opened
                                .transcoded
                                .document
                                .history_delta(true)
                                .map_err(|e| format!("{e:?}"))?
                                .into_iter()
                                .map(|edit| bareline_file_io::recovery::RecoveryEdit {
                                    offset: edit.range.start.0 as u64,
                                    removed: edit.removed.into_bytes(),
                                    inserted: edit.inserted.into_bytes(),
                                })
                                .collect();
                            opened
                                .transcoded
                                .document
                                .undo()
                                .map_err(|error| format!("{error:?}"))?;
                        }
                        Action::Redo => {
                            recovery_edits = opened
                                .transcoded
                                .document
                                .history_delta(false)
                                .map_err(|e| format!("{e:?}"))?
                                .into_iter()
                                .map(|edit| bareline_file_io::recovery::RecoveryEdit {
                                    offset: edit.range.start.0 as u64,
                                    removed: edit.removed.into_bytes(),
                                    inserted: edit.inserted.into_bytes(),
                                })
                                .collect();
                            opened
                                .transcoded
                                .document
                                .redo()
                                .map_err(|error| format!("{error:?}"))?;
                        }
                        Action::Save {
                            target,
                            expected,
                            platform,
                        } => {
                            let snapshot = opened.transcoded.document.snapshot();
                            let policy = PagedSavePolicy {
                                store: opened.transcoded.store.clone(),
                                generation: opened.transcoded.source.source().generation(),
                                encoding: opened.transcoded.store.state.save_target,
                                bom: opened.transcoded.store.state.bom,
                            };
                            let result = save_paged_cancellable(
                                snapshot,
                                &target,
                                expected.as_ref(),
                                &policy,
                                platform.as_ref(),
                                &cancellation,
                            )
                            .map_err(|error| format!("{error:?}"))?;
                            opened.fingerprint = result.fingerprint.clone();
                            opened.path = target;
                            saved = Some(result.fingerprint);
                        }
                    }
                    let snapshot = opened.transcoded.document.snapshot();
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
                    if !recovery_edits.is_empty() {
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
                        &snapshot,
                        start,
                        WINDOW,
                        &budget,
                        &cancellation,
                    );
                    Ok(Completed {
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
        Ok(())
    }
    pub fn pump(&mut self) -> bool {
        let Some(receiver) = &self.pending else {
            return false;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("Paged worker stopped.".into()),
        };
        self.pending = None;
        match result {
            Ok(completed) => {
                self.can_undo = completed.can_undo;
                self.can_redo = completed.can_redo;
                if let Some(input) = self.pending_input.take() {
                    self.surface.acknowledge(input);
                }
                self.viewport_valid = false;
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
                        let mut caret = completed
                            .caret
                            .saturating_sub(self.viewport_start)
                            .min(self.surface.snapshot.len());
                        while !self.surface.snapshot.is_boundary(TextOffset(caret)) {
                            caret -= 1;
                        }
                        self.surface.selection = Selection {
                            anchor: caret,
                            caret,
                        };
                        if let Some((anchor, caret)) = self.restoring_selection.take() {
                            let local = |offset: usize| {
                                offset
                                    .saturating_sub(self.viewport_start)
                                    .min(self.surface.snapshot.len())
                            };
                            let (anchor, caret) = (local(anchor), local(caret));
                            if self.surface.snapshot.is_boundary(TextOffset(anchor))
                                && self.surface.snapshot.is_boundary(TextOffset(caret))
                            {
                                self.surface.selection = Selection { anchor, caret };
                            }
                        }
                        self.surface.selections = self.surface.selection.into();
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
                self.pending_input = None;
                self.error = Some(error);
            }
        }
        true
    }
}
impl Drop for PagedEditorSurface {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
fn read_window(
    opened: &mut PagedOpened,
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
            WindowPoll::Pending(ticket) => opened
                .transcoded
                .source
                .read_page(ticket)
                .map_err(|error| format!("{error:?}"))?,
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
