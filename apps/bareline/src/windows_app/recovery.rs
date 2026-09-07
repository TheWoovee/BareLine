// SPDX-License-Identifier: MPL-2.0
//! Native consumer for background recovery discovery and exact-directory restore.
use super::*;
use std::sync::Arc;
use bareline_renderer::DrawOp;
use bareline_ui::{rect,text};
use bareline_platform::LocalFileSystem;
use std::sync::mpsc::{self, Receiver, TryRecvError};
type RecoveryDiscovery =
    Result<Vec<(PathBuf, bareline_file_io::recovery::RecoveryInspection)>, String>;
#[derive(Default)]
pub(super) struct RecoveryRuntime {
    root: Option<PathBuf>,
    started: bool,
    pending: Option<Receiver<RecoveryDiscovery>>,
    entries: Vec<(PathBuf, bareline_file_io::recovery::RecoveryInspection)>,
    cancellation: bareline_file_io::cancellation::Cancellation,
    last_status: Option<String>,
    open: bool,
    selected: usize,
    confirm_discard: Option<PathBuf>,
    hits: Vec<(bareline_renderer::Rect, String)>,
    operation: Option<Receiver<Result<Option<PathBuf>, String>>>,
    preview_path: Option<PathBuf>,
    preview: Option<Receiver<Result<String, String>>>,
    preview_text: String,
    preview_cancellation: bareline_file_io::cancellation::Cancellation,
}
impl RecoveryRuntime {
    pub(super) fn configure(&mut self, root: Option<PathBuf>) {
        self.cancellation.cancel();
        self.preview_cancellation.cancel();
        self.root = root;
        self.started = false;
        self.pending = None;
        self.entries.clear();
        self.cancellation = Default::default();
    }
}
impl Drop for RecoveryRuntime {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.preview_cancellation.cancel();
    }
}
impl Shell {
    pub(super) fn recovery_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        match id {
            "recovery.open" => {
                self.recovery.open = true;
                self.recovery.started = false;
            }
            "recovery.keep" => {
                self.recovery.open = false;
                self.recovery.preview_cancellation.cancel();
                self.recovery.confirm_discard = None;
            }
            "recovery.restore_selected" => {
                if let Some((directory, _)) =
                    self.recovery.entries.get(self.recovery.selected).cloned()
                    && self.ensure_workspace(el)
                {
                    self.workspace
                        .as_mut()
                        .unwrap()
                        .restore_paged_recovery(directory);
                    self.recovery.open = false;
                }
            }
            "recovery.discard" => {
                self.recovery.confirm_discard = self
                    .recovery
                    .entries
                    .get(self.recovery.selected)
                    .map(|(path, _)| path.clone());
            }
            "recovery.confirm_discard" | "recovery.export" => {
                if self.recovery.operation.is_some() {
                    return true;
                }
                let discard = id == "recovery.confirm_discard";
                let directory = if discard {
                    self.recovery.confirm_discard.take()
                } else {
                    self.recovery
                        .entries
                        .get(self.recovery.selected)
                        .map(|(path, _)| path.clone())
                };
                let destination = if discard {
                    None
                } else {
                    self.platform
                        .as_ref()
                        .and_then(|p| p.pick_folder().ok().flatten())
                };
                if let Some(directory) = directory
                    && (discard || destination.is_some())
                {
                    let (tx, rx) = mpsc::sync_channel(1);
                    let notify = self.notify.clone();
                    let cancel = self.recovery.cancellation.clone();
                    if std::thread::Builder::new()
                        .name("recovery-action".into())
                        .spawn(move || {
                            let result = (|| {
                                let platform = bareline_platform_windows::WindowsFileSystem;
                                let _guard = platform
                                    .guard_directory(&directory)
                                    .map_err(|e| e.to_string())?;
                                if discard {
                                    bareline_file_io::recovery::discard(&directory, &platform)
                                        .map_err(|e| e.to_string())?;
                                    Ok(Some(directory))
                                } else {
                                    let parent = destination.unwrap();
                                    let _destination_guard = platform
                                        .guard_directory(&parent)
                                        .map_err(|e| e.to_string())?;
                                    let export = parent.join(format!(
                                        "recovery-edits-{}",
                                        std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_nanos()
                                    ));
                                    bareline_file_io::recovery::export_edits(
                                        &directory, &export, &cancel,
                                    )
                                    .map_err(|e| e.to_string())?;
                                    Ok(None)
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
                    self.workspace
                        .as_mut()
                        .unwrap()
                        .restore_paged_recovery(directory);
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
                    self.workspace
                        .as_mut()
                        .unwrap()
                        .restore_paged_recovery(directory);
                } else if let Some(workspace) = &mut self.workspace {
                    workspace.message=Some("No complete recovery checkpoint was found. Open a recovery folder to inspect it.".into());
                }
            }
            "recovery.retry" => {
                if let Some(workspace) = &mut self.workspace {
                    workspace.message = workspace
                        .editors
                        .get_mut(self.app.active)
                        .and_then(|editor| editor.retry_recovery().err());
                }
            }
            "recovery.save_as" => self.dispatch(el, Action::SaveAs),
            _ => {
                if let Some(index) = id
                    .strip_prefix("recovery.select.")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    self.recovery.selected =
                        index.min(self.recovery.entries.len().saturating_sub(1));
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
    pub(super) fn recovery_pump(&mut self, _el: &ActiveEventLoop) {
        let selected_preview = self
            .recovery
            .entries
            .get(self.recovery.selected)
            .map(|(path, _)| path.clone());
        if !self.recovery.open || selected_preview != self.recovery.preview_path {
            self.recovery.preview_cancellation.cancel();
        }
        if self.recovery.open && self.recovery.preview.is_none() {
            let path = self
                .recovery
                .entries
                .get(self.recovery.selected)
                .map(|(path, _)| path.clone());
            if path != self.recovery.preview_path {
                self.recovery.preview_path = path.clone();
                if let Some(path) = path {
                    self.recovery.preview_text = "Preparing bounded preview…".into();
                    let (tx, rx) = mpsc::sync_channel(1);
                    let notify = self.notify.clone();
                    self.recovery.preview_cancellation = Default::default();
                    let cancel = self.recovery.preview_cancellation.clone();
                    if std::thread::Builder::new().name("recovery-preview".into()).spawn(move || {
                        let result=(|| {
                            let bytes=bareline_document::Budget::new(256<<20);
                            let mut opened=bareline_file_io::paged_recovery::restore(&path,Arc::new(bareline_platform_windows::WindowsFileSystem),bytes.clone(),bareline_document::Budget::new(0),&cancel)?;
                            let snapshot=opened.transcoded.document.snapshot();
                            let mut request=snapshot.begin_viewport(bareline_document::TextOffset(0),8192,&bytes).map_err(|e|format!("{e:?}"))?;
                            loop {
                                cancel.check().map_err(|e|format!("{e:?}"))?;
                                match request.poll() {
                                    bareline_document::paged::WindowPoll::Ready(window)=>return Ok(window.text().to_string()),
                                    bareline_document::paged::WindowPoll::Pending(ticket)=>opened.transcoded.source.read_page(ticket).map_err(|e|format!("{e:?}"))?,
                                    _=>return Err("Preview range unavailable; export validated saved edits with its gap report.".into()),
                                }
                            }
                        })(); let _=tx.send(result);notify();
                    }).is_ok(){self.recovery.preview=Some(rx);}
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
                    let message = match result {
                        Ok(Ok(Some(path))) => {
                            self.recovery.entries.retain(|(entry, _)| *entry != path);
                            "Recovery discarded.".into()
                        }
                        Ok(Ok(None)) => {
                            "Saved edits exported with gaps.json in the selected folder.".into()
                        }
                        Ok(Err(error)) => format!("Recovery action failed: {error}"),
                        Err(_) => "Recovery worker stopped; checkpoint retained.".into(),
                    };
                    if let Some(workspace) = &mut self.workspace {
                        workspace.message = Some(message);
                    }
                    self.recovery.selected = self
                        .recovery
                        .selected
                        .min(self.recovery.entries.len().saturating_sub(1));
                }
            }
        }
        if !self.recovery.started {
            self.recovery.started = true;
            if let Some(root) = self.recovery.root.clone() {
                let (tx, rx) = mpsc::sync_channel(1);
                let cancel = self.recovery.cancellation.clone();
                let notify = self.notify.clone();
                if std::thread::Builder::new()
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
                            let mut entries = Vec::new();
                            for entry in std::fs::read_dir(&root)
                                .map_err(|e| e.to_string())?
                                .take(256)
                            {
                                cancel.check().map_err(|e| format!("{e:?}"))?;
                                let entry = entry.map_err(|e| e.to_string())?;
                                if !entry.file_name().to_string_lossy().starts_with("paged-") {
                                    continue;
                                }
                                let directory = entry.path();
                                let Ok(_guard) = platform.guard_directory(&directory) else {
                                    continue;
                                };
                                if let Ok(inspection) =
                                    bareline_file_io::recovery::inspect(&directory, &cancel)
                                    && inspection.status
                                        != bareline_file_io::recovery::RecoveryStatus::Discarded
                                {
                                    entries.push((directory, inspection));
                                }
                            }
                            entries.sort_by_key(|(_, inspection)| {
                                std::cmp::Reverse(
                                    inspection
                                        .last_durable
                                        .map_or(0, |receipt| receipt.protected_unix_ms),
                                )
                            });
                            Ok(entries)
                        })();
                        let _ = tx.send(result);
                        notify();
                    })
                    .is_ok()
                {
                    self.recovery.pending = Some(rx);
                }
            }
        }
        if let Some(receiver) = &self.recovery.pending {
            match receiver.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.recovery.pending = None;
                    match result {
                        Ok(Ok(entries)) => {
                            let count = entries.len();
                            self.recovery.entries = entries;
                            self.recovery.open |= count > 0;
                            if count > 0
                                && let Some(workspace) = &mut self.workspace
                            {
                                workspace.message = Some(format!(
                                    "{count} recovery checkpoint(s) available. Use Restore Latest Recovery or Open Recovery Folder."
                                ));
                            }
                        }
                        Ok(Err(error)) => {
                            if let Some(workspace) = &mut self.workspace {
                                workspace.message =
                                    Some(format!("Recovery discovery unavailable: {error}"));
                            }
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        if let Some(workspace) = &mut self.workspace
            && let Some(editor) = workspace.editors.get(self.app.active)
        {
            let status = editor.recovery_status();
            let message = if let Some(error) = status.error {
                Some(format!(
                    "Recovery unavailable: {error}. Retry Recovery or Save As."
                ))
            } else if status.directory.is_some() && !status.complete {
                Some("Recovery snapshot preparing; editing remains available.".into())
            } else {
                None
            };
            if message != self.recovery.last_status {
                if workspace.message == self.recovery.last_status {
                    workspace.message = None;
                }
                self.recovery.last_status = message.clone();
                if let Some(message) = message {
                    workspace.message = Some(message);
                }
            }
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
                if let Some((_, id)) = self.recovery.hits.iter().find(|(r, _)| r.contains(pointer))
                {
                    let id = id.clone();
                    self.recovery_dispatch(el, &id);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        self.recovery.confirm_discard = None;
                        self.recovery.open = false;
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        self.recovery.selected = (self.recovery.selected + 1)
                            .min(self.recovery.entries.len().saturating_sub(1))
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        self.recovery.selected = self.recovery.selected.saturating_sub(1)
                    }
                    Key::Named(NamedKey::Enter) => {
                        self.recovery_dispatch(el, "recovery.restore_selected");
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
    pub(super) fn draw(
        &mut self,
        theme: bareline_ui::theme::UiTheme,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) {
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
        let visible = ((height - 380.0).max(40.0) / 42.0) as usize;
        let start = self.selected.saturating_sub(visible.saturating_sub(1));
        for (row, (index, (path, inspection))) in self
            .entries
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .enumerate()
        {
            let bounds = rect(20.0, 90.0 + row as f32 * 42.0, width - 40.0, 38.0);
            if index == self.selected {
                ops.push(DrawOp::Stroke(bounds, theme.focus, 1.0));
            }
            let name = inspection
                .metadata
                .original_path
                .as_ref()
                .unwrap_or(path)
                .to_string_lossy();
            text(
                ops,
                30.0,
                bounds.y + 8.0,
                &format!(
                    "{} � {:?} � protected {}",
                    name,
                    inspection.status,
                    inspection.last_durable.map_or(0, |r| r.protected_unix_ms)
                ),
                13.0,
                theme.text,
            );
            self.hits.push((bounds, format!("recovery.select.{index}")));
        }
        let preview_y = (height - 270.0).max(145.0);
        text(
            ops,
            24.0,
            preview_y,
            "Recovered copy — first 8 KiB",
            14.0,
            theme.text,
        );
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
        let y = (height - 70.0).max(140.0);
        if let Some(path) = &self.confirm_discard {
            text(
                ops,
                24.0,
                y - 30.0,
                &format!(
                    "Permanently discard {}? This cannot be undone.",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                13.0,
                theme.text,
            );
        } else if let Some((_, inspection)) = self.entries.get(self.selected) {
            let warning = match inspection.status {
                bareline_file_io::recovery::RecoveryStatus::Complete => {
                    "Open recovered copy preserves the original file. Save As chooses a new destination."
                }
                _ => {
                    "Only validated data is available. Export saved edits includes an explicit gap report."
                }
            };
            text(ops, 24.0, y - 30.0, warning, 13.0, theme.text);
        }
        let actions = if self.confirm_discard.is_some() {
            vec![
                ("Confirm irreversible discard", "recovery.confirm_discard"),
                ("Keep recovery", "recovery.keep"),
            ]
        } else {
            vec![
                ("Open recovered copy", "recovery.restore_selected"),
                ("Export saved edits", "recovery.export"),
                ("Discard�", "recovery.discard"),
                ("Keep recovery", "recovery.keep"),
            ]
        };
        for (index, (label, id)) in actions.iter().enumerate() {
            let bounds = rect(24.0 + index as f32 * 185.0, y, 178.0, 34.0);
            ops.push(DrawOp::Stroke(bounds, theme.focus, 1.0));
            text(ops, bounds.x + 8.0, bounds.y + 8.0, *label, 12.0, theme.text);
            self.hits.push((bounds, (*id).into()));
        }
        if self.entries.is_empty() {
            text(
                ops,
                24.0,
                100.0,
                "No recovery checkpoints found.",
                14.0,
                theme.text,
            );
        }
    }
}

fn center_consumes_event(event: &WindowEvent) -> bool {
    matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn center_preserves_window_lifecycle_and_consumes_text_input() {
        for event in [WindowEvent::RedrawRequested, WindowEvent::CloseRequested, WindowEvent::Focused(true), WindowEvent::Resized(winit::dpi::PhysicalSize::new(800,600))] {
            assert!(!center_consumes_event(&event));
        }
        assert!(center_consumes_event(&WindowEvent::Ime(winit::event::Ime::Commit("x".into()))));
    }
}
