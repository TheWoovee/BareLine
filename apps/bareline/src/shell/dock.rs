// SPDX-License-Identifier: MPL-2.0
use bareline_renderer::{DrawOp, Point, Rect};
use bareline_ui::controls::ControlState;
use bareline_ui::theme::UiTheme;
use bareline_ui::widgets::{SemanticAction, SemanticRole, Semantics};

pub(super) fn register(registry: &mut bareline_commands::CommandRegistry) {
    for (id, title) in [
        ("view.bottom_panel.search", "Show Search Panel"),
        ("view.bottom_panel.compare", "Show Compare Panel"),
        ("view.bottom_panel.output", "Show Output Panel"),
        ("view.bottom_panel.close", "Collapse Bottom Panel"),
    ] {
        registry
            .register(bareline_commands::CommandSpec {
                id: bareline_commands::CommandId(id),
                title,
                category: "View",
                shortcut: "",
                action: bareline_commands::Action::Contributed(bareline_commands::CommandId(id)),
            })
            .expect("unique bottom panel command");
    }
}
use bareline_ui::{ViewId, rect, text};

pub(super) const TAB_LIST_ID: u64 = 80_000;
pub(super) const TAB_IDS: [u64; 3] = [80_001, 80_002, 80_003];
const PANEL_ID: u64 = 80_010;
pub(super) const CLOSE_ID: u64 = 80_011;
const HEADER_HEIGHT: f32 = 34.0;
const SPLITTER_HEIGHT: f32 = 5.0;
const MIN_HEIGHT: f32 = 140.0;
const DEFAULT_HEIGHT: f32 = 260.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DockTab {
    Search,
    Compare,
    Output,
}
impl DockTab {
    const ALL: [Self; 3] = [Self::Search, Self::Compare, Self::Output];
    fn index(self) -> usize {
        match self {
            Self::Search => 0,
            Self::Compare => 1,
            Self::Output => 2,
        }
    }
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Search => "Search",
            Self::Compare => "Compare",
            Self::Output => "Output",
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct DockSurfaceState {
    pub available: bool,
    pub busy: bool,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DockLayout {
    pub outer: Rect,
    pub splitter: Rect,
    pub header: Rect,
    pub body: Rect,
}

pub(super) struct DockRuntime {
    active: Option<DockTab>,
    collapsed: bool,
    height: f32,
    available: [bool; 3],
    busy: [bool; 3],
    revision: [u64; 3],
    unread: [bool; 3],
    focused: Option<u64>,
    dragging: bool,
    layout: Option<DockLayout>,
    restored: bool,
    requested: Option<DockTab>,
}
impl Default for DockRuntime {
    fn default() -> Self {
        Self {
            active: None,
            collapsed: true,
            height: DEFAULT_HEIGHT,
            available: [false; 3],
            busy: [false; 3],
            revision: [0; 3],
            unread: [false; 3],
            focused: None,
            dragging: false,
            layout: None,
            restored: false,
            requested: None,
        }
    }
}
impl DockRuntime {
    pub(super) fn annotate_context(&self, context: &mut bareline_commands::CommandContext) {
        for (tab, id) in DockTab::ALL.into_iter().zip([
            "view.bottom_panel.search",
            "view.bottom_panel.compare",
            "view.bottom_panel.output",
        ]) {
            if !self.available[tab.index()] {
                context.states.insert(
                    bareline_commands::CommandId(id),
                    bareline_commands::CommandState::disabled("The panel has no retained state"),
                );
            }
        }
        if self.active().is_none() {
            context.states.insert(
                bareline_commands::CommandId("view.bottom_panel.close"),
                bareline_commands::CommandState::disabled("The bottom panel is collapsed"),
            );
        }
    }
    pub(super) fn sync(&mut self, states: [DockSurfaceState; 3]) {
        let old_available = self.available;
        for (index, state) in states.into_iter().enumerate() {
            if state.available && self.revision[index] != state.revision {
                self.unread[index] = self.active() != Some(DockTab::ALL[index]);
            }
            self.available[index] = state.available;
            self.busy[index] = state.busy;
            self.revision[index] = state.revision;
            if !state.available {
                self.unread[index] = false;
            }
        }
        if let Some(requested) = self.requested
            && self.available[requested.index()]
        {
            self.activate(requested);
            self.requested = None;
        } else if !self.restored
            && let Some(opened) = DockTab::ALL
                .into_iter()
                .rev()
                .find(|tab| self.available[tab.index()] && !old_available[tab.index()])
        {
            self.activate(opened);
        } else if self.active.is_none_or(|tab| !self.available[tab.index()]) {
            let preserve_collapsed = self.restored && self.collapsed;
            self.active = DockTab::ALL.into_iter().find(|tab| self.available[tab.index()]);
            self.collapsed = preserve_collapsed || self.active.is_none();
            self.focused = (!self.collapsed)
                .then(|| self.active.map(|tab| TAB_IDS[tab.index()]))
                .flatten();
        }
        self.restored = false;
    }
    pub(super) fn activate(&mut self, tab: DockTab) -> bool {
        if !self.available[tab.index()] {
            return false;
        }
        self.active = Some(tab);
        self.collapsed = false;
        self.unread[tab.index()] = false;
        self.focused = Some(TAB_IDS[tab.index()]);
        true
    }
    pub(super) fn request(&mut self, tab: DockTab) {
        self.requested = Some(tab);
        if self.available[tab.index()] {
            self.activate(tab);
            self.requested = None;
        }
    }
    pub(super) fn blur_focus(&mut self) {
        self.focused = None;
    }
    pub(super) fn focus_active(&mut self) {
        self.focused = self.active().map(|tab| TAB_IDS[tab.index()]);
    }
    pub(super) fn collapse(&mut self) {
        self.collapsed = true;
        self.dragging = false;
        self.focused = None;
    }
    pub(super) fn active(&self) -> Option<DockTab> {
        (!self.collapsed).then_some(self.active).flatten()
    }
    pub(super) fn accessibility_focus(&self) -> Option<u64> {
        self.focused
    }
    pub(super) fn accessibility_action(&mut self, id: u64, invoke: bool) -> Option<DockTab> {
        if id == CLOSE_ID && invoke {
            self.collapse();
            return None;
        }
        let tab = TAB_IDS
            .iter()
            .position(|candidate| *candidate == id)
            .map(|index| DockTab::ALL[index])?;
        if !self.available[tab.index()] {
            return None;
        }
        self.focused = Some(id);
        if invoke {
            self.activate(tab);
        }
        Some(tab)
    }
    pub(super) fn occupied_height(&self, viewport_height: f32) -> f32 {
        if self.active().is_none() {
            return 0.0;
        }
        let available = (viewport_height - bareline_ui::STATUS_HEIGHT - bareline_ui::TAB_HEIGHT).max(0.0);
        let maximum = (viewport_height * 0.55).min(available);
        if maximum < MIN_HEIGHT {
            maximum
        } else {
            self.height.clamp(MIN_HEIGHT, maximum)
        }
    }
    pub(super) fn layout(&mut self, width: f32, height: f32) -> Option<DockLayout> {
        let occupied = self.occupied_height(height);
        if occupied == 0.0 {
            self.layout = None;
            return None;
        }
        let top = (height - bareline_ui::STATUS_HEIGHT - occupied).max(bareline_ui::TAB_HEIGHT);
        let outer = rect(0.0, top, width.max(0.0), occupied);
        let splitter = rect(outer.x, outer.y, outer.width, SPLITTER_HEIGHT);
        let header = rect(outer.x, outer.y + SPLITTER_HEIGHT, outer.width, HEADER_HEIGHT);
        let body = rect(
            outer.x,
            header.y + header.height,
            outer.width,
            (outer.height - SPLITTER_HEIGHT - HEADER_HEIGHT).max(0.0),
        );
        let layout = DockLayout {
            outer,
            splitter,
            header,
            body,
        };
        self.layout = Some(layout);
        Some(layout)
    }
    pub(super) fn current_layout(&self) -> Option<DockLayout> {
        self.layout
    }
    pub(super) fn pointer(&mut self, point: Point, down: bool, viewport_height: f32) -> Option<DockTab> {
        let layout = self.layout?;
        if down && layout.splitter.contains(point) {
            self.dragging = true;
            return None;
        }
        if !down {
            self.dragging = false;
        }
        if self.dragging {
            self.resize(point.y, viewport_height);
            return None;
        }
        if down {
            for tab in DockTab::ALL {
                if self.available[tab.index()] && self.tab_bounds(layout, tab).contains(point) {
                    self.activate(tab);
                    return Some(tab);
                }
            }
            if self.close_bounds(layout).contains(point) {
                self.collapse();
            }
        }
        None
    }
    pub(super) fn drag(&mut self, point: Point, viewport_height: f32) -> bool {
        if !self.dragging {
            return false;
        }
        self.resize(point.y, viewport_height);
        true
    }
    fn resize(&mut self, pointer_y: f32, viewport_height: f32) {
        let available = (viewport_height - bareline_ui::STATUS_HEIGHT - bareline_ui::TAB_HEIGHT).max(0.0);
        let maximum = (viewport_height * 0.55).min(available);
        if maximum >= MIN_HEIGHT {
            self.height = (viewport_height - bareline_ui::STATUS_HEIGHT - pointer_y).clamp(MIN_HEIGHT, maximum);
        }
    }
    pub(super) fn is_dragging(&self) -> bool {
        self.dragging
    }
    pub(super) fn focus_next_tab(&mut self, backwards: bool) -> Option<DockTab> {
        let current = self
            .focused
            .and_then(|id| TAB_IDS.iter().position(|candidate| *candidate == id))
            .or_else(|| self.active.map(DockTab::index))?;
        for step in 1..=DockTab::ALL.len() {
            let index = if backwards {
                (current + DockTab::ALL.len() - step) % DockTab::ALL.len()
            } else {
                (current + step) % DockTab::ALL.len()
            };
            if self.available[index] {
                let tab = DockTab::ALL[index];
                self.focused = Some(TAB_IDS[index]);
                return Some(tab);
            }
        }
        None
    }
    fn tab_bounds(&self, layout: DockLayout, tab: DockTab) -> Rect {
        let width = ((layout.header.width - 50.0).max(0.0) / 3.0).min(100.0);
        rect(
            layout.header.x + 8.0 + tab.index() as f32 * width,
            layout.header.y,
            width,
            HEADER_HEIGHT,
        )
    }
    fn close_bounds(&self, layout: DockLayout) -> Rect {
        rect(
            layout.header.x + layout.header.width - 34.0,
            layout.header.y + 3.0,
            28.0,
            28.0,
        )
    }
    pub(super) fn draw_chrome(&self, theme: UiTheme, ops: &mut Vec<DrawOp>) {
        let Some(layout) = self.layout else { return };
        ops.push(DrawOp::Fill(layout.outer, theme.elevated));
        ops.push(DrawOp::Fill(layout.splitter, theme.border));
        ops.push(DrawOp::Stroke(layout.outer, theme.border, 1.0));
        for tab in DockTab::ALL {
            let bounds = self.tab_bounds(layout, tab);
            let selected = self.active() == Some(tab);
            if selected {
                ops.push(DrawOp::Fill(
                    rect(bounds.x, bounds.y + bounds.height - 2.0, bounds.width, 2.0),
                    theme.focus,
                ));
            }
            let suffix = if self.busy[tab.index()] {
                " …"
            } else if self.unread[tab.index()] {
                " •"
            } else {
                ""
            };
            text(
                ops,
                bounds.x + 10.0,
                bounds.y + 9.0,
                format!("{}{suffix}", tab.name()),
                13.0,
                if selected { theme.text } else { theme.muted },
            );
            if self.focused == Some(TAB_IDS[tab.index()]) {
                ops.push(DrawOp::Stroke(bounds, theme.focus, 1.0));
            }
        }
        text(
            ops,
            layout.header.x + layout.header.width - 27.0,
            layout.header.y + 8.0,
            "×",
            14.0,
            theme.text,
        );
    }
    pub(super) fn semantics(&self) -> Vec<Semantics> {
        let Some(layout) = self.layout else { return Vec::new() };
        let group = Semantics::new(
            ViewId(TAB_LIST_ID),
            SemanticRole::Group,
            "Bottom panel tabs",
            "view.bottom_panel",
            layout.header,
            ControlState::default(),
        );
        let mut nodes = vec![group];
        for tab in DockTab::ALL {
            let mut node = Semantics::new(
                ViewId(TAB_IDS[tab.index()]),
                SemanticRole::Tab,
                tab.name(),
                match tab {
                    DockTab::Search => "view.bottom_panel.search",
                    DockTab::Compare => "view.bottom_panel.compare",
                    DockTab::Output => "view.bottom_panel.output",
                },
                self.tab_bounds(layout, tab),
                ControlState {
                    disabled: !self.available[tab.index()],
                    focused: self.focused == Some(TAB_IDS[tab.index()]),
                    ..Default::default()
                },
            )
            .action(SemanticAction::Focus);
            if self.available[tab.index()] {
                node = node.action(SemanticAction::Select).action(SemanticAction::Invoke);
            }
            node.selected = self.active() == Some(tab);
            node.value = Some(
                if self.busy[tab.index()] {
                    "Running"
                } else if self.unread[tab.index()] {
                    "Updated"
                } else {
                    "Ready"
                }
                .into(),
            );
            nodes.push(node);
        }
        let mut panel = Semantics::new(
            ViewId(PANEL_ID),
            SemanticRole::Group,
            self.active().map_or("Bottom panel", DockTab::name),
            "view.bottom_panel.active",
            layout.body,
            ControlState::default(),
        );
        panel.selected = true;
        nodes.push(panel);
        nodes.push(
            Semantics::new(
                ViewId(CLOSE_ID),
                SemanticRole::Button,
                "Collapse bottom panel",
                "view.bottom_panel.close",
                self.close_bounds(layout),
                ControlState::default(),
            )
            .action(SemanticAction::Invoke),
        );
        nodes
    }
    pub(super) fn restore(&mut self, active: Option<DockTab>, collapsed: bool, height: f32) {
        self.active = active;
        self.collapsed = collapsed;
        self.height = if height.is_finite() {
            height.clamp(MIN_HEIGHT, 10_000.0)
        } else {
            DEFAULT_HEIGHT
        };
        self.restored = true;
    }
    pub(super) fn persisted(&self) -> (Option<DockTab>, bool, f32) {
        (self.active, self.collapsed, self.height)
    }
}

impl super::Shell {
    pub(super) fn dock_dispatch(&mut self, _el: &winit::event_loop::ActiveEventLoop, id: &str) -> bool {
        let tab = match id {
            "view.bottom_panel.search" => Some(DockTab::Search),
            "view.bottom_panel.compare" => Some(DockTab::Compare),
            "view.bottom_panel.output" => Some(DockTab::Output),
            "view.bottom_panel.close" => {
                self.dock.collapse();
                self.deactivate_dock_focus();
                None
            }
            _ => return false,
        };
        if let Some(tab) = tab
            && self.dock.activate(tab)
        {
            self.activate_dock_tab(tab);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready(revision: u64) -> DockSurfaceState {
        DockSurfaceState {
            available: true,
            revision,
            ..Default::default()
        }
    }
    #[test]
    fn surfaces_share_one_bounded_region_and_switch_without_geometry_drift() {
        let mut dock = DockRuntime::default();
        dock.sync([ready(1), DockSurfaceState::default(), DockSurfaceState::default()]);
        assert_eq!(dock.active(), Some(DockTab::Search));
        let first = dock.layout(1000.0, 800.0).unwrap();
        dock.sync([ready(1), ready(1), ready(1)]);
        assert_eq!(dock.active(), Some(DockTab::Output));
        let switched = dock.layout(1000.0, 800.0).unwrap();
        assert_eq!(first.outer, switched.outer);
        assert!(dock.activate(DockTab::Search));
        assert_eq!(dock.layout(1000.0, 800.0).unwrap().outer, first.outer);
    }
    #[test]
    fn collapse_resize_restore_and_semantics_keep_one_selected_panel() {
        let mut dock = DockRuntime::default();
        dock.sync([ready(1), ready(1), ready(1)]);
        let layout = dock.layout(500.0, 400.0).unwrap();
        dock.pointer(
            Point {
                x: 10.0,
                y: layout.splitter.y + 1.0,
            },
            true,
            400.0,
        );
        assert!(dock.drag(Point { x: 10.0, y: 100.0 }, 400.0));
        dock.pointer(Point::default(), false, 400.0);
        let (_, _, height) = dock.persisted();
        assert!((MIN_HEIGHT..=220.0).contains(&height));
        let nodes = dock.semantics();
        assert_eq!(
            nodes
                .iter()
                .filter(|node| node.role == SemanticRole::Tab && node.selected)
                .count(),
            1
        );
        dock.collapse();
        assert_eq!(dock.occupied_height(400.0), 0.0);
        dock.restore(Some(DockTab::Compare), false, height);
        dock.sync([ready(1), ready(1), ready(1)]);
        assert_eq!(dock.active(), Some(DockTab::Compare));
    }
    #[test]
    fn inactive_completion_is_unread_until_selected_and_removed_tabs_retire_focus() {
        let mut dock = DockRuntime::default();
        dock.sync([ready(1), ready(1), DockSurfaceState::default()]);
        dock.activate(DockTab::Search);
        dock.sync([ready(1), ready(2), DockSurfaceState::default()]);
        assert!(dock.unread[DockTab::Compare.index()]);
        assert!(dock.activate(DockTab::Compare));
        assert!(!dock.unread[DockTab::Compare.index()]);
        dock.sync([ready(1), DockSurfaceState::default(), DockSurfaceState::default()]);
        assert_eq!(dock.active(), Some(DockTab::Search));
        assert_ne!(dock.accessibility_focus(), Some(TAB_IDS[DockTab::Compare.index()]));

        dock.collapse();
        dock.sync([ready(2), DockSurfaceState::default(), DockSurfaceState::default()]);
        assert!(dock.unread[DockTab::Search.index()]);
        dock.request(DockTab::Search);
        assert_eq!(dock.active(), Some(DockTab::Search));
        assert!(!dock.unread[DockTab::Search.index()]);
    }
    #[test]
    fn tiny_viewport_bounds_panel_and_exposes_stable_disabled_tabs() {
        let mut dock = DockRuntime::default();
        dock.sync([ready(1), DockSurfaceState::default(), DockSurfaceState::default()]);
        let layout = dock.layout(240.0, 120.0).unwrap();
        assert!(layout.outer.y + layout.outer.height <= 120.0 - bareline_ui::STATUS_HEIGHT);
        let tabs: Vec<_> = dock
            .semantics()
            .into_iter()
            .filter(|node| node.role == SemanticRole::Tab)
            .collect();
        assert_eq!(tabs.len(), 3);
        assert_eq!(tabs.iter().filter(|node| node.selected).count(), 1);
        assert_eq!(tabs.iter().filter(|node| node.disabled).count(), 2);

        dock.restore(Some(DockTab::Output), true, 300.0);
        dock.sync([ready(2), DockSurfaceState::default(), DockSurfaceState::default()]);
        assert_eq!(dock.active(), None);
        assert_eq!(dock.persisted().0, Some(DockTab::Search));
    }
    #[test]
    fn tiny_resize_preserves_a_session_valid_preferred_height() {
        let mut dock = DockRuntime::default();
        dock.sync([ready(1), DockSurfaceState::default(), DockSurfaceState::default()]);
        let layout = dock.layout(240.0, 120.0).unwrap();
        dock.pointer(
            Point {
                x: layout.splitter.x + 1.0,
                y: layout.splitter.y + 1.0,
            },
            true,
            120.0,
        );
        dock.drag(Point { x: 1.0, y: 110.0 }, 120.0);
        let (_, _, preferred) = dock.persisted();
        assert!(preferred >= MIN_HEIGHT);
        let mut manifest = bareline_file_io::session::SessionManifest::default();
        manifest.layout.bottom_panel = Some("search".into());
        manifest.layout.bottom_panel_collapsed = false;
        manifest.layout.bottom_panel_height_bits = preferred.to_bits();
        assert!(manifest.validate().is_ok());
    }
    #[test]
    fn retained_surface_requests_switch_and_reopen_the_requested_tab() {
        let mut dock = DockRuntime::default();
        dock.sync([ready(1), DockSurfaceState::default(), ready(1)]);
        assert!(dock.activate(DockTab::Output));
        dock.request(DockTab::Search);
        assert_eq!(dock.active(), Some(DockTab::Search));
        dock.collapse();
        dock.request(DockTab::Output);
        assert_eq!(dock.active(), Some(DockTab::Output));
    }
}
