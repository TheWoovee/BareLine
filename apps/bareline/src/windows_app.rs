// SPDX-License-Identifier: MPL-2.0
mod accessibility;
mod compare;
mod encoding;
mod extensions;
mod instance;
mod inventory;
mod language;
mod launch;
mod lifecycle;
mod macros;
mod migration;
mod performance;
mod power;
mod recovery;
mod scrolling;
mod search;
mod session;
mod settings;
mod shell_integration;
mod shortcuts;
mod toolbar;
mod update;
mod utilities;
mod views;
mod watch;
mod workspace_panels;
use bareline_app::App;
use bareline_app::text_prototype::TextPrototype;
use bareline_app::workspace::{Input, Workspace};
use bareline_commands::Action;
use bareline_diagnostics::{Event, LocalLog, StartupAction, StartupLedger};
use bareline_platform::PlatformServices;
use bareline_platform_windows::{WindowsPlatform, WindowsRenderer};
use bareline_renderer::{FrameStatus, Point, RenderBackend};
use bareline_settings::RendererMode;
use std::{
    path::PathBuf,
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

struct Shell {
    // Drop renderer/platform before destroying the window.
    renderer: Option<WindowsRenderer>,
    platform: Option<WindowsPlatform>,
    accessibility: Option<bareline_platform_windows::WindowsAccessibility>,
    shell_integration: shell_integration::ShellIntegrationRuntime,
    window: Option<Window>,
    app: App,
    palette: bareline_app::palette::PaletteController,
    ui_focus: bareline_ui::focus::FocusChain,
    ui_router: bareline_ui::focus::EventRouter,
    ledger: StartupLedger,
    modifiers: ModifiersState,
    software: bool,
    first_frame: bool,
    smoke: bool,
    failed: bool,
    prototype: Option<TextPrototype>,
    workspace: Option<Workspace>,
    notify: std::sync::Arc<dyn Fn() + Send + Sync>,
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
}
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut ledger = StartupLedger::default();
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let (args, inventory_request) = inventory::parse(args)?;
    let mut launch = {
        let _phase = bareline_diagnostics::startup_span(StartupAction::ParseCli);
        launch::parse(&args, &mut ledger)?
    };
    if inventory_request.is_some() && !launch.paths.is_empty() {
        return Err("Command inventory export does not open documents".into());
    }
    if launch.help {
        println!(
            "Bareline [--line N] [--column N] [--read-only] [--monitor] [--no-session] [--no-extensions] [--new-instance] [--] [files...]"
        );
        return Ok(());
    }
    if launch.version {
        println!("Bareline {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let settings_document = match launch.settings_path.as_ref() {
        Some(path) => match ledger.read_config(path, StartupAction::ReadSettings, 64 * 1024)? {
            Some(bytes) => {
                bareline_settings::SettingsDocument::parse(&bytes, bareline_settings::Scope::User)?
            }
            None => bareline_settings::SettingsDocument::empty(bareline_settings::Scope::User),
        },
        None => bareline_settings::SettingsDocument::empty(bareline_settings::Scope::User),
    };
    let settings = bareline_settings::resolve(&settings_document, None, false, None).values;
    let software =
        launch.software || (settings.renderer == RendererMode::Software && !launch.hardware);
    let smoke = launch.smoke;
    let prototype = launch.prototype;
    let perf = launch.perf;
    let startup_paths = launch.paths.clone();
    let mut builder = EventLoop::<usize>::with_user_event();
    let (tx, rx) = std::sync::mpsc::channel();
    let (tray_tx, tray_rx) = std::sync::mpsc::channel();
    builder.with_msg_hook(move |message| {
        // SAFETY: winit supplies a valid MSG pointer during the hook invocation.
        if let Some(id) = unsafe { WindowsPlatform::command_message(message) } {
            let _ = tx.send(id);
        }
        if let Some(action) =
            unsafe { bareline_platform_windows::shell_integration::tray_message(message) }
        {
            let _ = tray_tx.send(action);
        }
        false
    });
    let event_loop = builder.build()?;
    let proxy = event_loop.create_proxy();
    event_loop.set_control_flow(ControlFlow::Wait);
    let notify: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
        let _ = proxy.send_event(0);
    });
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
        ui_focus: Default::default(),
        ui_router: Default::default(),
        ledger,
        modifiers: ModifiersState::empty(),
        software,
        first_frame: false,
        smoke,
        failed: false,
        prototype: prototype.then(TextPrototype::mixed_script),
        workspace: None,
        notify,
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
    };
    shell.shell_integration.portable = launch.portable;
    shell.performance.configure(launch.performance.clone());
    shell.recovery.configure(launch.recovery_path.clone());
    shell.macros.configure(
        launch
            .settings_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|root| root.join("macros")),
    );
    for (id, title) in [
        ("recovery.open", "Recovery Center"),
        ("recovery.open_folder", "Open Recovery Folder"),
        ("recovery.restore_latest", "Restore Latest Recovery"),
        ("recovery.retry", "Retry Recovery"),
        ("recovery.save_as", "Save Recovered Document As"),
    ] {
        shell
            .app
            .commands
            .register(bareline_commands::CommandSpec {
                id: bareline_commands::CommandId(id),
                title,
                category: "File",
                shortcut: "",
                action: Action::Contributed(bareline_commands::CommandId(id)),
            })
            .expect("unique recovery command");
    }
    shell.settings = settings::SettingsRuntime::new(
        settings_document,
        launch.settings_path.clone(),
        shell.notify.clone(),
    );
    shell
        .session
        .configure(launch.session_path.clone(), !launch.no_session);
    shell
        .extensions
        .configure(launch.extensions_path.clone(), !launch.no_extensions);
    for (id, title) in [
        (
            "file.transcode.resume",
            "Increase Temporary Disk Limit and Resume Conversion",
        ),
        ("file.transcode.cancel", "Cancel Paused Conversion"),
    ] {
        shell
            .app
            .commands
            .register(bareline_commands::CommandSpec {
                id: bareline_commands::CommandId(id),
                title,
                category: "File",
                shortcut: "",
                action: Action::Contributed(bareline_commands::CommandId(id)),
            })
            .expect("unique conversion command");
    }
    bareline_settings::register_commands(&mut shell.app.commands)
        .expect("unique settings commands");
    for command in update::commands()
        .into_iter()
        .chain(migration::commands())
        .chain(shell_integration::commands())
    {
        shell
            .app
            .commands
            .register(command)
            .expect("unique update command");
    }
    bareline_app::language::register_commands(&mut shell.app.commands);
    extensions::register(&mut shell.app.commands);
    toolbar::register(&mut shell.app.commands);
    shortcuts::register(&mut shell.app.commands);
    lifecycle::register(&mut shell.app.commands);
    power::register(&mut shell.app.commands);
    utilities::register(&mut shell.app.commands);
    shell.inventory.configure(inventory_request);
    search::register(&mut shell.app.commands);
    bareline_app::encoding::register(&mut shell.app.commands);
    compare::register(&mut shell.app.commands);
    views::register(&mut shell.app.commands);
    bareline_app::macros::register_commands(&mut shell.app.commands);
    bareline_app::workspace_panel::register_commands(&mut shell.app.commands);
    for command in watch::commands() {
        shell
            .app
            .commands
            .register(command)
            .expect("unique watch command");
    }
    if prototype {
        shell.app.apply(Action::New);
    }
    // Native commands are drained by about_to_wait, without a worker or timer.
    struct Handler {
        shell: Shell,
        commands: std::sync::mpsc::Receiver<usize>,
        tray_actions:
            std::sync::mpsc::Receiver<bareline_platform_windows::shell_integration::TrayAction>,
    }
    impl ApplicationHandler<usize> for Handler {
        fn user_event(&mut self, el: &ActiveEventLoop, _: usize) {
            self.shell.accessibility_actions(el);
            if self.shell.workspace.as_mut().is_some_and(|w| w.pump()) {
                if let Some(workspace) = &self.shell.workspace {
                    if workspace.editors.len() > self.shell.app.tabs.len() {
                        self.shell.app.active = workspace.editors.len() - 1;
                    }
                    self.shell.app.tabs = workspace.titles();
                    self.shell.app.active = self
                        .shell
                        .app
                        .active
                        .min(self.shell.app.tabs.len().saturating_sub(1));
                }
                if let Some(window) = &self.shell.window {
                    window.request_redraw();
                }
            }
            if self.shell.search_pump() {
                if let Some(window) = &self.shell.window {
                    window.request_redraw();
                }
            }
            self.shell.shortcuts_pump(el);
            self.shell.record_acknowledged_inputs();
            self.shell.instance_pump(el);
            self.shell.launch_pump();
            self.shell.session_pump(el);
            self.shell.recovery_pump(el);
            self.shell.lifecycle_pump(el);
            self.shell.encoding_pump(el);
            self.shell.migration_pump(el);
            self.shell.shell_recent_pump();
            self.shell.performance_pump(el);
            self.shell.power_pump();
            self.shell.utilities_pump(el);
            self.shell.macros_pump(el);
            self.shell.panels_pump(el);
            self.shell.watch_pump(el);
            self.shell.language_pump(el);
            self.shell.extensions_pump(el);
            self.shell.sync_contributions();
            self.shell.inventory_pump(el);
            self.shell.compare_pump(el);
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
            if self.shell.settings.poll() {
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
            let deadline = caret_deadline
                .into_iter()
                .chain(self.shell.idle_at)
                .chain(self.shell.inventory.deadline())
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
impl Shell {
    fn editor_has_input_focus(&self) -> bool {
        self.window.as_ref().is_some_and(Window::has_focus)
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
                        Key::Character(value) if value == " " => {
                            Some(bareline_ui::controls::Key::Space)
                        }
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
            WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::Ime(_) => {}
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
        deadline = deadline
            .into_iter()
            .chain(self.views.secondary_blink_deadline())
            .min();
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
        let commands = [
            "edit.undo",
            "edit.redo",
            "edit.cut",
            "edit.copy",
            "edit.paste",
            "edit.select_all",
            "search.find",
        ]
        .map(bareline_commands::CommandId);
        let result = self.platform.as_ref().unwrap().context_menu_in(
            origin.x + (caret.x as f64 * scale) as i32,
            origin.y + ((caret.y + caret.height) as f64 * scale) as i32,
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
    fn command_context(&self) -> bareline_commands::CommandContext {
        use bareline_commands::CommandState;
        let mut context = bareline_commands::CommandContext::default();
        let field_active = self.settings.controller.open
            || self.shortcuts.open
            || self.palette.open
            || self.workspace.as_ref().is_some_and(|workspace| {
                workspace.find.has_focus()
                    || (workspace.search_focus && workspace.search_panel.focused)
            });
        let editor = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.editors.get(self.app.active));
        for command in self.app.commands.entries() {
            if field_active
                && matches!(
                    command.action,
                    Action::Copy
                        | Action::Cut
                        | Action::Paste
                        | Action::SelectAll
                        | Action::Undo
                        | Action::Redo
                )
            {
                continue;
            }
            let reason = match command.action {
                Action::Undo
                    if !self.views.history_available(
                        self.workspace.as_ref(),
                        self.app.active,
                        true,
                    ) =>
                {
                    Some("Nothing to undo")
                }
                Action::Redo
                    if !self.views.history_available(
                        self.workspace.as_ref(),
                        self.app.active,
                        false,
                    ) =>
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
                context
                    .states
                    .insert(command.id, CommandState::disabled(reason));
            } else {
                let checked = match command.action {
                    Action::FindMatchCase => self
                        .workspace
                        .as_ref()
                        .is_some_and(|w| w.find.case_sensitive),
                    Action::FindWholeWord => {
                        self.workspace.as_ref().is_some_and(|w| w.find.whole_word)
                    }
                    _ => false,
                };
                if checked {
                    context.states.insert(
                        command.id,
                        CommandState {
                            checked,
                            ..Default::default()
                        },
                    );
                }
            }
        }
        if editor.is_some_and(|editor| editor.paged()) {
            for id in [
                "search.find",
                "search.replace",
                "search.find_next",
                "search.find_previous",
                "search.replace_one",
                "search.replace_all",
            ] {
                context.states.insert(
                    bareline_commands::CommandId(id),
                    CommandState::disabled(
                        "Full-document search is not yet connected to paged storage",
                    ),
                );
            }
            for id in [
                "view.split_vertical",
                "view.split_horizontal",
                "view.clone_other",
                "view.move_other",
            ] {
                context.states.insert(
                    bareline_commands::CommandId(id),
                    CommandState::disabled("Split views are unavailable for paged documents"),
                );
            }
        }
        let pause = self
            .workspace
            .as_ref()
            .and_then(|w| w.paused_transcode_info());
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
        context.states.insert(
            bareline_commands::CommandId("file.transcode.resume"),
            resume,
        );
        context.states.insert(
            bareline_commands::CommandId("file.transcode.cancel"),
            if pause.is_some() {
                CommandState::default()
            } else {
                CommandState::disabled("No conversion is paused")
            },
        );
        self.views.annotate_context(&mut context);
        if self
            .workspace
            .as_ref()
            .is_some_and(|w| w.editors.iter().any(|e| e.paged()))
        {
            context.states.insert(
                bareline_commands::CommandId("search.open_documents"),
                CommandState::disabled(
                    "Open-document search is not yet connected to paged storage",
                ),
            );
        }
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
        self.compare
            .annotate_context(&mut context, self.workspace.as_ref());
        self.panels.annotate_context(&mut context);
        self.toolbar.annotate_context(&mut context);
        self.lifecycle
            .annotate_context(&mut context, self.workspace.as_ref(), self.app.active);
        self.migration.annotate_context(&mut context);
        self.shell_integration.annotate_context(
            &mut context,
            self.workspace
                .as_ref()
                .and_then(|w| w.path(self.app.active))
                .is_some(),
            self.window
                .as_ref()
                .and_then(|w| w.is_visible())
                .unwrap_or(true),
        );
        self.macros.annotate_context(&mut context);
        self.encoding_context(&mut context);
        context
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
                    workspace.resident_max_bytes = settings.resident_max_bytes;
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
    fn request_close(&mut self, el: &ActiveEventLoop) {
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
            if workspace.editors.iter().any(|e| e.dirty() || e.busy())
                && !self.platform.as_ref().unwrap().confirm_discard()
            {
                return;
            }
        }
        if !self.session_before_exit(el) {
            el.exit();
        }
    }
    fn dispatch(&mut self, el: &ActiveEventLoop, action: Action) {
        if self.session.closing() {
            return;
        }
        self.sync_contributions();
        self.record_acknowledged_inputs();
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
            return;
        }
        if self.views_action(el, action) {
            return;
        }
        if self.palette.open
            && matches!(
                action,
                Action::SelectAll
                    | Action::Copy
                    | Action::Cut
                    | Action::Paste
                    | Action::Undo
                    | Action::Redo
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
            return;
        }
        if let Some(workspace) = &mut self.workspace
            && workspace.find.has_focus()
            && matches!(
                action,
                Action::SelectAll
                    | Action::Copy
                    | Action::Cut
                    | Action::Paste
                    | Action::Undo
                    | Action::Redo
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
                            workspace.message =
                                Some("Find accepts a single line up to 16 KiB.".into());
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
                        workspace.message =
                            Some("Clipboard write failed; selection was preserved.".into());
                    }
                }
                _ => {}
            }
            self.window.as_ref().unwrap().request_redraw();
            return;
        }
        match action {
            Action::Contributed(id) => {
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
                if self.migration_dispatch(el, id.0) || self.shell_integration_command(id.0) {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                if id.0 == "internal.dynamic.invoke" {
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
                        } else if let Some((_, used, required, limit)) =
                            workspace.paused_transcode_info()
                        {
                            workspace.resume_transcode(
                                limit.saturating_mul(2).max(used.saturating_add(required)),
                            );
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
                if self.lifecycle_dispatch(el, id.0)
                    || self.recovery_dispatch(el, id.0)
                    || self.utilities_dispatch(el, id.0)
                    || self.power_dispatch(el, id.0)
                    || self.shortcuts_dispatch(el, id.0)
                    || self.toolbar_dispatch(el, id.0)
                    || self.compare_dispatch(el, id.0)
                    || self.language_dispatch(el, id.0)
                    || self.extensions_dispatch(el, id.0)
                    || self.settings_dispatch(el, id.0)
                    || self.macros_dispatch(el, id.0)
                    || self.views_dispatch(el, id.0)
                    || self.panels_dispatch(el, id.0)
                    || self.watch_dispatch(el, id.0)
                {
                    return;
                }
                if let Some(workspace) = &mut self.workspace {
                    match id.0 {
                        "search.open_documents" => {
                            workspace.find.blur();
                            workspace.search_panel.show();
                            workspace.search_focus = true;
                        }
                        "search.cancel_panel" => workspace.search_panel.cancel(),
                        "search.close_panel" => {
                            workspace.search_panel.hide();
                            workspace.search_focus = false;
                        }
                        _ => {
                            if let Some(editor) = workspace.editors.get_mut(self.app.active)
                                && let Err(error) = editor.execute_power(id.0)
                            {
                                workspace.message = Some(error);
                            }
                        }
                    }
                }
            }
            Action::Palette => {
                if !self.app.palette {
                    let invoker = if self.shortcuts.open {
                        if self.shortcuts.binding_focus {
                            19001
                        } else {
                            19000
                        }
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
                            if let Some(editor) = workspace.editors.get_mut(self.app.active) {
                                editor.cancel_composition();
                                workspace.find.show();
                            }
                        }
                        Action::FindClose => workspace.find.hide(),
                        Action::Replace => {
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
                        Action::FindWholeWord => {
                            workspace.find.whole_word = !workspace.find.whole_word
                        }
                        Action::FindMatchCase => {
                            workspace.find.case_sensitive = !workspace.find.case_sensitive
                        }
                        _ => workspace.find_next(self.app.active, action == Action::FindPrevious),
                    }
                }
            }
            Action::Close => {
                if let (Some(workspace), Some(renderer), Some(platform)) =
                    (&mut self.workspace, &mut self.renderer, &self.platform)
                {
                    let index = self.app.active;
                    if workspace.document_busy(index) {
                        platform.pending_operation_notice();
                        return;
                    }
                    let dirty = workspace
                        .editors
                        .get(index)
                        .is_some_and(|editor| editor.dirty());
                    if dirty
                        && !platform.confirm_discard_document(
                            self.app
                                .tabs
                                .get(index)
                                .map_or("this document", String::as_str),
                        )
                    {
                        return;
                    }
                    if workspace.close(index, dirty, renderer).is_ok() {
                        self.app.tabs = workspace.titles();
                        self.app.active = index.min(self.app.tabs.len().saturating_sub(1));
                    }
                }
            }
            Action::CancelFileOperations => {
                if let Some(workspace) = &mut self.workspace {
                    workspace.cancel_file_operations();
                }
            }
            Action::Quit => self.request_close(el),
            Action::About => {
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
                    if self.log.is_some() {
                        "Enabled"
                    } else {
                        "Unavailable"
                    }
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
                            bareline_platform_windows::AboutAction::ThirdPartyNotices => {
                                "THIRD-PARTY-NOTICES.md"
                            }
                            bareline_platform_windows::AboutAction::CopyDiagnostics => {
                                unreachable!()
                            }
                        };
                        match std::env::current_exe().and_then(|path| {
                            path.parent()
                                .map(|parent| parent.join(name))
                                .ok_or_else(|| {
                                    std::io::Error::other("Executable directory unavailable")
                                })
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
                if let Some(editor) = self
                    .workspace
                    .as_mut()
                    .and_then(|w| w.editors.get_mut(self.app.active))
                {
                    editor.enqueue(match action {
                        Action::Undo => Input::Undo,
                        Action::Redo => Input::Redo,
                        _ => Input::SelectAll,
                    });
                }
            }
            Action::Copy | Action::Cut | Action::Paste => {
                if let Some(editor) = self
                    .workspace
                    .as_mut()
                    .and_then(|w| w.editors.get_mut(self.app.active))
                {
                    let platform = self.platform.as_ref().unwrap();
                    if action == Action::Paste {
                        match platform.clipboard_text() {
                            Ok(text) => editor.commit_with_origin(
                                text,
                                bareline_document::history::EditOrigin::Paste,
                            ),
                            Err(_) => {
                                editor.error = Some(
                                    "Clipboard text is unavailable or exceeds the 4 MiB limit."
                                        .into(),
                                )
                            }
                        }
                    } else {
                        match editor.selected_text() {
                            Ok(text) if !text.is_empty() => match platform.set_clipboard_text(&text) {
                                Ok(()) if action == Action::Cut => { self.power.copied(&text); editor.enqueue(Input::Insert(String::new())); },
                                Ok(()) => { self.power.copied(&text); }, Err(_) => editor.error = Some("Could not write text to the clipboard. Selection was preserved.".into()),
                            },
                            Ok(_) => {}, Err(message) => editor.error = Some(message.into()),
                        }
                    }
                }
            }
            Action::Open if self.prototype.is_none() => {
                let result = self.platform.as_ref().unwrap().open_file();
                match result {
                    Ok(Some(path)) => {
                        if self.ensure_workspace(el) {
                            self.workspace.as_mut().unwrap().open(path);
                        }
                    }
                    Ok(None) => {}
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
                let existing = if action == Action::Save {
                    workspace.path(self.app.active).map(|p| p.to_owned())
                } else {
                    None
                };
                let path = match existing {
                    Some(path) => Some(path),
                    None => match self.platform.as_ref().unwrap().save_file() {
                        Ok(path) => path,
                        Err(error) => {
                            self.workspace.as_mut().unwrap().message = Some(error);
                            None
                        }
                    },
                };
                if let Some(path) = path {
                    self.workspace.as_mut().unwrap().save(self.app.active, path);
                }
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
        match unsafe { WindowsPlatform::new(handle.hwnd.get(), &self.app.commands) } {
            Ok(platform) => self.platform = Some(platform),
            Err(e) => {
                self.fail(el, e);
                return;
            }
        }
        let initial =
            bareline_app::accessibility::snapshot("Bareline", 1200.0, 760.0, None, Vec::new(), 1);
        match unsafe {
            bareline_platform_windows::WindowsAccessibility::new(
                handle.hwnd.get(),
                initial,
                self.notify.clone(),
            )
        } {
            Ok(mut provider) => {
                provider.set_text_source(self.accessibility_text_source());
                self.accessibility = Some(provider);
            }
            Err(error) => {
                self.fail(el, error);
                return;
            }
        }
        window.set_ime_allowed(true);
        window.set_visible(!self.smoke);
        window.request_redraw();
        self.window = Some(window);
        if self.smoke {
            // Hidden windows do not receive WM_PAINT. Exercise the real renderer explicitly.
            self.window_event(
                el,
                self.window.as_ref().unwrap().id(),
                WindowEvent::RedrawRequested,
            );
        }
    }
    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
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
        if matches!(
            &event,
            WindowEvent::ThemeChanged(_) | WindowEvent::Focused(true)
        ) {
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
        let editor_bounds = self.editor_bounds();
        let editor_pointer = self.editor_pointer();
        if self.recovery_event(el, &event)
            || self.macros_event(el, &event)
            || self.settings_keymap_event(el, &event)
            || self.utilities_event(el, &event)
            || self.encoding_event(el, &event)
            || (self.power.open && self.power_event(el, &event))
            || self.shortcuts_event(el, &event)
            || self.toolbar_event(el, &event)
            || self.settings_event(el, &event)
            || self.extensions_event(el, &event)
            || self.language_event(el, &event)
            || self.compare_event(el, &event)
            || self.search_overlay_event(&event)
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
                let logical =
                    position.to_logical::<f32>(self.window.as_ref().unwrap().scale_factor());
                self.pointer = Point {
                    x: logical.x,
                    y: logical.y,
                };
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
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
                            bounds: bareline_ui::rect(
                                0.0,
                                0.0,
                                size.width as f32,
                                size.height as f32,
                            ),
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
                    if let Some(action) =
                        command.and_then(|id| self.app.commands.dispatch_in(id, &context).ok())
                    {
                        self.dispatch(el, action);
                    }
                    self.window.as_ref().unwrap().request_redraw();
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
                        if let Some((source, range)) =
                            workspace.search_panel.pointer(editor_pointer)
                            && let Some(index) = workspace.activate_search(source, range)
                        {
                            self.app.active = index;
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
                    if workspace.find.open
                        && (34.0..34.0 + workspace.find.height()).contains(&editor_pointer.y)
                    {
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
                        self.workspace
                            .as_mut()
                            .and_then(|w| w.editors.get_mut(self.app.active)),
                        &self.renderer,
                    )
                {
                    if matches!(editor, bareline_app::workspace::WorkspaceEditor::Paged(paged) if !paged.paged_frame_state().ready)
                    {
                        return;
                    }
                    if let Err(error) =
                        editor.click(renderer, editor_pointer, self.modifiers.shift_key())
                    {
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
                let panel_commands = self.panels_context_commands();
                if self.pointer.y < 34.0
                    && let Some(window) = &self.window
                {
                    self.app.click_tab(
                        window.inner_size().width as f32 / window.scale_factor() as f32,
                        self.pointer,
                    );
                }
                let window = self.window.as_ref().unwrap();
                if let Ok(origin) = window.inner_position() {
                    let scale = window.scale_factor();
                    let context = self.command_context();
                    let commands: Vec<_> = if self.pointer.y < 34.0 {
                        ["file.save", "file.save_as", "file.close"]
                            .into_iter()
                            .map(bareline_commands::CommandId)
                            .collect()
                    } else if let Some(commands) = panel_commands {
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
                        origin.x + (self.pointer.x as f64 * scale) as i32,
                        origin.y + (self.pointer.y as f64 * scale) as i32,
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
            WindowEvent::Ime(event) => {
                if self.app.palette {
                    let context = self.command_context();
                    let keymap = self.settings.keymap.keymap.clone();
                    match event {
                        Ime::Preedit(value, cursor) => self.palette.preedit(value, cursor),
                        Ime::Commit(value) => {
                            self.palette
                                .commit(&value, &self.app.commands, &context, &keymap);
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
                            Ime::Preedit(text, cursor) => {
                                workspace.find.active_field().preedit(text, cursor)
                            }
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
                if let Some(editor) = self
                    .workspace
                    .as_mut()
                    .and_then(|w| w.editors.get_mut(self.app.active))
                {
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
            WindowEvent::Focused(false) => {
                if let Some(workspace) = &mut self.workspace {
                    workspace.find.active_field().cancel();
                }
                if let Some(editor) = self
                    .workspace
                    .as_mut()
                    .and_then(|w| w.editors.get_mut(self.app.active))
                {
                    editor.cancel_composition();
                }
                if let Some(p) = &mut self.prototype {
                    p.cancel();
                }
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let editor_focused = !self.palette.open
                    && self.workspace.as_ref().is_some_and(|workspace| {
                        !workspace.find.has_focus() && !workspace.search_focus
                    });
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
                    let context = self.command_context();
                    let keymap = self.settings.keymap.keymap.clone();
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
                            Key::Character(value)
                                if self.modifiers.control_key() && !self.modifiers.alt_key() =>
                            {
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
                            Key::Named(NamedKey::ArrowLeft) => {
                                self.palette.field.horizontal(false, extend)
                            }
                            Key::Named(NamedKey::ArrowRight) => {
                                self.palette.field.horizontal(true, extend)
                            }
                            Key::Named(NamedKey::Home) => self.palette.field.edge(false, extend),
                            Key::Named(NamedKey::End) => self.palette.field.edge(true, extend),
                            Key::Named(NamedKey::Backspace) => {
                                self.palette
                                    .delete(false, &self.app.commands, &context, &keymap);
                            }
                            Key::Named(NamedKey::Delete) => {
                                self.palette
                                    .delete(true, &self.app.commands, &context, &keymap);
                            }
                            _ if !self.modifiers.control_key() || self.modifiers.alt_key() => {
                                if let Some(value) = &event.text {
                                    self.palette.insert(
                                        value,
                                        &self.app.commands,
                                        &context,
                                        &keymap,
                                    );
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
                            Key::Character(value)
                                if self.modifiers.control_key() && !self.modifiers.alt_key() =>
                            {
                                match value.to_ascii_lowercase().as_str() {
                                    "a" => field.select_all(),
                                    "v" => {
                                        if let Ok(value) =
                                            self.platform.as_ref().unwrap().clipboard_text()
                                        {
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
                if let Some(key_name) = key_name {
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
                            && (workspace.find.field.composing()
                                || workspace.find.replacement.composing()))
                            || workspace
                                .editors
                                .get(self.app.active)
                                .is_some_and(|editor| editor.composition_text().is_some())
                    });
                    if let Ok(chord) = bareline_commands::KeyChord::parse(&chord)
                        && let bareline_commands::KeyResolution::Command(id) =
                            bareline_commands::Keymap::defaults(&self.app.commands).resolve(
                                &[chord],
                                bareline_commands::InputContext {
                                    alt_gr: self.modifiers.control_key()
                                        && self.modifiers.alt_key(),
                                    ime_composing: composing,
                                    dead_key: matches!(event.logical_key, Key::Dead(_)),
                                },
                            )
                        && let Ok(action) =
                            self.app.commands.dispatch_in(id, &self.command_context())
                    {
                        self.dispatch(el, action);
                        return;
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
                                    Key::Named(NamedKey::ArrowLeft) => {
                                        field.horizontal(false, extend)
                                    }
                                    Key::Named(NamedKey::ArrowRight) => {
                                        field.horizontal(true, extend)
                                    }
                                    Key::Named(NamedKey::Home) => field.edge(false, extend),
                                    Key::Named(NamedKey::End) => field.edge(true, extend),
                                    Key::Named(NamedKey::Backspace) => {
                                        field.delete(false);
                                    }
                                    Key::Named(NamedKey::Delete) => {
                                        field.delete(true);
                                    }
                                    _ if !self.modifiers.control_key()
                                        || self.modifiers.alt_key() =>
                                    {
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
                    if let Some(editor) = self
                        .workspace
                        .as_mut()
                        .and_then(|w| w.editors.get_mut(self.app.active))
                    {
                        let extend = self.modifiers.shift_key();
                        if matches!(
                            event.logical_key,
                            Key::Named(NamedKey::PageDown | NamedKey::PageUp)
                        ) {
                            let forward =
                                matches!(event.logical_key, Key::Named(NamedKey::PageDown));
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
                            Key::Named(NamedKey::ArrowLeft) => Some(Input::Left(extend)),
                            Key::Named(NamedKey::ArrowRight) => Some(Input::Right(extend)),
                            Key::Named(NamedKey::ArrowUp) => Some(Input::Up(extend)),
                            Key::Named(NamedKey::ArrowDown) => Some(Input::Down(extend)),
                            Key::Named(NamedKey::Home) => Some(Input::Home(extend)),
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
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let window = self.window.as_ref().unwrap();
                if let Some(editor) = self
                    .workspace
                    .as_mut()
                    .and_then(|w| w.editors.get_mut(self.app.active))
                {
                    let (horizontal, vertical, zoom) = match delta {
                        MouseScrollDelta::LineDelta(x, y) => {
                            (-x as f64 * 72.0, -y as f64 * 72.0, y)
                        }
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
                                if let Err(error) =
                                    paged.scroll_viewport(vertical, editor_bounds.height)
                                {
                                    paged.surface.error = Some(error);
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
                if self.renderer.is_none() {
                    self.ledger.record(StartupAction::CreateRenderer);
                    let _renderer_phase =
                        bareline_diagnostics::startup_span(StartupAction::CreateRenderer);
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
                            bareline_diagnostics::set_renderer_state(
                                bareline_diagnostics::RendererState::Failed,
                            );
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
                let window = self.window.as_ref().unwrap();
                let size = window.inner_size();
                if size.width == 0 || size.height == 0 {
                    return;
                }
                let scale = window.scale_factor() as f32;
                let renderer = self.renderer.as_mut().unwrap();
                let mut operations = bareline_ui::shell_with_theme(
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                    &self.app.tabs,
                    self.app.active,
                    false,
                    self.settings.ui_theme(),
                );
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
                            .language_override
                            .or(editor.detected_language)
                            .unwrap_or(detected[index]);
                        let stable_id = bareline_syntax::catalog::CATALOG
                            .iter()
                            .find(|entry| entry.language == language)
                            .map_or("text", |entry| entry.id);
                        editor.syntax_preference = match effective.language_policy(stable_id).lexer
                        {
                            bareline_settings::LexerPreference::Primary => {
                                bareline_syntax::LexerPreference::Lexilla
                            }
                            bareline_settings::LexerPreference::Native => {
                                bareline_syntax::LexerPreference::Native
                            }
                        };
                    }
                    if self
                        .applied_settings
                        .as_ref()
                        .is_none_or(|(previous, count)| {
                            *previous != effective || *count != workspace.editors.len()
                        })
                    {
                        workspace.resident_max_bytes = effective.resident_max_bytes;
                        workspace.transcode_quota_bytes = effective.transcode_quota_bytes;
                        let editor_theme = self.settings.editor_theme();
                        for editor in &mut workspace.editors {
                            editor.theme = editor_theme;
                            editor.set_wrap(effective.word_wrap);
                            if let Err(error) =
                                editor.set_font_family(&effective.editor_font_family)
                            {
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
                    let editor_start = operations.len();
                    operations.push(bareline_renderer::DrawOp::PushClip(bareline_ui::rect(
                        editor_bounds.x,
                        34.0 + editor_bounds.y,
                        editor_bounds.width,
                        (editor_bounds.height - 34.0).max(0.0),
                    )));
                    match self.views.draw(
                        workspace,
                        &mut self.app,
                        renderer,
                        editor_bounds.width,
                        editor_bounds.height,
                        &mut operations,
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
                            return;
                        }
                    }
                    if let Err(error) = self.compare.draw(
                        workspace,
                        &mut self.views,
                        &self.settings,
                        renderer,
                        editor_bounds.width,
                        editor_bounds.height,
                        &mut operations,
                    ) {
                        self.fail(el, format!("compare layout: {error:?}"));
                        return;
                    }
                    self.recovery.draw(
                        self.settings.ui_theme(),
                        editor_bounds.width,
                        editor_bounds.height,
                        &mut operations,
                    );
                    translate_operations(
                        &mut operations[editor_start + 1..],
                        editor_bounds.x,
                        editor_bounds.y,
                    );
                    operations.push(bareline_renderer::DrawOp::PopClip);
                }
                if let Some(workspace) = &self.workspace
                    && let Err(error) = self.panels.draw(
                        renderer,
                        size.width as f32 / scale,
                        size.height as f32 / scale,
                        workspace,
                        self.app.active,
                        &mut operations,
                    )
                {
                    self.fail(el, format!("panel layout: {error:?}"));
                    return;
                }
                if let Some(workspace) = &self.workspace {
                    self.scrolling.draw(
                        workspace,
                        &self.views,
                        self.app.active,
                        editor_bounds,
                        &mut operations,
                    );
                }
                self.search.draw(
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                    &mut operations,
                );
                self.extensions.draw(
                    renderer,
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                    &mut operations,
                );
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
                    &mut operations,
                ) {
                    self.fail(el, format!("language layout: {error:?}"));
                    return;
                }
                if let Err(error) = self.settings.draw(
                    renderer,
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                    &mut operations,
                ) {
                    self.fail(el, format!("settings layout: {error:?}"));
                    return;
                }
                if let Some(p) = &mut self.prototype {
                    match p.draw(renderer, size.width as f32 / scale, &mut operations) {
                        Ok(caret) => window.set_ime_cursor_area(
                            LogicalPosition::new(caret.x as f64, caret.y as f64),
                            LogicalSize::new(caret.width as f64, caret.height as f64),
                        ),
                        Err(error) => {
                            self.fail(el, format!("text layout: {error:?}"));
                            return;
                        }
                    }
                }
                if let Err(error) = self.power.draw(
                    renderer,
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                    &mut operations,
                ) {
                    self.fail(el, format!("power editor layout: {error:?}"));
                    return;
                }
                if let Err(error) = self.toolbar.draw(
                    renderer,
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                    &mut operations,
                ) {
                    self.fail(el, format!("toolbar layout: {error:?}"));
                    return;
                }
                match self.shortcuts.draw(
                    renderer,
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                    self.settings.ui_theme(),
                    &self.app.commands,
                    &self.settings.keymap.keymap,
                    &mut operations,
                ) {
                    Ok(Some(caret)) => window.set_ime_cursor_area(
                        LogicalPosition::new(caret.x as f64, caret.y as f64),
                        LogicalSize::new(caret.width as f64, caret.height as f64),
                    ),
                    Ok(None) => {}
                    Err(error) => {
                        self.fail(el, format!("shortcut layout: {error:?}"));
                        return;
                    }
                }
                self.macros.draw(
                    renderer,
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                    &mut operations,
                );
                self.utilities.draw(
                    &self.settings,
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                    &mut operations,
                );
                if self.palette.open {
                    match self.palette.draw_with_theme(
                        renderer,
                        size.width as f32 / scale,
                        size.height as f32 / scale,
                        self.settings.ui_theme(),
                        &mut operations,
                    ) {
                        Ok(caret) => window.set_ime_cursor_area(
                            LogicalPosition::new(caret.x as f64, caret.y as f64),
                            LogicalSize::new(caret.width as f64, caret.height as f64),
                        ),
                        Err(error) => {
                            self.fail(el, format!("palette layout: {error:?}"));
                            return;
                        }
                    }
                } else {
                    self.palette.release(renderer);
                }
                let result = renderer
                    .resize(size.width, size.height, scale)
                    .and_then(|_| {
                        let mut frame = renderer.begin_frame();
                        frame.extend(&operations);
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
                            if !self.smoke && !self.perf && !self.performance.enabled() {
                                if let Err(error) = bareline_platform_windows::shell_integration::initialize_jump_list(self.shell_integration.portable) {
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
                                        if let Some((code, software)) = renderer.take_init_failure()
                                        {
                                            let _ = log.event(
                                                bareline_diagnostics::Event::BackendInitFailed {
                                                    code,
                                                    software,
                                                },
                                            );
                                        }
                                        let _ = log.ledger(&self.ledger);
                                        self.log = Some(log);
                                    }
                                    Err(error) => eprintln!(
                                        "event=diagnostics_unavailable kind={:?}",
                                        error.kind()
                                    ),
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
                        bareline_diagnostics::set_renderer_state(
                            bareline_diagnostics::RendererState::Recreating,
                        );
                        window.request_redraw();
                    }
                    Err(error) => {
                        bareline_diagnostics::set_renderer_state(
                            bareline_diagnostics::RendererState::Failed,
                        );
                        self.fail(el, error);
                    }
                }
                if presented {
                    self.performance_frame();
                }
                let accessible = self.accessibility_snapshot(
                    (size.width as f32 / scale) as f64,
                    (size.height as f32 / scale) as f64,
                    scale as f64,
                );
                let text_source = self.accessibility_text_source();
                if let Some(provider) = &mut self.accessibility {
                    provider.set_text_source(text_source);
                    provider.update(accessible);
                }
                if let Some(platform) = &self.platform
                    && let Err(error) = platform.sync_commands_localized(
                        &self.app.commands,
                        &self.command_context(),
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
                self.settings.load_keymap(&self.app.commands);
                self.session_first_frame(el);
                self.recovery_pump(el);
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
                        for path in std::mem::take(&mut self.startup_paths) {
                            self.workspace.as_mut().unwrap().open(path);
                        }
                        self.window.as_ref().unwrap().request_redraw();
                    }
                }
            }
            _ => {}
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

impl Shell {
    fn sync_contributions(&mut self) {
        let records = self.extensions.contributions();
        if self.app.commands.contributions.entries().eq(records.iter()) {
            return;
        }
        let mut owners = std::collections::BTreeMap::<
            String,
            Vec<bareline_commands::DynamicCommandRecord>,
        >::new();
        for record in records {
            owners
                .entry(record.identity.owner.clone())
                .or_default()
                .push(record);
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
    }
}
