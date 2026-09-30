// SPDX-License-Identifier: MPL-2.0
//! Explicit native update controller. No check is scheduled automatically.
use bareline_platform_windows::update as native;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};

#[derive(Default)]
pub(super) struct UpdateRuntime {
    worker: Option<Receiver<Result<(), String>>>,
    worker_kind: WorkerKind,
    cancel: Arc<AtomicBool>,
    pub status: String,
    pub ready: bool,
    apply_on_exit: bool,
    rollback_on_exit: bool,
    acknowledged: bool,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum WorkerKind {
    #[default]
    Check,
    Discard,
    Rollback,
}
struct Config {
    key: &'static str,
    /// `trust.publisher`, the identity in every signed manifest (SEC-01).
    publisher: &'static str,
    /// Compiled Authenticode pin; a signed release authority may rotate it (SEC-08).
    signer: bareline_distribution::update::PublisherPin,
    channel: &'static str,
    floor: u64,
    offline_policy: native::OfflineRootPolicy<'static>,
    host: &'static str,
    manifest: &'static str,
    signature: &'static str,
    artifact: &'static str,
}
impl Config {
    fn compiled() -> Result<Self, String> {
        match env!("BARELINE_BUILD_MODE") {
            "configured" => (),
            "preview" => {
                return Err(
                    "Updates are disabled in this unsigned preview build; no public release configuration was compiled"
                        .into(),
                );
            }
            // Fixture builds compile public private-seed keys and never update (SEC-18).
            _ => return Err("Updates are disabled in this nonshipping fixture build".into()),
        }
        let signer = bareline_distribution::update::PublisherPin::parse(
            env!("BARELINE_AUTHENTICODE_SUBJECT"),
            env!("BARELINE_AUTHENTICODE_ISSUERS"),
        )
        .map_err(|_| "Invalid publisher configuration")?;
        Ok(Self {
            key: env!("BARELINE_RELEASE_PUBLIC_KEY"),
            publisher: env!("BARELINE_PUBLISHER"),
            signer,
            channel: env!("BARELINE_RELEASE_CHANNEL"),
            floor: env!("BARELINE_METADATA_FLOOR")
                .parse()
                .map_err(|_| "Invalid metadata floor")?,
            offline_policy: native::OfflineRootPolicy {
                public_key: env!("BARELINE_OFFLINE_ROOT_PUBLIC_KEY"),
                minimum_version: env!("BARELINE_ROOT_VERSION_FLOOR")
                    .parse()
                    .map_err(|_| "Invalid offline root floor")?,
            },
            host: env!("BARELINE_UPDATE_HOST"),
            manifest: env!("BARELINE_UPDATE_MANIFEST_PATH"),
            signature: env!("BARELINE_UPDATE_SIGNATURE_PATH"),
            artifact: env!("BARELINE_UPDATE_ARTIFACT_PATH"),
        })
    }
}
/// Whether this build carries a release update configuration. The unsigned
/// preview does not, so its update commands stay hidden.
pub(super) fn available() -> bool {
    Config::compiled().is_ok()
}
fn installation() -> Result<std::path::PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let root = executable.parent().ok_or("Missing installation directory")?.to_owned();
    native::validate_install_root(&root).map_err(|e| e.to_string())?;
    Ok(root)
}
/// The installation (read only) and the per-user update state beside it (SEC-04).
fn locations() -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let root = installation()?;
    let state = native::update_state_root(&root).map_err(|e| format!("Update state: {e}"))?;
    Ok((root, state))
}
fn now() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|_| "Clock unavailable".into())
}
impl Config {
    fn authority(
        &self,
        root: &std::path::Path,
        state: &std::path::Path,
        freshness: native::AuthorityFreshness,
    ) -> Result<native::ResolvedReleaseAuthority, String> {
        native::resolve_release_authority(
            root,
            state,
            self.key,
            &self.signer,
            self.floor,
            Some(self.offline_policy),
            freshness,
            now()?,
        )
        .map_err(|e| e.to_string())
    }
}
/// What [`startup_recovery`] decided for this launch.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StartupRecovery {
    /// Start normally.
    Continue,
    /// The helper restores the previous build and starts it: this process must exit.
    HandedOff,
    /// Automatic rollback was attempted and failed: start normally and say so.
    Failed,
}
pub(super) const AUTOMATIC_ROLLBACK_FAILED: &str = "Automatic rollback failed: the updated version kept failing to start and the previous version could not be restored. Try Roll Back Last Update, or reinstall Bareline with its installer.";
/// Count this launch of a freshly updated build that has not yet reached a healthy
/// frame (SEC-09). After [`native::UPDATE_LAUNCH_ATTEMPTS`] such launches the helper
/// restores the build the update replaced and starts it again. That is attempted once:
/// later launches, and this one if the handoff fails, start normally and report it. Any
/// other failure here leaves the launch alone.
pub(super) fn startup_recovery() -> StartupRecovery {
    let Ok(config) = Config::compiled() else {
        return StartupRecovery::Continue;
    };
    let Ok((root, state)) = locations() else {
        return StartupRecovery::Continue;
    };
    match native::record_update_launch(&root, &state, native::UPDATE_LAUNCH_ATTEMPTS) {
        Ok(native::LaunchDecision::Recover) => {}
        Ok(native::LaunchDecision::RecoveryFailed) => return StartupRecovery::Failed,
        _ => return StartupRecovery::Continue,
    }
    let handed_off = config
        .authority(&root, &state, native::AuthorityFreshness::Installed)
        .and_then(|authority| {
            native::launch_update_helper(&root, &authority, native::HelperAction::AutoRecover)
                .map_err(|e| e.to_string())
        })
        .is_ok();
    if handed_off {
        StartupRecovery::HandedOff
    } else {
        StartupRecovery::Failed
    }
}
impl UpdateRuntime {
    pub fn check(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
        if self.worker.is_some() || self.ready {
            return;
        }
        let config = match Config::compiled() {
            Ok(c) => c,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        self.cancel = Arc::new(AtomicBool::new(false));
        self.worker_kind = WorkerKind::Check;
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let spawn = std::thread::Builder::new()
            .name("bareline-update-check".into())
            .spawn(move || {
                let result = (|| {
                    let (root, state) = locations()?;
                    let authority = config.authority(&root, &state, native::AuthorityFreshness::Required)?;
                    // The core executable's own floor and ledger (SEC-03).
                    let floor = native::core_metadata_floor(&root, &state, authority.minimum_metadata_version)
                        .map_err(|e| format!("Version ledger: {e}"))?;
                    let policy = bareline_distribution::update::core_update_policy(
                        &authority.release_public_key,
                        config.publisher,
                        config.channel,
                        floor,
                    );
                    let now = now()?;
                    let prepared = native::fetch_verified_update(
                        config.host,
                        config.manifest,
                        config.signature,
                        config.artifact,
                        &policy,
                        now,
                        &authority.signer,
                        &std::env::temp_dir(),
                        &cancel,
                    )
                    .map_err(|e| format!("Update verification: {e:?}"))?;
                    // Trust state delivered with the update is verified before staging (SEC-02).
                    // The held files are released at once; the helper reverifies them.
                    let delivered = native::verify_delivered_trust(
                        &prepared.directory,
                        &root,
                        &state,
                        prepared.manifest.metadata(),
                        config.offline_policy,
                        &prepared.file,
                        now,
                    )
                    .map(drop);
                    if let Err(error) = delivered {
                        native::discard_prepared_update(prepared);
                        return Err(format!("Update trust: {error}"));
                    }
                    if cancel.load(Ordering::Acquire) {
                        native::discard_prepared_update(prepared);
                        return Err("Update cancelled".into());
                    }
                    native::transfer_update(prepared, &root, &state).map_err(|e| {
                        if e.kind() == std::io::ErrorKind::PermissionDenied {
                            // A per-machine installation is read-only for the user (SEC-04).
                            "This installation is shared by all users; install the new version with its installer."
                                .into()
                        } else {
                            format!("Update staging: {e}")
                        }
                    })
                })();
                let _ = tx.send(result);
                notify();
            });
        match spawn {
            Ok(_) => {
                self.worker = Some(rx);
                self.status = "Checking for an update…".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    pub fn poll(&mut self) {
        if let Some(result) = self.worker.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.worker = None;
            match (result, self.worker_kind) {
                (Ok(()), WorkerKind::Check) => {
                    self.ready = true;
                    self.status = "Verified update ready. Choose Apply Update on Exit.".into();
                }
                (Ok(()), WorkerKind::Discard) => {
                    self.status = "Unapplied staging retained in update history. A fresh check is available.".into();
                }
                (Ok(()), WorkerKind::Rollback) => {
                    self.rollback_on_exit = true;
                    self.apply_on_exit = false;
                    self.status = "The previous version will be restored after the editor closes.".into();
                }
                (Err(e), _) => self.status = e,
            }
        }
    }
    pub fn apply_on_exit(&mut self) {
        if self.ready {
            self.apply_on_exit = true;
            self.rollback_on_exit = false;
            self.status = "Update will apply after the editor closes.".into();
        }
    }
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.apply_on_exit = false;
        self.rollback_on_exit = false;
    }
    /// "Roll back last update" (SEC-09): confirm off the UI thread that the journal or the
    /// applied-update ledger names the running build, then restore the build it replaced
    /// after the editor closes. The helper verifies everything again.
    pub fn rollback(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
        if self.worker.is_some() {
            self.status = "Wait for the current update operation.".into();
            return;
        }
        if let Err(e) = Config::compiled() {
            self.status = e;
            return;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("bareline-update-rollback".into())
            .spawn(move || {
                let result = locations().and_then(|(root, state)| {
                    match native::recovery_source(&root, &state).map_err(|e| e.to_string())? {
                        Some(_) => Ok(()),
                        None => Err("No applied update of this version can be rolled back.".into()),
                    }
                });
                let _ = tx.send(result);
                notify();
            }) {
            Ok(_) => {
                self.worker_kind = WorkerKind::Rollback;
                self.worker = Some(rx);
                self.status = "Checking the previous version…".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    pub fn discard(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
        self.cancel();
        if self.worker.is_some() {
            self.status = "Cancelling current check; retry Discard Pending Update after it stops.".into();
            return;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("bareline-update-discard".into())
            .spawn(move || {
                let result = locations()
                    .and_then(|(root, state)| native::discard_pending_update(&root, &state).map_err(|e| e.to_string()));
                let _ = tx.send(result);
                notify();
            }) {
            Ok(_) => {
                self.worker_kind = WorkerKind::Discard;
                self.worker = Some(rx);
                self.status = "Retaining unapplied staging…".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    /// Invoke only after the normal event loop has returned and dirty-close choices resolved.
    pub fn finish(&mut self) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        let (action, freshness) = if self.rollback_on_exit {
            (native::HelperAction::Recover, native::AuthorityFreshness::Installed)
        } else if self.apply_on_exit {
            (native::HelperAction::Apply, native::AuthorityFreshness::Required)
        } else {
            return Ok(());
        };
        let config = Config::compiled()?;
        let (root, state) = locations()?;
        let authority = config.authority(&root, &state, freshness)?;
        native::launch_update_helper(&root, &authority, action).map_err(|e| e.to_string())
    }
    /// Call after a successful ordinary frame, never during startup probes.
    pub fn healthy_frame(&mut self) {
        if self.acknowledged {
            return;
        }
        self.acknowledged = true;
        // Installed-state use: an expired authority never blocks the acknowledgement
        // that ends the failed-launch count (SEC-02, SEC-09).
        if let Ok(config) = Config::compiled() {
            let _ = std::thread::Builder::new()
                .name("bareline-update-ack".into())
                .spawn(move || {
                    if let Ok(root) = installation() {
                        if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                            && let Ok(authority) = native::resolve_release_authority(
                                &root,
                                &native::update_state_root(&root).ok()?,
                                config.key,
                                &config.signer,
                                config.floor,
                                Some(config.offline_policy),
                                native::AuthorityFreshness::Installed,
                                now.as_secs(),
                            )
                        {
                            let _ = native::launch_update_helper(&root, &authority, native::HelperAction::Acknowledge);
                        }
                    }
                    Some(())
                });
        }
    }
}
impl Drop for UpdateRuntime {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
pub(super) fn commands() -> Vec<bareline_commands::CommandSpec> {
    [
        ("update.check", "Check for Updates"),
        ("update.apply_on_exit", "Apply Update on Exit"),
        ("update.cancel", "Cancel Update"),
        ("update.discard", "Discard Pending Update"),
        ("update.rollback", "Roll Back Last Update"),
    ]
    .into_iter()
    .map(|(id, title)| bareline_commands::CommandSpec {
        id: bareline_commands::CommandId(id),
        title,
        category: "Help",
        shortcut: "",
        action: bareline_commands::Action::Contributed(bareline_commands::CommandId(id)),
    })
    .collect()
}
