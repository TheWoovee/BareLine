// SPDX-License-Identifier: MPL-2.0
//! Native Shell consumer of the bounded session worker and restore queue.
use super::*;
use bareline_app::{
    session_service::{SessionCompletion, SessionRequest, SessionService, SessionTicket},
    session_ui::{RestoreQueue, RestoreState},
};
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_file_io::session::{SessionDocument, SessionManifest, SessionTab, ViewState};
use bareline_platform::{SerializedPath, TrustedRead};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, mpsc::TryRecvError},
};

struct Restoring {
    guard: TrustedRead,
    tabs: Vec<SessionTab>,
}
enum CapturedDocument {
    Resident(DocumentSnapshot),
    Paged(bareline_document::paged::PagedSnapshot),
}
impl CapturedDocument {
    fn new(editor: &bareline_app::workspace::WorkspaceEditor) -> Self {
        match editor {
            bareline_app::workspace::WorkspaceEditor::Resident(editor) => {
                Self::Resident(editor.snapshot().clone())
            }
            bareline_app::workspace::WorkspaceEditor::Paged(editor) => {
                Self::Paged(editor.snapshot().clone())
            }
        }
    }
    fn same_editor(&self, editor: &bareline_app::workspace::WorkspaceEditor) -> bool {
        match (self, editor) {
            (
                Self::Resident(snapshot),
                bareline_app::workspace::WorkspaceEditor::Resident(editor),
            ) => snapshot.same_document(editor.snapshot()),
            (Self::Paged(snapshot), bareline_app::workspace::WorkspaceEditor::Paged(editor)) => {
                snapshot.same_document(editor.snapshot())
            }
            _ => false,
        }
    }
    fn same_state(&self, editor: &bareline_app::workspace::WorkspaceEditor) -> bool {
        match (self, editor) {
            (
                Self::Resident(snapshot),
                bareline_app::workspace::WorkspaceEditor::Resident(editor),
            ) => snapshot.content_state == editor.snapshot().content_state,
            (Self::Paged(snapshot), bareline_app::workspace::WorkspaceEditor::Paged(editor)) => {
                snapshot.content_state == editor.snapshot().content_state
            }
            _ => false,
        }
    }
}
struct Restored {
    tab: SessionTab,
    snapshot: CapturedDocument,
}
pub(super) struct SessionRuntime {
    path: Option<PathBuf>,
    restore: bool,
    started: bool,
    service: Option<SessionService>,
    load: Option<SessionTicket>,
    resolve: Option<SessionTicket>,
    save: Option<SessionTicket>,
    queue: Option<RestoreQueue>,
    guards: HashMap<u64, TrustedRead>,
    opening: HashMap<u64, Restoring>,
    restored: Vec<Restored>,
    exit_requested: bool,
    exit_snapshots: Vec<CapturedDocument>,
    exit_failed: bool,
    finalized: bool,
}
impl Default for SessionRuntime {
    fn default() -> Self {
        Self {
            path: std::env::var_os("APPDATA")
                .map(|root| PathBuf::from(root).join("Bareline/session.json")),
            restore: true,
            started: false,
            service: None,
            load: None,
            resolve: None,
            save: None,
            queue: None,
            guards: HashMap::new(),
            opening: HashMap::new(),
            restored: Vec::new(),
            exit_requested: false,
            exit_snapshots: Vec::new(),
            exit_failed: false,
            finalized: false,
        }
    }
}
impl SessionRuntime {
    pub(super) fn configure(&mut self, path: Option<PathBuf>, restore: bool) {
        self.path = path;
        self.restore = restore;
    }
    pub(super) fn startup_pending(&self) -> bool {
        self.load.is_some() || self.queue.as_ref().is_some_and(RestoreQueue::pending)
    }
    pub(super) fn closing(&self) -> bool {
        self.exit_requested
    }
    fn service(&mut self, notify: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<&SessionService> {
        if self.service.is_none() {
            self.service = Some(SessionService::new(
                Arc::new(bareline_platform_windows::WindowsFileSystem),
                notify,
            )?);
        }
        Ok(self.service.as_ref().unwrap())
    }
}
impl Shell {
    pub(super) fn session_first_frame(&mut self, _el: &ActiveEventLoop) {
        if !self.first_frame || self.session.started {
            return;
        }
        self.session.started = true;
        if self.smoke
            || self.perf
            || self.prototype.is_some()
            || !self.startup_paths.is_empty()
            || !self.session.restore
        {
            return;
        }
        let Some(path) = self.session.path.clone() else {
            return;
        };
        self.ledger.record(StartupAction::SpawnWorker);
        match self
            .session
            .service(self.notify.clone())
            .and_then(|service| service.submit(SessionRequest::Load { path }))
        {
            Ok(ticket) => self.session.load = Some(ticket),
            Err(error) => self.session_message(format!("Session restore unavailable: {error}")),
        }
    }
    fn session_message(&mut self, message: String) {
        if let Some(workspace) = &mut self.workspace {
            workspace.message = Some(message);
        } else {
            eprintln!("event=session message={message}");
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    pub(super) fn session_pump(&mut self, el: &ActiveEventLoop) {
        if !self.first_frame {
            return;
        }
        let loaded = self
            .session
            .load
            .as_ref()
            .and_then(|ticket| match ticket.try_recv() {
                Err(TryRecvError::Empty) => None,
                result => Some(result),
            });
        if let Some(result) = loaded {
            self.session.load = None;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            match result {
                Ok(SessionCompletion::Loaded(Ok(loaded))) => {
                    let diagnostic_warning=(!loaded.diagnostics.is_empty()).then(||format!("{}{}",if loaded.recovered_previous {"Recovered the previous session generation. "}else{""},loaded.diagnostics.summary()));
                    if !loaded.manifest.recent.is_empty() && self.ensure_workspace(el) {
                        self.workspace.as_mut().unwrap().restore_recent_paths(&loaded.manifest.recent);
                    }
                    if !loaded.manifest.documents.is_empty() && self.ensure_workspace(el) {
                        let warning = loaded.recovered_previous;
                        match RestoreQueue::new(loaded.manifest) {
                            Ok(mut queue) => {
                                queue.first_frame_presented();
                                let untitled: Vec<_> = queue
                                    .manifest()
                                    .documents
                                    .iter()
                                    .filter(|doc| doc.path.is_none())
                                    .map(|doc| doc.id)
                                    .collect();
                                for id in untitled {
                                    let active = queue.manifest().active_tab;
                                    let tab = queue
                                        .manifest()
                                        .tabs
                                        .iter()
                                        .filter(|tab| tab.document_id == id)
                                        .find(|tab| Some(tab.id) == active)
                                        .or_else(|| {
                                            queue
                                                .manifest()
                                                .tabs
                                                .iter()
                                                .find(|tab| tab.document_id == id)
                                        })
                                        .cloned();
                                    if let Some(tab) = tab {
                                        let workspace = self.workspace.as_mut().unwrap();
                                        if workspace.new_document().is_ok() {
                                            let index = workspace.editors.len() - 1;
                                            apply_view(workspace, index, &tab.view);
                                            self.session.restored.push(Restored {
                                                tab,
                                                snapshot: CapturedDocument::new(
                                                    &workspace.editors[index],
                                                ),
                                            });
                                        }
                                    } else {
                                        // The document survived but its tab metadata did not.
                                        let _ = self.workspace.as_mut().unwrap().new_document();
                                    }
                                }
                                self.session.queue = Some(queue);
                                if warning {
                                    self.session_message("Recovered the previous session manifest; the latest was unavailable.".into());
                                }
                            }
                            Err(error) => {
                                self.session_message(format!("Session restore refused: {error}"))
                            }
                        }
                    }
                    if let Some(warning)=diagnostic_warning {self.session_message(warning);}
                }
                Ok(SessionCompletion::Loaded(Err(error)))
                    if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(SessionCompletion::Loaded(Err(error))) => {
                    self.session_message(format!("Session restore unavailable: {error}"))
                }
                _ => {
                    self.session_message("Session worker stopped before restore completed.".into())
                }
            }
        }
        let resolved = self
            .session
            .resolve
            .as_ref()
            .and_then(|ticket| match ticket.try_recv() {
                Err(TryRecvError::Empty) => None,
                result => Some(result),
            });
        if let Some(result) = resolved {
            self.session.resolve = None;
            match result {
                Ok(SessionCompletion::Resolved(results)) => {
                    for (id, result) in results {
                        match result {
                            Ok(guard) => {
                                if let Some(queue) = &mut self.session.queue {
                                    queue.approve(id, guard.trust.canonical.clone());
                                }
                                self.session.guards.insert(id, guard);
                            }
                            Err(error) => {
                                if let Some(queue) = &mut self.session.queue {
                                    queue.reject(id, error.to_string());
                                }
                                self.session_message(format!(
                                    "A session file could not be restored: {error}"
                                ));
                            }
                        }
                    }
                }
                _ => {
                    if let Some(queue) = &mut self.session.queue {
                        let ids: Vec<_> = queue
                            .manifest()
                            .documents
                            .iter()
                            .map(|doc| doc.id)
                            .collect();
                        for id in ids {
                            queue.reject(id, "Session trust worker stopped".into());
                        }
                    }
                }
            }
        }
        // Inspect only already-open workspace state here; path classification and all reads stay on workers.
        let completed: Vec<_> = self
            .session
            .opening
            .iter()
            .filter_map(|(id, pending)| {
                let workspace = self.workspace.as_ref()?;
                if let Some(index) = (0..workspace.editors.len()).find(|index| {
                    workspace.path(*index) == Some(pending.guard.trust.canonical.as_path())
                }) {
                    if workspace.editors[index].busy() {
                        return None;
                    }
                    Some((*id, Some(index)))
                } else if !workspace.path_loading(&pending.guard.trust.canonical) {
                    Some((*id, None))
                } else {
                    None
                }
            })
            .collect();
        for (id, index) in completed {
            let pending = self.session.opening.remove(&id).unwrap();
            let success = index.is_some();
            if let Some(index) = index {
                let active = self
                    .session
                    .queue
                    .as_ref()
                    .and_then(|q| q.manifest().active_tab);
                if let Some(tab) = pending
                    .tabs
                    .iter()
                    .find(|tab| Some(tab.id) == active)
                    .or_else(|| pending.tabs.first())
                    .cloned()
                {
                    apply_view(self.workspace.as_mut().unwrap(), index, &tab.view);
                    let editor = &self.workspace.as_ref().unwrap().editors[index];
                    self.session.restored.push(Restored {
                        tab: tab.clone(),
                        snapshot: CapturedDocument::new(editor),
                    });
                    if Some(tab.id) == active {
                        self.app.active = index;
                    }
                }
            }
            if let Some(queue) = &mut self.session.queue {
                queue.completed(
                    id,
                    if success {
                        Ok(())
                    } else {
                        Err("File open failed".into())
                    },
                );
            }
        }
        if self.session.resolve.is_none() && self.session.opening.len() < 2 {
            let documents = self
                .session
                .queue
                .as_ref()
                .map(|queue| {
                    queue
                        .manifest()
                        .restore_order()
                        .into_iter()
                        .filter(|id| matches!(queue.state(*id), Some(RestoreState::AwaitingTrust)))
                        .take(2 - self.session.opening.len())
                        .filter_map(|id| {
                            queue
                                .manifest()
                                .documents
                                .iter()
                                .find(|doc| doc.id == id)
                                .cloned()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !documents.is_empty() {
                let request = SessionRequest::ResolvePaths {
                    documents,
                    provider: Arc::new(bareline_platform_windows::WindowsSessionPathTrustProvider),
                };
                match self
                    .session
                    .service(self.notify.clone())
                    .and_then(|service| service.submit(request))
                {
                    Ok(ticket) => self.session.resolve = Some(ticket),
                    Err(error) => {
                        self.session_message(format!("Session trust unavailable: {error}"))
                    }
                }
            }
        }
        while let Some(open) = self
            .session
            .queue
            .as_mut()
            .and_then(RestoreQueue::next_open)
        {
            let Some(guard) = self.session.guards.remove(&open.document_id) else {
                if let Some(queue) = &mut self.session.queue {
                    queue.completed(
                        open.document_id,
                        Err("Missing retained trust capability".into()),
                    );
                }
                continue;
            };
            if let Some(workspace) = &mut self.workspace {
                self.ledger.record(StartupAction::ReadDocument);
                workspace.open(open.path);
                self.session.opening.insert(
                    open.document_id,
                    Restoring {
                        guard,
                        tabs: open.tabs,
                    },
                );
            }
        }
        self.session_finish_restore();
        self.session_resolve_languages();
        let saved = self
            .session
            .save
            .as_ref()
            .and_then(|ticket| match ticket.try_recv() {
                Err(TryRecvError::Empty) => None,
                result => Some(result),
            });
        if let Some(result) = saved {
            self.session.save = None;
            match result {
                Ok(SessionCompletion::Written(Ok(()))) => {
                    if self.session.exit_requested {
                        let unchanged = self.workspace.as_ref().is_some_and(|workspace| {
                            exit_unchanged(workspace, &self.session.exit_snapshots)
                        });
                        if unchanged {
                            el.exit();
                        } else {
                            self.session.exit_requested = false;
                            self.session_message("Documents changed while saving the session. Close again to review unsaved changes.".into());
                        }
                    }
                }
                Ok(SessionCompletion::Written(Err(error))) => {
                    self.session.exit_requested = false;
                    self.session.exit_failed = true;
                    self.session_message(format!("Session could not be saved: {error}. Close again to exit without session persistence."));
                }
                _ => {
                    self.session.exit_requested = false;
                    self.session.exit_failed = true;
                    self.session_message(
                        "Session worker stopped. Close again to exit without session persistence."
                            .into(),
                    );
                }
            }
        }
        if let Some(workspace) = &self.workspace {
            self.app.tabs = workspace.titles();
            self.app.active = self.app.active.min(self.app.tabs.len().saturating_sub(1));
        }
    }
    fn session_finish_restore(&mut self) {
        if self.session.finalized || self.session.startup_pending() || self.session.queue.is_none()
        {
            return;
        }
        self.session.finalized = true;
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        let queue = self.session.queue.as_ref().unwrap();
        // Preserve persisted order after active-first physical loading. Cloned-view instantiation
        // remains the ViewController's responsibility; this path deduplicates shared documents.
        let mut order = Vec::new();
        for tab in &queue.manifest().tabs {
            if let Some(restored) = self
                .session
                .restored
                .iter()
                .find(|r| r.tab.document_id == tab.document_id)
                && let Some(index) = workspace
                    .editors
                    .iter()
                    .position(|e| restored.snapshot.same_editor(e))
                && !order.contains(&index)
            {
                order.push(index);
            }
        }
        for index in 0..workspace.editors.len() {
            if !order.contains(&index) {
                order.push(index);
            }
        }
        workspace.reorder(&order);
        let view_tabs: Vec<_> = queue
            .manifest()
            .tabs
            .iter()
            .filter_map(|tab| {
                let restored = self
                    .session
                    .restored
                    .iter()
                    .find(|restored| restored.tab.document_id == tab.document_id)?;
                let index = workspace
                    .editors
                    .iter()
                    .position(|editor| restored.snapshot.same_editor(editor))?;
                Some((tab.id, index))
            })
            .collect();
        if let Some(active) = queue
            .manifest()
            .active_tab
            .and_then(|id| queue.manifest().tabs.iter().find(|tab| tab.id == id))
            && let Some(restored) = self
                .session
                .restored
                .iter()
                .find(|r| r.tab.document_id == active.document_id)
            && let Some(index) = workspace
                .editors
                .iter()
                .position(|e| restored.snapshot.same_editor(e))
        {
            self.app.active = index;
        }
        self.views
            .restore_session(workspace, &mut self.app, queue.manifest(), &view_tabs);
        if let Some(compare) = &queue.manifest().compare {
            let documents: Vec<_> = view_tabs
                .iter()
                .filter_map(|(id, index)| {
                    queue
                        .manifest()
                        .tabs
                        .iter()
                        .find(|tab| tab.id == *id)
                        .map(|tab| (tab.document_id, *index))
                })
                .collect();
            match self.compare.restore(
                workspace,
                &mut self.views,
                compare,
                &documents,
                self.notify.clone(),
            ) {
                Ok(index) => self.app.active = index,
                Err(error) => workspace.message = Some(error),
            }
        }
    }
    pub(super) fn session_before_exit(&mut self, _el: &ActiveEventLoop) -> bool {
        if !self.first_frame
            || self.smoke
            || self.perf
            || self.prototype.is_some()
            || self.session.exit_failed
        {
            return false;
        }
        if self.session.exit_requested {
            return true;
        }
        if self.views.pending_edits() {
            self.session_message("Wait for pending split-view edits before closing.".into());
            return true;
        }
        // Closing during startup must not replace the previous session with a partial restore.
        if self.session.startup_pending() || self.workspace.is_none() {
            return false;
        }
        let Some(path) = self.session.path.clone() else {
            return false;
        };
        let manifest = self.capture_session();
        match manifest.and_then(|manifest| {
            self.session
                .service(self.notify.clone())
                .and_then(|service| {
                    service.submit(SessionRequest::Save {
                        path,
                        manifest: Box::new(manifest),
                    })
                })
        }) {
            Ok(ticket) => {
                self.session.save = Some(ticket);
                self.session.exit_snapshots = self
                    .workspace
                    .as_ref()
                    .map(|workspace| {
                        workspace
                            .editors
                            .iter()
                            .map(CapturedDocument::new)
                            .collect()
                    })
                    .unwrap_or_default();
                self.session.exit_requested = true;
                true
            }
            Err(error) => {
                self.session.exit_failed = true;
                self.session_message(format!("Session could not be saved: {error}. Close again to exit without session persistence."));
                true
            }
        }
    }
    fn capture_session(&self) -> std::io::Result<SessionManifest> {
        let Some(workspace) = &self.workspace else {
            return Ok(SessionManifest::default());
        };
        let previous = self.session.queue.as_ref().map(RestoreQueue::manifest);
        let mut manifest = SessionManifest {
            layout: previous.map(|m| m.layout.clone()).unwrap_or_default(),
            ..Default::default()
        };
        let mut next = previous
            .map(|m| {
                m.tabs
                    .iter()
                    .map(|t| t.id)
                    .chain(m.documents.iter().map(|d| d.id))
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1)
            })
            .unwrap_or(1);
        let titles = workspace.titles();
        let mut used = HashSet::new();
        let mut captured: Vec<(CapturedDocument, u64)> = Vec::new();
        let mut view_tabs = Vec::new();
        for (index, editor) in workspace.editors.iter().enumerate() {
            if !editor.paged() && !editor.snapshot().is_complete() {
                continue;
            }
            let old = self
                .session
                .restored
                .iter()
                .find(|r| r.snapshot.same_editor(editor) && !used.contains(&r.tab.id));
            let mut tab = if let Some(old) = old {
                old.tab.clone()
            } else {
                let id = next;
                next = next.checked_add(1).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "session identity exhausted",
                    )
                })?;
                SessionTab {
                    id,
                    document_id: id,
                    pinned: false,
                    view: ViewState::default(),
                }
            };
            used.insert(tab.id);
            if let Some((_, id)) = captured
                .iter()
                .find(|(snapshot, _)| snapshot.same_editor(editor))
            {
                tab.document_id = *id;
            }
            tab.view.language = editor.session_language_selection();
            tab.view.caret = editor.selection.caret as u64;
            tab.view.anchor = editor.selection.anchor as u64;
            if let bareline_app::workspace::WorkspaceEditor::Paged(paged) = editor {
                let selection=paged.global_selection();
                tab.view.caret=selection.caret as u64;
                tab.view.anchor=selection.anchor as u64;
            }
            tab.view.scroll_y_bits = editor.scroll_y.max(0.0).to_bits();
            tab.view.folds = editor.persisted_folds();
            if !manifest
                .documents
                .iter()
                .any(|doc| doc.id == tab.document_id)
            {
                manifest.documents.push(SessionDocument {
                    id: tab.document_id,
                    path: workspace.path(index).map(SerializedPath::from_native),
                    title: titles[index].clone(),
                });
                captured.push((CapturedDocument::new(editor), tab.document_id));
            }
            if index == self.app.active {
                manifest.active_tab = Some(tab.id);
                manifest.mru.push(tab.document_id);
            }
            view_tabs.push((index, tab.id));
            manifest.tabs.push(tab);
        }
        if let Some(queue) = &self.session.queue {
            for tab in &queue.manifest().tabs {
                if !used.contains(&tab.id)
                    && manifest
                        .documents
                        .iter()
                        .any(|doc| doc.id == tab.document_id)
                {
                    manifest.tabs.push(tab.clone());
                }
            }
            for document in &queue.manifest().documents {
                if matches!(queue.state(document.id), Some(RestoreState::Failed(_)))
                    && !manifest.documents.iter().any(|doc| doc.id == document.id)
                {
                    manifest.documents.push(document.clone());
                    manifest.tabs.extend(
                        queue
                            .manifest()
                            .tabs
                            .iter()
                            .filter(|tab| tab.document_id == document.id)
                            .cloned(),
                    );
                }
            }
        }
        manifest.layout.active_tabs = [None, None];
        manifest.layout.split = manifest.tabs.iter().any(|tab| tab.view.split == 1);
        manifest.layout.active_pane = 0;
        for tab in &manifest.tabs {
            if Some(tab.id) == manifest.active_tab {
                manifest.layout.active_pane = tab.view.split;
            }
            if manifest.layout.active_tabs[tab.view.split as usize].is_none()
                || Some(tab.id) == manifest.active_tab
            {
                manifest.layout.active_tabs[tab.view.split as usize] = Some(tab.id);
            }
        }
        // Preserve pinned partition without changing relative order inside each partition.
        self.views
            .capture_session(workspace, &mut manifest, &view_tabs);
        let documents: Vec<_> = view_tabs
            .iter()
            .filter_map(|(index, id)| {
                manifest
                    .tabs
                    .iter()
                    .find(|tab| tab.id == *id)
                    .map(|tab| (*index, tab.document_id))
            })
            .collect();
        manifest.compare = self.compare.capture(workspace, &documents);
        manifest.tabs.sort_by_key(|tab| !tab.pinned);
        manifest.recent = workspace.recent_paths().to_vec();
        manifest.validate()?;
        Ok(manifest)
    }
}
fn exit_unchanged(workspace: &Workspace, captured: &[CapturedDocument]) -> bool {
    !workspace.io_busy()
        && workspace.editors.len() == captured.len()
        && workspace
            .editors
            .iter()
            .zip(captured)
            .all(|(editor, captured)| {
                !editor.busy() && captured.same_editor(editor) && captured.same_state(editor)
            })
}

fn apply_view(workspace: &mut Workspace, index: usize, view: &ViewState) {
    let editor = &mut workspace.editors[index];
    if let bareline_app::workspace::WorkspaceEditor::Paged(paged) = editor {
        let caret = usize::try_from(view.caret)
            .unwrap_or(usize::MAX)
            .min(paged.snapshot().len());
        let anchor = usize::try_from(view.anchor)
            .unwrap_or(usize::MAX)
            .min(paged.snapshot().len());
        if let Err(error) = paged.restore_selection(TextOffset(anchor), TextOffset(caret)) {
            paged.error = Some(error);
        }
        return;
    }
    let bound = |raw: u64| {
        let mut offset = usize::try_from(raw)
            .unwrap_or(usize::MAX)
            .min(editor.snapshot().len());
        while !editor.snapshot().is_boundary(TextOffset(offset)) {
            offset -= 1;
        }
        offset
    };
    let caret = bound(view.caret);
    let anchor = bound(view.anchor);
    editor.selection.caret = caret;
    editor.selection.anchor = anchor;
    editor.scroll_y = f64::from_bits(view.scroll_y_bits);
    editor.restore_folds(&view.folds);
}

#[cfg(test)]
mod close_tests {
    use super::*;
    #[test]
    fn deferred_session_write_cannot_exit_after_an_intervening_edit_or_new_tab() {
        let mut workspace = Workspace::new(
            Arc::new(|| {}),
            Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        let captured = vec![CapturedDocument::new(&workspace.editors[0])];
        assert!(exit_unchanged(&workspace, &captured));
        workspace.editors[0].enqueue(Input::Insert("unsaved while session writes".into()));
        assert!(!exit_unchanged(&workspace, &captured));
        let deadline = Instant::now() + Duration::from_secs(5);
        while workspace.editors[0].busy() {
            assert!(Instant::now() < deadline);
            workspace.pump();
            std::thread::yield_now();
        }
        assert!(!exit_unchanged(&workspace, &captured));
        let captured = vec![CapturedDocument::new(&workspace.editors[0])];
        assert!(exit_unchanged(&workspace, &captured));
        workspace.new_document().unwrap();
        assert!(!exit_unchanged(&workspace, &captured));
    }
}

impl Shell {
    fn session_resolve_languages(&mut self){
        if !self.language.controller.catalog_ready(){return;}
        let catalog=&self.language.controller;
        let resolve=|editor:&mut bareline_editor_surface::EditorSurface|{
            let Some(selection)=editor.pending_session_language.take()else{return;};
            // An explicit selection made after restore supersedes its deferred catalog lookup.
            if editor.udl.is_some()||editor.language_override.is_some(){return;}
            if let bareline_file_io::session::LanguageSelection::Udl(id)=selection{
                if let Some(definition)=catalog.definition_by_id(&id){editor.udl=Some(definition);}
                else{editor.language=bareline_syntax::Language::PlainText;editor.language_override=Some(bareline_syntax::Language::PlainText);editor.error=Some(format!("Saved user language {id} is unavailable; using plain text. Import its definition explicitly to select it again."));}
            }
        };
        if let Some(workspace)=&mut self.workspace{for editor in &mut workspace.editors{resolve(editor);}}
        if let Some(editor)=&mut self.views.secondary{resolve(editor);}
    }
}