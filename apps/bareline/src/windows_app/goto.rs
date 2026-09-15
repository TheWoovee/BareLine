// SPDX-License-Identifier: MPL-2.0
//! Go To Line overlay: a small prompt that accepts an absolute line, a
//! line:column, a relative +/- jump or a percentage, and moves the active
//! editor there. Paged documents resolve the target through the line index,
//! falling back to a byte position while the index is still being built.
use super::*;
use bareline_app::workspace::WorkspaceEditor;
use bareline_commands::{CommandId, CommandRegistry, CommandSpec};
use bareline_document::TextOffset;
use bareline_editor_surface::{EditorSurface, Selection, paged_view::PagedEditorSurface};
use bareline_renderer::{DrawOp, LayoutError, Rect};
use bareline_ui::{rect, text, text_field::TextField};

#[derive(Default)]
pub(super) struct GotoRuntime {
    pub open: bool,
    pub field: TextField,
    pub bounds: Rect,
    pub field_bounds: Rect,
    pub submit_bounds: Rect,
    pub cancel_bounds: Rect,
    pub status: String,
}

/// What the user typed, before it is resolved against the current document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum GotoRequest {
    /// A 1-based line, optionally with a 1-based column.
    Absolute { line: u64, column: Option<u64> },
    /// A signed number of lines relative to the caret's current line.
    Relative(i64),
    /// A position expressed as a percentage (0-100) through the document.
    Percent(f64),
}

/// Parse the overlay input. Accepts `123`, `123:4`, `+10`, `-10` and `50%`;
/// returns `None` for anything else (empty, non-numeric, zero line/column).
pub(super) fn parse_goto(input: &str) -> Option<GotoRequest> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_suffix('%') {
        let value: f64 = rest.trim().parse().ok()?;
        if !value.is_finite() || !(0.0..=100.0).contains(&value) {
            return None;
        }
        return Some(GotoRequest::Percent(value));
    }
    if let Some(rest) = text.strip_prefix('+') {
        let value: i64 = rest.trim().parse().ok()?;
        return Some(GotoRequest::Relative(value));
    }
    if let Some(rest) = text.strip_prefix('-') {
        let value: i64 = rest.trim().parse().ok()?;
        return Some(GotoRequest::Relative(-value));
    }
    let mut parts = text.splitn(2, ':');
    let line: u64 = parts.next()?.trim().parse().ok()?;
    if line == 0 {
        return None;
    }
    let column = match parts.next() {
        Some(raw) => {
            let column: u64 = raw.trim().parse().ok()?;
            if column == 0 {
                return None;
            }
            Some(column)
        }
        None => None,
    };
    Some(GotoRequest::Absolute { line, column })
}

pub(super) fn register(registry: &mut CommandRegistry) {
    let id = CommandId("search.goto");
    // The Ctrl+G accelerator is carried on the command spec, so the default
    // keymap (built from the registry) binds it automatically.
    let _ = registry.register(CommandSpec {
        id,
        title: "Go to Line…",
        category: "Search",
        shortcut: "Ctrl+G",
        action: Action::Contributed(id),
    });
}

/// Move a resident (fully in-memory) editor to the resolved position.
fn navigate_resident(surface: &mut EditorSurface, request: GotoRequest) -> Result<Option<String>, String> {
    let snapshot = surface.snapshot();
    let total = snapshot.line_count().max(1);
    let current = snapshot.line_at(TextOffset(surface.selection.caret)).unwrap_or(0);
    let (line0, column) = match request {
        GotoRequest::Absolute { line, column } => (((line - 1) as usize).min(total - 1), column),
        GotoRequest::Relative(delta) => {
            let target = (current as i64 + delta).clamp(0, total as i64 - 1);
            (target as usize, None)
        }
        GotoRequest::Percent(pct) => {
            let target = ((pct / 100.0) * (total as f64 - 1.0)).round() as usize;
            (target.min(total - 1), None)
        }
    };
    let range = snapshot
        .line_range(line0)
        .map_err(|error| format!("Line unavailable: {error:?}"))?;
    let mut offset = range.start.0;
    if let Some(column) = column {
        let text = snapshot.read(range.start..range.end, 1 << 20).unwrap_or_default();
        let trimmed = text.trim_end_matches(['\r', '\n']);
        let byte = trimmed
            .char_indices()
            .nth((column - 1) as usize)
            .map_or(trimmed.len(), |(index, _)| index);
        offset = range.start.0 + byte;
    }
    // set_selections snaps to grapheme boundaries and flags the caret to be
    // revealed on the next frame.
    surface.set_selections(
        Selection {
            anchor: offset,
            caret: offset,
        }
        .into(),
    )?;
    Ok(None)
}

