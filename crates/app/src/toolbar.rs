// SPDX-License-Identifier: MPL-2.0
//! Optional command strip. All execution is revalidated by its registry owner.
use bareline_commands::{CommandContext, CommandId, CommandRegistry, Keymap, ToolbarModel};
use bareline_renderer::{DrawOp, Point};
use bareline_ui::{
    ViewId,
    controls::{Button, ControlState, Key, UiEvent},
    rect, text,
    theme::UiTheme,
    widgets::{SemanticAction, SemanticRole, Semantics},
};

pub struct ToolbarController {
    pub model: ToolbarModel,
    pub customizing: bool,
    pub status: Option<String>,
    buttons: Vec<(CommandId, Button, String, Option<String>)>,
    catalog: Vec<(CommandId, String)>,
    selected: usize,
    first: usize,
    rows: usize,
    focus: Option<usize>,
    width: f32,
    popup: bareline_renderer::Rect,
}
impl Default for ToolbarController {
    fn default() -> Self {
        Self {
            model: ToolbarModel {
                visible: false,
                commands: vec![
                    CommandId("file.new"),
                    CommandId("file.open"),
                    CommandId("file.save"),
                    CommandId("edit.undo"),
                    CommandId("search.find"),
                ],
            },
            customizing: false,
            status: None,
            buttons: Vec::new(),
            catalog: Vec::new(),
            selected: 0,
            first: 0,
            rows: 1,
            focus: None,
            width: 0.,
            popup: rect(0., 0., 0., 0.),
        }
    }
}
impl ToolbarController {
    pub fn height(&self) -> f32 {
        if self.model.visible { 36. } else { 0. }
    }
    pub fn focused(&self) -> bool {
        self.customizing || self.focus.is_some()
    }
    pub fn dismiss(&mut self) {
        self.customizing = false;
        self.focus = None;
        for button in &mut self.buttons {
            button.1.state.pressed = false;
        }
    }
    pub fn customize(&mut self, registry: &CommandRegistry) {
        self.catalog = registry
            .entries()
            .filter(|spec| !registry.presentation(spec.id).is_some_and(|p| p.internal))
            .map(|spec| (spec.id, spec.title.to_string()))
            .collect();
        self.catalog.sort_by(|a, b| a.1.cmp(&b.1));
        self.selected = 0;
        self.first = 0;
        self.customizing = true;
    }
    pub fn refresh(
        &mut self,
        registry: &CommandRegistry,
        context: &CommandContext,
        keymap: &Keymap,
        width: f32,
    ) {
        self.width = width;
        let focused_id = self.focus.and_then(|i| self.buttons.get(i)).map(|b| b.0);
        let pressed_id = self
            .buttons
            .iter()
            .find(|button| button.1.state.pressed)
            .map(|button| button.0);
        self.buttons.clear();
        if !self.model.visible {
            self.focus = None;
            return;
        }
        let mut x = 6.;
        for id in self.model.commands.iter().copied() {
            let Some(spec) = registry.entries().find(|spec| spec.id == id) else {
                continue;
            };
            let Some(state) = registry.state(id, context) else {
                continue;
            };
            let title = state.label.as_deref().unwrap_or(spec.title);
            let shortcut = keymap.shortcut_label(id);
            let label = if shortcut.is_empty() {
                title.to_string()
            } else {
                format!("{title}  {shortcut}")
            };
            let width = (label.chars().count() as f32 * 7. + 20.).clamp(54., 260.);
            // Keep overflow reachable by keyboard: focus scrolls the strip below.
            let name = registry
                .presentation(id)
                .and_then(|p| p.accessible_name.clone())
                .unwrap_or_else(|| title.to_string());
            let index = self.buttons.len();
            self.buttons.push((
                id,
                Button {
                    id: ViewId(18000 + index as u64),
                    label,
                    bounds: rect(x, 4., width, 28.),
                    toggle: false,
                    state: ControlState {
                        disabled: !state.enabled,
                        checked: state.checked,
                        focused: focused_id == Some(id),
                        pressed: pressed_id == Some(id),
                        ..Default::default()
                    },
                },
                name,
                state.disabled_reason,
            ));
            x += width + 4.;
        }
        self.focus = focused_id.and_then(|id| {
            self.buttons
                .iter()
                .position(|b| b.0 == id && !b.1.state.disabled)
        });
        self.reveal_focus();
    }
    fn reveal_focus(&mut self) {
        if let Some(button) = self.focus.and_then(|i| self.buttons.get(i)) {
            let shift = (button.1.bounds.x + button.1.bounds.width - self.width + 6.).max(0.);
            for button in &mut self.buttons {
                button.1.bounds.x -= shift;
            }
        }
    }
    pub fn focus(&mut self) {
        self.focus = self.buttons.iter().position(|b| !b.1.state.disabled);
        self.reveal_focus();
    }
    fn toggle_selected(&mut self) {
        if let Some((id, _)) = self.catalog.get(self.selected) {
            if let Some(index) = self.model.commands.iter().position(|v| v == id) {
                self.model.commands.remove(index);
            } else if self.model.commands.len() < 32 {
                self.model.commands.push(*id);
            } else {
                self.status = Some(
                    "Toolbar supports at most 32 commands. Remove one before adding another."
                        .into(),
                );
            }
        }
    }
    pub fn key(&mut self, key: Key, shift: bool) -> Option<CommandId> {
        if key == Key::Escape {
            self.dismiss();
            return None;
        }
        if self.customizing {
            match key {
                Key::Up => self.selected = self.selected.saturating_sub(1),
                Key::Down => {
                    self.selected = (self.selected + 1).min(self.catalog.len().saturating_sub(1))
                }
                Key::Home => self.selected = 0,
                Key::End => self.selected = self.catalog.len().saturating_sub(1),
                Key::Enter | Key::Space => self.toggle_selected(),
                Key::Left | Key::Right => {
                    if let Some((id, _)) = self.catalog.get(self.selected) {
                        if let Some(index) = self.model.commands.iter().position(|v| v == id) {
                            let other = if key == Key::Left {
                                index.saturating_sub(1)
                            } else {
                                (index + 1).min(self.model.commands.len() - 1)
                            };
                            self.model.commands.swap(index, other);
                        }
                    }
                }
                _ => {}
            }
            if self.selected < self.first {
                self.first = self.selected;
            }
            if self.selected >= self.first + self.rows {
                self.first = self.selected + 1 - self.rows;
            }
            return None;
        }
        if matches!(key, Key::Left | Key::Right | Key::Tab) {
            let indices: Vec<_> = self
                .buttons
                .iter()
                .enumerate()
                .filter(|(_, b)| !b.1.state.disabled)
                .map(|(i, _)| i)
                .collect();
            if !indices.is_empty() {
                let at = self
                    .focus
                    .and_then(|i| indices.iter().position(|v| *v == i))
                    .unwrap_or(0);
                let next = if key == Key::Left || (key == Key::Tab && shift) {
                    (at + indices.len() - 1) % indices.len()
                } else {
                    (at + 1) % indices.len()
                };
                self.focus = Some(indices[next]);
                self.reveal_focus();
            }
        }
        if matches!(key, Key::Enter | Key::Space) {
            return self
                .focus
                .and_then(|i| self.buttons.get(i))
                .filter(|b| !b.1.state.disabled)
                .map(|b| b.0);
        }
        None
    }
    pub fn pointer(&mut self, point: Point, down: bool) -> Option<CommandId> {
        if self.customizing {
            if !down
                && self.popup.contains(point)
                && point.y >= self.popup.y + 44.
                && point.y < self.popup.y + 44. + self.rows as f32 * 28.
            {
                let index = self.first + ((point.y - self.popup.y - 44.) / 28.) as usize;
                if index < self.catalog.len() {
                    self.selected = index;
                    self.toggle_selected();
                }
            }
            return None;
        }
        if !self.model.visible {
            return None;
        }
        for (index, button) in self.buttons.iter_mut().enumerate() {
            if down && button.1.bounds.contains(point) && !button.1.state.disabled {
                self.focus = Some(index);
            }
            if button
                .1
                .event(if down {
                    UiEvent::PointerDown(point)
                } else {
                    UiEvent::PointerUp(point)
                })
                .is_some()
            {
                return Some(button.0);
            }
        }
        None
    }
    pub fn draw(&mut self, width: f32, height: f32, theme: UiTheme, ops: &mut Vec<DrawOp>) {
        if self.model.visible {
            ops.push(DrawOp::Fill(rect(0., 0., width, 36.), theme.chrome));
            ops.push(DrawOp::PushClip(rect(0., 0., width, 36.)));
            for (index, button) in self.buttons.iter_mut().enumerate() {
                button.1.state.focused = self.focus == Some(index);
                button.1.paint_with_theme(theme, ops);
            }
            ops.push(DrawOp::PopClip);
        }
        if !self.customizing {
            return;
        }
        self.rows = (((height - 150.).max(28.) / 28.) as usize).clamp(1, 12);
        let w = (width - 24.).clamp(1., 620.);
        self.popup = rect((width - w) / 2., 50., w, 88. + self.rows as f32 * 28.);
        ops.push(DrawOp::FillRounded(self.popup, theme.elevated, 6.));
        ops.push(DrawOp::StrokeRounded(self.popup, theme.border, 6., 1.));
        ops.push(DrawOp::PushClip(self.popup));
        text(
            ops,
            self.popup.x + 12.,
            62.,
            "Customize toolbar",
            15.,
            theme.text,
        );
        for (row, (id, title)) in self
            .catalog
            .iter()
            .skip(self.first)
            .take(self.rows)
            .enumerate()
        {
            let bounds = rect(self.popup.x + 8., 94. + row as f32 * 28., w - 16., 28.);
            if self.selected == self.first + row {
                ops.push(DrawOp::Fill(bounds, theme.selection));
            }
            let position = self.model.commands.iter().position(|value| value == id);
            let label = position
                .map(|p| format!("{}. {title}", p + 1))
                .unwrap_or_else(|| format!("Add: {title}"));
            text(ops, bounds.x + 8., bounds.y + 7., &label, 13., theme.text);
        }
        text(
            ops,
            self.popup.x + 12.,
            self.popup.y + self.popup.height - 30.,
            self.status
                .as_deref()
                .unwrap_or("Up/Down select · Space add/remove · Left/Right reorder · Esc done"),
            11.,
            theme.muted,
        );
        ops.push(DrawOp::PopClip);
    }
    pub fn semantics(&self) -> Vec<Semantics> {
        if self.customizing {
            return self
                .catalog
                .iter()
                .enumerate()
                .skip(self.first)
                .take(self.rows)
                .map(|(index, (id, title))| {
                    let position = self.model.commands.iter().position(|value| value == id);
                    let mut node = Semantics::new(
                        ViewId(18500 + index as u64),
                        SemanticRole::Checkbox,
                        title,
                        "view.toolbar_customize",
                        rect(
                            self.popup.x + 8.,
                            self.popup.y + 44. + (index - self.first) as f32 * 28.,
                            self.popup.width - 16.,
                            28.,
                        ),
                        ControlState {
                            focused: index == self.selected,
                            checked: position.is_some(),
                            ..Default::default()
                        },
                    )
                    .action(SemanticAction::Toggle)
                    .action(SemanticAction::Focus);
                    node.value = position.map(|p| format!("Toolbar position {}", p + 1));
                    node
                })
                .collect();
        }
        self.buttons
            .iter()
            .enumerate()
            .filter(|(_, b)| b.1.bounds.x + b.1.bounds.width > 0. && b.1.bounds.x < self.width)
            .map(|(index, b)| {
                let mut node = Semantics::new(
                    b.1.id,
                    SemanticRole::Button,
                    &b.2,
                    b.0.0,
                    b.1.bounds,
                    ControlState {
                        focused: self.focus == Some(index),
                        ..b.1.state
                    },
                )
                .action(SemanticAction::Focus);
                if !b.1.state.disabled {
                    node.actions.push(SemanticAction::Invoke);
                }
                node.invalid = b.3.clone();
                node.value = Some(b.1.label.clone());
                node
            })
            .collect()
    }
    pub fn accessibility(&mut self, id: u64, activate: bool) -> Option<CommandId> {
        if self.customizing {
            if let Some(index) = id
                .checked_sub(18500)
                .map(|v| v as usize)
                .filter(|v| *v < self.catalog.len())
            {
                self.selected = index;
                if activate {
                    self.toggle_selected();
                }
            }
            return None;
        }
        if let Some(index) = id
            .checked_sub(18000)
            .map(|v| v as usize)
            .filter(|v| *v < self.buttons.len() && !self.buttons[*v].1.state.disabled)
        {
            self.focus = Some(index);
            if activate {
                return Some(self.buttons[index].0);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pointer_press_survives_snapshot_refresh_but_disable_cancels_activation() {
        let registry = bareline_commands::shell_commands();
        let keymap = Keymap::defaults(&registry);
        let mut context = CommandContext::default();
        let mut strip = ToolbarController::default();
        strip.model.visible = true;
        strip.refresh(&registry, &context, &keymap, 800.);
        let point = Point { x: 12., y: 12. };
        strip.pointer(point, true);
        strip.refresh(&registry, &context, &keymap, 800.);
        assert_eq!(strip.pointer(point, false), Some(CommandId("file.new")));
        strip.pointer(point, true);
        context.states.insert(
            CommandId("file.new"),
            bareline_commands::CommandState::disabled("Busy"),
        );
        strip.refresh(&registry, &context, &keymap, 800.);
        assert_eq!(strip.pointer(point, false), None);
    }
    #[test]
    fn disabled_focus_and_customization_preserve_ids() {
        let registry = bareline_commands::shell_commands();
        let keymap = Keymap::defaults(&registry);
        let mut context = CommandContext::default();
        context.states.insert(
            CommandId("file.new"),
            bareline_commands::CommandState::disabled("Unavailable"),
        );
        let mut strip = ToolbarController::default();
        assert_eq!(strip.height(), 0.);
        strip.model.visible = true;
        strip.refresh(&registry, &context, &keymap, 800.);
        strip.focus();
        assert_ne!(strip.key(Key::Enter, false), Some(CommandId("file.new")));
        strip.customize(&registry);
        let id = strip.catalog[0].0;
        let before = strip.model.commands.contains(&id);
        strip.key(Key::Space, false);
        assert_eq!(strip.model.commands.contains(&id), !before);
        strip.key(Key::Escape, false);
        assert!(!strip.focused());
        assert!(
            strip
                .model
                .commands
                .iter()
                .all(|id| registry.dispatch(*id).is_some())
        );
    }
}
