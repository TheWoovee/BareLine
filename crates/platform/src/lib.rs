// SPDX-License-Identifier: MPL-2.0
pub mod clipboard;
pub mod executor;
pub mod remote_read;
pub use remote_read::{RemoteReadAccess, RemoteReadAction, RemoteReadGrant};
use std::path::{Path, PathBuf};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageKind {
    Local,
    Removable,
    Network,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathOrigin {
    User,
    Session,
    Extension,
}
#[derive(Clone, Debug)]
pub struct PathTrust {
    pub canonical: PathBuf,
    pub storage: StorageKind,
    pub origin: PathOrigin,
    pub traverses_reparse_point: bool,
}
#[derive(Clone, Copy, Debug)]
pub enum PathOperation {
    Read,
    Write,
    Execute,
}
/// Retained handles bind classification to the actual opened object and its ancestors.
pub struct TrustedRead {
    pub trust: PathTrust,
    pub file: std::fs::File,
    pub ancestors: Vec<std::fs::File>,
}
pub trait PathTrustProvider {
    fn open_read(&self, _: &Path, _: PathOrigin) -> std::io::Result<TrustedRead> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "retained read capability unavailable",
        ))
    }
    fn canonicalize(&self, path: &Path, origin: PathOrigin) -> std::io::Result<PathTrust>;
    fn permits(&self, _: &PathTrust, _: PathOperation) -> bool {
        false
    }
}
/// Classification is intentionally unknown until PR-015 supplies handle-based evaluation.
pub struct RestrictedPaths;
impl PathTrustProvider for RestrictedPaths {
    fn canonicalize(&self, _: &Path, _: PathOrigin) -> std::io::Result<PathTrust> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "path trust provider unavailable",
        ))
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Support {
    Supported,
    Unsupported,
    Unknown,
}
#[derive(Clone, Copy, Debug)]
pub struct CapabilityReport {
    pub atomic_replace: Support,
    pub acl: Support,
    pub ads: Support,
    pub hard_links: Support,
}
pub trait FilesystemCapability {
    fn report(&self, path: &Path) -> std::io::Result<CapabilityReport>;
}
pub trait PlatformServices {
    fn clipboard_text(&self) -> Result<String, String> {
        Err("clipboard unavailable".into())
    }
    fn set_clipboard_text(&self, _: &str) -> Result<(), String> {
        Err("clipboard unavailable".into())
    }
    /// Optional metadata publication may degrade to successful plain-text copy.
    fn set_clipboard_text_with_metadata(&self, text: &str, _: &str, _: &[u8]) -> Result<(), String> {
        self.set_clipboard_text(text)
    }
    fn clipboard_metadata(&self, _: &str, _: usize) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    /// Implementations supporting metadata must read both formats under one ownership lock.
    fn clipboard_text_with_metadata(&self, _: &str, _: usize) -> Result<clipboard::ClipboardContents, String> {
        Ok(clipboard::ClipboardContents {
            text: self.clipboard_text()?,
            metadata: None,
        })
    }
    fn about(&self);
    fn open_file(&self) -> Result<Option<PathBuf>, String>;
    fn save_file(&self) -> Result<Option<PathBuf>, String>;
    /// Save As with a suggested initial file name (e.g. "Untitled 1.txt" or the
    /// document name). Defaults to a plain Save dialog for platforms that do not
    /// pre-fill the name.
    fn save_file_named(&self, _default_name: &str) -> Result<Option<PathBuf>, String> {
        self.save_file()
    }
    /// Save dialog with a document-derived name and a trusted starting directory.
    fn save_document_file_at(
        &self,
        default_name: &str,
        _default_directory: Option<&Path>,
    ) -> Result<Option<PathBuf>, String> {
        self.save_file_named(default_name)
    }
    fn pick_folder(&self) -> Result<Option<PathBuf>, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub volume: u64,
    pub file: u64,
    pub length: u64,
    pub modified: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheProcessIdentity {
    pub pid: u32,
    pub created: u64,
    pub nonce: [u8; 16],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheDirectoryIdentity {
    pub volume: u64,
    pub file: u64,
}

pub struct CacheDirectoryLease {
    pub path: PathBuf,
    pub identity: CacheDirectoryIdentity,
    pub guard: std::sync::Arc<dyn Send + Sync>,
    pub migration_publisher: Option<std::sync::Arc<dyn Fn(&Path) -> std::io::Result<()> + Send + Sync>>,
}

pub struct CacheRemovalOutcome {
    pub visited: usize,
    pub retry_authority_retained: bool,
    pub result: std::io::Result<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessLiveness {
    Alive,
    Dead,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitMode {
    CreateNew,
    Replace,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitState {
    Precommit,
    Created,
    Replaced,
    Conflict,
    CleanupPending,
    Unverified,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupResponsibility {
    None,
    Caller,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreservedFile {
    pub path: PathBuf,
    pub identity: FileIdentity,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRecovery {
    pub target: Option<PathBuf>,
    pub proposed: PathBuf,
    pub displaced: Option<PathBuf>,
    pub journal: PathBuf,
    pub state: CommitState,
    pub verified: bool,
}
pub struct PreparedCommit {
    pub staged: PathBuf,
    pub target: PathBuf,
    pub mode: CommitMode,
    pub displaced_path: Option<PathBuf>,
    pub proposed_path: Option<PathBuf>,
    pub journal_path: Option<PathBuf>,
    pub guard: Option<Box<dyn std::any::Any + Send>>,
}
impl std::fmt::Debug for PreparedCommit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedCommit")
            .field("staged", &self.staged)
            .field("target", &self.target)
            .field("mode", &self.mode)
            .field("displaced_path", &self.displaced_path)
            .field("proposed_path", &self.proposed_path)
            .field("journal_path", &self.journal_path)
            .field("guarded", &self.guard.is_some())
            .finish()
    }
}
pub struct CommitReceipt {
    pub target: FileIdentity,
    pub displaced: Option<PreservedFile>,
    pub proposed: Option<PreservedFile>,
    pub journal: Option<PathBuf>,
    pub state: CommitState,
    pub cleanup: CleanupResponsibility,
    pub cleanup_token: Option<Box<dyn CommitCleanup>>,
}
impl std::fmt::Debug for CommitReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommitReceipt")
            .field("target", &self.target)
            .field("displaced", &self.displaced)
            .field("proposed", &self.proposed)
            .field("journal", &self.journal)
            .field("state", &self.state)
            .field("cleanup", &self.cleanup)
            .field("verification_held", &self.cleanup_token.is_some())
            .finish()
    }
}
pub trait CommitCleanup: Send {
    /// Publish and pin the durable authority needed to resume cleanup after receipt loss.
    fn publish_cleanup_authority(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn cleanup(&mut self) -> std::io::Result<()>;
}
pub trait CommitCancellation {
    fn check(&self) -> std::io::Result<()>;
}
/// Background file operations; implementations must refuse unsupported replacement semantics.
pub trait LocalFileSystem: Send + Sync {
    fn cache_process_identity(&self) -> std::io::Result<CacheProcessIdentity> {
        use std::sync::{
            OnceLock,
            atomic::{AtomicU64, Ordering},
        };
        static CREATED: OnceLock<u64> = OnceLock::new();
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let created = *CREATED.get_or_init(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64
        });
        let next = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut nonce = [0; 16];
        nonce[..8].copy_from_slice(&created.to_le_bytes());
        nonce[8..].copy_from_slice(&next.to_le_bytes());
        Ok(CacheProcessIdentity {
            pid: std::process::id(),
            created,
            nonce,
        })
    }
    fn cache_process_liveness(&self, _: u32, _: u64) -> ProcessLiveness {
        ProcessLiveness::Unknown
    }
    fn cache_directory_guard(&self, path: &Path) -> std::io::Result<Option<CacheDirectoryLease>> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "stable directory identity unavailable",
            ))
        } else {
            Ok(None)
        }
    }
    fn migration_entry_guard(&self, _: &Path) -> std::io::Result<CacheDirectoryLease> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "migration entry guards unavailable",
        ))
    }
    fn open_migration_read(&self, _: &CacheDirectoryLease) -> std::io::Result<std::fs::File> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "migration read lease unavailable",
        ))
    }
    fn publish_migration_entry(&self, _: &Path, _: CacheDirectoryLease) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "verified migration publication unavailable",
        ))
    }
    fn remove_owned_cache_directory(
        &self,
        _: &Path,
        _: &Path,
        _: CacheDirectoryIdentity,
        _: CacheDirectoryIdentity,
        _: &str,
        _: &[u8],
        _: usize,
        _: std::time::Duration,
        _: &dyn Fn() -> bool,
    ) -> CacheRemovalOutcome {
        CacheRemovalOutcome {
            visited: 0,
            retry_authority_retained: true,
            result: Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "owned cache deletion unavailable",
            )),
        }
    }
    /// Release source-only guards after a private snapshot has been sealed.
    fn release_source_read(&self, _: &Path) {}
    /// Recheck an admitted read capability without opening or querying a path.
    fn check_source_read(&self, _: &Path) -> std::io::Result<()> {
        Ok(())
    }
    fn scoped_remote_read(&self, _: RemoteReadAccess) -> std::io::Result<std::sync::Arc<dyn LocalFileSystem>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "remote read capability unavailable",
        ))
    }
    /// Open a mutable followed file without following a final reparse point; retain
    /// the verified directory chain while allowing final-file rotation.
    fn open_follow_read(&self, _: &Path) -> std::io::Result<(std::fs::File, std::sync::Arc<dyn Send + Sync>)> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "follow read capability unavailable",
        ))
    }

    /// Hold a verified local directory chain against reparse retargeting during recovery.
    fn guard_directory(&self, _: &Path) -> std::io::Result<std::sync::Arc<dyn Send + Sync>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "directory guards unavailable",
        ))
    }

    fn available_space(&self, _: &Path) -> std::io::Result<u64> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "free-space query unavailable",
        ))
    }

    /// Keep bytes and directory identity immutable against write/delete until handle drop.
    /// Platforms without this capability must refuse sealed-store export.
    fn open_sealed_read(&self, _: &Path) -> std::io::Result<std::fs::File> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "sealed read handles unavailable",
        ))
    }

    /// Explicit explorer action; implementations must retain safe parent handles.
    fn create_entry(&self, _: &Path, _: bool) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "entry creation unavailable",
        ))
    }
    /// Never replace an existing destination, including in a race.
    fn rename_entry(&self, _: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "entry rename unavailable",
        ))
    }
    /// Delete one entry only; a nonempty directory must fail.
    fn delete_entry(&self, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "entry deletion unavailable",
        ))
    }
    /// Read eligibility may allow read-only and hard-linked sources while replacement does not.
    fn validate_source(&self, path: &Path) -> std::io::Result<()> {
        self.validate_target(path)
    }
    fn identity(&self, file: &std::fs::File) -> std::io::Result<FileIdentity>;
    fn validate_target(&self, path: &Path) -> std::io::Result<()>;
    fn prepare_commit(
        &self,
        _: &Path,
        _: &Path,
        _: CommitMode,
        _: &dyn CommitCancellation,
    ) -> std::io::Result<PreparedCommit> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic prepared commit transactions are unavailable",
        ))
    }
    fn commit_transaction(&self, _: PreparedCommit) -> std::io::Result<CommitReceipt> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic prepared commit transactions are unavailable",
        ))
    }
    fn cleanup_commit(&self, receipt: &mut CommitReceipt) -> std::io::Result<()> {
        if let Some(cleanup) = receipt.cleanup_token.as_mut() {
            return cleanup.cleanup();
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "verified commit cleanup ownership is unavailable",
        ))
    }
    fn mark_commit_state(&self, receipt: &CommitReceipt, state: CommitState) -> std::io::Result<()> {
        match &receipt.journal {
            Some(path) => publish_commit_state(path, state),
            None => Ok(()),
        }
    }
    /// Enumerate transaction directories using platform path guards and bounded,
    /// no-follow record reads. Unsupported providers have no recoverable native
    /// document-save transactions.
    fn inspect_commit_transactions(
        &self,
        _: &Path,
        _: &dyn CommitCancellation,
    ) -> std::io::Result<Vec<CommitRecovery>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "save transaction inspection is unavailable",
        ))
    }
    /// Reacquire exact cleanup ownership for a verified interrupted cleanup.
    fn resume_commit_cleanup(&self, _: &CommitRecovery) -> std::io::Result<Option<CommitReceipt>> {
        Ok(None)
    }
    /// Compatibility primitive for platform adapters that do not participate in document saves.
    fn commit(&self, staged: &Path, target: &Path, existed: bool) -> std::io::Result<()>;
}

