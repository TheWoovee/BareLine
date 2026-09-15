// SPDX-License-Identifier: MPL-2.0
mod accessibility;
mod charsets;
mod compare;
mod encoding;
mod extensions;
mod goto;
mod instance;
mod inventory;
mod language;
mod launch;
mod lifecycle;
mod macros;
mod migration;
mod modal;
mod performance;
mod power;
mod recovery;
mod run_prompt;
mod scrolling;
mod search;
mod session;
mod settings;
mod shell_integration;
mod shortcuts;
mod toast;
mod toolbar;
mod update;
mod utilities;
mod views;
mod watch;
mod workspace_panels;
use bareline_app::App;
use bareline_app::task::{Source, Wake};
use bareline_app::text_prototype::TextPrototype;
use bareline_app::workspace::{Input, Workspace};
use bareline_commands::Action;
use bareline_diagnostics::{Event, LocalLog, StartupAction, StartupLedger};
use bareline_platform::PlatformServices;
use bareline_platform_windows::{SaveChoice, WindowsPlatform, WindowsRenderer};
use bareline_renderer::{FrameStatus, Point, RenderBackend};
use bareline_settings::RendererMode;
use std::{
    path::PathBuf,
    sync::OnceLock,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, ModifiersState, NamedKey},
    platform::windows::EventLoopBuilderExtWindows,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowId},
};

#[derive(Clone, Copy)]
struct CloseTarget {
    index: usize,
    identity: (u64, u64),
    tab: Option<u64>,
    /// Set once the user answered Save: the prompt must not appear twice.
    saving: bool,
    /// Set once the user answered Don't Save while durable discard is pending.
    discarding: bool,
    /// User read-only state restored if the discarded tab is reopened.
    was_read_only: bool,
}
impl CloseTarget {
    fn matches(self, index: usize, identity: (u64, u64), tab: Option<u64>) -> bool {
        self.index == index && self.identity == identity && self.tab == tab
    }
}
enum PendingClose {
    Document(CloseTarget),
    Application,
    ApplicationDiscarding(Vec<DiscardConsent>),
    /// The user already chose Save on the exit prompt; we are only waiting for
    /// those saves to land before leaving.
    ApplicationAfterSaves,
}
#[derive(Clone, Copy)]
struct DiscardConsent {
    owner: u64,
    revision: u64,
    was_read_only: bool,
}
struct Shell {
    // Drop renderer/platform before destroying the window.
    renderer: Option<WindowsRenderer>,
    platform: Option<WindowsPlatform>,
    accessibility: Option<bareline_platform_windows::WindowsAccessibility>,
    shell_integration: shell_integration::ShellIntegrationRuntime,
    window: Option<Window>,
    app: App,
    palette: bareline_app::palette::PaletteController,
    pending_close: Option<PendingClose>,
    ui_focus: bareline_ui::focus::FocusChain,
    ui_router: bareline_ui::focus::EventRouter,
    modal: Option<modal::ModalDescriptor>,
    modal_identity_serial: u64,
    ledger: StartupLedger,
    modifiers: ModifiersState,
    software: bool,
    first_frame: bool,
    profile_initialization: launch::ProfileInitializationRuntime,
    profile_settings_path: Option<PathBuf>,
    profile_settings_revision: u64,
    profile_extensions_path: Option<PathBuf>,
    legacy_settings_path: Option<PathBuf>,
    legacy_session_path: Option<PathBuf>,
    legacy_recovery_path: Option<PathBuf>,
    legacy_extensions_path: Option<PathBuf>,
    smoke: bool,
    failed: bool,
    prototype: Option<TextPrototype>,
    workspace: Option<Workspace>,
    notify: std::sync::Arc<dyn Fn() + Send + Sync>,
    /// Sends a typed wake so a worker completion can pump only its own runtime
    /// instead of every runtime. A worker clones this and calls it with
    /// `Wake::One(source)`; `user_event` then runs just that pump.
    wake: std::sync::Arc<dyn Fn(Wake) + Send + Sync>,
    pointer: Point,
    editor_caret: Option<bareline_renderer::Rect>,
    perf: bool,
    idle_at: Option<Instant>,
    frames: u64,
    log: Option<LocalLog>,
    log_directory: Option<PathBuf>,
    startup_paths: Vec<PathBuf>,
    session: session::SessionRuntime,
    settings: settings::SettingsRuntime,
    views: views::ViewsRuntime,
    macros: macros::MacrosRuntime,
    watch: watch::WatchRuntime,
    panels: workspace_panels::WorkspacePanelsRuntime,
    launch: launch::LaunchRuntime,
    applied_settings: Option<(bareline_settings::EffectiveSettings, usize)>,
    update: update::UpdateRuntime,
    language: language::LanguageRuntime,
    extensions: extensions::ExtensionsRuntime,
    toolbar: toolbar::ToolbarRuntime,
    compare: compare::CompareRuntime,
    instance: instance::InstanceRuntime,
    recovery_root: Option<PathBuf>,
    shortcuts: shortcuts::ShortcutsRuntime,
    goto: goto::GotoRuntime,
    charsets: charsets::CharsetRuntime,
    run_prompt: run_prompt::RunPromptRuntime,
    recovery: recovery::RecoveryRuntime,
    lifecycle: lifecycle::LifecycleRuntime,
    performance: performance::PerformanceRuntime,
    power: power::PowerRuntime,
    utilities: utilities::UtilitiesRuntime,
    migration: migration::MigrationRuntime,
    search: search::SearchRuntime,
    scrolling: scrolling::Runtime,
    encoding: encoding::EncodingRuntime,
    inventory: inventory::InventoryRuntime,
    toasts: toast::ToastStack,
    /// Clickable status-bar picker regions (Language/Indent/EOL/Encoding),
    /// rebuilt each frame and hit-tested on a left click (UX-40).
    status_pickers: Vec<(bareline_renderer::Rect, &'static str)>,
}

fn tooltip_clock_ms() -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

/// The feature handler that a contributed command ID is routed to. This is the
/// single source of truth for command routing: `dispatch` consults it (via
/// [`DISPATCH_CHAIN`] and the inline pre/post handlers) and the inventory
/// validation rejects any registered command whose ID has no route (ARCH-06,
/// ARCH-15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    Encoding,
    Search,
    Migration,
    ShellIntegration,
    ThemeCycle,
    DynamicInvoke,
    Transcode,
    Update,
    Profile,
    Lifecycle,
    Recovery,
    Utilities,
    Goto,
    Shortcuts,
    Toolbar,
    Compare,
    Language,
    Extensions,
    Settings,
    Macros,
    Views,
    Panels,
    Watch,
    SearchPanel,
    /// Editor surface commands handled by `power_dispatch` or the editor fallback.
    EditorPower,
}

/// Classify a contributed command ID to the handler that owns it. The order of
/// the checks mirrors `dispatch` so overlapping prefixes (for example
/// `settings.keymap_open`, claimed by the shortcuts editor before the settings
/// page) resolve to the same handler the runtime picks. Returns `None` for an
/// ID that no handler claims — an unrouted command.
pub(super) fn command_route(id: &str) -> Option<Route> {
    use Route::*;
    // Inline handlers that run before the dispatch chain, in the same order.
    if id.starts_with("encoding.") {
        return Some(Encoding);
    }
    if id == "search.folder"
        || id == "search.scope.selection"
        || id == "search.scope.current"
        || id.starts_with("search.mark.")
        || id.starts_with("search.replaceIn")
        || id.starts_with("search.replacePreview.")
    {
        return Some(Search);
    }
    if id.starts_with("migration.") {
        return Some(Migration);
    }
    if id.starts_with("file.recent.")
        || matches!(
            id,
            "file.reveal" | "file.terminal" | "tray.toggle" | "tray.hide" | "tray.restore"
        )
    {
        return Some(ShellIntegration);
    }
    if id == "view.theme.cycle" {
        return Some(ThemeCycle);
    }
    if id == "internal.dynamic.invoke" {
        return Some(DynamicInvoke);
    }
    if id == "file.transcode.resume" || id == "file.transcode.cancel" {
        return Some(Transcode);
    }
    if id.starts_with("update.") {
        return Some(Update);
    }
    if id == "profile.migration.retry" {
        return Some(Profile);
    }
    // The dispatch chain, in order.
    if matches!(
        id,
        "file.save_copy"
            | "file.save_all"
            | "file.cancel_save_all"
            | "file.restore_closed"
            | "file.read_only"
            | "file.save_conflict_compare"
            | "file.save_conflict_save_elsewhere"
            | "file.save_conflict_retain_other"
            | "file.save_conflict_next"
            | "file.retry_save_cleanup"
            | "file.retry_save_recovery"
    ) {
        return Some(Lifecycle);
    }
    if id.starts_with("recovery.") {
        return Some(Recovery);
    }
    if id.starts_with("utilities.") {
        return Some(Utilities);
    }
    if id == "search.goto" {
        return Some(Goto);
    }
    // The shortcuts editor claims a few `settings.*` IDs before the settings page.
    if matches!(
        id,
        "settings.shortcuts"
            | "settings.shortcut_apply"
            | "settings.shortcut_close"
            | "settings.keymap_export"
            | "settings.keymap_open"
    ) {
        return Some(Shortcuts);
    }
    if matches!(
        id,
        "view.toolbar_toggle" | "view.toolbar_customize" | "view.toolbar_focus"
    ) {
        return Some(Toolbar);
    }
    if id.starts_with("compare.") {
        return Some(Compare);
    }
    // Language owns `editor.completion.show` and the fold family; everything else
    // under `editor.` falls to the editor surface below.
    if id == "editor.completion.show" || id.starts_with("language.") || id.starts_with("view.fold.") {
        return Some(Language);
    }
    if id.starts_with("extensions.") || id.starts_with("ext.") {
        return Some(Extensions);
    }
    if id.starts_with("settings.") {
        return Some(Settings);
    }
    if id.starts_with("macro.") || id.starts_with("output.") || id.starts_with("run.") {
        return Some(Macros);
    }
    if id.starts_with("window.select.")
        || id.starts_with("view.tabs.")
        || matches!(
            id,
            "view.clone_other"
                | "view.close_split"
                | "view.focus_other"
                | "view.move_other"
                | "view.split_horizontal"
                | "view.split_vertical"
                | "view.sync_horizontal"
                | "view.sync_vertical"
        )
    {
        return Some(Views);
    }
    if id.starts_with("documents.")
        || id.starts_with("outline.")
        || id.starts_with("workspace.")
        || matches!(
            id,
            "view.documentMap" | "view.documents" | "view.outline" | "view.workspace"
        )
    {
        return Some(Panels);
    }
    if id.starts_with("file.remote.") || id.starts_with("file.monitor.") || id.starts_with("file.external.") {
        return Some(Watch);
    }
    // Post-chain handlers in the workspace block.
    if matches!(
        id,
        "search.open_documents" | "search.cancel_panel" | "search.close_panel"
    ) {
        return Some(SearchPanel);
    }
    if id.starts_with("editor.") {
        return Some(EditorPower);
    }
    None
}

/// The ordered feature handlers `dispatch` walks for a contributed command. Each
/// returns `true` once it claims the ID. Kept as data so the routing order is
/// declared in one place rather than a hand-maintained boolean chain (ARCH-06).
const DISPATCH_CHAIN: &[fn(&mut Shell, &ActiveEventLoop, &str) -> bool] = &[
    Shell::lifecycle_dispatch,
    Shell::recovery_dispatch,
    Shell::utilities_dispatch,
    Shell::power_dispatch,
    Shell::goto_dispatch,
    Shell::shortcuts_dispatch,
    Shell::toolbar_dispatch,
    Shell::compare_dispatch,
    Shell::language_dispatch,
    Shell::extensions_dispatch,
    Shell::settings_dispatch,
    Shell::macros_dispatch,
    Shell::views_dispatch,
    Shell::panels_dispatch,
    Shell::watch_dispatch,
];

