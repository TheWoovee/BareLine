// SPDX-License-Identifier: MPL-2.0
//! Existing Paged actors spill resident edit/history leaves on the I/O worker.
use super::*;
struct SpillCompletion {
    actor: PagedSession,
    wake: JobWake,
    finished: bool,
}
impl SpillCompletion {
    fn new(actor: PagedSession, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            actor,
            wake: JobWake::new(notify),
            finished: false,
        }
    }
    fn finish(mut self) {
        self.finished = true;
        self.wake.fire();
    }
}
impl Drop for SpillCompletion {
    fn drop(&mut self) {
        if !self.finished {
            self.actor.finish_spill(Some("Paged spill worker failed".into()));
        }
    }
}
impl PagedEditorSurface {
    pub fn configure_owned_spill(
        &mut self,
        cache: PathBuf,
        platform: Arc<dyn LocalFileSystem>,
        options: bareline_file_io::source::SourceOptions,
    ) {
        self.actor.configure_spill(cache, platform, options);
    }
    pub(super) fn pump_owned_spill(&mut self) {
        if let Some(error) = self.actor.take_spill_error() {
            self.error = Some(error);
        }
        if self.captured.is_some() || self.busy() || self.budget.used() <= self.budget.limit().saturating_mul(3) / 4 {
            return;
        }
        let stamp = (self.snapshot.identity_token().0, self.snapshot.content_state);
        let Some(reservation) = self.actor.reserve_spill(stamp) else {
            return;
        };
        let bareline_file_io::paged_service::PagedSpillReservation {
            cache,
            platform,
            options,
        } = reservation;
        let actor = self.actor.clone();
        let peer = self.peer.clone();
        let budget = self.budget.clone();
        let quota = self.streaming_quota;
        let cancel = self.cancellation.clone();
        let notify = self.notify.clone();
        let submitted = worker().submit(
            WorkKind::Maintenance,
            Box::new(move || {
                let completion = SpillCompletion::new(actor.clone(), notify);
                let result = (|| -> Result<(), String> {
                    cancel
                        .check()
                        .map_err(|error| format!("Paged spill cancelled: {error:?}"))?;
                    let requested = actor.state().map_err(|error| error.to_string())?.stamp;
                    let plan = {
                        let opened = actor.lock_document().map_err(|error| error.to_string())?;
                        let snapshot = opened.document().snapshot();
                        if (snapshot.identity_token().0, snapshot.content_state) != stamp {
                            return Ok(());
                        }
                        opened
                            .document()
                            .capture_spill()
                            .map_err(|error| format!("Paged spill capture: {error:?}"))?
                    };
                    // Source-backed leaves are already spill-owned. Copy only deduplicated
                    // Resident allocations, including retained undo/redo inverse leaves.
                    let owned_bytes = plan
                        .segments()
                        .try_fold(0usize, |sum, segment| sum.checked_add(segment.text.len()))
                        .ok_or("Paged spill size overflow")?;
                    if owned_bytes == 0 {
                        return Ok(());
                    }
                    let prepared = bareline_file_io::owned_store::prepare_segments(
                        plan, None, &cache, quota, platform, options, budget, &cancel,
                    )
                    .map_err(|error| format!("Paged spill unavailable: {error:?}"))?;
                    cancel
                        .check()
                        .map_err(|error| format!("Paged spill cancelled: {error:?}"))?;
                    let receipt = actor.execute(
                        bareline_file_io::paged_service::PagedLifecycleCommand::Spill { requested, prepared },
                        &cancel,
                    );
                    if !actor.receipt_applies(&receipt) {
                        return Ok(());
                    }
                    match receipt.terminal.map_err(|error| error.to_string())? {
                        bareline_file_io::paged_service::PagedTerminalOutcome::Spilled => {}
                        _ => return Err("Unexpected spill receipt".into()),
                    }
                    let mut peer = peer.lock().map_err(|_| "Paged peer state stopped")?;
                    peer.epoch = peer.epoch.wrapping_add(1);
                    Ok(())
                })();
                actor.finish_spill(result.err());
                completion.finish();
            }),
        );
        if submitted.is_err() {
            self.actor.cancel_spill_reservation();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_file_io::{
        codecs::disk::DiskOptions,
        lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
        source::SourceOptions,
    };
    use std::{
        fs::{self, File},
        path::Path,
        time::{Duration, Instant},
    };
    struct Platform;
    impl LocalFileSystem for Platform {
        fn validate_target(&self, _: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn available_space(&self, _: &Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        fn guard_directory(&self, _: &Path) -> std::io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn open_sealed_read(&self, path: &Path) -> std::io::Result<File> {
            File::open(path)
        }
        fn identity(&self, file: &File) -> std::io::Result<bareline_platform::FileIdentity> {
            let m = file.metadata()?;
            Ok(bareline_platform::FileIdentity {
                volume: 1,
                file: 1,
                length: m.len(),
                modified: m.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64,
            })
        }
        fn commit(&self, stage: &Path, target: &Path, _: bool) -> std::io::Result<()> {
            fs::rename(stage, target)
        }
    }
    #[test]
    fn pressure_spills_existing_paged_edit_history_and_refreshes_same_identity() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bareline-paged-spill-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("input.txt");
        fs::write(&path, b"a").unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let options = SourceOptions {
            resident_max_bytes: 0,
            page_size_bytes: 4096,
            page_cache_bytes: 8192,
        };
        let TranscodeOutcome::Complete(mut opened) = open_paged_encoded(
            PagedOpenRequest {
                path,
                bytes: budget.clone(),
                history: Budget::new(8 * 1024 * 1024),
                cache: root.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 8 * 1024 * 1024,
                    interpret: None,
                },
                source_options: options,
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("fixture open")
        };
        let snapshot = opened.transcoded.document.snapshot();
        let mut request = snapshot.begin_viewport(TextOffset(0), 1, &budget).unwrap();
        let window = loop {
            match request.poll() {
                bareline_document::paged::WindowPoll::Ready(window) => break window,
                bareline_document::paged::WindowPoll::Pending(ticket) => {
                    opened.transcoded.source.read_page(ticket).unwrap();
                }
                _ => panic!("fixture window unavailable"),
            }
        };
        opened
            .transcoded
            .document
            .apply_materialized(
                EditTransaction {
                    base_revision: snapshot.revision,
                    edits: vec![Edit {
                        range: TextOffset(0)..TextOffset(0),
                        insert: "x\n".repeat(524288),
                    }],
                },
                &[window],
            )
            .unwrap();
        let identity = opened.transcoded.document.snapshot().identity_token();
        let content = opened.transcoded.document.snapshot().content_state;
        let mut view = PagedEditorSurface::new(opened, budget.clone(), Arc::new(|| {})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while view.busy() {
            view.pump();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        view.configure_owned_spill(root.clone(), Arc::new(Platform), options);
        let pressure = budget
            .claim((budget.limit() * 4 / 5).saturating_sub(budget.used()))
            .unwrap();
        loop {
            view.pump();
            assert!(Instant::now() < deadline);
            let done = view.actor.spill_finished().unwrap();
            if done {
                break;
            }
            std::thread::yield_now();
        }
        {
            let opened = view.actor.lock_document().unwrap();
            let doc = opened.document();
            assert_eq!(doc.snapshot().identity_token(), identity);
            assert_eq!(doc.snapshot().content_state, content);
            assert_eq!(doc.history_stats().undo_changes, 1);
            assert_eq!(doc.capture_spill().unwrap().segments().count(), 0);
        }
        drop(pressure);
        view.refresh_peer();
        while view.busy() {
            view.pump();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.snapshot().identity_token(), identity);
        {
            let mut opened = view.actor.lock_document().unwrap();
            opened.document_mut().undo().unwrap();
            assert_eq!(opened.document().snapshot().len(), 1);
        }
        // Keep destruction on this thread: viewport prefetch also retains the
        // actor, but runs independently of the shared paged I/O worker.
        let mut actor = view.actor.clone();
        drop(view);
        drop(snapshot);
        // A result can arrive before its worker closure releases the actor.
        // Drain preceding jobs before removing Windows-backed fixture files.
        let (released, complete) = std::sync::mpsc::channel();
        worker()
            .submit(
                WorkKind::Maintenance,
                Box::new(move || {
                    let _ = released.send(());
                }),
            )
            .unwrap();
        complete.recv_timeout(Duration::from_secs(15)).unwrap();
        let release_deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match actor.try_unwrap() {
                Ok(actor) => {
                    drop(actor);
                    break;
                }
                Err(retained) => {
                    actor = retained;
                    assert!(
                        Instant::now() < release_deadline,
                        "Paged fixture actor still retained by background work"
                    );
                    std::thread::yield_now();
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
}
