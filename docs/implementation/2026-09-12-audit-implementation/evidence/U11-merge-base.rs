// SPDX-License-Identifier: MPL-2.0
use super::*;
mod location;
use bareline_app::macros::{
    MacrosController,
    model::{
        PlaybackState, Repeat,
        process::{ExternalDefinition, LaunchMode, PlaceholderContext, ProcessPermission, ProcessState},
    },
};
use bareline_renderer::{DrawOp, Rect};
use bareline_ui::controls::{Key as UiKey, UiEvent};
use std::{
    io::{Read, Write},
    sync::mpsc,
};

#[cfg(test)]
pub(super) fn accessibility_test_cases() -> Vec<(
    &'static str,
    Vec<bareline_platform::accessibility::AccessibilityNode>,
    Option<u64>,
)> {
    fn snapshot(
        runtime: &mut MacrosRuntime,
    ) -> (Vec<bareline_platform::accessibility::AccessibilityNode>, Option<u64>) {
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        let mut ops = Vec::new();
        runtime.bounds = bareline_ui::rect(0., 800. - 24. - runtime.height(), 1000., runtime.height());
        runtime.controller.draw_output(runtime.bounds, runtime.theme, &mut ops);
        runtime
            .controller
            .manager
            .draw(
                &mut backend,
                1000.,
                800.,
                &runtime.controller.status,
                runtime.theme,
                &runtime.command_context,
                &mut ops,
            )
            .unwrap();
        let semantics = runtime.controller.semantics();
        let focus = semantics.iter().find(|node| node.focused).map(|node| node.id.0);
        let nodes = semantics
            .iter()
            .map(|node| bareline_app::accessibility::semantic_node(node, 1))
            .collect();
        runtime.controller.manager.release(&mut backend);
        (nodes, focus)
    }
    let mut runtime = MacrosRuntime::default();
    runtime.storage_ready = true;
    runtime.controller.library.insert(
        "Example".into(),
        bareline_app::macros::model::Macro {
            name: "Example".into(),
            events: Vec::new(),
        },
    );
    runtime.controller.selected = Some("Example".into());
    let mut cases = Vec::new();
    let (nodes, focus) = snapshot(&mut runtime);
    cases.push(("macros_closed", nodes, focus));
    runtime.controller.show_manager();
    let (nodes, focus) = snapshot(&mut runtime);
    cases.push(("macros_manager", nodes, focus));
    assert!(runtime.controller.manager.accessibility(23200, false).is_none());
    assert_eq!(runtime.controller.selected.as_deref(), Some("Example"));
    let (nodes, focus) = snapshot(&mut runtime);
    cases.push(("macros_button_focus", nodes, focus));
    runtime.controller.manager.accessibility(23100, false);
    let field = runtime.controller.manager.active_field().unwrap();
    field.select_all();
    field.insert("Renamed value");
    assert!(runtime.controller.library.contains_key("Example"));
    let (nodes, focus) = snapshot(&mut runtime);
    cases.push(("macros_name_value", nodes, focus));
    runtime.controller.manager.dismiss();
    runtime.controller.output_open = true;
    runtime.controller.status = "Process exited with code 0".into();
    let (nodes, focus) = snapshot(&mut runtime);
    cases.push(("macros_output_empty", nodes, focus));
    let mut output = bareline_app::macros::model::process::OutputBuffer::new(4096);
    output.push(
        bareline_app::macros::model::process::OutputStream::Stdout,
        b"src/example.rs:12:3\nBuild finished\n",
    );
    let value = output.text();
    runtime
        .controller
        .update_output_snapshot(Some(&value), (output.byte_len(), output.discarded_bytes), "Exited(0)");
    let (nodes, focus) = snapshot(&mut runtime);
    cases.push(("macros_output_populated", nodes, focus));
    assert!(runtime.controller.output_accessibility(2_000_000, false).is_none());
    let (nodes, focus) = snapshot(&mut runtime);
    cases.push(("macros_output_link_focus", nodes, focus));
    runtime.controller.output_open = false;
    let (nodes, focus) = snapshot(&mut runtime);
    cases.push(("macros_closed_after_output", nodes, focus));
    cases
}

