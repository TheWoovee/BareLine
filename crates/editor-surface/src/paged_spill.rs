// SPDX-License-Identifier: MPL-2.0
//! Existing Paged actors spill resident edit/history leaves on the I/O worker.
use super::*;
#[derive(Default)]
pub(super) struct SpillState {
    pending: bool,
    config: Option<(
        PathBuf,
        Arc<dyn LocalFileSystem>,
        bareline_file_io::source::SourceOptions,
    )>,
    attempted: Option<(u64, ContentStateId)>,
    error: Option<String>,
}
impl PagedEditorSurface {
    pub fn configure_owned_spill(
        &mut self,
        cache: PathBuf,
        platform: Arc<dyn LocalFileSystem>,
        options: bareline_file_io::source::SourceOptions,
    ) {
        if let Ok(mut state) = self.owned_spill.lock() {
            state.config = Some((cache, platform, options));
            state.attempted = None;
        }
    }
    pub(super) fn pump_owned_spill(&mut self) {
        if let Ok(mut state) = self.owned_spill.try_lock() {
            if let Some(error) = state.error.take() {
                self.error = Some(error);
            }
        }
        if self.captured.is_some()
            || self.busy()
            || self.budget.used() <= self.budget.limit().saturating_mul(3) / 4
        {
            return;
        }
        let stamp = (
            self.snapshot.identity_token().0,
            self.snapshot.content_state,
        );
        let Ok(mut state) = self.owned_spill.try_lock() else {
            return;
        };
        if state.pending || state.attempted == Some(stamp) {
            return;
        }
        let Some((cache, platform, options)) = state.config.clone() else {
            return;
        };
        state.pending = true;
        state.attempted = Some(stamp);
        drop(state);
        let actor = self.actor.clone();
        let peer = self.peer.clone();
        let state = self.owned_spill.clone();
        let budget = self.budget.clone();
        let quota = self.streaming_quota;
        let cancel = self.cancellation.clone();
        let notify = self.notify.clone();
        let submitted = worker().try_send(Box::new(move || {
            let result = (|| -> Result<(), String> {
                cancel
                    .check()
                    .map_err(|error| format!("Paged spill cancelled: {error:?}"))?;
                let plan = {
                    let opened = actor.lock().map_err(|_| "Paged actor stopped")?;
                    let snapshot = opened.transcoded.document.snapshot();
                    if (snapshot.identity_token().0, snapshot.content_state) != stamp {
                        return Ok(());
                    }
                    opened
                        .transcoded
                        .document
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
                let mut opened = actor.lock().map_err(|_| "Paged actor stopped")?;
                // Core validates document/revision/content/saved state and both history
                // depths. Any concurrent change discards preparation without publication.
                opened
                    .transcoded
                    .document
                    .attach_spill(prepared)
                    .map_err(|error| format!("Paged spill stale or unavailable: {error:?}"))?;
                let mut peer = peer.lock().map_err(|_| "Paged peer state stopped")?;
                peer.epoch = peer.epoch.wrapping_add(1);
                Ok(())
            })();
            if let Ok(mut state) = state.lock() {
                state.pending = false;
                if let Err(error) = result {
                    state.error = Some(error);
                }
            }
            notify();
        }));
        if submitted.is_err() {
            if let Ok(mut state) = self.owned_spill.lock() {
                state.pending = false;
                state.attempted = None;
            }
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
                modified: m
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
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
        let window = read_window(
            &mut opened,
            &mut None,
            &snapshot,
            0,
            1,
            &budget,
            &Cancellation::default(),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
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
            let done = {
                let state = view.owned_spill.lock().unwrap();
                assert!(state.error.is_none(), "{:?}", state.error);
                state.attempted.is_some() && !state.pending
            };
            if done {
                break;
            }
            std::thread::yield_now();
        }
        {
            let opened = view.actor.lock().unwrap();
            let doc = &opened.transcoded.document;
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
            let mut opened = view.actor.lock().unwrap();
            opened.transcoded.document.undo().unwrap();
            assert_eq!(opened.transcoded.document.snapshot().len(), 1);
        }
        drop(view);
        drop(snapshot);
        fs::remove_dir_all(root).unwrap();
    }
}
