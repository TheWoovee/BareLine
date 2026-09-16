// SPDX-License-Identifier: MPL-2.0
//! Bounded UTF-8 Resident lifecycle. Call only on the I/O worker.
use crate::cancellation::Cancellation;
use crate::codecs::{
    Encoding,
    disk::{DiskError, DiskOptions, DiskTranscoder, PagedTranscoded},
    resident::{ResidentBuilder, ResidentEncoding, ResidentError},
};
use bareline_document::{Budget, Document, DocumentBuilder, DocumentSnapshot, TextOffset};
use bareline_platform::{CleanupResponsibility, CommitMode, CommitState, FileIdentity, LocalFileSystem};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
impl bareline_platform::CommitCancellation for Cancellation {
    fn check(&self) -> io::Result<()> {
        Cancellation::check(self).map_err(|_| io::Error::from(io::ErrorKind::Interrupted))
    }
}

/// Initial fast path is exactly one source page; larger files await streaming integration.
pub const OPEN_LIMIT: usize = 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub identity: FileIdentity,
    pub sha256: [u8; 32],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DestinationCondition {
    MustBeAbsent,
    ReplaceCaptured(Fingerprint),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveOperation {
    Save,
    SaveAs,
    SaveCopy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationConsent {
    ExistingDocument,
    NotRequired,
    OverwriteConfirmed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedDestination {
    pub path: PathBuf,
    pub condition: DestinationCondition,
    pub consent: DestinationConsent,
    pub document: (u64, u64),
    pub operation: SaveOperation,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveConflict {
    pub target: Option<PathBuf>,
    pub editor_version: PathBuf,
    pub other_version: Option<PathBuf>,
    pub transaction: PathBuf,
    pub state: CommitState,
    pub verified: bool,
}
#[derive(Clone)]
pub struct SaveCleanup {
    pub target: PathBuf,
    pub editor_version: PathBuf,
    pub displaced_version: Option<PathBuf>,
    pub transaction: PathBuf,
    pub error: String,
    retry: std::sync::Arc<std::sync::Mutex<Option<bareline_platform::CommitReceipt>>>,
}
impl std::fmt::Debug for SaveCleanup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SaveCleanup")
            .field("target", &self.target)
            .field("editor_version", &self.editor_version)
            .field("displaced_version", &self.displaced_version)
            .field("transaction", &self.transaction)
            .field("error", &self.error)
            .finish()
    }
}
impl PartialEq for SaveCleanup {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target
            && self.editor_version == other.editor_version
            && self.displaced_version == other.displaced_version
            && self.transaction == other.transaction
            && self.error == other.error
    }
}
impl Eq for SaveCleanup {}
impl SaveCleanup {
    pub fn retry(&self, platform: &dyn LocalFileSystem) -> Result<bool, FileError> {
        let mut owner = self
            .retry
            .lock()
            .map_err(|_| FileError::Io(io::Error::other("save cleanup owner is unavailable")))?;
        let Some(receipt) = owner.as_mut() else {
            return Ok(false);
        };
        if let Some(cleanup) = receipt.cleanup_token.as_mut() {
            cleanup.publish_cleanup_authority()?;
        }
        platform.mark_commit_state(receipt, CommitState::CleanupPending)?;
        platform.cleanup_commit(receipt)?;
        *owner = None;
        Ok(true)
    }
}
#[derive(Debug, Default)]
pub struct SaveRecovery {
    pub conflicts: Vec<SaveConflict>,
    pub cleanups: Vec<SaveCleanup>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestinationPreflight {
    path: PathBuf,
    condition: DestinationCondition,
    document: (u64, u64),
    operation: SaveOperation,
}
impl DestinationPreflight {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn requires_overwrite_consent(&self) -> bool {
        matches!(self.condition, DestinationCondition::ReplaceCaptured(_))
    }
    pub fn approve(self, overwrite_confirmed: bool) -> Option<PreparedDestination> {
        let consent = match self.condition {
            DestinationCondition::MustBeAbsent => DestinationConsent::NotRequired,
            DestinationCondition::ReplaceCaptured(_) if overwrite_confirmed => DestinationConsent::OverwriteConfirmed,
            DestinationCondition::ReplaceCaptured(_) => return None,
        };
        Some(PreparedDestination {
            path: self.path,
            condition: self.condition,
            consent,
            document: self.document,
            operation: self.operation,
        })
    }
}

/// Capture the destination state on a worker before asking for overwrite consent.
/// The returned fingerprint is the only replacement authority a subsequent save may use.
pub fn preflight_destination(
    target: &Path,
    source: Option<&Path>,
    document: (u64, u64),
    operation: SaveOperation,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
) -> Result<DestinationPreflight, FileError> {
    cancellation.check()?;
    platform.validate_target(target)?;
    let path = match fs::canonicalize(target) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = target
                .parent()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?;
            fs::canonicalize(parent)?.join(
                target
                    .file_name()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no file name"))?,
            )
        }
        Err(error) => return Err(error.into()),
    };
    let condition = match fingerprint(&path, platform, cancellation) {
        Ok(captured) => DestinationCondition::ReplaceCaptured(captured),
        Err(FileError::Io(error)) if error.kind() == io::ErrorKind::NotFound => DestinationCondition::MustBeAbsent,
        Err(error) => return Err(error),
    };
    if operation == SaveOperation::SaveCopy
        && let Some(source) = source
        && let (Ok(source), Ok(target)) = (File::open(source), File::open(&path))
    {
        let source = platform.identity(&source)?;
        let target = platform.identity(&target)?;
        if source.volume == target.volume && source.file == target.file {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Save Copy destination is the document source or an alias",
            )
            .into());
        }
    }
    Ok(DestinationPreflight {
        path,
        condition,
        document,
        operation,
    })
}
pub struct Opened {
    pub encoding: Option<ResidentEncoding>,
    pub document: Document,
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
    pub bom: bool,
}
#[derive(Debug)]
pub enum FileError {
    EncodingAt(crate::codecs::failure::EncodingFailure),
    Transcode(DiskError),
    Encoding(ResidentError),
    Cancelled,
    Io(io::Error),
    Changed,
    UnsupportedEncoding,
    StreamingRequired,
    IncompleteSource,
    Budget,
    Conflict {
        target: PathBuf,
        proposed: PathBuf,
        transaction: PathBuf,
    },
    ConflictAfterCommit {
        target: PathBuf,
        proposed: PathBuf,
        displaced: PathBuf,
        transaction: PathBuf,
        approved: Fingerprint,
        actual_displaced: Fingerprint,
    },
    ConflictAfterCreate {
        target: PathBuf,
        proposed: PathBuf,
        transaction: PathBuf,
    },
    CancelledAfterCommit {
        target: PathBuf,
        proposed: PathBuf,
        displaced: Option<PathBuf>,
        transaction: PathBuf,
    },
    VerificationAfterCommit {
        target: PathBuf,
        proposed: PathBuf,
        displaced: Option<PathBuf>,
        transaction: PathBuf,
        reason: String,
    },
    Commit {
        staged: PathBuf,
        proposed: Option<PathBuf>,
        displaced: Option<PathBuf>,
        transaction: Option<PathBuf>,
        error: io::Error,
    },
}
impl FileError {
    pub fn save_conflict(&self) -> Option<SaveConflict> {
        match self {
            Self::Conflict {
                target,
                proposed,
                transaction,
            } => Some(SaveConflict {
                target: Some(target.clone()),
                editor_version: proposed.clone(),
                other_version: Some(target.clone()),
                transaction: transaction.clone(),
                state: CommitState::Conflict,
                verified: true,
            }),
            Self::ConflictAfterCommit {
                target,
                proposed,
                displaced,
                transaction,
                ..
            } => Some(SaveConflict {
                target: Some(target.clone()),
                editor_version: proposed.clone(),
                other_version: Some(displaced.clone()),
                transaction: transaction.clone(),
                state: CommitState::Conflict,
                verified: true,
            }),
            Self::ConflictAfterCreate {
                target,
                proposed,
                transaction,
            } => Some(SaveConflict {
                target: Some(target.clone()),
                editor_version: proposed.clone(),
                other_version: Some(target.clone()),
                transaction: transaction.clone(),
                state: CommitState::Conflict,
                verified: true,
            }),
            Self::CancelledAfterCommit {
                target,
                proposed,
                displaced,
                transaction,
            } => Some(SaveConflict {
                target: Some(target.clone()),
                editor_version: proposed.clone(),
                other_version: displaced.clone(),
                transaction: transaction.clone(),
                state: CommitState::Conflict,
                verified: true,
            }),
            Self::VerificationAfterCommit {
                target,
                proposed,
                displaced,
                transaction,
                ..
            } => Some(SaveConflict {
                target: Some(target.clone()),
                editor_version: proposed.clone(),
                other_version: displaced.clone(),
                transaction: transaction.clone(),
                state: CommitState::Conflict,
                verified: true,
            }),
            Self::Commit {
                proposed: Some(proposed),
                displaced,
                transaction: Some(transaction),
                ..
            } => Some(SaveConflict {
                target: None,
                editor_version: proposed.clone(),
                other_version: displaced.clone(),
                transaction: transaction.clone(),
                state: CommitState::Unverified,
                verified: false,
            }),
            _ => None,
        }
    }
}

