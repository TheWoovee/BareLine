// SPDX-License-Identifier: MPL-2.0
//! Validated, machine-written installed language definitions, one atomic file per ID.
//! Reading and writing this store is worker-only. User-authored import/export remains separate.
use bareline_platform::LocalFileSystem;
use bareline_syntax::udl::Definition;
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
};

const MAX_FILE: usize = 128 * 1024;
const MAX_TOTAL: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 128;

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    file_system: Arc<dyn LocalFileSystem>,
}

impl Store {
    pub fn new(root: PathBuf, file_system: Arc<dyn LocalFileSystem>) -> Self {
        Self { root, file_system }
    }

    fn paths(&self) -> Result<Vec<PathBuf>, String> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };
        let mut paths = Vec::new();
        for (visited, entry) in entries.enumerate() {
            if visited >= MAX_ENTRIES * 2 {
                return Err("Language directory entry limit exceeded".into());
            }
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                if paths.len() == MAX_ENTRIES {
                    return Err("Language catalog limit reached".into());
                }
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }

    pub fn load(&self) -> Result<Vec<Definition>, String> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let _guard = self
            .file_system
            .guard_directory(&self.root)
            .map_err(|e| e.to_string())?;
        let mut definitions = Vec::new();
        let mut total = 0;
        for path in self.paths()? {
            let file = self.file_system.open_sealed_read(&path).map_err(|e| e.to_string())?;
            if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                return Err("Language catalog requires regular files".into());
            }
            let mut raw = Vec::new();
            file.take(MAX_FILE as u64 + 1)
                .read_to_end(&mut raw)
                .map_err(|e| e.to_string())?;
            total += raw.len();
            if raw.len() > MAX_FILE || total > MAX_TOTAL {
                return Err("Language catalog exceeds its byte budget".into());
            }
            let text = std::str::from_utf8(&raw).map_err(|e| e.to_string())?;
            let definition = Definition::from_json(text).map_err(|e| format!("Invalid installed language: {e:?}"))?;
            if path.file_stem().and_then(|s| s.to_str()) != Some(&definition.id) {
                return Err("Installed language filename does not match its ID".into());
            }
            definitions.push(definition);
        }
        Ok(definitions)
    }

    pub fn save(&self, definition: &Definition, cancel: &bareline_syntax::Cancellation) -> Result<(), String> {
        definition.validate().map_err(|e| format!("Invalid language: {e:?}"))?;
        let raw = definition.to_json().map_err(|e| format!("Invalid language: {e:?}"))?;
        if raw.len() > MAX_FILE {
            return Err("Installed language exceeds 128 KiB".into());
        }
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let _guard = self
            .file_system
            .guard_directory(&self.root)
            .map_err(|e| e.to_string())?;
        let path = self.root.join(format!("{}.json", definition.id));
        let existing = self.load()?;
        let count = existing.iter().filter(|item| item.id != definition.id).count();
        let bytes: usize = existing
            .iter()
            .filter(|item| item.id != definition.id)
            .map(|item| item.to_json().map_or(MAX_TOTAL + 1, |text| text.len()))
            .sum();
        if count >= MAX_ENTRIES || bytes + raw.len() > MAX_TOTAL {
            return Err("Language catalog budget exceeded; previous definitions retained".into());
        }
        self.file_system.validate_target(&path).map_err(|e| e.to_string())?;
        let stage = self.root.join(format!(
            ".language-{}-{}.tmp",
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
                .map_err(|e| e.to_string())?;
            file.write_all(raw.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|e| e.to_string())?;
            drop(file);
            if cancel.is_cancelled() {
                return Err("Language installation cancelled".into());
            }
            self.file_system
                .commit(&stage, &path, path.exists())
                .map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&stage);
        }
        result
    }
}