/// Move a paged (very large) editor. Line targets go through the line index;
/// percentages use a byte position because exact line totals are not known
/// until indexing finishes.
fn navigate_paged(paged: &mut PagedEditorSurface, request: GotoRequest, height: f32) -> Result<Option<String>, String> {
    match request {
        GotoRequest::Absolute { line, .. } => {
            paged.request_global_scroll(line - 1, 0.0, 0.0)?;
            Ok(None)
        }
        GotoRequest::Relative(delta) => {
            let base = paged
                .viewport_first_global_line()
                .ok_or("The current line is still being indexed; try again in a moment.")?;
            let target = if delta < 0 {
                base.saturating_sub(delta.unsigned_abs())
            } else {
                base.saturating_add(delta as u64)
            };
            paged.request_global_scroll(target, 0.0, 0.0)?;
            Ok(None)
        }
        GotoRequest::Percent(pct) => {
            paged.request_byte_scroll_in_view(pct / 100.0, height)?;
            Ok(Some(
                "Jumped to an approximate position while the document finishes indexing.".into(),
            ))
        }
    }
}

fn navigate(editor: &mut WorkspaceEditor, request: GotoRequest, height: f32) -> Result<Option<String>, String> {
    match editor {
        WorkspaceEditor::Resident(surface) => navigate_resident(surface, request),
        WorkspaceEditor::Paged(paged) => navigate_paged(paged, request, height),
    }
}

impl GotoRuntime {
    pub(super) fn draw(
        &mut self,
        renderer: &mut impl bareline_renderer::TextBackend,
        width: f32,
        height: f32,
        theme: bareline_ui::theme::UiTheme,
        focused: u64,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        if !self.open {
            self.field.release(renderer);
            return Ok(None);
        }
        let w = width.clamp(240.0, 360.0);
        let y = 60.0f32.min((height - 150.0).max(0.0));
        self.bounds = rect((width - w) / 2.0, y, w, 136.0);
        let b = self.bounds;
        ops.push(DrawOp::Fill(b, theme.elevated));
        ops.push(DrawOp::Stroke(b, theme.border, 1.0));
        text(ops, b.x + 16.0, b.y + 12.0, "Go to line", 14.0, theme.text);
        self.field_bounds = rect(b.x + 16.0, b.y + 36.0, b.width - 32.0, 28.0);
        let caret =
            self.field
                .draw_with_theme(renderer, self.field_bounds, focused == modal::GOTO_FIELD_ID, theme, ops)?;
        let hint = if self.status.is_empty() {
            "line, line:column, +/- lines or NN%"
        } else {
            self.status.as_str()
        };
        text(ops, b.x + 16.0, b.y + 70.0, hint, 12.0, theme.muted);
        self.cancel_bounds = rect(b.x + b.width - 156.0, b.y + 96.0, 66.0, 28.0);
        self.submit_bounds = rect(b.x + b.width - 80.0, b.y + 96.0, 64.0, 28.0);
        for (bounds, label, fill, id) in [
            (self.cancel_bounds, "Cancel", theme.elevated, modal::GOTO_CANCEL_ID),
            (self.submit_bounds, "Go", theme.focus, modal::GOTO_SUBMIT_ID),
        ] {
            ops.push(DrawOp::FillRounded(bounds, fill, 4.0));
            ops.push(DrawOp::StrokeRounded(
                bounds,
                if focused == id { theme.focus } else { theme.border },
                4.0,
                if focused == id { 2.0 } else { 1.0 },
            ));
            text(ops, bounds.x + 12.0, bounds.y + 6.0, label, 12.0, theme.text);
        }
        Ok((focused == modal::GOTO_FIELD_ID).then_some(caret))
    }
}

