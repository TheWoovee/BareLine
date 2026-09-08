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
const ACCESS_GROUP: u64 = 90_000_009;
const ACCESS_EXPLORER: u64 = 0x9009_0000_0000_0000;
const ACCESS_DOCUMENTS: u64 = 0x9009_1000_0000_0000;
const ACCESS_OUTLINE: u64 = 0x9009_2000_0000_0000;
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
    import_cancel: bareline_syntax::outline::OutlineJob,
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
            import_cancel: Default::default(),
            document_filter: String::new(),
            outline_filter: String::new(),
            excludes: Vec::new(),
        }
    }
}
impl Drop for WorkspacePanelsRuntime {
    fn drop(&mut self) { self.import_cancel.cancel(); }
}
fn receive_job<T>(receiver: &Option<Receiver<Result<T, String>>>) -> Option<Result<T, String>> {
    match receiver.as_ref()?.try_recv() {
        Ok(result) => Some(result),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(mpsc::TryRecvError::Disconnected) => Some(Err("Workspace worker stopped before completing".into())),
    }
}
impl WorkspacePanelsRuntime {
    fn semantics(&self) -> Vec<bareline_ui::semantics::SemanticEntry> {
        use bareline_ui::{ViewId, controls::ControlState, semantics::SemanticEntry, widgets::{Semantics, SemanticRole, SemanticAction}};
        let mut entries = Vec::new();
        for (open, id, label, role, bounds, filter) in [
            (self.explorer.as_ref().is_some_and(|p| p.open), ACCESS_EXPLORER, "Workspace", SemanticRole::Tree, self.left, None),
            (self.documents.open, ACCESS_DOCUMENTS, "Documents", SemanticRole::List, self.left, Some(self.document_filter.as_str())),
            (self.outline.open, ACCESS_OUTLINE, "Outline", SemanticRole::Tree, self.right, Some(self.outline_filter.as_str())),
        ] {
            if !open { continue; }
            entries.push(SemanticEntry { parent: ViewId(ACCESS_GROUP), node: Semantics::new(ViewId(id), role, label, "", bounds, ControlState::default()) });
            if let Some(value) = filter {
                let mut node = Semantics::new(ViewId(id + 1), SemanticRole::TextField, &format!("Filter {label}"), if id == ACCESS_DOCUMENTS { "documents.filter" } else { "outline.filter" },
                    Rect { height: 28.0, ..bounds }, ControlState::default()).action(SemanticAction::Focus).action(SemanticAction::SetValue);
                node.value = Some(value.into());
                entries.push(SemanticEntry { parent: ViewId(id), node });
            }
        }
        if let Some(panel) = &self.explorer {
            let mut nodes = panel.semantics(ViewId(ACCESS_EXPLORER), ACCESS_EXPLORER + 65536);
            for entry in &mut nodes {
                entry.node.bounds.y += TAB_HEIGHT;
                entry.node.focused &= self.focus == Focus::Explorer;
            }
            entries.extend(nodes);
        }
        entries.extend(self.documents.semantics(ViewId(ACCESS_DOCUMENTS), ACCESS_DOCUMENTS, self.focus == Focus::Documents));
        entries.extend(self.outline.semantics(ViewId(ACCESS_OUTLINE), ACCESS_OUTLINE, self.focus == Focus::Outline));
        entries
    }
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
impl WorkspacePanelsRuntime {
    fn accessibility_nodes(&self) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        let entries = self.semantics();
        if entries.is_empty() { return Vec::new(); }
        let mut nodes = vec![AccessibilityNode { id: ACCESS_GROUP, parent: 1, role: AccessibilityRole::Group, name: "Navigation panels".into(), value: None,
            bounds: [0.0, TAB_HEIGHT as f64, 0.0, 0.0], disabled: false, selected: false, expanded: None, focusable: false, invokable: false }];
        nodes.extend(entries.iter().map(|entry| bareline_app::accessibility::semantic_node(&entry.node, entry.parent.0)));
        nodes
    }
    fn accessibility_focus(&self) -> Option<u64> {
        self.semantics().into_iter().find(|entry| entry.node.focused).map(|entry| entry.node.id.0)
    }
}
impl Shell {
    pub(super) fn panels_accessibility_nodes(&self) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        self.panels.accessibility_nodes()
    }
    pub(super) fn panels_accessibility_focus(&self) -> Option<u64> {
        self.panels.accessibility_focus()
    }
    pub(super) fn panels_accessibility(&mut self, el: &ActiveEventLoop, action: &bareline_platform::accessibility::AccessibilityAction) -> bool {
        use bareline_platform::accessibility::AccessibilityAction;
        use bareline_ui::widgets::SemanticAction;
        let (id, invoke) = match action {
            AccessibilityAction::Focus(id) => (*id, false),
            AccessibilityAction::Invoke(id) => (*id, true),
            AccessibilityAction::SetValue { id, value } => {
                if value.len() > 256 { return false; }
                if *id == ACCESS_DOCUMENTS + 1 && self.panels.documents.open {
                    self.panels.document_filter = value.clone();
                    self.panels.documents.set_filter(value);
                    self.panels.focus = Focus::Documents;
                } else if *id == ACCESS_OUTLINE + 1 && self.panels.outline.open {
                    self.panels.outline_filter = value.clone();
                    self.panels.outline.set_filter(value);
                    self.panels.focus = Focus::Outline;
                } else { return false; }
                if let Some(window) = &self.window { window.request_redraw(); }
                return true;
            }
            _ => return false,
        };
        if !self.panels.semantics().iter().any(|entry| entry.node.id.0 == id) { return false; }
        if (ACCESS_EXPLORER..ACCESS_DOCUMENTS).contains(&id) {
            self.panels.focus = Focus::Explorer;
            if let Some(local) = id.checked_sub(ACCESS_EXPLORER + 65536) {
                let action = self.panels.explorer().accessibility_action(bareline_ui::virtual_tree::NodeId(local), if invoke { SemanticAction::Invoke } else { SemanticAction::Focus });
                if let Some(PanelAction::Open(path)) = action && self.ensure_workspace(el) {
                    self.workspace.as_mut().unwrap().open(path);
                }
            }
        } else if (ACCESS_DOCUMENTS..ACCESS_OUTLINE).contains(&id) {
            self.panels.focus = Focus::Documents;
            if let Some(action) = self.panels.documents.accessibility_action(id, ACCESS_DOCUMENTS, invoke) {
                self.panel_document_action(el, action);
            }
        } else {
            self.panels.focus = Focus::Outline;
            if let Some(editor) = self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active))
                && let Some(offset) = self.panels.outline.accessibility_action(id, ACCESS_OUTLINE, invoke, editor.snapshot()) {
                let mut selection = editor.selection;
                selection.anchor = offset.0; selection.caret = offset.0;
                let _ = editor.set_selections(selection.into());
            }
        }
        if let Some(window) = &self.window { window.request_redraw(); }
        true
    }
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
            "outline.cancelImport" => {
                self.panels.import_cancel.cancel();
                self.panels.outline.status = "Cancelling outline import…".into();
            }
            "outline.importFunctionList" | "outline.loadDefinition" => {
                if self.panels.outline_import.is_some() { return true; }
                let path = self.platform.as_ref().and_then(|p| p.open_file().ok().flatten());
                let Some(path) = path else { return true; };
                let extension = self.workspace.as_ref().and_then(|w| w.path(self.app.active))
                    .and_then(|p| p.extension()).and_then(|e| e.to_str()).unwrap_or("").to_owned();
                let xml = id == "outline.importFunctionList";
                self.panels.import_cancel = Default::default();
                let cancel = self.panels.import_cancel.clone();
                let (tx, rx) = mpsc::sync_channel(1);
                let notify = self.notify.clone();
                match std::thread::Builder::new().name("outline-definition-import".into()).spawn(move || {
                    use std::io::Read;
                    let result = (|| -> Result<_, String> {
                        let _guard = bareline_platform_windows::WindowsPathTrustProvider.open_read(&path, PathOrigin::User).map_err(|e| e.to_string())?;
                        let file = bareline_platform_windows::WindowsFileSystem.open_sealed_read(&path).map_err(|e| e.to_string())?;
                        let mut bytes = Vec::new();
                        file.take(256 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                        if bytes.len() > 256 * 1024 { return Err("Outline definition exceeds 256 KiB".into()); }
                        let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
                        let (definition, report) = if xml {
                            let (definition, report) = bareline_syntax::outline::import_function_list_with_job(text, &cancel)?;
                            let report = report.iter().map(|m| format!("{:?}: {} — {}", m.kind, m.field, m.reason)).collect::<Vec<_>>().join("\n");
                            (definition, report)
                        } else {
                            (bareline_syntax::outline::Definition::from_toml(text)?, "Outline definition loaded".into())
                        };
                        if cancel.is_cancelled() { return Err("Outline import cancelled".into()); }
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
                    } else {
                        self.panels.explorer().message = Some("Could not start workspace authorization worker".into());
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
                } else {
                    self.panels.explorer().message = Some("Could not start workspace file operation".into());
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
        if let Some(result) = receive_job(&self.panels.outline_import) {
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
        let root = receive_job(&self.panels.root);
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
        let operation = receive_job(&self.panels.operation);
        if let Some(result) = operation {
            self.panels.operation = None;
            changed = true;
            let message = match result {
                    Ok(undo) => {
                        if self.panels.restoring { self.panels.deleted.pop(); }
                        if let Some(panel) = &mut self.panels.explorer { panel.refresh_tree(); }
                        if let Some(undo) = undo {
                            let message = format!("Deleted · Undo Delete available · retained at {}", undo.retained.display());
                            self.panels.deleted.push(undo);
                            message
                        } else { "File operation completed".into() }
                    }
                    Err(error) => format!("File operation failed: {error}"),
                };
            self.panels.explorer().message = Some(message.clone());
            if let Some(workspace) = &mut self.workspace { workspace.message = Some(message); }
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
/// Native golden fixtures exercise retained production layout and model actions;
/// they never create a window, enumerate a folder, or write a platform file.
#[cfg(test)]
pub(super) fn accessibility_test_cases() -> Vec<(&'static str, Vec<bareline_platform::accessibility::AccessibilityNode>, Option<u64>)> {
    use bareline_document::{Budget, Document};
    use bareline_ui::widgets::SemanticAction;
    fn capture(runtime: &WorkspacePanelsRuntime, name: &'static str) -> (&'static str, Vec<bareline_platform::accessibility::AccessibilityNode>, Option<u64>) {
        (name, runtime.accessibility_nodes(), runtime.accessibility_focus())
    }
    let mut runtime = WorkspacePanelsRuntime::default();
    let mut renderer = bareline_renderer_recording::RecordingBackend::default();
    let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
    let source = Document::from_utf8("fn first() {}\nfn second() {}\n", Budget::new(4096), Budget::new(4096)).unwrap().snapshot();
    workspace.add_snapshot_preview(&source, "main.rs".into()).unwrap();
    workspace.add_snapshot_preview(&source, "notes.rs".into()).unwrap();
    let mut ops = Vec::new();
    runtime.draw(&mut renderer, 1000.0, 800.0, &workspace, 0, &mut ops).unwrap();
    let mut cases = vec![capture(&runtime, "panels.closed")];

    let mut explorer = WorkspacePanel::new(Arc::new(|| {}));
    explorer.add_root(PathBuf::from("golden-workspace"));
    runtime.explorer = Some(explorer);
    runtime.draw(&mut renderer, 1000.0, 800.0, &workspace, 0, &mut ops).unwrap();
    cases.push(capture(&runtime, "panels.explorer.open"));
    runtime.focus = Focus::Explorer;
    runtime.explorer.as_mut().unwrap().key(UiKey::Home);
    let root_id = runtime.explorer.as_ref().unwrap().semantics(bareline_ui::ViewId(ACCESS_EXPLORER), ACCESS_EXPLORER + 65536)[0].node.id.0;
    runtime.explorer.as_mut().unwrap().accessibility_action(bareline_ui::virtual_tree::NodeId(root_id - ACCESS_EXPLORER - 65536), SemanticAction::Focus);
    cases.push(capture(&runtime, "panels.explorer.focus"));

    runtime.explorer.as_mut().unwrap().hide();
    runtime.documents.open = true;
    runtime.focus = Focus::Documents;
    runtime.draw(&mut renderer, 1000.0, 800.0, &workspace, 0, &mut ops).unwrap();
    cases.push(capture(&runtime, "panels.documents.populated"));
    runtime.document_filter = "notes".into();
    runtime.documents.set_filter("notes");
    runtime.documents.draw(runtime.left, &mut ops);
    let selected = runtime.documents.semantics(bareline_ui::ViewId(ACCESS_DOCUMENTS), ACCESS_DOCUMENTS, true)[0].node.id.0;
    assert!(matches!(runtime.documents.accessibility_action(selected, ACCESS_DOCUMENTS, true), Some(DocumentAction::Activate(1))));
    cases.push(capture(&runtime, "panels.documents.filtered-invoked"));

    runtime.documents.open = false;
    runtime.outline.open = true;
    runtime.focus = Focus::Outline;
    runtime.draw(&mut renderer, 1000.0, 800.0, &workspace, 0, &mut ops).unwrap();
    let (wake, ready) = mpsc::channel();
    runtime.outline.refresh(&source, Some(std::path::Path::new("main.rs")), "main.rs", Arc::new(move || { let _ = wake.send(()); }));
    ready.recv_timeout(std::time::Duration::from_secs(5)).expect("bounded outline fixture completion");
    assert!(runtime.outline.pump());
    runtime.outline.draw(runtime.right, &mut ops);
    cases.push(capture(&runtime, "panels.outline.populated"));
    runtime.outline_filter = "second".into();
    runtime.outline.set_filter("second");
    runtime.outline.draw(runtime.right, &mut ops);
    let selected = runtime.outline.semantics(bareline_ui::ViewId(ACCESS_OUTLINE), ACCESS_OUTLINE, true)[0].node.id.0;
    assert_eq!(runtime.outline.accessibility_action(selected, ACCESS_OUTLINE, false, &source), None);
    assert_eq!(runtime.outline.accessibility_action(selected, ACCESS_OUTLINE, true, &source), Some(bareline_document::TextOffset(17)));
    cases.push(capture(&runtime, "panels.outline.filtered-invoked"));
    // Exercise the combined owner tree, including the normally alternative left
    // panels. Each controller retains its real populated model and draw bounds.
    runtime.explorer.as_mut().unwrap().show();
    runtime.documents.open = true;
    runtime.left.width = runtime.width_left();
    runtime.document_filter.clear();
    runtime.documents.set_filter("");
    runtime.documents.draw(runtime.left, &mut ops);
    runtime.outline_filter.clear();
    runtime.outline.set_filter("");
    runtime.outline.draw(runtime.right, &mut ops);
    cases.push(capture(&runtime, "panels.all_open"));
    cases
}

#[cfg(test)]
mod workspace_panel_regressions {
    use super::*;
    #[test]
    fn disconnected_worker_is_a_terminal_error() {
        let (sender, receiver) = mpsc::sync_channel::<Result<(), String>>(1);
        let pending = Some(receiver);
        assert!(receive_job(&pending).is_none());
        drop(sender);
        assert!(receive_job(&pending).unwrap().is_err());
    }
    #[test]
    fn retained_folder_delete_restores_contents_and_refuses_collision() {
        let root = std::env::temp_dir().join(format!("bareline-explorer-retain-{}", std::process::id()));
        let original = root.join("folder");
        std::fs::create_dir_all(&original).unwrap();
        std::fs::write(original.join("child"), b"retained bytes").unwrap();
        let fs = bareline_platform_windows::WindowsFileSystem;
        let undo = bareline_platform_windows::retain_deleted_entry(&fs, &original).unwrap();
        assert!(!original.exists());
        assert_eq!(std::fs::read(undo.retained.join("child")).unwrap(), b"retained bytes");
        std::fs::create_dir(&original).unwrap();
        assert!(bareline_platform_windows::restore_deleted_entry(&fs, &undo).is_err());
        assert!(undo.retained.join("child").exists());
        std::fs::remove_dir(&original).unwrap();
        bareline_platform_windows::restore_deleted_entry(&fs, &undo).unwrap();
        assert_eq!(std::fs::read(original.join("child")).unwrap(), b"retained bytes");
        std::fs::remove_dir_all(root).unwrap();
    }
}
