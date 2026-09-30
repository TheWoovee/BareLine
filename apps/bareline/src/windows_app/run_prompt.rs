// SPDX-License-Identifier: MPL-2.0
//! Run… (F5) prompt. Collects a program (absolute, or a name found on `PATH`)
//! and its arguments with Notepad++ `$(...)` variables, then launches through
//! the same placeholder quoting, confirmation and job-object path as a loaded
//! external-command definition. Only cmd.exe and batch files use shell mode.
use super::*;
use bareline_app::macros::model::process::{self, ExternalDefinition};
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

/// Help text under the Run field; also the field's accessible description.
pub(super) const RUN_HINT: &str = "tool.exe or C:\\path\\tool.exe \"$(FULL_CURRENT_PATH)\"";

/// A Run line: the program token, the text after it as typed (cmd.exe parses
/// that itself), and that text split on whitespace with double-quote grouping.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RunLine {
    pub program: String,
    pub remainder: String,
    pub arguments: Vec<String>,
}

pub(super) fn parse_command_line(input: &str) -> Result<RunLine, String> {
    let input = input.trim();
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut remainder = input.len();
    for (index, ch) in input.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            ch if ch.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                    if tokens.len() == 1 {
                        remainder = index;
                    }
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
    let mut tokens = tokens.into_iter();
    let program = tokens.next().ok_or("Type a program to run")?;
    Ok(RunLine {
        program,
        remainder: input[remainder..].trim_start().to_string(),
        arguments: tokens.collect(),
    })
}