pub fn inspect_save_transactions(
    parent: &Path,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
) -> Result<Vec<SaveConflict>, FileError> {
    Ok(inspect_save_recovery(parent, platform, cancellation)?.conflicts)
}
pub fn inspect_save_recovery(
    parent: &Path,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
) -> Result<SaveRecovery, FileError> {
    match platform.inspect_commit_transactions(parent, cancellation) {
        Ok(found) => {
            let mut result = SaveRecovery::default();
            for recovery in found {
                cancellation.check()?;
                if recovery.verified && recovery.state == CommitState::CleanupPending {
                    if let Some(receipt) = platform.resume_commit_cleanup(&recovery)? {
                        let target = recovery.target.clone().ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidData, "verified cleanup target is missing")
                        })?;
                        result.cleanups.push(SaveCleanup {
                            target,
                            editor_version: recovery.proposed,
                            displaced_version: recovery.displaced,
                            transaction: recovery.journal,
                            error: "Cleanup was interrupted before restart.".into(),
                            retry: std::sync::Arc::new(std::sync::Mutex::new(Some(receipt))),
                        });
                        continue;
                    }
                }
                result.conflicts.push(SaveConflict {
                    target: recovery.target,
                    editor_version: recovery.proposed,
                    other_version: recovery.displaced,
                    transaction: recovery.journal,
                    state: recovery.state,
                    verified: recovery.verified,
                });
            }
            Ok(result)
        }
        Err(error) if error.kind() == io::ErrorKind::Unsupported => Ok(SaveRecovery::default()),
        Err(error) => Err(error.into()),
    }
}
pub struct PagedOpenRequest {
    pub path: PathBuf,
    pub bytes: Budget,
    pub history: Budget,
    pub cache: PathBuf,
    pub options: DiskOptions,
    pub source_options: crate::source::SourceOptions,
}
pub struct PagedOpened {
    pub recovery_origin: Option<PathBuf>,
    /// Fully materialized recovered text, produced by the I/O worker when the
    /// untitled recovery fits the resident adoption limit.
    pub recovered_resident: Option<String>,
    pub transcoded: PagedTranscoded,
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
}
pub struct PausedTranscode {
    pub path: PathBuf,
    pub error: DiskError,
    job: DiskTranscoder,
    bytes: Budget,
    history: Budget,
    source_options: crate::source::SourceOptions,
}
pub enum TranscodeOutcome {
    Complete(Box<PagedOpened>),
    Paused(Box<PausedTranscode>),
    Failed(FileError),
}
/// Background-only entry. A quota pause owns its private segments until resumed or
/// dropped; dropping is explicit cancellation and never changes the source file.
pub fn open_paged_encoded(
    request: PagedOpenRequest,
    platform: std::sync::Arc<dyn LocalFileSystem>,
    cancellation: Cancellation,
    on_prefix: impl FnMut(DocumentSnapshot),
) -> TranscodeOutcome {
    let job = (|| -> Result<DiskTranscoder, FileError> {
        cancellation.check()?;
        platform.validate_source(&request.path)?;
        let file = File::open(&request.path)?;
        DiskTranscoder::new(
            FileInput {
                path: request.path.clone(),
                file,
            },
            platform.clone(),
            &request.cache,
            request.options,
            request.bytes.clone(),
            cancellation.clone(),
        )
        .map_err(FileError::Transcode)
    })();
    match job {
        Err(e) => TranscodeOutcome::Failed(e),
        Ok(job) => run_transcode(
            PausedTranscode {
                path: request.path,
                error: DiskError::NotComplete,
                job,
                bytes: request.bytes,
                history: request.history,
                source_options: request.source_options,
            },
            platform,
            cancellation,
            on_prefix,
        ),
    }
}
pub fn resume_transcode(
    mut paused: PausedTranscode,
    quota: u64,
    platform: std::sync::Arc<dyn LocalFileSystem>,
    cancellation: Cancellation,
    on_prefix: impl FnMut(DocumentSnapshot),
) -> TranscodeOutcome {
    paused.job.set_quota(quota);
    paused.job.set_cancellation(cancellation.clone());
    run_transcode(paused, platform, cancellation, on_prefix)
}
fn run_transcode(
    mut paused: PausedTranscode,
    platform: std::sync::Arc<dyn LocalFileSystem>,
    cancellation: Cancellation,
    mut on_prefix: impl FnMut(DocumentSnapshot),
) -> TranscodeOutcome {
    loop {
        if cancellation.check().is_err() {
            return TranscodeOutcome::Failed(FileError::Cancelled);
        }
        match paused.job.step() {
            Err(e @ DiskError::Quota { .. }) => {
                paused.error = e;
                return TranscodeOutcome::Paused(Box::new(paused));
            }
            Err(e) => return TranscodeOutcome::Failed(FileError::Transcode(e)),
            Ok(progress) => {
                if let Some(preview) = paused.job.take_preview() {
                    on_prefix(preview);
                }
                if progress.complete {
                    break;
                }
            }
        }
    }
    match paused.job.finish() {
        Err(e) => TranscodeOutcome::Failed(FileError::Transcode(e)),
        Ok(store) => match store.open_paged(
            platform,
            paused.source_options,
            paused.bytes,
            paused.history,
            Cancellation::default(),
        ) {
            Err(e) => TranscodeOutcome::Failed(FileError::Transcode(e)),
            Ok(transcoded) => TranscodeOutcome::Complete(Box::new(PagedOpened {
                recovery_origin: None,
                recovered_resident: None,
                path: paused.path,
                fingerprint: store.fingerprint.clone(),
                transcoded,
            })),
        },
    }
}
impl From<io::Error> for FileError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
fn fingerprint(
    path: &Path,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
) -> Result<Fingerprint, FileError> {
    cancellation.check()?;
    let mut file = File::open(path)?;
    let before = platform.identity(&file)?;
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        cancellation.check()?;
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        hash.update(&chunk[..count]);
    }
    if before != platform.identity(&file)? || before != platform.identity(&File::open(path)?)? {
        return Err(FileError::Changed);
    }
    Ok(Fingerprint {
        identity: before,
        sha256: hash.finalize().into(),
    })
}
pub fn open_utf8(
    path: &Path,
    platform: &dyn LocalFileSystem,
    bytes: Budget,
    history: Budget,
) -> Result<Opened, FileError> {
    open_utf8_cancellable(path, platform, bytes, history, &Cancellation::default())
}
/// Lossless encoded Resident path. Larger sources return StreamingRequired rather
/// than accumulating raw bytes or provenance beyond the explicit resident quota.
pub fn open_encoded_cancellable(
    path: &Path,
    platform: &dyn LocalFileSystem,
    bytes: Budget,
    history: Budget,
    interpret: Option<Encoding>,
    cancellation: &Cancellation,
) -> Result<Opened, FileError> {
    open_encoded_streaming(
        path,
        platform,
        bytes,
        history,
        cancellation,
        DecodeOptions {
            resident_max_bytes: OPEN_LIMIT as u64,
            interpret,
        },
        |_| {},
    )
}
pub fn open_utf8_cancellable(
    path: &Path,
    platform: &dyn LocalFileSystem,
    bytes: Budget,
    history: Budget,
    cancellation: &Cancellation,
) -> Result<Opened, FileError> {
    cancellation.check()?;
    platform.validate_source(path)?;
    let mut file = File::open(path)?;
    let before = platform.identity(&file)?;
    if before.length > OPEN_LIMIT as u64 {
        return Err(FileError::StreamingRequired);
    }
    let mut raw = Vec::with_capacity(before.length as usize);
    let mut chunk = [0u8; 64 * 1024];
    while raw.len() <= OPEN_LIMIT {
        cancellation.check()?;
        let capacity = chunk.len().min(OPEN_LIMIT + 1 - raw.len());
        let count = file.read(&mut chunk[..capacity])?;
        if count == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..count]);
    }
    if raw.len() > OPEN_LIMIT {
        return Err(FileError::StreamingRequired);
    }
    if before != platform.identity(&file)? || before != platform.identity(&File::open(path)?)? {
        return Err(FileError::Changed);
    }
    let fingerprint = Fingerprint {
        identity: before,
        sha256: Sha256::digest(&raw).into(),
    };
    let bom = raw.starts_with(&[0xef, 0xbb, 0xbf]);
    let text = std::str::from_utf8(if bom { &raw[3..] } else { &raw }).map_err(|_| FileError::UnsupportedEncoding)?;
    let document = Document::from_utf8(text, bytes, history).map_err(|_| FileError::Budget)?;
    cancellation.check()?;
    Ok(Opened {
        encoding: None,
        document,
        path: path.to_owned(),
        fingerprint,
        bom,
    })
}
/// Generic resident streaming open. Encoding detection sees only the first 64 KiB;
/// decoded prefixes publish before EOF. Raw provenance and decoded storage share
/// the caller's aggregate budget; failure invalidates every published preview.
#[derive(Clone, Copy, Debug)]
pub struct DecodeOptions {
    pub resident_max_bytes: u64,
    pub interpret: Option<Encoding>,
}
pub fn open_encoded_streaming(
    path: &Path,
    platform: &dyn LocalFileSystem,
    bytes: Budget,
    history: Budget,
    cancellation: &Cancellation,
    options: DecodeOptions,
    mut on_prefix: impl FnMut(DocumentSnapshot),
) -> Result<Opened, FileError> {
    cancellation.check()?;
    platform.validate_source(path)?;
    let mut file = File::open(path)?;
    let before = platform.identity(&file)?;
    if before.length > options.resident_max_bytes {
        return Err(FileError::StreamingRequired);
    }
    let raw_limit = usize::try_from(before.length).map_err(|_| FileError::StreamingRequired)?;
    let mut buffer = [0; 65536];
    let mut first = 0;
    while first < buffer.len() {
        cancellation.check()?;
        let n = file.read(&mut buffer[first..])?;
        if n == 0 {
            break;
        }
        first += n;
    }
    let mut builder = ResidentBuilder::new(
        &buffer[..first],
        options.interpret,
        bytes,
        history,
        raw_limit,
        64 * 1024 * 1024,
    )
    .map_err(FileError::Encoding)?;
    let mut hash = Sha256::new();
    let mut total = first as u64;
    hash.update(&buffer[..first]);
    builder.push(&buffer[..first]).map_err(FileError::Encoding)?;
    if before != platform.identity(&file)? || before != platform.identity(&File::open(path)?)? {
        return Err(FileError::Changed);
    }
    cancellation.check()?;
    on_prefix(builder.prefix().map_err(FileError::Encoding)?);
    loop {
        cancellation.check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > before.length {
            return Err(FileError::Changed);
        }
        hash.update(&buffer[..count]);
        builder.push(&buffer[..count]).map_err(FileError::Encoding)?;
    }
    if total != before.length
        || before != platform.identity(&file)?
        || before != platform.identity(&File::open(path)?)?
    {
        return Err(FileError::Changed);
    }
    cancellation.check()?;
    let (document, encoding) = builder.finish().map_err(FileError::Encoding)?;
    cancellation.check()?;
    Ok(Opened {
        bom: encoding.state.bom,
        encoding: Some(encoding),
        document,
        path: path.to_owned(),
        fingerprint: Fingerprint {
            identity: before,
            sha256: hash.finalize().into(),
        },
    })
}
/// Resident load with a bounded first-prefix publication before reading the rest.
/// Prefixes are read-only loading previews; only the returned document is complete.
/// An error after publication must discard the preview, never expose it as a full file.
pub fn open_utf8_streaming(
    path: &Path,
    platform: &dyn LocalFileSystem,
    bytes: Budget,
    history: Budget,
    cancellation: &Cancellation,
    resident_max_bytes: u64,
    on_prefix: impl FnMut(DocumentSnapshot),
) -> Result<Opened, FileError> {
    cancellation.check()?;
    platform.validate_source(path)?;
    let file = File::open(path)?;
    open_utf8_streaming_handle(
        FileInput {
            path: path.to_owned(),
            file,
        },
        platform,
        bytes,
        history,
        cancellation,
        resident_max_bytes,
        on_prefix,
    )
}
/// Content is read exclusively from this already-approved handle. The path is
/// used only for generation checks; caller retains any required traversal guards.
pub struct FileInput {
    pub path: PathBuf,
    pub file: File,
}
pub fn open_utf8_streaming_handle(
    source: FileInput,
    platform: &dyn LocalFileSystem,
    bytes: Budget,
    history: Budget,
    cancellation: &Cancellation,
    resident_max_bytes: u64,
    mut on_prefix: impl FnMut(DocumentSnapshot),
) -> Result<Opened, FileError> {
    cancellation.check()?;
    let path = &source.path;
    let mut file = source.file;
    let before = platform.identity(&file)?;
    if before.length > resident_max_bytes {
        return Err(FileError::StreamingRequired);
    }
    let mut builder = DocumentBuilder::new(bytes, history).map_err(|_| FileError::Budget)?;
    let mut buffer = [0u8; 64 * 1024];
    let mut pending = Vec::with_capacity(buffer.len() + 3);
    let mut total = 0u64;
    let mut hash = Sha256::new();
    let mut bom = false;
    let mut detected = false;
    let mut published = false;
    loop {
        cancellation.check()?;
        let count = file.read(&mut buffer)?;
        total = total.checked_add(count as u64).ok_or(FileError::Changed)?;
        if total > before.length {
            return Err(FileError::Changed);
        }
        hash.update(&buffer[..count]);
        pending.extend_from_slice(&buffer[..count]);
        if !detected && (pending.len() >= 3 || count == 0) {
            bom = pending.starts_with(&[0xef, 0xbb, 0xbf]);
            if bom {
                pending.drain(..3);
            }
            detected = true;
        }
        if detected {
            let valid = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(error) if error.error_len().is_none() && count != 0 => error.valid_up_to(),
                Err(_) => return Err(FileError::UnsupportedEncoding),
            };
            let text = std::str::from_utf8(&pending[..valid]).map_err(|_| FileError::UnsupportedEncoding)?;
            builder.append(text).map_err(|_| FileError::Budget)?;
            pending.drain(..valid);
            if !published && (valid != 0 || count == 0) {
                if before != platform.identity(&file)? || before != platform.identity(&File::open(path)?)? {
                    return Err(FileError::Changed);
                }
                cancellation.check()?;
                on_prefix(builder.prefix());
                published = true;
            }
        }
        if count == 0 {
            break;
        }
    }
    if total != before.length
        || before != platform.identity(&file)?
        || before != platform.identity(&File::open(path)?)?
    {
        return Err(FileError::Changed);
    }
    cancellation.check()?;
    Ok(Opened {
        encoding: None,
        document: builder.finish(),
        path: path.to_owned(),
        fingerprint: Fingerprint {
            identity: before,
            sha256: hash.finalize().into(),
        },
        bom,
    })
}