/// Register every command the shell contributes. Kept separate from `run` so the
/// inventory validation and its tests can build the exact production command set.
pub(super) fn register_all_commands(registry: &mut bareline_commands::CommandRegistry) {
    for (id, title) in [
        ("recovery.open", "Recovery Center"),
        ("recovery.open_folder", "Open Recovery Folder"),
        ("recovery.restore_latest", "Restore Latest Recovery"),
        ("recovery.retry", "Retry Recovery"),
        ("recovery.save_as", "Save Recovered Document As"),
    ] {
        registry
            .register(bareline_commands::CommandSpec {
                id: bareline_commands::CommandId(id),
                title,
                category: "File",
                shortcut: "",
                action: Action::Contributed(bareline_commands::CommandId(id)),
            })
            .expect("unique recovery command");
    }
    registry
        .register(bareline_commands::CommandSpec {
            id: bareline_commands::CommandId("profile.migration.retry"),
            title: "Retry Profile Migration",
            category: "File",
            shortcut: "",
            action: Action::Contributed(bareline_commands::CommandId("profile.migration.retry")),
        })
        .expect("unique profile migration command");
    for (id, title) in [
        (
            "file.transcode.resume",
            "Increase Temporary Disk Limit and Resume Conversion",
        ),
        ("file.transcode.cancel", "Cancel Paused Conversion"),
    ] {
        registry
            .register(bareline_commands::CommandSpec {
                id: bareline_commands::CommandId(id),
                title,
                category: "File",
                shortcut: "",
                action: Action::Contributed(bareline_commands::CommandId(id)),
            })
            .expect("unique conversion command");
    }
    bareline_settings::register_commands(registry).expect("unique settings commands");
    for command in update::commands()
        .into_iter()
        .chain(migration::commands())
        .chain(shell_integration::commands())
    {
        registry.register(command).expect("unique update command");
    }
    bareline_app::language::register_commands(registry);
    extensions::register(registry);
    toolbar::register(registry);
    shortcuts::register(registry);
    goto::register(registry);
    lifecycle::register(registry);
    power::register(registry);
    utilities::register(registry);
    search::register(registry);
    bareline_app::encoding::register(registry);
    compare::register(registry);
    views::register(registry);
    bareline_app::macros::register_commands(registry);
    bareline_app::workspace_panel::register_commands(registry);
    // Debug helper for verifying that every overlay follows the theme (UX-55):
    // flips the resolved appearance between light and dark on demand.
    registry
        .register(bareline_commands::CommandSpec {
            id: bareline_commands::CommandId("view.theme.cycle"),
            title: "Cycle Theme (Light/Dark)",
            category: "View",
            shortcut: "",
            action: Action::Contributed(bareline_commands::CommandId("view.theme.cycle")),
        })
        .expect("unique theme command");
    for command in watch::commands() {
        registry.register(command).expect("unique watch command");
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut ledger = StartupLedger::default();
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let (args, inventory_request) = inventory::parse(args)?;
    let parsed = {
        let _phase = bareline_diagnostics::startup_span(StartupAction::ParseCli);
        launch::parse(&args, &mut ledger)?
    };
    if inventory_request.is_some() && parsed.has_paths() {
        return Err("Command inventory export does not open documents".into());
    }
    match parsed.mode() {
        launch::LaunchMode::Help => {
            println!("{}", launch::HELP);
            return Ok(());
        }
        launch::LaunchMode::Version => {
            println!("Bareline {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => {}
    }
    let mut launch = launch::prepare(parsed, &mut ledger)?;
    let mut settings_bytes = match launch.settings_path.as_ref() {
        Some(path) => ledger.read_config(path, StartupAction::ReadSettings, 64 * 1024)?,
        None => None,
    };
    if settings_bytes.is_none()
        && let Some(legacy) = launch
            .legacy_settings_path
            .as_ref()
            .filter(|legacy| Some(*legacy) != launch.settings_path.as_ref())
    {
        // A locked or untrusted legacy file is not equivalent to no settings:
        // fail startup rather than silently presenting defaults before migration.
        settings_bytes = ledger.read_config(legacy, StartupAction::ReadSettings, 64 * 1024)?;
    }
    let settings_document = match settings_bytes {
        Some(bytes) => bareline_settings::SettingsDocument::parse(&bytes, bareline_settings::Scope::User)?,
        None => bareline_settings::SettingsDocument::empty(bareline_settings::Scope::User),
    };
    let settings = bareline_settings::resolve(&settings_document, None, false, None).values;
    let software = launch.software || (settings.renderer == RendererMode::Software && !launch.hardware);
    let smoke = launch.smoke;
    let prototype = launch.prototype;
    let perf = launch.perf;
    let startup_paths = launch.paths.clone();
    let mut builder = EventLoop::<Wake>::with_user_event();
    let (tx, rx) = std::sync::mpsc::channel();
    let (tray_tx, tray_rx) = std::sync::mpsc::channel();
    builder.with_msg_hook(move |message| {
        // SAFETY: winit supplies a valid MSG pointer during the hook invocation.
        if let Some(id) = unsafe { WindowsPlatform::command_message(message) } {
            let _ = tx.send(id);
        }
        if let Some(action) = unsafe { bareline_platform_windows::shell_integration::tray_message(message) } {
            let _ = tray_tx.send(action);
        }
        false
    });
    let event_loop = builder.build()?;
    let proxy = event_loop.create_proxy();
    event_loop.set_control_flow(ControlFlow::Wait);
    // The generic notify wakes the loop for a full pump. Sources that route their
    // worker to a specific runtime use `Shell::wake_for` instead, so only that
    // runtime is pumped on wake (see `user_event`).
    let wake_dispatch: std::sync::Arc<dyn Fn(Wake) + Send + Sync> = {
        let proxy = proxy.clone();
        std::sync::Arc::new(move |wake: Wake| {
            let _ = proxy.send_event(wake);
        })
    };
    let notify: std::sync::Arc<dyn Fn() + Send + Sync> = {
        let wake = wake_dispatch.clone();
        std::sync::Arc::new(move || wake(Wake::All))
    };
    if !smoke && !perf && !prototype {
        ledger.record(StartupAction::InstanceHandoff);
    }
    let Some(instance) = instance::prepare(&mut launch, notify.clone())? else {
        return Ok(());
    };
    let mut shell = Shell {
        renderer: None,
        platform: None,
        accessibility: None,
        shell_integration: Default::default(),
        window: None,
        app: App::default(),
        palette: Default::default(),
        pending_close: None,
        ui_focus: Default::default(),
        ui_router: Default::default(),
        modal: None,
        modal_identity_serial: 0,
        ledger,
        modifiers: ModifiersState::empty(),
        software,
        first_frame: false,
        profile_initialization: launch::ProfileInitializationRuntime::new(launch.profile_initialization.take()),
        profile_settings_path: launch.settings_path.clone(),
        profile_settings_revision: 0,
        profile_extensions_path: launch.extensions_path.clone(),
        legacy_settings_path: launch.legacy_settings_path.clone(),
        legacy_session_path: launch.legacy_session_path.clone(),
        legacy_recovery_path: launch.legacy_recovery_path.clone(),
        legacy_extensions_path: launch.legacy_extensions_path.clone(),
        smoke,
        failed: false,
        prototype: prototype.then(TextPrototype::mixed_script),
        workspace: None,
        notify,
        wake: wake_dispatch,
        pointer: Point::default(),
        editor_caret: None,
        perf,
        idle_at: None,
        frames: 0,
        log: None,
        log_directory: launch.diagnostics_path.clone(),
        startup_paths,
        session: Default::default(),
        settings: Default::default(),
        views: Default::default(),
        macros: Default::default(),
        watch: Default::default(),
        panels: Default::default(),
        launch: launch::LaunchRuntime::new(&launch),
        applied_settings: None,
        update: Default::default(),
        language: Default::default(),
        extensions: Default::default(),
        toolbar: Default::default(),
        compare: Default::default(),
        instance,
        recovery_root: launch.recovery_path.clone(),
        shortcuts: Default::default(),
        goto: Default::default(),
        charsets: Default::default(),
        run_prompt: Default::default(),
        recovery: Default::default(),
        lifecycle: Default::default(),
        performance: Default::default(),
        power: Default::default(),
        utilities: Default::default(),
        migration: Default::default(),
        search: Default::default(),
        scrolling: Default::default(),
        encoding: encoding::EncodingRuntime::default(),
        inventory: inventory::InventoryRuntime::default(),
        toasts: Default::default(),
        status_pickers: Vec::new(),
    };
    shell.shell_integration.portable = launch.portable;
    // Recent Files live next to the other machine-local data (portable keeps them
    // in the portable data folder); the OS shell MRU is handled separately.
    shell.shell_integration.recent_files.configure(
        launch
            .settings_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|root| root.join("recent.json")),
    );
    shell.performance.configure(launch.performance.clone());
    shell.recovery.configure(launch.recovery_path.clone(), true);
    shell.macros.configure(
        launch
            .settings_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|root| root.join("macros")),
    );
    shell.settings =
        settings::SettingsRuntime::new(settings_document, launch.settings_path.clone(), shell.notify.clone());
    shell.profile_settings_revision = shell.settings.controller.revision;
    let session_restore_path = launch
        .session_path
        .as_ref()
        .filter(|path| path.exists())
        .cloned()
        .or_else(|| launch.legacy_session_path.clone());
    shell
        .session
        .configure(launch.session_path.clone(), session_restore_path, !launch.no_session);
    shell
        .extensions
        .configure(launch.extensions_path.clone(), !launch.no_extensions);
    shell.inventory.configure(inventory_request);
    register_all_commands(&mut shell.app.commands);
    if prototype {
        shell.app.apply(Action::New);
    }
    let mut handler = Handler {
        shell,
        commands: rx,
        tray_actions: tray_rx,
    };
    event_loop.run_app(&mut handler)?;
    if !handler.shell.failed {
        handler.shell.update.finish()?;
    }
    shell = handler.shell;
    if shell.failed {
        return Err("Native shell failed; see diagnostic output".into());
    }
    Ok(())
}

// Native commands are drained by about_to_wait, without a worker or timer.
struct Handler {
    shell: Shell,
    commands: std::sync::mpsc::Receiver<usize>,
    tray_actions: std::sync::mpsc::Receiver<bareline_platform_windows::shell_integration::TrayAction>,
}
impl ApplicationHandler<Wake> for Handler {
    fn user_event(&mut self, el: &ActiveEventLoop, wake: Wake) {
        if self.shell.profile_initialization.pump() {
            match self.shell.profile_initialization.take_completion() {
                Some(Ok(result)) => self.shell.reconcile_profile_initialization(result),
                Some(Err(error)) => self.shell.profile_initialization_message(error),
                None => {}
            }
        }
        // Always-on work: accessibility, acknowledged inputs, and the workspace
        // editor pump (paged/resident document workers). The `wake.runs(..)`
        // guards below skip every feature pump except the one whose worker woke
        // the loop; a `Wake::All` (the generic notify) runs them all as before.
        self.shell.accessibility_actions(el);
        if self.shell.workspace.as_mut().is_some_and(|w| w.pump()) {
            if let Some(workspace) = &self.shell.workspace {
                if workspace.editors.len() > self.shell.app.tabs.len() {
                    self.shell.app.active = workspace.editors.len() - 1;
                }
                self.shell.app.tabs = workspace.titles();
                self.shell.app.active = self.shell.app.active.min(self.shell.app.tabs.len().saturating_sub(1));
            }
            if let Some(window) = &self.shell.window {
                window.request_redraw();
            }
        }
        self.shell.record_acknowledged_inputs();
        if wake.runs(Source::Search) && self.shell.search_pump() {
            if let Some(window) = &self.shell.window {
                window.request_redraw();
            }
        }
        if wake.runs(Source::Shortcuts) {
            self.shell.shortcuts_pump(el);
        }
        if wake.runs(Source::Instance) {
            self.shell.instance_pump(el);
        }
        if wake.runs(Source::Launch) {
            self.shell.launch_pump();
        }
        if wake.runs(Source::Session) && self.shell.profile_initialization.settled() {
            self.shell.session_pump(el);
        }
        if wake.runs(Source::Recovery) && self.shell.profile_initialization.settled() {
            self.shell.recovery_pump(el);
        }
        if wake.runs(Source::Lifecycle) {
            self.shell.lifecycle_pump(el);
        }
        if wake.runs(Source::Encoding) {
            self.shell.encoding_pump(el);
        }
        if wake.runs(Source::Migration) {
            self.shell.migration_pump(el);
        }
        if wake.runs(Source::ShellRecent) {
            self.shell.shell_recent_pump();
        }
        if wake.runs(Source::Performance) {
            self.shell.performance_pump(el);
        }
        if wake.runs(Source::Power) {
            self.shell.power_pump();
        }
        if wake.runs(Source::Utilities) {
            self.shell.utilities_pump(el);
        }
        if wake.runs(Source::Macros)
            && (self.shell.profile_initialization.settled() || self.shell.macros.operation_active())
        {
            self.shell.macros_pump(el);
        }
        if wake.runs(Source::Panels) {
            self.shell.panels_pump(el);
        }
        if wake.runs(Source::Watch) {
            self.shell.watch_pump(el);
        }
        if wake.runs(Source::Language) {
            self.shell.language_pump(el);
        }
        if wake.runs(Source::Extensions)
            && (self.shell.profile_initialization.settled() || self.shell.extensions.operation_active())
        {
            self.shell.extensions_pump(el);
            self.shell.sync_contributions();
        }
        if wake.runs(Source::Inventory) {
            self.shell.inventory_pump(el);
        }
        if wake.runs(Source::Compare) {
            self.shell.compare_pump(el);
        }
        if wake.runs(Source::Update) {
            let update_status = self.shell.update.status.clone();
            self.shell.update.poll();
            if self.shell.update.status != update_status {
                if let Some(workspace) = &mut self.shell.workspace {
                    workspace.message = Some(self.shell.update.status.clone());
                }
                if let Some(window) = &self.shell.window {
                    window.request_redraw();
                }
            }
        }
        if wake.runs(Source::Settings) && self.shell.settings.poll() {
            if self.shell.shortcuts.open
                && self.shell.shortcuts.status == "Saving shortcut changes..."
                && !self.shell.settings.keymap_busy()
            {
                self.shell.shortcuts.status = self
                    .shell
                    .settings
                    .controller
                    .error
                    .clone()
                    .unwrap_or_else(|| "Shortcut changes saved".into());
            }
            if let Some(window) = &self.shell.window {
                window.request_redraw();
            }
        }
    }
    fn resumed(&mut self, el: &ActiveEventLoop) {
        self.shell.resumed(el);
    }
    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.shell.window_event(el, id, event);
    }
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        // Native modal creation must happen after the input WndProc unwinds.
        self.shell.drain_pending_close(el);
        if (self.shell.profile_initialization.settled() || self.shell.macros.operation_active())
            && self.shell.macros.next_tick.is_some_and(|tick| tick <= Instant::now())
        {
            self.shell.macros_pump(el);
        }
        self.shell.inventory_pump(el);
        let caret_deadline = self.shell.caret_timer(Instant::now());
        if self.shell.search_pump() {
            if let Some(window) = &self.shell.window {
                window.request_redraw();
            }
        }
        while let Ok(action) = self.tray_actions.try_recv() {
            use bareline_platform_windows::shell_integration::TrayAction;
            if let Some(window) = &self.shell.window {
                window.set_visible(true);
                window.set_minimized(false);
                window.focus_window();
            }
            match action {
                TrayAction::Restore => {}
                TrayAction::New => self.shell.dispatch(el, Action::New),
                TrayAction::Open => self.shell.dispatch(el, Action::Open),
                TrayAction::Find => self.shell.dispatch(el, Action::Find),
                TrayAction::Exit => {
                    self.shell.shell_integration.keep_in_tray = false;
                    self.shell.request_close(el);
                }
            }
        }
        while let Ok(id) = self.commands.try_recv() {
            if let Some(action) = self
                .shell
                .platform
                .as_ref()
                .and_then(|p| p.command_id(id))
                .and_then(|id| {
                    self.shell
                        .app
                        .commands
                        .dispatch_in(id, &self.shell.command_context())
                        .ok()
                })
            {
                self.shell.dispatch(el, action);
            }
        }
        // Repaint once an info toast reaches its auto-dismiss time so it
        // clears itself even while the app is otherwise idle (UX-60).
        if self
            .shell
            .toasts
            .next_deadline()
            .is_some_and(|deadline| deadline <= Instant::now())
            && let Some(window) = &self.shell.window
        {
            window.request_redraw();
        }
        let tooltip_now_ms = tooltip_clock_ms();
        let tooltip_due = self
            .shell
            .workspace
            .as_mut()
            .is_some_and(|workspace| workspace.find.tooltip_tick(tooltip_now_ms));
        let tooltip_deadline = self
            .shell
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.find.tooltip_deadline_ms())
            .map(|deadline| Instant::now() + Duration::from_millis(deadline.saturating_sub(tooltip_now_ms)));
        if tooltip_due && let Some(window) = &self.shell.window {
            window.request_redraw();
        }
        let deadline = caret_deadline
            .into_iter()
            .chain(self.shell.idle_at)
            .chain(self.shell.inventory.deadline())
            .chain(self.shell.macros.next_tick)
            .chain(self.shell.toasts.next_deadline())
            .chain(tooltip_deadline)
            .min();
        el.set_control_flow(deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
        if let Some(deadline) = self.shell.idle_at {
            if Instant::now() >= deadline {
                match bareline_platform_windows::private_bytes() {
                    Ok(bytes) => {
                        println!(
                            "{{\"event\":\"idle\",\"private_bytes\":{bytes},\"frames\":{}}}",
                            self.shell.frames
                        );
                        if let Some(log) = &mut self.shell.log {
                            let _ = log.event(Event::Idle {
                                private_bytes: bytes,
                                frames: self.shell.frames,
                            });
                        }
                    }
                    Err(error) => {
                        self.shell.fail(el, error);
                        return;
                    }
                }
                self.shell.idle_at = None;
                el.exit();
            }
        }
    }
}
impl Shell {
    fn migrated_item_path(
        report: &bareline_file_io::profile_migration::MigrationReport,
        name: &str,
        local: Option<PathBuf>,
        legacy: Option<PathBuf>,
    ) -> Option<PathBuf> {
        match report.authority(name) {
            Some(bareline_file_io::profile_migration::ReadAuthority::Local) => local,
            Some(bareline_file_io::profile_migration::ReadAuthority::Legacy) => legacy,
            _ => None,
        }
    }

    fn reconcile_profile_initialization(&mut self, result: launch::ProfileInitializationResult) {
        eprintln!(
            "event=profile_cleanup roots={} candidates={} removed={} visited={} limit={} cancelled={}",
            result.cleanup.roots,
            result.cleanup.candidates,
            result.cleanup.removed,
            result.cleanup.visited_entries,
            result.cleanup.limit_reached,
            result.cleanup.cancelled
        );
        let authorities = result.authorities;
        let report = match result.migration {
            Ok(report) => report,
            Err(error) => {
                self.profile_initialization_message(format!(
                    "Profile migration paused: {error}. Use Retry Profile Migration; legacy data was retained."
                ));
                Default::default()
            }
        };

        let local_settings = report.items.iter().any(|item| {
            item.name == "settings.toml"
                && item.migrated
                && item.destination_present
                && item.authority == bareline_file_io::profile_migration::ReadAuthority::Local
        });
        if local_settings {
            match self.settings.reconcile_migrated_user(self.profile_settings_revision) {
                Ok(true) => self.profile_settings_revision = self.settings.controller.revision,
                Ok(false) => self.profile_initialization_message(
                    "Migrated settings were retained, but settings changed after startup; the newer live settings remain active."
                        .into(),
                ),
                Err(error) => self.profile_initialization_message(format!(
                    "Migrated settings could not be applied: {error}. Use Retry Profile Migration."
                )),
            }
        }

        let local_session = self
            .profile_settings_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|root| root.join("session.json"));
        let session_path = Self::migrated_item_path(
            &authorities,
            "session.json",
            local_session,
            self.legacy_session_path.clone(),
        );
        if !self.session.set_restore_path(session_path) {
            self.profile_initialization_message(
                "Profile session migration completed after session restore began; the retained session will be considered on restart."
                    .into(),
            );
        }

        let recovery_root = Self::migrated_item_path(
            &authorities,
            "recovery",
            self.recovery_root.clone(),
            self.legacy_recovery_path.clone(),
        );
        let recovery_mutation_allowed =
            authorities.authority("recovery") == Some(bareline_file_io::profile_migration::ReadAuthority::Local);
        self.recovery.configure(recovery_root, recovery_mutation_allowed);

        let extensions_root = Self::migrated_item_path(
            &authorities,
            "extensions",
            self.profile_extensions_path.clone(),
            self.legacy_extensions_path.clone(),
        );
        let extensions_local =
            authorities.authority("extensions") == Some(bareline_file_io::profile_migration::ReadAuthority::Local);
        self.extensions
            .set_profile_root_before_restore(extensions_root, extensions_local);