impl Shell {
    pub(super) fn goto_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if id != "search.goto" {
            return false;
        }
        self.palette.dismiss();
        self.app.palette = false;
        self.finish_palette_focus();
        self.activate_modal(modal::ModalSurface::Goto);
        self.goto.open = true;
        self.goto.status.clear();
        self.goto.field.select_all();
        self.goto.field.insert("");
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn goto_submit(&mut self) {
        let Some(request) = parse_goto(self.goto.field.value()) else {
            self.goto.status = "Enter a line like 120, 120:8, +10, -10 or 50%".into();
            return;
        };
        let height = self.editor_bounds().height.max(1.0);
        let active = self.app.active;
        let result = self
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.editors.get_mut(active))
            .ok_or_else(|| "Open a document first".to_owned())
            .and_then(|editor| navigate(editor, request, height));
        match result {
            Ok(note) => {
                self.dismiss_modal(modal::ModalSurface::Goto);
                if let (Some(note), Some(workspace)) = (note, self.workspace.as_mut()) {
                    workspace.message = Some(note);
                }
            }
            Err(error) => self.goto.status = error,
        }
    }
    pub(super) fn goto_event(&mut self, _el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if !self.goto.open || self.palette.open {
            return false;
        }
        match event {
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let focused = self.modal.map_or(modal::GOTO_FIELD_ID, |modal| modal.focused);
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        let composing = self.goto.field.composing();
                        self.goto.field.cancel();
                        if !composing {
                            self.dismiss_modal(modal::ModalSurface::Goto);
                        }
                    }
                    Key::Named(NamedKey::Tab) => {
                        let controls = [modal::GOTO_FIELD_ID, modal::GOTO_SUBMIT_ID, modal::GOTO_CANCEL_ID];
                        let current = controls.iter().position(|id| *id == focused).unwrap_or(0);
                        let next = if self.modifiers.shift_key() {
                            (current + controls.len() - 1) % controls.len()
                        } else {
                            (current + 1) % controls.len()
                        };
                        self.modal_focus(modal::ModalSurface::Goto, controls[next]);
                    }
                    Key::Named(NamedKey::Enter | NamedKey::Space) if focused == modal::GOTO_CANCEL_ID => {
                        self.dismiss_modal(modal::ModalSurface::Goto);
                    }
                    Key::Named(NamedKey::Enter | NamedKey::Space) if focused == modal::GOTO_SUBMIT_ID => {
                        self.goto_submit();
                    }
                    Key::Named(NamedKey::Enter) => self.goto_submit(),
                    key if focused == modal::GOTO_FIELD_ID => {
                        let field = &mut self.goto.field;
                        match key {
                            Key::Named(NamedKey::Backspace) => {
                                field.delete(false);
                            }
                            Key::Named(NamedKey::Delete) => {
                                field.delete(true);
                            }
                            Key::Named(NamedKey::ArrowLeft) => field.horizontal(false, self.modifiers.shift_key()),
                            Key::Named(NamedKey::ArrowRight) => field.horizontal(true, self.modifiers.shift_key()),
                            Key::Named(NamedKey::Home) => field.edge(false, self.modifiers.shift_key()),
                            Key::Named(NamedKey::End) => field.edge(true, self.modifiers.shift_key()),
                            Key::Character(v) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                                match v.to_lowercase().as_str() {
                                    "a" => field.select_all(),
                                    "c" | "x" => {
                                        if let Some(platform) = &self.platform
                                            && platform.set_clipboard_text(field.selected()).is_ok()
                                            && v.eq_ignore_ascii_case("x")
                                        {
                                            field.insert("");
                                        }
                                    }
                                    "v" => {
                                        if let Some(platform) = &self.platform
                                            && let Ok(value) = platform.clipboard_text()
                                        {
                                            field.commit(&value);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            _ if !self.modifiers.control_key() || self.modifiers.alt_key() => {
                                if let Some(value) = &event.text {
                                    field.insert(value);
                                }
                            }
                            _ => {}
                        }
                        self.goto.status.clear();
                    }
                    _ => {}
                }
            }
            WindowEvent::Ime(ime)
                if self
                    .modal
                    .is_some_and(|modal| modal.active_text_owner == Some(modal::GOTO_FIELD_ID)) =>
            {
                let field = &mut self.goto.field;
                match ime {
                    Ime::Preedit(v, c) => field.preedit(v.clone(), *c),
                    Ime::Commit(v) => {
                        field.commit(v);
                    }
                    Ime::Disabled => field.cancel(),
                    Ime::Enabled => {}
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !self.goto.bounds.contains(self.pointer) {
                    self.dismiss_modal(modal::ModalSurface::Goto);
                } else if self.goto.submit_bounds.contains(self.pointer) {
                    self.goto_submit();
                } else if self.goto.cancel_bounds.contains(self.pointer) {
                    self.dismiss_modal(modal::ModalSurface::Goto);
                } else if let Some(renderer) = &self.renderer {
                    let _ = self
                        .goto
                        .field
                        .click(renderer, self.pointer, self.modifiers.shift_key());
                    self.modal_focus(modal::ModalSurface::Goto, modal::GOTO_FIELD_ID);
                }
            }
            WindowEvent::Focused(false) => {
                self.goto.field.cancel();
                return false;
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
                return false;
            }
            WindowEvent::CursorMoved { .. }
            | WindowEvent::RedrawRequested
            | WindowEvent::CloseRequested
            | WindowEvent::Resized(_)
            | WindowEvent::ScaleFactorChanged { .. } => return false,
            _ => {}
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
    #[test]
    fn parses_all_five_forms_and_rejects_invalid_input() {
        assert_eq!(
            parse_goto("123"),
            Some(GotoRequest::Absolute {
                line: 123,
                column: None
            })
        );
        assert_eq!(
            parse_goto("123:4"),
            Some(GotoRequest::Absolute {
                line: 123,
                column: Some(4)
            })
        );
        assert_eq!(parse_goto("+10"), Some(GotoRequest::Relative(10)));
        assert_eq!(parse_goto("-10"), Some(GotoRequest::Relative(-10)));
        assert_eq!(parse_goto("50%"), Some(GotoRequest::Percent(50.0)));
        // Invalid: empty, non-numeric, zero line/column, out-of-range percent,
        // and a bare sign.
        assert_eq!(parse_goto(""), None);
        assert_eq!(parse_goto("   "), None);
        assert_eq!(parse_goto("abc"), None);
        assert_eq!(parse_goto("0"), None);
        assert_eq!(parse_goto("12:0"), None);
        assert_eq!(parse_goto("1:2:3"), None);
        assert_eq!(parse_goto("150%"), None);
        assert_eq!(parse_goto("-"), None);
    }

    #[test]
    fn keyboard_button_focus_has_a_visible_cue_and_hides_the_field_caret() {
        let mut prompt = GotoRuntime::default();
        prompt.open = true;
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let theme = bareline_ui::theme::UiTheme::default();
        let mut ops = Vec::new();
        let caret = prompt
            .draw(&mut renderer, 1000.0, 800.0, theme, modal::GOTO_SUBMIT_ID, &mut ops)
            .unwrap();
        assert!(caret.is_none());
        assert!(ops.iter().any(|op| {
            matches!(op, DrawOp::StrokeRounded(bounds, color, _, width)
                if *bounds == prompt.submit_bounds && *color == theme.focus && *width == 2.0)
        }));
    }
}
