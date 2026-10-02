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
    /// Append-only owned text shared by this journal's roots (REC-09).
    owned: Option<Box<OwnedStore>>,
    /// Roots this journal published, oldest first, with their owned file names.
    roots: std::collections::VecDeque<(u64, Option<String>)>,
    /// Every group-committed revision. A group commit marker names the roots of all
    /// its members and restore of any member verifies each of them, so these are
    /// never pruned while the journal lives; they are bounded by the transfers.
    group_roots: std::collections::BTreeSet<u64>,
    /// Superseded files whose removal failed; retried after the next durable root.
    stale: Vec<String>,
    /// Write every root as a complete per-revision file (receipt version 2), the
    /// layout journals had before the append-only store; kept for compatibility tests.
    per_revision_roots: bool,
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
            owned: None,
            roots: Default::default(),
            group_roots: Default::default(),
            stale: Vec::new(),
            per_revision_roots: false,
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
                // The copy wrote files no admission accounted for (REC-09).
                if let Ok(mut writer) = writer.lock() {
                    writer.usage.invalidate();
                }
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
        let owned = self.owned_mode();
        let result = (|| -> Result<Appended, String> {
            let mut writer = self.writer.lock().map_err(|_| "Recovery writer stopped".to_owned())?;
            // REC-07: as in `append_sources`, the revision recipe is durable before the
            // journal names that revision, so a crash in between cannot strand restore.
            writer.prepare_recipe_revision(revision).map_err(|e| e.to_string())?;
            let (root, store) = prepare_root(
                &self.directory,
                snapshot,
                Some(&self.store),
                RecipeContext {
                    platform: self.platform.as_ref(),
                    cancel: &self.cancellation,
                    quota: 20 * 1024 * 1024 * 1024,
                    usage: &mut writer.usage,
                },
                owned,
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
            Ok((receipt, maintenance.err().map(|e| e.to_string()), root, store))
        })();
        self.settle_append(revision, result)
    }
    /// Journal a resident snapshot as one root over this journal's sealed source
    /// (REC-10). Original text stays a range of that source and unchanged owned text
    /// is found in the append-only store, so a checkpoint while typing writes only
    /// the new text and the recipe instead of copying the whole document again.
    pub fn append_resident(
        &mut self,
        snapshot: &bareline_document::DocumentSnapshot,
        encoding: Option<&crate::codecs::resident::ResidentEncoding>,
    ) -> Result<(), String> {
        use crate::codecs::resident::RecoverySpan;
        let revision = snapshot.revision.0;
        let _sealed = crate::recovery_seal::active();
        let spans = match encoding {
            Some(encoding) => encoding.recovery_spans(snapshot).map_err(|e| format!("{e:?}"))?,
            None => snapshot
                .chunks(bareline_document::TextOffset(0)..bareline_document::TextOffset(snapshot.len()))
                .map_err(|e| format!("{e:?}"))?
                .map(RecoverySpan::Text)
                .collect(),
        };
        let owned = self.owned_mode();
        let result = (|| -> Result<Appended, String> {
            self.cancellation.check().map_err(|e| format!("{e:?}"))?;
            let mut writer = self.writer.lock().map_err(|_| "Recovery writer stopped".to_owned())?;
            writer.prepare_recipe_revision(revision).map_err(|e| e.to_string())?;
            let pieces = spans.iter().map(|span| match span {
                RecoverySpan::Original(range) => RecipePiece::Original(range.clone()),
                RecoverySpan::Text(text) => RecipePiece::Text(*text),
            });
            let (root, store) = prepare_recipe(
                &self.directory,
                revision,
                snapshot.metadata(),
                Default::default(),
                pieces,
                RecipeContext {
                    platform: self.platform.as_ref(),
                    cancel: &self.cancellation,
                    quota: 20 * 1024 * 1024 * 1024,
                    usage: &mut writer.usage,
                },
                owned,
            )
            .map_err(|e| e.to_string())?;
            let store = store.map(|mut store| {
                store._retained = Some(Retained::Resident {
                    _snapshot: snapshot.clone(),
                });
                store
            });
            let receipt = writer
                .append_metadata(revision, snapshot.metadata())
                .map_err(|e| e.to_string())?;
            let maintenance = publish_root(&self.directory, &root, self.platform.as_ref())
                .and_then(|_| writer.checkpoint(self.platform.as_ref()));
            Ok((receipt, maintenance.err().map(|e| e.to_string()), root, store))
        })();
        self.settle_append(revision, result)
    }
    /// Bytes held by the append-only owned store; drives the resident full-copy schedule.
    pub fn owned_bytes(&self) -> u64 {
        self.owned.as_ref().map_or(0, |store| store.len)
    }
    /// Revision of the newest root this journal published.
    pub fn last_root(&self) -> Option<u64> {
        self.roots.back().map(|(revision, _)| *revision)
    }
    fn owned_mode(&mut self) -> OwnedMode {
        if self.per_revision_roots {
            OwnedMode::PerRevision
        } else {
            // Taken: any failure before the root is durable starts a new store.
            OwnedMode::Append(self.owned.take())
        }
    }
    fn settle_append(&mut self, revision: u64, result: Result<Appended, String>) -> Result<(), String> {
        match result {
            Ok((receipt, maintenance, root, store)) => {
                self.root_committed(&root, store);
                let mut status = self.status.lock().map_err(|_| "Recovery state stopped")?;
                status.record_append(receipt, maintenance);
                Ok(())
            }
            Err(error) => {
                if let Ok(writer) = self.writer.lock() {
                    let _ = writer.prepare_recipe_revision(revision);
                }
                let mut status = self.status.lock().map_err(|_| "Recovery state stopped")?;
                status.error = Some(error.clone());
                Err(error)
            }
        }
    }
    /// The journal names `root` durably: keep its store and prune what it supersedes.
    fn root_committed(&mut self, root: &RootReceipt, store: Option<Box<OwnedStore>>) {
        self.owned = store;
        self.remember_root(root);
    }
    /// Prune roots superseded by the durable `root` (REC-09). The newest two stay (the
    /// older one is restore's fallback when the newest record is damaged), and so does
    /// every group-committed root: another member's group pointer can still name the
    /// marker that lists it, however many newer groups this journal joined. Removal
    /// runs only after the journal names the new root, so any interruption merely
    /// leaves extra files; failed removals are retried after the next durable root.
    fn remember_root(&mut self, root: &RootReceipt) {
        self.roots
            .push_back((root.revision, root.owned.as_ref().map(|owned| owned.name.clone())));
        let keep = self.roots.len().saturating_sub(2);
        let group_roots = &self.group_roots;
        let mut index = 0;
        let mut superseded = Vec::new();
        self.roots.retain(|(revision, owned)| {
            let kept = index >= keep || group_roots.contains(revision);
            index += 1;
            if !kept {
                superseded.push((*revision, owned.clone()));
            }
            kept
        });
        for (revision, owned) in superseded {
            self.stale.push(format!("root-{revision}.json"));
            self.stale.push(format!("root-{revision}.receipt.json"));
            if let Some(owned) = owned
                && !self.references_owned(&owned)
            {
                self.stale.push(owned);
            }
        }
        let directory = &self.directory;
        self.stale.retain(|name| remove_superseded(directory, name).is_err());
        let excess = self.stale.len().saturating_sub(1024);
        if excess > 0 {
            self.stale = self.stale.split_off(excess);
        }
    }
    fn references_owned(&self, name: &str) -> bool {
        self.owned.as_ref().is_some_and(|store| store.name == name)
            || self.roots.iter().any(|(_, owned)| owned.as_deref() == Some(name))
    }
    /// A group commit published `root` for this journal; it is never pruned, because
    /// restoring any member of that group verifies the roots of all its members.
    fn group_root_committed(&mut self, root: &RootReceipt) {
        self.group_roots.insert(root.revision);
        self.remember_root(root);
    }
}

