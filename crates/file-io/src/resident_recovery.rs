// SPDX-License-Identifier: MPL-2.0
//! Complete Resident/untitled snapshot checkpoints with original-codec provenance.
//! Captures are coalesced while a checkpoint is preparing; all disk work is off UI.
use crate::{
    cancellation::Cancellation,
    codecs::{
        disk::{DiskOptions, DiskTranscoder},
        resident::ResidentEncoding,
    },
    lifecycle::FileInput,
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
    generation: u64,
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
    pending: Option<Receiver<Result<PagedRecovery, String>>>,
    current: Option<PagedRecovery>,
    status: Arc<Mutex<PagedRecoveryStatus>>,
    captured: Option<bareline_document::ContentStateId>,
    cancellation: Cancellation,
    discard: Option<crate::recovery_retirement::DiscardTicket>,
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
            status: Arc::new(Mutex::new(Default::default())),
            captured: None,
            cancellation: Cancellation::default(),
            discard: None,
        }
    }
    pub fn status(&self) -> PagedRecoveryStatus {
        self.status.lock().map(|state| state.clone()).unwrap_or_default()
    }
    pub fn set_original_path(&mut self, path: Option<PathBuf>) {
        self.original_path = path;
    }
    pub fn retry(&mut self) {
        self.captured = None;
        self.retry_retirement = true;
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
                        Ok(Ok(recovery)) => {
                            if let Some(previous) = self.current.replace(recovery) {
                                self.previous.push(previous);
                            }
                        }
                        Ok(Err(error)) => {
                            if let Ok(mut status) = self.status.lock() {
                                status.error = Some(error);
                            }
                        }
                        Err(_) => {
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
        self.generation += 1;
        let checkpoint_directory = root.join(format!("{}-g{}", self.slot, self.generation % 2));
        let (tx, rx) = mpsc::sync_channel(1);
        let job: Job = Box::new(move || {
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
                    for chunk in encoding.original_bytes().chunks(65536) {
                        cancel.check().map_err(|e| format!("{e:?}"))?;
                        raw.write_all(chunk).map_err(|e| e.to_string())?;
                    }
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
                    let snapshot = document.snapshot();
                    let mut recovery = PagedRecovery::create_in(
                        checkpoint_directory,
                        store,
                        original_path,
                        snapshot.clone(),
                        platform,
                        status,
                        notify.clone(),
                    )?;
                    recovery.append(&snapshot, &[])?;
                    Ok(recovery)
                })();
                let _ = std::fs::remove_file(raw_path);
                result
            })();
            let _ = tx.send(result);
            notify();
        });
        match worker().try_send(job) {
            Ok(()) => {
                self.pending = Some(rx);
                self.captured = Some(state);
            }
            Err(_) => {
                if let Ok(mut status) = self.status.lock() {
                    status.error = Some("Recovery queue full; retry.".into());
                }
            }
        }
        changed
    }
}
impl ResidentRecovery {
    /// Request a durable discard without waiting for checkpoint or storage workers.
    pub fn discard(&mut self) -> crate::recovery_retirement::DiscardPoll {
        self.cancellation.cancel();
        if self.discard.is_none() {
            let pending = self.pending.take();
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
                        && let Ok(Ok(recovery)) = receiver.recv()
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
                crate::recovery_retirement::DiscardPoll::Pending => {
                    status.error = Some("Discard requested; waiting for durable recovery tombstone.".into())
                }
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
}
