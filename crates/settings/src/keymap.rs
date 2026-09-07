// SPDX-License-Identifier: MPL-2.0
use crate::{MAX_CONFIG_BYTES, atomic_write_config, read_config};
use bareline_commands::{CommandRegistry, KeyBinding, Keymap};
use bareline_platform::LocalFileSystem;
use std::{io, path::Path};
use toml_edit::{DocumentMut, Item};
#[derive(Clone, Debug)]
pub struct KeymapDocument {
    document: DocumentMut,
    pub keymap: Keymap,
}
impl KeymapDocument {
    pub fn parse(text: &str, registry: &CommandRegistry) -> Result<Self, String> {
        if text.len() > MAX_CONFIG_BYTES {
            return Err("Keymap exceeds 1 MiB".into());
        }
        let mut keymap = Keymap::default();
        keymap.import_toml(text, registry)?;
        let document = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
        Ok(Self { document, keymap })
    }
    pub fn defaults(registry: &CommandRegistry) -> Self {
        Self::parse(&Keymap::defaults(registry).export_toml(), registry)
            .expect("valid built-in keymap")
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
    pub fn set_binding(
        &mut self,
        binding: KeyBinding,
        registry: &CommandRegistry,
    ) -> Result<(), String> {
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
        if let Some(tables) = document
            .get_mut("bindings")
            .and_then(Item::as_array_of_tables_mut)
        {
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
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Self::parse(text, registry).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    pub fn save(&self, path: &Path, platform: &dyn LocalFileSystem) -> io::Result<()> {
        atomic_write_config(path, self.to_toml().as_bytes(), platform)
    }
}
