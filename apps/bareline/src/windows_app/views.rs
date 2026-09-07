// SPDX-License-Identifier: MPL-2.0
//! Native two-pane consumer. Both surfaces address the same document actor when cloned.
use super::*;
use bareline_app::views::{
    Orientation, ScrollPosition, SessionTab, SharedEditorView, SharedSyntaxView, ViewController,
    ViewSnapshot, ViewState,
};
use bareline_commands::{CommandId, CommandPresentation, CommandRegistry, CommandSpec};
use bareline_renderer::{DrawOp, LayoutError, Rect, TextBackend};
use bareline_ui::{ACCENT, BORDER, CHROME, MUTED, TAB_HEIGHT, TEXT, rect, text};
use std::collections::VecDeque;

#[cfg(test)]
mod tests {
    use super::*;

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
    document: ViewSnapshot,
    input: Input,
}
#[derive(Default)]
pub(super) struct ViewsRuntime {
    controller: Option<ViewController>,
    primary: Option<ViewSnapshot>,
    secondary: Option<SharedEditorView>,
    retired: Vec<SharedEditorView>,
    bounds: [Option<Rect>; 2],
    splitter: Option<Rect>,
    dragging: bool,
    queued: VecDeque<QueuedInput>,
    compare: bool,
    alignment: Option<bareline_app::views::AlignmentMap>,
}
impl ViewsRuntime {
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
        for id in ["view.close_split", "view.focus_other", "view.sync_vertical"] {
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
            || [left, right].iter().any(|&index| {
                workspace
                    .editors
                    .get(index)
                    .is_none_or(|editor| editor.paged())
            })
        {
            return false;
        }
        self.split(workspace, left, Orientation::Vertical);
        self.primary = Some(workspace.editors[left].snapshot().clone());
        if let Some(old) = self
            .secondary
            .replace(workspace.editors[right].clone_view())
        {
            self.retired.push(old);
        }
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
                first
                    .and_then(|index| workspace.editors.get_mut(index))
                    .map(|editor| &mut **editor),
            ),
            (right, self.secondary.as_mut()),
        ] {
            if let Some(editor) = editor {
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
        if !manifest.layout.split {
            return;
        }
        let find = |pane: usize| {
            manifest.layout.active_tabs[pane].and_then(|id| {
                tabs.iter()
                    .find(|(tab, _)| *tab == id)
                    .map(|(_, index)| (id, *index))
            })
        };
        let (Some((first_id, first)), Some((second_id, second))) = (find(0), find(1)) else {
            return;
        };
        if workspace.editors[first].paged() || workspace.editors[second].paged() {
            return;
        }
        let Ok(controller) = ViewController::from_session(manifest) else {
            return;
        };
        self.primary = Some(workspace.editors[first].snapshot().clone());
        self.secondary = Some(workspace.editors[second].clone_view());
        self.controller = Some(controller);
        for (id, editor) in [
            (first_id, &mut *workspace.editors[first]),
            (second_id, self.secondary.as_mut().unwrap()),
        ] {
            if let Some(tab) = manifest.tabs.iter().find(|tab| tab.id == id) {
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
                let anchor = bound(tab.view.anchor);
                let caret = bound(tab.view.caret);
                editor.selection.anchor = anchor;
                editor.selection.caret = caret;
                editor.scroll_y = f64::from_bits(tab.view.scroll_y_bits);
                editor.restore_folds(&tab.view.folds);
            }
        }
        self.activate(workspace, app, manifest.layout.active_pane);
    }

    pub(super) fn capture_session(
        &self,
        workspace: &Workspace,
        manifest: &mut bareline_file_io::session::SessionManifest,
        tabs: &[(usize, u64)],
    ) {
        if !self.open() {
            for tab in &mut manifest.tabs {
                tab.view.split = 0;
            }
            manifest.layout.split = false;
            manifest.layout.active_pane = 0;
            manifest.layout.active_tabs = [manifest.active_tab, None];
            return;
        }
        let Some(primary) = self
            .primary_index(workspace)
            .and_then(|index| tabs.iter().find(|(i, _)| *i == index).map(|(_, id)| *id))
        else {
            return;
        };
        let Some(secondary) = self
            .secondary_index(workspace)
            .and_then(|index| tabs.iter().find(|(i, _)| *i == index).map(|(_, id)| *id))
        else {
            return;
        };
        let Some(mut secondary_tab) = manifest
            .tabs
            .iter()
            .find(|tab| tab.id == secondary)
            .cloned()
        else {
            return;
        };
        for tab in &mut manifest.tabs {
            tab.view.split = 0;
        }
        let duplicate = manifest
            .tabs
            .iter()
            .find(|tab| {
                tab.document_id == secondary_tab.document_id
                    && tab.id != primary
                    && tab.id != secondary
            })
            .map(|tab| tab.id);
        let secondary_id = if primary != secondary {
            secondary
        } else if let Some(id) = duplicate {
            id
        } else {
            let Some(id) = manifest
                .tabs
                .iter()
                .map(|tab| tab.id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
            else {
                return;
            };
            secondary_tab.id = id;
            manifest.tabs.push(secondary_tab);
            id
        };
        if let Some(peer) = &self.secondary {
            let tab = manifest
                .tabs
                .iter_mut()
                .find(|tab| tab.id == secondary_id)
                .unwrap();
            tab.view.split = 1;
            tab.view.anchor = peer.selection.anchor as u64;
            tab.view.caret = peer.selection.caret as u64;
            tab.view.scroll_y_bits = peer.scroll_y.to_bits();
            tab.view.folds = peer.persisted_folds();
        }
        let controller = self.controller.as_ref().unwrap();
        manifest.layout.split = true;
        manifest.layout.orientation = match controller.orientation {
            Orientation::Vertical => bareline_file_io::session::SplitOrientation::Vertical,
            Orientation::Horizontal => bareline_file_io::session::SplitOrientation::Horizontal,
        };
        manifest.layout.ratio_bits = controller.ratio.to_bits();
        manifest.layout.sync_vertical = controller.sync_vertical;
        manifest.layout.sync_horizontal = controller.sync_horizontal;
        manifest.layout.active_pane = self.pane();
        manifest.layout.active_tabs = [Some(primary), Some(secondary_id)];
        manifest.active_tab = manifest.layout.active_tabs[self.pane() as usize];
    }
    pub(super) fn pending_edits(&self) -> bool {
        !self.queued.is_empty() || self.secondary.as_ref().is_some_and(SharedEditorView::busy)
    }
    pub(super) fn take_acknowledged_inputs(&mut self) -> Vec<Input> {
        self.secondary
            .as_mut()
            .map(|editor| editor.take_acknowledged_inputs())
            .unwrap_or_default()
    }
    fn open(&self) -> bool {
        self.secondary.is_some() && self.controller.as_ref().is_some_and(|c| c.split)
    }
    fn pane(&self) -> u32 {
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
        self.primary
            .as_ref()
            .and_then(|s| Self::index_of(workspace, s))
    }
    fn secondary_index(&self, workspace: &Workspace) -> Option<usize> {
        self.secondary
            .as_ref()
            .and_then(|e| Self::index_of(workspace, e.snapshot()))
    }
    fn busy(&self, workspace: &Workspace) -> bool {
        !self.queued.is_empty()
            || self.secondary.as_ref().is_some_and(SharedEditorView::busy)
            || self
                .primary_index(workspace)
                .is_some_and(|i| workspace.editors[i].busy())
    }
    pub(super) fn pump(&mut self, workspace: &mut Workspace) -> bool {
        let mut changed = self.secondary.as_mut().is_some_and(SharedEditorView::pump);
        if let Some(index) = self.secondary_index(workspace)
            && let Some(peer) = &mut self.secondary
        {
            if peer.snapshot().revision.0 > workspace.editors[index].snapshot().revision.0 {
                changed |= workspace.editors[index].refresh_peer(peer.snapshot());
            } else if workspace.editors[index].snapshot().revision.0 > peer.snapshot().revision.0 {
                changed |= peer.refresh_peer(workspace.editors[index].snapshot());
            }
            peer.sync_saved_from(&workspace.editors[index]);
            peer.theme = workspace.editors[index].theme;
            peer.sync_fold_metadata_from(&workspace.editors[index]);
        }
        if self.open() && self.secondary_index(workspace).is_none() {
            self.collapse(workspace, false);
            changed = true;
        }
        let primary_busy = self
            .primary_index(workspace)
            .is_some_and(|i| workspace.editors[i].busy());
        if !primary_busy
            && !self.secondary.as_ref().is_some_and(SharedEditorView::busy)
            && let Some(queued) = self.queued.pop_front()
        {
            let editor = if queued.pane == 1 {
                self.secondary.as_mut()
            } else {
                self.primary_index(workspace)
                    .and_then(|i| workspace.editors.get_mut(i))
                    .map(|editor| &mut **editor)
            };
            if let Some(editor) = editor
                && editor.snapshot().same_document(&queued.document)
            {
                editor.enqueue(queued.input);
            } else {
                workspace.message =
                    Some("The view changed before queued input could be applied.".into());
            }
            changed = true;
        }
        changed
    }
    fn input(&mut self, workspace: &mut Workspace, pane: u32, input: Input) {
        self.pump(workspace);
        let document = if pane == 1 {
            self.secondary.as_ref().map(|e| e.snapshot().clone())
        } else {
            self.primary_index(workspace)
                .map(|i| workspace.editors[i].snapshot().clone())
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
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before changing views.".into());
            return;
        }
        let Some(editor) = workspace.editors.get(index) else {
            return;
        };
        if editor.paged() {
            workspace.message = Some("Split views are unavailable for paged documents.".into());
            return;
        }
        if !self.open() {
            let mut peer = editor.clone_view();
            peer.selection = editor.selection;
            peer.scroll_y = editor.scroll_y;
            self.primary = Some(editor.snapshot().clone());
            self.secondary = Some(peer);
            let mut controller = ViewController::new(
                vec![SessionTab {
                    id: 1,
                    document_id: 1,
                    pinned: false,
                    view: ViewState::default(),
                }],
                Some(1),
            )
            .unwrap();
            let _ = controller.clone_to_other(1);
            self.controller = Some(controller);
        }
        if let Some(controller) = &mut self.controller {
            controller.orientation = orientation;
            controller.split = true;
        }
    }
    fn collapse(&mut self, workspace: &mut Workspace, keep_secondary: bool) {
        if keep_secondary
            && let Some(index) = self.secondary_index(workspace)
            && let Some(peer) = &mut self.secondary
        {
            std::mem::swap(&mut *workspace.editors[index], peer);
        }
        if let Some(peer) = self.secondary.take() {
            self.retired.push(peer);
        }
        if let Some(controller) = &mut self.controller {
            controller.collapse();
        }
        self.bounds = [None, None];
        self.splitter = None;
        self.dragging = false;
    }
    fn clone_active(&mut self, workspace: &mut Workspace, app: &mut App) {
        if workspace
            .editors
            .get(app.active)
            .is_some_and(|editor| editor.paged())
        {
            workspace.message = Some("Split views are unavailable for paged documents.".into());
            return;
        }
        if !self.open() {
            self.split(workspace, app.active, Orientation::Vertical);
            return;
        }
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before changing views.".into());
            return;
        }
        if self.pane() == 0 {
            let Some(index) = self.primary_index(workspace) else {
                return;
            };
            let mut clone = workspace.editors[index].clone_view();
            clone.selection = workspace.editors[index].selection;
            clone.scroll_y = workspace.editors[index].scroll_y;
            if let Some(old) = self.secondary.replace(clone) {
                self.retired.push(old);
            }
            self.activate(workspace, app, 1);
        } else {
            let Some(index) = self.secondary_index(workspace) else {
                return;
            };
            if let Some(peer) = &self.secondary {
                workspace.editors[index].selection = peer.selection;
                workspace.editors[index].scroll_y = peer.scroll_y;
            }
            self.primary = Some(workspace.editors[index].snapshot().clone());
            self.activate(workspace, app, 0);
        }
    }
    fn move_active(&mut self, workspace: &mut Workspace, app: &mut App) {
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before moving a view.".into());
            return;
        }
        if self.open() && self.pane() == 1 {
            self.collapse(workspace, true);
            return;
        }
        let moving = app.active;
        self.clone_active(workspace, app);
        if let Some(other) =
            (0..workspace.editors.len()).find(|i| *i != moving && !workspace.editors[*i].paged())
        {
            self.primary = Some(workspace.editors[other].snapshot().clone());
        } else {
            self.collapse(workspace, true);
        }
    }
    fn activate(&mut self, workspace: &Workspace, app: &mut App, pane: u32) {
        if let Some(controller) = &mut self.controller
            && let Some(id) = controller.active_tab(pane)
        {
            let _ = controller.activate(id);
        }
        if let Some(index) = if pane == 0 {
            self.primary_index(workspace)
        } else {
            self.secondary_index(workspace)
        } {
            app.active = index;
        }
    }
    fn sync_scroll(&mut self, workspace: &mut Workspace, pane: u32) {
        let y = if pane == 1 {
            self.secondary.as_ref().map(|e| e.scroll_y)
        } else {
            self.primary_index(workspace)
                .map(|i| workspace.editors[i].scroll_y)
        };
        let Some(y) = y else {
            return;
        };
        let Some(controller) = &mut self.controller else {
            return;
        };
        if let Ok(Some(update)) = controller.begin_scroll(
            pane,
            ScrollPosition {
                line: (y / 19.2).floor() as u64,
                fraction: (y / 19.2).fract(),
                x: 0.0,
            },
            self.alignment.as_ref(),
        ) {
            let y = (update.position.line as f64 + update.position.fraction) * 19.2;
            if update.pane == 1 {
                if let Some(peer) = &mut self.secondary {
                    peer.scroll_y = y;
                }
            } else if let Some(index) = self.primary_index(workspace) {
                workspace.editors[index].scroll_y = y;
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
        if workspace
            .editors
            .get(app.active)
            .is_some_and(|editor| editor.paged())
            && self.open()
        {
            self.collapse(workspace, false);
        }
        if !self.open() {
            return workspace.draw(app.active, renderer, width, height, ops);
        }
        let pane = self.pane();
        let expected = if pane == 0 {
            self.primary_index(workspace)
        } else {
            self.secondary_index(workspace)
        };
        if expected != Some(app.active)
            && !self.busy(workspace)
            && let Some(editor) = workspace.editors.get(app.active)
        {
            if pane == 0 {
                self.primary = Some(editor.snapshot().clone());
            } else {
                if let Some(old) = self.secondary.replace(editor.clone_view()) {
                    self.retired.push(old);
                }
            }
        }
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
        let titles = workspace.titles();
        let second = self.secondary_index(workspace).unwrap_or(first);
        let mut active_caret = None;
        let mut status = Vec::new();
        for side in 0..2 {
            let Some(bounds) = self.bounds[side] else {
                continue;
            };
            let editor = if side == 0 {
                &mut workspace.editors[first]
            } else {
                self.secondary.as_mut().unwrap()
            };
            // EditorSurface already reserves TAB_HEIGHT for this pane's header.
            editor.top_inset = 0.0;
            editor.bottom_inset = 0.0;
            let mut local = Vec::new();
            let local_height = bounds.height + 24.0;
            let caret = editor.draw_styled(
                renderer,
                bounds.width,
                local_height,
                &mut local,
                SharedSyntaxView {
                    result: None,
                    language: editor.language.label(),
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

impl Shell {
    pub(super) fn views_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if !matches!(
            id,
            "view.split_vertical"
                | "view.split_horizontal"
                | "view.clone_other"
                | "view.move_other"
                | "view.close_split"
                | "view.focus_other"
                | "view.sync_vertical"
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
            _ => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn views_action(&mut self, _el: &ActiveEventLoop, action: Action) -> bool {
        if !self.views.open() {
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
        if action == Action::Close
            && (self.views.pane() == 1
                || self.views.primary_index(workspace) == self.views.secondary_index(workspace))
        {
            self.views.collapse(workspace, self.views.pane() == 0);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return true;
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
                    self.views.secondary.as_ref()
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
        if !self.views.open() || self.palette.open {
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
                        self.views.secondary.as_mut()
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
                    let amount = match delta {
                        MouseScrollDelta::LineDelta(_, y) => -*y as f64 * 72.0,
                        MouseScrollDelta::PixelDelta(p) => -p.y / window.scale_factor(),
                    };
                    let height = self.views.bounds[pane].unwrap().height + 24.0;
                    if pane == 1 {
                        if let Some(peer) = &mut self.views.secondary {
                            peer.scroll(amount, height);
                        }
                    } else if let Some(index) = self.views.primary_index(workspace) {
                        workspace.editors[index].scroll(amount, height);
                    }
                    self.views.sync_scroll(workspace, pane as u32);
                    handled = true;
                }
            }
            WindowEvent::Ime(ime) => {
                let pane = self.views.pane();
                let editor = if pane == 1 {
                    self.views.secondary.as_mut()
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
