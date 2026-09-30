// SPDX-License-Identifier: MPL-2.0
//! Native byte scrollbar capture using the shared PR023 control, plus the
//! horizontal bar each editor pane shows for long lines with wrap off (EDT-28).
use super::*;
use bareline_app::workspace::WorkspaceEditor;
use bareline_platform::accessibility::AccessibilityNode;
use bareline_renderer::{DrawOp, Rect};
use bareline_ui::controls::{HorizontalScrollbar, ScrollAction, Scrollbar, ScrollbarInteraction, UiEvent};
/// Accessibility ids of the per-pane horizontal scroll bars (pane 0, pane 1).
pub(super) const HORIZONTAL_SCROLLBAR_ID: u64 = 90_000_060;
/// Accessibility ids of the per-pane vertical scroll bars (pane 0, pane 1).
pub(super) const VERTICAL_SCROLLBAR_ID: u64 = 90_000_062;
#[derive(Default)]
pub(super) struct Runtime {
    bars: [Option<Scrollbar>; 2],
    interactions: [ScrollbarInteraction; 2],
    identities: [Option<(u64, u64)>; 2],
    captured: Option<usize>,
    /// Kept in window coordinates for hit testing, drag capture and
    /// accessibility. A resident surface paints its own; a paged one is
    /// measured over whole source lines and painted here (EDT-28).
    horizontal: [Option<HorizontalScrollbar>; 2],
    horizontal_interactions: [ScrollbarInteraction; 2],
    horizontal_captured: Option<usize>,
    /// A resident surface paints its own bar, so a deferred drag hands it the
    /// previewed offset; this notes one may still be held there.
    horizontal_previewing: bool,
}
impl Runtime {
    pub(super) fn draw(
        &mut self,
        workspace: &bareline_app::workspace::Workspace,
        views: &super::views::ViewsRuntime,
        active: usize,
        bounds: Rect,
        ops: &mut Vec<DrawOp>,
    ) {
        let split = views.secondary.is_some();
        for pane in 0..2 {
            let editor = if pane == 1 {
                views.secondary.as_ref()
            } else {
                workspace.editors.get(views.primary_index(workspace).unwrap_or(active))
            };
            let Some(editor) = editor else {
                self.bars[pane] = None;
                self.horizontal[pane] = None;
                continue;
            };
            let area = if split {
                views.bounds[pane].map(|r| Rect {
                    x: r.x + bounds.x,
                    y: r.y + bounds.y,
                    ..r
                })
            } else if pane == 0 {
                // The single pane sits beside any vertical tab strip, offset
                // and narrowed exactly as `ViewsRuntime::draw` draws it.
                let (inset, width) = views.find_horizontal_geometry(bounds.width);
                Some(Rect {
                    x: bounds.x + inset,
                    width,
                    ..bounds
                })
            } else {
                None
            };
            let Some(area) = area else {
                self.bars[pane] = None;
                self.horizontal[pane] = None;
                continue;
            };
            let identity = match editor {
                WorkspaceEditor::Paged(editor) => editor.snapshot().identity_token(),
                WorkspaceEditor::Resident(editor) => editor.snapshot().identity_token(),
            };
            if self.identities[pane] != Some(identity) {
                self.interactions[pane] = ScrollbarInteraction::default();
                self.interactions[pane].deferred = true;
                if self.captured == Some(pane) {
                    self.captured = None;
                }
                self.horizontal_interactions[pane] = ScrollbarInteraction::default();
                if self.horizontal_captured == Some(pane) {
                    self.horizontal_captured = None;
                }
                self.identities[pane] = Some(identity);
            }
            let body_height = (area.height
                - bareline_ui::TAB_HEIGHT
                - editor.viewport().top_inset
                - editor.viewport().bottom_inset
                - if split { 0.0 } else { bareline_ui::STATUS_HEIGHT })
            .max(0.0);
            // A paged view still loading, or a failed open, paints a
            // placeholder instead of the text, so it has no horizontal bar.
            let document = if pane == 1 {
                None
            } else {
                Some(views.primary_index(workspace).unwrap_or(active))
            };
            let shown = !matches!(editor, WorkspaceEditor::Paged(paged) if !paged.paged_frame_state().ready)
                && document.is_none_or(|index| workspace.failed_open(index).is_none());
            let horizontal = match editor {
                WorkspaceEditor::Paged(paged) if shown && paged.needs_horizontal_scrollbar(area.width, body_height) => {
                    Some(paged.horizontal_scrollbar(area.width, body_height))
                }
                WorkspaceEditor::Resident(resident)
                    if shown && resident.needs_horizontal_scrollbar(area.width, body_height) =>
                {
                    Some(resident.horizontal_scrollbar(area.width, body_height))
                }
                _ => None,
            };
            self.horizontal[pane] = horizontal.map(|mut bar| {
                bar.bounds.x += area.x;
                bar.bounds.y += area.y;
                if self.horizontal_captured == Some(pane)
                    && let Some(previous) = &self.horizontal[pane]
                {
                    // Hold the dragged thumb and its scale until release.
                    bar.offset = previous.offset;
                    bar.total = previous.total;
                }
                if matches!(editor, WorkspaceEditor::Paged(_)) {
                    bar.paint_with_theme(workspace.theme, ops);
                }
                bar
            });
            if self.horizontal_captured != Some(pane) {
                // Estimated extents commit once, on release: a paged or long
                // line may need a window load or a new fragment per commit.
                self.horizontal_interactions[pane].deferred =
                    matches!(editor, WorkspaceEditor::Paged(_)) || editor.viewport().horizontal_estimated();
            }
            let geometry = Rect {
                x: area.x + area.width - 12.0,
                y: area.y + bareline_ui::TAB_HEIGHT + editor.viewport().top_inset,
                width: 12.0,
                height: body_height,
            };
            let mut bar = match editor {
                WorkspaceEditor::Paged(editor) => {
                    let metrics = editor.paged_scroll_metrics(body_height);
                    Scrollbar {
                        bounds: geometry,
                        offset: metrics.offset,
                        viewport: metrics.viewport,
                        total: Some(metrics.total),
                    }
                }
                WorkspaceEditor::Resident(editor) => editor.scrollbar(geometry),
            };
            if self.captured == Some(pane) {
                if let Some(previous) = &self.bars[pane] {
                    bar.offset = previous.offset;
                }
            }
            bar.paint_with_theme(workspace.theme, ops);
            self.bars[pane] = Some(bar);
        }
    }
}
impl Shell {
    pub(super) fn scrolling_event(&mut self, event: &WindowEvent) -> bool {
        let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        let ui = match event {
            WindowEvent::CursorMoved { position, .. } => UiEvent::PointerMove(Point {
                x: (position.x / scale) as f32,
                y: (position.y / scale) as f32,
            }),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => UiEvent::PointerDown(self.pointer),
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => UiEvent::PointerUp(self.pointer),
            WindowEvent::Focused(false) => UiEvent::Focus(false),
            _ => return false,
        };
        self.scrolling_ui_event(ui)
    }
    /// Routes one pointer or focus event to the scroll bar it hits or holds.
    fn scrolling_ui_event(&mut self, ui: UiEvent) -> bool {
        if matches!(ui, UiEvent::Focus(false)) {
            for pane in 0..2 {
                if let Some(bar) = &mut self.scrolling.bars[pane] {
                    self.scrolling.interactions[pane].event(bar, ui, true, false, 1.0);
                }
                if let Some(bar) = &mut self.scrolling.horizontal[pane] {
                    self.scrolling.horizontal_interactions[pane].horizontal_event(bar, ui, true, false, 1.0);
                }
            }
            self.scrolling.captured = None;
            self.scrolling.horizontal_captured = None;
            self.release_horizontal_preview();
            return false;
        }
        self.release_horizontal_preview();
        let horizontal = self.scrolling.horizontal_captured.or_else(|| match ui {
            UiEvent::PointerDown(p) if self.scrolling.captured.is_none() => self
                .scrolling
                .horizontal
                .iter()
                .position(|bar| bar.as_ref().is_some_and(|bar| bar.bounds.contains(p))),
            _ => None,
        });
        if let Some(pane) = horizontal {
            return self.horizontal_scrolling_event(pane, ui);
        }
        let pane = self.scrolling.captured.or_else(|| match ui {
            UiEvent::PointerDown(p) => self
                .scrolling
                .bars
                .iter()
                .position(|bar| bar.as_ref().is_some_and(|bar| bar.bounds.contains(p))),
            _ => None,
        });
        let Some(pane) = pane else {
            return false;
        };
        if matches!(ui, UiEvent::PointerDown(_)) {
            self.scrolling.captured = Some(pane);
        }
        let Some(bar) = &mut self.scrolling.bars[pane] else {
            return false;
        };
        let action = self.scrolling.interactions[pane].event(bar, ui, true, false, 1.0);
        let fraction = if bar.maximum() > 0.0 {
            bar.offset / bar.maximum()
        } else {
            0.0
        };
        let height = bar.bounds.height;
        if matches!(ui, UiEvent::PointerUp(_)) {
            self.scrolling.captured = None;
        }
        if let Some(ScrollAction::Commit(_)) = action {
            let index = self
                .workspace
                .as_ref()
                .and_then(|w| self.views.primary_index(w))
                .unwrap_or(self.app.active);
            let mut editor = if pane == 1 {
                self.views.secondary.as_mut()
            } else {
                self.workspace.as_mut().and_then(|w| w.editors.get_mut(index))
            };
            if let Some(WorkspaceEditor::Resident(editor)) = &mut editor {
                if self.scrolling.identities[pane] == Some(editor.snapshot().identity_token()) {
                    editor.scroll(
                        bar.offset - editor.scroll_y,
                        height + bareline_ui::TAB_HEIGHT + editor.top_inset + bareline_ui::STATUS_HEIGHT,
                    );
                }
            }
            if let Some(WorkspaceEditor::Paged(editor)) = editor {
                if self.scrolling.identities[pane] == Some(editor.snapshot().identity_token()) {
                    if let Err(error) = editor.request_byte_scroll_in_view(fraction, height) {
                        editor.error = Some(error);
                    }
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    /// Thumb drag and track paging for a pane's horizontal bar. Exact extents
    /// commit every drag step, since panning only moves `scroll_x`; estimated
    /// ones (paged views, long lines) commit on release, see `Runtime::draw`.
    fn horizontal_scrolling_event(&mut self, pane: usize, ui: UiEvent) -> bool {
        match ui {
            UiEvent::PointerDown(_) => self.scrolling.horizontal_captured = Some(pane),
            UiEvent::PointerUp(_) => self.scrolling.horizontal_captured = None,
            _ => {}
        }
        let Some(bar) = &mut self.scrolling.horizontal[pane] else {
            self.scrolling.horizontal_captured = None;
            return false;
        };
        let action = self.scrolling.horizontal_interactions[pane].horizontal_event(bar, ui, true, false, 1.0);
        let (offset, viewport) = (bar.offset, bar.viewport);
        if action.is_some() {
            let index = self
                .workspace
                .as_ref()
                .and_then(|w| self.views.primary_index(w))
                .unwrap_or(self.app.active);
            let editor = if pane == 1 {
                self.views.secondary.as_mut()
            } else {
                self.workspace.as_mut().and_then(|w| w.editors.get_mut(index))
            };
            // The identity is the one Runtime::draw recorded: the source
            // snapshot for a paged view, not its window projection. A paged view
            // that is still loading has no bar to follow; skip until ready.
            if let Some(editor) = editor
                && self.scrolling.identities[pane] == Some(editor.document_identity())
            {
                match (editor, action) {
                    (WorkspaceEditor::Paged(paged), Some(ScrollAction::Commit(_))) => {
                        if paged.paged_frame_state().ready
                            && let Err(error) = paged.scroll_horizontal_to(offset, viewport)
                        {
                            paged.error = Some(error);
                        }
                    }
                    // Runtime::draw paints a paged view's held thumb itself.
                    (WorkspaceEditor::Paged(_), _) => {}
                    (WorkspaceEditor::Resident(resident), Some(ScrollAction::Commit(_))) => {
                        resident.scroll_horizontal_to(offset)
                    }
                    (WorkspaceEditor::Resident(resident), _) => {
                        resident.preview_horizontal_scroll(Some(offset));
                        self.scrolling.horizontal_previewing = true;
                    }
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    /// Releases a resident surface's previewed thumb once the drag holding it
    /// has ended without committing: focus loss, a vanished bar, or a document
    /// change that reset the capture mid-drag.
    fn release_horizontal_preview(&mut self) {
        if self.scrolling.horizontal_captured.is_some() || !self.scrolling.horizontal_previewing {
            return;
        }
        self.scrolling.horizontal_previewing = false;
        let editors = self
            .workspace
            .iter_mut()
            .flat_map(|workspace| workspace.editors.iter_mut());
        for editor in editors.chain(self.views.secondary.iter_mut()) {
            if let WorkspaceEditor::Resident(resident) = editor {
                resident.preview_horizontal_scroll(None);
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
impl Runtime {
    /// Scroll-bar nodes for the bars currently shown, in logical window
    /// coordinates like the other chrome nodes. The provider reads each bar's
    /// axis from its shape; the value is its position as a percentage.
    pub(super) fn accessibility_nodes(&self) -> Vec<AccessibilityNode> {
        let name = |axis: &str, pane: usize| {
            if pane == 0 {
                format!("{axis} scroll bar")
            } else {
                format!("{axis} scroll bar, pane {}", pane + 1)
            }
        };
        let state = bareline_ui::controls::ControlState::default();
        let mut nodes = Vec::new();
        for pane in 0..2 {
            if let Some(bar) = &self.bars[pane] {
                let semantics = bar.semantics(
                    bareline_ui::ViewId(VERTICAL_SCROLLBAR_ID + pane as u64),
                    &name("Vertical", pane),
                    "view.scrollVertical",
                    state,
                );
                nodes.push(bareline_app::accessibility::semantic_node(&semantics, 1));
            }
            if let Some(bar) = &self.horizontal[pane] {
                let semantics = bar.semantics(
                    bareline_ui::ViewId(HORIZONTAL_SCROLLBAR_ID + pane as u64),
                    &name("Horizontal", pane),
                    "view.scrollHorizontal",
                    state,
                );
                nodes.push(bareline_app::accessibility::semantic_node(&semantics, 1));
            }
        }
        nodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_thumb_reaches_distant_source_with_deferred_capture() {
        let mut bar = Scrollbar {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                width: 12.0,
                height: 600.0,
            },
            offset: 0.0,
            viewport: 65_536.0,
            total: Some(1_073_741_824.0),
        };
        let mut interaction = ScrollbarInteraction::default();
        interaction.deferred = true;
        let down = Point {
            x: 6.0,
            y: bar.thumb().y + 2.0,
        };
        assert!(
            interaction
                .event(&mut bar, UiEvent::PointerDown(down), true, false, 1.0)
                .is_none()
        );
        let far = Point { x: 6.0, y: 590.0 };
        assert!(matches!(
            interaction.event(&mut bar, UiEvent::PointerMove(far), true, false, 1.0),
            Some(ScrollAction::Preview(_))
        ));
        let Some(ScrollAction::Commit(value)) = interaction.event(&mut bar, UiEvent::PointerUp(far), true, false, 1.0)
        else {
            panic!("drag did not commit")
        };
        assert!(value / bar.maximum() > 0.95);
        interaction.event(&mut bar, UiEvent::Focus(false), true, false, 1.0);
        assert!(
            interaction
                .event(&mut bar, UiEvent::PointerMove(down), true, false, 1.0)
                .is_none()
        );
    }
    #[test]
    fn paged_thumb_position_tracks_byte_offset() {
        // 106 MiB document, a 64 KiB window resident, scrolled to the halfway byte.
        // The thumb must sit at the same fraction of the track as offset/total, so a
        // thumb dragged to 50% lands around byte 53 MiB regardless of the line total.
        let total = 106.0 * 1024.0 * 1024.0;
        let bar = Scrollbar {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                width: 12.0,
                height: 600.0,
            },
            offset: total / 2.0,
            viewport: 65_536.0,
            total: Some(total),
        };
        let byte_fraction = bar.offset / bar.maximum();
        assert!((byte_fraction - 0.5).abs() < 0.01, "byte fraction {byte_fraction}");
        let thumb = bar.thumb();
        let travel = (bar.bounds.height - thumb.height) as f64;
        let thumb_fraction = (thumb.y - bar.bounds.y) as f64 / travel;
        assert!(
            (thumb_fraction - byte_fraction).abs() < 0.01,
            "thumb fraction {thumb_fraction} vs byte fraction {byte_fraction}"
        );
    }
    /// EDT-28 / MT-24: a line longer than the paged window. The bar spans the
    /// whole source line, and paging and dragging it through the shell's
    /// hit-test route move the view, across windows when needed.
    #[test]
    fn paged_horizontal_bar_spans_the_source_line_and_drags_across_windows() {
        use bareline_app::workspace::Workspace;
        use bareline_renderer_recording::RecordingBackend;
        use std::time::{Duration, Instant};
        const WIDTH: f32 = 800.0;
        const HEIGHT: f32 = 600.0;
        // Pumps the source and paints the pane until the window, its line
        // mapping, the long line's fragment and any horizontal anchor settle.
        fn settle(workspace: &mut Workspace, backend: &mut RecordingBackend) {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                workspace.pump();
                let io = workspace.io_busy();
                if !io
                    && let Some(WorkspaceEditor::Paged(paged)) = workspace.editors.get_mut(0)
                    && paged.paged_frame_state().ready
                    && paged.viewport_first_line_start().is_some()
                {
                    paged.viewport_mut().set_external_scrollbar(true);
                    let mut ops = Vec::new();
                    paged.viewport_mut().draw(backend, WIDTH, HEIGHT, &mut ops).unwrap();
                    let preparing = ops
                        .iter()
                        .any(|op| matches!(op, DrawOp::Text { text, .. } if text == "Preparing line…"));
                    if !preparing
                        && !paged.surface.horizontal_anchor_pending()
                        && paged.viewport().horizontal_estimated()
                    {
                        return;
                    }
                }
                assert!(Instant::now() < deadline, "{:?}", workspace.message);
                std::thread::yield_now();
            }
        }
        fn paged(shell: &mut Shell) -> &mut bareline_editor_surface::paged_view::PagedEditorSurface {
            match shell.workspace.as_mut().unwrap().editors.get_mut(0) {
                Some(WorkspaceEditor::Paged(paged)) => paged,
                _ => panic!("the fixture must open paged"),
            }
        }
        fn bar(shell: &mut Shell) -> HorizontalScrollbar {
            let bounds = Rect {
                x: 0.0,
                y: 0.0,
                width: WIDTH,
                height: HEIGHT,
            };
            let workspace = shell.workspace.as_ref().unwrap();
            shell
                .scrolling
                .draw(workspace, &shell.views, shell.app.active, bounds, &mut Vec::new());
            shell.scrolling.horizontal[0].expect("a long line shows the horizontal bar")
        }
        let root = std::env::temp_dir().join(format!(
            "bareline-paged-hscroll-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("line.txt");
        // Four 64 KiB paged windows of one line; each byte is 9.6 px wide.
        let length = 256 * 1024;
        std::fs::write(&path, "x".repeat(length)).unwrap();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.resident_max_bytes = 4;
        workspace.open(path);
        let mut backend = RecordingBackend::default();
        settle(&mut workspace, &mut backend);
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        shell.workspace = Some(workspace);

        // The extent is the whole source line, not the loaded window.
        let first = bar(&mut shell);
        let whole = length as f64 * 9.6;
        assert!((first.total.unwrap() - whole).abs() < whole * 0.001, "{first:?}");
        assert_eq!(first.offset, 0.0);

        // Clicking the track right of the thumb pages by one text width.
        let track = Point {
            x: first.bounds.x + first.bounds.width - 4.0,
            y: first.bounds.y + first.bounds.height / 2.0,
        };
        assert!(shell.scrolling_ui_event(UiEvent::PointerDown(track)));
        assert!(shell.scrolling_ui_event(UiEvent::PointerUp(track)));
        assert!((paged(&mut shell).viewport().scroll_x() - first.viewport).abs() < 1e-6);

        // Dragging the thumb to the far end loads the window at the line's end
        // and anchors the view there, leaving the thumb at the end of its track.
        settle(shell.workspace.as_mut().unwrap(), &mut backend);
        let before = bar(&mut shell);
        assert!((before.offset - before.viewport).abs() < 1e-6, "{before:?}");
        let thumb = before.thumb();
        let grab = Point {
            x: thumb.x + thumb.width / 2.0,
            y: thumb.y + thumb.height / 2.0,
        };
        let end = Point {
            x: before.bounds.x + before.bounds.width + 100.0,
            y: grab.y,
        };
        assert!(shell.scrolling_ui_event(UiEvent::PointerDown(grab)));
        assert!(shell.scrolling_ui_event(UiEvent::PointerMove(end)));
        // A paged drag previews until release instead of loading every step.
        assert_eq!(paged(&mut shell).viewport_start().0, 0);
        assert!(paged(&mut shell).paged_frame_state().ready);
        assert!(shell.scrolling_ui_event(UiEvent::PointerUp(end)));
        assert!(!paged(&mut shell).paged_frame_state().ready);
        settle(shell.workspace.as_mut().unwrap(), &mut backend);
        let view = paged(&mut shell);
        assert!(
            view.viewport_start().0 >= length - 64 * 1024,
            "{:?}",
            view.viewport_start()
        );
        assert!(view.viewport().scroll_x() > before.viewport, "scroll_x did not move");
        let after = bar(&mut shell);
        assert!((after.offset - after.maximum()).abs() < 20.0, "{after:?}");
        assert!((after.total.unwrap() - whole).abs() < whole * 0.001, "{after:?}");
        let thumb = after.thumb();
        assert!((thumb.x + thumb.width - (after.bounds.x + after.bounds.width)).abs() < 0.5);

        // Both bars are exposed, with their position as a percentage.
        let nodes = shell.scrolling.accessibility_nodes();
        let horizontal = nodes.iter().find(|node| node.id == HORIZONTAL_SCROLLBAR_ID).unwrap();
        assert_eq!(horizontal.value.as_deref(), Some("100%"));
        assert!(horizontal.bounds[2] > horizontal.bounds[3]);
        let vertical = nodes.iter().find(|node| node.id == VERTICAL_SCROLLBAR_ID).unwrap();
        assert!(vertical.value.as_deref().is_some_and(|value| value.ends_with('%')));
        assert!(vertical.bounds[3] > vertical.bounds[2]);
        drop(shell);
        let _ = std::fs::remove_dir_all(root);
    }
    const VIEW_WIDTH: f32 = 1000.0;
    const VIEW_HEIGHT: f32 = 700.0;
    /// A headless shell whose only tab is a resident view of `text`.
    fn resident_shell(text: &str) -> Shell {
        let mut workspace = bareline_app::workspace::Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        let document = bareline_document::Document::from_utf8(
            text,
            bareline_document::Budget::new(1 << 20),
            bareline_document::Budget::new(1 << 20),
        )
        .unwrap();
        let index = workspace
            .add_snapshot_preview(&document.snapshot(), "line.txt".into())
            .unwrap();
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        shell.app.active = index;
        shell.workspace = Some(workspace);
        shell
    }
    fn resident(shell: &mut Shell) -> &mut bareline_editor_surface::EditorSurface {
        match shell.workspace.as_mut().unwrap().editors.get_mut(shell.app.active) {
            Some(WorkspaceEditor::Resident(resident)) => resident,
            _ => panic!("the fixture must open resident"),
        }
    }
    /// Paints the editor layer the way the shell does: the views draw the
    /// surface, then the scroll runtime measures its bars against it.
    fn paint(shell: &mut Shell, backend: &mut bareline_renderer_recording::RecordingBackend) -> Vec<DrawOp> {
        let mut ops = Vec::new();
        let workspace = shell.workspace.as_mut().unwrap();
        shell
            .views
            .draw(
                workspace,
                &mut shell.app,
                backend,
                VIEW_WIDTH,
                VIEW_HEIGHT,
                &mut ops,
                std::sync::Arc::new(|| {}),
            )
            .unwrap();
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: VIEW_WIDTH,
            height: VIEW_HEIGHT,
        };
        let workspace = shell.workspace.as_ref().unwrap();
        shell
            .scrolling
            .draw(workspace, &shell.views, shell.app.active, bounds, &mut Vec::new());
        ops
    }
    /// Whether `ops` fills a rectangle at `thumb`.
    fn painted(ops: &[DrawOp], thumb: Rect) -> bool {
        ops.iter().any(|op| {
            matches!(op, DrawOp::Fill(r, _) if (r.x - thumb.x).abs() < 0.01
                && (r.y - thumb.y).abs() < 0.01
                && (r.width - thumb.width).abs() < 0.01
                && (r.height - thumb.height).abs() < 0.01)
        })
    }
    /// EDT-28: with vertical tabs the single pane is drawn beside the tab
    /// strip, shifted and narrowed by its inset. The runtime's horizontal bar
    /// must sit exactly where the surface paints it, so it hits its own thumb
    /// and leaves clicks on the strip to the strip.
    #[test]
    fn single_pane_horizontal_bar_matches_the_surface_beside_vertical_tabs() {
        // 200 columns at 9.6 px overflow the text area.
        let mut shell = resident_shell(&format!("short\n{}\n", "x".repeat(200)));
        let workspace = shell.workspace.as_ref().unwrap();
        shell.views.test_set_vertical_tabs(workspace, true);
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        paint(&mut shell, &mut backend);
        let ops = paint(&mut shell, &mut backend);
        let (inset, content_width) = shell.views.find_horizontal_geometry(VIEW_WIDTH);
        assert_eq!(inset, 176.0);
        let bar = shell.scrolling.horizontal[0].expect("a long line shows the horizontal bar");

        // The surface's own bar, drawn at its width and translated by the inset.
        let view = resident(&mut shell);
        let body =
            VIEW_HEIGHT - bareline_ui::TAB_HEIGHT - view.top_inset - view.bottom_inset - bareline_ui::STATUS_HEIGHT;
        let mut surface = view.horizontal_scrollbar(content_width, body);
        surface.bounds.x += inset;
        assert_eq!(bar.bounds, surface.bounds);
        assert_eq!(bar.viewport, surface.viewport);
        assert_eq!(bar.total, surface.total);
        assert_eq!(bar.thumb(), surface.thumb());
        assert!(
            painted(&ops, bar.thumb()),
            "the runtime thumb is not where the surface painted it"
        );
        // The accessibility node reports the same bounds.
        let nodes = shell.scrolling.accessibility_nodes();
        let node = nodes.iter().find(|node| node.id == HORIZONTAL_SCROLLBAR_ID).unwrap();
        assert_eq!(node.bounds[0], f64::from(bar.bounds.x));

        // The bottom of the tab strip, left of the text, is not the bar.
        let strip = Point {
            x: inset / 2.0,
            y: bar.bounds.y + bar.bounds.height / 2.0,
        };
        assert!(!shell.scrolling_ui_event(UiEvent::PointerDown(strip)));
        assert!(!shell.scrolling_ui_event(UiEvent::PointerUp(strip)));
        // A click on the painted track right of the thumb pages the view.
        let track = Point {
            x: bar.bounds.x + bar.bounds.width - 4.0,
            y: bar.bounds.y + bar.bounds.height / 2.0,
        };
        assert!(shell.scrolling_ui_event(UiEvent::PointerDown(track)));
        assert!(shell.scrolling_ui_event(UiEvent::PointerUp(track)));
        assert!((resident(&mut shell).scroll_x() - bar.viewport).abs() < 1e-6);
    }
    /// EDT-28 / MT-24: a resident line past the 4 KiB virtual-line threshold
    /// has an estimated extent, so its thumb drag commits only on release. The
    /// surface paints that bar itself, and must paint the previewed thumb
    /// while the drag is held rather than leave it frozen until release.
    #[test]
    fn resident_long_line_drag_moves_the_painted_thumb_before_release() {
        use std::time::{Duration, Instant};
        // Paints until the line's fragment is prepared and any anchor landed.
        fn settle(shell: &mut Shell, backend: &mut bareline_renderer_recording::RecordingBackend) -> Vec<DrawOp> {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let ops = paint(shell, backend);
                let preparing = ops
                    .iter()
                    .any(|op| matches!(op, DrawOp::Text { text, .. } if text == "Preparing line…"));
                let view = resident(shell);
                if !preparing && !view.horizontal_anchor_pending() && view.horizontal_estimated() {
                    return ops;
                }
                assert!(Instant::now() < deadline, "long line never prepared");
                std::thread::yield_now();
            }
        }
        let mut shell = resident_shell(&"x".repeat(40_000));
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        let ops = settle(&mut shell, &mut backend);
        let before = shell.scrolling.horizontal[0].expect("a long line shows the horizontal bar");
        assert!(before.total.unwrap() > 40_000.0 * 9.0, "{before:?}");
        let thumb = before.thumb();
        assert!(painted(&ops, thumb));

        let grab = Point {
            x: thumb.x + thumb.width / 2.0,
            y: thumb.y + thumb.height / 2.0,
        };
        let middle = Point {
            x: before.bounds.x + before.bounds.width / 2.0,
            y: grab.y,
        };
        assert!(shell.scrolling_ui_event(UiEvent::PointerDown(grab)));
        assert!(shell.scrolling_ui_event(UiEvent::PointerMove(middle)));
        // Nothing is committed yet: the view has not panned...
        assert_eq!(resident(&mut shell).scroll_x(), 0.0);
        assert!(!resident(&mut shell).horizontal_anchor_pending());
        // ...but the thumb the surface paints follows the pointer.
        let ops = paint(&mut shell, &mut backend);
        let held = shell.scrolling.horizontal[0].unwrap();
        assert!(held.thumb().x > thumb.x + 100.0, "{held:?}");
        assert!(painted(&ops, held.thumb()), "the painted thumb did not follow the drag");
        assert!(!painted(&ops, thumb), "the painted thumb stayed at the drag start");

        // Release commits there, and the view lands where the thumb was held.
        assert!(shell.scrolling_ui_event(UiEvent::PointerUp(middle)));
        let ops = settle(&mut shell, &mut backend);
        let after = shell.scrolling.horizontal[0].unwrap();
        assert!((after.offset - held.offset).abs() < 20.0, "{after:?} vs {held:?}");
        // The surface paints from its own pan again, with no preview held.
        assert!(painted(&ops, after.thumb()));
    }
}
