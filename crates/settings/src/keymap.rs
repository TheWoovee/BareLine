// SPDX-License-Identifier: MPL-2.0
use crate::{MAX_CONFIG_BYTES, atomic_write_config, read_config};
use bareline_commands::{CommandRegistry, KeyBinding, Keymap, KeymapPreset};
use bareline_platform::LocalFileSystem;
use std::{io, path::Path};
use toml_edit::{DocumentMut, Item};
/// The commands that switch the keymap preset (BIZ-08): ID, title and preset.
pub const KEYMAP_PRESET_COMMANDS: [(&str, &str, KeymapPreset); 2] = [
    (
        "settings.keymap_preset_notepadpp",
        "Use Notepad++ Shortcuts",
        KeymapPreset::NotepadPlusPlus,
    ),
    (
        "settings.keymap_preset_bareline",
        "Use Bareline Shortcuts",
        KeymapPreset::Bareline,
    ),
];
/// The preset a `KEYMAP_PRESET_COMMANDS` command switches to.
pub fn keymap_preset_command(id: &str) -> Option<KeymapPreset> {
    KEYMAP_PRESET_COMMANDS
        .iter()
        .find(|(command, _, _)| *command == id)
        .map(|&(_, _, preset)| preset)
}
#[derive(Clone, Debug)]
pub struct KeymapDocument {
    document: DocumentMut,
    pub keymap: Keymap,
    /// The preset the bindings were laid out from (`preset = "…"`; absent means
    /// Bareline), so a preset switch can tell the person's own changes (BIZ-08).
    preset: KeymapPreset,
}
impl KeymapDocument {
    pub fn parse(text: &str, registry: &CommandRegistry) -> Result<Self, String> {
        if text.len() > MAX_CONFIG_BYTES {
            return Err("Keymap exceeds 1 MiB".into());
        }
        let mut keymap = Keymap::default();
        keymap.import_toml(text, registry)?;
        let document = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
        let preset = match document.get("preset") {
            None => KeymapPreset::Bareline,
            Some(item) => item
                .as_str()
                .and_then(KeymapPreset::from_id)
                .ok_or("Unknown keymap preset")?,
        };
        Ok(Self {
            document,
            keymap,
            preset,
        })
    }
    pub fn defaults(registry: &CommandRegistry) -> Self {
        Self::parse(&Keymap::defaults(registry).export_toml(), registry).expect("valid built-in keymap")
    }
    /// A fresh document for `keymap`, laid out from `preset`.
    pub fn from_keymap(keymap: &Keymap, preset: KeymapPreset, registry: &CommandRegistry) -> Result<Self, String> {
        let mut document = keymap.export_toml().parse::<DocumentMut>().map_err(|e| e.to_string())?;
        if preset != KeymapPreset::Bareline {
            // Older builds reject the field, so the default preset leaves it out.
            document["preset"] = toml_edit::value(preset.id());
        }
        Self::parse(&document.to_string(), registry)
    }
    pub fn preset(&self) -> KeymapPreset {
        self.preset
    }
    /// This keymap moved onto `preset`: shortcuts the person changed from the
    /// current preset stay as they are, everything else takes the new preset's
    /// bindings. The document is rebuilt, so its comments are not kept.
    pub fn with_preset(&self, preset: KeymapPreset, registry: &CommandRegistry) -> Result<Self, String> {
        if preset == self.preset {
            return Ok(self.clone());
        }
        let keymap = self.keymap.rebase(
            &Keymap::preset(registry, self.preset),
            &Keymap::preset(registry, preset),
            registry,
        )?;
        Self::from_keymap(&keymap, preset, registry)
    }
    pub fn to_toml(&self) -> String {
        self.document.to_string()
    }
    /// Transactional import retains the old document/map if parsing or conflicts fail.
    pub fn import(&mut self, text: &str, registry: &CommandRegistry) -> Result<(), String> {
        let next = Self::parse(text, registry)?;
        *self = next;
        Ok(())
    }
    /// Change one command while retaining all unrelated binding tables and comments.
    pub fn set_binding(&mut self, binding: KeyBinding, registry: &CommandRegistry) -> Result<(), String> {
        let mut bindings = self
            .keymap
            .bindings()
            .iter()
            .filter(|old| old.command != binding.command)
            .cloned()
            .collect::<Vec<_>>();
        bindings.push(binding.clone());
        let mut keymap = self.keymap.clone();
        keymap.replace(bindings, registry)?;
        let mut document = self.document.clone();
        let keys: toml_edit::Array = binding.sequence.iter().map(|key| key.label()).collect();
        let mut updated = false;
        if let Some(tables) = document.get_mut("bindings").and_then(Item::as_array_of_tables_mut) {
            // At most one replacement is generated even if the old map has alternate bindings.
            let mut replacement = toml_edit::ArrayOfTables::new();
            for table in tables.iter() {
                if table.get("command").and_then(Item::as_str) == Some(binding.command.0) {
                    if !updated {
                        let mut table = table.clone();
                        let mut value = toml_edit::Value::Array(keys.clone());
                        if let Some(old) = table.get("keys").and_then(Item::as_value) {
                            *value.decor_mut() = old.decor().clone();
                        }
                        table["keys"] = Item::Value(value);
                        replacement.push(table);
                        updated = true;
                    }
                } else {
                    replacement.push(table.clone());
                }
            }
            *tables = replacement;
        }
        if !updated {
            let mut table = toml_edit::Table::new();
            table["command"] = toml_edit::value(binding.command.0);
            table["keys"] = Item::Value(toml_edit::Value::Array(keys));
            if document.get("bindings").is_none() {
                document["bindings"] = Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
            }
            document["bindings"]
                .as_array_of_tables_mut()
                .ok_or("Invalid bindings")?
                .push(table);
        }
        let text = document.to_string();
        if text.len() > MAX_CONFIG_BYTES {
            return Err("Keymap exceeds 1 MiB".into());
        }
        self.document = document;
        self.keymap = keymap;
        Ok(())
    }
    pub fn load(path: &Path, registry: &CommandRegistry) -> io::Result<Self> {
        let bytes = read_config(path)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Self::parse(text, registry).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    pub fn save(&self, path: &Path, platform: &dyn LocalFileSystem) -> io::Result<()> {
        atomic_write_config(path, self.to_toml().as_bytes(), platform)
    }
}
