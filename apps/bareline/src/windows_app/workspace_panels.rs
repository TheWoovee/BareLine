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
    operation: Option<Receiver<Result<(), String>>>,
    document_filter: String,
    outline_filter: String,
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
            document_filter: String::new(),
            outline_filter: String::new(),
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
            | DrawOp::PushClip(r) => r.y += dy,
            DrawOp::Text { origin, .. } | DrawOp::Layout { origin, .. } => origin.y += dy,
            DrawOp::Line { from, to, .. } => {
                from.y += dy;
                to.y += dy
            }
            DrawOp::PopClip => {}
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
            | "workspace.delete" => {
                if self.panels.operation.is_some() {
                    return true;
                }
                let selected = self
                    .panels
                    .explorer
                    .as_ref()
                    .and_then(|p| p.selected_path())
                    .map(PathBuf::from);
                let destination = if id == "workspace.delete" {
                    None
                } else {
                    self.platform
                        .as_ref()
                        .and_then(|p| p.save_file().ok().flatten())
                };
                if id != "workspace.delete" && destination.is_none() {
                    return true;
                }
                if (id == "workspace.rename" || id == "workspace.delete") && selected.is_none() {
                    return true;
                }
                // Existing open tabs retain their immutable document; prevent path metadata
                // divergence until close/reopen can reflect the user's file operation.
                if let Some(path) = &selected
                    && self.workspace.as_ref().is_some_and(|w| {
                        w.editors
                            .iter()
                            .enumerate()
                            .any(|(i, _)| w.path(i).is_some_and(|p| p == path))
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
                        let result = match kind.as_str() {
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
                        }
                        .map_err(|e| e.to_string());
                        let _ = tx.send(result);
                        notify();
                    })
                    .is_ok()
                {
                    self.panels.operation = Some(rx);
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
        let mut changed = self.panels.outline.pump() | self.panels.map.pump();
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
                    Ok(()) => {
                        p.refresh_tree();
                        "File operation completed".into()
                    }
                    Err(error) => format!("File operation failed: {error}"),
                });
            }
        }
        if changed && let Some(w) = &self.window {
            w.request_redraw();
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