struct Staged {
    path: PathBuf,
    retain: bool,
}
impl Drop for Staged {
    fn drop(&mut self) {
        if !self.retain {
            let _ = fs::remove_file(&self.path);
        }
    }
}
pub struct Saved {
    pub fingerprint: Fingerprint,
    pub captured: DocumentSnapshot,
    pub cleanup: Option<SaveCleanup>,
}
pub struct PagedSaved {
    pub fingerprint: Fingerprint,
    pub captured: bareline_document::paged::PagedSnapshot,
    pub cleanup: Option<SaveCleanup>,
}
pub struct PagedSavePolicy {
    pub store: crate::codecs::disk::DiskDecoded,
    pub generation: bareline_document::source::Generation,
    pub encoding: Encoding,
    pub bom: bool,
}
pub fn save_paged_cancellable(
    snapshot: bareline_document::paged::PagedSnapshot,
    target: &Path,
    expected: Option<&Fingerprint>,
    policy: &PagedSavePolicy,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
) -> Result<PagedSaved, FileError> {
    let condition = expected
        .cloned()
        .map(DestinationCondition::ReplaceCaptured)
        .unwrap_or(DestinationCondition::MustBeAbsent);
    save_paged_to_cancellable(snapshot, target, &condition, policy, platform, cancellation)
}
pub fn save_paged_to_cancellable(
    snapshot: bareline_document::paged::PagedSnapshot,
    target: &Path,
    condition: &DestinationCondition,
    policy: &PagedSavePolicy,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
) -> Result<PagedSaved, FileError> {
    let receipt =
        save_bytes(target, condition, platform, cancellation, |out| {
            policy
                .store
                .write_snapshot(
                    &snapshot,
                    policy.generation,
                    policy.encoding,
                    policy.bom,
                    out,
                    cancellation,
                )
                .map_err(|error| match error {
                    DiskError::At { range, reason } => FileError::EncodingAt(
                        crate::codecs::failure::EncodingFailure::new(snapshot.identity_token(), range, reason),
                    ),
                    error => FileError::Transcode(error),
                })
        })?;
    Ok(PagedSaved {
        fingerprint: receipt.fingerprint,
        captured: snapshot,
        cleanup: receipt.cleanup,
    })
}
pub fn save_utf8(
    snapshot: DocumentSnapshot,
    target: &Path,
    expected: Option<&Fingerprint>,
    bom: bool,
    platform: &dyn LocalFileSystem,
) -> Result<Saved, FileError> {
    save_utf8_cancellable(snapshot, target, expected, bom, platform, &Cancellation::default())
}
/// Cancellation is honored until the final commit checkpoint. Once replacement starts,
/// finish verification and report its actual outcome, even if cancellation then arrives.
pub fn save_utf8_cancellable(
    snapshot: DocumentSnapshot,
    target: &Path,
    expected: Option<&Fingerprint>,
    bom: bool,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
) -> Result<Saved, FileError> {
    save_impl(snapshot, target, expected, bom, platform, cancellation, None)
}
pub fn save_utf8_to_cancellable(
    snapshot: DocumentSnapshot,
    target: &Path,
    condition: &DestinationCondition,
    bom: bool,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
) -> Result<Saved, FileError> {
    save_impl_to(snapshot, target, condition, bom, platform, cancellation, None)
}
pub fn save_encoded_cancellable(
    snapshot: DocumentSnapshot,
    target: &Path,
    expected: Option<&Fingerprint>,
    bom: bool,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
    encoding: &ResidentEncoding,
) -> Result<Saved, FileError> {
    save_impl(snapshot, target, expected, bom, platform, cancellation, Some(encoding))
}
pub fn save_encoded_to_cancellable(
    snapshot: DocumentSnapshot,
    target: &Path,
    condition: &DestinationCondition,
    bom: bool,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
    encoding: &ResidentEncoding,
) -> Result<Saved, FileError> {
    save_impl_to(snapshot, target, condition, bom, platform, cancellation, Some(encoding))
}
fn save_impl(
    snapshot: DocumentSnapshot,
    target: &Path,
    expected: Option<&Fingerprint>,
    bom: bool,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
    encoding: Option<&ResidentEncoding>,
) -> Result<Saved, FileError> {
    let condition = expected
        .cloned()
        .map(DestinationCondition::ReplaceCaptured)
        .unwrap_or(DestinationCondition::MustBeAbsent);
    save_impl_to(snapshot, target, &condition, bom, platform, cancellation, encoding)
}
fn save_impl_to(
    snapshot: DocumentSnapshot,
    target: &Path,
    condition: &DestinationCondition,
    bom: bool,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
    encoding: Option<&ResidentEncoding>,
) -> Result<Saved, FileError> {
    if !snapshot.is_complete() {
        return Err(FileError::IncompleteSource);
    }
    let receipt = save_bytes(target, condition, platform, cancellation, |out| {
        if let Some(encoding) = encoding {
            encoding
                .write_snapshot(&snapshot, encoding.state.save_target, bom, out)
                .map_err(|error| match error {
                    ResidentError::At { range, reason } => FileError::EncodingAt(
                        crate::codecs::failure::EncodingFailure::new(snapshot.identity_token(), range, reason),
                    ),
                    error => FileError::Encoding(error),
                })
        } else {
            let policy = crate::codecs::state::metadata_encoding(snapshot.metadata());
            let (encoding, bom) = policy.map_or((Encoding::Utf8, bom), |state| (state.save_target, state.bom));
            if bom {
                out.write_all(encoding.bom())?;
            }
            let encoder = crate::codecs::Encoder::new(encoding, false);
            let mut offset = 0usize;
            for chunk in snapshot
                .chunks(TextOffset(0)..TextOffset(snapshot.len()))
                .map_err(|_| FileError::Budget)?
            {
                for (local, part) in crate::codecs::failure::bounded_chunks(chunk) {
                    cancellation.check()?;
                    let encoded = encoder.encode_text(part).map_err(|error| {
                        FileError::EncodingAt(crate::codecs::failure::EncodingFailure::new(
                            snapshot.identity_token(),
                            crate::codecs::failure::rejected_range(part, encoding, offset + local),
                            format!("{error:?}"),
                        ))
                    })?;
                    out.write_all(&encoded)?;
                }
                offset += chunk.len();
            }
            Ok(())
        }
    })?;
    Ok(Saved {
        fingerprint: receipt.fingerprint,
        captured: snapshot,
        cleanup: receipt.cleanup,
    })
}
/// Hold the source file against mutation for the entire copy, including alias targets.
/// A removed source is fine: the immutable document is still exportable.
pub fn guard_copy_source(
    source: &Path,
    target: &Path,
    platform: &dyn LocalFileSystem,
) -> Result<Option<File>, FileError> {
    let source = match platform.open_sealed_read(source) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let identity = platform.identity(&source)?;
    platform.validate_target(target)?;
    match File::open(target) {
        Ok(target) => {
            let target = platform.identity(&target)?;
            if identity.volume == target.volume && identity.file == target.file {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Save Copy destination is the document source or an alias",
                )
                .into());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(Some(source))
}
struct SaveBytesReceipt {
    fingerprint: Fingerprint,
    cleanup: Option<SaveCleanup>,
}
fn save_bytes(
    target: &Path,
    condition: &DestinationCondition,
    platform: &dyn LocalFileSystem,
    cancellation: &Cancellation,
    emit: impl FnOnce(&mut dyn Write) -> Result<(), FileError>,
) -> Result<SaveBytesReceipt, FileError> {
    cancellation.check()?;
    platform.validate_target(target)?;
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?;
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let (mut file, mut staged) = loop {
        cancellation.check()?;
        let path = parent.join(format!(
            ".bareline-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                break (file, Staged { path, retain: false });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    #[cfg(test)]
    fault_transitions::hit(fault_transitions::Point::StageCreated)?;
    let write_result = (|| -> Result<[u8; 32], FileError> {
        let mut staged_hash = Sha256::new();
        struct Writer<'a> {
            file: &'a mut File,
            hash: &'a mut Sha256,
            cancellation: &'a Cancellation,
        }
        impl Write for Writer<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.cancellation.check().map_err(|_| io::Error::other("cancelled"))?;
                let n = self.file.write(bytes)?;
                #[cfg(test)]
                fault_transitions::hit(fault_transitions::Point::StageWritten)?;
                self.hash.update(&bytes[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> io::Result<()> {
                self.file.flush()
            }
        }
        let result = emit(&mut Writer {
            file: &mut file,
            hash: &mut staged_hash,
            cancellation,
        });
        cancellation.check()?;
        result?;
        cancellation.check()?;
        #[cfg(test)]
        fault_transitions::hit(fault_transitions::Point::BeforeStageFlush)?;
        file.sync_all()?;
        #[cfg(feature = "qa-faults")]
        crate::qa_faults::hit("StageFlushed", target)?;
        #[cfg(test)]
        fault_transitions::hit(fault_transitions::Point::StageFlushed)?;
        Ok(staged_hash.finalize().into())
    })();
    // Close before propagating write/cancellation errors so Windows can remove the stage.
    drop(file);
    let written_hash = write_result?;
    // Revalidate metadata policy immediately before replacement. No in-place fallback exists.
    platform.validate_target(target)?;
    cancellation.check()?;
    let mode = if matches!(condition, DestinationCondition::ReplaceCaptured(_)) {
        CommitMode::Replace
    } else {
        CommitMode::CreateNew
    };
    let transaction = match platform.prepare_commit(&staged.path, target, mode, cancellation) {
        Ok(transaction) => transaction,
        Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(FileError::Cancelled),
        Err(error) => {
            staged.retain = true;
            return Err(FileError::Commit {
                staged: staged.path.clone(),
                proposed: None,
                displaced: None,
                transaction: None,
                error,
            });
        }
    };
    let proposed_recovery = transaction.proposed_path.clone();
    let displaced_recovery = transaction.displaced_path.clone();
    let transaction_recovery = transaction.journal_path.clone();
    let unchanged = match condition {
        DestinationCondition::ReplaceCaptured(expected) => match fingerprint(target, platform, cancellation) {
            Ok(current) => &current == expected,
            Err(FileError::Cancelled) => return Err(FileError::Cancelled),
            Err(_) => false,
        },
        DestinationCondition::MustBeAbsent => {
            matches!(File::open(target), Err(error) if error.kind() == io::ErrorKind::NotFound)
        }
    };
    #[cfg(test)]
    fault_transitions::hit(fault_transitions::Point::ExpectedFingerprintChecked)?;
    if !unchanged {
        let proposed = proposed_recovery
            .clone()
            .ok_or_else(|| io::Error::other("prepared save did not retain editor bytes"))?;
        let transaction_path = transaction_recovery
            .clone()
            .ok_or_else(|| io::Error::other("prepared save did not publish recovery authority"))?;
        let _ = bareline_platform::publish_commit_state(&transaction_path, CommitState::Conflict);
        return Err(FileError::Conflict {
            target: target.to_path_buf(),
            proposed,
            transaction: transaction_path,
        });
    }
    #[cfg(test)]
    fault_transitions::hit(fault_transitions::Point::BeforeReplace)?;
    #[cfg(feature = "qa-faults")]
    crate::qa_faults::hit("BeforeReplace", target)?;
    let mut receipt = match platform.commit_transaction(transaction) {
        Ok(receipt) => receipt,
        Err(error) => {
            staged.retain = true;
            if mode == CommitMode::CreateNew
                && error.kind() == io::ErrorKind::AlreadyExists
                && let (Some(proposed), Some(transaction)) = (proposed_recovery.clone(), transaction_recovery.clone())
            {
                staged.retain = false;
                return Err(FileError::ConflictAfterCreate {
                    target: target.to_path_buf(),
                    proposed,
                    transaction,
                });
            }
            return Err(FileError::Commit {
                staged: staged.path.clone(),
                proposed: proposed_recovery,
                displaced: displaced_recovery,
                transaction: transaction_recovery,
                error,
            });
        }
    };
    if mode == CommitMode::Replace && (receipt.proposed.is_none() || receipt.displaced.is_none()) {
        staged.retain = true;
        return Err(FileError::Commit {
            staged: staged.path.clone(),
            proposed: proposed_recovery,
            displaced: displaced_recovery,
            transaction: transaction_recovery,
            error: io::Error::new(
                io::ErrorKind::Unsupported,
                "replacement receipt did not retain both versions",
            ),
        });
    }
    if cancellation.check().is_err() {
        let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
        return Err(FileError::CancelledAfterCommit {
            target: target.to_path_buf(),
            proposed: receipt
                .proposed
                .as_ref()
                .map(|file| file.path.clone())
                .or(proposed_recovery)
                .unwrap_or_else(|| staged.path.clone()),
            displaced: receipt
                .displaced
                .as_ref()
                .map(|file| file.path.clone())
                .or(displaced_recovery),
            transaction: receipt.journal.clone().unwrap_or_default(),
        });
    }
    #[cfg(test)]
    fault_transitions::hit(fault_transitions::Point::AfterReplace)?;
    #[cfg(feature = "qa-faults")]
    crate::qa_faults::hit("AfterReplace", target)?;
    let postcommit_error = |reason: String| FileError::VerificationAfterCommit {
        target: target.to_path_buf(),
        proposed: receipt
            .proposed
            .as_ref()
            .map(|file| file.path.clone())
            .or_else(|| proposed_recovery.clone())
            .unwrap_or_else(|| staged.path.clone()),
        displaced: receipt
            .displaced
            .as_ref()
            .map(|file| file.path.clone())
            .or_else(|| displaced_recovery.clone()),
        transaction: receipt.journal.clone().unwrap_or_default(),
        reason,
    };
    let new_fingerprint = match fingerprint(target, platform, &Cancellation::default()) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
            return Err(postcommit_error(format!("{error:?}")));
        }
    };
    #[cfg(test)]
    fault_transitions::hit(fault_transitions::Point::TargetFingerprintChecked)?;
    let target_is_output = new_fingerprint.identity == receipt.target && new_fingerprint.sha256 == written_hash;
    if let DestinationCondition::ReplaceCaptured(approved) = condition {
        let proposed = receipt.proposed.as_ref().expect("replacement receipt checked");
        let displaced = receipt.displaced.as_ref().expect("replacement receipt checked");
        let proposed_fingerprint = match fingerprint(&proposed.path, platform, &Cancellation::default()) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
                return Err(postcommit_error(format!("{error:?}")));
            }
        };
        let actual_displaced = match fingerprint(&displaced.path, platform, &Cancellation::default()) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
                return Err(postcommit_error(format!("{error:?}")));
            }
        };
        if proposed_fingerprint.identity != proposed.identity
            || proposed_fingerprint.sha256 != written_hash
            || actual_displaced.identity != displaced.identity
            || &actual_displaced != approved
            || !target_is_output
        {
            let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
            return Err(FileError::ConflictAfterCommit {
                target: target.to_path_buf(),
                proposed: proposed.path.clone(),
                displaced: displaced.path.clone(),
                transaction: receipt.journal.clone().unwrap_or_default(),
                approved: approved.clone(),
                actual_displaced,
            });
        }
    } else if !target_is_output {
        let proposed = receipt
            .proposed
            .as_ref()
            .map(|file| file.path.clone())
            .or(proposed_recovery)
            .unwrap_or_else(|| staged.path.clone());
        let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
        return Err(FileError::ConflictAfterCreate {
            target: target.to_path_buf(),
            proposed,
            transaction: receipt.journal.clone().unwrap_or_default(),
        });
    }
    #[cfg(test)]
    fault_transitions::hit(fault_transitions::Point::BeforeReceipt)?;
    if receipt.cleanup == CleanupResponsibility::Caller {
        let proposed = receipt
            .proposed
            .as_ref()
            .map(|file| file.path.clone())
            .or(proposed_recovery)
            .unwrap_or_default();
        let displaced = receipt
            .displaced
            .as_ref()
            .map(|file| file.path.clone())
            .or(displaced_recovery);
        let transaction = receipt.journal.clone().unwrap_or_default();
        let cleanup_result = receipt
            .cleanup_token
            .as_mut()
            .map_or(Ok(()), |cleanup| cleanup.publish_cleanup_authority())
            .and_then(|_| platform.mark_commit_state(&receipt, CommitState::CleanupPending))
            .and_then(|_| platform.cleanup_commit(&mut receipt));
        if let Err(error) = cleanup_result {
            return Ok(SaveBytesReceipt {
                fingerprint: new_fingerprint,
                cleanup: Some(SaveCleanup {
                    target: target.to_path_buf(),
                    editor_version: proposed,
                    displaced_version: displaced,
                    transaction,
                    error: error.to_string(),
                    retry: std::sync::Arc::new(std::sync::Mutex::new(Some(receipt))),
                }),
            });
        }
    }
    Ok(SaveBytesReceipt {
        fingerprint: new_fingerprint,
        cleanup: None,
    })
}

