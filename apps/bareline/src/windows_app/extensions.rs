// SPDX-License-Identifier: MPL-2.0
//! Lazy native host lifecycle. One bounded invocation worker, no idle host/thread.
use bareline_app::extensions::manager::{InstalledState, ManagerIndex};
use bareline_app::extensions::{InvocationBroker, InvocationOutput};
use bareline_document::DocumentSnapshot;
use bareline_extensions_protocol::{Invocation, broker::ExtensionSession};
mod readers;
mod ui;
#[cfg(test)]
pub(super) fn accessibility_test_cases() -> Vec<(
    &'static str,
    Vec<bareline_platform::accessibility::AccessibilityNode>,
    Option<u64>,
)> {
    ui::accessibility_test_cases()
}
use bareline_platform::PlatformServices;
use bareline_platform_windows::extension_transport::{
    HostLaunch, HostLifecycle, run_verified_host_observed,
};
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
    pub budget: bareline_extensions_protocol::ExecutionBudget,
    pub source: DocumentSnapshot,
    pub original: Option<bareline_app::workspace::extensions::OriginalSource>,
    pub paged: Option<bareline_editor_surface::paged_view::PagedReadHandle>,
    pub session: ExtensionSession,
    pub panels: Vec<String>,
    /// Set by the document owner only after retained original-byte provenance is
    /// known safe for replacement. Read-only invocations do not need this flag.
    pub edits_preserve_original: bool,
}
struct Pending {
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<InvocationOutput, String>>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtensionLifecyclePhase {
    #[default]
    Idle,
    Requested,
    Started,
    Authenticated,
    Drained,
    Rejected,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtensionLifecycleReceipt {
    pub generation: u64,
    pub phase: ExtensionLifecyclePhase,
    pub pid: Option<u32>,
    /// Set only when the UI drains broker.finish's outcome and rechecks cancel.
    /// Drained alone describes process cleanup, including failed invocations.
    pub succeeded: Option<bool>,
}
pub struct ExtensionsRuntime {
    lifecycle: Arc<std::sync::Mutex<ExtensionLifecycleReceipt>>,
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
    runtime_package: Option<bareline_platform_windows::update::InstalledRuntime>,
    selected: usize,
    index: ManagerIndex,
    restore_pending: bool,
    inventory_restored: bool,
    inventory_error: Option<String>,
    permission_review: Option<usize>,
    deferred_disabled: std::collections::BTreeSet<String>,
    command_selection: usize,
    ui: ui::ManagerUi,
}
impl Default for ExtensionsRuntime {
    fn default() -> Self {
        Self {
            lifecycle: Arc::new(std::sync::Mutex::new(ExtensionLifecycleReceipt::default())),
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
            index: ManagerIndex::default(),
            restore_pending: false,
            inventory_restored: false,
            inventory_error: None,
            permission_review: None,
            deferred_disabled: std::collections::BTreeSet::new(),
            command_selection: 0,
            ui: ui::ManagerUi::default(),
        }
    }
}
impl ExtensionsRuntime {
    pub fn command_inventory_ready(&self) -> Result<bool, String> {
        if !self.enabled {
            return Ok(true);
        }
        if let Some(error) = &self.inventory_error {
            return Err(error.clone());
        }
        if self.restore_pending || self.manager_pending.is_some() || !self.inventory_restored {
            return Ok(false);
        }
        if !self.valid_contribution_budget() {
            return Err("Verified extension contribution limit exceeded".into());
        }
        Ok(true)
    }
    pub fn lifecycle_receipt(&self) -> Option<ExtensionLifecycleReceipt> {
        self.lifecycle.try_lock().ok().map(|receipt| *receipt)
    }
    pub fn configure(&mut self, root: Option<PathBuf>, enabled: bool) {
        self.root = root;
        self.trust = compiled_trust();
        self.restore_pending = enabled;
        self.inventory_restored = !enabled;
        self.inventory_error = None;
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
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        if !self.enabled {
            return Err("Extensions disabled for this session".into());
        }
        if self.running() {
            return Err("An extension is running; cancel it before starting another".into());
        }
        let mut broker = if job.paged.is_some() {
            InvocationBroker::new_paged(
                job.invocation.clone(),
                job.source,
                job.session,
                job.panels,
            )?
        } else {
            InvocationBroker::new(job.invocation.clone(), job.source, job.session, job.panels)?
        };
        let (send, receive) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let lifecycle = self.lifecycle.clone();
        {
            let mut receipt = lifecycle
                .lock()
                .map_err(|_| "Extension lifecycle unavailable")?;
            receipt.generation = receipt
                .generation
                .checked_add(1)
                .ok_or("Extension lifecycle generation exhausted")?;
            receipt.phase = ExtensionLifecyclePhase::Requested;
            receipt.pid = None;
            receipt.succeeded = None;
        }
        std::thread::spawn(move || {
            let readers = std::cell::RefCell::new(readers::Readers::new(
                job.original,
                job.paged,
                worker_cancel.clone(),
                std::time::Instant::now()
                    + std::time::Duration::from_millis(job.budget.timeout_ms()),
            ));
            let outcome = run_verified_host_observed(HostLaunch { executable: &job.runtime.executable, executable_sha256: job.runtime.executable_sha256, publisher_certificate_sha256: job.runtime.publisher_certificate_sha256, component: &job.component, component_sha256: job.component_sha256, invocation: &job.invocation, budget:job.budget }, worker_cancel.clone(), |event| {
                if let Ok(mut receipt) = lifecycle.lock() { let (phase,pid) = match event {HostLifecycle::Started(pid)=>(ExtensionLifecyclePhase::Started,pid),HostLifecycle::Authenticated(pid)=>(ExtensionLifecyclePhase::Authenticated,pid),HostLifecycle::Drained(pid)=>(ExtensionLifecyclePhase::Drained,pid)}; receipt.phase=phase;receipt.pid=Some(pid); }
            }, |message| {
                if !job.edits_preserve_original && matches!(message.request, bareline_extensions_protocol::Request::ApplyEdits { .. } | bareline_extensions_protocol::Request::BeginEdits { .. }) {
                    return bareline_extensions_protocol::BrokerResponse { request_id: message.request_id, result: Err("Formatting is unavailable while original undecodable bytes require preservation".into()) };
                }
                broker.request_with_text(message, |_, range| readers.borrow_mut().raw(range), |range| readers.borrow_mut().text(range))
            }).map_err(|e| e.to_string());
            if let Ok(mut receipt) = lifecycle.lock()
                && receipt.phase == ExtensionLifecyclePhase::Requested
            {
                receipt.phase = ExtensionLifecyclePhase::Rejected;
            }
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
        if let Ok(mut receipt) = self.lifecycle.lock() {
            receipt.succeeded = Some(result.is_ok());
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
            "Install Signed Offline Runtime",
        ),
        ("extensions.install", "Install Selected Package"),
        ("extensions.run_selected", "Run Selected Extension Command"),
        (
            "extensions.run_background",
            "Run Declared Background Command (120 seconds)",
        ),
        ("extensions.next_command", "Select Next Extension Command"),
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
        self.draw_manager(_renderer, width, height, ops);
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
                    if id == "extensions.runtime_catalog" {
                        self.extensions.install_runtime(path, self.notify.clone())?;
                    } else {
                        self.extensions
                            .open_catalog(path, false, self.notify.clone())?;
                    }
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
                    self.extensions.save_permission(true, self.notify.clone())
                } else {
                    Err("Review the selected extension permissions first".into())
                }
            }
            "extensions.disable" => self.extensions.save_permission(false, self.notify.clone()),
            "extensions.remove" => self.extensions.remove_selected(self.notify.clone()),
            "extensions.remove_runtime" => self.extensions.remove_runtime(self.notify.clone()),
            "extensions.next_command" => {
                if let Some(row) = self.extensions.installed.get(self.extensions.selected) {
                    if !row.package.manifest.commands.is_empty() {
                        self.extensions.command_selection = (self.extensions.command_selection + 1)
                            % row.package.manifest.commands.len();
                    }
                }
                Ok(())
            }
            "extensions.run_selected" | "extensions.run_background" => (|| {
                let row = self
                    .extensions
                    .installed
                    .get(self.extensions.selected)
                    .ok_or("Select an installed extension")?;
                let command = row
                    .package
                    .manifest
                    .commands
                    .get(self.extensions.command_selection)
                    .ok_or("Select a command")?
                    .clone();
                let owner = row.package.id.clone();
                self.start_owned_extension(
                    Some(&owner),
                    &command,
                    if id == "extensions.run_background" {
                        bareline_extensions_protocol::ExecutionBudget::Background
                    } else {
                        bareline_extensions_protocol::ExecutionBudget::Interactive
                    },
                )
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
        self.extensions.start_restore(self.notify.clone());
        if self.extensions.pump_manager()
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        self.extensions.flush_disabled(self.notify.clone());
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
        self.extensions_ui_event(_el, event)
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
    pub release_public_key: String,
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
    Installed(
        bareline_extensions_protocol::InstalledPackage,
        ManagerIndex,
        Option<String>,
    ),
    Restored(
        ManagerIndex,
        Vec<InstalledRow>,
        Option<bareline_platform_windows::update::InstalledRuntime>,
        Vec<String>,
    ),
    Permissions(ManagerIndex),
    Removed(String, ManagerIndex),
    RuntimeInstalled(
        bareline_platform_windows::update::InstalledRuntime,
        ManagerIndex,
    ),
    RuntimeRemoved(ManagerIndex),
}
struct InstalledRow {
    package: bareline_extensions_protocol::InstalledPackage,
    state: InstalledState,
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
        if self.manager_pending.is_some() {
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
        if entry.artifact_type != "extension" {
            return Err("Use the verified native runtime provider for runtime installation".into());
        }
        let mut index = self.index.clone();
        // Persisted counts are hints. Reconcile against verified manifests before
        // deciding whether an update fits the bounded contribution registry.
        for row in &self.installed {
            if let Some(entry) = index
                .entries
                .iter_mut()
                .find(|entry| entry.id == row.package.id)
            {
                entry.command_count = row.package.manifest.commands.len();
            }
        }
        let previous = self
            .installed
            .iter()
            .find(|row| row.package.id == entry.id)
            .map(|row| row.package.clone());
        self.manager_work(notify, move |cancel| {
            let package = catalog
                .source
                .fetch(&PackageRequest {
                    id: entry.id,
                    version: entry.version,
                })
                .map_err(|e| format!("Package verification: {e:?}"))?;
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let (index, installed) =
                bareline_app::extensions::manager::install(&root, &package, &index, &cancel)?;
            let cleanup = previous
                .filter(|old| old.directory() != installed.directory())
                .and_then(|old| old.remove_cached().err())
                .map(|error| {
                    format!("Update installed; old version cleanup needs attention: {error:?}")
                });
            Ok(ManagerResult::Installed(installed, index, cleanup))
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
            Ok(ManagerResult::Installed(package, index, cleanup)) => {
                let state = index
                    .entries
                    .iter()
                    .find(|entry| entry.id == package.id)
                    .expect("published installation record")
                    .clone();
                self.installed.retain(|row| row.package.id != package.id);
                self.installed.push(InstalledRow { package, state });
                self.index = index;
                self.tab = 0;
                self.selected = 0;
                self.message =
                    Some(cleanup.unwrap_or_else(|| {
                        "Installed. Review permissions before enabling.".into()
                    }));
            }
            Ok(ManagerResult::Restored(index, rows, runtime, errors)) => {
                self.inventory_restored = true;
                self.inventory_error = (!errors.is_empty()).then(|| errors.join("; "));
                self.runtime_package = runtime;
                self.index = index;
                self.installed = rows;
                self.message = Some(if errors.is_empty() {
                    "Installed extensions restored and verified; host stopped".into()
                } else {
                    errors.join("; ")
                });
            }
            Ok(ManagerResult::Permissions(index)) => {
                for row in &mut self.installed {
                    if let Some(state) = index
                        .entries
                        .iter()
                        .find(|entry| entry.id == row.package.id)
                    {
                        row.state = state.clone();
                    }
                }
                self.index = index;
                self.message = Some("Permissions saved".into());
            }
            Ok(ManagerResult::Removed(id, index)) => {
                self.index = index;
                self.installed.retain(|row| row.package.id != id);
                self.message = Some("Removed installed package".into())
            }
            Ok(ManagerResult::RuntimeInstalled(runtime, index)) => {
                self.runtime_package = Some(runtime);
                self.index = index;
                self.message = Some("Verified runtime installed; host stopped".into());
            }
            Ok(ManagerResult::RuntimeRemoved(index)) => {
                self.runtime_package = None;
                self.index = index;
                self.message = Some("Runtime removed".into());
            }
            Err(error) => {
                if !self.inventory_restored {
                    self.inventory_error = Some(error.clone());
                }
                self.message = Some(error);
            }
        }
        for row in &mut self.installed {
            if self.deferred_disabled.contains(&row.package.id) {
                row.state.enabled = false;
            }
        }
        true
    }
}

impl super::Shell {
    fn start_selected_extension(&mut self, command: &str) -> Result<(), String> {
        self.start_owned_extension(
            None,
            command,
            bareline_extensions_protocol::ExecutionBudget::Interactive,
        )
    }
    fn start_owned_extension(
        &mut self,
        owner: Option<&str>,
        command: &str,
        budget: bareline_extensions_protocol::ExecutionBudget,
    ) -> Result<(), String> {
        use bareline_extensions_protocol::{Capability, Scope, broker::Grant};
        if !self.extensions.valid_contribution_budget() {
            return Err("Verified extension contribution limit (1024) exceeded".into());
        }
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
            .find(|row| {
                owner.is_none_or(|owner| row.package.id == owner)
                    && row.state.enabled
                    && row.package.manifest.commands.iter().any(|id| id == command)
            })
            .ok_or("Install and enable an extension contributing this command")?;
        if budget == bareline_extensions_protocol::ExecutionBudget::Background
            && !row
                .package
                .manifest
                .background_commands
                .iter()
                .any(|id| id == command)
        {
            return Err("This signed command does not declare background execution".into());
        }
        let workspace = self.workspace.as_ref().ok_or("Open a document first")?;
        let editor = workspace
            .editors
            .get(self.app.active)
            .ok_or("Open a document first")?;
        if editor.busy() {
            return Err("Extension requires a complete available snapshot".into());
        }
        let original = workspace.raw_source_descriptor(self.app.active)?;
        let raw_length = original.as_ref().map_or(0, |source| source.len());
        let paged = match editor {
            bareline_app::workspace::WorkspaceEditor::Paged(editor) => Some(editor.read_handle()),
            _ => None,
        };
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
            arguments: self.extensions.argument_text()?,
            document: 1,
            revision: source.revision.0,
            // Tokens are scoped to this single authenticated invocation; the
            // original authority is immutable even while editor text is dirty.
            source_generation: 1,
            text_length: paged
                .as_ref()
                .map_or(source.len(), |handle| handle.snapshot().len())
                as u64,
            raw_length,
            grant_generation: session.generation(),
        };
        let job = InvocationJob {
            runtime: VerifiedRuntime {
                executable: runtime.executable.clone(),
                executable_sha256: runtime.executable_sha256,
                publisher_certificate_sha256: trust.publisher_certificate_sha256,
            },
            component: row
                .package
                .directory()
                .join(&row.package.manifest.entry_component),
            component_sha256: row.package.component_sha256,
            invocation,
            budget,
            source,
            original,
            paged,
            session,
            panels: row.package.manifest.panels.clone(),
            edits_preserve_original: workspace.extension_edits_preserve_original(self.app.active),
        };
        self.extensions.start(job, self.notify.clone())
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
        let indices: Vec<usize> = self
            .visible_rows()
            .into_iter()
            .map(|(index, _)| index)
            .collect();
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
    fn deferred_disable_persists_against_latest_completed_manager_generation() {
        let root = std::env::temp_dir().join(format!(
            "bareline-deferred-disable-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut runtime = ExtensionsRuntime::default();
        runtime.root = Some(root.clone());
        let index = ManagerIndex {
            generation: 8,
            entries: vec![InstalledState {
                id: "fixture.tools".into(),
                digest: "a".repeat(64),
                version: "2".into(),
                approved: vec![bareline_extensions_protocol::Capability::DocumentRead],
                enabled: true,
                generation: 8,
                command_count: 1,
            }],
            ..Default::default()
        };
        let (send, receive) = mpsc::sync_channel(1);
        assert!(send.send(Ok(ManagerResult::Permissions(index))).is_ok());
        runtime.manager_pending = Some(receive);
        runtime.deferred_disabled.insert("fixture.tools".into());
        assert!(runtime.pump_manager());
        runtime.flush_disabled(Arc::new(|| {}));
        let result = runtime
            .manager_pending
            .take()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        assert!(send.send(result).is_ok());
        runtime.manager_pending = Some(receive);
        assert!(runtime.pump_manager());
        let saved = ManagerIndex::load(&root).unwrap();
        assert!(!saved.entries[0].enabled);
        assert_eq!(saved.entries[0].version, "2");
        assert!(saved.generation > 8);
        std::fs::remove_file(root.join("manager-v1.json")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
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

impl ExtensionsRuntime {
    fn start_restore(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
        if !self.restore_pending {
            return;
        }
        self.restore_pending = false;
        let Some(trust) = self.trust.clone() else {
            self.message = Some(
                "Owner trust policy unavailable; installed packages cannot be verified".into(),
            );
            self.inventory_error = self.message.clone();
            return;
        };
        let Some(root) = self.root.clone() else {
            self.inventory_error = Some("Extension storage unavailable".into());
            return;
        };
        if let Err(error) = self.manager_work(notify, move |cancel| {
            let index = ManagerIndex::load(&root)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs();
            let policy = bareline_extensions_protocol::CatalogPolicy {
                public_key: &trust.catalog_public_key,
                publisher: &trust.publisher,
                channel: &trust.channel,
                platform: "windows-x64",
                artifact_type: "extension",
                highest_metadata_version: 0,
                now_unix: now,
            };
            let mut rows = Vec::new();
            let mut errors = Vec::new();
            for entry in &index.entries {
                if cancel.load(Ordering::Acquire) {
                    return Err("Restore cancelled".into());
                }
                match bareline_extensions_protocol::restore_cached(
                    &root,
                    &entry.digest,
                    &policy,
                    &cancel,
                ) {
                    Ok(package) if package.id == entry.id && package.version == entry.version => {
                        let mut state = entry.clone();
                        if package
                            .manifest
                            .capabilities
                            .iter()
                            .any(|cap| !state.approved.contains(cap))
                        {
                            state.enabled = false;
                        }
                        rows.push(InstalledRow { package, state });
                    }
                    Ok(_) => errors.push(format!("{}: installed identity mismatch", entry.id)),
                    Err(error) => errors.push(format!("{}: verification {error:?}", entry.id)),
                }
            }
            let runtime = if let Some(digest) = &index.runtime_digest {
                match bareline_platform_windows::update::restore_verified_runtime(
                    &root,
                    digest,
                    &trust.runtime_policy(index.runtime_metadata_version),
                    now,
                    &trust.publisher_certificate_sha256,
                ) {
                    Ok(runtime) => Some(runtime),
                    Err(error) => {
                        errors.push(format!("Runtime verification: {error}"));
                        None
                    }
                }
            } else {
                None
            };
            Ok(ManagerResult::Restored(index, rows, runtime, errors))
        }) {
            self.message = Some(error);
        }
    }
    fn save_permission(
        &mut self,
        approve: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        let row = self
            .installed
            .get(self.selected)
            .ok_or("Select an installed extension")?;
        let id = row.package.id.clone();
        let requested = row.package.manifest.capabilities.clone();
        let root = self.root.clone().ok_or("Extension storage unavailable")?;
        let mut index = self.index.clone();
        index.set_permission(&id, &requested, approve)?;
        if !approve {
            self.cancel();
            if let Some(row) = self.installed.get_mut(self.selected) {
                row.state.enabled = false;
                row.state.generation = index.generation;
            }
            if self.manager_pending.is_some() {
                self.deferred_disabled.insert(id);
                self.message =
                    Some("Extension stopped; saving disabled state after current operation".into());
                return Ok(());
            }
        }
        self.manager_work(notify, move |_cancel| {
            index.save(&root)?;
            Ok(ManagerResult::Permissions(index))
        })
    }
    fn flush_disabled(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
        if self.manager_pending.is_some() || self.deferred_disabled.is_empty() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let mut index = self.index.clone();
        for id in &self.deferred_disabled {
            if index.entries.iter().any(|entry| entry.id == *id)
                && let Err(error) = index.set_permission(id, &[], false)
            {
                self.message = Some(error);
                return;
            }
        }
        if self
            .manager_work(notify, move |_| {
                index.save(&root)?;
                Ok(ManagerResult::Permissions(index))
            })
            .is_ok()
        {
            self.deferred_disabled.clear();
        }
    }
    pub fn contributions(&self) -> Vec<bareline_commands::DynamicCommandRecord> {
        if !self.valid_contribution_budget() {
            return vec![];
        }
        self.installed
            .iter()
            .flat_map(|row| {
                row.package.manifest.commands.iter().map(move |id| {
                    let reason = if !self.enabled {
                        Some("Extensions disabled for this session")
                    } else if !row.state.enabled {
                        Some("Extension permission not enabled")
                    } else if self.runtime_package.is_none() {
                        Some("Runtime not installed")
                    } else {
                        None
                    };
                    bareline_commands::DynamicCommandRecord {
                        identity: bareline_commands::DynamicCommandIdentity {
                            owner: row.package.id.clone(),
                            id: id.clone(),
                            generation: row.state.generation,
                        },
                        title: id.clone(),
                        enabled: reason.is_none(),
                        disabled_reason: reason.map(str::to_owned),
                    }
                })
            })
            .collect()
    }
    fn valid_contribution_budget(&self) -> bool {
        self.installed
            .iter()
            .map(|row| row.package.manifest.commands.len())
            .sum::<usize>()
            <= 1024
    }
}
impl super::Shell {
    pub(super) fn extensions_invoke_contribution(
        &mut self,
        identity: bareline_commands::DynamicCommandIdentity,
    ) -> Result<(), String> {
        if !self.extensions.enabled
            || !self.extensions.installed.iter().any(|row| {
                row.package.id == identity.owner
                    && row.state.enabled
                    && row.state.generation == identity.generation
                    && row.package.manifest.commands.contains(&identity.id)
            })
        {
            return Err("Extension contribution was revoked or changed".into());
        }
        self.start_owned_extension(
            Some(&identity.owner),
            &identity.id,
            bareline_extensions_protocol::ExecutionBudget::Interactive,
        )
    }
}

impl OwnerTrust {
    fn runtime_policy(&self, highest: u64) -> bareline_distribution::update::TrustPolicy<'_> {
        bareline_distribution::update::TrustPolicy {
            release_public_key: &self.release_public_key,
            channel: &self.channel,
            artifact_type: "bareline-exthost-x64",
            platform: "windows-x64",
            publisher: &self.publisher,
            protocol: 1,
            highest_metadata_version: highest,
            maximum_package_bytes: 256 * 1024 * 1024,
        }
    }
}
impl ExtensionsRuntime {
    fn install_runtime(
        &mut self,
        executable: PathBuf,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        let trust = self
            .trust
            .clone()
            .ok_or("Owner runtime trust policy unavailable")?;
        let root = self.root.clone().ok_or("Extension storage unavailable")?;
        let mut index = self.index.clone();
        self.manager_work(notify, move |cancel| {
            use std::io::Read;
            let directory = executable.parent().ok_or("Runtime package directory")?;
            let mut metadata = Vec::new();
            std::fs::File::open(directory.join("runtime.json"))
                .map_err(|e| e.to_string())?
                .take(65537)
                .read_to_end(&mut metadata)
                .map_err(|e| e.to_string())?;
            let mut signature = String::new();
            std::fs::File::open(directory.join("runtime.minisig"))
                .map_err(|e| e.to_string())?
                .take(8193)
                .read_to_string(&mut signature)
                .map_err(|e| e.to_string())?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs();
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let runtime = bareline_platform_windows::update::install_verified_runtime(
                &executable,
                &metadata,
                &signature,
                &trust.runtime_policy(index.runtime_metadata_version),
                now,
                &trust.publisher_certificate_sha256,
                &root,
                &cancel,
            )
            .map_err(|e| e.to_string())?;
            if cancel.load(Ordering::Acquire) {
                return Err("Runtime installation cancelled".into());
            }
            index.runtime_digest = Some(
                runtime
                    .executable_sha256
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            );
            index.runtime_metadata_version =
                index.runtime_metadata_version.max(runtime.metadata_version);
            index.save(&root)?;
            Ok(ManagerResult::RuntimeInstalled(runtime, index))
        })
    }
    fn remove_runtime(&mut self, notify: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        if self.running() {
            self.cancel();
            return Err("Runtime cancellation requested; remove again after the host stops".into());
        }
        let runtime = self
            .runtime_package
            .clone()
            .ok_or("Runtime not installed")?;
        let root = self.root.clone().ok_or("Extension storage unavailable")?;
        let mut index = self.index.clone();
        self.manager_work(notify, move |_cancel| {
            bareline_platform_windows::update::remove_verified_runtime(&runtime)
                .map_err(|e| e.to_string())?;
            index.runtime_digest = None;
            index.save(&root)?;
            Ok(ManagerResult::RuntimeRemoved(index))
        })
    }
    fn remove_selected(&mut self, notify: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        if self.running() {
            self.cancel();
            return Err("Cancellation requested; remove again after the host stops".into());
        }
        let row = self
            .installed
            .get_mut(self.selected)
            .ok_or("Select an installed extension")?;
        row.state.enabled = false;
        let package = row.package.clone();
        let id = package.id.clone();
        let root = self.root.clone().ok_or("Extension storage unavailable")?;
        let mut index = self.index.clone();
        self.manager_work(notify, move |_cancel| {
            package
                .remove_cached()
                .map_err(|e| format!("Removal: {e:?}"))?;
            index.remove(&id)?;
            index.save(&root)?;
            Ok(ManagerResult::Removed(id, index))
        })
    }
}
