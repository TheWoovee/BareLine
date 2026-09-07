// SPDX-License-Identifier: MPL-2.0
//! Lazy native host lifecycle. One bounded invocation worker, no idle host/thread.
use bareline_app::extensions::{InvocationBroker, InvocationOutput};
use bareline_document::DocumentSnapshot;
use bareline_extensions_protocol::{Invocation, RawRange, broker::ExtensionSession};
use bareline_platform::PlatformServices;
use bareline_platform_windows::extension_transport::{HostLaunch, run_verified_host};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

#[derive(Clone)]
pub struct VerifiedRuntime {
    pub executable: PathBuf,
    pub executable_sha256: [u8; 32],
    pub publisher_certificate_sha256: [u8; 32],
}
pub struct InvocationJob {
    pub runtime: VerifiedRuntime,
    pub component: PathBuf,
    pub component_sha256: [u8; 32],
    pub invocation: Invocation,
    pub source: DocumentSnapshot,
    pub session: ExtensionSession,
    pub panels: Vec<String>,
    /// Set by the document owner only after retained original-byte provenance is
    /// known safe for replacement. Read-only invocations do not need this flag.
    pub edits_preserve_original: bool,
}
type OriginalReader = Box<dyn FnMut(u64, RawRange) -> Result<Vec<u8>, String> + Send>;
struct Pending {
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<InvocationOutput, String>>,
}
pub struct ExtensionsRuntime {
    pending: Option<Pending>,
    enabled: bool,
    root: Option<PathBuf>,
    pub message: Option<String>,
    pub open: bool,
    tab: usize,
    bounds: bareline_renderer::Rect,
    panel_output: String,
    trust: Option<OwnerTrust>,
    catalog: Option<CatalogSelection>,
    manager_pending: Option<mpsc::Receiver<Result<ManagerResult, String>>>,
    manager_cancel: Arc<AtomicBool>,
    installed: Vec<InstalledRow>,
    runtime_package: Option<bareline_extensions_protocol::InstalledPackage>,
    selected: usize,
    permission_review: Option<usize>,
}
impl Default for ExtensionsRuntime {
    fn default() -> Self {
        Self {
            pending: None,
            enabled: true,
            root: None,
            message: None,
            open: false,
            tab: 0,
            bounds: Default::default(),
            panel_output: String::new(),
            trust: None,
            catalog: None,
            manager_pending: None,
            manager_cancel: Arc::new(AtomicBool::new(false)),
            installed: vec![],
            runtime_package: None,
            selected: 0,
            permission_review: None,
        }
    }
}
impl ExtensionsRuntime {
    pub fn configure(&mut self, root: Option<PathBuf>, enabled: bool) {
        self.root = root;
        self.trust = compiled_trust();
        self.enabled = enabled;
        if !enabled {
            self.cancel();
            self.message = Some("Extensions disabled for this session (--no-extensions)".into());
        }
    }
    pub fn running(&self) -> bool {
        self.pending.is_some()
    }
    pub fn cancel(&mut self) {
        self.manager_cancel.store(true, Ordering::Release);
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Release);
        }
    }
    pub fn start(
        &mut self,
        job: InvocationJob,
        mut original: OriginalReader,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        if !self.enabled {
            return Err("Extensions disabled for this session".into());
        }
        if self.running() {
            return Err("An extension is running; cancel it before starting another".into());
        }
        let mut broker =
            InvocationBroker::new(job.invocation.clone(), job.source, job.session, job.panels)?;
        let (send, receive) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        std::thread::spawn(move || {
            let outcome = run_verified_host(HostLaunch { executable: &job.runtime.executable, executable_sha256: job.runtime.executable_sha256, publisher_certificate_sha256: job.runtime.publisher_certificate_sha256, component: &job.component, component_sha256: job.component_sha256, invocation: &job.invocation }, worker_cancel.clone(), |message| {
                if !job.edits_preserve_original && matches!(message.request, bareline_extensions_protocol::Request::ApplyEdits { .. } | bareline_extensions_protocol::Request::BeginEdits { .. }) {
                    return bareline_extensions_protocol::BrokerResponse { request_id: message.request_id, result: Err("Formatting is unavailable while original undecodable bytes require preservation".into()) };
                }
                broker.request(message, &mut original)
            }).map_err(|e| e.to_string());
            let outcome = if worker_cancel.load(Ordering::Acquire) {
                Err("Extension cancelled; document unchanged".into())
            } else {
                outcome
            };
            let _ = send.send(broker.finish(outcome));
            notify();
        });
        self.pending = Some(Pending {
            cancel,
            result: receive,
        });
        self.message = Some("Extension running in a separate process".into());
        Ok(())
    }
    /// Main-loop caller applies the returned transaction with apply_prepared;
    /// stale identity/revision, IME and read-only restrictions remain actor-owned.
    pub fn pump(&mut self) -> Option<Result<InvocationOutput, String>> {
        let pending = self.pending.as_ref()?;
        let mut result = match pending.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Extension worker stopped; document unchanged".into())
            }
        };
        if pending.cancel.load(Ordering::Acquire) {
            result = Err("Extension cancelled; document unchanged".into());
        }
        self.message = Some(match &result {
            Ok(_) => "Extension completed; host stopped".into(),
            Err(error) => error.clone(),
        });
        self.pending = None;
        Some(result)
    }
}
impl Drop for ExtensionsRuntime {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn register(registry: &mut bareline_commands::CommandRegistry) {
    use bareline_commands::{Action, CommandId, CommandSpec};
    for (id, title) in [
        ("extensions.manage", "Manage Extensions"),
        ("extensions.cancel", "Cancel Extension"),
        ("extensions.close", "Close Extensions"),
        (
            "extensions.catalog",
            "Open Signed Offline Extension Catalog",
        ),
        (
            "extensions.runtime_catalog",
            "Open Signed Offline Runtime Catalog",
        ),
        ("extensions.install", "Install Selected Package"),
        (
            "extensions.permissions",
            "Review Selected Extension Permissions",
        ),
        (
            "extensions.approve",
            "Approve Reviewed Extension Permissions",
        ),
        ("extensions.disable", "Disable Selected Extension"),
        ("extensions.remove", "Uninstall Selected Extension"),
        ("extensions.remove_runtime", "Remove Runtime"),
        ("ext.json.validate", "JSON: Validate"),
        ("ext.json.format", "JSON: Format"),
        ("ext.json.minify", "JSON: Minify"),
        ("ext.json.tree", "JSON: Tree"),
        ("ext.xml.validate", "XML: Validate"),
        ("ext.xml.format", "XML: Format"),
        ("ext.xml.xpath", "XML: XPath"),
        ("ext.hex.open", "Hex: Original Bytes"),
    ] {
        registry
            .register(CommandSpec {
                id: CommandId(id),
                title,
                category: "Extensions",
                shortcut: "",
                action: Action::Contributed(CommandId(id)),
            })
            .expect("unique extension command");
    }
}
impl ExtensionsRuntime {
    pub fn draw(
        &mut self,
        _renderer: &mut super::WindowsRenderer,
        width: f32,
        height: f32,
        ops: &mut Vec<bareline_renderer::DrawOp>,
    ) {
        use bareline_renderer::DrawOp;
        use bareline_ui::{ACCENT, BORDER, CHROME, ELEVATED, MUTED, TEXT, rect};
        if !self.open {
            return;
        }
        let y = 82.0;
        self.bounds = rect(0.0, y, width, (height - y - 24.0).max(0.0));
        ops.push(DrawOp::Fill(self.bounds, CHROME));
        let sidebar = (width * 0.186).clamp(140.0, 296.0);
        ops.push(DrawOp::Stroke(
            rect(sidebar, y, 0.0, self.bounds.height),
            BORDER,
            1.0,
        ));
        ops.push(text(20.0, y + 24.0, "Extensions", 16.0, ACCENT));
        ops.push(DrawOp::FillRounded(
            rect(10.0, y + 54.0, sidebar - 20.0, 48.0),
            ELEVATED,
            6.0,
        ));
        ops.push(text(30.0, y + 70.0, "Extensions", 16.0, TEXT));
        let x = sidebar + 24.0;
        let available = (width - x - 24.0).max(0.0);
        ops.push(text(x, y + 20.0, "Extensions", 22.0, TEXT));
        for (i, label) in ["Installed", "Discover", "Updates", "Disabled"]
            .iter()
            .enumerate()
        {
            let tab_x = x + i as f32 * 110.0;
            ops.push(text(
                tab_x + 12.0,
                y + 70.0,
                *label,
                16.0,
                if self.tab == i { ACCENT } else { MUTED },
            ));
            if self.tab == i {
                ops.push(DrawOp::Fill(rect(tab_x, y + 98.0, 104.0, 2.0), ACCENT));
            }
        }
        ops.push(DrawOp::StrokeRounded(
            rect(x, y + 110.0, available, 54.0),
            BORDER,
            1.0,
            8.0,
        ));
        ops.push(text(
            x + 20.0,
            y + 127.0,
            "Extensions run isolated in a separate process.",
            16.0,
            TEXT,
        ));
        ops.push(text(x, y + 183.0, "Runtime", 16.0, TEXT));
        ops.push(DrawOp::FillRounded(
            rect(x, y + 210.0, available, 76.0),
            ELEVATED,
            8.0,
        ));
        ops.push(text(
            x + 20.0,
            y + 230.0,
            if self.running() {
                "Runtime host running"
            } else {
                if self.runtime_package.is_some() {
                    "Runtime installed; host stopped"
                } else {
                    "Runtime not installed"
                }
            },
            17.0,
            TEXT,
        ));
        ops.push(text(
            x + 20.0,
            y + 255.0,
            "A verified offline runtime pack and owner trust policy are required.",
            13.0,
            MUTED,
        ));
        let empty = match self.tab {
            1 => "Catalog unavailable offline. No extensions were downloaded.",
            2 => "No verified updates are available.",
            3 => "No disabled extensions.",
            _ => "No installed extensions.",
        };
        let labels: Vec<(usize, String)> = if self.tab == 1 {
            self.catalog
                .as_ref()
                .map(|catalog| {
                    catalog
                        .entries
                        .iter()
                        .enumerate()
                        .map(|(i, entry)| {
                            (
                                i,
                                format!("{} {} — {}", entry.id, entry.version, entry.publisher),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else if self.tab == 2 {
            vec![]
        } else {
            self.installed
                .iter()
                .enumerate()
                .filter(|(_, row)| self.tab != 3 || !row.enabled)
                .map(|(i, row)| {
                    (
                        i,
                        format!(
                            "{} {} — {} — {}",
                            row.package.id,
                            row.package.version,
                            if row.enabled { "Enabled" } else { "Disabled" },
                            row.package
                                .manifest
                                .capabilities
                                .iter()
                                .map(|c| c.name())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                })
                .collect()
        };
        if labels.is_empty() {
            ops.push(text(x, y + 315.0, empty, 16.0, MUTED));
        }
        for (position, (index, label)) in labels.iter().take(6).enumerate() {
            let row_y = y + 310.0 + position as f32 * 32.0;
            if *index == self.selected {
                ops.push(DrawOp::FillRounded(
                    rect(x, row_y - 5.0, available, 30.0),
                    ELEVATED,
                    4.0,
                ));
            }
            ops.push(text(
                x + 8.0,
                row_y,
                label,
                14.0,
                if *index == self.selected {
                    ACCENT
                } else {
                    TEXT
                },
            ));
        }
        if let Some(message) = &self.message {
            ops.push(text(x, y + 520.0, message, 14.0, TEXT));
        }
        if !self.panel_output.is_empty() {
            ops.push(DrawOp::PushClip(rect(
                x,
                y + 560.0,
                available,
                (height - y - 610.0).max(0.0),
            )));
            ops.push(text(x, y + 560.0, &self.panel_output, 14.0, TEXT));
            ops.push(DrawOp::PopClip);
        }
        ops.push(text(
            x,
            height - 58.0,
            "Disabling extensions stops the host. Removing the runtime frees disk space.",
            13.0,
            MUTED,
        ));
    }
}
impl super::Shell {
    pub(super) fn extensions_dispatch(&mut self, _el: &super::ActiveEventLoop, id: &str) -> bool {
        let result: Result<(), String> = match id {
            "extensions.manage" => {
                self.extensions.open = true;
                Ok(())
            }
            "extensions.close" => {
                self.extensions.open = false;
                Ok(())
            }
            "extensions.cancel" => {
                self.extensions.cancel();
                Ok(())
            }
            "extensions.catalog" | "extensions.runtime_catalog" => (|| {
                let path = self
                    .platform
                    .as_ref()
                    .ok_or("Window unavailable")?
                    .open_file()?;
                if let Some(path) = path {
                    self.extensions.open_catalog(
                        path,
                        id == "extensions.runtime_catalog",
                        self.notify.clone(),
                    )?;
                }
                Ok(())
            })(),
            "extensions.install" => self.extensions.install_selected(self.notify.clone()),
            "extensions.permissions" => {
                if let Some(row) = self.extensions.installed.get(self.extensions.selected) {
                    self.extensions.permission_review = Some(self.extensions.selected);
                    self.extensions.message = Some(format!(
                        "{} requests: {}. Choose Approve Reviewed Extension Permissions to enable.",
                        row.package.id,
                        row.package
                            .manifest
                            .capabilities
                            .iter()
                            .map(|c| c.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    Ok(())
                } else {
                    Err("Select an installed extension".into())
                }
            }
            "extensions.approve" => {
                if self.extensions.permission_review.take() == Some(self.extensions.selected) {
                    if let Some(row) = self.extensions.installed.get_mut(self.extensions.selected) {
                        row.enabled = true;
                        self.extensions.message =
                            Some("Extension enabled with reviewed permissions".into());
                        Ok(())
                    } else {
                        Err("Select an installed extension".into())
                    }
                } else {
                    Err("Review the selected extension permissions first".into())
                }
            }
            "extensions.disable" => {
                self.extensions.cancel();
                if let Some(row) = self.extensions.installed.get_mut(self.extensions.selected) {
                    row.enabled = false;
                }
                Ok(())
            }
            "extensions.remove" | "extensions.remove_runtime" => (|| {
                if self.extensions.running() || self.extensions.manager_pending.is_some() {
                    return Err("Cancel or finish current operation before removal".into());
                }
                let package = if id == "extensions.remove_runtime" {
                    self.extensions
                        .runtime_package
                        .clone()
                        .ok_or("Runtime not installed")?
                } else {
                    if self.extensions.selected >= self.extensions.installed.len() {
                        return Err("Select an installed extension".into());
                    }
                    self.extensions.installed[self.extensions.selected]
                        .package
                        .clone()
                };
                self.extensions
                    .manager_work(self.notify.clone(), move |_cancel| {
                        let id = package.id.clone();
                        package.remove().map_err(|e| format!("Removal: {e:?}"))?;
                        Ok(ManagerResult::Removed(id))
                    })
            })(),
            command if command.starts_with("ext.") => self.start_selected_extension(command),
            _ => return false,
        };
        if let Err(error) = result {
            self.extensions.message = Some(error);
            self.extensions.open = true;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn extensions_pump(&mut self, _el: &super::ActiveEventLoop) {
        if self.extensions.pump_manager()
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        if let Some(result) = self.extensions.pump() {
            match result {
                Ok(output) => {
                    if let Some(transaction) = output.transaction {
                        let result = self
                            .workspace
                            .as_mut()
                            .and_then(|workspace| {
                                workspace
                                    .editors
                                    .iter_mut()
                                    .find(|editor| editor.snapshot().same_document(&output.source))
                            })
                            .ok_or("Document closed; extension edits discarded")
                            .and_then(|editor| editor.apply_prepared(&output.source, transaction));
                        if let Err(error) = result {
                            self.extensions.message = Some(error.into());
                        }
                    }
                    self.extensions.panel_output = output
                        .panels
                        .into_iter()
                        .map(|(_, text)| text)
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                Err(error) => self.extensions.message = Some(error),
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
    pub(super) fn extensions_event(
        &mut self,
        _el: &super::ActiveEventLoop,
        event: &super::WindowEvent,
    ) -> bool {
        use super::{ElementState, Key, MouseButton, NamedKey, WindowEvent};
        if !self.extensions.open || self.palette.open {
            return false;
        }
        match event {
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::ArrowDown) => {
                        self.extensions.move_selection(1);
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        self.extensions.move_selection(-1);
                    }
                    Key::Named(NamedKey::Escape) => self.extensions.open = false,
                    Key::Named(NamedKey::ArrowRight) => {
                        self.extensions.tab = (self.extensions.tab + 1) % 4
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        self.extensions.tab = (self.extensions.tab + 3) % 4
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let sidebar = (self.extensions.bounds.width * 0.186).clamp(140.0, 296.0);
                let left = sidebar + 24.0;
                if self.pointer.y >= 142.0
                    && self.pointer.y <= 182.0
                    && self.pointer.x >= left
                    && self.pointer.x < left + 440.0
                {
                    self.extensions.tab = ((self.pointer.x - left) / 110.0) as usize;
                }
            }
            _ => return self.extensions.bounds.contains(self.pointer),
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

fn text(
    x: f32,
    y: f32,
    value: impl Into<String>,
    size: f32,
    color: bareline_renderer::Color,
) -> bareline_renderer::DrawOp {
    bareline_renderer::DrawOp::Text {
        origin: bareline_renderer::Point { x, y },
        text: value.into(),
        size,
        color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancel_after_worker_completion_discards_queued_proposal() {
        let document = bareline_document::Document::from_utf8(
            "text",
            bareline_document::Budget::new(4096),
            bareline_document::Budget::new(4096),
        )
        .unwrap();
        let (send, result) = mpsc::channel();
        send.send(Ok(InvocationOutput {
            source: document.snapshot(),
            transaction: None,
            panels: vec![("panel".into(), "late reply".into())],
        }))
        .unwrap();
        let mut runtime = ExtensionsRuntime::default();
        runtime.pending = Some(Pending {
            cancel: Arc::new(AtomicBool::new(false)),
            result,
        });
        runtime.cancel();
        assert!(runtime.pump().unwrap().is_err());
        assert!(!runtime.running());
    }
}

/// Owner pins are supplied by release composition, never read from a selected
/// package/catalog. None keeps a development build explicitly unavailable.
#[derive(Clone)]
pub struct OwnerTrust {
    pub catalog_public_key: String,
    pub publisher: String,
    pub channel: String,
    pub publisher_certificate_sha256: [u8; 32],
}
struct CatalogSelection {
    source: bareline_extensions_protocol::OfflinePackageSource,
    entries: Vec<bareline_extensions_protocol::CatalogEntry>,
}
enum ManagerResult {
    Catalog(CatalogSelection),
    Installed(bareline_extensions_protocol::InstalledPackage, String),
    Removed(String),
}
struct InstalledRow {
    package: bareline_extensions_protocol::InstalledPackage,
    enabled: bool,
}

impl ExtensionsRuntime {
    fn manager_work(
        &mut self,
        notify: Arc<dyn Fn() + Send + Sync>,
        work: impl FnOnce(Arc<AtomicBool>) -> Result<ManagerResult, String> + Send + 'static,
    ) -> Result<(), String> {
        if !self.enabled {
            return Err("Extensions disabled for this session".into());
        }
        if self.manager_pending.is_some() || self.running() {
            return Err("Wait for the current extension operation".into());
        }
        let (send, receive) = mpsc::sync_channel(1);
        self.manager_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.manager_cancel.clone();
        std::thread::spawn(move || {
            let _ = send.send(work(cancel));
            notify();
        });
        self.manager_pending = Some(receive);
        Ok(())
    }
    fn open_catalog(
        &mut self,
        path: PathBuf,
        runtime: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        let trust = self
            .trust
            .clone()
            .ok_or("Owner trust policy is not configured in this build")?;
        let storage = self.root.clone().ok_or("Extension storage unavailable")?;
        self.manager_work(notify, move |cancel| {
            if cancel.load(Ordering::Acquire) {
                return Err("Operation cancelled".into());
            }
            use bareline_extensions_protocol::{CatalogPolicy, OfflinePackageSource};
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .map_err(|e| e.to_string())?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 1024 * 1024 {
                return Err("Catalog size limit".into());
            }
            let mut signature = String::new();
            let signature_path = path.with_file_name(format!(
                "{}.minisig",
                path.file_name()
                    .ok_or("Catalog filename")?
                    .to_string_lossy()
            ));
            std::fs::File::open(signature_path)
                .map_err(|e| e.to_string())?
                .take(8193)
                .read_to_string(&mut signature)
                .map_err(|e| e.to_string())?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs();
            let artifact_type = if runtime { "runtime" } else { "extension" };
            std::fs::create_dir_all(&storage).map_err(|e| e.to_string())?;
            let version_path = storage.join(format!("{artifact_type}-metadata-version"));
            let highest = match std::fs::File::open(&version_path) {
                Ok(file) => {
                    let mut value = String::new();
                    file.take(65)
                        .read_to_string(&mut value)
                        .map_err(|e| e.to_string())?;
                    if value.len() > 64 {
                        return Err("Metadata version record limit".into());
                    }
                    value
                        .trim()
                        .parse::<u64>()
                        .map_err(|_| "Invalid metadata version record")?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
                Err(error) => return Err(error.to_string()),
            };
            let source = OfflinePackageSource::open(
                path.parent().ok_or("Catalog directory")?.to_path_buf(),
                &bytes,
                &signature,
                &CatalogPolicy {
                    public_key: &trust.catalog_public_key,
                    publisher: &trust.publisher,
                    channel: &trust.channel,
                    platform: "windows-x64",
                    artifact_type,
                    highest_metadata_version: highest,
                    now_unix: now,
                },
            )
            .map_err(|e| format!("Catalog verification: {e:?}"))?;
            if cancel.load(Ordering::Acquire) {
                return Err("Operation cancelled".into());
            }
            let temporary = storage.join(format!(
                "{artifact_type}-metadata-{}-{}.tmp",
                std::process::id(),
                now
            ));
            {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .map_err(|e| e.to_string())?;
                file.write_all(source.metadata_version().to_string().as_bytes())
                    .and_then(|_| file.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            if let Err(error) = std::fs::rename(&temporary, &version_path) {
                let _ = std::fs::remove_file(&temporary);
                return Err(error.to_string());
            }
            let entries = source
                .entries()
                .iter()
                .filter(|entry| entry.artifact_type == artifact_type)
                .cloned()
                .collect();
            Ok(ManagerResult::Catalog(CatalogSelection { source, entries }))
        })
    }
    fn install_selected(&mut self, notify: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        use bareline_extensions_protocol::{PackageRequest, VerifiedPackageSource};
        if self.manager_pending.is_some() || self.running() {
            return Err("Wait for the current operation".into());
        }
        let root = self.root.clone().ok_or("Extension storage unavailable")?;
        let catalog = self
            .catalog
            .take()
            .ok_or("Open a verified offline catalog first")?;
        let entry = catalog
            .entries
            .get(self.selected)
            .cloned()
            .ok_or("Select an available package")?;
        self.manager_work(notify, move |cancel| {
            let package = catalog
                .source
                .fetch(&PackageRequest {
                    id: entry.id,
                    version: entry.version,
                })
                .map_err(|e| format!("Package verification: {e:?}"))?;
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let installed = package
                .install(&root, &cancel)
                .map_err(|e| format!("Installation: {e:?}"))?;
            Ok(ManagerResult::Installed(installed, entry.artifact_type))
        })
    }
    fn pump_manager(&mut self) -> bool {
        let Some(receiver) = &self.manager_pending else {
            return false;
        };
        let result = match receiver.try_recv() {
            Ok(value) => value,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(_) => Err("Package worker stopped".into()),
        };
        self.manager_pending = None;
        self.permission_review = None;
        match result {
            Ok(ManagerResult::Catalog(catalog)) => {
                self.catalog = Some(catalog);
                self.selected = 0;
                self.tab = 1;
                self.message = Some(
                    "Verified catalog loaded. Select a package, then Install Selected Package."
                        .into(),
                );
            }
            Ok(ManagerResult::Installed(package, kind)) => {
                if kind == "runtime" {
                    self.runtime_package = Some(package);
                } else {
                    self.installed.push(InstalledRow {
                        package,
                        enabled: false,
                    });
                }
                self.tab = 0;
                self.selected = 0;
                self.message = Some("Installed. Review permissions before enabling.".into());
            }
            Ok(ManagerResult::Removed(id)) => {
                self.installed.retain(|row| row.package.id != id);
                if self
                    .runtime_package
                    .as_ref()
                    .is_some_and(|package| package.id == id)
                {
                    self.runtime_package = None;
                }
                self.message = Some("Removed installed package".into())
            }
            Err(error) => self.message = Some(error),
        }
        true
    }
}

impl super::Shell {
    fn start_selected_extension(&mut self, command: &str) -> Result<(), String> {
        use bareline_extensions_protocol::{Capability, Scope, broker::Grant};
        let runtime = self
            .extensions
            .runtime_package
            .as_ref()
            .ok_or("Runtime not installed")?;
        let trust = self
            .extensions
            .trust
            .as_ref()
            .ok_or("Owner runtime trust policy unavailable")?;
        let row = self
            .extensions
            .installed
            .iter()
            .find(|row| row.enabled && row.package.manifest.commands.iter().any(|id| id == command))
            .ok_or("Install and enable an extension contributing this command")?;
        let workspace = self.workspace.as_ref().ok_or("Open a document first")?;
        let editor = workspace
            .editors
            .get(self.app.active)
            .ok_or("Open a document first")?;
        if editor.busy() || editor.paged() {
            return Err("Extension requires a complete available snapshot".into());
        }
        if command.starts_with("ext.hex.") {
            return Err(
                "Original byte snapshot provider is unavailable; Hex cannot substitute edited text"
                    .into(),
            );
        }
        let source = editor.snapshot().clone();
        let mut session =
            ExtensionSession::new(row.package.id.clone()).map_err(|e| format!("{e:?}"))?;
        session.approve(
            row.package
                .manifest
                .capabilities
                .iter()
                .map(|capability| Grant {
                    capability: *capability,
                    scope: if matches!(
                        capability,
                        Capability::DocumentRead | Capability::DocumentEdit
                    ) {
                        Scope::Document(1)
                    } else {
                        Scope::Extension
                    },
                })
                .collect(),
        );
        let invocation = Invocation {
            extension_id: row.package.id.clone(),
            command: command.into(),
            arguments: String::new(),
            document: 1,
            revision: source.revision.0,
            source_generation: 0,
            text_length: source.len() as u64,
            raw_length: 0,
            grant_generation: session.generation(),
        };
        let job = InvocationJob {
            runtime: VerifiedRuntime {
                executable: runtime.directory().join(&runtime.manifest.entry_component),
                executable_sha256: runtime.component_sha256,
                publisher_certificate_sha256: trust.publisher_certificate_sha256,
            },
            component: row
                .package
                .directory()
                .join(&row.package.manifest.entry_component),
            component_sha256: row.package.component_sha256,
            invocation,
            source,
            session,
            panels: row.package.manifest.panels.clone(),
            edits_preserve_original: workspace.path(self.app.active).is_none()
                && !editor.read_only(),
        };
        self.extensions.start(
            job,
            Box::new(|_, _| Err("Original source unavailable".into())),
            self.notify.clone(),
        )
    }
}

// Release composition supplies owner-controlled pins. This development build
// deliberately has no production signing policy and never trusts package keys.
fn compiled_trust() -> Option<OwnerTrust> {
    None
}

impl ExtensionsRuntime {
    fn move_selection(&mut self, delta: isize) {
        self.permission_review = None;
        let indices: Vec<usize> = if self.tab == 1 {
            self.catalog
                .as_ref()
                .map(|c| (0..c.entries.len()).collect())
                .unwrap_or_default()
        } else {
            self.installed
                .iter()
                .enumerate()
                .filter(|(_, row)| self.tab != 3 || !row.enabled)
                .map(|(i, _)| i)
                .collect()
        };
        if indices.is_empty() {
            self.selected = 0;
            return;
        }
        let position = indices
            .iter()
            .position(|i| *i == self.selected)
            .unwrap_or(0);
        self.selected =
            indices[(position as isize + delta).rem_euclid(indices.len() as isize) as usize];
    }
}

#[cfg(test)]
mod manager_tests {
    use super::*;
    #[test]
    fn release_without_owner_pins_cannot_open_or_install_catalog() {
        let mut runtime = ExtensionsRuntime::default();
        runtime.configure(Some(PathBuf::from("unused-extension-storage")), true);
        let error = runtime
            .open_catalog(
                PathBuf::from("never-opened-catalog.json"),
                false,
                Arc::new(|| {}),
            )
            .unwrap_err();
        assert!(error.contains("Owner trust policy"));
        assert!(runtime.manager_pending.is_none());
        assert!(runtime.installed.is_empty());
        runtime.permission_review = Some(0);
        runtime.move_selection(1);
        assert!(runtime.permission_review.is_none());
    }
}