// Bounded queue has at most sixteen completions; avoiding per-result allocation
// preserves the existing worker/UI message contract.
#[allow(clippy::large_enum_variant)]
pub enum IoCompletion {
    SaveCleanupRetried {
        cleanup: SaveCleanup,
        result: Result<bool, FileError>,
    },
    SaveRecoveryInspection {
        parent: PathBuf,
        result: Result<SaveRecovery, FileError>,
    },
    ResidentSpilled {
        captured: DocumentSnapshot,
        result: Result<(PagedTranscoded, Option<bareline_document::spill::PreparedSpill>), FileError>,
    },
    Transcode(TranscodeOutcome),
    Open(Result<Opened, FileError>),
    Save(Result<Saved, FileError>),
}
pub enum IoRequest {
    RetrySaveCleanup {
        cleanup: SaveCleanup,
    },
    InspectSaveRecovery {
        parent: PathBuf,
    },
    SpillOwnedResident {
        saved_state: bareline_document::ContentStateId,
        service: bareline_document::service::DocumentService,
        captured: DocumentSnapshot,
        encoding: Option<ResidentEncoding>,
        original: Option<(PathBuf, Fingerprint)>,
        cache: PathBuf,
        quota: u64,
        options: crate::source::SourceOptions,
        bytes: Budget,
        history: Budget,
    },
    SpillResident {
        captured: DocumentSnapshot,
        encoding: Option<ResidentEncoding>,
        bom: bool,
        cache: PathBuf,
        quota: u64,
        options: crate::source::SourceOptions,
        bytes: Budget,
        history: Budget,
    },
    SaveCopy {
        snapshot: DocumentSnapshot,
        destination: PreparedDestination,
        source: Option<PathBuf>,
        bom: bool,
        encoding: Option<ResidentEncoding>,
    },
    RestorePagedRecovery {
        directory: PathBuf,
        bytes: Budget,
        history: Budget,
        resident_max_bytes: u64,
    },
    OpenPagedEncoded(PagedOpenRequest),
    ResumeTranscode {
        paused: Box<PausedTranscode>,
        temp_quota_bytes: u64,
    },
    Interpret(Box<InterpretRequest>),
    InterpretPaged(Box<InterpretPagedRequest>),
    OpenEncoded {
        path: PathBuf,
        bytes: Budget,
        history: Budget,
        interpret: Option<Encoding>,
        resident_max_bytes: u64,
    },
    SaveEncoded {
        snapshot: DocumentSnapshot,
        destination: PreparedDestination,
        bom: bool,
        encoding: ResidentEncoding,
    },
    OpenStreaming {
        path: PathBuf,
        bytes: Budget,
        history: Budget,
        resident_max_bytes: u64,
    },
    Open {
        path: PathBuf,
        bytes: Budget,
        history: Budget,
    },
    Save {
        snapshot: DocumentSnapshot,
        destination: PreparedDestination,
        bom: bool,
    },
}
/// Reinterpret the retained sealed original on the worker without touching disk.
pub struct InterpretPagedRequest {
    pub source: crate::codecs::disk::DiskDecoded,
    pub target: Encoding,
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
    pub cache: PathBuf,
    pub quota: u64,
    pub options: crate::source::SourceOptions,
    pub bytes: Budget,
    pub history: Budget,
}
pub struct InterpretRequest {
    pub source: ResidentEncoding,
    pub target: Encoding,
    pub dirty: bool,
    pub discard_confirmed: bool,
    pub bytes: Budget,
    pub history: Budget,
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
}
type Notification = std::sync::Arc<dyn Fn() + Send + Sync>;
enum ReadAuthorization {
    Grant(bareline_platform::RemoteReadGrant, bareline_platform::RemoteReadAction),
    Access(bareline_platform::RemoteReadAccess),
}
struct Job {
    authorization: Option<ReadAuthorization>,
    cancellation: Cancellation,
    request: IoRequest,
    reply: std::sync::mpsc::SyncSender<IoCompletion>,
    prefix: std::sync::mpsc::SyncSender<DocumentSnapshot>,
    notify: Notification,
}
/// One shared I/O worker serializes saves, including two documents targeting one path.
pub struct IoService {
    sender: std::sync::mpsc::SyncSender<Job>,
}
/// Dropping a pending result cancels its queued/running work without blocking the UI.
pub struct IoTicket {
    receiver: std::sync::mpsc::Receiver<IoCompletion>,
    prefix: std::sync::mpsc::Receiver<DocumentSnapshot>,
    cancellation: Cancellation,
}
impl IoTicket {
    /// Read-only loading preview, never a complete editable document.
    pub fn try_prefix(&self) -> Result<DocumentSnapshot, std::sync::mpsc::TryRecvError> {
        self.prefix.try_recv()
    }
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
    pub fn try_recv(&self) -> Result<IoCompletion, std::sync::mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
}
impl Drop for IoTicket {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl IoService {
    pub fn new(platform: std::sync::Arc<dyn LocalFileSystem>) -> io::Result<Self> {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Job>(16);
        std::thread::Builder::new().name("file-io".into()).spawn(move || {
            while let Ok(job) = receiver.recv() {
                let scoped = (|| -> io::Result<std::sync::Arc<dyn LocalFileSystem>> {
                    let Some(authorization) = job.authorization else {
                        return Ok(platform.clone());
                    };
                    let path = read_request_path(&job.request).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "remote grant cannot authorize this operation",
                        )
                    })?;
                    let access = match authorization {
                        ReadAuthorization::Grant(grant, action) => {
                            job.cancellation
                                .check()
                                .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "read cancelled"))?;
                            grant.claim(path, action, std::sync::Arc::new(|| false))?
                        }
                        ReadAuthorization::Access(access) => {
                            access.check(path)?;
                            access
                        }
                    };
                    platform.scoped_remote_read(access)
                })();
                let platform = match scoped {
                    Ok(platform) => platform,
                    Err(error) => {
                        let _ = job.reply.try_send(IoCompletion::Open(Err(FileError::Io(error))));
                        (job.notify)();
                        continue;
                    }
                };
                let result = match job.request {
                    IoRequest::RetrySaveCleanup { cleanup } => {
                        let result = cleanup.retry(platform.as_ref());
                        IoCompletion::SaveCleanupRetried { cleanup, result }
                    }
                    IoRequest::InspectSaveRecovery { parent } => IoCompletion::SaveRecoveryInspection {
                        result: inspect_save_recovery(&parent, platform.as_ref(), &job.cancellation),
                        parent,
                    },
                    IoRequest::SpillOwnedResident {
                        saved_state,
                        service,
                        captured,
                        encoding,
                        original,
                        cache,
                        quota,
                        options,
                        bytes,
                        history,
                    } => {
                        let result = (|| {
                            let plan = service
                                .capture_spill_with_saved(&captured, saved_state)
                                .map_err(|_| FileError::Budget)?;
                            if !plan.matches_resident(&captured) {
                                return Err(FileError::Changed);
                            }
                            let baseline = if encoding.is_none()
                                && let Some((path, expected)) = original
                            {
                                let request = PagedOpenRequest {
                                    path,
                                    bytes: bytes.clone(),
                                    history: history.clone(),
                                    cache: cache.clone(),
                                    options: DiskOptions {
                                        temp_quota_bytes: quota / 2,
                                        interpret: Some(Encoding::Utf8),
                                    },
                                    source_options: options,
                                };
                                match open_paged_encoded(request, platform.clone(), job.cancellation.clone(), |_| {}) {
                                    TranscodeOutcome::Complete(opened) if opened.fingerprint == expected => {
                                        opened.transcoded
                                    }
                                    TranscodeOutcome::Complete(_) => return Err(FileError::Changed),
                                    TranscodeOutcome::Failed(error) => return Err(error),
                                    TranscodeOutcome::Paused(paused) => return Err(FileError::Transcode(paused.error)),
                                }
                            } else {
                                crate::owned_store::prepare_original_baseline(
                                    encoding.as_ref(),
                                    &cache,
                                    quota / 2,
                                    platform.clone(),
                                    options,
                                    bytes.clone(),
                                    history.clone(),
                                    job.cancellation.clone(),
                                )?
                            };
                            let source = baseline.source.source();
                            let prepared = crate::owned_store::prepare_segments(
                                plan,
                                encoding.as_ref().map(|encoding| (encoding, &source)),
                                &cache,
                                quota - quota / 2,
                                platform.clone(),
                                options,
                                bytes,
                                &job.cancellation,
                            )?;
                            Ok((baseline, Some(prepared)))
                        })();
                        IoCompletion::ResidentSpilled { captured, result }
                    }
                    IoRequest::SpillResident {
                        captured,
                        encoding,
                        bom,
                        cache,
                        quota,
                        options,
                        bytes,
                        history,
                    } => {
                        let result = crate::owned_store::prepare_resident(
                            &captured,
                            encoding.as_ref(),
                            bom,
                            &cache,
                            quota,
                            platform.clone(),
                            options,
                            bytes,
                            history,
                            job.cancellation.clone(),
                        );
                        IoCompletion::ResidentSpilled {
                            captured,
                            result: result.map(|transcoded| (transcoded, None)),
                        }
                    }
                    IoRequest::RestorePagedRecovery {
                        directory,
                        bytes,
                        history,
                        resident_max_bytes,
                    } => IoCompletion::Transcode(
                        match crate::paged_recovery::restore(
                            &directory,
                            platform.clone(),
                            bytes.clone(),
                            history,
                            &job.cancellation,
                        ) {
                            Ok(mut opened) => match (|| -> Result<(), FileError> {
                                let inspection = crate::recovery::inspect(&directory, &job.cancellation)?;
                                if inspection.metadata.original_path.is_none() {
                                    opened.recovered_resident = crate::paged_recovery::restore_text(
                                        &mut opened,
                                        resident_max_bytes,
                                        &bytes,
                                        &job.cancellation,
                                    )
                                    .map_err(|error| FileError::Io(io::Error::other(error)))?;
                                }
                                Ok(())
                            })() {
                                Ok(()) => TranscodeOutcome::Complete(Box::new(opened)),
                                Err(error) => TranscodeOutcome::Failed(error),
                            },
                            Err(error) => TranscodeOutcome::Failed(FileError::Io(io::Error::other(error))),
                        },
                    ),
                    IoRequest::OpenPagedEncoded(request) => IoCompletion::Transcode(open_paged_encoded(
                        request,
                        platform.clone(),
                        job.cancellation.clone(),
                        |snapshot| {
                            let _ = job.prefix.try_send(snapshot);
                            (job.notify)();
                        },
                    )),
                    IoRequest::ResumeTranscode {
                        paused,
                        temp_quota_bytes,
                    } => IoCompletion::Transcode(resume_transcode(
                        *paused,
                        temp_quota_bytes,
                        platform.clone(),
                        job.cancellation.clone(),
                        |snapshot| {
                            let _ = job.prefix.try_send(snapshot);
                            (job.notify)();
                        },
                    )),
                    IoRequest::InterpretPaged(request) => {
                        let result = crate::owned_store::reinterpret_paged(
                            &request.source,
                            request.target,
                            &request.cache,
                            request.quota,
                            platform.clone(),
                            request.options,
                            request.bytes,
                            request.history,
                            job.cancellation.clone(),
                        );
                        IoCompletion::Transcode(match result {
                            Ok(transcoded) => TranscodeOutcome::Complete(Box::new(PagedOpened {
                                recovery_origin: None,
                                recovered_resident: None,
                                transcoded,
                                path: request.path,
                                fingerprint: request.fingerprint,
                            })),
                            Err(error) => TranscodeOutcome::Failed(error),
                        })
                    }
                    IoRequest::Interpret(request) => IoCompletion::Open((|| {
                        job.cancellation.check()?;
                        if request.dirty && !request.discard_confirmed {
                            return Err(FileError::Encoding(ResidentError::DirtyInterpret));
                        }
                        let (document, encoding) = request
                            .source
                            .reinterpret_streaming(
                                request.target,
                                request.bytes,
                                request.history,
                                64 * 1024 * 1024,
                                || job.cancellation.check().map_err(|_| ResidentError::Cancelled),
                            )
                            .map_err(|e| match e {
                                ResidentError::Cancelled => FileError::Cancelled,
                                e => FileError::Encoding(e),
                            })?;
                        job.cancellation.check()?;
                        Ok(Opened {
                            document,
                            bom: encoding.state.bom,
                            encoding: Some(encoding),
                            path: request.path,
                            fingerprint: request.fingerprint,
                        })
                    })()),
                    IoRequest::OpenEncoded {
                        path,
                        bytes,
                        history,
                        interpret,
                        resident_max_bytes,
                    } => IoCompletion::Open(open_encoded_streaming(
                        &path,
                        platform.as_ref(),
                        bytes,
                        history,
                        &job.cancellation,
                        DecodeOptions {
                            resident_max_bytes,
                            interpret,
                        },
                        |snapshot| {
                            let _ = job.prefix.try_send(snapshot);
                            (job.notify)();
                        },
                    )),
                    IoRequest::SaveCopy {
                        snapshot,
                        destination,
                        source,
                        bom,
                        encoding,
                    } => IoCompletion::Save((|| {
                        let _source = source
                            .as_ref()
                            .map(|source| guard_copy_source(source, &destination.path, platform.as_ref()))
                            .transpose()?;
                        if let Some(encoding) = encoding {
                            save_encoded_to_cancellable(
                                snapshot,
                                &destination.path,
                                &destination.condition,
                                bom,
                                platform.as_ref(),
                                &job.cancellation,
                                &encoding,
                            )
                        } else {
                            save_utf8_to_cancellable(
                                snapshot,
                                &destination.path,
                                &destination.condition,
                                bom,
                                platform.as_ref(),
                                &job.cancellation,
                            )
                        }
                    })()),
                    IoRequest::SaveEncoded {
                        snapshot,
                        destination,
                        bom,
                        encoding,
                    } => IoCompletion::Save(save_encoded_to_cancellable(
                        snapshot,
                        &destination.path,
                        &destination.condition,
                        bom,
                        platform.as_ref(),
                        &job.cancellation,
                        &encoding,
                    )),
                    IoRequest::OpenStreaming {
                        path,
                        bytes,
                        history,
                        resident_max_bytes,
                    } => IoCompletion::Open(open_encoded_streaming(
                        &path,
                        platform.as_ref(),
                        bytes,
                        history,
                        &job.cancellation,
                        DecodeOptions {
                            resident_max_bytes,
                            interpret: None,
                        },
                        |snapshot| {
                            let _ = job.prefix.try_send(snapshot);
                            (job.notify)();
                        },
                    )),
                    IoRequest::Open { path, bytes, history } => IoCompletion::Open(open_utf8_cancellable(
                        &path,
                        platform.as_ref(),
                        bytes,
                        history,
                        &job.cancellation,
                    )),
                    IoRequest::Save {
                        snapshot,
                        destination,
                        bom,
                    } => IoCompletion::Save(save_utf8_to_cancellable(
                        snapshot,
                        &destination.path,
                        &destination.condition,
                        bom,
                        platform.as_ref(),
                        &job.cancellation,
                    )),
                };
                let _ = job.reply.try_send(result);
                (job.notify)();
            }
        })?;
        Ok(Self { sender })
    }
    pub fn submit(&self, request: IoRequest, notify: Notification) -> Result<IoTicket, Box<IoRequest>> {
        self.submit_inner(request, notify, None)
    }
    pub fn submit_authorized(
        &self,
        request: IoRequest,
        grant: bareline_platform::RemoteReadGrant,
        action: bareline_platform::RemoteReadAction,
        notify: Notification,
    ) -> Result<IoTicket, Box<IoRequest>> {
        if read_request_path(&request).is_none() {
            return Err(Box::new(request));
        }
        self.submit_inner(request, notify, Some(ReadAuthorization::Grant(grant, action)))
    }
    pub fn submit_follow_read(
        &self,
        request: IoRequest,
        access: bareline_platform::RemoteReadAccess,
        notify: Notification,
    ) -> Result<IoTicket, Box<IoRequest>> {
        if access.action() != bareline_platform::RemoteReadAction::Follow
            || read_request_path(&request).is_none_or(|path| access.check(path).is_err())
        {
            return Err(Box::new(request));
        }
        self.submit_inner(request, notify, Some(ReadAuthorization::Access(access)))
    }
    fn submit_inner(
        &self,
        request: IoRequest,
        notify: Notification,
        authorization: Option<ReadAuthorization>,
    ) -> Result<IoTicket, Box<IoRequest>> {
        let (reply, receiver) = std::sync::mpsc::sync_channel(1);
        let (prefix_sender, prefix) = std::sync::mpsc::sync_channel(1);
        let cancellation = match &authorization {
            Some(ReadAuthorization::Grant(grant, _)) => {
                let grant = grant.clone();
                Cancellation::with_check(std::sync::Arc::new(move || grant.is_revoked()))
            }
            Some(ReadAuthorization::Access(access)) => {
                let access = access.clone();
                Cancellation::with_check(std::sync::Arc::new(move || access.is_revoked()))
            }
            None => Cancellation::default(),
        };
        self.sender
            .try_send(Job {
                authorization,
                cancellation: cancellation.clone(),
                request,
                reply,
                prefix: prefix_sender,
                notify,
            })
            .map_err(|error| match error {
                std::sync::mpsc::TrySendError::Full(job) | std::sync::mpsc::TrySendError::Disconnected(job) => {
                    Box::new(job.request)
                }
            })?;
        Ok(IoTicket {
            receiver,
            prefix,
            cancellation,
        })
    }
}

