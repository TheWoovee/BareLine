// SPDX-License-Identifier: MPL-2.0
use super::*;
use bareline_app::macros::{
    MacrosController,
    model::{
        MacroEvent, PlaybackState, Repeat,
        process::{
            ExternalDefinition, LaunchMode, PlaceholderContext, ProcessPermission, ProcessState,
        },
    },
};
use bareline_renderer::{DrawOp, Rect};
use bareline_ui::controls::{Key as UiKey, UiEvent};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    sync::mpsc,
};

enum FileResult {
    Macro(String),
    External(String),
    Saved,
}
pub struct MacrosRuntime {
    pub controller: MacrosController,
    external: Option<ExternalDefinition>,
    pending: Option<mpsc::Receiver<Result<FileResult, String>>>,
    bounds: Rect,
    focused: bool,
    next_name: u32,
}
impl Default for MacrosRuntime {
    fn default() -> Self {
        Self {
            controller: MacrosController::default(),
            external: None,
            pending: None,
            bounds: Rect::default(),
            focused: false,
            next_name: 1,
        }
    }
}
impl MacrosRuntime {
    pub fn annotate_context(&self, context: &mut bareline_commands::CommandContext) {
        use bareline_commands::{CommandId, CommandState};
        for (id, reason) in [
            (
                "macro.stop",
                (!self.controller.recorder.recording()).then_some("Macro recording is not active"),
            ),
            (
                "macro.play",
                self.controller
                    .selected
                    .is_none()
                    .then_some("Record or import a macro first"),
            ),
            (
                "macro.play_eof",
                self.controller
                    .selected
                    .is_none()
                    .then_some("Record or import a macro first"),
            ),
            (
                "macro.export",
                self.controller
                    .selected
                    .is_none()
                    .then_some("Select a macro first"),
            ),
            (
                "run.execute",
                self.external
                    .is_none()
                    .then_some("Load an external command definition first"),
            ),
        ] {
            if let Some(reason) = reason {
                context
                    .states
                    .insert(CommandId(id), CommandState::disabled(reason));
            }
        }
        if let Some(definition) = &self.external {
            context
                .states
                .entry(CommandId("run.execute"))
                .or_default()
                .label = Some(format!("Run {}", definition.name));
        }
    }
    pub fn height(&self) -> f32 {
        if self.controller.output_open {
            200.0
        } else {
            0.0
        }
    }
    pub fn draw(
        &mut self,
        _renderer: &mut WindowsRenderer,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) {
        self.bounds = bareline_ui::rect(
            0.0,
            (height - 24.0 - self.height()).max(0.0),
            width,
            self.height(),
        );
        self.controller.draw_output(self.bounds, ops);
    }
    fn read(
        &mut self,
        path: PathBuf,
        external: bool,
        notify: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("A macro/configuration file operation is pending".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("bareline-macro-file".into())
            .spawn(move || {
                let result = (|| {
                    let mut bytes = Vec::new();
                    std::fs::File::open(path)
                        .map_err(|error| error.to_string())?
                        .take(4 * 1024 * 1024 + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|error| error.to_string())?;
                    if bytes.len() > 4 * 1024 * 1024 {
                        return Err("Configuration file exceeds 4 MiB".into());
                    }
                    let text = String::from_utf8(bytes)
                        .map_err(|_| "Configuration must be UTF-8".to_string())?;
                    Ok(if external {
                        FileResult::External(text)
                    } else {
                        FileResult::Macro(text)
                    })
                })();
                let _ = tx.send(result);
                notify();
            })
            .map_err(|error| error.to_string())?;
        self.pending = Some(rx);
        Ok(())
    }
    fn save(
        &mut self,
        path: PathBuf,
        text: String,
        notify: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("A macro/configuration file operation is pending".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("bareline-macro-save".into())
            .spawn(move || {
                use bareline_platform::LocalFileSystem;
                let stage = path.with_file_name(format!(
                    ".bareline-macro-{}-{}.tmp",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ));
                let result = (|| {
                    let fs = bareline_platform_windows::WindowsFileSystem;
                    fs.validate_target(&path)
                        .map_err(|error| error.to_string())?;
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&stage)
                        .map_err(|error| error.to_string())?;
                    file.write_all(text.as_bytes())
                        .and_then(|()| file.sync_all())
                        .map_err(|error| error.to_string())?;
                    drop(file);
                    fs.commit(&stage, &path, path.exists())
                        .map_err(|error| error.to_string())?;
                    Ok(FileResult::Saved)
                })();
                if stage.exists() {
                    let _ = std::fs::remove_file(&stage);
                }
                let _ = tx.send(result);
                notify();
            })
            .map_err(|error| error.to_string())?;
        self.pending = Some(rx);
        Ok(())
    }
}
impl Shell {
    /// Call after the exact normalized Input has acknowledged success, including navigation.
    pub(super) fn macros_record_input(&mut self, input: &Input) {
        if !self.macros.controller.recorder.recording() {
            return;
        }
        let mut arguments = BTreeMap::new();
        let id = match input {
            Input::Insert(value) => {
                arguments.insert("text".into(), value.clone());
                "edit.insert_text"
            }
            Input::Backspace => "edit.backspace",
            Input::Delete => "edit.delete",
            Input::Undo => "edit.undo",
            Input::Redo => "edit.redo",
            Input::SelectAll => "edit.select_all",
            Input::Left(extend)
            | Input::Right(extend)
            | Input::Up(extend)
            | Input::Down(extend)
            | Input::Home(extend)
            | Input::End(extend) => {
                arguments.insert("extend".into(), extend.to_string());
                match input {
                    Input::Left(_) => "edit.move_left",
                    Input::Right(_) => "edit.move_right",
                    Input::Up(_) => "edit.move_up",
                    Input::Down(_) => "edit.move_down",
                    Input::Home(_) => "edit.move_home",
                    _ => "edit.move_end",
                }
            }
            _ => return,
        };
        if let Err(error) = self.macros.controller.recorded(
            MacroEvent::Command {
                id: id.into(),
                arguments,
            },
            true,
            &self.app.commands,
        ) {
            self.macros.controller.status = error;
        }
    }
    pub(super) fn macros_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        let result: Result<(), String> = match id {
            "macro.record" => self.macros.controller.record(),
            "macro.stop" => {
                if self.views.pending_edits()
                    || self.workspace.as_ref().is_some_and(|workspace| {
                        workspace.editors.iter().any(|editor| editor.busy())
                    })
                {
                    self.macros.controller.status =
                        "Wait for the pending edit before stopping recording.".into();
                    return true;
                }
                let name = format!("Macro {}", self.macros.next_name);
                let result = self
                    .macros
                    .controller
                    .stop_recording(&name, &self.app.commands);
                if result.is_ok() {
                    self.macros.next_name += 1;
                }
                result
            }
            "macro.play" | "macro.play_eof" => self.macros.controller.play(
                if id == "macro.play_eof" {
                    Repeat::UntilEof
                } else {
                    Repeat::Once
                },
                &self.app.commands,
            ),
            "macro.cancel" => {
                self.macros.controller.cancel();
                Ok(())
            }
            "run.cancel" => {
                self.macros.controller.cancel_process();
                Ok(())
            }
            "output.clear" => {
                self.macros.controller.clear_output();
                Ok(())
            }
            "output.close" => {
                self.macros.controller.output_open = false;
                self.macros.focused = false;
                Ok(())
            }
            "output.copy" => self
                .platform
                .as_ref()
                .unwrap()
                .set_clipboard_text(&self.macros.controller.copy_output())
                .map_err(|error| error.to_string()),
            "macro.import" | "run.load" => match self.platform.as_ref().unwrap().open_file() {
                Ok(Some(path)) => self
                    .macros
                    .read(path, id == "run.load", self.notify.clone()),
                Ok(None) => Ok(()),
                Err(error) => Err(error),
            },
            "macro.export" | "output.save" => {
                let text = if id == "macro.export" {
                    self.macros.controller.export_selected()
                } else {
                    Ok(self.macros.controller.copy_output())
                };
                match text {
                    Err(error) => Err(error),
                    Ok(text) => match self.platform.as_ref().unwrap().save_file() {
                        Ok(Some(path)) => self.macros.save(path, text, self.notify.clone()),
                        Ok(None) => Ok(()),
                        Err(error) => Err(error),
                    },
                }
            }
            "run.execute" => self.macros_run_loaded(),
            _ => return false,
        };
        if let Err(error) = result {
            self.macros.controller.status = error;
            self.macros.controller.output_open = true;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    fn macros_run_loaded(&mut self) -> Result<(), String> {
        let definition = self
            .macros
            .external
            .as_ref()
            .ok_or("Load a user command definition first")?;
        let context = match &self.workspace {
            Some(workspace) => {
                bareline_app::macros::placeholder_context(workspace, self.app.active)?
            }
            None => PlaceholderContext::default(),
        };
        let request = definition.request(&context)?;
        let (program, arguments) = match &request.mode {
            LaunchMode::Direct { program, arguments }
            | LaunchMode::Shell { program, arguments } => (program, arguments),
        };
        if !bareline_platform_windows::confirm_external_command(
            program,
            arguments,
            definition.shell,
        ) {
            return Ok(());
        }
        self.macros.controller.run(
            request,
            if definition.shell {
                ProcessPermission::UserGrantedShell
            } else {
                ProcessPermission::UserGrantedDirect
            },
            std::sync::Arc::new(bareline_platform_windows::WindowsProcessLauncher),
        )
    }
    pub(super) fn macros_pump(&mut self, el: &ActiveEventLoop) {
        if let Some(receiver) = &self.macros.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    self.macros.pending = None;
                    let result = match result {
                        Ok(FileResult::Macro(text)) => {
                            self.macros.controller.import(&text, &self.app.commands)
                        }
                        Ok(FileResult::External(text)) => ExternalDefinition::import_toml(&text)
                            .map(|definition| {
                                self.macros.controller.status =
                                    format!("Loaded {} — {}", definition.name, definition.program);
                                self.macros.external = Some(definition);
                            }),
                        Ok(FileResult::Saved) => {
                            self.macros.controller.status = "Saved".into();
                            Ok(())
                        }
                        Err(error) => Err(error),
                    };
                    if let Err(error) = result {
                        self.macros.controller.status = error;
                    }
                    self.macros.controller.output_open = true;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.macros.pending = None;
                    self.macros.controller.status = "File worker stopped".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let context = self.command_context();
        if let Some(workspace) = &mut self.workspace
            && let Some(state) = self.macros.controller.tick(
                Instant::now(),
                &self.app.commands,
                workspace,
                self.app.active,
                context,
            )
        {
            if matches!(state, PlaybackState::Running | PlaybackState::Waiting(_)) {
                el.set_control_flow(ControlFlow::WaitUntil(
                    Instant::now() + Duration::from_millis(16),
                ));
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        if self.macros.controller.refresh_output()
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        if self
            .macros
            .controller
            .process
            .as_ref()
            .is_some_and(|process| {
                matches!(
                    process.state(),
                    ProcessState::Starting | ProcessState::Running
                )
            })
        {
            el.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(30),
            ));
        }
    }
    pub(super) fn macros_event(&mut self, _el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if !self.macros.controller.output_open || self.palette.open {
            return false;
        }
        let ui = match event {
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } if self.macros.bounds.contains(self.pointer) => {
                self.macros.focused = true;
                Some(if *state == ElementState::Pressed {
                    UiEvent::PointerDown(self.pointer)
                } else {
                    UiEvent::PointerUp(self.pointer)
                })
            }
            WindowEvent::KeyboardInput { event, .. }
                if self.macros.focused && event.state == ElementState::Pressed =>
            {
                match event.logical_key {
                    Key::Named(NamedKey::ArrowUp) => Some(UiEvent::Key(UiKey::Up)),
                    Key::Named(NamedKey::ArrowDown) => Some(UiEvent::Key(UiKey::Down)),
                    Key::Named(NamedKey::Enter) => Some(UiEvent::Key(UiKey::Enter)),
                    _ => None,
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                self.macros.focused = false;
                None
            }
            _ => None,
        };
        if let Some(ui) = ui {
            if let Some(link) = self.macros.controller.output_event(ui)
                && let Some(workspace) = &mut self.workspace
            {
                workspace.open(link.path);
                self.macros.controller.status =
                    format!("Opening output location {}:{}", link.line, link.column);
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return true;
        }
        false
    }
}