/// Deterministic test preparation only. Production providers additionally own
/// transaction directories, immutable manifests, and verification handles.
pub fn prepare_simulated_commit(
    file_system: &dyn LocalFileSystem,
    staged: &Path,
    target: &Path,
    mode: CommitMode,
    cancellation: &dyn CommitCancellation,
) -> std::io::Result<PreparedCommit> {
    let guard = target
        .parent()
        .map(|parent| file_system.guard_directory(parent))
        .transpose()?;
    let displaced_path = if mode == CommitMode::Replace {
        let displaced = unique_commit_path(target, "displaced")?;
        std::fs::remove_file(&displaced)?;
        Some(displaced)
    } else {
        None
    };
    let proposed = unique_commit_path(target, "proposed")?;
    std::fs::remove_file(&proposed)?;
    if let Err(error) = copy_commit_bytes(staged, &proposed, cancellation) {
        let _ = std::fs::remove_file(&proposed);
        return Err(error);
    }
    let journal = unique_commit_path(target, "transaction")?;
    std::fs::remove_file(&journal)?;
    publish_commit_state(&journal, CommitState::Precommit)?;
    Ok(PreparedCommit {
        staged: staged.to_path_buf(),
        target: target.to_path_buf(),
        mode,
        displaced_path,
        proposed_path: Some(proposed),
        journal_path: Some(journal),
        guard: guard.map(|guard| Box::new(guard) as Box<dyn std::any::Any + Send>),
    })
}