#[cfg(test)]
mod encoded_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    struct Platform;
    impl LocalFileSystem for Platform {
        fn guard_directory(&self, _: &std::path::Path) -> std::io::Result<std::sync::Arc<dyn Send + Sync>> {
            Ok(std::sync::Arc::new(()))
        }
        fn available_space(&self, _: &std::path::Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        fn open_sealed_read(&self, path: &std::path::Path) -> std::io::Result<std::fs::File> {
            std::fs::File::open(path)
        }
        fn identity(&self, f: &File) -> io::Result<FileIdentity> {
            let m = f.metadata()?;
            Ok(FileIdentity {
                volume: 1,
                file: 1,
                length: m.len(),
                modified: m.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64,
            })
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn prepare_commit(
            &self,
            staged: &Path,
            target: &Path,
            mode: bareline_platform::CommitMode,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> io::Result<bareline_platform::PreparedCommit> {
            bareline_platform::prepare_simulated_commit(self, staged, target, mode, cancellation)
        }
        fn commit_transaction(
            &self,
            transaction: bareline_platform::PreparedCommit,
        ) -> io::Result<bareline_platform::CommitReceipt> {
            bareline_platform::simulate_commit_transaction(self, transaction)
        }
        fn commit(&self, stage: &Path, target: &Path, _: bool) -> io::Result<()> {
            fs::copy(stage, target)?;
            fs::remove_file(stage)
        }
    }
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let p = std::env::temp_dir().join(format!(
                "bareline-codecs-{}-{}",
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
    #[test]
    fn destination_consent_carries_the_captured_version_into_commit() {
        let temp = Temp::new();
        let existing = temp.0.join("existing.txt");
        fs::write(&existing, b"old target").unwrap();
        let declined = preflight_destination(
            &existing,
            None,
            (7, 3),
            SaveOperation::SaveAs,
            &Platform,
            &Cancellation::default(),
        )
        .unwrap();
        assert!(declined.requires_overwrite_consent());
        assert!(declined.clone().approve(false).is_none());

        let approved = declined.approve(true).unwrap();
        fs::write(&existing, b"newer external target").unwrap();
        let document = Document::from_utf8("editor bytes", Budget::new(1024), Budget::new(0)).unwrap();
        let result = save_utf8_to_cancellable(
            document.snapshot(),
            &approved.path,
            &approved.condition,
            false,
            &Platform,
            &Cancellation::default(),
        );
        let Err(error @ FileError::Conflict { .. }) = result else {
            panic!("changed destination must return a typed recovery conflict")
        };
        let conflict = error.save_conflict().unwrap();
        assert_eq!(conflict.target.as_ref(), Some(&approved.path));
        assert_eq!(fs::read(&conflict.editor_version).unwrap(), b"editor bytes");
        assert_eq!(conflict.other_version.as_ref(), Some(&approved.path));
        assert_eq!(conflict.state, CommitState::Conflict);
        assert_eq!(fs::read(&existing).unwrap(), b"newer external target");

        let absent = temp.0.join("absent.txt");
        let prepared = preflight_destination(
            &absent,
            None,
            (7, 3),
            SaveOperation::SaveCopy,
            &Platform,
            &Cancellation::default(),
        )
        .unwrap()
        .approve(false)
        .unwrap();
        assert_eq!(prepared.condition, DestinationCondition::MustBeAbsent);
        assert_eq!(prepared.consent, DestinationConsent::NotRequired);
    }

    #[test]
    fn destination_preflight_cancels_while_fingerprinting() {
        struct GatedPlatform {
            entered: Mutex<std::sync::mpsc::SyncSender<()>>,
            release: Mutex<std::sync::mpsc::Receiver<()>>,
        }
        impl LocalFileSystem for GatedPlatform {
            fn identity(&self, file: &File) -> io::Result<FileIdentity> {
                let metadata = file.metadata()?;
                let _ = self.entered.lock().unwrap().send(());
                let _ = self.release.lock().unwrap().recv();
                Ok(FileIdentity {
                    volume: 1,
                    file: 1,
                    length: metadata.len(),
                    modified: 1,
                })
            }
            fn validate_target(&self, _: &Path) -> io::Result<()> {
                Ok(())
            }
            fn commit(&self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
                unreachable!()
            }
        }
        let temp = Temp::new();
        let path = temp.0.join("large.txt");
        fs::write(&path, vec![b'x'; 128 * 1024]).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let platform = Arc::new(GatedPlatform {
            entered: Mutex::new(entered_tx),
            release: Mutex::new(release_rx),
        });
        let cancellation = Cancellation::default();
        let worker_cancellation = cancellation.clone();
        let worker_platform = platform.clone();
        let worker_path = path.clone();
        let worker = std::thread::spawn(move || {
            preflight_destination(
                &worker_path,
                None,
                (9, 1),
                SaveOperation::SaveAs,
                worker_platform.as_ref(),
                &worker_cancellation,
            )
        });
        entered_rx.recv().unwrap();
        cancellation.cancel();
        release_tx.send(()).unwrap();
        assert!(matches!(worker.join().unwrap(), Err(FileError::Cancelled)));
    }
    #[test]
    fn unicode_streaming_prefix_save_and_refused_conversion_preserve_disk() {
        let temp = Temp::new();
        let path = temp.0.join("text.txt");
        let mut raw = Encoding::Utf16Le.bom().to_vec();
        raw.extend(
            crate::codecs::Encoder::new(Encoding::Utf16Le, false)
                .encode_text(&"A\r\n😀\r".repeat(30000))
                .unwrap(),
        );
        fs::write(&path, &raw).unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let mut prefix = false;
        let opened = open_encoded_streaming(
            &path,
            &Platform,
            budget.clone(),
            Budget::new(1024 * 1024),
            &Cancellation::default(),
            DecodeOptions {
                resident_max_bytes: 2 * 1024 * 1024,
                interpret: None,
            },
            |p| {
                prefix = true;
                assert!(!p.is_complete());
                assert!(p.len() < 30000 * 9);
            },
        )
        .unwrap();
        assert!(prefix);
        assert_eq!(opened.encoding.as_ref().unwrap().state.detected, Encoding::Utf16Le);
        let mut encoding = opened.encoding.unwrap();
        save_encoded_cancellable(
            opened.document.snapshot(),
            &path,
            Some(&opened.fingerprint),
            true,
            &Platform,
            &Cancellation::default(),
            &encoding,
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), raw);
        let expected = fingerprint(&path, &Platform, &Cancellation::default()).unwrap();
        encoding.state.convert_to(Encoding::Latin1);
        assert!(matches!(
            save_encoded_cancellable(
                opened.document.snapshot(),
                &path,
                Some(&expected),
                false,
                &Platform,
                &Cancellation::default(),
                &encoding
            ),
            Err(FileError::EncodingAt(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), raw);
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
        drop(encoding);
        drop(opened.document);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn streaming_quota_and_cancel_after_prefix_never_touch_source() {
        let temp = Temp::new();
        let path = temp.0.join("text.txt");
        let raw = vec![b'A'; 200000];
        fs::write(&path, &raw).unwrap();
        let cancel = Cancellation::default();
        let budget = Budget::new(1000000);
        assert!(matches!(
            open_encoded_streaming(
                &path,
                &Platform,
                budget.clone(),
                Budget::new(1000),
                &cancel,
                DecodeOptions {
                    resident_max_bytes: 1000000,
                    interpret: None
                },
                |_| cancel.cancel()
            ),
            Err(FileError::Cancelled)
        ));
        assert_eq!(budget.used(), 0);
        assert_eq!(fs::read(&path).unwrap(), raw);
        let budget = Budget::new(100000);
        assert!(
            open_encoded_streaming(
                &path,
                &Platform,
                budget.clone(),
                Budget::new(1000),
                &Cancellation::default(),
                DecodeOptions {
                    resident_max_bytes: 1000000,
                    interpret: None
                },
                |_| panic!("quota must refuse before prefix")
            )
            .is_err()
        );
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn paged_quota_receipt_resumes_and_staged_save_preserves_raw_without_materialization() {
        let temp = Temp::new();
        let path = temp.0.join("paged.txt");
        let raw = [255, 254, 65, 0, 0, 0xd8];
        fs::write(&path, raw).unwrap();
        let budget = Budget::new(8 * 1024 * 1024);
        let platform = std::sync::Arc::new(Platform);
        let request = PagedOpenRequest {
            path: path.clone(),
            bytes: budget.clone(),
            history: Budget::new(1000),
            cache: temp.0.clone(),
            options: DiskOptions {
                temp_quota_bytes: 8,
                interpret: None,
            },
            source_options: crate::source::SourceOptions {
                resident_max_bytes: 0,
                page_size_bytes: 4,
                page_cache_bytes: 32,
            },
        };
        let paused = match open_paged_encoded(request, platform.clone(), Cancellation::default(), |_| {
            panic!("quota before prefix")
        }) {
            TranscodeOutcome::Paused(p) => p,
            _ => panic!("expected quota pause"),
        };
        let mut had_prefix = false;
        let opened = match resume_transcode(*paused, 10000, platform.clone(), Cancellation::default(), |p| {
            had_prefix = true;
            assert!(!p.is_complete());
        }) {
            TranscodeOutcome::Complete(p) => p,
            _ => panic!("expected final paged document"),
        };
        assert!(had_prefix);
        let mut policy = PagedSavePolicy {
            store: opened.transcoded.store.clone(),
            generation: opened.transcoded.source.source().generation(),
            encoding: Encoding::Utf16Le,
            bom: true,
        };
        let saved = save_paged_cancellable(
            opened.transcoded.document.snapshot(),
            &path,
            Some(&opened.fingerprint),
            &policy,
            platform.as_ref(),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), raw);
        policy.encoding = Encoding::Utf8;
        assert!(matches!(
            save_paged_cancellable(
                opened.transcoded.document.snapshot(),
                &path,
                Some(&saved.fingerprint),
                &policy,
                platform.as_ref(),
                &Cancellation::default()
            ),
            Err(FileError::EncodingAt(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), raw);
        policy.encoding = Encoding::Utf16Le;
        let mut tampered = raw;
        tampered[2] = 66;
        fs::write(policy.store.original_path(), tampered).unwrap();
        assert!(matches!(
            save_paged_cancellable(
                opened.transcoded.document.snapshot(),
                &path,
                Some(&saved.fingerprint),
                &policy,
                platform.as_ref(),
                &Cancellation::default()
            ),
            Err(FileError::Transcode(DiskError::Changed))
        ));
        assert_eq!(
            fs::read(&path).unwrap(),
            raw,
            "sealed-cache corruption must preserve original"
        );
        drop(saved);
        drop(policy);
        drop(opened);
        assert_eq!(budget.used(), 0);
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
    }
}

#[cfg(test)]
#[path = "fault_transitions.rs"]
mod fault_transitions;

fn read_request_path(request: &IoRequest) -> Option<&Path> {
    match request {
        IoRequest::Open { path, .. } | IoRequest::OpenStreaming { path, .. } | IoRequest::OpenEncoded { path, .. } => {
            Some(path)
        }
        IoRequest::OpenPagedEncoded(request) => Some(&request.path),
        _ => None,
    }
}
#[cfg(test)]
mod authorized_read_tests {
    use super::*;
    use bareline_platform::{RemoteReadAccess, RemoteReadAction, RemoteReadGrant};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Platform {
        access: Option<RemoteReadAccess>,
        reads: Arc<AtomicUsize>,
    }
    impl LocalFileSystem for Platform {
        fn scoped_remote_read(&self, access: RemoteReadAccess) -> io::Result<Arc<dyn LocalFileSystem>> {
            Ok(Arc::new(Self {
                access: Some(access),
                reads: self.reads.clone(),
            }))
        }
        fn validate_source(&self, path: &Path) -> io::Result<()> {
            self.access
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "no action grant"))?
                .check(path)?;
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only capability"))
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
            Err(io::Error::other("writes unavailable"))
        }
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            Ok(FileIdentity {
                volume: 1,
                file: 1,
                length: file.metadata()?.len(),
                modified: 0,
            })
        }
    }
    fn request(path: &Path) -> IoRequest {
        IoRequest::Open {
            path: path.into(),
            bytes: Budget::new(1024 * 1024),
            history: Budget::new(1024 * 1024),
        }
    }
    fn result(ticket: &IoTicket) -> IoCompletion {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match ticket.try_recv() {
                Ok(result) => return result,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::yield_now();
                }
                Err(error) => panic!("{error:?}"),
            }
        }
    }
    #[test]
    fn grant_travels_to_only_its_read_job_and_never_enables_the_service() {
        let path = std::env::temp_dir().join(format!(
            "bareline-authorized-local-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, b"fixture").unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let service = IoService::new(Arc::new(Platform {
            access: None,
            reads: reads.clone(),
        }))
        .unwrap();
        let notify: Notification = Arc::new(|| {});
        let plain = service.submit(request(&path), notify.clone()).ok().unwrap();
        assert!(matches!(result(&plain), IoCompletion::Open(Err(_))));
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        let grant =
            RemoteReadGrant::after_consent(path.clone(), RemoteReadAction::Open, std::time::Duration::from_secs(10))
                .unwrap();
        let wrong = service
            .submit_authorized(
                request(&path.with_extension("other")),
                grant.clone(),
                RemoteReadAction::Open,
                notify.clone(),
            )
            .ok()
            .unwrap();
        assert!(matches!(result(&wrong), IoCompletion::Open(Err(_))));
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        let authorized = service
            .submit_authorized(request(&path), grant.clone(), RemoteReadAction::Open, notify.clone())
            .ok()
            .unwrap();
        assert!(matches!(result(&authorized), IoCompletion::Open(Ok(_))));
        assert!(reads.load(Ordering::SeqCst) > 0);
        let repeated = service
            .submit_authorized(request(&path), grant, RemoteReadAction::Open, notify.clone())
            .ok()
            .unwrap();
        assert!(matches!(result(&repeated), IoCompletion::Open(Err(_))));
        let plain_again = service.submit(request(&path), notify.clone()).ok().unwrap();
        assert!(matches!(result(&plain_again), IoCompletion::Open(Err(_))));
        let revoked =
            RemoteReadGrant::after_consent(path.clone(), RemoteReadAction::Open, std::time::Duration::from_secs(10))
                .unwrap();
        revoked.revoke();
        let ticket = service
            .submit_authorized(request(&path), revoked, RemoteReadAction::Open, notify)
            .ok()
            .unwrap();
        assert!(matches!(result(&ticket), IoCompletion::Open(Err(_))));
        fs::remove_file(path).unwrap();
    }
}
