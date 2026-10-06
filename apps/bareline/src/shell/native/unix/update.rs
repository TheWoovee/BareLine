// SPDX-License-Identifier: MPL-2.0
//! In-app updates are a Windows feature; these systems update through their
//! package managers (ADR-C tier 3). Every entry point reports `Unsupported`,
//! starting with the installation check that each update flow runs first.
use bareline_distribution::update::{Manifest, PublisherPin, TrustPolicy, VerifiedManifest};
use bareline_platform::{Capability, Unsupported};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

fn unsupported() -> io::Error {
    super::error::unsupported_io(Capability::Update)
}
pub const UPDATE_LAUNCH_ATTEMPTS: u32 = 3;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityFreshness {
    Required,
    Installed,
}
#[allow(
    dead_code,
    reason = "in-app updates are unsupported here, so these values are never produced or read"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchDecision {
    NotPending,
    Counted(u32),
    Recover,
    RecoveryFailed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperAction {
    Apply,
    Acknowledge,
    Recover,
    AutoRecover,
}
#[allow(
    dead_code,
    reason = "in-app updates are unsupported here, so these values are never produced or read"
)]
#[derive(Clone, Copy, Debug)]
pub struct OfflineRootPolicy<'a> {
    pub public_key: &'a str,
    pub minimum_version: u64,
}
#[allow(
    dead_code,
    reason = "in-app updates are unsupported here, so these values are never produced or read"
)]
pub struct ResolvedReleaseAuthority {
    pub release_public_key: String,
    pub signer: PublisherPin,
    pub update_helper_sha256: Option<String>,
    pub minimum_metadata_version: u64,
    pub minimum_runtime_metadata_version: u64,
    pub minimum_catalog_metadata_version: u64,
    pub catalog_public_key: Option<String>,
}
pub struct PreparedUpdate {
    pub manifest: VerifiedManifest,
    pub file: File,
    pub directory: PathBuf,
}
#[allow(
    dead_code,
    reason = "in-app updates are unsupported here, so these values are never produced or read"
)]
#[derive(Clone, Debug)]
pub struct InstalledRuntime {
    pub executable: PathBuf,
    pub executable_sha256: [u8; 32],
    pub signer: PublisherPin,
    pub version: String,
    pub metadata_version: u64,
}
pub fn validate_install_root(_root: &Path) -> io::Result<()> {
    Err(unsupported())
}
pub fn update_state_root(_root: &Path) -> io::Result<PathBuf> {
    Err(unsupported())
}
#[allow(clippy::too_many_arguments, reason = "mirrors the Windows adapter's signature")]
pub fn resolve_release_authority(
    _root: &Path,
    _state: &Path,
    _embedded_key: &str,
    _embedded_signer: &PublisherPin,
    _embedded_floor: u64,
    _offline_policy: Option<OfflineRootPolicy<'_>>,
    _freshness: AuthorityFreshness,
    _now: u64,
) -> io::Result<ResolvedReleaseAuthority> {
    Err(unsupported())
}
pub fn record_update_launch(_root: &Path, _state: &Path, _limit: u32) -> io::Result<LaunchDecision> {
    Err(unsupported())
}
pub fn launch_update_helper(
    _root: &Path,
    _authority: &ResolvedReleaseAuthority,
    _action: HelperAction,
) -> io::Result<()> {
    Err(unsupported())
}
pub fn core_metadata_floor(_root: &Path, _state: &Path, _authority_floor: u64) -> io::Result<u64> {
    Err(unsupported())
}
#[allow(clippy::too_many_arguments, reason = "mirrors the Windows adapter's signature")]
pub fn fetch_verified_update(
    _host: &str,
    _manifest_path: &str,
    _signature_path: &str,
    _artifact_path: &str,
    _policy: &TrustPolicy<'_>,
    _now_unix: u64,
    _signer: &PublisherPin,
    _stage_parent: &Path,
    _cancel: &AtomicBool,
) -> Result<PreparedUpdate, Unsupported> {
    Err(Unsupported {
        capability: Capability::Update,
    })
}
pub fn verify_delivered_trust(
    _directory: &Path,
    _root: &Path,
    _state: &Path,
    _manifest: &Manifest,
    _policy: OfflineRootPolicy<'_>,
    _core: &File,
    _now: u64,
) -> io::Result<Option<()>> {
    Err(unsupported())
}
pub fn discard_prepared_update(_prepared: PreparedUpdate) {}
pub fn transfer_update(_prepared: PreparedUpdate, _root: &Path, _state: &Path) -> io::Result<()> {
    Err(unsupported())
}
pub fn recovery_source(_root: &Path, _state: &Path) -> io::Result<Option<()>> {
    Err(unsupported())
}
pub fn discard_pending_update(_root: &Path, _state: &Path) -> io::Result<()> {
    Err(unsupported())
}
pub fn restore_verified_runtime(
    _extensions_root: &Path,
    _hash: &str,
    _trust: &TrustPolicy<'_>,
    _now: u64,
    _publisher: &PublisherPin,
) -> io::Result<InstalledRuntime> {
    Err(unsupported())
}
#[allow(clippy::too_many_arguments, reason = "mirrors the Windows adapter's signature")]
pub fn install_verified_runtime(
    _executable_path: &Path,
    _metadata_bytes: &[u8],
    _signature_text: &str,
    _trust: &TrustPolicy<'_>,
    _now: u64,
    _publisher: &PublisherPin,
    _extensions_root: &Path,
    _cancel: &AtomicBool,
) -> io::Result<InstalledRuntime> {
    Err(unsupported())
}
pub fn remove_verified_runtime(_runtime: &InstalledRuntime) -> io::Result<()> {
    Err(unsupported())
}
