// SPDX-License-Identifier: MPL-2.0
//! Paged recovery owns the original codec triplet and journals bounded UTF-8 edits.
//! Baseline copying runs on a separate bounded worker without locking the edit actor.
#[path = "paged_group_recovery.rs"]
pub mod group;
use crate::{
    cancellation::Cancellation,
    codecs::disk::DiskDecoded,
    recovery::{DurableReceipt, RecoveryEdit, RecoveryMetadata, RecoveryWriter},
};
use bareline_platform::LocalFileSystem;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{self, SyncSender},
    },
};
#[derive(Clone, Default, Debug)]
pub struct PagedRecoveryStatus {
    pub directory: Option<PathBuf>,
    pub durable: Option<DurableReceipt>,
    pub complete: bool,
    pub error: Option<String>,
    /// A failed baseline copy. Kept apart from `error` so a later successful append,
    /// which replaces `error` with its own outcome, never hides it (REC-12): the
    /// journal stays unrestorable and waiters must see the failure.
    baseline_error: Option<String>,
}
impl PagedRecoveryStatus {
    /// Publish a durable append whose pointer/checkpoint maintenance reported
    /// `maintenance`. Clears an earlier append error, but not a baseline failure.
    fn record_append(&mut self, receipt: DurableReceipt, maintenance: Option<String>) {
        self.durable = Some(receipt);
        self.error = maintenance.or_else(|| self.baseline_error.clone());
    }
    fn fail_baseline(&mut self, error: String) {
        self.baseline_error = Some(error.clone());
        self.error = Some(error);
    }
}
type Job = Box<dyn FnOnce() + Send>;
fn baseline_worker() -> &'static SyncSender<Job> {
    static WORKER: OnceLock<SyncSender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(16);
        // Thread creation can fail under handle/address-space exhaustion (e.g. a
        // very large file open). A failed spawn drops `rx` with the closure, so the
        // caller's `try_send` fails and surfaces "recovery baseline queue is full"
        // instead of aborting the process.
        let spawned = std::thread::Builder::new()
            .name("recovery-baselines".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job();
                }
            });
        debug_assert!(spawned.is_ok(), "recovery worker");
        tx
    })
}
/// Wakes waiters when a baseline copy settles, so group commits block on a condvar
/// instead of spinning (REC-12).
#[derive(Default)]
pub struct BaselineSignal {
    lock: Mutex<()>,
    settled: std::sync::Condvar,
}
impl BaselineSignal {
    /// Call after publishing the settled state to the status, never while holding it.
    fn notify(&self) {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.settled.notify_all();
    }
    /// Wait until `status` reports the baseline complete (Ok) or failed (its error).
    /// Waits are bounded so `cancelled` is observed promptly.
    pub fn wait(&self, status: &Mutex<PagedRecoveryStatus>, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        let mut guard = self.lock.lock().map_err(|_| "Recovery state stopped")?;
        loop {
            {
                let state = status.lock().map_err(|_| "Recovery state stopped")?;
                if state.complete {
                    return Ok(());
                }
                if let Some(error) = &state.error {
                    return Err(error.clone());
                }
            }
            if cancelled() {
                return Err("Transfer cancelled".into());
            }
            guard = self
                .settled
                .wait_timeout(guard, std::time::Duration::from_millis(50))
                .map_err(|_| "Recovery state stopped")?
                .0;
        }
    }
}
/// A journal's baseline copy, waitable after its owner's lock is released. The
/// journal's own cancellation also ends the wait: a journal retired or dropped
/// meanwhile stops its copy without settling it, so nothing else would (REC-12).
pub struct BaselineWait {
    settled: Arc<BaselineSignal>,
    status: Arc<Mutex<PagedRecoveryStatus>>,
    journal: Cancellation,
}
impl BaselineWait {
    pub fn wait(&self, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        let retired = || self.journal.check().is_err();
        self.settled
            .wait(&self.status, &|| cancelled() || retired())
            .map_err(|error| {
                if retired() && !cancelled() {
                    "Recovery checkpoint was retired while waiting for its baseline; retry.".into()
                } else {
                    error
                }
            })
    }
}
pub struct PagedRecovery {
    writer: Arc<Mutex<RecoveryWriter>>,
    pub status: Arc<Mutex<PagedRecoveryStatus>>,
    baseline_settled: Arc<BaselineSignal>,
    directory: PathBuf,
    store: DiskDecoded,
    baseline: bareline_document::paged::PagedSnapshot,
    platform: Arc<dyn LocalFileSystem>,
    notify: Arc<dyn Fn() + Send + Sync>,
    attempt: u64,
    cancellation: Cancellation,
    _claim: DirectoryClaim,
}
/// Journal directories owned by a live `PagedRecovery`.
static LIVE_DIRECTORIES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// Registration of a journal directory for as long as its `PagedRecovery` lives, so
/// `create_in` can never clear a directory that still holds a live checkpoint.
struct DirectoryClaim(PathBuf);
impl DirectoryClaim {
    fn acquire(directory: &Path) -> Result<Self, String> {
        let mut live = LIVE_DIRECTORIES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let owned = live.iter().any(|owned| owned == directory);
        debug_assert!(
            !owned,
            "recovery journal {} is owned by a live checkpoint",
            directory.display()
        );
        if owned {
            return Err(format!(
                "Recovery journal {} is owned by a live checkpoint",
                directory.display()
            ));
        }
        live.push(directory.to_path_buf());
        Ok(Self(directory.to_path_buf()))
    }
}
impl Drop for DirectoryClaim {
    fn drop(&mut self) {
        let mut live = LIVE_DIRECTORIES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(index) = live.iter().position(|owned| *owned == self.0) {
            live.swap_remove(index);
        }
    }
}
impl PagedRecovery {
    pub fn create(
        root: &Path,
        store: DiskDecoded,
        original_path: Option<PathBuf>,
        baseline: bareline_document::paged::PagedSnapshot,
        platform: Arc<dyn LocalFileSystem>,
        status: Arc<Mutex<PagedRecoveryStatus>>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let directory = root.join(format!(
            "paged-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        Self::create_in(directory, store, original_path, baseline, platform, status, notify)
    }
    /// Create (or overwrite) the journal at an exact directory. Callers that keep one
    /// directory per document rotate through a small fixed set of names with this.
    /// A directory still owned by a live `PagedRecovery` is refused, never cleared.
    pub fn create_in(
        directory: PathBuf,
        store: DiskDecoded,
        original_path: Option<PathBuf>,
        baseline: bareline_document::paged::PagedSnapshot,
        platform: Arc<dyn LocalFileSystem>,
        status: Arc<Mutex<PagedRecoveryStatus>>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let claim = DirectoryClaim::acquire(&directory)?;
        if directory.exists() {
            std::fs::remove_dir_all(&directory).map_err(|e| e.to_string())?;
        }
        if let Some(parent) = directory.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let writer = RecoveryWriter::create(
            &directory,
            RecoveryMetadata {
                original_path,
                source_generation: format!("{:x?}", store.fingerprint.sha256),
                codec_catalog_version: "bareline-codecs-v1".into(),
                original_len: baseline.len() as u64,
            },
            platform.as_ref(),
        )
        .map_err(|e| format!("Create paged recovery journal at {}: {e}", directory.display()))?;
        *status.lock().map_err(|_| "Recovery state stopped")? = PagedRecoveryStatus {
            directory: Some(directory.clone()),
            ..Default::default()
        };
        let mut recovery = Self {
            writer: Arc::new(Mutex::new(writer)),
            status,
            baseline_settled: Arc::default(),
            directory,
            store,
            baseline,
            platform,
            notify,
            attempt: 0,
            cancellation: Cancellation::default(),
            _claim: claim,
        };
        if let Err(error) = recovery.prepare_baseline() {
            recovery
                .status
                .lock()
                .map_err(|_| "Recovery state stopped")?
                .fail_baseline(error);
        }
        Ok(recovery)
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn platform(&self) -> Arc<dyn LocalFileSystem> {
        self.platform.clone()
    }
    /// Wait on the baseline copy without polling and without holding this journal.
    pub fn baseline_wait(&self) -> BaselineWait {
        BaselineWait {
            settled: self.baseline_settled.clone(),
            status: self.status.clone(),
            journal: self.cancellation.clone(),
        }
    }
    pub fn retire(self) -> Result<PathBuf, String> {
        self.cancellation.cancel();
        let _writer = self.writer.lock().map_err(|_| "Recovery writer stopped")?;
        crate::recovery::discard(&self.directory, self.platform.as_ref()).map_err(|e| e.to_string())?;
        Ok(self.directory.clone())
    }
    pub fn tombstone(&mut self) -> Result<(), String> {
        self.cancellation.cancel();
        let _writer = self.writer.lock().map_err(|_| "Recovery writer stopped")?;
        tombstone_directory(&self.directory, self.platform.as_ref())
    }
    /// Retire and then remove the journal directory outright. Used when the document
    /// is closed cleanly or its changes are discarded, so nothing is left on disk.
    pub fn purge(self) -> Result<PathBuf, String> {
        let directory = self.directory.clone();
        let platform = self.platform.clone();
        self.retire()?;
        purge_directory(&directory, platform.as_ref())?;
        Ok(directory)
    }
    pub fn prepare_baseline(&mut self) -> Result<(), String> {
        let preparation = self
            .writer
            .lock()
            .map_err(|_| "Recovery writer stopped")?
            .prepare_baseline()
            .map_err(|e| e.to_string())?;
        self.attempt += 1;
        let name = format!("source-{}", self.attempt);
        let source_path = self.directory.join(&name);
        let directory = self.directory.clone();
        let store = self.store.clone();
        let writer = self.writer.clone();
        let status = self.status.clone();
        let platform = self.platform.clone();
        let notify = self.notify.clone();
        let settled = self.baseline_settled.clone();
        let cancel = self.cancellation.clone();
        let baseline = self.baseline.clone();
        baseline_worker()
            .try_send(crate::recovery_seal::tracked(move || {
                let result = (|| -> Result<(), String> {
                    let retained = store
                        .retain_recovery(&source_path, &cancel)
                        .map_err(|e| format!("Retain paged baseline at {}: {e:?}", source_path.display()))?;
                    let text = retained
                        .sealed_text_reader(&cancel)
                        .map_err(|e| format!("Open retained paged baseline at {}: {e:?}", source_path.display()))?;
                    let mut text = SnapshotRead {
                        source: text,
                        store: store.clone(),
                        foreign_readers: std::collections::BTreeMap::new(),
                        snapshot: baseline,
                        offset: 0,
                        cancellation: cancel.clone(),
                    };
                    let prepared = preparation
                        .copy(&mut text, || Ok(true), &cancel)
                        .map_err(|e| format!("Copy paged baseline into {}: {e}", directory.display()))?;
                    crate::session::publish_json(
                        &directory.join("paged-source.json"),
                        &serde_json::to_vec(&serde_json::json!({"version":1,"source":name}))
                            .map_err(|e| e.to_string())?,
                        platform.as_ref(),
                    )
                    .map_err(|e| format!("Publish paged baseline receipt in {}: {e}", directory.display()))?;
                    writer
                        .lock()
                        .map_err(|_| "Recovery writer stopped")?
                        .attach_baseline(prepared, platform.as_ref())
                        .map_err(|e| format!("Attach paged baseline in {}: {e}", directory.display()))?;
                    Ok(())
                })();
                if let Ok(mut state) = status.lock() {
                    match result {
                        Ok(()) => {
                            state.complete = true;
                        }
                        // A checkpoint retired before its baseline copy finished is not a
                        // failure the reader needs to hear about.
                        Err(error) if cancel.check().is_ok() => state.fail_baseline(error),
                        Err(_) => {}
                    }
                }
                settled.notify();
                notify();
            }))
            .map_err(|_| "Recovery baseline queue is full; retry.".to_owned())
    }
    pub fn append(
        &mut self,
        snapshot: &bareline_document::paged::PagedSnapshot,
        edits: &[RecoveryEdit],
    ) -> Result<(), String> {
        let revision = snapshot.revision.0;
        let _sealed = crate::recovery_seal::active();
        let result = (|| {
            let mut writer = self.writer.lock().map_err(|_| "Recovery writer stopped".to_owned())?;
            // REC-07: as in `append_sources`, the revision recipe is durable before the
            // journal names that revision, so a crash in between cannot strand restore.
            writer.prepare_recipe_revision(revision).map_err(|e| e.to_string())?;
            let root = prepare_root(
                &self.directory,
                snapshot,
                self.platform.as_ref(),
                &self.cancellation,
                20 * 1024 * 1024 * 1024,
                Some(&self.store),
            )
            .map_err(|e| e.to_string())?;
            let receipt = if edits.is_empty() {
                writer.append_metadata(revision, snapshot.metadata())
            } else {
                writer.append(revision, edits)
            }
            .map_err(|e| e.to_string())?;
            // Pointer/checkpoint failures must not turn a durable transaction into an
            // in-memory rejection; restore falls back to the historical receipt.
            let maintenance = publish_root(&self.directory, &root, self.platform.as_ref())
                .and_then(|_| writer.checkpoint(self.platform.as_ref()));
            Ok::<_, String>((receipt, maintenance.err().map(|e| e.to_string())))
        })();
        if result.is_err()
            && let Ok(writer) = self.writer.lock()
        {
            let _ = writer.prepare_recipe_revision(revision);
        }
        let mut status = self.status.lock().map_err(|_| "Recovery state stopped")?;
        match result {
            Ok((receipt, maintenance)) => {
                status.record_append(receipt, maintenance);
                Ok(())
            }
            Err(error) => {
                status.error = Some(error.clone());
                Err(error)
            }
        }
    }
}

impl Drop for PagedRecovery {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

pub(crate) const CLEANUP_PROOF_NAME: &str = ".bareline-cache-cleanup-retained.json";
const CLEANUP_PROOF: &[u8] = br#"{"version":1,"kind":"recovery-retirement"}"#;
const CLEANUP_MAX_ENTRIES: usize = 4096;
const CLEANUP_MAX_TIME: std::time::Duration = std::time::Duration::from_millis(250);
const CLEANUP_PROOF_MAX_BYTES: u64 = 4096;

#[derive(Clone)]
pub(crate) struct CleanupReceipt {
    root: PathBuf,
    candidate: PathBuf,
    root_identity: bareline_platform::CacheDirectoryIdentity,
    candidate_identity: bareline_platform::CacheDirectoryIdentity,
    proof: Vec<u8>,
}
impl CleanupReceipt {
    pub(crate) fn candidate(&self) -> &Path {
        &self.candidate
    }
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        self.root == other.root
            && self.candidate == other.candidate
            && self.root_identity == other.root_identity
            && self.candidate_identity == other.candidate_identity
            && self.proof == other.proof
    }
}

fn cleanup_receipt(directory: &Path, platform: &dyn LocalFileSystem) -> Result<Option<CleanupReceipt>, String> {
    let root = directory.parent().ok_or("Recovery cleanup directory has no root")?;
    let root_lease = platform
        .cache_directory_guard(root)
        .map_err(|error| error.to_string())?
        .ok_or("Recovery cleanup root is not a stable plain directory")?;
    let candidate_lease = match platform.cache_directory_guard(directory) {
        Ok(Some(lease)) => lease,
        Ok(None) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    capture_cleanup_receipt(root, directory, root_lease, candidate_lease, platform)
}

fn capture_cleanup_receipt(
    root: &Path,
    directory: &Path,
    root_lease: bareline_platform::CacheDirectoryLease,
    candidate_lease: bareline_platform::CacheDirectoryLease,
    platform: &dyn LocalFileSystem,
) -> Result<Option<CleanupReceipt>, String> {
    let proof_path = directory.join(CLEANUP_PROOF_NAME);
    let proof_file = match platform.open_sealed_read(&proof_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if proof_file.metadata().map_err(|error| error.to_string())?.len() > CLEANUP_PROOF_MAX_BYTES {
        return Err("Recovery cleanup proof exceeds its bounded size".into());
    }
    let mut proof = Vec::new();
    let mut bounded_proof = proof_file.take(CLEANUP_PROOF_MAX_BYTES + 1);
    bounded_proof
        .read_to_end(&mut proof)
        .map_err(|error| error.to_string())?;
    if proof != CLEANUP_PROOF {
        return Err("Recovery cleanup proof changed".into());
    }
    Ok(Some(CleanupReceipt {
        root: root.to_path_buf(),
        candidate: directory.to_path_buf(),
        root_identity: root_lease.identity,
        candidate_identity: candidate_lease.identity,
        proof,
    }))
}

pub(crate) fn purge_receipt(receipt: &CleanupReceipt, platform: &dyn LocalFileSystem) -> Result<(), String> {
    let outcome = platform.remove_owned_cache_directory(
        &receipt.root,
        &receipt.candidate,
        receipt.root_identity,
        receipt.candidate_identity,
        CLEANUP_PROOF_NAME,
        &receipt.proof,
        CLEANUP_MAX_ENTRIES,
        CLEANUP_MAX_TIME,
        &|| false,
    );
    if !outcome.retry_authority_retained {
        return Err("Recovery cleanup proof could not be retained".into());
    }
    outcome.result.map(|_| ()).map_err(|error| error.to_string())
}

pub(crate) fn publish_cleanup_receipt(
    directory: &Path,
    platform: &dyn LocalFileSystem,
) -> Result<Option<CleanupReceipt>, String> {
    let root = directory.parent().ok_or("Recovery cleanup directory has no root")?;
    let root_lease = platform
        .cache_directory_guard(root)
        .map_err(|error| error.to_string())?
        .ok_or("Recovery cleanup root is not a stable plain directory")?;
    let candidate_lease = match platform.cache_directory_guard(directory) {
        Ok(Some(lease)) => lease,
        Ok(None) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    tombstone_directory(directory, platform)?;
    capture_cleanup_receipt(root, directory, root_lease, candidate_lease, platform)?
        .ok_or_else(|| "Recovery cleanup proof was not retained after tombstone publication".into())
        .map(Some)
}

/// Delete a tombstoned journal through the platform's identity-bound, proof-preserving
/// owned-directory primitive. The platform retains the proof whenever final deletion
/// is uncertain, and unsupported platforms fail closed.
pub fn purge_directory(directory: &Path, platform: &dyn LocalFileSystem) -> Result<(), String> {
    let receipt = match cleanup_receipt(directory, platform)? {
        Some(receipt) => Some(receipt),
        None => publish_cleanup_receipt(directory, platform)?,
    };
    match receipt {
        Some(receipt) => purge_receipt(&receipt, platform),
        None => Ok(()),
    }
}

/// Publish only the content-free durable retirement marker. The guarded path is
/// retained for later cleanup and is never traversed recursively.
pub fn tombstone_directory(directory: &Path, platform: &dyn LocalFileSystem) -> Result<(), String> {
    let _guard = match platform.guard_directory(directory) {
        Ok(guard) => guard,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let metadata = match std::fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Recovery tombstone target is not a guarded directory".into());
    }
    crate::recovery::discard(directory, platform).map_err(|error| error.to_string())?;
    crate::session::publish_json(&directory.join(CLEANUP_PROOF_NAME), CLEANUP_PROOF, platform)
        .map_err(|error| error.to_string())
}

/// Process id encoded in a `paged-<pid>-…` journal directory name.
pub fn directory_owner(name: &str) -> Option<u32> {
    name.strip_prefix("paged-")?.split('-').next()?.parse::<u32>().ok()
}

/// Time the journal was named, in nanoseconds since the Unix epoch, encoded in a
/// `paged-<pid>-<nanos>-…` journal directory name.
pub fn directory_created_nanos(name: &str) -> Option<u128> {
    name.strip_prefix("paged-")?.split('-').nth(1)?.parse::<u128>().ok()
}

/// Publish the cleanup proof for a journal whose manifest can no longer be read, after
/// the user confirmed deletion. A readable journal is retired through `recovery::discard`
/// instead. The directory is removed later by `sweep` once its owner is gone.
pub fn retire_unreadable(directory: &Path, platform: &dyn LocalFileSystem) -> Result<(), String> {
    let _guard = match platform.guard_directory(directory) {
        Ok(guard) => guard,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let metadata = match std::fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Recovery tombstone target is not a guarded directory".into());
    }
    crate::session::publish_json(&directory.join(CLEANUP_PROOF_NAME), CLEANUP_PROOF, platform)
        .map_err(|error| error.to_string())
}

/// Process id of checkpoint scratch that only its creating process uses: the
/// `bareline-transcode-<pid>-<n>` directory and `resident-input-<pid>-<n>.tmp` file a
/// resident checkpoint writes beside its journals. Symlinks never match.
fn scratch_owner(name: &str, directory: bool, file: bool) -> Option<u32> {
    let rest = if directory {
        name.strip_prefix("bareline-transcode-")?
    } else if file {
        name.strip_prefix("resident-input-")?.strip_suffix(".tmp")?
    } else {
        return None;
    };
    let (pid, serial) = rest.split_once('-')?;
    serial.parse::<u64>().ok()?;
    pid.parse().ok()
}

/// True once a journal carries its cleanup proof and only awaits removal by `sweep`.
pub fn cleanup_pending(directory: &Path) -> bool {
    directory.join(CLEANUP_PROOF_NAME).is_file()
}

/// Startup sweep: delete journal directories whose owning process is gone, that no
/// live session still references, and that were already retired (a `Discarded`
/// manifest or a published cleanup proof). Recoverable journals are never deleted
/// here, and neither is a journal whose inspection fails; only the user removes those.
/// Checkpoint scratch left by a dead process (a `bareline-transcode-<pid>-<n>`
/// directory or a `resident-input-<pid>-<n>.tmp` file) is removed too (REC-11).
pub fn sweep(
    root: &Path,
    referenced: &std::collections::HashSet<PathBuf>,
    alive: &dyn Fn(u32) -> bool,
    platform: &dyn LocalFileSystem,
) -> std::io::Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut removed = Vec::new();
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(owner) = scratch_owner(name, kind.is_dir(), kind.is_file())
            && !alive(owner)
        {
            let path = entry.path();
            let deleted = if kind.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if deleted.is_ok() {
                removed.push(path);
            }
            continue;
        }
        if !kind.is_dir() {
            continue;
        }
        let Some(owner) = directory_owner(name) else {
            continue;
        };
        let directory = entry.path();
        if alive(owner) || referenced.contains(&directory) {
            continue;
        }
        let retired = matches!(cleanup_receipt(&directory, platform), Ok(Some(_)))
            || crate::recovery::inspect(&directory, &Cancellation::default())
                .is_ok_and(|inspection| inspection.status == crate::recovery::RecoveryStatus::Discarded);
        if retired && purge_directory(&directory, platform).is_ok() {
            removed.push(directory);
        }
    }
    Ok(removed)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
enum RootPiece {
    Original { start: u64, end: u64 },
    Foreign { generation: u64, start: u64, end: u64 },
    Inserted { text: String },
    Owned { start: u64, end: u64 },
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(Clone)]
struct RootOwned {
    name: String,
    len: u64,
    sha256: [u8; 32],
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(Clone)]
struct RootReceipt {
    version: u32,
    revision: u64,
    file: String,
    sha256: [u8; 32],
    #[serde(default)]
    metadata: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    owned: Option<RootOwned>,
    #[serde(default, deserialize_with = "read_foreign_references")]
    foreign: std::collections::BTreeMap<u64, String>,
}
fn read_foreign_references<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<std::collections::BTreeMap<u64, String>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = std::collections::BTreeMap<u64, String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("at most 100 retained source references")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut result = std::collections::BTreeMap::new();
            while let Some(key) = map.next_key::<u64>()? {
                if result.len() >= 100 {
                    return Err(serde::de::Error::custom("Recovery foreign source limit"));
                }
                let name = map.next_value::<String>()?;
                if name != format!("foreign-{key}") || result.insert(key, name).is_some() {
                    return Err(serde::de::Error::custom("Invalid foreign source reference"));
                }
            }
            Ok(result)
        }
    }
    decoder.deserialize_map(Visitor)
}
fn prepare_root(
    directory: &Path,
    snapshot: &bareline_document::paged::PagedSnapshot,
    platform: &dyn LocalFileSystem,
    cancel: &Cancellation,
    quota: u64,
    sources: Option<&DiskDecoded>,
) -> std::io::Result<RootReceipt> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    let mut foreign = std::collections::BTreeMap::new();
    if let Some(sources) = sources {
        for (generation, store) in sources
            .foreign_sources()
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?
        {
            let name = format!("foreign-{generation}");
            let target = directory.join(&name);
            if !target.exists() {
                crate::recovery::admit_disk(
                    directory,
                    quota,
                    store
                        .retained_size()
                        .map_err(|e| std::io::Error::other(format!("{e:?}")))?,
                    platform,
                    cancel,
                )?;
                store
                    .retain_recovery(&target, cancel)
                    .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
            }
            foreign.insert(generation, name);
        }
    }
    let receipt_bound = RootReceipt {
        version: 2,
        revision: snapshot.revision.0,
        file: format!("root-{}.json", snapshot.revision.0),
        sha256: [255; 32],
        metadata: snapshot.metadata().values().clone(),
        owned: Some(RootOwned {
            name: format!("root-owned-{}.bin", snapshot.revision.0),
            len: u64::MAX,
            sha256: [255; 32],
        }),
        foreign: foreign.clone(),
    };
    let receipt_bytes = serde_json::to_vec(&receipt_bound).map_err(std::io::Error::other)?.len() as u64;
    // Historical receipt plus the future atomic latest-pointer staging file.
    let remaining_quota = crate::recovery::admit_disk(
        directory,
        quota,
        receipt_bytes
            .checked_mul(2)
            .ok_or_else(|| std::io::Error::other("Recovery quota overflow"))?,
        platform,
        cancel,
    )?;
    let remaining = std::rc::Rc::new(std::cell::Cell::new(remaining_quota));
    let mut cleanup = RecipeCleanup {
        directory: directory.into(),
        revision: snapshot.revision.0,
        preserve: false,
        owned: false,
        json: false,
        receipt: false,
    };
    let owned_name = format!("root-owned-{}.bin", snapshot.revision.0);
    let mut owned = RecipeQuotaFile {
        file: std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join(&owned_name))?,
        remaining: remaining.clone(),
        limit: remaining_quota,
        written: 0,
        directory,
        platform,
        cancel,
    };
    cleanup.owned = true;
    let mut owned_hash = Sha256::new();
    let mut owned_len = 0u64;
    let mut store_owned = |text: &str| -> std::io::Result<std::ops::Range<u64>> {
        cancel
            .check()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Interrupted, "Recovery cancelled"))?;
        let start = owned_len;
        if text.len() as u64 > remaining.get().min(platform.available_space(directory)? / 5) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "Recovery owned quota",
            ));
        }
        for chunk in text.as_bytes().chunks(65536) {
            cancel
                .check()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::Interrupted, "Recovery cancelled"))?;
            owned.write_all(chunk)?;
            owned_hash.update(chunk);
            owned_len += chunk.len() as u64;
        }
        Ok(start..owned_len)
    };
    let name = format!("root-{}.json", snapshot.revision.0);
    let mut file = RecipeQuotaFile {
        file: std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join(&name))?,
        remaining: remaining.clone(),
        limit: 128 * 1024 * 1024,
        written: 0,
        directory,
        platform,
        cancel,
    };
    cleanup.json = true;
    file.write_all(b"[")?;
    let mut first = true;
    let mut count = 0usize;
    let mut emit = |piece: RootPiece| -> std::io::Result<()> {
        cancel
            .check()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Interrupted, "Recovery cancelled"))?;
        if count >= 65536 {
            return Err(std::io::Error::other("Recovery piece limit"));
        }
        count += 1;
        if !first {
            file.write_all(b",")?;
        }
        first = false;
        serde_json::to_writer(&mut file, &piece).map_err(std::io::Error::other)?;
        if file.metadata()?.len() > 128 * 1024 * 1024 {
            return Err(std::io::Error::other("Recovery recipe size limit"));
        }
        Ok(())
    };
    for piece in snapshot.pieces() {
        use bareline_document::paged::PagedPiece;
        match piece {
            PagedPiece::Original { source, range } | PagedPiece::OriginalOwned { source, range, .. } => {
                if foreign.contains_key(&source.generation().0) {
                    emit(RootPiece::Foreign {
                        generation: source.generation().0,
                        start: range.start,
                        end: range.end,
                    })?
                } else {
                    emit(RootPiece::Original {
                        start: range.start,
                        end: range.end,
                    })?
                }
            }
            PagedPiece::Inserted(text) => {
                let range = store_owned(text)?;
                emit(RootPiece::Owned {
                    start: range.start,
                    end: range.end,
                })?;
            }
            PagedPiece::OwnedSource {
                source,
                range,
                original,
            } => {
                if let Some((original_source, original_range)) = original {
                    if foreign.contains_key(&original_source.generation().0) {
                        emit(RootPiece::Foreign {
                            generation: original_source.generation().0,
                            start: original_range.start,
                            end: original_range.end,
                        })?;
                    } else {
                        emit(RootPiece::Original {
                            start: original_range.start,
                            end: original_range.end,
                        })?;
                    }
                } else {
                    let mut stored: Option<std::ops::Range<u64>> = None;
                    crate::owned_read::visit_utf8::<std::io::Error>(source, range, cancel, |text| {
                        let next = store_owned(text)?;
                        if let Some(previous) = stored.as_mut() {
                            previous.end = next.end;
                        } else {
                            stored = Some(next);
                        }
                        Ok(())
                    })?;
                    if let Some(range) = stored {
                        emit(RootPiece::Owned {
                            start: range.start,
                            end: range.end,
                        })?;
                    }
                }
            }
        }
    }
    drop(store_owned);
    owned.sync_all()?;
    drop(owned);
    let mut _owned_seal = platform.open_sealed_read(&directory.join(&owned_name))?;
    let mut sealed_hash = Sha256::new();
    let mut sealed_buffer = [0u8; 65536];
    loop {
        cancel
            .check()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Interrupted, "Recovery cancelled"))?;
        let count = _owned_seal.read(&mut sealed_buffer)?;
        if count == 0 {
            break;
        }
        sealed_hash.update(&sealed_buffer[..count]);
    }
    if sealed_hash.finalize() != owned_hash.clone().finalize() {
        return Err(std::io::Error::other("Recovery owned bytes changed before seal"));
    }
    file.write_all(b"]")?;
    if file.metadata()?.len() > 128 * 1024 * 1024 {
        return Err(std::io::Error::other("Recovery recipe size limit"));
    }
    file.sync_all()?;
    drop(file);
    let mut file = std::fs::File::open(directory.join(&name))?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    let receipt = RootReceipt {
        version: 2,
        foreign,
        revision: snapshot.revision.0,
        file: name,
        sha256: hash.finalize().into(),
        metadata: snapshot.metadata().values().clone(),
        owned: Some(RootOwned {
            name: owned_name,
            len: owned_len,
            sha256: owned_hash.finalize().into(),
        }),
    };
    if directory
        .join(format!("root-{}.receipt.json", receipt.revision))
        .try_exists()?
    {
        return Err(std::io::Error::other("Recovery receipt already exists"));
    }
    cleanup.receipt = true;
    crate::session::publish_json(
        &directory.join(format!("root-{}.receipt.json", receipt.revision)),
        &serde_json::to_vec(&receipt).map_err(std::io::Error::other)?,
        platform,
    )?;
    cleanup.preserve = true;
    Ok(receipt)
}
fn publish_root(directory: &Path, receipt: &RootReceipt, platform: &dyn LocalFileSystem) -> std::io::Result<()> {
    crate::session::publish_json(
        &directory.join("paged-root.json"),
        &serde_json::to_vec(receipt).map_err(std::io::Error::other)?,
        platform,
    )
}
/// Read the recovery viewport using every retained source in its validated recipe.
/// Run on an I/O worker; owned/foreign pages are not the primary text generation.
pub fn preview(
    opened: &mut crate::lifecycle::PagedOpened,
    bytes: &bareline_document::Budget,
    cancel: &Cancellation,
) -> Result<String, String> {
    use bareline_document::paged::WindowPoll;
    let snapshot = opened.transcoded.document.snapshot();
    let mut request = snapshot
        .begin_viewport(bareline_document::TextOffset(0), 8192, bytes)
        .map_err(|error| format!("{error:?}"))?;
    loop {
        cancel.check().map_err(|error| format!("{error:?}"))?;
        match request.poll() {
            WindowPoll::Ready(window) => return Ok(window.text().to_string()),
            WindowPoll::Pending(ticket) => {
                if !snapshot.resolve_owned(ticket).map_err(|error| format!("{error:?}"))? {
                    opened
                        .transcoded
                        .source
                        .read_page(ticket)
                        .map_err(|error| format!("{error:?}"))?;
                }
            }
            _ => return Err("Preview range unavailable; export validated saved edits with its gap report.".into()),
        }
    }
}

