// SPDX-License-Identifier: MPL-2.0
pub mod controls;
pub mod focus;
pub mod overlays;
pub mod text_field;
pub mod variable_list;
pub mod virtual_tree;
pub mod widgets;
use bareline_renderer::{Color, DrawOp, Point, Rect};

pub const EDITOR: Color = Color(0x1F2328);
pub const CHROME: Color = Color(0x181B1F);
pub const ELEVATED: Color = Color(0x262B31);
pub const BORDER: Color = Color(0x343A42);
pub const TEXT: Color = Color(0xE6E8EA);
pub const MUTED: Color = Color(0x9AA3AD);
pub const ACCENT: Color = Color(0x2ED3C4);
pub const TAB_HEIGHT: f32 = 34.0;
pub const STATUS_HEIGHT: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewId(pub u64);
pub struct LayoutNode {
    pub id: ViewId,
    pub bounds: Rect,
    pub focusable: bool,
}
#[derive(Default)]
pub struct ViewTree {
    pub nodes: Vec<LayoutNode>,
    pub focus: Option<ViewId>,
    pub invalidated: bool,
}
impl ViewTree {
    pub fn hit_test(&self, point: Point) -> Option<ViewId> {
        self.nodes
            .iter()
            .rev()
            .find(|n| n.bounds.contains(point))
            .map(|n| n.id)
    }
    pub fn focus_next(&mut self, backwards: bool) {
        let ids: Vec<_> = self
            .nodes
            .iter()
            .filter(|n| n.focusable)
            .map(|n| n.id)
            .collect();
        if ids.is_empty() {
            self.focus = None;
            return;
        }
        let next = match ids.iter().position(|id| Some(*id) == self.focus) {
            Some(i) if backwards => (i + ids.len() - 1) % ids.len(),
            Some(i) => (i + 1) % ids.len(),
            None if backwards => ids.len() - 1,
            None => 0,
        };
        self.focus = Some(ids[next]);
        self.invalidated = true;
    }
}
pub fn text(
    ops: &mut Vec<DrawOp>,
    x: f32,
    y: f32,
    value: impl Into<String>,
    size: f32,
    color: Color,
) {
    ops.push(DrawOp::Text {
        origin: Point { x, y },
        text: value.into(),
        size,
        color,
    });
}
pub fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect {
        x,
        y,
        width: width.max(0.0),
        height: height.max(0.0),
    }
}

/// Initial shell composition uses the reference's tab/editor/status hierarchy.
/// Documents and concrete controls are owned by subsequent PRs.
pub fn shell(
    width: f32,
    height: f32,
    tabs: &[String],
    active: usize,
    palette: bool,
) -> Vec<DrawOp> {
    shell_with_theme(
        width,
        height,
        tabs,
        active,
        palette,
        theme::UiTheme::default(),
    )
}
pub fn palette(width: f32, ops: &mut Vec<DrawOp>) {
    palette_with_theme(width, theme::UiTheme::default(), ops);
}
pub fn shell_with_theme(
    width: f32,
    height: f32,
    tabs: &[String],
    active: usize,
    palette: bool,
    theme: theme::UiTheme,
) -> Vec<DrawOp> {
    let mut ops = vec![
        DrawOp::Fill(rect(0.0, 0.0, width, height), theme.editor),
        DrawOp::Fill(rect(0.0, 0.0, width, TAB_HEIGHT), theme.chrome),
    ];
    ops.push(DrawOp::PushClip(rect(0.0, 0.0, width, TAB_HEIGHT)));
    let strip = controls::TabStrip {
        width,
        count: tabs.len(),
        active,
    };
    let visible = strip.visible();
    for (index, title) in tabs
        .iter()
        .enumerate()
        .skip(visible.start)
        .take(visible.len())
    {
        let x = strip.bounds(index).unwrap().x;
        ops.push(DrawOp::Fill(
            rect(x, 0.0, 150.0, TAB_HEIGHT),
            if index == active {
                theme.editor
            } else {
                theme.chrome
            },
        ));
        ops.push(DrawOp::Stroke(
            rect(x, 0.0, 150.0, TAB_HEIGHT),
            theme.border,
            1.0,
        ));
        text(
            &mut ops,
            x + 16.0,
            8.0,
            title,
            13.0,
            if index == active {
                theme.text
            } else {
                theme.muted
            },
        );
        if index == active {
            ops.push(DrawOp::Fill(rect(x, 32.0, 150.0, 2.0), theme.focus));
        }
    }
    ops.push(DrawOp::PopClip);
    let status_y = (height - STATUS_HEIGHT).max(TAB_HEIGHT);
    ops.push(DrawOp::PushClip(rect(
        0.0,
        TAB_HEIGHT,
        width,
        status_y - TAB_HEIGHT,
    )));
    if !tabs.is_empty() {
        ops.push(DrawOp::Fill(
            rect(48.0, TAB_HEIGHT + 4.0, width - 48.0, 24.0),
            theme.elevated,
        ));
        text(&mut ops, 26.0, TAB_HEIGHT + 4.0, "1", 16.0, theme.muted);
        ops.push(DrawOp::Fill(
            rect(63.0, TAB_HEIGHT + 6.0, 1.5, 20.0),
            theme.focus,
        ));
    }
    ops.push(DrawOp::Fill(
        rect(48.0, TAB_HEIGHT, 1.0, status_y - TAB_HEIGHT),
        theme.border,
    ));
    text(
        &mut ops,
        (width - 234.0).max(64.0),
        (status_y - 28.0).max(TAB_HEIGHT),
        "Ctrl+Shift+P for commands",
        13.0,
        theme.muted,
    );
    ops.push(DrawOp::PopClip);
    ops.push(DrawOp::Fill(
        rect(0.0, status_y, width, STATUS_HEIGHT),
        theme.chrome,
    ));
    ops.push(DrawOp::Fill(rect(0.0, status_y, width, 1.0), theme.border));
    for (x, label) in [
        (16.0, "Plain text"),
        (130.0, "0 B · 1 line"),
        ((width - 420.0).max(280.0), "Ln 1, Col 1"),
        ((width - 240.0).max(410.0), "LF"),
        ((width - 155.0).max(460.0), "UTF-8"),
        ((width - 50.0).max(540.0), "INS"),
    ] {
        text(
            &mut ops,
            x,
            status_y + 4.0,
            if tabs.is_empty() { "—" } else { label },
            13.0,
            theme.muted,
        );
    }
    if palette {
        crate::palette_with_theme(width, theme, &mut ops);
    }
    ops
}
pub fn palette_with_theme(width: f32, theme: theme::UiTheme, ops: &mut Vec<DrawOp>) {
    let pw = (width - 48.0).clamp(200.0, 620.0);
    let x = (width - pw) / 2.0;
    ops.push(DrawOp::Fill(rect(x, 60.0, pw, 64.0), theme.elevated));
    ops.push(DrawOp::Stroke(rect(x, 60.0, pw, 64.0), theme.focus, 2.0));
    text(
        ops,
        x + 18.0,
        80.0,
        "Command palette · available in PR-011",
        13.0,
        theme.text,
    );
}

pub mod semantics;
pub mod theme;