enum FileResult {
    Macro(String),
    External(String),
    Saved,
    Library(Vec<(usize, String)>, Option<String>),
    Prepared(bareline_app::macros::model::process::ProcessRequest),
}
pub struct MacrosRuntime {
    pub(super) next_tick: Option<Instant>,
    pub controller: MacrosController,
    external: Option<ExternalDefinition>,
    pending: Option<mpsc::Receiver<Result<FileResult, String>>>,
    bounds: Rect,
    focused: bool,
    next_name: u32,
    directory: Option<PathBuf>,
    loaded: bool,
    storage_ready: bool,
    dirty: bool,
    output_target: Option<bareline_app::macros::model::process::OutputLink>,
    location: Option<
        mpsc::Receiver<Result<(bareline_document::paged::PagedSnapshot, bareline_document::TextOffset), String>>,
    >,
    location_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    prepare_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    output_directory: Option<PathBuf>,
    theme: bareline_ui::theme::UiTheme,
    command_context: bareline_commands::CommandContext,
    power_replay: Option<u64>,
}
impl Default for MacrosRuntime {
    fn default() -> Self {
        Self {
            next_tick: None,
            controller: MacrosController::default(),
            external: None,
            pending: None,
            bounds: Rect::default(),
            focused: false,
            next_name: 1,
            directory: None,
            loaded: false,
            storage_ready: false,
            dirty: false,
            output_target: None,
            location: None,
            location_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            prepare_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            output_directory: None,
            theme: Default::default(),
            command_context: Default::default(),
            power_replay: None,
        }
    }
}
impl MacrosRuntime {
    pub fn configure(&mut self, directory: Option<PathBuf>) {
        self.directory = directory;
    }
    fn load_library(&mut self, notify: std::sync::Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        if self.loaded || self.pending.is_some() {
            return Ok(());
        }
        self.loaded = true;
        let Some(directory) = self.directory.clone() else {
            self.storage_ready = true;
            return Ok(());
        };
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("bareline-macro-load".into())
            .spawn(move || {
                let result = (|| {
                    let mut entries = Vec::new();
                    let mut budget = 0usize;
                    for slot in 0..32 {
                        let path = directory.join(format!("macro-{:02}.toml", slot + 1));
                        let bytes = match bounded_read(&path) {
                            Ok(bytes) => bytes,
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(error) => return Err(error.to_string()),
                        };
                        budget = budget.saturating_add(bytes.len());
                        if bytes.len() > 4 * 1024 * 1024 || budget > 16 * 1024 * 1024 {
                            return Err("Saved macro library exceeds its size limit".into());
                        }
                        entries.push((
                            slot,
                            String::from_utf8(bytes).map_err(|_| "Saved macro is not UTF-8".to_string())?,
                        ));
                    }
                    let external = match bounded_read(&directory.join("external-command.toml")) {
                        Ok(bytes) => {
                            Some(String::from_utf8(bytes).map_err(|_| "External command is not UTF-8".to_string())?)
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(error) => return Err(error.to_string()),
                    };
                    Ok(FileResult::Library(entries, external))
                })();
                let _ = tx.send(result);
                notify();
            })
            .map_err(|error| error.to_string())?;
        self.pending = Some(rx);
        Ok(())
    }
    fn save_library(&mut self, notify: std::sync::Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        if !self.storage_ready {
            return Err("Macro storage has not loaded successfully. Use Retry Loading Macros before saving.".into());
        }
        if self.pending.is_some() {
            self.dirty = true;
            return Ok(());
        }
        let directory = self
            .directory
            .clone()
            .ok_or("Macro storage is unavailable; use Export to save the selected macro")?;
        let mut entries: Vec<_> = self
            .controller
            .serialized_slots()
            .into_iter()
            .map(|(slot, text)| (format!("macro-{:02}.toml", slot + 1), text))
            .collect();
        if let Some(definition) = &self.external {
            entries.push(("external-command.toml".into(), definition.export_toml()));
        }
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("bareline-macro-library-save".into())
            .spawn(move || {
                let result = (|| {
                    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
                    for (name, text) in entries {
                        atomic_text(&directory.join(name), &text)?;
                    }
                    Ok(FileResult::Saved)
                })();
                let _ = tx.send(result);
                notify();
            })
            .map_err(|error| error.to_string())?;
        self.pending = Some(rx);
        self.dirty = false;
        self.controller.status = "Saving macros…".into();
        Ok(())
    }
    pub fn annotate_context(&self, context: &mut bareline_commands::CommandContext) {
        use bareline_commands::{CommandId, CommandState};
        let playing = self
            .controller
            .playback
            .as_ref()
            .is_some_and(|playback| matches!(playback.state(), PlaybackState::Running | PlaybackState::Waiting(_)));
        context.states.insert(
            CommandId("macro.record"),
            CommandState {
                checked: self.controller.recorder.recording(),
                ..Default::default()
            },
        );
        if self.controller.recorder.recording() || playing {
            context.states.insert(
                CommandId("macro.record"),
                CommandState::disabled("Stop the current recording or playback first"),
            );
        }
        for id in [
            "macro.play",
            "macro.play_eof",
            "macro.play_n",
            "macro.rename",
            "macro.ghost",
            "macro.shortcut",
        ] {
            if self.controller.selected.is_none() {
                context.states.insert(
                    CommandId(id),
                    CommandState::disabled("Select a recorded or imported macro first"),
                );
            } else if playing || self.controller.recorder.recording() {
                context.states.insert(
                    CommandId(id),
                    CommandState::disabled("Stop recording or playback first"),
                );
            }
        }
        if !playing {
            context.states.insert(
                CommandId("macro.cancel"),
                CommandState::disabled("Macro playback is not active"),
            );
        }
        if !self
            .controller
            .playback
            .as_ref()
            .is_some_and(|playback| matches!(playback.state(), PlaybackState::Failed { .. }))
        {
            context.states.insert(
                CommandId("macro.resume"),
                CommandState::disabled("No failed macro location is available"),
            );
        }
        if self.storage_ready || self.pending.is_some() {
            context.states.insert(
                CommandId("macro.reload"),
                CommandState::disabled("Macro storage is already loaded or loading"),
            );
        }
        if !self.storage_ready {
            for id in [
                "macro.record",
                "macro.stop",
                "macro.import",
                "macro.rename",
                "macro.ghost",
                "macro.save",
                "run.load",
            ] {
                context.states.insert(
                    CommandId(id),
                    CommandState::disabled("Macro storage is loading or failed; use Retry Loading Macros"),
                );
            }
        }
        for (index, id) in bareline_app::macros::SAVED_COMMANDS.iter().enumerate() {
            context.states.insert(
                CommandId(id),
                match self.controller.slot_name(index) {
                    Some(name) => CommandState {
                        label: Some(format!("Play {name}")),
                        ..Default::default()
                    },
                    None => CommandState::disabled("Saved macro slot is empty"),
                },
            );
        }
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
                self.controller.selected.is_none().then_some("Select a macro first"),
            ),
            (
                "run.execute",
                self.external
                    .is_none()
                    .then_some("Load an external command definition first"),
            ),
        ] {
            if let Some(reason) = reason {
                context.states.insert(CommandId(id), CommandState::disabled(reason));
            }
        }
        if let Some(definition) = &self.external {
            context.states.entry(CommandId("run.execute")).or_default().label =
                Some(format!("Run {}", definition.name));
        }
        if !self.controller.manager.open {
            context.states.insert(
                CommandId("macro.manager_close"),
                CommandState::not_applicable("The macro manager is not open"),
            );
        }
        if self.controller.process.is_none() {
            context.states.insert(
                CommandId("run.cancel"),
                CommandState::not_applicable("No external command is running"),
            );
        }
    }
    pub fn height(&self) -> f32 {
        if self.controller.output_open { 200.0 } else { 0.0 }
    }
    pub fn draw_output(&mut self, width: f32, height: f32, ops: &mut Vec<DrawOp>) {
        self.bounds = bareline_ui::rect(0.0, (height - 24.0 - self.height()).max(0.0), width, self.height());
        self.controller.draw_output(self.bounds, self.theme, ops);
    }
    pub fn draw(&mut self, renderer: &mut WindowsRenderer, width: f32, height: f32, ops: &mut Vec<DrawOp>) {
        self.bounds = bareline_ui::rect(0.0, (height - 24.0 - self.height()).max(0.0), width, self.height());
        if let Err(error) = self.controller.manager.draw(
            renderer,
            width,
            height,
            &self.controller.status,
            self.theme,
            &self.command_context,
            ops,
        ) {
            self.controller.status = format!("Macro manager layout failed: {error:?}");
        }
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
                    let text = String::from_utf8(bytes).map_err(|_| "Configuration must be UTF-8".to_string())?;
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
                    fs.validate_target(&path).map_err(|error| error.to_string())?;
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
fn bounded_read(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Configuration exceeds 4 MiB",
        ));
    }
    Ok(bytes)
}
fn atomic_text(path: &std::path::Path, text: &str) -> Result<(), String> {
    use bareline_platform::LocalFileSystem;
    let fs = bareline_platform_windows::WindowsFileSystem;
    fs.validate_target(path).map_err(|error| error.to_string())?;
    let stage = path.with_file_name(format!(
        ".bareline-macro-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)
            .map_err(|error| error.to_string())?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| error.to_string())?;
        drop(file);
        fs.commit(&stage, path, path.exists())
            .map_err(|error| error.to_string())
    })();
    let _ = std::fs::remove_file(&stage);
    result
}
impl Shell {
    fn refresh_macro_command_context(&mut self) {
        self.macros.command_context = self.command_context();
    }
    fn show_macro_manager(&mut self) {
        self.macros.controller.show_manager();
        self.refresh_macro_command_context();
    }
    fn dismiss_macro_manager(&mut self) {
        self.macros.controller.manager.dismiss();
        if let Some(renderer) = &mut self.renderer {
            self.macros.controller.manager.release(renderer);
        }
        self.macros.focused = false;
        self.ui_focus.focus(bareline_ui::ViewId(2));
        self.refresh_macro_command_context();
    }
    pub(super) fn macros_accessibility(
        &mut self,
        el: &ActiveEventLoop,
        id: u64,
        invoke: bool,
        value: Option<String>,
    ) -> bool {
        if !self.macros.controller.semantics().iter().any(|node| node.id.0 == id) {
            return false;
        }
        if id >= 2_000_000 {
            if let Some(link) = self.macros.controller.output_accessibility(id, invoke) {
                self.macros_open_link(link);
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return true;
        }
        let effect = self.macros.controller.manager.accessibility(id, invoke);
        if let Some(value) = value {
            if let Some(field) = self.macros.controller.manager.active_field() {
                field.select_all();
                field.commit(&value);
            }
        }
        self.macros_manager_effect(el, effect);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    fn macros_manager_effect(&mut self, el: &ActiveEventLoop, effect: Option<bareline_app::macros::ManagerEffect>) {
        match effect {
            Some(bareline_app::macros::ManagerEffect::Select(name)) => self.macros.controller.selected = Some(name),
            Some(bareline_app::macros::ManagerEffect::Command(id)) => {
                if let Ok(action) = self.app.commands.dispatch_in(id, &self.command_context()) {
                    self.dispatch(el, action);
                }
            }
            Some(bareline_app::macros::ManagerEffect::Dismiss) => self.dismiss_macro_manager(),
            None => {}
        }
    }
    pub(super) fn macros_record_receipts(
        &mut self,
        receipts: Vec<bareline_editor_surface::power::consumer::OrderedReceipt>,
    ) {
        if let Err(error) = self.macros.controller.record_receipts(receipts, &self.app.commands) {
            self.macros.controller.status = error;
            self.macros.controller.output_open = true;
        }
    }
    pub(super) fn macros_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if id == "run.prompt" {
            self.palette.dismiss();
            self.app.palette = false;
            self.finish_palette_focus();
            self.activate_modal(modal::ModalSurface::Run);
            self.run_prompt.open();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return true;
        }
        if (matches!(id, "macro.play" | "macro.play_eof" | "macro.play_n")
            || bareline_app::macros::SAVED_COMMANDS.contains(&id))
            && !self
                .workspace
                .as_ref()
                .is_some_and(|workspace| self.app.active < workspace.editors.len())
        {
            self.macros.controller.status = "Open a document before playing a macro".into();
            self.macros.controller.output_open = true;
            return true;
        }
        if !self.macros.storage_ready
            && matches!(
                id,
                "macro.record"
                    | "macro.stop"
                    | "macro.import"
                    | "macro.rename"
                    | "macro.ghost"
                    | "macro.save"
                    | "run.load"
            )
        {
            self.macros.controller.status =
                "Macro storage is loading or failed; use Retry Loading Macros before changing it.".into();
            self.macros.controller.output_open = true;
            return true;
        }
        if matches!(id, "macro.rename" | "macro.play_n" | "macro.shortcut" | "macro.ghost")
            && !self.macros.controller.manager.open
        {
            self.palette.dismiss();
            self.show_macro_manager();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return true;
        }
        if let Some(slot) = bareline_app::macros::SAVED_COMMANDS
            .iter()
            .position(|value| *value == id)
        {
            let result = self
                .macros
                .controller
                .select_slot(slot)
                .and_then(|()| self.macros.controller.play(Repeat::Once, &self.app.commands));
            if let Err(error) = result {
                self.macros.controller.status = error;
                self.macros.controller.output_open = true;
            } else if let Some(workspace) = &self.workspace {
                self.macros.controller.capture_replay_target(workspace, self.app.active);
            }
            (self.notify)();
            return true;
        }
        let result: Result<(), String> = match id {
            "macro.reload" => {
                if self.macros.pending.is_some() {
                    Err("Wait for the current macro file operation".into())
                } else if self.macros.storage_ready {
                    Err("Macro storage is already loaded".into())
                } else {
                    self.macros.loaded = false;
                    self.macros.load_library(self.notify.clone())
                }
            }
            "macro.manager" => {
                self.palette.dismiss();
                self.show_macro_manager();
                Ok(())
            }
            "macro.manager_close" => {
                self.dismiss_macro_manager();
                Ok(())
            }
            "macro.save" => self.macros.save_library(self.notify.clone()),
            "macro.rename" => {
                let name = self.macros.controller.manager.name.value().to_string();
                let old = self.macros.controller.selected.clone().unwrap_or_default();
                self.macros.controller.rename(&old, &name).and_then(|()| {
                    self.macros.controller.show_manager();
                    self.macros.save_library(self.notify.clone())
                })
            }
            "macro.play_n" => self
                .macros
                .controller
                .manager
                .repetitions
                .value()
                .parse::<u32>()
                .map_err(|_| "Repeat count must be 1–10000".to_string())
                .and_then(|count| self.macros.controller.play(Repeat::Times(count), &self.app.commands)),
            "macro.resume" => {
                if self
                    .macros
                    .controller
                    .playback
                    .as_mut()
                    .is_some_and(|playback| playback.resume())
                {
                    Ok(())
                } else {
                    Err("No failed macro location can be resumed".into())
                }
            }
            "macro.ghost" => self
                .macros
                .controller
                .manager
                .delay
                .value()
                .parse::<u64>()
                .map_err(|_| "Typing delay must be 0–60000 ms".to_string())
                .and_then(|delay| self.macros.controller.set_typing_delay(delay, &self.app.commands))
                .and_then(|()| self.macros.save_library(self.notify.clone())),
            "macro.shortcut" => self.macros_assign_shortcut(),
            "macro.record" => {
                self.macros.controller.output_open = true;
                self.macros.controller.record()
            }
            "macro.stop" => {
                if self.views.pending_edits()
                    || self
                        .workspace
                        .as_ref()
                        .is_some_and(|workspace| workspace.editors.iter().any(|editor| editor.busy()))
                {
                    self.macros.controller.status = "Wait for the pending edit before stopping recording.".into();
                    return true;
                }
                while self
                    .macros
                    .controller
                    .library
                    .contains_key(&format!("Macro {}", self.macros.next_name))
                {
                    self.macros.next_name += 1;
                }
                let name = format!("Macro {}", self.macros.next_name);
                let result = self.macros.controller.stop_recording(&name, &self.app.commands);
                if result.is_ok() {
                    self.macros.next_name += 1;
                    self.macros.dirty = true;
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
                self.macros
                    .prepare_cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                self.macros.controller.cancel_process();
                Ok(())
            }
            "output.clear" => {
                self.macros.controller.clear_output();
                Ok(())
            }
            "output.close" => {
                self.macros
                    .location_cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                self.macros.output_target = None;
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
            "output.open_link" => {
                if let Some(link) = self.macros.controller.output_event(UiEvent::Key(UiKey::Enter)) {
                    self.macros_open_link(link);
                    Ok(())
                } else {
                    Err("Select a path:line:column output location first".into())
                }
            }
            "macro.import" | "run.load" => match self.platform.as_ref().unwrap().open_file() {
                Ok(Some(path)) => self.macros.read(path, id == "run.load", self.notify.clone()),
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
        if result.is_ok() && matches!(id, "macro.play" | "macro.play_eof" | "macro.play_n") {
            self.macros.next_tick = Some(Instant::now());
            (self.notify)();
            if let Some(workspace) = &self.workspace {
                self.macros.controller.capture_replay_target(workspace, self.app.active);
            }
        }
        if let Err(error) = result {
            self.macros.controller.status = error;
            self.macros.controller.output_open = true;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    fn macros_assign_shortcut(&mut self) -> Result<(), String> {
        use bareline_commands::{CommandId, KeyBinding, KeyChord};
        if self.settings.keymap_busy() {
            return Err("Wait for the current shortcut save".into());
        }
        let slot = self.macros.controller.selected_slot().ok_or("Select a macro first")?;
        let chord = KeyChord::parse(self.macros.controller.manager.shortcut.value())?;
        let mut document = self.settings.keymap.clone();
        document.set_binding(
            KeyBinding {
                command: CommandId(bareline_app::macros::SAVED_COMMANDS[slot]),
                sequence: vec![chord],
            },
            &self.app.commands,
        )?;
        self.settings.save_keymap(document, None);
        if let Some(error) = &self.settings.controller.error {
            return Err(error.clone());
        }
        self.macros.controller.status = "Saving shortcut…".into();
        Ok(())
    }
    fn macros_run_loaded(&mut self) -> Result<(), String> {
        let definition = self
            .macros
            .external
            .as_ref()
            .ok_or("Load a user command definition first")?
            .clone();
        if definition
            .arguments
            .iter()
            .any(|argument| argument.contains("${line}") || argument.contains("${column}"))
            && !self
                .workspace
                .as_ref()
                .is_some_and(|workspace| self.app.active < workspace.editors.len())
        {
            return Err("Open a document before using ${line} or ${column}".into());
        }
        if let Some(workspace) = &self.workspace {
            if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) =
                workspace.editors.get(self.app.active)
            {
                if definition
                    .arguments
                    .iter()
                    .any(|argument| argument.contains("${line}") || argument.contains("${column}"))
                {
                    if self.macros.pending.is_some() {
                        return Err("Wait for the pending macro configuration operation".into());
                    }
                    let templates: Vec<_> = definition
                        .arguments
                        .iter()
                        .map(|argument| argument.replace("${line}", "").replace("${column}", ""))
                        .collect();
                    let mut context =
                        bareline_app::macros::placeholder_context(workspace, self.app.active, &templates)?;
                    context.workspace = self.settings.workspace_root().map(std::path::Path::to_path_buf);
                    let source = editor.read_handle();
                    let offset = editor
                        .viewport_start()
                        .0
                        .saturating_add(editor.viewport().selection.caret);
                    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    self.macros.prepare_cancel = cancel.clone();
                    let notify = self.notify.clone();
                    let (tx, rx) = mpsc::sync_channel(1);
                    std::thread::Builder::new()
                        .name("bareline-command-position".into())
                        .spawn(move || {
                            let result = location::paged_line_column(&source, offset, &cancel)
                                .and_then(|(line, column)| {
                                    context.line = line;
                                    context.column = column;
                                    definition.request(&context)
                                })
                                .map(FileResult::Prepared);
                            let _ = tx.send(result);
                            notify();
                        })
                        .map_err(|error| error.to_string())?;
                    self.macros.pending = Some(rx);
                    self.macros.controller.output_open = true;
                    self.macros.controller.status =
                        "Preparing command position… Cancel External Command stops this scan.".into();
                    return Ok(());
                }
            }
        }
        let mut context = match &self.workspace {
            Some(workspace) => {
                bareline_app::macros::placeholder_context(workspace, self.app.active, &definition.arguments)?
            }
            None => PlaceholderContext::default(),
        };
        context.workspace = self.settings.workspace_root().map(std::path::Path::to_path_buf);
        let request = definition.request(&context)?;
        self.macros_confirm_run(request)
    }
    pub(super) fn macros_confirm_run(
        &mut self,
        request: bareline_app::macros::model::process::ProcessRequest,
    ) -> Result<(), String> {
        let shell = matches!(request.mode, LaunchMode::Shell { .. });
        let (program, arguments) = match &request.mode {
            LaunchMode::Direct { program, arguments } | LaunchMode::Shell { program, arguments } => {
                (program, arguments)
            }
        };
        if !self
            .platform
            .as_ref()
            .is_some_and(|platform| platform.confirm_external_command(program, arguments, shell))
        {
            return Ok(());
        }
        let directory = request.directory.clone().or_else(|| std::env::current_dir().ok());
        self.macros.controller.run(
            request,
            if shell {
                ProcessPermission::UserGrantedShell
            } else {
                ProcessPermission::UserGrantedDirect
            },
            std::sync::Arc::new(bareline_platform_windows::WindowsProcessLauncher),
        )?;
        self.macros.output_directory = directory;
        Ok(())
    }
    pub(super) fn macros_pump(&mut self, _el: &ActiveEventLoop) {
        self.macros.next_tick = None;
        self.macros.theme = self.settings.ui_theme();
        self.macros_poll_location();
        if let Err(error) = self.macros.load_library(self.notify.clone()) {
            self.macros.controller.status = error;
        }
        if let Some(receiver) = &self.macros.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    self.macros.pending = None;
                    // Background persistence is silent on success, including fresh startup.
                    let background = matches!(&result, Ok(FileResult::Library(..) | FileResult::Saved));
                    let result = match result {
                        Ok(FileResult::Macro(text)) => {
                            self.macros.controller.import(&text, &self.app.commands).map(|()| {
                                self.macros.dirty = true;
                                self.macros.controller.show_manager();
                            })
                        }
                        Ok(FileResult::External(text)) => ExternalDefinition::import_toml(&text).map(|definition| {
                            self.macros.controller.status =
                                format!("Loaded {} — {}", definition.name, definition.program);
                            self.macros.external = Some(definition);
                            self.macros.dirty = true;
                        }),
                        Ok(FileResult::Library(entries, external)) => external
                            .as_deref()
                            .map(ExternalDefinition::import_toml)
                            .transpose()
                            .and_then(|external| {
                                self.macros.controller.restore_library(entries, &self.app.commands)?;
                                self.macros.external = external;
                                self.macros.storage_ready = true;
                                Ok(())
                            }),
                        Ok(FileResult::Saved) => {
                            self.macros.controller.status = "Saved".into();
                            Ok(())
                        }
                        Ok(FileResult::Prepared(request)) => {
                            if self.macros.prepare_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                                Err("Command preparation cancelled".into())
                            } else {
                                self.macros_confirm_run(request)
                            }
                        }
                        Err(error) => Err(error),
                    };
                    if let Err(error) = result {
                        self.macros.controller.status = error;
                        self.macros.controller.output_open = true;
                    } else if !background {
                        self.macros.controller.output_open = true;
                    }
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
        if self.macros.dirty && self.macros.pending.is_none() {
            if let Err(error) = self.macros.save_library(self.notify.clone()) {
                self.macros.dirty = false;
                self.macros.controller.status = format!("Changes not saved: {error}. Use Save Macros to retry.");
            }
        }
        self.macros_pump_power_replay();
        let context = self.command_context();
        if let Some(workspace) = &mut self.workspace
            && let Some(state) = self.macros.controller.tick(
                Instant::now(),
                &self.app.commands,
                workspace,
                self.app.active,
                context,
                self.notify.clone(),
            )
        {
            if matches!(state, PlaybackState::Running | PlaybackState::Waiting(_)) {
                self.macros.next_tick = Some(Instant::now() + Duration::from_millis(16));
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        self.macros_pump_power_replay();
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
            .is_some_and(|process| matches!(process.state(), ProcessState::Starting | ProcessState::Running))
        {
            let process_tick = Instant::now() + Duration::from_millis(30);
            self.macros.next_tick = Some(
                self.macros
                    .next_tick
                    .map_or(process_tick, |tick| tick.min(process_tick)),
            );
        }
        // Background load/import can open the manager during this pump. Install
        // the resulting state before the next draw or accessibility snapshot.
        self.refresh_macro_command_context();
    }
    pub(super) fn macros_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if self.macros.controller.manager.open && !self.palette.open {
            let mut effect = None;
            match event {
                WindowEvent::MouseInput {
                    state,
                    button: MouseButton::Left,
                    ..
                } => {
                    effect = self.macros.controller.manager.event(
                        if *state == ElementState::Pressed {
                            UiEvent::PointerDown(self.pointer)
                        } else {
                            UiEvent::PointerUp(self.pointer)
                        },
                        self.modifiers.shift_key(),
                    );
                    if *state == ElementState::Pressed {
                        if let Some(renderer) = &self.renderer {
                            self.macros.controller.manager.click_field(
                                renderer,
                                self.pointer,
                                self.modifiers.shift_key(),
                            );
                        }
                    }
                }
                WindowEvent::Ime(Ime::Preedit(value, cursor)) => {
                    if let Some(field) = self.macros.controller.manager.active_field() {
                        field.preedit(value.clone(), *cursor);
                    }
                }
                WindowEvent::Ime(Ime::Commit(value)) => {
                    if let Some(field) = self.macros.controller.manager.active_field() {
                        field.commit(value);
                    }
                }
                WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                    let ctrl = self.modifiers.control_key();
                    let shift = self.modifiers.shift_key();
                    let mut handled_field = false;
                    if let Some(field) = self.macros.controller.manager.active_field() {
                        if !field.composing() {
                            match &event.logical_key {
                                Key::Named(NamedKey::Backspace) => {
                                    field.delete(false);
                                    handled_field = true;
                                }
                                Key::Named(NamedKey::Space) => {
                                    field.insert(" ");
                                    handled_field = true;
                                }
                                Key::Named(NamedKey::Delete) => {
                                    field.delete(true);
                                    handled_field = true;
                                }
                                Key::Named(NamedKey::ArrowLeft) => {
                                    field.horizontal(false, shift);
                                    handled_field = true;
                                }
                                Key::Named(NamedKey::ArrowRight) => {
                                    field.horizontal(true, shift);
                                    handled_field = true;
                                }
                                Key::Named(NamedKey::Home) => {
                                    field.edge(false, shift);
                                    handled_field = true;
                                }
                                Key::Named(NamedKey::End) => {
                                    field.edge(true, shift);
                                    handled_field = true;
                                }
                                Key::Character(value) if ctrl && !self.modifiers.alt_key() => {
                                    match value.to_ascii_lowercase().as_str() {
                                        "a" => {
                                            field.select_all();
                                            handled_field = true;
                                        }
                                        "z" => {
                                            field.undo(shift);
                                            handled_field = true;
                                        }
                                        "y" => {
                                            field.undo(true);
                                            handled_field = true;
                                        }
                                        "c" | "x" => {
                                            if let Some(platform) = &self.platform {
                                                if platform.set_clipboard_text(field.selected()).is_ok()
                                                    && value.eq_ignore_ascii_case("x")
                                                {
                                                    field.insert("");
                                                }
                                            }
                                            handled_field = true;
                                        }
                                        "v" => {
                                            if let Some(platform) = &self.platform {
                                                if let Ok(value) = platform.clipboard_text() {
                                                    field.commit(&value);
                                                }
                                            }
                                            handled_field = true;
                                        }
                                        _ => {}
                                    }
                                }
                                Key::Character(value) if !ctrl || self.modifiers.alt_key() => {
                                    field.insert(value);
                                    handled_field = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    if !handled_field {
                        let key = match event.logical_key {
                            Key::Named(NamedKey::Escape) => Some(UiKey::Escape),
                            Key::Named(NamedKey::Tab) => Some(UiKey::Tab),
                            Key::Named(NamedKey::Enter) => Some(UiKey::Enter),
                            Key::Named(NamedKey::Space) => Some(UiKey::Space),
                            Key::Named(NamedKey::ArrowUp) => Some(UiKey::Up),
                            Key::Named(NamedKey::ArrowDown) => Some(UiKey::Down),
                            Key::Named(NamedKey::Home) => Some(UiKey::Home),
                            Key::Named(NamedKey::End) => Some(UiKey::End),
                            _ => None,
                        };
                        if let Some(key) = key {
                            effect = self.macros.controller.manager.event(UiEvent::Key(key), shift);
                        } else if ctrl || self.modifiers.alt_key() {
                            return false;
                        }
                    }
                }
                WindowEvent::RedrawRequested
                | WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. }
                | WindowEvent::CloseRequested
                | WindowEvent::ModifiersChanged(_)
                | WindowEvent::CursorMoved { .. } => return false,
                WindowEvent::Focused(false) => {
                    if let Some(field) = self.macros.controller.manager.active_field() {
                        field.cancel();
                    }
                    return false;
                }
                _ => {}
            }
            self.macros_manager_effect(el, effect);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return true;
        }
        if !self.macros.controller.output_open
            || self.palette.open
            || self.utilities.has_input_focus()
            || self.search_modal()
            || self.settings.controller.open
            || self.shortcuts.open
            || self.power.open
            || self.extensions.open
            || self.language.controller.open
            || self.recovery.has_input_focus()
            || self.compare.options_open
            || self.workspace.as_ref().is_some_and(|workspace| {
                workspace.find.has_focus() || (workspace.search_panel.open && workspace.search_focus)
            })
        {
            return false;
        }
        let ui = match event {
            WindowEvent::MouseWheel { delta, .. } if self.macros.bounds.contains(self.pointer) => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -*y as f64 * 28.,
                    MouseScrollDelta::PixelDelta(point) => {
                        -point.y / self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.)
                    }
                };
                self.macros.controller.scroll_output(dy);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return true;
            }
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
            WindowEvent::KeyboardInput { event, .. } if self.macros.focused && event.state == ElementState::Pressed => {
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
            if let Some(link) = self.macros.controller.output_event(ui) {
                self.macros_open_link(link);
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return true;
        }
        false
    }
}

impl Shell {
    fn macros_pump_power_replay(&mut self) {
        if let Some(id) = self.macros.power_replay {
            if !self.macros.controller.power_replay_active(id) {
                self.power_replay_cancel(id);
            }
            if let Some(completion) = self.power_replay_poll(id) {
                self.macros.controller.complete_power_replay(id, completion);
                self.macros.power_replay = None;
            }
        }
        if self.macros.power_replay.is_none()
            && let Some(request) = self.macros.controller.take_power_replay()
        {
            let id = request.id;
            if !self.macros.controller.power_replay_active(id) {
                return;
            }
            match self.power_replay_start(request) {
                Ok(()) => self.macros.power_replay = Some(id),
                Err(error) => self.macros.controller.complete_power_replay(
                    id,
                    bareline_app::macros::PowerReplayCompletion {
                        target: None,
                        result: Err(error),
                    },
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_commands::{CommandContext, CommandId};

    fn assert_open_close_enabled(runtime: &MacrosRuntime, label: &str) {
        assert!(runtime.controller.manager.open, "{label}");
        let mut context = CommandContext::default();
        runtime.annotate_context(&mut context);
        assert!(
            context
                .states
                .get(&CommandId("macro.manager_close"))
                .is_none_or(|state| state.enabled && !state.hidden),
            "Close was unavailable in {label} state"
        );
    }

    #[test]
    fn manager_close_context_is_enabled_for_every_runtime_state() {
        let mut empty = MacrosRuntime::default();
        empty.controller.show_manager();
        assert_open_close_enabled(&empty, "empty");

        let mut selected = MacrosRuntime::default();
        selected.controller.library.insert(
            "Example".into(),
            bareline_app::macros::model::Macro {
                name: "Example".into(),
                events: vec![bareline_app::macros::model::MacroEvent::Command {
                    id: "edit.insert_text".into(),
                    arguments: std::collections::BTreeMap::from([("text".into(), "x".into())]),
                }],
            },
        );
        selected.controller.selected = Some("Example".into());
        selected.controller.show_manager();
        assert_open_close_enabled(&selected, "selected");

        let mut loading = MacrosRuntime::default();
        let (_sender, receiver) = mpsc::sync_channel(1);
        loading.pending = Some(receiver);
        loading.controller.show_manager();
        assert_open_close_enabled(&loading, "loading");

        let mut failed = MacrosRuntime::default();
        failed.loaded = true;
        failed.controller.status = "injected load failure".into();
        failed.controller.show_manager();
        assert_open_close_enabled(&failed, "failed loading");

        let mut recording = MacrosRuntime::default();
        recording.controller.record().unwrap();
        recording.controller.show_manager();
        assert_open_close_enabled(&recording, "recording");

        let mut playback = selected;
        let mut registry = bareline_commands::shell_commands();
        bareline_app::macros::register_commands(&mut registry);
        playback.controller.play(Repeat::Once, &registry).unwrap();
        assert!(playback.controller.playback.as_ref().is_some_and(|playback| {
            matches!(playback.state(), PlaybackState::Running | PlaybackState::Waiting(_))
        }));
        playback.controller.show_manager();
        assert_open_close_enabled(&playback, "playback");
    }

    #[test]
    fn inapplicable_manager_and_run_commands_are_hidden_not_greyed() {
        let mut runtime = MacrosRuntime::default();
        let mut context = CommandContext::default();
        runtime.annotate_context(&mut context);
        assert!(
            context
                .states
                .get(&CommandId("macro.manager_close"))
                .is_some_and(|state| state.hidden),
            "closing a closed manager must not appear in menus"
        );
        assert!(
            context
                .states
                .get(&CommandId("run.cancel"))
                .is_some_and(|state| state.hidden),
            "cancelling when nothing runs must not appear in menus"
        );

        runtime.controller.manager.open = true;
        let mut context = CommandContext::default();
        runtime.annotate_context(&mut context);
        assert!(
            !context
                .states
                .get(&CommandId("macro.manager_close"))
                .is_some_and(|state| state.hidden),
            "an open manager keeps its close action visible"
        );
    }
}