/// Reconstruct the primary paged document with unchanged raw provenance intact.
/// Never writes to the original source file.
pub fn restore(
    directory: &Path,
    platform: Arc<dyn LocalFileSystem>,
    bytes: bareline_document::Budget,
    history: bareline_document::Budget,
    cancel: &Cancellation,
) -> Result<crate::lifecycle::PagedOpened, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let _directory_guard = platform.guard_directory(directory).map_err(|e| e.to_string())?;
    let read_small = |name: &str| -> Result<Vec<u8>, String> {
        let file = platform
            .open_sealed_read(&directory.join(name))
            .map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > 524288 {
            return Err("Recovery metadata limit".into());
        }
        let mut value = Vec::new();
        file.take(524288).read_to_end(&mut value).map_err(|e| e.to_string())?;
        Ok(value)
    };
    let source: serde_json::Value =
        serde_json::from_slice(&read_small("paged-source.json")?).map_err(|e| e.to_string())?;
    let name = source
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or("Missing recovery source")?;
    if source.get("version").and_then(|v| v.as_u64()) != Some(1)
        || !name.starts_with("source-")
        || !name[7..].bytes().all(|b| b.is_ascii_digit())
    {
        return Err("Invalid recovery source".into());
    }
    let committed_group = group::committed_root(directory, platform.as_ref(), cancel)?;
    let group_revision = committed_group.as_ref().map(|root| root.revision);
    let pointer = read_small("paged-root.json")
        .and_then(|bytes| serde_json::from_slice::<RootReceipt>(&bytes).map_err(|e| e.to_string()));
    let mut root: RootReceipt = match pointer {
        Ok(root) => root,
        Err(error) => committed_group.clone().ok_or(error)?,
    };
    if !matches!(root.version, 1 | 2) || root.file != format!("root-{}.json", root.revision) {
        return Err("Invalid recovery root".into());
    }
    let inspection = crate::recovery::inspect(directory, cancel).map_err(|e| e.to_string())?;
    if inspection.status == crate::recovery::RecoveryStatus::Discarded {
        return Err("Recovery checkpoint was discarded".into());
    }
    // REC-07: journals written before the recipe was prepared ahead of the append can
    // name a revision whose receipt never became durable. Fall back to the newest
    // valid receipt at or below that revision instead of failing the whole restore.
    let mut fell_back = None;
    let mut receipt_at_or_below = |revision: u64| -> Result<RootReceipt, String> {
        let valid = |candidate: u64| -> Option<RootReceipt> {
            let receipt: RootReceipt =
                serde_json::from_slice(&read_small(&format!("root-{candidate}.receipt.json")).ok()?).ok()?;
            (matches!(receipt.version, 1 | 2)
                && receipt.revision == candidate
                && receipt.file == format!("root-{candidate}.json")
                && directory.join(&receipt.file).is_file())
            .then_some(receipt)
        };
        if let Some(receipt) = valid(revision) {
            return Ok(receipt);
        }
        let mut older: Vec<u64> = std::fs::read_dir(directory)
            .map_err(|e| e.to_string())?
            .filter_map(|entry| {
                let name = entry.ok()?.file_name();
                let revision = name.to_str()?.strip_prefix("root-")?.strip_suffix(".receipt.json")?;
                revision.parse::<u64>().ok()
            })
            .filter(|candidate| *candidate < revision)
            .collect();
        older.sort_unstable_by(|a, b| b.cmp(a));
        let receipt = older
            .into_iter()
            .find_map(valid)
            .ok_or_else(|| format!("No valid recovery root at or below revision {revision}"))?;
        fell_back = Some(revision);
        Ok(receipt)
    };
    if inspection.status == crate::recovery::RecoveryStatus::CorruptTail
        && let Some(validated) = inspection.last_durable
        && validated.revision < root.revision
        && group_revision.is_none_or(|revision| revision < root.revision)
    {
        root = receipt_at_or_below(validated.revision)?;
    }
    if let Some(group_root) = committed_group
        && group_root.revision >= root.revision
    {
        root = group_root;
    }
    // The historical recipe is flushed before a streamed journal commit. A
    // failed latest-pointer update must not hide that acknowledged revision.
    if let Some(durable) = inspection.last_durable
        && durable.revision != root.revision
        && (durable.revision > root.revision || group_revision.is_none_or(|revision| revision < root.revision))
    {
        root = receipt_at_or_below(durable.revision)?;
    }
    if !inspection.complete_baseline
        || fell_back.is_none()
            && inspection.last_durable.is_none_or(|r| r.revision != root.revision)
            && group_revision != Some(root.revision)
    {
        return Err("Recovery root is stale or not durable; inspect/export protected edits".into());
    }
    let file = platform
        .open_sealed_read(&directory.join(&root.file))
        .map_err(|e| e.to_string())?;
    let length =
        usize::try_from(file.metadata().map_err(|e| e.to_string())?.len()).map_err(|_| "Recovery recipe limit")?;
    if length > 128 * 1024 * 1024 {
        return Err("Recovery recipe limit".into());
    }
    let charge = length
        .checked_mul(2)
        .and_then(|n| {
            n.checked_add(
                65536
                    * (std::mem::size_of::<RootPiece>()
                        + std::mem::size_of::<bareline_document::paged::RestoredPiece>()),
            )
        })
        .ok_or("Recovery recipe memory limit")?;
    let _scratch = bytes.claim(charge).map_err(|_| "Recovery recipe memory limit")?;
    let mut data = Vec::with_capacity(length);
    file.take(length as u64)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    let actual: [u8; 32] = Sha256::digest(&data).into();
    if actual != root.sha256 {
        return Err("Recovery root hash mismatch".into());
    }
    let pieces = read_pieces(&data).map_err(|e| e.to_string())?;
    let store =
        DiskDecoded::open_retained(&directory.join(name), platform.clone(), cancel).map_err(|e| format!("{e:?}"))?;
    let mut transcoded = store
        .open_paged(
            platform.clone(),
            crate::source::SourceOptions::default(),
            bytes.clone(),
            history.clone(),
            // Completion drops/cancels the restore IoTicket. The published page
            // source outlives that operation, just as a normal finished transcode.
            Cancellation::default(),
        )
        .map_err(|e| format!("{e:?}"))?;
    let owned_source = root
        .owned
        .as_ref()
        .map(|owned| {
            crate::recovery::open_retained_owned(
                directory,
                &owned.name,
                owned.len,
                owned.sha256,
                platform.as_ref(),
                crate::source::SourceOptions::default(),
                bytes.clone(),
                cancel,
            )
        })
        .transpose()
        .map_err(|e| e.to_string())?;
    let mut foreign_sources = std::collections::BTreeMap::new();
    if root.foreign.len() > 100 {
        return Err("Recovery foreign source limit".into());
    }
    for (generation, name) in &root.foreign {
        if *name != format!("foreign-{generation}") {
            return Err("Invalid foreign source name".into());
        }
        let foreign_store = DiskDecoded::open_retained(&directory.join(name), platform.clone(), cancel)
            .map_err(|e| format!("{e:?}"))?;
        let foreign_opened = foreign_store
            .open_paged(
                platform.clone(),
                crate::source::SourceOptions::default(),
                bytes.clone(),
                history.clone(),
                Cancellation::default(),
            )
            .map_err(|e| format!("{e:?}"))?;
        let source = foreign_opened.source.source();
        foreign_store
            .attach_text_loader(&source, cancel)
            .map_err(|e| format!("{e:?}"))?;
        store
            .retain_foreign(source.generation(), &foreign_store)
            .map_err(|e| format!("{e:?}"))?;
        foreign_sources.insert(*generation, source);
    }
    let pieces = pieces
        .into_iter()
        .map(|piece| match piece {
            RootPiece::Original { start, end } => Ok(bareline_document::paged::RestoredPiece::Original(start..end)),
            RootPiece::Foreign { generation, start, end } => {
                let source = foreign_sources
                    .get(&generation)
                    .ok_or("Missing foreign provenance")?
                    .clone();
                Ok(bareline_document::paged::RestoredPiece::OriginalSource {
                    source,
                    range: start..end,
                })
            }
            RootPiece::Inserted { text } => Ok(bareline_document::paged::RestoredPiece::Inserted(text)),
            RootPiece::Owned { start, end } => Ok(bareline_document::paged::RestoredPiece::OwnedSource {
                source: owned_source.clone().ok_or("Missing recovery owned source")?,
                range: start..end,
                original: None,
            }),
        })
        .collect::<Result<Vec<_>, String>>()?;
    transcoded.document = bareline_document::paged::PagedDocument::restore_pieces(
        transcoded.source.source(),
        pieces,
        bytes,
        history,
        bareline_document::Revision(root.revision),
    )
    .map_err(|e| format!("{e:?}"))?;
    transcoded
        .document
        .restore_metadata(bareline_document::DocumentMetadata::new(root.metadata).map_err(|e| format!("{e:?}"))?)
        .map_err(|e| format!("{e:?}"))?;
    cancel.check().map_err(|error| format!("{error:?}"))?;
    // Restore under the document's own name so the tab reads like the document the
    // user lost, not like an internal checkpoint.
    let title = crate::recovery::inspect(directory, cancel)
        .ok()
        .and_then(|inspection| inspection.metadata.original_path)
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Untitled 1".to_owned());
    Ok(crate::lifecycle::PagedOpened {
        recovery_origin: Some(directory.into()),
        recovered_resident: None,
        unrestored_revision: fell_back,
        fingerprint: store.fingerprint.clone(),
        path: directory.join(title),
        transcoded,
    })
}

