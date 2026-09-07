// SPDX-License-Identifier: MPL-2.0
//! Reusable logical-pixel primitives. Owners hold product data; controls emit actions.
use crate::{TAB_HEIGHT, ViewId, rect, text};
use bareline_renderer::{DrawOp, Point, Rect};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default)]
pub struct ControlState {
    pub disabled: bool,
    pub focused: bool,
    pub hovered: bool,
    pub pressed: bool,
    pub checked: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Enter,
    Space,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Tab,
    Home,
    End,
}
#[derive(Clone, Copy, Debug)]
pub enum UiEvent {
    PointerMove(Point),
    PointerDown(Point),
    PointerUp(Point),
    Key(Key),
    Focus(bool),
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControlAction {
    Activated,
    Checked(bool),
    Selected(usize),
    ScrollTo(f64),
    Dismissed(ViewId),
}
#[derive(Clone, Copy, Debug)]
pub enum Role {
    Button,
    Checkbox,
    Tab,
    Scrollbar,
    Group,
}
pub struct SemanticNode {
    pub id: ViewId,
    pub role: Role,
    pub name: String,
    pub bounds: Rect,
    pub disabled: bool,
    pub focused: bool,
    pub checked: bool,
}

pub struct Button {
    pub id: ViewId,
    pub label: String,
    pub bounds: Rect,
    pub toggle: bool,
    pub state: ControlState,
}
impl Button {
    pub fn event(&mut self, event: UiEvent) -> Option<ControlAction> {
        if let UiEvent::Focus(focused) = event {
            self.state.focused = focused && !self.state.disabled;
            if !self.state.focused {
                self.state.pressed = false;
            }
            return None;
        }
        if self.state.disabled {
            self.state.pressed = false;
            return None;
        }
        let activate = match event {
            UiEvent::PointerMove(p) => {
                self.state.hovered = self.bounds.contains(p);
                false
            }
            UiEvent::PointerDown(p) => {
                self.state.pressed = self.bounds.contains(p);
                false
            }
            UiEvent::PointerUp(p) => {
                std::mem::take(&mut self.state.pressed) && self.bounds.contains(p)
            }
            UiEvent::Key(Key::Enter | Key::Space) => self.state.focused,
            UiEvent::Focus(focused) => {
                self.state.focused = focused;
                if !focused {
                    self.state.pressed = false;
                }
                false
            }
            _ => false,
        };
        if !activate {
            return None;
        }
        if self.toggle {
            self.state.checked = !self.state.checked;
            Some(ControlAction::Checked(self.state.checked))
        } else {
            Some(ControlAction::Activated)
        }
    }
    pub fn paint(&self, ops: &mut Vec<DrawOp>) {
        self.paint_with_theme(crate::theme::UiTheme::default(), ops);
    }
    pub fn paint_with_theme(&self, theme: crate::theme::UiTheme, ops: &mut Vec<DrawOp>) {
        let checked = self.toggle && self.state.checked && !self.state.disabled;
        ops.push(DrawOp::FillRounded(
            self.bounds,
            if checked {
                theme.selection
            } else if self.state.pressed {
                theme.interactive
            } else {
                theme.elevated
            },
            4.0,
        ));
        ops.push(DrawOp::StrokeRounded(
            self.bounds,
            if self.state.focused {
                theme.focus
            } else {
                theme.interactive
            },
            4.0,
            if self.state.focused { 2.0 } else { 1.0 },
        ));
        text(
            ops,
            self.bounds.x + 10.0,
            self.bounds.y + 6.0,
            &self.label,
            13.0,
            if self.state.disabled {
                theme.muted
            } else {
                theme.text
            },
        );
    }
    pub fn semantic(&self) -> SemanticNode {
        SemanticNode {
            id: self.id,
            role: if self.toggle {
                Role::Checkbox
            } else {
                Role::Button
            },
            name: self.label.clone(),
            bounds: self.bounds,
            disabled: self.state.disabled,
            focused: self.state.focused,
            checked: self.state.checked,
        }
    }
}

pub struct TabStrip {
    pub width: f32,
    pub count: usize,
    pub active: usize,
}
impl TabStrip {
    pub const TAB_WIDTH: f32 = 150.0;
    pub fn visible(&self) -> Range<usize> {
        let count = (self.width / Self::TAB_WIDTH).floor().max(1.0) as usize;
        let start = self
            .active
            .min(self.count.saturating_sub(1))
            .saturating_sub(count - 1);
        start..start.saturating_add(count).min(self.count)
    }
    pub fn bounds(&self, index: usize) -> Option<Rect> {
        let visible = self.visible();
        visible.contains(&index).then(|| {
            rect(
                (index - visible.start) as f32 * Self::TAB_WIDTH,
                0.0,
                Self::TAB_WIDTH,
                TAB_HEIGHT,
            )
        })
    }
    pub fn hit_test(&self, p: Point) -> Option<usize> {
        if p.x < 0.0 || p.x >= self.width || p.y < 0.0 || p.y >= TAB_HEIGHT {
            return None;
        }
        let index = self.visible().start + (p.x / Self::TAB_WIDTH) as usize;
        self.visible().contains(&index).then_some(index)
    }
    pub fn navigate(&self, key: Key) -> Option<ControlAction> {
        if self.count == 0 {
            return None;
        }
        let next = match key {
            Key::Left => self.active.saturating_sub(1),
            Key::Right => (self.active + 1).min(self.count - 1),
            Key::Home => 0,
            Key::End => self.count - 1,
            _ => return None,
        };
        Some(ControlAction::Selected(next))
    }
}

pub struct Scrollbar {
    pub bounds: Rect,
    pub offset: f64,
    pub viewport: f64,
    pub total: Option<f64>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollAction {
    Preview(f64),
    Commit(f64),
}
/// Keeps drag capture separate from the backwards-compatible scrollbar props.
#[derive(Default)]
pub struct ScrollbarInteraction {
    grab: Option<f32>,
    drag_start: Option<f64>,
    pub deferred: bool,
}
impl ScrollbarInteraction {
    pub fn event(
        &mut self,
        scrollbar: &mut Scrollbar,
        event: UiEvent,
        enabled: bool,
        focused: bool,
        step: f64,
    ) -> Option<ScrollAction> {
        if !enabled || matches!(event, UiEvent::Focus(false)) {
            self.grab = None;
            if let Some(start) = self.drag_start.take()
                && self.deferred
            {
                scrollbar.offset = start;
            }
            return None;
        }
        let value = match event {
            UiEvent::PointerDown(p) if scrollbar.bounds.contains(p) => {
                let thumb = scrollbar.thumb();
                if thumb.contains(p) {
                    self.grab = Some(p.y - thumb.y);
                    self.drag_start = Some(scrollbar.offset);
                    return None;
                }
                Some(
                    (scrollbar.offset
                        + if p.y < thumb.y {
                            -scrollbar.viewport
                        } else {
                            scrollbar.viewport
                        })
                    .clamp(0.0, scrollbar.maximum()),
                )
            }
            UiEvent::PointerMove(p) | UiEvent::PointerUp(p) if self.grab.is_some() => {
                let ControlAction::ScrollTo(value) = scrollbar.drag(p.y - self.grab.unwrap())
                else {
                    return None;
                };
                scrollbar.offset = value;
                if matches!(event, UiEvent::PointerUp(_)) {
                    self.grab = None;
                    self.drag_start = None;
                    return Some(ScrollAction::Commit(value));
                }
                return Some(if self.deferred {
                    ScrollAction::Preview(value)
                } else {
                    ScrollAction::Commit(value)
                });
            }
            UiEvent::Key(key) if focused => match key {
                Key::Up | Key::Left => Some((scrollbar.offset - step.max(0.0)).max(0.0)),
                Key::Down | Key::Right => {
                    Some((scrollbar.offset + step.max(0.0)).min(scrollbar.maximum()))
                }
                Key::Home => Some(0.0),
                Key::End => Some(scrollbar.maximum()),
                _ => None,
            },
            _ => None,
        }?;
        if !value.is_finite() || value == scrollbar.offset {
            return None;
        }
        scrollbar.offset = value;
        Some(ScrollAction::Commit(value))
    }
    /// Incorporate a newly discovered extent without moving/growing the thumb
    /// by more than its old height. False requests another owner-driven update.
    pub fn refine_total(&self, scrollbar: &mut Scrollbar, target: f64) -> bool {
        if !target.is_finite() || target < 0.0 {
            return true;
        }
        let old = scrollbar.thumb();
        let prior = scrollbar
            .total
            .unwrap_or(scrollbar.maximum() + scrollbar.viewport);
        let target = target.max(scrollbar.viewport).max(0.0);
        let original = scrollbar.total;
        scrollbar.total = Some(target);
        let next = scrollbar.thumb();
        let within = |thumb: Rect| {
            (thumb.y - old.y).abs() <= old.height && (thumb.height - old.height).abs() <= old.height
        };
        if within(next) {
            return true;
        }
        let mut low = 0.0;
        let mut high = 1.0;
        for _ in 0..40 {
            let fraction = (low + high) / 2.0;
            scrollbar.total = Some(prior + (target - prior) * fraction);
            if within(scrollbar.thumb()) {
                low = fraction;
            } else {
                high = fraction;
            }
        }
        scrollbar.total = if low > 0.0 {
            Some(prior + (target - prior) * low)
        } else {
            original
        };
        false
    }
}
impl Scrollbar {
    pub fn maximum(&self) -> f64 {
        self.total
            .map_or(self.offset + self.viewport.max(1.0) * 4.0, |n| {
                (n - self.viewport).max(0.0)
            })
    }
    pub fn thumb(&self) -> Rect {
        let extent = self
            .total
            .unwrap_or(self.maximum() + self.viewport)
            .max(self.viewport)
            .max(1.0);
        let height = (self.bounds.height * (self.viewport / extent) as f32)
            .clamp(18.0f32.min(self.bounds.height), self.bounds.height);
        let travel = self.bounds.height - height;
        let y = self.bounds.y
            + if self.maximum() > 0.0 {
                (self.offset / self.maximum()).clamp(0.0, 1.0) as f32 * travel
            } else {
                0.0
            };
        rect(
            self.bounds.x + 2.0,
            y,
            (self.bounds.width - 4.0).max(1.0),
            height,
        )
    }
    pub fn drag(&self, thumb_top: f32) -> ControlAction {
        let travel = self.bounds.height - self.thumb().height;
        let fraction = if travel > 0.0 {
            ((thumb_top - self.bounds.y) / travel).clamp(0.0, 1.0) as f64
        } else {
            0.0
        };
        ControlAction::ScrollTo(fraction * self.maximum())
    }
    pub fn paint(&self, ops: &mut Vec<DrawOp>) {
        self.paint_with_theme(crate::theme::UiTheme::default(), ops);
    }
    pub fn paint_with_theme(&self, theme: crate::theme::UiTheme, ops: &mut Vec<DrawOp>) {
        ops.push(DrawOp::Fill(self.thumb(), theme.interactive));
    }
}

pub fn visible_rows(
    offset: f64,
    viewport: f64,
    row_height: f64,
    total: Option<usize>,
    overscan: usize,
) -> Range<usize> {
    if row_height <= 0.0 || !row_height.is_finite() || !offset.is_finite() || !viewport.is_finite()
    {
        return 0..0;
    }
    let first = (offset.max(0.0) / row_height).floor() as usize;
    let end = ((offset.max(0.0) + viewport.max(0.0)) / row_height).ceil() as usize;
    let end = end
        .saturating_add(overscan)
        .min(total.unwrap_or(usize::MAX));
    first.saturating_sub(overscan).min(end)..end
}
pub struct Popover {
    pub anchor: ViewId,
    pub bounds: Rect,
    pub open: bool,
}
impl Popover {
    pub fn place(anchor: ViewId, rectangle: Rect, width: f32, height: f32, viewport: Rect) -> Self {
        let width = width.min(viewport.width).max(0.0);
        let height = height.min(viewport.height).max(0.0);
        let below = rectangle.y + rectangle.height;
        let y = if below + height <= viewport.y + viewport.height {
            below
        } else {
            rectangle.y - height
        };
        Self {
            anchor,
            bounds: rect(
                rectangle
                    .x
                    .clamp(viewport.x, viewport.x + viewport.width - width),
                y.clamp(viewport.y, viewport.y + viewport.height - height),
                width,
                height,
            ),
            open: true,
        }
    }
    pub fn event(&mut self, event: UiEvent) -> Option<ControlAction> {
        let dismiss = matches!(event, UiEvent::Key(Key::Escape) | UiEvent::Focus(false))
            || matches!(event, UiEvent::PointerDown(p) if !self.bounds.contains(p));
        if self.open && dismiss {
            self.open = false;
            Some(ControlAction::Dismissed(self.anchor))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_scroll_includes_bottom_row_and_disabled_focus_clears() {
        assert_eq!(visible_rows(14.0, 56.0, 28.0, Some(100), 0), 0..3);
        let mut button = Button {
            id: ViewId(1),
            label: "Apply".into(),
            bounds: rect(0.0, 0.0, 80.0, 28.0),
            toggle: false,
            state: ControlState {
                disabled: true,
                focused: true,
                pressed: true,
                ..ControlState::default()
            },
        };
        button.event(UiEvent::Focus(false));
        assert!(!button.state.focused);
        assert!(!button.state.pressed);
    }
    #[test]
    fn virtual_controls_stay_bounded_and_active_tab_is_visible() {
        assert_eq!(
            visible_rows(28.0 * 500_000.0, 560.0, 28.0, Some(1_000_000), 2).len(),
            24
        );
        let tabs = TabStrip {
            width: 1200.0,
            count: 5000,
            active: 4999,
        };
        assert_eq!(tabs.visible().len(), 8);
        let bounds = tabs.bounds(4999).unwrap();
        assert_eq!(
            tabs.hit_test(Point {
                x: bounds.x + 10.0,
                y: 10.0
            }),
            Some(4999)
        );
        let scroll = Scrollbar {
            bounds: rect(0.0, 0.0, 12.0, 500.0),
            offset: 5e12,
            viewport: 30.0,
            total: Some(1e13),
        };
        let thumb = scroll.thumb();
        assert!(thumb.y > 200.0 && thumb.y < 300.0);
        assert_eq!(thumb.height, 18.0);
    }
    #[test]
    fn disabled_activation_and_popup_focus_return() {
        let mut button = Button {
            id: ViewId(1),
            label: "Apply".into(),
            bounds: rect(0.0, 0.0, 90.0, 28.0),
            toggle: false,
            state: ControlState {
                disabled: true,
                focused: true,
                ..ControlState::default()
            },
        };
        assert_eq!(button.event(UiEvent::Key(Key::Enter)), None);
        button.state.disabled = false;
        assert_eq!(
            button.event(UiEvent::Key(Key::Enter)),
            Some(ControlAction::Activated)
        );
        let mut popover = Popover::place(
            ViewId(1),
            rect(90.0, 95.0, 10.0, 5.0),
            60.0,
            40.0,
            rect(0.0, 0.0, 100.0, 100.0),
        );
        assert_eq!(popover.bounds, rect(40.0, 55.0, 60.0, 40.0));
        assert_eq!(
            popover.event(UiEvent::Key(Key::Escape)),
            Some(ControlAction::Dismissed(ViewId(1)))
        );
    }
}
