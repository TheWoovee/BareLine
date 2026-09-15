// SPDX-License-Identifier: MPL-2.0
//! Owner-composed controls: no I/O or product data, only bounded state and actions.
use crate::controls::{ControlAction, ControlState, Key, UiEvent, visible_rows};
use crate::{ACCENT, BORDER, ELEVATED, MUTED, TEXT, ViewId, rect, text};
use bareline_renderer::{Color, DrawOp, Point, Rect};

/// Semantic colors supplied by the theme owner; defaults preserve existing chrome.
#[derive(Clone, Copy)]
pub struct Theme {
    pub surface: Color,
    pub text: Color,
    pub muted: Color,
    pub selection: Color,
    pub border: Color,
    pub focus: Color,
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            surface: ELEVATED,
            text: TEXT,
            muted: MUTED,
            selection: BORDER,
            border: BORDER,
            focus: ACCENT,
        }
    }
}
#[derive(Clone, Copy)]
pub struct Metrics {
    pub row_height: f32,
    pub padding: f32,
    pub font_size: f32,
}
impl Metrics {
    pub const COMPACT: Self = Self {
        row_height: 28.0,
        padding: 10.0,
        font_size: 13.0,
    };
    pub const COMFORTABLE: Self = Self {
        row_height: 36.0,
        padding: 12.0,
        font_size: 13.0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticRole {
    Tree,
    TreeItem,
    Button,
    Checkbox,
    Radio,
    Combo,
    List,
    ListItem,
    TextField,
    Slider,
    SpinButton,
    Separator,
    Tooltip,
    Alert,
    Tab,
    Scrollbar,
    Group,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticAction {
    Focus,
    Invoke,
    Toggle,
    Select,
    SetValue,
    Expand,
    Collapse,
    Scroll,
}
/// Explicit localized name and stable command ID are supplied by the owner.
/// This additive record leaves the original controls::SemanticNode API intact.
pub struct Semantics {
    pub id: ViewId,
    pub role: SemanticRole,
    pub name: String,
    pub command_id: String,
    pub value: Option<String>,
    pub bounds: Rect,
    pub disabled: bool,
    pub focused: bool,
    pub selected: bool,
    pub expanded: Option<bool>,
    pub invalid: Option<String>,
    pub actions: Vec<SemanticAction>,
}
impl Semantics {
    pub fn new(
        id: ViewId,
        role: SemanticRole,
        name: &str,
        command_id: &str,
        bounds: Rect,
        state: ControlState,
    ) -> Self {
        Self {
            id,
            role,
            name: name.into(),
            command_id: command_id.into(),
            value: None,
            bounds,
            disabled: state.disabled,
            focused: state.focused,
            selected: state.checked,
            expanded: None,
            invalid: None,
            actions: Vec::new(),
        }
    }
    pub fn action(mut self, action: SemanticAction) -> Self {
        if !self.disabled {
            self.actions.push(action);
        }
        self
    }
}

/// Uniform-height sources are queried only for visible rows. Unknown totals are
/// represented separately from the currently discovered prefix.
pub trait ItemSource {
    fn len(&self) -> Option<usize>;
    fn discovered(&self) -> usize;
    fn label(&self, index: usize) -> &str;
    fn enabled(&self, _index: usize) -> bool {
        true
    }
    fn is_empty(&self) -> bool {
        self.len() == Some(0) || self.discovered() == 0
    }
}
pub struct List {
    pub bounds: Rect,
    pub state: ControlState,
    pub selected: Option<usize>,
    pub offset: f64,
    pub metrics: Metrics,
}
impl List {
    fn count(&self, source: &impl ItemSource) -> usize {
        source.len().unwrap_or(source.discovered()).min(source.discovered())
    }
    pub fn visible(&self, source: &impl ItemSource) -> std::ops::Range<usize> {
        visible_rows(
            self.offset,
            self.bounds.height as f64,
            self.metrics.row_height as f64,
            Some(self.count(source)),
            1,
        )
    }
    pub fn row_bounds(&self, index: usize) -> Rect {
        rect(
            self.bounds.x,
            self.bounds.y + (index as f64 * self.metrics.row_height as f64 - self.offset) as f32,
            self.bounds.width,
            self.metrics.row_height,
        )
    }
    fn select(&mut self, index: usize) -> Option<ControlAction> {
        let top = index as f64 * self.metrics.row_height as f64;
        let bottom = top + self.metrics.row_height as f64;
        if top < self.offset {
            self.offset = top;
        } else if bottom > self.offset + self.bounds.height as f64 {
            self.offset = (bottom - self.bounds.height as f64).max(0.0);
        }
        if self.selected == Some(index) {
            return None;
        }
        self.selected = Some(index);
        Some(ControlAction::Selected(index))
    }
    pub fn event(&mut self, event: UiEvent, source: &impl ItemSource) -> Option<ControlAction> {
        if let UiEvent::Focus(focused) = event {
            self.state.focused = focused && !self.state.disabled;
            return None;
        }
        if self.state.disabled || self.metrics.row_height <= 0.0 {
            return None;
        }
        let count = self.count(source);
        if count == 0 {
            self.selected = None;
            return None;
        }
        match event {
            UiEvent::PointerDown(p) if self.bounds.contains(p) => {
                let index = ((p.y - self.bounds.y) as f64 + self.offset) / self.metrics.row_height as f64;
                let index = index.max(0.0) as usize;
                if index < count && source.enabled(index) {
                    self.select(index)
                } else {
                    None
                }
            }
            UiEvent::Key(Key::Enter | Key::Space) if self.state.focused => self
                .selected
                .filter(|i| *i < count && source.enabled(*i))
                .map(|_| ControlAction::Activated),
            UiEvent::Key(key @ (Key::Up | Key::Down | Key::Home | Key::End)) if self.state.focused => {
                let reverse = matches!(key, Key::Up | Key::End);
                let start = match key {
                    Key::Home => 0,
                    Key::End => count - 1,
                    Key::Up => self.selected.unwrap_or(count).saturating_sub(1).min(count - 1),
                    _ => self.selected.map_or(0, |i| i.saturating_add(1)).min(count - 1),
                };
                let index = if reverse {
                    (0..=start).rev().find(|i| source.enabled(*i))
                } else {
                    (start..count).find(|i| source.enabled(*i))
                };
                index.and_then(|i| self.select(i))
            }
            _ => None,
        }
    }
    pub fn paint(&self, source: &impl ItemSource, theme: Theme, ops: &mut Vec<DrawOp>) {
        ops.push(DrawOp::PushClip(self.bounds));
        for index in self.visible(source) {
            let bounds = self.row_bounds(index);
            if self.selected == Some(index) {
                ops.push(DrawOp::Fill(bounds, theme.selection));
            }
            text(
                ops,
                bounds.x + self.metrics.padding,
                bounds.y + 6.0,
                source.label(index),
                self.metrics.font_size,
                if self.state.disabled || !source.enabled(index) {
                    theme.muted
                } else {
                    theme.text
                },
            );
        }
        ops.push(DrawOp::PopClip);
        if self.state.focused {
            ops.push(DrawOp::Stroke(self.bounds, theme.focus, 2.0));
        }
    }
    pub fn semantics(&self, id: ViewId, name: &str, command: &str) -> Semantics {
        let mut node = Semantics::new(id, SemanticRole::List, name, command, self.bounds, self.state)
            .action(SemanticAction::Focus)
            .action(SemanticAction::Select);
        node.value = self.selected.map(|i| i.to_string());
        node
    }
}

/// Combo search receives committed text only, never IME preedit or shortcut keys.
pub struct Combo {
    pub list: List,
    pub open: bool,
    search: String,
    last_input_ms: Option<u64>,
    before_open: Option<usize>,
}
impl Combo {
    pub fn new(list: List) -> Self {
        Self {
            list,
            open: false,
            search: String::new(),
            last_input_ms: None,
            before_open: None,
        }
    }
    pub fn set_open(&mut self, open: bool) {
        if open && !self.open {
            self.before_open = self.list.selected;
        }
        self.open = open && !self.list.state.disabled;
        self.search.clear();
        self.last_input_ms = None;
    }
    pub fn search(&mut self, committed: &str, now_ms: u64, source: &impl ItemSource) -> Option<ControlAction> {
        if self.list.state.disabled
            || !self.list.state.focused
            || committed.is_empty()
            || committed.chars().any(char::is_control)
        {
            return None;
        }
        if self.last_input_ms.is_none_or(|last| now_ms.saturating_sub(last) > 1000) {
            self.search.clear();
        }
        if self.search.len() + committed.len() > 256 {
            return None;
        }
        self.search.push_str(&committed.to_lowercase());
        self.last_input_ms = Some(now_ms);
        // Form choices are deliberately bounded, unlike the virtual results list.
        let index = (0..self.list.count(source).min(4096))
            .find(|i| source.enabled(*i) && source.label(*i).to_lowercase().starts_with(&self.search));
        index.and_then(|i| self.list.select(i))
    }
    pub fn event(&mut self, event: UiEvent, source: &impl ItemSource) -> Option<ControlAction> {
        if self.list.state.disabled {
            self.set_open(false);
            return None;
        }
        match event {
            UiEvent::Key(Key::Escape) if self.open => {
                let previous = self.before_open;
                self.set_open(false);
                if self.list.selected != previous {
                    self.list.selected = previous;
                    return previous.map(ControlAction::Selected);
                }
                None
            }
            UiEvent::Key(Key::Enter | Key::Space) if self.list.state.focused => {
                self.set_open(!self.open);
                None
            }
            UiEvent::Focus(false) => {
                self.set_open(false);
                self.list.event(event, source)
            }
            _ => self.list.event(event, source),
        }
    }
    /// Header bounds are distinct from the owner's anchored dropdown bounds.
    pub fn paint(&self, bounds: Rect, source: &impl ItemSource, theme: Theme, ops: &mut Vec<DrawOp>) {
        ops.push(DrawOp::FillRounded(bounds, theme.surface, 4.0));
        ops.push(DrawOp::StrokeRounded(
            bounds,
            if self.list.state.focused {
                theme.focus
            } else {
                theme.border
            },
            4.0,
            if self.list.state.focused { 2.0 } else { 1.0 },
        ));
        ops.push(DrawOp::PushClip(bounds));
        if let Some(index) = self.list.selected.filter(|i| *i < self.list.count(source)) {
            text(
                ops,
                bounds.x + 10.0,
                bounds.y + 6.0,
                source.label(index),
                13.0,
                if self.list.state.disabled {
                    theme.muted
                } else {
                    theme.text
                },
            );
        }
        text(
            ops,
            bounds.x + bounds.width - 20.0,
            bounds.y + 6.0,
            "⌄",
            13.0,
            theme.muted,
        );
        ops.push(DrawOp::PopClip);
        if self.open {
            self.list.paint(source, theme, ops);
        }
    }
    pub fn semantics(
        &self,
        id: ViewId,
        name: &str,
        command: &str,
        bounds: Rect,
        source: &impl ItemSource,
    ) -> Semantics {
        let mut node = Semantics::new(id, SemanticRole::Combo, name, command, bounds, self.list.state)
            .action(SemanticAction::Focus)
            .action(if self.open {
                SemanticAction::Collapse
            } else {
                SemanticAction::Expand
            })
            .action(SemanticAction::Select);
        node.expanded = Some(self.open);
        node.value = self
            .list
            .selected
            .filter(|i| *i < self.list.count(source))
            .map(|i| source.label(i).into());
        node
    }
}

/// Radio groups share the list's bounded navigation and disabled-item rules.
pub struct RadioGroup {
    pub list: List,
}
impl RadioGroup {
    pub fn event(&mut self, event: UiEvent, source: &impl ItemSource) -> Option<ControlAction> {
        let event = match event {
            UiEvent::Key(Key::Left) => UiEvent::Key(Key::Up),
            UiEvent::Key(Key::Right) => UiEvent::Key(Key::Down),
            other => other,
        };
        self.list.event(event, source)
    }
    pub fn paint(&self, source: &impl ItemSource, theme: Theme, ops: &mut Vec<DrawOp>) {
        ops.push(DrawOp::PushClip(self.list.bounds));
        for index in self.list.visible(source) {
            let row = self.list.row_bounds(index);
            let marker = rect(row.x + 4.0, row.y + 6.0, 16.0, 16.0);
            ops.push(DrawOp::StrokeRounded(
                marker,
                if self.list.selected == Some(index) {
                    theme.focus
                } else {
                    theme.border
                },
                8.0,
                1.0,
            ));
            if self.list.selected == Some(index) {
                ops.push(DrawOp::FillRounded(
                    rect(marker.x + 4.0, marker.y + 4.0, 8.0, 8.0),
                    if self.list.state.disabled || !source.enabled(index) {
                        theme.muted
                    } else {
                        theme.focus
                    },
                    4.0,
                ));
            }
            text(
                ops,
                row.x + 28.0,
                row.y + 6.0,
                source.label(index),
                self.list.metrics.font_size,
                if self.list.state.disabled || !source.enabled(index) {
                    theme.muted
                } else {
                    theme.text
                },
            );
        }
        ops.push(DrawOp::PopClip);
        if self.list.state.focused {
            ops.push(DrawOp::Stroke(self.list.bounds, theme.focus, 2.0));
        }
    }
    pub fn item_semantics(
        &self,
        index: usize,
        id: ViewId,
        name: &str,
        command: &str,
        source: &impl ItemSource,
    ) -> Option<Semantics> {
        if index >= self.list.count(source) {
            return None;
        }
        let mut state = self.list.state;
        state.disabled |= !source.enabled(index);
        state.checked = self.list.selected == Some(index);
        state.focused &= state.checked;
        Some(
            Semantics::new(
                id,
                SemanticRole::Radio,
                name,
                command,
                self.list.row_bounds(index),
                state,
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::Select),
        )
    }
}

pub struct Checkbox {
    pub id: ViewId,
    pub bounds: Rect,
    pub state: ControlState,
}
impl Checkbox {
    pub fn event(&mut self, event: UiEvent) -> Option<ControlAction> {
        let mut button = crate::controls::Button {
            id: self.id,
            bounds: self.bounds,
            state: self.state,
            label: String::new(),
            toggle: true,
        };
        let action = button.event(event);
        self.state = button.state;
        action
    }
    pub fn paint(&self, label: &str, theme: Theme, ops: &mut Vec<DrawOp>) {
        let marker = rect(
            self.bounds.x + 4.0,
            self.bounds.y + (self.bounds.height - 16.0) / 2.0,
            16.0,
            16.0,
        );
        ops.push(DrawOp::StrokeRounded(
            marker,
            if self.state.checked { theme.focus } else { theme.border },
            3.0,
            1.0,
        ));
        ops.push(DrawOp::PushClip(self.bounds));
        if self.state.checked {
            text(
                ops,
                marker.x + 2.0,
                marker.y,
                "✓",
                13.0,
                if self.state.disabled { theme.muted } else { theme.focus },
            );
        }
        text(
            ops,
            self.bounds.x + 28.0,
            self.bounds.y + 6.0,
            label,
            13.0,
            if self.state.disabled { theme.muted } else { theme.text },
        );
        ops.push(DrawOp::PopClip);
        if self.state.focused {
            ops.push(DrawOp::Stroke(self.bounds, theme.focus, 2.0));
        }
    }
    pub fn semantics(&self, name: &str, command: &str) -> Semantics {
        Semantics::new(self.id, SemanticRole::Checkbox, name, command, self.bounds, self.state)
            .action(SemanticAction::Focus)
            .action(SemanticAction::Toggle)
    }
}

/// Shared finite range for slider and numeric stepper. Invalid ranges are rejected.
#[derive(Clone, Copy, Debug)]
pub struct NumberRange {
    min: f64,
    max: f64,
    step: f64,
    value: f64,
}
impl NumberRange {
    pub fn new(min: f64, max: f64, step: f64, value: f64) -> Option<Self> {
        if ![min, max, step, value].iter().all(|n| n.is_finite())
            || min > max
            || step <= 0.0
            || !(max - min).is_finite()
        {
            return None;
        }
        Some(Self {
            min,
            max,
            step,
            value: value.clamp(min, max),
        })
    }
    pub fn value(&self) -> f64 {
        self.value
    }
    pub fn set(&mut self, value: f64) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }
        let value = value.clamp(self.min, self.max);
        if value == self.value {
            return None;
        }
        self.value = value;
        Some(value)
    }
    pub fn key(&mut self, key: Key) -> Option<f64> {
        match key {
            Key::Left | Key::Down => self.set((self.value - self.step).max(self.min)),
            Key::Right | Key::Up => self.set((self.value + self.step).min(self.max)),
            Key::Home => self.set(self.min),
            Key::End => self.set(self.max),
            _ => None,
        }
    }
}
pub struct Slider {
    pub bounds: Rect,
    pub state: ControlState,
    pub range: NumberRange,
    dragging: bool,
}
impl Slider {
    pub fn new(bounds: Rect, range: NumberRange) -> Self {
        Self {
            bounds,
            range,
            state: ControlState::default(),
            dragging: false,
        }
    }
    pub fn event(&mut self, event: UiEvent) -> Option<f64> {
        if let UiEvent::Focus(focused) = event {
            self.state.focused = focused && !self.state.disabled;
            if !focused {
                self.dragging = false;
            }
            return None;
        }
        if self.state.disabled {
            self.dragging = false;
            return None;
        }
        let point = match event {
            UiEvent::PointerDown(p) if self.bounds.contains(p) => {
                self.dragging = true;
                Some(p)
            }
            UiEvent::PointerMove(p) if self.dragging => Some(p),
            UiEvent::PointerUp(p) if self.dragging => {
                self.dragging = false;
                Some(p)
            }
            UiEvent::Key(key) if self.state.focused => return self.range.key(key),
            _ => None,
        }?;
        let fraction = ((point.x - self.bounds.x - 6.0) / (self.bounds.width - 12.0).max(1.0)).clamp(0.0, 1.0) as f64;
        let value = self.range.min + fraction * (self.range.max - self.range.min);
        let snapped = self.range.min + ((value - self.range.min) / self.range.step).round() * self.range.step;
        self.range.set(snapped)
    }
    pub fn paint(&self, theme: Theme, ops: &mut Vec<DrawOp>) {
        let fraction = if self.range.max > self.range.min {
            (self.range.value - self.range.min) / (self.range.max - self.range.min)
        } else {
            0.0
        };
        ops.push(DrawOp::Fill(
            rect(
                self.bounds.x + 6.0,
                self.bounds.y + self.bounds.height / 2.0 - 1.0,
                (self.bounds.width - 12.0).max(0.0),
                2.0,
            ),
            theme.border,
        ));
        ops.push(DrawOp::FillRounded(
            rect(
                self.bounds.x + fraction as f32 * (self.bounds.width - 12.0).max(0.0),
                self.bounds.y + (self.bounds.height - 12.0) / 2.0,
                12.0,
                12.0,
            ),
            if self.state.disabled { theme.muted } else { theme.focus },
            6.0,
        ));
        if self.state.focused {
            ops.push(DrawOp::Stroke(self.bounds, theme.focus, 2.0));
        }
    }
    pub fn semantics(&self, id: ViewId, name: &str, command: &str) -> Semantics {
        let mut node = Semantics::new(id, SemanticRole::Slider, name, command, self.bounds, self.state)
            .action(SemanticAction::Focus)
            .action(SemanticAction::SetValue);
        node.value = Some(self.range.value.to_string());
        node
    }
}

pub struct Stepper {
    pub bounds: Rect,
    pub state: ControlState,
    pub range: NumberRange,
}
impl Stepper {
    pub fn event(&mut self, event: UiEvent) -> Option<f64> {
        if let UiEvent::Focus(focused) = event {
            self.state.focused = focused && !self.state.disabled;
            return None;
        }
        if self.state.disabled {
            return None;
        }
        match event {
            UiEvent::Key(key) if self.state.focused => self.range.key(key),
            UiEvent::PointerDown(p) if self.bounds.contains(p) && p.x >= self.bounds.x + self.bounds.width - 24.0 => {
                self.range.key(if p.y < self.bounds.y + self.bounds.height / 2.0 {
                    Key::Up
                } else {
                    Key::Down
                })
            }
            _ => None,
        }
    }
    /// Invalid user input remains local to the owning text field.
    pub fn commit(&mut self, value: &str) -> Option<f64> {
        if self.state.disabled {
            return None;
        }
        let value = value.trim().parse::<f64>().ok()?;
        if value < self.range.min || value > self.range.max {
            return None;
        }
        self.range.set(value)
    }
    pub fn paint(&self, theme: Theme, ops: &mut Vec<DrawOp>) {
        ops.push(DrawOp::FillRounded(self.bounds, theme.surface, 4.0));
        ops.push(DrawOp::StrokeRounded(
            self.bounds,
            if self.state.focused { theme.focus } else { theme.border },
            4.0,
            if self.state.focused { 2.0 } else { 1.0 },
        ));
        ops.push(DrawOp::PushClip(self.bounds));
        text(
            ops,
            self.bounds.x + 10.0,
            self.bounds.y + 6.0,
            self.range.value.to_string(),
            13.0,
            if self.state.disabled { theme.muted } else { theme.text },
        );
        text(
            ops,
            self.bounds.x + self.bounds.width - 18.0,
            self.bounds.y,
            "+",
            12.0,
            theme.muted,
        );
        text(
            ops,
            self.bounds.x + self.bounds.width - 18.0,
            self.bounds.y + self.bounds.height / 2.0,
            "−",
            12.0,
            theme.muted,
        );
        ops.push(DrawOp::PopClip);
    }
    pub fn semantics(&self, id: ViewId, name: &str, command: &str) -> Semantics {
        let mut node = Semantics::new(id, SemanticRole::SpinButton, name, command, self.bounds, self.state)
            .action(SemanticAction::Focus)
            .action(SemanticAction::SetValue);
        node.value = Some(self.range.value.to_string());
        node
    }
}

/// Logical-pixel splitter; owners apply emitted positions to their pane layout.
pub struct Splitter {
    pub bounds: Rect,
    pub state: ControlState,
    pub vertical: bool,
    pub range: NumberRange,
    drag_origin: Option<(Point, f64)>,
}
impl Splitter {
    pub fn new(bounds: Rect, vertical: bool, range: NumberRange) -> Self {
        Self {
            bounds,
            vertical,
            range,
            state: ControlState::default(),
            drag_origin: None,
        }
    }
    pub fn event(&mut self, event: UiEvent) -> Option<f64> {
        if let UiEvent::Focus(focused) = event {
            self.state.focused = focused && !self.state.disabled;
            if !focused {
                self.drag_origin = None;
            }
            return None;
        }
        if self.state.disabled {
            self.drag_origin = None;
            return None;
        }
        match event {
            UiEvent::PointerDown(p) if self.bounds.contains(p) => {
                self.drag_origin = Some((p, self.range.value));
                None
            }
            UiEvent::PointerMove(p) | UiEvent::PointerUp(p) => {
                let (origin, value) = self.drag_origin?;
                if matches!(event, UiEvent::PointerUp(_)) {
                    self.drag_origin = None;
                }
                self.range.set(
                    value
                        + if self.vertical {
                            (p.x - origin.x) as f64
                        } else {
                            (p.y - origin.y) as f64
                        },
                )
            }
            UiEvent::Key(key) if self.state.focused => self.range.key(key),
            _ => None,
        }
    }
    pub fn paint(&self, theme: Theme, ops: &mut Vec<DrawOp>) {
        ops.push(DrawOp::Fill(
            self.bounds,
            if self.state.focused { theme.focus } else { theme.border },
        ));
    }
    pub fn semantics(&self, id: ViewId, name: &str, command: &str) -> Semantics {
        let mut node = Semantics::new(id, SemanticRole::Separator, name, command, self.bounds, self.state)
            .action(SemanticAction::Focus)
            .action(SemanticAction::SetValue);
        node.value = Some(self.range.value.to_string());
        node
    }
}

/// Which docks are currently showing. Open state is derived from the panels
/// each frame; only the widths persist.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DockVisibility {
    pub left: bool,
    pub right: bool,
    pub bottom: bool,
}
/// Persisted dock widths/heights in logical pixels. Values are clamped on every
/// layout, so a stale or corrupt persisted value can never push the tab strip
/// or the editor off-screen.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DockWidths {
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
}
impl Default for DockWidths {
    fn default() -> Self {
        Self {
            left: 238.0,
            right: 240.0,
            bottom: 180.0,
        }
    }
}
impl DockWidths {
    pub const MIN: f32 = 120.0;
    pub const MIN_BOTTOM: f32 = 80.0;
    /// Draggable splitter thickness / hit zone (UX-50).
    pub const SPLITTER: f32 = 6.0;
    /// Compact, human-diffable persistence form; round-trips through [`parse`].
    pub fn serialize(&self) -> String {
        format!(
            "left={:.1};right={:.1};bottom={:.1}",
            self.left, self.right, self.bottom
        )
    }
    /// Parse the persisted form. Unknown keys, non-finite or unparsable values
    /// fall back to the default for that field, so persistence never fails.
    pub fn parse(text: &str) -> Self {
        let mut widths = Self::default();
        for part in text.split(';') {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let Ok(value) = value.trim().parse::<f32>() else {
                continue;
            };
            if !value.is_finite() {
                continue;
            }
            match key.trim() {
                "left" => widths.left = value,
                "right" => widths.right = value,
                "bottom" => widths.bottom = value,
                _ => {}
            }
        }
        widths
    }
    fn clamped(self, width: f32, content_height: f32) -> Self {
        let max_side = (width * 0.6).max(Self::MIN);
        let max_bottom = (content_height * 0.7).max(Self::MIN_BOTTOM);
        Self {
            left: self.left.clamp(Self::MIN, max_side),
            right: self.right.clamp(Self::MIN, max_side),
            bottom: self.bottom.clamp(Self::MIN_BOTTOM, max_bottom),
        }
    }
}
/// Rects produced by [`DockLayout::compute`]. Every dock rect is guaranteed to
/// sit at or below `tab_strip.y + tab_strip.height`, so toggling any panel
/// never overlaps the tab strip (fixes ISSUE-016/018).
#[derive(Clone, Copy, Debug)]
pub struct DockRects {
    pub tab_strip: Rect,
    pub left: Option<Rect>,
    pub right: Option<Rect>,
    pub bottom: Option<Rect>,
    pub editor: Rect,
    pub left_splitter: Option<Rect>,
    pub right_splitter: Option<Rect>,
    pub bottom_splitter: Option<Rect>,
}
/// Pure dock geometry. The tab strip is laid out first, full width, and every
/// dock is placed strictly below it; the editor takes whatever remains.
pub struct DockLayout;
impl DockLayout {
    pub fn compute(
        width: f32,
        height: f32,
        tab_height: f32,
        status_height: f32,
        visibility: DockVisibility,
        widths: DockWidths,
    ) -> DockRects {
        let s = DockWidths::SPLITTER;
        let content_top = tab_height;
        let content_bottom = (height - status_height).max(content_top);
        let content_height = (content_bottom - content_top).max(0.0);
        let widths = widths.clamped(width, content_height);
        let tab_strip = rect(0.0, 0.0, width.max(0.0), tab_height.max(0.0));

        let left = visibility
            .left
            .then(|| rect(0.0, content_top, widths.left, content_height));
        let left_splitter = visibility
            .left
            .then(|| rect(widths.left, content_top, s, content_height));
        let right = visibility.right.then(|| {
            rect(
                (width - widths.right).max(0.0),
                content_top,
                widths.right,
                content_height,
            )
        });
        let right_splitter = visibility
            .right
            .then(|| rect((width - widths.right - s).max(0.0), content_top, s, content_height));

        let center_x = if visibility.left { widths.left + s } else { 0.0 };
        let center_right = if visibility.right {
            width - widths.right - s
        } else {
            width
        };
        let center_width = (center_right - center_x).max(0.0);

        let (bottom, bottom_splitter, editor_height) = if visibility.bottom {
            let bottom_height = widths.bottom.min((content_height - s).max(0.0));
            let bottom = rect(center_x, content_bottom - bottom_height, center_width, bottom_height);
            let splitter = rect(center_x, content_bottom - bottom_height - s, center_width, s);
            (
                Some(bottom),
                Some(splitter),
                (content_height - bottom_height - s).max(0.0),
            )
        } else {
            (None, None, content_height)
        };
        let editor = rect(center_x, content_top, center_width, editor_height);
        DockRects {
            tab_strip,
            left,
            right,
            bottom,
            editor,
            left_splitter,
            right_splitter,
            bottom_splitter,
        }
    }
}
/// One stacked, collapsible section in the left dock (Workspace / Document List
/// / Outline). Each carries a header with a title, a collapse chevron and an ×.
#[derive(Clone, Copy, Debug)]
pub struct SectionLayout {
    pub header: Rect,
    pub body: Option<Rect>,
    /// The × hit box at the right of the header.
    pub close: Rect,
}
pub const SECTION_HEADER: f32 = 26.0;
/// Lay out stacked collapsible sections inside `dock`. Collapsed sections keep
/// only their header; the remaining height is split evenly among expanded ones,
/// so two (or three) sections can be open on the left at once (UX-50).
pub fn stack_sections(dock: Rect, collapsed: &[bool]) -> Vec<SectionLayout> {
    let expanded = collapsed.iter().filter(|c| !**c).count().max(1);
    let bodies_height = (dock.height - collapsed.len() as f32 * SECTION_HEADER).max(0.0);
    let each = bodies_height / expanded as f32;
    let mut y = dock.y;
    let mut out = Vec::with_capacity(collapsed.len());
    for &is_collapsed in collapsed {
        let header = rect(dock.x, y, dock.width, SECTION_HEADER);
        let close = rect(dock.x + dock.width - 24.0, y + 3.0, 20.0, 20.0);
        y += SECTION_HEADER;
        let body = if is_collapsed {
            None
        } else {
            let body = rect(dock.x, y, dock.width, each);
            y += each;
            Some(body)
        };
        out.push(SectionLayout { header, body, close });
    }
    out
}

