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
/// Plain-language reason shown to the user (UI-03); `Debug` stays for diagnostics.
impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Size => "the signed update information is larger than allowed",
            Self::Signature => "the signature is not valid; nothing was installed",
            Self::Metadata => "the signed update information could not be read",
            Self::Policy => "it does not match what this build trusts; nothing was installed",
            Self::Rollback => "it is older than the version already trusted; nothing was installed",
            Self::Expired => "the signed update information has expired; try again later",
            Self::Length => "the downloaded file has the wrong size; nothing was installed",
            Self::Hash => "the downloaded file does not match its signed checksum; nothing was installed",
            Self::Io => "the update files could not be read",
        })
    }
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
    /// Trust state delivered with a core update (SEC-02): the SHA-256 of the exact
    /// root-signed release authority and, when roots rotate, of the root transition
    /// chain. The authority pins the update helper it ships with. The release key of
    /// the current authority signs these digests; the root still verifies the authority.
    #[serde(default)]
    pub authority_sha256: Option<String>,
    #[serde(default)]
    pub root_transitions_sha256: Option<String>,
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
/// Core executable manifest type written by the release pipeline.
pub const CORE_ARTIFACT_TYPE: &str = "bareline-executable-x64";
/// The one core-update policy used by the app, the update helper and release
/// verification (SEC-01). `publisher` is the release configuration's `trust.publisher`,
/// the identity the release pipeline writes into every signed manifest. It is never a
/// certificate property; Authenticode is checked separately against a [`PublisherPin`].
pub fn core_update_policy<'a>(
    release_public_key: &'a str,
    publisher: &'a str,
    channel: &'a str,
    highest_metadata_version: u64,
) -> TrustPolicy<'a> {
    TrustPolicy {
        release_public_key,
        channel,
        artifact_type: CORE_ARTIFACT_TYPE,
        platform: "windows-x64",
        publisher,
        protocol: 1,
        highest_metadata_version,
        maximum_package_bytes: 256 * 1024 * 1024,
    }
}
/// Owner Authenticode pin (SEC-08): the signer certificate's subject and the accepted
/// issuing CAs (a rotation list), compared as Windows simple display names; the
/// platform adapter also requires the code-signing EKU. Leaf certificate hashes are
/// never pinned, so renewals and short-lived leaves keep working. A pin never
/// authorizes bytes alone: callers always pair it with the signed SHA-256 of the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublisherPin {
    pub subject: String,
    pub issuers: Vec<String>,
}
impl PublisherPin {
    /// Parse the compiled form: a subject plus a `|`-separated issuer list.
    pub fn parse(subject: &str, issuers: &str) -> Result<Self, VerifyError> {
        let pin = Self {
            subject: subject.to_owned(),
            issuers: issuers.split('|').map(str::to_owned).collect(),
        };
        pin.validate()?;
        Ok(pin)
    }
    pub fn validate(&self) -> Result<(), VerifyError> {
        if !certificate_name(&self.subject)
            || self.issuers.is_empty()
            || self.issuers.len() > 8
            || !self.issuers.iter().all(|issuer| certificate_name(issuer))
            || (1..self.issuers.len()).any(|index| self.issuers[..index].contains(&self.issuers[index]))
        {
            return Err(VerifyError::Policy);
        }
        Ok(())
    }
    /// Exact, case-sensitive comparison with the verified signer's display names.
    pub fn accepts(&self, subject: &str, issuer: &str) -> bool {
        self.subject == subject && self.issuers.iter().any(|accepted| accepted == issuer)
    }
}
/// Mirrors `scripts/release_config.py`: ASCII names that Windows and PowerShell report
/// identically, without the compiled list separator.
fn certificate_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (3..=128).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && (bytes[bytes.len() - 1].is_ascii_alphanumeric() || matches!(bytes[bytes.len() - 1], b'.' | b')'))
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b" .,&()'_-".contains(byte))
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
    key.verify(bytes, &signature, false).map_err(|_| VerifyError::Signature)
}
/// A lowercase hexadecimal SHA-256 digest.
pub fn sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
        || !sha256_hex(&m.sha256)
        || !m.authority_sha256.as_deref().is_none_or(sha256_hex)
        || !m.root_transitions_sha256.as_deref().is_none_or(sha256_hex)
        || (m.root_transitions_sha256.is_some() && m.authority_sha256.is_none())
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
            authority_sha256: None,
            root_transitions_sha256: None,
        }
    }
    #[test]
    fn established_minisign_vector_and_tampering() {
        assert_eq!(verify_minisign(b"test", SIG, KEY), Ok(()));
        assert_eq!(verify_minisign(b"tent", SIG, KEY), Err(VerifyError::Signature));
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
        assert_eq!(validate_metadata(&metadata(), &policy(), 0), Err(VerifyError::Expired));
        let mut m = metadata();
        m.metadata_version = 2;
        assert_eq!(validate_metadata(&m, &policy(), 100), Err(VerifyError::Rollback));
        for field in ["channel", "publisher", "platform", "artifact"] {
            let mut m = metadata();
            match field {
                "channel" => m.channel = "preview".into(),
                "publisher" => m.publisher = "other".into(),
                "platform" => m.platform = "linux-x64".into(),
                _ => m.artifact_type = "host".into(),
            }
            assert_eq!(validate_metadata(&m, &policy(), 100), Err(VerifyError::Policy));
        }
        let verified = VerifiedManifest(metadata());
        assert_eq!(verified.verify_package(&mut &b"test"[..]), Ok(()));
        assert_eq!(verified.verify_package(&mut &b"tent"[..]), Err(VerifyError::Hash));
        assert_eq!(verified.verify_package(&mut &b"tes"[..]), Err(VerifyError::Length));
        assert_eq!(verified.verify_package(&mut &b"tests"[..]), Err(VerifyError::Length));
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("unavailable"))
            }
        }
        assert_eq!(verified.verify_package(&mut Broken), Err(VerifyError::Io));
    }
    #[test]
    fn delivered_trust_digests_are_validated_and_optional() {
        // SEC-02: a core manifest may bind the next authority and root chain by digest.
        let mut m = metadata();
        m.authority_sha256 = Some("ab".repeat(32));
        assert_eq!(validate_metadata(&m, &policy(), 100), Ok(()));
        m.root_transitions_sha256 = Some("cd".repeat(32));
        assert_eq!(validate_metadata(&m, &policy(), 100), Ok(()));
        for (authority, transitions) in [
            (Some("AB".repeat(32)), None),
            (Some("ab".repeat(31)), None),
            (None, Some("cd".repeat(32))),
            (Some("ab".repeat(32)), Some("zz".repeat(32))),
        ] {
            let mut m = metadata();
            m.authority_sha256 = authority;
            m.root_transitions_sha256 = transitions;
            assert_eq!(validate_metadata(&m, &policy(), 100), Err(VerifyError::Policy));
        }
        let parsed: Manifest = serde_json::from_slice(include_bytes!("../tests/fixtures/valid.json")).unwrap();
        assert_eq!(parsed.authority_sha256, None);
    }
    #[test]
    fn publisher_pin_matches_subject_and_any_listed_issuer_only() {
        let pin = PublisherPin::parse("SignPath Foundation", "Issuing CA 2021|Issuing CA 2025").unwrap();
        assert!(pin.accepts("SignPath Foundation", "Issuing CA 2021"));
        assert!(pin.accepts("SignPath Foundation", "Issuing CA 2025"));
        assert!(!pin.accepts("SignPath Foundation", "Other CA"));
        assert!(!pin.accepts("signpath foundation", "Issuing CA 2021"));
        assert!(!pin.accepts("Other Publisher", "Issuing CA 2021"));
        let long = "a".repeat(129);
        for (subject, issuers) in [
            ("", "Issuing CA"),
            ("SignPath Foundation", ""),
            ("SignPath Foundation", "Issuing CA|Issuing CA"),
            ("SignPath Foundation", "Issuing CA|"),
            ("Tab\tName", "Issuing CA"),
            (" Leading", "Issuing CA"),
            ("SignPath Foundation", "CA1|CA2|CA3|CA4|CA5|CA6|CA7|CA8|CA9"),
            (long.as_str(), "Issuing CA"),
        ] {
            assert_eq!(
                PublisherPin::parse(subject, issuers),
                Err(VerifyError::Policy),
                "{subject:?}"
            );
        }
    }
    #[test]
    fn core_policy_is_the_single_shared_update_policy() {
        let policy = core_update_policy(KEY, "Bareline", "stable", 3);
        assert_eq!(policy.artifact_type, CORE_ARTIFACT_TYPE);
        assert_eq!(policy.platform, "windows-x64");
        assert_eq!(policy.protocol, 1);
        let mut m = metadata();
        m.artifact_type = CORE_ARTIFACT_TYPE.into();
        m.publisher = "Bareline".into();
        assert_eq!(validate_metadata(&m, &policy, 100), Ok(()));
        // A certificate digest is not the manifest publisher identity (SEC-01).
        m.publisher = "07".repeat(32);
        assert_eq!(validate_metadata(&m, &policy, 100), Err(VerifyError::Policy));
    }
    #[test]
    fn verify_errors_have_plain_language() {
        for error in [
            VerifyError::Size,
            VerifyError::Signature,
            VerifyError::Metadata,
            VerifyError::Policy,
            VerifyError::Rollback,
            VerifyError::Expired,
            VerifyError::Length,
            VerifyError::Hash,
            VerifyError::Io,
        ] {
            // Exhaustive without a wildcard: a new variant fails to compile here.
            match error {
                VerifyError::Size
                | VerifyError::Signature
                | VerifyError::Metadata
                | VerifyError::Policy
                | VerifyError::Rollback
                | VerifyError::Expired
                | VerifyError::Length
                | VerifyError::Hash
                | VerifyError::Io => {}
            }
            let message = error.to_string();
            assert!(message.contains(' '), "{message}");
            assert_ne!(message, format!("{error:?}"));
        }
    }
}
