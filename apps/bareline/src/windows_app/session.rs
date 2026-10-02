// SPDX-License-Identifier: MPL-2.0
//! Native Shell consumer of the bounded session worker and restore queue.
use super::*;
use bareline_app::{
    session_service::{SessionCompletion, SessionRequest, SessionService, SessionTicket},
    session_ui::{RestoreQueue, RestoreState},
};
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_file_io::session::{SessionDocument, SessionManifest, SessionTab, SessionWindow, ViewState};
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
            bareline_app::workspace::WorkspaceEditor::Resident(editor) => Self::Resident(editor.snapshot().clone()),
            bareline_app::workspace::WorkspaceEditor::Paged(editor) => Self::Paged(editor.snapshot().clone()),
        }
    }
    fn same_editor(&self, editor: &bareline_app::workspace::WorkspaceEditor) -> bool {
        match (self, editor) {
            (Self::Resident(snapshot), bareline_app::workspace::WorkspaceEditor::Resident(editor)) => {
                snapshot.same_document(editor.snapshot())
            }
            (Self::Paged(snapshot), bareline_app::workspace::WorkspaceEditor::Paged(editor)) => {
                snapshot.same_document(editor.snapshot())
            }
            _ => false,
        }
    }
    fn same_state(&self, editor: &bareline_app::workspace::WorkspaceEditor) -> bool {
        match (self, editor) {
            (Self::Resident(snapshot), bareline_app::workspace::WorkspaceEditor::Resident(editor)) => {
                snapshot.content_state == editor.snapshot().content_state
            }
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
/// File ▸ Load Session… and Save Session As… (BIZ-07), one at a time.
enum NamedSession {
    /// The chosen file is read and checked on the session worker.
    Reading(SessionTicket),
    /// The open documents close first, each unsaved one with its own prompt.
    /// `queued` holds the identities of the documents open when it began.
    Closing {
        manifest: Box<SessionManifest>,
        queued: Vec<u64>,
    },
    /// The session file is written on the session worker.
    Writing {
        ticket: SessionTicket,
        path: PathBuf,
        left_out: usize,
    },
}
pub(super) struct SessionRuntime {
    path: Option<PathBuf>,
    restore_path: Option<PathBuf>,
    restore_authorized: bool,
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
    /// The user chose the active tab while the restore ran, so the restore does
    /// not move it to the saved active tab (APP-07).
    user_focused: bool,
    /// Shared with the logoff/shutdown subclass of the main window.
    end: std::rc::Rc<bareline_platform_windows::SessionEndSignal>,
    end_monitor: Option<bareline_platform_windows::SessionEndMonitor>,
    /// `SESSION_END_BUDGET`; tests on a loaded machine allow more.
    end_budget: Duration,
    named: Option<NamedSession>,
    /// The restore running now loads a named session, so its end is reported.
    named_restore: bool,
    /// The clean, empty Untitled that closing the last tab opened (UX-31),
    /// replaced once the named session has loaded a file.
    named_placeholder: Option<u64>,
}
impl Default for SessionRuntime {
    fn default() -> Self {
        Self {
            path: std::env::var_os("APPDATA").map(|root| PathBuf::from(root).join("Bareline/session.json")),
            restore_path: None,
            restore_authorized: false,
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
            user_focused: false,
            end: Default::default(),
            end_monitor: None,
            end_budget: SESSION_END_BUDGET,
            named: None,
            named_restore: false,
            named_placeholder: None,
        }
    }
}
impl SessionRuntime {
    pub(super) fn configure(&mut self, path: Option<PathBuf>, restore_path: Option<PathBuf>, restore: bool) {
        self.path = path;
        self.restore_path = restore_path;
        self.restore_authorized = self.restore_path.is_some();
        self.restore = restore;
    }
    /// The session folder refuses writes: exit without trying to save, so one
    /// close is enough (APP-13). The restore already read stays in effect.
    pub(super) fn disable_persistence(&mut self) {
        self.path = None;
    }
    #[cfg(test)]
    pub(super) fn persists(&self) -> bool {
        self.path.is_some()
    }
    pub(super) fn set_restore_path(&mut self, path: Option<PathBuf>) -> bool {
        if !self.started {
            self.restore_authorized = path.is_some();
            self.restore_path = path;
            true
        } else {
            false
        }
    }
    pub(super) fn startup_pending(&self) -> bool {
        self.load.is_some() || self.queue.as_ref().is_some_and(RestoreQueue::pending)
    }
    /// The previous session is restored, failed to load, or is not restored by
    /// this launch. Command-line files wait for this, and the session file is
    /// never written before it, so a launch with files cannot replace the saved
    /// session with only those files (APP-06).
    pub(super) fn restore_settled(&self) -> bool {
        !self.startup_pending() && (self.started || !self.restore || !self.restore_authorized)
    }
    /// Notes an explicit tab choice: a user open, a tab switch or close. Once
    /// the restore has finished this no longer matters (APP-07).
    pub(super) fn note_user_focus(&mut self) {
        if !self.finalized {
            self.user_focused = true;
        }
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
    /// Starts restoring the previous session. Files named on the command line
    /// do not skip it: they open on top of it afterwards (APP-06).
    pub(super) fn session_first_frame(&mut self) {
        if !self.first_frame || self.session.started {
            return;
        }
        self.session.started = true;
        if self.smoke || self.perf || self.prototype.is_some() || !self.session.restore {
            return;
        }
        if !self.session.restore_authorized {
            return;
        }
        let Some(path) = self.session.restore_path.clone() else {
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
    /// Document id of the active tab.
    pub(super) fn active_document(&self) -> Option<u64> {
        let workspace = self.workspace.as_ref()?;
        Some(workspace.editors.get(self.app.active)?.document_identity().0)
    }
    /// After user input: if it changed the active tab, a running restore leaves
    /// that choice alone (APP-07).
    pub(super) fn note_focus_input(&mut self, before: Option<u64>) {
        if self.active_document() != before {
            self.session.note_user_focus();
        }
    }
    fn session_message(&mut self, message: String) {
        eprintln!("event=session message={message}");
        let revision = toast::next_revision();
        self.toasts.push_typed(
            format!("session-outcome-{revision}"),
            revision,
            bareline_ui::theme::ToastLevel::Error,
            toast::NotificationKind::Outcome,
            message,
            None,
            None,
            toast::NotificationLifetime::Persistent,
            Instant::now(),
        );
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    pub(super) fn session_pump(&mut self, el: &ActiveEventLoop) {
        if !self.first_frame {
            return;
        }
        let loaded = self.session.load.as_ref().and_then(|ticket| match ticket.try_recv() {
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
                    if let Some(saved) = loaded.manifest.window {
                        self.restore_window(saved);
                    }
                    let diagnostic_warning = (!loaded.diagnostics.is_empty()).then(|| {
                        format!(
                            "{}{}",
                            if loaded.recovered_previous {
                                "Recovered the previous session generation. "
                            } else {
                                ""
                            },
                            loaded.diagnostics.summary()
                        )
                    });
                    if !loaded.manifest.recent.is_empty() && self.ensure_workspace(el) {
                        self.workspace
                            .as_mut()
                            .unwrap()
                            .restore_recent_paths(&loaded.manifest.recent);
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
                                    .map(|doc| (doc.id, doc.title.clone()))
                                    .collect();
                                for (id, title) in untitled {
                                    let active = queue.manifest().active_tab;
                                    let tab = queue
                                        .manifest()
                                        .tabs
                                        .iter()
                                        .filter(|tab| tab.document_id == id)
                                        .find(|tab| Some(tab.id) == active)
                                        .or_else(|| queue.manifest().tabs.iter().find(|tab| tab.document_id == id))
                                        .cloned();
                                    if let Some(tab) = tab {
                                        let workspace = self.workspace.as_mut().unwrap();
                                        if workspace.new_document().is_ok() {
                                            let index = workspace.editors.len() - 1;
                                            apply_view(workspace, index, &tab.view);
                                            restore_untitled_title(workspace, index, &title);
                                            self.session.restored.push(Restored {
                                                tab,
                                                snapshot: CapturedDocument::new(&workspace.editors[index]),
                                            });
                                        }
                                    } else {
                                        // The document survived but its tab metadata did not.
                                        let _ = self.workspace.as_mut().unwrap().new_document();
                                    }
                                }
                                self.session.queue = Some(queue);
                                if warning {
                                    self.session_message(
                                        "Recovered the previous session manifest; the latest was unavailable.".into(),
                                    );
                                }
                            }
                            Err(error) => self.session_message(format!("Session restore refused: {error}")),
                        }
                    }
                    if let Some(warning) = diagnostic_warning {
                        self.session_message(warning);
                    }
                }
                Ok(SessionCompletion::Loaded(Err(error))) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(SessionCompletion::Loaded(Err(error))) => {
                    self.session_message(format!("Session restore unavailable: {error}"))
                }
                _ => self.session_message("Session worker stopped before restore completed.".into()),
            }
        }
        self.session_restore_pump();
        // Launch and forwarded files held back during the restore open now (APP-06).
        if self.session.restore_settled() && self.launch.has_requests() {
            self.launch_pump();
        }
        let saved = self.session.save.as_ref().and_then(|ticket| match ticket.try_recv() {
            Err(TryRecvError::Empty) => None,
            result => Some(result),
        });
        if let Some(result) = saved {
            self.session.save = None;
            match result {
                Ok(SessionCompletion::Written(Ok(()))) => {
                    if self.session.exit_requested {
                        let unchanged = self
                            .workspace
                            .as_ref()
                            .is_some_and(|workspace| exit_unchanged(workspace, &self.session.exit_snapshots));
                        if !unchanged {
                            self.session.exit_requested = false;
                            self.session_message(
                                "Documents changed while saving the session. Close again to review unsaved changes."
                                    .into(),
                            );
                        } else if self.instance_exit_ready() {
                            el.exit();
                        } else {
                            // Launches acknowledged since the close began open now;
                            // the next close saves them with the session.
                            self.session.exit_requested = false;
                            self.instance_exit_cancelled(el);
                        }
                    }
                }
                Ok(SessionCompletion::Written(Err(error))) => {
                    self.session.exit_requested = false;
                    self.session.exit_failed = true;
                    self.session_message(format!(
                        "Session could not be saved: {error}. Close again to exit without session persistence."
                    ));
                }
                _ => {
                    self.session.exit_requested = false;
                    self.session.exit_failed = true;
                    self.session_message(
                        "Session worker stopped. Close again to exit without session persistence.".into(),
                    );
                }
            }
        }
        if let Some(workspace) = &self.workspace {
            self.app.tabs = workspace.titles();
            self.app.active = self.app.active.min(self.app.tabs.len().saturating_sub(1));
        }
    }
    /// Resolve trust for, open and finish the documents of a session being
    /// restored: the previous session at startup or a named session (BIZ-07).
    pub(super) fn session_restore_pump(&mut self) {
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
                                // Name the file: a missing file is reported, never fatal (BIZ-07).
                                let name = self
                                    .session
                                    .queue
                                    .as_ref()
                                    .and_then(|queue| queue.candidate(id))
                                    .map(|path| path.display.clone())
                                    .unwrap_or_else(|| "A session file".to_owned());
                                if let Some(queue) = &mut self.session.queue {
                                    queue.reject(id, error.to_string());
                                }
                                self.session_message(format!("{name} could not be restored: {error}"));
                            }
                        }
                    }
                }
                _ => {
                    if let Some(queue) = &mut self.session.queue {
                        let ids: Vec<_> = queue.manifest().documents.iter().map(|doc| doc.id).collect();
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
                if let Some(index) = (0..workspace.editors.len())
                    .find(|index| workspace.path(*index) == Some(pending.guard.trust.canonical.as_path()))
                {
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
                let active = self.session.queue.as_ref().and_then(|q| q.manifest().active_tab);
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
                    // The saved active tab shows as soon as it loads, unless the
                    // user has chosen a tab meanwhile (APP-07).
                    if Some(tab.id) == active && !self.session.user_focused {
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
                        .filter_map(|id| queue.manifest().documents.iter().find(|doc| doc.id == id).cloned())
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
                    Err(error) => self.session_message(format!("Session trust unavailable: {error}")),
                }
            }
        }
        while let Some(open) = self.session.queue.as_mut().and_then(RestoreQueue::next_open) {
            let Some(guard) = self.session.guards.remove(&open.document_id) else {
                if let Some(queue) = &mut self.session.queue {
                    queue.completed(open.document_id, Err("Missing retained trust capability".into()));
                }
                continue;
            };
            if let Some(workspace) = &mut self.workspace {
                self.ledger.record(StartupAction::ReadDocument);
                // Restored files load in the background; only the saved active
                // tab is activated, below (APP-07).
                workspace.open_in_background(open.path);
                self.session
                    .opening
                    .insert(open.document_id, Restoring { guard, tabs: open.tabs });
            }
        }
        self.session_finish_restore();
        self.session_resolve_languages();
    }
    fn session_finish_restore(&mut self) {
        if self.session.finalized || self.session.startup_pending() || self.session.queue.is_none() {
            return;
        }
        self.session.finalized = true;
        if std::mem::take(&mut self.session.named_restore)
            && let Some(queue) = &self.session.queue
        {
            let documents = &queue.manifest().documents;
            let loaded = documents
                .iter()
                .filter(|doc| matches!(queue.state(doc.id), Some(RestoreState::Loaded)))
                .count();
            let message = if loaded == documents.len() {
                format!("Session loaded: {loaded} file{}", if loaded == 1 { "" } else { "s" })
            } else {
                format!(
                    "Session loaded: {loaded} of {} files; the others could not be restored",
                    documents.len()
                )
            };
            if let Some(workspace) = &mut self.workspace {
                workspace.message = Some(message);
            }
            // The Untitled left by the close gives way to the loaded files; it
            // closes through the normal path, so an edit made meanwhile keeps it.
            if let Some(owner) = self.session.named_placeholder.take()
                && loaded > 0
                && self.pending_close.is_none()
                && let Some(workspace) = &self.workspace
                && let Some(index) = placeholder_index(workspace, owner)
            {
                self.pending_close = Some(PendingClose::Document(CloseTarget {
                    index,
                    identity: workspace.editors[index].document_identity(),
                    tab: None,
                    saving: false,
                    discarding: false,
                    was_read_only: false,
                    deferred: true,
                }));
                (self.notify)();
            }
        }
        // A tab the user chose while the restore ran stays active after the
        // saved layout is applied, wherever the reorder moves it (APP-07).
        let chosen = self
            .session
            .user_focused
            .then(|| {
                let workspace = self.workspace.as_ref()?;
                Some(workspace.editors.get(self.app.active)?.document_identity().0)
            })
            .flatten();
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
                && let Some(index) = workspace.editors.iter().position(|e| restored.snapshot.same_editor(e))
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
            && let Some(index) = workspace.editors.iter().position(|e| restored.snapshot.same_editor(e))
            && !self.session.user_focused
        {
            self.app.active = index;
        }
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
            match self
                .compare
                .restore(workspace, &mut self.views, compare, &documents, self.notify.clone())
            {
                Ok(index) => self.app.active = index,
                Err(error) => workspace.message = Some(error),
            }
        }
        // Comparison setup assigns documents and reinstalls panes. Restore the
        // persisted views last so it cannot replace their carets or active pane.
        self.views
            .restore_session(workspace, &mut self.app, queue.manifest(), &view_tabs);
        let panel = match queue.manifest().layout.bottom_panel.as_deref() {
            Some("search") => Some(super::dock::DockTab::Search),
            Some("compare") => Some(super::dock::DockTab::Compare),
            Some("output") => Some(super::dock::DockTab::Output),
            _ => None,
        };
        self.dock.restore(
            panel,
            queue.manifest().layout.bottom_panel_collapsed,
            f32::from_bits(queue.manifest().layout.bottom_panel_height_bits),
        );
        if let Some(document) = chosen
            && let Some(index) = workspace
                .editors
                .iter()
                .position(|editor| editor.document_identity().0 == document)
        {
            self.app.active = index;
        }
    }
    pub(super) fn session_before_exit(&mut self, _el: &ActiveEventLoop) -> bool {
        if !self.first_frame || self.smoke || self.perf || self.prototype.is_some() || self.session.exit_failed {
            return false;
        }
        if self.session.exit_requested {
            return true;
        }
        if self.views.pending_edits() {
            self.session_message("Wait for pending split-view edits before closing.".into());
            return true;
        }
        // Closing during startup must not replace the previous session with a
        // partial restore, or with only the command-line files (APP-06).
        if !self.session.restore_settled() || self.workspace.is_none() {
            return false;
        }
        let Some(path) = self.session.path.clone() else {
            return false;
        };
        let manifest = self.capture_session();
        match manifest.and_then(|manifest| {
            self.session.service(self.notify.clone()).and_then(|service| {
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
                    .map(|workspace| workspace.editors.iter().map(CapturedDocument::new).collect())
                    .unwrap_or_default();
                self.session.exit_requested = true;
                true
            }
            Err(error) => {
                self.session.exit_failed = true;
                self.session_message(format!(
                    "Session could not be saved: {error}. Close again to exit without session persistence."
                ));
                true
            }
        }
    }
    /// Records where the window is so the next launch opens in the same place.
    /// A minimized window keeps whatever placement was saved before.
    fn capture_window(&self) -> Option<SessionWindow> {
        let window = self.window.as_ref()?;
        if window.is_minimized().unwrap_or(false) {
            return self.session.queue.as_ref().and_then(|queue| queue.manifest().window);
        }
        // Inner size pairs with `request_inner_size` on restore so the window
        // does not grow by the frame width on every restart.
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let position = window.outer_position().ok()?;
        Some(SessionWindow {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            maximized: window.is_maximized(),
        })
    }
    /// Restores a saved placement, but only when it still lands on a connected
    /// monitor: an unplugged second screen must not hide the window.
    fn restore_window(&self, saved: SessionWindow) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if saved.width == 0 || saved.height == 0 {
            return;
        }
        let left = saved.x;
        let top = saved.y;
        let right = left.saturating_add(saved.width as i32);
        let bottom = top.saturating_add(saved.height as i32);
        let visible = window.available_monitors().any(|monitor| {
            let origin = monitor.position();
            let size = monitor.size();
            let m_right = origin.x.saturating_add(size.width as i32);
            let m_bottom = origin.y.saturating_add(size.height as i32);
            // Require a real overlap, not a single shared pixel row.
            left < m_right - 64 && right > origin.x + 64 && top < m_bottom - 32 && bottom > origin.y
        });
        if !visible {
            return;
        }
        let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(saved.width, saved.height));
        window.set_outer_position(winit::dpi::PhysicalPosition::new(saved.x, saved.y));
        if saved.maximized {
            window.set_maximized(true);
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
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "session identity exhausted")
                })?;
                SessionTab {
                    id,
                    document_id: id,
                    pinned: false,
                    view: ViewState::default(),
                }
            };
            used.insert(tab.id);
            if let Some((_, id)) = captured.iter().find(|(snapshot, _)| snapshot.same_editor(editor)) {
                tab.document_id = *id;
            }
            tab.view.language = editor.session_language_selection();
            tab.view.caret = editor.viewport().selection.caret as u64;
            tab.view.anchor = editor.viewport().selection.anchor as u64;
            if let bareline_app::workspace::WorkspaceEditor::Paged(paged) = editor {
                let selection = paged.global_selection();
                tab.view.caret = selection.1.0 as u64;
                tab.view.anchor = selection.0.0 as u64;
            }
            tab.view.scroll_y_bits = editor.viewport().scroll_y.max(0.0).to_bits();
            tab.view.folds = editor.persisted_folds();
            if !manifest.documents.iter().any(|doc| doc.id == tab.document_id) {
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
                if !used.contains(&tab.id) && manifest.documents.iter().any(|doc| doc.id == tab.document_id) {
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
            if manifest.layout.active_tabs[tab.view.split as usize].is_none() || Some(tab.id) == manifest.active_tab {
                manifest.layout.active_tabs[tab.view.split as usize] = Some(tab.id);
            }
        }
        // Preserve pinned partition without changing relative order inside each partition.
        self.views.capture_session(workspace, &mut manifest, &view_tabs);
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
        let (panel, collapsed, height) = self.dock.persisted();
        manifest.layout.bottom_panel = panel.map(|panel| {
            match panel {
                super::dock::DockTab::Search => "search",
                super::dock::DockTab::Compare => "compare",
                super::dock::DockTab::Output => "output",
            }
            .into()
        });
        manifest.layout.bottom_panel_collapsed = collapsed;
        manifest.layout.bottom_panel_height_bits = height.to_bits();
        manifest.tabs.sort_by_key(|tab| !tab.pinned);
        manifest.recent = workspace.recent_paths().to_vec();
        manifest.window = self.capture_window();
        manifest.validate()?;
        Ok(manifest)
    }
}
/// How long one logoff or shutdown notification may hold the UI thread. Windows
/// gives each window about five seconds before it reports the app as not responding.
const SESSION_END_BUDGET: Duration = Duration::from_secs(3);
impl Shell {
    /// Subclass the new main window for logoff and shutdown, and register for
    /// relaunch after update restarts.
    pub(super) fn session_end_attach(&mut self, hwnd: isize) {
        // SAFETY: `hwnd` is the live main window created on this thread. The
        // monitor removes its subclass on drop or on WM_NCDESTROY, whichever is first.
        match unsafe { bareline_platform_windows::SessionEndMonitor::attach(hwnd, self.session.end.clone()) } {
            Ok(monitor) => self.session.end_monitor = Some(monitor),
            Err(error) => eprintln!("event=session_end_unavailable error={error}"),
        }
        if !self.smoke
            && !self.perf
            && self.prototype.is_none()
            && let Err(error) = bareline_platform_windows::register_application_restart()
        {
            eprintln!("event=restart_registration_failed error={error}");
        }
    }
    /// Keep the subclass's dirty flag current so it can explain a shutdown
    /// delay even while a modal dialog holds the handler.
    pub(super) fn session_end_track_dirty(&self) {
        self.session.end.set_dirty(
            self.workspace
                .as_ref()
                .is_some_and(|workspace| workspace.editors.iter().any(|editor| editor.dirty())),
        );
    }
    /// Claim a flush that the session-end subclass routed through winit as
    /// `CloseRequested`, and run it. Ordinary close requests return false.
    pub(super) fn session_end_event(&mut self, event: &WindowEvent) -> bool {
        if !matches!(event, WindowEvent::CloseRequested) || !self.session.end.take_request() {
            return false;
        }
        let complete = self.session_end_flush(Instant::now() + self.session.end_budget);
        self.session.end.finish(complete);
        true
    }
    /// Write session.json, then pump until every document's latest text is in a
    /// durable recovery checkpoint, giving up at `deadline`. Never prompts and
    /// never exits: Windows ends the process once the session ends.
    fn session_end_flush(&mut self, deadline: Instant) -> bool {
        let written = self.session_end_write(deadline);
        let mut settled = true;
        let mut changed = false;
        let before = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.tab_documents())
            .unwrap_or_default();
        if let Some(workspace) = &mut self.workspace {
            loop {
                changed |= workspace.pump();
                // Queued split-view input reaches its document only through this pump.
                changed |= self.views.pump(workspace);
                settled = !self.views.pending_edits() && workspace.recovery_settled();
                if settled || Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        // The same follow-up as `user_event`: a cancelled logoff keeps running
        // with the completions this flush consumed.
        if changed {
            self.follow_workspace_activation(&before);
            self.sync_data_safety_notifications();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        self.session_end_track_dirty();
        written && settled
    }
    fn session_end_write(&mut self, deadline: Instant) -> bool {
        // The guards of `session_before_exit`: never replace the previous session
        // with a partial restore or with a diagnostic launch's state.
        if !self.first_frame
            || self.smoke
            || self.perf
            || self.prototype.is_some()
            || !self.session.restore_settled()
            || self.workspace.is_none()
        {
            return true;
        }
        let Some(path) = self.session.path.clone() else {
            return true;
        };
        let submitted = self.capture_session().and_then(|manifest| {
            self.session.service(self.notify.clone()).and_then(|service| {
                service.submit(SessionRequest::Save {
                    path,
                    manifest: Box::new(manifest),
                })
            })
        });
        let ticket = match submitted {
            Ok(ticket) => ticket,
            Err(error) => {
                eprintln!("event=session_end_save_failed error={error}");
                return false;
            }
        };
        loop {
            match ticket.try_recv() {
                Ok(SessionCompletion::Written(Ok(()))) => return true,
                Err(TryRecvError::Empty) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                Ok(SessionCompletion::Written(Err(error))) => {
                    eprintln!("event=session_end_save_failed error={error}");
                    return false;
                }
                _ => return false,
            }
        }
    }
}
impl Shell {
    /// File ▸ Load Session… and Save Session As… (BIZ-07).
    pub(super) fn session_named_command(&mut self, id: &str) {
        let result = if id == "file.session.save" {
            self.session_named_save()
        } else {
            self.session_named_load()
        };
        if let Err(error) = result {
            self.session_named_note(error);
        }
    }
    /// A status line for a named session, or a notice while no document shows one.
    fn session_named_note(&mut self, message: String) {
        match &mut self.workspace {
            Some(workspace) => workspace.message = Some(message),
            None => self.session_message(message),
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// Write the saved files, with their tabs, pins and views, to a session
    /// file the person names. Untitled documents are left out and counted.
    fn session_named_save(&mut self) -> Result<(), String> {
        if self.session.named.is_some() || !self.session.restore_settled() {
            return Err("Wait for the session being restored, loaded or saved to finish".into());
        }
        let manifest = self
            .capture_session()
            .map_err(|error| format!("Could not capture the session: {error}"))?;
        let left_out = manifest.documents.iter().filter(|doc| doc.path.is_none()).count();
        let named = manifest.named();
        if named.documents.is_empty() {
            return Err("Save at least one document before saving a session".into());
        }
        let platform = self.platform.as_ref().ok_or("Window unavailable")?;
        let options = bareline_platform::SaveDialogOptions::new(bareline_platform::SaveFileKind::Json)
            .named("session.json".to_owned());
        let Some(path) = platform.save_file_with(&options)? else {
            return Ok(());
        };
        let ticket = self
            .session
            .service(self.notify.clone())
            .and_then(|service| {
                service.submit(SessionRequest::Export {
                    path: path.clone(),
                    manifest: Box::new(named),
                })
            })
            .map_err(|error| format!("Could not save the session: {error}"))?;
        self.session.named = Some(NamedSession::Writing { ticket, path, left_out });
        Ok(())
    }
    /// Read a session file the person chooses; `session_named_pump` then
    /// closes the open documents and restores the session's files.
    fn session_named_load(&mut self) -> Result<(), String> {
        if self.session.named.is_some() || !self.session.restore_settled() || self.session.closing() {
            return Err("Wait for the session being restored, loaded or saved to finish".into());
        }
        let platform = self.platform.as_ref().ok_or("Window unavailable")?;
        let Some(path) = platform.open_file()? else {
            return Ok(());
        };
        self.session_named_read(path)
    }
    /// Start reading the session file at `path` on the session worker.
    fn session_named_read(&mut self, path: PathBuf) -> Result<(), String> {
        let ticket = self
            .session
            .service(self.notify.clone())
            .and_then(|service| service.submit(SessionRequest::Import { path }))
            .map_err(|error| format!("Could not read the session: {error}"))?;
        self.session.named = Some(NamedSession::Reading(ticket));
        Ok(())
    }
    /// Drive a named session load or save. Cheap to call on every loop turn.
    pub(super) fn session_named_pump(&mut self) {
        let Some(named) = self.session.named.take() else {
            return;
        };
        match named {
            NamedSession::Reading(ticket) => match ticket.try_recv() {
                Err(TryRecvError::Empty) => self.session.named = Some(NamedSession::Reading(ticket)),
                Ok(SessionCompletion::Loaded(Ok(loaded))) => {
                    if !loaded.diagnostics.is_empty() {
                        self.session_message(loaded.diagnostics.summary());
                    }
                    let manifest = loaded.manifest.named();
                    if manifest.documents.is_empty() {
                        self.session_named_note("The session file lists no saved files.".into());
                        return;
                    }
                    // Loading replaces the open documents; each unsaved one asks
                    // first, and a Cancel keeps everything as it is.
                    let queued: Vec<u64> = self
                        .workspace
                        .as_ref()
                        .map(|workspace| {
                            workspace
                                .editors
                                .iter()
                                .map(|editor| editor.document_identity().0)
                                .collect()
                        })
                        .unwrap_or_default();
                    self.session.named = Some(NamedSession::Closing {
                        manifest: Box::new(manifest),
                        queued,
                    });
                    if !self.tab_close_everything() {
                        self.session.named = None;
                        self.session_named_note(
                            "Wait for the documents being closed, then load the session again.".into(),
                        );
                    }
                }
                Ok(SessionCompletion::Loaded(Err(error))) => {
                    self.session_named_note(format!("Could not load the session: {error}"))
                }
                _ => self.session_named_note("The session worker stopped before the session was read.".into()),
            },
            NamedSession::Closing { manifest, queued } => {
                if self.tab_close_running() {
                    self.session.named = Some(NamedSession::Closing { manifest, queued });
                    return;
                }
                // Closing the last tab opens a fresh Untitled (UX-31), so only a
                // queued document still open means a Cancel or a refused close.
                let Some(workspace) = &self.workspace else {
                    self.session_named_install(*manifest);
                    return;
                };
                if workspace
                    .editors
                    .iter()
                    .any(|editor| queued.contains(&editor.document_identity().0))
                {
                    self.session_named_note("The session was not loaded because documents are still open.".into());
                    return;
                }
                self.session.named_placeholder = workspace
                    .editors
                    .iter()
                    .map(|editor| editor.document_identity().0)
                    .find(|owner| placeholder_index(workspace, *owner).is_some());
                self.session_named_install(*manifest);
            }
            NamedSession::Writing { ticket, path, left_out } => match ticket.try_recv() {
                Err(TryRecvError::Empty) => self.session.named = Some(NamedSession::Writing { ticket, path, left_out }),
                Ok(SessionCompletion::Written(Ok(()))) => {
                    let mut message = format!("Session saved to {}", path.display());
                    if left_out == 1 {
                        message.push_str("; 1 Untitled document was left out");
                    } else if left_out > 1 {
                        message.push_str(&format!("; {left_out} Untitled documents were left out"));
                    }
                    self.session_named_note(message);
                }
                Ok(SessionCompletion::Written(Err(error))) => {
                    self.session_named_note(format!("Could not save the session: {error}"))
                }
                _ => self.session_named_note("The session worker stopped before the session was saved.".into()),
            },
        }
    }
    /// Restore `manifest` through the startup restore queue: each path is
    /// checked on a worker, missing or refused files are reported one by one
    /// and never stop the rest, and tabs get their pins, carets and scroll.
    pub(super) fn session_named_install(&mut self, manifest: SessionManifest) {
        match RestoreQueue::new(manifest) {
            Ok(mut queue) => {
                queue.first_frame_presented();
                self.session.queue = Some(queue);
                self.session.finalized = false;
                self.session.user_focused = false;
                self.session.named_restore = true;
                self.session.restored.clear();
                self.session.guards.clear();
                self.session.opening.clear();
                // The session pump checks and opens the files.
                (self.notify)();
            }
            Err(error) => {
                self.session.named_placeholder = None;
                self.session_named_note(format!("Could not load the session: {error}"));
            }
        }
    }
}
fn exit_unchanged(workspace: &Workspace, captured: &[CapturedDocument]) -> bool {
    !workspace.io_busy()
        && workspace.editors.len() == captured.len()
        && workspace
            .editors
            .iter()
            .zip(captured)
            .all(|(editor, captured)| !editor.busy() && captured.same_editor(editor) && captured.same_state(editor))
}

/// Index of document `owner` while it is still a clean, empty, idle Untitled.
fn placeholder_index(workspace: &Workspace, owner: u64) -> Option<usize> {
    let index = workspace
        .editors
        .iter()
        .position(|editor| editor.document_identity().0 == owner)?;
    let editor = &workspace.editors[index];
    (workspace.path(index).is_none()
        && !editor.dirty()
        && !editor.busy()
        && editor.resident().is_some_and(|surface| surface.snapshot().is_empty()))
    .then_some(index)
}
/// A renamed Untitled tab gets its title back (WSP-01); the default
/// "Untitled N" titles are numbered afresh. Saved titles carry the unsaved mark.
fn restore_untitled_title(workspace: &mut Workspace, index: usize, title: &str) {
    let title = title.strip_suffix(" \u{2022}").unwrap_or(title);
    let numbered = title
        .strip_prefix("Untitled ")
        .is_some_and(|number| number.parse::<u64>().is_ok());
    if !numbered {
        let _ = workspace.rename_untitled(index, title);
    }
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
        let mut offset = usize::try_from(raw).unwrap_or(usize::MAX).min(editor.snapshot().len());
        while !editor.snapshot().is_boundary(TextOffset(offset)) {
            offset -= 1;
        }
        offset
    };
    let caret = bound(view.caret);
    let anchor = bound(view.anchor);
    editor.viewport_mut().selection.caret = caret;
    editor.viewport_mut().selection.anchor = anchor;
    editor.viewport_mut().scroll_y = f64::from_bits(view.scroll_y_bits);
    editor.restore_folds(&view.folds);
}

#[cfg(test)]
mod close_tests {
    use super::*;
    #[test]
    fn deferred_session_write_cannot_exit_after_an_intervening_edit_or_new_tab() {
        let mut workspace =
            Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
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

    #[test]
    fn command_line_files_open_on_top_of_the_restored_session_and_keep_focus() {
        let root = std::env::temp_dir().join(format!(
            "bareline-session-launch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let requested = root.join("requested.txt");
        let restored = root.join("restored.txt");
        std::fs::write(&requested, "opened from the command line\n").unwrap();
        std::fs::write(&restored, "restored from the session\n").unwrap();
        let session = root.join("session.json");
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        shell.first_frame = true;
        shell.startup_paths = vec![requested.clone()];
        shell.session.configure(Some(session.clone()), Some(session), true);
        shell.workspace =
            Some(Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap());
        shell
            .launch
            .queue(&bareline_platform_windows::instance::OpenRequest {
                paths: vec![requested.clone()],
                line: None,
                column: None,
                read_only: false,
                monitor: false,
            })
            .unwrap();
        // Nothing opens, and nothing may be saved, before the restore has run (APP-06).
        assert!(!shell.session.restore_settled());
        shell.launch_pump();
        assert!(!shell.workspace.as_ref().unwrap().path_loading(&requested));
        // Files on the command line no longer skip the restore.
        shell.session_first_frame();
        assert!(shell.session.startup_pending(), "the saved session is restored first");
        shell.launch_pump();
        let workspace = shell.workspace.as_ref().unwrap();
        assert!(workspace.editors.is_empty() && !workspace.path_loading(&requested));
        // The restore completes with a file still loading in the background.
        shell.session.load = None;
        shell.workspace.as_mut().unwrap().open_in_background(restored.clone());
        assert!(shell.session.restore_settled());
        shell.launch_pump();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let before = shell.workspace.as_ref().unwrap().tab_documents();
            if shell.workspace.as_mut().unwrap().pump() {
                shell.follow_workspace_activation(&before);
            }
            shell.launch_pump();
            let workspace = shell.workspace.as_ref().unwrap();
            if !shell.launch.has_requests()
                && !workspace.io_busy()
                && !workspace.editors.iter().any(|editor| editor.busy())
            {
                break;
            }
            assert!(Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        let workspace = shell.workspace.as_ref().unwrap();
        assert_eq!(workspace.editors.len(), 2);
        // The requested file is active; the session file finishing never took focus (APP-07).
        assert_eq!(workspace.path(shell.app.active), Some(requested.as_path()));
        drop(shell);
        let _ = std::fs::remove_dir_all(root);
    }

    /// BIZ-07: a named session read back from its file opens the saved files
    /// at their carets; a file that has gone missing is reported by name and
    /// never stops the rest. Its Untitled documents were never in the file.
    #[test]
    fn named_session_restores_its_files_and_reports_missing_ones() {
        let root = std::env::temp_dir().join(format!(
            "bareline-named-session-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let present = root.join("present.txt");
        let missing = root.join("missing.txt");
        std::fs::write(&present, "first line\nsecond line\n").unwrap();
        let document = |id, path: Option<&std::path::Path>| SessionDocument {
            id,
            path: path.map(SerializedPath::from_native),
            title: format!("document {id}"),
        };
        let tab = |id, pinned, caret| SessionTab {
            id,
            document_id: id,
            pinned,
            view: ViewState {
                caret,
                anchor: caret,
                ..Default::default()
            },
        };
        let session = SessionManifest {
            documents: vec![
                document(1, Some(present.as_path())),
                document(2, Some(missing.as_path())),
                document(3, None),
            ],
            tabs: vec![tab(1, true, 11), tab(2, false, 0), tab(3, false, 0)],
            active_tab: Some(3),
            ..Default::default()
        };
        // Save Session As writes the named form; Load Session reads it back.
        let bytes = bareline_file_io::session::encode(&session.named()).unwrap();
        let manifest = bareline_file_io::session::decode(&bytes).unwrap();
        assert_eq!(manifest.documents.len(), 2);
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        shell.first_frame = true;
        shell.workspace =
            Some(Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap());
        shell.session_named_install(manifest);
        assert!(shell.session.startup_pending());
        let deadline = Instant::now() + Duration::from_secs(20);
        while !shell.session.finalized {
            let before = shell.workspace.as_ref().unwrap().tab_documents();
            if shell.workspace.as_mut().unwrap().pump() {
                shell.follow_workspace_activation(&before);
            }
            shell.session_restore_pump();
            assert!(
                Instant::now() < deadline,
                "{:?}",
                shell.workspace.as_ref().unwrap().message
            );
            std::thread::yield_now();
        }
        let workspace = shell.workspace.as_ref().unwrap();
        assert_eq!(workspace.editors.len(), 1);
        assert_eq!(
            workspace.path(0).and_then(|path| path.file_name()),
            Some(std::ffi::OsStr::new("present.txt"))
        );
        assert_eq!(workspace.editors[0].viewport().selection.caret, 11);
        let queue = shell.session.queue.as_ref().unwrap();
        assert!(matches!(queue.state(2), Some(RestoreState::Failed(_))));
        assert_eq!(shell.toasts.persistent_len(), 1, "the missing file is reported once");
        assert_eq!(
            workspace.message.as_deref(),
            Some("Session loaded: 1 of 2 files; the others could not be restored")
        );
        drop(shell);
        let _ = std::fs::remove_dir_all(root);
    }

    /// BIZ-07: Load Session reads the file, closes the open documents one at a
    /// time and then loads. Closing the last tab opens a fresh Untitled (UX-31);
    /// that must not stop the load, and it gives way to the loaded file. A
    /// document whose close is refused (Cancel) stops the load and stays open.
    #[test]
    fn load_session_closes_the_open_documents_then_loads_unless_one_stays_open() {
        fn drive(shell: &mut Shell, renderer: &mut bareline_renderer_recording::RecordingBackend) {
            match shell.pending_close.take() {
                Some(PendingClose::Document(target)) => shell.close_document_with_renderer(target, renderer),
                other => shell.pending_close = other,
            }
            shell.tab_close_advance();
            let before = shell.workspace.as_ref().unwrap().tab_documents();
            if shell.workspace.as_mut().unwrap().pump() {
                shell.follow_workspace_activation(&before);
            }
            shell.session_named_pump();
            shell.session_restore_pump();
        }
        let root = std::env::temp_dir().join(format!(
            "bareline-load-session-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let present = root.join("present.txt");
        std::fs::write(&present, "first line\nsecond line\n").unwrap();
        let session_file = root.join("named.json");
        let session = SessionManifest {
            documents: vec![SessionDocument {
                id: 1,
                path: Some(SerializedPath::from_native(&present)),
                title: "present.txt".into(),
            }],
            tabs: vec![SessionTab {
                id: 1,
                document_id: 1,
                pinned: false,
                view: ViewState {
                    caret: 6,
                    anchor: 6,
                    ..Default::default()
                },
            }],
            active_tab: Some(1),
            ..Default::default()
        };
        std::fs::write(&session_file, bareline_file_io::session::encode(&session).unwrap()).unwrap();
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let two_documents = |dirty: bool| {
            let mut workspace =
                Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
            workspace.new_document().unwrap();
            workspace.new_document().unwrap();
            if dirty {
                workspace.editors[1].enqueue(Input::Insert("unsaved".into()));
                let deadline = Instant::now() + Duration::from_secs(5);
                while workspace.editors[1].busy() {
                    assert!(Instant::now() < deadline);
                    workspace.pump();
                    std::thread::yield_now();
                }
                assert!(workspace.editors[1].dirty());
            }
            workspace
        };

        // Every document closes: the load goes ahead.
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        shell.first_frame = true;
        shell.workspace = Some(two_documents(false));
        let closed: Vec<u64> = shell.workspace.as_ref().unwrap().tab_documents();
        shell.session_named_read(session_file.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while shell.session.named.is_some() || !shell.session.finalized {
            drive(&mut shell, &mut renderer);
            let message = shell.workspace.as_ref().unwrap().message.clone();
            assert!(
                shell.session.named.is_some() || shell.session.queue.is_some(),
                "the load stopped: {message:?}"
            );
            assert!(Instant::now() < deadline, "{message:?}");
            std::thread::yield_now();
        }
        assert_eq!(
            shell.workspace.as_ref().unwrap().message.as_deref(),
            Some("Session loaded: 1 file")
        );
        assert!(
            shell.pending_close.is_some(),
            "the Untitled left by the close is closed"
        );
        while shell.pending_close.is_some() {
            drive(&mut shell, &mut renderer);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let workspace = shell.workspace.as_ref().unwrap();
        assert_eq!(workspace.editors.len(), 1, "the Untitled left by the close gave way");
        assert_eq!(
            workspace.path(0).and_then(|path| path.file_name()),
            Some(std::ffi::OsStr::new("present.txt"))
        );
        assert!(!closed.contains(&workspace.editors[0].document_identity().0));
        assert_eq!(workspace.editors[0].viewport().selection.caret, 6);
        drop(shell);

        // A dirty document refuses its close (no prompt headless, like Cancel):
        // nothing loads and it stays open.
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        shell.first_frame = true;
        shell.workspace = Some(two_documents(true));
        shell.session_named_read(session_file).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while shell.session.named.is_some() {
            drive(&mut shell, &mut renderer);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let workspace = shell.workspace.as_ref().unwrap();
        assert_eq!(
            workspace.message.as_deref(),
            Some("The session was not loaded because documents are still open.")
        );
        assert_eq!(workspace.editors.len(), 1);
        assert!(workspace.editors[0].dirty());
        assert!(shell.session.queue.is_none());
        drop(shell);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_tab_the_user_chose_during_the_restore_keeps_focus() {
        let tab = |id| SessionTab {
            id,
            document_id: id,
            pinned: false,
            view: ViewState::default(),
        };
        let document = |id| SessionDocument {
            id,
            path: None,
            title: format!("Untitled {id}"),
        };
        for user_chose in [false, true] {
            let mut shell = crate::windows_app::accessibility::tests::headless_shell();
            let mut workspace =
                Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
            workspace.new_document().unwrap();
            workspace.new_document().unwrap();
            let saved = workspace.editors[0].document_identity().0;
            let other = workspace.editors[1].document_identity().0;
            shell.session.restored = (0..2)
                .map(|index| Restored {
                    tab: tab(index as u64 + 1),
                    snapshot: CapturedDocument::new(&workspace.editors[index]),
                })
                .collect();
            let manifest = SessionManifest {
                documents: vec![document(1), document(2)],
                tabs: vec![tab(1), tab(2)],
                active_tab: Some(1),
                ..Default::default()
            };
            shell.session.queue = Some(RestoreQueue::new(manifest).unwrap());
            shell.workspace = Some(workspace);
            shell.app.tabs = shell.workspace.as_ref().unwrap().titles();
            shell.app.active = 1;
            if user_chose {
                shell.session.note_user_focus();
            }
            shell.session_finish_restore();
            // The saved active tab is applied only if the user has not chosen one
            // while the restore ran (APP-07).
            let expected = if user_chose { other } else { saved };
            assert_eq!(shell.active_document(), Some(expected), "user chose: {user_chose}");
            // After the restore, tab choices are ordinary and no longer recorded.
            shell.session.note_user_focus();
            assert_eq!(shell.session.user_focused, user_chose);
        }
    }

    /// Plays the main window: winit hands each routed WM_CLOSE to the idle handler.
    struct MainWindow<'a> {
        shell: &'a mut Shell,
        ordinary_closes: u32,
    }
    impl bareline_platform_windows::SessionEndHost for MainWindow<'_> {
        fn deliver(&mut self) {
            if !self.shell.session_end_event(&WindowEvent::CloseRequested) {
                self.ordinary_closes += 1;
            }
        }
        fn block(&mut self) -> bool {
            true
        }
        fn unblock(&mut self) {}
    }

    #[test]
    fn query_end_session_writes_the_session_and_makes_unsaved_text_recoverable() {
        let root = std::env::temp_dir().join(format!(
            "bareline-session-end-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        shell.first_frame = true;
        shell.session.configure(Some(root.join("session.json")), None, true);
        // Headroom for the process-global recovery worker under a loaded
        // parallel run; the flush still has to settle on its own.
        shell.session.end_budget = Duration::from_secs(60);
        let mut workspace =
            Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.recovery_root = Some(root.join("recovery"));
        workspace.new_document().unwrap();
        // Typed just before logoff: neither acknowledged nor checkpointed yet.
        workspace.editors[0].enqueue(Input::Insert("unsaved at logoff".into()));
        shell.workspace = Some(workspace);
        let signal = shell.session.end.clone();
        signal.set_dirty(true);
        let mut window = MainWindow {
            shell: &mut shell,
            ordinary_closes: 0,
        };
        assert_eq!(
            signal.respond(&mut window, bareline_platform_windows::SessionEndMessage::Query),
            1
        );
        assert_eq!(
            signal.respond(
                &mut window,
                bareline_platform_windows::SessionEndMessage::End { ending: true }
            ),
            0
        );
        assert_eq!(
            window.ordinary_closes, 0,
            "a routed flush must not start an ordinary close"
        );
        assert!(!shell.session.closing());
        let workspace = shell.workspace.as_ref().unwrap();
        assert!(workspace.editors[0].dirty());
        assert!(workspace.recovery_settled());
        let status = workspace.editors[0].recovery_status();
        assert!(status.error.is_none(), "{:?}", status.error);
        assert!(status.durable.is_some() && status.complete);
        let loaded = bareline_file_io::session::SessionStore::new(root.join("session.json"))
            .load()
            .unwrap();
        assert_eq!(loaded.manifest.documents.len(), 1);
        assert!(loaded.manifest.documents[0].path.is_none());
        drop(shell);
        let _ = std::fs::remove_dir_all(root);
    }
}

impl Shell {
    fn session_resolve_languages(&mut self) {
        if !self.language.controller.catalog_ready() {
            return;
        }
        let catalog = &self.language.controller;
        let resolve = |editor: &mut bareline_editor_surface::EditorSurface| {
            let Some(selection) = editor.pending_session_language.take() else {
                return;
            };
            // An explicit selection made after restore supersedes its deferred catalog lookup.
            if editor.udl.is_some() || editor.language_override.is_some() {
                return;
            }
            if let bareline_file_io::session::LanguageSelection::Udl(id) = selection {
                if let Some(definition) = catalog.definition_by_id(&id) {
                    editor.udl = Some(definition);
                } else {
                    editor.language = bareline_syntax::Language::PlainText;
                    editor.language_override = Some(bareline_syntax::Language::PlainText);
                    editor.error = Some(format!(
                        "Saved user language {id} is unavailable; using plain text. Import its definition explicitly to select it again."
                    ));
                }
            }
        };
        if let Some(workspace) = &mut self.workspace {
            for editor in &mut workspace.editors {
                resolve(editor.viewport_mut());
            }
        }
        if let Some(editor) = &mut self.views.secondary {
            resolve(editor.viewport_mut());
        }
    }
}
