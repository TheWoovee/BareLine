// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signed offline package source. Verified bytes are retained, eliminating a path
//! reopen between signature/hash verification and archive installation.
use crate::{Capability, PROTOCOL_VERSION, valid_id};
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};
const MAX_PACKAGE: u64 = 128 * 1024 * 1024;
const MAX_METADATA: usize = 1024 * 1024;
#[derive(Debug)]
pub struct PackageRequest {
    pub id: String,
    pub version: String,
}
#[derive(Debug)]
pub struct VerifiedPackage {
    path: PathBuf,
    digest: [u8; 32],
    bytes: Vec<u8>,
    entry: CatalogEntry,
    evidence: CatalogEvidence,
}

/// Retained signed bytes are the authority for an installed package, never the
/// editable manager index or extracted manifest file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEvidence {
    pub metadata: String,
    pub signature: String,
    pub accepted_unix: u64,
}
impl VerifiedPackage {
    pub fn cache(&self, root: &Path) -> Result<(), PackageError> {
        if fs::symlink_metadata(root)
            .map_err(|_| PackageError::Io)?
            .file_type()
            .is_symlink()
        {
            return Err(PackageError::UnsafeArchive);
        }
        let archive = root.join(format!("{}.blex", self.entry.sha256));
        match fs::OpenOptions::new().write(true).create_new(true).open(&archive) {
            Ok(mut file) => {
                let result = file.write_all(&self.bytes).and_then(|_| file.sync_all());
                drop(file);
                if result.is_err() {
                    let _ = fs::remove_file(&archive);
                    return Err(PackageError::Io);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let mut bytes = Vec::new();
                fs::File::open(&archive)
                    .map_err(|_| PackageError::Io)?
                    .take(MAX_PACKAGE + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| PackageError::Io)?;
                if bytes != self.bytes {
                    return Err(PackageError::HashMismatch);
                }
            }
            Err(_) => return Err(PackageError::Io),
        }
        let receipt = serde_json::to_vec(&self.evidence).map_err(|_| PackageError::Metadata)?;
        atomic_record(&root.join(format!("{}.receipt.json", self.entry.sha256)), &receipt)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
    pub fn metadata(&self) -> &CatalogEntry {
        &self.entry
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum PackageError {
    OnlineUnavailable,
    InvalidSignature,
    WrongIdentity,
    HashMismatch,
    Metadata,
    Expired,
    Rollback,
    Io,
    Size,
    UnsafeArchive,
    AlreadyInstalled,
    Cancelled,
}
pub trait VerifiedPackageSource {
    fn fetch(&self, request: &PackageRequest) -> Result<VerifiedPackage, PackageError>;
}
pub struct OfflineOnlyOnlineStub;
impl VerifiedPackageSource for OfflineOnlyOnlineStub {
    fn fetch(&self, _: &PackageRequest) -> Result<VerifiedPackage, PackageError> {
        Err(PackageError::OnlineUnavailable)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub id: String,
    pub version: String,
    pub publisher: String,
    pub artifact_type: String,
    pub platform: String,
    pub channel: String,
    pub length: u64,
    pub sha256: String,
    pub minimum_protocol: u16,
    pub maximum_protocol: u16,
    pub capabilities: Vec<Capability>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub metadata_version: u64,
    pub expires_unix: u64,
    pub entries: Vec<CatalogEntry>,
}
/// A caller-supplied owner-pinned catalog key, never loaded from the package.
pub struct CatalogPolicy<'a> {
    pub public_key: &'a str,
    pub publisher: &'a str,
    pub channel: &'a str,
    pub platform: &'a str,
    pub artifact_type: &'a str,
    pub highest_metadata_version: u64,
    pub now_unix: u64,
}
pub struct OfflinePackageSource {
    catalog: Catalog,
    root: PathBuf,
    publisher: String,
    channel: String,
    platform: String,
    artifact_type: String,
    evidence: CatalogEvidence,
}
impl OfflinePackageSource {
    pub fn open(
        root: PathBuf,
        bytes: &[u8],
        signature: &str,
        policy: &CatalogPolicy<'_>,
    ) -> Result<Self, PackageError> {
        if bytes.len() > MAX_METADATA || signature.len() > 8192 {
            return Err(PackageError::Size);
        }
        let key = PublicKey::from_base64(policy.public_key).map_err(|_| PackageError::InvalidSignature)?;
        let signature_text = signature;
        let signature = Signature::decode(signature).map_err(|_| PackageError::InvalidSignature)?;
        key.verify(bytes, &signature, false)
            .map_err(|_| PackageError::InvalidSignature)?;
        let catalog: Catalog = serde_json::from_slice(bytes).map_err(|_| PackageError::Metadata)?;
        if catalog.schema_version != 1 || catalog.entries.len() > 4096 {
            return Err(PackageError::Metadata);
        }
        if policy.now_unix == 0 || catalog.expires_unix <= policy.now_unix {
            return Err(PackageError::Expired);
        }
        if catalog.metadata_version < policy.highest_metadata_version {
            return Err(PackageError::Rollback);
        }
        let mut identities = std::collections::BTreeSet::new();
        for entry in &catalog.entries {
            if !identities.insert((&entry.id, &entry.version)) {
                return Err(PackageError::Metadata);
            }
        }
        Ok(Self {
            catalog,
            root,
            publisher: policy.publisher.into(),
            channel: policy.channel.into(),
            platform: policy.platform.into(),
            artifact_type: policy.artifact_type.into(),
            evidence: CatalogEvidence {
                metadata: std::str::from_utf8(bytes)
                    .map_err(|_| PackageError::Metadata)?
                    .to_owned(),
                signature: signature_text.to_owned(),
                accepted_unix: policy.now_unix,
            },
        })
    }
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.catalog.entries
    }
    pub fn metadata_version(&self) -> u64 {
        self.catalog.metadata_version
    }
}
impl VerifiedPackageSource for OfflinePackageSource {
    fn fetch(&self, request: &PackageRequest) -> Result<VerifiedPackage, PackageError> {
        let entry = self
            .catalog
            .entries
            .iter()
            .find(|e| e.id == request.id && e.version == request.version)
            .ok_or(PackageError::WrongIdentity)?;
        if !valid_id(&entry.id)
            || entry.publisher != self.publisher
            || entry.channel != self.channel
            || entry.platform != self.platform
            || entry.artifact_type != self.artifact_type
            || entry.minimum_protocol > PROTOCOL_VERSION
            || entry.maximum_protocol < PROTOCOL_VERSION
        {
            return Err(PackageError::WrongIdentity);
        }
        if entry.length == 0
            || entry.length > MAX_PACKAGE
            || entry.sha256.len() != 64
            || !entry
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(PackageError::Size);
        }
        // Neither extension ID nor version is used as a path.
        let path = self.root.join(format!("{}.blex", entry.sha256));
        let file = fs::File::open(&path).map_err(|_| PackageError::Io)?;
        let mut bytes = Vec::new();
        file.take(entry.length + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| PackageError::Io)?;
        if bytes.len() as u64 != entry.length {
            return Err(PackageError::Size);
        }
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        if format!("{:x}", Sha256::digest(&bytes)) != entry.sha256 {
            return Err(PackageError::HashMismatch);
        }
        Ok(VerifiedPackage {
            path,
            digest,
            bytes,
            entry: entry.clone(),
            evidence: self.evidence.clone(),
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub publisher: String,
    pub minimum_protocol: u16,
    pub maximum_protocol: u16,
    pub entry_component: String,
    pub commands: Vec<String>,
    #[serde(default)]
    pub background_commands: Vec<String>,
    pub panels: Vec<String>,
    pub capabilities: Vec<Capability>,
}
#[derive(Debug, Clone)]
pub struct InstalledPackage {
    pub manifest: ExtensionManifest,
    pub component_sha256: [u8; 32],
    directory: PathBuf,
    files: Vec<String>,
    pub id: String,
    pub version: String,
}
impl InstalledPackage {
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    /// Uninstall the verified version and its two fixed retained cache records.
    /// No directory scan or recursive deletion is used; unrelated files survive.
    pub fn remove_cached(self) -> Result<(), PackageError> {
        let root = self.directory.parent().ok_or(PackageError::UnsafeArchive)?;
        let digest = self
            .directory
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(PackageError::UnsafeArchive)?;
        let paths = [
            root.join(format!("{digest}.blex")),
            root.join(format!("{digest}.receipt.json")),
        ];
        for path in &paths {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(PackageError::UnsafeArchive),
            }
        }
        self.remove()?;
        for path in paths {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(PackageError::Io),
            }
        }
        Ok(())
    }
    /// Only the previously enumerated verified files are removed. Added files keep
    /// the directory nonempty. Refuse links rather than following a substituted root.
    pub fn remove(self) -> Result<(), PackageError> {
        if fs::symlink_metadata(&self.directory)
            .map_err(|_| PackageError::Io)?
            .file_type()
            .is_symlink()
        {
            return Err(PackageError::UnsafeArchive);
        }
        for name in &self.files {
            let file = self.directory.join(name);
            let metadata = fs::symlink_metadata(&file).map_err(|_| PackageError::Io)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(PackageError::UnsafeArchive);
            }
        }
        for name in self.files {
            fs::remove_file(self.directory.join(name)).map_err(|_| PackageError::Io)?;
        }
        fs::remove_dir(self.directory).map_err(|_| PackageError::Io)
    }
}
fn safe_flat_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.ends_with(' ')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && ![
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
            "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ]
        .contains(&name.split('.').next().unwrap_or("").to_ascii_uppercase().as_str())
}

/// Versioned owner state is published only after its replacement is durable.
pub fn atomic_record(path: &Path, bytes: &[u8]) -> Result<(), PackageError> {
    if bytes.len() > 8 * MAX_METADATA {
        return Err(PackageError::Size);
    }
    let parent = path.parent().ok_or(PackageError::Io)?;
    let metadata = fs::symlink_metadata(parent).map_err(|_| PackageError::Io)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(PackageError::UnsafeArchive);
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| PackageError::Io)?
        .as_nanos();
    let temporary = parent.join(format!("record-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| PackageError::Io)?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| PackageError::Io)?;
        drop(file);
        fs::rename(&temporary, path).map_err(|_| PackageError::Io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

/// Restore an already installed digest from retained signed evidence. Current
/// metadata freshness gates new installs/updates, not use of a prior installation
/// (FC-07/08 offline usability). Rechecking the signature and every extracted byte
/// prevents the editable index or manifest from becoming package authority.
pub fn restore_cached(
    root: &Path,
    digest: &str,
    policy: &CatalogPolicy<'_>,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<InstalledPackage, PackageError> {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(PackageError::WrongIdentity);
    }
    let mut bytes = Vec::new();
    fs::File::open(root.join(format!("{digest}.receipt.json")))
        .map_err(|_| PackageError::Io)?
        .take((8 * MAX_METADATA + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| PackageError::Io)?;
    if bytes.len() > 8 * MAX_METADATA {
        return Err(PackageError::Size);
    }
    let evidence: CatalogEvidence = serde_json::from_slice(&bytes).map_err(|_| PackageError::Metadata)?;
    if evidence.accepted_unix == 0 || evidence.accepted_unix > policy.now_unix {
        return Err(PackageError::Metadata);
    }
    let historical = CatalogPolicy {
        public_key: policy.public_key,
        publisher: policy.publisher,
        channel: policy.channel,
        platform: policy.platform,
        artifact_type: policy.artifact_type,
        highest_metadata_version: 0,
        now_unix: evidence.accepted_unix,
    };
    let source = OfflinePackageSource::open(
        root.to_owned(),
        evidence.metadata.as_bytes(),
        &evidence.signature,
        &historical,
    )?;
    let entry = source
        .entries()
        .iter()
        .find(|entry| entry.sha256 == digest)
        .ok_or(PackageError::WrongIdentity)?;
    let package = source.fetch(&PackageRequest {
        id: entry.id.clone(),
        version: entry.version.clone(),
    })?;
    package.restore(root, cancelled)
}
impl VerifiedPackage {
    /// v1 uses a deliberately flat archive namespace. No links, directories, devices,
    /// duplicate case-insensitive paths, or decompression beyond the signed limits.
    pub fn install(
        &self,
        root: &Path,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<InstalledPackage, PackageError> {
        self.materialize(root, cancelled, false)
    }

    pub fn restore(
        &self,
        root: &Path,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<InstalledPackage, PackageError> {
        self.materialize(root, cancelled, true)
    }

    fn materialize(
        &self,
        root: &Path,
        cancelled: &std::sync::atomic::AtomicBool,
        restoring: bool,
    ) -> Result<InstalledPackage, PackageError> {
        let declared = validate_zip_directory(&self.bytes)?;
        let mut archive = zip::ZipArchive::new(Cursor::new(&self.bytes)).map_err(|_| PackageError::UnsafeArchive)?;
        // Duplicate central-directory names collapse in the zip reader, so an entry
        // count that disagrees with the EOCD record means the archive is ambiguous.
        if archive.is_empty() || archive.len() > 256 || archive.len() as u32 != declared {
            return Err(PackageError::UnsafeArchive);
        }
        let mut files = Vec::new();
        let mut total = 0u64;
        let mut names = std::collections::BTreeSet::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i).map_err(|_| PackageError::UnsafeArchive)?;
            if !safe_flat_name(file.name())
                || file.is_dir()
                || file
                    .unix_mode()
                    .is_some_and(|m| (m & 0o170000) != 0 && (m & 0o170000) != 0o100000)
                || !names.insert(file.name().to_ascii_lowercase())
            {
                return Err(PackageError::UnsafeArchive);
            }
            total = total.checked_add(file.size()).ok_or(PackageError::Size)?;
            if total > MAX_PACKAGE {
                return Err(PackageError::Size);
            }
            files.push(file.name().to_owned());
        }
        let mut manifest_bytes = Vec::new();
        archive
            .by_name("manifest.toml")
            .map_err(|_| PackageError::Metadata)?
            .take(65537)
            .read_to_end(&mut manifest_bytes)
            .map_err(|_| PackageError::Io)?;
        if manifest_bytes.len() > 65536 {
            return Err(PackageError::Size);
        }
        let manifest: ExtensionManifest =
            toml_edit::de::from_slice(&manifest_bytes).map_err(|_| PackageError::Metadata)?;
        if manifest.schema_version != 1
            || manifest.id != self.entry.id
            || manifest.version != self.entry.version
            || manifest.publisher != self.entry.publisher
            || manifest.minimum_protocol != self.entry.minimum_protocol
            || manifest.maximum_protocol != self.entry.maximum_protocol
            || manifest.capabilities != self.entry.capabilities
            || !safe_flat_name(&manifest.entry_component)
            || !files.contains(&manifest.entry_component)
            || manifest.commands.iter().any(|c| !valid_id(c))
            || manifest.panels.iter().any(|p| !valid_id(p))
            || manifest.commands.len() > 256
            || manifest
                .commands
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != manifest.commands.len()
            || manifest.panels.iter().collect::<std::collections::BTreeSet<_>>().len() != manifest.panels.len()
            || manifest
                .background_commands
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != manifest.background_commands.len()
            || manifest
                .background_commands
                .iter()
                .any(|command| !manifest.commands.contains(command))
            || manifest.panels.len() > 32
        {
            return Err(PackageError::WrongIdentity);
        }
        let mut component = archive
            .by_name(&manifest.entry_component)
            .map_err(|_| PackageError::Metadata)?;
        let mut component_hash = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            let count = component.read(&mut buffer).map_err(|_| PackageError::Io)?;
            if count == 0 {
                break;
            }
            component_hash.update(&buffer[..count]);
        }
        drop(component);
        let root_metadata = fs::symlink_metadata(root).map_err(|_| PackageError::Io)?;
        if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
            return Err(PackageError::UnsafeArchive);
        }
        let directory = root.join(&self.entry.sha256);
        if restoring {
            let metadata = fs::symlink_metadata(&directory).map_err(|_| PackageError::Io)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(PackageError::UnsafeArchive);
            }
            for name in &files {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(PackageError::Cancelled);
                }
                let path = directory.join(name);
                let metadata = fs::symlink_metadata(&path).map_err(|_| PackageError::Io)?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(PackageError::UnsafeArchive);
                }
                let mut expected = archive.by_name(name).map_err(|_| PackageError::UnsafeArchive)?;
                if metadata.len() != expected.size() {
                    return Err(PackageError::HashMismatch);
                }
                let mut actual = fs::File::open(path).map_err(|_| PackageError::Io)?;
                let mut left = [0; 65536];
                let mut right = [0; 65536];
                loop {
                    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                        return Err(PackageError::Cancelled);
                    }
                    let count = expected.read(&mut left).map_err(|_| PackageError::Io)?;
                    if count == 0 {
                        let mut extra = [0];
                        if actual.read(&mut extra).map_err(|_| PackageError::Io)? != 0 {
                            return Err(PackageError::HashMismatch);
                        }
                        break;
                    }
                    actual
                        .read_exact(&mut right[..count])
                        .map_err(|_| PackageError::HashMismatch)?;
                    if left[..count] != right[..count] {
                        return Err(PackageError::HashMismatch);
                    }
                }
            }
            return Ok(InstalledPackage {
                directory,
                files,
                id: manifest.id.clone(),
                version: manifest.version.clone(),
                manifest,
                component_sha256: component_hash.finalize().into(),
            });
        }
        fs::create_dir(&directory).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                PackageError::AlreadyInstalled
            } else {
                PackageError::Io
            }
        })?;
        let result = (|| {
            for name in &files {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(PackageError::Cancelled);
                }
                let mut input = archive.by_name(name).map_err(|_| PackageError::UnsafeArchive)?;
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(directory.join(name))
                    .map_err(|_| PackageError::Io)?;
                let mut buffer = [0; 65536];
                let mut written = 0u64;
                loop {
                    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                        return Err(PackageError::Cancelled);
                    }
                    let count = input.read(&mut buffer).map_err(|_| PackageError::Io)?;
                    if count == 0 {
                        break;
                    }
                    written += count as u64;
                    if written > input.size() || written > MAX_PACKAGE {
                        return Err(PackageError::Size);
                    }
                    output.write_all(&buffer[..count]).map_err(|_| PackageError::Io)?;
                }
                if written != input.size() {
                    return Err(PackageError::Size);
                }
                output.sync_all().map_err(|_| PackageError::Io)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            for name in &files {
                let _ = fs::remove_file(directory.join(name));
            }
            let _ = fs::remove_dir(&directory);
            return Err(error);
        }
        Ok(InstalledPackage {
            directory,
            files,
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            manifest,
            component_sha256: component_hash.finalize().into(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_central_directory_names_fail_the_entry_count_assertion() {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for name in ["a.wasm", "b.wasm"] {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(name.as_bytes()).unwrap();
        }
        let mut bytes = zip.finish().unwrap().into_inner();
        assert_eq!(validate_zip_directory(&bytes).unwrap(), 2);
        assert_eq!(zip::ZipArchive::new(Cursor::new(&bytes)).unwrap().len(), 2);
        // Rewrite the *central directory* copy of the second name so both records claim
        // "a.wasm"; the reader keeps one entry while the EOCD still declares two.
        let directory = bytes
            .windows(4)
            .rposition(|w| w == b"PK\x01\x02")
            .expect("central directory");
        let position = bytes[directory..]
            .windows(6)
            .position(|w| w == b"b.wasm")
            .expect("second name")
            + directory;
        bytes[position..position + 6].copy_from_slice(b"a.wasm");
        assert_eq!(validate_zip_directory(&bytes).unwrap(), 2);
        let archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        assert_eq!(archive.len(), 1, "the zip reader collapses duplicate names");
        assert_ne!(archive.len() as u32, validate_zip_directory(&bytes).unwrap());
    }
    #[test]
    fn archive_names_reject_escape_devices_and_ads() {
        for name in ["../x", "x/y", "x\\y", "C:x", "NUL.wasm", "COM1", "a.", ".", "x "] {
            assert!(!safe_flat_name(name), "{name}");
        }
        assert!(safe_flat_name("entry.wasm"));
    }
    #[test]
    fn online_unavailable_and_unsigned_catalog_rejected() {
        assert!(matches!(
            OfflineOnlyOnlineStub.fetch(&PackageRequest {
                id: "x".into(),
                version: "1".into()
            }),
            Err(PackageError::OnlineUnavailable)
        ));
        let policy = CatalogPolicy {
            public_key: "untrusted",
            publisher: "test",
            channel: "stable",
            platform: "windows-x64",
            artifact_type: "extension",
            highest_metadata_version: 0,
            now_unix: 1,
        };
        assert!(matches!(
            OfflinePackageSource::open(PathBuf::new(), b"not JSON", "bad", &policy),
            Err(PackageError::InvalidSignature)
        ));
    }
}

#[cfg(test)]
mod signed_tests {
    use super::*;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/security/package_corpus.rs"
    ));
    use base64::{Engine, engine::general_purpose::STANDARD};
    use ed25519_dalek::{Signer, SigningKey};
    fn sign(bytes: &[u8]) -> (String, String) {
        // Deterministic TEST-ONLY private key; never accepted by production policy.
        let key = SigningKey::from_bytes(&[42; 32]);
        let id = [7; 8];
        let mut public = b"Ed".to_vec();
        public.extend_from_slice(&id);
        public.extend_from_slice(key.verifying_key().as_bytes());
        let digest = blake2::Blake2b512::digest(bytes);
        let signature = key.sign(&digest).to_bytes();
        let mut body = b"ED".to_vec();
        body.extend_from_slice(&id);
        body.extend_from_slice(&signature);
        let comment = "fixture-only";
        let mut global = signature.to_vec();
        global.extend_from_slice(comment.as_bytes());
        (
            STANDARD.encode(public),
            format!(
                "untrusted comment: test fixture\n{}\ntrusted comment: {comment}\n{}",
                STANDARD.encode(body),
                STANDARD.encode(key.sign(&global).to_bytes())
            ),
        )
    }
    fn fixture() -> (Vec<u8>, Catalog) {
        let manifest = ExtensionManifest {
            schema_version: 1,
            id: "fixture.tools".into(),
            version: "1".into(),
            publisher: "fixture".into(),
            minimum_protocol: 1,
            maximum_protocol: 1,
            entry_component: "entry.wasm".into(),
            commands: vec![],
            background_commands: vec![],
            panels: vec![],
            capabilities: vec![],
        };
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("manifest.toml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(toml_edit::ser::to_string(&manifest).unwrap().as_bytes())
            .unwrap();
        zip.start_file("entry.wasm", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"safe nonexecuted fixture").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let entry = CatalogEntry {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            publisher: manifest.publisher,
            artifact_type: "extension".into(),
            platform: "windows-x64".into(),
            channel: "stable".into(),
            length: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            minimum_protocol: 1,
            maximum_protocol: 1,
            capabilities: vec![],
        };
        (
            bytes,
            Catalog {
                schema_version: 1,
                metadata_version: 3,
                expires_unix: 200,
                entries: vec![entry],
            },
        )
    }
    fn policy(key: &str) -> CatalogPolicy<'_> {
        CatalogPolicy {
            public_key: key,
            publisher: "fixture",
            channel: "stable",
            platform: "windows-x64",
            artifact_type: "extension",
            highest_metadata_version: 3,
            now_unix: 100,
        }
    }
    #[test]
    fn signed_offline_install_tampering_freshness_and_remove() {
        let (bytes, catalog) = fixture();
        let metadata = serde_json::to_vec(&catalog).unwrap();
        let (key, signature) = sign(&metadata);
        let root = std::env::temp_dir().join(format!(
            "bareline-signed-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let package_file = root.join(format!("{}.blex", catalog.entries[0].sha256));
        fs::write(&package_file, &bytes).unwrap();
        let source = OfflinePackageSource::open(root.clone(), &metadata, &signature, &policy(&key)).unwrap();
        let request = PackageRequest {
            id: "fixture.tools".into(),
            version: "1".into(),
        };
        let verified = source.fetch(&request).unwrap();
        // Replacing source after verification cannot alter extracted bytes.
        fs::write(&package_file, b"tampered").unwrap();
        let installed = verified
            .install(&root, &std::sync::atomic::AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            fs::read(installed.directory().join("entry.wasm")).unwrap(),
            b"safe nonexecuted fixture"
        );
        assert_eq!(installed.manifest.id, "fixture.tools");
        assert_eq!(
            installed.component_sha256,
            <[u8; 32]>::from(Sha256::digest(b"safe nonexecuted fixture"))
        );
        // Retained receipts restore the exact accepted package offline after
        // metadata expiry; a new install still requires fresh metadata.
        assert!(matches!(verified.cache(&root), Err(PackageError::HashMismatch)));
        fs::write(&package_file, &bytes).unwrap();
        verified.cache(&root).unwrap();
        let mut later = policy(&key);
        later.now_unix = 300;
        let restored = restore_cached(
            &root,
            &catalog.entries[0].sha256,
            &later,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(restored.component_sha256, installed.component_sha256);
        fs::write(
            installed.directory().join("entry.wasm"),
            b"tampered extracted component",
        )
        .unwrap();
        assert!(
            restore_cached(
                &root,
                &catalog.entries[0].sha256,
                &later,
                &std::sync::atomic::AtomicBool::new(false)
            )
            .is_err()
        );
        fs::write(installed.directory().join("entry.wasm"), b"safe nonexecuted fixture").unwrap();
        assert!(
            restore_cached(
                &root,
                &catalog.entries[0].sha256,
                &later,
                &std::sync::atomic::AtomicBool::new(true)
            )
            .is_err()
        );
        let retained = installed.directory().to_owned();
        assert!(retained.exists());
        installed.remove().unwrap();
        assert!(!retained.exists());
        fs::write(&package_file, b"tampered").unwrap();
        assert!(matches!(source.fetch(&request), Err(PackageError::Size)));
        let mut changed = metadata.clone();
        changed[0] ^= 1;
        assert!(matches!(
            OfflinePackageSource::open(root.clone(), &changed, &signature, &policy(&key)),
            Err(PackageError::InvalidSignature)
        ));
        let mut expired = policy(&key);
        expired.now_unix = 200;
        assert!(matches!(
            OfflinePackageSource::open(root.clone(), &metadata, &signature, &expired),
            Err(PackageError::Expired)
        ));
        let mut rollback = policy(&key);
        rollback.highest_metadata_version = 4;
        assert!(matches!(
            OfflinePackageSource::open(root.clone(), &metadata, &signature, &rollback),
            Err(PackageError::Rollback)
        ));
        let mut wrong = policy(&key);
        wrong.publisher = "other";
        let source = OfflinePackageSource::open(root.clone(), &metadata, &signature, &wrong).unwrap();
        assert!(matches!(source.fetch(&request), Err(PackageError::WrongIdentity)));
        fs::write(&package_file, &bytes).unwrap();
        verified.cache(&root).unwrap();
        let again = verified
            .install(&root, &std::sync::atomic::AtomicBool::new(false))
            .unwrap();
        let unrelated = root.join("unrelated-owner-file");
        fs::write(&unrelated, b"keep").unwrap();
        again.remove_cached().unwrap();
        assert!(!package_file.exists());
        assert!(
            !root
                .join(format!("{}.receipt.json", catalog.entries[0].sha256))
                .exists()
        );
        assert_eq!(fs::read(&unrelated).unwrap(), b"keep");
        fs::remove_file(unrelated).unwrap();
        fs::remove_dir(root).unwrap();
    }
}

// Inspect classic EOCD before zip allocates per-entry metadata. v1 deliberately
// rejects ZIP64/multidisk; our 128 MiB / 256-file cap never needs either.
fn validate_zip_directory(bytes: &[u8]) -> Result<u32, PackageError> {
    let start = bytes.len().saturating_sub(65557);
    let offset = (start..bytes.len().saturating_sub(21))
        .rev()
        .find(|&i| bytes[i..].starts_with(b"PK\x05\x06"))
        .ok_or(PackageError::UnsafeArchive)?;
    let end = &bytes[offset..];
    let u16_at = |i| u16::from_le_bytes([end[i], end[i + 1]]);
    let u32_at = |i| u32::from_le_bytes([end[i], end[i + 1], end[i + 2], end[i + 3]]);
    let count = u16_at(10);
    let directory_size = u32_at(12) as usize;
    let directory_offset = u32_at(16) as usize;
    if u16_at(4) != 0
        || u16_at(6) != 0
        || u16_at(8) != count
        || count == 0
        || count > 256
        || directory_size > 1024 * 1024
        || directory_offset.checked_add(directory_size) != Some(offset)
        || offset + 22 + u16_at(20) as usize != bytes.len()
    {
        return Err(PackageError::UnsafeArchive);
    }
    Ok(count as u32)
}
