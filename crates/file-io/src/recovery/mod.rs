// SPDX-License-Identifier: MPL-2.0
//! Disk-backed recovery. All calls belong on the I/O worker. Each writer owns a new
//! private directory; an existing journal is read-only and is never resumed by another writer.
use crate::cancellation::Cancellation;
use bareline_platform::LocalFileSystem;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

mod streaming;
pub use streaming::{replay_source_transactions, open_retained_owned};
pub(crate) use streaming::admit_disk;
#[cfg(test)]
pub(crate) use streaming::disk_usage;
const VERSION: u32 = 2;
const MAX_RECORD: usize = 1024 * 1024;
const MAX_TRANSACTION: usize = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;
const MAX_JOURNAL: u64 = 64 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryMetadata {
    #[serde(default)]
    pub original_path: Option<PathBuf>,
    pub source_generation: String,
    pub codec_catalog_version: String,
    pub original_len: u64,
}
/// Edits are sorted, nonoverlapping offsets in the pre-transaction byte domain.
/// The caller owns both inserted and inverse bytes before committing its transaction.
#[derive(Clone, Debug)]
pub struct RecoveryEdit {
    pub offset: u64,
    pub removed: Vec<u8>,
    pub inserted: Vec<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableReceipt {
    pub revision: u64,
    pub protected_unix_ms: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryStatus {
    Complete,
    EditsOnly,
    CorruptTail,
    SourceUnavailable,
    Discarded,
}
#[derive(Clone, Debug)]
pub struct RecoveryInspection {
    pub status: RecoveryStatus,
    pub metadata: RecoveryMetadata,
    pub last_durable: Option<DurableReceipt>,
    /// Durable checkpoint watermark may be ahead of the validated prefix after corruption.
    pub checkpoint_durable: Option<DurableReceipt>,
    pub validated_records: usize,
    pub complete_baseline: bool,
    /// Latest validated policy transaction; absent in legacy v1 byte-only journals.
    pub document_metadata: Option<std::collections::BTreeMap<String, String>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    SegmentsWritten,
    SegmentsFlushed,
    JournalWritten,
    JournalFlushed,
    CheckpointWritten,
    CheckpointCommitted,
}
/// Error injection models interrupted storage at each publication boundary.
pub trait FaultInjector {
    fn boundary(&mut self, at: Boundary) -> io::Result<()>;
}
struct NoFault;
impl FaultInjector for NoFault {
    fn boundary(&mut self, _: Boundary) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Blob {
    name: String,
    len: u64,
    sha256: [u8; 32],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    metadata: RecoveryMetadata,
    baseline: Option<Blob>,
    durable: Option<DurableReceipt>,
    retired: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditRef {
    offset: u64,
    removed: u64,
    inserted: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata: Option<std::collections::BTreeMap<String, String>>,
    receipt: DurableReceipt,
    segment: Blob,
    edits: Vec<EditRef>,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn cancelled(cancel: &Cancellation) -> io::Result<()> {
    cancel
        .check()
        .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "recovery cancelled"))
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

/// An original-generation copy job that may run independently of the actor lock.
pub struct BaselinePreparation {
    identity: std::sync::Arc<()>,
    directory: PathBuf,
    original_len: u64,
    receipt: Option<DurableReceipt>,
}
pub struct PreparedBaseline {
    job: BaselinePreparation,
    blob: Blob,
    preserve: bool,
}
impl Drop for PreparedBaseline {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = fs::remove_file(self.job.directory.join(&self.blob.name));
        }
    }
}
impl BaselinePreparation {
    /// Revalidate the complete source fingerprint after copying; changed sources cannot seal.
    pub fn copy<R: Read>(
        self,
        source: &mut R,
        verify_source: impl FnOnce() -> io::Result<bool>,
        cancel: &Cancellation,
    ) -> io::Result<PreparedBaseline> {
        let path = self.directory.join("baseline.bin");
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let result = (|| {
            let mut hash = Sha256::new();
            let mut len = 0u64;
            let mut buffer = [0u8; CHUNK];
            loop {
                cancelled(cancel)?;
                let n = source.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                len = len
                    .checked_add(n as u64)
                    .ok_or_else(|| invalid("baseline length overflow"))?;
                if len > self.original_len {
                    return Err(invalid("baseline grew"));
                }
                file.write_all(&buffer[..n])?;
                hash.update(&buffer[..n]);
            }
            if len != self.original_len || !verify_source()? {
                return Err(invalid("source generation changed during baseline seal"));
            }
            cancelled(cancel)?;
            file.sync_all()?;
            Ok(Blob {
                name: "baseline.bin".into(),
                len,
                sha256: hash.finalize().into(),
            })
        })();
        drop(file);
        match result {
            Ok(blob) => Ok(PreparedBaseline {
                job: self,
                blob,
                preserve: false,
            }),
            Err(error) => {
                let _ = fs::remove_file(path);
                Err(error)
            }
        }
    }
}

pub struct RecoveryWriter {
    identity: std::sync::Arc<()>,
    directory: PathBuf,
    journal: File,
    manifest: Manifest,
    poisoned: bool,
    records: usize,
    current_len: u64,
}
impl RecoveryWriter {
    pub fn create(
        directory: &Path,
        metadata: RecoveryMetadata,
        platform: &dyn LocalFileSystem,
    ) -> io::Result<Self> {
        if metadata.source_generation.len() > 4096 || metadata.codec_catalog_version.len() > 4096 {
            return Err(invalid("recovery metadata limit"));
        }
        // create_dir, not create_dir_all: existing recovery directories cannot acquire a second writer.
        fs::create_dir(directory)?;
        let manifest = Manifest {
            version: VERSION,
            metadata,
            baseline: None,
            durable: None,
            retired: false,
        };
        let journal = OpenOptions::new()
            .create_new(true)
            .read(true)
            .append(true)
            .open(directory.join("journal.bin"))?;
        journal.sync_all()?;
        publish(directory, &manifest, platform, &mut NoFault)?;
        let current_len = manifest.metadata.original_len;
        Ok(Self {
            identity: std::sync::Arc::new(()),
            directory: directory.into(),
            journal,
            manifest,
            poisoned: false,
            records: 0,
            current_len,
        })
    }
    /// A group root is committed but this ordinary journal did not continue.
    /// Further writes require a fresh writer based on that authoritative root.
    pub(crate) fn break_continuity(&mut self) { self.poisoned=true; }
    pub fn last_durable(&self) -> Option<DurableReceipt> {
        self.manifest.durable
    }
    /// Capture an owned copy job without borrowing the writer during source I/O.
    pub fn prepare_baseline(&self) -> io::Result<BaselinePreparation> {
        self.ensure_writable()?;
        if self.manifest.baseline.is_some() {
            return Err(invalid("baseline already sealed"));
        }
        Ok(BaselinePreparation {
            identity: self.identity.clone(),
            directory: self.directory.clone(),
            original_len: self.manifest.metadata.original_len,
            receipt: self.last_durable(),
        })
    }
    /// Publish against the current manifest, retaining edits acknowledged during copying.
    pub fn attach_baseline(
        &mut self,
        mut prepared: PreparedBaseline,
        platform: &dyn LocalFileSystem,
    ) -> io::Result<()> {
        self.ensure_writable()?;
        if !std::sync::Arc::ptr_eq(&self.identity, &prepared.job.identity)
            || self.manifest.baseline.is_some()
            || prepared.job.receipt.is_some_and(|old| {
                self.last_durable()
                    .is_none_or(|current| current.revision < old.revision)
            })
            || self.directory.join("retired.json").try_exists()?
        {
            return Err(invalid("stale baseline preparation"));
        }
        let mut next = self.manifest.clone();
        next.baseline = Some(prepared.blob.clone());
        // A failed atomic publication may already reference this file. Preserve it.
        prepared.preserve = true;
        if let Err(error) = publish(&self.directory, &next, platform, &mut NoFault) {
            self.poisoned = true;
            return Err(error);
        }
        self.manifest = next;
        Ok(())
    }
    pub fn seal_baseline<R: Read>(
        &mut self,
        source: &mut R,
        verify_source: impl FnOnce() -> io::Result<bool>,
        cancel: &Cancellation,
        platform: &dyn LocalFileSystem,
    ) -> io::Result<()> {
        let prepared = self
            .prepare_baseline()?
            .copy(source, verify_source, cancel)?;
        self.attach_baseline(prepared, platform)
    }
    pub fn append(&mut self, revision: u64, edits: &[RecoveryEdit]) -> io::Result<DurableReceipt> {
        self.append_with_faults(revision, edits, &mut NoFault)
    }
    pub fn append_with_faults(
        &mut self,
        revision: u64,
        edits: &[RecoveryEdit],
        faults: &mut dyn FaultInjector,
    ) -> io::Result<DurableReceipt> {
        self.append_record(revision, edits, None, faults)
    }
    /// Policy bytes are part of the CRC-framed journal, never a dummy text edit.
    pub fn append_metadata(
        &mut self,
        revision: u64,
        metadata: &bareline_document::DocumentMetadata,
    ) -> io::Result<DurableReceipt> {
        self.append_record(revision, &[], Some(metadata.values().clone()), &mut NoFault)
    }
    fn append_record(
        &mut self,
        revision: u64,
        edits: &[RecoveryEdit],
        metadata: Option<std::collections::BTreeMap<String, String>>,
        faults: &mut dyn FaultInjector,
    ) -> io::Result<DurableReceipt> {
        self.ensure_writable()?;
        if (edits.is_empty() && metadata.is_none())
            || edits.len() > 4096
            || self.records >= MAX_RECORDS
            || self
                .manifest
                .durable
                .is_some_and(|r| revision <= r.revision)
        {
            return Err(invalid("invalid recovery transaction"));
        }
        let size = edits
            .iter()
            .try_fold(0usize, |total, e| {
                total
                    .checked_add(e.removed.len())?
                    .checked_add(e.inserted.len())
            })
            .filter(|n| *n <= MAX_TRANSACTION)
            .ok_or_else(|| invalid("recovery transaction limit"))?;
        let refs: Vec<EditRef> = edits
            .iter()
            .map(|e| EditRef {
                offset: e.offset,
                removed: e.removed.len() as u64,
                inserted: e.inserted.len() as u64,
            })
            .collect();
        let next_len = edited_len(self.current_len, &refs)?;
        let result = (|| {
            let name = format!("segment-{revision}.bin");
            let mut segment = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(self.directory.join(&name))?;
            let mut hash = Sha256::new();
            for edit in edits {
                segment.write_all(&edit.removed)?;
                hash.update(&edit.removed);
                segment.write_all(&edit.inserted)?;
                hash.update(&edit.inserted);
            }
            faults.boundary(Boundary::SegmentsWritten)?;
            segment.sync_all()?;
            faults.boundary(Boundary::SegmentsFlushed)?;
            let receipt = DurableReceipt {
                revision,
                protected_unix_ms: now_ms(),
            };
            let record = Record {
                version: VERSION,
                metadata,
                receipt,
                segment: Blob {
                    name,
                    len: size as u64,
                    sha256: hash.finalize().into(),
                },
                edits: refs,
            };
            let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
            if bytes.len() > MAX_RECORD
                || self.journal.metadata()?.len() + bytes.len() as u64 + 8 > MAX_JOURNAL
            {
                return Err(invalid("recovery journal quota"));
            }
            self.journal
                .write_all(&(bytes.len() as u32).to_le_bytes())?;
            self.journal.write_all(&crc32c(&bytes).to_le_bytes())?;
            self.journal.write_all(&bytes)?;
            faults.boundary(Boundary::JournalWritten)?;
            self.journal.sync_all()?;
            faults.boundary(Boundary::JournalFlushed)?;
            self.manifest.durable = Some(receipt);
            self.records += 1;
            self.current_len = next_len;
            Ok(receipt)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub fn checkpoint(&self, platform: &dyn LocalFileSystem) -> io::Result<()> {
        self.checkpoint_with_faults(platform, &mut NoFault)
    }
    pub fn checkpoint_with_faults(
        &self,
        platform: &dyn LocalFileSystem,
        faults: &mut dyn FaultInjector,
    ) -> io::Result<()> {
        self.ensure_writable()?;
        publish(&self.directory, &self.manifest, platform, faults)
    }
    fn ensure_writable(&self) -> io::Result<()> {
        if self.poisoned
            || self.manifest.retired
            || self.directory.join("retired.json").try_exists()?
        {
            Err(io::Error::other(
                "recovery unavailable; keep last durable receipt and create a new writer",
            ))
        } else {
            Ok(())
        }
    }
}
fn publish(
    directory: &Path,
    manifest: &Manifest,
    platform: &dyn LocalFileSystem,
    faults: &mut dyn FaultInjector,
) -> io::Result<()> {
    let path = directory.join("manifest.json");
    if let Ok(previous) = read_manifest_file(&path) {
        publish_file(
            &directory.join("manifest.previous.json"),
            &previous,
            platform,
            &mut NoFault,
        )?;
    }
    publish_file(&path, manifest, platform, faults)
}
fn publish_file(
    path: &Path,
    manifest: &Manifest,
    platform: &dyn LocalFileSystem,
    faults: &mut dyn FaultInjector,
) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut staged_name = path.as_os_str().to_os_string();
    staged_name.push(format!(
        ".{}-{}.pending",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let staged = PathBuf::from(staged_name);
    platform.validate_target(path)?;
    let bytes = serde_json::to_vec(manifest).map_err(io::Error::other)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staged)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        faults.boundary(Boundary::CheckpointWritten)?;
        platform.commit(&staged, path, path.try_exists()?)?;
        faults.boundary(Boundary::CheckpointCommitted)
    })();
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}
fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f63b78 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
fn read_manifest(directory: &Path) -> io::Result<Manifest> {
    match read_manifest_file(&directory.join("manifest.json")) {
        Ok(manifest) => Ok(manifest),
        Err(primary) => {
            read_manifest_file(&directory.join("manifest.previous.json")).map_err(|_| primary)
        }
    }
}
fn read_manifest_file(path: &Path) -> io::Result<Manifest> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(32 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 {
        return Err(invalid("recovery manifest limit"));
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if !matches!(manifest.version, 1 | VERSION)
        || manifest.metadata.source_generation.len() > 4096
        || manifest.metadata.codec_catalog_version.len() > 4096
    {
        return Err(invalid("unsupported recovery manifest"));
    }
    if manifest.baseline.as_ref().is_some_and(|blob| {
        blob.name != "baseline.bin" || blob.len != manifest.metadata.original_len
    }) {
        return Err(invalid("invalid baseline identity"));
    }
    Ok(manifest)
}
fn verify_blob(directory: &Path, blob: &Blob, cancel: &Cancellation) -> io::Result<()> {
    // All accepted names are checked by the caller before this join.
    if fs::symlink_metadata(directory.join(&blob.name))?
        .file_type()
        .is_symlink()
    {
        return Err(invalid("recovery segment cannot be a symlink"));
    }
    let mut file = File::open(directory.join(&blob.name))?;
    if file.metadata()?.len() != blob.len {
        return Err(invalid("recovery segment length mismatch"));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; CHUNK];
    loop {
        cancelled(cancel)?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    let actual: [u8; 32] = hash.finalize().into();
    if actual != blob.sha256 {
        return Err(invalid("recovery segment hash mismatch"));
    }
    Ok(())
}
struct Scan {
    manifest: Manifest,
    records: Vec<Record>,
    corrupt: bool,
    baseline_valid: bool,
}
fn scan(directory: &Path, cancel: &Cancellation) -> io::Result<Scan> {
    let mut manifest = read_manifest(directory)?;
    if directory.join("retired.json").try_exists()? {
        manifest.retired = true;
    }
    if manifest.retired {
        return Ok(Scan {
            manifest,
            records: vec![],
            corrupt: false,
            baseline_valid: false,
        });
    }
    let baseline_valid = match &manifest.baseline {
        Some(blob) => verify_blob(directory, blob, cancel).is_ok(),
        None => false,
    };
    cancelled(cancel)?;
    let mut journal = File::open(directory.join("journal.bin"))?;
    if journal.metadata()?.len() > MAX_JOURNAL {
        return Err(invalid("recovery journal limit"));
    }
    let mut records: Vec<Record> = Vec::new();
    let mut corrupt = false;
    let mut current_len = manifest.metadata.original_len;
    loop {
        cancelled(cancel)?;
        let mut header = [0u8; 8];
        let n = journal.read(&mut header[..1])?;
        if n == 0 {
            break;
        }
        if records.len() >= MAX_RECORDS || journal.read_exact(&mut header[1..]).is_err() {
            corrupt = true;
            break;
        }
        let len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        if len == 0 || len > MAX_RECORD {
            corrupt = true;
            break;
        }
        let mut bytes = vec![0u8; len];
        if journal.read_exact(&mut bytes).is_err()
            || crc32c(&bytes) != u32::from_le_bytes(header[4..].try_into().unwrap())
        {
            corrupt = true;
            break;
        }
        let Ok(record) = serde_json::from_slice::<Record>(&bytes) else {
            corrupt = true;
            break;
        };
        let sum = record.edits.iter().try_fold(0u64, |n, e| {
            n.checked_add(e.removed)?.checked_add(e.inserted)
        });
        if !matches!(record.version, 1 | VERSION | 3)
            || (record.version == 1 && record.metadata.is_some())
            || (record.edits.is_empty() && record.metadata.is_none())
            || record.metadata.as_ref().is_some_and(|metadata| {
                bareline_document::DocumentMetadata::new(metadata.clone()).is_err()
            })
            || record.edits.len() > 4096
            || (record.version != 3 && record.segment.len > MAX_TRANSACTION as u64)
            || sum != Some(record.segment.len)
            || record.segment.name != format!("segment-{}.bin", record.receipt.revision)
            || records
                .last()
                .is_some_and(|r| r.receipt.revision >= record.receipt.revision)
            || verify_blob(directory, &record.segment, cancel).is_err()
        {
            corrupt = true;
            break;
        }
        cancelled(cancel)?;
        match edited_len(current_len, &record.edits) {
            Ok(len) => current_len = len,
            Err(_) => {
                corrupt = true;
                break;
            }
        }
        records.push(record);
    }
    cancelled(cancel)?;
    if manifest.durable.is_some_and(|receipt| {
        records
            .last()
            .is_none_or(|record| record.receipt.revision < receipt.revision)
    }) {
        corrupt = true;
    }
    if baseline_valid {
        let valid = validate_inverse_prefix(directory, &records, cancel)?;
        if valid < records.len() {
            records.truncate(valid);
            corrupt = true;
        }
    }
    Ok(Scan {
        manifest,
        records,
        corrupt,
        baseline_valid,
    })
}
fn edited_len(source_len: u64, edits: &[EditRef]) -> io::Result<u64> {
    let mut cursor = 0;
    let mut result = source_len;
    for edit in edits {
        let end = edit
            .offset
            .checked_add(edit.removed)
            .filter(|end| *end <= source_len)
            .ok_or_else(|| invalid("recovery edit outside source"))?;
        if edit.offset < cursor {
            return Err(invalid("overlapping recovery edits"));
        }
        cursor = end;
        result = result
            .checked_sub(edit.removed)
            .and_then(|n| n.checked_add(edit.inserted))
            .ok_or_else(|| invalid("recovery length overflow"))?;
    }
    Ok(result)
}
impl Scan {
    fn inspection(&self) -> RecoveryInspection {
        let status = if self.manifest.retired {
            RecoveryStatus::Discarded
        } else if self.corrupt {
            RecoveryStatus::CorruptTail
        } else if self.baseline_valid {
            RecoveryStatus::Complete
        } else if self.manifest.baseline.is_some() {
            RecoveryStatus::SourceUnavailable
        } else {
            RecoveryStatus::EditsOnly
        };
        RecoveryInspection {
            status,
            metadata: self.manifest.metadata.clone(),
            last_durable: self.records.last().map(|r| r.receipt),
            checkpoint_durable: self.manifest.durable,
            validated_records: self.records.len(),
            complete_baseline: self.baseline_valid,
            document_metadata: self
                .records
                .iter()
                .rev()
                .find_map(|record| record.metadata.clone()),
        }
    }
}
pub fn inspect(directory: &Path, cancel: &Cancellation) -> io::Result<RecoveryInspection> {
    Ok(scan(directory, cancel)?.inspection())
}
/// Visit the validated journal prefix one transaction at a time. Payload decoding
/// temporarily holds at most 32 MiB (16 MiB segment plus owned edit bytes), plus
/// bounded journal metadata; callers control any payload retention.
/// Without a sealed baseline inverse applicability must be checked by the caller.
/// This never resumes or mutates the existing journal.
pub fn replay_transactions(
    directory: &Path,
    cancel: &Cancellation,
    mut visit: impl FnMut(DurableReceipt, Vec<RecoveryEdit>) -> io::Result<()>,
) -> io::Result<RecoveryInspection> {
    let scanned = scan(directory, cancel)?;
    for record in &scanned.records {
        cancelled(cancel)?;
        let mut segment = File::open(directory.join(&record.segment.name))?;
        if record.segment.len > MAX_TRANSACTION as u64 { return Err(invalid("streaming transaction requires source-range replay")); }
        let mut bytes = vec![0; record.segment.len as usize];
        segment.read_exact(&mut bytes)?;
        if <[u8; 32]>::from(Sha256::digest(&bytes)) != record.segment.sha256 {
            return Err(invalid("recovery segment changed during replay"));
        }
        let mut cursor = 0;
        let mut edits = Vec::with_capacity(record.edits.len());
        for edit in &record.edits {
            let removed_end = cursor + edit.removed as usize;
            let inserted_end = removed_end + edit.inserted as usize;
            edits.push(RecoveryEdit {
                offset: edit.offset,
                removed: bytes[cursor..removed_end].to_vec(),
                inserted: bytes[removed_end..inserted_end].to_vec(),
            });
            cursor = inserted_end;
        }
        drop(bytes);
        visit(record.receipt, edits)?;
    }
    Ok(scanned.inspection())
}

fn validate_inverse_prefix(
    directory: &Path,
    records: &[Record],
    cancel: &Cancellation,
) -> io::Result<usize> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if records.is_empty() {
        return Ok(0);
    }
    let scratch = directory.join(format!(
        "validate-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&scratch)?;
    let result = (|| {
        let mut current = File::open(directory.join("baseline.bin"))?;
        for (index, record) in records.iter().enumerate() {
            let mut next = OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(scratch.join(if index % 2 == 0 { "a" } else { "b" }))?;
            if let Err(error) = apply_record(directory, record, &mut current, &mut next, cancel) {
                if matches!(
                    error.kind(),
                    io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
                ) {
                    return Ok(index);
                }
                return Err(error);
            }
            next.seek(SeekFrom::Start(0))?;
            current = next;
        }
        Ok(records.len())
    })();
    let _ = fs::remove_file(scratch.join("a"));
    let _ = fs::remove_file(scratch.join("b"));
    let _ = fs::remove_dir(scratch);
    result
}

/// Reconstruct into a new path only. Existing destinations (including the original)
/// are refused. Corrupt tails expose only the validated prefix with its warning status.
pub fn recover_to(
    directory: &Path,
    destination: &Path,
    cancel: &Cancellation,
) -> io::Result<RecoveryInspection> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let scanned = scan(directory, cancel)?;
    if !scanned.baseline_valid || scanned.manifest.retired {
        return Err(invalid("complete recovery baseline unavailable"));
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let scratch = directory.join(format!(
        "replay-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(error) = fs::create_dir(&scratch) {
        drop(output);
        let _ = fs::remove_file(destination);
        return Err(error);
    }
    let result = (|| {
        let mut current = File::open(directory.join("baseline.bin"))?;
        for (index, record) in scanned.records.iter().enumerate() {
            cancelled(cancel)?;
            let target_path = scratch.join(if index % 2 == 0 { "a" } else { "b" });
            let mut next = OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(target_path)?;
            apply_record(directory, record, &mut current, &mut next, cancel)?;
            next.seek(SeekFrom::Start(0))?;
            current = next;
        }
        current.seek(SeekFrom::Start(0))?;
        copy_all(&mut current, &mut output, cancel)?;
        output.sync_all()?;
        Ok(scanned.inspection())
    })();
    drop(output);
    // Only these files were created inside our exclusively created scratch directory.
    let _ = fs::remove_file(scratch.join("a"));
    let _ = fs::remove_file(scratch.join("b"));
    let _ = fs::remove_dir(scratch);
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}
fn copy_all(
    source: &mut impl Read,
    target: &mut impl Write,
    cancel: &Cancellation,
) -> io::Result<()> {
    let mut buffer = [0u8; CHUNK];
    loop {
        cancelled(cancel)?;
        let n = source.read(&mut buffer)?;
        if n == 0 {
            return Ok(());
        }
        target.write_all(&buffer[..n])?;
    }
}
fn copy_exact(
    source: &mut impl Read,
    target: &mut impl Write,
    mut count: u64,
    cancel: &Cancellation,
) -> io::Result<()> {
    let mut buffer = [0u8; CHUNK];
    while count != 0 {
        cancelled(cancel)?;
        let n = count.min(CHUNK as u64) as usize;
        source.read_exact(&mut buffer[..n])?;
        target.write_all(&buffer[..n])?;
        count -= n as u64;
    }
    Ok(())
}
fn apply_record(
    directory: &Path,
    record: &Record,
    source: &mut File,
    target: &mut File,
    cancel: &Cancellation,
) -> io::Result<()> {
    source.seek(SeekFrom::Start(0))?;
    let source_len = source.metadata()?.len();
    let mut segment = File::open(directory.join(&record.segment.name))?;
    let mut cursor = 0u64;
    for edit in &record.edits {
        if edit.offset < cursor
            || edit
                .offset
                .checked_add(edit.removed)
                .is_none_or(|end| end > source_len)
        {
            return Err(invalid("recovery edit outside source"));
        }
        copy_exact(source, target, edit.offset - cursor, cancel)?;
        // Check inverse bytes as well as segment hashes: applying to the wrong baseline fails closed.
        let mut remaining = edit.removed;
        let mut before = [0u8; CHUNK];
        let mut inverse = [0u8; CHUNK];
        while remaining != 0 {
            cancelled(cancel)?;
            let n = remaining.min(CHUNK as u64) as usize;
            source.read_exact(&mut before[..n])?;
            segment.read_exact(&mut inverse[..n])?;
            if before[..n] != inverse[..n] {
                return Err(invalid("recovery inverse does not match baseline"));
            }
            remaining -= n as u64;
        }
        copy_exact(&mut segment, target, edit.inserted, cancel)?;
        cursor = edit.offset + edit.removed;
    }
    copy_exact(source, target, source_len - cursor, cancel)
}

#[derive(Serialize)]
struct GapReport<'a> {
    version: u32,
    metadata: &'a RecoveryMetadata,
    complete: bool,
    unavailable_original: Option<(u64, u64)>,
    records: &'a [Record],
}
/// Export owned transaction segments plus offsets and an explicit original-content gap.
/// The export is a directory, never a file masquerading as a complete reconstruction.
pub fn export_edits(
    directory: &Path,
    destination: &Path,
    cancel: &Cancellation,
) -> io::Result<RecoveryInspection> {
    let scanned = scan(directory, cancel)?;
    if scanned.manifest.retired {
        return Err(invalid("recovery discarded"));
    }
    fs::create_dir(destination)?;
    for record in &scanned.records {
        cancelled(cancel)?;
        let mut source = File::open(directory.join(&record.segment.name))?;
        let mut target = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination.join(&record.segment.name))?;
        copy_all(&mut source, &mut target, cancel)?;
        target.sync_all()?;
    }
    let report = GapReport {
        version: VERSION,
        metadata: &scanned.manifest.metadata,
        complete: false,
        unavailable_original: Some((0, scanned.manifest.metadata.original_len)),
        records: &scanned.records,
    };
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination.join("gaps.json"))?;
    serde_json::to_writer(&mut file, &report).map_err(io::Error::other)?;
    file.sync_all()?;
    Ok(scanned.inspection())
}
/// The UI must obtain named, irreversible-discard confirmation before calling this.
/// Publishes a durable tombstone. Garbage collection is deliberately separate: live
/// undo/checkpoint references must be traced before any segment is deleted.
pub fn discard(directory: &Path, platform: &dyn LocalFileSystem) -> io::Result<()> {
    let mut manifest = read_manifest(directory)?;
    manifest.retired = true;
    publish_file(
        &directory.join("retired.json"),
        &manifest,
        platform,
        &mut NoFault,
    )
}

/// Trace result supplied by the owner of every live undo/checkpoint reference.
/// Garbage collection fails closed until that owner certifies the scan is complete.
pub struct LiveReferences {
    pub complete: bool,
    pub segment_names: std::collections::HashSet<String>,
}
/// Removes only retired, unreferenced segment files. Keeps the durable tombstone,
/// manifest and journal as retirement evidence. Never follows a symlink or recurses.
pub fn collect_retired(directory: &Path, references: &LiveReferences) -> io::Result<usize> {
    if !references.complete || !directory.join("retired.json").try_exists()? {
        return Err(invalid(
            "complete live-reference trace and durable retirement required",
        ));
    }
    // Validate the tombstone before deleting anything; mere marker existence is insufficient.
    if !read_manifest_file(&directory.join("retired.json"))?.retired {
        return Err(invalid("invalid retirement tombstone"));
    }
    let mut removed = 0;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let segment = name
            .strip_prefix("segment-")
            .and_then(|n| n.strip_suffix(".bin"))
            .is_some_and(|n| {
                !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) && n.parse::<u64>().is_ok()
            });
        if (name == "baseline.bin" || segment)
            && !references.segment_names.contains(name)
            && entry.file_type()?.is_file()
        {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Event-driven durability deadline accounting. No thread or timer is created here.
/// The app arms its existing event-loop deadline to `next_deadline` and submits a
/// journal batch when `due` becomes true. Storage failure retains the last receipt.
#[derive(Default)]
pub struct RecoverySchedule {
    pending: std::collections::VecDeque<(u64, usize, std::time::Instant)>,
    pub last_durable: Option<DurableReceipt>,
    pub unavailable: Option<String>,
}
impl RecoverySchedule {
    pub fn edited(
        &mut self,
        revision: u64,
        bytes: usize,
        now: std::time::Instant,
    ) -> io::Result<()> {
        if self.pending.len() >= 4096
            || self
                .pending
                .back()
                .is_some_and(|(previous, _, _)| revision <= *previous)
            || self.last_durable.is_some_and(|r| revision <= r.revision)
        {
            return Err(invalid("recovery schedule limit or nonmonotonic revision"));
        }
        self.pending.push_back((revision, bytes, now));
        Ok(())
    }
    pub fn next_deadline(&self) -> Option<std::time::Instant> {
        if self.unavailable.is_some() {
            return None;
        }
        self.pending
            .front()
            .and_then(|(_, _, time)| time.checked_add(std::time::Duration::from_secs(5)))
    }
    pub fn due(&self, now: std::time::Instant) -> bool {
        self.unavailable.is_none()
            && (self.next_deadline().is_some_and(|deadline| now >= deadline)
                || self
                    .pending
                    .iter()
                    .fold(0usize, |sum, (_, bytes, _)| sum.saturating_add(*bytes))
                    >= 256 * 1024)
    }
    pub fn protected(&mut self, receipt: DurableReceipt) {
        if self
            .last_durable
            .is_some_and(|last| receipt.revision < last.revision)
        {
            return;
        }
        self.last_durable = Some(receipt);
        self.unavailable = None;
        while self
            .pending
            .front()
            .is_some_and(|(revision, _, _)| *revision <= receipt.revision)
        {
            self.pending.pop_front();
        }
    }
    pub fn failed(&mut self, message: String) {
        self.unavailable = Some(message);
    }
    /// Explicit Retry re-arms any overdue batch without claiming storage succeeded.
    pub fn retry(&mut self) {
        self.unavailable = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::FileIdentity;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct FakeFs;
    impl LocalFileSystem for FakeFs {
        fn identity(&self, _: &File) -> io::Result<FileIdentity> {
            Err(io::Error::other("unused"))
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
            fs::rename(staged, target)
        }
    }
    struct FailAt(Boundary);
    impl FaultInjector for FailAt {
        fn boundary(&mut self, at: Boundary) -> io::Result<()> {
            if at == self.0 {
                Err(io::Error::other("injected interruption"))
            } else {
                Ok(())
            }
        }
    }
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "bareline-recovery-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn writer(temp: &Temp, baseline: bool) -> RecoveryWriter {
        let mut writer = RecoveryWriter::create(
            &temp.0.join("item"),
            RecoveryMetadata {
                original_path: None,
                source_generation: "full-sha256-test".into(),
                codec_catalog_version: "utf8-v1".into(),
                original_len: 5,
            },
            &FakeFs,
        )
        .unwrap();
        if baseline {
            writer
                .seal_baseline(
                    &mut &b"hello"[..],
                    || Ok(true),
                    &Cancellation::default(),
                    &FakeFs,
                )
                .unwrap();
        }
        writer
    }
    fn edit() -> RecoveryEdit {
        RecoveryEdit {
            offset: 1,
            removed: b"ell".to_vec(),
            inserted: b"ipp".to_vec(),
        }
    }
    #[test]
    fn prepared_baseline_preserves_concurrent_receipts_and_rejects_other_writer() {
        let temp = Temp::new();
        let mut owner = writer(&temp, false);
        let job = owner.prepare_baseline().unwrap();
        let receipt = owner.append(1, &[edit()]).unwrap();
        let prepared = job
            .copy(&mut &b"hello"[..], || Ok(true), &Cancellation::default())
            .unwrap();
        owner.attach_baseline(prepared, &FakeFs).unwrap();
        assert_eq!(owner.last_durable(), Some(receipt));
        let mut seen = Vec::new();
        let inspection = replay_transactions(
            &temp.0.join("item"),
            &Cancellation::default(),
            |r, edits| {
                seen.push((r, edits[0].inserted.clone()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(inspection.status, RecoveryStatus::Complete);
        assert_eq!(seen, vec![(receipt, b"ipp".to_vec())]);
        let other_temp = Temp::new();
        let mut other = writer(&other_temp, false);
        let third_temp = Temp::new();
        let third = writer(&third_temp, false);
        let prepared = third
            .prepare_baseline()
            .unwrap()
            .copy(&mut &b"hello"[..], || Ok(true), &Cancellation::default())
            .unwrap();
        assert!(other.attach_baseline(prepared, &FakeFs).is_err());
        assert!(!third_temp.0.join("item/baseline.bin").exists());
        assert_eq!(
            inspect(&other_temp.0.join("item"), &Cancellation::default())
                .unwrap()
                .status,
            RecoveryStatus::EditsOnly
        );
    }
    #[test]
    fn crc32c_known_vector() {
        assert_eq!(crc32c(b"123456789"), 0xe3069283);
    }
    #[test]
    fn sealed_recovery_is_independent_and_never_overwrites_destination() {
        let temp = Temp::new();
        let mut writer = writer(&temp, true);
        let receipt = writer.append(1, &[edit()]).unwrap();
        writer.checkpoint(&FakeFs).unwrap();
        let destination = temp.0.join("copy");
        let inspection =
            recover_to(&temp.0.join("item"), &destination, &Cancellation::default()).unwrap();
        assert_eq!(inspection.status, RecoveryStatus::Complete);
        assert_eq!(inspection.last_durable, Some(receipt));
        assert_eq!(fs::read(&destination).unwrap(), b"hippo");
        assert!(recover_to(&temp.0.join("item"), &destination, &Cancellation::default()).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"hippo");
    }
    #[test]
    fn interruptions_never_acknowledge_unflushed_revision_and_prefix_remains_recoverable() {
        for boundary in [
            Boundary::SegmentsWritten,
            Boundary::SegmentsFlushed,
            Boundary::JournalWritten,
            Boundary::JournalFlushed,
        ] {
            let temp = Temp::new();
            let mut writer = writer(&temp, true);
            assert!(
                writer
                    .append_with_faults(1, &[edit()], &mut FailAt(boundary))
                    .is_err()
            );
            assert_eq!(writer.last_durable(), None);
            assert!(writer.append(2, &[edit()]).is_err());
            drop(writer);
            let recovered = inspect(&temp.0.join("item"), &Cancellation::default()).unwrap();
            let expected = usize::from(matches!(
                boundary,
                Boundary::JournalWritten | Boundary::JournalFlushed
            ));
            // Written-but-unacknowledged bytes may survive a process death; they are only replayed after CRC/hash verification.
            assert_eq!(recovered.validated_records, expected);
        }
    }
    #[test]
    fn checkpoint_interruptions_keep_committed_references_and_previous_manifest() {
        for boundary in [Boundary::CheckpointWritten, Boundary::CheckpointCommitted] {
            let temp = Temp::new();
            let mut writer = writer(&temp, true);
            writer.append(1, &[edit()]).unwrap();
            assert!(
                writer
                    .checkpoint_with_faults(&FakeFs, &mut FailAt(boundary))
                    .is_err()
            );
            assert_eq!(
                inspect(&temp.0.join("item"), &Cancellation::default())
                    .unwrap()
                    .validated_records,
                1
            );
            fs::write(temp.0.join("item/manifest.json"), b"bad").unwrap();
            assert!(
                inspect(&temp.0.join("item"), &Cancellation::default())
                    .unwrap()
                    .complete_baseline
            );
        }
    }
    #[test]
    fn corrupt_tail_replays_valid_prefix_and_missing_segments_are_not_claimed() {
        let temp = Temp::new();
        let mut writer = writer(&temp, true);
        writer.append(1, &[edit()]).unwrap();
        drop(writer);
        let mut journal = OpenOptions::new()
            .append(true)
            .open(temp.0.join("item/journal.bin"))
            .unwrap();
        journal.write_all(&[1, 2, 3]).unwrap();
        drop(journal);
        let result = recover_to(
            &temp.0.join("item"),
            &temp.0.join("copy"),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(result.status, RecoveryStatus::CorruptTail);
        assert_eq!(fs::read(temp.0.join("copy")).unwrap(), b"hippo");
        fs::remove_file(temp.0.join("item/segment-1.bin")).unwrap();
        let result = inspect(&temp.0.join("item"), &Cancellation::default()).unwrap();
        assert_eq!(result.validated_records, 0);
        assert_eq!(result.status, RecoveryStatus::CorruptTail);
    }
    #[test]
    fn changed_baseline_and_cancellation_leave_honest_edits_only_export() {
        let temp = Temp::new();
        let mut writer = writer(&temp, false);
        assert!(
            writer
                .seal_baseline(
                    &mut &b"hello"[..],
                    || Ok(false),
                    &Cancellation::default(),
                    &FakeFs
                )
                .is_err()
        );
        writer.append(1, &[edit()]).unwrap();
        assert_eq!(
            inspect(&temp.0.join("item"), &Cancellation::default())
                .unwrap()
                .status,
            RecoveryStatus::EditsOnly
        );
        assert!(
            recover_to(
                &temp.0.join("item"),
                &temp.0.join("copy"),
                &Cancellation::default()
            )
            .is_err()
        );
        export_edits(
            &temp.0.join("item"),
            &temp.0.join("export"),
            &Cancellation::default(),
        )
        .unwrap();
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.0.join("export/gaps.json")).unwrap()).unwrap();
        assert_eq!(report["complete"], false);
        assert_eq!(report["unavailable_original"], serde_json::json!([0, 5]));
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(inspect(&temp.0.join("item"), &cancel).is_err());
    }
    #[test]
    fn discard_is_durable_and_prevents_live_writer_and_recovery() {
        let temp = Temp::new();
        let mut writer = writer(&temp, true);
        writer.append(1, &[edit()]).unwrap();
        discard(&temp.0.join("item"), &FakeFs).unwrap();
        assert!(writer.append(2, &[edit()]).is_err());
        assert_eq!(
            inspect(&temp.0.join("item"), &Cancellation::default())
                .unwrap()
                .status,
            RecoveryStatus::Discarded
        );
        assert!(temp.0.join("item/segment-1.bin").exists());
    }
    #[test]
    fn corrupt_baseline_and_inverse_fail_without_leaving_complete_copy() {
        let temp = Temp::new();
        let mut writer = writer(&temp, true);
        writer
            .append(
                1,
                &[RecoveryEdit {
                    offset: 1,
                    removed: b"bad".to_vec(),
                    inserted: b"ipp".to_vec(),
                }],
            )
            .unwrap();
        let destination = temp.0.join("copy");
        let inspection =
            recover_to(&temp.0.join("item"), &destination, &Cancellation::default()).unwrap();
        assert_eq!(inspection.status, RecoveryStatus::CorruptTail);
        assert_eq!(inspection.validated_records, 0);
        assert_eq!(fs::read(&destination).unwrap(), b"hello");
        fs::write(temp.0.join("item/baseline.bin"), b"wrong").unwrap();
        assert_eq!(
            inspect(&temp.0.join("item"), &Cancellation::default())
                .unwrap()
                .status,
            RecoveryStatus::SourceUnavailable
        );
    }
    #[test]
    fn checkpoint_watermark_detects_empty_and_clean_boundary_truncation() {
        for keep_first in [false, true] {
            let temp = Temp::new();
            let mut writer = writer(&temp, true);
            writer.append(1, &[edit()]).unwrap();
            let first_len = writer.journal.metadata().unwrap().len();
            writer
                .append(
                    2,
                    &[RecoveryEdit {
                        offset: 0,
                        removed: b"h".to_vec(),
                        inserted: b"H".to_vec(),
                    }],
                )
                .unwrap();
            writer.checkpoint(&FakeFs).unwrap();
            drop(writer);
            OpenOptions::new()
                .write(true)
                .open(temp.0.join("item/journal.bin"))
                .unwrap()
                .set_len(if keep_first { first_len } else { 0 })
                .unwrap();
            let inspection = inspect(&temp.0.join("item"), &Cancellation::default()).unwrap();
            assert_eq!(inspection.status, RecoveryStatus::CorruptTail);
            assert_eq!(inspection.validated_records, usize::from(keep_first));
        }
    }
    #[test]
    fn duplicate_writer_and_invalid_offsets_are_refused() {
        let temp = Temp::new();
        let mut writer = writer(&temp, true);
        assert!(
            RecoveryWriter::create(
                &temp.0.join("item"),
                writer.manifest.metadata.clone(),
                &FakeFs
            )
            .is_err()
        );
        assert!(
            writer
                .append(
                    1,
                    &[RecoveryEdit {
                        offset: 50,
                        removed: vec![],
                        inserted: vec![1]
                    }]
                )
                .is_err()
        );
        assert_eq!(writer.last_durable(), None);
    }
    #[test]
    fn schedule_uses_first_edit_deadline_and_preserves_newer_edits_on_receipt() {
        let start = std::time::Instant::now();
        let mut schedule = RecoverySchedule::default();
        schedule.edited(1, 12, start).unwrap();
        assert!(!schedule.due(start + std::time::Duration::from_secs(4)));
        assert!(schedule.due(start + std::time::Duration::from_secs(5)));
        schedule
            .edited(2, 256 * 1024, start + std::time::Duration::from_secs(1))
            .unwrap();
        let receipt = DurableReceipt {
            revision: 1,
            protected_unix_ms: 20,
        };
        schedule.protected(receipt);
        assert!(schedule.due(start + std::time::Duration::from_secs(1)));
        schedule.failed("Disk full".into());
        assert_eq!(schedule.last_durable, Some(receipt));
        assert_eq!(schedule.unavailable.as_deref(), Some("Disk full"));
        assert!(!schedule.due(start + std::time::Duration::from_secs(10)));
        assert_eq!(schedule.next_deadline(), None);
        schedule.retry();
        assert!(schedule.due(start + std::time::Duration::from_secs(10)));
        schedule.protected(DurableReceipt {
            revision: 2,
            protected_unix_ms: 21,
        });
        assert!(!schedule.due(start + std::time::Duration::from_secs(10)));
    }
    #[test]
    fn garbage_collection_requires_tombstone_and_preserves_traced_live_segments() {
        let temp = Temp::new();
        let mut writer = writer(&temp, true);
        writer.append(1, &[edit()]).unwrap();
        drop(writer);
        let directory = temp.0.join("item");
        let mut references = LiveReferences {
            complete: true,
            segment_names: ["segment-1.bin".to_string()].into_iter().collect(),
        };
        assert!(collect_retired(&directory, &references).is_err());
        discard(&directory, &FakeFs).unwrap();
        assert_eq!(collect_retired(&directory, &references).unwrap(), 1);
        assert!(directory.join("segment-1.bin").exists());
        references.complete = false;
        assert!(collect_retired(&directory, &references).is_err());
        references.complete = true;
        references.segment_names.clear();
        assert_eq!(collect_retired(&directory, &references).unwrap(), 1);
        assert!(directory.join("retired.json").exists());
    }
    #[test]
    fn stale_checkpoint_stage_does_not_block_durable_discard() {
        let temp = Temp::new();
        let writer = writer(&temp, true);
        drop(writer);
        fs::write(
            temp.0.join("item/retired.pending"),
            b"interrupted prior process",
        )
        .unwrap();
        discard(&temp.0.join("item"), &FakeFs).unwrap();
        assert_eq!(
            inspect(&temp.0.join("item"), &Cancellation::default())
                .unwrap()
                .status,
            RecoveryStatus::Discarded
        );
    }
    #[test]
    fn baseline_read_failure_never_publishes_a_partial_seal() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "source unavailable",
                ))
            }
        }
        let temp = Temp::new();
        let mut writer = writer(&temp, false);
        writer.append(1, &[edit()]).unwrap();
        let receipt = writer.last_durable();
        assert!(
            writer
                .seal_baseline(&mut Broken, || Ok(true), &Cancellation::default(), &FakeFs)
                .is_err()
        );
        assert_eq!(writer.last_durable(), receipt);
        assert!(!temp.0.join("item/baseline.bin").exists());
        assert_eq!(
            inspect(&temp.0.join("item"), &Cancellation::default())
                .unwrap()
                .status,
            RecoveryStatus::EditsOnly
        );
    }
}
