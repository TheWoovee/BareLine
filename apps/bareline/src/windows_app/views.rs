// SPDX-License-Identifier: MPL-2.0
//! Native two-pane consumer. Both surfaces address the same document actor when cloned.
use super::*;
use bareline_app::views::{
    Orientation, ScrollPosition, SessionTab, SharedEditorView, SharedSyntaxView, ViewController,
    ViewSnapshot, ViewState,
};
use bareline_app::workspace::WorkspaceEditor;
use bareline_commands::{CommandId, CommandPresentation, CommandRegistry, CommandSpec};
use bareline_renderer::{DrawOp, LayoutError, Rect, TextBackend};
use bareline_ui::{ACCENT, BORDER, CHROME, MUTED, TAB_HEIGHT, TEXT, rect, text};
use std::collections::VecDeque;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_capture_preserves_unavailable_tab_when_temporary_id_collides() {
        let mut workspace = Workspace::new(std::sync::Arc::new(|| {}), std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        views.sync_documents(&workspace);
        let mut manifest = bareline_file_io::session::SessionManifest {
            documents: vec![
                bareline_file_io::session::SessionDocument { id: 1, path: None, title: "Live".into() },
                bareline_file_io::session::SessionDocument { id: 2, path: None, title: "Awaiting recovery".into() },
            ],
            tabs: vec![
                SessionTab { id: 7, document_id: 1, pinned: false, view: ViewState::default() },
                SessionTab { id: 1, document_id: 2, pinned: false, view: ViewState::default() },
            ],
            active_tab: Some(7),
            ..Default::default()
        };
        views.capture_session(&workspace, &mut manifest, &[(0, 7)]);
        manifest.validate().unwrap();
        assert_eq!(manifest.tabs.len(), 2);
        assert!(manifest.tabs.iter().any(|tab| tab.document_id == 2));
        assert_ne!(manifest.tabs[0].id, manifest.tabs[1].id);
    }

    #[test]
    fn queued_edits_in_both_native_panes_share_the_document() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        views.split(&mut workspace, 0, Orientation::Vertical);
        views.input(&mut workspace, 0, Input::Insert("first".into()));
        views.input(&mut workspace, 1, Input::End(false));
        views.input(&mut workspace, 1, Input::Insert(" second".into()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while views.busy(&workspace) {
            assert!(
                Instant::now() < deadline,
                "pane edit acknowledgment timed out"
            );
            workspace.pump();
            views.pump(&mut workspace);
            std::thread::yield_now();
        }
        let primary = workspace.editors[0].snapshot().clone();
        let secondary = views.secondary.as_ref().unwrap().snapshot();
        assert!(primary.same_document(secondary));
        assert_eq!(primary.revision, secondary.revision);
        assert_eq!(
            primary
                .read(
                    bareline_document::TextOffset(0)..bareline_document::TextOffset(primary.len()),
                    100
                )
                .unwrap(),
            "first second"
        );
        workspace.editors[0].set_font_family("Consolas").unwrap();
        views.pump(&mut workspace);
        assert_eq!(views.secondary.as_ref().unwrap().font_family(), "Consolas");
        workspace.editors[0].set_logical_scroll(0, 0.0, 84.0);
        views
            .secondary
            .as_mut()
            .unwrap()
            .set_logical_scroll(0, 0.0, 13.0);
        views.controller.as_mut().unwrap().sync_vertical = true;
        views.sync_scroll(&mut workspace, 0);
        assert_eq!(views.secondary.as_ref().unwrap().logical_scroll().2, 13.0);
        views.controller.as_mut().unwrap().sync_horizontal = true;
        views.sync_scroll(&mut workspace, 0);
        assert_eq!(views.secondary.as_ref().unwrap().logical_scroll().2, 84.0);
        views.secondary.as_mut().unwrap().selection.anchor = 6;
        views.secondary.as_mut().unwrap().scroll_y = 40.0;
        views.controller.as_mut().unwrap().ratio = 0.65;
        let mut manifest = bareline_file_io::session::SessionManifest::default();
        manifest
            .documents
            .push(bareline_file_io::session::SessionDocument {
                id: 1,
                path: None,
                title: "Untitled".into(),
            });
        manifest.tabs.push(SessionTab {
            id: 1,
            document_id: 1,
            pinned: false,
            view: ViewState::default(),
        });
        manifest.active_tab = Some(1);
        views.capture_session(&workspace, &mut manifest, &[(0, 1)]);
        manifest.validate().unwrap();
        assert_eq!(manifest.tabs.len(), 2);
        let mut restored = ViewsRuntime::default();
        let mut app = App::default();
        restored.restore_session(&mut workspace, &mut app, &manifest, &[(1, 0), (2, 0)]);
        assert!(restored.open());
        assert_eq!(restored.secondary.as_ref().unwrap().selection.anchor, 6);
        assert_eq!(restored.secondary.as_ref().unwrap().scroll_y, 40.0);
        assert_eq!(restored.controller.as_ref().unwrap().ratio, 0.65);
        views.collapse(&mut workspace, true);
        assert!(!views.open());
        assert_eq!(workspace.editors[0].snapshot().revision, primary.revision);
    }
}

pub(super) fn register(registry: &mut CommandRegistry) {
    for (id, title, shortcut) in [
        ("view.split_vertical", "Split Vertically", ""),
        ("view.split_horizontal", "Split Horizontally", ""),
        ("view.clone_other", "Clone to Other View", ""),
        ("view.move_other", "Move to Other View", ""),
        ("view.close_split", "Close Split View", ""),
        ("view.focus_other", "Focus Other View", "F6"),
        ("view.sync_vertical", "Synchronize Vertical Scrolling", ""),
        (
            "view.sync_horizontal",
            "Synchronize Horizontal Scrolling",
            "",
        ),
        ("view.tabs.vertical", "Vertical Tabs", ""),
        ("view.tabs.pin", "Pin or Unpin Tab", ""),
        ("view.tabs.color", "Cycle Tab Color", ""),
        ("view.tabs.sort_name", "Sort Tabs by Name", ""),
        ("view.tabs.sort_path", "Sort Tabs by Path", ""),
        ("view.tabs.sort_descending", "Sort Tabs Descending", ""),
        ("view.tabs.move_left", "Move Tab Left", "Ctrl+Shift+PageUp"),
        (
            "view.tabs.move_right",
            "Move Tab Right",
            "Ctrl+Shift+PageDown",
        ),
        ("view.tabs.previous", "Previous Tab", "Ctrl+PageUp"),
        ("view.tabs.next", "Next Tab", "Ctrl+PageDown"),
        ("view.tabs.mru", "Recent Document Switcher", "Ctrl+Tab"),
    ] {
        let id = CommandId(id);
        let _ = registry.register(CommandSpec {
            id,
            title,
            category: "View",
            shortcut,
            action: Action::Contributed(id),
        });
        let _ = registry.set_presentation(
            id,
            CommandPresentation {
                menu_path: format!("View > {title}"),
                keywords: vec!["split".into(), "pane".into()],
                accessible_name: Some(title.into()),
                ..Default::default()
            },
        );
    }
}

struct QueuedInput {
    pane: u32,
    document: DocumentBinding,
    input: Input,
}
#[derive(Clone)]
enum DocumentBinding {
    Resident(u64, ViewSnapshot),
    Paged(u64, bareline_document::paged::PagedSnapshot),
}
impl DocumentBinding {
    fn id(&self) -> u64 {
        match self {
            Self::Resident(id, _) | Self::Paged(id, _) => *id,
        }
    }
    fn new(id: u64, editor: &bareline_app::workspace::WorkspaceEditor) -> Self {
        match editor {
            bareline_app::workspace::WorkspaceEditor::Resident(editor) => {
                Self::Resident(id, editor.snapshot().clone())
            }
            bareline_app::workspace::WorkspaceEditor::Paged(editor) => {
                Self::Paged(id, editor.snapshot().clone())
            }
        }
    }
    fn matches(&self, editor: &bareline_app::workspace::WorkspaceEditor) -> bool {
        match (self, editor) {
            (
                Self::Resident(_, snapshot),
                bareline_app::workspace::WorkspaceEditor::Resident(editor),
            ) => snapshot.same_document(editor.snapshot()),
            (Self::Paged(_, snapshot), bareline_app::workspace::WorkspaceEditor::Paged(editor)) => {
                snapshot.same_document(editor.snapshot())
            }
            _ => false,
        }
    }
}
#[derive(Clone, Copy)]
struct TabHit {
    id: u64,
    pane: u32,
    bounds: Rect,
    close: Rect,
}
struct TabDrag {
    id: u64,
    start: Point,
    moved: bool,
}
struct MruPopup {
    ids: Vec<u64>,
    selected: usize,
    bounds: Rect,
}
#[derive(Default)]
pub(super) struct ViewsRuntime {
    documents: Vec<DocumentBinding>,
    closed_documents: VecDeque<(DocumentBinding, Vec<(usize, SessionTab, Option<u32>)>)>,
    next_document: u64,
    loaded_tabs: [Option<u64>; 2],
    pending_restore: [Option<ViewState>; 2],
    pending_view_scroll: [Option<ViewState>; 2],
    tab_hits: Vec<TabHit>,
    tab_strips: [Option<Rect>; 2],
    tab_nav: Vec<(u32, bool, Rect)>,
    tab_offset: [usize; 2],
    tab_drag: Option<TabDrag>,
    mru_popup: Option<MruPopup>,
    accessibility_focus: Option<u64>,
    pending_close: Option<usize>,
    controller: Option<ViewController>,
    primary: Option<ViewSnapshot>,
    pub(super) secondary: Option<WorkspaceEditor>,
    retired: Vec<WorkspaceEditor>,
    pub(super) bounds: [Option<Rect>; 2],
    splitter: Option<Rect>,
    dragging: bool,
    queued: VecDeque<QueuedInput>,
    compare: bool,
    alignment: Option<bareline_app::views::AlignmentMap>,
    applied_spacers: [Option<Vec<(u64, u64)>>; 2],
}
impl ViewsRuntime {
    fn draw_tab_strip(
        &mut self,
        workspace: &Workspace,
        pane: u32,
        bounds: Rect,
        vertical: bool,
        ops: &mut Vec<DrawOp>,
    ) {
        self.tab_strips[pane as usize] = Some(bounds);
        let Some(controller) = &self.controller else {
            return;
        };
        let tabs: Vec<_> = controller.pane_tabs(pane).cloned().collect();
        let step = if vertical {
            TAB_HEIGHT
        } else {
            bareline_ui::controls::TabStrip::TAB_WIDTH
        };
        let extent = if vertical {
            bounds.height
        } else {
            bounds.width
        };
        let count = ((extent - 48.0) / step).floor().max(1.0) as usize;
        let start = self.tab_offset[pane as usize].min(tabs.len().saturating_sub(count));
        self.tab_offset[pane as usize] = start;
        ops.push(DrawOp::Fill(bounds, workspace.theme.chrome));
        ops.push(DrawOp::PushClip(bounds));
        let titles = workspace.titles();
        for (row, tab) in tabs.iter().skip(start).take(count).enumerate() {
            let bounds = if vertical {
                rect(
                    bounds.x,
                    bounds.y + row as f32 * step,
                    bounds.width,
                    TAB_HEIGHT,
                )
            } else {
                rect(bounds.x + row as f32 * step, bounds.y, step, TAB_HEIGHT)
            };
            let selected = controller.active_tab(pane) == Some(tab.id);
            let index = self.document_index(workspace, tab.document_id);
            let title = index
                .and_then(|index| titles.get(index))
                .map(String::as_str)
                .unwrap_or("Document");
            let dirty = index.is_some_and(|index| workspace.editors[index].dirty());
            ops.push(DrawOp::Fill(
                bounds,
                if selected {
                    workspace.theme.editor
                } else {
                    workspace.theme.chrome
                },
            ));
            ops.push(DrawOp::Stroke(bounds, workspace.theme.border, 1.0));
            if let Some(color) = controller.tab_colors.get(&tab.id) {
                ops.push(DrawOp::Fill(
                    rect(bounds.x, bounds.y, 4.0, bounds.height),
                    bareline_renderer::Color(*color),
                ));
            }
            let label = format!(
                "{}{}{}",
                if tab.pinned { "◆ " } else { "" },
                title.chars().take(18).collect::<String>(),
                if dirty { " •" } else { "" }
            );
            text(
                ops,
                bounds.x + 10.0,
                bounds.y + 8.0,
                label,
                13.0,
                if selected {
                    workspace.theme.text
                } else {
                    workspace.theme.muted
                },
            );
            let close = rect(
                bounds.x + bounds.width - 24.0,
                bounds.y,
                24.0,
                bounds.height,
            );
            text(
                ops,
                close.x + 6.0,
                close.y + 7.0,
                "×",
                14.0,
                workspace.theme.muted,
            );
            if selected {
                ops.push(DrawOp::Fill(
                    rect(bounds.x, bounds.y + bounds.height - 2.0, bounds.width, 2.0),
                    workspace.theme.focus,
                ));
            }
            self.tab_hits.push(TabHit {
                id: tab.id,
                pane,
                bounds,
                close,
            });
        }
        for (next, offset, label) in [(false, 48.0, "‹"), (true, 24.0, "›")] {
            let nav = if vertical {
                rect(
                    bounds.x + if next { bounds.width / 2.0 } else { 0.0 },
                    bounds.y + bounds.height - 24.0,
                    bounds.width / 2.0,
                    24.0,
                )
            } else {
                rect(bounds.x + bounds.width - offset, bounds.y, 24.0, TAB_HEIGHT)
            };
            text(
                ops,
                nav.x + 8.0,
                nav.y + 6.0,
                label,
                14.0,
                workspace.theme.text,
            );
            self.tab_nav.push((pane, next, nav));
        }
        ops.push(DrawOp::PopClip);
    }
    fn draw_mru(&mut self, workspace: &Workspace, width: f32, height: f32, ops: &mut Vec<DrawOp>) {
        let Some(popup) = &self.mru_popup else {
            return;
        };
        let ids = popup.ids.clone();
        let selected = popup.selected;
        let bounds = rect(
            (width - 360.0).max(0.0) / 2.0,
            TAB_HEIGHT + 12.0,
            width.min(360.0),
            (height - 80.0).clamp(0.0, 12.0 * TAB_HEIGHT),
        );
        self.mru_popup.as_mut().unwrap().bounds = bounds;
        ops.push(DrawOp::Fill(bounds, workspace.theme.chrome));
        ops.push(DrawOp::Stroke(bounds, workspace.theme.border, 1.0));
        ops.push(DrawOp::PushClip(bounds));
        let titles = workspace.titles();
        let visible = mru_visible_rows(bounds);
        let start = selected.saturating_sub(visible.saturating_sub(1));
        for (row, id) in ids.iter().skip(start).take(visible).enumerate() {
            let row_bounds = rect(
                bounds.x,
                bounds.y + row as f32 * TAB_HEIGHT,
                bounds.width,
                TAB_HEIGHT,
            );
            if row + start == selected {
                ops.push(DrawOp::Fill(row_bounds, workspace.theme.interactive));
            }
            let title = self
                .tab_index(workspace, *id)
                .and_then(|index| titles.get(index))
                .cloned()
                .unwrap_or_default();
            text(
                ops,
                row_bounds.x + 12.0,
                row_bounds.y + 8.0,
                title,
                13.0,
                workspace.theme.text,
            );
        }
        ops.push(DrawOp::PopClip);
    }
    fn document_index(&self, workspace: &Workspace, id: u64) -> Option<usize> {
        let binding = self.documents.iter().find(|binding| binding.id() == id)?;
        workspace
            .editors
            .iter()
            .position(|editor| binding.matches(editor))
    }
    fn tab_index(&self, workspace: &Workspace, id: u64) -> Option<usize> {
        self.document_index(workspace, self.controller.as_ref()?.tab(id)?.document_id)
    }
    fn sync_documents(&mut self, workspace: &Workspace) {
        if self.controller.is_some()
            && self.documents.len() == workspace.editors.len()
            && self
                .documents
                .iter()
                .zip(&workspace.editors)
                .all(|(binding, editor)| binding.matches(editor))
        {
            return;
        }
        if self.controller.is_none() {
            self.controller = ViewController::new(Vec::new(), None).ok();
        }
        if let Some(controller) = &self.controller {
            for binding in &self.documents {
                if !workspace
                    .editors
                    .iter()
                    .any(|editor| binding.matches(editor))
                {
                    let tabs = controller
                        .tabs()
                        .iter()
                        .enumerate()
                        .filter(|(_, tab)| tab.document_id == binding.id())
                        .map(|(position, tab)| {
                            (
                                position,
                                tab.clone(),
                                controller.tab_colors.get(&tab.id).copied(),
                            )
                        })
                        .collect();
                    self.closed_documents.push_back((binding.clone(), tabs));
                    while self.closed_documents.len() > 20 {
                        self.closed_documents.pop_front();
                    }
                }
            }
        }
        self.documents.retain(|binding| {
            workspace
                .editors
                .iter()
                .any(|editor| binding.matches(editor))
        });
        for editor in &workspace.editors {
            if !self.documents.iter().any(|binding| binding.matches(editor)) {
                if let Some(position) = self
                    .closed_documents
                    .iter()
                    .position(|(binding, _)| binding.matches(editor))
                {
                    let (binding, tabs) = self.closed_documents.remove(position).unwrap();
                    self.documents.push(binding);
                    if let Some(controller) = &mut self.controller {
                        for (position, tab, color) in tabs {
                            let _ = controller.restore_tab(tab, position, color);
                        }
                    }
                    continue;
                }
                self.next_document = self.next_document.saturating_add(1);
                self.documents
                    .push(DocumentBinding::new(self.next_document, editor));
                if let Some(controller) = &mut self.controller {
                    if let Ok(id) = controller.add_document(self.next_document) {
                        let _ = controller.set_view_state(id, workspace_view_state(editor));
                    }
                }
            }
        }
        let live: Vec<_> = self.documents.iter().map(DocumentBinding::id).collect();
        if let Some(controller) = &mut self.controller {
            controller.retain_documents(&live);
        }
        self.documents.sort_by_key(|binding| {
            workspace
                .editors
                .iter()
                .position(|editor| binding.matches(editor))
                .unwrap_or(usize::MAX)
        });
    }
    fn save_view_states(&self, workspace: &Workspace, controller: &mut ViewController) {
        for pane in 0..2 {
            let Some(id) = self.loaded_tabs[pane] else {
                continue;
            };
            let editor = if pane == 1 {
                self.secondary.as_ref()
            } else {
                self.primary_index(workspace)
                    .map(|index| &workspace.editors[index])
            };
            if let Some(editor) = editor {
                let state = self.pending_restore[pane]
                    .as_ref()
                    .or(self.pending_view_scroll[pane].as_ref())
                    .cloned()
                    .unwrap_or_else(|| workspace_view_state(editor));
                let _ = controller.set_view_state(id, state);
            }
        }
    }
    fn save_current(&mut self, workspace: &Workspace) {
        if let Some(mut controller) = self.controller.clone() {
            self.save_view_states(workspace, &mut controller);
            self.controller = Some(controller);
        }
    }
    fn install_views(&mut self, workspace: &mut Workspace) {
        let Some(controller) = &self.controller else {
            return;
        };
        let ids = [
            controller.active_tab(0),
            if controller.split {
                controller.active_tab(1)
            } else {
                None
            },
        ];
        for pane in 0..2 {
            if ids[pane] == self.loaded_tabs[pane] {
                continue;
            }
            self.applied_spacers[pane] = None;
            let tab = ids[pane]
                .and_then(|id| self.controller.as_ref().unwrap().tab(id))
                .cloned();
            let index = tab
                .as_ref()
                .and_then(|tab| self.document_index(workspace, tab.document_id));
            if pane == 0 {
                self.pending_restore[pane] = None;
                self.pending_view_scroll[pane] = None;
                if let (Some(index), Some(tab)) = (index, tab) {
                    self.primary = Some(workspace.editors[index].snapshot().clone());
                    if workspace.editors[index].paged() {
                        self.pending_restore[pane] = Some(tab.view);
                    } else {
                        restore_workspace_view(&mut workspace.editors[index], &tab.view);
                    }
                } else {
                    self.primary = None;
                }
            } else {
                self.pending_restore[pane] = None;
                self.pending_view_scroll[pane] = None;
                if let Some(old) = self.secondary.take() {
                    self.retired.push(old);
                }
                if let (Some(index), Some(tab)) = (index, tab) {
                    let peer = match &workspace.editors[index] {
                        WorkspaceEditor::Resident(editor) => {
                            Ok(WorkspaceEditor::Resident(editor.clone_view()))
                        }
                        WorkspaceEditor::Paged(editor) => {
                            editor.clone_view().map(WorkspaceEditor::Paged)
                        }
                    };
                    match peer {
                        Ok(mut peer) => {
                            if peer.paged() {
                                self.pending_restore[pane] = Some(tab.view);
                            } else {
                                restore_workspace_view(&mut peer, &tab.view);
                            }
                            self.secondary = Some(peer);
                        }
                        Err(error) => workspace.message = Some(error),
                    }
                }
            }
            self.loaded_tabs[pane] = ids[pane];
        }
    }
    fn select_tab(&mut self, workspace: &mut Workspace, app: &mut App, id: u64) {
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before changing tabs.".into());
            return;
        }
        self.save_current(workspace);
        if self
            .controller
            .as_mut()
            .is_some_and(|controller| controller.activate(id).is_ok())
        {
            self.install_views(workspace);
            if let Some(index) = self.tab_index(workspace, id) {
                app.active = index;
            }
            if !self.tab_hits.iter().any(|hit| hit.id == id) {
                if let Some(controller) = &self.controller {
                    let pane = controller.active_pane();
                    self.tab_offset[pane as usize] = controller
                        .pane_tabs(pane)
                        .position(|tab| tab.id == id)
                        .unwrap_or(0);
                }
            }
        }
    }
    pub(super) fn active_editor<'a>(
        &'a self,
        workspace: &'a Workspace,
        fallback: usize,
    ) -> Option<&'a SharedEditorView> {
        if self.pane() == 1 {
            self.secondary.as_ref().map(|editor| &**editor)
        } else {
            workspace.editors.get(fallback).map(|editor| &**editor)
        }
    }
    pub(super) fn active_editor_mut<'a>(
        &'a mut self,
        workspace: &'a mut Workspace,
        fallback: usize,
    ) -> Option<&'a mut SharedEditorView> {
        if self.pane() == 1 {
            self.secondary.as_mut().map(|editor| &mut **editor)
        } else {
            workspace
                .editors
                .get_mut(fallback)
                .map(|editor| &mut **editor)
        }
    }
    pub(super) fn history_available(
        &self,
        workspace: Option<&Workspace>,
        active: usize,
        undo: bool,
    ) -> bool {
        if self.open() && self.pane() == 1 {
            return self
                .secondary
                .as_ref()
                .is_some_and(|e| if undo { e.can_undo() } else { e.can_redo() });
        }
        workspace
            .and_then(|w| w.editors.get(active))
            .is_some_and(|e| if undo { e.can_undo() } else { e.can_redo() })
    }

    pub(super) fn annotate_context(&self, context: &mut bareline_commands::CommandContext) {
        if let Some(controller) = &self.controller {
            context
                .states
                .entry(CommandId("view.tabs.vertical"))
                .or_default()
                .checked = controller.vertical_tabs;
            context
                .states
                .entry(CommandId("view.sync_horizontal"))
                .or_default()
                .checked = controller.sync_horizontal;
            context
                .states
                .entry(CommandId("view.tabs.pin"))
                .or_default()
                .checked = controller
                .active_tab(controller.active_pane())
                .and_then(|id| controller.tab(id))
                .is_some_and(|tab| tab.pinned);
        }
        for id in [
            "view.close_split",
            "view.focus_other",
            "view.sync_vertical",
            "view.sync_horizontal",
        ] {
            let state = context.states.entry(CommandId(id)).or_default();
            state.enabled = self.open();
            if !self.open() {
                state.disabled_reason = Some("Open a split view first.".into());
            }
        }
        for (id, checked) in [
            (
                "view.split_vertical",
                self.open()
                    && self
                        .controller
                        .as_ref()
                        .is_some_and(|controller| controller.orientation == Orientation::Vertical),
            ),
            (
                "view.split_horizontal",
                self.open()
                    && self.controller.as_ref().is_some_and(|controller| {
                        controller.orientation == Orientation::Horizontal
                    }),
            ),
            (
                "view.sync_vertical",
                self.controller
                    .as_ref()
                    .is_some_and(|controller| controller.sync_vertical),
            ),
        ] {
            context.states.entry(CommandId(id)).or_default().checked = checked;
        }
    }
    pub(super) fn compare_pair(
        &mut self,
        workspace: &mut Workspace,
        left: usize,
        right: usize,
    ) -> bool {
        if self.busy(workspace)
            || [left, right]
                .iter()
                .any(|&index| workspace.editors.get(index).is_none())
        {
            return false;
        }
        self.split(workspace, left, Orientation::Vertical);
        let document = self
            .documents
            .iter()
            .find(|binding| binding.matches(&workspace.editors[right]))
            .map(DocumentBinding::id)
            .unwrap();
        if let Some(controller) = &mut self.controller {
            if let Some(id) = controller.active_tab(1) {
                let _ = controller.assign_document(id, document);
            }
        }
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        self.compare = true;
        true
    }
    pub(super) fn close_compare(&mut self, workspace: &mut Workspace) {
        self.compare = false;
        self.alignment = None;
        self.collapse(workspace, false);
    }
    pub(super) fn compare_geometry(&self) -> [Option<Rect>; 2] {
        self.bounds
    }
    pub(super) fn compare_snapshots(&self, workspace: &Workspace) -> Option<[ViewSnapshot; 2]> {
        Some([
            workspace
                .editors
                .get(self.primary_index(workspace)?)?
                .snapshot()
                .clone(),
            self.secondary.as_ref()?.snapshot().clone(),
        ])
    }
    pub(super) fn compare_viewport_starts(&self, workspace: &Workspace) -> [usize; 2] {
        let start = |editor: &WorkspaceEditor| match editor {
            WorkspaceEditor::Paged(editor) => editor.viewport_start().0,
            _ => 0,
        };
        [
            self.primary_index(workspace)
                .map_or(0, |index| start(&workspace.editors[index])),
            self.secondary.as_ref().map_or(0, start),
        ]
    }
    pub(super) fn compare_scroll(&self, workspace: &Workspace) -> [f64; 2] {
        [
            self.primary_index(workspace)
                .map_or(0.0, |index| workspace.editors[index].scroll_y),
            self.secondary
                .as_ref()
                .map_or(0.0, |editor| editor.scroll_y),
        ]
    }
    pub(super) fn set_compare_alignment(
        &mut self,
        alignment: Option<bareline_app::views::AlignmentMap>,
    ) {
        self.alignment = alignment;
    }
    pub(super) fn compare_navigate(
        &mut self,
        workspace: &mut Workspace,
        left: bareline_document::TextOffset,
        right: bareline_document::TextOffset,
    ) {
        let first = self.primary_index(workspace);
        for (offset, editor) in [
            (
                left,
                first.and_then(|index| workspace.editors.get_mut(index)),
            ),
            (right, self.secondary.as_mut()),
        ] {
            if let Some(editor) = editor {
                if let WorkspaceEditor::Paged(editor) = editor {
                    if let Err(error) = editor.restore_selection(offset, offset) {
                        editor.error = Some(error);
                    }
                    continue;
                }
                let mut offset = offset.0.min(editor.snapshot().len());
                while !editor
                    .snapshot()
                    .is_boundary(bareline_document::TextOffset(offset))
                {
                    offset -= 1;
                }
                editor.selection.anchor = offset;
                editor.selection.caret = offset;
                editor.scroll_y = editor
                    .snapshot()
                    .line_at(bareline_document::TextOffset(offset))
                    .unwrap_or(0) as f64
                    * 19.2;
            }
        }
    }
    pub(super) fn restore_session(
        &mut self,
        workspace: &mut Workspace,
        app: &mut App,
        manifest: &bareline_file_io::session::SessionManifest,
        tabs: &[(u64, usize)],
    ) {
        let Ok(controller) = ViewController::from_session(manifest) else {
            return;
        };
        self.documents.clear();
        for (id, index) in tabs {
            if let (Some(tab), Some(editor)) = (
                manifest.tabs.iter().find(|tab| tab.id == *id),
                workspace.editors.get(*index),
            ) {
                if !self
                    .documents
                    .iter()
                    .any(|binding| binding.id() == tab.document_id)
                {
                    self.documents
                        .push(DocumentBinding::new(tab.document_id, editor));
                }
            }
        }
        self.next_document = manifest
            .documents
            .iter()
            .map(|document| document.id)
            .max()
            .unwrap_or(0);
        self.controller = Some(controller);
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        if let Some(id) = manifest
            .active_tab
            .and_then(|id| self.tab_index(workspace, id))
        {
            app.active = id;
        }
    }
    pub(super) fn capture_session(
        &self,
        workspace: &Workspace,
        manifest: &mut bareline_file_io::session::SessionManifest,
        tabs: &[(usize, u64)],
    ) {
        let Some(mut controller) = self.controller.clone() else {
            return;
        };
        self.save_view_states(workspace, &mut controller);
        let remap: Vec<_> = self
            .documents
            .iter()
            .filter_map(|binding| {
                let index = self.document_index(workspace, binding.id())?;
                let (_, tab_id) = tabs.iter().find(|(i, _)| *i == index)?;
                let document_id = manifest
                    .tabs
                    .iter()
                    .find(|tab| tab.id == *tab_id)?
                    .document_id;
                Some((binding.id(), document_id))
            })
            .collect();
        let retained: Vec<_> = manifest
            .tabs
            .iter()
            .filter(|tab| !remap.iter().any(|(_, id)| *id == tab.document_id))
            .cloned()
            .collect();
        controller.write_session(manifest);
        manifest.tabs.retain_mut(|tab| {
            if let Some((_, id)) = remap.iter().find(|(old, _)| *old == tab.document_id) {
                tab.document_id = *id;
                true
            } else {
                false
            }
        });
        manifest.mru = manifest
            .mru
            .iter()
            .filter_map(|old| remap.iter().find(|(id, _)| id == old).map(|(_, id)| *id))
            .collect();
        for mut tab in retained {
            if manifest.tabs.iter().any(|existing| existing.id == tab.id) {
                // Failed or deferred restores must retain a tab even if a new live view
                // reused the temporary capture ID. A bounded free ID avoids overflow.
                tab.id = (1..=10_001)
                    .find(|id| !manifest.tabs.iter().any(|existing| existing.id == *id))
                    .unwrap();
            }
            tab.view.split = 0;
            if !manifest.mru.contains(&tab.document_id) {
                manifest.mru.push(tab.document_id);
            }
            manifest.tabs.push(tab);
        }
        manifest.tabs.sort_by_key(|tab| !tab.pinned);
    }
    pub(super) fn pending_edits(&self) -> bool {
        !self.queued.is_empty() || self.secondary.as_ref().is_some_and(WorkspaceEditor::busy)
    }
    pub(super) fn take_acknowledged_inputs(&mut self) -> Vec<Input> {
        self.secondary
            .as_mut()
            .map(|editor| editor.take_acknowledged_inputs())
            .unwrap_or_default()
    }
    pub(super) fn take_ordered_receipts(
        &mut self,
    ) -> Vec<bareline_editor_surface::power::consumer::OrderedReceipt> {
        self.secondary
            .as_mut()
            .map(|editor| editor.take_ordered_receipts())
            .unwrap_or_default()
    }
    pub(super) fn take_acknowledged_commands(
        &mut self,
    ) -> Vec<(String, std::collections::BTreeMap<String, String>)> {
        self.secondary
            .as_mut()
            .map(|editor| editor.take_acknowledged_commands())
            .unwrap_or_default()
    }
    fn open(&self) -> bool {
        self.secondary.is_some() && self.controller.as_ref().is_some_and(|c| c.split)
    }
    pub(super) fn pane(&self) -> u32 {
        self.controller
            .as_ref()
            .map_or(0, ViewController::active_pane)
    }
    fn index_of(workspace: &Workspace, document: &ViewSnapshot) -> Option<usize> {
        workspace
            .editors
            .iter()
            .position(|e| e.snapshot().same_document(document))
    }
    fn primary_index(&self, workspace: &Workspace) -> Option<usize> {
        if let Some(index) = self.loaded_tabs[0].and_then(|id| self.tab_index(workspace, id)) {
            return Some(index);
        }
        self.primary
            .as_ref()
            .and_then(|s| Self::index_of(workspace, s))
    }
    fn secondary_index(&self, workspace: &Workspace) -> Option<usize> {
        self.loaded_tabs[1].and_then(|id| self.tab_index(workspace, id))
    }
    fn busy(&self, workspace: &Workspace) -> bool {
        !self.queued.is_empty()
            || self.secondary.as_ref().is_some_and(WorkspaceEditor::busy)
            || self
                .primary_index(workspace)
                .is_some_and(|i| workspace.editors[i].busy())
    }
    pub(super) fn pump(&mut self, workspace: &mut Workspace) -> bool {
        self.sync_documents(workspace);
        let mut changed = self.secondary.as_mut().is_some_and(WorkspaceEditor::pump);
        if let Some(index) = self.secondary_index(workspace)
            && let Some(peer) = &mut self.secondary
        {
            match (&mut workspace.editors[index], &mut *peer) {
                (WorkspaceEditor::Resident(primary), WorkspaceEditor::Resident(secondary)) => {
                    if secondary.snapshot().revision.0 > primary.snapshot().revision.0 {
                        changed |= primary.refresh_peer(secondary.snapshot());
                    } else if primary.snapshot().revision.0 > secondary.snapshot().revision.0 {
                        changed |= secondary.refresh_peer(primary.snapshot());
                    }
                    secondary.sync_saved_from(primary);
                    secondary.sync_fold_metadata_from(primary);
                }
                (WorkspaceEditor::Paged(primary), WorkspaceEditor::Paged(secondary)) => {
                    changed |= primary.refresh_peer();
                    changed |= secondary.refresh_peer();
                }
                _ => {}
            }
            peer.theme = workspace.editors[index].theme;
            let _ = peer.set_font_family(workspace.editors[index].font_family());
        }
        for pane in 0..2 {
            let index = self.primary_index(workspace);
            let editor = if pane == 1 {
                self.secondary.as_mut()
            } else {
                index.and_then(|index| workspace.editors.get_mut(index))
            };
            if let Some(editor) = editor {
                if !editor.busy() {
                    if let Some(state) = self.pending_view_scroll[pane].take() {
                        editor.set_logical_scroll(state.scroll_line, 0.0, state.scroll_x as f64);
                        editor.scroll_y = f64::from_bits(state.scroll_y_bits);
                        changed = true;
                    }
                    if let Some(state) = self.pending_restore[pane].take() {
                        if editor.paged() {
                            self.pending_view_scroll[pane] = Some(state.clone());
                        }
                        restore_workspace_view(editor, &state);
                        changed = true;
                    }
                }
            } else {
                self.pending_restore[pane] = None;
            }
        }
        if self.open() && self.secondary_index(workspace).is_none() {
            self.collapse(workspace, false);
            changed = true;
        }
        let primary_busy = self
            .primary_index(workspace)
            .is_some_and(|i| workspace.editors[i].busy());
        if !primary_busy
            && !self.secondary.as_ref().is_some_and(WorkspaceEditor::busy)
            && let Some(queued) = self.queued.pop_front()
        {
            if queued.pane == 0 {
                if let Some(index) = self.primary_index(workspace) {
                    if queued.document.matches(&workspace.editors[index]) {
                        workspace.editors[index].enqueue(queued.input);
                    } else {
                        workspace.message =
                            Some("The view changed before queued input could be applied.".into());
                    }
                }
            } else if let Some(editor) = &mut self.secondary {
                if queued.document.matches(editor) {
                    editor.enqueue(queued.input);
                } else {
                    workspace.message =
                        Some("The view changed before queued input could be applied.".into());
                }
            }
            changed = true;
        }
        changed
    }
    fn input(&mut self, workspace: &mut Workspace, pane: u32, input: Input) {
        self.pump(workspace);
        let document = if pane == 1 {
            self.secondary
                .as_ref()
                .map(|editor| DocumentBinding::new(0, editor))
        } else {
            self.primary_index(workspace)
                .map(|i| DocumentBinding::new(0, &workspace.editors[i]))
        };
        let Some(document) = document else {
            return;
        };
        if self.queued.len() >= 256 {
            workspace.message =
                Some("Split-view input queue is full; wait for the pending edit.".into());
            return;
        }
        self.queued.push_back(QueuedInput {
            pane,
            document,
            input,
        });
        self.pump(workspace);
    }
    fn split(&mut self, workspace: &mut Workspace, index: usize, orientation: Orientation) {
        self.sync_documents(workspace);
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before changing views.".into());
            return;
        }
        if workspace.editors.get(index).is_none() {
            workspace.message = Some("The document is no longer available.".into());
            return;
        }
        self.save_current(workspace);
        let document = self
            .documents
            .iter()
            .find(|binding| binding.matches(&workspace.editors[index]))
            .map(DocumentBinding::id)
            .unwrap();
        let controller = self.controller.as_mut().unwrap();
        let id = controller
            .tabs()
            .iter()
            .find(|tab| tab.document_id == document && tab.view.split == controller.active_pane())
            .or_else(|| {
                controller
                    .tabs()
                    .iter()
                    .find(|tab| tab.document_id == document)
            })
            .map(|tab| tab.id)
            .unwrap();
        let _ = controller.activate(id);
        if !controller.split {
            let _ = controller.clone_to_other(id);
        }
        controller.orientation = orientation;
        controller.split = true;
        self.install_views(workspace);
    }
    fn collapse(&mut self, workspace: &mut Workspace, keep_secondary: bool) {
        for editor in &mut workspace.editors { let _ = editor.set_view_spacers(&[]); }
        self.applied_spacers = [None, None];
        self.save_current(workspace);
        if let Some(controller) = &mut self.controller {
            if keep_secondary {
                if let Some(id) = controller.active_tab(1) {
                    let _ = controller.activate(id);
                }
            }
            controller.collapse();
        }
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        self.bounds = [None, None];
        self.splitter = None;
        self.dragging = false;
    }
    fn clone_active(&mut self, workspace: &mut Workspace, app: &mut App) {
        self.sync_documents(workspace);
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before cloning a view.".into());
            return;
        }
        if workspace.editors.get(app.active).is_none() {
            workspace.message = Some("The document is no longer available.".into());
            return;
        }
        self.save_current(workspace);
        if let Some(controller) = &mut self.controller {
            if let Some(id) = controller.active_tab(controller.active_pane()) {
                let _ = controller.clone_to_other(id);
            }
        }
        self.install_views(workspace);
        if let Some(id) = self
            .controller
            .as_ref()
            .and_then(|controller| controller.active_tab(controller.active_pane()))
            .and_then(|id| self.tab_index(workspace, id))
        {
            app.active = id;
        }
    }
    fn move_active(&mut self, workspace: &mut Workspace, app: &mut App) {
        self.sync_documents(workspace);
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before moving a view.".into());
            return;
        }
        self.save_current(workspace);
        if let Some(controller) = &mut self.controller {
            if let Some(id) = controller.active_tab(controller.active_pane()) {
                let _ = controller.move_to_other(id);
            }
            if controller.active_tab(0).is_none() || controller.active_tab(1).is_none() {
                controller.collapse();
            }
        }
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        if let Some(index) = self
            .controller
            .as_ref()
            .and_then(|controller| controller.active_tab(controller.active_pane()))
            .and_then(|id| self.tab_index(workspace, id))
        {
            app.active = index;
        }
    }
    fn activate(&mut self, workspace: &Workspace, app: &mut App, pane: u32) {
        if let Some(controller) = &mut self.controller {
            if let Some(id) = controller.active_tab(pane) {
                let _ = controller.activate(id);
            }
        }
        if let Some(index) = if pane == 0 {
            self.primary_index(workspace)
        } else {
            self.secondary_index(workspace)
        } {
            app.active = index;
        }
    }
    fn close_tab(&mut self, workspace: &mut Workspace, app: &mut App, id: u64) {
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before closing a view.".into());
            return;
        }
        let Some(index) = self.tab_index(workspace, id) else {
            return;
        };
        self.save_current(workspace);
        let controller = self.controller.as_ref().unwrap();
        let document = controller.tab(id).unwrap().document_id;
        if controller
            .tabs()
            .iter()
            .filter(|tab| tab.document_id == document)
            .count()
            == 1
        {
            app.active = index;
            self.pending_close = Some(index);
            return;
        }
        let _ =
            self.controller
                .as_mut()
                .unwrap()
                .close(id, workspace.editors[index].dirty(), false);
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        if let Some(index) = self
            .controller
            .as_ref()
            .and_then(|controller| controller.active_tab(controller.active_pane()))
            .and_then(|id| self.tab_index(workspace, id))
        {
            app.active = index;
        }
    }
    fn sync_scroll(&mut self, workspace: &mut Workspace, pane: u32) {
        let position = if pane == 1 {
            self.secondary
                .as_ref()
                .map(|editor| editor.logical_scroll())
        } else {
            self.primary_index(workspace)
                .map(|index| workspace.editors[index].logical_scroll())
        };
        let Some((line, fraction, x)) = position else {
            return;
        };
        let Some(controller) = &mut self.controller else {
            return;
        };
        let sync_vertical = controller.sync_vertical;
        let sync_horizontal = controller.sync_horizontal;
        if let Ok(Some(update)) = controller.begin_scroll(
            pane,
            ScrollPosition { line, fraction, x },
            self.alignment.as_ref(),
        ) {
            if update.pane == 1 {
                if let Some(peer) = &mut self.secondary {
                    let old = peer.logical_scroll();
                    peer.set_logical_scroll(
                        if sync_vertical {
                            update.position.line
                        } else {
                            old.0
                        },
                        if sync_vertical {
                            update.position.fraction
                        } else {
                            old.1
                        },
                        if sync_horizontal {
                            update.position.x
                        } else {
                            old.2
                        },
                    );
                }
            } else if let Some(index) = self.primary_index(workspace) {
                let old = workspace.editors[index].logical_scroll();
                workspace.editors[index].set_logical_scroll(
                    if sync_vertical {
                        update.position.line
                    } else {
                        old.0
                    },
                    if sync_vertical {
                        update.position.fraction
                    } else {
                        old.1
                    },
                    if sync_horizontal {
                        update.position.x
                    } else {
                        old.2
                    },
                );
            }
        }
    }
    pub(super) fn draw(
        &mut self,
        workspace: &mut Workspace,
        app: &mut App,
        renderer: &mut impl TextBackend,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        for mut peer in self.retired.drain(..) {
            peer.release_layouts(renderer);
        }
        self.pump(workspace);
        let desired = self.controller.as_ref().and_then(|controller| {
            controller
                .pane_tabs(controller.active_pane())
                .find(|tab| self.document_index(workspace, tab.document_id) == Some(app.active))
                .or_else(|| {
                    controller.tabs().iter().find(|tab| {
                        self.document_index(workspace, tab.document_id) == Some(app.active)
                    })
                })
                .map(|tab| tab.id)
        });
        if let Some(id) = desired {
            if self.loaded_tabs[self.pane() as usize] != Some(id) {
                self.select_tab(workspace, app, id);
            }
        }
        self.install_views(workspace);
        self.tab_hits.clear();
        self.tab_nav.clear();
        self.tab_strips = [None, None];
        let vertical = self
            .controller
            .as_ref()
            .is_some_and(|controller| controller.vertical_tabs);
        if !self.open() {
            let inset = if vertical {
                176.0f32.min(width * 0.4)
            } else {
                0.0
            };
            let mut local = Vec::new();
            let caret = workspace.draw(
                app.active,
                renderer,
                (width - inset).max(0.0),
                height,
                &mut local,
            )?;
            ops.extend(local.into_iter().map(|op| translate(op, inset, 0.0)));
            self.bounds = [
                Some(rect(inset, 0.0, (width - inset).max(0.0), height - 24.0)),
                None,
            ];
            self.draw_tab_strip(
                workspace,
                0,
                if vertical {
                    rect(0.0, 0.0, inset, height - 24.0)
                } else {
                    rect(0.0, 0.0, width, TAB_HEIGHT)
                },
                vertical,
                ops,
            );
            self.draw_mru(workspace, width, height, ops);
            return Ok(caret.map(|caret| rect(caret.x + inset, caret.y, caret.width, caret.height)));
        }
        let pane = self.pane();
        let Some(first) = self.primary_index(workspace) else {
            self.collapse(workspace, true);
            return workspace.draw(app.active, renderer, width, height, ops);
        };
        let find_height = if workspace.find.open {
            workspace.find.height()
        } else {
            0.0
        };
        let panel_height = workspace.search_panel.height();
        let compare_height = if self.compare { 44.0 } else { 0.0 };
        let geometry = self.controller.as_ref().unwrap().geometry(rect(
            0.0,
            TAB_HEIGHT + find_height + compare_height,
            width,
            (height - TAB_HEIGHT - find_height - compare_height - 24.0 - panel_height).max(0.0),
        ));
        self.bounds = geometry.panes;
        self.splitter = geometry.splitter;
        let strips = self.bounds.map(|bounds| {
            bounds.map(|bounds| {
                if vertical {
                    rect(
                        bounds.x,
                        bounds.y,
                        176.0f32.min(bounds.width * 0.4),
                        bounds.height,
                    )
                } else {
                    rect(bounds.x, bounds.y, bounds.width, TAB_HEIGHT)
                }
            })
        });
        if vertical {
            for bounds in self.bounds.iter_mut().flatten() {
                let inset = 176.0f32.min(bounds.width * 0.4);
                bounds.x += inset;
                bounds.width -= inset;
            }
        }
        let titles = workspace.titles();
        let second = self.secondary_index(workspace).unwrap_or(first);
        let mut active_caret = None;
        let mut status = Vec::new();
        self.draw_tab_strip(
            workspace,
            pane,
            rect(0.0, 0.0, width, TAB_HEIGHT),
            false,
            ops,
        );
        for side in 0..2 {
            let Some(bounds) = self.bounds[side] else {
                continue;
            };
            let editor = if side == 0 {
                &mut workspace.editors[first]
            } else {
                self.secondary.as_mut().unwrap()
            };
            let spacers = if editor.paged() {
                Vec::new()
            } else {
                self.alignment
                    .as_ref()
                    .map(|alignment| alignment.spacers(side))
                    .unwrap_or_default()
            };
            if self.applied_spacers[side].as_ref() != Some(&spacers) {
                if let Err(error) = editor.set_view_spacers(&spacers) { editor.error = Some(error); }
                self.applied_spacers[side] = Some(spacers);
            }
            // EditorSurface already reserves TAB_HEIGHT for this pane's header.
            editor.top_inset = 0.0;
            editor.bottom_inset = 0.0;
            let mut local = Vec::new();
            let local_height = bounds.height + 24.0;
            let language = editor.language.label();
            let caret = editor.draw_styled(
                renderer,
                bounds.width,
                local_height,
                &mut local,
                SharedSyntaxView {
                    result: None,
                    language,
                    unavailable: false,
                },
            )?;
            if side as u32 == pane {
                status = local
                    .iter()
                    .filter_map(|op| {
                        if let DrawOp::Text { origin, text, .. } = op {
                            (origin.y == local_height - 20.0).then(|| text.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                active_caret =
                    caret.map(|r| rect(r.x + bounds.x, r.y + bounds.y, r.width, r.height));
            } else if let Some(caret) = caret {
                local.retain(|op| !matches!(op, DrawOp::Fill(r, _) if *r == caret));
            }
            ops.push(DrawOp::PushClip(bounds));
            ops.extend(
                local
                    .into_iter()
                    .map(|op| translate(op, bounds.x, bounds.y)),
            );
            ops.push(DrawOp::Fill(
                rect(bounds.x, bounds.y, bounds.width, TAB_HEIGHT),
                CHROME,
            ));
            text(
                ops,
                bounds.x + 12.0,
                bounds.y + 8.0,
                titles
                    .get(if side == 0 { first } else { second })
                    .map_or("Document", String::as_str),
                13.0,
                if side as u32 == pane { TEXT } else { MUTED },
            );
            if side as u32 == pane {
                ops.push(DrawOp::Fill(
                    rect(bounds.x, bounds.y + TAB_HEIGHT - 2.0, bounds.width, 2.0),
                    ACCENT,
                ));
            }
            ops.push(DrawOp::PopClip);
        }
        if let Some(splitter) = self.splitter {
            ops.push(DrawOp::Fill(splitter, BORDER));
        }
        for (pane, bounds) in strips.into_iter().enumerate() {
            if let Some(bounds) = bounds {
                self.draw_tab_strip(workspace, pane as u32, bounds, vertical, ops);
            }
        }
        ops.push(DrawOp::Fill(rect(0.0, height - 24.0, width, 24.0), CHROME));
        for (index, label) in status.into_iter().take(6).enumerate() {
            text(
                ops,
                12.0 + width * index as f32 / 6.0,
                height - 20.0,
                label,
                13.0,
                MUTED,
            );
        }
        if let Some(caret) =
            workspace
                .find
                .draw_with_theme(renderer, width, workspace.theme, ops)?
        {
            active_caret = Some(caret);
        }
        let labels: Vec<_> = workspace
            .editors
            .iter()
            .zip(titles)
            .map(|(e, title)| (e.snapshot().clone(), title))
            .collect();
        if let Some(caret) = workspace
            .search_panel
            .draw(renderer, width, height, &labels, ops)?
        {
            active_caret = Some(caret);
        }
        self.draw_mru(workspace, width, height, ops);
        Ok(active_caret)
    }
}

fn translate(op: DrawOp, x: f32, y: f32) -> DrawOp {
    let r = |r: Rect| rect(r.x + x, r.y + y, r.width, r.height);
    let p = |p: Point| Point {
        x: p.x + x,
        y: p.y + y,
    };
    match op {
        DrawOp::Fill(a, c) => DrawOp::Fill(r(a), c),
        DrawOp::Stroke(a, c, w) => DrawOp::Stroke(r(a), c, w),
        DrawOp::FillRounded(a, c, v) => DrawOp::FillRounded(r(a), c, v),
        DrawOp::StrokeRounded(a, c, v, w) => DrawOp::StrokeRounded(r(a), c, v, w),
        DrawOp::Text {
            origin,
            text,
            size,
            color,
        } => DrawOp::Text {
            origin: p(origin),
            text,
            size,
            color,
        },
        DrawOp::Layout {
            origin,
            layout,
            color,
        } => DrawOp::Layout {
            origin: p(origin),
            layout,
            color,
        },
        DrawOp::PushClip(a) => DrawOp::PushClip(r(a)),
        DrawOp::PopClip => DrawOp::PopClip,
        DrawOp::Image {
            image,
            destination,
            opacity,
        } => DrawOp::Image {
            image,
            destination: r(destination),
            opacity,
        },
        DrawOp::PushLayer { bounds, opacity } => DrawOp::PushLayer {
            bounds: r(bounds),
            opacity,
        },
        DrawOp::PopLayer => DrawOp::PopLayer,
        DrawOp::Line {
            from,
            to,
            color,
            width,
        } => DrawOp::Line {
            from: p(from),
            to: p(to),
            color,
            width,
        },
    }
}

fn workspace_view_state(editor: &WorkspaceEditor) -> ViewState {
    let mut state = view_state(editor);
    if let WorkspaceEditor::Paged(paged) = editor {
        state.anchor = state.anchor.saturating_add(paged.viewport_start().0 as u64);
        state.caret = state.caret.saturating_add(paged.viewport_start().0 as u64);
    }
    state
}
fn restore_workspace_view(editor: &mut WorkspaceEditor, state: &ViewState) {
    match editor {
        WorkspaceEditor::Resident(editor) => restore_view(editor, state),
        WorkspaceEditor::Paged(editor) => {
            if let Err(error) = editor.restore_selection(
                bareline_document::TextOffset(usize::try_from(state.anchor).unwrap_or(usize::MAX)),
                bareline_document::TextOffset(usize::try_from(state.caret).unwrap_or(usize::MAX)),
            ) {
                editor.error = Some(error);
            }
            editor.surface.scroll_y = f64::from_bits(state.scroll_y_bits);
            editor
                .surface
                .set_logical_scroll(state.scroll_line, 0.0, state.scroll_x as f64);
        }
    }
}
fn view_state(editor: &SharedEditorView) -> ViewState {
    let (line, _, x) = editor.logical_scroll();
    ViewState {
        anchor: editor.selection.anchor as u64,
        caret: editor.selection.caret as u64,
        scroll_line: line,
        scroll_x: x.clamp(0.0, u32::MAX as f64) as u32,
        scroll_y_bits: editor.scroll_y.max(0.0).to_bits(),
        folds: editor.persisted_folds(),
        ..Default::default()
    }
}
fn restore_view(editor: &mut SharedEditorView, state: &ViewState) {
    let bound = |offset: u64| {
        let mut offset = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(editor.snapshot().len());
        while !editor
            .snapshot()
            .is_boundary(bareline_document::TextOffset(offset))
        {
            offset -= 1;
        }
        offset
    };
    let anchor = bound(state.anchor);
    let caret = bound(state.caret);
    editor.selection.anchor = anchor;
    editor.selection.caret = caret;
    editor.set_logical_scroll(state.scroll_line, 0.0, state.scroll_x as f64);
    editor.scroll_y = f64::from_bits(state.scroll_y_bits);
    editor.restore_folds(&state.folds);
}

fn mru_visible_rows(bounds: Rect) -> usize {
    ((bounds.height / TAB_HEIGHT).floor() as usize).clamp(1, 12)
}
const ACCESS_TAB_BASE: u64 = 0x1000_0000_0000_0000;
const ACCESS_NAV_BASE: u64 = 0x2000_0000_0000_0000;
const ACCESS_MRU_BASE: u64 = 0x3000_0000_0000_0000;
fn access_tab_id(tab: u64) -> Option<u64> {
    tab.checked_mul(2)
        .and_then(|id| id.checked_add(ACCESS_TAB_BASE))
        .filter(|id| *id < ACCESS_NAV_BASE)
}
impl Shell {
    pub(super) fn views_accessibility_nodes(
        &self,
    ) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        let Some(workspace) = &self.workspace else {
            return Vec::new();
        };
        let Some(controller) = &self.views.controller else {
            return Vec::new();
        };
        let offset = self.editor_bounds();
        let bounds = |rect: Rect| {
            [
                (rect.x + offset.x) as f64,
                (rect.y + offset.y) as f64,
                rect.width as f64,
                rect.height as f64,
            ]
        };
        let titles = workspace.titles();
        let mut nodes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for hit in &self.views.tab_hits {
            if !seen.insert(hit.id) {
                continue;
            }
            let Some(id) = access_tab_id(hit.id) else {
                continue;
            };
            let Some(tab) = controller.tab(hit.id) else {
                continue;
            };
            let Some(index) = self.views.tab_index(workspace, hit.id) else {
                continue;
            };
            let title = titles.get(index).cloned().unwrap_or_default();
            let name = format!(
                "{}{}, pane {}{}",
                title,
                if tab.pinned { ", pinned" } else { "" },
                hit.pane + 1,
                if workspace.editors[index].dirty() {
                    ", modified"
                } else {
                    ""
                }
            );
            nodes.push(AccessibilityNode {
                id,
                parent: 1,
                role: AccessibilityRole::Tab,
                name,
                value: controller
                    .tab_colors
                    .get(&hit.id)
                    .map(|color| format!("#{color:06x}")),
                bounds: bounds(hit.bounds),
                disabled: self.views.busy(workspace),
                selected: controller.active_tab(hit.pane) == Some(hit.id),
                expanded: None,
                focusable: true,
                invokable: true,
            });
            nodes.push(AccessibilityNode {
                id: id + 1,
                parent: id,
                role: AccessibilityRole::Button,
                name: format!("Close {title}"),
                value: None,
                bounds: bounds(hit.close),
                disabled: self.views.busy(workspace),
                selected: false,
                expanded: None,
                focusable: true,
                invokable: true,
            });
        }
        for (pane, forward, rect) in &self.views.tab_nav {
            let id = ACCESS_NAV_BASE + *pane as u64 * 2 + u64::from(*forward);
            if nodes.iter().any(|node| node.id == id) {
                continue;
            }
            nodes.push(AccessibilityNode {
                id,
                parent: 1,
                role: AccessibilityRole::Button,
                name: format!(
                    "{} tabs in pane {}",
                    if *forward { "Next" } else { "Previous" },
                    pane + 1
                ),
                value: None,
                bounds: bounds(*rect),
                disabled: false,
                selected: false,
                expanded: None,
                focusable: true,
                invokable: true,
            });
        }
        if let Some(popup) = &self.views.mru_popup {
            nodes.push(AccessibilityNode {
                id: ACCESS_MRU_BASE,
                parent: 1,
                role: AccessibilityRole::List,
                name: "Recent documents".into(),
                value: None,
                bounds: bounds(popup.bounds),
                disabled: false,
                selected: false,
                expanded: Some(true),
                focusable: false,
                invokable: false,
            });
            let start = popup
                .selected
                .saturating_sub(mru_visible_rows(popup.bounds).saturating_sub(1));
            for (row, tab) in popup
                .ids
                .iter()
                .skip(start)
                .take(mru_visible_rows(popup.bounds))
                .enumerate()
            {
                let Some(id) = access_tab_id(*tab).and_then(|id| id.checked_add(ACCESS_MRU_BASE))
                else {
                    continue;
                };
                let Some(index) = self.views.tab_index(workspace, *tab) else {
                    continue;
                };
                let row_bounds = rect(
                    popup.bounds.x,
                    popup.bounds.y + row as f32 * TAB_HEIGHT,
                    popup.bounds.width,
                    TAB_HEIGHT.min((popup.bounds.height - row as f32 * TAB_HEIGHT).max(0.0)),
                );
                if row_bounds.height == 0.0 {
                    continue;
                }
                nodes.push(AccessibilityNode {
                    id,
                    parent: ACCESS_MRU_BASE,
                    role: AccessibilityRole::ListItem,
                    name: titles.get(index).cloned().unwrap_or_default(),
                    value: None,
                    bounds: bounds(row_bounds),
                    disabled: self.views.busy(workspace),
                    selected: start + row == popup.selected,
                    expanded: None,
                    focusable: true,
                    invokable: true,
                });
            }
        }
        nodes
    }
    pub(super) fn views_accessibility_focus(&self) -> Option<u64> {
        if let Some(popup) = &self.views.mru_popup {
            return access_tab_id(*popup.ids.get(popup.selected)?)
                .and_then(|id| id.checked_add(ACCESS_MRU_BASE));
        }
        self.views.accessibility_focus.filter(|id| {
            self.views_accessibility_nodes()
                .iter()
                .any(|node| node.id == *id)
        })
    }
    pub(super) fn views_accessibility(
        &mut self,
        el: &winit::event_loop::ActiveEventLoop,
        action: &bareline_platform::accessibility::AccessibilityAction,
    ) -> bool {
        use bareline_platform::accessibility::AccessibilityAction;
        let (id, invoke) = match action {
            AccessibilityAction::Focus(id) => (*id, false),
            AccessibilityAction::Invoke(id) => (*id, true),
            _ => return false,
        };
        if let Some(popup) = &mut self.views.mru_popup {
            if let Some(position) = popup.ids.iter().position(|tab| {
                access_tab_id(*tab).and_then(|id| id.checked_add(ACCESS_MRU_BASE)) == Some(id)
            }) {
                popup.selected = position;
                if invoke {
                    let tab = popup.ids[position];
                    self.views.mru_popup = None;
                    if let Some(workspace) = &mut self.workspace {
                        self.views.select_tab(workspace, &mut self.app, tab);
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return true;
            }
        }
        if let Some((pane, forward, _)) = self
            .views
            .tab_nav
            .iter()
            .find(|(pane, forward, _)| {
                ACCESS_NAV_BASE + *pane as u64 * 2 + u64::from(*forward) == id
            })
            .copied()
        {
            self.views.accessibility_focus = if invoke { None } else { Some(id) };
            if invoke {
                let offset = &mut self.views.tab_offset[pane as usize];
                *offset = if forward {
                    offset.saturating_add(1)
                } else {
                    offset.saturating_sub(1)
                };
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            return true;
        }
        let Some(hit) = self
            .views
            .tab_hits
            .iter()
            .find(|hit| access_tab_id(hit.id).is_some_and(|base| id == base || id == base + 1))
            .copied()
        else {
            return false;
        };
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        if self.views.busy(workspace) {
            return true;
        }
        self.views.accessibility_focus = if invoke { None } else { Some(id) };
        if invoke && access_tab_id(hit.id).is_some_and(|base| id == base + 1) {
            self.views.close_tab(workspace, &mut self.app, hit.id);
        } else {
            self.views.select_tab(workspace, &mut self.app, hit.id);
        }
        if self.views.pending_close.take().is_some() {
            self.dispatch(el, Action::Close);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

impl Shell {
    fn tabs_dispatch(&mut self, id: &str) -> bool {
        if !id.starts_with("view.tabs.") {
            return false;
        }
        let Some(workspace) = &mut self.workspace else {
            return true;
        };
        self.views.sync_documents(workspace);
        self.views.save_current(workspace);
        let Some(controller) = &mut self.views.controller else {
            return true;
        };
        let active = controller.active_tab(controller.active_pane());
        match id {
            "view.tabs.vertical" => controller.vertical_tabs = !controller.vertical_tabs,
            "view.tabs.pin" => {
                if let Some(id) = active {
                    let pinned = controller.tab(id).unwrap().pinned;
                    let _ = controller.pin(id, !pinned);
                }
            }
            "view.tabs.color" => {
                if let Some(id) = active {
                    let colors = [0x36c9c6, 0xd19a66, 0xc678dd, 0x61afef];
                    let current = controller.tab_colors.get(&id).copied();
                    let next = current
                        .and_then(|color| colors.iter().position(|value| *value == color))
                        .map_or(Some(colors[0]), |index| colors.get(index + 1).copied());
                    let _ = controller.color(id, next);
                }
            }
            "view.tabs.sort_name" | "view.tabs.sort_descending" | "view.tabs.sort_path" => {
                let titles = workspace.titles();
                let labels: Vec<_> = self
                    .views
                    .documents
                    .iter()
                    .filter_map(|binding| {
                        workspace
                            .editors
                            .iter()
                            .position(|editor| binding.matches(editor))
                            .map(|index| {
                                (
                                    binding.id(),
                                    if id == "view.tabs.sort_path" {
                                        workspace
                                            .path(index)
                                            .map(|path| path.to_string_lossy().into_owned())
                                            .unwrap_or_else(|| titles[index].clone())
                                    } else {
                                        titles[index].clone()
                                    },
                                )
                            })
                    })
                    .collect();
                controller.sort_by_label(&labels, id == "view.tabs.sort_descending");
                if id == "view.tabs.sort_path" {
                    controller.tab_sort = "path".into();
                }
            }
            "view.tabs.move_left" | "view.tabs.move_right" => {
                if let Some(id_active) = active {
                    let _ = controller.keyboard_reorder(id_active, id == "view.tabs.move_left");
                }
            }
            "view.tabs.previous" | "view.tabs.next" => {
                let tabs: Vec<_> = controller
                    .pane_tabs(controller.active_pane())
                    .map(|tab| tab.id)
                    .collect();
                if let Some(index) = active.and_then(|id| tabs.iter().position(|tab| *tab == id)) {
                    let next = if id == "view.tabs.previous" {
                        (index + tabs.len() - 1) % tabs.len()
                    } else {
                        (index + 1) % tabs.len()
                    };
                    let id = tabs[next];
                    self.views.select_tab(workspace, &mut self.app, id);
                }
            }
            "view.tabs.mru" => {
                if let Some(popup) = &mut self.views.mru_popup {
                    if !popup.ids.is_empty() {
                        popup.selected = (popup.selected + 1) % popup.ids.len();
                    }
                } else {
                    let mut seen = std::collections::HashSet::new();
                    let ids = controller
                        .mru()
                        .filter(|id| {
                            controller
                                .tab(*id)
                                .is_some_and(|tab| seen.insert(tab.document_id))
                        })
                        .collect::<Vec<_>>();
                    let selected = usize::from(ids.len() > 1);
                    self.views.mru_popup = Some(MruPopup {
                        ids,
                        selected,
                        bounds: Rect::default(),
                    });
                }
            }
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    fn tabs_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if self.palette.open {
            return false;
        }
        let origin = self.editor_bounds();
        let point = Point {
            x: self.pointer.x - origin.x,
            y: self.pointer.y - origin.y,
        };
        let mut handled = false;
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        if self.views.mru_popup.is_some() {
            let mut accept = false;
            let mut cancel = false;
            let popup = self.views.mru_popup.as_mut().unwrap();
            match event {
                WindowEvent::ModifiersChanged(modifiers) if !modifiers.state().control_key() => {
                    accept = true
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    match event.logical_key {
                        Key::Named(NamedKey::Escape) => cancel = true,
                        Key::Named(NamedKey::Enter) => accept = true,
                        Key::Named(NamedKey::ArrowUp) => {
                            popup.selected = popup.selected.saturating_sub(1);
                        }
                        Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::Tab) => {
                            if !popup.ids.is_empty() {
                                popup.selected = (popup.selected + 1) % popup.ids.len();
                            }
                        }
                        _ => {}
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    if popup.bounds.contains(point) {
                        let start = popup
                            .selected
                            .saturating_sub(mru_visible_rows(popup.bounds).saturating_sub(1));
                        popup.selected = (start
                            + ((point.y - popup.bounds.y) / TAB_HEIGHT) as usize)
                            .min(popup.ids.len().saturating_sub(1));
                        accept = true;
                    } else {
                        cancel = true;
                    }
                }
                _ => return false,
            }
            if accept || cancel {
                let popup = self.views.mru_popup.take().unwrap();
                if accept {
                    if let Some(id) = popup.ids.get(popup.selected) {
                        self.views.select_tab(workspace, &mut self.app, *id);
                    }
                }
            }
            handled = true;
        } else {
            match event {
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    if let Some((pane, next, _)) = self
                        .views
                        .tab_nav
                        .iter()
                        .find(|(_, _, bounds)| bounds.contains(point))
                        .copied()
                    {
                        let offset = &mut self.views.tab_offset[pane as usize];
                        *offset = if next {
                            offset.saturating_add(1)
                        } else {
                            offset.saturating_sub(1)
                        };
                        handled = true;
                    } else if let Some(hit) = self
                        .views
                        .tab_hits
                        .iter()
                        .find(|hit| hit.bounds.contains(point))
                        .copied()
                    {
                        if hit.close.contains(point) {
                            self.views.close_tab(workspace, &mut self.app, hit.id);
                        } else {
                            self.views.select_tab(workspace, &mut self.app, hit.id);
                            self.views.tab_drag = Some(TabDrag {
                                id: hit.id,
                                start: point,
                                moved: false,
                            });
                        }
                        handled = true;
                    }
                }
                WindowEvent::CursorMoved { position, .. } if self.views.tab_drag.is_some() => {
                    let scale = self
                        .window
                        .as_ref()
                        .map_or(1.0, |window| window.scale_factor());
                    let p = position.to_logical::<f32>(scale);
                    self.pointer = Point { x: p.x, y: p.y };
                    let point = Point {
                        x: p.x - origin.x,
                        y: p.y - origin.y,
                    };
                    let drag = self.views.tab_drag.as_mut().unwrap();
                    drag.moved |=
                        (point.x - drag.start.x).abs() + (point.y - drag.start.y).abs() > 5.0;
                    handled = true;
                }
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                } if self.views.tab_drag.is_some() => {
                    let drag = self.views.tab_drag.take().unwrap();
                    if drag.moved && !self.views.busy(workspace) {
                        let target = self
                            .views
                            .tab_hits
                            .iter()
                            .find(|hit| hit.bounds.contains(point))
                            .map(|hit| (hit.pane, Some(hit.id)))
                            .or_else(|| {
                                self.views
                                    .bounds
                                    .iter()
                                    .enumerate()
                                    .find(|(_, bounds)| {
                                        bounds.is_some_and(|bounds| bounds.contains(point))
                                    })
                                    .map(|(pane, _)| (pane as u32, None))
                            });
                        if let Some((pane, before)) = target {
                            self.views.save_current(workspace);
                            if let Some(controller) = &mut self.views.controller {
                                if let Err(error) = controller.move_to_pane(drag.id, pane, before) {
                                    workspace.message =
                                        Some(format!("Tab cannot be moved: {error:?}"));
                                }
                            }
                            self.views.loaded_tabs = [None, None];
                            self.views.install_views(workspace);
                            self.views.select_tab(workspace, &mut self.app, drag.id);
                        }
                    }
                    handled = true;
                }
                WindowEvent::MouseWheel { delta, .. }
                    if self
                        .views
                        .tab_strips
                        .iter()
                        .any(|bounds| bounds.is_some_and(|bounds| bounds.contains(point))) =>
                {
                    let pane = self
                        .views
                        .tab_strips
                        .iter()
                        .position(|bounds| bounds.is_some_and(|bounds| bounds.contains(point)))
                        .unwrap();
                    let down = match delta {
                        MouseScrollDelta::LineDelta(_, y) => *y < 0.0,
                        MouseScrollDelta::PixelDelta(point) => point.y < 0.0,
                    };
                    let offset = &mut self.views.tab_offset[pane];
                    *offset = if down {
                        offset.saturating_add(1)
                    } else {
                        offset.saturating_sub(1)
                    };
                    handled = true;
                }
                WindowEvent::Focused(false) => {
                    self.views.tab_drag = None;
                }
                _ => {}
            }
        }
        let close = self.views.pending_close.take();
        if close.is_some() {
            self.dispatch(el, Action::Close);
        }
        if handled {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        handled
    }
    pub(super) fn views_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if self.tabs_dispatch(id) {
            return true;
        }
        if !matches!(
            id,
            "view.split_vertical"
                | "view.split_horizontal"
                | "view.clone_other"
                | "view.move_other"
                | "view.close_split"
                | "view.focus_other"
                | "view.sync_vertical"
                | "view.sync_horizontal"
        ) {
            return false;
        }
        let Some(workspace) = &mut self.workspace else {
            return true;
        };
        self.views.pump(workspace);
        match id {
            "view.split_vertical" => {
                self.views
                    .split(workspace, self.app.active, Orientation::Vertical)
            }
            "view.clone_other" => self.views.clone_active(workspace, &mut self.app),
            "view.move_other" => self.views.move_active(workspace, &mut self.app),
            "view.split_horizontal" => {
                self.views
                    .split(workspace, self.app.active, Orientation::Horizontal)
            }
            "view.close_split" if !self.views.busy(workspace) => {
                self.views.collapse(workspace, self.views.pane() == 1)
            }
            "view.focus_other" if self.views.open() => {
                let pane = 1 - self.views.pane();
                self.views.activate(workspace, &mut self.app, pane);
            }
            "view.sync_vertical" => {
                if let Some(controller) = &mut self.views.controller {
                    controller.sync_vertical = !controller.sync_vertical;
                }
            }
            "view.sync_horizontal" => {
                if let Some(controller) = &mut self.views.controller {
                    controller.sync_horizontal = !controller.sync_horizontal;
                }
            }
            _ => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn views_action(&mut self, _el: &ActiveEventLoop, action: Action) -> bool {
        if !self.views.open() && action != Action::Close {
            return false;
        }
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        self.views.pump(workspace);
        if matches!(
            action,
            Action::Save | Action::SaveAs | Action::Close | Action::Quit
        ) && self.views.busy(workspace)
        {
            workspace.message =
                Some("Wait for pending split-view edits before saving or closing.".into());
            return true;
        }
        if action == Action::Close {
            self.views.save_current(workspace);
            if let Some(id) = self
                .views
                .controller
                .as_ref()
                .and_then(|controller| controller.active_tab(controller.active_pane()))
            {
                let controller = self.views.controller.as_ref().unwrap();
                let document = controller.tab(id).unwrap().document_id;
                if controller
                    .tabs()
                    .iter()
                    .filter(|tab| tab.document_id == document)
                    .count()
                    > 1
                {
                    self.views.close_tab(workspace, &mut self.app, id);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return true;
                }
            }
            return false;
        }
        let pane = self.views.pane();
        let input = match action {
            Action::Undo => Some(Input::Undo),
            Action::Redo => Some(Input::Redo),
            Action::SelectAll => Some(Input::SelectAll),
            Action::Paste => self
                .platform
                .as_ref()
                .and_then(|p| p.clipboard_text().ok())
                .map(Input::Insert),
            Action::Copy | Action::Cut => {
                let editor = if pane == 1 {
                    self.views.secondary.as_ref().map(|editor| &**editor)
                } else {
                    self.views
                        .primary_index(workspace)
                        .and_then(|i| workspace.editors.get(i))
                        .map(|editor| &**editor)
                };
                let copied = editor
                    .and_then(|e| e.selected_text().ok())
                    .is_some_and(|value| {
                        !value.is_empty()
                            && self
                                .platform
                                .as_ref()
                                .is_some_and(|p| p.set_clipboard_text(&value).is_ok())
                    });
                if copied && action == Action::Cut {
                    Some(Input::Insert(String::new()))
                } else {
                    None
                }
            }
            _ => return false,
        };
        if let Some(input) = input {
            self.views.input(workspace, pane, input);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn views_event(&mut self, _el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if matches!(
            event,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } | WindowEvent::KeyboardInput { .. }
        ) {
            self.views.accessibility_focus = None;
        }
        if self.tabs_event(_el, event) {
            return true;
        }
        if (!self.views.open()
            && !self
                .views
                .controller
                .as_ref()
                .is_some_and(|controller| controller.vertical_tabs))
            || self.palette.open
        {
            return false;
        }
        let editor_bounds = self.editor_bounds();
        let pointer = Point {
            x: self.pointer.x - editor_bounds.x,
            y: self.pointer.y - editor_bounds.y,
        };
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        if workspace.find.has_focus() || workspace.search_focus {
            return false;
        }
        let Some(window) = &self.window else {
            return false;
        };
        let mut handled = false;
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let p = position.to_logical::<f32>(window.scale_factor());
                let p = Point {
                    x: p.x - editor_bounds.x,
                    y: p.y - editor_bounds.y,
                };
                if self.views.dragging {
                    if let (Some(first), Some(second), Some(controller)) = (
                        self.views.bounds[0],
                        self.views.bounds[1],
                        &mut self.views.controller,
                    ) {
                        controller.ratio = if controller.orientation == Orientation::Vertical {
                            ((p.x - first.x) / (second.x + second.width - first.x)).clamp(0.1, 0.9)
                                as f64
                        } else {
                            ((p.y - first.y) / (second.y + second.height - first.y)).clamp(0.1, 0.9)
                                as f64
                        };
                    }
                    handled = true;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if self.views.splitter.is_some_and(|r| r.contains(pointer)) {
                    self.views.dragging = true;
                    handled = true;
                } else if let Some(pane) = self
                    .views
                    .bounds
                    .iter()
                    .position(|r| r.is_some_and(|r| r.contains(pointer)))
                {
                    self.views.activate(workspace, &mut self.app, pane as u32);
                    let bounds = self.views.bounds[pane].unwrap();
                    let editor = if pane == 1 {
                        self.views.secondary.as_mut().map(|editor| &mut **editor)
                    } else {
                        self.views
                            .primary_index(workspace)
                            .and_then(|i| workspace.editors.get_mut(i))
                            .map(|editor| &mut **editor)
                    };
                    if let (Some(editor), Some(renderer)) = (editor, &self.renderer) {
                        let _ = editor.click(
                            renderer,
                            Point {
                                x: pointer.x - bounds.x,
                                y: pointer.y - bounds.y,
                            },
                            self.modifiers.shift_key(),
                        );
                    }
                    handled = true;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if self.views.dragging => {
                self.views.dragging = false;
                handled = true;
            }
            WindowEvent::Focused(false) => {
                self.views.dragging = false;
                if let Some(peer) = &mut self.views.secondary {
                    peer.cancel_composition();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(pane) = self
                    .views
                    .bounds
                    .iter()
                    .position(|r| r.is_some_and(|r| r.contains(pointer)))
                {
                    let horizontal = self.modifiers.shift_key()
                        || matches!(delta,MouseScrollDelta::LineDelta(x,y) if x.abs()>y.abs());
                    let amount = match delta {
                        MouseScrollDelta::LineDelta(x, _)
                            if horizontal && !self.modifiers.shift_key() =>
                        {
                            -*x as f64 * 72.0
                        }
                        MouseScrollDelta::PixelDelta(p)
                            if horizontal && !self.modifiers.shift_key() =>
                        {
                            -p.x / window.scale_factor()
                        }
                        MouseScrollDelta::LineDelta(_, y) => -*y as f64 * 72.0,
                        MouseScrollDelta::PixelDelta(p) => -p.y / window.scale_factor(),
                    };
                    let height = self.views.bounds[pane].unwrap().height + 24.0;
                    if pane == 1 {
                        if let Some(peer) = &mut self.views.secondary {
                            if horizontal {
                                peer.horizontal_scroll(amount);
                            } else {
                                if amount < 0.0 {
                                    if let WorkspaceEditor::Paged(editor) = peer {
                                        editor.set_follow_paused(true);
                                    }
                                }
                                peer.scroll(amount, height);
                            }
                        }
                    } else if let Some(index) = self.views.primary_index(workspace) {
                        if horizontal {
                            workspace.editors[index].horizontal_scroll(amount);
                        } else {
                            if amount < 0.0 {
                                if let WorkspaceEditor::Paged(editor) =
                                    &mut workspace.editors[index]
                                {
                                    editor.set_follow_paused(true);
                                }
                            }
                            workspace.editors[index].scroll(amount, height);
                        }
                    }
                    self.views.sync_scroll(workspace, pane as u32);
                    handled = true;
                }
            }
            WindowEvent::Ime(ime) => {
                let pane = self.views.pane();
                let editor = if pane == 1 {
                    self.views.secondary.as_mut().map(|editor| &mut **editor)
                } else {
                    self.views
                        .primary_index(workspace)
                        .and_then(|i| workspace.editors.get_mut(i))
                        .map(|editor| &mut **editor)
                };
                if let Some(peer) = editor {
                    match ime {
                        Ime::Preedit(value, cursor) => peer.preedit(value.clone(), *cursor),
                        Ime::Commit(_) => peer.cancel_composition(),
                        Ime::Disabled => peer.cancel_composition(),
                        Ime::Enabled => {}
                    }
                }
                if let Ime::Commit(value) = ime {
                    self.views
                        .input(workspace, pane, Input::Insert(value.clone()));
                }
                handled = true;
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let extend = self.modifiers.shift_key();
                if matches!(
                    event.logical_key,
                    Key::Named(NamedKey::PageDown | NamedKey::PageUp)
                ) && !self.modifiers.control_key()
                {
                    let forward = matches!(event.logical_key, Key::Named(NamedKey::PageDown));
                    let pane = self.views.pane();
                    let height =
                        self.views.bounds[pane as usize].map_or(400.0, |bounds| bounds.height);
                    let index = self.views.primary_index(workspace);
                    let editor = if pane == 1 {
                        self.views.secondary.as_mut()
                    } else {
                        index.and_then(|index| workspace.editors.get_mut(index))
                    };
                    if let Some(editor) = editor {
                        if !editor.busy() {
                            if !forward {
                                if let WorkspaceEditor::Paged(paged) = editor {
                                    paged.set_follow_paused(true);
                                }
                            }
                            if !editor.page_by(forward) {
                                editor.scroll(
                                    if forward {
                                        height as f64 * 0.8
                                    } else {
                                        -height as f64 * 0.8
                                    },
                                    height,
                                );
                            }
                        }
                    }
                    self.views.sync_scroll(workspace, pane);
                    window.request_redraw();
                    return true;
                }
                let input = match &event.logical_key {
                    Key::Named(NamedKey::ArrowLeft) => Some(Input::Left(extend)),
                    Key::Named(NamedKey::ArrowRight) => Some(Input::Right(extend)),
                    Key::Named(NamedKey::ArrowUp) => Some(Input::Up(extend)),
                    Key::Named(NamedKey::ArrowDown) => Some(Input::Down(extend)),
                    Key::Named(NamedKey::Home) => Some(Input::Home(extend)),
                    Key::Named(NamedKey::End) => Some(Input::End(extend)),
                    Key::Named(NamedKey::Backspace) => Some(Input::Backspace),
                    Key::Named(NamedKey::Delete) => Some(Input::Delete),
                    Key::Named(NamedKey::Enter) => Some(Input::Insert("\n".into())),
                    Key::Named(NamedKey::Tab) if !self.modifiers.control_key() => {
                        Some(Input::Insert("\t".into()))
                    }
                    _ if !self.modifiers.control_key() || self.modifiers.alt_key() => event
                        .text
                        .as_ref()
                        .filter(|text| !text.is_empty() && !text.chars().any(|c| c.is_control()))
                        .map(|text| Input::Insert(text.to_string())),
                    _ => None,
                };
                if let Some(input) = input {
                    let pane = self.views.pane();
                    self.views.input(workspace, pane, input);
                    handled = true;
                }
            }
            _ => {}
        }
        if handled {
            window.request_redraw();
        }
        handled
    }
}
