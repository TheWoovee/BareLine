// SPDX-License-Identifier: MPL-2.0
//! Offline-root-authorized release-key rotation and explicit revocation.
//! Deployment supplies the root key and monotonic floor; no package chooses its root.
use serde::{Deserialize, Serialize};
fn key_material(key: &str) -> Result<Vec<u8>, super::update::VerifyError> {
    use base64::Engine;
    minisign_verify::PublicKey::from_base64(key).map_err(|_| super::update::VerifyError::Policy)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|_| super::update::VerifyError::Policy)?;
    Ok(bytes[10..].to_vec())
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseAuthority {
    pub schema_version: u32,
    pub root_version: u64,
    /// The authority's own lifetime, separate from (and much longer than) the expiry
    /// of each release's metadata; a newer authority arrives with core updates (SEC-02).
    pub expires_unix: u64,
    /// Rollback floor for core executable metadata only (SEC-03).
    pub minimum_metadata_version: u64,
    /// Separate floors for the extension-host runtime and the extension catalogs, so a
    /// core release never invalidates installed runtimes or extensions (SEC-03). Absent
    /// means no authority floor; each artifact type still keeps its own ledger.
    #[serde(default)]
    pub minimum_runtime_metadata_version: u64,
    #[serde(default)]
    pub minimum_catalog_metadata_version: u64,
    pub release_public_key: String,
    pub catalog_public_key: String,
    /// Authenticode pin (SEC-08): signer subject and accepted issuing CAs, never a
    /// leaf certificate hash. See [`super::update::PublisherPin`].
    pub authenticode_subject: String,
    pub authenticode_issuers: Vec<String>,
    /// SHA-256 of the exact signed update helper this release installs. Launching the
    /// helper pairs Authenticode with this hash, never Authenticode alone (SEC-08).
    pub update_helper_sha256: String,
    pub revoked_release_keys: Vec<String>,
    /// Revoked Authenticode subjects; the pinned subject can never be one of them.
    pub revoked_publishers: Vec<String>,
}
impl ReleaseAuthority {
    pub fn publisher_pin(&self) -> super::update::PublisherPin {
        super::update::PublisherPin {
            subject: self.authenticode_subject.clone(),
            issuers: self.authenticode_issuers.clone(),
        }
    }
}
/// Verify an authority for accepting new metadata, packages or executables: it must
/// be unexpired at `now`.
pub fn verify_authority(
    bytes: &[u8],
    signature: &str,
    offline_root_key: &str,
    highest_root_version: u64,
    now: u64,
) -> Result<ReleaseAuthority, super::update::VerifyError> {
    authority(bytes, signature, offline_root_key, highest_root_version, Some(now))
}
/// Verify the installed authority for using already-verified installed state
/// (extension restore and invocation, health acknowledgement, recovery). Signature,
/// root floor, pins and revocations apply; expiry does not disable that state (SEC-02).
pub fn verify_installed_authority(
    bytes: &[u8],
    signature: &str,
    offline_root_key: &str,
    highest_root_version: u64,
) -> Result<ReleaseAuthority, super::update::VerifyError> {
    authority(bytes, signature, offline_root_key, highest_root_version, None)
}
fn authority(
    bytes: &[u8],
    signature: &str,
    offline_root_key: &str,
    highest_root_version: u64,
    now: Option<u64>,
) -> Result<ReleaseAuthority, super::update::VerifyError> {
    use super::update::{VerifyError, verify_minisign};
    if bytes.len() > 16384 {
        return Err(VerifyError::Size);
    }
    verify_minisign(bytes, signature, offline_root_key)?;
    let root: ReleaseAuthority = serde_json::from_slice(bytes).map_err(|_| VerifyError::Metadata)?;
    if root.schema_version != 1 || root.root_version == 0 || root.root_version < highest_root_version {
        return Err(VerifyError::Rollback);
    }
    if let Some(now) = now
        && (now == 0 || root.expires_unix <= now)
    {
        return Err(VerifyError::Expired);
    }
    if root.revoked_release_keys.len() > 32
        || root.revoked_publishers.len() > 32
        || root.release_public_key.len() > 128
        || root.publisher_pin().validate().is_err()
        || root.update_helper_sha256.len() != 64
        || !root
            .update_helper_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || root.revoked_release_keys.contains(&root.release_public_key)
        || root.revoked_release_keys.contains(&root.catalog_public_key)
        || root.catalog_public_key == root.release_public_key
        || root.catalog_public_key == offline_root_key
        || root.release_public_key == offline_root_key
        || root
            .revoked_publishers
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&root.authenticode_subject))
    {
        return Err(VerifyError::Policy);
    }
    let release = key_material(&root.release_public_key)?;
    let catalog = key_material(&root.catalog_public_key)?;
    let offline = key_material(offline_root_key)?;
    if release == catalog || release == offline || catalog == offline {
        return Err(VerifyError::Policy);
    }
    for revoked in &root.revoked_release_keys {
        let revoked = key_material(revoked)?;
        if revoked == release || revoked == catalog {
            return Err(VerifyError::Policy);
        }
    }
    Ok(root)
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootTransition {
    pub schema_version: u32,
    pub version: u64,
    pub old_root_key: String,
    pub new_root_key: String,
    pub expires_unix: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedTransition {
    payload: String,
    old_signature: String,
    new_signature: String,
}
/// Each exact transition payload must be authorized by both its prior and successor
/// root. The chain is persisted verbatim and reevaluated from the compiled root.
pub fn verify_root_chain(
    bytes: &[u8],
    compiled_root: &str,
    now: u64,
) -> Result<(String, u64, Vec<String>), super::update::VerifyError> {
    root_chain(bytes, compiled_root, Some(now))
}
/// The installed chain for already-verified installed state; see
/// [`verify_installed_authority`]. Transition expiry is not applied.
pub fn verify_installed_root_chain(
    bytes: &[u8],
    compiled_root: &str,
) -> Result<(String, u64, Vec<String>), super::update::VerifyError> {
    root_chain(bytes, compiled_root, None)
}
fn root_chain(
    bytes: &[u8],
    compiled_root: &str,
    now: Option<u64>,
) -> Result<(String, u64, Vec<String>), super::update::VerifyError> {
    use super::update::{VerifyError, verify_minisign};
    if bytes.len() > 262144 {
        return Err(VerifyError::Size);
    }
    let chain: Vec<SignedTransition> = serde_json::from_slice(bytes).map_err(|_| VerifyError::Metadata)?;
    if chain.is_empty() || chain.len() > 16 {
        return Err(VerifyError::Policy);
    }
    let mut key = compiled_root.to_owned();
    let mut version = 0;
    let mut lineage = vec![key.clone()];
    for signed in chain {
        verify_minisign(signed.payload.as_bytes(), &signed.old_signature, &key)?;
        let next: RootTransition = serde_json::from_str(&signed.payload).map_err(|_| VerifyError::Metadata)?;
        if next.schema_version != 1
            || next.old_root_key != key
            || next.new_root_key == key
            || next.version <= version
            || next.new_root_key.len() > 128
        {
            return Err(VerifyError::Policy);
        }
        if let Some(now) = now
            && (now == 0 || next.expires_unix <= now)
        {
            return Err(VerifyError::Expired);
        }
        verify_minisign(signed.payload.as_bytes(), &signed.new_signature, &next.new_root_key)?;
        let next_material = key_material(&next.new_root_key)?;
        for prior in &lineage {
            if key_material(prior)? == next_material {
                return Err(VerifyError::Policy);
            }
        }
        key = next.new_root_key;
        version = next.version;
        lineage.push(key.clone());
    }
    Ok((key, version, lineage))
}
