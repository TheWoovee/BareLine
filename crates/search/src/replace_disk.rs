// SPDX-License-Identifier: MPL-2.0
//! PR-026 closed UTF-8 files: reviewed exact edits, unique backups and durable receipts.
use super::*;
use bareline_document::Budget;
use bareline_file_io::{
    lifecycle::{
        DecodeOptions, Fingerprint, Opened, open_encoded_streaming, save_encoded_cancellable,
        save_utf8_cancellable,
    },
    session::publish_json,
};
use bareline_platform::{
    FileIdentity, LocalFileSystem, PathOperation, PathOrigin, PathTrustProvider, SerializedPath,
    TrustedRead,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, TryLockError},
};

const MAX_FILES: usize = 10_000;
const MAX_RECEIPT: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptFingerprint {
    pub volume: u64,
    pub file: u64,
    pub length: u64,
    pub modified: u64,
    pub sha256: [u8; 32],
}
impl From<&Fingerprint> for ReceiptFingerprint {
    fn from(f: &Fingerprint) -> Self {
        Self {
            volume: f.identity.volume,
            file: f.identity.file,
            length: f.identity.length,
            modified: f.identity.modified,
            sha256: f.sha256,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptState {
    Planned,
    Staged,
    Uncertain(String),
    Committed,
    ReconciledCommitted,
    RollbackStaged,
    RolledBack,
    Skipped(String),
    Failed(String),
    Conflict,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct FileReceipt {
    pub path: SerializedPath,
    pub original: ReceiptFingerprint,
    pub after_hash: Option<[u8; 32]>,
    pub backup: Option<SerializedPath>,
    pub matches: usize,
    pub state: ReceiptState,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ReplaceReceipt {
    pub version: u32,
    pub files: Vec<FileReceipt>,
}
pub struct DiskChange {
    pub range: Range<TextOffset>,
    pub before: String,
    pub after: String,
    pub included: bool,
    edit: Edit,
}
pub struct DiskPreviewFile {
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
    pub bom: bool,
    pub encoding: bareline_file_io::codecs::Encoding,
    pub eol: bareline_file_io::codecs::state::EolState,
    pub included: bool,
    pub changes: Vec<DiskChange>,
}
pub struct DiskReplacePreview {
    files: Vec<DiskPreviewFile>,
}
impl DiskReplacePreview {
    pub fn files(&self) -> &[DiskPreviewFile] {
        &self.files
    }
    pub fn set_file_included(&mut self, file: usize, included: bool) -> bool {
        if let Some(file) = self.files.get_mut(file) {
            file.included = included;
            true
        } else {
            false
        }
    }
    pub fn set_match_included(&mut self, file: usize, matched: usize, included: bool) -> bool {
        if let Some(change) = self
            .files
            .get_mut(file)
            .and_then(|file| file.changes.get_mut(matched))
        {
            change.included = included;
            true
        } else {
            false
        }
    }
    pub fn toggle_all(&mut self, included: bool) {
        for file in &mut self.files {
            file.included = included;
            for change in &mut file.changes {
                change.included = included;
            }
        }
    }
}
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "path is outside trusted replacement scope",
    )
}
fn approved(path: &Path, trust: &dyn PathTrustProvider, write: bool) -> io::Result<TrustedRead> {
    let guard = trust.open_read(path, PathOrigin::User)?;
    if guard.trust.traverses_reparse_point
        || !trust.permits(
            &guard.trust,
            if write {
                PathOperation::Write
            } else {
                PathOperation::Read
            },
        )
    {
        return Err(denied());
    }
    Ok(guard)
}
fn open(
    guard: TrustedRead,
    platform: &dyn LocalFileSystem,
    job: &SearchJob,
) -> io::Result<(Opened, Vec<File>)> {
    let ancestors = guard.ancestors;
    let expected = platform.identity(&guard.file)?;
    let opened = open_encoded_streaming(
        &guard.trust.canonical,
        platform,
        Budget::new(regex::SUBJECT_LIMIT * 8),
        Budget::new(MAX_RESULT_BYTES),
        &job.io_cancel,
        DecodeOptions {
            resident_max_bytes: regex::SUBJECT_LIMIT as u64,
            interpret: None,
        },
        |_| {},
    )
    .map_err(|error| io::Error::other(format!("{error:?}")))?;
    if opened.fingerprint.identity != expected || platform.identity(&guard.file)? != expected {
        return Err(io::Error::other("Source changed during trusted open"));
    }
    Ok((opened, ancestors))
}
/// Phase one is read-only and returns no whole-file storage. Unsupported codecs,
/// incomplete searches and changed files abort preview rather than propose unsafe edits.
pub fn preview_disk_files(
    paths: impl IntoIterator<Item = PathBuf>,
    query: &SearchQuery,
    replacement: &str,
    job: &SearchJob,
    trust: &dyn PathTrustProvider,
    platform: &dyn LocalFileSystem,
    ram_bytes: usize,
) -> io::Result<DiskReplacePreview> {
    if query.selection.is_some() || replacement.len() > MAX_PATTERN_BYTES {
        return Err(io::Error::other("invalid folder replacement options"));
    }
    let template = decode_replacement(replacement, query.mode)
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut remaining = ram_bytes.min(MAX_RESULT_BYTES);
    let mut files: Vec<DiskPreviewFile> = Vec::new();
    for (number, path) in paths.into_iter().enumerate() {
        if job.is_cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        if number >= MAX_FILES {
            return Err(io::Error::other("file count limit"));
        }
        let (opened, _guards) = open(approved(&path, trust, false)?, platform, job)?;
        if files.iter().any(|f| {
            f.fingerprint.identity.volume == opened.fingerprint.identity.volume
                && f.fingerprint.identity.file == opened.fingerprint.identity.file
        }) {
            return Err(io::Error::other("duplicate file identity"));
        }
        let snapshot = opened.document.snapshot();
        if snapshot
            .chunks(TextOffset(0)..TextOffset(snapshot.len()))
            .unwrap()
            .any(|c| c.as_bytes().contains(&0))
        {
            return Err(io::Error::other("binary source excluded"));
        }
        let mut scoped = query.clone();
        scoped.results_ram_bytes = remaining;
        let results = scan(&snapshot, &scoped, job, |_| {});
        if results.completeness() != Completeness::Complete {
            return Err(io::Error::other(
                "incomplete search cannot produce a replacement preview",
            ));
        }
        if results.is_empty() {
            continue;
        }
        let transaction = results
            .prepare_replace_scoped(&snapshot, &template, remaining, ReplaceScope::All, job)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        remaining = remaining
            .checked_sub(std::mem::size_of::<DiskPreviewFile>() + opened.path.as_os_str().len())
            .ok_or_else(|| io::Error::other("preview budget"))?;
        let mut changes = Vec::new();
        for edit in transaction.edits {
            let mut end = (edit.range.start.0 + 160).min(edit.range.end.0);
            while !snapshot.is_boundary(TextOffset(end)) {
                end -= 1;
            }
            let before = snapshot
                .read(edit.range.start..TextOffset(end), 160)
                .map_err(|e| io::Error::other(format!("{e:?}")))?;
            let mut end = edit.insert.len().min(160);
            while !edit.insert.is_char_boundary(end) {
                end -= 1;
            }
            let after = edit.insert[..end].to_owned();
            let used = std::mem::size_of::<DiskChange>()
                + before.capacity()
                + after.capacity()
                + edit.insert.capacity();
            remaining = remaining
                .checked_sub(used)
                .ok_or_else(|| io::Error::other("preview budget"))?;
            changes.reserve_exact(1);
            changes.push(DiskChange {
                range: edit.range.clone(),
                before,
                after,
                included: true,
                edit,
            });
        }
        files.reserve_exact(1);
        files.push(DiskPreviewFile {
            path: opened.path,
            fingerprint: opened.fingerprint,
            bom: opened.bom,
            encoding: opened
                .encoding
                .as_ref()
                .map(|encoding| encoding.original_encoding())
                .unwrap_or(bareline_file_io::codecs::Encoding::Utf8),
            eol: opened
                .encoding
                .as_ref()
                .map(|encoding| encoding.eol)
                .unwrap_or_default(),
            included: true,
            changes,
        });
    }
    Ok(DiskReplacePreview { files })
}

struct OpenEntry {
    active: Arc<AtomicBool>,
    path: PathBuf,
    volume: u64,
    file: u64,
}
#[derive(Clone, Default)]
pub struct OpenFileRegistry(Arc<Mutex<Vec<OpenEntry>>>);
pub struct OpenFileLease {
    active: Arc<AtomicBool>,
}
impl Drop for OpenFileLease {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}
impl OpenFileRegistry {
    /// Called before exposing a loaded file as an open document; Busy must defer that
    /// admission. Keep the lease for its whole open lifetime. Never blocks the UI.
    pub fn try_register(
        &self,
        canonical: PathBuf,
        identity: &FileIdentity,
    ) -> io::Result<OpenFileLease> {
        let mut entries = match self.0.try_lock() {
            Ok(entries) => entries,
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "replacement owns file admission",
                ));
            }
            Err(_) => return Err(io::Error::other("file admission unavailable")),
        };
        entries.retain(|entry| entry.active.load(Ordering::Acquire));
        if entries.len() >= 4096 {
            return Err(io::Error::other("open file admission limit"));
        }
        let active = Arc::new(AtomicBool::new(true));
        entries.push(OpenEntry {
            active: active.clone(),
            path: canonical,
            volume: identity.volume,
            file: identity.file,
        });
        Ok(OpenFileLease { active })
    }
}
#[derive(Clone, Copy)]
pub enum BackupPolicy {
    Required,
    DisabledForThisJob,
}
pub struct DiskReplaceOptions {
    pub receipt_directory: PathBuf,
    pub backup: BackupPolicy,
}
impl DiskReplaceOptions {
    pub fn new(receipt_directory: PathBuf) -> Self {
        Self {
            receipt_directory,
            backup: BackupPolicy::Required,
        }
    }
}
pub struct DiskApplySummary {
    pub receipt_path: PathBuf,
    pub receipt: ReplaceReceipt,
}
impl DiskApplySummary {
    pub fn changed_files(&self) -> usize {
        self.receipt
            .files
            .iter()
            .filter(|f| {
                matches!(
                    f.state,
                    ReceiptState::Committed | ReceiptState::ReconciledCommitted
                )
            })
            .count()
    }
    pub fn replaced_matches(&self) -> usize {
        self.receipt
            .files
            .iter()
            .filter(|f| {
                matches!(
                    f.state,
                    ReceiptState::Committed | ReceiptState::ReconciledCommitted
                )
            })
            .map(|f| f.matches)
            .sum()
    }
}
fn persist(
    path: &Path,
    receipt: &ReplaceReceipt,
    platform: &dyn LocalFileSystem,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(receipt).map_err(io::Error::other)?;
    if bytes.len() > MAX_RECEIPT {
        return Err(io::Error::other("receipt budget"));
    }
    publish_json(path, &bytes, platform)
}
fn new_job_directory(parent: &Path) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    for _ in 0..64 {
        let path = parent.join(format!(
            "replace-{}-{time}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("unique receipt directory unavailable"))
}
fn snapshot_hash(snapshot: &DocumentSnapshot, bom: bool) -> [u8; 32] {
    let mut hash = Sha256::new();
    if bom {
        hash.update([0xef, 0xbb, 0xbf]);
    }
    for chunk in snapshot
        .chunks(TextOffset(0)..TextOffset(snapshot.len()))
        .unwrap()
    {
        hash.update(chunk.as_bytes());
    }
    hash.finalize().into()
}
fn backup(
    path: &Path,
    target: &Path,
    expected: &Fingerprint,
    trust: &dyn PathTrustProvider,
    platform: &dyn LocalFileSystem,
    job: &SearchJob,
) -> io::Result<()> {
    let guard = approved(path, trust, false)?;
    let mut file = guard.file;
    let _ancestors = guard.ancestors;
    if platform.identity(&file)? != expected.identity {
        return Err(io::Error::other("changed before backup"));
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    let mut buffer = [0u8; 65536];
    let mut hash = Sha256::new();
    let mut total = 0usize;
    loop {
        if job.is_cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n;
        if total > regex::SUBJECT_LIMIT {
            return Err(io::Error::other("backup budget"));
        }
        hash.update(&buffer[..n]);
        output.write_all(&buffer[..n])?;
    }
    if <[u8; 32]>::from(hash.finalize()) != expected.sha256
        || platform.identity(&file)? != expected.identity
    {
        return Err(io::Error::other("changed during backup"));
    }
    output.sync_all()
}
/// Explicit phase two. Caller must share `open_files` with every open-document admission.
/// Every source is committed separately; receipts are flushed before advancing. Backups
/// are unique and retained. Any receipt publication failure stops immediately.
pub fn apply_disk_files(
    preview: DiskReplacePreview,
    options: &DiskReplaceOptions,
    open_files: &OpenFileRegistry,
    job: &SearchJob,
    trust: &dyn PathTrustProvider,
    platform: &dyn LocalFileSystem,
) -> io::Result<DiskApplySummary> {
    let directory_guard = approved(&options.receipt_directory, trust, true)?;
    let directory = new_job_directory(&directory_guard.trust.canonical)?;
    let receipt_path = directory.join("receipt.json");
    let selected: Vec<_> = preview
        .files
        .into_iter()
        .filter(|f| f.included && f.changes.iter().any(|c| c.included))
        .collect();
    let mut receipt = ReplaceReceipt {
        version: 1,
        files: selected
            .iter()
            .map(|f| FileReceipt {
                path: SerializedPath::from_native(&f.path),
                original: (&f.fingerprint).into(),
                after_hash: None,
                backup: None,
                matches: f.changes.iter().filter(|c| c.included).count(),
                state: ReceiptState::Planned,
            })
            .collect(),
    };
    persist(&receipt_path, &receipt, platform)?;
    for (index, file) in selected.into_iter().enumerate() {
        if job.is_cancelled() {
            for record in &mut receipt.files[index..] {
                record.state = ReceiptState::Skipped("Cancelled before commit".into());
            }
            persist(&receipt_path, &receipt, platform)?;
            break;
        }
        let admission = open_files
            .0
            .lock()
            .map_err(|_| io::Error::other("open-file registry unavailable"))?;
        if admission.iter().any(|entry| {
            entry.active.load(Ordering::Acquire)
                && (entry.path == file.path
                    || (entry.volume == file.fingerprint.identity.volume
                        && entry.file == file.fingerprint.identity.file))
        }) {
            receipt.files[index].state =
                ReceiptState::Skipped("File is open; review its document revision".into());
            persist(&receipt_path, &receipt, platform)?;
            continue;
        }
        let mut attempted_commit = false;
        let mut review_changed = false;
        let outcome = (|| -> io::Result<()> {
            let (mut opened, _ancestors) = open(approved(&file.path, trust, true)?, platform, job)?;
            if opened.fingerprint != file.fingerprint {
                review_changed = true;
                return Err(io::Error::other("Source changed; review again"));
            }
            let snapshot = opened.document.snapshot();
            let edits = file
                .changes
                .into_iter()
                .filter(|c| c.included)
                .map(|c| c.edit)
                .collect();
            opened
                .document
                .apply(EditTransaction {
                    base_revision: snapshot.revision,
                    edits,
                })
                .map_err(|e| io::Error::other(format!("{e:?}")))?;
            let after = opened.document.snapshot();
            receipt.files[index].after_hash = Some(if let Some(encoding) = &opened.encoding {
                struct HashOutput(Sha256);
                impl Write for HashOutput {
                    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                        self.0.update(bytes);
                        Ok(bytes.len())
                    }
                    fn flush(&mut self) -> io::Result<()> {
                        Ok(())
                    }
                }
                let mut output = HashOutput(Sha256::new());
                encoding
                    .write_snapshot(&after, encoding.original_encoding(), file.bom, &mut output)
                    .map_err(|error| io::Error::other(format!("{error:?}")))?;
                output.0.finalize().into()
            } else {
                snapshot_hash(&after, file.bom)
            });
            if matches!(options.backup, BackupPolicy::Required) {
                let target = directory.join(format!("original-{index}.bak"));
                backup(&file.path, &target, &file.fingerprint, trust, platform, job)?;
                receipt.files[index].backup = Some(SerializedPath::from_native(&target));
            }
            receipt.files[index].state = ReceiptState::Staged;
            persist(&receipt_path, &receipt, platform)?;
            attempted_commit = true;
            if let Some(encoding) = &opened.encoding {
                save_encoded_cancellable(
                    after,
                    &file.path,
                    Some(&file.fingerprint),
                    file.bom,
                    platform,
                    &job.io_cancel,
                    encoding,
                )
            } else {
                save_utf8_cancellable(
                    after,
                    &file.path,
                    Some(&file.fingerprint),
                    file.bom,
                    platform,
                    &job.io_cancel,
                )
            }
            .map_err(|error| io::Error::other(format!("{error:?}")))?;
            Ok(())
        })();
        receipt.files[index].state = match outcome {
            Ok(()) => ReceiptState::Committed,
            Err(error) if attempted_commit => ReceiptState::Uncertain(error.to_string()),
            Err(error) if review_changed => ReceiptState::Skipped(error.to_string()),
            Err(error) => ReceiptState::Failed(error.to_string()),
        };
        // A failed post-commit publication leaves Staged on disk with the new hash;
        // reconciliation recognizes that commit without ever replaying replacement.
        persist(&receipt_path, &receipt, platform).map_err(|error| {
            io::Error::other(format!(
                "Receipt publication failed; reconcile {}: {error}",
                receipt_path.display()
            ))
        })?;
        drop(admission);
    }
    Ok(DiskApplySummary {
        receipt_path,
        receipt,
    })
}
/// Restore only committed targets whose current full hash still equals this job's output.
/// A modified target or backup is a conflict, never an overwrite. Each rollback is atomic.
pub fn rollback_receipt(
    path: &Path,
    open_files: &OpenFileRegistry,
    job: &SearchJob,
    trust: &dyn PathTrustProvider,
    platform: &dyn LocalFileSystem,
) -> io::Result<ReplaceReceipt> {
    let mut receipt = reconcile_receipt(path, job, trust, platform)?;
    for index in 0..receipt.files.len() {
        if job.is_cancelled() {
            break;
        }
        if !matches!(
            receipt.files[index].state,
            ReceiptState::Committed | ReceiptState::ReconciledCommitted
        ) {
            continue;
        }
        let record = receipt.files[index].clone();
        let Some(backup_path) = &record.backup else {
            continue;
        };
        let target = record
            .path
            .to_native()
            .map_err(|error| io::Error::other(format!("{error:?}")))?;
        let backup_path = backup_path
            .to_native()
            .map_err(|error| io::Error::other(format!("{error:?}")))?;
        let admission = open_files
            .0
            .lock()
            .map_err(|_| io::Error::other("open-file registry unavailable"))?;
        let outcome = (|| -> io::Result<()> {
            let (current, _target_ancestors) =
                open(approved(&target, trust, true)?, platform, job)?;
            if admission.iter().any(|entry| {
                entry.active.load(Ordering::Acquire)
                    && (entry.path == target
                        || (entry.volume == current.fingerprint.identity.volume
                            && entry.file == current.fingerprint.identity.file))
            }) {
                return Err(io::Error::other("Target is open; close it before rollback"));
            }
            if record.after_hash != Some(current.fingerprint.sha256) {
                return Err(io::Error::other("Target changed since replacement"));
            }
            let (original, _backup_ancestors) =
                open(approved(&backup_path, trust, false)?, platform, job)?;
            if original.fingerprint.sha256 != record.original.sha256 {
                return Err(io::Error::other("Backup fingerprint changed"));
            }
            receipt.files[index].state = ReceiptState::RollbackStaged;
            persist(path, &receipt, platform)?;
            if let Some(encoding) = &original.encoding {
                save_encoded_cancellable(
                    original.document.snapshot(),
                    &target,
                    Some(&current.fingerprint),
                    original.bom,
                    platform,
                    &job.io_cancel,
                    encoding,
                )
            } else {
                save_utf8_cancellable(
                    original.document.snapshot(),
                    &target,
                    Some(&current.fingerprint),
                    original.bom,
                    platform,
                    &job.io_cancel,
                )
            }
            .map_err(|error| io::Error::other(format!("{error:?}")))?;
            Ok(())
        })();
        drop(admission);
        match outcome {
            Ok(()) => receipt.files[index].state = ReceiptState::RolledBack,
            Err(error) if receipt.files[index].state == ReceiptState::RollbackStaged => {
                // Keep the durable intent: a restart compares original/output fingerprints.
                persist(path, &receipt, platform)?;
                return Err(error);
            }
            Err(_) => receipt.files[index].state = ReceiptState::Conflict,
        }
        persist(path, &receipt, platform)?;
    }
    Ok(receipt)
}
/// Recovery only classifies uncertain Planned/Staged records. It never reapplies edits.
pub fn reconcile_receipt(
    path: &Path,
    job: &SearchJob,
    trust: &dyn PathTrustProvider,
    platform: &dyn LocalFileSystem,
) -> io::Result<ReplaceReceipt> {
    let guard = approved(path, trust, true)?;
    let mut bytes = Vec::new();
    guard
        .file
        .take((MAX_RECEIPT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_RECEIPT {
        return Err(io::Error::other("receipt budget"));
    }
    let mut receipt: ReplaceReceipt = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if receipt.version != 1 || receipt.files.len() > MAX_FILES {
        return Err(io::Error::other("unsupported receipt"));
    }
    for record in &mut receipt.files {
        if !matches!(
            record.state,
            ReceiptState::Planned
                | ReceiptState::Staged
                | ReceiptState::Uncertain(_)
                | ReceiptState::RollbackStaged
        ) {
            continue;
        }
        if job.is_cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        let target = record
            .path
            .to_native()
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        let rolling_back = record.state == ReceiptState::RollbackStaged;
        match approved(&target, trust, false).and_then(|guard| open(guard, platform, job)) {
            Ok((opened, _guards))
                if rolling_back && record.original.sha256 == opened.fingerprint.sha256 =>
            {
                record.state = ReceiptState::RolledBack
            }
            Ok((opened, _guards)) if record.after_hash == Some(opened.fingerprint.sha256) => {
                record.state = ReceiptState::ReconciledCommitted
            }
            Ok((opened, _guards)) if record.original.sha256 == opened.fingerprint.sha256 => {
                record.state =
                    ReceiptState::Skipped("Interrupted before commit; no automatic retry".into())
            }
            Ok(_) => record.state = ReceiptState::Conflict,
            Err(error) => record.state = ReceiptState::Failed(error.to_string()),
        }
    }
    persist(path, &receipt, platform)?;
    Ok(receipt)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use bareline_platform_windows::{WindowsFileSystem, WindowsPathTrustProvider};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let path = std::env::temp_dir().join(format!(
                "bareline-replace-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::create_dir(path.join("receipts")).unwrap();
            Self(path)
        }
        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, bytes).unwrap();
            path
        }
        fn options(&self) -> DiskReplaceOptions {
            DiskReplaceOptions::new(self.0.join("receipts"))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn preview(paths: Vec<PathBuf>, job: &SearchJob) -> DiskReplacePreview {
        preview_disk_files(
            paths,
            &SearchQuery::literal("x"),
            "Y",
            job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
            MAX_RESULT_BYTES,
        )
        .unwrap()
    }
    #[test]
    fn reviewed_selection_preserves_bom_eol_and_unique_original_backup() {
        let fixture = Fixture::new();
        let original = b"\xef\xbb\xbfx\r\nx\n";
        let path = fixture.file("one.txt", original);
        let job = SearchJob::default();
        let mut preview = preview(vec![path.clone()], &job);
        preview.set_match_included(0, 1, false);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(preview.files()[0].changes[0].before, "x");
        let summary = apply_disk_files(
            preview,
            &fixture.options(),
            &OpenFileRegistry::default(),
            &job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
        )
        .unwrap();
        assert_eq!(summary.changed_files(), 1);
        assert_eq!(summary.replaced_matches(), 1);
        assert_eq!(fs::read(&path).unwrap(), b"\xef\xbb\xbfY\r\nx\n");
        let backup = summary.receipt.files[0]
            .backup
            .as_ref()
            .unwrap()
            .to_native()
            .unwrap();
        assert_eq!(fs::read(backup).unwrap(), original);
        let receipt: ReplaceReceipt =
            serde_json::from_slice(&fs::read(summary.receipt_path).unwrap()).unwrap();
        assert_eq!(receipt.files[0].state, ReceiptState::Committed);
    }
    #[test]
    fn encoded_replace_and_backup_rollback_preserve_utf16_bytes() {
        let fixture = Fixture::new();
        let original = b"\xff\xfex\0\r\0\n\0";
        let path = fixture.file("utf16.txt", original);
        let job = SearchJob::default();
        let registry = OpenFileRegistry::default();
        let summary = apply_disk_files(
            preview(vec![path.clone()], &job),
            &fixture.options(),
            &registry,
            &job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"\xff\xfeY\0\r\0\n\0");
        let rolled = rollback_receipt(
            &summary.receipt_path,
            &registry,
            &job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
        )
        .unwrap();
        assert_eq!(rolled.files[0].state, ReceiptState::RolledBack);
        assert_eq!(fs::read(&path).unwrap(), original);
        let again = rollback_receipt(
            &summary.receipt_path,
            &registry,
            &job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
        )
        .unwrap();
        assert_eq!(again.files[0].state, ReceiptState::RolledBack);
        let summary = apply_disk_files(
            preview(vec![path.clone()], &job),
            &fixture.options(),
            &registry,
            &job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
        )
        .unwrap();
        fs::write(&path, b"external").unwrap();
        let conflict = rollback_receipt(
            &summary.receipt_path,
            &registry,
            &job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
        )
        .unwrap();
        assert_eq!(conflict.files[0].state, ReceiptState::Conflict);
        assert_eq!(fs::read(&path).unwrap(), b"external");
    }
    #[test]
    fn changed_file_and_new_open_document_are_skipped_for_review() {
        let fixture = Fixture::new();
        let path = fixture.file("one.txt", b"x");
        let job = SearchJob::default();
        let reviewed = preview(vec![path.clone()], &job);
        fs::write(&path, b"external").unwrap();
        let summary = apply_disk_files(
            reviewed,
            &fixture.options(),
            &OpenFileRegistry::default(),
            &job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
        )
        .unwrap();
        assert_eq!(summary.changed_files(), 0);
        assert!(matches!(
            summary.receipt.files[0].state,
            ReceiptState::Skipped(_)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"external");
        fs::write(&path, b"x").unwrap();
        let reviewed = preview(vec![path.clone()], &job);
        let registry = OpenFileRegistry::default();
        let lease = registry
            .try_register(
                reviewed.files()[0].path.clone(),
                &reviewed.files()[0].fingerprint.identity,
            )
            .unwrap();
        let summary = apply_disk_files(
            reviewed,
            &fixture.options(),
            &registry,
            &job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
        )
        .unwrap();
        assert_eq!(summary.changed_files(), 0);
        assert!(matches!(
            summary.receipt.files[0].state,
            ReceiptState::Skipped(_)
        ));
        drop(lease);
        assert_eq!(fs::read(&path).unwrap(), b"x");
    }
    struct Injected {
        source: PathBuf,
        fail_receipt: bool,
        fail_source: bool,
        committed: AtomicBool,
        cancel: Option<SearchJob>,
    }
    impl LocalFileSystem for Injected {
        fn validate_source(&self, path: &Path) -> io::Result<()> {
            WindowsFileSystem.validate_source(path)
        }
        fn validate_target(&self, path: &Path) -> io::Result<()> {
            WindowsFileSystem.validate_target(path)
        }
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            WindowsFileSystem.identity(file)
        }
        fn commit(&self, stage: &Path, target: &Path, existed: bool) -> io::Result<()> {
            if target == self.source && self.fail_source {
                return Err(io::Error::other("injected disk full before commit"));
            }
            if self.fail_receipt
                && self.committed.load(Ordering::Acquire)
                && target.file_name().is_some_and(|n| n == "receipt.json")
            {
                return Err(io::Error::other(
                    "injected receipt failure after source commit",
                ));
            }
            WindowsFileSystem.commit(stage, target, existed)?;
            if target == self.source {
                self.committed.store(true, Ordering::Release);
                if let Some(job) = &self.cancel {
                    job.cancel();
                }
            }
            Ok(())
        }
    }
    #[test]
    fn interrupted_commit_reconciles_by_hash_without_reapplying() {
        let fixture = Fixture::new();
        let path = fixture.file("one.txt", b"x");
        let job = SearchJob::default();
        let reviewed = preview(vec![path.clone()], &job);
        let canonical = reviewed.files()[0].path.clone();
        let platform = Injected {
            source: canonical,
            fail_receipt: true,
            fail_source: false,
            committed: AtomicBool::new(false),
            cancel: None,
        };
        assert!(
            apply_disk_files(
                reviewed,
                &fixture.options(),
                &OpenFileRegistry::default(),
                &job,
                &WindowsPathTrustProvider,
                &platform
            )
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"Y");
        let directory = fs::read_dir(fixture.0.join("receipts"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let receipt_path = directory.join("receipt.json");
        let staged: ReplaceReceipt =
            serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        assert_eq!(staged.files[0].state, ReceiptState::Staged);
        let recovered = reconcile_receipt(
            &receipt_path,
            &job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
        )
        .unwrap();
        assert_eq!(recovered.files[0].state, ReceiptState::ReconciledCommitted);
        assert_eq!(fs::read(&path).unwrap(), b"Y");
    }
    #[test]
    fn cancellation_preserves_completed_file_and_leaves_rest_byte_identical() {
        let fixture = Fixture::new();
        let a = fixture.file("a.txt", b"x");
        let b = fixture.file("b.txt", b"x");
        let job = SearchJob::default();
        let reviewed = preview(vec![a.clone(), b.clone()], &job);
        let platform = Injected {
            source: reviewed.files()[0].path.clone(),
            fail_receipt: false,
            fail_source: false,
            committed: AtomicBool::new(false),
            cancel: Some(job.clone()),
        };
        let summary = apply_disk_files(
            reviewed,
            &fixture.options(),
            &OpenFileRegistry::default(),
            &job,
            &WindowsPathTrustProvider,
            &platform,
        )
        .unwrap();
        assert_eq!(summary.changed_files(), 1);
        assert_eq!(fs::read(a).unwrap(), b"Y");
        assert_eq!(fs::read(b).unwrap(), b"x");
        assert!(matches!(
            summary.receipt.files[1].state,
            ReceiptState::Skipped(_)
        ));
    }
    #[test]
    fn failed_source_commit_keeps_original_and_receipt_reconciliation_is_safe() {
        let fixture = Fixture::new();
        let path = fixture.file("one.txt", b"x");
        let job = SearchJob::default();
        let reviewed = preview(vec![path.clone()], &job);
        let platform = Injected {
            source: reviewed.files()[0].path.clone(),
            fail_receipt: false,
            fail_source: true,
            committed: AtomicBool::new(false),
            cancel: None,
        };
        let summary = apply_disk_files(
            reviewed,
            &fixture.options(),
            &OpenFileRegistry::default(),
            &job,
            &WindowsPathTrustProvider,
            &platform,
        )
        .unwrap();
        assert_eq!(summary.changed_files(), 0);
        assert!(matches!(
            summary.receipt.files[0].state,
            ReceiptState::Uncertain(_)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"x");
        let recovered = reconcile_receipt(
            &summary.receipt_path,
            &job,
            &WindowsPathTrustProvider,
            &WindowsFileSystem,
        )
        .unwrap();
        assert!(matches!(recovered.files[0].state, ReceiptState::Skipped(_)));
        assert_eq!(fs::read(path).unwrap(), b"x");
    }
}
