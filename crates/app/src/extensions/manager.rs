// SPDX-License-Identifier: MPL-2.0
//! Durable manager state is an index, not package/signature authority.
use bareline_extensions_protocol::{
    Capability, ExtensionManifest, InstalledPackage, PackageError, VerifiedPackage, atomic_record,
};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::Path, sync::atomic::AtomicBool};
const MAX_RECORD: usize = 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledState {
    pub id: String,
    pub digest: String,
    pub version: String,
    pub approved: Vec<Capability>,
    pub enabled: bool,
    pub generation: u64,
    /// Resource-accounting hint only; manifests must be reverified before use.
    #[serde(default)]
    pub command_count: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagerIndex {
    pub schema_version: u32,
    pub entries: Vec<InstalledState>,
    pub runtime_digest: Option<String>,
    #[serde(default)]
    pub runtime_metadata_version: u64,
    pub generation: u64,
}
impl Default for ManagerIndex {
    fn default() -> Self {
        Self {
            schema_version: 1,
            entries: vec![],
            runtime_digest: None,
            runtime_metadata_version: 0,
            generation: 0,
        }
    }
}
impl ManagerIndex {
    fn advance(&mut self) -> Result<u64, String> {
        self.generation = self.generation.checked_add(1).ok_or("Extension generation exhausted")?;
        Ok(self.generation)
    }
    pub fn upsert(&mut self, package: &InstalledPackage, digest: String) -> Result<(), String> {
        self.upsert_manifest(&package.manifest, digest)
    }
    // Private to this module: callers cannot replace verified package authority
    // with a manifest or persisted index record.
    fn upsert_manifest(&mut self, manifest: &ExtensionManifest, digest: String) -> Result<(), String> {
        let count = self
            .entries
            .iter()
            .filter(|entry| entry.id != manifest.id)
            .try_fold(manifest.commands.len(), |sum, entry| {
                sum.checked_add(entry.command_count)
            })
            .ok_or("Installed command contribution limit (1024)")?;
        if manifest.commands.len() > 256 || count > 1024 {
            return Err("Installed command contribution limit (1024)".into());
        }
        let old = self.entries.iter().find(|entry| entry.id == manifest.id).cloned();
        if old.is_none() && self.entries.len() >= 64 {
            return Err("Installed extension limit (64)".into());
        }
        let generation = self.advance()?;
        let requested = &manifest.capabilities;
        let approved = old
            .as_ref()
            .map(|old| {
                old.approved
                    .iter()
                    .copied()
                    .filter(|cap| requested.contains(cap))
                    .collect()
            })
            .unwrap_or_default();
        let enabled = old
            .as_ref()
            .is_some_and(|old| old.enabled && requested.iter().all(|cap| old.approved.contains(cap)));
        self.entries.retain(|entry| entry.id != manifest.id);
        self.entries.push(InstalledState {
            id: manifest.id.clone(),
            digest,
            version: manifest.version.clone(),
            approved,
            enabled,
            generation,
            command_count: manifest.commands.len(),
        });
        Ok(())
    }
    pub fn set_permission(&mut self, id: &str, requested: &[Capability], approve: bool) -> Result<(), String> {
        let generation = self.advance()?;
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or("Extension no longer installed")?;
        entry.generation = generation;
        entry.enabled = approve;
        if approve {
            entry.approved = requested.to_vec();
        }
        Ok(())
    }
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        self.advance()?;
        self.entries.retain(|entry| entry.id != id);
        Ok(())
    }
    pub fn save(&self, root: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_RECORD {
            return Err("Manager index limit".into());
        }
        atomic_record(&root.join("manager-v1.json"), &bytes).map_err(|e| format!("Manager state: {e:?}"))
    }
    pub fn load(root: &Path) -> Result<Self, String> {
        let file = match fs::File::open(root.join("manager-v1.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.to_string()),
        };
        let mut bytes = Vec::new();
        file.take((MAX_RECORD + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_RECORD {
            return Err("Manager index limit".into());
        }
        let index: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if index.schema_version != 1 || index.entries.len() > 64 {
            return Err("Unsupported manager index".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut commands = 0usize;
        for entry in &index.entries {
            commands = commands
                .checked_add(entry.command_count)
                .ok_or("Manager command count overflow")?;
            if !bareline_extensions_protocol::valid_id(&entry.id)
                || !digest(&entry.digest)
                || !ids.insert(&entry.id)
                || entry.generation > index.generation
                || entry.approved.len() > 8
                || entry.command_count > 256
                || commands > 1024
            {
                return Err("Invalid installed state".into());
            }
        }
        if index.runtime_digest.as_deref().is_some_and(|value| !digest(value)) {
            return Err("Invalid runtime digest".into());
        }
        Ok(index)
    }
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
/// Installation is staged outside the published index; a failed index write does
/// not replace the old active record or silently enable an update.
pub fn install(
    root: &Path,
    package: &VerifiedPackage,
    index: &ManagerIndex,
    cancel: &AtomicBool,
) -> Result<(ManagerIndex, InstalledPackage), String> {
    let installed = match package.install(root, cancel) {
        Ok(value) => value,
        Err(PackageError::AlreadyInstalled) => package
            .restore(root, cancel)
            .map_err(|e| format!("Existing package verification: {e:?}"))?,
        Err(error) => return Err(format!("Installation: {error:?}")),
    };
    package.cache(root).map_err(|e| format!("Package receipt: {e:?}"))?;
    let mut next = index.clone();
    next.upsert(&installed, package.metadata().sha256.clone())?;
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Installation cancelled".into());
    }
    next.save(root)?;
    Ok((next, installed))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_or_forged_index_is_not_loaded() {
        let root = std::env::temp_dir().join(format!("bareline-manager-index-{}", std::process::id()));
        let _ = fs::create_dir(&root);
        fs::write(
            root.join("manager-v1.json"),
            br#"{"schema_version":99,"entries":[],"runtime_digest":null,"generation":0}"#,
        )
        .unwrap();
        assert!(ManagerIndex::load(&root).is_err());
        fs::remove_file(root.join("manager-v1.json")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    fn manifest(id: &str, caps: Vec<Capability>, count: usize) -> ExtensionManifest {
        ExtensionManifest {
            schema_version: 1,
            id: id.into(),
            version: "1.0.0".into(),
            publisher: "fixture".into(),
            minimum_protocol: 1,
            maximum_protocol: 1,
            entry_component: "extension.wasm".into(),
            commands: (0..count).map(|i| format!("{id}.command{i}")).collect(),
            background_commands: vec![],
            panels: vec![],
            capabilities: caps,
        }
    }
    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "bareline-manager-lifecycle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn update_retains_only_approved_subset_and_new_capability_disables() {
        let mut index = ManagerIndex::default();
        let mut m = manifest("fixture", vec![Capability::DocumentRead, Capability::DocumentEdit], 1);
        index.upsert_manifest(&m, "a".repeat(64)).unwrap();
        index
            .set_permission("fixture", &[Capability::DocumentRead], true)
            .unwrap();
        let generation = index.entries[0].generation;
        m.version = "1.0.1".into();
        index.upsert_manifest(&m, "b".repeat(64)).unwrap();
        assert_eq!(index.entries[0].approved, [Capability::DocumentRead]);
        assert!(!index.entries[0].enabled);
        assert!(index.entries[0].generation > generation);
        index.set_permission("fixture", &m.capabilities, true).unwrap();
        let generation = index.entries[0].generation;
        m.capabilities.push(Capability::Network);
        m.version = "2.0.0".into();
        index.upsert_manifest(&m, "c".repeat(64)).unwrap();
        assert!(!index.entries[0].enabled);
        assert!(!index.entries[0].approved.contains(&Capability::Network));
        assert!(index.entries[0].generation > generation);
        m.capabilities = vec![Capability::DocumentRead];
        index.upsert_manifest(&m, "d".repeat(64)).unwrap();
        assert_eq!(index.entries[0].approved, [Capability::DocumentRead]);
    }
    #[test]
    fn durable_disable_and_remove_retain_runtime_and_advance_generation() {
        let root = path();
        fs::create_dir(&root).unwrap();
        let mut index = ManagerIndex::default();
        index.runtime_digest = Some("f".repeat(64));
        index.runtime_metadata_version = 9;
        index
            .upsert_manifest(&manifest("fixture", vec![Capability::DocumentRead], 1), "a".repeat(64))
            .unwrap();
        index
            .set_permission("fixture", &[Capability::DocumentRead], true)
            .unwrap();
        index.save(&root).unwrap();
        let mut loaded = ManagerIndex::load(&root).unwrap();
        assert!(loaded.entries[0].enabled);
        assert_eq!(loaded.entries[0].command_count, 1);
        let generation = loaded.generation;
        loaded.set_permission("fixture", &[], false).unwrap();
        assert!(!loaded.entries[0].enabled);
        assert!(loaded.generation > generation);
        loaded.save(&root).unwrap();
        let mut loaded = ManagerIndex::load(&root).unwrap();
        assert!(!loaded.entries[0].enabled);
        let generation = loaded.generation;
        loaded.remove("fixture").unwrap();
        assert!(loaded.entries.is_empty());
        assert!(loaded.generation > generation);
        assert_eq!(loaded.runtime_digest, Some("f".repeat(64)));
        assert_eq!(loaded.runtime_metadata_version, 9);
        loaded.save(&root).unwrap();
        assert!(ManagerIndex::load(&root).unwrap().entries.is_empty());
        fs::remove_file(root.join("manager-v1.json")).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn contributions_are_bounded_before_index_mutation_and_on_reload() {
        let mut index = ManagerIndex::default();
        for i in 0..4 {
            index
                .upsert_manifest(&manifest(&format!("fixture{i}"), vec![], 256), "a".repeat(64))
                .unwrap();
        }
        let generation = index.generation;
        assert!(
            index
                .upsert_manifest(&manifest("overflow", vec![], 1), "b".repeat(64))
                .is_err()
        );
        assert_eq!(index.entries.len(), 4);
        assert_eq!(index.generation, generation);
        // Out-of-range persisted counts are rejected. An understated hint does
        // not replace the router's budget over reverified manifests.
        let root = path();
        fs::create_dir(&root).unwrap();
        index.entries[0].command_count = 257;
        index.save(&root).unwrap();
        assert!(ManagerIndex::load(&root).is_err());
        fs::remove_file(root.join("manager-v1.json")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
