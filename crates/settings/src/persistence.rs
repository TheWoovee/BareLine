// SPDX-License-Identifier: MPL-2.0
//! Bounded synchronous config I/O. Call from a background worker after startup.
use crate::{MAX_CONFIG_BYTES, Scope, SettingValue, SettingsDocument};
use bareline_platform::LocalFileSystem;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
pub fn read_config(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Configuration exceeds 1 MiB",
        ));
    }
    Ok(bytes)
}
/// Stages and syncs the complete replacement before invoking the native atomic commit.
/// No delete-target/rename fallback is allowed. Failure leaves the previous target intact.
pub fn atomic_write_config(path: &Path, bytes: &[u8], platform: &dyn LocalFileSystem) -> io::Result<()> {
    write_config(path, bytes, platform, false)
}
/// Publish a default configuration only when absent; a concurrent creator is preserved.
pub fn atomic_create_config(path: &Path, bytes: &[u8], platform: &dyn LocalFileSystem) -> io::Result<()> {
    platform.validate_target(path)?;
    if path.try_exists()? {
        return Ok(());
    }
    write_config(path, bytes, platform, true)
}
fn write_config(path: &Path, bytes: &[u8], platform: &dyn LocalFileSystem, create_only: bool) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Configuration exceeds 1 MiB",
        ));
    }
    platform.validate_target(path)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staged = parent.join(format!(
        ".bareline-config-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&staged)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        platform.commit(&staged, path, !create_only && path.try_exists()?)
    })();
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}
impl SettingsDocument {
    pub fn load(path: &Path, scope: Scope) -> io::Result<Self> {
        Self::parse(&read_config(path)?, scope).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    pub fn save(&self, path: &Path, platform: &dyn LocalFileSystem) -> io::Result<()> {
        if self.scope == Scope::Session {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Session overrides cannot be persisted as settings",
            ));
        }
        atomic_write_config(path, self.to_toml().as_bytes(), platform)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveStatus {
    Saved,
    Pending,
    Failed(String),
}
impl SaveStatus {
    pub fn label(&self) -> &str {
        match self {
            Self::Saved => "All changes saved",
            Self::Pending => "Changes not saved",
            Self::Failed(_) => "Changes not saved",
        }
    }
}
/// Holds persisted and opening-session snapshots independently of live preview.
/// Autosave acknowledgements advance `saved`, while Revert always uses `opening`.
#[derive(Clone, Debug)]
pub struct SettingsEditor {
    pub document: SettingsDocument,
    saved: SettingsDocument,
    opening: Option<SettingsDocument>,
    generation: u64,
    pub status: SaveStatus,
}
impl SettingsEditor {
    /// A background save may finish after a newer edit; never overwrite that live edit.
    pub fn acknowledge_saved(&mut self, generation: u64, snapshot: SettingsDocument) {
        let current = self.generation == generation && self.document.to_toml() == snapshot.to_toml();
        self.saved = snapshot;
        self.status = if current {
            SaveStatus::Saved
        } else {
            SaveStatus::Pending
        };
    }
    pub fn new(document: SettingsDocument) -> Self {
        Self {
            saved: document.clone(),
            document,
            opening: None,
            generation: 0,
            status: SaveStatus::Saved,
        }
    }
    pub fn begin_session(&mut self) {
        self.opening = Some(self.document.clone());
    }
    pub fn end_session(&mut self) {
        self.opening = None;
    }
    pub fn changed_from_opening(&self) -> bool {
        self.opening
            .as_ref()
            .is_some_and(|opening| opening.to_toml() != self.document.to_toml())
    }
    pub fn changed_from_saved(&self) -> bool {
        self.saved.to_toml() != self.document.to_toml()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Apply an explicit external-reload decision without changing the opening baseline.
    pub fn replace_from_disk(&mut self, document: SettingsDocument) {
        self.generation = self.generation.wrapping_add(1);
        self.saved = document.clone();
        self.document = document;
        self.status = SaveStatus::Saved;
    }
    pub fn set(&mut self, key: &str, value: SettingValue) -> Result<(), String> {
        self.document.set(key, value)?;
        self.generation = self.generation.wrapping_add(1);
        self.status = SaveStatus::Pending;
        Ok(())
    }
    pub fn reset_section(&mut self, category: &str) -> Vec<&'static str> {
        let keys = self.document.reset_section(category);
        if !keys.is_empty() {
            self.generation = self.generation.wrapping_add(1);
            self.status = SaveStatus::Pending;
        }
        keys
    }
    pub fn revert(&mut self) -> bool {
        let Some(opening) = self.opening.as_ref() else {
            return false;
        };
        if opening.to_toml() == self.document.to_toml() {
            return false;
        }
        self.document = opening.clone();
        self.generation = self.generation.wrapping_add(1);
        self.status = SaveStatus::Pending;
        true
    }
    pub fn save(&mut self, path: &Path, platform: &dyn LocalFileSystem) -> io::Result<()> {
        match self.document.save(path, platform) {
            Ok(()) => {
                self.acknowledge_saved(self.generation, self.document.clone());
                Ok(())
            }
            Err(error) => {
                self.status = SaveStatus::Failed(error.to_string());
                Err(error)
            }
        }
    }
}
