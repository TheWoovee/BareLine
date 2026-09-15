// SPDX-License-Identifier: MPL-2.0
//! Bounded macro management surface; effects are dispatched by the native owner.
use super::Macro;
use bareline_commands::CommandId;
use bareline_renderer::{DrawOp, LayoutError, Point, Rect, TextBackend};
use bareline_ui::{
    ViewId,
    controls::{Button, ControlState, Key, UiEvent},
    rect, text,
    text_field::TextField,
    theme::UiTheme,
    widgets::{SemanticAction, SemanticRole, Semantics},
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagerEffect {
    Select(String),
    Command(CommandId),
    Dismiss,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_ignores_cached_command_disablement_and_all_dismiss_inputs_match() {
        let mut manager = MacroManager::default();
        manager.show(&BTreeMap::new(), None);
        let mut context = bareline_commands::CommandContext::default();
        context.states.insert(
            CommandId("macro.manager_close"),
            bareline_commands::CommandState::disabled("stale closed-manager state"),
        );
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        manager
            .draw(
                &mut backend,
                1000.,
                800.,
                "loading failed while recording and playing",
                UiTheme::default(),
                &context,
                &mut Vec::new(),
            )
            .unwrap();
        let close = manager
            .semantics()
            .into_iter()
            .find(|node| node.id == ViewId(23205))
            .unwrap();
        assert!(!close.disabled);
        assert!(close.actions.contains(&SemanticAction::Invoke));
        assert!(close.invalid.is_none());
        assert_eq!(manager.accessibility(23205, true), Some(ManagerEffect::Dismiss));
        assert_eq!(
            manager.event(UiEvent::Key(Key::Escape), false),
            Some(ManagerEffect::Dismiss)
        );
        assert!(manager.open, "the native owner applies the shared dismissal effect");
    }
}
pub struct MacroManager {
    pub open: bool,
    pub name: TextField,
    pub repetitions: TextField,
    pub shortcut: TextField,
    pub delay: TextField,
    names: Vec<String>,
    selected: usize,
    first: usize,
    focus: usize,
    list: Rect,
    fields: [Rect; 4],
    buttons: Vec<(CommandId, Button)>,
    reasons: BTreeMap<CommandId, String>,
}
impl Default for MacroManager {
    fn default() -> Self {
        let mut repetitions = TextField::default();
        repetitions.insert("1");
        let mut delay = TextField::default();
        delay.insert("50");
        Self {
            open: false,
            name: TextField::default(),
            repetitions,
            shortcut: TextField::default(),
            delay,
            names: Vec::new(),
            selected: 0,
            first: 0,
            focus: 0,
            list: Rect::default(),
            fields: [Rect::default(); 4],
            buttons: Vec::new(),
            reasons: BTreeMap::new(),
        }
    }
}
impl MacroManager {
    pub fn show(&mut self, library: &BTreeMap<String, Macro>, selected: Option<&str>) {
        self.names = library.keys().cloned().collect();
        self.selected = selected
            .and_then(|name| self.names.iter().position(|v| v == name))
            .unwrap_or(0);
        self.first = self.selected.saturating_sub(5);
        self.focus = 0;
        self.open = true;
        self.update_name();
    }
    fn update_name(&mut self) {
        self.name.select_all();
        self.name
            .insert(self.names.get(self.selected).map(String::as_str).unwrap_or(""));
    }
    pub fn dismiss(&mut self) {
        self.open = false;
        for field in [
            &mut self.name,
            &mut self.repetitions,
            &mut self.shortcut,
            &mut self.delay,
        ] {
            field.cancel();
        }
    }
    pub fn active_field(&mut self) -> Option<&mut TextField> {
        match self.focus {
            1 => Some(&mut self.name),
            2 => Some(&mut self.repetitions),
            3 => Some(&mut self.shortcut),
            4 => Some(&mut self.delay),
            _ => None,
        }
    }
    pub fn release(&mut self, backend: &mut impl TextBackend) {
        for field in [
            &mut self.name,
            &mut self.repetitions,
            &mut self.shortcut,
            &mut self.delay,
        ] {
            field.release(backend);
        }
    }
    pub fn event(&mut self, event: UiEvent, shift: bool) -> Option<ManagerEffect> {
        if !self.open {
            return None;
        }
        match event {
            UiEvent::Key(Key::Escape) => {
                return Some(ManagerEffect::Dismiss);
            }
            UiEvent::Key(Key::Tab) => {
                let count = 5 + self.buttons.len();
                self.focus = if shift {
                    (self.focus + count - 1) % count
                } else {
                    (self.focus + 1) % count
                };
                return None;
            }
            UiEvent::Key(key) if self.focus == 0 => {
                match key {
                    Key::Up => self.selected = self.selected.saturating_sub(1),
                    Key::Down => self.selected = (self.selected + 1).min(self.names.len().saturating_sub(1)),
                    Key::Home => self.selected = 0,
                    Key::End => self.selected = self.names.len().saturating_sub(1),
                    _ => return None,
                }
                self.first = self.first.min(self.selected);
                if self.selected >= self.first + 6 {
                    self.first = self.selected - 5;
                }
                self.update_name();
                return self.names.get(self.selected).cloned().map(ManagerEffect::Select);
            }
            UiEvent::PointerDown(point) => {
                if self.list.contains(point) {
                    self.focus = 0;
                    let selected = self.first + ((point.y - self.list.y) / 28.) as usize;
                    if selected < self.names.len() {
                        self.selected = selected;
                        self.update_name();
                        return self.names.get(selected).cloned().map(ManagerEffect::Select);
                    }
                }
                for (index, bounds) in self.fields.iter().enumerate() {
                    if bounds.contains(point) {
                        self.focus = index + 1;
                        return None;
                    }
                }
                for (index, (_, button)) in self.buttons.iter().enumerate() {
                    if button.bounds.contains(point) {
                        self.focus = index + 5;
                    }
                }
            }
            _ => {}
        }
        for (index, (id, button)) in self.buttons.iter_mut().enumerate() {
            button.state.focused = self.focus == index + 5;
            if button.event(event).is_some() {
                return Some(if id.0 == "macro.manager_close" {
                    ManagerEffect::Dismiss
                } else {
                    ManagerEffect::Command(*id)
                });
            }
        }
        None
    }
    pub fn draw(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        height: f32,
        status: &str,
        theme: UiTheme,
        context: &bareline_commands::CommandContext,
        ops: &mut Vec<DrawOp>,
    ) -> Result<(), LayoutError> {
        if !self.open {
            return Ok(());
        }
        self.reasons = context
            .states
            .iter()
            .filter(|(id, _)| id.0 != "macro.manager_close")
            .filter_map(|(id, state)| state.disabled_reason.clone().map(|reason| (*id, reason)))
            .collect();
        let w = (width - 24.).clamp(1., 700.);
        let bounds = rect((width - w) / 2., 40., w, (height - 60.).clamp(1., 470.));
        ops.push(DrawOp::FillRounded(bounds, theme.elevated, 6.));
        ops.push(DrawOp::StrokeRounded(bounds, theme.border, 6., 1.));
        ops.push(DrawOp::PushClip(bounds));
        text(ops, bounds.x + 12., bounds.y + 12., "Macros", 16., theme.text);
        self.list = rect(bounds.x + 12., bounds.y + 40., w - 24., 168.);
        if self.names.is_empty() {
            text(
                ops,
                self.list.x + 8.,
                self.list.y + 8.,
                "Record or import a macro to begin",
                13.,
                theme.muted,
            );
        }
        for (row, name) in self.names.iter().skip(self.first).take(6).enumerate() {
            let r = rect(self.list.x, self.list.y + row as f32 * 28., self.list.width, 28.);
            if self.selected == self.first + row {
                ops.push(DrawOp::Fill(r, theme.selection));
                if self.focus == 0 {
                    ops.push(DrawOp::Stroke(r, theme.focus, 2.));
                }
            }
            text(ops, r.x + 8., r.y + 7., name, 13., theme.text);
        }
        let half = (w - 36.) / 2.;
        let y = bounds.y + 230.;
        self.fields = [
            rect(bounds.x + 12., y, half, 28.),
            rect(bounds.x + 24. + half, y, half, 28.),
            rect(bounds.x + 12., y + 52., half, 28.),
            rect(bounds.x + 24. + half, y + 52., half, 28.),
        ];
        for (index, label) in [
            "Name",
            "Repeat count (1–10000)",
            "Shortcut (for example Ctrl+Alt+M)",
            "Typing delay in ms (0–60000)",
        ]
        .iter()
        .enumerate()
        {
            let r = self.fields[index];
            text(ops, r.x, r.y - 17., *label, 11., theme.muted);
        }
        self.name
            .draw_with_theme(backend, self.fields[0], self.focus == 1, theme, ops)?;
        self.repetitions
            .draw_with_theme(backend, self.fields[1], self.focus == 2, theme, ops)?;
        self.shortcut
            .draw_with_theme(backend, self.fields[2], self.focus == 3, theme, ops)?;
        self.delay
            .draw_with_theme(backend, self.fields[3], self.focus == 4, theme, ops)?;
        let previous = std::mem::take(&mut self.buttons);
        for (index, (id, label)) in [
            ("macro.rename", "Rename"),
            ("macro.play_n", "Play N times"),
            ("macro.shortcut", "Assign shortcut"),
            ("macro.ghost", "Set delay"),
            ("macro.save", "Save"),
            ("macro.manager_close", "Close"),
        ]
        .into_iter()
        .enumerate()
        {
            let r = rect(
                bounds.x + 12. + (index % 3) as f32 * (w - 24.) / 3.,
                bounds.y + 326. + (index / 3) as f32 * 32.,
                (w - 36.) / 3.,
                28.,
            );
            let button = Button {
                id: ViewId(23200 + index as u64),
                label: label.into(),
                bounds: r,
                toggle: false,
                state: ControlState {
                    disabled: id != "macro.manager_close"
                        && context.states.get(&CommandId(id)).is_some_and(|state| !state.enabled),
                    focused: self.focus == index + 5,
                    pressed: previous.get(index).is_some_and(|b| b.1.state.pressed),
                    ..Default::default()
                },
            };
            button.paint_with_theme(theme, ops);
            self.buttons.push((CommandId(id), button));
        }
        text(
            ops,
            bounds.x + 12.,
            bounds.y + 402.,
            self.focus
                .checked_sub(5)
                .and_then(|index| self.buttons.get(index))
                .and_then(|(id, _)| self.reasons.get(id))
                .map(String::as_str)
                .unwrap_or(status),
            12.,
            theme.muted,
        );
        text(
            ops,
            bounds.x + 12.,
            bounds.y + 427.,
            "Tab changes control · Up/Down selects macro · Esc returns",
            11.,
            theme.muted,
        );
        ops.push(DrawOp::PopClip);
        Ok(())
    }
    pub fn semantics(&self) -> Vec<Semantics> {
        if !self.open {
            return Vec::new();
        }
        let mut nodes: Vec<_> = self
            .names
            .iter()
            .enumerate()
            .skip(self.first)
            .take(6)
            .map(|(index, name)| {
                let mut n = Semantics::new(
                    ViewId(23000 + index as u64),
                    SemanticRole::ListItem,
                    name,
                    "macro.manager",
                    rect(
                        self.list.x,
                        self.list.y + (index - self.first) as f32 * 28.,
                        self.list.width,
                        28.,
                    ),
                    ControlState {
                        focused: self.focus == 0 && self.selected == index,
                        ..Default::default()
                    },
                )
                .action(SemanticAction::Select)
                .action(SemanticAction::Focus);
                n.selected = self.selected == index;
                n
            })
            .collect();
        for (index, (name, field)) in [
            ("Macro name", &self.name),
            ("Repeat count", &self.repetitions),
            ("Macro shortcut", &self.shortcut),
            ("Typing delay in milliseconds", &self.delay),
        ]
        .into_iter()
        .enumerate()
        {
            let mut n = Semantics::new(
                ViewId(23100 + index as u64),
                SemanticRole::TextField,
                name,
                "macro.manager",
                self.fields[index],
                ControlState {
                    focused: self.focus == index + 1,
                    ..Default::default()
                },
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::SetValue);
            n.value = Some(field.value().into());
            nodes.push(n);
        }
        nodes.extend(self.buttons.iter().enumerate().map(|(index, (id, b))| {
            let mut node = Semantics::new(
                b.id,
                SemanticRole::Button,
                &b.label,
                id.0,
                b.bounds,
                ControlState {
                    focused: self.focus == index + 5,
                    ..b.state
                },
            )
            .action(SemanticAction::Focus);
            if !b.state.disabled {
                node.actions.push(SemanticAction::Invoke);
            }
            node.invalid = self.reasons.get(id).cloned();
            node
        }));
        nodes
    }
    pub fn accessibility(&mut self, id: u64, invoke: bool) -> Option<ManagerEffect> {
        if !self.open {
            return None;
        }
        if let Some(index) = id
            .checked_sub(23000)
            .map(|v| v as usize)
            .filter(|v| *v < self.names.len())
        {
            self.focus = 0;
            self.selected = index;
            self.update_name();
            return Some(ManagerEffect::Select(self.names[index].clone()));
        }
        if (23100..23104).contains(&id) {
            self.focus = (id - 23100) as usize + 1;
            return None;
        }
        if let Some(index) = id
            .checked_sub(23200)
            .map(|v| v as usize)
            .filter(|v| *v < self.buttons.len())
        {
            self.focus = index + 5;
            if invoke {
                return Some(if self.buttons[index].0.0 == "macro.manager_close" {
                    ManagerEffect::Dismiss
                } else {
                    ManagerEffect::Command(self.buttons[index].0)
                });
            }
        }
        None
    }
    pub fn click_field(&mut self, backend: &impl TextBackend, point: Point, shift: bool) {
        if let Some(field) = self.active_field() {
            let _ = field.click(backend, point, shift);
        }
    }
}
