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
pub struct PagedRecovery {
    writer: Arc<Mutex<RecoveryWriter>>,
    pub status: Arc<Mutex<PagedRecoveryStatus>>,
    directory: PathBuf,
    store: DiskDecoded,
    baseline: bareline_document::paged::PagedSnapshot,
    platform: Arc<dyn LocalFileSystem>,
    notify: Arc<dyn Fn() + Send + Sync>,
    attempt: u64,
    cancellation: Cancellation,
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
    pub fn create_in(
        directory: PathBuf,
        store: DiskDecoded,
        original_path: Option<PathBuf>,
        baseline: bareline_document::paged::PagedSnapshot,
        platform: Arc<dyn LocalFileSystem>,
        status: Arc<Mutex<PagedRecoveryStatus>>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
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
            directory,
            store,
            baseline,
            platform,
            notify,
            attempt: 0,
            cancellation: Cancellation::default(),
        };
        if let Err(error) = recovery.prepare_baseline() {
            recovery.status.lock().map_err(|_| "Recovery state stopped")?.error = Some(error);
        }
        Ok(recovery)
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn platform(&self) -> Arc<dyn LocalFileSystem> {
        self.platform.clone()
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
        let cancel = self.cancellation.clone();
        let baseline = self.baseline.clone();
        baseline_worker()
            .try_send(Box::new(move || {
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
                        Err(error) if cancel.check().is_ok() => state.error = Some(error),
                        Err(_) => {}
                    }
                }
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
        let result = (|| {
            let mut writer = self.writer.lock().map_err(|_| "Recovery writer stopped".to_owned())?;
            let receipt = if edits.is_empty() {
                writer.append_metadata(revision, snapshot.metadata())
            } else {
                writer.append(revision, edits)
            }
            .map_err(|e| e.to_string())?;
            write_root(
                &self.directory,
                snapshot,
                self.platform.as_ref(),
                &self.cancellation,
                &self.store,
            )
            .map_err(|e| e.to_string())?;
            writer.checkpoint(self.platform.as_ref()).map_err(|e| e.to_string())?;
            Ok::<_, String>(receipt)
        })();
        let mut status = self.status.lock().map_err(|_| "Recovery state stopped")?;
        match result {
            Ok(receipt) => {
                status.durable = Some(receipt);
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

/// Startup sweep: delete journal directories whose owning process is gone and that no
/// live session still references. The `keep_newest` most recently modified abandoned
/// directories are retained as a safety net so a user can still recover by hand.
pub fn sweep(
    root: &Path,
    referenced: &std::collections::HashSet<PathBuf>,
    alive: &dyn Fn(u32) -> bool,
    keep_newest: usize,
    platform: &dyn LocalFileSystem,
) -> std::io::Result<Vec<PathBuf>> {
    let mut abandoned: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut removed = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(owner) = directory_owner(name) else {
            continue;
        };
        let directory = entry.path();
        if alive(owner) || referenced.contains(&directory) {
            continue;
        }
        if crate::recovery::inspect(&directory, &Cancellation::default())
            .is_ok_and(|inspection| inspection.status == crate::recovery::RecoveryStatus::Discarded)
        {
            if purge_directory(&directory, platform).is_ok() {
                removed.push(directory);
            }
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        abandoned.push((modified, directory));
    }
    abandoned.sort_by(|left, right| right.0.cmp(&left.0));
    for (_, directory) in abandoned.into_iter().skip(keep_newest) {
        if purge_directory(&directory, platform).is_ok() {
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
fn write_root(
    directory: &Path,
    snapshot: &bareline_document::paged::PagedSnapshot,
    platform: &dyn LocalFileSystem,
    cancel: &Cancellation,
    store: &DiskDecoded,
) -> std::io::Result<()> {
    let receipt = prepare_root(
        directory,
        snapshot,
        platform,
        cancel,
        20 * 1024 * 1024 * 1024,
        Some(store),
    )?;
    publish_root(directory, &receipt, platform)
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
    if inspection.status == crate::recovery::RecoveryStatus::CorruptTail
        && let Some(validated) = inspection.last_durable
        && validated.revision < root.revision
        && group_revision.is_none_or(|revision| revision < root.revision)
    {
        root = serde_json::from_slice(&read_small(&format!("root-{}.receipt.json", validated.revision))?)
            .map_err(|e| e.to_string())?;
        if !matches!(root.version, 1 | 2)
            || root.revision != validated.revision
            || root.file != format!("root-{}.json", validated.revision)
        {
            return Err("Invalid validated-prefix recovery root".into());
        }
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
        let candidate: RootReceipt =
            serde_json::from_slice(&read_small(&format!("root-{}.receipt.json", durable.revision))?)
                .map_err(|e| e.to_string())?;
        if !matches!(candidate.version, 1 | 2)
            || candidate.revision != durable.revision
            || candidate.file != format!("root-{}.json", durable.revision)
        {
            return Err("Invalid durable recovery root".into());
        }
        root = candidate;
    }
    if !inspection.complete_baseline
        || inspection.last_durable.is_none_or(|r| r.revision != root.revision) && group_revision != Some(root.revision)
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
                status.durable = Some(receipt);
                status.error = maintenance.err().map(|e| e.to_string());
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
                status.durable = Some(receipt);
                status.error = maintenance.err().map(|e| e.to_string());
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
    #[test]
    fn sweep_removes_abandoned_journals_and_keeps_live_and_referenced_ones() {
        let root = std::env::temp_dir().join(format!(
            "bareline-sweep-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let dead = root.join("paged-424242-1-1");
        let referenced = root.join("paged-424243-1-1");
        let live = root.join(format!("paged-{}-1-1", std::process::id()));
        let other = root.join("not-a-journal");
        for directory in [&dead, &referenced, &live] {
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
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("manifest.json"), b"{}").unwrap();
        let references: std::collections::HashSet<PathBuf> = [referenced.clone()].into_iter().collect();
        let removed = sweep(&root, &references, &|id| id == std::process::id(), 0, &Platform).unwrap();
        assert_eq!(removed, vec![dead.clone()]);
        assert!(!dead.exists());
        assert!(referenced.exists() && live.exists() && other.exists());
        // The newest abandoned journals are retained as a safety net.
        drop(
            crate::recovery::RecoveryWriter::create(
                &dead,
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
        assert!(
            sweep(&root, &Default::default(), &|_| false, 20, &Platform)
                .unwrap()
                .is_empty()
        );
        assert!(dead.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