/// Outcome of one durable append: receipt, maintenance error, root and owned store.
type Appended = (DurableReceipt, Option<String>, RootReceipt, Option<Box<OwnedStore>>);

/// Remove one superseded root file. Only plain files are removed; a missing file is done.
fn remove_superseded(directory: &Path, name: &str) -> std::io::Result<()> {
    let path = directory.join(name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => std::fs::remove_file(path),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
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
/// One recipe piece, borrowed from the snapshot the recipe describes.
enum RecipePiece<'a> {
    /// Text range of the journal's primary source.
    Original(std::ops::Range<u64>),
    /// Text range of a retained foreign source.
    Foreign(u64, std::ops::Range<u64>),
    /// Immutable in-memory text; its address identifies it while retained.
    Text(&'a str),
    /// Range of an immutable owned source.
    Source(&'a bareline_document::source::MemorySource, std::ops::Range<u64>),
}
/// Where a root stores its owned text.
enum OwnedMode {
    /// A complete file per revision (`root-owned-<revision>.bin`, receipt version 2),
    /// as group roots require: their commit markers pin that name and length.
    PerRevision,
    /// The journal's append-only store (receipt version 3); `None` starts a new one.
    Append(Option<Box<OwnedStore>>),
}
/// Keeps alive the snapshot whose text addresses key an `OwnedStore` index.
enum Retained {
    Paged {
        _snapshot: bareline_document::paged::PagedSnapshot,
    },
    Resident {
        _snapshot: bareline_document::DocumentSnapshot,
    },
}
/// Append-only owned text shared by a journal's successive roots (REC-09). Pieces are
/// keyed by identity: the address of immutable in-memory text (identity 0), or an
/// owned source's shared state plus a range. `_retained` holds the snapshot every key
/// was taken from, so no keyed allocation can be freed and reused by other bytes
/// while its key is known; a hit therefore always names identical bytes. Each root
/// appends only text no earlier root stored and seals the first `len` bytes.
struct OwnedStore {
    name: String,
    /// Sealed length: every published root references a prefix of this many bytes.
    len: u64,
    /// SHA-256 state over the first `len` bytes.
    hash: sha2::Sha256,
    /// (identity, start) -> (end, owned offset).
    index: std::collections::BTreeMap<(usize, u64), (u64, u64)>,
    _retained: Option<Retained>,
}
impl OwnedStore {
    fn new(revision: u64) -> Self {
        use sha2::Digest;
        Self {
            name: format!("root-owned-{revision}.bin"),
            len: 0,
            hash: sha2::Sha256::new(),
            index: Default::default(),
            _retained: None,
        }
    }
    /// The stored entry covering `start..end` of `identity`, if an earlier root stored it.
    fn find(&self, identity: usize, start: u64, end: u64) -> Option<((usize, u64), (u64, u64))> {
        let (&key, &value) = self.index.range(..=(identity, start)).next_back()?;
        (key.0 == identity && end <= value.0).then_some((key, value))
    }
}
/// Storage policy shared by everything one append writes.
struct RecipeContext<'a> {
    platform: &'a dyn LocalFileSystem,
    cancel: &'a Cancellation,
    quota: u64,
    usage: &'a mut crate::recovery::UsageLedger,
}
fn prepare_root(
    directory: &Path,
    snapshot: &bareline_document::paged::PagedSnapshot,
    sources: Option<&DiskDecoded>,
    mut context: RecipeContext<'_>,
    mode: OwnedMode,
) -> std::io::Result<(RootReceipt, Option<Box<OwnedStore>>)> {
    let mut foreign = std::collections::BTreeMap::new();
    if let Some(sources) = sources {
        for (generation, store) in sources
            .foreign_sources()
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?
        {
            let name = format!("foreign-{generation}");
            let target = directory.join(&name);
            if !target.exists() {
                context.usage.admit(
                    directory,
                    context.quota,
                    store
                        .retained_size()
                        .map_err(|e| std::io::Error::other(format!("{e:?}")))?,
                    context.platform,
                    context.cancel,
                )?;
                store
                    .retain_recovery(&target, context.cancel)
                    .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
            }
            foreign.insert(generation, name);
        }
    }
    let generations: std::collections::BTreeSet<u64> = foreign.keys().copied().collect();
    let pieces = snapshot.pieces().map(move |piece| paged_piece(&generations, piece));
    let (receipt, store) = prepare_recipe(
        directory,
        snapshot.revision.0,
        snapshot.metadata(),
        foreign,
        pieces,
        context,
        mode,
    )?;
    let store = store.map(|mut store| {
        store._retained = Some(Retained::Paged {
            _snapshot: snapshot.clone(),
        });
        store
    });
    Ok((receipt, store))
}
fn paged_piece<'a>(
    foreign: &std::collections::BTreeSet<u64>,
    piece: bareline_document::paged::PagedPiece<'a>,
) -> RecipePiece<'a> {
    use bareline_document::paged::PagedPiece;
    match piece {
        PagedPiece::Original { source, range } | PagedPiece::OriginalOwned { source, range, .. } => {
            original_piece(foreign, source.generation().0, range)
        }
        PagedPiece::Inserted(text) => RecipePiece::Text(text),
        PagedPiece::OwnedSource {
            original: Some((source, range)),
            ..
        } => original_piece(foreign, source.generation().0, range),
        PagedPiece::OwnedSource {
            source,
            range,
            original: None,
        } => RecipePiece::Source(source, range),
    }
}
fn original_piece<'a>(
    foreign: &std::collections::BTreeSet<u64>,
    generation: u64,
    range: std::ops::Range<u64>,
) -> RecipePiece<'a> {
    if foreign.contains(&generation) {
        RecipePiece::Foreign(generation, range)
    } else {
        RecipePiece::Original(range)
    }
}
/// Write the recipe for `pieces` and publish its receipt. Order per append: owned
/// bytes written, fsynced and re-read under a sealed handle; recipe written and
/// fsynced; receipt published atomically. The caller journals the revision only
/// after this returns, and prunes superseded roots only after that.
fn prepare_recipe<'a>(
    directory: &Path,
    revision: u64,
    metadata: &bareline_document::DocumentMetadata,
    foreign: std::collections::BTreeMap<u64, String>,
    pieces: impl Iterator<Item = RecipePiece<'a>>,
    context: RecipeContext<'_>,
    mode: OwnedMode,
) -> std::io::Result<(RootReceipt, Option<Box<OwnedStore>>)> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Seek, SeekFrom, Write};
    let RecipeContext {
        platform,
        cancel,
        quota,
        usage,
    } = context;
    let interrupted = || std::io::Error::new(std::io::ErrorKind::Interrupted, "Recovery cancelled");
    let receipt_bound = RootReceipt {
        version: 3,
        revision,
        file: format!("root-{revision}.json"),
        sha256: [255; 32],
        metadata: metadata.values().clone(),
        owned: Some(RootOwned {
            name: format!("root-owned-{revision}.bin"),
            len: u64::MAX,
            sha256: [255; 32],
        }),
        foreign: foreign.clone(),
    };
    let receipt_bytes = serde_json::to_vec(&receipt_bound).map_err(std::io::Error::other)?.len() as u64;
    // Historical receipt plus the future atomic latest-pointer staging file.
    let remaining_quota = usage.admit(
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
        revision,
        preserve: false,
        owned: false,
        json: false,
        receipt: false,
    };
    let (append, previous) = match mode {
        OwnedMode::PerRevision => (false, None),
        OwnedMode::Append(previous) => (true, previous),
    };
    // Continue the journal's store only while its file holds exactly the sealed
    // prefix. Anything else (an earlier failed append, a sealed reader holding the
    // file, an unexpected entry) starts a new complete store instead.
    let continued = previous.and_then(|store| {
        let path = directory.join(&store.name);
        let plain = std::fs::symlink_metadata(&path).is_ok_and(|metadata| {
            metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() == store.len
        });
        if !plain {
            return None;
        }
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).ok()?;
        (file.seek(SeekFrom::End(0)).ok()? == store.len).then_some((store, file))
    });
    let (mut store, owned_file) = match continued {
        Some(continued) => continued,
        None => {
            let store = Box::new(OwnedStore::new(revision));
            let file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(directory.join(&store.name))?;
            cleanup.owned = true;
            (store, file)
        }
    };
    let created = cleanup.owned;
    let base_len = store.len;
    let mut owned = RecipeQuotaFile {
        file: owned_file,
        remaining: remaining.clone(),
        limit: remaining_quota,
        written: 0,
        directory,
        platform,
        cancel,
    };
    let mut appended_hash = Sha256::new();
    let mut next_hash = store.hash.clone();
    let mut next_len = base_len;
    let mut next_index = std::collections::BTreeMap::new();
    let name = format!("root-{revision}.json");
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
    {
        let mut store_owned = |text: &str| -> std::io::Result<std::ops::Range<u64>> {
            cancel.check().map_err(|_| interrupted())?;
            let start = next_len;
            if text.len() as u64 > remaining.get().min(platform.available_space(directory)? / 5) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::StorageFull,
                    "Recovery owned quota",
                ));
            }
            for chunk in text.as_bytes().chunks(65536) {
                cancel.check().map_err(|_| interrupted())?;
                owned.write_all(chunk)?;
                appended_hash.update(chunk);
                next_hash.update(chunk);
                next_len += chunk.len() as u64;
            }
            Ok(start..next_len)
        };
        let mut first = true;
        let mut count = 0usize;
        let mut emit = |piece: RootPiece| -> std::io::Result<()> {
            cancel.check().map_err(|_| interrupted())?;
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
        for piece in pieces {
            match piece {
                RecipePiece::Original(range) => emit(RootPiece::Original {
                    start: range.start,
                    end: range.end,
                })?,
                RecipePiece::Foreign(generation, range) => emit(RootPiece::Foreign {
                    generation,
                    start: range.start,
                    end: range.end,
                })?,
                RecipePiece::Text(text) => {
                    if text.is_empty() {
                        continue;
                    }
                    let start = text.as_ptr() as usize as u64;
                    let end = start + text.len() as u64;
                    let stored = match store.find(0, start, end).filter(|_| append) {
                        Some((key, value)) => {
                            next_index.insert(key, value);
                            value.1 + (start - key.1)..value.1 + (end - key.1)
                        }
                        None => {
                            let stored = store_owned(text)?;
                            if append {
                                next_index.insert((0, start), (end, stored.start));
                            }
                            stored
                        }
                    };
                    emit(RootPiece::Owned {
                        start: stored.start,
                        end: stored.end,
                    })?;
                }
                RecipePiece::Source(source, range) => {
                    if range.is_empty() {
                        continue;
                    }
                    let identity = source.identity();
                    let stored = match store.find(identity, range.start, range.end).filter(|_| append) {
                        Some((key, value)) => {
                            next_index.insert(key, value);
                            value.1 + (range.start - key.1)..value.1 + (range.end - key.1)
                        }
                        None => {
                            let mut stored: Option<std::ops::Range<u64>> = None;
                            crate::owned_read::visit_utf8::<std::io::Error>(source, range.clone(), cancel, |text| {
                                let next = store_owned(text)?;
                                if let Some(previous) = stored.as_mut() {
                                    previous.end = next.end;
                                } else {
                                    stored = Some(next);
                                }
                                Ok(())
                            })?;
                            let stored = stored
                                .filter(|stored| stored.end - stored.start == range.end - range.start)
                                .ok_or_else(|| std::io::Error::other("Recovery owned source length changed"))?;
                            if append {
                                next_index.insert((identity, range.start), (range.end, stored.start));
                            }
                            stored
                        }
                    };
                    emit(RootPiece::Owned {
                        start: stored.start,
                        end: stored.end,
                    })?;
                }
            }
        }
    }
    // An unchanged store has nothing new to make durable.
    if created || next_len > base_len {
        owned.sync_all()?;
    }
    drop(owned);
    // Re-read only the appended bytes under a sealed handle; earlier roots sealed
    // the prefix, and restore rehashes the whole prefix it uses.
    let mut sealed = platform.open_sealed_read(&directory.join(&store.name))?;
    if sealed.metadata()?.len() != next_len {
        return Err(std::io::Error::other("Recovery owned bytes changed before seal"));
    }
    sealed.seek(SeekFrom::Start(base_len))?;
    let mut sealed_hash = Sha256::new();
    let mut sealed_buffer = vec![0u8; 65536];
    let mut unread = next_len - base_len;
    while unread != 0 {
        cancel.check().map_err(|_| interrupted())?;
        let count = unread.min(65536) as usize;
        sealed.read_exact(&mut sealed_buffer[..count])?;
        sealed_hash.update(&sealed_buffer[..count]);
        unread -= count as u64;
    }
    drop(sealed);
    if sealed_hash.finalize() != appended_hash.finalize() {
        return Err(std::io::Error::other("Recovery owned bytes changed before seal"));
    }
    file.write_all(b"]")?;
    if file.metadata()?.len() > 128 * 1024 * 1024 {
        return Err(std::io::Error::other("Recovery recipe size limit"));
    }
    file.sync_all()?;
    drop(file);
    // Owned and recipe bytes were checked against the admitted quota as they streamed.
    usage.charge(remaining_quota - remaining.get());
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
        version: if append { 3 } else { 2 },
        foreign,
        revision,
        file: name,
        sha256: hash.finalize().into(),
        metadata: metadata.values().clone(),
        owned: Some(RootOwned {
            name: store.name.clone(),
            len: next_len,
            sha256: next_hash.clone().finalize().into(),
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
    if !append {
        return Ok((receipt, None));
    }
    store.len = next_len;
    store.hash = next_hash;
    store.index = next_index;
    Ok((receipt, Some(store)))
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
    // Version 3 roots share the journal's append-only owned store (REC-09).
    if !matches!(root.version, 1..=3) || root.file != format!("root-{}.json", root.revision) {
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
            (matches!(receipt.version, 1..=3)
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
        let revision = snapshot.revision.0;
        let owned = self.owned_mode();
        let result = (|| -> Result<Appended, String> {
            let mut writer = self.writer.lock().map_err(|_| "Recovery writer stopped".to_owned())?;
            writer.prepare_recipe_revision(revision).map_err(|e| e.to_string())?;
            let (root, store) = prepare_root(
                &self.directory,
                snapshot,
                Some(&self.store),
                RecipeContext {
                    platform: self.platform.as_ref(),
                    cancel: &self.cancellation,
                    quota,
                    usage: &mut writer.usage,
                },
                owned,
            )
            .map_err(|e| e.to_string())?;
            let journal_quota = quota
                .checked_sub(serde_json::to_vec(&root).map_err(|e| e.to_string())?.len() as u64)
                .ok_or("Recovery pointer quota")?;
            let receipt = writer
                .append_source_transaction(
                    revision,
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
            Ok((receipt, maintenance.err().map(|e| e.to_string()), root, store))
        })();
        self.settle_append(revision, result)
    }
}
impl PagedRecovery {
    pub fn append_source_history(
        &mut self,
        snapshot: &bareline_document::paged::PagedSnapshot,
        edits: &[bareline_document::paged::HistorySourceEdit],
        quota: u64,
    ) -> Result<(), String> {
        let revision = snapshot.revision.0;
        let owned = self.owned_mode();
        let result = (|| -> Result<Appended, String> {
            let mut writer = self.writer.lock().map_err(|_| "Recovery writer stopped".to_owned())?;
            writer.prepare_recipe_revision(revision).map_err(|e| e.to_string())?;
            let (root, store) = prepare_root(
                &self.directory,
                snapshot,
                Some(&self.store),
                RecipeContext {
                    platform: self.platform.as_ref(),
                    cancel: &self.cancellation,
                    quota,
                    usage: &mut writer.usage,
                },
                owned,
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
            let receipt = writer
                .append_streams(
                    revision,
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
            Ok((receipt, maintenance.err().map(|e| e.to_string()), root, store))
        })();
        self.settle_append(revision, result)
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
    fn prepare(path: &Path, snapshot: &bareline_document::paged::PagedSnapshot, quota: u64) -> io::Result<RootReceipt> {
        let cancel = Cancellation::default();
        let mut usage = crate::recovery::UsageLedger::default();
        let context = RecipeContext {
            platform: &Platform,
            cancel: &cancel,
            quota,
            usage: &mut usage,
        };
        prepare_root(path, snapshot, None, context, OwnedMode::Append(None)).map(|(root, _)| root)
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
        let root = prepare(&path, &snapshot, 8192).unwrap();
        assert_eq!(root.owned.as_ref().unwrap().len, 0);
        let physical = crate::recovery::disk_usage(&path, &cancel).unwrap();
        assert!(physical > 2);
        fs::remove_file(path.join(root.file)).unwrap();
        fs::remove_file(path.join("root-0.receipt.json")).unwrap();
        fs::remove_file(path.join("root-owned-0.bin")).unwrap();
        assert!(prepare(&path, &snapshot, physical - 1).is_err());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        fs::write(path.join("retained.bin"), b"existing").unwrap();
        assert!(prepare(&path, &snapshot, 0).is_err());
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
    /// `fail_commit` fails the atomic publication of files whose path ends with it.
    struct Platform {
        fail_recipe: AtomicBool,
        fail_baseline: AtomicBool,
        fail_commit: std::sync::Mutex<Option<&'static str>>,
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
            if self
                .fail_commit
                .lock()
                .unwrap()
                .is_some_and(|suffix| target.to_string_lossy().ends_with(suffix))
            {
                return Err(io::Error::other("injected publication failure"));
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
        let platform = Arc::new(Platform {
            fail_recipe: AtomicBool::new(false),
            fail_baseline: AtomicBool::new(fail_baseline),
            fail_commit: std::sync::Mutex::new(None),
        });
        let (document, recovery) = open_journal(&root, &platform, "source.txt", fail_baseline);
        Fixture {
            root,
            platform,
            document: Some(document),
            recovery: Some(recovery),
        }
    }
    /// Open `name` under `root` as a paged document with a journal in `root/recovery`
    /// whose baseline copy has settled: complete, or failed when `fail_baseline` is set.
    fn open_journal(
        root: &Path,
        platform: &Arc<Platform>,
        name: &str,
        fail_baseline: bool,
    ) -> (PagedDocument, PagedRecovery) {
        let source = root.join(name);
        fs::write(&source, b"alpha\n").unwrap();
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path: source.clone(),
                bytes: Budget::new(4 * 1024 * 1024),
                history: Budget::new(1024 * 1024),
                cache: root.to_path_buf(),
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
        (transcoded.document, recovery)
    }
    fn revise(document: &mut PagedDocument, value: &str) -> bareline_document::paged::PagedSnapshot {
        let base = document.snapshot().revision;
        let metadata = DocumentMetadata::new([("test.revision".to_owned(), value.to_owned())].into()).unwrap();
        document.apply_metadata(base, metadata).unwrap();
        document.snapshot()
    }
    /// The restored revision, its marker, and the acknowledged revision it could not restore.
    fn restored_revision(fixture: &Fixture, directory: &Path) -> (u64, Option<String>, Option<u64>) {
        // Restore charges worst-case recipe scratch (65,536 pieces, about 6 MiB) up
        // front, as the other restore tests budget for.
        let restored = restore(
            directory,
            fixture.platform.clone(),
            Budget::new(64 * 1024 * 1024),
            Budget::new(16 * 1024 * 1024),
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
        for name in [format!("root-{revision}.receipt.json"), format!("root-{revision}.json")] {
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
    const PASTE: usize = 256 * 1024;
    /// An immutable owned store holding a large paste, as a paged paste produces.
    fn pasted(root: &Path, platform: &Arc<Platform>) -> bareline_document::source::MemorySource {
        use std::io::Write;
        let mut stage = crate::owned_store::StreamingStoreBuilder::new(
            &root.join("paste"),
            64 * 1024 * 1024,
            platform.clone(),
            SourceOptions {
                resident_max_bytes: 0,
                page_size_bytes: 4096,
                page_cache_bytes: 65536,
            },
            Budget::new(4 * 1024 * 1024),
            Cancellation::default(),
        )
        .unwrap();
        let chunk = [b'p'; 4096];
        for _ in 0..PASTE / 4096 {
            stage.write_all(&chunk).unwrap();
        }
        stage.finish().unwrap()
    }
    /// The paste split around `typed` characters typed into its middle.
    fn typed_into(
        paste: &bareline_document::source::MemorySource,
        typed: usize,
    ) -> bareline_document::paged::PagedSnapshot {
        use bareline_document::paged::RestoredPiece;
        let half = paste.len() / 2;
        let owned = |range: std::ops::Range<u64>| RestoredPiece::OwnedSource {
            source: paste.clone(),
            range,
            original: None,
        };
        PagedDocument::restore_pieces(
            paste.clone(),
            vec![
                owned(0..half),
                RestoredPiece::Inserted("t".repeat(typed)),
                owned(half..paste.len()),
            ],
            Budget::new(4 * 1024 * 1024),
            Budget::new(0),
            bareline_document::Revision(typed as u64),
        )
        .unwrap()
        .snapshot()
    }
    fn typed_text(typed: usize) -> String {
        format!(
            "{}{}{}",
            "p".repeat(PASTE / 2),
            "t".repeat(typed),
            "p".repeat(PASTE / 2)
        )
    }
    fn restored(platform: &Arc<Platform>, directory: &Path) -> (u64, String) {
        let bytes = Budget::new(64 * 1024 * 1024);
        let mut restored = restore(
            directory,
            platform.clone(),
            bytes.clone(),
            Budget::new(16 * 1024 * 1024),
            &Cancellation::default(),
        )
        .unwrap();
        let text = restore_text(&mut restored, 1 << 20, &bytes, &Cancellation::default())
            .unwrap()
            .expect("restored text");
        (restored.transcoded.document.snapshot().revision.0, text)
    }
    fn names(directory: &Path, matches: impl Fn(&str) -> bool) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| matches(name.as_str()))
            .collect();
        names.sort();
        names
    }
    #[test]
    fn paste_then_typing_appends_only_new_owned_text_and_prunes_superseded_roots() {
        let mut fixture = fixture("owned-delta");
        let paste = pasted(&fixture.root, &fixture.platform);
        let recovery = fixture.recovery.as_mut().unwrap();
        for typed in 1..=20 {
            recovery.append(&typed_into(&paste, typed), &[]).unwrap();
        }
        let directory = recovery.directory().to_path_buf();
        // One append-only store holds the paste once plus each typed run (REC-09); the
        // replaced layout rewrote the whole paste into a new file for every append.
        let stores = names(&directory, |name| name.starts_with("root-owned-"));
        assert_eq!(stores.len(), 1, "{stores:?}");
        let stored = fs::metadata(directory.join(&stores[0])).unwrap().len();
        assert!(stored >= PASTE as u64);
        assert!(stored <= PASTE as u64 + 210, "{stored} owned bytes for one paste");
        // Superseded roots are pruned once a newer one is durable.
        assert_eq!(names(&directory, |name| name.ends_with(".receipt.json")).len(), 2);
        assert_eq!(
            names(&directory, |name| name.starts_with("root-") && name.ends_with(".json")).len(),
            4
        );
        // Disk usage is tracked incrementally instead of walking the journal per append.
        assert!(recovery.writer.lock().unwrap().usage.walks <= 2);
        assert_eq!(restored(&fixture.platform, &directory), (20, typed_text(20)));
    }
    #[test]
    fn interrupted_append_steps_keep_the_last_acknowledged_root_restorable() {
        // Each step of an append in its write-then-fsync-then-publish order: the owned
        // bytes' sealed re-read, the receipt publication, and the latest-root pointer
        // after the journal already names the revision.
        for (label, suffix, seal, durable) in [
            ("seal", None, true, false),
            ("receipt", Some(".receipt.json"), false, false),
            ("pointer", Some("paged-root.json"), false, true),
        ] {
            let mut fixture = fixture(label);
            let paste = pasted(&fixture.root, &fixture.platform);
            let recovery = fixture.recovery.as_mut().unwrap();
            recovery.append(&typed_into(&paste, 1), &[]).unwrap();
            recovery.append(&typed_into(&paste, 2), &[]).unwrap();
            let directory = recovery.directory().to_path_buf();
            fixture.platform.fail_recipe.store(seal, Ordering::SeqCst);
            *fixture.platform.fail_commit.lock().unwrap() = suffix;
            let result = recovery.append(&typed_into(&paste, 3), &[]);
            fixture.platform.fail_recipe.store(false, Ordering::SeqCst);
            *fixture.platform.fail_commit.lock().unwrap() = None;
            assert_eq!(result.is_ok(), durable, "{label}");
            // An interrupted store append leaves bytes past the sealed prefix; restore
            // hashes and exposes only the prefix its root sealed.
            let acknowledged = if durable { 3 } else { 2 };
            assert_eq!(
                restored(&fixture.platform, &directory),
                (acknowledged as u64, typed_text(acknowledged)),
                "{label}"
            );
            recovery.append(&typed_into(&paste, 4), &[]).unwrap();
            assert_eq!(restored(&fixture.platform, &directory), (4, typed_text(4)), "{label}");
        }
    }
    #[test]
    fn journals_with_per_revision_roots_still_restore() {
        let mut fixture = fixture("legacy-roots");
        let paste = pasted(&fixture.root, &fixture.platform);
        let recovery = fixture.recovery.as_mut().unwrap();
        // The layout every journal had before the append-only store: a complete owned
        // file per root and receipt version 2.
        recovery.per_revision_roots = true;
        for typed in 1..=3 {
            recovery.append(&typed_into(&paste, typed), &[]).unwrap();
        }
        let directory = recovery.directory().to_path_buf();
        let receipt = |revision: u64| -> RootReceipt {
            serde_json::from_slice(&fs::read(directory.join(format!("root-{revision}.receipt.json"))).unwrap()).unwrap()
        };
        let legacy = receipt(3);
        assert_eq!(legacy.version, 2);
        let owned = legacy.owned.unwrap();
        assert_eq!(owned.name, "root-owned-3.bin");
        assert_eq!(fs::metadata(directory.join(&owned.name)).unwrap().len(), owned.len);
        let inspection = crate::recovery::inspect(&directory, &Cancellation::default()).unwrap();
        assert_eq!(inspection.status, crate::recovery::RecoveryStatus::Complete);
        assert_eq!(inspection.last_durable.map(|receipt| receipt.revision), Some(3));
        assert_eq!(restored(&fixture.platform, &directory), (3, typed_text(3)));
        // Continuing such a journal switches to the append-only store.
        recovery.per_revision_roots = false;
        recovery.append(&typed_into(&paste, 4), &[]).unwrap();
        assert_eq!(receipt(4).version, 3);
        assert_eq!(restored(&fixture.platform, &directory), (4, typed_text(4)));
    }
    /// Commit one transfer group over two journals, each at its next metadata revision.
    fn commit_pair(
        first: &mut PagedRecovery,
        second: &mut PagedRecovery,
        snapshots: [bareline_document::paged::PagedSnapshot; 2],
        id: u64,
    ) {
        let edits = [group::GroupEdits::History(&[]), group::GroupEdits::History(&[])];
        group::commit(
            &mut [first, second],
            &snapshots,
            &edits,
            id,
            20 * 1024 * 1024 * 1024,
            &Cancellation::default(),
        )
        .unwrap();
    }
    #[test]
    fn newer_groups_never_prune_a_root_another_members_group_marker_names() {
        let fixture = fixture("group-roots");
        let platform = fixture.platform.clone();
        let (mut a_document, mut a) = open_journal(&fixture.root, &platform, "a.txt", false);
        let (mut b_document, mut b) = open_journal(&fixture.root, &platform, "b.txt", false);
        let (mut c_document, mut c) = open_journal(&fixture.root, &platform, "c.txt", false);
        // Group 1 commits A and B; B's group pointer keeps naming it from then on.
        let a_first = revise(&mut a_document, "a1");
        let b_first = revise(&mut b_document, "b1");
        let (a1, b1) = (a_first.revision.0, b_first.revision.0);
        commit_pair(&mut a, &mut b, [a_first, b_first], 1);
        // A then joins two newer groups with C and keeps typing on its own.
        for id in [2, 3] {
            let a_next = revise(&mut a_document, &format!("a{id}"));
            let c_next = revise(&mut c_document, &format!("c{id}"));
            commit_pair(&mut a, &mut c, [a_next, c_next], id);
        }
        let mut ordinary = Vec::new();
        for value in ["a4", "a5", "a6"] {
            let next = revise(&mut a_document, value);
            ordinary.push(next.revision.0);
            a.append(&next, &[]).unwrap();
        }
        let a_directory = a.directory().to_path_buf();
        // Restoring B verifies every member root of group 1, so A keeps its group-1
        // root (REC-09); A's superseded ordinary roots are still pruned.
        for name in [format!("root-{a1}.json"), format!("root-{a1}.receipt.json")] {
            assert!(a_directory.join(&name).is_file(), "{name}");
        }
        assert!(!a_directory.join(format!("root-{}.json", ordinary[0])).exists());
        assert!(a_directory.join(format!("root-{}.json", ordinary[2])).is_file());
        let restore_journal = |directory: &Path| {
            let restored = restore(
                directory,
                platform.clone(),
                Budget::new(64 * 1024 * 1024),
                Budget::new(16 * 1024 * 1024),
                &Cancellation::default(),
            )
            .unwrap();
            let snapshot = restored.transcoded.document.snapshot();
            (
                snapshot.revision.0,
                snapshot.metadata().get("test.revision").map(str::to_owned),
            )
        };
        assert_eq!(restore_journal(b.directory()), (b1, Some("b1".to_owned())));
        assert_eq!(restore_journal(&a_directory), (ordinary[2], Some("a6".to_owned())));
        drop((a, b, c));
        drop((a_document, b_document, c_document));
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
