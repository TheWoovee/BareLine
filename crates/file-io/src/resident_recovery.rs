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
        std::thread::Builder::new()
            .name("resident-recovery".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job();
                }
            })
            .expect("resident recovery worker");
        tx
    })
}
pub struct ResidentRecovery {
    root: PathBuf,
    platform: Arc<dyn LocalFileSystem>,
    encoding: Option<ResidentEncoding>,
    original_path: Option<PathBuf>,
    bytes: Budget,
    notify: Arc<dyn Fn() + Send + Sync>,
    previous: Vec<PagedRecovery>,
    retire_paths: Vec<PathBuf>,
    retry_retirement: bool,
    retirement: Option<Receiver<Result<Vec<PathBuf>, String>>>,
    pending: Option<Receiver<Result<PagedRecovery, String>>>,
    current: Option<PagedRecovery>,
    status: Arc<Mutex<PagedRecoveryStatus>>,
    captured: Option<bareline_document::ContentStateId>,
    cancellation: Cancellation,
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
            platform,
            encoding,
            original_path,
            bytes,
            notify,
            previous: Vec::new(),
            retire_paths: Vec::new(),
            retry_retirement: true,
            retirement: None,
            pending: None,
            current: None,
            status: Arc::new(Mutex::new(Default::default())),
            captured: None,
            cancellation: Cancellation::default(),
        }
    }
    pub fn status(&self) -> PagedRecoveryStatus {
        self.status
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }
    pub fn set_original_path(&mut self, path: Option<PathBuf>) {
        self.original_path = path;
    }
    pub fn retry(&mut self) {
        self.captured = None;
        self.retry_retirement = true;
    }
    pub fn observe(&mut self, snapshot: DocumentSnapshot, dirty: bool) -> bool {
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
                            Err(_) => {
                                status.error = Some("Recovery retirement worker stopped".into())
                            }
                        }
                    }
                }
            }
        }
        if self.retirement.is_none()
            && self.pending.is_none()
            && self.current.as_ref().is_some_and(|current| {
                let status = current.status.lock().unwrap();
                status.complete && status.durable.is_some() && status.error.is_none()
            })
        {
            let mut obsolete = std::mem::take(&mut self.previous);
            if !dirty && self.captured == Some(snapshot.content_state) {
                if let Some(current) = self.current.take() {
                    obsolete.push(current);
                }
                self.captured = None;
            }
            if !obsolete.is_empty() {
                self.retire_paths.extend(
                    obsolete
                        .iter()
                        .map(|recovery| recovery.directory().to_path_buf()),
                );
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
                        crate::recovery::discard(path, platform.as_ref())
                            .map_err(|error| error.to_string())?;
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
                        status.error = Some(
                            "Recovery retirement queue full; retry retained checkpoint retirement"
                                .into(),
                        );
                    }
                }
            }
        }
        if (!dirty && self.captured.is_none())
            || !snapshot.is_complete()
            || self.pending.is_some()
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
                            interpret: encoding
                                .as_ref()
                                .map(|encoding| encoding.original_encoding()),
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
                        Some(encoding) => encoding
                            .recovery_pieces(&snapshot)
                            .map_err(|e| format!("{e:?}"))?,
                        None => snapshot
                            .chunks(TextOffset(0)..TextOffset(snapshot.len()))
                            .map_err(|e| format!("{e:?}"))?
                            .map(|text| {
                                bareline_document::paged::RestoredPiece::Inserted(text.to_owned())
                            })
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
                    document.restore_metadata(snapshot.metadata().clone()).map_err(|e|format!("{e:?}"))?;
                    let snapshot = document.snapshot();
                    let mut recovery = PagedRecovery::create(
                        &root,
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
impl Drop for ResidentRecovery {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
