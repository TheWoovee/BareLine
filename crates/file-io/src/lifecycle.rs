// SPDX-License-Identifier: MPL-2.0
//! Bounded UTF-8 Resident lifecycle. Call only on the I/O worker.
use crate::cancellation::Cancellation;
use crate::codecs::{
    Encoding,
    disk::{DiskError, DiskOptions, DiskTranscoder, PagedTranscoded},
    resident::{ResidentBuilder, ResidentEncoding, ResidentError},
};
use crate::owned_store::Reinterpreting;
use bareline_document::{Budget, Document, DocumentBuilder, DocumentSnapshot, TextOffset};
use bareline_platform::{CleanupResponsibility, CommitMode, CommitState, FileIdentity, LocalFileSystem};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    ops::ControlFlow,
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
    // Keep the user's spelling, made absolute lexically (FIO-11). It becomes the
    // document path shown in the title and Recent list; a canonical form would leak
    // `\\?\` or turn mapped drives into UNC, and fails where final-path queries are
    // unsupported. File identity comes from handles below, never from the spelling.
    let path = std::path::absolute(target)?;
    if path.file_name().is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "target has no file name").into());
    }
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
    ConflictAfterCommit(Box<PostCommitConflict>),
    ConflictAfterCreate {
        target: PathBuf,
        proposed: PathBuf,
        transaction: PathBuf,
    },
    /// Retained for existing presenters; saves ignore cancellation once committed.
    CancelledAfterCommit(Box<PostCommitCancellation>),
    VerificationAfterCommit(Box<PostCommitVerification>),
    Commit(Box<CommitFailure>),
}
// The commit-failure and post-commit records below are boxed inside `FileError`
// (QA-18): they carry up to four recovery paths plus two fingerprints, and keeping
// them inline made every `Result<_, FileError>` in the I/O layer at least 256 bytes.
// They are built only on rare save-recovery paths, so the allocation is off the
// hot path while every ordinary I/O result stays small.
/// The destination changed during replacement: the displaced file no longer matched
/// the fingerprint the save approved.
#[derive(Debug)]
pub struct PostCommitConflict {
    pub target: PathBuf,
    pub proposed: PathBuf,
    pub displaced: PathBuf,
    pub transaction: PathBuf,
    pub approved: Fingerprint,
    pub actual_displaced: Fingerprint,
}
/// Cancellation arrived after the replacement or creation had already committed.
#[derive(Debug)]
pub struct PostCommitCancellation {
    pub target: PathBuf,
    pub proposed: PathBuf,
    pub displaced: Option<PathBuf>,
    pub transaction: PathBuf,
}
/// The committed result failed verification and needs recovery.
#[derive(Debug)]
pub struct PostCommitVerification {
    pub target: PathBuf,
    pub proposed: PathBuf,
    pub displaced: Option<PathBuf>,
    pub transaction: PathBuf,
    pub reason: String,
}
/// The commit itself failed; the transaction files it retained are listed.
#[derive(Debug)]
pub struct CommitFailure {
    pub staged: PathBuf,
    pub proposed: Option<PathBuf>,
    pub displaced: Option<PathBuf>,
    pub transaction: Option<PathBuf>,
    pub error: io::Error,
}
/// Plain-language status text shown to the user (UI-03): what happened, where the
/// user's text is, and what to do next. `Debug` stays for diagnostics.
impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transcode(error) => write!(f, "File conversion stopped: {error}."),
            Self::EncodingAt(failure) => write!(
                f,
                "{} at text bytes {}..{}.",
                failure.reason, failure.range.start.0, failure.range.end.0
            ),
            Self::Encoding(error) => write!(f, "Encoding operation was not applied: {error}."),
            Self::Cancelled => f.write_str("File operation cancelled."),
            Self::IncompleteSource => f.write_str("File is still loading; wait before saving."),
            Self::StreamingRequired => {
                f.write_str("This file exceeds the configured resident limit; reopen with paged storage.")
            }
            Self::UnsupportedEncoding => {
                f.write_str("This file needs an encoding that is not available in this build.")
            }
            Self::Changed => f.write_str("The file changed during the operation. Your edits remain in memory."),
            Self::Conflict { target, proposed, .. } => write!(
                f,
                "The destination changed before replacement. The current file is at {}; your editor version remains at {}.",
                target.display(),
                proposed.display()
            ),
            Self::ConflictAfterCommit(conflict) => write!(
                f,
                "The destination changed during replacement. Compare {} with the preserved other version at {}. Your editor version remains at {}; save it elsewhere or retain the other version.",
                conflict.target.display(),
                conflict.displaced.display(),
                conflict.proposed.display()
            ),
            Self::ConflictAfterCreate { target, proposed, .. } => write!(
                f,
                "The new destination changed while the save completed. The current file is at {}; your editor version remains at {}.",
                target.display(),
                proposed.display()
            ),
            Self::CancelledAfterCommit(cancellation) => match &cancellation.displaced {
                Some(displaced) => write!(
                    f,
                    "Save cancellation arrived after replacement. No success was recorded; inspect {}. Your editor version is at {}, and the displaced version is at {}.",
                    cancellation.target.display(),
                    cancellation.proposed.display(),
                    displaced.display()
                ),
                None => write!(
                    f,
                    "Save cancellation arrived after the new file was created. No success was recorded; inspect {} or recover your editor version from {}.",
                    cancellation.target.display(),
                    cancellation.proposed.display()
                ),
            },
            Self::VerificationAfterCommit(verification) => match &verification.displaced {
                Some(displaced) => write!(
                    f,
                    "Save replacement needs recovery because verification failed ({}). Inspect {}; editor version: {}; displaced version: {}.",
                    verification.reason,
                    verification.target.display(),
                    verification.proposed.display(),
                    displaced.display()
                ),
                None => write!(
                    f,
                    "The created file needs recovery because verification failed ({}). Inspect {}; editor version: {}.",
                    verification.reason,
                    verification.target.display(),
                    verification.proposed.display()
                ),
            },
            Self::Commit(failure) => match (&failure.proposed, &failure.displaced) {
                (Some(proposed), Some(displaced)) => write!(
                    f,
                    "Save could not finish the replacement ({}). Retained transaction files: {}, {}, and {}",
                    failure.error,
                    failure.staged.display(),
                    proposed.display(),
                    displaced.display()
                ),
                (Some(proposed), None) => write!(
                    f,
                    "Save could not replace the destination ({}). Staged copies: {} and {}",
                    failure.error,
                    failure.staged.display(),
                    proposed.display()
                ),
                _ => write!(
                    f,
                    "Save could not replace the destination ({}). Staged copy: {}",
                    failure.error,
                    failure.staged.display()
                ),
            },
            Self::Io(error) => write!(f, "File operation failed: {error}"),
            Self::Budget => f.write_str("Document memory budget reached."),
        }
    }
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
            Self::ConflictAfterCommit(conflict) => Some(SaveConflict {
                target: Some(conflict.target.clone()),
                editor_version: conflict.proposed.clone(),
                other_version: Some(conflict.displaced.clone()),
                transaction: conflict.transaction.clone(),
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
            Self::CancelledAfterCommit(cancellation) => Some(SaveConflict {
                target: Some(cancellation.target.clone()),
                editor_version: cancellation.proposed.clone(),
                other_version: cancellation.displaced.clone(),
                transaction: cancellation.transaction.clone(),
                state: CommitState::Conflict,
                verified: true,
            }),
            Self::VerificationAfterCommit(verification) => Some(SaveConflict {
                target: Some(verification.target.clone()),
                editor_version: verification.proposed.clone(),
                other_version: verification.displaced.clone(),
                transaction: verification.transaction.clone(),
                state: CommitState::Conflict,
                verified: true,
            }),
            Self::Commit(failure) => match &**failure {
                CommitFailure {
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
            },
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
                let mut verified = recovery.verified;
                if recovery.verified && recovery.state == CommitState::CleanupPending {
                    // One cleanup record that cannot be resumed is reported for review as
                    // an unverified transaction; it never aborts the listing (REC-14).
                    match (platform.resume_commit_cleanup(&recovery), recovery.target.clone()) {
                        (Ok(Some(receipt)), Some(target)) => {
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
                        (Ok(None), _) => {}
                        (Ok(Some(_)), None) | (Err(_), _) => verified = false,
                    }
                }
                result.conflicts.push(SaveConflict {
                    target: recovery.target,
                    editor_version: recovery.proposed,
                    other_version: recovery.displaced,
                    transaction: recovery.journal,
                    state: recovery.state,
                    verified,
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
    /// Set by a recovery restore that had to fall back to an older checkpoint: the
    /// acknowledged revision whose root was missing and so was not restored (REC-07).
    pub unrestored_revision: Option<u64>,
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
    match start_paged_open(request, &platform, &cancellation) {
        Err(e) => TranscodeOutcome::Failed(e),
        Ok(paused) => run_transcode(paused, platform, cancellation, on_prefix),
    }
}
/// Validate and open the source; no transcode step has run yet.
fn start_paged_open(
    request: PagedOpenRequest,
    platform: &std::sync::Arc<dyn LocalFileSystem>,
    cancellation: &Cancellation,
) -> Result<PausedTranscode, FileError> {
    cancellation.check()?;
    platform.validate_source(&request.path)?;
    let file = File::open(&request.path)?;
    let job = DiskTranscoder::new(
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
    .map_err(FileError::Transcode)?;
    Ok(PausedTranscode {
        path: request.path,
        error: DiskError::NotComplete,
        job,
        bytes: request.bytes,
        history: request.history,
        source_options: request.source_options,
    })
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
        match transcode_slice(paused, &platform, &cancellation, &mut on_prefix, usize::MAX) {
            ControlFlow::Break(outcome) => return outcome,
            ControlFlow::Continue(next) => paused = *next,
        }
    }
}
/// Run at most `steps` bounded (64 KiB) transcode steps, checking cancellation
/// before each, so the I/O worker can interleave other work between slices of
/// a multi-GB transcode (FIO-14). `Continue` is a job not complete after the
/// allowed steps; the caller schedules the rest.
fn transcode_slice(
    mut paused: PausedTranscode,
    platform: &std::sync::Arc<dyn LocalFileSystem>,
    cancellation: &Cancellation,
    on_prefix: &mut impl FnMut(DocumentSnapshot),
    steps: usize,
) -> ControlFlow<TranscodeOutcome, Box<PausedTranscode>> {
    match step_transcoder(&mut paused.job, cancellation, on_prefix, steps) {
        ControlFlow::Continue(()) => ControlFlow::Continue(Box::new(paused)),
        ControlFlow::Break(Ok(())) => ControlFlow::Break(finish_transcode(paused, platform.clone())),
        ControlFlow::Break(Err(FileError::Transcode(e @ DiskError::Quota { .. }))) => {
            paused.error = e;
            ControlFlow::Break(TranscodeOutcome::Paused(Box::new(paused)))
        }
        ControlFlow::Break(Err(e)) => ControlFlow::Break(TranscodeOutcome::Failed(e)),
    }
}
/// Run at most `steps` transcode steps, checking cancellation before each.
/// `Break(Ok(()))` is a completed transcode, `Continue` one with steps left.
fn step_transcoder(
    job: &mut DiskTranscoder,
    cancellation: &Cancellation,
    on_prefix: &mut impl FnMut(DocumentSnapshot),
    steps: usize,
) -> ControlFlow<Result<(), FileError>> {
    for _ in 0..steps {
        if cancellation.check().is_err() {
            return ControlFlow::Break(Err(FileError::Cancelled));
        }
        match job.step() {
            Err(e) => return ControlFlow::Break(Err(FileError::Transcode(e))),
            Ok(progress) => {
                if let Some(preview) = job.take_preview() {
                    on_prefix(preview);
                }
                if progress.complete {
                    return ControlFlow::Break(Ok(()));
                }
            }
        }
    }
    ControlFlow::Continue(())
}
fn finish_transcode(paused: PausedTranscode, platform: std::sync::Arc<dyn LocalFileSystem>) -> TranscodeOutcome {
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
                unrestored_revision: None,
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
impl From<ResidentError> for FileError {
    fn from(e: ResidentError) -> Self {
        Self::Encoding(e)
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
        #[cfg(test)]
        FINGERPRINTED_BYTES.with(|bytes| bytes.set(bytes.get() + count as u64));
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
#[cfg(test)]
thread_local! {
    /// Bytes `fingerprint` read on this thread, so tests count full-file read passes (FIO-07).
    static FINGERPRINTED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
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
/// Provenance-quota or budget exhaustion is a size outcome, not an encoding
/// failure: callers fall back to paged storage (FIO-01).
fn resident_open_error(error: ResidentError) -> FileError {
    match error {
        ResidentError::Limit | ResidentError::Document(bareline_document::Error::BudgetExceeded) => {
            FileError::StreamingRequired
        }
        error => FileError::Encoding(error),
    }
}
/// Interpret As for a resident document: reinterpret its retained original bytes.
/// Provenance-quota or budget exhaustion is a size outcome (StreamingRequired)
/// that the caller answers with a paged reinterpretation, not an encoding error.
fn interpret_resident(request: InterpretRequest, cancellation: &Cancellation) -> Result<Opened, FileError> {
    cancellation.check()?;
    if request.dirty && !request.discard_confirmed {
        return Err(FileError::Encoding(ResidentError::DirtyInterpret));
    }
    let (document, encoding) = request
        .source
        .reinterpret_streaming(request.target, request.bytes, request.history, 64 * 1024 * 1024, || {
            cancellation.check().map_err(|_| ResidentError::Cancelled)
        })
        .map_err(|e| match e {
            ResidentError::Cancelled => FileError::Cancelled,
            e => resident_open_error(e),
        })?;
    cancellation.check()?;
    Ok(Opened {
        document,
        bom: encoding.state.bom,
        encoding: Some(encoding),
        path: request.path,
        fingerprint: request.fingerprint,
    })
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
    .map_err(resident_open_error)?;
    let mut hash = Sha256::new();
    let mut total = first as u64;
    hash.update(&buffer[..first]);
    builder.push(&buffer[..first]).map_err(resident_open_error)?;
    if before != platform.identity(&file)? || before != platform.identity(&File::open(path)?)? {
        return Err(FileError::Changed);
    }
    cancellation.check()?;
    on_prefix(builder.prefix().map_err(resident_open_error)?);
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
        builder.push(&buffer[..count]).map_err(resident_open_error)?;
    }
    if total != before.length
        || before != platform.identity(&file)?
        || before != platform.identity(&File::open(path)?)?
    {
        return Err(FileError::Changed);
    }
    cancellation.check()?;
    let (document, encoding) = builder.finish().map_err(resident_open_error)?;
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
impl Staged {
    /// Keep the stage as a reported recovery copy. It leaves the `.tmp` grammar so
    /// `sweep_dead_stages` never deletes a copy the user was told about.
    fn keep(&mut self) -> PathBuf {
        self.retain = true;
        let kept = self.path.with_extension("kept");
        if fs::rename(&self.path, &kept).is_ok() {
            self.path = kept;
        }
        self.path.clone()
    }
}
impl Drop for Staged {
    fn drop(&mut self) {
        if !self.retain {
            let _ = fs::remove_file(&self.path);
        }
    }
}
/// Parent folders already swept for dead stages by this process.
static SWEPT_STAGE_PARENTS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
/// Process id of a `.bareline-<pid>-<n>.tmp` stage name.
fn stage_owner(name: &str) -> Option<u32> {
    let (pid, serial) = name.strip_prefix(".bareline-")?.strip_suffix(".tmp")?.split_once('-')?;
    serial.parse::<u64>().ok()?;
    pid.parse().ok()
}
/// A stage untouched for this long is no longer being written. Pids are only
/// meaningful on this machine, and a shared folder can hold another machine's
/// in-flight stage whose pid reads as dead here, so fresh stages are never swept.
const STAGE_SWEEP_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
/// Remove stages a crashed process left beside a destination (REC-11), once per folder
/// per process. Only a regular file whose owner is provably gone and that has not been
/// modified for `STAGE_SWEEP_MIN_AGE` is removed, and only a bounded prefix of the
/// listing is examined so a large folder never stalls a save.
fn sweep_dead_stages(parent: &Path, platform: &dyn LocalFileSystem) {
    {
        let mut swept = SWEPT_STAGE_PARENTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if swept.iter().any(|swept| swept == parent) {
            return;
        }
        if swept.len() >= 256 {
            swept.clear();
        }
        swept.push(parent.to_path_buf());
    }
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.take(4096).flatten() {
        let name = entry.file_name();
        let Some(owner) = name.to_str().and_then(stage_owner) else {
            continue;
        };
        // A zero creation time asks for pid-only liveness: only a missing or exited
        // process reports Dead; a live or reused id is never treated as gone.
        // A modification time in the future (clock skew) never counts as old.
        let stale = || {
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age >= STAGE_SWEEP_MIN_AGE))
        };
        if owner != std::process::id()
            && entry.file_type().is_ok_and(|kind| kind.is_file())
            && stale()
            && platform.cache_process_liveness(owner, 0) == bareline_platform::ProcessLiveness::Dead
        {
            let _ = fs::remove_file(entry.path());
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
                            error.to_string(),
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
    sweep_dead_stages(parent, platform);
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
        struct Writer<'a, 'f> {
            file: &'a mut io::BufWriter<&'f mut File>,
            hash: &'a mut Sha256,
            cancellation: &'a Cancellation,
        }
        impl Write for Writer<'_, '_> {
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
        // Large buffered writes instead of one write per piece or chunk (FIO-08). The
        // hash covers exactly the bytes handed to the stage.
        let mut buffered = io::BufWriter::with_capacity(crate::owned_store::WRITE_BUFFER, &mut file);
        let result = emit(&mut Writer {
            file: &mut buffered,
            hash: &mut staged_hash,
            cancellation,
        });
        cancellation.check()?;
        result?;
        cancellation.check()?;
        buffered.flush()?;
        drop(buffered);
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
            return Err(FileError::Commit(Box::new(CommitFailure {
                staged: staged.keep(),
                proposed: None,
                displaced: None,
                transaction: None,
                error,
            })));
        }
    };
    let proposed_recovery = transaction.proposed_path.clone();
    let displaced_recovery = transaction.displaced_path.clone();
    let transaction_recovery = transaction.journal_path.clone();
    // Validate before commit (FIO-17): a replacement that cannot retain both versions
    // is refused while the target is still untouched, never reported after replacing it.
    if mode == CommitMode::Replace && (proposed_recovery.is_none() || displaced_recovery.is_none()) {
        let _ = platform.abort_commit(transaction);
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "replacement transaction would not retain both versions",
        )
        .into());
    }
    // A copy the provider hashed while making it (and guards against writers since)
    // shows now, before the target is touched, whether it holds the written bytes; it
    // is then not read again after the commit (FIO-07). A stage changed after it was
    // written is refused here instead of being committed and reported afterwards.
    let proposed_hashed = transaction.proposed_sha256.is_some();
    if transaction.proposed_sha256.is_some_and(|copied| copied != written_hash) {
        let _ = platform.abort_commit(transaction);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the save stage changed before it was committed; the file was not replaced",
        )
        .into());
    }
    let unchanged = match condition {
        DestinationCondition::ReplaceCaptured(expected) => match fingerprint(target, platform, cancellation) {
            Ok(current) => &current == expected,
            Err(FileError::Cancelled) => {
                // Nothing was committed: drop the prepared recovery copy (FIO-10).
                let _ = platform.abort_commit(transaction);
                return Err(FileError::Cancelled);
            }
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
            if mode == CommitMode::CreateNew
                && error.kind() == io::ErrorKind::AlreadyExists
                && let (Some(proposed), Some(transaction)) = (proposed_recovery.clone(), transaction_recovery.clone())
            {
                return Err(FileError::ConflictAfterCreate {
                    target: target.to_path_buf(),
                    proposed,
                    transaction,
                });
            }
            return Err(FileError::Commit(Box::new(CommitFailure {
                staged: staged.keep(),
                proposed: proposed_recovery,
                displaced: displaced_recovery,
                transaction: transaction_recovery,
                error,
            })));
        }
    };
    // The target is replaced from here on. Cancellation is ignored (FIO-10): only the
    // verified outcome below is reported, never a cancel that arrived too late.
    #[cfg(test)]
    fault_transitions::hit(fault_transitions::Point::AfterReplace)?;
    #[cfg(feature = "qa-faults")]
    crate::qa_faults::hit("AfterReplace", target)?;
    let postcommit_error = |reason: String| {
        FileError::VerificationAfterCommit(Box::new(PostCommitVerification {
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
        }))
    };
    if mode == CommitMode::Replace && (receipt.proposed.is_none() || receipt.displaced.is_none()) {
        // The provider replaced the target but lost a version it prepared: report the
        // committed transaction for review instead of a failed save.
        let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
        return Err(postcommit_error(
            "replacement receipt did not retain both versions".into(),
        ));
    }
    let new_fingerprint = match fingerprint(target, platform, &Cancellation::default()) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
            return Err(postcommit_error(error.to_string()));
        }
    };
    #[cfg(test)]
    fault_transitions::hit(fault_transitions::Point::TargetFingerprintChecked)?;
    let target_is_output = new_fingerprint.identity == receipt.target && new_fingerprint.sha256 == written_hash;
    if let DestinationCondition::ReplaceCaptured(approved) = condition {
        let proposed = receipt.proposed.as_ref().expect("replacement receipt checked");
        let displaced = receipt.displaced.as_ref().expect("replacement receipt checked");
        // A hashed copy was checked before commit and nobody could write it since:
        // confirm only that its name still holds it. Any other copy is read again.
        let proposed_changed = if proposed_hashed {
            File::open(&proposed.path)
                .and_then(|file| platform.identity(&file))
                .map(|identity| identity != proposed.identity)
                .map_err(FileError::from)
        } else {
            fingerprint(&proposed.path, platform, &Cancellation::default())
                .map(|actual| actual.identity != proposed.identity || actual.sha256 != written_hash)
        };
        let proposed_changed = match proposed_changed {
            Ok(changed) => changed,
            Err(error) => {
                let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
                return Err(postcommit_error(error.to_string()));
            }
        };
        let actual_displaced = match fingerprint(&displaced.path, platform, &Cancellation::default()) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
                return Err(postcommit_error(error.to_string()));
            }
        };
        // Renames on FAT-family volumes and in-place rewrites do not keep the file
        // index, so those strategies must retain exactly the approved bytes instead.
        let displaced_is_approved = if receipt.strategy == bareline_platform::SaveStrategy::Transactional {
            &actual_displaced == approved
        } else {
            actual_displaced.identity.length == approved.identity.length && actual_displaced.sha256 == approved.sha256
        };
        if proposed_changed
            || actual_displaced.identity != displaced.identity
            || !displaced_is_approved
            || !target_is_output
        {
            let _ = platform.mark_commit_state(&receipt, CommitState::Conflict);
            return Err(FileError::ConflictAfterCommit(Box::new(PostCommitConflict {
                target: target.to_path_buf(),
                proposed: proposed.path.clone(),
                displaced: displaced.path.clone(),
                transaction: receipt.journal.clone().unwrap_or_default(),
                approved: approved.clone(),
                actual_displaced,
            })));
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
/// Which worker runs a request (FIO-14). Saves and the small save-maintenance
/// requests have their own worker, so they never queue behind opens,
/// transcodes, spills or reinterpretation, which can take minutes on multi-GB
/// files. Cancellation needs no lane: `IoTicket::cancel` sets the flag that
/// running work polls between bounded steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lane {
    Save = 0,
    Bulk = 1,
}
/// Requests each lane accepts beyond the running one (a transcode between
/// slices counts); submission then fails instead of blocking the UI.
const LANE_DEPTH: usize = 16;
/// An idle save worker exits after this long and starts again with the next
/// save, so a session that saved once does not keep a second idle thread.
const SAVE_WORKER_IDLE: std::time::Duration = std::time::Duration::from_secs(30);
/// 64 KiB steps a sliced transcode (open, resumed open, paged reinterpretation
/// and its sealed-store validation, spill baseline) runs before it goes back
/// behind other queued bulk work; with nothing queued it resumes at once.
#[cfg(not(test))]
const TRANSCODE_SLICE_STEPS: usize = 16;
#[cfg(test)]
const TRANSCODE_SLICE_STEPS: usize = 2;
impl IoRequest {
    fn lane(&self) -> Lane {
        match self {
            Self::Save { .. }
            | Self::SaveEncoded { .. }
            | Self::SaveCopy { .. }
            | Self::RetrySaveCleanup { .. }
            | Self::InspectSaveRecovery { .. } => Lane::Save,
            _ => Lane::Bulk,
        }
    }
    /// Files this request reads or replaces, as `ordering_key`s. Requests
    /// sharing one run in submission order across both lanes.
    fn ordering_paths(&self) -> Vec<OrderingKey> {
        let paths: Vec<&Path> = match self {
            Self::Save { destination, .. } | Self::SaveEncoded { destination, .. } => vec![destination.path.as_path()],
            Self::SaveCopy {
                destination, source, ..
            } => std::iter::once(destination.path.as_path())
                .chain(source.as_deref())
                .collect(),
            Self::RetrySaveCleanup { cleanup } => vec![cleanup.target.as_path()],
            Self::Open { path, .. } | Self::OpenStreaming { path, .. } | Self::OpenEncoded { path, .. } => {
                vec![path.as_path()]
            }
            Self::OpenPagedEncoded(request) => vec![request.path.as_path()],
            Self::ResumeTranscode { paused, .. } => vec![paused.path.as_path()],
            Self::Interpret(request) => vec![request.path.as_path()],
            Self::InterpretPaged(request) => vec![request.path.as_path()],
            Self::SpillOwnedResident { original, .. } => original.iter().map(|(path, _)| path.as_path()).collect(),
            Self::InspectSaveRecovery { .. } | Self::SpillResident { .. } | Self::RestorePagedRecovery { .. } => {
                Vec::new()
            }
        };
        paths.into_iter().map(ordering_key).collect()
    }
}
/// Deliberately coarse identity of a file: its lowercased name and the name of
/// its directory. Spellings of one file that differ in case, separators, a
/// verbatim prefix or the route to that directory (mapped drive or UNC share)
/// still order, while same-named files in differently named directories (two
/// `app.log`s) do not wait for each other. An over-match only serializes two
/// unrelated requests. Aliases with another file or directory name (hard links,
/// 8.3 names, junctions) are not ordered; the editor reloads and saves a
/// document by its own path.
#[derive(Clone, Debug, PartialEq, Eq)]
struct OrderingKey {
    name: String,
    /// `None` at a drive or share root, which a mapped drive can make any
    /// directory, so it matches every directory.
    directory: Option<String>,
}
impl OrderingKey {
    fn overlaps(&self, other: &Self) -> bool {
        self.name == other.name
            && (self.directory.is_none() || other.directory.is_none() || self.directory == other.directory)
    }
}
fn ordering_key(path: &Path) -> OrderingKey {
    fn lowercase_name(path: &Path) -> Option<String> {
        path.file_name().map(|name| name.to_string_lossy().to_lowercase())
    }
    OrderingKey {
        name: lowercase_name(path).unwrap_or_else(|| path.as_os_str().to_string_lossy().to_lowercase()),
        directory: path.parent().and_then(lowercase_name),
    }
}
/// A transcode between slices, holding everything its request would need.
struct Transcoding {
    stage: Stage,
    platform: std::sync::Arc<dyn LocalFileSystem>,
    cancellation: Cancellation,
    reply: std::sync::mpsc::SyncSender<IoCompletion>,
    prefix: std::sync::mpsc::SyncSender<DocumentSnapshot>,
    notify: Notification,
}
/// What a sliced transcode is for, and so what its completion becomes.
enum Stage {
    /// An open or resumed open: previews go to the ticket, a quota stop pauses.
    Open(Box<PausedTranscode>),
    /// Encoding reinterpretation of a paged document's retained original: the
    /// sealed store's validation, then the transcode, both sliced. The request
    /// keeps that original's store alive until the transcode finishes.
    Reinterpret(Reinterpreting, Box<InterpretPagedRequest>),
    /// The original-file baseline of a memory spill, which must still match the
    /// fingerprint; the spill's segments are written once it is ready.
    SpillBaseline(Box<PausedTranscode>, Fingerprint, Box<OwnedSpill>),
}
impl Transcoding {
    /// Run one slice; `Some` is the rest of the transcode, to be requeued.
    fn run(self, steps: usize) -> Option<Transcoding> {
        let Self {
            stage,
            platform,
            cancellation,
            reply,
            prefix,
            notify,
        } = self;
        let next = match stage {
            Stage::Open(paused) => {
                let mut on_prefix = |snapshot: DocumentSnapshot| {
                    let _ = prefix.try_send(snapshot);
                    notify();
                };
                match transcode_slice(*paused, &platform, &cancellation, &mut on_prefix, steps) {
                    ControlFlow::Break(outcome) => ControlFlow::Break(IoCompletion::Transcode(outcome)),
                    ControlFlow::Continue(paused) => ControlFlow::Continue(Stage::Open(paused)),
                }
            }
            Stage::Reinterpret(Reinterpreting::Validating(mut validation), request) => {
                match validation.step(steps, &cancellation) {
                    Ok(false) => {
                        ControlFlow::Continue(Stage::Reinterpret(Reinterpreting::Validating(validation), request))
                    }
                    // Validated: the transcode starts behind whatever queued meanwhile.
                    Ok(true) => match crate::owned_store::start_reinterpret_transcode(
                        *validation,
                        &request,
                        &platform,
                        &cancellation,
                    ) {
                        Ok(transcode) => ControlFlow::Continue(Stage::Reinterpret(
                            Reinterpreting::Transcoding(Box::new(transcode)),
                            request,
                        )),
                        Err(error) => ControlFlow::Break(IoCompletion::Transcode(TranscodeOutcome::Failed(error))),
                    },
                    Err(error) => ControlFlow::Break(IoCompletion::Transcode(TranscodeOutcome::Failed(match error {
                        DiskError::Cancelled => FileError::Cancelled,
                        error => FileError::Transcode(error),
                    }))),
                }
            }
            Stage::Reinterpret(Reinterpreting::Transcoding(mut transcode), request) => {
                match step_transcoder(&mut transcode.job, &cancellation, &mut |_: DocumentSnapshot| {}, steps) {
                    ControlFlow::Continue(()) => {
                        ControlFlow::Continue(Stage::Reinterpret(Reinterpreting::Transcoding(transcode), request))
                    }
                    ControlFlow::Break(stepped) => {
                        let result =
                            stepped.and_then(|()| transcode.finish(&request, platform.clone(), cancellation.clone()));
                        ControlFlow::Break(IoCompletion::Transcode(match result {
                            Ok(transcoded) => TranscodeOutcome::Complete(Box::new(PagedOpened {
                                recovery_origin: None,
                                recovered_resident: None,
                                unrestored_revision: None,
                                transcoded,
                                path: request.path,
                                fingerprint: request.fingerprint,
                            })),
                            Err(error) => TranscodeOutcome::Failed(error),
                        }))
                    }
                }
            }
            Stage::SpillBaseline(paused, expected, spill) => {
                match transcode_slice(*paused, &platform, &cancellation, &mut |_: DocumentSnapshot| {}, steps) {
                    ControlFlow::Continue(paused) => {
                        ControlFlow::Continue(Stage::SpillBaseline(paused, expected, spill))
                    }
                    ControlFlow::Break(outcome) => {
                        let baseline = match outcome {
                            TranscodeOutcome::Complete(opened) if opened.fingerprint == expected => {
                                Ok(opened.transcoded)
                            }
                            TranscodeOutcome::Complete(_) => Err(FileError::Changed),
                            TranscodeOutcome::Failed(error) => Err(error),
                            TranscodeOutcome::Paused(paused) => Err(FileError::Transcode(paused.error)),
                        };
                        ControlFlow::Break(spill.finish(baseline, None, &platform, &cancellation))
                    }
                }
            }
        };
        match next {
            ControlFlow::Continue(stage) => Some(Self {
                stage,
                platform,
                cancellation,
                reply,
                prefix,
                notify,
            }),
            ControlFlow::Break(completion) => {
                let _ = reply.try_send(completion);
                notify();
                None
            }
        }
    }
}
/// The rest of a `SpillOwnedResident` request once its baseline is ready.
struct OwnedSpill {
    plan: bareline_document::spill::SpillPlan,
    captured: DocumentSnapshot,
    cache: PathBuf,
    quota: u64,
    options: crate::source::SourceOptions,
    bytes: Budget,
}
impl OwnedSpill {
    fn finish(
        self,
        baseline: Result<PagedTranscoded, FileError>,
        encoding: Option<&ResidentEncoding>,
        platform: &std::sync::Arc<dyn LocalFileSystem>,
        cancellation: &Cancellation,
    ) -> IoCompletion {
        let Self {
            plan,
            captured,
            cache,
            quota,
            options,
            bytes,
        } = self;
        let result = baseline.and_then(|baseline| {
            let source = baseline.source.source();
            let prepared = crate::owned_store::prepare_segments(
                plan,
                encoding.map(|encoding| (encoding, &source)),
                &cache,
                quota - quota / 2,
                platform.clone(),
                options,
                bytes,
                cancellation,
            )?;
            Ok((baseline, Some(prepared)))
        });
        IoCompletion::ResidentSpilled { captured, result }
    }
}
enum Work {
    Request(Box<Job>),
    Transcoding(Transcoding),
}
struct Queued {
    sequence: u64,
    work: Work,
}
#[derive(Default)]
struct LaneState {
    queues: [VecDeque<Queued>; 2],
    /// Ordering paths of every accepted request that has not completed:
    /// queued, running, or requeued between transcode slices.
    unfinished: BTreeMap<u64, Vec<OrderingKey>>,
    next_sequence: u64,
    /// Lanes whose worker is running; the save worker starts with the first
    /// save and exits when idle, so a session that is not saving keeps one
    /// idle I/O thread.
    started: [bool; 2],
    closed: bool,
}
impl LaneState {
    /// Ordering guarantees (FIO-14):
    /// - Requests that share a file (by `ordering_key`) run in submission
    ///   order, whichever lane they are in, so a save never races its own
    ///   pending open or reload and a reload submitted after a save reads the
    ///   saved file.
    /// - Each lane has one worker, so saves never overlap one another (two
    ///   documents targeting one path still commit in submission order).
    /// - Requests on different paths may overlap (different lanes) or overtake
    ///   a request that is waiting for its path (same lane).
    ///
    /// The oldest unfinished request is never held, and a lane only passes over
    /// held requests, so every accepted request eventually runs.
    fn runnable(&self, sequence: u64) -> bool {
        let Some(keys) = self.unfinished.get(&sequence) else {
            return true;
        };
        self.unfinished
            .range(..sequence)
            .all(|(_, earlier)| !earlier.iter().any(|key| keys.iter().any(|mine| mine.overlaps(key))))
    }
    fn take(&mut self, lane: Lane) -> Option<Queued> {
        let index = self.queues[lane as usize]
            .iter()
            .position(|queued| self.runnable(queued.sequence))?;
        self.queues[lane as usize].remove(index)
    }
}
struct Lanes {
    state: std::sync::Mutex<LaneState>,
    changed: std::sync::Condvar,
    /// How long the save worker waits for work before exiting.
    save_idle: std::time::Duration,
}
impl Lanes {
    fn lock(&self) -> std::sync::MutexGuard<'_, LaneState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }
    fn run(&self, lane: Lane, platform: &std::sync::Arc<dyn LocalFileSystem>) {
        loop {
            let queued = {
                let mut state = self.lock();
                // Set when the save queue empties: bulk progress also wakes
                // this worker and must not restart its idle time.
                let mut idle_until = None;
                loop {
                    if let Some(queued) = state.take(lane) {
                        break queued;
                    }
                    let idle = state.queues[lane as usize].is_empty();
                    if state.closed && idle {
                        return;
                    }
                    if lane == Lane::Save && idle {
                        let until = *idle_until.get_or_insert_with(|| std::time::Instant::now() + self.save_idle);
                        let now = std::time::Instant::now();
                        if now >= until {
                            // The next save starts a worker again (`IoService::start`).
                            state.started[lane as usize] = false;
                            return;
                        }
                        state = self
                            .changed
                            .wait_timeout(state, until - now)
                            .unwrap_or_else(|error| error.into_inner())
                            .0;
                    } else {
                        idle_until = None;
                        state = self.changed.wait(state).unwrap_or_else(|error| error.into_inner());
                    }
                }
            };
            let rest = {
                let _unwind = Running {
                    lanes: self,
                    lane,
                    sequence: queued.sequence,
                };
                match queued.work {
                    Work::Request(job) => IoService::run(*job, platform),
                    Work::Transcoding(transcoding) => transcoding.run(TRANSCODE_SLICE_STEPS),
                }
            };
            let mut state = self.lock();
            match rest {
                // Behind whatever queued meanwhile; alone, it is taken again at once.
                Some(transcoding) => state.queues[lane as usize].push_back(Queued {
                    sequence: queued.sequence,
                    work: Work::Transcoding(transcoding),
                }),
                None => {
                    state.unfinished.remove(&queued.sequence);
                }
            }
            drop(state);
            self.changed.notify_all();
        }
    }
}
/// Gives a lane and its request back if the worker panics while running it
/// (debug and test builds; release aborts): the next submit starts a new
/// worker, and requests on the same file are no longer held behind the lost
/// one, whose ticket reports a disconnect.
struct Running<'a> {
    lanes: &'a Lanes,
    lane: Lane,
    sequence: u64,
}
impl Drop for Running<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let mut state = self.lanes.lock();
        state.started[self.lane as usize] = false;
        state.unfinished.remove(&self.sequence);
        drop(state);
        self.lanes.changed.notify_all();
    }
}
/// File I/O workers: one lane for saves, one for bulk reads (see `Lane`, and
/// `LaneState::runnable` for the ordering guarantees).
pub struct IoService {
    lanes: std::sync::Arc<Lanes>,
    platform: std::sync::Arc<dyn LocalFileSystem>,
}
impl Drop for IoService {
    /// Accepted requests still drain; dropping never blocks the UI.
    fn drop(&mut self) {
        self.lanes.close();
    }
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
        Self::with_save_idle(platform, SAVE_WORKER_IDLE)
    }
    fn with_save_idle(
        platform: std::sync::Arc<dyn LocalFileSystem>,
        save_idle: std::time::Duration,
    ) -> io::Result<Self> {
        let service = Self {
            lanes: std::sync::Arc::new(Lanes {
                state: Default::default(),
                changed: std::sync::Condvar::new(),
                save_idle,
            }),
            platform,
        };
        let started = service.start(&mut service.lanes.lock(), Lane::Bulk);
        started.map(|()| service)
    }
    /// Start `lane`'s worker unless it is running. Called with the lane state
    /// locked, so a lane never gets two workers.
    fn start(&self, state: &mut LaneState, lane: Lane) -> io::Result<()> {
        if state.started[lane as usize] {
            return Ok(());
        }
        let lanes = self.lanes.clone();
        let platform = self.platform.clone();
        let name = match lane {
            Lane::Save => "file-io-save",
            Lane::Bulk => "file-io",
        };
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || lanes.run(lane, &platform))?;
        state.started[lane as usize] = true;
        Ok(())
    }
    /// Run one request; `Some` is a transcode that yielded after its first slice.
    fn run(job: Job, platform: &std::sync::Arc<dyn LocalFileSystem>) -> Option<Transcoding> {
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
                return None;
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
                let plan = service
                    .capture_spill_with_saved(&captured, saved_state)
                    .map_err(|_| FileError::Budget)
                    .and_then(|plan| {
                        if plan.matches_resident(&captured) {
                            Ok(plan)
                        } else {
                            Err(FileError::Changed)
                        }
                    });
                match plan {
                    Err(error) => IoCompletion::ResidentSpilled {
                        captured,
                        result: Err(error),
                    },
                    Ok(plan) => {
                        let spill = Box::new(OwnedSpill {
                            plan,
                            captured,
                            cache,
                            quota,
                            options,
                            bytes,
                        });
                        if encoding.is_none()
                            && let Some((path, expected)) = original
                        {
                            // Rebuilt from the original file, which may be large:
                            // sliced like an open (FIO-14).
                            let request = PagedOpenRequest {
                                path,
                                bytes: spill.bytes.clone(),
                                history,
                                cache: spill.cache.clone(),
                                options: DiskOptions {
                                    temp_quota_bytes: quota / 2,
                                    interpret: Some(Encoding::Utf8),
                                },
                                source_options: options,
                            };
                            match start_paged_open(request, &platform, &job.cancellation) {
                                Ok(paused) => {
                                    return Transcoding {
                                        stage: Stage::SpillBaseline(Box::new(paused), expected, spill),
                                        platform,
                                        cancellation: job.cancellation,
                                        reply: job.reply,
                                        prefix: job.prefix,
                                        notify: job.notify,
                                    }
                                    .run(TRANSCODE_SLICE_STEPS);
                                }
                                Err(error) => spill.finish(Err(error), None, &platform, &job.cancellation),
                            }
                        } else {
                            // The Resident's own retained original (or none), bounded
                            // by the Resident size limit, so it runs in one go.
                            let baseline = crate::owned_store::prepare_original_baseline(
                                encoding.as_ref(),
                                &spill.cache,
                                quota / 2,
                                platform.clone(),
                                options,
                                spill.bytes.clone(),
                                history,
                                job.cancellation.clone(),
                            );
                            spill.finish(baseline, encoding.as_ref(), &platform, &job.cancellation)
                        }
                    }
                }
            }
            // Not sliced: the snapshot is a Resident, bounded by the Resident size
            // limit, not a multi-GB file.
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
            // Not sliced yet (follow-up FIO-14b): restoring hashes the retained
            // sources in one run. It runs only when the user restores a recovered
            // document; saves still run on their own lane meanwhile.
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
            // Transcodes run in slices so other bulk work can interleave (FIO-14).
            IoRequest::OpenPagedEncoded(request) => match start_paged_open(request, &platform, &job.cancellation) {
                Ok(paused) => {
                    return Transcoding {
                        stage: Stage::Open(Box::new(paused)),
                        platform,
                        cancellation: job.cancellation,
                        reply: job.reply,
                        prefix: job.prefix,
                        notify: job.notify,
                    }
                    .run(TRANSCODE_SLICE_STEPS);
                }
                Err(error) => IoCompletion::Transcode(TranscodeOutcome::Failed(error)),
            },
            IoRequest::ResumeTranscode {
                mut paused,
                temp_quota_bytes,
            } => {
                paused.job.set_quota(temp_quota_bytes);
                paused.job.set_cancellation(job.cancellation.clone());
                return Transcoding {
                    stage: Stage::Open(paused),
                    platform,
                    cancellation: job.cancellation,
                    reply: job.reply,
                    prefix: job.prefix,
                    notify: job.notify,
                }
                .run(TRANSCODE_SLICE_STEPS);
            }
            IoRequest::InterpretPaged(request) => match crate::owned_store::start_reinterpret(&request) {
                Ok(reinterpreting) => {
                    return Transcoding {
                        stage: Stage::Reinterpret(reinterpreting, request),
                        platform,
                        cancellation: job.cancellation,
                        reply: job.reply,
                        prefix: job.prefix,
                        notify: job.notify,
                    }
                    .run(TRANSCODE_SLICE_STEPS);
                }
                Err(error) => IoCompletion::Transcode(TranscodeOutcome::Failed(error)),
            },
            IoRequest::Interpret(request) => IoCompletion::Open(interpret_resident(*request, &job.cancellation)),
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
        None
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
        let lane = request.lane();
        let mut state = self.lanes.lock();
        if state.closed || state.queues[lane as usize].len() >= LANE_DEPTH || self.start(&mut state, lane).is_err() {
            return Err(Box::new(request));
        }
        let sequence = state.next_sequence;
        state.next_sequence += 1;
        state.unfinished.insert(sequence, request.ordering_paths());
        state.queues[lane as usize].push_back(Queued {
            sequence,
            work: Work::Request(Box::new(Job {
                authorization,
                cancellation: cancellation.clone(),
                request,
                reply,
                prefix: prefix_sender,
                notify,
            })),
        });
        drop(state);
        self.lanes.changed.notify_all();
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
    fn interpret_beyond_resident_limits_asks_for_paged_reinterpretation() {
        let raw = "aé\r\n".repeat(4000).into_bytes();
        let (_document, source) = ResidentEncoding::open(
            raw.clone(),
            None,
            Budget::new(1 << 20),
            Budget::new(1 << 16),
            raw.len(),
            64 << 20,
        )
        .unwrap();
        let request = |bytes: Budget| InterpretRequest {
            source: source.clone(),
            target: Encoding::Windows1252,
            dirty: false,
            discard_confirmed: false,
            bytes,
            history: Budget::new(1 << 16),
            path: PathBuf::from("interpret.txt"),
            fingerprint: Fingerprint {
                identity: FileIdentity {
                    volume: 1,
                    file: 1,
                    length: raw.len() as u64,
                    modified: 0,
                },
                sha256: [0; 32],
            },
        };
        // The reinterpretation's raw copy exceeds the budget: a size outcome the
        // caller answers with a paged reinterpretation, not an encoding error.
        assert!(matches!(
            interpret_resident(request(Budget::new(4096)), &Cancellation::default()),
            Err(FileError::StreamingRequired)
        ));
        let opened = interpret_resident(request(Budget::new(16 << 20)), &Cancellation::default()).unwrap();
        assert_eq!(
            opened.encoding.as_ref().unwrap().state.user_override,
            Some(Encoding::Windows1252)
        );
        assert_eq!(
            opened
                .document
                .snapshot()
                .read(TextOffset(0)..TextOffset(7), 100)
                .unwrap(),
            "aÃ©\r\n"
        );
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
    fn one_unresumable_cleanup_record_does_not_abort_the_save_recovery_listing() {
        struct Listing;
        impl LocalFileSystem for Listing {
            fn identity(&self, f: &File) -> io::Result<FileIdentity> {
                Platform.identity(f)
            }
            fn validate_target(&self, _: &Path) -> io::Result<()> {
                Ok(())
            }
            fn commit(&self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
                unreachable!()
            }
            fn inspect_commit_transactions(
                &self,
                parent: &Path,
                _: &dyn bareline_platform::CommitCancellation,
            ) -> io::Result<Vec<bareline_platform::CommitRecovery>> {
                Ok(["bad", "good"]
                    .into_iter()
                    .map(|name| bareline_platform::CommitRecovery {
                        target: Some(parent.join(format!("{name}.txt"))),
                        proposed: parent.join(format!("{name}-proposed")),
                        displaced: None,
                        journal: parent.join(format!("{name}-journal")),
                        state: if name == "bad" {
                            CommitState::CleanupPending
                        } else {
                            CommitState::Conflict
                        },
                        verified: true,
                    })
                    .collect())
            }
            fn resume_commit_cleanup(
                &self,
                _: &bareline_platform::CommitRecovery,
            ) -> io::Result<Option<bareline_platform::CommitReceipt>> {
                Err(io::Error::new(io::ErrorKind::InvalidData, "corrupt cleanup authority"))
            }
        }
        let parent = std::env::temp_dir();
        let recovery = inspect_save_recovery(&parent, &Listing, &Cancellation::default()).unwrap();
        assert!(recovery.cleanups.is_empty());
        let listed: Vec<_> = recovery
            .conflicts
            .iter()
            .map(|conflict| (conflict.target.clone(), conflict.verified))
            .collect();
        assert_eq!(
            listed,
            vec![
                (Some(parent.join("bad.txt")), false),
                (Some(parent.join("good.txt")), true)
            ]
        );
    }
    #[test]
    fn save_sweeps_only_stages_of_dead_processes_and_keeps_reported_copies() {
        struct Liveness;
        impl LocalFileSystem for Liveness {
            fn guard_directory(&self, _: &std::path::Path) -> std::io::Result<std::sync::Arc<dyn Send + Sync>> {
                Ok(std::sync::Arc::new(()))
            }
            fn cache_process_liveness(&self, pid: u32, _: u64) -> bareline_platform::ProcessLiveness {
                if pid == 424242 {
                    bareline_platform::ProcessLiveness::Dead
                } else {
                    bareline_platform::ProcessLiveness::Unknown
                }
            }
            fn identity(&self, f: &File) -> io::Result<FileIdentity> {
                Platform.identity(f)
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
                fs::rename(stage, target)
            }
        }
        let temp = Temp::new();
        let dead = temp.0.join(".bareline-424242-7.tmp");
        let unknown = temp.0.join(".bareline-424243-7.tmp");
        let kept = temp.0.join(".bareline-424242-8.kept");
        let fresh = temp.0.join(".bareline-424242-9.tmp");
        let old = std::time::SystemTime::now() - STAGE_SWEEP_MIN_AGE - std::time::Duration::from_secs(60);
        for leftover in [&dead, &unknown, &kept, &fresh] {
            fs::write(leftover, b"crash leftover").unwrap();
            if leftover != &fresh {
                File::options()
                    .write(true)
                    .open(leftover)
                    .unwrap()
                    .set_modified(old)
                    .unwrap();
            }
        }
        let document = Document::from_utf8("editor bytes", Budget::new(1024), Budget::new(0)).unwrap();
        let target = temp.0.join("target.txt");
        save_utf8(document.snapshot(), &target, None, false, &Liveness).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"editor bytes");
        // REC-11: the dead owner's old stage is swept; a live/unknown owner's stage, a
        // retained, reported copy, and a stage still fresh enough to be another
        // machine's in-flight save on a shared folder are never touched.
        assert!(!dead.exists());
        assert!(unknown.exists() && kept.exists() && fresh.exists());
    }
    #[test]
    fn destination_preflight_keeps_the_user_path_spelling() {
        let temp = Temp::new();
        let existing = temp.0.join("existing.txt");
        fs::write(&existing, b"old target").unwrap();
        for expected in [existing, temp.0.join("absent.txt")] {
            let prepared = preflight_destination(
                &expected,
                None,
                (7, 3),
                SaveOperation::SaveAs,
                &Platform,
                &Cancellation::default(),
            )
            .unwrap();
            assert_eq!(prepared.path(), expected.as_path());
            assert!(!prepared.path().to_string_lossy().starts_with(r"\\?\"));
        }
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Script {
        CancelAfterPrepare,
        CancelAfterCommit,
        PrepareWithoutDisplaced,
        ReceiptWithoutDisplaced,
    }
    struct ScriptedPlatform {
        script: Script,
        cancellation: Cancellation,
    }
    impl LocalFileSystem for ScriptedPlatform {
        fn guard_directory(&self, _: &std::path::Path) -> std::io::Result<std::sync::Arc<dyn Send + Sync>> {
            Ok(std::sync::Arc::new(()))
        }
        fn identity(&self, f: &File) -> io::Result<FileIdentity> {
            Platform.identity(f)
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
            let mut prepared = bareline_platform::prepare_simulated_commit(self, staged, target, mode, cancellation)?;
            match self.script {
                Script::CancelAfterPrepare => self.cancellation.cancel(),
                Script::PrepareWithoutDisplaced => prepared.displaced_path = None,
                _ => {}
            }
            Ok(prepared)
        }
        fn commit_transaction(
            &self,
            transaction: bareline_platform::PreparedCommit,
        ) -> io::Result<bareline_platform::CommitReceipt> {
            let mut receipt = bareline_platform::simulate_commit_transaction(self, transaction)?;
            match self.script {
                Script::CancelAfterCommit => self.cancellation.cancel(),
                Script::ReceiptWithoutDisplaced => receipt.displaced = None,
                _ => {}
            }
            Ok(receipt)
        }
        fn commit(&self, stage: &Path, target: &Path, _: bool) -> io::Result<()> {
            fs::rename(stage, target)
        }
    }
    fn scripted_save(script: Script) -> (Temp, PathBuf, Result<Saved, FileError>) {
        let temp = Temp::new();
        let path = temp.0.join("target.txt");
        fs::write(&path, b"disk bytes").unwrap();
        let expected = fingerprint(&path, &Platform, &Cancellation::default()).unwrap();
        let document = Document::from_utf8("editor bytes", Budget::new(1024), Budget::new(0)).unwrap();
        let cancellation = Cancellation::default();
        let platform = ScriptedPlatform {
            script,
            cancellation: cancellation.clone(),
        };
        let result = save_utf8_cancellable(
            document.snapshot(),
            &path,
            Some(&expected),
            false,
            &platform,
            &cancellation,
        );
        (temp, path, result)
    }
    fn entries(directory: &Path) -> Vec<std::ffi::OsString> {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    }
    #[test]
    fn cancel_after_prepare_aborts_the_prepared_transaction() {
        let (temp, path, result) = scripted_save(Script::CancelAfterPrepare);
        assert!(matches!(result, Err(FileError::Cancelled)));
        assert_eq!(fs::read(&path).unwrap(), b"disk bytes");
        assert_eq!(entries(&temp.0), vec![std::ffi::OsString::from("target.txt")]);
    }
    #[test]
    fn cancel_after_commit_reports_the_real_saved_outcome() {
        let (temp, path, result) = scripted_save(Script::CancelAfterCommit);
        let saved = result.unwrap_or_else(|error| panic!("committed save must report success: {error:?}"));
        assert!(saved.cleanup.is_none());
        assert_eq!(fs::read(&path).unwrap(), b"editor bytes");
        assert_eq!(
            saved.fingerprint,
            fingerprint(&path, &Platform, &Cancellation::default()).unwrap()
        );
        assert_eq!(entries(&temp.0), vec![std::ffi::OsString::from("target.txt")]);
    }
    #[test]
    fn replacement_without_both_versions_is_refused_before_commit() {
        let (temp, path, result) = scripted_save(Script::PrepareWithoutDisplaced);
        let Err(FileError::Io(error)) = result else {
            panic!("incomplete replacement transaction must be refused before commit")
        };
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(fs::read(&path).unwrap(), b"disk bytes");
        assert_eq!(entries(&temp.0), vec![std::ffi::OsString::from("target.txt")]);
    }
    /// Counts the bytes copied into the editor version. When `hashed`, it reports the
    /// copy's hash the way a provider that guards the copy does; when `tamper`, the
    /// stage changes after it was written and before it is copied.
    struct CopyCountingPlatform {
        hashed: bool,
        tamper: bool,
        copied: AtomicU64,
    }
    impl LocalFileSystem for CopyCountingPlatform {
        fn guard_directory(&self, _: &std::path::Path) -> std::io::Result<std::sync::Arc<dyn Send + Sync>> {
            Ok(std::sync::Arc::new(()))
        }
        fn identity(&self, f: &File) -> io::Result<FileIdentity> {
            Platform.identity(f)
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
            if self.tamper {
                fs::write(staged, b"changed after it was written")?;
            }
            let mut hash = Sha256::new();
            let mut prepared = bareline_platform::prepare_simulated_commit_observed(
                self,
                staged,
                target,
                mode,
                cancellation,
                &mut |bytes: &[u8]| {
                    self.copied.fetch_add(bytes.len() as u64, Ordering::SeqCst);
                    hash.update(bytes);
                },
            )?;
            if self.hashed {
                prepared.proposed_sha256 = Some(hash.finalize().into());
            }
            Ok(prepared)
        }
        fn commit_transaction(
            &self,
            transaction: bareline_platform::PreparedCommit,
        ) -> io::Result<bareline_platform::CommitReceipt> {
            bareline_platform::simulate_commit_transaction(self, transaction)
        }
        fn commit(&self, stage: &Path, target: &Path, _: bool) -> io::Result<()> {
            fs::rename(stage, target)
        }
    }
    /// FIO-07 / PERF-07: the full-file passes of one replacing save, counted through
    /// the editor-version copy and the fingerprint reader. The stage is written once
    /// and copied once; the target is read before the commit (the conflict check) and
    /// after it, and so is the displaced version. The copy is read again only when
    /// it was not hashed while it was made.
    #[test]
    fn replacing_save_reads_the_editor_copy_only_while_making_it() {
        let text = "editor bytes\n".repeat(20_000);
        let disk = vec![b'd'; 70_000];
        for hashed in [false, true] {
            let temp = Temp::new();
            let path = temp.0.join("target.txt");
            fs::write(&path, &disk).unwrap();
            let expected = fingerprint(&path, &Platform, &Cancellation::default()).unwrap();
            let document = Document::from_utf8(&text, Budget::new(4 << 20), Budget::new(0)).unwrap();
            let platform = CopyCountingPlatform {
                hashed,
                tamper: false,
                copied: AtomicU64::new(0),
            };
            FINGERPRINTED_BYTES.with(|bytes| bytes.set(0));
            let saved = save_utf8(document.snapshot(), &path, Some(&expected), false, &platform).unwrap();
            let fingerprinted = FINGERPRINTED_BYTES.with(std::cell::Cell::get);
            let (old, new) = (disk.len() as u64, text.len() as u64);
            assert_eq!(platform.copied.load(Ordering::SeqCst), new, "hashed: {hashed}");
            let reread = if hashed { 0 } else { new };
            assert_eq!(fingerprinted, old + new + old + reread, "hashed: {hashed}");
            assert!(saved.cleanup.is_none());
            assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
            assert_eq!(
                saved.fingerprint,
                fingerprint(&path, &Platform, &Cancellation::default()).unwrap()
            );
            assert_eq!(entries(&temp.0), vec![std::ffi::OsString::from("target.txt")]);
        }
    }
    /// FIO-07 keeps the FIO-17 guarantee: a stage that no longer holds the written
    /// bytes is refused before the target is touched.
    #[test]
    fn changed_stage_is_refused_before_the_target_is_replaced() {
        let temp = Temp::new();
        let path = temp.0.join("target.txt");
        fs::write(&path, b"disk bytes").unwrap();
        let expected = fingerprint(&path, &Platform, &Cancellation::default()).unwrap();
        let document = Document::from_utf8("editor bytes", Budget::new(1024), Budget::new(0)).unwrap();
        let platform = CopyCountingPlatform {
            hashed: true,
            tamper: true,
            copied: AtomicU64::new(0),
        };
        let result = save_utf8(document.snapshot(), &path, Some(&expected), false, &platform);
        let Err(FileError::Io(error)) = result else {
            panic!("a changed stage must be refused before commit")
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), b"disk bytes");
        assert_eq!(entries(&temp.0), vec![std::ffi::OsString::from("target.txt")]);
    }
    #[test]
    fn committed_replacement_with_incomplete_receipt_is_not_a_failed_save() {
        let (_temp, path, result) = scripted_save(Script::ReceiptWithoutDisplaced);
        let Err(error @ FileError::VerificationAfterCommit(_)) = result else {
            panic!("a replaced target must be reported as committed, not as a failed save")
        };
        assert_eq!(fs::read(&path).unwrap(), b"editor bytes");
        let conflict = error.save_conflict().unwrap();
        assert_eq!(conflict.target.as_ref(), Some(&path));
        assert_eq!(fs::read(&conflict.editor_version).unwrap(), b"editor bytes");
        assert_eq!(fs::read(conflict.other_version.unwrap()).unwrap(), b"disk bytes");
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
    /// Resident open with the workspace's default budget and resident limit.
    fn open_ordinary(
        path: &Path,
        raw: &[u8],
        budget: &Budget,
        interpret: Option<Encoding>,
    ) -> Result<Opened, FileError> {
        fs::write(path, raw).unwrap();
        open_encoded_streaming(
            path,
            &Platform,
            budget.clone(),
            Budget::new(128 << 20),
            &Cancellation::default(),
            DecodeOptions {
                resident_max_bytes: 256 << 20,
                interpret,
            },
            |_| {},
        )
    }
    /// FIO-01: ordinary UTF-8 opens resident without a raw copy or per-scalar
    /// provenance, however often its scalar widths alternate, and saves exactly.
    fn assert_opens_resident(name: &str, raw: &[u8]) {
        let temp = Temp::new();
        let path = temp.0.join(name);
        let budget = Budget::new(256 << 20);
        let opened = open_ordinary(&path, raw, &budget, None).unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(opened.document.snapshot().len(), raw.len(), "{name}");
        // Only the decoded text is charged: no second raw baseline.
        assert!(budget.used() < raw.len() + raw.len() / 8, "{name}: {}", budget.used());
        let encoding = opened.encoding.unwrap();
        assert_eq!(encoding.original_len(), raw.len(), "{name}");
        let mut original = Vec::new();
        encoding
            .visit_original(0..raw.len(), |chunk| {
                original.extend_from_slice(chunk);
                Ok::<(), ResidentError>(())
            })
            .unwrap();
        assert!(original == raw, "{name}: original bytes differ");
        save_encoded_cancellable(
            opened.document.snapshot(),
            &path,
            Some(&opened.fingerprint),
            false,
            &Platform,
            &Cancellation::default(),
            &encoding,
        )
        .unwrap();
        assert!(fs::read(&path).unwrap() == raw, "{name}: save changed bytes");
    }
    /// The FIO-01 mixed-width UTF-8 shapes: `ae` repeats of "aé", about
    /// `french_bytes` of accented prose, and a CJK CSV of `csv_rows` rows.
    fn assert_mixed_width_opens_resident(ae: usize, french_bytes: usize, csv_rows: usize) {
        assert_opens_resident("ae.txt", "aé".repeat(ae).as_bytes());
        let french = "Où est l'élève? Le garçon déçu a mangé la crème brûlée à Noël.\n";
        assert_opens_resident("french.txt", french.repeat(french_bytes / french.len() + 1).as_bytes());
        let mut csv = String::from("编号,姓名,城市,省份,备注\n");
        for row in 0..csv_rows {
            csv.push_str(&format!("{row},张三,北京,河北,上海,广州\n"));
        }
        assert_opens_resident("cjk.csv", csv.as_bytes());
    }
    /// The property is that provenance does not grow per scalar, so a few hundred
    /// KiB of each alternation pattern exercises it; the budget bound above would
    /// fail on any per-scalar mapping or raw copy.
    #[test]
    fn mixed_width_utf8_text_opens_resident() {
        assert_mixed_width_opens_resident(200_000, 1 << 20, 10_000);
    }
    #[test]
    #[ignore = "writes and opens the full-size FIO-01 files (3 MB, 12 MB, CJK CSV, 140 MB); run once by the integrator"]
    fn full_size_text_opens_resident_within_default_budget() {
        assert_mixed_width_opens_resident(1_000_000, 12 * 1024 * 1024, 150_000);
        let line = "2026-09-30T12:00:00Z INFO request served path=/api/v1/items status=200\n";
        assert_opens_resident("large.log", line.repeat(140 * 1024 * 1024 / line.len() + 1).as_bytes());
    }
    /// FIO-01: quota and budget exhaustion are size outcomes that select the paged
    /// fallback instead of surfacing as encoding failures.
    #[test]
    fn resident_quota_and_budget_exhaustion_require_streaming() {
        let temp = Temp::new();
        let path = temp.0.join("legacy.txt");
        let budget = Budget::new(256 << 20);
        // Valid legacy text of alternating widths maps as a few mixed-width runs
        // (FIO-02), so it no longer exhausts the provenance quota and opens resident.
        let alternating = b"a\xe9".repeat(700_000);
        let opened = open_ordinary(&path, &alternating, &budget, Some(Encoding::Windows1252)).unwrap();
        assert_eq!(opened.document.snapshot().len(), 700_000 * "aé".len());
        drop(opened);
        assert_eq!(budget.used(), 0);
        // Each invalid unit still maps on its own: 1.4 million of them overrun the
        // 64 MiB provenance quota.
        let invalid = b"a\xff".repeat(1_400_000);
        assert!(matches!(
            open_ordinary(&path, &invalid, &budget, Some(Encoding::Utf8)),
            Err(FileError::StreamingRequired)
        ));
        assert_eq!(budget.used(), 0);
        let budget = Budget::new(100_000);
        assert!(matches!(
            open_ordinary(&path, &vec![b'a'; 200_000], &budget, Some(Encoding::Windows1252)),
            Err(FileError::StreamingRequired)
        ));
        assert!(matches!(
            open_ordinary(&path, &vec![b'a'; 200_000], &budget, None),
            Err(FileError::StreamingRequired)
        ));
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
    /// Records which file each I/O request touches, and holds the first touch
    /// of `held` until the test releases it (FIO-14). Sealed opens are only
    /// recorded.
    struct LanePlatform {
        held: PathBuf,
        events: Mutex<Vec<String>>,
        entered: Mutex<Option<std::sync::mpsc::SyncSender<()>>>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl LanePlatform {
        fn new(
            held: PathBuf,
        ) -> (
            Arc<Self>,
            std::sync::mpsc::Receiver<()>,
            std::sync::mpsc::SyncSender<()>,
        ) {
            let (entered, entered_rx) = std::sync::mpsc::sync_channel(1);
            let (release_tx, release) = std::sync::mpsc::sync_channel(1);
            let platform = Arc::new(Self {
                held,
                events: Mutex::new(Vec::new()),
                entered: Mutex::new(Some(entered)),
                release: Mutex::new(release),
            });
            (platform, entered_rx, release_tx)
        }
        fn record(&self, event: &str, path: &Path) {
            let name = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            self.events.lock().unwrap().push(format!("{event}:{name}"));
        }
        fn touch(&self, event: &str, path: &Path) {
            self.record(event, path);
            let entered = if path == self.held.as_path() {
                self.entered.lock().unwrap().take()
            } else {
                None
            };
            if let Some(entered) = entered {
                entered.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
            }
        }
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }
    impl LocalFileSystem for LanePlatform {
        fn guard_directory(&self, path: &Path) -> io::Result<Arc<dyn Send + Sync>> {
            Platform.guard_directory(path)
        }
        fn available_space(&self, path: &Path) -> io::Result<u64> {
            Platform.available_space(path)
        }
        fn open_sealed_read(&self, path: &Path) -> io::Result<File> {
            self.record("seal", path);
            Platform.open_sealed_read(path)
        }
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            Platform.identity(file)
        }
        fn validate_source(&self, path: &Path) -> io::Result<()> {
            self.touch("read", path);
            Ok(())
        }
        fn check_source_read(&self, path: &Path) -> io::Result<()> {
            self.touch("step", path);
            Ok(())
        }
        fn validate_target(&self, path: &Path) -> io::Result<()> {
            self.touch("save", path);
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
        fn commit(&self, stage: &Path, target: &Path, existed: bool) -> io::Result<()> {
            Platform.commit(stage, target, existed)
        }
    }
    /// Hang guard only; ordering is asserted from the recorded events.
    fn completion(ticket: &IoTicket) -> IoCompletion {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match ticket.try_recv() {
                Ok(result) => return result,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    assert!(std::time::Instant::now() < deadline, "I/O request never completed");
                    std::thread::yield_now();
                }
                Err(error) => panic!("{error:?}"),
            }
        }
    }
    fn open_request(path: &Path) -> IoRequest {
        IoRequest::Open {
            path: path.to_path_buf(),
            bytes: Budget::new(1 << 20),
            history: Budget::new(1 << 20),
        }
    }
    fn save_request(text: &str, path: &Path, condition: DestinationCondition) -> IoRequest {
        let document = Document::from_utf8(text, Budget::new(1024), Budget::new(0)).unwrap();
        IoRequest::Save {
            snapshot: document.snapshot(),
            destination: PreparedDestination {
                path: path.to_path_buf(),
                condition,
                consent: DestinationConsent::ExistingDocument,
                document: (1, 1),
                operation: SaveOperation::Save,
            },
            bom: false,
        }
    }
    #[test]
    fn saves_never_queue_behind_a_held_open_or_a_full_bulk_lane() {
        let temp = Temp::new();
        let held = temp.0.join("held.txt");
        fs::write(&held, b"held").unwrap();
        let (platform, entered, release) = LanePlatform::new(held.clone());
        let service = IoService::new(platform.clone()).unwrap();
        // The save worker starts with the first save, not with the service.
        assert_eq!(service.lanes.lock().started, [false, true]);
        let notify: Notification = Arc::new(|| {});
        let open = service.submit(open_request(&held), notify.clone()).ok().unwrap();
        entered.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        // The bulk lane accepts LANE_DEPTH waiting requests, then refuses more.
        let queued: Vec<IoTicket> = (0..LANE_DEPTH)
            .map(|index| {
                service
                    .submit(
                        open_request(&temp.0.join(format!("queued-{index}.txt"))),
                        notify.clone(),
                    )
                    .ok()
                    .unwrap()
            })
            .collect();
        assert!(
            service
                .submit(open_request(&temp.0.join("refused.txt")), notify.clone())
                .is_err()
        );
        let target = temp.0.join("saved.txt");
        let save = service
            .submit(
                save_request("saved text", &target, DestinationCondition::MustBeAbsent),
                notify.clone(),
            )
            .ok()
            .unwrap();
        assert!(matches!(completion(&save), IoCompletion::Save(Ok(_))));
        assert_eq!(service.lanes.lock().started, [true, true]);
        assert_eq!(fs::read(&target).unwrap(), b"saved text");
        assert!(
            matches!(open.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
            "the save finished while the open was still held"
        );
        release.send(()).unwrap();
        assert!(matches!(completion(&open), IoCompletion::Open(Ok(_))));
        for ticket in &queued {
            assert!(matches!(completion(ticket), IoCompletion::Open(Err(_))));
        }
    }
    #[test]
    fn requests_on_one_path_keep_submission_order_across_lanes() {
        let temp = Temp::new();
        let shared = temp.0.join("shared.txt");
        fs::write(&shared, b"disk").unwrap();
        let captured = fingerprint(&shared, &Platform, &Cancellation::default()).unwrap();
        let (platform, entered, release) = LanePlatform::new(shared.clone());
        let service = IoService::new(platform.clone()).unwrap();
        let notify: Notification = Arc::new(|| {});
        let reload = service.submit(open_request(&shared), notify.clone()).ok().unwrap();
        entered.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        let save_shared = service
            .submit(
                save_request("edited", &shared, DestinationCondition::ReplaceCaptured(captured)),
                notify.clone(),
            )
            .ok()
            .unwrap();
        let other = temp.0.join("other.txt");
        let save_other = service
            .submit(
                save_request("other", &other, DestinationCondition::MustBeAbsent),
                notify.clone(),
            )
            .ok()
            .unwrap();
        // The later save of another file overtakes the one waiting for its file;
        // the save lane would otherwise have run the earlier one first.
        assert!(matches!(completion(&save_other), IoCompletion::Save(Ok(_))));
        assert!(matches!(
            save_shared.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        assert!(
            !platform.events().iter().any(|event| event == "save:shared.txt"),
            "a save raced the pending reload of its own file"
        );
        release.send(()).unwrap();
        match completion(&reload) {
            IoCompletion::Open(Ok(opened)) => assert_eq!(
                opened
                    .document
                    .snapshot()
                    .read(TextOffset(0)..TextOffset(4), 4)
                    .unwrap(),
                "disk"
            ),
            _ => panic!("reload failed"),
        }
        assert!(matches!(completion(&save_shared), IoCompletion::Save(Ok(_))));
        assert_eq!(fs::read(&shared).unwrap(), b"edited");
    }
    #[test]
    fn transcode_lets_queued_bulk_work_run_between_slices() {
        let temp = Temp::new();
        let big = temp.0.join("big.txt");
        // Six 64 KiB transcode steps; the test slice is two.
        fs::write(&big, vec![b'a'; 6 * 65536]).unwrap();
        let small = temp.0.join("small.txt");
        fs::write(&small, b"small").unwrap();
        let (platform, entered, release) = LanePlatform::new(big.clone());
        let service = IoService::new(platform.clone()).unwrap();
        let notify: Notification = Arc::new(|| {});
        let transcode = service
            .submit(
                IoRequest::OpenPagedEncoded(PagedOpenRequest {
                    path: big.clone(),
                    bytes: Budget::new(32 << 20),
                    history: Budget::new(1 << 20),
                    cache: temp.0.join("cache"),
                    options: DiskOptions {
                        temp_quota_bytes: 64 << 20,
                        interpret: Some(Encoding::Utf8),
                    },
                    source_options: crate::source::SourceOptions {
                        resident_max_bytes: 0,
                        page_size_bytes: 4096,
                        page_cache_bytes: 1 << 20,
                    },
                }),
                notify.clone(),
            )
            .ok()
            .unwrap();
        entered.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        let open = service.submit(open_request(&small), notify.clone()).ok().unwrap();
        release.send(()).unwrap();
        assert!(matches!(completion(&open), IoCompletion::Open(Ok(_))));
        assert!(matches!(
            completion(&transcode),
            IoCompletion::Transcode(TranscodeOutcome::Complete(_))
        ));
        let events = platform.events();
        let small_read = events.iter().position(|event| event == "read:small.txt").unwrap();
        let later_steps = events[small_read..]
            .iter()
            .filter(|event| *event == "step:big.txt")
            .count();
        assert!(later_steps >= 2, "the open waited for the whole transcode: {events:?}");
    }
    #[test]
    fn reinterpretation_lets_queued_bulk_work_run_between_slices() {
        let temp = Temp::new();
        let big = temp.0.join("big.txt");
        // Six 64 KiB transcode steps; the test slice is two.
        fs::write(&big, vec![b'a'; 6 * 65536]).unwrap();
        let small = temp.0.join("small.txt");
        fs::write(&small, b"small").unwrap();
        let source_options = crate::source::SourceOptions {
            resident_max_bytes: 0,
            page_size_bytes: 4096,
            page_cache_bytes: 1 << 20,
        };
        let opened = match open_paged_encoded(
            PagedOpenRequest {
                path: big.clone(),
                bytes: Budget::new(32 << 20),
                history: Budget::new(1 << 20),
                cache: temp.0.join("cache"),
                options: DiskOptions {
                    temp_quota_bytes: 64 << 20,
                    interpret: Some(Encoding::Utf8),
                },
                source_options,
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) {
            TranscodeOutcome::Complete(opened) => opened,
            _ => panic!("paged open failed"),
        };
        // The reinterpretation transcodes the retained original; hold its first step.
        let (platform, entered, release) = LanePlatform::new(opened.transcoded.store.original_path());
        let service = IoService::new(platform.clone()).unwrap();
        let notify: Notification = Arc::new(|| {});
        let reinterpret = service
            .submit(
                IoRequest::InterpretPaged(Box::new(InterpretPagedRequest {
                    source: opened.transcoded.store.clone(),
                    target: Encoding::Windows1252,
                    path: big.clone(),
                    fingerprint: opened.fingerprint.clone(),
                    cache: temp.0.join("cache"),
                    quota: 64 << 20,
                    options: source_options,
                    bytes: Budget::new(32 << 20),
                    history: Budget::new(1 << 20),
                })),
                notify.clone(),
            )
            .ok()
            .unwrap();
        entered.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        let open = service.submit(open_request(&small), notify.clone()).ok().unwrap();
        release.send(()).unwrap();
        assert!(matches!(completion(&open), IoCompletion::Open(Ok(_))));
        match completion(&reinterpret) {
            IoCompletion::Transcode(TranscodeOutcome::Complete(reinterpreted)) => assert_eq!(reinterpreted.path, big),
            _ => panic!("reinterpretation failed"),
        }
        let events = platform.events();
        let small_read = events.iter().position(|event| event == "read:small.txt").unwrap();
        let later_steps = events[small_read..]
            .iter()
            .filter(|event| *event == "step:original.raw")
            .count();
        assert!(
            later_steps >= 2,
            "the open waited for the whole reinterpretation: {events:?}"
        );
    }
    #[test]
    fn reinterpretation_validates_its_sealed_store_between_other_bulk_work() {
        let temp = Temp::new();
        let big = temp.0.join("big.txt");
        // original.raw, not yet proven, takes seven 64 KiB reads to validate;
        // the test slice is two.
        fs::write(&big, vec![b'a'; 6 * 65536]).unwrap();
        let small = temp.0.join("small.txt");
        fs::write(&small, b"small").unwrap();
        let held = temp.0.join("held.txt");
        fs::write(&held, b"held").unwrap();
        let source_options = crate::source::SourceOptions {
            resident_max_bytes: 0,
            page_size_bytes: 4096,
            page_cache_bytes: 1 << 20,
        };
        let opened = match open_paged_encoded(
            PagedOpenRequest {
                path: big.clone(),
                bytes: Budget::new(32 << 20),
                history: Budget::new(1 << 20),
                cache: temp.0.join("cache"),
                options: DiskOptions {
                    temp_quota_bytes: 64 << 20,
                    interpret: Some(Encoding::Utf8),
                },
                source_options,
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) {
            TranscodeOutcome::Complete(opened) => opened,
            _ => panic!("paged open failed"),
        };
        let (platform, entered, release) = LanePlatform::new(held.clone());
        let service = IoService::new(platform.clone()).unwrap();
        let notify: Notification = Arc::new(|| {});
        // Hold the bulk worker so the reinterpretation and the open queue in order.
        let blocker = service.submit(open_request(&held), notify.clone()).ok().unwrap();
        entered.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        let reinterpret = service
            .submit(
                IoRequest::InterpretPaged(Box::new(InterpretPagedRequest {
                    source: opened.transcoded.store.clone(),
                    target: Encoding::Windows1252,
                    path: big.clone(),
                    fingerprint: opened.fingerprint.clone(),
                    cache: temp.0.join("cache"),
                    quota: 64 << 20,
                    options: source_options,
                    bytes: Budget::new(32 << 20),
                    history: Budget::new(1 << 20),
                })),
                notify.clone(),
            )
            .ok()
            .unwrap();
        let open = service.submit(open_request(&small), notify.clone()).ok().unwrap();
        release.send(()).unwrap();
        assert!(matches!(completion(&blocker), IoCompletion::Open(Ok(_))));
        assert!(matches!(completion(&open), IoCompletion::Open(Ok(_))));
        match completion(&reinterpret) {
            IoCompletion::Transcode(TranscodeOutcome::Complete(reinterpreted)) => assert_eq!(reinterpreted.path, big),
            _ => panic!("reinterpretation failed"),
        }
        // The transcode opens the original in the slice that finishes validation.
        let events = platform.events();
        let small_read = events.iter().position(|event| event == "read:small.txt").unwrap();
        let validated = events.iter().position(|event| event == "seal:original.raw").unwrap();
        assert!(
            small_read < validated,
            "the open waited for the whole sealed-store validation: {events:?}"
        );
    }
    #[test]
    fn save_lane_refuses_requests_beyond_its_depth() {
        let temp = Temp::new();
        let held = temp.0.join("held.txt");
        let (platform, entered, release) = LanePlatform::new(held.clone());
        let service = IoService::new(platform.clone()).unwrap();
        let notify: Notification = Arc::new(|| {});
        let first = service
            .submit(
                save_request("held", &held, DestinationCondition::MustBeAbsent),
                notify.clone(),
            )
            .ok()
            .unwrap();
        entered.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        let queued: Vec<IoTicket> = (0..LANE_DEPTH)
            .map(|index| {
                service
                    .submit(
                        save_request(
                            "queued",
                            &temp.0.join(format!("queued-{index}.txt")),
                            DestinationCondition::MustBeAbsent,
                        ),
                        notify.clone(),
                    )
                    .ok()
                    .unwrap()
            })
            .collect();
        let refused = temp.0.join("refused.txt");
        assert!(
            service
                .submit(
                    save_request("refused", &refused, DestinationCondition::MustBeAbsent),
                    notify.clone(),
                )
                .is_err()
        );
        // A full save lane does not hold up bulk work.
        let other = temp.0.join("other.txt");
        fs::write(&other, b"other").unwrap();
        let open = service.submit(open_request(&other), notify.clone()).ok().unwrap();
        assert!(matches!(completion(&open), IoCompletion::Open(Ok(_))));
        release.send(()).unwrap();
        assert!(matches!(completion(&first), IoCompletion::Save(Ok(_))));
        for ticket in &queued {
            assert!(matches!(completion(ticket), IoCompletion::Save(Ok(_))));
        }
        assert!(!refused.exists());
    }
    #[test]
    fn same_named_files_in_other_directories_do_not_wait_for_each_other() {
        let temp = Temp::new();
        fs::create_dir(temp.0.join("logs")).unwrap();
        fs::create_dir(temp.0.join("proj")).unwrap();
        let held = temp.0.join("logs").join("app.log");
        fs::write(&held, b"held").unwrap();
        let (platform, entered, release) = LanePlatform::new(held.clone());
        let service = IoService::new(platform.clone()).unwrap();
        let notify: Notification = Arc::new(|| {});
        let open = service.submit(open_request(&held), notify.clone()).ok().unwrap();
        entered.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        let target = temp.0.join("proj").join("app.log");
        let save = service
            .submit(
                save_request("saved", &target, DestinationCondition::MustBeAbsent),
                notify.clone(),
            )
            .ok()
            .unwrap();
        assert!(matches!(completion(&save), IoCompletion::Save(Ok(_))));
        assert!(
            matches!(open.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
            "the save finished while the open of the other app.log was still held"
        );
        release.send(()).unwrap();
        assert!(matches!(completion(&open), IoCompletion::Open(Ok(_))));
    }
    #[test]
    fn ordering_keys_match_spellings_of_one_file() {
        let key = |path: &str| ordering_key(Path::new(path));
        assert!(key("C:/Proj/App.log").overlaps(&key("c:/proj/app.LOG")));
        assert!(!key("C:/proj/app.log").overlaps(&key("D:/logs/app.log")));
        assert!(!key("C:/proj/app.log").overlaps(&key("C:/proj/other.log")));
        // A root file may be any directory's file seen through a mapped drive.
        assert!(key("/app.log").overlaps(&key("D:/logs/app.log")));
        #[cfg(windows)]
        {
            assert!(key(r"\\?\C:\Proj\App.log").overlaps(&key(r"c:\proj\app.log")));
            assert!(key(r"\\server\share\logs\app.log").overlaps(&key(r"Z:\logs\app.log")));
            assert!(key(r"\\server\share\app.log").overlaps(&key(r"Z:\app.log")));
        }
    }
    #[test]
    fn a_panicked_lane_worker_gives_back_its_lane_and_file() {
        let temp = Temp::new();
        let held = temp.0.join("held.txt");
        fs::write(&held, b"held").unwrap();
        let (platform, entered, release) = LanePlatform::new(held.clone());
        let service = IoService::new(platform.clone()).unwrap();
        let notify: Notification = Arc::new(|| {});
        let lost = service.submit(open_request(&held), notify.clone()).ok().unwrap();
        entered.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        // With its release gone, the held read panics on the bulk worker.
        drop(release);
        // Hang guard only: the unwinding worker gives its lane back.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while service.lanes.lock().started[Lane::Bulk as usize] {
            assert!(
                std::time::Instant::now() < deadline,
                "the panicked worker kept its lane"
            );
            std::thread::yield_now();
        }
        assert!(service.lanes.lock().unfinished.is_empty());
        assert!(matches!(
            lost.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
        // The next request on the same file starts a worker and is not held
        // behind the lost one.
        let retry = service.submit(open_request(&held), notify).ok().unwrap();
        assert!(matches!(completion(&retry), IoCompletion::Open(Ok(_))));
        assert!(service.lanes.lock().started[Lane::Bulk as usize]);
    }
    #[test]
    fn lane_wakes_do_not_keep_an_idle_save_worker() {
        let temp = Temp::new();
        let service = IoService::with_save_idle(Arc::new(Platform), std::time::Duration::from_millis(50)).unwrap();
        let notify: Notification = Arc::new(|| {});
        let target = temp.0.join("saved.txt");
        let save = service
            .submit(
                save_request("saved", &target, DestinationCondition::MustBeAbsent),
                notify,
            )
            .ok()
            .unwrap();
        assert!(matches!(completion(&save), IoCompletion::Save(Ok(_))));
        // Wake the lanes far more often than the idle time, as bulk slices do.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while service.lanes.lock().started[Lane::Save as usize] {
            assert!(
                std::time::Instant::now() < deadline,
                "lane wakes kept the idle save worker"
            );
            service.lanes.changed.notify_all();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    #[test]
    fn idle_save_worker_exits_and_restarts_with_the_next_save() {
        let temp = Temp::new();
        let service = IoService::with_save_idle(Arc::new(Platform), std::time::Duration::ZERO).unwrap();
        let notify: Notification = Arc::new(|| {});
        for round in 0..2 {
            let target = temp.0.join(format!("saved-{round}.txt"));
            let save = service
                .submit(
                    save_request("saved", &target, DestinationCondition::MustBeAbsent),
                    notify.clone(),
                )
                .ok()
                .unwrap();
            assert!(matches!(completion(&save), IoCompletion::Save(Ok(_))));
            assert_eq!(fs::read(&target).unwrap(), b"saved");
            // Hang guard only: the idle worker gives its thread back.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while service.lanes.lock().started[Lane::Save as usize] {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the idle save worker kept its thread"
                );
                std::thread::yield_now();
            }
        }
        assert!(service.lanes.lock().started[Lane::Bulk as usize]);
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
