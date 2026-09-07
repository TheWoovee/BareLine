// SPDX-License-Identifier: MPL-2.0
//! App-owned macro library, confirmed document replay and captured output panel.
use crate::workspace::{Input, Workspace};
use bareline_commands::{Action, CommandContext, CommandId, CommandRegistry, CommandSpec};
pub use bareline_macros as model;
pub fn placeholder_context(
    workspace: &Workspace,
    active: usize,
) -> Result<model::process::PlaceholderContext, String> {
    let mut context = model::process::PlaceholderContext {
        file: workspace.path(active).map(std::path::PathBuf::from),
        ..Default::default()
    };
    if let Some(editor) = workspace.editors.get(active) {
        context.selection = editor.selected_text().map_err(str::to_string)?;
        let caret = bareline_document::TextOffset(editor.selection.caret);
        let line = editor
            .snapshot()
            .line_at(caret)
            .map_err(|error| format!("{error:?}"))?;
        let range = editor
            .snapshot()
            .line_range(line)
            .map_err(|error| format!("{error:?}"))?;
        let prefix = editor
            .snapshot()
            .read(range.start..caret, 1024 * 1024)
            .map_err(|error| format!("Cannot derive column: {error:?}"))?;
        context.line = line as u64 + 1;
        context.column = prefix.chars().count() as u64 + 1;
    }
    Ok(context)
}
use bareline_macros::{
    Macro, MacroEvent, MacroExecutor, Playback, PlaybackState, Progress, Recorder, Repeat,
    process::{
        self, ProcessHandle, ProcessLauncher, ProcessPermission, ProcessRequest, ProcessState,
    },
};
use bareline_renderer::{DrawOp, Rect};
use bareline_ui::{
    controls::{ControlAction, ControlState, UiEvent},
    widgets::{ItemSource, List, Metrics, Theme},
    *,
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};