/// Builds the definition a Run line stands for. `resolve` turns the program
/// token into an absolute path (a `PATH` lookup that never searches the current
/// directory, SEC-04). Notepad++ variables map onto the shared placeholders, so
/// they get the same per-interpreter quoting (SEC-10). Bareline's own `${...}`
/// placeholders expand as well, so a Run line cannot carry literal `${...}` text.
pub(super) fn run_definition(
    line: &RunLine,
    resolve: impl Fn(&str) -> Result<std::path::PathBuf, String>,
) -> Result<ExternalDefinition, String> {
    let resolved = resolve(&line.program)?;
    let remainder = process::normalize_notepad_variables(&line.remainder);
    let batch = resolved
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd"));
    let (program, arguments, shell) = if batch {
        let script = resolved.to_str().ok_or("The program path is not valid Unicode")?;
        if script.contains(['"', '%', '!', '$']) {
            return Err("This batch file path cannot be passed to cmd.exe safely".into());
        }
        let shell = process::system_command_shell().ok_or("%SystemRoot% is not set")?;
        let command = format!("\"{script}\" {remainder}").trim_end().to_string();
        (shell, vec!["/c".to_string(), command], true)
    } else if process::Interpreter::of(&resolved) == Some(process::Interpreter::CommandShell) {
        // cmd.exe reads the rest of the line itself, so it gets the text as typed.
        (resolved, vec![remainder], true)
    } else {
        let arguments = line
            .arguments
            .iter()
            .map(|argument| process::normalize_notepad_variables(argument))
            .collect();
        (resolved, arguments, false)
    };
    Ok(ExternalDefinition {
        name: "Run".into(),
        program: program.to_str().ok_or("The program path is not valid Unicode")?.into(),
        arguments,
        shell,
        capture: true,
    })
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
            RUN_HINT
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
        let line = match parse_command_line(&input) {
            Ok(line) => line,
            Err(error) => {
                self.run_prompt.status = error;
                return;
            }
        };
        if std::path::Path::new(&line.program).is_absolute() {
            // An absolute program is used as typed; resolving it touches no file.
            let result = run_definition(&line, bareline_platform_windows::resolve_program)
                .and_then(|definition| self.macros_run_definition(definition));
            self.run_prompt_finish(result);
            return;
        }
        // A PATH lookup probes files in every PATH folder, and a slow or disconnected
        // network folder would stall the window, so it runs on a worker (P1-E5).
        let program = line.program.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let notify = self.notify.clone();
        let spawned = std::thread::Builder::new()
            .name("bareline-run-lookup".into())
            .spawn(move || {
                let _ = tx.send(run_definition(&line, bareline_platform_windows::resolve_program));
                notify();
            });
        match spawned {
            Ok(_) => {
                self.run_prompt.status = format!("Looking up {program} on PATH… Cancel stops waiting.");
                self.macros.run_lookup = Some((input, rx));
            }
            Err(error) => self.run_prompt.status = error.to_string(),
        }
    }
    /// Delivers a finished `PATH` lookup. The result is dropped when its line is no
    /// longer in the open prompt (the prompt was cancelled, closed or edited since).
    pub(super) fn run_prompt_poll(&mut self) {
        let Some((input, receiver)) = &self.macros.run_lookup else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("The PATH lookup stopped unexpectedly".into()),
        };
        let current = self.run_prompt.open && self.run_prompt.field.value() == input.as_str();
        self.macros.run_lookup = None;
        if current {
            let result = result.and_then(|definition| self.macros_run_definition(definition));
            self.run_prompt_finish(result);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
    fn run_prompt_finish(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.dismiss_modal(modal::ModalSurface::Run);
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
                                            && let Ok(Some(value)) =
                                                platform.clipboard_text_within(bareline_ui::text_field::LIMIT)
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
    fn parses_quoted_programs_and_keeps_the_typed_remainder() {
        let line = parse_command_line(r#"  "C:\Program Files\Tool\tool.exe"   --flag "a b" plain "#).unwrap();
        assert_eq!(line.program, r"C:\Program Files\Tool\tool.exe");
        assert_eq!(line.remainder, r#"--flag "a b" plain"#);
        assert_eq!(line.arguments, ["--flag", "a b", "plain"]);
        let bare = parse_command_line("where.exe").unwrap();
        assert_eq!((bare.program.as_str(), bare.remainder.as_str()), ("where.exe", ""));
        assert!(bare.arguments.is_empty());
        assert!(parse_command_line("").is_err());
        assert!(parse_command_line(r#""C:\unclosed"#).is_err());
    }

    #[test]
    fn run_lines_resolve_on_path_and_map_notepad_plus_plus_variables() {
        let system_cmd = process::system_command_shell().unwrap();
        let resolve = |name: &str| -> Result<std::path::PathBuf, String> {
            match name {
                "where" => Ok(std::path::PathBuf::from(r"C:\Windows\System32\where.exe")),
                "cmd" => Ok(system_cmd.clone()),
                "build" => Ok(std::path::PathBuf::from(r"C:\tools\build.bat")),
                "pwsh" => Ok(std::path::PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe")),
                name if std::path::Path::new(name).is_absolute() => Ok(name.into()),
                _ => Err(format!("{name} was not found on PATH")),
            }
        };
        let run = |input: &str| run_definition(&parse_command_line(input).unwrap(), &resolve);
        // A bare name is shown and launched as its resolved absolute path.
        let direct = run(r#"where "$(FULL_CURRENT_PATH)" $(CURRENT_WORD):$(CURRENT_LINE)"#).unwrap();
        assert_eq!(direct.program, r"C:\Windows\System32\where.exe");
        assert_eq!(direct.arguments, ["${file}", "${word}:${line}"]);
        assert!(!direct.shell);
        assert!(run(r".\tool.exe").is_err());
        // cmd.exe gets the typed text as one shell command with the same variables.
        let shell = run(r#"cmd /c type "$(FULL_CURRENT_PATH)" & echo $(FILE_NAME)"#).unwrap();
        assert!(shell.shell);
        assert_eq!(shell.program, system_cmd.to_str().unwrap());
        assert_eq!(shell.arguments, [r#"/c type "${file}" & echo ${file_name}"#]);
        let batch = run("build $(NAME_PART)").unwrap();
        assert!(batch.shell);
        assert_eq!(batch.program, system_cmd.to_str().unwrap());
        assert_eq!(batch.arguments, ["/c", r#""C:\tools\build.bat" ${name_part}"#]);
        // The mapped variables get the interpreter's quoting when the request is built.
        let context = bareline_app::macros::model::process::PlaceholderContext {
            file: Some(std::path::PathBuf::from(r"C:\notes\a & b.txt")),
            word: "$(calc)".into(),
            ..Default::default()
        };
        let request = run("pwsh -c Write-Output $(CURRENT_WORD)")
            .unwrap()
            .request(&context)
            .unwrap();
        assert!(matches!(
            request.mode,
            process::LaunchMode::Direct { ref arguments, .. } if arguments[2] == "'$(calc)'"
        ));
        let request = shell.request(&context).unwrap();
        assert!(matches!(
            request.mode,
            process::LaunchMode::Shell { ref arguments, .. }
                if arguments[..] == [std::ffi::OsString::from(r#"/c type "C:\notes\a & b.txt" & echo "a & b.txt""#)]
        ));
        // cmd.exe quoting cannot protect a value from an interpreter it starts.
        let nested = run("cmd /c powershell -c Write-Output $(CURRENT_WORD)").unwrap();
        assert!(nested.shell);
        assert!(nested.request(&context).is_err());
        // A batch file receives the value as one quoted argument.
        assert!(batch.request(&context).is_ok());
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
