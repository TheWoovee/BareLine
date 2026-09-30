// SPDX-License-Identifier: MPL-2.0
//! Complete Resident/untitled snapshot checkpoints with original-codec provenance.
//! Captures are coalesced while a checkpoint is preparing; all disk work is off UI.
use crate::{
    cancellation::Cancellation,
    codecs::{
        disk::{DiskOptions, DiskTranscoder},
        resident::ResidentEncoding,
    },
    lifecycle::{FileError, FileInput},
    paged_recovery::{PagedRecovery, PagedRecoveryStatus},
};
use bareline_document::{Budget, DocumentSnapshot, TextOffset};
use bareline_platform::LocalFileSystem;
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
};
type Job = Box<dyn FnOnce() + Send>;
fn worker() -> &'static SyncSender<Job> {
    static WORKER: OnceLock<SyncSender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(16);
        // Thread creation can fail under handle/address-space exhaustion. A failed
        // spawn drops `rx` with the closure, so the caller's `try_send` fails and
        // surfaces "recovery queue full; retry" instead of aborting the process.
        let spawned = std::thread::Builder::new()
            .name("resident-recovery".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job();
                }
            });
        debug_assert!(spawned.is_ok(), "resident recovery worker");
        tx
    })
}
/// Incremental checkpoints appended to one journal before the next full copy (REC-10).
const MAX_INCREMENTAL: usize = 256;
/// Outcome a checkpoint job reports back to `observe`.
enum Checkpoint {
    /// A new journal holding a full copy; replaces `current` when it succeeded.
    Full(Result<PagedRecovery, String>),
    /// `current` handed back after appending one root, with that append's outcome.
    Incremental(PagedRecovery, Result<(), String>),
}
fn next_slot() -> String {
    static SLOT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "paged-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SLOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
pub struct ResidentRecovery {
    root: PathBuf,
    /// Stable directory stem for this document. Checkpoints alternate between two
    /// generations under this stem so the document never owns more than two journals.
    slot: String,
    /// Generation of the last successful checkpoint. A failed or unqueued attempt
    /// never advances it, so `current` always owns this generation's parity slot.
    generation: u64,
    /// Generation the queued checkpoint commits when (and only when) it succeeds.
    pending_generation: u64,
    clean: bool,
    platform: Arc<dyn LocalFileSystem>,
    encoding: Option<ResidentEncoding>,
    original_path: Option<PathBuf>,
    bytes: Budget,
    notify: Arc<dyn Fn() + Send + Sync>,
    previous: Vec<PagedRecovery>,
    adopted_paths: Vec<PathBuf>,
    retire_paths: Vec<PathBuf>,
    retry_retirement: bool,
    retirement: Option<Receiver<Result<Vec<PathBuf>, String>>>,
    pending: Option<Receiver<Checkpoint>>,
    current: Option<PagedRecovery>,
    /// Incremental checkpoints appended to `current` since its full copy (REC-10).
    incremental: usize,
    /// Most incremental checkpoints before the next full copy.
    max_incremental: usize,
    /// Directory of `current` while an incremental checkpoint job holds it.
    appending: Option<PathBuf>,
    /// Journal metadata changed (the original path): the next checkpoint is a full copy.
    force_full: bool,
    status: Arc<Mutex<PagedRecoveryStatus>>,
    captured: Option<bareline_document::ContentStateId>,
    cancellation: Cancellation,
    discard: Option<crate::recovery_retirement::DiscardTicket>,
    #[cfg(test)]
    inject_io_failure: bool,
    #[cfg(test)]
    inject_queue_full: bool,
}
impl ResidentRecovery {
    pub fn new(
        root: PathBuf,
        platform: Arc<dyn LocalFileSystem>,
        encoding: Option<ResidentEncoding>,
        original_path: Option<PathBuf>,
        bytes: Budget,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            root,
            slot: next_slot(),
            generation: 0,
            pending_generation: 0,
            clean: true,
            platform,
            encoding,
            original_path,
            bytes,
            notify,
            previous: Vec::new(),
            adopted_paths: Vec::new(),
            retire_paths: Vec::new(),
            retry_retirement: true,
            retirement: None,
            pending: None,
            current: None,
            incremental: 0,
            max_incremental: MAX_INCREMENTAL,
            appending: None,
            force_full: false,
            status: Arc::new(Mutex::new(Default::default())),
            captured: None,
            cancellation: Cancellation::default(),
            discard: None,
            #[cfg(test)]
            inject_io_failure: false,
            #[cfg(test)]
            inject_queue_full: false,
        }
    }
    pub fn status(&self) -> PagedRecoveryStatus {
        self.status.lock().map(|state| state.clone()).unwrap_or_default()
    }
    pub fn set_original_path(&mut self, path: Option<PathBuf>) {
        // Journals record the original path when created; a new one needs a new journal.
        if self.original_path != path {
            self.force_full = true;
        }
        self.original_path = path;
    }
    pub fn retry(&mut self) {
        self.captured = None;
        self.retry_retirement = true;
    }
    /// True once no checkpoint is running, the last one started covers `state`
    /// (or the document needs none), and its baseline copy has landed, since a
    /// journal restores only with it. A failed checkpoint also settles: its
    /// error is already on `status` and waiting cannot make it durable.
    /// Logoff and shutdown wait on this, with a deadline, while pumping `observe`.
    pub fn settled(&self, state: bareline_document::ContentStateId, dirty: bool) -> bool {
        if self.discard.is_some() {
            return true;
        }
        let baseline = self.current.is_none()
            || match self.status.lock() {
                Ok(status) => status.complete || status.error.is_some(),
                Err(_) => true,
            };
        self.pending.is_none() && (!dirty || self.captured == Some(state)) && baseline
    }
    /// Keep an adopted journal until a replacement checkpoint is durable or the
    /// document is explicitly discarded.
    pub fn adopt_path(&mut self, path: PathBuf) {
        if !self.adopted_paths.contains(&path) {
            self.adopted_paths.push(path);
        }
    }
    pub fn resume_after_discard(&mut self) {
        self.discard = None;
        self.slot = next_slot();
        self.generation = 0;
        self.previous.clear();
        self.adopted_paths.clear();
        self.retire_paths.clear();
        self.retirement = None;
        self.pending = None;
        self.current = None;
        self.incremental = 0;
        self.appending = None;
        self.force_full = false;
        self.cancellation = Cancellation::default();
        self.captured = None;
        self.clean = false;
        self.status = Arc::new(Mutex::new(Default::default()));
    }
    pub fn observe(&mut self, snapshot: DocumentSnapshot, dirty: bool) -> bool {
        self.clean = !dirty;
        if self.discard.is_some() {
            return false;
        }
        let mut changed = false;
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.pending = None;
                    changed = true;
                    match result {
                        Ok(Checkpoint::Full(Ok(recovery))) => {
                            self.generation = self.pending_generation;
                            self.incremental = 0;
                            self.force_full = false;
                            if let Some(previous) = self.current.replace(recovery) {
                                self.previous.push(previous);
                            }
                        }
                        Ok(Checkpoint::Full(Err(error))) => {
                            if let Ok(mut status) = self.status.lock() {
                                status.error = Some(error);
                            }
                        }
                        Ok(Checkpoint::Incremental(recovery, outcome)) => {
                            // The journal stays current either way: it still holds its
                            // last durable root. A failure makes the next checkpoint full.
                            self.appending = None;
                            self.current = Some(recovery);
                            match outcome {
                                Ok(()) => self.incremental += 1,
                                Err(error) => {
                                    if let Ok(mut status) = self.status.lock() {
                                        status.error = Some(error);
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            self.appending = None;
                            if let Ok(mut status) = self.status.lock() {
                                status.error = Some("Recovery worker stopped".into());
                            }
                        }
                    }
                }
            }
        }
        if let Some(receiver) = &self.retirement {
            match receiver.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.retirement = None;
                    changed = true;
                    if let Ok(mut status) = self.status.lock() {
                        match result {
                            Ok(Ok(paths)) => {
                                self.retire_paths.retain(|path| !paths.contains(path));
                                if status
                                    .directory
                                    .as_ref()
                                    .is_some_and(|directory| paths.contains(directory))
                                {
                                    *status = Default::default();
                                }
                            }
                            Ok(Err(error)) => status.error = Some(error),
                            Err(_) => status.error = Some("Recovery retirement worker stopped".into()),
                        }
                    }
                }
            }
        }
        if self.retirement.is_none()
            && self.pending.is_none()
            && self.current.as_ref().is_some_and(|current| {
                // A checkpoint becomes retirable as soon as its successor is durable on
                // disk. Waiting for the baseline copy to also report complete never
                // happens for untitled documents, which left every journal on disk.
                let status = current.status.lock().unwrap();
                status.durable.is_some() && status.error.is_none()
            })
        {
            self.retire_paths.append(&mut self.adopted_paths);
            let mut obsolete = std::mem::take(&mut self.previous);
            if !dirty && self.captured == Some(snapshot.content_state) {
                if let Some(current) = self.current.take() {
                    obsolete.push(current);
                }
                self.captured = None;
            }
            if !obsolete.is_empty() {
                self.retire_paths
                    .extend(obsolete.iter().map(|recovery| recovery.directory().to_path_buf()));
                drop(obsolete);
                self.retry_retirement = true;
            }
        }
        // Retire by identity: the live checkpoint's directory is never purged, even
        // when a stale predecessor or adopted journal names the same slot.
        if let Some(current) = &self.current {
            self.retire_paths.retain(|path| path.as_path() != current.directory());
        }
        if let Some(appending) = &self.appending {
            self.retire_paths.retain(|path| path != appending);
        }
        if self.retirement.is_none() && self.retry_retirement && !self.retire_paths.is_empty() {
            let paths = self.retire_paths.clone();
            let platform = self.platform.clone();
            let notify = self.notify.clone();
            let (tx, rx) = mpsc::sync_channel(1);
            let job: Job = Box::new(move || {
                let result = (|| -> Result<Vec<PathBuf>, String> {
                    for path in &paths {
                        crate::paged_recovery::purge_directory(path, platform.as_ref())?;
                    }
                    Ok(paths)
                })();
                let _ = tx.send(result);
                notify();
            });
            self.retry_retirement = false;
            match worker().try_send(job) {
                Ok(()) => self.retirement = Some(rx),
                Err(_) => {
                    if let Ok(mut status) = self.status.lock() {
                        status.error =
                            Some("Recovery retirement queue full; retry retained checkpoint retirement".into());
                    }
                }
            }
        }
        if (!dirty && self.captured.is_none())
            || !snapshot.is_complete()
            || self.pending.is_some()
            // Alternating slots cannot be reused until cleanup is acknowledged.
            // Otherwise an old retirement reply can erase the new slot's status.
            || self.retirement.is_some()
            || !self.retire_paths.is_empty()
            || self.captured == Some(snapshot.content_state)
            || self.current.as_ref().is_some_and(|current| {
                let status = current.status.lock().unwrap();
                !status.complete && status.error.is_none()
            })
        {
            return changed;
        }
        let root = self.root.clone();
        let platform = self.platform.clone();
        let encoding = self.encoding.clone();
        let original_path = self.original_path.clone();
        let bytes = self.bytes.clone();
        let notify = self.notify.clone();
        let status = self.status.clone();
        let cancel = self.cancellation.clone();
        let state = snapshot.content_state;
        if self.incremental_allowed(&snapshot) {
            return self.append_incremental(snapshot, changed);
        }
        // Write into the parity slot `current` does not own, so an attempt can never
        // clear the only durable checkpoint. The generation commits only on success.
        let generation = self.generation + 1;
        let mut checkpoint_directory = self.slot_directory(generation);
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.directory() == checkpoint_directory.as_path())
        {
            checkpoint_directory = self.slot_directory(generation + 1);
        }
        // A predecessor still waiting on retirement may own the target slot when the
        // current checkpoint reported an error. `current` stays durable while the slot
        // is rewritten, so release the superseded owner before `create_in` clears it.
        self.previous
            .retain(|previous| previous.directory() != checkpoint_directory.as_path());
        let inject_io_failure = self.take_injected_io_failure();
        let (tx, rx) = mpsc::sync_channel(1);
        // Tracked so a fatal panic elsewhere waits for this checkpoint to land.
        let job: Job = crate::recovery_seal::tracked(move || {
            let result = (|| -> Result<PagedRecovery, String> {
                cancel.check().map_err(|e| format!("{e:?}"))?;
                std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                let raw_path = root.join(format!(
                    "resident-input-{}-{}.tmp",
                    std::process::id(),
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
                let mut raw = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&raw_path)
                    .map_err(|e| e.to_string())?;
                if let Some(encoding) = &encoding {
                    // Streams the retained original; no whole-file copy is built.
                    encoding
                        .visit_original(0..encoding.original_len(), |chunk| {
                            cancel.check()?;
                            raw.write_all(chunk).map_err(FileError::Io)
                        })
                        .map_err(|e| match e {
                            FileError::Io(e) => e.to_string(),
                            e => format!("{e:?}"),
                        })?;
                }
                raw.sync_all().map_err(|e| e.to_string())?;
                drop(raw);
                let result = (|| -> Result<PagedRecovery, String> {
                    let input = FileInput {
                        file: std::fs::File::open(&raw_path).map_err(|e| e.to_string())?,
                        path: raw_path.clone(),
                    };
                    let mut transcoder = DiskTranscoder::new(
                        input,
                        platform.clone(),
                        &root,
                        DiskOptions {
                            temp_quota_bytes: 20u64 << 30,
                            interpret: encoding.as_ref().map(|encoding| encoding.original_encoding()),
                        },
                        bytes.clone(),
                        cancel.clone(),
                    )
                    .map_err(|e| format!("{e:?}"))?;
                    while !transcoder.step().map_err(|e| format!("{e:?}"))?.complete {}
                    let mut store = transcoder.finish().map_err(|e| format!("{e:?}"))?;
                    if let Some(encoding) = &encoding {
                        store.state = encoding.state.clone();
                        store.eol = encoding.eol;
                    }
                    let source = store
                        .open_paged(
                            platform.clone(),
                            crate::source::SourceOptions::default(),
                            bytes.clone(),
                            Budget::new(0),
                            cancel.clone(),
                        )
                        .map_err(|e| format!("{e:?}"))?;
                    let pieces = match &encoding {
                        Some(encoding) => encoding.recovery_pieces(&snapshot).map_err(|e| format!("{e:?}"))?,
                        None => snapshot
                            .chunks(TextOffset(0)..TextOffset(snapshot.len()))
                            .map_err(|e| format!("{e:?}"))?
                            .map(|text| bareline_document::paged::RestoredPiece::Inserted(text.to_owned()))
                            .collect(),
                    };
                    let mut document = bareline_document::paged::PagedDocument::restore_pieces(
                        source.source.source(),
                        pieces,
                        bytes,
                        Budget::new(0),
                        snapshot.revision,
                    )
                    .map_err(|e| format!("{e:?}"))?;
                    document
                        .restore_metadata(snapshot.metadata().clone())
                        .map_err(|e| format!("{e:?}"))?;
                    let paged = document.snapshot();
                    if inject_io_failure {
                        return Err(std::io::Error::other("injected checkpoint I/O failure").to_string());
                    }
                    let mut recovery = PagedRecovery::create_in(
                        checkpoint_directory,
                        store,
                        original_path,
                        paged,
                        platform,
                        status,
                        notify.clone(),
                    )?;
                    // Keyed by the resident text, so later incremental checkpoints
                    // find this copy's owned text instead of writing it again.
                    recovery.append_resident(&snapshot, encoding.as_ref())?;
                    Ok(recovery)
                })();
                let _ = std::fs::remove_file(raw_path);
                result
            })();
            let _ = tx.send(Checkpoint::Full(result));
            notify();
        });
        if !self.take_injected_queue_full() && worker().try_send(job).is_ok() {
            self.pending = Some(rx);
            self.pending_generation = generation;
            self.captured = Some(state);
        } else if let Ok(mut status) = self.status.lock() {
            status.error = Some("Recovery queue full; retry.".into());
        }
        changed
    }
    /// Append to the current journal instead of copying the document again (REC-10)
    /// while that journal is healthy, the revision advances, the schedule allows it,
    /// and the owned store has not grown far past the text it must describe.
    fn incremental_allowed(&self, snapshot: &DocumentSnapshot) -> bool {
        !self.force_full
            && self.incremental < self.max_incremental
            && self.current.as_ref().is_some_and(|current| {
                let healthy = current
                    .status
                    .lock()
                    .is_ok_and(|status| status.durable.is_some() && status.error.is_none());
                let bound = (snapshot.len() as u64).saturating_mul(2).saturating_add(8 << 20);
                healthy
                    && current
                        .last_root()
                        .is_some_and(|revision| revision < snapshot.revision.0)
                    && current.owned_bytes() <= bound
            })
    }
    fn append_incremental(&mut self, snapshot: DocumentSnapshot, changed: bool) -> bool {
        let state = snapshot.content_state;
        let Some(directory) = self.current.as_ref().map(|current| current.directory().to_path_buf()) else {
            return changed;
        };
        // Shared with the job so an unqueued job hands the journal straight back.
        let slot = Arc::new(Mutex::new(self.current.take()));
        let held = slot.clone();
        let encoding = self.encoding.clone();
        let cancel = self.cancellation.clone();
        let notify = self.notify.clone();
        let inject_io_failure = self.take_injected_io_failure();
        let (tx, rx) = mpsc::sync_channel(1);
        let job: Job = crate::recovery_seal::tracked(move || {
            let taken = held.lock().ok().and_then(|mut held| held.take());
            let message = match taken {
                Some(mut recovery) => {
                    let outcome = if inject_io_failure {
                        Err(std::io::Error::other("injected checkpoint I/O failure").to_string())
                    } else {
                        cancel
                            .check()
                            .map_err(|e| format!("{e:?}"))
                            .and_then(|_| recovery.append_resident(&snapshot, encoding.as_ref()))
                    };
                    Checkpoint::Incremental(recovery, outcome)
                }
                None => Checkpoint::Full(Err("Recovery checkpoint unavailable; retry.".into())),
            };
            let _ = tx.send(message);
            notify();
        });
        if !self.take_injected_queue_full() && worker().try_send(job).is_ok() {
            self.pending = Some(rx);
            self.appending = Some(directory);
            self.captured = Some(state);
        } else {
            self.current = slot.lock().ok().and_then(|mut slot| slot.take());
            if let Ok(mut status) = self.status.lock() {
                status.error = Some("Recovery queue full; retry.".into());
            }
        }
        changed
    }
    fn slot_directory(&self, generation: u64) -> PathBuf {
        self.root.join(format!("{}-g{}", self.slot, generation % 2))
    }
    #[cfg(test)]
    fn take_injected_io_failure(&mut self) -> bool {
        std::mem::take(&mut self.inject_io_failure)
    }
    #[cfg(not(test))]
    fn take_injected_io_failure(&mut self) -> bool {
        false
    }
    #[cfg(test)]
    fn take_injected_queue_full(&mut self) -> bool {
        std::mem::take(&mut self.inject_queue_full)
    }
    #[cfg(not(test))]
    fn take_injected_queue_full(&mut self) -> bool {
        false
    }
}
impl ResidentRecovery {
    /// Request a durable discard without waiting for checkpoint or storage workers.
    pub fn discard(&mut self) -> crate::recovery_retirement::DiscardPoll {
        self.cancellation.cancel();
        if self.discard.is_none() {
            let pending = self.pending.take();
            self.appending = None;
            let retirement = self.retirement.take();
            let mut recoveries = std::mem::take(&mut self.previous);
            recoveries.extend(self.current.take());
            let mut paths = std::mem::take(&mut self.retire_paths);
            paths.append(&mut self.adopted_paths);
            let root = self.root.clone();
            let slot = self.slot.clone();
            let platform = self.platform.clone();
            let notify = self.notify.clone();
            for generation in 0..2 {
                let path = root.join(format!("{slot}-g{generation}"));
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
            self.discard = Some(crate::recovery_retirement::DiscardTicket::request_after_drain(
                crate::recovery_retirement::RecoveryOwnership {
                    recoveries,
                    paths,
                    platform,
                },
                move || {
                    let mut additional = Vec::new();
                    if let Some(receiver) = pending
                        && let Ok(Checkpoint::Full(Ok(recovery)) | Checkpoint::Incremental(recovery, _)) =
                            receiver.recv()
                    {
                        additional.push(recovery);
                    }
                    let removed_paths = retirement
                        .and_then(|receiver| receiver.recv().ok())
                        .and_then(Result::ok)
                        .unwrap_or_default();
                    crate::recovery_retirement::RecoveryAddition {
                        recoveries: additional,
                        paths: Vec::new(),
                        removed_paths,
                    }
                },
                notify,
            ));
        }
        let outcome = self.discard.as_ref().unwrap().poll();
        if matches!(outcome, crate::recovery_retirement::DiscardPoll::TombstoneFailed(_)) {
            self.discard.as_ref().unwrap().retry();
        }
        if let Ok(mut status) = self.status.lock() {
            match &outcome {
                // Waiting for the tombstone is the normal discard path, not a
                // recovery failure; reporting it as an error raised a stale
                // "Recovery unavailable" notice while reload or close waited.
                crate::recovery_retirement::DiscardPoll::Pending => status.error = None,
                crate::recovery_retirement::DiscardPoll::CleanupPending(error)
                | crate::recovery_retirement::DiscardPoll::TombstoneFailed(error) => status.error = Some(error.clone()),
                crate::recovery_retirement::DiscardPoll::Durable => *status = Default::default(),
            }
        }
        outcome
    }
}
impl Drop for ResidentRecovery {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if self.clean {
            let _ = self.discard();
        }
    }
}