pub fn copy_commit_bytes(
    source: &Path,
    destination: &Path,
    cancellation: &dyn CommitCancellation,
) -> std::io::Result<()> {
    use std::io::{Read as _, Write as _};
    let mut source = std::fs::File::open(source)?;
    let mut destination = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        cancellation.check()?;
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        destination.write_all(&buffer[..read])?;
    }
    cancellation.check()?;
    destination.sync_all()
}

/// Deterministic test adapter only. Production document-save providers must implement
/// an atomic platform transaction and must never call this pathname-based simulation.
pub fn simulate_commit_transaction(
    file_system: &dyn LocalFileSystem,
    transaction: PreparedCommit,
) -> std::io::Result<CommitReceipt> {
    let PreparedCommit {
        staged,
        target,
        mode,
        displaced_path,
        proposed_path,
        journal_path,
        guard: _guard,
    } = transaction;
    let displaced = if let Some(path) = displaced_path {
        std::fs::rename(&target, &path)?;
        Some(path)
    } else {
        None
    };
    file_system.commit(&staged, &target, false)?;
    let target_identity = file_system.identity(&std::fs::File::open(&target)?)?;
    let displaced = displaced
        .map(|path| -> std::io::Result<PreservedFile> {
            let identity = file_system.identity(&std::fs::File::open(&path)?)?;
            Ok(PreservedFile { path, identity })
        })
        .transpose()?;
    let proposed = proposed_path
        .map(|path| -> std::io::Result<PreservedFile> {
            let identity = file_system.identity(&std::fs::File::open(&path)?)?;
            Ok(PreservedFile { path, identity })
        })
        .transpose()?;
    let state = if mode == CommitMode::Replace {
        CommitState::Replaced
    } else {
        CommitState::Created
    };
    if let Some(journal) = &journal_path {
        publish_commit_state(journal, state)?;
    }
    let cleanup = SimulatedCommitCleanup {
        preserved: [displaced.as_ref(), proposed.as_ref()]
            .into_iter()
            .flatten()
            .map(|file| file.path.clone())
            .collect(),
        journal: journal_path.clone(),
    };
    Ok(CommitReceipt {
        target: target_identity,
        displaced,
        proposed,
        journal: journal_path,
        state,
        cleanup: CleanupResponsibility::Caller,
        cleanup_token: Some(Box::new(cleanup)),
    })
}

