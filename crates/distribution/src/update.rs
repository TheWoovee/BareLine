// SPDX-License-Identifier: MPL-2.0
//! Offline verification building blocks. These do not authorize execution: the Windows
//! adapter must additionally validate Authenticode publisher on the held file handle.
use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;

const MAX_MANIFEST_BYTES: usize = 64 * 1024;
#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    Size,
    Signature,
    Metadata,
    Policy,
    Rollback,
    Expired,
    Length,
    Hash,
    Io,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub metadata_version: u64,
    pub version: String,
    pub channel: String,
    pub artifact_type: String,
    pub platform: String,
    pub publisher: String,
    pub length: u64,
    pub sha256: String,
    pub minimum_protocol: u32,
    pub expires_unix: u64,
}
/// Trust configuration comes only from owner-pinned policy, never workspace metadata.
pub struct TrustPolicy<'a> {
    pub release_public_key: &'a str,
    pub channel: &'a str,
    pub artifact_type: &'a str,
    pub platform: &'a str,
    pub publisher: &'a str,
    pub protocol: u32,
    pub highest_metadata_version: u64,
    pub maximum_package_bytes: u64,
}
/// Capability proving minisign verification and metadata policy checks, not Authenticode.
#[derive(Debug)]
pub struct VerifiedManifest(Manifest);
impl VerifiedManifest {
    pub fn metadata(&self) -> &Manifest {
        &self.0
    }
    /// Streams exact package bytes with fixed memory. Caller holds a non-writable,
    /// non-delete-share file handle throughout hash, publisher verification and apply.
    pub fn verify_package(&self, reader: &mut impl Read) -> Result<(), VerifyError> {
        let mut hash = Sha256::new();
        let mut total = 0_u64;
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            let count = reader.read(&mut chunk).map_err(|_| VerifyError::Io)?;
            if count == 0 {
                break;
            }
            total = total.checked_add(count as u64).ok_or(VerifyError::Length)?;
            if total > self.0.length {
                return Err(VerifyError::Length);
            }
            hash.update(&chunk[..count]);
        }
        if total != self.0.length {
            return Err(VerifyError::Length);
        }
        if format!("{:x}", hash.finalize()) != self.0.sha256 {
            return Err(VerifyError::Hash);
        }
        Ok(())
    }
}
/// Verify the original signed bytes before parsing any fields. `now_unix` must
/// come from a trusted clock; unavailable freshness must prevent installation.
pub fn verify_manifest(
    bytes: &[u8],
    signature: &str,
    policy: &TrustPolicy<'_>,
    now_unix: u64,
) -> Result<VerifiedManifest, VerifyError> {
    verify_minisign(bytes, signature, policy.release_public_key)?;
    let metadata: Manifest = serde_json::from_slice(bytes).map_err(|_| VerifyError::Metadata)?;
    validate_metadata(&metadata, policy, now_unix)?;
    Ok(VerifiedManifest(metadata))
}
pub(super) fn verify_minisign(bytes: &[u8], signature: &str, key: &str) -> Result<(), VerifyError> {
    if bytes.len() > MAX_MANIFEST_BYTES || signature.len() > 8192 {
        return Err(VerifyError::Size);
    }
    let key = PublicKey::from_base64(key).map_err(|_| VerifyError::Signature)?;
    let signature = Signature::decode(signature).map_err(|_| VerifyError::Signature)?;
    key.verify(bytes, &signature, false)
        .map_err(|_| VerifyError::Signature)
}
fn validate_metadata(m: &Manifest, p: &TrustPolicy<'_>, now: u64) -> Result<(), VerifyError> {
    if m.schema_version != 1
        || m.channel != p.channel
        || m.artifact_type != p.artifact_type
        || m.platform != p.platform
        || m.publisher != p.publisher
        || m.minimum_protocol > p.protocol
        || m.length == 0
        || m.length > p.maximum_package_bytes
        || m.version.is_empty()
        || m.sha256.len() != 64
        || !m
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(VerifyError::Policy);
    }
    if m.metadata_version < p.highest_metadata_version {
        return Err(VerifyError::Rollback);
    }
    if now == 0 || m.expires_unix <= now {
        return Err(VerifyError::Expired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const SIG: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";
    fn policy() -> TrustPolicy<'static> {
        TrustPolicy {
            release_public_key: KEY,
            channel: "stable",
            artifact_type: "bareline-x64",
            platform: "windows-x64",
            publisher: "test-publisher",
            protocol: 1,
            highest_metadata_version: 3,
            maximum_package_bytes: 4096,
        }
    }
    fn metadata() -> Manifest {
        Manifest {
            schema_version: 1,
            metadata_version: 3,
            version: "1.0.0".into(),
            channel: "stable".into(),
            artifact_type: "bareline-x64".into(),
            platform: "windows-x64".into(),
            publisher: "test-publisher".into(),
            length: 4,
            sha256: format!("{:x}", Sha256::digest(b"test")),
            minimum_protocol: 1,
            expires_unix: 200,
        }
    }
    #[test]
    fn established_minisign_vector_and_tampering() {
        assert_eq!(verify_minisign(b"test", SIG, KEY), Ok(()));
        assert_eq!(
            verify_minisign(b"tent", SIG, KEY),
            Err(VerifyError::Signature)
        );
        assert_eq!(
            verify_minisign(b"test", &SIG.replace("RUQf", "RUQg"), KEY),
            Err(VerifyError::Signature)
        );
        assert_eq!(
            verify_manifest(b"test", SIG, &policy(), 100).unwrap_err(),
            VerifyError::Metadata
        );
        assert_eq!(
            verify_manifest(b"{}", SIG, &policy(), 100).unwrap_err(),
            VerifyError::Signature
        );
    }
    #[test]
    fn freshness_rollback_identity_and_hash_are_enforced() {
        assert_eq!(validate_metadata(&metadata(), &policy(), 100), Ok(()));
        assert_eq!(
            validate_metadata(&metadata(), &policy(), 200),
            Err(VerifyError::Expired)
        );
        assert_eq!(
            validate_metadata(&metadata(), &policy(), 0),
            Err(VerifyError::Expired)
        );
        let mut m = metadata();
        m.metadata_version = 2;
        assert_eq!(
            validate_metadata(&m, &policy(), 100),
            Err(VerifyError::Rollback)
        );
        for field in ["channel", "publisher", "platform", "artifact"] {
            let mut m = metadata();
            match field {
                "channel" => m.channel = "preview".into(),
                "publisher" => m.publisher = "other".into(),
                "platform" => m.platform = "linux-x64".into(),
                _ => m.artifact_type = "host".into(),
            }
            assert_eq!(
                validate_metadata(&m, &policy(), 100),
                Err(VerifyError::Policy)
            );
        }
        let verified = VerifiedManifest(metadata());
        assert_eq!(verified.verify_package(&mut &b"test"[..]), Ok(()));
        assert_eq!(
            verified.verify_package(&mut &b"tent"[..]),
            Err(VerifyError::Hash)
        );
        assert_eq!(
            verified.verify_package(&mut &b"tes"[..]),
            Err(VerifyError::Length)
        );
        assert_eq!(
            verified.verify_package(&mut &b"tests"[..]),
            Err(VerifyError::Length)
        );
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("unavailable"))
            }
        }
        assert_eq!(verified.verify_package(&mut Broken), Err(VerifyError::Io));
    }
}
