// SPDX-License-Identifier: MPL-2.0
//! Normalized command macros and bounded, explicitly authorized external command execution.
use bareline_commands::{CommandContext, CommandId, CommandRegistry};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

const MAX_EVENTS: usize = 10_000;
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MacroEvent {
    Command {
        id: String,
        arguments: BTreeMap<String, String>,
    },
    /// Text is sent through the same normal edit command, one Unicode scalar per tick.
    TypeText { id: String, text: String, interval_ms: u64 },
}
impl MacroEvent {
    pub fn command_id(&self) -> &str {
        match self {
            Self::Command { id, .. } | Self::TypeText { id, .. } => id,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Macro {
    pub name: String,
    pub events: Vec<MacroEvent>,
}
impl Macro {
    pub fn validate(&self, registry: &CommandRegistry) -> Result<(), String> {
        if self.name.trim().is_empty() || self.name.len() > 4096 || self.name.chars().any(char::is_control) {
            return Err("Macro name is empty".into());
        }
        if self.events.is_empty() || self.events.len() > MAX_EVENTS {
            return Err("Macro requires 1–10000 events".into());
        }
        let mut bytes = self.name.len();
        for event in &self.events {
            if registry.lookup(event.command_id()).is_none() {
                return Err(format!("Unknown command: {}", event.command_id()));
            }
            match event {
                MacroEvent::Command { id, arguments } => {
                    bytes += id.len() + arguments.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>();
                }
                MacroEvent::TypeText { id, text, interval_ms } => {
                    if text.is_empty() {
                        return Err("Typing action requires nonempty text".into());
                    }
                    bytes += id.len() + text.len();
                    if *interval_ms > 60_000 {
                        return Err("Typing interval exceeds 60 seconds".into());
                    }
                }
            }
            if bytes > MAX_FILE_BYTES {
                return Err("Macro exceeds 4 MiB".into());
            }
        }
        if self.serialized_bound(&self.name) > MAX_FILE_BYTES {
            return Err("Macro exceeds the serialized TOML size limit".into());
        }
        Ok(())
    }
    // Conservative TOML escaping/framing bound, checked before serialization allocates.
    fn serialized_bound(&self, name: &str) -> usize {
        self.events
            .iter()
            .fold(128usize.saturating_add(name.len().saturating_mul(6)), |total, event| {
                total.saturating_add(128).saturating_add(match event {
                    MacroEvent::Command { id, arguments } => id.len().saturating_mul(6).saturating_add(
                        arguments
                            .iter()
                            .map(|(key, value)| {
                                key.len()
                                    .saturating_add(value.len())
                                    .saturating_mul(6)
                                    .saturating_add(16)
                            })
                            .sum::<usize>(),
                    ),
                    MacroEvent::TypeText { id, text, .. } => id.len().saturating_add(text.len()).saturating_mul(6),
                })
            })
    }
    pub fn rename(&mut self, name: &str) -> Result<(), String> {
        if name.trim().is_empty() || name.len() > 4096 || name.chars().any(char::is_control) {
            return Err("Invalid macro name".into());
        }
        if self.serialized_bound(name) > MAX_FILE_BYTES {
            return Err("Renamed macro exceeds the serialized TOML size limit".into());
        }
        self.name = name.into();
        Ok(())
    }
    pub fn export_toml(&self) -> String {
        let mut text = format!("format_version = 1\nname = {}\n", quote(&self.name));
        for event in &self.events {
            text.push_str("\n[[events]]\n");
            match event {
                MacroEvent::Command { id, arguments } => {
                    text.push_str(&format!("kind = 'command'\ncommand = {}\n", quote(id)));
                    text.push_str("arguments = { ");
                    text.push_str(
                        &arguments
                            .iter()
                            .map(|(key, value)| format!("{} = {}", quote(key), quote(value)))
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                    text.push_str(" }\n");
                }
                MacroEvent::TypeText {
                    id,
                    text: value,
                    interval_ms,
                } => text.push_str(&format!(
                    "kind = 'type_text'\ncommand = {}\ntext = {}\ninterval_ms = {}\n",
                    quote(id),
                    quote(value),
                    interval_ms
                )),
            }
        }
        text
    }
    pub fn import_toml(text: &str, registry: &CommandRegistry) -> Result<Self, String> {
        if text.len() > MAX_FILE_BYTES {
            return Err("Macro exceeds 4 MiB".into());
        }
        let doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| error.to_string())?;
        if doc.get("format_version").and_then(|v| v.as_integer()) != Some(1) {
            return Err("Unsupported macro format_version".into());
        }
        if doc
            .iter()
            .any(|(key, _)| !["format_version", "name", "events"].contains(&key))
        {
            return Err("Unknown macro field".into());
        }
        let name = doc
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("Missing macro name")?
            .to_string();
        let tables = doc
            .get("events")
            .and_then(|v| v.as_array_of_tables())
            .ok_or("Missing macro events")?;
        if tables.len() > MAX_EVENTS {
            return Err("Too many macro events".into());
        }
        let mut events = Vec::new();
        for table in tables {
            let kind = table.get("kind").and_then(|v| v.as_str()).ok_or("Missing event kind")?;
            let id = table
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or("Missing event command")?
                .to_string();
            let allowed: &[&str] = match kind {
                "command" => &["kind", "command", "arguments"],
                "type_text" => &["kind", "command", "text", "interval_ms"],
                _ => return Err(format!("Unknown event kind: {kind}")),
            };
            if table.iter().any(|(key, _)| !allowed.contains(&key)) {
                return Err("Unknown event field".into());
            }
            events.push(match kind {
                "command" => {
                    let mut arguments = BTreeMap::new();
                    if let Some(value) = table.get("arguments") {
                        let values = value.as_inline_table().ok_or("arguments must be an inline table")?;
                        for (key, value) in values {
                            arguments.insert(
                                key.to_string(),
                                value.as_str().ok_or("Arguments must be strings")?.to_string(),
                            );
                        }
                    }
                    MacroEvent::Command { id, arguments }
                }
                _ => MacroEvent::TypeText {
                    id,
                    text: table
                        .get("text")
                        .and_then(|v| v.as_str())
                        .ok_or("Missing typing text")?
                        .to_string(),
                    interval_ms: u64::try_from(
                        table
                            .get("interval_ms")
                            .and_then(|v| v.as_integer())
                            .ok_or("Missing typing interval")?,
                    )
                    .map_err(|_| "Negative typing interval")?,
                },
            });
        }
        let result = Self { name, events };
        result.validate(registry)?;
        Ok(result)
    }
}
fn quote(value: &str) -> String {
    let mut output = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if ch.is_control() => output.push_str(&format!("\\u{:04X}", ch as u32)),
            ch => output.push(ch),
        }
    }
    output.push('"');
    output
}

#[derive(Clone, Default)]
pub struct Recorder {
    recording: bool,
    events: Vec<MacroEvent>,
    bytes: usize,
}
impl Recorder {
    pub fn start(&mut self) {
        self.recording = true;
        self.events.clear();
        self.bytes = 0;
    }
    pub fn cancel(&mut self) {
        self.recording = false;
        self.events.clear();
        self.bytes = 0;
    }
    pub fn recording(&self) -> bool {
        self.recording
    }
    /// Call only after successful execution. The owner explicitly declares normalized deterministic events.
    pub fn executed(
        &mut self,
        event: MacroEvent,
        recordable: bool,
        registry: &CommandRegistry,
    ) -> Result<bool, String> {
        if !self.recording || !recordable {
            return Ok(false);
        }
        if registry.lookup(event.command_id()).is_none() {
            return Err("Unknown recorded command".into());
        }
        let bytes = match &event {
            MacroEvent::Command { id, arguments } => {
                id.len().saturating_mul(6)
                    + arguments
                        .iter()
                        .map(|(k, v)| (k.len() + v.len()).saturating_mul(6).saturating_add(16))
                        .sum::<usize>()
            }
            MacroEvent::TypeText { id, text, .. } => (id.len() + text.len()).saturating_mul(6),
        }
        .saturating_add(128);
        if self.events.len() >= MAX_EVENTS
            || self.bytes.saturating_add(bytes).saturating_add(128 + 4096 * 6) > MAX_FILE_BYTES
        {
            return Err("Macro recording limit reached".into());
        }
        self.bytes += bytes;
        self.events.push(event);
        Ok(true)
    }
    pub fn stop(&mut self, name: &str, registry: &CommandRegistry) -> Result<Macro, String> {
        let result = Macro {
            name: name.into(),
            events: self.events.clone(),
        };
        result.validate(registry)?;
        self.recording = false;
        self.events.clear();
        self.bytes = 0;
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub document: u64,
    pub position: u64,
    pub revision: u64,
    pub eof: bool,
}
pub trait MacroExecutor {
    fn context(&self) -> CommandContext;
    fn progress(&self) -> Progress;
    /// Async document adapters return false until the submitted transaction acknowledges completion.
    /// An error must mean that the last transaction did not commit, so retrying its location is safe.
    fn poll_execution(&mut self) -> Result<bool, String> {
        Ok(true)
    }
    /// Must dispatch through normal app/document transactions; never write document bytes directly.
    fn execute(&mut self, command: CommandId, arguments: &BTreeMap<String, String>) -> Result<(), String>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    Once,
    Times(u32),
    UntilEof,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Location {
    pub iteration: u32,
    pub event: usize,
    pub text_byte: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackState {
    Running,
    Waiting(Duration),
    Complete,
    Cancelled,
    Failed { location: Location, reason: String },
}
pub struct Playback {
    definition: Macro,
    repeat: Repeat,
    max_iterations: u32,
    location: Location,
    state: PlaybackState,
    iteration_start: Option<Progress>,
    due: Option<Instant>,
    pending: Option<Option<(usize, usize, u64)>>,
}
impl Playback {
    pub fn new(
        definition: Macro,
        repeat: Repeat,
        max_iterations: u32,
        registry: &CommandRegistry,
    ) -> Result<Self, String> {
        definition.validate(registry)?;
        if max_iterations == 0 || matches!(repeat, Repeat::Times(0)) {
            return Err("Iteration limit must be positive".into());
        }
        if let Repeat::Times(count) = repeat
            && count > max_iterations
        {
            return Err("Repeat count exceeds safeguard".into());
        }
        Ok(Self {
            definition,
            repeat,
            max_iterations,
            location: Location::default(),
            state: PlaybackState::Running,
            iteration_start: None,
            due: None,
            pending: None,
        })
    }
    pub fn state(&self) -> &PlaybackState {
        &self.state
    }
    pub fn location(&self) -> Location {
        self.location
    }
    pub fn cancel(&mut self) {
        if matches!(self.state, PlaybackState::Running | PlaybackState::Waiting(_)) {
            self.state = PlaybackState::Cancelled;
            self.due = None;
        }
    }
    pub fn resume(&mut self) -> bool {
        if matches!(self.state, PlaybackState::Failed { .. }) && self.location.event < self.definition.events.len() {
            self.state = PlaybackState::Running;
            self.due = None;
            true
        } else {
            false
        }
    }
    fn fail(&mut self, reason: impl Into<String>) -> PlaybackState {
        self.state = PlaybackState::Failed {
            location: self.location,
            reason: reason.into(),
        };
        self.state.clone()
    }
    /// Performs at most one normal command/edit per tick; caller schedules Waiting without blocking UI.
    pub fn tick(
        &mut self,
        now: Instant,
        registry: &CommandRegistry,
        executor: &mut impl MacroExecutor,
    ) -> PlaybackState {
        if !matches!(self.state, PlaybackState::Running | PlaybackState::Waiting(_)) {
            return self.state.clone();
        }
        if let Some(next_text) = self.pending {
            match executor.poll_execution() {
                Ok(false) => {
                    self.state = PlaybackState::Waiting(Duration::from_millis(1));
                    return self.state.clone();
                }
                Err(error) => {
                    self.pending = None;
                    return self.fail(error);
                }
                Ok(true) => {
                    self.pending = None;
                    return self.finish_step(now, next_text, executor.progress());
                }
            }
        }
        if let Some(due) = self.due
            && now < due
        {
            self.state = PlaybackState::Waiting(due - now);
            return self.state.clone();
        }
        self.state = PlaybackState::Running;
        self.due = None;
        if self.iteration_start.is_none() {
            let progress = executor.progress();
            if self.repeat == Repeat::UntilEof && progress.eof {
                self.state = PlaybackState::Complete;
                return self.state.clone();
            }
            self.iteration_start = Some(progress);
        }
        let event = &self.definition.events[self.location.event];
        let Some(command) = registry.lookup(event.command_id()) else {
            return self.fail("Command no longer registered");
        };
        if let Err(error) = registry.dispatch_in(command, &executor.context()) {
            return self.fail(format!("Command unavailable: {error:?}"));
        }
        let mut next_text = None;
        let arguments = match event {
            MacroEvent::Command { arguments, .. } => arguments.clone(),
            MacroEvent::TypeText { text, interval_ms, .. } => {
                if let Some(ch) = text[self.location.text_byte..].chars().next() {
                    next_text = Some((self.location.text_byte + ch.len_utf8(), text.len(), *interval_ms));
                    BTreeMap::from([("text".into(), ch.to_string())])
                } else {
                    BTreeMap::from([("text".into(), String::new())])
                }
            }
        };
        if let Err(error) = executor.execute(command, &arguments) {
            return self.fail(error);
        }
        match executor.poll_execution() {
            Ok(false) => {
                self.pending = Some(next_text);
                self.state = PlaybackState::Waiting(Duration::from_millis(1));
                return self.state.clone();
            }
            Err(error) => return self.fail(error),
            Ok(true) => {}
        }
        self.finish_step(now, next_text, executor.progress())
    }
    fn finish_step(
        &mut self,
        now: Instant,
        next_text: Option<(usize, usize, u64)>,
        progress: Progress,
    ) -> PlaybackState {
        self.state = PlaybackState::Running;
        if let Some((next, length, delay)) = next_text
            && next < length
        {
            self.location.text_byte = next;
            self.due = Some(now + Duration::from_millis(delay));
            self.state = PlaybackState::Waiting(Duration::from_millis(delay));
            return self.state.clone();
        }
        self.location.text_byte = 0;
        self.location.event += 1;
        if self.location.event == self.definition.events.len() {
            self.location.iteration += 1;
            let complete = match self.repeat {
                Repeat::Once => true,
                Repeat::Times(count) => self.location.iteration >= count,
                Repeat::UntilEof => progress.eof,
            };
            if complete {
                self.state = PlaybackState::Complete;
            } else if self.location.iteration >= self.max_iterations {
                return self.fail("Maximum macro iteration safeguard reached");
            } else if self.repeat == Repeat::UntilEof && self.iteration_start == Some(progress) {
                return self.fail("Until-EOF macro made no progress");
            } else {
                self.location.event = 0;
                self.iteration_start = Some(progress);
            }
        }
        self.state.clone()
    }
}

pub mod process;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serialized_bound_and_name_validation_do_not_discard_recording() {
        let registry = bareline_commands::shell_commands();
        let mut recorder = Recorder::default();
        recorder.start();
        recorder
            .executed(
                MacroEvent::Command {
                    id: "file.new".into(),
                    arguments: BTreeMap::new(),
                },
                true,
                &registry,
            )
            .unwrap();
        assert!(recorder.stop("bad\nname", &registry).is_err());
        assert!(recorder.recording());
        assert_eq!(recorder.stop("Saved", &registry).unwrap().events.len(), 1);
        let oversized = Macro {
            name: "Large".into(),
            events: vec![MacroEvent::TypeText {
                id: "file.new".into(),
                text: "\u{1}".repeat(800_000),
                interval_ms: 1,
            }],
        };
        assert!(oversized.validate(&registry).is_err());
        let mut near_limit = Macro {
            name: "a".into(),
            events: vec![MacroEvent::TypeText {
                id: "file.new".into(),
                text: "a".repeat(695_000),
                interval_ms: 1,
            }],
        };
        near_limit.validate(&registry).unwrap();
        assert!(near_limit.rename(&"b".repeat(4096)).is_err());
        assert_eq!(near_limit.name, "a");
    }
    use bareline_commands::shell_commands;
    fn definition() -> Macro {
        Macro {
            name: "Example".into(),
            events: vec![MacroEvent::Command {
                id: "edit.paste".into(),
                arguments: BTreeMap::from([("text".into(), "hello\n\"world\"\t".into())]),
            }],
        }
    }
    struct Executor {
        progress: Progress,
        failed: bool,
        values: Vec<String>,
    }
    impl Default for Executor {
        fn default() -> Self {
            Self {
                progress: Progress {
                    document: 1,
                    position: 0,
                    revision: 0,
                    eof: false,
                },
                failed: false,
                values: Vec::new(),
            }
        }
    }
    impl MacroExecutor for Executor {
        fn context(&self) -> CommandContext {
            CommandContext::default()
        }
        fn progress(&self) -> Progress {
            self.progress
        }
        fn execute(&mut self, _: CommandId, args: &BTreeMap<String, String>) -> Result<(), String> {
            if self.failed {
                return Err("Edit refused".into());
            }
            self.values.push(args.get("text").cloned().unwrap_or_default());
            self.progress.position += 1;
            Ok(())
        }
    }
    #[test]
    fn persistence_recording_and_repeat_preserve_normalized_text() {
        let registry = shell_commands();
        let value = definition();
        assert_eq!(Macro::import_toml(&value.export_toml(), &registry).unwrap(), value);
        assert!(
            Macro::import_toml(
                &value.export_toml().replace("format_version = 1", "format_version = 2"),
                &registry
            )
            .is_err()
        );
        let mut recorder = Recorder::default();
        recorder.start();
        assert!(!recorder.executed(value.events[0].clone(), false, &registry).unwrap());
        recorder.executed(value.events[0].clone(), true, &registry).unwrap();
        assert_eq!(recorder.stop("Example", &registry).unwrap(), value);
        let mut playback = Playback::new(value, Repeat::Times(2), 10, &registry).unwrap();
        let mut executor = Executor::default();
        assert_eq!(
            playback.tick(Instant::now(), &registry, &mut executor),
            PlaybackState::Running
        );
        assert_eq!(
            playback.tick(Instant::now(), &registry, &mut executor),
            PlaybackState::Complete
        );
        assert_eq!(executor.values, vec!["hello\n\"world\"\t"; 2]);
    }
    #[test]
    fn failed_command_resumes_exact_location_and_cancel_stops() {
        let registry = shell_commands();
        let mut playback = Playback::new(definition(), Repeat::Times(2), 2, &registry).unwrap();
        let mut executor = Executor {
            failed: true,
            ..Default::default()
        };
        assert!(matches!(
            playback.tick(Instant::now(), &registry, &mut executor),
            PlaybackState::Failed {
                location: Location {
                    iteration: 0,
                    event: 0,
                    ..
                },
                ..
            }
        ));
        executor.failed = false;
        assert!(playback.resume());
        playback.tick(Instant::now(), &registry, &mut executor);
        assert_eq!(executor.values.len(), 1);
        playback.cancel();
        assert_eq!(
            playback.tick(Instant::now(), &registry, &mut executor),
            PlaybackState::Cancelled
        );
        assert_eq!(executor.values.len(), 1);
    }
    #[test]
    fn ghost_typing_is_nonblocking_and_iteration_guard_cannot_resume_out_of_bounds() {
        let registry = shell_commands();
        let value = Macro {
            name: "Demo".into(),
            events: vec![MacroEvent::TypeText {
                id: "edit.paste".into(),
                text: "é好".into(),
                interval_ms: 50,
            }],
        };
        let mut playback = Playback::new(value, Repeat::UntilEof, 1, &registry).unwrap();
        let mut executor = Executor::default();
        let now = Instant::now();
        assert_eq!(
            playback.tick(now, &registry, &mut executor),
            PlaybackState::Waiting(Duration::from_millis(50))
        );
        assert!(matches!(
            playback.tick(now + Duration::from_millis(20), &registry, &mut executor),
            PlaybackState::Waiting(_)
        ));
        assert_eq!(executor.values, vec!["é"]);
        assert!(matches!(
            playback.tick(now + Duration::from_millis(50), &registry, &mut executor),
            PlaybackState::Failed { .. }
        ));
        assert_eq!(executor.values, vec!["é", "好"]);
        assert!(!playback.resume());
    }
    #[test]
    fn until_eof_stops_without_dispatch_at_eof_and_detects_no_progress() {
        let registry = shell_commands();
        let mut playback = Playback::new(definition(), Repeat::UntilEof, 100, &registry).unwrap();
        let mut executor = Executor::default();
        executor.progress.eof = true;
        assert_eq!(
            playback.tick(Instant::now(), &registry, &mut executor),
            PlaybackState::Complete
        );
        assert!(executor.values.is_empty());
        struct Stationary;
        impl MacroExecutor for Stationary {
            fn context(&self) -> CommandContext {
                Default::default()
            }
            fn progress(&self) -> Progress {
                Progress {
                    document: 1,
                    position: 0,
                    revision: 0,
                    eof: false,
                }
            }
            fn execute(&mut self, _: CommandId, _: &BTreeMap<String, String>) -> Result<(), String> {
                Ok(())
            }
        }
        let mut playback = Playback::new(definition(), Repeat::UntilEof, 100, &registry).unwrap();
        assert!(
            matches!(playback.tick(Instant::now(),&registry,&mut Stationary),PlaybackState::Failed {reason,..} if reason.contains("no progress"))
        );
    }
}
