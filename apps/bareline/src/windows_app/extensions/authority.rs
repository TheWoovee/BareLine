// SPDX-License-Identifier: MPL-2.0
//! Resolve current signed owner authority on extension workers, before use.
use super::*;
use bareline_platform_windows::update::AuthorityFreshness;

pub(super) fn now() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|error| error.to_string())
}

impl OwnerTrust {
    /// Resolve the current signed authority. Catalog opening and package or runtime
    /// installation accept new metadata and need a fresh authority; restoring and
    /// invoking already-verified installed extensions uses installed state, which an
    /// expired authority never disables (SEC-02).
    pub(super) fn current(&self, freshness: AuthorityFreshness) -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        self.at_installation(
            executable.parent().ok_or("Missing installation directory")?,
            freshness,
            now()?,
        )
    }

    pub(super) fn at_installation(
        &self,
        root: &std::path::Path,
        freshness: AuthorityFreshness,
        now: u64,
    ) -> Result<Self, String> {
        // Ledgers and the lock are per-user state; a per-machine installation stays
        // read-only (SEC-04).
        let state = bareline_platform_windows::update::update_state_root(root)
            .map_err(|error| format!("Extension authority state: {error}"))?;
        let authority = bareline_platform_windows::update::resolve_release_authority(
            root,
            &state,
            &self.release_public_key,
            &self.signer,
            self.metadata_floor,
            Some(bareline_platform_windows::update::OfflineRootPolicy {
                public_key: env!("BARELINE_OFFLINE_ROOT_PUBLIC_KEY"),
                minimum_version: env!("BARELINE_ROOT_VERSION_FLOOR")
                    .parse()
                    .map_err(|_| "Invalid offline root floor")?,
            }),
            freshness,
            now,
        )
        .map_err(|error| format!("Extension authority: {error}"))?;
        Ok(Self {
            catalog_public_key: authority.catalog_public_key.ok_or("Missing catalog authority")?,
            release_public_key: authority.release_public_key,
            signer: authority.signer,
            metadata_floor: authority.minimum_metadata_version,
            runtime_floor: authority.minimum_runtime_metadata_version,
            catalog_floor: authority.minimum_catalog_metadata_version,
            publisher: self.publisher.clone(),
            channel: self.channel.clone(),
        })
    }

    /// Catalog policy with the catalogs' own floor, never the core floor (SEC-03).
    pub(super) fn catalog_policy(
        &self,
        artifact_type: &'static str,
        highest: u64,
        now: u64,
    ) -> bareline_extensions_protocol::CatalogPolicy<'_> {
        bareline_extensions_protocol::CatalogPolicy {
            public_key: &self.catalog_public_key,
            publisher: &self.publisher,
            channel: &self.channel,
            platform: "windows-x64",
            artifact_type,
            highest_metadata_version: highest.max(self.catalog_floor),
            now_unix: now,
        }
    }
}

/// Retained identity of the queued invocation. Reverify receipts on the worker;
/// key rotation/revocation after startup must not leave a cached launch grant.
pub(super) struct InvocationCheck {
    trust: OwnerTrust,
    root: PathBuf,
    runtime_digest: String,
    runtime_path: PathBuf,
    package_digest: String,
    component_path: PathBuf,
    component_digest: [u8; 32],
    extension_id: String,
    command: String,
}

impl InvocationCheck {
    pub(super) fn new(runtime: &ExtensionsRuntime, job: &InvocationJob) -> Result<Option<Self>, String> {
        let Some(trust) = runtime.trust.clone() else {
            return Ok(None);
        };
        let root = runtime.inventory_root.clone().ok_or("Extension storage unavailable")?;
        let package_digest = job
            .component
            .parent()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .ok_or("Invalid installed component path")?
            .to_owned();
        Ok(Some(Self {
            trust,
            root,
            runtime_digest: job
                .runtime
                .executable_sha256
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            runtime_path: job.runtime.executable.clone(),
            package_digest,
            component_path: job.component.clone(),
            component_digest: job.component_sha256,
            extension_id: job.invocation.extension_id.clone(),
            command: job.invocation.command.clone(),
        }))
    }

    pub(super) fn verify(self, cancel: &AtomicBool) -> Result<bareline_distribution::update::PublisherPin, String> {
        let trust = self.trust.current(AuthorityFreshness::Installed)?;
        let now = now()?;
        let index = ManagerIndex::load(&self.root)?;
        let entry = index
            .entries
            .iter()
            .find(|entry| entry.id == self.extension_id && entry.digest == self.package_digest && entry.enabled)
            .ok_or("Extension was removed or disabled before invocation")?;
        let package = bareline_extensions_protocol::restore_cached(
            &self.root,
            &self.package_digest,
            &trust.catalog_policy("extension", 0, now),
            cancel,
        )
        .map_err(|error| format!("The extension could not be verified: {error}."))?;
        if package.id != self.extension_id
            || package.component_sha256 != self.component_digest
            || package.directory().join(&package.manifest.entry_component) != self.component_path
            || !package.manifest.commands.contains(&self.command)
            || package
                .manifest
                .capabilities
                .iter()
                .any(|capability| !entry.approved.contains(capability))
            || index.runtime_digest.as_deref() != Some(self.runtime_digest.as_str())
        {
            return Err("Queued extension identity or permissions changed".into());
        }
        let runtime = bareline_platform_windows::update::restore_verified_runtime(
            &self.root,
            &self.runtime_digest,
            &trust.runtime_policy(index.runtime_metadata_version),
            now,
            &trust.signer,
        )
        .map_err(|error| format!("Runtime authority verification: {error}"))?;
        if runtime.executable != self.runtime_path {
            return Err("Queued runtime identity changed".into());
        }
        Ok(trust.signer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_and_catalog_floors_are_independent_of_the_core_floor() {
        // SEC-03: a core release raising the core floor never invalidates the installed
        // runtime or extensions, whose floors come from their own ledgers.
        let trust = OwnerTrust {
            metadata_floor: 9,
            runtime_floor: 0,
            catalog_floor: 2,
            catalog_public_key: "catalog".into(),
            release_public_key: "release".into(),
            publisher: "Bareline".into(),
            channel: "stable".into(),
            signer: bareline_distribution::update::PublisherPin::parse("Bareline", "Fixture CA").unwrap(),
        };
        assert_eq!(trust.runtime_policy(3).highest_metadata_version, 3);
        assert_eq!(trust.catalog_policy("extension", 0, 100).highest_metadata_version, 2);
        assert_eq!(trust.catalog_policy("runtime", 5, 100).highest_metadata_version, 5);
    }
}