struct SimulatedCommitCleanup {
    preserved: Vec<PathBuf>,
    journal: Option<PathBuf>,
}
impl CommitCleanup for SimulatedCommitCleanup {
    fn cleanup(&mut self) -> std::io::Result<()> {
        while let Some(path) = self.preserved.last() {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            self.preserved.pop();
        }
        if let Some(journal) = &self.journal {
            for state in commit_states() {
                match std::fs::remove_file(commit_state_path(journal, state)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }
}

fn unique_commit_path(target: &Path, kind: &str) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let parent = target
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "target has no parent"))?;
    loop {
        let path = parent.join(format!(
            ".bareline-save-{}-{}-{kind}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn commit_states() -> [CommitState; 5] {
    [
        CommitState::Precommit,
        CommitState::Created,
        CommitState::Replaced,
        CommitState::Conflict,
        CommitState::CleanupPending,
    ]
}
pub fn commit_state_path(base: &Path, state: CommitState) -> PathBuf {
    base.with_extension(match state {
        CommitState::Precommit => "precommit",
        CommitState::Created => "created",
        CommitState::Replaced => "replaced",
        CommitState::Conflict => "conflict",
        CommitState::CleanupPending => "cleanup",
        CommitState::Unverified => "unverified",
    })
}
pub fn publish_commit_state(path: &Path, state: CommitState) -> std::io::Result<()> {
    let bytes: &[u8] = match state {
        CommitState::Precommit => b"bareline-save-v1\nprecommit\n",
        CommitState::Created => b"bareline-save-v1\ncreated\n",
        CommitState::Replaced => b"bareline-save-v1\ncommitted-unacknowledged\n",
        CommitState::Conflict => b"bareline-save-v1\nconflict\n",
        CommitState::CleanupPending => b"bareline-save-v1\ncleanup-pending\n",
        CommitState::Unverified => return Err(std::io::Error::other("cannot publish unverified state")),
    };
    let state_path = commit_state_path(path, state);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&state_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_bounded(&state_path, 128)? == bytes {
                return Ok(());
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "save transaction state record changed",
            ));
        }
        Err(error) => return Err(error),
    };
    use std::io::Write as _;
    file.write_all(bytes)?;
    file.sync_all()
}

