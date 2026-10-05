// SPDX-License-Identifier: MPL-2.0
use crate::controls::{ControlAction, ControlState, Popover, UiEvent};
use crate::widgets::{SemanticRole, Semantics, Theme};
use crate::{ViewId, rect, text};
use bareline_renderer::{DrawOp, Rect};

/// Sticky panels close through their owner's explicit command only.
pub struct AnchoredPanel {
    pub popover: Popover,
    pub sticky: bool,
}
impl AnchoredPanel {
    pub fn event(&mut self, event: UiEvent) -> Option<ControlAction> {
        if self.sticky { None } else { self.popover.event(event) }
    }
    pub fn dismiss(&mut self) -> Option<ControlAction> {
        if !self.popover.open {
            return None;
        }
        self.popover.open = false;
        Some(ControlAction::Dismissed(self.popover.anchor))
    }
    pub fn paint(&self, theme: Theme, ops: &mut Vec<DrawOp>) {
        if self.popover.open {
            ops.push(DrawOp::FillRounded(self.popover.bounds, theme.surface, 6.0));
            ops.push(DrawOp::StrokeRounded(self.popover.bounds, theme.border, 6.0, 1.0));
        }
    }
    pub fn semantics(&self, id: ViewId, name: &str) -> Option<Semantics> {
        self.popover.open.then(|| {
            Semantics::new(
                id,
                SemanticRole::Group,
                name,
                "",
                self.popover.bounds,
                ControlState::default(),
            )
        })
    }
}
/// The owner schedules a single wakeup at `show_at`; there is no animation loop.
#[derive(Default)]
pub struct Tooltip {
    show_at: Option<u64>,
    pub delay_ms: u64,
}
impl Tooltip {
    pub fn hover(&mut self, inside: bool, now_ms: u64) {
        if !inside {
            self.show_at = None;
        } else if self.show_at.is_none() {
            self.show_at = Some(now_ms.saturating_add(self.delay_ms.max(500)));
        }
    }
    pub fn dismiss(&mut self) {
        self.show_at = None;
    }
    pub fn deadline(&self) -> Option<u64> {
        self.show_at
    }
    pub fn visible(&self, now_ms: u64) -> bool {
        self.show_at.is_some_and(|at| now_ms >= at)
    }
    pub fn paint(&self, now_ms: u64, bounds: Rect, label: &str, theme: Theme, ops: &mut Vec<DrawOp>) {
        if !self.visible(now_ms) {
            return;
        }
        ops.push(DrawOp::FillRounded(bounds, theme.surface, 4.0));
        ops.push(DrawOp::StrokeRounded(bounds, theme.border, 4.0, 1.0));
        ops.push(DrawOp::PushClip(bounds));
        for (line, label) in label.lines().enumerate() {
            text(
                ops,
                bounds.x + 8.0,
                bounds.y + 6.0 + line as f32 * 18.0,
                label,
                13.0,
                theme.text,
            );
        }
        ops.push(DrawOp::PopClip);
    }
    pub fn semantics(&self, now_ms: u64, id: ViewId, name: &str, bounds: Rect) -> Option<Semantics> {
        self.visible(now_ms)
            .then(|| Semantics::new(id, SemanticRole::Tooltip, name, "", bounds, ControlState::default()))
    }
}

/// Severity is conveyed by text as well as the owner's semantic theme color.
pub struct Banner {
    pub id: ViewId,
    pub bounds: Rect,
    pub open: bool,
    pub label: String,
    close: crate::controls::Button,
}
impl Banner {
    pub fn new(id: ViewId, bounds: Rect, label: String, close_id: ViewId) -> Self {
        Self {
            id,
            bounds,
            open: true,
            label,
            close: crate::controls::Button {
                id: close_id,
                label: "×".into(),
                bounds: rect(bounds.x + bounds.width - 28.0, bounds.y, 28.0, bounds.height),
                toggle: false,
                state: ControlState::default(),
            },
        }
    }
    pub fn event(&mut self, event: UiEvent) -> Option<ControlAction> {
        if !self.open {
            return None;
        }
        self.close.bounds = rect(
            self.bounds.x + self.bounds.width - 28.0,
            self.bounds.y,
            28.0,
            self.bounds.height,
        );
        if self.close.event(event) == Some(ControlAction::Activated) {
            self.open = false;
            Some(ControlAction::Dismissed(self.id))
        } else {
            None
        }
    }
    pub fn paint(&self, theme: Theme, ops: &mut Vec<DrawOp>) {
        if !self.open {
            return;
        }
        ops.push(DrawOp::Fill(self.bounds, theme.surface));
        ops.push(DrawOp::Stroke(self.bounds, theme.border, 1.0));
        ops.push(DrawOp::PushClip(rect(
            self.bounds.x,
            self.bounds.y,
            (self.bounds.width - 32.0).max(0.0),
            self.bounds.height,
        )));
        text(
            ops,
            self.bounds.x + 10.0,
            self.bounds.y + 6.0,
            &self.label,
            13.0,
            theme.text,
        );
        ops.push(DrawOp::PopClip);
        let close_bounds = rect(
            self.bounds.x + self.bounds.width - 28.0,
            self.bounds.y,
            28.0,
            self.bounds.height,
        );
        text(ops, close_bounds.x + 8.0, close_bounds.y + 6.0, "×", 13.0, theme.muted);
        if self.close.state.focused {
            ops.push(DrawOp::Stroke(close_bounds, theme.focus, 2.0));
        }
    }
    pub fn semantics(&self, name: &str, close_name: &str, close_command: &str) -> Vec<Semantics> {
        if !self.open {
            return Vec::new();
        }
        vec![
            Semantics::new(
                self.id,
                SemanticRole::Alert,
                name,
                "",
                self.bounds,
                ControlState::default(),
            ),
            self.close.semantics(close_name, close_command),
        ]
    }
}
