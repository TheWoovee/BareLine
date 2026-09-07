// SPDX-License-Identifier: MPL-2.0
//! App-owned macro library, confirmed document replay and captured output panel.
use crate::workspace::{Input, Workspace};
use bareline_commands::{Action, CommandContext, CommandId, CommandRegistry, CommandSpec};
mod manager;
pub use bareline_macros as model;
pub use manager::{MacroManager, ManagerEffect};
/// Stable bounded slots retain shortcut identities across display-name changes.
pub const SAVED_COMMANDS: [&str; 32] = [
    "macro.saved.01",
    "macro.saved.02",
    "macro.saved.03",
    "macro.saved.04",
    "macro.saved.05",
    "macro.saved.06",
    "macro.saved.07",
    "macro.saved.08",
    "macro.saved.09",
    "macro.saved.10",
    "macro.saved.11",
    "macro.saved.12",
    "macro.saved.13",
    "macro.saved.14",
    "macro.saved.15",
    "macro.saved.16",
    "macro.saved.17",
    "macro.saved.18",
    "macro.saved.19",
    "macro.saved.20",
    "macro.saved.21",
    "macro.saved.22",
    "macro.saved.23",
    "macro.saved.24",
    "macro.saved.25",
    "macro.saved.26",
    "macro.saved.27",
    "macro.saved.28",
    "macro.saved.29",
    "macro.saved.30",
    "macro.saved.31",
    "macro.saved.32",
];
pub fn search_arguments(query: &bareline_search::SearchQuery) -> BTreeMap<String, String> {
    let mut args = BTreeMap::from([
        ("pattern".into(), query.pattern.clone()),
        (
            "mode".into(),
            match query.mode {
                bareline_search::SearchMode::Literal => "literal",
                bareline_search::SearchMode::Extended => "extended",
                bareline_search::SearchMode::Regex => "regex",
            }
            .into(),
        ),
        (
            "case".into(),
            matches!(query.case, bareline_search::Case::Sensitive).to_string(),
        ),
        ("whole_word".into(), query.whole_word.to_string()),
    ]);
    if let Some(range) = &query.selection {
        args.insert("selection_start".into(), range.start.0.to_string());
        args.insert("selection_end".into(), range.end.0.to_string());
    }
    args
}
pub fn placeholder_context(
    workspace: &Workspace,
    active: usize,
    templates: &[String],
) -> Result<model::process::PlaceholderContext, String> {
    let mut context = model::process::PlaceholderContext {
        file: workspace.path(active).map(std::path::PathBuf::from),
        ..Default::default()
    };
    if let Some(editor) = workspace.editors.get(active) {
        let needed = |name: &str| templates.iter().any(|template| template.contains(name));
        if needed("${selection}") {
            context.selection = editor.selected_text().map_err(str::to_string)?;
        }
        if !needed("${line}") && !needed("${column}") {
            return Ok(context);
        }
        if editor.paged() {
            return Err("Global line/column metadata must be prepared on the paged worker".into());
        }
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
    controls::{ControlState, UiEvent},
    variable_list::{VariableItemSource, VariableList},
    widgets::{SemanticAction, SemanticRole, Semantics, Theme},
    *,
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};

pub fn register_commands(registry: &mut CommandRegistry) {
    for id in SAVED_COMMANDS {
        registry
            .register(CommandSpec {
                id: CommandId(id),
                title: "Saved Macro",
                category: "Macro",
                shortcut: "",
                action: Action::Contributed(CommandId(id)),
            })
            .expect("unique macro slot");
        registry
            .set_presentation(
                CommandId(id),
                bareline_commands::CommandPresentation {
                    internal: true,
                    ..Default::default()
                },
            )
            .expect("registered macro slot");
    }
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
        ("macro.manager", "Manage Macros…", "Macro"),
        ("macro.rename", "Rename Selected Macro", "Macro"),
        ("macro.play_n", "Play Macro N Times", "Macro"),
        ("macro.resume", "Resume Failed Macro", "Macro"),
        ("macro.shortcut", "Assign Macro Shortcut", "Macro"),
        ("macro.ghost", "Set Macro Typing Delay", "Macro"),
        ("macro.save", "Save Macros", "Macro"),
        ("macro.reload", "Retry Loading Macros", "Macro"),
        ("macro.manager_close", "Close Macro Manager", "Macro"),
        ("run.load", "Load External Command Definition…", "Run"),
        ("run.execute", "Run Loaded External Command", "Run"),
        ("run.cancel", "Cancel External Command", "Run"),
        ("output.clear", "Clear Output", "Tools"),
        ("output.copy", "Copy Output", "Tools"),
        ("output.save", "Save Output…", "Tools"),
        ("output.close", "Close Output", "Tools"),
        ("output.open_link", "Open Selected Output Location", "Tools"),
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
impl VariableItemSource for OutputRows {
    fn len(&self) -> Option<usize> {
        Some(self.0.len())
    }
    fn discovered(&self) -> usize {
        self.0.len()
    }
    fn label(&self, index: usize) -> &str {
        self.0.get(index).map(String::as_str).unwrap_or("")
    }
    fn index_at(&self, offset: f64) -> usize {
        (offset.max(0.) / 28.) as usize
    }
    fn offset_of(&self, index: usize) -> f64 {
        index as f64 * 28.
    }
    fn item_height(&self, _index: usize) -> f32 {
        28.
    }
}
pub struct MacrosController {
    pub manager: MacroManager,
    slots: Vec<Option<String>>,
    pub recorder: Recorder,
    pub library: BTreeMap<String, Macro>,
    pub selected: Option<String>,
    pub playback: Option<Playback>,
    pub process: Option<ProcessHandle>,
    pub output_open: bool,
    pub status: String,
    rows: OutputRows,
    output_fingerprint: (usize, u64),
    list: VariableList,
    output_pressed: Option<usize>,
    replay_document: Option<ReplayDocument>,
    pending_search: Option<PendingSearch>,
    process_status: String,
}
struct PendingSearch {
    backwards: bool,
    query: bareline_search::SearchQuery,
    source: ReplayDocument,
}
enum ReplayDocument {
    Resident(bareline_document::DocumentSnapshot),
    Paged(bareline_document::paged::PagedSnapshot),
}
impl ReplayDocument {
    fn same_state(&self, editor: &crate::workspace::WorkspaceEditor) -> bool {
        match (self, editor) {
            (Self::Resident(source), crate::workspace::WorkspaceEditor::Resident(editor)) => {
                source.same_document(editor.snapshot())
                    && source.revision == editor.snapshot().revision
            }
            (Self::Paged(source), crate::workspace::WorkspaceEditor::Paged(editor)) => {
                source.same_document(editor.snapshot())
                    && source.revision == editor.snapshot().revision
            }
            _ => false,
        }
    }
    fn capture(editor: &crate::workspace::WorkspaceEditor) -> Self {
        match editor {
            crate::workspace::WorkspaceEditor::Resident(editor) => {
                Self::Resident(editor.snapshot().clone())
            }
            crate::workspace::WorkspaceEditor::Paged(editor) => {
                Self::Paged(editor.snapshot().clone())
            }
        }
    }
    fn matches(&self, editor: &crate::workspace::WorkspaceEditor) -> bool {
        match (self, editor) {
            (Self::Resident(snapshot), crate::workspace::WorkspaceEditor::Resident(editor)) => {
                snapshot.same_document(editor.snapshot())
            }
            (Self::Paged(snapshot), crate::workspace::WorkspaceEditor::Paged(editor)) => {
                snapshot.same_document(editor.snapshot())
            }
            _ => false,
        }
    }
}
impl Default for MacrosController {
    fn default() -> Self {
        Self {
            manager: MacroManager::default(),
            slots: vec![None; 32],
            recorder: Recorder::default(),
            library: BTreeMap::new(),
            selected: None,
            playback: None,
            process: None,
            output_open: false,
            status: String::new(),
            rows: OutputRows(Vec::new()),
            output_fingerprint: (0, 0),
            list: VariableList {
                bounds: Rect::default(),
                selected: None,
                offset: 0.0,
            },
            output_pressed: None,
            replay_document: None,
            pending_search: None,
            process_status: String::new(),
        }
    }
}
impl MacrosController {
    pub fn capture_replay_target(&mut self, workspace: &Workspace, active: usize) {
        self.replay_document = workspace.editors.get(active).map(ReplayDocument::capture);
    }
    pub fn slot_name(&self, index: usize) -> Option<&str> {
        self.slots.get(index).and_then(Option::as_deref)
    }
    pub fn selected_slot(&self) -> Option<usize> {
        self.selected.as_ref().and_then(|name| {
            self.slots
                .iter()
                .position(|entry| entry.as_ref() == Some(name))
        })
    }
    pub fn serialized_slots(&self) -> Vec<(usize, String)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, name)| {
                name.as_ref()
                    .and_then(|name| self.library.get(name))
                    .map(|definition| (slot, definition.export_toml()))
            })
            .collect()
    }
    pub fn restore_library(
        &mut self,
        entries: Vec<(usize, String)>,
        registry: &CommandRegistry,
    ) -> Result<(), String> {
        let mut staged = Self::default();
        for (slot, text) in entries {
            staged.import_slot(slot, &text, registry)?;
        }
        self.library = staged.library;
        self.slots = staged.slots;
        self.selected = staged.selected;
        Ok(())
    }
    pub fn import_slot(
        &mut self,
        slot: usize,
        text: &str,
        registry: &CommandRegistry,
    ) -> Result<(), String> {
        if slot >= 32 || self.slots[slot].is_some() {
            return Err("Macro slot is already occupied".into());
        }
        let definition = Macro::import_toml(text, registry)?;
        self.check_library_budget(&definition)?;
        if self.library.contains_key(&definition.name) {
            return Err("A macro with this name already exists".into());
        }
        self.slots[slot] = Some(definition.name.clone());
        self.selected.get_or_insert_with(|| definition.name.clone());
        self.library.insert(definition.name.clone(), definition);
        Ok(())
    }
    fn check_library_budget(&self, definition: &Macro) -> Result<(), String> {
        let bytes = self
            .library
            .values()
            .map(|value| value.export_toml().len())
            .sum::<usize>();
        if bytes.saturating_add(definition.export_toml().len()) > 16 * 1024 * 1024 {
            return Err("Macro library exceeds 16 MiB".into());
        }
        Ok(())
    }
    pub fn select_slot(&mut self, slot: usize) -> Result<(), String> {
        self.selected = Some(
            self.slot_name(slot)
                .ok_or("Saved macro slot is empty")?
                .to_string(),
        );
        Ok(())
    }
    pub fn show_manager(&mut self) {
        self.manager.show(&self.library, self.selected.as_deref());
    }
    pub fn set_typing_delay(
        &mut self,
        interval_ms: u64,
        registry: &CommandRegistry,
    ) -> Result<(), String> {
        if interval_ms > 60_000 {
            return Err("Typing delay must be 0–60000 ms".into());
        }
        let name = self.selected.as_ref().ok_or("Select a macro first")?;
        let mut definition = self.library.get(name).cloned().ok_or("Macro not found")?;
        let mut changed = false;
        for event in &mut definition.events {
            match event {
                MacroEvent::Command { id, arguments } if id == "edit.insert_text" => {
                    if let Some(text) = arguments.get("text").filter(|text| !text.is_empty()) {
                        *event = MacroEvent::TypeText {
                            id: id.clone(),
                            text: text.clone(),
                            interval_ms,
                        };
                        changed = true;
                    }
                }
                MacroEvent::TypeText {
                    interval_ms: delay, ..
                } => {
                    *delay = interval_ms;
                    changed = true;
                }
                _ => {}
            }
        }
        if !changed {
            return Err("Selected macro has no recorded typing".into());
        }
        definition.validate(registry)?;
        self.library.insert(name.clone(), definition);
        Ok(())
    }
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
        let slot = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or("Macro library is full (32 macros)")?;
        if self.library.contains_key(name) {
            return Err("A macro with this name already exists".into());
        }
        let previous = self.recorder.clone();
        let definition = self.recorder.stop(name, registry)?;
        if let Err(error) = self.check_library_budget(&definition) {
            self.recorder = previous;
            return Err(error);
        }
        self.slots[slot] = Some(name.into());
        self.library.insert(name.into(), definition);
        self.selected = Some(name.into());
        self.status = format!("Recorded {name}");
        Ok(())
    }
    pub fn import(&mut self, text: &str, registry: &CommandRegistry) -> Result<(), String> {
        let slot = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or("Macro library is full (32 macros)")?;
        self.import_slot(slot, text, registry)?;
        self.select_slot(slot)?;
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
        let bytes = self
            .library
            .iter()
            .filter(|(name, _)| name.as_str() != old)
            .map(|(_, value)| value.export_toml().len())
            .sum::<usize>();
        if bytes.saturating_add(definition.export_toml().len()) > 16 * 1024 * 1024 {
            return Err("Renamed macro would exceed the 16 MiB library limit".into());
        }
        self.library.remove(old);
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.as_deref() == Some(old))
        {
            *slot = Some(new.into());
        }
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
        self.replay_document = None;
        self.pending_search = None;
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
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Option<PlaybackState> {
        let playback = self.playback.as_mut()?;
        if !matches!(
            playback.state(),
            PlaybackState::Running | PlaybackState::Waiting(_)
        ) {
            return None;
        }
        if self.replay_document.is_none() {
            self.replay_document = workspace.editors.get(active).map(ReplayDocument::capture);
        }
        let active = self
            .replay_document
            .as_ref()
            .and_then(|target| {
                workspace
                    .editors
                    .iter()
                    .position(|editor| target.matches(editor))
            })
            .unwrap_or(usize::MAX);
        let mut executor = WorkspaceExecutor {
            workspace,
            active,
            context,
            pending_search: &mut self.pending_search,
            notify,
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
        let changed = fingerprint != self.output_fingerprint || self.process_status != status;
        if fingerprint != self.output_fingerprint {
            let value = output.text();
            self.rows.0 = value
                .split('\n')
                .take(20_000)
                .map(|line| line.chars().take(8192).collect())
                .collect();
            self.output_fingerprint = fingerprint;
        }
        self.process_status = status;
        changed
    }
    pub fn output_event(&mut self, event: UiEvent) -> Option<process::OutputLink> {
        use bareline_ui::controls::Key;
        let activate = match event {
            UiEvent::PointerDown(point) => {
                self.output_pressed = self.list.hit_test(&self.rows, point);
                self.list.selected = self.output_pressed;
                false
            }
            UiEvent::PointerUp(point) => {
                let selected = self.output_pressed.take();
                selected.is_some() && selected == self.list.hit_test(&self.rows, point)
            }
            UiEvent::Key(Key::Up | Key::Down) => {
                let index = self.list.selected.unwrap_or(0);
                let index = if matches!(event, UiEvent::Key(Key::Up)) {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(self.rows.0.len().saturating_sub(1))
                };
                self.list.reveal(&self.rows, index);
                false
            }
            UiEvent::Key(Key::Enter) => true,
            _ => false,
        };
        if activate {
            self.list
                .selected
                .and_then(|index| self.rows.0.get(index))
                .and_then(|line| process::parse_output_link(line))
        } else {
            None
        }
    }
    pub fn scroll_output(&mut self, delta: f64) {
        let maximum = (self.rows.0.len() as f64 * 28. - self.list.bounds.height as f64).max(0.);
        self.list.offset = (self.list.offset + delta).clamp(0., maximum);
    }
    pub fn semantics(&self) -> Vec<Semantics> {
        let mut nodes = self.manager.semantics();
        if !self.output_open {
            return nodes;
        }
        for row in self.list.visible(&self.rows, 0) {
            let label = self.rows.label(row.index);
            let mut node = Semantics::new(
                ViewId(2_000_000 + row.index as u64),
                SemanticRole::ListItem,
                label,
                "output.open_link",
                row.bounds,
                ControlState {
                    focused: self.list.selected == Some(row.index),
                    ..Default::default()
                },
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::Select);
            node.selected = self.list.selected == Some(row.index);
            if process::parse_output_link(label).is_some() {
                node.actions.push(SemanticAction::Invoke);
            }
            nodes.push(node);
        }
        nodes
    }
    pub fn output_accessibility(&mut self, id: u64, invoke: bool) -> Option<process::OutputLink> {
        let index = usize::try_from(id.checked_sub(2_000_000)?).ok()?;
        if !self.list.reveal(&self.rows, index) {
            return None;
        }
        if invoke {
            self.rows
                .0
                .get(index)
                .and_then(|line| process::parse_output_link(line))
        } else {
            None
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
            &format!("{}  {}", self.status, self.process_status),
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
    pending_search: &'a mut Option<PendingSearch>,
    notify: Arc<dyn Fn() + Send + Sync>,
}
impl MacroExecutor for WorkspaceExecutor<'_> {
    fn context(&self) -> CommandContext {
        self.context.clone()
    }
    fn progress(&self) -> Progress {
        self.workspace
            .editors
            .get(self.active)
            .map(|editor| match editor {
                crate::workspace::WorkspaceEditor::Resident(editor) => Progress {
                    document: 0,
                    position: editor.selection.caret as u64,
                    revision: editor.snapshot().revision.0,
                    eof: editor.selection.caret >= editor.snapshot().len(),
                },
                crate::workspace::WorkspaceEditor::Paged(editor) => {
                    let position = editor
                        .viewport_start()
                        .0
                        .saturating_add(editor.surface.selection.caret);
                    Progress {
                        document: 0,
                        position: position as u64,
                        revision: editor.snapshot().revision.0,
                        eof: position >= editor.snapshot().len(),
                    }
                }
            })
            .unwrap_or(Progress {
                document: u64::MAX,
                position: 0,
                revision: 0,
                eof: true,
            })
    }
    fn poll_execution(&mut self) -> Result<bool, String> {
        if let Some(pending) = self.pending_search.as_ref() {
            if self.workspace.find.query() != pending.query
                || !self
                    .workspace
                    .editors
                    .get(self.active)
                    .is_some_and(|editor| pending.source.same_state(editor))
            {
                *self.pending_search = None;
                return Err(
                    "Recorded search was superseded; resume to run its captured query again".into(),
                );
            }
            let backwards = pending.backwards;
            self.workspace.find.pump();
            if self.workspace.find.searching() {
                return Ok(false);
            }
            *self.pending_search = None;
            if self.workspace.find.completed_results().is_none()
                && self.workspace.find.completed_paged_results().is_none()
            {
                return Err(format!(
                    "Search did not complete: {}",
                    self.workspace.find.status
                ));
            }
            let editor = self
                .workspace
                .editors
                .get_mut(self.active)
                .ok_or("Macro document closed")?;
            let at = if backwards {
                editor.selection.anchor.min(editor.selection.caret)
            } else {
                editor.selection.anchor.max(editor.selection.caret)
            };
            match editor {
                crate::workspace::WorkspaceEditor::Resident(editor) => {
                    let range = self
                        .workspace
                        .find
                        .next(editor.snapshot(), at, backwards)
                        .ok_or("Search has no current match")?;
                    editor.enqueue(Input::SetCaret(range.start.0, false));
                    editor.enqueue(Input::SetCaret(range.end.0, true));
                }
                crate::workspace::WorkspaceEditor::Paged(editor) => {
                    let at = editor.viewport_start().0.saturating_add(at);
                    let range = self
                        .workspace
                        .find
                        .next_paged(editor.snapshot(), at, backwards)
                        .ok_or("Search has no current match")?;
                    editor.restore_selection(range.start, range.end)?;
                }
            }
            editor.search_selection = true;
        }
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
        if matches!(
            command.0,
            "search.next" | "search.previous" | "search.find_next" | "search.find_previous"
        ) {
            let pattern = args
                .get("pattern")
                .ok_or("Recorded search requires an explicit query")?;
            let mode = match args.get("mode").map(String::as_str) {
                Some("literal") => bareline_search::SearchMode::Literal,
                Some("extended") => bareline_search::SearchMode::Extended,
                Some("regex") => bareline_search::SearchMode::Regex,
                _ => return Err("Recorded search mode is invalid".into()),
            };
            let flag = |key: &str| {
                args.get(key)
                    .and_then(|value| value.parse::<bool>().ok())
                    .ok_or_else(|| format!("Recorded search {key} is invalid"))
            };
            let case = flag("case")?;
            let whole_word = flag("whole_word")?;
            let scope = match (args.get("selection_start"), args.get("selection_end")) {
                (None, None) => None,
                (Some(start), Some(end)) => {
                    let start = start
                        .parse::<usize>()
                        .map_err(|_| "Invalid search selection")?;
                    let end = end
                        .parse::<usize>()
                        .map_err(|_| "Invalid search selection")?;
                    let length = match &*editor {
                        crate::workspace::WorkspaceEditor::Resident(editor) => {
                            editor.snapshot().len()
                        }
                        crate::workspace::WorkspaceEditor::Paged(editor) => editor.snapshot().len(),
                    };
                    if start > end || end > length {
                        return Err("Recorded search selection is outside the document".into());
                    }
                    Some(bareline_document::TextOffset(start)..bareline_document::TextOffset(end))
                }
                _ => return Err("Recorded search selection is incomplete".into()),
            };
            let find = &mut self.workspace.find;
            let mut query = bareline_search::SearchQuery::literal(pattern);
            query.mode = mode;
            query.case = if case {
                bareline_search::Case::Sensitive
            } else {
                bareline_search::Case::Folded
            };
            query.whole_word = whole_word;
            query.selection = scope;
            find.set_query(&query).map_err(str::to_string)?;
            find.open = true;
            find.focused = false;
            match &*editor {
                crate::workspace::WorkspaceEditor::Resident(editor) => {
                    find.refresh(editor.snapshot(), self.notify.clone())
                }
                crate::workspace::WorkspaceEditor::Paged(editor) => {
                    find.refresh_paged(editor.read_handle(), self.notify.clone())
                }
            }
            *self.pending_search = Some(PendingSearch {
                backwards: command.0.ends_with("previous"),
                query,
                source: ReplayDocument::capture(editor),
            });
            return Ok(());
        }
        if command.0.starts_with("editor.") {
            editor.error = None;
            return editor.execute_power_recorded(command.0, args);
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

#[cfg(test)]
mod tests {
    use super::*;
    fn registry() -> CommandRegistry {
        let mut registry = bareline_commands::shell_commands();
        register_commands(&mut registry);
        registry
    }
    #[test]
    fn saved_slot_survives_rename_and_restart_and_rejects_duplicate_import() {
        let registry = registry();
        let mut controller = MacrosController::default();
        let definition = Macro {
            name: "Original".into(),
            events: vec![MacroEvent::Command {
                id: "edit.insert_text".into(),
                arguments: BTreeMap::from([("text".into(), "hello".into())]),
            }],
        };
        controller
            .import(&definition.export_toml(), &registry)
            .unwrap();
        let slot = controller.selected_slot().unwrap();
        controller.rename("Original", "Renamed").unwrap();
        assert_eq!(controller.selected_slot(), Some(slot));
        let saved = controller.serialized_slots();
        let mut reopened = MacrosController::default();
        for (slot, text) in saved {
            reopened.import_slot(slot, &text, &registry).unwrap();
        }
        reopened.select_slot(slot).unwrap();
        assert_eq!(reopened.selected.as_deref(), Some("Renamed"));
        assert!(registry.lookup(SAVED_COMMANDS[slot]).is_some());
        assert!(
            reopened
                .import(&reopened.export_selected().unwrap(), &registry)
                .is_err()
        );
        assert_eq!(reopened.library.len(), 1);
    }
    #[test]
    fn rename_rejects_library_budget_growth_without_changing_slot() {
        let registry = registry();
        let mut controller = MacrosController::default();
        let mut used = 0usize;
        for slot in 0..25 {
            let name = format!("Macro {slot}");
            let definition = Macro {
                name: name.clone(),
                events: vec![MacroEvent::TypeText {
                    id: "edit.insert_text".into(),
                    text: "a".repeat(650_000),
                    interval_ms: 1,
                }],
            };
            definition.validate(&registry).unwrap();
            used += definition.export_toml().len();
            controller.slots[slot] = Some(name.clone());
            controller.library.insert(name, definition);
        }
        let mut last = Macro {
            name: "Last".into(),
            events: vec![MacroEvent::TypeText {
                id: "edit.insert_text".into(),
                text: String::new(),
                interval_ms: 1,
            }],
        };
        let remaining = 16 * 1024 * 1024 - used - last.export_toml().len() - 1;
        if let MacroEvent::TypeText { text, .. } = &mut last.events[0] {
            *text = "a".repeat(remaining);
        }
        last.validate(&registry).unwrap();
        controller.library.insert("Last".into(), last);
        controller.slots[25] = Some("Last".into());
        controller.selected = Some("Last".into());
        assert!(controller.rename("Last", "Longer name").is_err());
        assert_eq!(controller.selected.as_deref(), Some("Last"));
        assert_eq!(controller.slot_name(25), Some("Last"));
        assert!(!controller.library.contains_key("Longer name"));
    }
    #[test]
    fn output_arrows_only_select_enter_activates_and_scroll_is_bounded() {
        let mut controller = MacrosController::default();
        controller.rows.0 = vec![
            "C:\\work\\first.rs:2:3".into(),
            "C:\\work\\second.rs:4:5".into(),
        ];
        controller.list.bounds = rect(0., 0., 300., 28.);
        assert_eq!(
            controller.output_event(UiEvent::Key(bareline_ui::controls::Key::Down)),
            None
        );
        let link = controller
            .output_event(UiEvent::Key(bareline_ui::controls::Key::Enter))
            .unwrap();
        assert_eq!((link.line, link.column), (4, 5));
        controller.scroll_output(f64::MAX);
        assert_eq!(controller.list.offset, 28.);
        controller.scroll_output(-f64::MAX);
        assert_eq!(controller.list.offset, 0.);
    }
}
