// SPDX-License-Identifier: MPL-2.0
use super::*;
use bareline_app::workspace_panel::{
    PanelAction, WorkspacePanel,
    documents::{DocumentAction, DocumentItem, DocumentList, Sort},
    map::DocumentMap,
    outline::OutlinePanel,
};
use bareline_platform::{LocalFileSystem, PathOperation, PathOrigin, PathTrustProvider};
use bareline_renderer::{DrawOp, LayoutError, Rect, TextBackend};
use bareline_ui::{STATUS_HEIGHT, TAB_HEIGHT, controls::Key as UiKey};
use std::sync::{
    Arc,
    mpsc::{self, Receiver},
};
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Editor,
    Explorer,
    Documents,
    Outline,
}
pub struct WorkspacePanelsRuntime {
    explorer: Option<WorkspacePanel>,
    documents: DocumentList,
    outline: OutlinePanel,
    map: DocumentMap,
    notify: Arc<dyn Fn() + Send + Sync>,
    focus: Focus,
    left: Rect,
    right: Rect,
    map_bounds: Rect,
    root: Option<Receiver<Result<PathBuf, String>>>,
    operation: Option<Receiver<Result<Option<bareline_platform_windows::WorkspaceDeleteUndo>, String>>>,
    deleted: Vec<bareline_platform_windows::WorkspaceDeleteUndo>,
    restoring: bool,
    outline_import: Option<Receiver<Result<(bareline_syntax::outline::Definition, String, String), String>>>,
    document_filter: String,
    outline_filter: String,
    excludes: Vec<String>,
}
impl Default for WorkspacePanelsRuntime {
    fn default() -> Self {
        Self {
            explorer: None,
            documents: DocumentList::default(),
            outline: OutlinePanel::default(),
            map: DocumentMap::default(),
            notify: Arc::new(|| {}),
            focus: Focus::Editor,
            left: Rect::default(),
            right: Rect::default(),
            map_bounds: Rect::default(),
            root: None,
            operation: None,
            deleted: Vec::new(),
            restoring: false,
            outline_import: None,
            document_filter: String::new(),
            outline_filter: String::new(),
            excludes: Vec::new(),
        }
    }
}
impl WorkspacePanelsRuntime {
    fn explorer(&mut self) -> &mut WorkspacePanel {
        self.explorer.get_or_insert_with(|| {
            let mut panel = WorkspacePanel::new(self.notify.clone());
            panel.set_directory_guard(|path| {
                let retained = bareline_platform_windows::WindowsPathTrustProvider
                    .open_read(path, PathOrigin::User)?;
                Ok(Box::new(retained) as Box<dyn Send>)
            });
            panel
        })
    }
    pub fn width_left(&self) -> f32 {
        if self.documents.open || self.explorer.as_ref().is_some_and(|p| p.open) {
            238.0
        } else {
            0.0
        }
    }
    pub fn width_right(&self) -> f32 {
        (if self.outline.open { 240.0 } else { 0.0 }) + if self.map.open { 64.0 } else { 0.0 }
    }
    pub fn draw(
        &mut self,
        renderer: &mut impl TextBackend,
        width: f32,
        height: f32,
        workspace: &Workspace,
        active: usize,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        let panel_height = (height - TAB_HEIGHT - STATUS_HEIGHT).max(0.0);
        self.left = Rect {
            x: 0.0,
            y: TAB_HEIGHT,
            width: self.width_left(),
            height: panel_height,
        };
        self.right = Rect {
            x: width - if self.outline.open { 240.0 } else { 0.0 },
            y: TAB_HEIGHT,
            width: if self.outline.open { 240.0 } else { 0.0 },
            height: panel_height,
        };
        self.map_bounds = Rect {
            x: width - self.width_right(),
            y: TAB_HEIGHT,
            width: if self.map.open { 64.0 } else { 0.0 },
            height: panel_height,
        };
        if self.documents.open {
            let titles = workspace.titles();
            self.documents.update(
                workspace
                    .editors
                    .iter()
                    .enumerate()
                    .map(|(index, e)| DocumentItem {
                        index,
                        title: titles.get(index).cloned().unwrap_or_default(),
                        path: workspace
                            .path(index)
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        dirty: e.dirty(),
                    })
                    .collect(),
            );
            self.documents.draw(self.left, ops);
        } else if let Some(explorer) = &mut self.explorer {
            let mut local = Vec::new();
            explorer.draw(renderer, width, panel_height, &mut local)?;
            translate_y(&mut local, TAB_HEIGHT);
            ops.extend(local);
        }
        if let Some(editor) = workspace.editors.get(active) {
            let title = workspace.titles().get(active).cloned().unwrap_or_default();
            self.outline.refresh(
                editor.snapshot(),
                workspace.path(active),
                &title,
                self.notify.clone(),
            );
            if self.focus != Focus::Outline {
                self.outline
                    .follow_caret(bareline_document::TextOffset(editor.selection.caret));
            }
            self.map.refresh(
                editor.snapshot(),
                editor.visible_text.clone(),
                self.notify.clone(),
            );
        }
        if workspace.editors.get(active).is_none() {
            self.outline.clear();
            self.map.clear();
        }
        self.outline.draw(self.right, ops);
        self.map.draw(self.map_bounds, ops);
        Ok(if self.width_left() > 0.0 {
            Some(self.left)
        } else {
            None
        })
    }
}
fn translate_y(ops: &mut [DrawOp], dy: f32) {
    for op in ops {
        match op {
            DrawOp::Fill(r, _)
            | DrawOp::Stroke(r, _, _)
            | DrawOp::FillRounded(r, _, _)
            | DrawOp::StrokeRounded(r, _, _, _)
            | DrawOp::PushClip(r)
            | DrawOp::Image { destination: r, .. }
            | DrawOp::PushLayer { bounds: r, .. } => r.y += dy,
            DrawOp::Text { origin, .. } | DrawOp::Layout { origin, .. } => origin.y += dy,
            DrawOp::Line { from, to, .. } => {
                from.y += dy;
                to.y += dy
            }
            DrawOp::PopClip | DrawOp::PopLayer => {}
        }
    }
}
impl WorkspacePanelsRuntime {
    pub(super) fn annotate_context(&self, context: &mut bareline_commands::CommandContext) {
        use bareline_commands::{CommandId, CommandState};
        for (id, checked) in [
            (
                "view.workspace",
                self.explorer.as_ref().is_some_and(|p| p.open),
            ),
            ("view.documents", self.documents.open),
            ("view.outline", self.outline.open),
            ("view.documentMap", self.map.open),
        ] {
            context.states.insert(
                CommandId(id),
                CommandState {
                    checked,
                    ..Default::default()
                },
            );
        }
        if self.operation.is_some() {
            for id in [
                "workspace.createFile",
                "workspace.createFolder",
                "workspace.rename",
                "workspace.delete",
            ] {
                context.states.insert(
                    CommandId(id),
                    CommandState::disabled("Wait for the pending workspace operation"),
                );
            }
        } else if self
            .explorer
            .as_ref()
            .and_then(|p| p.selected_path())
            .is_none()
        {
            for id in ["workspace.rename", "workspace.delete"] {
                context.states.insert(
                    CommandId(id),
                    CommandState::disabled("Select a workspace entry first"),
                );
            }
        }
    }
}
impl Shell {
    pub(super) fn panels_context_commands(&mut self) -> Option<Vec<bareline_commands::CommandId>> {
        if !self.panels.left.contains(self.pointer) {
            return None;
        }
        let commands = if self.panels.documents.open {
            self.panels.documents.pointer(self.pointer);
            self.panels.focus = Focus::Documents;
            vec!["documents.save", "documents.close"]
        } else {
            self.panels.focus = Focus::Explorer;
            let point = Point {
                x: self.pointer.x,
                y: self.pointer.y - TAB_HEIGHT,
            };
            self.panels.explorer().select_context(point);
            vec![
                "workspace.createFile",
                "workspace.createFolder",
                "workspace.rename",
                "workspace.delete",
            ]
        };
        Some(
            commands
                .into_iter()
                .map(bareline_commands::CommandId)
                .collect(),
        )
    }

