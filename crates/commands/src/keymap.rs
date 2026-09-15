// SPDX-License-Identifier: MPL-2.0
use crate::{CommandId, CommandRegistry};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    Logical(String),
    Physical(String),
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyChord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub key: Key,
}
impl KeyChord {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut chord = Self {
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
            key: Key::Logical(String::new()),
        };
        let parts: Vec<_> = value.split('+').collect();
        for modifier in &parts[..parts.len().saturating_sub(1)] {
            let flag = match modifier.to_ascii_lowercase().as_str() {
                "ctrl" => &mut chord.ctrl,
                "alt" => &mut chord.alt,
                "shift" => &mut chord.shift,
                "meta" => &mut chord.meta,
                _ => return Err(format!("Unknown modifier: {modifier}")),
            };
            if *flag {
                return Err(format!("Repeated modifier: {modifier}"));
            }
            *flag = true;
        }
        let key = parts.last().copied().unwrap_or_default();
        if key.is_empty() || key.chars().any(char::is_whitespace) {
            return Err("A chord requires one nonempty key".into());
        }
        chord.key = if let Some(code) = key.strip_prefix("Physical:") {
            if code.is_empty() {
                return Err("Physical key code is empty".into());
            }
            Key::Physical(code.into())
        } else {
            let normalized = key.to_uppercase();
            let normalized = match normalized.as_str() {
                "ARROWLEFT" => "LEFT".into(),
                "ARROWRIGHT" => "RIGHT".into(),
                "ARROWUP" => "UP".into(),
                "ARROWDOWN" => "DOWN".into(),
                _ => normalized,
            };
            let character = key.chars().count() == 1 && !key.chars().any(char::is_control);
            let named = matches!(
                normalized.as_str(),
                "BACKSPACE"
                    | "TAB"
                    | "ENTER"
                    | "ESCAPE"
                    | "SPACE"
                    | "DELETE"
                    | "INSERT"
                    | "HOME"
                    | "END"
                    | "PAGEUP"
                    | "PAGEDOWN"
                    | "LEFT"
                    | "RIGHT"
                    | "UP"
                    | "DOWN"
                    | "ARROWLEFT"
                    | "ARROWRIGHT"
                    | "ARROWUP"
                    | "ARROWDOWN"
                    | "CONTEXTMENU"
                    | "PRINTSCREEN"
                    | "SCROLLLOCK"
                    | "PAUSE"
                    | "CAPSLOCK"
                    | "NUMLOCK"
            );
            let function = normalized
                .strip_prefix('F')
                .and_then(|number| number.parse::<u8>().ok())
                .is_some_and(|number| (1..=35).contains(&number));
            if !character && !named && !function {
                return Err(format!("Unknown key: {key}"));
            }
            Key::Logical(normalized)
        };
        Ok(chord)
    }
    pub fn label(&self) -> String {
        let mut value = String::new();
        for (enabled, text) in [
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
            (self.meta, "Meta+"),
        ] {
            if enabled {
                value.push_str(text);
            }
        }
        match &self.key {
            Key::Logical(key) => value.push_str(key),
            Key::Physical(key) => {
                value.push_str("Physical:");
                value.push_str(key);
            }
        }
        value
    }
}
/// Windows-convention display of a chord, used everywhere a shortcut is shown
/// to a person (menus, palette, tooltips, the shortcut mapper). Storage still
/// goes through `KeyChord::label`, so exported keymaps keep round-tripping.
pub fn display_chord(chord: &KeyChord) -> String {
    let mut value = String::new();
    for (enabled, text) in [
        (chord.ctrl, "Ctrl+"),
        (chord.alt, "Alt+"),
        (chord.shift, "Shift+"),
        (chord.meta, "Win+"),
    ] {
        if enabled {
            value.push_str(text);
        }
    }
    match &chord.key {
        Key::Physical(code) => {
            // Physical bindings keep their explicit form so they remain
            // unambiguous (and round-trip through the shortcut mapper).
            value.push_str("Physical:");
            value.push_str(code);
        }
        Key::Logical(key) => value.push_str(&friendly_key(key)),
    }
    value
}
fn friendly_key(key: &str) -> String {
    match key {
        "UP" => "Up",
        "DOWN" => "Down",
        "LEFT" => "Left",
        "RIGHT" => "Right",
        "TAB" => "Tab",
        "ESCAPE" => "Esc",
        "SPACE" => "Space",
        "ENTER" | "RETURN" => "Enter",
        "BACKSPACE" => "Backspace",
        "DELETE" => "Del",
        "INSERT" => "Ins",
        "HOME" => "Home",
        "END" => "End",
        "PAGEUP" => "PgUp",
        "PAGEDOWN" => "PgDn",
        "CONTEXTMENU" => "Menu",
        "PRINTSCREEN" => "PrtSc",
        "SCROLLLOCK" => "ScrLk",
        "PAUSE" => "Pause",
        "CAPSLOCK" => "Caps",
        "NUMLOCK" => "NumLk",
        other => return other.to_string(),
    }
    .to_string()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    pub command: CommandId,
    pub sequence: Vec<KeyChord>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    Exact,
    Prefix,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyConflict {
    pub first: CommandId,
    pub second: CommandId,
    pub kind: ConflictKind,
}
#[derive(Default, Clone, Debug)]
pub struct Keymap {
    bindings: Vec<KeyBinding>,
}
#[derive(Default, Clone, Copy, Debug)]
pub struct InputContext {
    pub alt_gr: bool,
    pub ime_composing: bool,
    pub dead_key: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyResolution {
    TextInput,
    NoMatch,
    Pending,
    Command(CommandId),
}
impl Keymap {
    pub fn defaults(registry: &CommandRegistry) -> Self {
        Self {
            bindings: registry
                .entries()
                .filter(|s| !s.shortcut.is_empty())
                .map(|s| KeyBinding {
                    command: s.id,
                    sequence: vec![KeyChord::parse(s.shortcut).expect("valid built-in shortcut")],
                })
                .collect(),
        }
    }
    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }
    pub fn conflicts(bindings: &[KeyBinding]) -> Vec<KeyConflict> {
        let mut conflicts = Vec::new();
        for (index, first) in bindings.iter().enumerate() {
            for second in &bindings[index + 1..] {
                if first.sequence.starts_with(&second.sequence) || second.sequence.starts_with(&first.sequence) {
                    conflicts.push(KeyConflict {
                        first: first.command,
                        second: second.command,
                        kind: if first.sequence == second.sequence {
                            ConflictKind::Exact
                        } else {
                            ConflictKind::Prefix
                        },
                    });
                }
            }
        }
        conflicts.sort_by_key(|c| (c.first, c.second));
        conflicts
    }
    /// Replaces atomically: invalid targets, empty chords and conflicts preserve the old map.
    pub fn replace(&mut self, bindings: Vec<KeyBinding>, registry: &CommandRegistry) -> Result<(), String> {
        for binding in &bindings {
            if registry.dispatch(binding.command).is_none() {
                return Err(format!("Unknown command: {}", binding.command.0));
            }
            if binding.sequence.is_empty() || binding.sequence.len() > 4 {
                return Err("Bindings require one to four chords".into());
            }
        }
        let conflicts = Self::conflicts(&bindings);
        if !conflicts.is_empty() {
            return Err(format!("Shortcut conflicts: {conflicts:?}"));
        }
        self.bindings = bindings;
        Ok(())
    }
    pub fn shortcut_label(&self, command: CommandId) -> String {
        self.bindings
            .iter()
            .filter(|b| b.command == command)
            .map(|b| b.sequence.iter().map(display_chord).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join(", ")
    }
    pub fn resolve(&self, sequence: &[KeyChord], context: InputContext) -> KeyResolution {
        if context.alt_gr || context.ime_composing || context.dead_key {
            return KeyResolution::TextInput;
        }
        if sequence.is_empty() {
            return KeyResolution::NoMatch;
        }
        for binding in &self.bindings {
            if binding.sequence == sequence {
                return KeyResolution::Command(binding.command);
            }
        }
        if self.bindings.iter().any(|b| b.sequence.starts_with(sequence)) {
            KeyResolution::Pending
        } else {
            KeyResolution::NoMatch
        }
    }
    pub fn export_toml(&self) -> String {
        fn quote(value: &str) -> String {
            format!(
                "\"{}\"",
                value
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\t', "\\t")
            )
        }
        let mut text = "version = 1\n".to_string();
        for binding in &self.bindings {
            text.push_str(&format!(
                "\n[[bindings]]\ncommand = {}\nkeys = [{}]\n",
                quote(binding.command.0),
                binding
                    .sequence
                    .iter()
                    .map(|key| quote(&key.label()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        text
    }
    pub fn import_toml(&mut self, text: &str, registry: &CommandRegistry) -> Result<(), String> {
        if text.len() > 1024 * 1024 {
            return Err("Keymap exceeds 1 MiB".into());
        }
        let doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| error.to_string())?;
        if doc.get("version").and_then(|v| v.as_integer()) != Some(1) {
            return Err("Unsupported keymap version".into());
        }
        if doc.iter().any(|(key, _)| key != "version" && key != "bindings") {
            return Err("Unknown keymap field".into());
        }
        let mut bindings = Vec::new();
        if let Some(value) = doc.get("bindings") {
            let tables = value
                .as_array_of_tables()
                .ok_or("bindings must be an array of tables")?;
            for table in tables {
                if table.iter().any(|(key, _)| key != "command" && key != "keys") {
                    return Err("Unknown binding field".into());
                }
                let command = table
                    .get("command")
                    .and_then(|v| v.as_str())
                    .ok_or("Binding command must be a string")?;
                let command = registry
                    .lookup(command)
                    .ok_or_else(|| format!("Unknown command: {command}"))?;
                let keys = table
                    .get("keys")
                    .and_then(|v| v.as_array())
                    .ok_or("Binding keys must be an array")?;
                let sequence = keys
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .ok_or_else(|| "Key must be a string".to_string())
                            .and_then(KeyChord::parse)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                bindings.push(KeyBinding { command, sequence });
            }
        }
        self.replace(bindings, registry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_commands;
    #[test]
    fn conflicts_fail_atomically_and_prefix_resolution_is_explicit() {
        let registry = shell_commands();
        let mut keymap = Keymap::defaults(&registry);
        let original = keymap.export_toml();
        let chord = KeyChord::parse("Ctrl+K").unwrap();
        let first = KeyBinding {
            command: CommandId("file.new"),
            sequence: vec![chord.clone()],
        };
        let second = KeyBinding {
            command: CommandId("file.open"),
            sequence: vec![chord.clone(), KeyChord::parse("Ctrl+O").unwrap()],
        };
        assert_eq!(
            Keymap::conflicts(&[first.clone(), second.clone()])[0].kind,
            ConflictKind::Prefix
        );
        assert!(keymap.replace(vec![first, second.clone()], &registry).is_err());
        assert_eq!(keymap.export_toml(), original);
        keymap.replace(vec![second.clone()], &registry).unwrap();
        assert_eq!(
            keymap.resolve(&[chord], InputContext::default()),
            KeyResolution::Pending
        );
        assert_eq!(
            keymap.resolve(&second.sequence, InputContext::default()),
            KeyResolution::Command(second.command)
        );
    }
    #[test]
    fn toml_roundtrip_remap_and_bad_import_preserves_previous() {
        let registry = shell_commands();
        let mut keymap = Keymap::defaults(&registry);
        let text = keymap.export_toml();
        keymap.import_toml(&text, &registry).unwrap();
        assert_eq!(keymap.export_toml(), text);
        for bad in [
            "version = 2",
            "version = 1\nextra = true",
            "version = 1\n[[bindings]]\ncommand='missing'\nkeys=['Ctrl+K']",
            "version = 1\n[[bindings]]\ncommand='file.new'\nkeys=[]",
            "version = 1\n[[bindings]]\ncommand='file.new'\nkeys=['Ctrl+K']\n[[bindings]]\ncommand='file.open'\nkeys=['Ctrl+K']",
        ] {
            assert!(keymap.import_toml(bad, &registry).is_err());
            assert_eq!(keymap.export_toml(), text);
        }
        keymap
            .import_toml(
                "# user keymap\nversion=1\n[[bindings]]\ncommand='file.new'\nkeys=['Ctrl+Physical:KeyN']",
                &registry,
            )
            .unwrap();
        assert_eq!(keymap.shortcut_label(CommandId("file.new")), "Ctrl+Physical:KeyN");
    }
    #[test]
    fn display_labels_follow_windows_conventions() {
        for (chord, expected) in [
            ("Alt+Up", "Alt+Up"),
            ("Ctrl+Alt+Down", "Ctrl+Alt+Down"),
            ("Tab", "Tab"),
            ("Escape", "Esc"),
            ("Ctrl+Space", "Ctrl+Space"),
            ("Delete", "Del"),
            ("PageUp", "PgUp"),
            ("Ctrl+PageDown", "Ctrl+PgDn"),
            ("Shift+Enter", "Shift+Enter"),
            ("Ctrl+Shift+Left", "Ctrl+Shift+Left"),
            ("Ctrl+S", "Ctrl+S"),
            ("F5", "F5"),
        ] {
            assert_eq!(
                display_chord(&KeyChord::parse(chord).unwrap()),
                expected,
                "display of {chord}"
            );
        }
        // The storage form keeps the uppercase key names so exports still parse.
        assert_eq!(KeyChord::parse("Alt+Up").unwrap().label(), "Alt+UP");
        assert_eq!(KeyChord::parse("Escape").unwrap().label(), "ESCAPE");
    }
    #[test]
    fn altgr_ime_and_dead_keys_always_reach_text_input() {
        let registry = shell_commands();
        let mut keymap = Keymap::default();
        let sequence = vec![KeyChord::parse("Ctrl+Alt+E").unwrap()];
        keymap
            .replace(
                vec![KeyBinding {
                    command: CommandId("file.new"),
                    sequence: sequence.clone(),
                }],
                &registry,
            )
            .unwrap();
        for context in [
            InputContext {
                alt_gr: true,
                ..Default::default()
            },
            InputContext {
                ime_composing: true,
                ..Default::default()
            },
            InputContext {
                dead_key: true,
                ..Default::default()
            },
        ] {
            assert_eq!(keymap.resolve(&sequence, context), KeyResolution::TextInput);
        }
        assert_eq!(
            keymap.resolve(&sequence, InputContext::default()),
            KeyResolution::Command(CommandId("file.new"))
        );
        assert!(KeyChord::parse("Ctrl+Ctrl+A").is_err());
        assert!(KeyChord::parse("Ctrl+").is_err());
    }
}