/// Read the whole restored document as text when it fits `limit` bytes, so a small
/// journal can be adopted by an ordinary in-memory editor instead of a paged one.
pub fn restore_text(
    opened: &mut crate::lifecycle::PagedOpened,
    limit: u64,
    bytes: &bareline_document::Budget,
    cancel: &Cancellation,
) -> Result<Option<String>, String> {
    use bareline_document::paged::WindowPoll;
    let snapshot = opened.transcoded.document.snapshot();
    let length = snapshot.len();
    if length as u64 > limit {
        return Ok(None);
    }
    let mut request = snapshot
        .begin_viewport(bareline_document::TextOffset(0), length, bytes)
        .map_err(|error| format!("{error:?}"))?;
    loop {
        cancel.check().map_err(|error| format!("{error:?}"))?;
        match request.poll() {
            WindowPoll::Ready(window) => return Ok(Some(window.text().to_string())),
            WindowPoll::Pending(ticket) => {
                if !snapshot.resolve_owned(ticket).map_err(|error| format!("{error:?}"))? {
                    opened
                        .transcoded
                        .source
                        .read_page(ticket)
                        .map_err(|error| format!("{error:?}"))?;
                }
            }
            _ => return Ok(None),
        }
    }
}

fn read_pieces(bytes: &[u8]) -> Result<Vec<RootPiece>, serde_json::Error> {
    struct Bounded;
    impl<'de> serde::de::Visitor<'de> for Bounded {
        type Value = Vec<RootPiece>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("at most 65536 recovery pieces")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            let mut pieces = Vec::new();
            while let Some(piece) = sequence.next_element::<RootPiece>()? {
                if pieces.len() >= 65536 {
                    return Err(serde::de::Error::custom("Recovery piece limit"));
                }
                pieces.push(piece);
            }
            Ok(pieces)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let pieces = serde::de::Deserializer::deserialize_seq(&mut deserializer, Bounded)?;
    deserializer.end()?;
    Ok(pieces)
}

struct SnapshotRead {
    source: crate::codecs::disk::SealedStoreRead,
    store: DiskDecoded,
    foreign_readers: std::collections::BTreeMap<u64, crate::codecs::disk::SealedStoreRead>,
    snapshot: bareline_document::paged::PagedSnapshot,
    offset: usize,
    cancellation: Cancellation,
}
impl std::io::Read for SnapshotRead {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        use std::io::{Seek, SeekFrom};
        self.cancellation
            .check()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Interrupted, "Recovery cancelled"))?;
        if out.is_empty() || self.offset == self.snapshot.len() {
            return Ok(0);
        }
        let mut start = 0;
        for piece in self.snapshot.pieces() {
            let length = match &piece {
                bareline_document::paged::PagedPiece::OwnedSource { range, .. } => (range.end - range.start) as usize,
                bareline_document::paged::PagedPiece::Original { range, .. }
                | bareline_document::paged::PagedPiece::OriginalOwned { range, .. } => {
                    (range.end - range.start) as usize
                }
                bareline_document::paged::PagedPiece::Inserted(text) => text.len(),
            };
            if self.offset >= start + length {
                start += length;
                continue;
            }
            let local = self.offset - start;
            let count = (length - local).min(out.len()).min(65536);
            match piece {
                bareline_document::paged::PagedPiece::OwnedSource { source, range, .. } => {
                    crate::owned_read::read_exact(
                        source,
                        range.start + local as u64,
                        &mut out[..count],
                        &self.cancellation,
                    )?
                }
                bareline_document::paged::PagedPiece::Original { source, range }
                | bareline_document::paged::PagedPiece::OriginalOwned { source, range, .. } => {
                    let foreign = self
                        .store
                        .foreign_source(source.generation())
                        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
                    if let Some(store) = foreign {
                        if !self.foreign_readers.contains_key(&source.generation().0) {
                            let reader = store
                                .sealed_text_reader(&self.cancellation)
                                .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
                            self.foreign_readers.insert(source.generation().0, reader);
                        }
                        let reader = self
                            .foreign_readers
                            .get_mut(&source.generation().0)
                            .expect("retained foreign reader");
                        reader.seek(SeekFrom::Start(range.start + local as u64))?;
                        reader.read_exact(&mut out[..count])?;
                    } else {
                        self.source.seek(SeekFrom::Start(range.start + local as u64))?;
                        self.source.read_exact(&mut out[..count])?;
                    }
                }
                bareline_document::paged::PagedPiece::Inserted(text) => {
                    out[..count].copy_from_slice(&text.as_bytes()[local..local + count])
                }
            }
            self.offset += count;
            return Ok(count);
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "Recovery snapshot ended early",
        ))
    }
}

