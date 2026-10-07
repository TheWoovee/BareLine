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
use bareline_ui::widgets::{DockWidths, SectionLayout, stack_sections};
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
/// A stacked section in the left dock (UX-50). Any combination can be open at
/// once; Outline joins Workspace and Open Documents here (UX-51).
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeftSection {
    Workspace,
    Documents,
    Outline,
}
/// Right-click menus of the Open Documents list and the Workspace explorer.
pub(super) const DOCUMENTS_CONTEXT_COMMANDS: [&str; 2] = ["documents.save", "documents.close"];
pub(super) const EXPLORER_CONTEXT_COMMANDS: [&str; 4] = [
    "workspace.createFile",
    "workspace.createFolder",
    "workspace.rename",
    "workspace.delete",
];
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
    map_bounds: Rect,
    root: Option<Receiver<Result<PathBuf, String>>>,
    /// A file operation's completion message.
    operation: Option<Receiver<Result<&'static str, String>>>,
    outline_import: Option<Receiver<Result<(bareline_syntax::outline::Definition, String, String), String>>>,
    import_cancel: bareline_syntax::outline::OutlineJob,
    document_filter: String,
    outline_filter: String,
    excludes: Vec<String>,
    widths: DockWidths,
    widths_loaded: bool,
    splitter_left: Rect,
    dragging_left: bool,
    left_sections: Vec<(LeftSection, SectionLayout)>,
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
            map_bounds: Rect::default(),
            root: None,
            operation: None,
            outline_import: None,
            import_cancel: Default::default(),
            document_filter: String::new(),
            outline_filter: String::new(),
            excludes: Vec::new(),
            widths: DockWidths::default(),
            widths_loaded: false,
            splitter_left: Rect::default(),
            dragging_left: false,
            left_sections: Vec::new(),
        }
    }
}
impl Drop for WorkspacePanelsRuntime {
    fn drop(&mut self) {
        self.import_cancel.cancel();
    }
}
fn receive_job<T>(receiver: &Option<Receiver<Result<T, String>>>) -> Option<Result<T, String>> {
    match receiver.as_ref()?.try_recv() {
        Ok(result) => Some(result),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(mpsc::TryRecvError::Disconnected) => Some(Err("Workspace worker stopped before completing".into())),
    }
}
impl WorkspacePanelsRuntime {
    /// Whether window point `point` is on the left dock's splitter, or the
    /// splitter is being dragged (its pointer shape, LNX-UI-015).
    pub(super) fn left_splitter_at(&self, point: Point) -> bool {
        self.dragging_left || (self.splitter_left.width > 0.0 && self.splitter_left.contains(point))
    }
    fn semantics(&self) -> Vec<bareline_ui::semantics::SemanticEntry> {
        use bareline_ui::{
            ViewId,
            controls::ControlState,
            semantics::SemanticEntry,
            widgets::{SemanticAction, SemanticRole, Semantics},
        };
        let mut entries = Vec::new();
        for (open, id, label, role, bounds, filter) in [
            (
                self.explorer.as_ref().is_some_and(|p| p.open),
                ACCESS_EXPLORER,
                "Workspace",
                SemanticRole::Tree,
                self.left,
                None,
            ),
            (
                self.documents.open,
                ACCESS_DOCUMENTS,
                "Documents",
                SemanticRole::List,
                self.left,
                Some(self.document_filter.as_str()),
            ),
            (
                self.outline.open,
                ACCESS_OUTLINE,
                "Outline",
                SemanticRole::Tree,
                self.left,
                Some(self.outline_filter.as_str()),
            ),
        ] {
            if !open {
                continue;
            }
            entries.push(SemanticEntry {
                parent: ViewId(ACCESS_GROUP),
                node: Semantics::new(ViewId(id), role, label, "", bounds, ControlState::default()),
            });
            if let Some(value) = filter {
                let mut node = Semantics::new(
                    ViewId(id + 1),
                    SemanticRole::TextField,
                    &format!("Filter {label}"),
                    if id == ACCESS_DOCUMENTS {
                        "documents.filter"
                    } else {
                        "outline.filter"
                    },
                    Rect { height: 28.0, ..bounds },
                    ControlState::default(),
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::SetValue);
                node.value = Some(value.into());
                entries.push(SemanticEntry {
                    parent: ViewId(id),
                    node,
                });
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
        entries.extend(self.documents.semantics(
            ViewId(ACCESS_DOCUMENTS),
            ACCESS_DOCUMENTS,
            self.focus == Focus::Documents,
        ));
        entries.extend(
            self.outline
                .semantics(ViewId(ACCESS_OUTLINE), ACCESS_OUTLINE, self.focus == Focus::Outline),
        );
        entries
    }
    fn explorer(&mut self) -> &mut WorkspacePanel {
        self.explorer.get_or_insert_with(|| {
            let mut panel = WorkspacePanel::new(self.notify.clone());
            panel.set_directory_guard(|path| {
                let retained = crate::shell::native::PathTrust.open_read(path, PathOrigin::User)?;
                Ok(Box::new(retained) as Box<dyn Send>)
            });
            panel
        })
    }
    fn workspace_open(&self) -> bool {
        self.explorer.as_ref().is_some_and(|p| p.open)
    }
    /// Load persisted dock widths once, on the first frame after settings are
    /// available. Ignored afterwards so an in-session drag is never overwritten
    /// by the persisted value (widths survive restart — UX-50).
    pub fn apply_persisted_widths(&mut self, serialized: &str) {
        if self.widths_loaded {
            return;
        }
        self.widths_loaded = true;
        if !serialized.is_empty() {
            self.widths = DockWidths::parse(serialized);
        }
    }
    /// The current dock widths in the persistence form for [`DockWidths::parse`].
    pub fn dock_widths_serialized(&self) -> String {
        self.widths.serialize()
    }
    pub fn width_left(&self) -> f32 {
        if self.documents.open || self.workspace_open() || self.outline.open {
            self.widths.left
        } else {
            0.0
        }
    }
    pub fn width_right(&self) -> f32 {
        // The right dock now holds only the Map; Outline moved into the left
        // dock stack (UX-51).
        if self.map.open { 64.0 } else { 0.0 }
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
        let ui = workspace.theme;
        let theme = ui.panel();
        let panel_height = (height - TAB_HEIGHT - STATUS_HEIGHT).max(0.0);
        self.left = Rect {
            x: 0.0,
            y: TAB_HEIGHT,
            width: self.width_left(),
            height: panel_height,
        };
        self.map_bounds = Rect {
            x: width - self.width_right(),
            y: TAB_HEIGHT,
            width: if self.map.open { 64.0 } else { 0.0 },
            height: panel_height,
        };
        // Refresh the outline and map from the active document before any panel
        // is drawn, so the left-dock Outline section paints current data.
        for identity in workspace.take_closed_documents() {
            self.map.forget(identity);
        }
        if let Some(editor) = workspace.editors.get(active) {
            let title = workspace.titles().get(active).cloned().unwrap_or_default();
            self.outline
                .refresh(editor.snapshot(), workspace.path(active), &title, self.notify.clone());
            if self.focus != Focus::Outline {
                self.outline
                    .follow_caret(bareline_document::TextOffset(editor.viewport().selection.caret));
            }
            self.map.refresh(
                editor.snapshot(),
                editor.viewport().visible_text.clone(),
                editor.paged(),
                self.notify.clone(),
            );
        }
        if workspace.editors.get(active).is_none() {
            self.outline.clear();
            self.map.clear();
        }
        // Left dock: Workspace, Open Documents and Outline stack as collapsible
        // sections with a header and × (UX-50/UX-51). Any combination can be open
        // at once, and the tab strip above (y=0..TAB_HEIGHT) is never touched.
        self.left_sections.clear();
        let mut open_sections = Vec::new();
        if self.workspace_open() {
            open_sections.push(LeftSection::Workspace);
        }
        if self.documents.open {
            open_sections.push(LeftSection::Documents);
        }
        if self.outline.open {
            open_sections.push(LeftSection::Outline);
        }
        if !open_sections.is_empty() {
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
            }
            let collapsed = vec![false; open_sections.len()];
            let layouts = stack_sections(self.left, &collapsed);
            for (section, layout) in open_sections.iter().copied().zip(layouts) {
                ops.push(DrawOp::Fill(layout.header, ui.chrome));
                let title = match section {
                    LeftSection::Workspace => "Workspace",
                    LeftSection::Documents => "Open Documents",
                    LeftSection::Outline => "Outline",
                };
                bareline_ui::text(ops, layout.header.x + 10.0, layout.header.y + 6.0, title, 12.0, ui.text);
                bareline_ui::text(ops, layout.close.x + 4.0, layout.close.y + 1.0, "×", 15.0, ui.muted);
                if let Some(body) = layout.body {
                    match section {
                        LeftSection::Documents => {
                            self.documents.set_focused(self.focus == Focus::Documents);
                            self.documents.draw_with_theme(body, theme, ops)
                        }
                        LeftSection::Outline => self.outline.draw_with_theme(body, theme, ops),
                        LeftSection::Workspace => {
                            if let Some(explorer) = &mut self.explorer {
                                let mut local = Vec::new();
                                explorer.set_focused(self.focus == Focus::Explorer);
                                explorer.draw_with_theme(renderer, body.width, body.height, theme, &mut local)?;
                                translate_y(&mut local, body.y);
                                ops.extend(local);
                            }
                        }
                    }
                }
                self.left_sections.push((section, layout));
            }
            // Draggable 6 px splitter on the right edge of the left dock.
            self.splitter_left = Rect {
                x: self.left.width,
                y: TAB_HEIGHT,
                width: DockWidths::SPLITTER,
                height: panel_height,
            };
            ops.push(DrawOp::Fill(self.splitter_left, ui.border));
        } else {
            self.splitter_left = Rect::default();
        }
        self.map.draw_with_theme(self.map_bounds, theme, ops);
        Ok(if self.width_left() > 0.0 { Some(self.left) } else { None })
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
            ("view.workspace", self.explorer.as_ref().is_some_and(|p| p.open)),
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
        } else if self.explorer.as_ref().and_then(|p| p.selected_path()).is_none() {
            for id in ["workspace.rename", "workspace.delete"] {
                context
                    .states
                    .insert(CommandId(id), CommandState::disabled("Select a workspace entry first"));
            }
        }
    }
}
impl WorkspacePanelsRuntime {
    fn accessibility_nodes(&self) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        let entries = self.semantics();
        if entries.is_empty() {
            return Vec::new();
        }
        let mut nodes = vec![AccessibilityNode {
            id: ACCESS_GROUP,
            parent: 1,
            role: AccessibilityRole::Group,
            name: "Navigation panels".into(),
            value: None,
            bounds: [0.0, TAB_HEIGHT as f64, 0.0, 0.0],
            disabled: false,
            selected: false,
            expanded: None,
            focusable: false,
            invokable: false,
            position_in_set: None,
            size_of_set: None,
        }];
        nodes.extend(
            entries
                .iter()
                .map(|entry| bareline_app::accessibility::semantic_node(&entry.node, entry.parent.0)),
        );
        nodes
    }
    fn accessibility_focus(&self) -> Option<u64> {
        self.semantics()
            .into_iter()
            .find(|entry| entry.node.focused)
            .map(|entry| entry.node.id.0)
    }
}
impl Shell {
    pub(super) fn panels_accessibility_nodes(&self) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        self.panels.accessibility_nodes()
    }
    pub(super) fn panels_accessibility_focus(&self) -> Option<u64> {
        self.panels.accessibility_focus()
    }
    pub(super) fn panels_accessibility(
        &mut self,
        el: &ActiveEventLoop,
        action: &bareline_platform::accessibility::AccessibilityAction,
    ) -> bool {
        use bareline_platform::accessibility::AccessibilityAction;
        use bareline_ui::widgets::SemanticAction;
        let (id, invoke) = match action {
            AccessibilityAction::Focus(id) => (*id, false),
            AccessibilityAction::Invoke(id) => (*id, true),
            AccessibilityAction::SetValue { id, value } => {
                if value.len() > 256 {
                    return false;
                }
                if *id == ACCESS_DOCUMENTS + 1 && self.panels.documents.open {
                    self.panels.document_filter = value.clone();
                    self.panels.documents.set_filter(value);
                    self.panels.focus = Focus::Documents;
                } else if *id == ACCESS_OUTLINE + 1 && self.panels.outline.open {
                    self.panels.outline_filter = value.clone();
                    self.panels.outline.set_filter(value);
                    self.panels.focus = Focus::Outline;
                } else {
                    return false;
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return true;
            }
            _ => return false,
        };
        if !self.panels.semantics().iter().any(|entry| entry.node.id.0 == id) {
            return false;
        }
        if (ACCESS_EXPLORER..ACCESS_DOCUMENTS).contains(&id) {
            self.panels.focus = Focus::Explorer;
            if let Some(local) = id.checked_sub(ACCESS_EXPLORER + 65536) {
                let action = self.panels.explorer().accessibility_action(
                    bareline_ui::virtual_tree::NodeId(local),
                    if invoke {
                        SemanticAction::Invoke
                    } else {
                        SemanticAction::Focus
                    },
                );
                if let Some(PanelAction::Open(path)) = action
                    && self.ensure_workspace(el)
                {
                    self.panel_open_file(el, path);
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
                && let Some(offset) =
                    self.panels
                        .outline
                        .accessibility_action(id, ACCESS_OUTLINE, invoke, editor.snapshot())
            {
                let mut selection = editor.viewport().selection;
                selection.anchor = offset.0;
                selection.caret = offset.0;
                let _ = editor.set_selections(selection.into());
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn panels_context_commands(&mut self) -> Option<Vec<bareline_commands::CommandId>> {
        if !self.panels.left.contains(self.pointer) {
            return None;
        }
        let commands: &[&'static str] = if self.panels.documents.open {
            self.panels.documents.pointer(self.pointer);
            self.panels.focus = Focus::Documents;
            &DOCUMENTS_CONTEXT_COMMANDS
        } else {
            self.panels.focus = Focus::Explorer;
            let point = Point {
                x: self.pointer.x,
                y: self.pointer.y - TAB_HEIGHT,
            };
            self.panels.explorer().select_context(point);
            &EXPLORER_CONTEXT_COMMANDS
        };
        Some(commands.iter().copied().map(bareline_commands::CommandId).collect())
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
                if self.panels.outline_import.is_some() {
                    return true;
                }
                let path = self.platform.as_ref().and_then(|p| p.open_file().ok().flatten());
                let Some(path) = path else {
                    return true;
                };
                let extension = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.path(self.app.active))
                    .and_then(|p| p.extension())
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_owned();
                let xml = id == "outline.importFunctionList";
                self.panels.import_cancel = Default::default();
                let cancel = self.panels.import_cancel.clone();
                let (tx, rx) = mpsc::sync_channel(1);
                let notify = self.notify.clone();
                match std::thread::Builder::new()
                    .name("outline-definition-import".into())
                    .spawn(move || {
                        use std::io::Read;
                        let result = (|| -> Result<_, String> {
                            let _guard = crate::shell::native::PathTrust
                                .open_read(&path, PathOrigin::User)
                                .map_err(|e| e.to_string())?;
                            let file = crate::shell::native::FileSystem
                                .open_sealed_read(&path)
                                .map_err(|e| e.to_string())?;
                            let mut bytes = Vec::new();
                            file.take(256 * 1024 + 1)
                                .read_to_end(&mut bytes)
                                .map_err(|e| e.to_string())?;
                            if bytes.len() > 256 * 1024 {
                                return Err("Outline definition exceeds 256 KiB".into());
                            }
                            let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
                            let (definition, report) = if xml {
                                let (definition, report) =
                                    bareline_syntax::outline::import_function_list_with_job(text, &cancel)?;
                                let report = report
                                    .iter()
                                    .map(|m| format!("{}: {} — {}", m.kind.label(), m.field, m.reason))
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                (definition, report)
                            } else {
                                (
                                    bareline_syntax::outline::Definition::from_toml(text)?,
                                    "Outline definition loaded".into(),
                                )
                            };
                            if cancel.is_cancelled() {
                                return Err("Outline import cancelled".into());
                            }
                            Ok((definition, extension, report))
                        })();
                        let _ = tx.send(result);
                        notify();
                    }) {
                    Ok(_) => {
                        self.panels.outline_import = Some(rx);
                        self.panels.outline.status = "Importing outline definition…".into();
                    }
                    Err(error) => self.panels.outline.status = error.to_string(),
                }
            }
            "outline.exportDefinition" => {
                if self.panels.operation.is_some() {
                    return true;
                }
                let Some(definition) = self.panels.outline.definition() else {
                    return true;
                };
                let definition = definition.clone();
                let options = bareline_platform::SaveDialogOptions::new(bareline_platform::SaveFileKind::Toml);
                let Some(path) = self
                    .platform
                    .as_ref()
                    .and_then(|p| p.save_file_with(&options).ok().flatten())
                else {
                    return true;
                };
                let (tx, rx) = mpsc::sync_channel(1);
                let notify = self.notify.clone();
                match std::thread::Builder::new()
                    .name("outline-definition-export".into())
                    .spawn(move || {
                        use std::io::Write;
                        let result = (|| -> Result<_, String> {
                            let text = definition.to_toml()?;
                            let fs = crate::shell::native::FileSystem;
                            let parent = path.parent().ok_or("Missing destination parent")?;
                            let _guard = fs.guard_directory(parent).map_err(|e| e.to_string())?;
                            fs.validate_target(&path).map_err(|e| e.to_string())?;
                            let stage = parent.join(format!(
                                ".bareline-outline-{}-{}.tmp",
                                std::process::id(),
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_nanos()
                            ));
                            let mut file = std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(&stage)
                                .map_err(|e| e.to_string())?;
                            let result = file.write_all(text.as_bytes()).and_then(|()| file.sync_all());
                            drop(file);
                            let result = result.and_then(|()| fs.commit(&stage, &path, path.exists()));
                            if result.is_err() {
                                let _ = std::fs::remove_file(&stage);
                            }
                            result.map_err(|e| e.to_string())?;
                            Ok("File operation completed")
                        })();
                        let _ = tx.send(result);
                        notify();
                    }) {
                    Ok(_) => {
                        self.panels.operation = Some(rx);
                    }
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
                    self.panels_open_root(path);
                }
            }
            "workspace.createFile" | "workspace.createFolder" | "workspace.rename" | "workspace.delete" => {
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
                    self.platform.as_ref().and_then(|p| p.save_file().ok().flatten())
                };
                if id != "workspace.delete" && destination.is_none() {
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
                        w.message = Some("Close the document before renaming or deleting its file".into());
                    }
                    return true;
                }
                let kind = id.to_owned();
                // The shell's warning for an entry it cannot recycle is owned by the
                // editor window, so it cannot open behind it (WSP-17).
                let owner = self
                    .window
                    .as_ref()
                    .and_then(|window| crate::shell::native::raw_window(window).ok())
                    .unwrap_or(0);
                let (tx, rx) = mpsc::sync_channel(1);
                let notify = self.notify.clone();
                if std::thread::Builder::new()
                    .name("workspace-file-action".into())
                    .spawn(move || {
                        let fs = crate::shell::native::FileSystem;
                        let result = match kind.as_str() {
                            // Restorable from the Recycle Bin or Trash; nothing hidden stays in the folder.
                            "workspace.delete" => {
                                crate::shell::native::recycle_entry(&fs, selected.as_ref().unwrap(), owner)
                                    .map(|()| crate::shell::native::RECYCLED)
                            }
                            "workspace.createFile" => fs
                                .create_entry(destination.as_ref().unwrap(), false)
                                .map(|()| "File operation completed"),
                            "workspace.createFolder" => fs
                                .create_entry(destination.as_ref().unwrap(), true)
                                .map(|()| "File operation completed"),
                            _ => fs
                                .rename_entry(selected.as_ref().unwrap(), destination.as_ref().unwrap())
                                .map(|()| "File operation completed"),
                        }
                        .map_err(|e| e.to_string());
                        let _ = tx.send(result);
                        notify();
                    })
                    .is_ok()
                {
                    self.panels.operation = Some(rx);
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
    fn panel_open_file(&mut self, el: &ActiveEventLoop, path: PathBuf) {
        let existing = self.workspace.as_ref().and_then(|workspace| {
            (0..workspace.editors.len()).find(|index| workspace.path(*index) == Some(path.as_path()))
        });
        if let Some(index) = existing {
            self.panel_document_action(el, DocumentAction::Activate(index));
        } else if self.ensure_workspace(el) {
            self.workspace.as_mut().unwrap().open(path);
        }
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
    /// Authorizes `path` off the UI thread; `panels_pump` then opens it as the
    /// workspace folder. False while another folder is still being authorized.
    pub(super) fn panels_open_root(&mut self, path: PathBuf) -> bool {
        if self.panels.root.is_some() {
            return false;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let notify = self.notify.clone();
        if std::thread::Builder::new()
            .name("workspace-root-trust".into())
            .spawn(move || {
                let provider = crate::shell::native::PathTrust;
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
        true
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
                    self.shell_recent_folder_opened(&path);
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
                Ok(message) => {
                    if let Some(panel) = &mut self.panels.explorer {
                        panel.refresh_tree();
                    }
                    message.to_string()
                }
                Err(error) => format!("File operation failed: {error}"),
            };
            self.panels.explorer().message = Some(message.clone());
            if let Some(workspace) = &mut self.workspace {
                workspace.message = Some(message);
            }
        }
        if changed && let Some(w) = &self.window {
            w.request_redraw();
        }
    }
    pub(super) fn workspace_watch_roots(&self) -> Vec<PathBuf> {
        self.panels
            .explorer
            .as_ref()
            .map(|p| p.watch_roots())
            .unwrap_or_default()
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
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if self.panels.dragging_left => {
                self.panels.dragging_left = false;
                handled = true;
                // Persist the new width so it survives restart (UX-50).
                let serialized = self.panels.dock_widths_serialized();
                let _ = self.settings.controller.edit(
                    "workspace.dock.widths",
                    bareline_settings::SettingValue::Text(serialized),
                );
            }
            WindowEvent::CursorMoved { position, .. } if self.panels.dragging_left => {
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor() as f32);
                let x = position.x as f32 / scale;
                self.panels.widths.left = x.clamp(DockWidths::MIN, 600.0);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                return true;
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let point = self.pointer;
                if let Some((section, _)) = self
                    .panels
                    .left_sections
                    .iter()
                    .copied()
                    .find(|(_, layout)| layout.close.contains(point))
                {
                    // Header × closes just that section.
                    handled = true;
                    match section {
                        LeftSection::Workspace => {
                            if let Some(explorer) = &mut self.panels.explorer {
                                explorer.open = false;
                            }
                        }
                        LeftSection::Documents => self.panels.documents.open = false,
                        LeftSection::Outline => self.panels.outline.open = false,
                    }
                } else if self.panels.splitter_left.contains(point) {
                    handled = true;
                    self.panels.dragging_left = true;
                } else if self.panels.left.contains(point) {
                    handled = true;
                    if let Some((section, layout)) = self
                        .panels
                        .left_sections
                        .iter()
                        .copied()
                        .find(|(_, layout)| layout.body.is_some_and(|body| body.contains(point)))
                    {
                        match section {
                            LeftSection::Documents => {
                                self.panels.focus = Focus::Documents;
                                document = self.panels.documents.pointer(point);
                            }
                            LeftSection::Workspace => {
                                self.panels.focus = Focus::Explorer;
                                let body = layout.body.unwrap();
                                explorer = self.panels.explorer().pointer(Point {
                                    x: point.x,
                                    y: point.y - body.y,
                                });
                            }
                            LeftSection::Outline => {
                                self.panels.focus = Focus::Outline;
                                if let Some(e) = self.workspace.as_ref().and_then(|w| w.editors.get(self.app.active)) {
                                    navigation = self.panels.outline.pointer(point, e.snapshot());
                                }
                            }
                        }
                    }
                } else if self.panels.map_bounds.contains(point) {
                    handled = true;
                    if let Some(e) = self.workspace.as_ref().and_then(|w| w.editors.get(self.app.active)) {
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
                            self.panels.documents.set_filter(&self.panels.document_filter);
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
                            if let Some(e) = self.workspace.as_ref().and_then(|w| w.editors.get(self.app.active)) {
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
            self.panel_open_file(el, path);
        }
        if let Some(action) = document {
            self.panel_document_action(el, action);
        }
        if let Some(offset) = navigation
            && let Some(editor) = self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active))
        {
            let mut selection = editor.viewport().selection;
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
pub(super) fn accessibility_test_cases() -> Vec<(
    &'static str,
    Vec<bareline_platform::accessibility::AccessibilityNode>,
    Option<u64>,
)> {
    use bareline_document::{Budget, Document};
    use bareline_ui::widgets::SemanticAction;
    fn capture(
        runtime: &WorkspacePanelsRuntime,
        name: &'static str,
    ) -> (
        &'static str,
        Vec<bareline_platform::accessibility::AccessibilityNode>,
        Option<u64>,
    ) {
        (name, runtime.accessibility_nodes(), runtime.accessibility_focus())
    }
    let mut runtime = WorkspacePanelsRuntime::default();
    let mut renderer = bareline_renderer_recording::RecordingBackend::default();
    let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(crate::shell::native::FileSystem)).unwrap();
    let source = Document::from_utf8("fn first() {}\nfn second() {}\n", Budget::new(4096), Budget::new(4096))
        .unwrap()
        .snapshot();
    workspace.add_snapshot_preview(&source, "main.rs".into()).unwrap();
    workspace.add_snapshot_preview(&source, "notes.rs".into()).unwrap();
    let mut ops = Vec::new();
    runtime
        .draw(&mut renderer, 1000.0, 800.0, &workspace, 0, &mut ops)
        .unwrap();
    let mut cases = vec![capture(&runtime, "panels.closed")];

    let mut explorer = WorkspacePanel::new(Arc::new(|| {}));
    explorer.add_root(PathBuf::from("golden-workspace"));
    runtime.explorer = Some(explorer);
    runtime
        .draw(&mut renderer, 1000.0, 800.0, &workspace, 0, &mut ops)
        .unwrap();
    cases.push(capture(&runtime, "panels.explorer.open"));
    runtime.focus = Focus::Explorer;
    runtime.explorer.as_mut().unwrap().key(UiKey::Home);
    let root_id = runtime
        .explorer
        .as_ref()
        .unwrap()
        .semantics(bareline_ui::ViewId(ACCESS_EXPLORER), ACCESS_EXPLORER + 65536)[0]
        .node
        .id
        .0;
    runtime.explorer.as_mut().unwrap().accessibility_action(
        bareline_ui::virtual_tree::NodeId(root_id - ACCESS_EXPLORER - 65536),
        SemanticAction::Focus,
    );
    cases.push(capture(&runtime, "panels.explorer.focus"));

    runtime.explorer.as_mut().unwrap().hide();
    runtime.documents.open = true;
    runtime.focus = Focus::Documents;
    runtime
        .draw(&mut renderer, 1000.0, 800.0, &workspace, 0, &mut ops)
        .unwrap();
    cases.push(capture(&runtime, "panels.documents.populated"));
    runtime.document_filter = "notes".into();
    runtime.documents.set_filter("notes");
    runtime.documents.draw(runtime.left, &mut ops);
    let selected = runtime
        .documents
        .semantics(bareline_ui::ViewId(ACCESS_DOCUMENTS), ACCESS_DOCUMENTS, true)[0]
        .node
        .id
        .0;
    assert!(matches!(
        runtime.documents.accessibility_action(selected, ACCESS_DOCUMENTS, true),
        Some(DocumentAction::Activate(1))
    ));
    cases.push(capture(&runtime, "panels.documents.filtered-invoked"));

    runtime.documents.open = false;
    runtime.outline.open = true;
    runtime.focus = Focus::Outline;
    runtime
        .draw(&mut renderer, 1000.0, 800.0, &workspace, 0, &mut ops)
        .unwrap();
    let (wake, ready) = mpsc::channel();
    runtime.outline.refresh(
        &source,
        Some(std::path::Path::new("main.rs")),
        "main.rs",
        Arc::new(move || {
            let _ = wake.send(());
        }),
    );
    ready
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("bounded outline fixture completion");
    assert!(runtime.outline.pump());
    runtime.outline.draw(runtime.left, &mut ops);
    cases.push(capture(&runtime, "panels.outline.populated"));
    runtime.outline_filter = "second".into();
    runtime.outline.set_filter("second");
    runtime.outline.draw(runtime.left, &mut ops);
    let selected = runtime
        .outline
        .semantics(bareline_ui::ViewId(ACCESS_OUTLINE), ACCESS_OUTLINE, true)[0]
        .node
        .id
        .0;
    assert_eq!(
        runtime
            .outline
            .accessibility_action(selected, ACCESS_OUTLINE, false, &source),
        None
    );
    assert_eq!(
        runtime
            .outline
            .accessibility_action(selected, ACCESS_OUTLINE, true, &source),
        Some(bareline_document::TextOffset(17))
    );
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
    runtime.outline.draw(runtime.left, &mut ops);
    cases.push(capture(&runtime, "panels.all_open"));
    cases
}

#[cfg(test)]
mod workspace_panel_regressions {
    use super::*;
    use bareline_ui::rect;

    /// One frame composed as `render_frame` does it: the base chrome, the editor
    /// layer (views and their tab strip) beside the docks, then the docks.
    fn frame(
        shell: &mut Shell,
        renderer: &mut bareline_renderer_recording::RecordingBackend,
        width: f32,
        height: f32,
    ) -> (Vec<DrawOp>, Rect) {
        let editor = shell.editor_bounds_in(width, height);
        let mut ops = shell.frame_chrome(width, height);
        let start = ops.len();
        let workspace = shell.workspace.as_mut().unwrap();
        shell.views.sync(workspace, &mut shell.app);
        shell
            .views
            .draw(
                workspace,
                &mut shell.app,
                renderer,
                editor.width,
                editor.height,
                &mut ops,
                Arc::new(|| {}),
            )
            .unwrap();
        crate::shell::translate_operations(&mut ops[start..], editor.x, editor.y);
        let workspace = shell.workspace.as_ref().unwrap();
        shell
            .panels
            .draw(renderer, width, height, workspace, shell.app.active, &mut ops)
            .unwrap();
        (ops, editor)
    }

    /// Reported on Windows with the Workspace panel open: a second tab row ran
    /// from x=0 over the dock's column, and the tree sat in a fixed 238 px box
    /// that stayed put when the window or the dock grew. Every frame lays the
    /// dock, the editor pane and its one tab strip out from that frame's window
    /// size and dock width, and the dock's splitter keeps its resize pointer.
    #[test]
    fn workspace_dock_follows_the_window_and_leaves_one_tab_strip_over_the_editor() {
        use bareline_ui::widgets::SECTION_HEADER;
        use winit::window::CursorIcon;
        let mut shell = super::super::accessibility::tests::headless_shell();
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(crate::shell::native::FileSystem)).unwrap();
        for _ in 0..3 {
            workspace.new_document().unwrap();
        }
        shell.app.tabs = workspace.titles();
        shell.workspace = Some(workspace);
        // A chosen folder opens the Workspace section; adding the root does no I/O.
        let mut explorer = WorkspacePanel::new(Arc::new(|| {}));
        explorer.add_root(PathBuf::from("dock-fixture"));
        shell.panels.explorer = Some(explorer);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let panel = shell.workspace.as_ref().unwrap().theme.panel();
        frame(&mut shell, &mut renderer, 1200.0, 760.0);
        // Without keyboard focus on the explorer the tree draws no ring: the
        // files are never framed while the editor or another panel is focused.
        {
            shell.panels.focus = Focus::Editor;
            let (ops, _) = frame(&mut shell, &mut renderer, 1200.0, 760.0);
            assert!(
                !ops.iter()
                    .any(|op| matches!(op, DrawOp::Stroke(_, color, _) if *color == panel.focus)),
                "the tree rings its bounds without focus"
            );
        }
        // A click under the rows focuses the tree, which rings its bounds while
        // the explorer owns the shell's focus.
        assert!(shell.panels.explorer().pointer(Point { x: 20.0, y: 400.0 }).is_none());
        shell.panels.focus = Focus::Explorer;

        let mut stale: Vec<Rect> = Vec::new();
        for (width, height, dock) in [
            (1200.0, 760.0, DockWidths::default().left),
            (1600.0, 1000.0, DockWidths::default().left),
            (1600.0, 1000.0, 320.0),
        ] {
            shell.panels.widths.left = dock;
            let (ops, editor) = frame(&mut shell, &mut renderer, width, height);
            let size = format!("{width}x{height} with a {dock} px dock");
            assert_eq!(editor, rect(dock, 0.0, width - dock, height), "{size}");

            // Exactly one tab strip, over the editor pane: each title is drawn
            // once, right of the dock, and the tab row above the dock's column
            // holds nothing but the band's own chrome fill.
            assert_eq!(
                shell.views.test_tab_strips(),
                [Some(rect(0.0, 0.0, editor.width, TAB_HEIGHT)), None],
                "{size}"
            );
            for title in shell.workspace.as_ref().unwrap().titles() {
                let drawn: Vec<f32> = ops
                    .iter()
                    .filter_map(|op| match op {
                        DrawOp::Text { origin, text, .. } if origin.y < TAB_HEIGHT && *text == title => Some(origin.x),
                        _ => None,
                    })
                    .collect();
                assert_eq!(drawn.len(), 1, "{title} at {drawn:?}, {size}");
                assert!(drawn[0] > editor.x, "{title} at {drawn:?}, {size}");
            }
            let above_dock: Vec<_> = ops
                .iter()
                .filter(|op| match op {
                    DrawOp::Fill(r, _) | DrawOp::Stroke(r, _, _) => r.y < TAB_HEIGHT && r.x < dock,
                    DrawOp::Text { origin, .. } => origin.y < TAB_HEIGHT && origin.x < dock,
                    _ => false,
                })
                .collect();
            assert_eq!(above_dock.len(), 2, "{above_dock:?}, {size}");
            assert!(
                above_dock
                    .iter()
                    .all(|op| matches!(op, DrawOp::Fill(r, _) if r.x == 0.0 && r.width == width)),
                "{above_dock:?}, {size}"
            );

            // The Workspace section fills the dock column under its header, and
            // the tree fills the section under the panel's title.
            let body = rect(
                0.0,
                TAB_HEIGHT + SECTION_HEADER,
                dock,
                height - TAB_HEIGHT - STATUS_HEIGHT - SECTION_HEADER,
            );
            assert!(ops.contains(&DrawOp::Fill(body, panel.surface)), "{size}");
            assert!(ops.contains(&DrawOp::PushClip(body)), "{size}");
            let rings: Vec<Rect> = ops
                .iter()
                .filter_map(|op| match op {
                    DrawOp::Stroke(r, color, _) if *color == panel.focus && r.x < dock => Some(*r),
                    _ => None,
                })
                .collect();
            let tree = rect(0.0, body.y + 34.0, dock, body.height - 34.0);
            assert_eq!(rings, [tree], "{size}");

            // Nothing is drawn or clipped at an earlier frame's size.
            for old in &stale {
                assert!(
                    !ops.iter().any(|op| matches!(
                        op,
                        DrawOp::Fill(r, _) | DrawOp::Stroke(r, _, _) | DrawOp::PushClip(r) if r == old
                    )),
                    "stale {old:?} at {size}"
                );
            }
            stale.extend([body, tree]);

            // The splitter on the dock's edge resizes it; the dock itself does not.
            let at = |x: f32, y: f32| shell.pointer_cursor_in(Point { x, y }, editor);
            assert_eq!(at(dock + 3.0, height / 2.0), CursorIcon::ColResize, "{size}");
            assert_eq!(at(dock / 2.0, height / 2.0), CursorIcon::Default, "{size}");
        }

        // The bottom panel's sash resizes that panel beside the open dock.
        let ready = crate::shell::dock::DockSurfaceState {
            available: true,
            revision: 1,
            ..Default::default()
        };
        let empty = crate::shell::dock::DockSurfaceState::default();
        shell.dock.sync([ready, empty, empty]);
        let (_, editor) = frame(&mut shell, &mut renderer, 1600.0, 1000.0);
        let sash = shell.dock.layout(editor.width, editor.height).unwrap().splitter;
        let point = Point {
            x: editor.x + 200.0,
            y: editor.y + sash.y + 2.0,
        };
        assert_eq!(shell.pointer_cursor_in(point, editor), CursorIcon::RowResize);
    }

    #[test]
    fn disconnected_worker_is_a_terminal_error() {
        let (sender, receiver) = mpsc::sync_channel::<Result<(), String>>(1);
        let pending = Some(receiver);
        assert!(receive_job(&pending).is_none());
        drop(sender);
        assert!(receive_job(&pending).unwrap().is_err());
    }
}
