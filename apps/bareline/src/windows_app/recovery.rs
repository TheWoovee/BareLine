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
    focus: Option<usize>,
    pending_compare: Option<PendingCompare>,
    confirm_discard: Option<PathBuf>,
    hits: Vec<(bareline_renderer::Rect, String)>,
    operation: Option<Receiver<Result<Option<PathBuf>, String>>>,
    preview_path: Option<PathBuf>,
    preview: Option<Receiver<Result<String, String>>>,
    preview_text: String,
    preview_cancellation: bareline_file_io::cancellation::Cancellation,
}
impl RecoveryRuntime {
    fn action_enabled(&self, command:&str)->bool {
        let selected=self.entries.get(self.selected);
        match command {
            "recovery.restore_selected"=>selected.is_some_and(|(_,inspection)|inspection.complete_baseline),
            "recovery.compare"=>selected.is_some_and(|(_,inspection)|inspection.complete_baseline&&inspection.metadata.original_path.is_some()),
            "recovery.export"|"recovery.discard"=>selected.is_some()&&self.operation.is_none(),
            "recovery.confirm_discard"=>self.confirm_discard.is_some()&&self.operation.is_none(),
            _=>true,
        }
    }
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
        if !self.recovery.action_enabled(id) {
            if let Some(workspace)=&mut self.workspace {workspace.message=Some("This recovery action needs a selected available checkpoint; incomplete snapshots can export saved edits with a gap report.".into());}
            return true;
        }
        match id {
            "recovery.open" => {
                if !self.ensure_workspace(el) {
                    return true;
                }
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
            "recovery.compare" => {
                if self.recovery.pending_compare.is_some() {
                    return true;
                }
                if let Some((directory, inspection)) =
                    self.recovery.entries.get(self.recovery.selected).cloned()
                    && self.ensure_workspace(el)
                {
                    let workspace = self.workspace.as_mut().unwrap();
                    if let Some(original) = inspection.metadata.original_path {
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
                        self.recovery.open = false;
                    } else {
                        workspace.message=Some("This recovery has no original disk path. Open the recovered copy or export saved edits.".into());
                    }
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
        if self.recovery.pending_compare.is_some()
            && self.workspace.as_ref().is_some_and(|w| !w.io_busy())
        {
            let mut pending = self.recovery.pending_compare.take().unwrap();
            let workspace = self.workspace.as_mut().unwrap();
            let new = workspace
                .editors
                .iter()
                .enumerate()
                .find(|(index, editor)| !pending.existing.iter().any(|id| id.matches(editor)) && if pending.recovered.is_none() {
                    matches!(editor, bareline_app::workspace::WorkspaceEditor::Paged(paged) if paged.recovery_origin_path()==Some(pending.directory.as_path()))
                } else { workspace.path(*index)==Some(pending.original.as_path()) && !editor.dirty() })
                .map(|(index, _)| index);
            if let Some(index) = new {
                workspace.editors[index].set_read_only(true);
                if let Some(recovered) = pending.recovered {
                    let left = workspace
                        .editors
                        .iter()
                        .position(|editor| recovered.matches(editor));
                    if workspace.path(index) == Some(pending.original.as_path()) {
                        if let Some(left) = left {
                            self.compare_recovery_pair(left, index);
                        }
                    } else {
                        workspace.message = Some(
                            "Current disk compare source did not open; recovered copy is retained."
                                .into(),
                        );
                    }
                } else {
                    pending.recovered = Some(super::lifecycle::Identity::capture(
                        &workspace.editors[index],
                    ));
                    pending.existing = workspace
                        .editors
                        .iter()
                        .map(super::lifecycle::Identity::capture)
                        .collect();
                    workspace.open(pending.original.clone());
                    self.recovery.pending_compare = Some(pending);
                }
                self.app.tabs = self.workspace.as_ref().unwrap().titles();
            } else if workspace.message.is_none() {
                workspace.message = Some("Recovery comparison could not open its source.".into());
            }
        }
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
                            for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
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
                                    if entries.len() == 256 {
                                        break;
                                    }
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
                            if count > 0 { self.ensure_workspace(_el); }
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
                        self.recovery.confirm_discard = None;
                        self.recovery.open = false;
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        self.recovery.focus = None;
                        self.recovery.selected = (self.recovery.selected + 1)
                            .min(self.recovery.entries.len().saturating_sub(1))
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        self.recovery.focus = None;
                        self.recovery.selected = self.recovery.selected.saturating_sub(1)
                    }
                    Key::Named(NamedKey::Tab) => {
                        self.recovery.focus = next_recovery_focus(
                            self.recovery.focus,
                            self.recovery.hits.len(),
                            self.modifiers.shift_key(),
                        );
                    }
                    Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                        let id = self
                            .recovery
                            .focus
                            .and_then(|index| self.recovery.hits.get(index))
                            .map(|(_, id)| id.clone())
                            .unwrap_or_else(|| "recovery.restore_selected".into());
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
                    "{} | {} | protected {}",
                    name,
                    recovery_state_label(inspection.status),
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
                ("Compare with disk", "recovery.compare"),
                ("Export saved edits", "recovery.export"),
                ("Discard...", "recovery.discard"),
                ("Keep recovery", "recovery.keep"),
            ]
        };
        for (index, (label, id)) in actions.iter().enumerate() {
            let button_width = ((width - 48.0) / actions.len() as f32).max(80.0);
            let bounds = rect(
                24.0 + index as f32 * button_width,
                y,
                button_width - 6.0,
                34.0,
            );
            ops.push(DrawOp::Stroke(bounds, theme.focus, 1.0));
            text(
                ops,
                bounds.x + 8.0,
                bounds.y + 8.0,
                *label,
                12.0,
                theme.text,
            );
            self.hits.push((bounds, (*id).into()));
        }
        if let Some((bounds, _)) = self.focus.and_then(|index| self.hits.get(index)) {
            ops.push(DrawOp::Stroke(*bounds, theme.focus, 2.0));
        }
        if self.entries.len() == 256 {
            text(
                ops,
                24.0,
                75.0,
                "Showing at most 256 recoverable checkpoints; Open Recovery Folder accesses an exact additional item.",
                12.0,
                theme.muted,
            );
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

fn next_recovery_focus(current: Option<usize>, count: usize, backwards: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match current {
        None => {
            if backwards {
                count - 1
            } else {
                0
            }
        }
        Some(index) => {
            if backwards {
                (index % count + count - 1) % count
            } else {
                (index % count + 1) % count
            }
        }
    })
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn center_actions_share_keyboard_and_accessibility_targets() {
        let mut runtime = RecoveryRuntime::default();
        runtime.open = true;
        let mut ops = Vec::new();
        runtime.draw(Default::default(), 1000.0, 600.0, &mut ops);
        let mut focus = None;
        for (_, command) in &runtime.hits {
            focus = next_recovery_focus(focus, runtime.hits.len(), false);
            assert_eq!(&runtime.hits[focus.unwrap()].1, command);
        }
        assert!(
            runtime
                .hits
                .iter()
                .any(|(_, command)| command == "recovery.compare")
        );
        assert_eq!(
            next_recovery_focus(focus, runtime.hits.len(), false),
            Some(0)
        );
        assert_eq!(
            next_recovery_focus(Some(0), runtime.hits.len(), true),
            Some(runtime.hits.len() - 1)
        );
        let mut ids: Vec<_> = runtime
            .hits
            .iter()
            .map(|(_, command)| recovery_action_id(command))
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
        runtime.confirm_discard = Some(PathBuf::from("named-recovery"));
        runtime.draw(Default::default(), 1000.0, 600.0, &mut ops);
        assert!(
            runtime
                .hits
                .iter()
                .any(|(_, command)| command == "recovery.confirm_discard")
        );
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
        assert!(center_consumes_event(&WindowEvent::Ime(
            winit::event::Ime::Commit("x".into())
        )));
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
            _ => 99,
        }
}
impl Shell {
    pub(super) fn recovery_accessibility_nodes(
        &self,
    ) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        if !self.recovery.open {
            return Vec::new();
        }
        let origin = self.editor_bounds();
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
        for (bounds, command) in &self.recovery.hits {
            let row = command
                .strip_prefix("recovery.select.")
                .and_then(|s| s.parse::<usize>().ok());
            let name = if let Some(index) = row {
                self.recovery
                    .entries
                    .get(index)
                    .map(|(path, inspection)| {
                        format!(
                            "{}; {}; last protected {}",
                            inspection
                                .metadata
                                .original_path
                                .as_ref()
                                .unwrap_or(path)
                                .display(),
                            recovery_state_label(inspection.status),
                            inspection.last_durable.map_or(0, |r| r.protected_unix_ms)
                        )
                    })
                    .unwrap_or_else(|| "Recovery checkpoint".into())
            } else {
                match command.as_str() {
                    "recovery.restore_selected" => "Open recovered copy",
                    "recovery.compare" => "Compare with current disk",
                    "recovery.export" => "Export saved edits and gap report",
                    "recovery.discard" => "Discard recovery",
                    "recovery.keep" => "Keep recovery",
                    "recovery.confirm_discard" => "Confirm irreversible discard",
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
                disabled: !self.recovery.action_enabled(command),
                selected: row == Some(self.recovery.selected),
                expanded: None,
                focusable: self.recovery.action_enabled(command),
                invokable: self.recovery.action_enabled(command),
            });
        }
        nodes.push(AccessibilityNode {
            id: 100_901,
            parent: 100_900,
            role: AccessibilityRole::Status,
            name: if let Some(path) = &self.recovery.confirm_discard {
                format!(
                    "Permanently discard {}? This cannot be undone.",
                    path.display()
                )
            } else {
                "Recovered copy preview".into()
            },
            value: Some(self.recovery.preview_text.clone()),
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
        nodes
    }
    pub(super) fn recovery_accessibility_focus(&self) -> Option<u64> {
        self.recovery.open.then(|| {
            self.recovery
                .focus
                .and_then(|index| self.recovery.hits.get(index))
                .map_or(
                    if self.recovery.entries.is_empty() {
                        100_504
                    } else {
                        100_000 + self.recovery.selected as u64
                    },
                    |(_, command)| recovery_action_id(command),
                )
        })
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
        if !self.recovery.action_enabled(&command){return false;}
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
