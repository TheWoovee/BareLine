// SPDX-License-Identifier: MPL-2.0
//! Settings storage: the bounded save worker, the request and completion
//! types it exchanges with the UI thread, and the controller's save queue.
//! The UI thread never reads or writes a settings file.
use super::*;
pub(super) struct SaveJob {
    pub(super) scope: Scope,
    pub(super) owner_epoch: u64,
    generation: u64,
    snapshot: SettingsDocument,
    pub(super) path: PathBuf,
    expected: DiskVersion,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DiskVersion {
    Absent,
    Bytes(Vec<u8>),
    /// Not read on the UI thread: the storage worker reads the baseline it was
    /// sent for this owner and compares a save against that (APP-19).
    Unread,
}
/// Work for the storage worker, in the order it was requested.
enum StorageRequest {
    /// Read what is on disk now as the baseline a later save of this owner is
    /// checked against. `fallback` stands in when the file cannot be read.
    Baseline {
        scope: Scope,
        owner_epoch: u64,
        path: PathBuf,
        fallback: Option<DiskVersion>,
    },
    /// Boxed (QA-18): a save job carries the settings snapshot and expected disk
    /// bytes and is far larger than a baseline request, so queue messages stay small.
    Save(Box<SaveJob>),
}
#[derive(Debug)]
enum SaveFailure {
    Io(String),
    ExternalChange(DiskVersion),
}
struct SaveCompletion {
    scope: Scope,
    owner_epoch: u64,
    generation: u64,
    snapshot: SettingsDocument,
    error: Option<SaveFailure>,
    path: PathBuf,
}
pub(super) struct Storage {
    pub(super) user: PathBuf,
    pub(super) workspace: Option<PathBuf>,
    sender: Sender<StorageRequest>,
    receiver: Receiver<SaveCompletion>,
    pub(super) active: Option<SaveOwner>,
    pub(super) pending: VecDeque<SaveJob>,
    pub(super) user_disk: DiskVersion,
    pub(super) workspace_disk: Option<DiskVersion>,
    pub(super) user_epoch: u64,
    pub(super) workspace_epoch: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SaveOwner {
    pub(super) scope: Scope,
    pub(super) owner_epoch: u64,
    pub(super) path: PathBuf,
}
fn read_disk_version(path: &std::path::Path) -> std::io::Result<DiskVersion> {
    match config::read_config(path) {
        Ok(bytes) => Ok(DiskVersion::Bytes(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DiskVersion::Absent),
        Err(error) => Err(error),
    }
}
impl Storage {
    /// Ask the worker to read the baseline for a new owner; the UI thread never
    /// reads the settings file, which may be up to 1 MiB or on a share (APP-19).
    pub(super) fn request_baseline(
        &self,
        scope: Scope,
        owner_epoch: u64,
        path: PathBuf,
        fallback: Option<DiskVersion>,
    ) {
        let _ = self.sender.send(StorageRequest::Baseline {
            scope,
            owner_epoch,
            path,
            fallback,
        });
    }
}
impl SettingsController {
    pub fn configure_storage(
        &mut self,
        user: PathBuf,
        workspace: Option<PathBuf>,
        platform: Arc<dyn LocalFileSystem>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<()> {
        let (sender, jobs) = mpsc::channel::<StorageRequest>();
        let (completed, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("bareline-settings-save".into())
            .spawn(move || {
                // The latest baseline per scope, read here instead of on the UI thread.
                let mut baselines: Vec<(Scope, u64, Result<DiskVersion, String>)> = Vec::new();
                while let Ok(request) = jobs.recv() {
                    let job = match request {
                        StorageRequest::Baseline {
                            scope,
                            owner_epoch,
                            path,
                            fallback,
                        } => {
                            let baseline =
                                read_disk_version(&path).or_else(|error| fallback.ok_or_else(|| error.to_string()));
                            baselines.retain(|(known, _, _)| *known != scope);
                            baselines.push((scope, owner_epoch, baseline));
                            continue;
                        }
                        StorageRequest::Save(job) => *job,
                    };
                    let expected = match &job.expected {
                        DiskVersion::Unread => baselines
                            .iter()
                            .find(|(scope, epoch, _)| *scope == job.scope && *epoch == job.owner_epoch)
                            .map_or_else(
                                || Err("Settings file state is unknown".to_owned()),
                                |(_, _, baseline)| baseline.clone(),
                            ),
                        known => Ok(known.clone()),
                    };
                    let current = read_disk_version(&job.path);
                    let prepare = if job.scope == Scope::Workspace {
                        job.path.parent().map_or(Ok(()), std::fs::create_dir_all)
                    } else {
                        Ok(())
                    };
                    let error = match (current, expected) {
                        (Err(error), _) => Some(SaveFailure::Io(error.to_string())),
                        (Ok(_), Err(error)) => Some(SaveFailure::Io(error)),
                        (Ok(current), Ok(expected)) if current != expected => {
                            Some(SaveFailure::ExternalChange(current))
                        }
                        (Ok(_), Ok(_)) => prepare
                            .and_then(|_| job.snapshot.save(&job.path, platform.as_ref()))
                            .err()
                            .map(|error| SaveFailure::Io(error.to_string())),
                    };
                    if completed
                        .send(SaveCompletion {
                            scope: job.scope,
                            owner_epoch: job.owner_epoch,
                            generation: job.generation,
                            snapshot: job.snapshot,
                            error,
                            path: job.path,
                        })
                        .is_err()
                    {
                        break;
                    }
                    wake();
                }
            })?;
        let workspace_disk = workspace
            .as_ref()
            .zip(self.workspace.as_ref())
            .map(|_| DiskVersion::Unread);
        let storage = Storage {
            user,
            workspace,
            sender,
            receiver,
            active: None,
            pending: VecDeque::new(),
            user_disk: DiskVersion::Unread,
            workspace_disk,
            user_epoch: 1,
            workspace_epoch: 1,
        };
        storage.request_baseline(Scope::User, storage.user_epoch, storage.user.clone(), None);
        if let (Some(path), Some(_)) = (&storage.workspace, &storage.workspace_disk) {
            storage.request_baseline(Scope::Workspace, storage.workspace_epoch, path.clone(), None);
        }
        self.storage = Some(storage);
        Ok(())
    }
    pub fn saving(&self) -> bool {
        self.storage
            .as_ref()
            .is_some_and(|storage| storage.active.is_some() || !storage.pending.is_empty())
    }
    pub fn retry_save(&mut self) {
        self.queue_save();
    }
    pub(super) fn queue_save(&mut self) {
        self.queue_scope(self.scope);
    }
    pub(super) fn queue_scope(&mut self, scope: Scope) {
        let Some(editor) = self.editor_for_scope_mut(scope) else {
            return;
        };
        editor.status = SaveStatus::Pending;
        let snapshot = editor.document.clone();
        let generation = editor.generation();
        let Some(storage) = self.storage.as_ref() else {
            self.editor_for_scope_mut(scope).unwrap().status =
                SaveStatus::Failed("Settings storage is not configured".into());
            return;
        };
        let path = if scope == Scope::Workspace {
            storage.workspace.clone()
        } else {
            Some(storage.user.clone())
        };
        let Some(path) = path else {
            self.editor_for_scope_mut(scope).unwrap().status =
                SaveStatus::Failed("Workspace settings path is unavailable".into());
            return;
        };
        let expected = if scope == Scope::Workspace {
            storage.workspace_disk.clone().unwrap_or(DiskVersion::Absent)
        } else {
            storage.user_disk.clone()
        };
        let owner_epoch = if scope == Scope::Workspace {
            storage.workspace_epoch
        } else {
            storage.user_epoch
        };
        let storage = self.storage.as_mut().unwrap();
        storage.pending.retain(|job| job.scope != scope);
        storage.pending.push_back(SaveJob {
            scope,
            owner_epoch,
            generation,
            snapshot,
            path,
            expected,
        });
        self.start_save();
    }
    fn start_save(&mut self) {
        // Preserve the pending decision owner. Starting another scope here could
        // discover a second conflict and replace the first actionable choice.
        if self.external_change.is_some() {
            return;
        }
        if let Some(storage) = self.storage.as_mut() {
            if storage.active.is_none() {
                while let Some(job) = storage.pending.pop_front() {
                    let current = if job.scope == Scope::Workspace {
                        storage.workspace.as_ref() == Some(&job.path) && storage.workspace_epoch == job.owner_epoch
                    } else {
                        storage.user == job.path && storage.user_epoch == job.owner_epoch
                    };
                    if !current {
                        continue;
                    }
                    let owner = SaveOwner {
                        scope: job.scope,
                        owner_epoch: job.owner_epoch,
                        path: job.path.clone(),
                    };
                    match storage.sender.send(StorageRequest::Save(Box::new(job))) {
                        Ok(()) => storage.active = Some(owner),
                        Err(error) => {
                            self.error = Some(format!("Settings worker unavailable: {error}"));
                        }
                    }
                    break;
                }
            }
        }
    }
    /// Dispatch the next queued save after a conflict decision. `start_save`
    /// keeps its name and privacy so the queue stays owned by this module.
    pub(super) fn resume_saves(&mut self) {
        self.start_save();
    }
    pub fn poll(&mut self) -> bool {
        let completion = self
            .storage
            .as_mut()
            .and_then(|storage| storage.receiver.try_recv().ok());
        let Some(completion) = completion else {
            return false;
        };
        let SaveCompletion {
            scope,
            owner_epoch,
            generation,
            snapshot,
            error,
            path,
        } = completion;
        if let Some(storage) = self.storage.as_mut() {
            storage.active = None;
        }
        let same_destination = self.storage.as_ref().is_some_and(|storage| {
            if scope == Scope::Workspace {
                storage.workspace.as_ref() == Some(&path) && storage.workspace_epoch == owner_epoch
            } else {
                storage.user == path && storage.user_epoch == owner_epoch
            }
        });
        if !same_destination {
            self.start_save();
            return true;
        }
        let completed_owner = SaveOwner {
            scope,
            owner_epoch,
            path: path.clone(),
        };
        let mut successful_snapshot = None;
        match error {
            None => {
                let disk = DiskVersion::Bytes(snapshot.to_toml().into_bytes());
                if let Some(storage) = &mut self.storage {
                    if scope == Scope::Workspace {
                        storage.workspace_disk = Some(disk.clone());
                    } else {
                        storage.user_disk = disk.clone();
                    }
                    for queued in storage
                        .pending
                        .iter_mut()
                        .filter(|job| job.scope == scope && job.owner_epoch == owner_epoch)
                    {
                        queued.expected = disk.clone();
                    }
                }
                if let Some(editor) = self.editor_for_scope_mut(scope) {
                    editor.acknowledge_saved(generation, snapshot.clone());
                }
                successful_snapshot = Some(snapshot);
            }
            Some(SaveFailure::Io(error)) => {
                if let Some(editor) = self.editor_for_scope_mut(scope)
                    && editor.generation() == generation
                    && editor.document.to_toml() == snapshot.to_toml()
                {
                    editor.status = SaveStatus::Failed(error);
                    self.error = None;
                }
            }
            Some(SaveFailure::ExternalChange(disk)) => {
                if let Some(editor) = self.editor_for_scope_mut(scope) {
                    editor.status =
                        SaveStatus::Failed("Settings changed on disk; choose Reload disk or Keep my settings".into());
                    self.external_change = Some(ExternalChange {
                        scope,
                        owner_epoch,
                        path,
                        disk,
                    });
                    self.error = None;
                }
            }
        }
        if let Some(snapshot) = successful_snapshot {
            self.apply_deferred_workspace(&completed_owner, snapshot);
        }
        self.start_save();
        true
    }
}

#[cfg(test)]
mod revert_contract_tests {
    use super::*;
    use bareline_platform::FileIdentity;
    use bareline_renderer_recording::RecordingBackend;
    use std::{
        fs, io,
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let path = std::env::temp_dir().join(format!(
                "bareline-settings-revert-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct TestPlatform {
        fail: AtomicBool,
    }
    impl LocalFileSystem for TestPlatform {
        fn identity(&self, _: &fs::File) -> io::Result<FileIdentity> {
            Err(io::ErrorKind::Unsupported.into())
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(io::Error::other("injected settings write failure"));
            }
            match fs::remove_file(target) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            fs::rename(staged, target)
        }
    }
    struct GatedPlatform {
        entered: mpsc::SyncSender<()>,
        release: std::sync::Mutex<mpsc::Receiver<()>>,
        fail: AtomicBool,
    }
    impl LocalFileSystem for GatedPlatform {
        fn identity(&self, _: &fs::File) -> io::Result<FileIdentity> {
            Err(io::ErrorKind::Unsupported.into())
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            if self.fail.load(Ordering::SeqCst) {
                return Err(io::Error::other("injected gated write failure"));
            }
            match fs::remove_file(target) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            fs::rename(staged, target)
        }
    }

    fn settle(controller: &mut SettingsController, done: impl Fn(&SettingsController) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !done(controller) {
            controller.poll();
            assert!(
                Instant::now() < deadline,
                "settings worker did not reach the expected state"
            );
            std::thread::yield_now();
        }
    }
    fn wait_for_gate(controller: &mut SettingsController, entered: &mpsc::Receiver<()>) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            controller.poll();
            match entered.try_recv() {
                Ok(()) => return,
                Err(mpsc::TryRecvError::Empty) => {}
                Err(error) => panic!("settings gate disconnected: {error}"),
            }
            assert!(
                Instant::now() < deadline,
                "settings worker did not enter the commit gate"
            );
            std::thread::yield_now();
        }
    }

    fn document(scope: Scope, size: f64) -> SettingsDocument {
        let mut document = SettingsDocument::empty(scope);
        document.set("editor.font.size", SettingValue::Number(size)).unwrap();
        document
    }

    fn font_size(controller: &SettingsController, scope: Scope) -> f64 {
        let editor = controller.editor_for_scope(scope).unwrap();
        config::resolve(&editor.document, None, false, None)
            .values
            .editor_font_size_pt
    }

    /// The save jobs a test worker receives; baseline reads are the real worker's
    /// own business and are skipped here.
    struct SaveJobs(mpsc::Receiver<StorageRequest>);
    impl SaveJobs {
        fn recv(&self) -> Result<SaveJob, mpsc::RecvError> {
            loop {
                if let StorageRequest::Save(job) = self.0.recv()? {
                    return Ok(*job);
                }
            }
        }
    }

    fn attach_storage(
        controller: &mut SettingsController,
        path: PathBuf,
    ) -> (SaveJobs, mpsc::SyncSender<SaveCompletion>) {
        let (jobs, job_receiver) = mpsc::channel();
        let job_receiver = SaveJobs(job_receiver);
        let (completed, completions) = mpsc::sync_channel(4);
        controller.storage = Some(Storage {
            user: path,
            workspace: None,
            sender: jobs,
            receiver: completions,
            active: None,
            pending: VecDeque::new(),
            user_disk: DiskVersion::Absent,
            workspace_disk: None,
            user_epoch: 1,
            workspace_epoch: 1,
        });
        (job_receiver, completed)
    }

    fn complete(completed: &mpsc::SyncSender<SaveCompletion>, job: SaveJob, error: Option<SaveFailure>) {
        completed
            .send(SaveCompletion {
                scope: job.scope,
                owner_epoch: job.owner_epoch,
                generation: job.generation,
                snapshot: job.snapshot,
                error,
                path: job.path,
            })
            .unwrap();
    }

    #[test]
    fn autosave_then_revert_restores_opening_value_and_fences_late_acknowledgement() {
        let mut controller = SettingsController::new(document(Scope::User, 11.0), None, SystemAppearance::default());
        let (jobs, completed) = attach_storage(&mut controller, PathBuf::from("user-settings.toml"));
        controller.show();
        controller.edit("editor.font.size", SettingValue::Number(12.0)).unwrap();
        let first = jobs.recv().unwrap();
        let revision = controller.revision;
        assert!(
            !controller.reconcile_user_document(document(Scope::User, 14.0), revision),
            "an active User write must defer migration reconciliation"
        );
        complete(&completed, first, None);
        assert!(controller.poll());
        assert_eq!(controller.user.status, SaveStatus::Saved);
        assert!(controller.can_revert());
        controller.revert_changes();
        let autosave_revert = jobs.recv().unwrap();
        complete(
            &completed,
            autosave_revert,
            Some(SaveFailure::Io("injected disk full".into())),
        );
        assert!(controller.poll());
        assert!(matches!(controller.user.status, SaveStatus::Failed(_)));
        controller.retry_save();
        let autosave_revert_retry = jobs.recv().unwrap();
        complete(&completed, autosave_revert_retry, None);
        assert!(controller.poll());
        assert_eq!(font_size(&controller, Scope::User), 11.0);
        assert!(!controller.can_revert());

        controller.edit("editor.font.size", SettingValue::Number(20.0)).unwrap();
        let superseded = jobs.recv().unwrap();
        controller.revert_changes();
        assert_eq!(font_size(&controller, Scope::User), 11.0);

        complete(&completed, superseded, None);
        assert!(controller.poll());
        assert!(matches!(controller.user.status, SaveStatus::Pending));
        assert!(controller.user.changed_from_saved());
        assert_eq!(font_size(&controller, Scope::User), 11.0);
        let reverted = jobs.recv().unwrap();
        assert_eq!(
            config::resolve(&reverted.snapshot, None, false, None)
                .values
                .editor_font_size_pt,
            11.0
        );
        complete(&completed, reverted, None);
        assert!(controller.poll());
        assert_eq!(controller.user.status, SaveStatus::Saved);
        assert!(!controller.user.changed_from_saved());
        assert!(!controller.can_revert());
    }

    #[test]
    fn reset_scope_switch_and_reopen_keep_independent_opening_baselines() {
        let user = document(Scope::User, 11.0);
        let workspace = document(Scope::Workspace, 13.0);
        let mut controller = SettingsController::new(user, Some(workspace), SystemAppearance::default());
        controller.show();
        controller.set_workspace_opt_in(true).unwrap();
        assert_eq!(controller.effective().editor_font_size_pt, 13.0);
        controller.revert_changes();
        assert!(!controller.workspace_opted_in);
        assert_eq!(controller.effective().editor_font_size_pt, 11.0);
        controller.request_reset();
        controller.confirm_reset(true);
        assert!(controller.can_revert());
        controller.revert_changes();
        assert_eq!(font_size(&controller, Scope::User), 11.0);

        controller.scope = Scope::Workspace;
        controller.edit("editor.font.size", SettingValue::Number(18.0)).unwrap();
        assert!(controller.can_revert());
        controller.scope = Scope::User;
        assert!(!controller.can_revert());
        controller.scope = Scope::Workspace;
        controller.dismiss();
        controller.show();
        assert!(
            !controller.can_revert(),
            "reopening must capture a fresh workspace baseline"
        );
        controller.scope = Scope::User;
        assert!(!controller.can_revert(), "the user baseline remains independent");
    }

    #[test]
    fn external_change_requires_reload_or_keep_and_preserves_opening_revert() {
        let mut controller = SettingsController::new(document(Scope::User, 11.0), None, SystemAppearance::default());
        let (jobs, completed) = attach_storage(&mut controller, PathBuf::from("user-settings.toml"));
        controller.show();
        controller.edit("editor.font.size", SettingValue::Number(12.0)).unwrap();
        let conflicting = jobs.recv().unwrap();
        let disk = document(Scope::User, 14.0);
        complete(
            &completed,
            conflicting,
            Some(SaveFailure::ExternalChange(DiskVersion::Bytes(
                disk.to_toml().into_bytes(),
            ))),
        );
        assert!(controller.poll());
        assert!(controller.external_change.is_some());
        controller
            .draw(
                rect(0.0, 34.0, 520.0, 420.0),
                &mut RecordingBackend::default(),
                &mut Vec::new(),
            )
            .unwrap();
        assert!(
            controller
                .rows
                .iter()
                .all(|row| { row.value.bounds.y + row.value.bounds.height <= controller.external_reload.y })
        );
        let decisions = controller.semantics();
        assert!(
            decisions
                .iter()
                .any(|node| node.id == ViewId(8013) && node.name.contains("Reload User settings"))
        );
        assert!(
            decisions
                .iter()
                .any(|node| node.id == ViewId(8014) && node.name.contains("Keep my User settings"))
        );
        controller.query_focused = false;
        controller.refresh_focus();
        controller.focus.focus(ViewId(8013));
        assert_eq!(
            controller.event(UiEvent::Key(Key::Enter)),
            Some(SettingsEffect::PreviewChanged)
        );
        assert_eq!(font_size(&controller, Scope::User), 14.0);
        assert!(controller.can_revert(), "reload must not redefine the opening baseline");
        controller.revert_changes();
        let reverted = jobs.recv().unwrap();
        assert_eq!(reverted.expected, DiskVersion::Bytes(disk.to_toml().into_bytes()));
        assert_eq!(
            config::resolve(&reverted.snapshot, None, false, None)
                .values
                .editor_font_size_pt,
            11.0
        );

        complete(&completed, reverted, None);
        assert!(controller.poll());
        controller.edit("editor.font.size", SettingValue::Number(16.0)).unwrap();
        let conflicting = jobs.recv().unwrap();
        let later_disk = document(Scope::User, 15.0);
        complete(
            &completed,
            conflicting,
            Some(SaveFailure::ExternalChange(DiskVersion::Bytes(
                later_disk.to_toml().into_bytes(),
            ))),
        );
        assert!(controller.poll());
        assert!(controller.keep_after_external_change());
        let kept = jobs.recv().unwrap();
        assert_eq!(kept.expected, DiskVersion::Bytes(later_disk.to_toml().into_bytes()));
        assert_eq!(
            config::resolve(&kept.snapshot, None, false, None)
                .values
                .editor_font_size_pt,
            16.0
        );
    }

    #[test]
    fn real_worker_refreshes_queued_authority_for_rapid_edit_and_revert() {
        let fixture = Fixture::new();
        let path = fixture.0.join("settings.toml");
        let opening = document(Scope::User, 11.0);
        fs::write(&path, opening.to_toml()).unwrap();
        let platform = Arc::new(TestPlatform {
            fail: AtomicBool::new(false),
        });
        let mut controller = SettingsController::new(opening, None, SystemAppearance::default());
        controller
            .configure_storage(path.clone(), None, platform, Arc::new(|| {}))
            .unwrap();
        controller.show();
        controller.edit("editor.font.size", SettingValue::Number(12.0)).unwrap();
        controller.edit("editor.font.size", SettingValue::Number(20.0)).unwrap();
        controller.revert_changes();
        settle(&mut controller, |controller| {
            !controller.saving() && controller.user.status == SaveStatus::Saved
        });
        assert!(!controller.has_external_change());
        let saved = SettingsDocument::load(&path, Scope::User).unwrap();
        assert_eq!(
            config::resolve(&saved, None, false, None).values.editor_font_size_pt,
            11.0
        );
    }

    #[test]
    fn real_worker_failure_retry_and_external_edit_keep_truthful_ownership() {
        let fixture = Fixture::new();
        let path = fixture.0.join("settings.toml");
        let opening = document(Scope::User, 11.0);
        fs::write(&path, opening.to_toml()).unwrap();
        let platform = Arc::new(TestPlatform {
            fail: AtomicBool::new(true),
        });
        let mut controller = SettingsController::new(opening, None, SystemAppearance::default());
        controller
            .configure_storage(path.clone(), None, platform.clone(), Arc::new(|| {}))
            .unwrap();
        controller.show();
        controller.edit("editor.font.size", SettingValue::Number(12.0)).unwrap();
        settle(&mut controller, |controller| {
            matches!(controller.user.status, SaveStatus::Failed(_))
        });
        assert!(controller.can_revert());
        platform.fail.store(false, Ordering::SeqCst);
        controller.retry_save();
        settle(&mut controller, |controller| {
            controller.user.status == SaveStatus::Saved
        });

        let external = document(Scope::User, 14.0);
        fs::write(&path, external.to_toml()).unwrap();
        controller.edit("editor.font.size", SettingValue::Number(16.0)).unwrap();
        settle(&mut controller, SettingsController::has_external_change);
        assert_eq!(font_size(&controller, Scope::User), 16.0);
        assert!(controller.reload_external_change());
        assert_eq!(font_size(&controller, Scope::User), 14.0);
        assert!(
            controller.can_revert(),
            "external reload must retain the opening baseline"
        );
    }

    #[test]
    fn workspace_replacement_invalidates_the_old_path_decision() {
        let workspace = document(Scope::Workspace, 12.0);
        let mut controller = SettingsController::new(
            document(Scope::User, 11.0),
            Some(workspace),
            SystemAppearance::default(),
        );
        let old_path = PathBuf::from("workspace-a/settings.toml");
        let (jobs, completed) = attach_storage(&mut controller, PathBuf::from("user-settings.toml"));
        let storage = controller.storage.as_mut().unwrap();
        storage.workspace = Some(old_path.clone());
        storage.workspace_disk = Some(DiskVersion::Absent);
        controller.show();
        controller.scope = Scope::Workspace;
        controller.edit("editor.font.size", SettingValue::Number(18.0)).unwrap();
        let old_job = jobs.recv().unwrap();
        complete(
            &completed,
            old_job,
            Some(SaveFailure::ExternalChange(DiskVersion::Bytes(
                document(Scope::Workspace, 15.0).to_toml().into_bytes(),
            ))),
        );
        assert!(controller.poll());
        assert!(controller.has_external_change());

        let new_path = PathBuf::from("workspace-b/settings.toml");
        controller.set_workspace_document(new_path, document(Scope::Workspace, 13.0));
        assert!(!controller.has_external_change());
        assert!(!controller.reload_external_change());
        assert_eq!(font_size(&controller, Scope::Workspace), 13.0);
    }

    #[test]
    fn same_path_workspace_handoff_waits_for_the_accepted_commit() {
        let fixture = Fixture::new();
        let user_path = fixture.0.join("user.toml");
        let workspace_path = fixture.0.join("workspace.toml");
        let user = document(Scope::User, 11.0);
        let workspace = document(Scope::Workspace, 12.0);
        fs::write(&user_path, user.to_toml()).unwrap();
        fs::write(&workspace_path, workspace.to_toml()).unwrap();
        let (entered, entered_rx) = mpsc::sync_channel(1);
        let (release, release_rx) = mpsc::sync_channel(1);
        let platform = Arc::new(GatedPlatform {
            entered,
            release: std::sync::Mutex::new(release_rx),
            fail: AtomicBool::new(false),
        });
        let mut controller = SettingsController::new(user, Some(workspace), SystemAppearance::default());
        controller
            .configure_storage(user_path, Some(workspace_path.clone()), platform, Arc::new(|| {}))
            .unwrap();
        controller.show();
        controller.scope = Scope::Workspace;
        controller.edit("editor.font.size", SettingValue::Number(18.0)).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        controller.edit("editor.font.size", SettingValue::Number(20.0)).unwrap();

        controller.set_workspace_document(workspace_path.clone(), document(Scope::Workspace, 13.0));
        assert!(controller.deferred_workspace.is_some());
        assert!(controller.edit("editor.font.size", SettingValue::Number(21.0)).is_err());
        assert_eq!(font_size(&controller, Scope::Workspace), 20.0);
        release.send(()).unwrap();
        wait_for_gate(&mut controller, &entered_rx);
        assert!(controller.deferred_workspace.is_some());
        release.send(()).unwrap();
        settle(&mut controller, |controller| {
            !controller.saving() && controller.deferred_workspace.is_none()
        });
        assert_eq!(font_size(&controller, Scope::Workspace), 20.0);
        let disk = SettingsDocument::load(&workspace_path, Scope::Workspace).unwrap();
        assert_eq!(
            config::resolve(&disk, None, false, None).values.editor_font_size_pt,
            20.0
        );
    }

    #[test]
    fn newer_workspace_owner_cancels_an_older_deferred_handoff() {
        let fixture = Fixture::new();
        let user_path = fixture.0.join("user.toml");
        let path_a = fixture.0.join("workspace-a.toml");
        let path_b = fixture.0.join("workspace-b.toml");
        let user = document(Scope::User, 11.0);
        let workspace = document(Scope::Workspace, 12.0);
        fs::write(&user_path, user.to_toml()).unwrap();
        fs::write(&path_a, workspace.to_toml()).unwrap();
        fs::write(&path_b, document(Scope::Workspace, 14.0).to_toml()).unwrap();
        let (entered, entered_rx) = mpsc::sync_channel(1);
        let (release, release_rx) = mpsc::sync_channel(1);
        let platform = Arc::new(GatedPlatform {
            entered,
            release: std::sync::Mutex::new(release_rx),
            fail: AtomicBool::new(false),
        });
        let mut controller = SettingsController::new(user, Some(workspace), SystemAppearance::default());
        controller
            .configure_storage(user_path, Some(path_a.clone()), platform, Arc::new(|| {}))
            .unwrap();
        controller.show();
        controller.scope = Scope::Workspace;
        controller.edit("editor.font.size", SettingValue::Number(18.0)).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        controller.set_workspace_document(path_a.clone(), document(Scope::Workspace, 13.0));
        assert!(controller.deferred_workspace.is_some());
        controller.set_workspace_document(path_b.clone(), document(Scope::Workspace, 14.0));
        assert!(controller.deferred_workspace.is_none());
        release.send(()).unwrap();
        settle(&mut controller, |controller| !controller.saving());
        assert_eq!(controller.storage.as_ref().unwrap().workspace.as_ref(), Some(&path_b));
        assert_eq!(font_size(&controller, Scope::Workspace), 14.0);

        controller.edit("editor.font.size", SettingValue::Number(16.0)).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        release.send(()).unwrap();
        settle(&mut controller, |controller| {
            !controller.saving() && controller.current().status == SaveStatus::Saved
        });
        assert_eq!(font_size(&controller, Scope::Workspace), 16.0);
        assert_eq!(
            config::resolve(
                &SettingsDocument::load(&path_b, Scope::Workspace).unwrap(),
                None,
                false,
                None,
            )
            .values
            .editor_font_size_pt,
            16.0
        );
    }

    #[test]
    fn failed_handoff_keeps_error_and_retries_before_installing_new_owner() {
        let fixture = Fixture::new();
        let user_path = fixture.0.join("user.toml");
        let workspace_path = fixture.0.join("workspace.toml");
        let user = document(Scope::User, 11.0);
        let workspace = document(Scope::Workspace, 12.0);
        fs::write(&user_path, user.to_toml()).unwrap();
        fs::write(&workspace_path, workspace.to_toml()).unwrap();
        let (entered, entered_rx) = mpsc::sync_channel(1);
        let (release, release_rx) = mpsc::sync_channel(1);
        let platform = Arc::new(GatedPlatform {
            entered,
            release: std::sync::Mutex::new(release_rx),
            fail: AtomicBool::new(true),
        });
        let mut controller = SettingsController::new(user, Some(workspace), SystemAppearance::default());
        controller
            .configure_storage(
                user_path,
                Some(workspace_path.clone()),
                platform.clone(),
                Arc::new(|| {}),
            )
            .unwrap();
        controller.show();
        controller.scope = Scope::Workspace;
        controller.edit("editor.font.size", SettingValue::Number(18.0)).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        controller.set_workspace_document(workspace_path.clone(), document(Scope::Workspace, 13.0));
        release.send(()).unwrap();
        settle(&mut controller, |controller| {
            matches!(controller.current().status, SaveStatus::Failed(_))
        });
        assert!(controller.deferred_workspace.is_some());
        assert_eq!(font_size(&controller, Scope::Workspace), 18.0);
        assert_eq!(
            config::resolve(
                &SettingsDocument::load(&workspace_path, Scope::Workspace).unwrap(),
                None,
                false,
                None,
            )
            .values
            .editor_font_size_pt,
            12.0
        );

        platform.fail.store(false, Ordering::SeqCst);
        controller.retry_save();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        release.send(()).unwrap();
        settle(&mut controller, |controller| {
            !controller.saving() && controller.deferred_workspace.is_none()
        });
        assert_eq!(font_size(&controller, Scope::Workspace), 18.0);
        assert_eq!(controller.current().status, SaveStatus::Saved);
    }

    /// APP-19: configuring storage reads nothing on the calling thread. An
    /// unreadable settings path used to fail right here, on the UI thread; the
    /// worker now reads it and reports the problem on the first save instead.
    #[test]
    fn configure_storage_leaves_the_settings_read_to_the_worker() {
        let fixture = Fixture::new();
        let unreadable = fixture.0.join("settings.toml");
        fs::create_dir(&unreadable).unwrap();
        let platform = Arc::new(TestPlatform {
            fail: AtomicBool::new(false),
        });
        let mut controller = SettingsController::new(document(Scope::User, 11.0), None, SystemAppearance::default());
        controller
            .configure_storage(unreadable, None, platform, Arc::new(|| {}))
            .expect("no settings read happens before the worker runs");
        controller.show();
        controller.edit("editor.font.size", SettingValue::Number(12.0)).unwrap();
        settle(&mut controller, |controller| {
            matches!(controller.user.status, SaveStatus::Failed(_))
        });
    }

    /// APP-19: the worker-read baseline still protects an external edit made
    /// after a save, and a migrated document gets a fresh worker-read baseline.
    #[test]
    fn worker_baseline_detects_external_edits_after_reconciliation() {
        let fixture = Fixture::new();
        let path = fixture.0.join("settings.toml");
        let opening = document(Scope::User, 11.0);
        fs::write(&path, opening.to_toml()).unwrap();
        let platform = Arc::new(TestPlatform {
            fail: AtomicBool::new(false),
        });
        let mut controller = SettingsController::new(opening, None, SystemAppearance::default());
        controller
            .configure_storage(path.clone(), None, platform, Arc::new(|| {}))
            .unwrap();
        let migrated = document(Scope::User, 13.0);
        fs::write(&path, migrated.to_toml()).unwrap();
        let revision = controller.revision;
        assert!(controller.reconcile_user_document(migrated, revision));
        controller.show();
        controller.edit("editor.font.size", SettingValue::Number(15.0)).unwrap();
        settle(&mut controller, |controller| {
            !controller.saving() && controller.user.status == SaveStatus::Saved
        });
        assert!(!controller.has_external_change());
        fs::write(&path, document(Scope::User, 17.0).to_toml()).unwrap();
        controller.edit("editor.font.size", SettingValue::Number(16.0)).unwrap();
        settle(&mut controller, SettingsController::has_external_change);
    }
}
