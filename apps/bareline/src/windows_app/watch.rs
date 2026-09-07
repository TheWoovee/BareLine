// SPDX-License-Identifier: MPL-2.0
//! Native composition only. Bounded background metadata reconciliation never replaces buffers.
use super::*;
use bareline_platform::{FileIdentity, LocalFileSystem, PathOrigin, PathTrustProvider};
use bareline_platform_windows::{WindowsFileSystem, WindowsPathTrustProvider, WindowsWatchService};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::mpsc::{self, Receiver},
};

type Registration = Result<Option<WindowsWatchService>, String>;
type Checked = (PathBuf, FileIdentity, Result<bool, String>);
#[derive(Default)]
pub(super) struct WatchRuntime {
    service: Option<WindowsWatchService>,
    setup: Option<Receiver<Registration>>,
    registered: Vec<PathBuf>,
    desired: Vec<PathBuf>,
    queue: VecDeque<(PathBuf, FileIdentity)>,
    checking: Option<Receiver<Vec<Checked>>>,
    conflicts: BTreeSet<PathBuf>,
    requested: bool,
    reopen_follow: BTreeSet<PathBuf>,
}
pub(super) fn commands() -> Vec<bareline_commands::CommandSpec> {
    [
        ("file.external.check", "Check for External Changes"),
        ("file.external.keep", "Keep Current Buffer"),
        ("file.external.reload", "Reload External Changes"),
        ("file.monitor.start", "Follow New Content"),
        ("file.monitor.pause", "Pause Following Scroll"),
        ("file.monitor.resume", "Resume Following"),
        ("file.monitor.reopen", "Reopen and Follow"),
        ("file.monitor.unlock", "Unlock to Edit (Stop Monitoring)"),
    ]
    .into_iter()
    .map(|(id, title)| bareline_commands::CommandSpec {
        id: bareline_commands::CommandId(id),
        title,
        category: "File",
        shortcut: "",
        action: Action::Contributed(bareline_commands::CommandId(id)),
    })
    .collect()
}
impl Shell {
    pub(super) fn watch_start_follow(&mut self, index: usize) -> Result<(), String> {
        let w = self.workspace.as_mut().ok_or("Workspace unavailable")?;
        if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = w.editors.get_mut(index) {
            return editor.start_follow(std::sync::Arc::new(WindowsFileSystem));
        }
        let editor = w.editors.get_mut(index).ok_or("Document unavailable")?;
        if editor.dirty() { return Err("Save or discard edits before monitoring.".into()); }
        let was_read_only = editor.user_read_only; editor.user_read_only = false;
        let path = w.path(index).ok_or("Save this file before monitoring")?.to_owned();
        let threshold = w.resident_max_bytes; w.resident_max_bytes = 0;
        let result = w.reload(index, false); w.resident_max_bytes = threshold;
        w.editors[index].user_read_only = was_read_only;
        result?;
        self.watch.reopen_follow.insert(path);
        Ok(())
    }
    pub(super) fn watch_sync(&mut self) {
        let mut paths = self.workspace_watch_roots();
        if let Some(w) = &self.workspace {
            for index in 0..w.editors.len() {
                if let Some(path) = w.path(index)
                    && let Some(parent) = path.parent()
                {
                    paths.push(parent.to_owned());
                }
            }
        }
        paths.sort();
        paths.dedup();
        if paths.len() > 63 {
            paths.truncate(63);
            if let Some(w) = &mut self.workspace { w.message = Some("Watching 63 directories. Additional directories reconcile on focus or Check for External Changes.".into()); }
        }
        self.watch.desired = paths;
        if self.watch.setup.is_some() || self.watch.desired == self.watch.registered {
            return;
        }
        let desired = self.watch.desired.clone();
        let previous = self.watch.service.take();
        let notify = self.notify.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("bareline-watch-setup".into())
            .spawn(move || {
                drop(previous);
                let result = if desired.is_empty() {
                    Ok(None)
                } else {
                    WindowsWatchService::start_notifying(desired, notify.clone())
                        .map(Some)
                        .map_err(|e| e.to_string())
                };
                let _ = tx.send(result);
                notify();
            }) {
            Ok(_) => {
                self.watch.registered = self.watch.desired.clone();
                self.watch.setup = Some(rx);
            }
            Err(e) => {
                if let Some(w) = &mut self.workspace {
                    w.message = Some(format!("File watching unavailable: {e}"));
                }
            }
        }
    }
    pub(super) fn watch_dispatch(&mut self, _: &ActiveEventLoop, id: &str) -> bool {
        match id {
            "file.monitor.start" => {
                if let Err(error) = self.watch_start_follow(self.app.active) { if let Some(w) = &mut self.workspace { w.message = Some(error); } }
                true
            }
            "file.monitor.pause" | "file.monitor.resume" => {
                if let Some(w) = &mut self.workspace && let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = w.editors.get_mut(self.app.active) {
                    editor.set_follow_paused(id == "file.monitor.pause");
                    if id == "file.monitor.resume" { let _ = editor.request_viewport(bareline_document::TextOffset(editor.snapshot().len().saturating_sub(32768))); }
                }
                true
            }
            "file.monitor.unlock" => {
                let confirmed = self.platform.as_ref().is_some_and(|p| p.confirm_discard_document("Stop monitoring and capture a fixed generation for editing?"));
                if confirmed && let Some(w) = &mut self.workspace && let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = w.editors.get_mut(self.app.active) {
                    if let Err(error) = editor.unlock_follow(true) { w.message = Some(error); }
                }
                true
            }
            "file.monitor.reopen" => {
                if let Some(w) = &mut self.workspace && let Some(editor) = w.editors.get_mut(self.app.active) {
                    editor.user_read_only = false;
                    let path = w.path(self.app.active).map(|p| p.to_owned());
                    match w.reload(self.app.active, false) {
                        Err(error) => { w.editors[self.app.active].user_read_only = true; w.message = Some(error); }
                        Ok(()) => { if let Some(path) = path { self.watch.reopen_follow.insert(path); } }
                    }
                }
                true
            }
            "file.external.check" => {
                self.watch.requested = true;
                if self.watch.service.is_none() && self.watch.setup.is_none() {
                    self.watch.registered.clear();
                }
                (self.notify)();
                true
            }
            "file.external.reload" => {
                let active = self.app.active;
                let dirty = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.editors.get(active))
                    .is_some_and(|e| e.dirty());
                let name = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.path(active))
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "document".into());
                let confirmed = !dirty
                    || self
                        .platform
                        .as_ref()
                        .is_some_and(|p| p.confirm_discard_document(&name));
                if confirmed
                    && let Some(w) = &mut self.workspace
                    && let Err(e) = w.reload(active, dirty)
                {
                    w.message = Some(e);
                }
                true
            }
            "file.external.keep" => {
                if let Some(w) = &mut self.workspace {
                    if let Some(path) = w.path(self.app.active) {
                        self.watch.conflicts.remove(path);
                    }
                    w.message=Some("Current buffer kept. Save will still check the disk version; use Save As to preserve both.".into());
                }
                true
            }
            _ => false,
        }
    }
    pub(super) fn watch_pump(&mut self, _: &ActiveEventLoop) {
        let pending: Vec<_> = self.watch.reopen_follow.iter().cloned().collect();
        for path in pending {
            let index = self.workspace.as_ref().and_then(|w| (0..w.editors.len()).find(|&i| w.path(i) == Some(path.as_path()) && matches!(&w.editors[i], bareline_app::workspace::WorkspaceEditor::Paged(e) if e.follow_status().is_none() && !e.busy())));
            if let Some(index) = index {
                if self.watch_start_follow(index).is_ok() { self.watch.reopen_follow.remove(&path); }
            }
        }
        if let Some(rx) = &self.watch.setup {
            match rx.try_recv() {
                Ok(result) => {
                    self.watch.setup = None;
                    match result {
                        Ok(service) => {
                            self.watch.service = service;
                            self.watch.requested = true;
                        }
                        Err(e) => {
                            if let Some(w) = &mut self.workspace {
                                w.message = Some(format!(
                                    "File watching unavailable: {e}. Check for External Changes remains available."
                                ));
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.watch.setup = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut events = Vec::new();
        if let Some(service) = &self.watch.service {
            for _ in 0..256 {
                let Some(event) = service.try_recv() else {
                    break;
                };
                events.push(event);
                self.watch.requested = true;
            }
        }
        for event in events { self.workspace_watch_event(&event); }
        if let Some(w) = &mut self.workspace {
            for editor in &mut w.editors {
                if let bareline_app::workspace::WorkspaceEditor::Paged(editor) = editor {
                    if editor.follow_status().is_some() && editor.surface.scroll_y > 0.0 { editor.set_follow_paused(true); }
                    if let Err(error) = editor.follow_tick(std::sync::Arc::new(WindowsFileSystem), self.watch.requested) { editor.error = Some(error); }
                }
            }
        }
        if let Some(rx) = &self.watch.checking {
            match rx.try_recv() {
                Ok(results) => {
                    self.watch.checking = None;
                    if let Some(w) = &mut self.workspace {
                        for (path, expected, result) in results {
                            let index = (0..w.editors.len()).find(|&i| {
                                w.path(i) == Some(path.as_path())
                                    && w.fingerprint(i).is_some_and(|f| f.identity == expected)
                            });
                            if let Some(index) = index
                                && !matches!(&w.editors[index], bareline_app::workspace::WorkspaceEditor::Paged(e) if e.follow_status().is_some())
                                && (result.as_ref().is_err() || result == Ok(true))
                            {
                                self.watch.conflicts.insert(path.clone());
                                let name = path.file_name().unwrap_or_default().to_string_lossy();
                                w.message = Some(if w.editors[index].dirty() {
                                    format!(
                                        "{name} changed outside Bareline. Your edits are preserved. Keep Current Buffer, Save As, or explicitly reload."
                                    )
                                } else {
                                    format!(
                                        "{name} changed or became unavailable outside Bareline. Current bytes are preserved; choose Keep Current Buffer or explicitly reload."
                                    )
                                });
                            }
                        }
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.watch.checking = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.watch.requested && self.watch.queue.is_empty() && self.watch.checking.is_none() {
            self.watch.requested = false;
            if let Some(w) = &self.workspace {
                for i in 0..w.editors.len() {
                    if let (Some(path), Some(f)) = (w.path(i), w.fingerprint(i)) {
                        self.watch.queue.push_back((path.to_owned(), f.identity));
                    }
                }
            }
        }
        if self.watch.checking.is_none() && !self.watch.queue.is_empty() {
            let batch: Vec<_> = (0..16)
                .filter_map(|_| self.watch.queue.pop_front())
                .collect();
            let notify = self.notify.clone();
            let (tx, rx) = mpsc::sync_channel(1);
            match std::thread::Builder::new()
                .name("bareline-watch-check".into())
                .spawn(move || {
                    let results = batch
                        .into_iter()
                        .map(|(path, expected)| {
                            let result = WindowsPathTrustProvider
                                .open_read(&path, PathOrigin::User)
                                .and_then(|opened| WindowsFileSystem.identity(&opened.file))
                                .map(|actual| actual != expected)
                                .map_err(|e| e.to_string());
                            (path, expected, result)
                        })
                        .collect();
                    let _ = tx.send(results);
                    notify();
                }) {
                Ok(_) => self.watch.checking = Some(rx),
                Err(e) => {
                    if let Some(w) = &mut self.workspace {
                        w.message = Some(format!("External-change check unavailable: {e}"));
                    }
                }
            }
        }
        self.watch_sync();
    }
}