impl PagedRecovery {
    /// Called while the core commit lease holds exclusive actor ownership. The
    /// entire provenance recipe is prepared before the journal becomes durable.
    pub fn append_sources(
        &mut self,
        snapshot: &bareline_document::paged::PagedSnapshot,
        edits: &[bareline_document::paged::SourceEdit],
        quota: u64,
    ) -> Result<(), String> {
        let result: Result<(), String> = (|| {
            self.writer
                .lock()
                .map_err(|_| "Recovery writer stopped")?
                .prepare_recipe_revision(snapshot.revision.0)
                .map_err(|e| e.to_string())?;
            let root = prepare_root(
                &self.directory,
                snapshot,
                self.platform.as_ref(),
                &self.cancellation,
                quota,
                Some(&self.store),
            )
            .map_err(|e| e.to_string())?;
            let journal_quota = quota
                .checked_sub(serde_json::to_vec(&root).map_err(|e| e.to_string())?.len() as u64)
                .ok_or("Recovery pointer quota")?;
            let mut writer = self.writer.lock().map_err(|_| "Recovery writer stopped".to_owned())?;
            let receipt = writer
                .append_source_transaction(
                    snapshot.revision.0,
                    edits,
                    snapshot.metadata(),
                    journal_quota,
                    &self.cancellation,
                    self.platform.as_ref(),
                )
                .map_err(|e| e.to_string())?;
            // The revision recipe is already durable. Pointer/checkpoint failures
            // must not turn a durable transaction into an in-memory rejection.
            let maintenance = publish_root(&self.directory, &root, self.platform.as_ref())
                .and_then(|_| writer.checkpoint(self.platform.as_ref()));
            if let Ok(mut status) = self.status.lock() {
                status.record_append(receipt, maintenance.err().map(|e| e.to_string()));
            }
            Ok(())
        })();
        if let Err(error) = &result {
            if let Ok(writer) = self.writer.lock() {
                let _ = writer.prepare_recipe_revision(snapshot.revision.0);
            }
            if let Ok(mut status) = self.status.lock() {
                status.error = Some(error.clone());
            }
        }
        result
    }
}
impl PagedRecovery {
    pub fn append_source_history(
        &mut self,
        snapshot: &bareline_document::paged::PagedSnapshot,
        edits: &[bareline_document::paged::HistorySourceEdit],
        quota: u64,
    ) -> Result<(), String> {
        let result: Result<(), String> = (|| {
            self.writer
                .lock()
                .map_err(|_| "Recovery writer stopped")?
                .prepare_recipe_revision(snapshot.revision.0)
                .map_err(|e| e.to_string())?;
            let root = prepare_root(
                &self.directory,
                snapshot,
                self.platform.as_ref(),
                &self.cancellation,
                quota,
                Some(&self.store),
            )
            .map_err(|e| e.to_string())?;
            let ranges: Vec<_> = edits
                .iter()
                .map(|edit| {
                    (
                        edit.range.start.0 as u64,
                        edit.removed.len() as u64,
                        edit.inserted.len() as u64,
                    )
                })
                .collect();
            let journal_quota = quota
                .checked_sub(serde_json::to_vec(&root).map_err(|e| e.to_string())?.len() as u64)
                .ok_or("Recovery pointer quota")?;
            let mut writer = self.writer.lock().map_err(|_| "Recovery writer stopped".to_owned())?;
            let receipt = writer
                .append_streams(
                    snapshot.revision.0,
                    &ranges,
                    snapshot.metadata(),
                    journal_quota,
                    &self.cancellation,
                    self.platform.as_ref(),
                    |output| {
                        for edit in edits {
                            for captured in [&edit.removed, &edit.inserted] {
                                let mut original = self
                                    .store
                                    .sealed_text_reader(&self.cancellation)
                                    .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
                                stream_snapshot(captured, &self.store, &mut original, &self.cancellation, output)?;
                            }
                        }
                        Ok(())
                    },
                )
                .map_err(|e| e.to_string())?;
            let maintenance = publish_root(&self.directory, &root, self.platform.as_ref())
                .and_then(|_| writer.checkpoint(self.platform.as_ref()));
            if let Ok(mut status) = self.status.lock() {
                status.record_append(receipt, maintenance.err().map(|e| e.to_string()));
            }
            Ok(())
        })();
        if let Err(error) = &result {
            if let Ok(writer) = self.writer.lock() {
                let _ = writer.prepare_recipe_revision(snapshot.revision.0);
            }
            if let Ok(mut status) = self.status.lock() {
                status.error = Some(error.clone());
            }
        }
        result
    }
}