#[cfg(test)]
mod journal_tests {
    use super::*;
    use bareline_document::Document;
    use std::{
        fs, io,
        path::Path,
        time::{Duration, Instant},
    };
    struct Platform;
    impl LocalFileSystem for Platform {
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
            _: Duration,
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
        fn identity(&self, file: &fs::File) -> io::Result<bareline_platform::FileIdentity> {
            let metadata = file.metadata()?;
            Ok(bareline_platform::FileIdentity {
                volume: 1,
                file: 1,
                length: metadata.len(),
                modified: metadata
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
            })
        }
        fn guard_directory(&self, _: &Path) -> io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
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
    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bareline-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn journals(root: &Path) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(root) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && entry.file_name().to_string_lossy().starts_with("paged-")
            })
            .map(|entry| entry.path())
            .collect()
    }
    /// Drive one checkpoint to completion and return the directory it landed in.
    fn checkpoint(recovery: &mut ResidentRecovery, text: &str) -> PathBuf {
        let document = Document::from_utf8(text, Budget::new(1 << 24), Budget::new(0)).unwrap();
        let snapshot = document.snapshot();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            recovery.observe(snapshot.clone(), true);
            let status = recovery.status();
            assert!(status.error.is_none(), "{:?}", status.error);
            if recovery.pending.is_none() && recovery.captured == Some(snapshot.content_state) {
                return status.directory.expect("checkpoint directory");
            }
            assert!(Instant::now() < deadline, "checkpoint never completed");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn await_retirement(recovery: &mut ResidentRecovery, text: &str, target: &Path) {
        let document = Document::from_utf8(text, Budget::new(1 << 24), Budget::new(0)).unwrap();
        let snapshot = document.snapshot();
        let deadline = Instant::now() + Duration::from_secs(60);
        while target.exists() {
            recovery.observe(snapshot.clone(), true);
            assert!(Instant::now() < deadline, "retirement never completed");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn next_durable_checkpoint_retires_the_previous_one_on_an_untitled_document() {
        let root = scratch("resident-retire");
        let mut recovery = ResidentRecovery::new(
            root.join("recovery"),
            Arc::new(Platform),
            None,
            None,
            Budget::new(1 << 24),
            Arc::new(|| {}),
        );
        let first = checkpoint(&mut recovery, "first draft");
        assert!(first.exists());
        let second = checkpoint(&mut recovery, "second draft");
        assert_ne!(first, second);
        await_retirement(&mut recovery, "second draft", &first);
        // The baseline copy need not have reported complete: durability of the
        // successor alone must retire the earlier checkpoint.
        assert!(!first.exists(), "previous checkpoint was not retired");
        assert!(second.exists());
        std::mem::forget(recovery);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn discard_removes_every_generation_even_mid_rotation() {
        let root = scratch("resident-discard");
        let recovery_root = root.join("recovery");
        let mut recovery = ResidentRecovery::new(
            recovery_root.clone(),
            Arc::new(Platform),
            None,
            None,
            Budget::new(1 << 24),
            Arc::new(|| {}),
        );
        checkpoint(&mut recovery, "first draft");
        checkpoint(&mut recovery, "second draft");
        // Discard before the asynchronous retirement has run: both generations
        // remain owned until the worker publishes its durable tombstones.
        assert_eq!(recovery.discard(), crate::recovery_retirement::DiscardPoll::Pending);
        let deadline = Instant::now() + Duration::from_secs(10);
        while matches!(recovery.discard(), crate::recovery_retirement::DiscardPoll::Pending) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        loop {
            let left = journals(&recovery_root);
            if left.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "discard cleanup left journal directories: {left:?}"
            );
            std::thread::yield_now();
        }
        std::mem::forget(recovery);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn checkpoint_waits_for_retirement_acknowledgment_before_reusing_a_slot() {
        let root = scratch("resident-delayed-retirement");
        let recovery_root = root.join("recovery");
        let mut recovery = ResidentRecovery::new(
            recovery_root.clone(),
            Arc::new(Platform),
            None,
            None,
            Budget::new(1 << 24),
            Arc::new(|| {}),
        );
        let retired = recovery_root.join(format!("{}-g1", recovery.slot));
        recovery.retire_paths.push(retired.clone());
        recovery.status.lock().unwrap().directory = Some(retired.clone());
        let (tx, rx) = mpsc::sync_channel(1);
        recovery.retirement = Some(rx);
        let document = Document::from_utf8("new draft", Budget::new(1 << 24), Budget::new(0)).unwrap();

        // Hold the cleanup acknowledgment independently of worker scheduling.
        // No new writer may reuse g1 while that acknowledgment can still clear it.
        recovery.observe(document.snapshot(), true);
        assert!(
            recovery.pending.is_none(),
            "checkpoint started before retirement acknowledgment"
        );
        assert!(recovery.captured.is_none());
        assert_eq!(recovery.generation, 0);
        tx.send(Ok(vec![retired.clone()])).unwrap();

        let replacement = checkpoint(&mut recovery, "new draft");
        assert_eq!(replacement, retired);
        assert!(replacement.exists());
        assert!(recovery.retire_paths.is_empty());
        assert_eq!(recovery.status().directory, Some(replacement));
        drop(recovery);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn fifty_checkpoints_keep_at_most_two_journal_directories() {
        let root = scratch("resident-rotate");
        let recovery_root = root.join("recovery");
        let mut recovery = ResidentRecovery::new(
            recovery_root.clone(),
            Arc::new(Platform),
            None,
            None,
            Budget::new(1 << 24),
            Arc::new(|| {}),
        );
        for index in 0..50 {
            checkpoint(&mut recovery, &format!("draft revision {index}"));
            let count = journals(&recovery_root).len();
            assert!(count <= 2, "{count} journal directories after checkpoint {index}");
        }
        std::mem::forget(recovery);
        let _ = fs::remove_dir_all(root);
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Fault {
        None,
        Io,
        QueueFull,
    }
    /// Drive one checkpoint attempt for `text` through `fault` until it settles. A
    /// successful attempt also waits for its predecessor's retirement to finish.
    fn attempt(recovery: &mut ResidentRecovery, text: &str, fault: Fault) -> Result<PathBuf, String> {
        let document = Document::from_utf8(text, Budget::new(1 << 24), Budget::new(0)).unwrap();
        let snapshot = document.snapshot();
        recovery.inject_io_failure = fault == Fault::Io;
        recovery.inject_queue_full = fault == Fault::QueueFull;
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut durable = None;
        loop {
            recovery.observe(snapshot.clone(), true);
            // Injected faults are consumed exactly when the attempt is built.
            let built = !recovery.inject_io_failure && !recovery.inject_queue_full;
            if durable.is_none() && built && recovery.pending.is_none() {
                let status = recovery.status();
                if fault == Fault::QueueFull {
                    assert_ne!(recovery.captured, Some(snapshot.content_state));
                    return Err(status.error.expect("queue-full error"));
                }
                if recovery.captured == Some(snapshot.content_state) {
                    if let Some(error) = status.error {
                        return Err(error);
                    }
                    durable = Some(status.directory.expect("checkpoint directory"));
                }
            }
            if let Some(directory) = &durable {
                let status = recovery.status();
                assert!(status.error.is_none(), "{:?}", status.error);
                if recovery.retirement.is_none() && recovery.retire_paths.is_empty() {
                    return Ok(directory.clone());
                }
            }
            assert!(Instant::now() < deadline, "checkpoint attempt never settled");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn durable_checkpoints(root: &Path) -> Vec<PathBuf> {
        journals(root)
            .into_iter()
            .filter(|directory| {
                crate::recovery::inspect(directory, &Cancellation::default()).is_ok_and(|inspection| {
                    inspection.status != crate::recovery::RecoveryStatus::Discarded && inspection.last_durable.is_some()
                })
            })
            .collect()
    }
    /// Checkpoint N is durable, N+1 fails through `fault`, and N+2 succeeds: exactly
    /// N+2 must remain on disk and restore.
    fn failed_attempt_between_durable_checkpoints_keeps_one_restorable(name: &str, fault: Fault) {
        let root = scratch(name);
        let recovery_root = root.join("recovery");
        let mut recovery = ResidentRecovery::new(
            recovery_root.clone(),
            Arc::new(Platform),
            None,
            None,
            Budget::new(1 << 24),
            Arc::new(|| {}),
        );
        let first = attempt(&mut recovery, "checkpoint N", Fault::None).unwrap();
        assert!(attempt(&mut recovery, "checkpoint N+1", fault).is_err());
        assert_eq!(recovery.generation, 1, "a failed attempt consumed a generation");
        assert_eq!(durable_checkpoints(&recovery_root), vec![first.clone()]);

        let last = attempt(&mut recovery, "checkpoint N+2", Fault::None).unwrap();
        assert_ne!(last, first, "N+2 rewrote the slot of the only durable checkpoint");
        assert_eq!(recovery.generation, 2);
        assert_eq!(journals(&recovery_root), vec![last.clone()]);
        assert_eq!(durable_checkpoints(&recovery_root), vec![last.clone()]);

        let deadline = Instant::now() + Duration::from_secs(60);
        while !crate::recovery::inspect(&last, &Cancellation::default())
            .is_ok_and(|inspection| inspection.complete_baseline)
        {
            assert!(Instant::now() < deadline, "checkpoint baseline never completed");
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(recovery);
        let bytes = Budget::new(64 << 20);
        let mut restored = crate::paged_recovery::restore(
            &last,
            Arc::new(Platform),
            bytes.clone(),
            Budget::new(0),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(
            crate::paged_recovery::restore_text(&mut restored, 1 << 20, &bytes, &Cancellation::default())
                .unwrap()
                .as_deref(),
            Some("checkpoint N+2")
        );
        drop(restored);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn io_failure_between_checkpoints_never_clears_the_durable_one() {
        failed_attempt_between_durable_checkpoints_keeps_one_restorable("resident-fault-io", Fault::Io);
    }
    #[test]
    fn full_queue_between_checkpoints_never_clears_the_durable_one() {
        failed_attempt_between_durable_checkpoints_keeps_one_restorable("resident-fault-queue", Fault::QueueFull);
    }
    /// Random mix of successful, failed and unqueued attempts from a fixed seed. While
    /// the document is dirty the latest successful checkpoint must stay durable.
    fn random_faults_keep_a_durable_checkpoint(name: &str, seed: u64, steps: usize) {
        let root = scratch(name);
        let recovery_root = root.join("recovery");
        let mut recovery = ResidentRecovery::new(
            recovery_root.clone(),
            Arc::new(Platform),
            None,
            None,
            Budget::new(1 << 24),
            Arc::new(|| {}),
        );
        let mut state = seed;
        let mut latest = None;
        for step in 0..steps {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let fault = match state % 4 {
                _ if step == 0 => Fault::None,
                0 => Fault::Io,
                1 => Fault::QueueFull,
                _ => Fault::None,
            };
            match attempt(&mut recovery, &format!("draft {step}"), fault) {
                Ok(directory) => {
                    assert_eq!(fault, Fault::None, "step {step}");
                    latest = Some(directory);
                }
                Err(error) => assert_ne!(fault, Fault::None, "step {step}: {error}"),
            }
            let latest = latest.as_ref().expect("first checkpoint");
            let durable = durable_checkpoints(&recovery_root);
            assert!(
                durable.contains(latest),
                "step {step} ({fault:?}): latest checkpoint {latest:?} not durable; durable {durable:?}"
            );
            let count = journals(&recovery_root).len();
            assert!(count <= 2, "step {step}: {count} journal directories");
        }
        drop(recovery);
        let _ = fs::remove_dir_all(root);
    }
    /// Observe `snapshot` until its checkpoint attempt settles; returns the status.
    fn settle(recovery: &mut ResidentRecovery, snapshot: DocumentSnapshot) -> PagedRecoveryStatus {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            recovery.observe(snapshot.clone(), true);
            if recovery.pending.is_none() && recovery.captured == Some(snapshot.content_state) {
                return recovery.status();
            }
            assert!(Instant::now() < deadline, "checkpoint never settled");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn checkpoint_snapshot(recovery: &mut ResidentRecovery, snapshot: DocumentSnapshot) -> PathBuf {
        let status = settle(recovery, snapshot);
        assert!(status.error.is_none(), "{:?}", status.error);
        status.directory.expect("checkpoint directory")
    }
    fn type_at(document: &mut Document, index: usize) {
        use bareline_document::{Edit, EditTransaction};
        let snapshot = document.snapshot();
        let at = TextOffset((index * 7919) % snapshot.len());
        document
            .apply(EditTransaction {
                base_revision: snapshot.revision,
                edits: vec![Edit {
                    range: at..at,
                    insert: "x".into(),
                }],
            })
            .unwrap();
    }
    fn owned_bytes(directory: &Path) -> u64 {
        fs::read_dir(directory)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("root-owned-"))
            .map(|entry| entry.metadata().unwrap().len())
            .sum()
    }
    fn restored_text(directory: &Path) -> String {
        let bytes = Budget::new(64 << 20);
        let mut restored = crate::paged_recovery::restore(
            directory,
            Arc::new(Platform),
            bytes.clone(),
            Budget::new(0),
            &Cancellation::default(),
        )
        .unwrap();
        crate::paged_recovery::restore_text(&mut restored, 16 << 20, &bytes, &Cancellation::default())
            .unwrap()
            .expect("restored text")
    }
    /// A journal restores only with its baseline, so wait for the copy to land before
    /// dropping the recovery (which cancels a copy still running).
    fn await_baseline(directory: &Path) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !crate::recovery::inspect(directory, &Cancellation::default())
            .is_ok_and(|inspection| inspection.complete_baseline)
        {
            assert!(Instant::now() < deadline, "checkpoint baseline never completed");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn full_text(document: &Document) -> String {
        let snapshot = document.snapshot();
        snapshot
            .read(TextOffset(0)..TextOffset(snapshot.len()), usize::MAX)
            .unwrap()
    }
    #[test]
    fn typing_checkpoints_append_only_new_text_to_one_journal() {
        let root = scratch("resident-incremental");
        let recovery_root = root.join("recovery");
        let mut recovery = ResidentRecovery::new(
            recovery_root.clone(),
            Arc::new(Platform),
            None,
            None,
            Budget::new(1 << 26),
            Arc::new(|| {}),
        );
        let text: String = (0..40_000)
            .map(|line| format!("line {line:05} of pasted text\n"))
            .collect();
        let mut document = Document::from_utf8(&text, Budget::new(1 << 26), Budget::new(1 << 24)).unwrap();
        let journal = checkpoint_snapshot(&mut recovery, document.snapshot());
        let full = owned_bytes(&journal);
        assert!(full >= text.len() as u64);
        let baseline = fs::metadata(journal.join("baseline.bin")).unwrap().len();
        for index in 0..20 {
            type_at(&mut document, index);
            let directory = checkpoint_snapshot(&mut recovery, document.snapshot());
            assert_eq!(
                directory, journal,
                "checkpoint {index} copied the document into a new journal"
            );
        }
        // Each checkpoint stored only text no earlier root held; a full copy per
        // checkpoint (the replaced behaviour) would have added the whole document 20 times.
        let appended = owned_bytes(&journal) - full;
        assert!(appended <= 20 * 64 * 1024, "{appended} owned bytes appended");
        assert!(appended < text.len() as u64);
        assert_eq!(fs::metadata(journal.join("baseline.bin")).unwrap().len(), baseline);
        assert_eq!(journals(&recovery_root), vec![journal.clone()]);
        // Superseded roots are pruned once a newer one is durable.
        let receipts = fs::read_dir(&journal)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name().to_string_lossy().ends_with(".receipt.json"))
            })
            .count();
        assert_eq!(receipts, 2);
        let expected = full_text(&document);
        await_baseline(&journal);
        drop(recovery);
        assert_eq!(restored_text(&journal), expected);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn failed_or_scheduled_incremental_checkpoint_takes_a_full_copy() {
        let root = scratch("resident-incremental-fallback");
        let recovery_root = root.join("recovery");
        let mut recovery = ResidentRecovery::new(
            recovery_root.clone(),
            Arc::new(Platform),
            None,
            None,
            Budget::new(1 << 26),
            Arc::new(|| {}),
        );
        let mut document = Document::from_utf8("first draft\n", Budget::new(1 << 24), Budget::new(1 << 20)).unwrap();
        let first = checkpoint_snapshot(&mut recovery, document.snapshot());
        let durable = crate::recovery::inspect(&first, &Cancellation::default())
            .unwrap()
            .last_durable;
        type_at(&mut document, 0);
        recovery.inject_io_failure = true;
        let status = settle(&mut recovery, document.snapshot());
        assert!(status.error.is_some());
        // The journal keeps its last durable root and stays current.
        assert_eq!(
            recovery.current.as_ref().map(|current| current.directory()),
            Some(first.as_path())
        );
        assert_eq!(
            crate::recovery::inspect(&first, &Cancellation::default())
                .unwrap()
                .last_durable,
            durable
        );
        type_at(&mut document, 1);
        let second = checkpoint_snapshot(&mut recovery, document.snapshot());
        assert_ne!(
            second, first,
            "a failed incremental checkpoint must be followed by a full copy"
        );
        // The bounded schedule: one incremental append, then a full copy again.
        recovery.max_incremental = 1;
        type_at(&mut document, 2);
        assert_eq!(checkpoint_snapshot(&mut recovery, document.snapshot()), second);
        type_at(&mut document, 3);
        let third = checkpoint_snapshot(&mut recovery, document.snapshot());
        assert_ne!(third, second);
        let expected = full_text(&document);
        await_baseline(&third);
        drop(recovery);
        assert_eq!(restored_text(&third), expected);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn random_checkpoint_faults_always_leave_a_durable_checkpoint() {
        random_faults_keep_a_durable_checkpoint("resident-fault-random", 0x9E37_79B9_7F4A_7C15, 200);
    }
    #[test]
    #[ignore = "1,000-step soak of the random checkpoint fault sequence; run with --ignored"]
    fn random_checkpoint_faults_soak() {
        random_faults_keep_a_durable_checkpoint("resident-fault-soak", 0xD1B5_4A32_D192_ED03, 1000);
    }
    /// Release builds abort on panic, so no unwinding reaches recovery code. A
    /// worker panic must still leave the queued checkpoint durable and restorable
    /// (the fatal hook seals before aborting) and a crash record without text.
    #[test]
    fn worker_panic_seals_queued_checkpoint_with_a_text_free_crash_record() {
        const SENTINEL: &str = "PRIVATE_DOCUMENT_SENTINEL_sealed_on_panic";
        const CHILD: &str = "BARELINE_RECOVERY_SEAL_CHILD";
        let text = format!("unsaved draft {SENTINEL}");
        if let Some(directory) = std::env::var_os(CHILD) {
            let directory = PathBuf::from(directory);
            let _log = bareline_diagnostics::LocalLog::open(&directory.join("logs")).unwrap();
            bareline_diagnostics::install_fatal_panic_hook(crate::recovery_seal::seal, Duration::from_secs(60));
            // Hold the recovery worker until the hook starts sealing, so the
            // checkpoint below is still queued when the panic happens. Without
            // sealing the process aborts before it ever runs.
            worker()
                .send(Box::new(|| {
                    let deadline = Instant::now() + Duration::from_secs(60);
                    while !crate::recovery_seal::sealing() && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }))
                .unwrap();
            let mut recovery = ResidentRecovery::new(
                directory.join("recovery"),
                Arc::new(Platform),
                None,
                None,
                Budget::new(1 << 24),
                Arc::new(|| {}),
            );
            let document = Document::from_utf8(&text, Budget::new(1 << 24), Budget::new(0)).unwrap();
            recovery.observe(document.snapshot(), true);
            assert!(recovery.pending.is_some(), "checkpoint was not queued");
            let panicking: std::thread::JoinHandle<()> = std::thread::Builder::new()
                .name("forced-worker-panic".into())
                .spawn(|| panic!("{SENTINEL}"))
                .unwrap();
            // The fatal hook aborts the process; returning here fails the parent.
            let _ = panicking.join();
            std::mem::forget(recovery);
            return;
        }
        let directory = scratch("recovery-seal");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resident_recovery::journal_tests::worker_panic_seals_queued_checkpoint_with_a_text_free_crash_record",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, &directory)
            .output()
            .unwrap();
        assert!(!output.status.success(), "the child must abort");
        let crash = fs::read_to_string(directory.join("logs").join("bareline.crash.log")).unwrap();
        for record in [
            &crash,
            &String::from_utf8_lossy(&output.stderr).into_owned(),
            &String::from_utf8_lossy(&output.stdout).into_owned(),
        ] {
            assert!(!record.contains(SENTINEL));
        }
        assert!(
            crash.contains("\"event\":\"panic\"") && crash.contains("\"recovery_sealed\":true"),
            "{crash}"
        );
        let checkpoints = journals(&directory.join("recovery"));
        assert_eq!(checkpoints.len(), 1, "{checkpoints:?}");
        let mut restored = crate::paged_recovery::restore(
            &checkpoints[0],
            Arc::new(Platform),
            Budget::new(64 << 20),
            Budget::new(16 << 20),
            &Cancellation::default(),
        )
        .unwrap();
        let restored_text = crate::paged_recovery::restore_text(
            &mut restored,
            1 << 20,
            &Budget::new(4 << 20),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(restored_text.as_deref(), Some(text.as_str()));
        drop(restored);
        let _ = fs::remove_dir_all(directory);
    }
}