pub fn read_commit_state(path: &Path) -> std::io::Result<CommitState> {
    let ordered = [
        (
            CommitState::CleanupPending,
            b"bareline-save-v1\ncleanup-pending\n".as_slice(),
        ),
        (CommitState::Conflict, b"bareline-save-v1\nconflict\n".as_slice()),
        (
            CommitState::Replaced,
            b"bareline-save-v1\ncommitted-unacknowledged\n".as_slice(),
        ),
        (CommitState::Created, b"bareline-save-v1\ncreated\n".as_slice()),
        (CommitState::Precommit, b"bareline-save-v1\nprecommit\n".as_slice()),
    ];
    let mut saw_record = false;
    for (state, expected) in ordered {
        let state_path = commit_state_path(path, state);
        match read_bounded(&state_path, 128) {
            Ok(bytes) if bytes == expected && !saw_record => return Ok(state),
            Ok(bytes) if bytes == expected => return Ok(CommitState::Unverified),
            Ok(_) => saw_record = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if saw_record {
        Ok(CommitState::Unverified)
    } else {
        Err(std::io::Error::from(std::io::ErrorKind::NotFound))
    }
}

fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "save transaction record too large",
        ));
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_network_origin_never_grants_access() {
        assert!(
            RestrictedPaths
                .canonicalize(Path::new(r"\\never-contact.invalid\share\file"), PathOrigin::Session)
                .is_err()
        );
        let path = PathTrust {
            canonical: PathBuf::from(r"\\server\share\file"),
            storage: StorageKind::Network,
            origin: PathOrigin::Session,
            traverses_reparse_point: false,
        };
        for op in [PathOperation::Read, PathOperation::Write, PathOperation::Execute] {
            assert!(!RestrictedPaths.permits(&path, op));
        }
    }
}

mod paths;
pub use paths::{PathDecodeError, PathEncoding, SerializedPath};
mod readiness;
pub use readiness::*;

mod watch;
pub use watch::{WatchEvent, WatchKind};

pub mod accessibility;

pub mod printing;