fn stream_snapshot(
    snapshot: &bareline_document::paged::PagedSnapshot,
    store: &DiskDecoded,
    original: &mut crate::codecs::disk::SealedStoreRead,
    cancel: &Cancellation,
    output: &mut dyn std::io::Write,
) -> std::io::Result<()> {
    use bareline_document::paged::PagedPiece;
    use std::io::{Read, Seek, SeekFrom};
    let mut buffer = [0u8; 65536];
    for piece in snapshot.pieces() {
        match piece {
            PagedPiece::Inserted(text) => {
                for chunk in text.as_bytes().chunks(65536) {
                    cancel
                        .check()
                        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Interrupted, "Recovery cancelled"))?;
                    output.write_all(chunk)?;
                }
            }
            PagedPiece::OwnedSource { source, range, .. } => {
                crate::owned_read::visit_utf8::<std::io::Error>(source, range, cancel, |text| {
                    output.write_all(text.as_bytes())
                })?
            }
            PagedPiece::Original { source, range } | PagedPiece::OriginalOwned { source, range, .. } => {
                let foreign = store
                    .foreign_source(source.generation())
                    .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
                let mut foreign_reader = foreign
                    .as_ref()
                    .map(|store| store.sealed_text_reader(cancel))
                    .transpose()
                    .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
                let reader = foreign_reader.as_mut().unwrap_or(&mut *original);
                reader.seek(SeekFrom::Start(range.start))?;
                let mut remaining = range.end - range.start;
                while remaining > 0 {
                    cancel
                        .check()
                        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Interrupted, "Recovery cancelled"))?;
                    let count = remaining.min(buffer.len() as u64) as usize;
                    reader.read_exact(&mut buffer[..count])?;
                    output.write_all(&buffer[..count])?;
                    remaining -= count as u64;
                }
            }
        }
    }
    Ok(())
}
struct RecipeQuotaFile<'a> {
    file: std::fs::File,
    remaining: std::rc::Rc<std::cell::Cell<u64>>,
    limit: u64,
    written: u64,
    directory: &'a Path,
    platform: &'a dyn LocalFileSystem,
    cancel: &'a Cancellation,
}
impl std::ops::Deref for RecipeQuotaFile<'_> {
    type Target = std::fs::File;
    fn deref(&self) -> &Self::Target {
        &self.file
    }
}
impl std::io::Write for RecipeQuotaFile<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.cancel
            .check()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Interrupted, "Recovery cancelled"))?;
        let length = bytes.len() as u64;
        if length > self.remaining.get()
            || length > self.limit.saturating_sub(self.written)
            || length > self.platform.available_space(self.directory)? / 5
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "Recovery recipe disk quota",
            ));
        }
        let count = self.file.write(bytes)?;
        self.remaining.set(self.remaining.get() - count as u64);
        self.written += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.file)
    }
}
struct RecipeCleanup {
    directory: PathBuf,
    revision: u64,
    preserve: bool,
    owned: bool,
    json: bool,
    receipt: bool,
}
impl Drop for RecipeCleanup {
    fn drop(&mut self) {
        if !self.preserve {
            for (created, name) in [
                (self.json, format!("root-{}.json", self.revision)),
                (self.receipt, format!("root-{}.receipt.json", self.revision)),
                (self.owned, format!("root-owned-{}.bin", self.revision)),
            ] {
                if created {
                    let _ = std::fs::remove_file(self.directory.join(name));
                }
            }
        }
    }
}
#[cfg(test)]
mod quota_tests {
    use super::*;
    use std::{fs, io};
    struct Platform;
    impl LocalFileSystem for Platform {
        fn guard_directory(&self, _: &Path) -> io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn identity(&self, _: &fs::File) -> io::Result<bareline_platform::FileIdentity> {
            Err(io::Error::other("unused"))
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
            fs::rename(staged, target)
        }
        fn available_space(&self, _: &Path) -> io::Result<u64> {
            Ok(1 << 40)
        }
        fn open_sealed_read(&self, path: &Path) -> io::Result<fs::File> {
            fs::File::open(path)
        }
    }
    #[test]
    fn empty_owned_recipe_still_consumes_quota_and_failed_prepare_cleans_files() {
        use bareline_document::{
            Budget,
            paged::PagedSnapshot,
            source::{Generation, MemorySource, SourceKind},
        };
        let path = std::env::temp_dir().join(format!(
            "bareline-recipe-quota-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let (source, _) =
            MemorySource::new(0, Generation(1), SourceKind::Paged, 4096, 4096, Budget::new(65536)).unwrap();
        let snapshot = PagedSnapshot::utf8(source, 0).unwrap();
        let cancel = Cancellation::default();
        let root = prepare_root(&path, &snapshot, &Platform, &cancel, 8192, None).unwrap();
        assert_eq!(root.owned.as_ref().unwrap().len, 0);
        let physical = crate::recovery::disk_usage(&path, &cancel).unwrap();
        assert!(physical > 2);
        fs::remove_file(path.join(root.file)).unwrap();
        fs::remove_file(path.join("root-0.receipt.json")).unwrap();
        fs::remove_file(path.join("root-owned-0.bin")).unwrap();
        assert!(prepare_root(&path, &snapshot, &Platform, &cancel, physical - 1, None).is_err());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        fs::write(path.join("retained.bin"), b"existing").unwrap();
        assert!(prepare_root(&path, &snapshot, &Platform, &cancel, 0, None).is_err());
        assert_eq!(fs::read(path.join("retained.bin")).unwrap(), b"existing");
        fs::remove_dir_all(path).unwrap();
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;
    use std::{fs, io};
    struct Platform;
    impl LocalFileSystem for Platform {
        fn guard_directory(&self, _: &Path) -> io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn cache_directory_guard(&self, path: &Path) -> io::Result<Option<bareline_platform::CacheDirectoryLease>> {
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Ok(None);
            }
            use std::hash::{Hash, Hasher};
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut hash);
            Ok(Some(bareline_platform::CacheDirectoryLease {
                path: path.to_path_buf(),
                identity: bareline_platform::CacheDirectoryIdentity {
                    volume: 1,
                    file: hash.finish(),
                },
                guard: Arc::new(()),
                migration_publisher: None,
            }))
        }
        fn remove_owned_cache_directory(
            &self,
            root: &Path,
            candidate: &Path,
            _: bareline_platform::CacheDirectoryIdentity,
            _: bareline_platform::CacheDirectoryIdentity,
            proof_name: &str,
            proof_bytes: &[u8],
            _: usize,
            _: std::time::Duration,
            _: &dyn Fn() -> bool,
        ) -> bareline_platform::CacheRemovalOutcome {
            let result = (|| {
                if candidate.parent() != Some(root) || fs::read(candidate.join(proof_name))? != proof_bytes {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "test cleanup proof changed",
                    ));
                }
                fs::remove_dir_all(candidate)?;
                Ok(1)
            })();
            bareline_platform::CacheRemovalOutcome {
                visited: 1,
                retry_authority_retained: true,
                result,
            }
        }
        fn identity(&self, _: &fs::File) -> io::Result<bareline_platform::FileIdentity> {
            Err(io::Error::other("unused"))
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
            fs::rename(staged, target)
        }
        fn available_space(&self, _: &Path) -> io::Result<u64> {
            Ok(1 << 40)
        }
        fn open_sealed_read(&self, path: &Path) -> io::Result<fs::File> {
            fs::File::open(path)
        }
    }
    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bareline-sweep-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn journal(directory: &Path) {
        drop(
            crate::recovery::RecoveryWriter::create(
                directory,
                crate::recovery::RecoveryMetadata {
                    original_path: None,
                    source_generation: "test".into(),
                    codec_catalog_version: "test".into(),
                    original_len: 0,
                },
                &Platform,
            )
            .unwrap(),
        );
    }
    #[test]
    fn sweep_removes_only_retired_abandoned_journals() {
        let root = temp_root("retired");
        let dead = root.join("paged-424242-1-1");
        let retired = root.join("paged-424244-1-1");
        let referenced = root.join("paged-424243-1-1");
        let live = root.join(format!("paged-{}-1-1", std::process::id()));
        let other = root.join("not-a-journal");
        for directory in [&dead, &retired, &referenced, &live] {
            journal(directory);
        }
        for directory in [&retired, &referenced, &live] {
            crate::recovery::discard(directory, &Platform).unwrap();
        }
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("manifest.json"), b"{}").unwrap();
        let references: std::collections::HashSet<PathBuf> = [referenced.clone()].into_iter().collect();
        let removed = sweep(&root, &references, &|id| id == std::process::id(), &Platform).unwrap();
        assert_eq!(removed, vec![retired.clone()]);
        assert!(!retired.exists());
        // A recoverable journal of a dead process is never swept.
        assert!(dead.exists() && referenced.exists() && live.exists() && other.exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sweep_removes_checkpoint_scratch_of_dead_processes_only() {
        let root = temp_root("scratch");
        let live = std::process::id();
        let dead_transcode = root.join("bareline-transcode-424242-3");
        let dead_input = root.join("resident-input-424242-7.tmp");
        let live_transcode = root.join(format!("bareline-transcode-{live}-3"));
        let live_input = root.join(format!("resident-input-{live}-7.tmp"));
        let unrelated = root.join("resident-input-424242-7.tmp.keep");
        for directory in [&dead_transcode, &live_transcode] {
            fs::create_dir(directory).unwrap();
            fs::write(directory.join("text.utf8"), b"scratch").unwrap();
        }
        for file in [&dead_input, &live_input, &unrelated] {
            fs::write(file, b"scratch").unwrap();
        }
        let mut removed = sweep(&root, &Default::default(), &|id| id == live, &Platform).unwrap();
        removed.sort();
        let mut expected = vec![dead_transcode.clone(), dead_input.clone()];
        expected.sort();
        assert_eq!(removed, expected);
        assert!(!dead_transcode.exists() && !dead_input.exists());
        assert!(live_transcode.exists() && live_input.exists() && unrelated.exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sweep_keeps_every_recoverable_journal_of_dead_processes() {
        let root = temp_root("many");
        let directories: Vec<PathBuf> = (0..25)
            .map(|index| root.join(format!("paged-{}-{index}-1", 500_000 + index)))
            .collect();
        for directory in &directories {
            journal(directory);
        }
        assert!(
            sweep(&root, &Default::default(), &|_| false, &Platform)
                .unwrap()
                .is_empty()
        );
        assert!(directories.iter().all(|directory| directory.exists()));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sweep_keeps_a_journal_whose_manifest_is_corrupt() {
        let root = temp_root("corrupt");
        let corrupt = root.join("paged-424245-1-1");
        journal(&corrupt);
        fs::write(corrupt.join("manifest.json"), b"not json").unwrap();
        let _ = fs::remove_file(corrupt.join("manifest.previous.json"));
        assert!(crate::recovery::inspect(&corrupt, &Cancellation::default()).is_err());
        assert!(
            sweep(&root, &Default::default(), &|_| false, &Platform)
                .unwrap()
                .is_empty()
        );
        assert!(corrupt.join("manifest.json").exists());
        // After the user confirms deletion the cleanup proof lets the sweep remove it.
        retire_unreadable(&corrupt, &Platform).unwrap();
        assert_eq!(
            sweep(&root, &Default::default(), &|_| false, &Platform).unwrap(),
            vec![corrupt.clone()]
        );
        assert!(!corrupt.exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod journal_order_tests {
    use super::*;
    use crate::{
        codecs::disk::DiskOptions,
        lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
        source::SourceOptions,
    };
    use bareline_document::{Budget, DocumentMetadata, paged::PagedDocument};
    use std::{
        fs::{self, File},
        io,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    /// Fails the recipe seal while `fail_recipe` is set, modelling a crash while the
    /// revision recipe is written, and the baseline receipt while `fail_baseline` is.
    struct Platform {
        fail_recipe: AtomicBool,
        fail_baseline: AtomicBool,
    }
    impl LocalFileSystem for Platform {
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn available_space(&self, _: &Path) -> io::Result<u64> {
            Ok(u64::MAX)
        }
        fn guard_directory(&self, _: &Path) -> io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn open_sealed_read(&self, path: &Path) -> io::Result<File> {
            let recipe = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("root-owned-"));
            if recipe && self.fail_recipe.load(Ordering::SeqCst) {
                return Err(io::Error::other("injected crash while writing the recipe"));
            }
            File::open(path)
        }
        fn identity(&self, file: &File) -> io::Result<bareline_platform::FileIdentity> {
            let metadata = file.metadata()?;
            Ok(bareline_platform::FileIdentity {
                volume: 1,
                file: metadata.len(),
                length: metadata.len(),
                modified: metadata
                    .modified()?
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
            })
        }
        fn commit(&self, stage: &Path, target: &Path, _: bool) -> io::Result<()> {
            if self.fail_baseline.load(Ordering::SeqCst)
                && target.file_name().is_some_and(|name| name == "paged-source.json")
            {
                return Err(io::Error::other("injected baseline copy failure"));
            }
            fs::rename(stage, target)
        }
    }
    struct Fixture {
        root: PathBuf,
        platform: Arc<Platform>,
        document: Option<PagedDocument>,
        recovery: Option<PagedRecovery>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            drop(self.recovery.take());
            drop(self.document.take());
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn fixture(label: &str) -> Fixture {
        fixture_with(label, false)
    }
    /// A paged document with a journal whose baseline copy has settled: complete, or
    /// failed when `fail_baseline` is set.
    fn fixture_with(label: &str, fail_baseline: bool) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "bareline-journal-order-{label}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source.txt");
        fs::write(&source, b"alpha\n").unwrap();
        let platform = Arc::new(Platform {
            fail_recipe: AtomicBool::new(false),
            fail_baseline: AtomicBool::new(fail_baseline),
        });
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path: source.clone(),
                bytes: Budget::new(4 * 1024 * 1024),
                history: Budget::new(1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 4 * 1024 * 1024,
                    interpret: None,
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    page_size_bytes: 4096,
                    page_cache_bytes: 8192,
                },
            },
            platform.clone(),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("paged fixture did not open")
        };
        let crate::lifecycle::PagedOpened { transcoded, .. } = *opened;
        let status = Arc::new(Mutex::new(PagedRecoveryStatus::default()));
        let recovery = PagedRecovery::create(
            &root.join("recovery"),
            transcoded.store.clone(),
            Some(source),
            transcoded.document.snapshot(),
            platform.clone(),
            status.clone(),
            Arc::new(|| {}),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let status = status.lock().unwrap().clone();
            if fail_baseline {
                assert!(!status.complete);
                if status.error.is_some() {
                    break;
                }
            } else {
                assert!(status.error.is_none(), "{:?}", status.error);
                if status.complete {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "recovery baseline never completed");
            std::thread::sleep(Duration::from_millis(1));
        }
        Fixture {
            root,
            platform,
            document: Some(transcoded.document),
            recovery: Some(recovery),
        }
    }
    fn revise(document: &mut PagedDocument, value: &str) -> bareline_document::paged::PagedSnapshot {
        let base = document.snapshot().revision;
        let metadata = DocumentMetadata::new([("test.revision".to_owned(), value.to_owned())].into()).unwrap();
        document.apply_metadata(base, metadata).unwrap();
        document.snapshot()
    }
    /// The restored revision, its marker, and the acknowledged revision it could not restore.
    fn restored_revision(fixture: &Fixture, directory: &Path) -> (u64, Option<String>, Option<u64>) {
        let restored = restore(
            directory,
            fixture.platform.clone(),
            Budget::new(4 * 1024 * 1024),
            Budget::new(1024 * 1024),
            &Cancellation::default(),
        )
        .unwrap();
        let snapshot = restored.transcoded.document.snapshot();
        (
            snapshot.revision.0,
            snapshot.metadata().get("test.revision").map(str::to_owned),
            restored.unrestored_revision,
        )
    }
    #[test]
    fn crash_while_writing_the_recipe_leaves_the_journal_on_the_previous_root() {
        let mut fixture = fixture("crash");
        let first = revise(fixture.document.as_mut().unwrap(), "1");
        let second = revise(fixture.document.as_mut().unwrap(), "2");
        let recovery = fixture.recovery.as_mut().unwrap();
        recovery.append(&first, &[]).unwrap();
        fixture.platform.fail_recipe.store(true, Ordering::SeqCst);
        assert!(recovery.append(&second, &[]).is_err());
        fixture.platform.fail_recipe.store(false, Ordering::SeqCst);
        let directory = recovery.directory().to_path_buf();
        // The journal never names a revision whose recipe is missing (REC-07).
        let inspection = crate::recovery::inspect(&directory, &Cancellation::default()).unwrap();
        assert_eq!(
            inspection.last_durable.map(|receipt| receipt.revision),
            Some(first.revision.0)
        );
        assert!(
            !directory
                .join(format!("root-{}.receipt.json", second.revision.0))
                .exists()
        );
        assert_eq!(
            restored_revision(&fixture, &directory),
            (first.revision.0, Some("1".to_owned()), None)
        );
    }
    #[test]
    fn restore_falls_back_to_the_newest_root_below_a_durable_revision_without_one() {
        let mut fixture = fixture("fallback");
        let first = revise(fixture.document.as_mut().unwrap(), "1");
        let second = revise(fixture.document.as_mut().unwrap(), "2");
        let recovery = fixture.recovery.as_mut().unwrap();
        recovery.append(&first, &[]).unwrap();
        recovery.append(&second, &[]).unwrap();
        let directory = recovery.directory().to_path_buf();
        // Model a journal written by the old ordering: revision 2 is durable in the
        // journal, but its recipe never landed and the pointer still names revision 1.
        let revision = second.revision.0;
        for name in [
            format!("root-{revision}.receipt.json"),
            format!("root-{revision}.json"),
            format!("root-owned-{revision}.bin"),
        ] {
            fs::remove_file(directory.join(name)).unwrap();
        }
        fs::copy(
            directory.join(format!("root-{}.receipt.json", first.revision.0)),
            directory.join("paged-root.json"),
        )
        .unwrap();
        let inspection = crate::recovery::inspect(&directory, &Cancellation::default()).unwrap();
        assert_eq!(inspection.last_durable.map(|receipt| receipt.revision), Some(revision));
        // The fallback is reported so the user hears that revision 2 was not restored.
        assert_eq!(
            restored_revision(&fixture, &directory),
            (first.revision.0, Some("1".to_owned()), Some(revision))
        );
    }
    #[test]
    fn successful_append_keeps_a_failed_baseline_visible() {
        let mut fixture = fixture_with("baseline-failed", true);
        let first = revise(fixture.document.as_mut().unwrap(), "1");
        let recovery = fixture.recovery.as_mut().unwrap();
        let failed = recovery.status.lock().unwrap().error.clone();
        assert!(failed.is_some());
        recovery.append(&first, &[]).unwrap();
        let status = recovery.status.lock().unwrap().clone();
        assert_eq!(status.durable.map(|receipt| receipt.revision), Some(first.revision.0));
        // The journal is still unrestorable: the notice, the pending check and every
        // baseline waiter must keep seeing the copy failure (REC-12).
        assert!(!status.complete);
        assert_eq!(status.error, failed);
        assert_eq!(recovery.baseline_wait().wait(&|| false), Err(failed.unwrap()));
    }
    #[test]
    fn retiring_or_dropping_the_journal_ends_a_pending_baseline_wait() {
        for retire in [true, false] {
            let mut fixture = fixture(if retire { "wait-retire" } else { "wait-drop" });
            let recovery = fixture.recovery.take().unwrap();
            // Model a copy still running when the journal goes away: its worker exits
            // on the journal's cancellation without settling the status.
            recovery.status.lock().unwrap().complete = false;
            let wait = recovery.baseline_wait();
            let (done_tx, done_rx) = mpsc::sync_channel(1);
            let waiter = std::thread::spawn(move || {
                let _ = done_tx.send(wait.wait(&|| false));
            });
            if retire {
                recovery.retire().unwrap();
            } else {
                drop(recovery);
            }
            let result = done_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("baseline wait outlived its journal");
            waiter.join().unwrap();
            assert!(result.unwrap_err().contains("retired"));
        }
    }
}

#[cfg(test)]
mod baseline_signal_tests {
    use super::*;
    #[test]
    fn group_wait_blocks_on_the_signal_until_the_baseline_settles() {
        let signal = Arc::new(BaselineSignal::default());
        let status = Arc::new(Mutex::new(PagedRecoveryStatus::default()));
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let waiter = {
            let signal = signal.clone();
            let status = status.clone();
            std::thread::spawn(move || {
                entered_tx.send(()).unwrap();
                signal.wait(&status, &|| false)
            })
        };
        entered_rx.recv().unwrap();
        status.lock().unwrap().complete = true;
        signal.notify();
        assert_eq!(waiter.join().unwrap(), Ok(()));
        let failed = Mutex::new(PagedRecoveryStatus {
            error: Some("copy failed".into()),
            ..Default::default()
        });
        assert_eq!(signal.wait(&failed, &|| false), Err("copy failed".to_owned()));
        let pending = Mutex::new(PagedRecoveryStatus::default());
        assert_eq!(signal.wait(&pending, &|| true), Err("Transfer cancelled".to_owned()));
    }
}