impl crate::controls::Button {
    pub fn semantics(&self, localized_name: &str, command: &str) -> Semantics {
        Semantics::new(
            self.id,
            if self.toggle {
                SemanticRole::Checkbox
            } else {
                SemanticRole::Button
            },
            localized_name,
            command,
            self.bounds,
            self.state,
        )
        .action(SemanticAction::Focus)
        .action(if self.toggle {
            SemanticAction::Toggle
        } else {
            SemanticAction::Invoke
        })
    }
}
impl crate::text_field::TextField {
    pub fn semantics(
        &self,
        id: ViewId,
        localized_name: &str,
        command: &str,
        bounds: Rect,
        state: ControlState,
    ) -> Semantics {
        let mut node = Semantics::new(id, SemanticRole::TextField, localized_name, command, bounds, state)
            .action(SemanticAction::Focus)
            .action(SemanticAction::SetValue);
        // Only committed text is exposed, never an in-progress IME composition.
        node.value = Some(self.semantic_value());
        node.invalid = self.validation().map(str::to_owned);
        node
    }
}
impl crate::controls::Scrollbar {
    pub fn semantics(&self, id: ViewId, localized_name: &str, command: &str, state: ControlState) -> Semantics {
        let mut node = Semantics::new(id, SemanticRole::Scrollbar, localized_name, command, self.bounds, state)
            .action(SemanticAction::Scroll);
        node.value = Some(self.offset.to_string());
        node
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_renderer::{RenderBackend, balanced_clips};
    use bareline_renderer_recording::RecordingBackend;
    use std::cell::Cell;
    struct Source {
        count: usize,
        reads: Cell<usize>,
    }
    impl ItemSource for Source {
        fn len(&self) -> Option<usize> {
            Some(self.count)
        }
        fn discovered(&self) -> usize {
            self.count
        }
        fn label(&self, index: usize) -> &str {
            self.reads.set(self.reads.get() + 1);
            match index {
                0 => "Alpha",
                1 => "Beta",
                _ => "Gamma",
            }
        }
        fn enabled(&self, index: usize) -> bool {
            index != 1
        }
    }
    fn list() -> List {
        List {
            bounds: rect(0.0, 0.0, 200.0, 56.0),
            state: ControlState {
                focused: true,
                ..ControlState::default()
            },
            selected: None,
            offset: 0.0,
            metrics: Metrics::COMPACT,
        }
    }
    #[test]
    fn million_row_paint_only_reads_visible_rows_and_navigation_reveals_selection() {
        let source = Source {
            count: 1_000_000,
            reads: Cell::new(0),
        };
        let mut list = list();
        list.offset = 500_000.0 * 28.0;
        let mut ops = Vec::new();
        list.paint(&source, Theme::default(), &mut ops);
        assert_eq!(source.reads.get(), 4);
        assert!(balanced_clips(&ops));
        assert_eq!(
            list.event(UiEvent::Key(Key::Home), &source),
            Some(ControlAction::Selected(0))
        );
        assert_eq!(list.offset, 0.0);
        assert_eq!(
            list.event(UiEvent::Key(Key::Down), &source),
            Some(ControlAction::Selected(2))
        );
        assert_eq!(list.offset, 28.0);
        list.state.disabled = true;
        assert_eq!(list.event(UiEvent::Key(Key::End), &source), None);
        assert_eq!(list.selected, Some(2));
        assert!(
            list.semantics(ViewId(1), "Commands", "palette.select")
                .actions
                .is_empty()
        );
    }
    #[test]
    fn combo_search_timeout_escape_and_disabled_choices() {
        let source = Source {
            count: 3,
            reads: Cell::new(0),
        };
        let mut combo = Combo::new(list());
        combo.list.selected = Some(0);
        combo.set_open(true);
        assert_eq!(combo.search("g", 10, &source), Some(ControlAction::Selected(2)));
        assert_eq!(combo.search("a", 30, &source), None);
        assert_eq!(combo.list.selected, Some(2));
        assert_eq!(
            combo.event(UiEvent::Key(Key::Escape), &source),
            Some(ControlAction::Selected(0))
        );
        assert!(!combo.open);
        assert_eq!(combo.search("b", 2000, &source), None);
        assert_eq!(combo.search("g", 4000, &source), Some(ControlAction::Selected(2)));
        assert_eq!(
            combo
                .semantics(ViewId(2), "Font", "settings.font", rect(0.0, 0.0, 100.0, 28.0), &source)
                .value
                .as_deref(),
            Some("Gamma")
        );
    }
    #[test]
    fn numeric_controls_reject_nonfinite_input_clamp_and_cancel_drag_on_focus_loss() {
        assert!(NumberRange::new(0.0, f64::INFINITY, 1.0, 0.0).is_none());
        assert!(NumberRange::new(0.0, 5.0, 0.0, 0.0).is_none());
        let mut slider = Slider::new(
            rect(0.0, 0.0, 112.0, 28.0),
            NumberRange::new(0.0, 10.0, 1.0, 0.0).unwrap(),
        );
        assert_eq!(
            slider.event(UiEvent::PointerDown(Point { x: 56.0, y: 14.0 })),
            Some(5.0)
        );
        assert_eq!(
            slider.event(UiEvent::PointerMove(Point { x: 200.0, y: 14.0 })),
            Some(10.0)
        );
        slider.event(UiEvent::Focus(false));
        assert_eq!(slider.event(UiEvent::PointerMove(Point { x: 0.0, y: 14.0 })), None);
        let mut stepper = Stepper {
            bounds: slider.bounds,
            state: ControlState::default(),
            range: slider.range,
        };
        for invalid in ["NaN", "inf", "11", "broken"] {
            assert_eq!(stepper.commit(invalid), None);
        }
        assert_eq!(stepper.commit("4"), Some(4.0));
        stepper.state.disabled = true;
        assert_eq!(stepper.commit("5"), None);
        let mut splitter = Splitter::new(
            rect(50.0, 0.0, 6.0, 100.0),
            true,
            NumberRange::new(20.0, 80.0, 5.0, 50.0).unwrap(),
        );
        splitter.event(UiEvent::PointerDown(Point { x: 52.0, y: 1.0 }));
        assert_eq!(
            splitter.event(UiEvent::PointerUp(Point { x: 200.0, y: 1.0 })),
            Some(80.0)
        );
        assert_eq!(splitter.event(UiEvent::PointerMove(Point { x: 30.0, y: 1.0 })), None);
    }
    #[test]
    fn controls_render_balanced_logical_geometry_at_required_scales() {
        let source = Source {
            count: 3,
            reads: Cell::new(0),
        };
        let radio = RadioGroup { list: list() };
        let checkbox = Checkbox {
            id: ViewId(1),
            bounds: rect(0.0, 60.0, 150.0, 28.0),
            state: ControlState {
                checked: true,
                focused: true,
                ..ControlState::default()
            },
        };
        let range = NumberRange::new(0.0, 100.0, 1.0, 50.0).unwrap();
        let slider = Slider::new(rect(0.0, 90.0, 150.0, 28.0), range);
        let stepper = Stepper {
            bounds: rect(0.0, 120.0, 150.0, 28.0),
            state: checkbox.state,
            range,
        };
        let mut ops = Vec::new();
        radio.paint(&source, Theme::default(), &mut ops);
        checkbox.paint("Word wrap", Theme::default(), &mut ops);
        slider.paint(Theme::default(), &mut ops);
        stepper.paint(Theme::default(), &mut ops);
        let mut backend = RecordingBackend::default();
        for scale in [1.0, 1.25, 1.5, 2.0, 2.5, 3.0] {
            backend
                .resize((200.0 * scale) as u32, (160.0 * scale) as u32, scale)
                .unwrap();
            backend.render(&ops).unwrap();
            assert_eq!(backend.operations.len(), ops.len());
            assert!(backend.operations.iter().any(
                |op| matches!(op, DrawOp::Stroke(bounds, _, width) if *bounds == checkbox.bounds && *width == 2.0)
            ));
        }
    }
    #[test]
    fn field_semantics_exclude_preedit_and_disabled_controls_have_no_actions() {
        let mut field = crate::text_field::TextField::default();
        field.insert("saved");
        field.preedit("pending".into(), None);
        let node = field.semantics(
            ViewId(1),
            "Find text",
            "search.find",
            rect(0.0, 0.0, 100.0, 28.0),
            ControlState {
                disabled: true,
                ..ControlState::default()
            },
        );
        assert_eq!(node.value.as_deref(), Some("saved"));
        assert!(node.actions.is_empty());
    }
    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }
    #[test]
    fn dock_layout_never_overlaps_the_tab_strip_for_any_panel_combination() {
        let tab_height = 28.0;
        let status_height = 24.0;
        let widths = DockWidths::default();
        let mut reference_tab = None;
        for mask in 0..8u8 {
            let visibility = DockVisibility {
                left: mask & 1 != 0,
                right: mask & 2 != 0,
                bottom: mask & 4 != 0,
            };
            let rects = DockLayout::compute(1200.0, 800.0, tab_height, status_height, visibility, widths);
            // The tab strip is identical regardless of which panels are open.
            let tab = reference_tab.get_or_insert(rects.tab_strip);
            assert_eq!(rects.tab_strip.x, tab.x);
            assert_eq!(rects.tab_strip.y, tab.y);
            assert_eq!(rects.tab_strip.width, tab.width);
            assert_eq!(rects.tab_strip.height, tab.height);
            for dock in [rects.left, rects.right, rects.bottom] {
                if let Some(dock) = dock {
                    assert!(
                        !overlaps(dock, rects.tab_strip),
                        "dock {dock:?} overlaps tab strip for mask {mask}"
                    );
                    assert!(dock.y >= tab_height - f32::EPSILON);
                }
            }
            assert!(!overlaps(rects.editor, rects.tab_strip));
            assert!(rects.editor.y >= tab_height - f32::EPSILON);
            // Docks never overlap each other or the editor.
            if let (Some(l), Some(r)) = (rects.left, rects.right) {
                assert!(!overlaps(l, r));
            }
            if let Some(b) = rects.bottom {
                assert!(!overlaps(b, rects.editor));
            }
        }
    }
    #[test]
    fn splitter_widths_round_trip_through_persistence() {
        let widths = DockWidths {
            left: 200.5,
            right: 264.0,
            bottom: 150.0,
        };
        assert_eq!(DockWidths::parse(&widths.serialize()), widths);
        // Defaults survive missing / corrupt fields.
        let defaults = DockWidths::default();
        assert_eq!(DockWidths::parse(""), defaults);
        assert_eq!(DockWidths::parse("left=NaN;right=oops"), defaults);
        assert_eq!(
            DockWidths::parse("left=300"),
            DockWidths {
                left: 300.0,
                ..defaults
            }
        );
        // A stale, oversized persisted value is clamped rather than hiding the editor.
        let huge = DockWidths {
            left: 5000.0,
            right: 5000.0,
            bottom: 5000.0,
        };
        let rects = DockLayout::compute(
            1000.0,
            600.0,
            28.0,
            24.0,
            DockVisibility {
                left: true,
                right: true,
                bottom: true,
            },
            huge,
        );
        assert!(rects.editor.width >= 0.0);
        assert!(!overlaps(rects.editor, rects.tab_strip));
    }
    #[test]
    fn two_left_sections_can_be_open_at_once() {
        let dock = rect(0.0, 28.0, 238.0, 600.0);
        let sections = stack_sections(dock, &[false, false, true]);
        assert_eq!(sections.len(), 3);
        // First two are expanded (have a body), the third is collapsed.
        assert!(sections[0].body.is_some());
        assert!(sections[1].body.is_some());
        assert!(sections[2].body.is_none());
        // Headers stack downward and stay inside the dock.
        assert!(sections[1].header.y > sections[0].header.y);
        for section in &sections {
            assert!(section.close.x + section.close.width <= dock.x + dock.width);
        }
    }
}
