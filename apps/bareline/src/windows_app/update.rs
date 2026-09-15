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
    worker_marks_ready: bool,
    cancel: Arc<AtomicBool>,
    pub status: String,
    pub ready: bool,
    apply_on_exit: bool,
    acknowledged: bool,
}
struct Config {
    key: &'static str,
    publisher: &'static str,
    channel: &'static str,
    floor: u64,
    host: &'static str,
    manifest: &'static str,
    signature: &'static str,
    artifact: &'static str,
}
impl Config {
    fn compiled() -> Result<Self, String> {
        if env!("BARELINE_BUILD_MODE") == "preview" {
            return Err(
                "Updates are disabled in this unsigned preview build; no public release configuration was compiled"
                    .into(),
            );
        }
        let publisher = env!("BARELINE_PUBLISHER_CERT_SHA256");
        if publisher.len() != 64 || !publisher.is_ascii() {
            return Err("Invalid publisher configuration".into());
        }
        Ok(Self {
            key: env!("BARELINE_RELEASE_PUBLIC_KEY"),
            publisher,
            channel: env!("BARELINE_RELEASE_CHANNEL"),
            floor: env!("BARELINE_METADATA_FLOOR")
                .parse()
                .map_err(|_| "Invalid metadata floor")?,
            host: env!("BARELINE_UPDATE_HOST"),
            manifest: env!("BARELINE_UPDATE_MANIFEST_PATH"),
            signature: env!("BARELINE_UPDATE_SIGNATURE_PATH"),
            artifact: env!("BARELINE_UPDATE_ARTIFACT_PATH"),
        })
    }
}
fn installation() -> Result<std::path::PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let root = executable.parent().ok_or("Missing installation directory")?.to_owned();
    native::validate_install_root(&root).map_err(|e| e.to_string())?;
    Ok(root)
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
        self.worker_marks_ready = true;
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let spawn = std::thread::Builder::new()
            .name("bareline-update-check".into())
            .spawn(move || {
                let result = (|| {
                    use std::io::Read;
                    let root = installation()?;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|_| "Clock unavailable")?
                        .as_secs();
                    let authority =
                        native::resolve_release_authority(&root, config.key, config.publisher, config.floor, now)
                            .map_err(|e| e.to_string())?;
                    let mut floor = authority.minimum_metadata_version;
                    let ledger = root.join("bareline.update-versions");
                    if ledger.try_exists().map_err(|e| e.to_string())? {
                        let mut bytes = Vec::new();
                        native::open_update_read_file(&ledger)
                            .map_err(|e| e.to_string())?
                            .take(65537)
                            .read_to_end(&mut bytes)
                            .map_err(|e| e.to_string())?;
                        if bytes.len() > 65536 {
                            return Err("Version ledger limit".into());
                        }
                        for line in std::str::from_utf8(&bytes)
                            .map_err(|_| "Invalid version ledger")?
                            .lines()
                        {
                            floor = floor.max(line.parse::<u64>().map_err(|_| "Invalid version ledger")?);
                        }
                    }
                    let policy = bareline_distribution::update::TrustPolicy {
                        release_public_key: &authority.release_public_key,
                        channel: config.channel,
                        artifact_type: "bareline-executable-x64",
                        platform: "windows-x64",
                        publisher: &authority.publisher,
                        protocol: 1,
                        highest_metadata_version: floor,
                        maximum_package_bytes: 256 * 1024 * 1024,
                    };
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|_| "Clock unavailable")?
                        .as_secs();
                    let prepared = native::fetch_verified_update(
                        config.host,
                        config.manifest,
                        config.signature,
                        config.artifact,
                        &policy,
                        now,
                        &authority.certificate,
                        &std::env::temp_dir(),
                        &cancel,
                    )
                    .map_err(|e| format!("Update verification: {e:?}"))?;
                    if cancel.load(Ordering::Acquire) {
                        return Err("Update cancelled".into());
                    }
                    native::transfer_update(prepared, &root).map_err(|e| format!("Update staging: {e}"))
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
            match result {
                Ok(()) => {
                    self.ready = self.worker_marks_ready;
                    self.status = if self.ready {
                        "Verified update ready. Choose Apply Update on Exit."
                    } else {
                        "Unapplied staging retained in update history. A fresh check is available."
                    }
                    .into();
                }
                Err(e) => self.status = e,
            }
        }
    }
    pub fn apply_on_exit(&mut self) {
        if self.ready {
            self.apply_on_exit = true;
            self.status = "Update will apply after the editor closes.".into();
        }
    }
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.apply_on_exit = false;
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
                let result =
                    installation().and_then(|root| native::discard_pending_update(&root).map_err(|e| e.to_string()));
                let _ = tx.send(result);
                notify();
            }) {
            Ok(_) => {
                self.worker_marks_ready = false;
                self.worker = Some(rx);
                self.status = "Retaining unapplied staging…".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    /// Invoke only after the normal event loop has returned and dirty-close choices resolved.
    pub fn finish(&mut self) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        if self.apply_on_exit {
            let config = Config::compiled()?;
            let root = installation()?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs();
            let authority = native::resolve_release_authority(&root, config.key, config.publisher, config.floor, now)
                .map_err(|e| e.to_string())?;
            native::launch_update_helper(&root, &authority.certificate, false).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// Call after a successful ordinary frame, never during startup probes.
    pub fn healthy_frame(&mut self) {
        if self.acknowledged {
            return;
        }
        self.acknowledged = true;
        if let Ok(config) = Config::compiled() {
            let _ = std::thread::Builder::new()
                .name("bareline-update-ack".into())
                .spawn(move || {
                    if let Ok(root) = installation() {
                        if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                            && let Ok(authority) = native::resolve_release_authority(
                                &root,
                                config.key,
                                config.publisher,
                                config.floor,
                                now.as_secs(),
                            )
                        {
                            let _ = native::launch_update_helper(&root, &authority.certificate, true);
                        }
                    }
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