        let local_macros = self
            .profile_settings_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|root| root.join("macros"));
        let legacy_macros = self
            .legacy_settings_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|root| root.join("macros"));
        let macros_root = Self::migrated_item_path(&authorities, "macros", local_macros, legacy_macros);
        self.macros.set_profile_read_directory(macros_root);

        if report.retryable || report.conflicts {
            let retained = report
                .items
                .iter()
                .filter_map(|item| item.retained_source.as_ref())
                .next()
                .map_or_else(|| "the legacy profile".into(), |path| path.display().to_string());
            self.profile_initialization_message(format!(
                "Profile migration needs attention. Use Retry Profile Migration; source data was retained at {retained}."
            ));
        }
        // Readers that were gated on the maintenance receipt get a fresh pump
        // only after their per-item read authority has been installed.
        (self.notify)();
    }

    fn profile_initialization_message(&mut self, message: String) {
        eprintln!("event=profile_migration_notice message={message}");
        if let Some(workspace) = &mut self.workspace {
            workspace.message = Some(message);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn editor_has_input_focus(&self) -> bool {
        self.window.as_ref().is_some_and(Window::has_focus)
            && self.modal.is_none()
            && !self.palette.open
            && !self.search_modal()
            && !self.settings.controller.open
            && !self.shortcuts.open
            && !self.macros.controller.manager.open
            && !self.extensions.open
            && !self.power.open
            && !self.recovery.has_input_focus()
            && !self.utilities.has_input_focus()
            && self.panels_accessibility_focus().is_none()
            && self.views_accessibility_focus().is_none()
            && self
                .workspace
                .as_ref()
                .is_some_and(|workspace| !workspace.find.has_focus() && !workspace.search_focus)
    }
    fn search_overlay_event(&mut self, event: &WindowEvent) -> bool {
        if self.search_folder_event(event) {
            return true;
        }
        if !self.search_modal() || self.palette.open {
            return false;
        }
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    let key = match &event.logical_key {
                        Key::Named(NamedKey::Escape) => Some(bareline_ui::controls::Key::Escape),
                        Key::Named(NamedKey::ArrowUp) => Some(bareline_ui::controls::Key::Up),
                        Key::Named(NamedKey::ArrowDown) => Some(bareline_ui::controls::Key::Down),
                        Key::Named(NamedKey::Home) => Some(bareline_ui::controls::Key::Home),
                        Key::Named(NamedKey::End) => Some(bareline_ui::controls::Key::End),
                        Key::Named(NamedKey::Enter) => Some(bareline_ui::controls::Key::Enter),
                        Key::Named(NamedKey::Space) => Some(bareline_ui::controls::Key::Space),
                        Key::Character(value) if value == " " => Some(bareline_ui::controls::Key::Space),
                        _ => None,
                    };
                    if let Some(key) = key {
                        self.search_key(key);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.window.as_ref().map_or(1.0, Window::scale_factor) as f32;
                self.pointer = Point {
                    x: position.x as f32 / scale,
                    y: position.y as f32 / scale,
                };
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                self.search_pointer(self.pointer);
            }
            WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } | WindowEvent::Ime(_) => {}
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    fn caret_timer(&mut self, now: Instant) -> Option<Instant> {
        let focused = self.editor_has_input_focus();
        let secondary = self.views.pane() == 1;
        let mut deadline = None;
        let mut redraw = false;
        if let Some(workspace) = &mut self.workspace {
            for (index, editor) in workspace.editors.iter_mut().enumerate() {
                editor.set_focused(focused && !secondary && index == self.app.active);
                redraw |= editor.tick_caret_blink(now);
                deadline = deadline.into_iter().chain(editor.blink_deadline()).min();
            }
        }
        self.views.set_secondary_focused(focused && secondary);
        redraw |= self.views.tick_secondary_caret_blink(now);
        deadline = deadline.into_iter().chain(self.views.secondary_blink_deadline()).min();
        if redraw {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        deadline
    }
    fn editor_context_menu(&mut self, el: &ActiveEventLoop) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Ok(origin) = window.inner_position() else {
            return;
        };
        let caret = self
            .editor_caret
            .unwrap_or_else(|| bareline_ui::rect(40.0, 60.0, 1.0, 20.0));
        let scale = window.scale_factor();
        // Editor menu: Undo/Redo · Cut/Copy/Paste/Select All · Case · Comment ·
        // Find/Go To Line · Bookmark. "-" is a separator (see context_menu_in).
        let commands = [
            "edit.undo",
            "edit.redo",
            "-",
            "edit.cut",
            "edit.copy",
            "edit.paste",
            "edit.select_all",
            "-",
            "editor.case.upper",
            "editor.case.lower",
            "editor.case.title",
            "editor.case.invert",
            "-",
            "editor.comment.toggleLine",
            "-",
            "search.find",
            "search.goto",
            "-",
            "editor.bookmark.toggle",
        ]
        .map(bareline_commands::CommandId);
        // Cut/Copy follow the selection: grey them out when nothing is selected.
        let has_selection = self.active_selection_nonempty();
        let mut context = self.command_context();
        for id in ["edit.cut", "edit.copy", "edit.delete"] {
            if let Some(reason) = selection_sensitive_disabled(id, has_selection) {
                context.states.insert(
                    bareline_commands::CommandId(id),
                    bareline_commands::CommandState::disabled(reason),
                );
            }
        }
        let result = self.platform.as_ref().unwrap().context_menu_in(
            origin.x + (caret.x as f64 * scale) as i32,
            origin.y + ((caret.y + caret.height) as f64 * scale) as i32,
            &self.app.commands,
            &context,
            &self.settings.keymap.keymap,
            &commands,
        );
        match result {
            Ok(Some(action)) => self.dispatch(el, action),
            Ok(None) => {}
            Err(error) => self.fail(el, error),
        }
    }
    fn active_selection_nonempty(&self) -> bool {
        use bareline_app::workspace::WorkspaceEditor;
        self.workspace
            .as_ref()
            .and_then(|workspace| workspace.editors.get(self.app.active))
            .is_some_and(|editor| match editor {
                WorkspaceEditor::Paged(paged) => {
                    let (anchor, caret) = paged.global_selection();
                    anchor != caret
                }
                WorkspaceEditor::Resident(editor) => editor.selection.anchor != editor.selection.caret,
            })
    }
    /// Hook the tab strip owner calls on a right-click to raise the tab context
    /// menu at screen coordinates (`x`, `y`) in physical pixels. Commands that do
    /// not exist in this build are dropped and their separators collapsed.
    pub(super) fn tab_context_menu(&mut self, el: &ActiveEventLoop, x: i32, y: i32) {
        let commands = [
            "file.close",
            "view.tabs.closeOthers",
            "view.tabs.closeAll",
            "-",
            "view.tabs.pin",
            "view.move_other",
            "-",
            "file.copyPath",
            "file.reveal",
            "file.rename",
        ]
        .map(bareline_commands::CommandId);
        let Some(platform) = self.platform.as_ref() else {
            return;
        };
        let result = platform.context_menu_in(
            x,
            y,
            &self.app.commands,
            &self.command_context(),
            &self.settings.keymap.keymap,
            &commands,
        );
        match result {
            Ok(Some(action)) => self.dispatch(el, action),
            Ok(None) => {}
            Err(error) => self.fail(el, error),
        }
    }
    fn record_acknowledged_inputs(&mut self) {
        let mut receipts = self.views.take_ordered_receipts();
        if let Some(workspace) = &mut self.workspace {
            receipts.extend(workspace.take_ordered_search_receipts());
            for editor in &mut workspace.editors {
                receipts.extend(editor.take_ordered_receipts());
            }
        }
        self.macros_record_receipts(receipts);
    }
    fn editor_bounds(&self) -> bareline_renderer::Rect {
        let (width, height) = self.window.as_ref().map_or((0.0, 0.0), |window| {
            let size = window.inner_size();
            let scale = window.scale_factor() as f32;
            (size.width as f32 / scale, size.height as f32 / scale)
        });
        bareline_ui::rect(
            self.panels.width_left(),
            self.toolbar.controller.height(),
            (width - self.panels.width_left() - self.panels.width_right()).max(0.0),
            (height - self.macros.height() - self.toolbar.controller.height()).max(0.0),
        )
    }
    fn editor_pointer(&self) -> Point {
        Point {
            x: self.pointer.x - self.editor_bounds().x,
            y: self.pointer.y - self.editor_bounds().y,
        }
    }
    fn finish_palette_focus(&mut self) {
        if self.palette.open {
            return;
        }
        self.ui_router.popup_capture = None;
        self.ui_focus.close_layer();
        if let Some(workspace) = &mut self.workspace {
            match self.ui_focus.focused().map(|id| id.0) {
                Some(6000 | 6001) => {
                    workspace
                        .find
                        .accessibility_action(self.ui_focus.focused().unwrap().0, true);
                }
                Some(20000) => {
                    workspace.search_focus = true;
                    workspace.search_panel.focused = true;
                }
                _ => {}
            }
        }
    }
    /// The focus state that suppresses editor clipboard/edit commands so a text
    /// field (Find, search, palette, Settings) keeps its own handling.
    fn context_field_active(&self) -> bool {
        self.settings.controller.open
            || self.shortcuts.open
            || self.palette.open
            || self.workspace.as_ref().is_some_and(|workspace| {
                workspace.find.has_focus() || (workspace.search_focus && workspace.search_panel.focused)
            })
    }
    /// The enable/disable/checked state for a single command's action, or `None`
    /// when it takes the default (enabled, unchecked) state. Shared by the full
    /// context and the per-key lazy context so both agree (ARCH-05).
    fn command_action_state(
        &self,
        action: Action,
        field_active: bool,
        editor: Option<&bareline_app::workspace::WorkspaceEditor>,
    ) -> Option<bareline_commands::CommandState> {
        use bareline_commands::CommandState;
        if field_active
            && matches!(
                action,
                Action::Copy | Action::Cut | Action::Paste | Action::SelectAll | Action::Undo | Action::Redo
            )
        {
            return None;
        }
        let reason = match action {
            Action::Undo
                if !self
                    .views
                    .history_available(self.workspace.as_ref(), self.app.active, true) =>
            {
                Some("Nothing to undo")
            }
            Action::Redo
                if !self
                    .views
                    .history_available(self.workspace.as_ref(), self.app.active, false) =>
            {
                Some("Nothing to redo")
            }
            Action::Save
            | Action::SaveAs
            | Action::Cut
            | Action::Paste
            | Action::Undo
            | Action::Redo
            | Action::ReplaceOne
            | Action::ReplaceAll
                if editor.is_none_or(|editor| editor.read_only() || editor.busy()) =>
            {
                Some("No editable document is ready")
            }
            Action::Copy
            | Action::SelectAll
            | Action::Close
            | Action::Find
            | Action::Replace
            | Action::FindNext
            | Action::FindPrevious
                if editor.is_none() =>
            {
                Some("Open a document first")
            }
            _ => None,
        };
        if let Some(reason) = reason {
            return Some(CommandState::disabled(reason));
        }
        let checked = match action {
            Action::FindMatchCase => self.workspace.as_ref().is_some_and(|w| w.find.case_sensitive),
            Action::FindWholeWord => self.workspace.as_ref().is_some_and(|w| w.find.whole_word),
            Action::FindMode => self
                .workspace
                .as_ref()
                .is_some_and(|w| w.find.mode != bareline_search::SearchMode::Literal),
            _ => false,
        };
        if checked {
            return Some(CommandState {
                checked,
                ..Default::default()
            });
        }
        None
    }
    /// Full command context for menus and the palette: the state of every
    /// command. The keystroke path uses [`Self::command_state_context`] instead,
    /// which computes only the resolved command's state (ARCH-05).
    fn command_context(&self) -> bareline_commands::CommandContext {
        let mut context = bareline_commands::CommandContext::default();
        let field_active = self.context_field_active();
        let editor = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.editors.get(self.app.active));
        for command in self.app.commands.entries() {
            if let Some(state) = self.command_action_state(command.action, field_active, editor) {
                context.states.insert(command.id, state);
            }
        }
        self.annotate_shared_context(&mut context);
        context
    }
    /// Context populated only for `id` (ARCH-05). `dispatch_in` consults just
    /// `states[id]`, and the per-subsystem annotators only touch their own IDs,
    /// so the state computed here for `id` matches [`Self::command_context`]
    /// without walking all commands on every key press.
    fn command_state_context(&self, id: bareline_commands::CommandId) -> bareline_commands::CommandContext {
        let mut context = bareline_commands::CommandContext::default();
        let field_active = self.context_field_active();
        let editor = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.editors.get(self.app.active));
        if let Some(command) = self.app.commands.entries().find(|command| command.id == id)
            && let Some(state) = self.command_action_state(command.action, field_active, editor)
        {
            context.states.insert(command.id, state);
        }
        self.annotate_shared_context(&mut context);
        context
    }
    /// Insert the states owned by each subsystem (transcode, settings, shortcuts
    /// and the per-runtime annotators). Shared by the full and single-command
    /// contexts so both agree on every ID.
    fn annotate_shared_context(&self, context: &mut bareline_commands::CommandContext) {
        use bareline_commands::CommandState;
        // Find/Replace and split/clone are implemented for paged documents
        // (crates/app/src/find.rs, search_panel.rs and windows_app/views.rs), so they
        // are no longer gated off here.
        let pause = self.workspace.as_ref().and_then(|w| w.paused_transcode_info());
        let resume = if let Some((_, used, required, limit)) = pause {
            CommandState {
                label: Some(format!(
                    "Resume Conversion with {} MiB Temporary Limit",
                    limit
                        .saturating_mul(2)
                        .max(used.saturating_add(required))
                        .div_ceil(1 << 20)
                )),
                ..Default::default()
            }
        } else {
            CommandState::disabled("No conversion is paused")
        };
        context
            .states
            .insert(bareline_commands::CommandId("file.transcode.resume"), resume);
        context.states.insert(
            bareline_commands::CommandId("file.transcode.cancel"),
            if pause.is_some() {
                CommandState::default()
            } else {
                CommandState::disabled("No conversion is paused")
            },
        );
        self.watch_annotate_context(context);
        self.views.annotate_context(context, &self.app.tabs, self.app.active);
        self.search_annotate_context(context);
        // Open-document search now includes paged documents
        // (search_panel.rs submit_mixed_open_documents), so it is no longer gated off.
        for id in [
            "settings.change",
            "settings.copy_key",
            "settings.reset_section",
            "settings.close",
        ] {
            if !self.settings.controller.open {
                context.states.insert(
                    bareline_commands::CommandId(id),
                    CommandState::disabled("Open Settings first"),
                );
            }
        }
        for id in ["settings.external_reload", "settings.external_keep"] {
            if !self.settings.controller.open || !self.settings.controller.has_external_change() {
                context.states.insert(
                    bareline_commands::CommandId(id),
                    CommandState::disabled(if self.settings.controller.open {
                        "No external settings change needs a decision"
                    } else {
                        "Open Settings first"
                    }),
                );
            }
        }
        if !self.settings.controller.open || !self.settings.controller.can_revert() {
            context.states.insert(
                bareline_commands::CommandId("settings.revert"),
                CommandState::disabled(if self.settings.controller.open {
                    "Settings match the opening-session values"
                } else {
                    "Open Settings first"
                }),
            );
        }
        for id in [
            "settings.keymap_import",
            "settings.keymap_export",
            "settings.shortcut_apply",
        ] {
            if self.settings.keymap_busy() {
                context.states.insert(
                    bareline_commands::CommandId(id),
                    CommandState::disabled("Wait for keymap persistence to finish"),
                );
            }
        }
        if !self.shortcuts.open {
            for id in ["settings.shortcut_apply", "settings.shortcut_close"] {
                context.states.insert(
                    bareline_commands::CommandId(id),
                    CommandState::disabled("Open Shortcut Mapper first"),
                );
            }
        }
        self.compare.annotate_context(context, self.workspace.as_ref());
        self.panels.annotate_context(context);
        self.toolbar.annotate_context(context);
        self.lifecycle
            .annotate_context(context, self.workspace.as_ref(), self.app.active);
        self.migration.annotate_context(context);
        self.shell_integration.annotate_context(
            context,
            self.workspace.as_ref().and_then(|w| w.path(self.app.active)).is_some(),
            self.window.as_ref().and_then(|w| w.is_visible()).unwrap_or(true),
        );
        self.macros.annotate_context(context);
        self.encoding_context(context);
    }
    fn ensure_workspace(&mut self, el: &ActiveEventLoop) -> bool {
        if self.workspace.is_none() {
            self.ledger.record(StartupAction::SpawnWorker);
            match Workspace::new(
                self.notify.clone(),
                std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
            ) {
                Ok(mut workspace) => {
                    workspace.recovery_root = self.recovery_root.clone();
                    let settings = self.settings.effective();
                    workspace.apply_resource_settings(&settings);
                    workspace.transcode_quota_bytes = settings.transcode_quota_bytes;
                    self.workspace = Some(workspace);
                }
                Err(error) => {
                    self.fail(el, error);
                    return false;
                }
            }
        }
        true
    }
    fn request_close(&mut self, _el: &ActiveEventLoop) {
        self.pending_close = Some(PendingClose::Application);
        (self.notify)();
    }
    fn drain_pending_close(&mut self, el: &ActiveEventLoop) {
        match self.pending_close.take() {
            Some(PendingClose::Application) => self.request_close_now(el, false),
            Some(PendingClose::ApplicationDiscarding(identities)) => {
                if self.discard_for_exit_identities(&identities) {
                    self.request_close_now(el, true);
                }
            }
            Some(PendingClose::ApplicationAfterSaves) => {
                if self.workspace.as_ref().is_some_and(|w| w.io_busy()) || self.lifecycle.busy() {
                    self.pending_close = Some(PendingClose::ApplicationAfterSaves);
                    el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(50)));
                } else if self
                    .workspace
                    .as_ref()
                    .is_some_and(|workspace| workspace.editors.iter().any(|editor| editor.dirty()))
                {
                    if let Some(workspace) = &mut self.workspace {
                        workspace.message = Some("Exit cancelled because one or more documents were not saved.".into());
                    }
                } else {
                    self.request_close_now(el, true);
                }
            }
            Some(PendingClose::Document(target)) => self.close_document_now(target),
            None => {}
        }
    }
    fn active_close_tab(&self) -> Option<u64> {
        self.views.pane_token(self.views.pane() as usize)
    }
    /// Runs the Save / Don't Save / Cancel prompt for one document. Returns
    /// `true` when the caller may close it right away.
    fn confirm_close_document(&mut self, index: usize, target: &mut CloseTarget) -> bool {
        if self.workspace.as_ref().is_some_and(|w| w.document_busy(index)) {
            if let Some(platform) = &self.platform {
                platform.pending_operation_notice();
            }
            return false;
        }
        if !self.workspace.as_ref().is_some_and(|w| w.editors[index].dirty()) {
            return true;
        }
        let Some(platform) = &self.platform else { return false };
        let name = self
            .app
            .tabs
            .get(index)
            .map_or("this document", String::as_str)
            .to_string();
        // Bind before the match: scrutinee temporaries would keep `self.platform`
        // borrowed across the arms that need `&mut self`.
        let choice = platform.confirm_save_document(&name);
        match choice {
            SaveChoice::Cancel => false,
            SaveChoice::DontSave => {
                if let Some(editor) = self
                    .workspace
                    .as_mut()
                    .and_then(|workspace| workspace.editors.get_mut(index))
                {
                    target.was_read_only = editor.read_only();
                    editor.set_read_only(true);
                    target.identity = editor.document_identity();
                }
                target.discarding = true;
                true
            }
            SaveChoice::Save => {
                if !self.save_index(index) {
                    return false;
                }
                self.pending_close = Some(PendingClose::Document(CloseTarget {
                    saving: true,
                    ..*target
                }));
                (self.notify)();
                false
            }
        }
    }
    /// Saves one document, asking for a location when it has none.
    /// Returns `false` when selection is cancelled or submission is rejected.
    fn save_index(&mut self, index: usize) -> bool {
        self.request_document_save(index, bareline_file_io::lifecycle::SaveOperation::Save)
    }
    fn close_document_now(&mut self, target: CloseTarget) {
        let Some(mut renderer) = self.renderer.take() else {
            return;
        };
        self.close_document_with_renderer(target, &mut renderer);
        self.renderer = Some(renderer);
    }
    fn close_document_with_renderer(
        &mut self,
        mut target: CloseTarget,
        renderer: &mut impl bareline_renderer::TextBackend,
    ) {
        let Some(index) = self
            .workspace
            .as_ref()
            .and_then(|workspace| Self::close_target_index(workspace, target.identity.0))
        else {
            return;
        };
        let editor = &self.workspace.as_ref().unwrap().editors[index];
        if editor.document_identity() != target.identity {
            if target.discarding
                && let Some(workspace) = &mut self.workspace
            {
                let editor = &mut workspace.editors[index];
                editor.set_read_only(target.was_read_only);
                editor.resume_recovery_after_discard();
                workspace.message = Some("Close cancelled because the document changed; recovery resumed.".into());
            }
            return;
        }
        target.index = index;
        if !target.discarding
            && !target.saving
            && !target.matches(self.app.active, editor.document_identity(), self.active_close_tab())
        {
            return;
        }
        if target.saving {
            // Waiting on the save the user already asked for.
            if self.workspace.as_ref().is_some_and(|w| w.document_busy(index))
                || self.lifecycle.preparing(target.identity)
            {
                self.pending_close = Some(PendingClose::Document(target));
                (self.notify)();
                return;
            }
            // A failed save leaves the document dirty; keep the tab and the message.
            if self.workspace.as_ref().is_some_and(|w| w.editors[index].dirty()) {
                return;
            }
        } else if !target.discarding && !self.confirm_close_document(index, &mut target) {
            return;
        }
        let mut closed = false;
        if let Some(workspace) = &mut self.workspace {
            let document_identity = workspace.editors[index].document_identity();
            let toast_identity = workspace.editors[index].snapshot().identity_token();
            match workspace.close(index, target.discarding, renderer) {
                Ok(()) => {
                    let prior_active = self.app.active;
                    self.launch.cancel_document(document_identity);
                    workspace.set_last_closed_read_only(target.was_read_only);
                    self.lifecycle.cancel_preflight(document_identity);
                    // A closed document leaves none of its notices behind (UX-60).
                    self.toasts.clear_document(toast_identity);
                    // Never leave a blank editor with dashes: closing the last tab
                    // opens a fresh Untitled and the status reads "Ready" (UX-31).
                    if workspace.editors.is_empty() {
                        let _ = workspace.new_document();
                        self.toasts.clear_all();
                        workspace.message = Some("Ready".into());
                    }
                    self.app.tabs = workspace.titles();
                    self.app.active = if prior_active > index {
                        prior_active - 1
                    } else if prior_active == index {
                        index.min(self.app.tabs.len().saturating_sub(1))
                    } else {
                        prior_active
                    };
                    closed = true;
                }
                Err(bareline_app::workspace::CloseError::RecoveryPending) => {
                    workspace.message = Some("Discard requested; waiting for durable recovery cleanup.".into());
                    self.pending_close = Some(PendingClose::Document(target));
                }
                Err(bareline_app::workspace::CloseError::RecoveryFailed(error)) => {
                    if target.discarding
                        && let Some(editor) = workspace.editors.get_mut(index)
                    {
                        editor.set_read_only(target.was_read_only);
                        editor.resume_recovery_after_discard();
                    }
                    workspace.message = Some(format!("Discard remains pending: {error}. Close again to retry."));
                }
                Err(_) => {}
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Sample handle counters after a close so `--diag handles` shows a leak as a
        // growing number the same way it does at startup.
        if closed && self.launch.diag_handles {
            launch::log_handle_counters(self.log_directory.as_deref());
        }
    }
    fn close_target_index(workspace: &Workspace, owner: u64) -> Option<usize> {
        workspace
            .editors
            .iter()
            .position(|editor| editor.document_identity().0 == owner)
    }
    fn request_close_now(&mut self, el: &ActiveEventLoop, confirmed: bool) {
        if self.settings.keymap_busy() || self.settings.controller.saving() {
            if let Some(platform) = &self.platform {
                platform.pending_operation_notice();
            }
            return;
        }
        if let Some(workspace) = &self.workspace {
            if workspace.io_busy() {
                self.platform.as_ref().unwrap().pending_operation_notice();
                return;
            }
        }
        if !confirmed && !self.confirm_exit() {
            return;
        }
        if !self.session_before_exit(el) {
            el.exit();
        }
    }
    /// Exit prompt: lists every unsaved document and offers Save All, Don't
    /// Save and Cancel. Returns `true` when the exit may continue now.
    fn confirm_exit(&mut self) -> bool {
        let unsaved: Vec<(usize, String)> = match &self.workspace {
            Some(workspace) => workspace
                .editors
                .iter()
                .enumerate()
                .filter(|(_, editor)| editor.dirty() || editor.busy())
                .map(|(index, _)| {
                    (
                        index,
                        self.app
                            .tabs
                            .get(index)
                            .cloned()
                            .unwrap_or_else(|| format!("Untitled {}", index + 1)),
                    )
                })
                .collect(),
            None => Vec::new(),
        };
        if unsaved.is_empty() {
            return true;
        }
        let names: Vec<String> = unsaved.iter().map(|(_, name)| name.clone()).collect();
        let choice = self.platform.as_ref().unwrap().confirm_save_all(&names);
        match choice {
            SaveChoice::Cancel => false,
            SaveChoice::DontSave => {
                let indexes: Vec<_> = unsaved.iter().map(|(index, _)| *index).collect();
                self.discard_for_exit_indexes(&indexes)
            }
            SaveChoice::Save => {
                self.start_save_all();
                self.pending_close = Some(PendingClose::ApplicationAfterSaves);
                (self.notify)();
                false
            }
        }
    }
    fn discard_for_exit_indexes(&mut self, indexes: &[usize]) -> bool {
        let consents: Vec<_> = self
            .workspace
            .as_mut()
            .map(|workspace| {
                indexes
                    .iter()
                    .filter_map(|index| {
                        workspace.editors.get_mut(*index).map(|editor| {
                            let (owner, revision) = editor.document_identity();
                            let was_read_only = editor.read_only();
                            editor.set_read_only(true);
                            DiscardConsent {
                                owner,
                                revision,
                                was_read_only,
                            }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.discard_for_exit_identities(&consents)
    }
    fn discard_for_exit_identities(&mut self, consents: &[DiscardConsent]) -> bool {
        let Some(workspace) = &mut self.workspace else {
            return true;
        };
        let mut indexes = Vec::with_capacity(consents.len());
        for consent in consents {
            let Some((index, revision)) = workspace.editors.iter().enumerate().find_map(|(index, editor)| {
                let (owner, revision) = editor.document_identity();
                (owner == consent.owner).then_some((index, revision))
            }) else {
                Self::cancel_exit_discard(workspace, consents);
                return false;
            };
            if revision != consent.revision {
                Self::cancel_exit_discard(workspace, consents);
                return false;
            }
            indexes.push(index);
        }
        match workspace.discard_recoveries(&indexes) {
            bareline_file_io::recovery_retirement::DiscardPoll::Durable => {
                if Self::exit_discard_still_current(workspace, consents) {
                    true
                } else {
                    Self::cancel_exit_discard(workspace, consents);
                    false
                }
            }
            bareline_file_io::recovery_retirement::DiscardPoll::CleanupPending(error) => {
                if Self::exit_discard_still_current(workspace, consents) {
                    workspace.message = Some(format!("Documents discarded; recovery cleanup pending: {error}"));
                    true
                } else {
                    Self::cancel_exit_discard(workspace, consents);
                    false
                }
            }
            bareline_file_io::recovery_retirement::DiscardPoll::Pending => {
                workspace.message = Some("Discard requested; waiting for durable recovery tombstones.".into());
                self.pending_close = Some(PendingClose::ApplicationDiscarding(consents.to_vec()));
                false
            }
            bareline_file_io::recovery_retirement::DiscardPoll::TombstoneFailed(error) => {
                Self::cancel_exit_discard(workspace, consents);
                workspace.message = Some(format!(
                    "Exit paused: discard tombstone failed: {error}. Quit again to retry."
                ));
                false
            }
        }
    }
    fn exit_discard_still_current(workspace: &Workspace, consents: &[DiscardConsent]) -> bool {
        workspace.editors.iter().all(|editor| {
            if !editor.dirty() && !editor.busy() {
                return true;
            }
            let (owner, revision) = editor.document_identity();
            consents
                .iter()
                .any(|consent| consent.owner == owner && consent.revision == revision)
        })
    }
    fn cancel_exit_discard(workspace: &mut Workspace, consents: &[DiscardConsent]) {
        for consent in consents {
            if let Some(editor) = workspace
                .editors
                .iter_mut()
                .find(|editor| editor.document_identity().0 == consent.owner)
            {
                editor.set_read_only(consent.was_read_only);
                editor.resume_recovery_after_discard();
            }
        }
        workspace.message = Some("Exit cancelled because the document set changed; recovery resumed.".into());
    }
    /// Handle a contributed command ID: the inline pre-chain handlers
    /// (encoding, search, migration, shell integration, theme cycle, dynamic
    /// invoke, transcode, update, Settings-tab close), then the table-driven
    /// [`DISPATCH_CHAIN`], then the search-panel and editor-surface fallbacks.
    /// Early returns match `dispatch`'s old arm exactly; the fall-through path
    /// issues the redraw that `dispatch` used to perform after the match.
    fn dispatch_contributed(&mut self, el: &ActiveEventLoop, id: bareline_commands::CommandId) {
        if id.0 == "profile.migration.retry" {
            let result = self
                .profile_initialization
                .retry()
                .and_then(|()| self.profile_initialization.schedule(self.notify.clone()).map(|_| ()));
            match result {
                Ok(()) => self.profile_initialization_message("Profile migration retry started".into()),
                Err(error) => self.profile_initialization_message(error),
            }
            return;
        }
        if id.0 == "search.folder" && !self.ensure_workspace(el) {
            return;
        }
        if self.encoding_dispatch(el, id.0) {
            return;
        }
        if self.search_command(id.0) {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.migration_dispatch(el, id.0) || self.shell_integration_command(el, id.0) {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if id.0 == "view.theme.cycle" {
            // Flip the resolved light/dark appearance so overlays can be
            // checked against both themes without changing the OS setting.
            self.settings.controller.system.dark = !self.settings.controller.system.dark;
            self.settings.invalidate_cache();
            self.applied_settings = None;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if id.0 == "internal.dynamic.invoke" {
            if !self.profile_initialization.settled() {
                self.profile_initialization_message("Profile storage is still being reconciled".into());
                return;
            }
            if let Some(identity) = self.palette.take_dynamic_activation()
                && let Err(error) = self.extensions_invoke_contribution(identity)
                && let Some(workspace) = &mut self.workspace
            {
                workspace.message = Some(error);
            }
            return;
        }
        if id.0 == "file.transcode.resume" || id.0 == "file.transcode.cancel" {
            if let Some(workspace) = &mut self.workspace {
                if id.0.ends_with("cancel") {
                    workspace.cancel_file_operations();
                } else if let Some((_, used, required, limit)) = workspace.paused_transcode_info() {
                    workspace.resume_transcode(limit.saturating_mul(2).max(used.saturating_add(required)));
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        match id.0 {
            "update.check" => {
                self.update.check(self.notify.clone());
            }
            "update.apply_on_exit" => self.update.apply_on_exit(),
            "update.cancel" => self.update.cancel(),
            "update.discard" => self.update.discard(self.notify.clone()),
            _ => {}
        }
        if id.0.starts_with("update.") {
            if self.ensure_workspace(el) {
                self.workspace.as_mut().unwrap().message = Some(self.update.status.clone());
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if id.0 == "file.close" && self.settings.controller.open {
            // Settings behaves like a tab: Ctrl+W closes it.
            self.settings.controller.dismiss();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        // Table-driven routing: each command ID is claimed by exactly one
        // handler (see `command_route`), so the dispatch order lives in one
        // data structure instead of a hand-maintained boolean chain.
        let mut handled = false;
        for handler in DISPATCH_CHAIN {
            if handler(self, el, id.0) {
                handled = true;
                break;
            }
        }
        if handled {
            return;
        }
        if let Some(workspace) = &mut self.workspace {
            match id.0 {
                "search.open_documents" => {
                    workspace.find.blur();
                    workspace.search_panel.show();
                    workspace
                        .search_panel
                        .set_scope(bareline_app::search_panel::SearchScope::OpenDocuments);
                    workspace.search_focus = true;
                }
                "search.cancel_panel" => {
                    self.search.close_folder();
                    workspace.search_panel.cancel();
                }
                "search.close_panel" => {
                    self.search.close_folder();
                    workspace.search_panel.hide();
                    workspace.search_focus = false;
                }
                // Only genuine editor commands reach the surface fallback.
                // Anything else is an unrouted ID (the inventory validation
                // rejects these at export time); report it plainly instead of
                // handing it to the editor with a misleading "busy" message.
                _ if id.0.starts_with("editor.") => {
                    if let Some(editor) = workspace.editors.get_mut(self.app.active)
                        && let Err(error) = editor.execute_power(id.0)
                    {
                        workspace.message = Some(error);
                    }
                }
                other => {
                    workspace.message = Some(format!("No command handler is registered for \"{other}\"."));
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// Show the About dialog and act on its choice (copy diagnostics, or open
    /// LICENSE / THIRD-PARTY-NOTICES). Falls through so `dispatch` redraws.
    fn dispatch_about(&mut self, el: &ActiveEventLoop) {
        let renderer = self
            .renderer
            .as_ref()
            .map(|r| if r.software { "Software" } else { "Hardware" })
            .unwrap_or("Not initialized");
        let details = format!(
            "Version: {}\nBuild: {}\nArchitecture: {}\nRenderer: {}\nMode: {}\nLocal diagnostics: {}",
            env!("CARGO_PKG_VERSION"),
            option_env!("BARELINE_BUILD_HASH").unwrap_or("unknown"),
            std::env::consts::ARCH,
            renderer,
            if self.shell_integration.portable {
                "Portable"
            } else {
                "Installed profile"
            },
            if self.log.is_some() { "Enabled" } else { "Unavailable" }
        );
        let result = self.platform.as_ref().map(|p| p.about_details(&details));
        match result {
            Some(Ok(Some(bareline_platform_windows::AboutAction::CopyDiagnostics))) => {
                if let Some(platform) = &self.platform {
                    if let Err(error) = platform.set_clipboard_text(&details) {
                        platform.operation_failed(&error.to_string());
                    }
                }
            }
            Some(Ok(Some(action))) => {
                let name = match action {
                    bareline_platform_windows::AboutAction::License => "LICENSE",
                    bareline_platform_windows::AboutAction::ThirdPartyNotices => "THIRD-PARTY-NOTICES.md",
                    bareline_platform_windows::AboutAction::CopyDiagnostics => {
                        unreachable!()
                    }
                };
                match std::env::current_exe().and_then(|path| {
                    path.parent()
                        .map(|parent| parent.join(name))
                        .ok_or_else(|| std::io::Error::other("Executable directory unavailable"))
                }) {
                    Ok(path) => {
                        if self.ensure_workspace(el) {
                            self.workspace.as_mut().unwrap().open(path);
                        }
                    }
                    Err(error) => {
                        if let Some(platform) = &self.platform {
                            platform.operation_failed(&error.to_string());
                        }
                    }
                }
            }
            Some(Err(error)) => {
                if let Some(platform) = &self.platform {
                    platform.operation_failed(&error.to_string());
                }
            }
            _ => {}
        }
    }
    /// Route clipboard and text-field edit actions (Copy/Cut/Paste/Undo/Redo/
    /// SelectAll) to whichever text surface owns focus — a folder-search or
    /// open-documents field, a Settings/Shortcuts field, the command palette, a
    /// split pane, or the Find field. Returns `true` when the action was
    /// consumed and `dispatch` should stop.
    fn route_text_and_clipboard(&mut self, el: &ActiveEventLoop, action: Action) -> bool {
        if !self.palette.open
            && matches!(
                action,
                Action::SelectAll | Action::Copy | Action::Cut | Action::Paste | Action::Undo | Action::Redo
            )
        {
            let paste = if action == Action::Paste {
                self.platform
                    .as_ref()
                    .and_then(|platform| platform.clipboard_text().ok())
            } else {
                None
            };
            let mut copied = None;
            let mut owned = false;
            {
                let field = if self.search_folder_open() {
                    self.search_folder_field()
                } else {
                    self.workspace
                        .as_mut()
                        .filter(|workspace| workspace.search_panel.open && workspace.search_focus)
                        .map(|workspace| &mut workspace.search_panel.field)
                };
                if let Some(field) = field {
                    owned = true;
                    match action {
                        Action::SelectAll => field.select_all(),
                        Action::Undo => field.undo(false),
                        Action::Redo => field.undo(true),
                        Action::Paste => {
                            if let Some(value) = paste {
                                field.commit(&value);
                            }
                        }
                        Action::Copy | Action::Cut => {
                            copied = Some(field.selected().to_owned());
                        }
                        _ => {}
                    }
                }
            }
            if let Some(value) = copied
                && !value.is_empty()
                && self
                    .platform
                    .as_ref()
                    .is_some_and(|platform| platform.set_clipboard_text(&value).is_ok())
                && action == Action::Cut
            {
                let field = if self.search_folder_open() {
                    self.search_folder_field()
                } else {
                    self.workspace
                        .as_mut()
                        .map(|workspace| &mut workspace.search_panel.field)
                };
                if let Some(field) = field {
                    field.insert("");
                }
            }
            if owned || self.search_folder_open() {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return true;
            }
        }
        if self.settings_text_action(action)
            || self.shortcuts_action(action)
            || (!self.palette.open
                && (self.power.open
                    || self
                        .workspace
                        .as_ref()
                        .is_none_or(|w| !w.find.has_focus() && !w.search_focus))
                && self.power_action(action))
        {
            return true;
        }
        if self.palette.open
            && matches!(
                action,
                Action::SelectAll | Action::Copy | Action::Cut | Action::Paste | Action::Undo | Action::Redo
            )
        {
            let field = &mut self.palette.field;
            let platform = self.platform.as_ref().unwrap();
            match action {
                Action::SelectAll => field.select_all(),
                Action::Undo => field.undo(false),
                Action::Redo => field.undo(true),
                Action::Paste => {
                    if let Ok(value) = platform.clipboard_text() {
                        field.commit(&value);
                    }
                }
                Action::Copy | Action::Cut
                    if !field.selected().is_empty()
                        && platform.set_clipboard_text(field.selected()).is_ok()
                        && action == Action::Cut =>
                {
                    field.insert("");
                }
                _ => {}
            }
            self.palette.refresh(
                &self.app.commands,
                &self.command_context(),
                &self.settings.keymap.keymap,
            );
            self.window.as_ref().unwrap().request_redraw();
            return true;
        }
        // Clipboard and edit actions belong to the palette before either split pane.
        if self.views_action(el, action) {
            return true;
        }
        if let Some(workspace) = &mut self.workspace
            && workspace.find.has_focus()
            && matches!(
                action,
                Action::SelectAll | Action::Copy | Action::Cut | Action::Paste | Action::Undo | Action::Redo
            )
        {
            let field = workspace.find.active_field();
            let platform = self.platform.as_ref().unwrap();
            match action {
                Action::SelectAll => field.select_all(),
                Action::Undo => field.undo(false),
                Action::Redo => field.undo(true),
                Action::Paste => match platform.clipboard_text() {
                    Ok(value) => {
                        if !field.commit(&value) {
                            workspace.message = Some("Find accepts a single line up to 16 KiB.".into());
                        }
                    }
                    Err(_) => workspace.message = Some("Clipboard text is unavailable.".into()),
                },
                Action::Copy | Action::Cut if !field.selected().is_empty() => {
                    if platform.set_clipboard_text(field.selected()).is_ok() {
                        if action == Action::Cut {
                            field.insert("");
                        }
                    } else {
                        workspace.message = Some("Clipboard write failed; selection was preserved.".into());
                    }
                }
                _ => {}
            }
            self.window.as_ref().unwrap().request_redraw();
            return true;
        }
        false
    }
    fn dispatch(&mut self, el: &ActiveEventLoop, action: Action) {
        if self.session.closing() {
            return;
        }
        // Modal controls dispatch directly. Reject commands originating from
        // dimmed editor, menu, toolbar, or stale accessibility state.
        if self.modal.is_some() {
            return;
        }
        self.sync_contributions();
        self.record_acknowledged_inputs();
        if self.route_text_and_clipboard(el, action) {
            return;
        }
        match action {
            Action::Contributed(id) => {
                self.dispatch_contributed(el, id);
                return;
            }
            Action::Palette => {
                if !self.app.palette {
                    let invoker = if self.shortcuts.open {
                        if self.shortcuts.binding_focus { 19001 } else { 19000 }
                    } else if self.settings.controller.open {
                        self.settings
                            .controller
                            .semantics()
                            .iter()
                            .find(|node| node.focused)
                            .map_or(8000, |node| node.id.0)
                    } else {
                        self.workspace.as_ref().map_or(2, |workspace| {
                            if workspace.find.has_focus() {
                                6000
                            } else if workspace.search_focus {
                                20000
                            } else {
                                2
                            }
                        })
                    };
                    self.ui_focus.set_targets(
                        [2, 6000, 6001, 20000, 8000, 19000, 19001, invoker]
                            .into_iter()
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter()
                            .map(|id| bareline_ui::focus::FocusTarget {
                                id: bareline_ui::ViewId(id),
                                enabled: true,
                            })
                            .collect(),
                    );
                    self.ui_focus.focus(bareline_ui::ViewId(invoker));
                    self.ui_focus.open_layer(
                        bareline_ui::ViewId(invoker),
                        vec![bareline_ui::focus::FocusTarget {
                            id: bareline_ui::ViewId(11000),
                            enabled: true,
                        }],
                    );
                    self.ui_router.popup_capture = Some(bareline_ui::ViewId(11000));
                }
                if let Some(workspace) = &mut self.workspace {
                    workspace.find.blur();
                    if let Some(editor) = workspace.editors.get_mut(self.app.active) {
                        editor.cancel_composition();
                    }
                }
                self.app.apply(action);
                if self.app.palette {
                    self.palette.show(
                        &self.app.commands,
                        &self.command_context(),
                        &self.settings.keymap.keymap,
                    );
                } else {
                    self.palette.dismiss();
                    self.finish_palette_focus();
                }
            }
            Action::Find
            | Action::FindNext
            | Action::FindPrevious
            | Action::FindClose
            | Action::FindMatchCase
            | Action::Replace
            | Action::ReplaceOne
            | Action::ReplaceAll
            | Action::FindMode
            | Action::FindCancel
            | Action::FindWholeWord => {
                if let Some(workspace) = &mut self.workspace {
                    match action {
                        Action::Find => {
                            let scope = if workspace.find.query().selection.is_some() {
                                bareline_app::search_panel::SearchScope::Selection
                            } else {
                                bareline_app::search_panel::SearchScope::CurrentDocument
                            };
                            workspace.search_panel.set_scope(scope);
                            if let Some(editor) = workspace.editors.get_mut(self.app.active) {
                                editor.cancel_composition();
                                workspace.find.show();
                            }
                        }
                        Action::FindClose => workspace.find.hide(),
                        Action::Replace => {
                            let scope = if workspace.find.query().selection.is_some() {
                                bareline_app::search_panel::SearchScope::Selection
                            } else {
                                bareline_app::search_panel::SearchScope::CurrentDocument
                            };
                            workspace.search_panel.set_scope(scope);
                            if let Some(editor) = workspace.editors.get_mut(self.app.active) {
                                editor.cancel_composition();
                                workspace.find.show_replace();
                            }
                        }
                        Action::ReplaceOne | Action::ReplaceAll => {
                            workspace.replace(self.app.active, action == Action::ReplaceAll)
                        }
                        Action::FindMode => workspace.find.toggle_mode(),
                        Action::FindCancel => workspace.cancel_search(),
                        Action::FindWholeWord => workspace.find.whole_word = !workspace.find.whole_word,
                        Action::FindMatchCase => workspace.find.case_sensitive = !workspace.find.case_sensitive,
                        _ => workspace.find_next(self.app.active, action == Action::FindPrevious),
                    }
                }
            }
            Action::Close => {
                if self.pending_close.is_none()
                    && let Some(editor) = self.workspace.as_ref().and_then(|w| w.editors.get(self.app.active))
                {
                    self.pending_close = Some(PendingClose::Document(CloseTarget {
                        index: self.app.active,
                        identity: editor.document_identity(),
                        tab: self.active_close_tab(),
                        saving: false,
                        discarding: false,
                        was_read_only: editor.read_only(),
                    }));
                    (self.notify)();
                }
            }
            Action::CancelFileOperations => {
                if let Some(workspace) = &mut self.workspace {
                    workspace.cancel_file_operations();
                }
            }
            Action::Quit => self.request_close(el),
            Action::About => self.dispatch_about(el),
            Action::New if self.prototype.is_none() => {
                if !self.ensure_workspace(el) {
                    return;
                }
                match self.workspace.as_mut().unwrap().new_document() {
                    Ok(()) => {
                        self.app.tabs = self.workspace.as_ref().unwrap().titles();
                        self.app.active = self.app.tabs.len() - 1;
                    }
                    Err(error) => {
                        self.fail(el, format!("new document: {error:?}"));
                        return;
                    }
                }
            }
            Action::Undo | Action::Redo | Action::SelectAll => {
                if let Some(editor) = self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active)) {
                    editor.enqueue(match action {
                        Action::Undo => Input::Undo,
                        Action::Redo => Input::Redo,
                        _ => Input::SelectAll,
                    });
                }
            }
            Action::Copy | Action::Cut | Action::Paste => {
                if let Some(editor) = self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active)) {
                    let platform = self.platform.as_ref().unwrap();
                    if action == Action::Paste {
                        match platform.clipboard_text() {
                            Ok(text) => editor.commit_with_origin(text, bareline_document::history::EditOrigin::Paste),
                            Err(_) => {
                                editor.viewport_mut().error =
                                    Some("Clipboard text is unavailable or exceeds the 4 MiB limit.".into())
                            }
                        }
                    } else {
                        match editor.selected_text() {
                            Ok(text) if !text.is_empty() => match platform.set_clipboard_text(&text) {
                                Ok(()) if action == Action::Cut => {
                                    self.power.copied(&text);
                                    editor.enqueue(Input::Insert(String::new()));
                                }
                                Ok(()) => {
                                    self.power.copied(&text);
                                }
                                Err(_) => {
                                    editor.viewport_mut().error =
                                        Some("Could not write text to the clipboard. Selection was preserved.".into())
                                }
                            },
                            Ok(_) => {}
                            Err(message) => editor.viewport_mut().error = Some(message.into()),
                        }
                    }
                }
            }
            Action::Open if self.prototype.is_none() => {
                let result = self.platform.as_ref().unwrap().open_files();
                match result {
                    Ok(paths) => {
                        if !paths.is_empty() && self.ensure_workspace(el) {
                            for path in paths {
                                self.workspace.as_mut().unwrap().open(path);
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(workspace) = &mut self.workspace {
                            workspace.message = Some(error);
                        }
                    }
                }
            }
            Action::Save | Action::SaveAs if self.prototype.is_none() => {
                let Some(workspace) = &self.workspace else {
                    return;
                };
                if self.app.active >= workspace.editors.len() {
                    return;
                }
                let operation = if action == Action::Save {
                    bareline_file_io::lifecycle::SaveOperation::Save
                } else {
                    bareline_file_io::lifecycle::SaveOperation::SaveAs
                };
                self.request_document_save(self.app.active, operation);
            }
            _ => self.app.apply(action),
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    fn fail(&mut self, el: &ActiveEventLoop, error: impl std::fmt::Display) {
        eprintln!("event=platform_failed error={error}");
        if let Some(workspace) = &mut self.workspace
            && !workspace.editors.is_empty()
        {
            let message = error.to_string();
            workspace.message = Some(message.clone());
            if let Some(platform) = &self.platform {
                platform.operation_failed(&message);
            }
            return;
        }
        self.failed = true;
        el.exit();
    }
}
impl ApplicationHandler for Shell {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        self.ledger.record(StartupAction::CreateWindow);
        let window = {
            let _window_phase = bareline_diagnostics::startup_span(StartupAction::CreateWindow);
            match el.create_window(
                Window::default_attributes()
                    .with_title("Bareline")
                    .with_inner_size(LogicalSize::new(1200.0, 760.0))
                    .with_min_inner_size(LogicalSize::new(640.0, 360.0))
                    // Must be created hidden: WindowsAccessibility::new refuses a
                    // visible HWND (accesskit installs its UIA subclass before the
                    // window is shown). We reveal it below, after accessibility and
                    // the renderer are ready.
                    .with_visible(false),
            ) {
                Ok(window) => window,
                Err(e) => {
                    self.fail(el, e);
                    return;
                }
            }
        };
        let handle = match window.window_handle() {
            Ok(h) => h.as_raw(),
            Err(e) => {
                self.fail(el, e);
                return;
            }
        };
        let RawWindowHandle::Win32(handle) = handle else {
            self.fail(el, "Expected Win32 window");
            return;
        };
        // SAFETY: window is owned below until after platform and renderer are dropped.
        let menu_model = bareline_app::menus::curated_model(&self.app.commands);
        match unsafe { WindowsPlatform::new(handle.hwnd.get(), &self.app.commands, menu_model) } {
            Ok(platform) => self.platform = Some(platform),
            Err(e) => {
                self.fail(el, e);
                return;
            }
        }
        let initial = bareline_app::accessibility::snapshot("Bareline", 1200.0, 760.0, None, Vec::new(), 1);
        match unsafe {
            bareline_platform_windows::WindowsAccessibility::new(handle.hwnd.get(), initial, self.notify.clone())
        } {
            Ok(mut provider) => {
                provider.set_text_sources(self.accessibility_text_sources());
                self.accessibility = Some(provider);
            }
            Err(error) => {
                self.fail(el, error);
                return;
            }
        }
        window.set_ime_allowed(true);
        // Reveal now that accessibility is attached to the still-hidden window.
        // set_visible drives winit's own flag diff, so its cached WS_VISIBLE stays
        // in sync and an external ShowWindow(SW_SHOW) no longer fights it.
        window.set_visible(!self.smoke);
        window.request_redraw();
        self.window = Some(window);
        if self.smoke {
            // Hidden windows do not receive WM_PAINT. Exercise the real renderer explicitly.
            self.window_event(el, self.window.as_ref().unwrap().id(), WindowEvent::RedrawRequested);
        }
    }
    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(&event, WindowEvent::CursorLeft { .. } | WindowEvent::Focused(false))
            && self
                .workspace
                .as_mut()
                .is_some_and(|workspace| workspace.find.dismiss_tooltip())
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        if self.session.closing()
            && matches!(
                event,
                WindowEvent::KeyboardInput { .. }
                    | WindowEvent::Ime(_)
                    | WindowEvent::DroppedFile(_)
                    | WindowEvent::MouseInput { .. }
                    | WindowEvent::MouseWheel { .. }
            )
        {
            return;
        }
        if matches!(&event, WindowEvent::Focused(true)) {
            self.watch_focus_check();
        }
        if matches!(&event, WindowEvent::ThemeChanged(_) | WindowEvent::Focused(true)) {
            self.applied_settings = None;
        }
        if self.editor_has_input_focus()
            && matches!(
                &event,
                WindowEvent::KeyboardInput { .. }
                    | WindowEvent::Ime(_)
                    | WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        ..
                    }
            )
        {
            if self.views.pane() == 1 {
                self.views.reset_secondary_caret_blink();
            } else if let Some(editor) = self
                .workspace
                .as_mut()
                .and_then(|workspace| workspace.editors.get_mut(self.app.active))
            {
                editor.reset_caret_blink();
            }
        }
        if self.settings.language_change.take().is_some() {
            self.applied_settings = None;
        }
        self.toolbar_refresh();
        if self.modal_event(el, &event) {
            return;
        }
        // A click on a toast's × dismisses it before any overlay sees the event.
        if let WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button: MouseButton::Left,
            ..
        } = &event
            && self.toasts.hit(self.pointer)
        {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        let editor_bounds = self.editor_bounds();
        let editor_pointer = self.editor_pointer();
        if self.recovery_event(el, &event)
            || self.macros_event(el, &event)
            || self.settings_keymap_event(el, &event)
            || self.utilities_event(el, &event)
            || self.encoding_event(el, &event)
            || (self.power.open && self.power_event(el, &event))
            || self.goto_event(el, &event)
            || self.charsets_event(el, &event)
            || self.run_prompt_event(el, &event)
            || self.shortcuts_event(el, &event)
            || self.toolbar_event(el, &event)
            || self.settings_event(el, &event)
            || self.extensions_event(el, &event)
            || self.language_event(el, &event)
            || self.compare_event(el, &event)
            || self.search_overlay_event(&event)
            || self.watch_event(el, &event)
            || self.scrolling_event(&event)
            || (!self.power.open && self.power_event(el, &event))
            || self.panels_event(el, &event)
            || self.views_event(el, &event)
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                if !self.shell_integration.keep_in_tray || self.hide_to_tray().is_err() {
                    self.request_close(el);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let logical = position.to_logical::<f32>(self.window.as_ref().unwrap().scale_factor());
                self.pointer = Point {
                    x: logical.x,
                    y: logical.y,
                };
                let find_pointer = Point {
                    x: logical.x - editor_bounds.x,
                    y: logical.y - editor_bounds.y,
                };
                let (find_inset, find_width) = self.views.find_horizontal_geometry(editor_bounds.width);
                let find_pointer = Point {
                    x: find_pointer.x - find_inset,
                    y: find_pointer.y,
                };
                let now_ms = tooltip_clock_ms();
                if self
                    .workspace
                    .as_mut()
                    .is_some_and(|workspace| workspace.find.hover_toggles(find_width, find_pointer, now_ms))
                {
                    self.window.as_ref().unwrap().request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                self.on_mouse_pressed_left(el, editor_bounds, editor_pointer);
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                let window = self.window.as_ref().unwrap();
                if !self.app.palette
                    && let (Some(workspace), Some(renderer)) = (&mut self.workspace, &self.renderer)
                    && workspace.find.open
                {
                    let action = workspace
                        .find
                        .pointer(
                            editor_bounds.width,
                            editor_pointer,
                            false,
                            renderer,
                            self.modifiers.shift_key(),
                        )
                        .ok()
                        .flatten();
                    window.request_redraw();
                    if let Some(action) = action {
                        self.dispatch(el, find_action(action));
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => {
                self.on_mouse_pressed_right(el);
            }
            WindowEvent::Ime(event) => {
                self.on_ime(event);
            }
            WindowEvent::Focused(false) => {
                if let Some(workspace) = &mut self.workspace {
                    workspace.find.active_field().cancel();
                }
                if let Some(editor) = self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active)) {
                    editor.cancel_composition();
                }
                if let Some(p) = &mut self.prototype {
                    p.cancel();
                }
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.on_key_pressed(el, event, editor_bounds);
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let window = self.window.as_ref().unwrap();
                if let Some(editor) = self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active)) {
                    let (horizontal, vertical, zoom) = match delta {
                        MouseScrollDelta::LineDelta(x, y) => (-x as f64 * 72.0, -y as f64 * 72.0, y),
                        MouseScrollDelta::PixelDelta(p) => (
                            -p.x / window.scale_factor(),
                            -p.y / window.scale_factor(),
                            (p.y / window.scale_factor() / 72.0) as f32,
                        ),
                    };
                    if self.modifiers.control_key() && !self.modifiers.alt_key() {
                        editor.zoom_by(zoom);
                    } else {
                        editor.scroll_horizontal(horizontal);
                        match editor {
                            bareline_app::workspace::WorkspaceEditor::Paged(paged) => {
                                if let Err(error) = paged.scroll_viewport(vertical, editor_bounds.height) {
                                    paged.viewport_mut().error = Some(error);
                                }
                            }
                            bareline_app::workspace::WorkspaceEditor::Resident(editor) => {
                                editor.scroll(vertical, editor_bounds.height)
                            }
                        }
                    }
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.on_redraw(el, editor_bounds);
            }
            _ => {}
        }
    }
}
impl Shell {
    fn on_mouse_pressed_left(
        &mut self,
        el: &ActiveEventLoop,
        editor_bounds: bareline_renderer::Rect,
        editor_pointer: Point,
    ) {
        if self.palette.open {
            let context = self.command_context();
            let mut command = None;
            let mut router = std::mem::take(&mut self.ui_router);
            let size = self.window.as_ref().unwrap().inner_size();
            router.dispatch(
                bareline_ui::controls::UiEvent::PointerDown(self.pointer),
                self.ui_focus.focused(),
                &[bareline_ui::focus::RouteTarget {
                    id: bareline_ui::ViewId(11000),
                    parent: None,
                    bounds: bareline_ui::rect(0.0, 0.0, size.width as f32, size.height as f32),
                    enabled: true,
                }],
                |_, _| {
                    command = self.renderer.as_ref().and_then(|renderer| {
                        self.palette
                            .click(renderer, self.pointer, &self.app.commands, &context)
                            .ok()
                            .flatten()
                    });
                    bareline_ui::focus::DispatchResult {
                        handled: true,
                        invalidated: None,
                    }
                },
            );
            self.ui_router = router;
            self.finish_palette_focus();
            self.app.palette = self.palette.open;
            if let Some(action) = command.and_then(|id| self.app.commands.dispatch_in(id, &context).ok()) {
                self.dispatch(el, action);
            }
            self.window.as_ref().unwrap().request_redraw();
            return;
        }
        // A click on a status-bar picker segment opens its picker (UX-40).
        if let Some(command) = self
            .status_pickers
            .iter()
            .find(|(rect, _)| rect.contains(self.pointer))
            .map(|(_, command)| *command)
        {
            self.dispatch(el, Action::Contributed(bareline_commands::CommandId(command)));
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if let Some(workspace) = &mut self.workspace {
            let height = editor_bounds.height;
            if workspace.search_panel.open
                && editor_pointer.y >= height - 24.0 - workspace.search_panel.height()
                && editor_pointer.y < height - 24.0
            {
                workspace.search_focus = true;
                workspace.find.blur();
                if let Some((source, range)) = workspace.search_panel.pointer(editor_pointer)
                    && let Some(index) = workspace.activate_search(source, range)
                {
                    self.app.active = index;
                }
                if let Some(tab) = workspace.search_panel.take_tab_requested() {
                    search::select_panel_tab(workspace, tab);
                }
                self.window.as_ref().unwrap().request_redraw();
                return;
            }
            workspace.search_focus = false;
        }
        let window = self.window.as_ref().unwrap();
        if !self.app.palette
            && let (Some(workspace), Some(renderer)) = (&mut self.workspace, &self.renderer)
        {
            if workspace.find.open && (34.0..34.0 + workspace.find.height()).contains(&editor_pointer.y) {
                if let Err(error) = workspace.find.pointer(
                    editor_bounds.width,
                    editor_pointer,
                    true,
                    renderer,
                    self.modifiers.shift_key(),
                ) {
                    workspace.message = Some(format!("Find hit test failed: {error:?}"));
                }
                window.request_redraw();
                return;
            }
            workspace.find.blur();
        }
        if self.app.click_tab(
            window.inner_size().width as f32 / window.scale_factor() as f32,
            self.pointer,
        ) {
            window.request_redraw();
            return;
        }
        if self.pointer.y >= 34.0
            && !self.app.palette
            && let (Some(editor), Some(renderer)) = (
                self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active)),
                &self.renderer,
            )
        {
            if matches!(editor, bareline_app::workspace::WorkspaceEditor::Paged(paged) if !paged.paged_frame_state().ready)
            {
                return;
            }
            if let Err(error) = editor.click(renderer, editor_pointer, self.modifiers.shift_key()) {
                self.fail(el, format!("editor hit test: {error:?}"));
                return;
            }
            window.request_redraw();
            return;
        }
        if (34.0..68.0).contains(&self.pointer.y)
            && !self.app.palette
            && let (Some(p), Some(renderer)) = (&mut self.prototype, &self.renderer)
        {
            if let Err(error) = p.click(renderer, self.pointer) {
                self.fail(el, format!("text hit test: {error:?}"));
                return;
            }
            self.window.as_ref().unwrap().request_redraw();
        }
    }
    fn on_mouse_pressed_right(&mut self, el: &ActiveEventLoop) {
        let panel_commands = self.panels_context_commands();
        if self.pointer.y < 34.0
            && let Some(window) = &self.window
        {
            self.app.click_tab(
                window.inner_size().width as f32 / window.scale_factor() as f32,
                self.pointer,
            );
        }
        let placement = self.window.as_ref().and_then(|window| {
            window
                .inner_position()
                .ok()
                .map(|origin| (origin, window.scale_factor()))
        });
        if let Some((origin, scale)) = placement {
            let screen_x = origin.x + (self.pointer.x as f64 * scale) as i32;
            let screen_y = origin.y + (self.pointer.y as f64 * scale) as i32;
            if self.pointer.y < 34.0 {
                self.tab_context_menu(el, screen_x, screen_y);
            } else {
                let context = self.command_context();
                let commands: Vec<_> = if let Some(commands) = panel_commands {
                    commands
                } else {
                    [
                        "edit.undo",
                        "edit.redo",
                        "edit.cut",
                        "edit.copy",
                        "edit.paste",
                        "edit.select_all",
                        "search.find",
                    ]
                    .into_iter()
                    .map(bareline_commands::CommandId)
                    .collect()
                };
                let result = self.platform.as_ref().unwrap().context_menu_in(
                    screen_x,
                    screen_y,
                    &self.app.commands,
                    &context,
                    &self.settings.keymap.keymap,
                    &commands,
                );
                match result {
                    Ok(Some(action)) => self.dispatch(el, action),
                    Ok(None) => {}
                    Err(error) => self.fail(el, error),
                }
            }
        }
    }
    fn on_ime(&mut self, event: Ime) {
        if self.app.palette {
            let context = self.command_context();
            // Borrow the keymap in place rather than cloning it per IME
            // event (ARCH-17): `self.palette` and `self.settings` are
            // disjoint fields.
            match event {
                Ime::Preedit(value, cursor) => self.palette.preedit(value, cursor),
                Ime::Commit(value) => {
                    self.palette
                        .commit(&value, &self.app.commands, &context, &self.settings.keymap.keymap);
                }
                Ime::Disabled => self.palette.field.cancel(),
                Ime::Enabled => {}
            }
            self.window.as_ref().unwrap().request_redraw();
            return;
        }
        if let Some(workspace) = &mut self.workspace
            && workspace.search_focus
            && workspace.search_panel.open
        {
            let field = &mut workspace.search_panel.field;
            match event {
                Ime::Preedit(value, cursor) => field.preedit(value, cursor),
                Ime::Commit(value) => {
                    field.commit(&value);
                }
                Ime::Disabled => field.cancel(),
                Ime::Enabled => {}
            }
            self.window.as_ref().unwrap().request_redraw();
            return;
        }
        if let Some(workspace) = &mut self.workspace
            && workspace.find.has_focus()
        {
            if workspace.find.focused {
                match event {
                    Ime::Preedit(text, cursor) => workspace.find.active_field().preedit(text, cursor),
                    Ime::Commit(text) => {
                        workspace.find.active_field().commit(&text);
                    }
                    Ime::Disabled => workspace.find.active_field().cancel(),
                    Ime::Enabled => {}
                }
            }
            self.window.as_ref().unwrap().request_redraw();
            return;
        }
        if let Some(editor) = self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active)) {
            match event {
                Ime::Preedit(text, cursor) => editor.preedit(text, cursor),
                Ime::Commit(text) => editor.commit(text),
                Ime::Disabled => editor.cancel_composition(),
                Ime::Enabled => {}
            }
            self.window.as_ref().unwrap().request_redraw();
            return;
        }
        if let Some(p) = &mut self.prototype {
            match event {
                Ime::Preedit(text, cursor) => p.preedit(text, cursor),
                Ime::Commit(text) => p.commit(&text),
                Ime::Disabled => p.cancel(),
                Ime::Enabled => {}
            }
            self.window.as_ref().unwrap().request_redraw();
        }
    }
    fn on_key_pressed(
        &mut self,
        el: &ActiveEventLoop,
        event: winit::event::KeyEvent,
        editor_bounds: bareline_renderer::Rect,
    ) {
        let editor_focused = !self.palette.open
            && self
                .workspace
                .as_ref()
                .is_some_and(|workspace| !workspace.find.has_focus() && !workspace.search_focus);
        if editor_focused
            && (event.logical_key == Key::Named(NamedKey::ContextMenu)
                || (event.logical_key == Key::Named(NamedKey::F10)
                    && self.modifiers.shift_key()
                    && !self.modifiers.control_key()
                    && !self.modifiers.alt_key()))
        {
            self.editor_context_menu(el);
            return;
        }
        if self.palette.open {
            self.on_key_palette(el, event);
            return;
        }
        if let Some(workspace) = &mut self.workspace
            && workspace.search_focus
            && workspace.search_panel.open
        {
            let extend = self.modifiers.shift_key();
            let key = match event.logical_key {
                Key::Named(NamedKey::Enter) => Some(bareline_ui::controls::Key::Enter),
                Key::Named(NamedKey::ArrowUp) => Some(bareline_ui::controls::Key::Up),
                Key::Named(NamedKey::ArrowDown) => Some(bareline_ui::controls::Key::Down),
                Key::Named(NamedKey::Tab) => Some(bareline_ui::controls::Key::Tab),
                Key::Named(NamedKey::Escape) => Some(bareline_ui::controls::Key::Escape),
                _ => None,
            };
            if let Some(key) = key {
                if let Some((source, range)) = workspace.search_panel.key(key)
                    && let Some(index) = workspace.activate_search(source, range)
                {
                    self.app.active = index;
                }
                if key == bareline_ui::controls::Key::Escape {
                    workspace.search_focus = false;
                }
            } else if workspace.search_panel.focused {
                let field = &mut workspace.search_panel.field;
                match &event.logical_key {
                    Key::Named(NamedKey::ArrowLeft) => field.horizontal(false, extend),
                    Key::Named(NamedKey::ArrowRight) => field.horizontal(true, extend),
                    Key::Named(NamedKey::Home) => field.edge(false, extend),
                    Key::Named(NamedKey::End) => field.edge(true, extend),
                    Key::Named(NamedKey::Delete) => {
                        field.delete(true);
                    }
                    Key::Named(NamedKey::Backspace) => {
                        field.delete(false);
                    }
                    Key::Character(value) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                        match value.to_ascii_lowercase().as_str() {
                            "a" => field.select_all(),
                            "v" => {
                                if let Ok(value) = self.platform.as_ref().unwrap().clipboard_text() {
                                    field.commit(&value);
                                }
                            }
                            "c" | "x"
                                if self
                                    .platform
                                    .as_ref()
                                    .unwrap()
                                    .set_clipboard_text(field.selected())
                                    .is_ok()
                                    && value.eq_ignore_ascii_case("x") =>
                            {
                                field.insert("");
                            }
                            _ => {}
                        }
                    }
                    _ if !self.modifiers.control_key() || self.modifiers.alt_key() => {
                        if let Some(value) = &event.text {
                            field.insert(value);
                        }
                    }
                    _ => {}
                }
            }
            self.window.as_ref().unwrap().request_redraw();
            return;
        }
        self.on_key_main(el, event, editor_bounds);
    }
    fn on_key_palette(&mut self, el: &ActiveEventLoop, event: winit::event::KeyEvent) {
        let context = self.command_context();
        let key = match event.logical_key {
            Key::Named(NamedKey::ArrowUp) => Some(bareline_ui::controls::Key::Up),
            Key::Named(NamedKey::ArrowDown) => Some(bareline_ui::controls::Key::Down),
            Key::Named(NamedKey::Enter) => Some(bareline_ui::controls::Key::Enter),
            Key::Named(NamedKey::Escape) => Some(bareline_ui::controls::Key::Escape),
            _ => None,
        };
        let mut action = None;
        if let Some(key) = key {
            action = self
                .palette
                .key(key, &self.app.commands, &context)
                .and_then(|id| self.app.commands.dispatch_in(id, &context).ok());
        } else {
            let extend = self.modifiers.shift_key();
            match &event.logical_key {
                Key::Character(value) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                    action = match value.to_ascii_lowercase().as_str() {
                        "a" => Some(Action::SelectAll),
                        "c" => Some(Action::Copy),
                        "x" => Some(Action::Cut),
                        "v" => Some(Action::Paste),
                        "z" => Some(Action::Undo),
                        "y" => Some(Action::Redo),
                        "p" if extend => Some(Action::Palette),
                        _ => None,
                    };
                }
                Key::Named(NamedKey::ArrowLeft) => self.palette.field.horizontal(false, extend),
                Key::Named(NamedKey::ArrowRight) => self.palette.field.horizontal(true, extend),
                Key::Named(NamedKey::Home) => self.palette.field.edge(false, extend),
                Key::Named(NamedKey::End) => self.palette.field.edge(true, extend),
                Key::Named(NamedKey::Backspace) => {
                    self.palette
                        .delete(false, &self.app.commands, &context, &self.settings.keymap.keymap);
                }
                Key::Named(NamedKey::Delete) => {
                    self.palette
                        .delete(true, &self.app.commands, &context, &self.settings.keymap.keymap);
                }
                _ if !self.modifiers.control_key() || self.modifiers.alt_key() => {
                    if let Some(value) = &event.text {
                        self.palette
                            .insert(value, &self.app.commands, &context, &self.settings.keymap.keymap);
                    }
                }
                _ => {}
            }
        }
        self.finish_palette_focus();
        self.app.palette = self.palette.open;
        if let Some(action) = action {
            self.dispatch(el, action);
        }
        self.window.as_ref().unwrap().request_redraw();
        return;
    }
    fn on_key_main(
        &mut self,
        el: &ActiveEventLoop,
        event: winit::event::KeyEvent,
        editor_bounds: bareline_renderer::Rect,
    ) {
        let key_name = match &event.logical_key {
            Key::Character(value) => Some(value.to_string()),
            Key::Named(key) => Some(match key {
                NamedKey::ArrowUp => "Up".into(),
                NamedKey::ArrowDown => "Down".into(),
                NamedKey::ArrowLeft => "Left".into(),
                NamedKey::ArrowRight => "Right".into(),
                _ => format!("{key:?}"),
            }),
            _ => None,
        };
        if let Some(key_name) = key_name
            && !self
                .workspace
                .as_ref()
                .is_some_and(|workspace| workspace.find.has_focus())
        {
            let mut chord = String::new();
            if self.modifiers.control_key() {
                chord.push_str("Ctrl+");
            }
            if self.modifiers.alt_key() {
                chord.push_str("Alt+");
            }
            if self.modifiers.shift_key() {
                chord.push_str("Shift+");
            }
            if self.modifiers.super_key() {
                chord.push_str("Meta+");
            }
            chord.push_str(&key_name);
            let composing = self.workspace.as_ref().is_some_and(|workspace| {
                (workspace.find.has_focus()
                    && (workspace.find.field.composing() || workspace.find.replacement.composing()))
                    || workspace
                        .editors
                        .get(self.app.active)
                        .is_some_and(|editor| editor.composition_text().is_some())
            });
            if let Ok(chord) = bareline_commands::KeyChord::parse(&chord)
                && let bareline_commands::KeyResolution::Command(id) = self.settings.resolve_default(
                    &self.app.commands,
                    &[chord],
                    bareline_commands::InputContext {
                        alt_gr: self.modifiers.control_key() && self.modifiers.alt_key(),
                        ime_composing: composing,
                        dead_key: matches!(event.logical_key, Key::Dead(_)),
                    },
                )
            {
                match self.app.commands.dispatch_in(id, &self.command_state_context(id)) {
                    Ok(action) => {
                        self.dispatch(el, action);
                        return;
                    }
                    // A key bound to a command that is off in this context used
                    // to do nothing at all. Show the reason so the shortcut is
                    // never silently ignored.
                    Err(bareline_commands::DispatchError::Disabled(reason)) => {
                        if let Some(workspace) = self.workspace.as_mut() {
                            workspace.message = Some(reason);
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                    Err(bareline_commands::DispatchError::Unknown(_)) => {}
                }
            }
        }
        if event.logical_key == Key::Named(NamedKey::Escape) && self.app.palette {
            self.app.palette = false;
            self.window.as_ref().unwrap().request_redraw();
            return;
        }
        if !self.app.palette {
            if let Some(workspace) = &mut self.workspace
                && workspace.find.has_focus()
            {
                let mut action = None;
                let extend = self.modifiers.shift_key();
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => action = Some(Action::FindClose),
                    Key::Named(NamedKey::Tab) => workspace.find.focus_next(extend),
                    // F3 / Shift+F3 cycle matches directly from the field.
                    Key::Named(NamedKey::F3) => {
                        action = Some(find_action(workspace.find.find_direction(extend)));
                    }
                    Key::Named(NamedKey::Enter) => {
                        let selected = find_action(workspace.find.enter_action());
                        action = Some(if extend && selected == Action::FindNext {
                            Action::FindPrevious
                        } else {
                            selected
                        });
                    }
                    Key::Named(NamedKey::Space) if !workspace.find.focused => {
                        action = workspace.find.focused_action().map(find_action)
                    }
                    _ if workspace.find.focused => {
                        let field = workspace.find.active_field();
                        match &event.logical_key {
                            Key::Named(NamedKey::ArrowLeft) => field.horizontal(false, extend),
                            Key::Named(NamedKey::ArrowRight) => field.horizontal(true, extend),
                            Key::Named(NamedKey::Home) => field.edge(false, extend),
                            Key::Named(NamedKey::End) => field.edge(true, extend),
                            Key::Named(NamedKey::Backspace) => {
                                field.delete(false);
                            }
                            Key::Named(NamedKey::Delete) => {
                                field.delete(true);
                            }
                            _ if !self.modifiers.control_key() || self.modifiers.alt_key() => {
                                if let Some(text) = &event.text {
                                    field.insert(text);
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                self.window.as_ref().unwrap().request_redraw();
                if let Some(action) = action {
                    self.dispatch(el, action);
                }
                return;
            }
            if let Some(editor) = self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active)) {
                let extend = self.modifiers.shift_key();
                if matches!(event.logical_key, Key::Named(NamedKey::PageDown | NamedKey::PageUp)) {
                    let forward = matches!(event.logical_key, Key::Named(NamedKey::PageDown));
                    if !editor.page_by(forward) {
                        editor.scroll(
                            if forward {
                                editor_bounds.height as f64 * 0.8
                            } else {
                                -editor_bounds.height as f64 * 0.8
                            },
                            editor_bounds.height,
                        );
                    }
                    self.window.as_ref().unwrap().request_redraw();
                    return;
                }
                let input = match &event.logical_key {
                    Key::Named(NamedKey::ArrowLeft) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                        Some(Input::WordLeft(extend))
                    }
                    Key::Named(NamedKey::ArrowLeft) => Some(Input::Left(extend)),
                    Key::Named(NamedKey::ArrowRight) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                        Some(Input::WordRight(extend))
                    }
                    Key::Named(NamedKey::ArrowRight) => Some(Input::Right(extend)),
                    Key::Named(NamedKey::ArrowUp) => Some(Input::Up(extend)),
                    Key::Named(NamedKey::ArrowDown) => Some(Input::Down(extend)),
                    Key::Named(NamedKey::Home) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                        Some(Input::DocumentHome(extend))
                    }
                    Key::Named(NamedKey::Home) => Some(Input::Home(extend)),
                    Key::Named(NamedKey::End) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                        Some(Input::DocumentEnd(extend))
                    }
                    Key::Named(NamedKey::End) => Some(Input::End(extend)),
                    Key::Named(NamedKey::Backspace) => Some(Input::Backspace),
                    Key::Named(NamedKey::Delete) => Some(Input::Delete),
                    Key::Named(NamedKey::Enter) => Some(Input::Insert("\n".into())),
                    Key::Named(NamedKey::Tab) => Some(Input::Insert("\t".into())),
                    Key::Named(NamedKey::Escape) => {
                        editor.cancel_composition();
                        None
                    }
                    _ if !self.modifiers.control_key() || self.modifiers.alt_key() => event
                        .text
                        .as_ref()
                        .filter(|s| !s.chars().any(char::is_control))
                        .map(|s| Input::Insert(s.to_string())),
                    _ => None,
                };
                if let Some(input) = input {
                    editor.enqueue(input);
                }
                self.window.as_ref().unwrap().request_redraw();
                return;
            }
            if event.logical_key == Key::Named(NamedKey::F6) && self.prototype.is_some() {
                if let Some(renderer) = &mut self.renderer {
                    renderer.invalidate_device();
                }
            } else if let Some(p) = &mut self.prototype {
                match event.logical_key {
                    Key::Named(NamedKey::ArrowLeft) => p.left(),
                    Key::Named(NamedKey::ArrowRight) => p.right(),
                    Key::Named(NamedKey::Home) => p.home(),
                    Key::Named(NamedKey::End) => p.end(),
                    Key::Named(NamedKey::Backspace) => p.backspace(),
                    Key::Named(NamedKey::Delete) => p.delete(),
                    Key::Named(NamedKey::Escape) => p.cancel(),
                    _ if !self.modifiers.control_key() || self.modifiers.alt_key() => {
                        if let Some(text) = event.text {
                            p.insert(&text);
                        }
                    }
                    _ => {}
                }
            }
            self.window.as_ref().unwrap().request_redraw();
        }
    }
    fn on_redraw(&mut self, el: &ActiveEventLoop, editor_bounds: bareline_renderer::Rect) {
        if self.renderer.is_none() {
            self.ledger.record(StartupAction::CreateRenderer);
            let _renderer_phase = bareline_diagnostics::startup_span(StartupAction::CreateRenderer);
            match self.platform.as_ref().unwrap().renderer(self.software) {
                Ok(r) => {
                    bareline_diagnostics::set_renderer_state(if r.software {
                        bareline_diagnostics::RendererState::Software
                    } else {
                        bareline_diagnostics::RendererState::Hardware
                    });
                    self.renderer = Some(r);
                }
                Err(e) => {
                    bareline_diagnostics::set_renderer_state(bareline_diagnostics::RendererState::Failed);
                    eprintln!(
                        "event=backend_init_failed code={} software={}",
                        e.code().0,
                        self.software
                    );
                    self.fail(el, e);
                    return;
                }
            }
        }
        let size = self.window.as_ref().unwrap().inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let scale = self.window.as_ref().unwrap().scale_factor() as f32;
        if let Some(platform) = &self.platform {
            let theme = self.settings.ui_theme();
            platform.menu_colors(theme.chrome.0, theme.text.0, theme.border.0);
            // Title bar and system menus follow the resolved theme.
            let chrome = theme.chrome.0;
            let luminance = ((chrome >> 16) & 0xff) * 2126 + ((chrome >> 8) & 0xff) * 7152 + (chrome & 0xff) * 722;
            platform.set_dark_mode(luminance < 128 * 10000);
        }
        // Bound the shaped-line cache to what the open editors can legitimately
        // display (open editors × visible rows × 2). Recomputed here so both the
        // first frame and every resize keep the budget in step with the window.
        let open_editors = self
            .workspace
            .as_ref()
            .map_or(0, |workspace| workspace.editors.len())
            .max(1);
        let visible_rows = ((size.height as f32 / scale / 10.0).ceil() as usize).max(1);
        // Fold the current status message into the toast stack (tagged with
        // the active document) and retire any expired info toasts (UX-60).
        let toast_now = Instant::now();
        let active_document = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.editors.get(self.app.active))
            .map(|editor| editor.snapshot().identity_token());
        let toast_message = self.workspace.as_ref().and_then(|workspace| workspace.message.clone());
        self.toasts.ingest(toast_message.as_deref(), active_document, toast_now);
        self.toasts.tick(toast_now);
        // Hand the renderer to the frame pipeline as a local so the draw
        // helpers can borrow it alongside disjoint `self` fields; it is
        // restored to `self` before the idle bootstrap runs (ARCH-07/ARCH-13).
        let mut renderer = self.renderer.take().unwrap();
        let outcome = self.render_frame(
            el,
            &mut renderer,
            editor_bounds,
            size,
            scale,
            open_editors,
            visible_rows,
        );
        self.renderer = Some(renderer);
        if outcome.is_err() {
            return;
        }
        if self.first_frame
            && !self.smoke
            && !self.perf
            && !self.performance.enabled()
            && !self.session.startup_pending()
            && self.startup_paths.is_empty()
            && self.workspace.as_ref().is_some_and(|w| !w.io_busy())
        {
            self.update.healthy_frame();
        }
        if self.profile_initialization.settled() {
            self.settings.load_keymap(&self.app.commands);
            self.session_first_frame(el);
            self.recovery_pump(el);
        }
        self.instance_pump(el);
        self.performance_pump(el);
        if self.first_frame
            && !self.session.startup_pending()
            && !self.smoke
            && self.prototype.is_none()
            && !self.performance.enabled()
            && self.workspace.is_none()
        {
            if self.startup_paths.is_empty() {
                self.dispatch(el, Action::New);
            } else if self.ensure_workspace(el) {
                self.startup_paths.clear();
                self.launch_pump();
                self.window.as_ref().unwrap().request_redraw();
            }
        }
    }
    fn render_frame(
        &mut self,
        el: &ActiveEventLoop,
        renderer: &mut WindowsRenderer,
        editor_bounds: bareline_renderer::Rect,
        size: winit::dpi::PhysicalSize<u32>,
        scale: f32,
        open_editors: usize,
        visible_rows: usize,
    ) -> Result<(), ()> {
        renderer.set_layout_budget(open_editors, visible_rows);
        let mut operations = bareline_ui::shell_with_theme(
            size.width as f32 / scale,
            size.height as f32 / scale,
            &self.app.tabs,
            self.app.active,
            false,
            self.settings.ui_theme(),
        );
        let footer_labels = self.draw_editor_layer(el, renderer, editor_bounds, &mut operations)?;
        self.macros
            .draw_output(size.width as f32 / scale, size.height as f32 / scale, &mut operations);
        self.status_pickers.clear();
        self.draw_footer(size, scale, &footer_labels, &mut operations);
        self.draw_panels(el, renderer, editor_bounds, size, scale, &mut operations)?;
        self.draw_overlays(el, renderer, size, scale, &mut operations)?;
        self.present_frame(el, renderer, size, scale, &operations);
        self.update_accessibility(size, scale);
        self.refresh_menus(el);
        Ok(())
    }
    fn draw_editor_layer(
        &mut self,
        el: &ActiveEventLoop,
        renderer: &mut WindowsRenderer,
        editor_bounds: bareline_renderer::Rect,
        operations: &mut Vec<bareline_renderer::DrawOp>,
    ) -> Result<Vec<String>, ()> {
        let window = self.window.as_ref().unwrap();
        let mut footer_labels = Vec::new();
        if let Some(workspace) = &mut self.workspace {
            workspace.theme = self.settings.ui_theme();
            let effective = self.settings.effective();
            self.power.configure_history(
                effective.clipboard_history_enabled,
                effective.clipboard_history_max_entries,
                effective.clipboard_history_max_total_bytes,
                effective.clipboard_history_max_entry_bytes,
            );
            let detected: Vec<_> = (0..workspace.editors.len())
                .map(|index| {
                    workspace
                        .path(index)
                        .map(bareline_syntax::Language::detect)
                        .unwrap_or(bareline_syntax::Language::PlainText)
                })
                .collect();
            for (index, editor) in workspace.editors.iter_mut().enumerate() {
                let language = editor
                    .viewport()
                    .language_override
                    .or(editor.viewport().detected_language)
                    .unwrap_or(detected[index]);
                let stable_id = bareline_syntax::catalog::CATALOG
                    .iter()
                    .find(|entry| entry.language == language)
                    .map_or("text", |entry| entry.id);
                editor.viewport_mut().syntax_preference = match effective.language_policy(stable_id).lexer {
                    bareline_settings::LexerPreference::Primary => bareline_syntax::LexerPreference::Lexilla,
                    bareline_settings::LexerPreference::Native => bareline_syntax::LexerPreference::Native,
                };
            }
            if self
                .applied_settings
                .as_ref()
                .is_none_or(|(previous, count)| *previous != effective || *count != workspace.editors.len())
            {
                // A theme change (settings or the OS switching light/dark, which
                // clears applied_settings) retires the old palette; drop the cached
                // brushes so they do not accumulate for the life of the session.
                renderer.clear_brushes();
                workspace.apply_resource_settings(&effective);
                workspace.transcode_quota_bytes = effective.transcode_quota_bytes;
                let editor_theme = self.settings.editor_theme();
                for editor in &mut workspace.editors {
                    editor.viewport_mut().theme = editor_theme;
                    editor.set_wrap(effective.word_wrap);
                    if let Err(error) = editor.set_font_family(&effective.editor_font_family) {
                        workspace.message = Some(error);
                    }
                    editor.apply_visual_preferences(
                        effective.editor_font_size_pt,
                        effective.tab_width,
                        effective.line_numbers,
                        effective.highlight_current_line,
                        &effective.whitespace,
                    );
                }
                if let Some(editor) = &mut self.views.secondary {
                    editor.set_wrap(effective.word_wrap);
                }
                self.applied_settings = Some((effective, workspace.editors.len()));
            }
            // Show a closable Settings tab in the strip while the page is open.
            self.views.settings_tab_open = self.settings.controller.open;
            let editor_start = operations.len();
            operations.push(bareline_renderer::DrawOp::PushClip(bareline_ui::rect(
                editor_bounds.x,
                editor_bounds.y,
                editor_bounds.width,
                if self.macros.height() > 0.0 {
                    (editor_bounds.height - bareline_ui::STATUS_HEIGHT).max(0.0)
                } else {
                    editor_bounds.height
                },
            )));
            // Pumping and tab selection happen here, before the layout
            // pass, so rendering itself stays side-effect free (ARCH-13).
            self.views.sync(workspace, &mut self.app);
            match self.views.draw(
                workspace,
                &mut self.app,
                renderer,
                editor_bounds.width,
                editor_bounds.height,
                operations,
                self.notify.clone(),
            ) {
                Ok(Some(mut caret)) => {
                    caret.x += editor_bounds.x;
                    caret.y += editor_bounds.y;
                    self.editor_caret = Some(caret);
                    window.set_ime_cursor_area(
                        LogicalPosition::new(caret.x as f64, caret.y as f64),
                        LogicalSize::new(caret.width as f64, caret.height as f64),
                    );
                }
                Ok(None) => {
                    self.editor_caret = None;
                }
                Err(error) => {
                    self.fail(el, format!("editor layout: {error:?}"));
                    return Err(());
                }
            }
            // Views already choose the active pane and produce bounded live labels.
            // Keep the global footer current even when Output reduces editor height.
            footer_labels = operations[editor_start + 1..]
                .iter()
                .filter_map(|op| {
                    if let bareline_renderer::DrawOp::Text { origin, text, .. } = op {
                        (origin.y == editor_bounds.height - 20.0).then(|| text.clone())
                    } else {
                        None
                    }
                })
                .collect();
            let scrollbar_start = operations.len();
            self.scrolling
                .draw(workspace, &self.views, self.app.active, editor_bounds, operations);
            // Store global hit bounds, but paint inside the local editor layer.
            translate_operations(&mut operations[scrollbar_start..], -editor_bounds.x, -editor_bounds.y);
            if let Err(error) = self.compare.draw(
                workspace,
                &mut self.views,
                &self.settings,
                renderer,
                editor_bounds.width,
                editor_bounds.height,
                operations,
            ) {
                self.fail(el, format!("compare layout: {error:?}"));
                return Err(());
            }
            self.recovery.draw(
                self.settings.ui_theme(),
                editor_bounds.width,
                editor_bounds.height,
                operations,
            );
            translate_operations(&mut operations[editor_start + 1..], editor_bounds.x, editor_bounds.y);
            operations.push(bareline_renderer::DrawOp::PopClip);
        }
        Ok(footer_labels)
    }
    fn draw_footer(
        &mut self,
        size: winit::dpi::PhysicalSize<u32>,
        scale: f32,
        footer_labels: &[String],
        operations: &mut Vec<bareline_renderer::DrawOp>,
    ) {
        if !footer_labels.is_empty() {
            let width = size.width as f32 / scale;
            let y = size.height as f32 / scale - 24.0;
            let theme = self.settings.ui_theme();
            operations.push(bareline_renderer::DrawOp::Fill(
                bareline_ui::rect(0.0, y, width, 24.0),
                theme.chrome,
            ));
            operations.push(bareline_renderer::DrawOp::Fill(
                bareline_ui::rect(0.0, y, width, 1.0),
                theme.border,
            ));
            // While the active document is still building its line index,
            // show an activity track along the top of the strip; the paged view's
            // retained sparse index reports a determinate fraction when it has
            // one, so the fill tracks real scan progress (UX-04).
            let (indexing, index_fraction) = self
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.editors.get(self.app.active))
                .map(|editor| {
                    let fraction = match editor {
                        bareline_app::workspace::WorkspaceEditor::Paged(paged) => paged.index_fraction(),
                        _ => None,
                    };
                    (!editor.snapshot().is_complete(), fraction)
                })
                .unwrap_or((false, None));
            if indexing {
                operations.push(bareline_renderer::DrawOp::Fill(
                    bareline_ui::rect(0.0, y, width, 2.0),
                    theme.border,
                ));
                let filled = index_fraction.map_or(width, |fraction| width * fraction.clamp(0.0, 1.0));
                operations.push(bareline_renderer::DrawOp::Fill(
                    bareline_ui::rect(0.0, y, filled, 2.0),
                    theme.focus,
                ));
            }
            for (x, label) in [
                16.0,
                130.0,
                (width - 420.0).max(310.0),
                width - 240.0,
                width - 155.0,
                width - 50.0,
            ]
            .into_iter()
            .zip(footer_labels)
            {
                bareline_ui::text(operations, x, y + 4.0, label, 13.0, theme.muted);
            }
            // Language, Indent, EOL and Encoding are clickable pickers
            // (UX-40); Position and INS/RO are read-only.
            for (px, pw, command) in [
                (16.0f32, 106.0f32, "language.choose"),
                (130.0, 90.0, "settings.open"),
                (width - 240.0, 80.0, "encoding.eol"),
                (width - 155.0, 100.0, "encoding.choose"),
            ] {
                self.status_pickers
                    .push((bareline_ui::rect(px - 6.0, y, pw, 24.0), command));
            }
            // Hovering the RO badge explains why editing is unavailable (UX-04).
            if self
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.editors.get(self.app.active))
                .is_some_and(|editor| editor.read_only())
                && bareline_ui::rect(width - 52.0, y, 52.0, 24.0).contains(self.pointer)
            {
                let tip = bareline_ui::rect((width - 240.0).max(0.0), y - 24.0, 230.0, 20.0);
                operations.push(bareline_renderer::DrawOp::Fill(tip, theme.elevated));
                operations.push(bareline_renderer::DrawOp::Stroke(tip, theme.border, 1.0));
                bareline_ui::text(
                    operations,
                    tip.x + 8.0,
                    tip.y + 3.0,
                    "Read-only document",
                    12.0,
                    theme.text,
                );
            }
            // Discoverability hint for the command palette, filling the gap
            // between the left segments and the caret position (UX-40).
            bareline_ui::text(
                operations,
                (width / 2.0 - 90.0).max(150.0),
                y + 4.0,
                "Ctrl+Shift+P for commands",
                13.0,
                theme.muted,
            );
        }
    }
    fn draw_panels(
        &mut self,
        el: &ActiveEventLoop,
        renderer: &mut WindowsRenderer,
        editor_bounds: bareline_renderer::Rect,
        size: winit::dpi::PhysicalSize<u32>,
        scale: f32,
        operations: &mut Vec<bareline_renderer::DrawOp>,
    ) -> Result<(), ()> {
        let persisted_dock_widths = self.settings.effective().dock_widths;
        self.panels.apply_persisted_widths(&persisted_dock_widths);
        if let Some(workspace) = &self.workspace
            && let Err(error) = self.panels.draw(
                renderer,
                size.width as f32 / scale,
                size.height as f32 / scale,
                workspace,
                self.app.active,
                operations,
            )
        {
            self.fail(el, format!("panel layout: {error:?}"));
            return Err(());
        }
        self.watch.hits.clear();
        if let Some(workspace) = &self.workspace {
            let primary = self.views.primary_index(workspace).unwrap_or(self.app.active);
            if let Some(mut bounds) = self.views.bounds[0] {
                bounds.x += editor_bounds.x;
                bounds.y += editor_bounds.y;
                let hits = self.watch.draw_banner(workspace, primary, bounds, operations);
                self.watch
                    .hits
                    .extend(hits.into_iter().map(|(rect, id)| (rect, 0, primary, id)));
            }
            if let (Some(editor), Some(mut bounds)) = (&self.views.secondary, self.views.bounds[1]) {
                if let Some(index) = workspace
                    .editors
                    .iter()
                    .position(|candidate| candidate.snapshot().same_document(editor.snapshot()))
                {
                    bounds.x += editor_bounds.x;
                    bounds.y += editor_bounds.y;
                    let hits = if matches!(editor,bareline_app::workspace::WorkspaceEditor::Paged(e) if e.follow_status().is_some())
                    {
                        watch::draw_banner(editor, bounds, operations)
                    } else {
                        self.watch.draw_banner(workspace, index, bounds, operations)
                    };
                    self.watch
                        .hits
                        .extend(hits.into_iter().map(|(rect, id)| (rect, 1, index, id)));
                }
            }
        }
        Ok(())
    }
    fn draw_overlays(
        &mut self,
        el: &ActiveEventLoop,
        renderer: &mut WindowsRenderer,
        size: winit::dpi::PhysicalSize<u32>,
        scale: f32,
        operations: &mut Vec<bareline_renderer::DrawOp>,
    ) -> Result<(), ()> {
        let window = self.window.as_ref().unwrap();
        self.search.draw(
            self.workspace.as_ref(),
            renderer,
            self.settings.ui_theme(),
            size.width as f32 / scale,
            size.height as f32 / scale,
            operations,
        );
        self.extensions.draw(
            renderer,
            size.width as f32 / scale,
            size.height as f32 / scale,
            self.settings.ui_theme(),
            operations,
        );
        if let Some(caret) = self.search.folder_ime_caret() {
            window.set_ime_cursor_area(
                LogicalPosition::new(caret.x as f64, caret.y as f64),
                LogicalSize::new(caret.width.max(1.0) as f64, caret.height.max(1.0) as f64),
            );
        }
        if let Some(caret) = self.extensions.ime_caret() {
            window.set_ime_cursor_area(
                LogicalPosition::new(caret.x as f64, caret.y as f64),
                LogicalSize::new(caret.width as f64, caret.height as f64),
            );
        }
        if let Err(error) = self.language.draw(
            renderer,
            size.width as f32 / scale,
            size.height as f32 / scale,
            self.settings.ui_theme(),
            operations,
        ) {
            self.fail(el, format!("language layout: {error:?}"));
            return Err(());
        }
        if let Err(error) = self.settings.draw(
            renderer,
            size.width as f32 / scale,
            size.height as f32 / scale,
            operations,
        ) {
            self.fail(el, format!("settings layout: {error:?}"));
            return Err(());
        }
        if let Some(p) = &mut self.prototype {
            match p.draw(renderer, size.width as f32 / scale, operations) {
                Ok(caret) => window.set_ime_cursor_area(
                    LogicalPosition::new(caret.x as f64, caret.y as f64),
                    LogicalSize::new(caret.width as f64, caret.height as f64),
                ),
                Err(error) => {
                    self.fail(el, format!("text layout: {error:?}"));
                    return Err(());
                }
            }
        }
        if let Err(error) = self.power.draw(
            renderer,
            size.width as f32 / scale,
            size.height as f32 / scale,
            self.settings.ui_theme(),
            operations,
        ) {
            self.fail(el, format!("power editor layout: {error:?}"));
            return Err(());
        }
        if let Err(error) = self.toolbar.draw(
            renderer,
            size.width as f32 / scale,
            size.height as f32 / scale,
            operations,
        ) {
            self.fail(el, format!("toolbar layout: {error:?}"));
            return Err(());
        }
        match self.shortcuts.draw(
            renderer,
            size.width as f32 / scale,
            size.height as f32 / scale,
            self.settings.ui_theme(),
            &self.app.commands,
            &self.settings.keymap.keymap,
            operations,
        ) {
            Ok(Some(caret)) => window.set_ime_cursor_area(
                LogicalPosition::new(caret.x as f64, caret.y as f64),
                LogicalSize::new(caret.width as f64, caret.height as f64),
            ),
            Ok(None) => {}
            Err(error) => {
                self.fail(el, format!("shortcut layout: {error:?}"));
                return Err(());
            }
        }
        match self.goto.draw(
            renderer,
            size.width as f32 / scale,
            size.height as f32 / scale,
            self.settings.ui_theme(),
            self.modal.map_or(modal::GOTO_FIELD_ID, |modal| modal.focused),
            operations,
        ) {
            Ok(Some(caret)) => window.set_ime_cursor_area(
                LogicalPosition::new(caret.x as f64, caret.y as f64),
                LogicalSize::new(caret.width as f64, caret.height as f64),
            ),
            Ok(None) => {}
            Err(error) => {
                self.fail(el, format!("go to line layout: {error:?}"));
                return Err(());
            }
        }
        match self.charsets.draw(
            renderer,
            size.width as f32 / scale,
            size.height as f32 / scale,
            self.settings.ui_theme(),
            operations,
        ) {
            Ok(Some(caret)) => window.set_ime_cursor_area(
                LogicalPosition::new(caret.x as f64, caret.y as f64),
                LogicalSize::new(caret.width as f64, caret.height as f64),
            ),
            Ok(None) => {}
            Err(error) => {
                self.fail(el, format!("character sets layout: {error:?}"));
                return Err(());
            }
        }
        match self.run_prompt.draw(
            renderer,
            size.width as f32 / scale,
            size.height as f32 / scale,
            self.settings.ui_theme(),
            self.modal.map_or(modal::RUN_FIELD_ID, |modal| modal.focused),
            operations,
        ) {
            Ok(Some(caret)) => window.set_ime_cursor_area(
                LogicalPosition::new(caret.x as f64, caret.y as f64),
                LogicalSize::new(caret.width as f64, caret.height as f64),
            ),
            Ok(None) => {}
            Err(error) => {
                self.fail(el, format!("run prompt layout: {error:?}"));
                return Err(());
            }
        }
        self.macros.draw(
            renderer,
            size.width as f32 / scale,
            size.height as f32 / scale,
            operations,
        );
        self.utilities.draw(
            &self.settings,
            size.width as f32 / scale,
            size.height as f32 / scale,
            operations,
        );
        self.toasts.draw(
            size.width as f32 / scale,
            size.height as f32 / scale,
            self.settings.ui_theme(),
            operations,
        );
        if self.palette.open {
            match self.palette.draw_with_theme(
                renderer,
                size.width as f32 / scale,
                size.height as f32 / scale,
                self.settings.ui_theme(),
                operations,
            ) {
                Ok(caret) => window.set_ime_cursor_area(
                    LogicalPosition::new(caret.x as f64, caret.y as f64),
                    LogicalSize::new(caret.width as f64, caret.height as f64),
                ),
                Err(error) => {
                    self.fail(el, format!("palette layout: {error:?}"));
                    return Err(());
                }
            }
        } else {
            self.palette.release(renderer);
        }
        Ok(())
    }
    fn present_frame(
        &mut self,
        el: &ActiveEventLoop,
        renderer: &mut WindowsRenderer,
        size: winit::dpi::PhysicalSize<u32>,
        scale: f32,
        operations: &[bareline_renderer::DrawOp],
    ) {
        let window = self.window.as_ref().unwrap();
        let result = renderer.resize(size.width, size.height, scale).and_then(|_| {
            let mut frame = renderer.begin_frame();
            frame.extend(operations);
            frame.finish()
        });
        let presented = matches!(&result, Ok(FrameStatus::Presented));
        match result {
            Ok(FrameStatus::Presented) => {
                bareline_diagnostics::set_renderer_state(if renderer.software {
                    bareline_diagnostics::RendererState::Software
                } else {
                    bareline_diagnostics::RendererState::Hardware
                });
                self.frames += 1;
                if !self.first_frame {
                    let micros = self.ledger.presented();
                    self.first_frame = true;
                    if let Err(error) = self.profile_initialization.schedule(self.notify.clone()) {
                        eprintln!("event=profile_initialization_failed reason={error}");
                    }
                    if !self.smoke && !self.perf && !self.performance.enabled() {
                        if let Err(error) = bareline_platform_windows::shell_integration::initialize_jump_list(
                            self.shell_integration.portable,
                        ) {
                            eprintln!("event=shell_initialization_unavailable reason={error}");
                        }
                    }
                    println!(
                        "{{\"event\":\"first_frame\",\"microseconds\":{micros},\"software\":{},\"version\":\"0.1.0\"}}",
                        renderer.software
                    );
                    eprintln!("event=startup_ledger entries={:?}", self.ledger.entries);
                    if let Some(dir) = &self.log_directory {
                        match LocalLog::open(dir) {
                            Ok(mut log) => {
                                let _ = log.event(Event::FirstFrame {
                                    micros,
                                    software: renderer.software,
                                });
                                if let Some((code, software)) = renderer.take_init_failure() {
                                    let _ =
                                        log.event(bareline_diagnostics::Event::BackendInitFailed { code, software });
                                }
                                let _ = log.ledger(&self.ledger);
                                self.log = Some(log);
                            }
                            Err(error) => eprintln!("event=diagnostics_unavailable kind={:?}", error.kind()),
                        }
                    }
                    if self.perf {
                        self.idle_at = Some(Instant::now() + Duration::from_secs(10));
                    }
                }
                if self.smoke {
                    el.exit();
                }
            }
            Ok(FrameStatus::Recreate) => {
                bareline_diagnostics::set_renderer_state(bareline_diagnostics::RendererState::Recreating);
                window.request_redraw();
            }
            Err(error) => {
                bareline_diagnostics::set_renderer_state(bareline_diagnostics::RendererState::Failed);
                self.fail(el, error);
            }
        }
        if presented {
            self.performance_frame();
        }
    }
    fn update_accessibility(&mut self, size: winit::dpi::PhysicalSize<u32>, scale: f32) {
        let accessible = self.accessibility_snapshot(
            (size.width as f32 / scale) as f64,
            (size.height as f32 / scale) as f64,
            scale as f64,
        );
        let text_sources = self.accessibility_text_sources();
        if let Some(provider) = &mut self.accessibility {
            provider.set_text_sources(text_sources);
            provider.update(accessible);
        }
    }
    fn refresh_menus(&mut self, el: &ActiveEventLoop) {
        // Rebuild the menu structure when contextual commands appear or
        // disappear (documents opened, tabs changed, Recent list grown);
        // this no-ops on the common frame where nothing changed.
        let menu_context = self.command_context();
        let mut refresh_error = None;
        if let Some(platform) = self.platform.as_mut()
            && let Err(error) = platform.refresh_structure(&self.app.commands, &menu_context)
        {
            refresh_error = Some(error);
        }
        if let Some(error) = refresh_error {
            self.fail(el, error);
        }
        if let Some(platform) = &self.platform
            && let Err(error) = platform.sync_commands_localized(
                &self.app.commands,
                &menu_context,
                &self.settings.keymap.keymap,
                |id, fallback| {
                    let key = if id.starts_with("menu.") {
                        id.to_owned()
                    } else {
                        format!("command.{id}")
                    };
                    self.settings.controller.label(&key, fallback)
                },
            )
        {
            self.fail(el, error);
        }
    }
}
fn find_action(action: bareline_app::find::FindAction) -> Action {
    match action {
        bareline_app::find::FindAction::Case => Action::FindMatchCase,
        bareline_app::find::FindAction::WholeWord => Action::FindWholeWord,
        bareline_app::find::FindAction::Previous => Action::FindPrevious,
        bareline_app::find::FindAction::Next => Action::FindNext,
        bareline_app::find::FindAction::Close => Action::FindClose,
        bareline_app::find::FindAction::Replace => Action::Replace,
        bareline_app::find::FindAction::ReplaceOne => Action::ReplaceOne,
        bareline_app::find::FindAction::ReplaceAll => Action::ReplaceAll,
        bareline_app::find::FindAction::Mode => Action::FindMode,
        bareline_app::find::FindAction::Cancel => Action::FindCancel,
    }
}

fn translate_operations(ops: &mut [bareline_renderer::DrawOp], dx: f32, dy: f32) {
    use bareline_renderer::DrawOp;
    for op in ops {
        match op {
            DrawOp::Fill(r, _)
            | DrawOp::Stroke(r, _, _)
            | DrawOp::FillRounded(r, _, _)
            | DrawOp::StrokeRounded(r, _, _, _)
            | DrawOp::PushClip(r)
            | DrawOp::PushLayer { bounds: r, .. }
            | DrawOp::Image { destination: r, .. } => {
                r.x += dx;
                r.y += dy;
            }
            DrawOp::Text { origin, .. } | DrawOp::Layout { origin, .. } => {
                origin.x += dx;
                origin.y += dy;
            }
            DrawOp::Line { from, to, .. } => {
                from.x += dx;
                to.x += dx;
                from.y += dy;
                to.y += dy;
            }
            DrawOp::PopClip | DrawOp::PopLayer => {}
        }
    }
}

/// Combine a UTF-16 surrogate pair — the two code units Windows delivers as
/// consecutive `WM_CHAR` messages for an astral character — into one scalar.
/// Returns `None` for an unpaired or malformed pair.
fn combine_surrogates(high: u16, low: u16) -> Option<char> {
    if !(0xD800..=0xDBFF).contains(&high) || !(0xDC00..=0xDFFF).contains(&low) {
        return None;
    }
    let scalar = 0x10000 + (((high as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
    char::from_u32(scalar)
}
/// Accumulates `WM_CHAR` code units into scalars. winit already coalesces its
/// `event.text`, so this guards the raw code-unit boundary: a high surrogate is
/// held until its low surrogate arrives, then the pair yields a single `char`.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Default)]
struct SurrogatePairing {
    high: Option<u16>,
}
impl SurrogatePairing {
    #[cfg_attr(not(test), allow(dead_code))]
    fn push(&mut self, unit: u16) -> Option<char> {
        if (0xD800..=0xDBFF).contains(&unit) {
            self.high = Some(unit);
            return None;
        }
        if let Some(high) = self.high.take() {
            return combine_surrogates(high, unit);
        }
        char::from_u32(unit as u32)
    }
}
/// Cut, Copy and Delete act on a selection; with none, the editor context menu
/// greys them out. Returns the reason string when the item must be disabled.
fn selection_sensitive_disabled(command: &str, has_selection: bool) -> Option<&'static str> {
    match command {
        "edit.cut" | "edit.copy" | "edit.delete" if !has_selection => Some("Select text first"),
        _ => None,
    }
}
impl Shell {
    fn sync_contributions(&mut self) {
        // The contributed command set only changes when the extensions runtime
        // does; a cheap generation stamp lets the common no-change case skip
        // cloning every extension command record (ARCH-16).
        let generation = self.extensions.contribution_generation();
        if self.extensions.synced_contribution_generation() == Some(generation) {
            return;
        }
        let records = self.extensions.contributions();
        if self.app.commands.contributions.entries().eq(records.iter()) {
            self.extensions.mark_contributions_synced(generation);
            return;
        }
        let mut owners = std::collections::BTreeMap::<String, Vec<bareline_commands::DynamicCommandRecord>>::new();
        for record in records {
            owners.entry(record.identity.owner.clone()).or_default().push(record);
        }
        let mut next = bareline_commands::DynamicContributions::default();
        for (owner, records) in owners {
            if let Err(error) = next.replace_owner(&owner, records) {
                self.app.commands.contributions = Default::default();
                if let Some(workspace) = &mut self.workspace {
                    workspace.message = Some(error.into());
                }
                return;
            }
        }
        self.app.commands.contributions = next;
        self.extensions.mark_contributions_synced(generation);
    }
}

#[cfg(test)]
mod deferred_close_tests {
    use super::{CloseTarget, PendingClose, accessibility::tests::headless_shell, lifecycle::Identity};
    use bareline_app::workspace::{Input, Workspace};
    use bareline_platform::LocalFileSystem;
    use std::{
        path::Path,
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };

    #[derive(Default)]
    struct TombstoneGate {
        armed: AtomicBool,
        released: AtomicBool,
        entered: Mutex<bool>,
        wake: Condvar,
    }
    impl TombstoneGate {
        fn arm(&self) {
            self.armed.store(true, Ordering::SeqCst);
        }
        fn wait_entered(&self) {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut entered = self.entered.lock().unwrap();
            while !*entered {
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(!remaining.is_zero(), "discard never reached tombstone publication");
                let (next, timeout) = self.wake.wait_timeout(entered, remaining).unwrap();
                entered = next;
                assert!(!timeout.timed_out() || *entered);
            }
        }
        fn release(&self) {
            self.released.store(true, Ordering::SeqCst);
            self.wake.notify_all();
        }
    }
    impl LocalFileSystem for TombstoneGate {
        fn cache_directory_guard(
            &self,
            path: &Path,
        ) -> std::io::Result<Option<bareline_platform::CacheDirectoryLease>> {
            LocalFileSystem::cache_directory_guard(&bareline_platform_windows::WindowsFileSystem, path)
        }
        fn remove_owned_cache_directory(
            &self,
            root: &Path,
            candidate: &Path,
            expected_root: bareline_platform::CacheDirectoryIdentity,
            expected_candidate: bareline_platform::CacheDirectoryIdentity,
            proof_name: &str,
            proof_bytes: &[u8],
            max_entries: usize,
            max_time: Duration,
            cancelled: &dyn Fn() -> bool,
        ) -> bareline_platform::CacheRemovalOutcome {
            LocalFileSystem::remove_owned_cache_directory(
                &bareline_platform_windows::WindowsFileSystem,
                root,
                candidate,
                expected_root,
                expected_candidate,
                proof_name,
                proof_bytes,
                max_entries,
                max_time,
                cancelled,
            )
        }
        fn open_sealed_read(&self, path: &Path) -> std::io::Result<std::fs::File> {
            LocalFileSystem::open_sealed_read(&bareline_platform_windows::WindowsFileSystem, path)
        }
        fn guard_directory(&self, path: &Path) -> std::io::Result<Arc<dyn Send + Sync>> {
            LocalFileSystem::guard_directory(&bareline_platform_windows::WindowsFileSystem, path)
        }
        fn identity(&self, file: &std::fs::File) -> std::io::Result<bareline_platform::FileIdentity> {
            LocalFileSystem::identity(&bareline_platform_windows::WindowsFileSystem, file)
        }
        fn validate_target(&self, path: &Path) -> std::io::Result<()> {
            LocalFileSystem::validate_target(&bareline_platform_windows::WindowsFileSystem, path)
        }
        fn commit(&self, staged: &Path, target: &Path, existed: bool) -> std::io::Result<()> {
            if self.armed.load(Ordering::SeqCst) && target.file_name().is_some_and(|name| name == "retired.json") {
                let mut entered = self.entered.lock().unwrap();
                *entered = true;
                self.wake.notify_all();
                while !self.released.load(Ordering::SeqCst) {
                    entered = self.wake.wait(entered).unwrap();
                }
            }
            LocalFileSystem::commit(&bareline_platform_windows::WindowsFileSystem, staged, target, existed)
        }
    }

    fn settle(workspace: &mut Workspace) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            workspace.pump();
            if !workspace.io_busy() && !workspace.editors.iter().any(|editor| editor.busy()) {
                break;
            }
            assert!(Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
    }

    #[test]
    fn deferred_close_rejects_changed_tab_document_or_index() {
        let target = CloseTarget {
            index: 2,
            identity: (41, 7),
            tab: Some(9),
            saving: false,
            discarding: false,
            was_read_only: false,
        };
        assert!(target.matches(2, (41, 7), Some(9)));
        assert!(!target.matches(1, (41, 7), Some(9)));
        assert!(!target.matches(2, (42, 7), Some(9)));
        assert!(!target.matches(2, (41, 8), Some(9)));
        assert!(!target.matches(2, (41, 7), Some(10)));
        assert!(!target.matches(2, (41, 7), None));
    }

    #[test]
    fn pending_exit_discard_revalidates_new_dirty_documents_before_exit() {
        let root = std::env::temp_dir().join(format!(
            "bareline-exit-discard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let gate = Arc::new(TombstoneGate::default());
        let mut workspace = Workspace::new(Arc::new(|| {}), gate.clone()).unwrap();
        let recovery_root = root.join("recovery");
        std::fs::create_dir_all(&recovery_root).unwrap();
        workspace.recovery_root = Some(recovery_root);
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("first draft".into()));
        settle(&mut workspace);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !workspace.editors[0].recovery_status().complete {
            workspace.pump();
            assert!(Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        let mut shell = headless_shell();
        shell.workspace = Some(workspace);
        gate.arm();
        assert!(!shell.discard_for_exit_indexes(&[0]));
        gate.wait_entered();
        let workspace = shell.workspace.as_mut().unwrap();
        workspace.new_document().unwrap();
        workspace.editors[1].enqueue(Input::Insert("new draft".into()));
        settle(workspace);
        let consents = match shell.pending_close.take() {
            Some(PendingClose::ApplicationDiscarding(consents)) => consents,
            _ => panic!("exit discard did not retain its consent owners"),
        };
        gate.release();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if !shell.discard_for_exit_identities(&consents) {
                if !matches!(shell.pending_close, Some(PendingClose::ApplicationDiscarding(_))) {
                    break;
                }
                shell.pending_close = None;
            }
            assert!(Instant::now() < deadline, "discard never reached a terminal outcome");
            std::thread::yield_now();
        }
        let workspace = shell.workspace.as_ref().unwrap();
        assert_eq!(workspace.editors.len(), 2);
        assert!(workspace.editors.iter().all(|editor| editor.dirty()));
        assert!(
            !workspace.editors[0].read_only(),
            "cancelled exit left the consent target frozen"
        );
        assert!(
            workspace
                .message
                .as_deref()
                .is_some_and(|message| message.contains("document set changed"))
        );
        drop(shell);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn discard_close_resolves_stable_owner_after_another_tab_moves() {
        let root = std::env::temp_dir().join(format!(
            "bareline-close-discard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let gate = Arc::new(TombstoneGate::default());
        let mut workspace = Workspace::new(Arc::new(|| {}), gate.clone()).unwrap();
        let recovery_root = root.join("recovery");
        std::fs::create_dir_all(&recovery_root).unwrap();
        workspace.recovery_root = Some(recovery_root);
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("discard me".into()));
        settle(&mut workspace);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !workspace.editors[0].recovery_status().complete {
            workspace.pump();
            assert!(Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        let identity = workspace.editors[0].document_identity();
        workspace.editors[0].set_read_only(true);
        let target = CloseTarget {
            index: 0,
            identity,
            tab: None,
            saving: false,
            discarding: true,
            was_read_only: false,
        };
        let mut shell = headless_shell();
        shell.workspace = Some(workspace);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        gate.arm();
        shell.close_document_with_renderer(target, &mut renderer);
        gate.wait_entered();
        assert!(matches!(shell.pending_close, Some(PendingClose::Document(_))));
        assert!(shell.workspace.as_mut().unwrap().reorder(&[1, 0]));
        gate.release();
        let deadline = Instant::now() + Duration::from_secs(10);
        while let Some(PendingClose::Document(target)) = shell.pending_close.take() {
            shell.close_document_with_renderer(target, &mut renderer);
            assert!(Instant::now() < deadline, "deferred close never completed");
            std::thread::yield_now();
        }
        let workspace = shell.workspace.as_mut().unwrap();
        assert_eq!(workspace.editors.len(), 1);
        assert_ne!(workspace.editors[0].document_identity().0, identity.0);
        assert_eq!(workspace.restore_last_closed(), Some(1));
        assert!(
            !workspace.editors[1].read_only(),
            "reopened discard target kept its temporary freeze"
        );
        drop(shell);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn clean_read_only_close_and_non_discard_mismatch_preserve_read_only_state() {
        let mut workspace =
            Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].set_read_only(true);
        let identity = workspace.editors[0].document_identity();
        let mut shell = headless_shell();
        shell.workspace = Some(workspace);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();

        shell.close_document_with_renderer(
            CloseTarget {
                index: 0,
                identity: (identity.0, identity.1.wrapping_add(1)),
                tab: None,
                saving: false,
                discarding: false,
                was_read_only: false,
            },
            &mut renderer,
        );
        assert!(shell.workspace.as_ref().unwrap().editors[0].read_only());

        let tab = shell.active_close_tab();
        shell.close_document_with_renderer(
            CloseTarget {
                index: 0,
                identity,
                tab,
                saving: false,
                discarding: false,
                was_read_only: true,
            },
            &mut renderer,
        );
        let workspace = shell.workspace.as_mut().unwrap();
        let restored = workspace.restore_last_closed().expect("closed read-only document");
        assert_eq!(restored, 1, "closing the last tab creates a fresh Untitled first");
        assert!(workspace.editors[restored].read_only());
    }

    #[test]
    fn forced_paged_close_and_preflight_share_full_document_identity_after_viewport_move() {
        let root = std::env::temp_dir().join(format!(
            "bareline-close-identity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("paged.txt");
        std::fs::write(&path, "line\n".repeat(2_000)).unwrap();
        let mut workspace =
            Workspace::new(Arc::new(|| {}), Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.resident_max_bytes = 4;
        workspace.open(path);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() || workspace.editors.iter().any(|editor| editor.busy()) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        workspace.editors[0].scroll(10_000.0, 400.0);
        while workspace.editors[0].busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        let editor = &workspace.editors[0];
        let document = editor.document_identity();
        let viewport = editor.snapshot().identity_token();
        assert_ne!(document, viewport);
        let close = CloseTarget {
            index: 0,
            identity: document,
            tab: Some(1),
            saving: true,
            discarding: false,
            was_read_only: false,
        };
        assert!(close.matches(0, document, Some(1)));
        assert!(!close.matches(0, viewport, Some(1)));
        assert!(Identity::capture(editor).matches(editor));
        drop(workspace);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod input_and_menu_tests {
    use super::{SurrogatePairing, combine_surrogates, selection_sensitive_disabled};

    #[test]
    fn surrogate_pair_from_two_wm_char_values_yields_one_scalar() {
        // U+1F389 🎉 arrives as the WM_CHAR pair D83C, DF89.
        let mut pairing = SurrogatePairing::default();
        assert_eq!(pairing.push(0xD83C), None);
        assert_eq!(pairing.push(0xDF89), Some('🎉'));
        // A lone BMP unit passes straight through.
        assert_eq!(SurrogatePairing::default().push(0x0041), Some('A'));
        // The pure combiner rejects a non-surrogate pair.
        assert_eq!(combine_surrogates(0xD83C, 0xDF89), Some('🎉'));
        assert_eq!(combine_surrogates(0x0041, 0x0042), None);
    }

    #[test]
    fn context_menu_item_enabled_state_follows_selection() {
        // Cut/Copy/Delete are disabled without a selection, enabled with one.
        for id in ["edit.cut", "edit.copy", "edit.delete"] {
            assert!(selection_sensitive_disabled(id, false).is_some());
            assert!(selection_sensitive_disabled(id, true).is_none());
        }
        // Selection-independent items are never gated by this rule.
        for id in ["edit.paste", "edit.undo", "edit.select_all", "search.find"] {
            assert!(selection_sensitive_disabled(id, false).is_none());
        }
    }
}
