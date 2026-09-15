// SPDX-License-Identifier: MPL-2.0
//! Run… (F5) prompt. Collects an absolute program and its arguments, then
//! launches through the same confirmation and job-object path as a loaded
//! external-command definition. Direct mode never uses a shell.
use super::*;
use bareline_app::macros::model::process::{LaunchMode, ProcessRequest};
use bareline_renderer::{DrawOp, LayoutError, Rect};
use bareline_ui::{rect, text, text_field::TextField};

#[derive(Default)]
pub(super) struct RunPromptRuntime {
    pub open: bool,
    pub field: TextField,
    pub bounds: Rect,
    pub field_bounds: Rect,
    pub submit_bounds: Rect,
    pub cancel_bounds: Rect,
    pub status: String,
}

/// Split a command line on whitespace with double-quote grouping. The program
/// must be absolute so Direct mode cannot resolve a bare name from the current
/// directory (SEC-04).
pub(super) fn parse_command_line(input: &str) -> Result<(std::path::PathBuf, Vec<std::ffi::OsString>), String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in input.trim().chars() {
        match ch {
            '"' => quoted = !quoted,
            ch if ch.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            ch => current.push(ch),
        }
    }
    if quoted {
        return Err("Unclosed quote in the command line".into());
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    let program = tokens.first().ok_or("Type a program to run")?;
    let program = std::path::PathBuf::from(program);
    if !program.is_absolute() {
        return Err("Enter an absolute program path, for example C:\\Windows\\System32\\where.exe".into());
    }
    let arguments = tokens[1..].iter().map(std::ffi::OsString::from).collect();
    Ok((program, arguments))
}

impl RunPromptRuntime {
    pub(super) fn open(&mut self) {
        self.open = true;
        self.status.clear();
        self.field.select_all();
        self.field.insert("");
    }
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
        let w = width.clamp(320.0, 520.0);
        let y = 60.0f32.min((height - 150.0).max(0.0));
        self.bounds = rect((width - w) / 2.0, y, w, 136.0);
        let b = self.bounds;
        ops.push(DrawOp::Fill(b, theme.elevated));
        ops.push(DrawOp::Stroke(b, theme.border, 1.0));
        text(ops, b.x + 16.0, b.y + 12.0, "Run", 14.0, theme.text);
        self.field_bounds = rect(b.x + 16.0, b.y + 36.0, b.width - 32.0, 28.0);
        let caret =
            self.field
                .draw_with_theme(renderer, self.field_bounds, focused == modal::RUN_FIELD_ID, theme, ops)?;
        let hint = if self.status.is_empty() {
            "C:\\path\\program.exe arguments — runs directly, never through a shell"
        } else {
            self.status.as_str()
        };
        text(ops, b.x + 16.0, b.y + 70.0, hint, 12.0, theme.muted);
        self.cancel_bounds = rect(b.x + b.width - 166.0, b.y + 96.0, 70.0, 28.0);
        self.submit_bounds = rect(b.x + b.width - 86.0, b.y + 96.0, 70.0, 28.0);
        for (bounds, label, fill, id) in [
            (self.cancel_bounds, "Cancel", theme.elevated, modal::RUN_CANCEL_ID),
            (self.submit_bounds, "Run", theme.focus, modal::RUN_SUBMIT_ID),
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
        Ok((focused == modal::RUN_FIELD_ID).then_some(caret))
    }
}

