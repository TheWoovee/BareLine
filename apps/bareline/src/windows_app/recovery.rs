// SPDX-License-Identifier: MPL-2.0
//! Native consumer for background recovery discovery and exact-directory restore.
use super::*;
use bareline_platform::LocalFileSystem;
use std::sync::mpsc::{self, Receiver, TryRecvError};
type RecoveryDiscovery =
    Result<Vec<(PathBuf, bareline_file_io::recovery::RecoveryInspection)>, String>;
#[derive(Default)]
pub(super) struct RecoveryRuntime {
    root: Option<PathBuf>,
    started: bool,
    pending: Option<Receiver<RecoveryDiscovery>>,
    entries: Vec<(PathBuf, bareline_file_io::recovery::RecoveryInspection)>,
    cancellation: bareline_file_io::cancellation::Cancellation,
    last_status: Option<String>,
}
impl RecoveryRuntime {
    pub(super) fn configure(&mut self, root: Option<PathBuf>) {
        self.cancellation.cancel();
        self.root = root;
        self.started = false;
        self.pending = None;
        self.entries.clear();
        self.cancellation = Default::default();
    }
}
impl Drop for RecoveryRuntime {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
impl Shell {
    pub(super) fn recovery_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        match id {
            "recovery.open" => {
                let selected = self
                    .platform
                    .as_ref()
                    .and_then(|platform| platform.pick_folder().ok().flatten());
                if let Some(directory) = selected
                    && self.ensure_workspace(el)
                {
                    self.workspace
                        .as_mut()
                        .unwrap()
                        .restore_paged_recovery(directory);
                }
            }
            "recovery.restore_latest" => {
                if let Some((directory, _)) = self
                    .recovery
                    .entries
                    .iter()
                    .find(|(_, inspection)| inspection.complete_baseline)
                    .cloned()
                    && self.ensure_workspace(el)
                {
                    self.workspace
                        .as_mut()
                        .unwrap()
                        .restore_paged_recovery(directory);
                } else if let Some(workspace) = &mut self.workspace {
                    workspace.message=Some("No complete recovery checkpoint was found. Open a recovery folder to inspect it.".into());
                }
            }
            "recovery.retry" => {
                if let Some(workspace) = &mut self.workspace {
                    workspace.message = workspace
                        .editors
                        .get_mut(self.app.active)
                        .and_then(|editor| editor.retry_recovery().err());
                }
            }
            "recovery.save_as" => self.dispatch(el, Action::SaveAs),
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn recovery_pump(&mut self, _el: &ActiveEventLoop) {
        if !self.recovery.started {
            self.recovery.started = true;
            if let Some(root) = self.recovery.root.clone() {
                let (tx, rx) = mpsc::sync_channel(1);
                let cancel = self.recovery.cancellation.clone();
                let notify = self.notify.clone();
                if std::thread::Builder::new()
                    .name("recovery-discovery".into())
                    .spawn(move || {
                        let result = (|| -> Result<Vec<_>, String> {
                            let platform = bareline_platform_windows::WindowsFileSystem;
                            let _guard = match platform.guard_directory(&root) {
                                Ok(guard) => guard,
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                    return Ok(Vec::new());
                                }
                                Err(error) => return Err(error.to_string()),
                            };
                            let mut entries = Vec::new();
                            for entry in std::fs::read_dir(&root)
                                .map_err(|e| e.to_string())?
                                .take(256)
                            {
                                cancel.check().map_err(|e| format!("{e:?}"))?;
                                let entry = entry.map_err(|e| e.to_string())?;
                                if !entry.file_name().to_string_lossy().starts_with("paged-") {
                                    continue;
                                }
                                let directory = entry.path();
                                let Ok(_guard) = platform.guard_directory(&directory) else {
                                    continue;
                                };
                                if let Ok(inspection) =
                                    bareline_file_io::recovery::inspect(&directory, &cancel)
                                    && inspection.status
                                        != bareline_file_io::recovery::RecoveryStatus::Discarded
                                {
                                    entries.push((directory, inspection));
                                }
                            }
                            entries.sort_by_key(|(_, inspection)| {
                                std::cmp::Reverse(
                                    inspection
                                        .last_durable
                                        .map_or(0, |receipt| receipt.protected_unix_ms),
                                )
                            });
                            Ok(entries)
                        })();
                        let _ = tx.send(result);
                        notify();
                    })
                    .is_ok()
                {
                    self.recovery.pending = Some(rx);
                }
            }
        }
        if let Some(receiver) = &self.recovery.pending {
            match receiver.try_recv() {
                Err(TryRecvError::Empty) => {}
                result => {
                    self.recovery.pending = None;
                    match result {
                        Ok(Ok(entries)) => {
                            let count = entries.len();
                            self.recovery.entries = entries;
                            if count > 0
                                && let Some(workspace) = &mut self.workspace
                            {
                                workspace.message = Some(format!(
                                    "{count} recovery checkpoint(s) available. Use Restore Latest Recovery or Open Recovery Folder."
                                ));
                            }
                        }
                        Ok(Err(error)) => {
                            if let Some(workspace) = &mut self.workspace {
                                workspace.message =
                                    Some(format!("Recovery discovery unavailable: {error}"));
                            }
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        if let Some(workspace) = &mut self.workspace
            && let Some(editor) = workspace.editors.get(self.app.active)
        {
            let status = editor.recovery_status();
            let message = if let Some(error) = status.error {
                Some(format!(
                    "Recovery unavailable: {error}. Retry Recovery or Save As."
                ))
            } else if status.directory.is_some() && !status.complete {
                Some("Recovery snapshot preparing; editing remains available.".into())
            } else {
                None
            };
            if message != self.recovery.last_status {
                if workspace.message == self.recovery.last_status {
                    workspace.message = None;
                }
                self.recovery.last_status = message.clone();
                if let Some(message) = message {
                    workspace.message = Some(message);
                }
            }
        }
    }
    pub(super) fn recovery_event(&mut self, _el: &ActiveEventLoop, _event: &WindowEvent) -> bool {
        false
    }
}
