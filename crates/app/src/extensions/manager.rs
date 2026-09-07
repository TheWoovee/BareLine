// SPDX-License-Identifier: MPL-2.0
//! Durable manager state is an index, not package/signature authority.
use bareline_extensions_protocol::{
    Capability, InstalledPackage, PackageError, VerifiedPackage, atomic_record,
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
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("Extension generation exhausted")?;
        Ok(self.generation)
    }
    pub fn upsert(&mut self, package: &InstalledPackage, digest: String) -> Result<(), String> {
        let old = self
            .entries
            .iter()
            .find(|entry| entry.id == package.id)
            .cloned();
        if old.is_none() && self.entries.len() >= 64 {
            return Err("Installed extension limit (64)".into());
        }
        let generation = self.advance()?;
        let requested = &package.manifest.capabilities;
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
        let enabled = old.as_ref().is_some_and(|old| {
            old.enabled && requested.iter().all(|cap| old.approved.contains(cap))
        });
        self.entries.retain(|entry| entry.id != package.id);
        self.entries.push(InstalledState {
            id: package.id.clone(),
            digest,
            version: package.version.clone(),
            approved,
            enabled,
            generation,
        });
        Ok(())
    }
    pub fn set_permission(
        &mut self,
        id: &str,
        requested: &[Capability],
        approve: bool,
    ) -> Result<(), String> {
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
        atomic_record(&root.join("manager-v1.json"), &bytes)
            .map_err(|e| format!("Manager state: {e:?}"))
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
        for entry in &index.entries {
            if !bareline_extensions_protocol::valid_id(&entry.id)
                || !digest(&entry.digest)
                || !ids.insert(&entry.id)
                || entry.generation > index.generation
                || entry.approved.len() > 8
            {
                return Err("Invalid installed state".into());
            }
        }
        if index
            .runtime_digest
            .as_deref()
            .is_some_and(|value| !digest(value))
        {
            return Err("Invalid runtime digest".into());
        }
        Ok(index)
    }
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
    package
        .cache(root)
        .map_err(|e| format!("Package receipt: {e:?}"))?;
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
        let root =
            std::env::temp_dir().join(format!("bareline-manager-index-{}", std::process::id()));
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