impl Shell {
    pub(super) fn run_prompt_submit(&mut self) {
        let input = self.run_prompt.field.value().to_string();
        match parse_command_line(&input) {
            Ok((program, arguments)) => {
                let directory = self
                    .settings
                    .workspace_root()
                    .map(std::path::Path::to_path_buf)
                    .or_else(|| std::env::current_dir().ok());
                let request = ProcessRequest {
                    mode: LaunchMode::Direct { program, arguments },
                    directory,
                    capture: true,
                };
                match self.macros_confirm_run(request) {
                    Ok(()) => {
                        self.dismiss_modal(modal::ModalSurface::Run);
                    }
                    Err(error) => self.run_prompt.status = error,
                }
            }
            Err(error) => self.run_prompt.status = error,
        }
    }
    pub(super) fn run_prompt_event(&mut self, _el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if !self.run_prompt.open || self.palette.open {
            return false;
        }
        match event {
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let focused = self.modal.map_or(modal::RUN_FIELD_ID, |modal| modal.focused);
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        let composing = self.run_prompt.field.composing();
                        self.run_prompt.field.cancel();
                        if !composing {
                            self.dismiss_modal(modal::ModalSurface::Run);
                        }
                    }
                    Key::Named(NamedKey::Tab) => {
                        let controls = [modal::RUN_FIELD_ID, modal::RUN_SUBMIT_ID, modal::RUN_CANCEL_ID];
                        let current = controls.iter().position(|id| *id == focused).unwrap_or(0);
                        let next = if self.modifiers.shift_key() {
                            (current + controls.len() - 1) % controls.len()
                        } else {
                            (current + 1) % controls.len()
                        };
                        self.modal_focus(modal::ModalSurface::Run, controls[next]);
                    }
                    Key::Named(NamedKey::Enter | NamedKey::Space) if focused == modal::RUN_CANCEL_ID => {
                        self.dismiss_modal(modal::ModalSurface::Run);
                    }
                    Key::Named(NamedKey::Enter | NamedKey::Space) if focused == modal::RUN_SUBMIT_ID => {
                        self.run_prompt_submit();
                    }
                    Key::Named(NamedKey::Enter) => self.run_prompt_submit(),
                    key if focused == modal::RUN_FIELD_ID => {
                        let field = &mut self.run_prompt.field;
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
                        self.run_prompt.status.clear();
                    }
                    _ => {}
                }
            }
            WindowEvent::Ime(ime)
                if self
                    .modal
                    .is_some_and(|modal| modal.active_text_owner == Some(modal::RUN_FIELD_ID)) =>
            {
                let field = &mut self.run_prompt.field;
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
                if !self.run_prompt.bounds.contains(self.pointer) {
                    self.dismiss_modal(modal::ModalSurface::Run);
                } else if self.run_prompt.submit_bounds.contains(self.pointer) {
                    self.run_prompt_submit();
                } else if self.run_prompt.cancel_bounds.contains(self.pointer) {
                    self.dismiss_modal(modal::ModalSurface::Run);
                } else if let Some(renderer) = &self.renderer {
                    let _ = self
                        .run_prompt
                        .field
                        .click(renderer, self.pointer, self.modifiers.shift_key());
                    self.modal_focus(modal::ModalSurface::Run, modal::RUN_FIELD_ID);
                }
            }
            WindowEvent::Focused(false) => {
                self.run_prompt.field.cancel();
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
    fn parses_quoted_programs_and_arguments_and_requires_an_absolute_path() {
        let (program, arguments) =
            parse_command_line(r#""C:\Program Files\Tool\tool.exe" --flag "a b" plain"#).unwrap();
        assert_eq!(program, std::path::PathBuf::from(r"C:\Program Files\Tool\tool.exe"));
        assert_eq!(
            arguments,
            vec![
                std::ffi::OsString::from("--flag"),
                std::ffi::OsString::from("a b"),
                std::ffi::OsString::from("plain"),
            ]
        );
        assert!(parse_command_line("").is_err());
        assert!(parse_command_line("where.exe").is_err());
        assert!(parse_command_line(r#""C:\unclosed"#).is_err());
    }

    #[test]
    fn keyboard_button_focus_has_a_visible_cue_and_hides_the_field_caret() {
        let mut prompt = RunPromptRuntime::default();
        prompt.open();
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let theme = bareline_ui::theme::UiTheme::default();
        let mut ops = Vec::new();
        let caret = prompt
            .draw(&mut renderer, 1000.0, 800.0, theme, modal::RUN_CANCEL_ID, &mut ops)
            .unwrap();
        assert!(caret.is_none());
        assert!(ops.iter().any(|op| {
            matches!(op, DrawOp::StrokeRounded(bounds, color, _, width)
                if *bounds == prompt.cancel_bounds && *color == theme.focus && *width == 2.0)
        }));
    }
}