pub fn register_commands(registry: &mut CommandRegistry) {
    for (id, title) in [
        ("edit.insert_text", "Insert Recorded Text"),
        ("edit.backspace", "Delete Previous Character"),
        ("edit.delete", "Delete Next Character"),
        ("edit.move_left", "Move Caret Left"),
        ("edit.move_right", "Move Caret Right"),
        ("edit.move_up", "Move Caret Up"),
        ("edit.move_down", "Move Caret Down"),
        ("edit.move_home", "Move Caret to Line Start"),
        ("edit.move_end", "Move Caret to Line End"),
    ] {
        let id = CommandId(id);
        registry
            .register(CommandSpec {
                id,
                title,
                category: "Edit",
                shortcut: "",
                action: Action::Contributed(id),
            })
            .expect("unique normalized editor command");
        registry
            .set_presentation(
                id,
                bareline_commands::CommandPresentation {
                    internal: true,
                    ..Default::default()
                },
            )
            .expect("registered normalized editor command");
    }
    for (id, title, category) in [
        ("macro.record", "Start Macro Recording", "Macro"),
        ("macro.stop", "Stop Macro Recording", "Macro"),
        ("macro.play", "Play Selected Macro", "Macro"),
        ("macro.cancel", "Cancel Macro Playback", "Macro"),
        ("macro.import", "Import Macro…", "Macro"),
        ("macro.export", "Export Selected Macro…", "Macro"),
        ("macro.play_eof", "Play Macro Until End of File", "Macro"),
        ("run.load", "Load External Command Definition…", "Run"),
        ("run.execute", "Run Loaded External Command", "Run"),
        ("run.cancel", "Cancel External Command", "Run"),
        ("output.clear", "Clear Output", "Tools"),
        ("output.copy", "Copy Output", "Tools"),
        ("output.save", "Save Output…", "Tools"),
        ("output.close", "Close Output", "Tools"),
    ] {
        let id = CommandId(id);
        registry
            .register(CommandSpec {
                id,
                title,
                category,
                shortcut: "",
                action: Action::Contributed(id),
            })
            .expect("unique macro/output command");
    }
}
struct OutputRows(Vec<String>);
impl ItemSource for OutputRows {
    fn len(&self) -> Option<usize> {
        Some(self.0.len())
    }
    fn discovered(&self) -> usize {
        self.0.len()
    }
    fn label(&self, index: usize) -> &str {
        self.0.get(index).map(String::as_str).unwrap_or("")
    }
}
pub struct MacrosController {
    pub recorder: Recorder,
    pub library: BTreeMap<String, Macro>,
    pub selected: Option<String>,
    pub playback: Option<Playback>,
    pub process: Option<ProcessHandle>,
    pub output_open: bool,
    pub status: String,
    rows: OutputRows,
    output_fingerprint: (usize, u64),
    list: List,
}
impl Default for MacrosController {
    fn default() -> Self {
        Self {
            recorder: Recorder::default(),
            library: BTreeMap::new(),
            selected: None,
            playback: None,
            process: None,
            output_open: false,
            status: String::new(),
            rows: OutputRows(Vec::new()),
            output_fingerprint: (0, 0),
            list: List {
                bounds: Rect::default(),
                state: ControlState::default(),
                selected: None,
                offset: 0.0,
                metrics: Metrics::COMPACT,
            },
        }
    }
}
impl MacrosController {
    pub fn record(&mut self) -> Result<(), String> {
        if self.playback.as_ref().is_some_and(|playback| {
            matches!(
                playback.state(),
                PlaybackState::Running | PlaybackState::Waiting(_)
            )
        }) {
            return Err("Stop playback before recording".into());
        }
        self.recorder.start();
        self.status = "Recording normalized commands".into();
        Ok(())
    }
    pub fn recorded(
        &mut self,
        event: MacroEvent,
        recordable: bool,
        registry: &CommandRegistry,
    ) -> Result<bool, String> {
        self.recorder.executed(event, recordable, registry)
    }
    pub fn stop_recording(&mut self, name: &str, registry: &CommandRegistry) -> Result<(), String> {
        if self.library.contains_key(name) {
            return Err("A macro with this name already exists".into());
        }
        let definition = self.recorder.stop(name, registry)?;
        self.library.insert(name.into(), definition);
        self.selected = Some(name.into());
        self.status = format!("Recorded {name}");
        Ok(())
    }
    pub fn import(&mut self, text: &str, registry: &CommandRegistry) -> Result<(), String> {
        let definition = Macro::import_toml(text, registry)?;
        if self.library.contains_key(&definition.name) {
            return Err("A macro with this name already exists".into());
        }
        self.selected = Some(definition.name.clone());
        self.library.insert(definition.name.clone(), definition);
        Ok(())
    }
    pub fn export_selected(&self) -> Result<String, String> {
        self.selected
            .as_ref()
            .and_then(|name| self.library.get(name))
            .map(Macro::export_toml)
            .ok_or_else(|| "Select a macro first".into())
    }
    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), String> {
        if self.library.contains_key(new) {
            return Err("A macro with this name already exists".into());
        }
        let mut definition = self.library.get(old).cloned().ok_or("Macro not found")?;
        definition.rename(new)?;
        self.library.remove(old);
        self.library.insert(new.into(), definition);
        if self.selected.as_deref() == Some(old) {
            self.selected = Some(new.into());
        }
        Ok(())
    }
    pub fn play(&mut self, repeat: Repeat, registry: &CommandRegistry) -> Result<(), String> {
        if self.recorder.recording() {
            return Err("Stop recording before playback".into());
        }
        if self.playback.as_ref().is_some_and(|playback| {
            matches!(
                playback.state(),
                PlaybackState::Running | PlaybackState::Waiting(_)
            )
        }) {
            return Err("Macro playback is already running".into());
        }
        let definition = self
            .selected
            .as_ref()
            .and_then(|name| self.library.get(name))
            .ok_or("Select a macro first")?
            .clone();
        self.playback = Some(Playback::new(definition, repeat, 10_000, registry)?);
        Ok(())
    }
    pub fn cancel(&mut self) {
        if let Some(playback) = self.playback.as_mut() {
            playback.cancel();
        }
        self.status = "Macro cancelled".into();
    }
    pub fn tick(
        &mut self,
        now: Instant,
        registry: &CommandRegistry,
        workspace: &mut Workspace,
        active: usize,
        context: CommandContext,
    ) -> Option<PlaybackState> {
        let playback = self.playback.as_mut()?;
        if !matches!(
            playback.state(),
            PlaybackState::Running | PlaybackState::Waiting(_)
        ) {
            return None;
        }
        let mut executor = WorkspaceExecutor {
            workspace,
            active,
            context,
        };
        let state = playback.tick(now, registry, &mut executor);
        self.status = format!("Macro: {state:?}");
        Some(state)
    }
    pub fn run(
        &mut self,
        request: ProcessRequest,
        permission: ProcessPermission,
        launcher: Arc<dyn ProcessLauncher>,
    ) -> Result<(), String> {
        if self.process.as_ref().is_some_and(|process| {
            matches!(
                process.state(),
                ProcessState::Starting | ProcessState::Running
            )
        }) {
            return Err("An external command is already running".into());
        }
        self.process = Some(process::launch(request, permission, launcher, 1024 * 1024)?);
        self.output_open = true;
        self.output_fingerprint = (0, 0);
        self.rows.0.clear();
        Ok(())
    }
    pub fn cancel_process(&self) {
        if let Some(process) = &self.process {
            process.cancel();
        }
    }
    pub fn clear_output(&mut self) {
        if let Some(process) = &self.process {
            process.output().clear();
        }
        self.rows.0.clear();
        self.output_fingerprint = (0, 0);
        self.list.selected = None;
        self.list.offset = 0.0;
    }
    pub fn copy_output(&self) -> String {
        self.process
            .as_ref()
            .map(|process| process.output().text())
            .unwrap_or_default()
    }
    pub fn refresh_output(&mut self) -> bool {
        let Some(process) = &self.process else {
            return false;
        };
        let output = process.output();
        let fingerprint = (output.byte_len(), output.discarded_bytes);
        let status = format!(
            "{:?} · {} bytes discarded",
            process.state(),
            output.discarded_bytes
        );
        let changed = fingerprint != self.output_fingerprint || self.status != status;
        if fingerprint != self.output_fingerprint {
            let value = output.text();
            self.rows.0 = value
                .split('\n')
                .take(20_000)
                .map(|line| line.chars().take(8192).collect())
                .collect();
            self.output_fingerprint = fingerprint;
        }
        self.status = status;
        changed
    }
    pub fn output_event(&mut self, event: UiEvent) -> Option<process::OutputLink> {
        match self.list.event(event, &self.rows) {
            Some(ControlAction::Selected(index)) => self
                .rows
                .0
                .get(index)
                .and_then(|line| process::parse_output_link(line)),
            _ => None,
        }
    }
    pub fn draw_output(&mut self, bounds: Rect, ops: &mut Vec<DrawOp>) {
        if !self.output_open {
            return;
        }
        ops.push(DrawOp::Fill(bounds, ELEVATED));
        text(ops, bounds.x + 10.0, bounds.y + 8.0, "Output", 13.0, TEXT);
        ops.push(DrawOp::PushClip(rect(
            bounds.x + 70.0,
            bounds.y,
            bounds.width - 80.0,
            28.0,
        )));
        text(
            ops,
            bounds.x + 70.0,
            bounds.y + 8.0,
            &self.status,
            12.0,
            MUTED,
        );
        ops.push(DrawOp::PopClip);
        self.list.bounds = rect(
            bounds.x,
            bounds.y + 28.0,
            bounds.width,
            bounds.height - 28.0,
        );
        self.list.paint(&self.rows, Theme::default(), ops);
    }
}
/// The adapter acknowledges actor completion before Playback advances its event location.
struct WorkspaceExecutor<'a> {
    workspace: &'a mut Workspace,
    active: usize,
    context: CommandContext,
}
impl MacroExecutor for WorkspaceExecutor<'_> {
    fn context(&self) -> CommandContext {
        self.context.clone()
    }
    fn progress(&self) -> Progress {
        self.workspace
            .editors
            .get(self.active)
            .map(|editor| Progress {
                document: self.active as u64,
                position: editor.selection.caret as u64,
                revision: editor.snapshot().revision.0,
                eof: editor.selection.caret >= editor.snapshot().len(),
            })
            .unwrap_or(Progress {
                document: u64::MAX,
                position: 0,
                revision: 0,
                eof: true,
            })
    }
    fn poll_execution(&mut self) -> Result<bool, String> {
        let editor = self
            .workspace
            .editors
            .get_mut(self.active)
            .ok_or("Active document closed during macro")?;
        editor.pump();
        if editor.busy() {
            return Ok(false);
        }
        if let Some(error) = editor.error.take() {
            return Err(error);
        }
        Ok(true)
    }
    fn execute(
        &mut self,
        command: CommandId,
        args: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let editor = self
            .workspace
            .editors
            .get_mut(self.active)
            .ok_or("No active document")?;
        if editor.busy() {
            return Err("Wait for the active edit before replaying".into());
        }
        if editor.composition_text().is_some() {
            return Err("Finish IME composition before replaying".into());
        }
        if command.0.starts_with("editor.") {
            editor.error = None;
            return editor.execute_power(command.0);
        }
        let input = match command.0 {
            "edit.paste" | "edit.insert_text" => Input::Insert(
                args.get("text")
                    .cloned()
                    .ok_or("Recorded paste requires explicit text")?,
            ),
            "edit.undo" => Input::Undo,
            "edit.redo" => Input::Redo,
            "edit.select_all" => Input::SelectAll,
            "edit.backspace" => Input::Backspace,
            "edit.delete" => Input::Delete,
            "edit.move_left" => {
                Input::Left(args.get("extend").is_some_and(|value| value == "true"))
            }
            "edit.move_right" => {
                Input::Right(args.get("extend").is_some_and(|value| value == "true"))
            }
            "edit.move_up" => Input::Up(args.get("extend").is_some_and(|value| value == "true")),
            "edit.move_down" => {
                Input::Down(args.get("extend").is_some_and(|value| value == "true"))
            }
            "edit.move_home" => {
                Input::Home(args.get("extend").is_some_and(|value| value == "true"))
            }
            "edit.move_end" => Input::End(args.get("extend").is_some_and(|value| value == "true")),
            _ => {
                return Err(format!(
                    "Command {} has no deterministic macro adapter",
                    command.0
                ));
            }
        };
        editor.error = None;
        editor.enqueue(input);
        Ok(())
    }
}