    pub(super) fn panels_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        self.panels.notify = self.notify.clone();
        match id {
            "view.workspace" => {
                let p = self.panels.explorer();
                p.open = !p.open;
                let open = p.open;
                self.panels.documents.open = false;
                self.panels.focus = if open { Focus::Explorer } else { Focus::Editor };
            }
            "view.documents" => {
                self.panels.documents.open = !self.panels.documents.open;
                if let Some(p) = &mut self.panels.explorer {
                    p.hide();
                }
                self.panels.focus = if self.panels.documents.open {
                    Focus::Documents
                } else {
                    Focus::Editor
                };
            }
            "view.outline" => {
                self.panels.outline.open = !self.panels.outline.open;
                self.panels.focus = if self.panels.outline.open {
                    Focus::Outline
                } else {
                    Focus::Editor
                };
            }
            "view.documentMap" => self.panels.map.open = !self.panels.map.open,
            "workspace.loadMore" => self.panels.explorer().load_more(),
            "workspace.refresh" => self.panels.explorer().refresh_tree(),
            "outline.importFunctionList" | "outline.loadDefinition" => {
                if self.panels.outline_import.is_some() { return true; }
                let path = self.platform.as_ref().and_then(|p| p.open_file().ok().flatten());
                let Some(path) = path else { return true; };
                let extension = self.workspace.as_ref().and_then(|w| w.path(self.app.active))
                    .and_then(|p| p.extension()).and_then(|e| e.to_str()).unwrap_or("").to_owned();
                let xml = id == "outline.importFunctionList";
                let (tx, rx) = mpsc::sync_channel(1);
                let notify = self.notify.clone();
                match std::thread::Builder::new().name("outline-definition-import".into()).spawn(move || {
                    use std::io::Read;
                    let result = (|| -> Result<_, String> {
                        let file = bareline_platform_windows::WindowsFileSystem.open_sealed_read(&path).map_err(|e| e.to_string())?;
                        let mut bytes = Vec::new();
                        file.take(256 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                        if bytes.len() > 256 * 1024 { return Err("Outline definition exceeds 256 KiB".into()); }
                        let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
                        let (definition, report) = if xml {
                            let (definition, report) = bareline_syntax::outline::import_function_list(text)?;
                            let report = report.iter().map(|m| format!("{:?}: {} — {}", m.kind, m.field, m.reason)).collect::<Vec<_>>().join("\n");
                            (definition, report)
                        } else {
                            (bareline_syntax::outline::Definition::from_toml(text)?, "Outline definition loaded".into())
                        };
                        Ok((definition, extension, report))
                    })();
                    let _ = tx.send(result); notify();
                }) {
                    Ok(_) => { self.panels.outline_import = Some(rx); self.panels.outline.status = "Importing outline definition…".into(); }
                    Err(error) => self.panels.outline.status = error.to_string(),
                }
            }
            "outline.exportDefinition" => {
                if self.panels.operation.is_some() { return true; }
                let Some(definition) = self.panels.outline.definition() else { return true; };
                let definition = definition.clone();
                let Some(path) = self.platform.as_ref().and_then(|p| p.save_file().ok().flatten()) else { return true; };
                let (tx, rx) = mpsc::sync_channel(1);
                let notify = self.notify.clone();
                match std::thread::Builder::new().name("outline-definition-export".into()).spawn(move || {
                    use std::io::Write;
                    let result = (|| -> Result<_, String> {
                        let text = definition.to_toml()?;
                        let fs = bareline_platform_windows::WindowsFileSystem;
                        let parent = path.parent().ok_or("Missing destination parent")?;
                        let _guard = fs.guard_directory(parent).map_err(|e| e.to_string())?;
                        fs.validate_target(&path).map_err(|e| e.to_string())?;
                        let stage = parent.join(format!(".bareline-outline-{}-{}.tmp", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()));
                        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&stage).map_err(|e| e.to_string())?;
                        let result = file.write_all(text.as_bytes()).and_then(|()| file.sync_all());
                        drop(file);
                        let result = result.and_then(|()| fs.commit(&stage, &path, path.exists()));
                        if result.is_err() { let _ = std::fs::remove_file(&stage); }
                        result.map_err(|e| e.to_string())?;
                        Ok(None)
                    })();
                    let _ = tx.send(result); notify();
                }) {
                    Ok(_) => { self.panels.operation = Some(rx); self.panels.restoring = false; }
                    Err(error) => self.panels.outline.status = error.to_string(),
                }
            }
            "documents.sortName" => self.panels.documents.set_sort(Sort::Name),
            "documents.sortPath" => self.panels.documents.set_sort(Sort::Path),
            "documents.sortTabOrder" => self.panels.documents.set_sort(Sort::TabOrder),
            "documents.save" | "documents.close" => {
                let a = if id == "documents.save" {
                    self.panels.documents.save_selected()
                } else {
                    self.panels.documents.close_selected()
                };
                if let Some(a) = a {
                    self.panel_document_action(el, a);
                }
            }
            "workspace.openFolder" => {
                if self.panels.root.is_some() {
                    return true;
                }
                if let Some(platform) = &self.platform
                    && let Ok(Some(path)) = platform.pick_folder()
                {
                    let (tx, rx) = mpsc::sync_channel(1);
                    let notify = self.notify.clone();
                    if std::thread::Builder::new()
                        .name("workspace-root-trust".into())
                        .spawn(move || {
                            let provider = bareline_platform_windows::WindowsPathTrustProvider;
                            let result = provider
                                .canonicalize(&path, PathOrigin::User)
                                .map_err(|e| e.to_string())
                                .and_then(|trust| {
                                    if provider.permits(&trust, PathOperation::Read) {
                                        Ok(trust.canonical)
                                    } else {
                                        Err("Workspace path is not authorized".into())
                                    }
                                });
                            let _ = tx.send(result);
                            notify();
                        })
                        .is_ok()
                    {
                        self.panels.root = Some(rx);
                    }
                }
            }
            "workspace.createFile"
            | "workspace.createFolder"
            | "workspace.rename"
            | "workspace.delete"
            | "workspace.undoDelete" => {
                if self.panels.operation.is_some() {
                    return true;
                }
                let selected = self
                    .panels
                    .explorer
                    .as_ref()
                    .and_then(|p| p.selected_path())
                    .map(PathBuf::from);
                let undo = if id == "workspace.undoDelete" { self.panels.deleted.last().cloned() } else { None };
                if id == "workspace.undoDelete" && undo.is_none() { return true; }
                if id == "workspace.delete" && self.panels.deleted.len() >= 32 {
                    self.panels.explorer().message = Some("Restore a retained deletion before deleting more entries".into());
                    return true;
                }
                let destination = if id == "workspace.delete" || id == "workspace.undoDelete" {
                    None
                } else {
                    self.platform
                        .as_ref()
                        .and_then(|p| p.save_file().ok().flatten())
                };
                if id != "workspace.delete" && id != "workspace.undoDelete" && destination.is_none() {
                    return true;
                }
                if (id == "workspace.rename" || id == "workspace.delete") && selected.is_none() {
                    return true;
                }
                // Existing open tabs retain their immutable document; prevent path metadata
                // divergence until close/reopen can reflect the user's file operation.
                if matches!(id, "workspace.rename" | "workspace.delete")
                    && let Some(path) = &selected
                    && self.workspace.as_ref().is_some_and(|w| {
                        w.editors
                            .iter()
                            .enumerate()
                            .any(|(i, _)| w.path(i).is_some_and(|p| p.starts_with(path)))
                    })
                {
                    if let Some(w) = &mut self.workspace {
                        w.message =
                            Some("Close the document before renaming or deleting its file".into());
                    }
                    return true;
                }
                let kind = id.to_owned();
                let (tx, rx) = mpsc::sync_channel(1);
                let notify = self.notify.clone();
                if std::thread::Builder::new()
                    .name("workspace-file-action".into())
                    .spawn(move || {
                        let fs = bareline_platform_windows::WindowsFileSystem;
                        let result = if kind == "workspace.delete" {
                            bareline_platform_windows::retain_deleted_entry(&fs, selected.as_ref().unwrap()).map(Some)
                        } else if let Some(undo) = undo {
                            bareline_platform_windows::restore_deleted_entry(&fs, &undo).map(|()| None)
                        } else { match kind.as_str() {
                            "workspace.createFile" => {
                                fs.create_entry(destination.as_ref().unwrap(), false)
                            }
                            "workspace.createFolder" => {
                                fs.create_entry(destination.as_ref().unwrap(), true)
                            }
                            "workspace.rename" => fs.rename_entry(
                                selected.as_ref().unwrap(),
                                destination.as_ref().unwrap(),
                            ),
                            _ => fs.delete_entry(selected.as_ref().unwrap()),
                        }.map(|()| None) }
                        .map_err(|e| e.to_string());
                        let _ = tx.send(result);
                        notify();
                    })
                    .is_ok()
                {
                    self.panels.operation = Some(rx);
                    self.panels.restoring = id == "workspace.undoDelete";
                }
            }
            _ => return false,
        }
        self.ensure_workspace(el);
        if let Some(w) = &self.window {
            w.request_redraw();
        }
        true
    }
    fn panel_document_action(&mut self, el: &ActiveEventLoop, action: DocumentAction) {
        let index = match action {
            DocumentAction::Activate(i) | DocumentAction::Save(i) | DocumentAction::Close(i) => i,
        };
        self.app.active = index;
        match action {
            DocumentAction::Save(_) => self.dispatch(el, Action::Save),
            DocumentAction::Close(_) => self.dispatch(el, Action::Close),
            _ => {}
        }
    }
    pub(super) fn panels_pump(&mut self, el: &ActiveEventLoop) {
        self.panels.notify = self.notify.clone();
        let excludes = self.settings.effective().search_excludes;
        if self.panels.excludes != excludes {
            self.panels.excludes = excludes.clone();
            self.panels.explorer().set_excludes(excludes);
        }
        let mut changed = self.panels.outline.pump() | self.panels.map.pump();
        if let Some(result) = self.panels.outline_import.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.panels.outline_import = None;
            changed = true;
            match result {
                Ok((definition, extension, report)) => {
                    self.panels.outline.set_definition(definition, extension);
                    self.panels.outline.open = true;
                    if let Some(workspace) = &mut self.workspace {
                        if workspace.new_document().is_ok() {
                            let index = workspace.editors.len() - 1;
                            workspace.editors[index].enqueue(Input::Insert(report));
                            self.app.tabs = workspace.titles();
                        }
                    }
                }
                Err(error) => self.panels.outline.status = error,
            }
        }
        if let Some(p) = &mut self.panels.explorer {
            changed |= p.pump();
        }
        let root = self.panels.root.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = root {
            self.panels.root = None;
            changed = true;
            match result {
                Ok(path) => {
                    self.ensure_workspace(el);
                    self.settings.set_workspace_root(path.clone());
                    self.panels.explorer().add_root(path);
                    self.panels.documents.open = false;
                }
                Err(error) => {
                    if let Some(w) = &mut self.workspace {
                        w.message = Some(error);
                    }
                }
            }
        }
        let operation = self
            .panels
            .operation
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if let Some(result) = operation {
            self.panels.operation = None;
            changed = true;
            if let Some(p) = &mut self.panels.explorer {
                p.message = Some(match result {
                    Ok(undo) => {
                        if self.panels.restoring { self.panels.deleted.pop(); }
                        p.refresh_tree();
                        if let Some(undo) = undo {
                            let message = format!("Deleted · Undo Delete available · retained at {}", undo.retained.display());
                            self.panels.deleted.push(undo);
                            message
                        } else { "File operation completed".into() }
                    }
                    Err(error) => format!("File operation failed: {error}"),
                });
            }
        }
        if changed && let Some(w) = &self.window {
            w.request_redraw();
        }
    }
    pub(super) fn workspace_watch_roots(&self) -> Vec<PathBuf> {
        self.panels.explorer.as_ref().map(|p| p.watch_roots()).unwrap_or_default()
    }
    pub(super) fn workspace_watch_event(&mut self, event: &bareline_platform::WatchEvent) {
        if let Some(panel) = &mut self.panels.explorer {
            panel.directory_changed(&event.directory);
        }
    }
    pub(super) fn panels_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if self.palette.open {
            return false;
        }
        let mut navigation = None;
        let mut document = None;
        let mut explorer = None;
        let mut handled = false;
        match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let point = self.pointer;
                if self.panels.left.contains(point) {
                    handled = true;
                    if self.panels.documents.open {
                        self.panels.focus = Focus::Documents;
                        document = self.panels.documents.pointer(point);
                    } else {
                        self.panels.focus = Focus::Explorer;
                        explorer = self.panels.explorer().pointer(Point {
                            x: point.x,
                            y: point.y - TAB_HEIGHT,
                        });
                    }
                } else if self.panels.right.contains(point) {
                    handled = true;
                    self.panels.focus = Focus::Outline;
                    if let Some(e) = self
                        .workspace
                        .as_ref()
                        .and_then(|w| w.editors.get(self.app.active))
                    {
                        navigation = self.panels.outline.pointer(point, e.snapshot());
                    }
                } else if self.panels.map_bounds.contains(point) {
                    handled = true;
                    if let Some(e) = self
                        .workspace
                        .as_ref()
                        .and_then(|w| w.editors.get(self.app.active))
                    {
                        navigation = self.panels.map.pointer(point, e.snapshot());
                    }
                } else {
                    self.panels.focus = Focus::Editor;
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && self.panels.focus != Focus::Editor =>
            {
                if !self.modifiers.control_key()
                    && !self.modifiers.alt_key()
                    && matches!(self.panels.focus, Focus::Documents | Focus::Outline)
                {
                    let filter = if self.panels.focus == Focus::Documents {
                        &mut self.panels.document_filter
                    } else {
                        &mut self.panels.outline_filter
                    };
                    let edited = match &event.logical_key {
                        Key::Character(text) if filter.len() + text.len() <= 256 => {
                            filter.push_str(text);
                            true
                        }
                        Key::Named(NamedKey::Backspace) => {
                            filter.pop();
                            true
                        }
                        _ => false,
                    };
                    if edited {
                        if self.panels.focus == Focus::Documents {
                            self.panels
                                .documents
                                .set_filter(&self.panels.document_filter);
                        } else {
                            self.panels.outline.set_filter(&self.panels.outline_filter);
                        }
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return true;
                    }
                }
                let key = match event.logical_key {
                    Key::Named(NamedKey::ArrowUp) => Some(UiKey::Up),
                    Key::Named(NamedKey::ArrowDown) => Some(UiKey::Down),
                    Key::Named(NamedKey::ArrowLeft) => Some(UiKey::Left),
                    Key::Named(NamedKey::ArrowRight) => Some(UiKey::Right),
                    Key::Named(NamedKey::Home) => Some(UiKey::Home),
                    Key::Named(NamedKey::End) => Some(UiKey::End),
                    Key::Named(NamedKey::Enter) => Some(UiKey::Enter),
                    Key::Named(NamedKey::Escape) => {
                        self.panels.focus = Focus::Editor;
                        Some(UiKey::Escape)
                    }
                    _ => None,
                };
                if let Some(key) = key {
                    handled = true;
                    match self.panels.focus {
                        Focus::Explorer => explorer = self.panels.explorer().key(key),
                        Focus::Documents => document = self.panels.documents.key(key),
                        Focus::Outline => {
                            if let Some(e) = self
                                .workspace
                                .as_ref()
                                .and_then(|w| w.editors.get(self.app.active))
                            {
                                navigation = self.panels.outline.key(key, e.snapshot());
                            }
                        }
                        Focus::Editor => {}
                    }
                }
            }
            _ => {}
        }
        if let Some(PanelAction::Open(path)) = explorer
            && self.ensure_workspace(el)
        {
            self.workspace.as_mut().unwrap().open(path);
        }
        if let Some(action) = document {
            self.panel_document_action(el, action);
        }
        if let Some(offset) = navigation
            && let Some(editor) = self
                .workspace
                .as_mut()
                .and_then(|w| w.editors.get_mut(self.app.active))
        {
            let mut selection = editor.selection;
            selection.anchor = offset.0;
            selection.caret = offset.0;
            let _ = editor.set_selections(selection.into());
        }
        if handled && let Some(w) = &self.window {
            w.request_redraw();
        }
        handled
    }
}
