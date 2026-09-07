// SPDX-License-Identifier: MPL-2.0
//! Command palette owns transient text/list state; execution stays with the app dispatcher.
use bareline_commands::{CommandContext, CommandId, CommandRegistry, Keymap, PaletteEntry};
use bareline_renderer::{DrawOp, LayoutError, Point, Rect, TextBackend};
use bareline_ui::{
    controls::{ControlState, Key},
    text_field::TextField,
    widgets::{SemanticAction, SemanticRole, Semantics},
    *,
};

const ROW_HEIGHT: f32 = 48.0;
const MAX_ROWS: usize = 7;
#[derive(Default)]
pub struct PaletteController {
    pub open: bool,
    pub field: TextField,
    entries: Vec<PaletteEntry>,
    selected: usize,
    first: usize,
    visible_rows: usize,
    bounds: Rect,
    input_bounds: Rect,
    list_bounds: Rect,
    pub status: Option<String>,
}
impl PaletteController {
    pub fn accessibility_focus(&mut self, id: u64) -> bool {
        if !self.open {
            return false;
        }
        if id == 11000 {
            return true;
        }
        let Some(index) = id
            .checked_sub(11001)
            .and_then(|index| usize::try_from(index).ok())
        else {
            return false;
        };
        if index >= self.first
            && index < self.first + self.visible_rows
            && self
                .entries
                .get(index)
                .is_some_and(|entry| entry.state.enabled)
        {
            self.selected = index;
            self.reveal();
            true
        } else {
            false
        }
    }
    pub fn results(&self) -> &[PaletteEntry] {
        &self.entries
    }
    pub fn selected(&self) -> Option<CommandId> {
        self.entries.get(self.selected).map(|entry| entry.id)
    }
    pub fn show(&mut self, registry: &CommandRegistry, context: &CommandContext, keymap: &Keymap) {
        self.open = true;
        self.field.cancel();
        self.field.select_all();
        self.refresh(registry, context, keymap);
    }
    pub fn dismiss(&mut self) {
        self.open = false;
        self.field.cancel();
        self.status = None;
    }
    pub fn release(&mut self, backend: &mut impl TextBackend) {
        self.field.release(backend);
    }
    /// Refresh after input, keymap edits or app-state changes; retain the selected stable ID.
    pub fn refresh(
        &mut self,
        registry: &CommandRegistry,
        context: &CommandContext,
        keymap: &Keymap,
    ) {
        let selected = self.selected();
        self.entries = registry.palette(self.field.value(), context, keymap, usize::MAX);
        self.selected = selected
            .and_then(|id| self.entries.iter().position(|entry| entry.id == id))
            .unwrap_or(0);
        self.status = None;
        self.reveal();
    }
    pub fn insert(
        &mut self,
        value: &str,
        registry: &CommandRegistry,
        context: &CommandContext,
        keymap: &Keymap,
    ) -> bool {
        let changed = self.field.insert(value);
        if changed {
            self.refresh(registry, context, keymap);
        }
        changed
    }
    pub fn preedit(&mut self, value: String, cursor: Option<(usize, usize)>) {
        self.field.preedit(value, cursor);
    }
    pub fn commit(
        &mut self,
        value: &str,
        registry: &CommandRegistry,
        context: &CommandContext,
        keymap: &Keymap,
    ) -> bool {
        let changed = self.field.commit(value);
        if changed {
            self.refresh(registry, context, keymap);
        }
        changed
    }
    pub fn delete(
        &mut self,
        forward: bool,
        registry: &CommandRegistry,
        context: &CommandContext,
        keymap: &Keymap,
    ) -> bool {
        let changed = self.field.delete(forward);
        if changed {
            self.refresh(registry, context, keymap);
        }
        changed
    }
    fn reveal(&mut self) {
        let count = self.visible_rows.max(1);
        if self.selected < self.first {
            self.first = self.selected;
        }
        if self.selected >= self.first + count {
            self.first = self.selected + 1 - count;
        }
        self.first = self.first.min(self.entries.len().saturating_sub(count));
    }
    /// Activation revalidates current state so a formerly-enabled row cannot dispatch stale state.
    pub fn activate(
        &mut self,
        registry: &CommandRegistry,
        context: &CommandContext,
    ) -> Option<CommandId> {
        if !self.open || self.field.composing() {
            return None;
        }
        let id = self.selected()?;
        match registry.dispatch_in(id, context) {
            Ok(_) => {
                self.dismiss();
                Some(id)
            }
            Err(error) => {
                self.status = Some(match error {
                    bareline_commands::DispatchError::Disabled(reason) => reason,
                    bareline_commands::DispatchError::Unknown(_) => {
                        "Command is no longer available".into()
                    }
                });
                None
            }
        }
    }
    pub fn key(
        &mut self,
        key: Key,
        registry: &CommandRegistry,
        context: &CommandContext,
    ) -> Option<CommandId> {
        if !self.open {
            return None;
        }
        if self.field.composing() {
            if key == Key::Escape {
                self.field.cancel();
            }
            return None;
        }
        match key {
            Key::Escape => self.dismiss(),
            Key::Enter => return self.activate(registry, context),
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => {
                self.selected = (self.selected + 1).min(self.entries.len().saturating_sub(1))
            }
            Key::Left => self.field.horizontal(false, false),
            Key::Right => self.field.horizontal(true, false),
            Key::Home => self.field.edge(false, false),
            Key::End => self.field.edge(true, false),
            _ => {}
        }
        self.reveal();
        None
    }
    pub fn click(
        &mut self,
        backend: &impl TextBackend,
        point: Point,
        registry: &CommandRegistry,
        context: &CommandContext,
    ) -> Result<Option<CommandId>, LayoutError> {
        if !self.open {
            return Ok(None);
        }
        if self.input_bounds.contains(point) {
            self.field.click(backend, point, false)?;
        } else if self.list_bounds.contains(point) {
            let index = self.first + ((point.y - self.list_bounds.y) / ROW_HEIGHT) as usize;
            if index < self.entries.len() {
                self.selected = index;
                return Ok(self.activate(registry, context));
            }
        }
        Ok(None)
    }
    pub fn draw(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Rect, LayoutError> {
        self.draw_with_theme(
            backend,
            width,
            height,
            bareline_ui::theme::UiTheme::default(),
            ops,
        )
    }
    pub fn draw_with_theme(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        height: f32,
        theme: bareline_ui::theme::UiTheme,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Rect, LayoutError> {
        if !self.open {
            return Ok(Rect::default());
        }
        let panel_width = (width - 24.0).clamp(0.0, 560.0);
        let x = (width - panel_width) / 2.0;
        let y = 98.0f32.min((height - 100.0).max(0.0));
        self.visible_rows =
            (((height - y - 80.0).max(ROW_HEIGHT) / ROW_HEIGHT) as usize).clamp(1, MAX_ROWS);
        self.reveal();
        let row_count = self.entries.len().clamp(1, self.visible_rows);
        self.bounds = rect(
            x,
            y,
            panel_width,
            62.0 + ROW_HEIGHT * row_count as f32 + 28.0,
        );
        self.input_bounds = rect(x + 10.0, y + 10.0, panel_width - 20.0, 30.0);
        self.list_bounds = rect(
            x + 6.0,
            y + 50.0,
            panel_width - 12.0,
            ROW_HEIGHT * row_count as f32,
        );
        ops.push(DrawOp::FillRounded(self.bounds, theme.elevated, 8.0));
        ops.push(DrawOp::StrokeRounded(self.bounds, theme.border, 8.0, 1.0));
        let caret = self
            .field
            .draw_with_theme(backend, self.input_bounds, true, theme, ops)?;
        ops.push(DrawOp::PushClip(self.list_bounds));
        if self.entries.is_empty() {
            text(
                ops,
                x + 20.0,
                self.list_bounds.y + 16.0,
                "No matching commands",
                13.0,
                theme.muted,
            );
        }
        for (row, entry) in self
            .entries
            .iter()
            .skip(self.first)
            .take(self.visible_rows)
            .enumerate()
        {
            let bounds = rect(
                self.list_bounds.x,
                self.list_bounds.y + row as f32 * ROW_HEIGHT,
                self.list_bounds.width,
                ROW_HEIGHT,
            );
            if self.first + row == self.selected {
                ops.push(DrawOp::FillRounded(bounds, theme.border, 4.0));
                ops.push(DrawOp::Fill(
                    rect(bounds.x, bounds.y, 4.0, bounds.height),
                    theme.focus,
                ));
            }
            let hint_width = if entry.shortcut.is_empty() {
                0.0
            } else {
                (entry.shortcut.chars().count() as f32 * 7.0 + 14.0).min(bounds.width * 0.45)
            };
            ops.push(DrawOp::PushClip(rect(
                bounds.x + 14.0,
                bounds.y,
                bounds.width - 24.0 - hint_width,
                ROW_HEIGHT,
            )));
            text(
                ops,
                bounds.x + 14.0,
                bounds.y + 8.0,
                &entry.title,
                13.0,
                if entry.state.enabled {
                    theme.text
                } else {
                    theme.muted
                },
            );
            let subtitle = if entry.state.enabled {
                &entry.menu_path
            } else {
                entry
                    .state
                    .disabled_reason
                    .as_deref()
                    .unwrap_or("Unavailable in the current context")
            };
            text(
                ops,
                bounds.x + 14.0,
                bounds.y + 27.0,
                subtitle,
                12.0,
                theme.muted,
            );
            ops.push(DrawOp::PopClip);
            if hint_width > 0.0 {
                let hint = rect(
                    bounds.x + bounds.width - hint_width - 8.0,
                    bounds.y + 11.0,
                    hint_width,
                    24.0,
                );
                ops.push(DrawOp::StrokeRounded(hint, theme.border, 4.0, 1.0));
                ops.push(DrawOp::PushClip(hint));
                text(
                    ops,
                    hint.x + 6.0,
                    hint.y + 5.0,
                    &entry.shortcut,
                    12.0,
                    theme.muted,
                );
                ops.push(DrawOp::PopClip);
            }
            ops.push(DrawOp::Fill(
                rect(bounds.x, bounds.y + bounds.height - 1.0, bounds.width, 1.0),
                theme.border,
            ));
        }
        ops.push(DrawOp::PopClip);
        ops.push(DrawOp::PushClip(rect(
            x + 10.0,
            self.list_bounds.y + self.list_bounds.height,
            panel_width - 20.0,
            32.0,
        )));
        text(
            ops,
            x + 10.0,
            self.list_bounds.y + self.list_bounds.height + 10.0,
            self.status
                .as_deref()
                .unwrap_or("↑↓ navigate · Enter run · Esc close"),
            12.0,
            theme.muted,
        );
        ops.push(DrawOp::PopClip);
        Ok(caret)
    }
    pub fn semantics(&self) -> Vec<Semantics> {
        if !self.open {
            return Vec::new();
        }
        let mut field = Semantics::new(
            ViewId(11000),
            SemanticRole::TextField,
            "Search commands",
            "view.command_palette",
            self.input_bounds,
            ControlState {
                focused: true,
                ..Default::default()
            },
        )
        .action(SemanticAction::Focus)
        .action(SemanticAction::SetValue);
        field.value = Some(self.field.value().into());
        let mut nodes = vec![field];
        for (row, entry) in self
            .entries
            .iter()
            .skip(self.first)
            .take(self.visible_rows)
            .enumerate()
        {
            let mut node = Semantics::new(
                ViewId(11001 + (self.first + row) as u64),
                SemanticRole::ListItem,
                &entry.accessible_name,
                entry.id.0,
                rect(
                    self.list_bounds.x,
                    self.list_bounds.y + row as f32 * ROW_HEIGHT,
                    self.list_bounds.width,
                    ROW_HEIGHT,
                ),
                ControlState {
                    disabled: !entry.state.enabled,
                    ..Default::default()
                },
            );
            node.selected = self.first + row == self.selected;
            node.value = Some(entry.shortcut.clone());
            node.invalid = entry.state.disabled_reason.clone();
            if entry.state.enabled {
                node.actions.push(SemanticAction::Invoke);
            }
            nodes.push(node);
        }
        nodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_commands::{CommandState, shell_commands};
    #[test]
    fn paint_and_pointer_use_current_results_and_expose_semantics() {
        let registry = shell_commands();
        let keymap = Keymap::defaults(&registry);
        let context = CommandContext::default();
        let mut palette = PaletteController::default();
        palette.show(&registry, &context, &keymap);
        palette.insert("locate", &registry, &context, &keymap);
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        let mut ops = Vec::new();
        palette.draw(&mut backend, 1120.0, 630.0, &mut ops).unwrap();
        assert!(bareline_renderer::balanced_clips(&ops));
        assert_eq!(palette.semantics()[1].command_id, "search.find");
        assert!(
            ops.iter()
                .any(|op| matches!(op, DrawOp::Text { text, .. } if text == "Ctrl+F"))
        );
        let point = Point {
            x: palette.list_bounds.x + 20.0,
            y: palette.list_bounds.y + 20.0,
        };
        assert_eq!(
            palette.click(&backend, point, &registry, &context).unwrap(),
            Some(CommandId("search.find"))
        );
        palette.release(&mut backend);
    }
    #[test]
    fn searches_navigates_and_revalidates_before_activation() {
        let registry = shell_commands();
        let keymap = Keymap::defaults(&registry);
        let mut context = CommandContext::default();
        let mut palette = PaletteController::default();
        palette.show(&registry, &context, &keymap);
        palette.insert("locate", &registry, &context, &keymap);
        assert_eq!(palette.selected(), Some(CommandId("search.find")));
        context.states.insert(
            CommandId("search.find"),
            CommandState::disabled("Document closed"),
        );
        assert_eq!(palette.key(Key::Enter, &registry, &context), None);
        assert_eq!(palette.status.as_deref(), Some("Document closed"));
        assert!(palette.open);
        context.states.clear();
        assert_eq!(
            palette.key(Key::Enter, &registry, &context),
            Some(CommandId("search.find"))
        );
        assert!(!palette.open);
    }
    #[test]
    fn composing_escape_cancels_preedit_before_palette() {
        let registry = shell_commands();
        let keymap = Keymap::defaults(&registry);
        let context = CommandContext::default();
        let mut palette = PaletteController::default();
        palette.show(&registry, &context, &keymap);
        palette.preedit("é".into(), Some((2, 2)));
        assert_eq!(palette.key(Key::Enter, &registry, &context), None);
        palette.key(Key::Escape, &registry, &context);
        assert!(palette.open);
        assert!(!palette.field.composing());
        palette.key(Key::Escape, &registry, &context);
        assert!(!palette.open);
    }
}
