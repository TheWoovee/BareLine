// SPDX-License-Identifier: MPL-2.0
use super::*;
use bareline_app::settings::{SettingsController, SettingsEffect};
use bareline_commands::{
    CommandRegistry, InputContext, Key as ChordKey, KeyChord, KeyResolution, Keymap,
};
use bareline_renderer::{DrawOp, LayoutError, Rect};
use bareline_settings::{
    EffectiveSettings, KeymapDocument, Scope, SettingsDocument, SystemAppearance, Theme,
};
use bareline_ui::controls::{Key as UiKey, UiEvent};
use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::Instant,
};

pub(super) struct SettingsRuntime {
    pub controller: SettingsController,
    pub keymap: KeymapDocument,
    path: Option<PathBuf>,
    keymap_path: Option<PathBuf>,
    notify: Arc<dyn Fn() + Send + Sync>,
    keymap_result: Option<Receiver<Result<KeymapDocument, String>>>,
    keymap_loaded: bool,
    pending: Vec<KeyChord>,
    pending_at: Instant,
    alt_gr: bool,
    ime: bool,
    dead_key: bool,
}
impl Default for SettingsRuntime {
    fn default() -> Self {
        Self::new(SettingsDocument::empty(Scope::User), None, Arc::new(|| {}))
    }
}
impl SettingsRuntime {
    pub fn new(
        document: SettingsDocument,
        path: Option<PathBuf>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let controller = SettingsController::new(
            document,
            None,
            SystemAppearance {
                dark: true,
                high_contrast: false,
            },
        );
        let keymap_path = path.as_ref().map(|p| p.with_file_name("keymap.toml"));
        Self {
            controller,
            keymap: KeymapDocument::defaults(&bareline_commands::shell_commands()),
            path,
            keymap_path,
            notify,
            keymap_result: None,
            keymap_loaded: false,
            pending: Vec::new(),
            pending_at: Instant::now(),
            alt_gr: false,
            ime: false,
            dead_key: false,
        }
    }
    pub fn effective(&self) -> EffectiveSettings {
        self.controller.effective()
    }
    pub fn ui_theme(&self) -> bareline_ui::theme::UiTheme {
        let settings = self.effective();
        let theme = Theme::resolve(
            settings.theme,
            self.controller.system,
            &settings.theme_overrides,
        )
        .unwrap_or_else(|_| Theme::builtin(self.controller.system.dark));
        bareline_ui::theme::UiTheme::from_tokens(|key| theme.color(key).map(|c| (c.rgb, c.alpha)))
            .unwrap_or_default()
    }
    pub fn editor_theme(&self) -> bareline_ui::theme::EditorTheme {
        let settings = self.effective();
        Theme::resolve(
            settings.theme,
            self.controller.system,
            &settings.theme_overrides,
        )
        .ok()
        .and_then(|theme| {
            bareline_ui::theme::EditorTheme::from_tokens(|key| {
                theme.color(key).map(|c| (c.rgb, c.alpha))
            })
        })
        .unwrap_or_default()
    }
    pub fn theme_color(&self, key: &str) -> Option<bareline_renderer::Color> {
        let settings = self.effective();
        let theme = Theme::resolve(
            settings.theme,
            self.controller.system,
            &settings.theme_overrides,
        )
        .ok()?;
        let color = theme.color(key)?;
        let background = theme.color("surface.editor")?.rgb;
        let mut rgb = 0;
        for shift in [16, 8, 0] {
            let foreground = (color.rgb >> shift) & 255;
            let behind = (background >> shift) & 255;
            rgb |= ((foreground * color.alpha as u32 + behind * (255 - color.alpha as u32) + 127)
                / 255)
                << shift;
        }
        Some(bareline_renderer::Color(rgb))
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = self.controller.poll();
        if let Some(result) = self
            .keymap_result
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.keymap_result = None;
            changed = true;
            match result {
                Ok(document) => {
                    self.keymap = document;
                    self.pending.clear();
                }
                Err(error) => self.controller.error = Some(error),
            }
        }
        changed
    }
    pub fn load_keymap(&mut self, registry: &CommandRegistry) {
        if !self.keymap_loaded {
            self.keymap_loaded = true;
            // Called after the first frame (or on explicit Settings open).
            if let Some(path) = &self.path
                && let Err(error) = self.controller.configure_storage(
                    path.clone(),
                    None,
                    Arc::new(bareline_platform_windows::WindowsFileSystem),
                    self.notify.clone(),
                )
            {
                self.controller.error = Some(error.to_string());
            }
            self.keymap = KeymapDocument::defaults(registry);
            if let Some(path) = self.keymap_path.clone()
                && path.exists()
            {
                self.import_keymap(path, registry, false);
            }
        }
    }
    pub fn import_keymap(&mut self, path: PathBuf, registry: &CommandRegistry, persist: bool) {
        if self.keymap_result.is_some() {
            self.controller.error = Some("A keymap operation is already running".into());
            return;
        }
        let mut owned_registry = CommandRegistry::default();
        for entry in registry.entries() {
            let _ = owned_registry.register(entry.clone());
        }
        let target = self.keymap_path.clone();
        let wake = self.notify.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        self.keymap_result = Some(rx);
        let spawn = std::thread::Builder::new()
            .name("bareline-keymap-load".into())
            .spawn(move || {
                let result = (|| {
                    let document =
                        KeymapDocument::load(&path, &owned_registry).map_err(|e| e.to_string())?;
                    if persist {
                        let target = target.ok_or("Keymap storage is unavailable")?;
                        if let Some(parent) = target.parent() {
                            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                        }
                        document
                            .save(&target, &bareline_platform_windows::WindowsFileSystem)
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(document)
                })();
                let _ = tx.send(result);
                wake();
            });
        if let Err(error) = spawn {
            self.keymap_result = None;
            self.controller.error = Some(error.to_string());
        }
    }
    pub(super) fn keymap_path(&self) -> Option<PathBuf> {
        self.keymap_path.clone()
    }
    pub(super) fn keymap_busy(&self) -> bool {
        self.keymap_result.is_some()
    }
    pub(super) fn save_keymap(&mut self, document: KeymapDocument, target: Option<PathBuf>) {
        if self.keymap_result.is_some() {
            self.controller.error = Some("A keymap operation is already running".into());
            return;
        }
        self.controller.error = None;
        let Some(target) = target.or_else(|| self.keymap_path.clone()) else {
            self.controller.error = Some("Keymap storage is unavailable".into());
            return;
        };
        let (tx, rx) = mpsc::sync_channel(1);
        self.keymap_result = Some(rx);
        let wake = self.notify.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("bareline-keymap-save".into())
            .spawn(move || {
                let result = (|| {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    document
                        .save(&target, &bareline_platform_windows::WindowsFileSystem)
                        .map_err(|e| e.to_string())?;
                    Ok(document)
                })();
                let _ = tx.send(result);
                wake();
            })
        {
            self.keymap_result = None;
            self.controller.error = Some(error.to_string());
        }
    }
    pub fn draw(
        &mut self,
        renderer: &mut WindowsRenderer,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        if !self.controller.open {
            return Ok(None);
        }
        let bounds = bareline_ui::rect(
            0.0,
            bareline_ui::TAB_HEIGHT,
            width,
            (height - bareline_ui::TAB_HEIGHT - bareline_ui::STATUS_HEIGHT).max(0.0),
        );
        self.controller.draw(bounds, renderer, ops)?;
        Ok(Some(bounds))
    }
}
impl Shell {
    pub(super) fn settings_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        match id {
            "settings.open" => {
                self.settings.controller.show();
                self.palette.dismiss();
                self.settings.load_keymap(&self.app.commands);
            }
            "settings.close" => self.settings.controller.dismiss(),
            "settings.retry" => self.settings.controller.retry_save(),
            "settings.revert" => self.settings.controller.revert_changes(),
            "settings.reset_section" => self.settings.controller.request_reset(),
            "settings.copy_key" => {
                if let Some(key) = self.settings.controller.copy_selected_key()
                    && let Some(platform) = &self.platform
                    && let Err(error) = platform.set_clipboard_text(&key)
                {
                    self.settings.controller.error = Some(error.to_string());
                }
            }
            "settings.keymap_import" => {
                if let Some(platform) = &self.platform {
                    match platform.open_file() {
                        Ok(Some(path)) => {
                            self.settings.import_keymap(path, &self.app.commands, true)
                        }
                        Ok(None) => {}
                        Err(error) => self.settings.controller.error = Some(error),
                    }
                }
            }
            "settings.keymap_open" => {
                if let Some(path) = self.settings.keymap_path.clone()
                    && let Some(workspace) = &mut self.workspace
                {
                    workspace.open(path);
                    self.settings.controller.dismiss();
                }
            }
            "settings.change" => {
                let effect = self.settings.controller.event(UiEvent::Key(UiKey::Enter));
                self.settings_effect(effect);
            }
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    fn settings_effect(&mut self, effect: Option<SettingsEffect>) {
        match effect {
            Some(SettingsEffect::CopyKey(key)) => {
                if let Some(platform) = &self.platform
                    && let Err(error) = platform.set_clipboard_text(&key)
                {
                    self.settings.controller.error = Some(error.to_string());
                }
            }
            Some(SettingsEffect::OpenToml(scope)) => {
                if scope == Scope::User {
                    let path = if self.settings.controller.category == "Keyboard" {
                        self.settings.keymap_path.clone()
                    } else {
                        self.settings.path.clone()
                    };
                    if let (Some(path), Some(workspace)) = (path, &mut self.workspace) {
                        workspace.open(path);
                        self.settings.controller.dismiss();
                    }
                } else {
                    self.settings.controller.error =
                        Some("Workspace settings path is unavailable".into());
                }
            }
            Some(SettingsEffect::Close) => self.settings.controller.dismiss(),
            Some(SettingsEffect::PreviewChanged) | None => {}
        }
    }
    pub(super) fn settings_event(&mut self, _el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if let WindowEvent::ThemeChanged(theme) = event {
            self.settings.controller.system.dark = *theme == winit::window::Theme::Dark;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }
        if !self.settings.controller.open {
            return false;
        }
        let mut effect = None;
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(window) = &self.window {
                    let logical = position.to_logical::<f32>(window.scale_factor());
                    self.pointer = Point {
                        x: logical.x,
                        y: logical.y,
                    };
                    effect = self
                        .settings
                        .controller
                        .event(UiEvent::PointerMove(self.pointer));
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                effect = self
                    .settings
                    .controller
                    .event(if *state == ElementState::Pressed {
                        UiEvent::PointerDown(self.pointer)
                    } else {
                        UiEvent::PointerUp(self.pointer)
                    });
                if *state == ElementState::Pressed
                    && self.settings.controller.query_focused
                    && let Some(renderer) = &self.renderer
                {
                    let _ = self.settings.controller.query.click(
                        renderer,
                        self.pointer,
                        self.modifiers.shift_key(),
                    );
                }
            }
            WindowEvent::Ime(Ime::Preedit(value, cursor)) => {
                if self.settings.controller.query_focused {
                    self.settings
                        .controller
                        .query
                        .preedit(value.clone(), *cursor);
                }
            }
            WindowEvent::Ime(Ime::Commit(value)) => {
                if self.settings.controller.query_focused {
                    self.settings.controller.query.commit(value);
                    self.settings.controller.query_changed();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let ctrl = self.modifiers.control_key();
                let shift = self.modifiers.shift_key();
                if self.settings.controller.query_focused
                    && !self.settings.controller.query.composing()
                {
                    let field = &mut self.settings.controller.query;
                    match &event.logical_key {
                        Key::Named(NamedKey::Backspace) => {
                            field.delete(false);
                        }
                        Key::Named(NamedKey::Delete) => {
                            field.delete(true);
                        }
                        Key::Named(NamedKey::ArrowLeft) => field.horizontal(false, shift),
                        Key::Named(NamedKey::ArrowRight) => field.horizontal(true, shift),
                        Key::Named(NamedKey::Home) => field.edge(false, shift),
                        Key::Named(NamedKey::End) => field.edge(true, shift),
                        Key::Character(key) if ctrl && !self.modifiers.alt_key() => {
                            match key.to_ascii_lowercase().as_str() {
                                "a" => field.select_all(),
                                "c" | "x" => {
                                    if let Some(platform) = &self.platform
                                        && platform.set_clipboard_text(field.selected()).is_ok()
                                        && key.eq_ignore_ascii_case("x")
                                    {
                                        field.insert("");
                                    }
                                }
                                "v" => {
                                    if let Some(platform) = &self.platform
                                        && let Ok(value) = platform.clipboard_text()
                                    {
                                        field.commit(&value);
                                    }
                                }
                                "z" => field.undo(shift),
                                "y" => field.undo(true),
                                _ => {}
                            }
                        }
                        Key::Character(value) if !ctrl || self.modifiers.alt_key() => {
                            field.insert(value);
                        }
                        _ => {}
                    }
                    self.settings.controller.query_changed();
                }
                let normalized = match &event.logical_key {
                    Key::Named(NamedKey::Escape) => Some(UiKey::Escape),
                    Key::Named(NamedKey::Tab) => Some(UiKey::Tab),
                    Key::Named(NamedKey::Enter) => Some(UiKey::Enter),
                    Key::Named(NamedKey::Space) => Some(UiKey::Space),
                    Key::Named(NamedKey::ArrowDown) => Some(UiKey::Down),
                    Key::Named(NamedKey::ArrowUp) => Some(UiKey::Up),
                    _ => None,
                };
                if let Some(key) = normalized {
                    effect = self.settings.controller.event(UiEvent::Key(key));
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                return false;
            }
            WindowEvent::Focused(value) => {
                effect = self.settings.controller.event(UiEvent::Focus(*value));
                if !value {
                    self.settings.controller.query.cancel();
                }
            }
            WindowEvent::RedrawRequested
            | WindowEvent::Resized(_)
            | WindowEvent::ScaleFactorChanged { .. }
            | WindowEvent::CloseRequested => return false,
            _ => {}
        }
        self.settings_effect(effect);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    /// Call before legacy shortcuts, but after focused palette/settings text handling.
    pub(super) fn settings_keymap_event(
        &mut self,
        el: &ActiveEventLoop,
        event: &WindowEvent,
    ) -> bool {
        use winit::keyboard::{KeyCode, PhysicalKey};
        if let WindowEvent::Ime(Ime::Preedit(text, _)) = event {
            self.settings.ime = !text.is_empty();
            return false;
        }
        if matches!(event, WindowEvent::Ime(Ime::Commit(_))) {
            self.settings.ime = false;
            return false;
        }
        let WindowEvent::KeyboardInput { event, .. } = event else {
            return false;
        };
        if event.physical_key == PhysicalKey::Code(KeyCode::AltRight) {
            self.settings.alt_gr = event.state == ElementState::Pressed;
        }
        if event.state != ElementState::Pressed || self.palette.open {
            return false;
        }
        if matches!(event.logical_key, Key::Dead(_)) {
            self.settings.dead_key = true;
            return false;
        }
        let dead_key = std::mem::take(&mut self.settings.dead_key);
        let context = InputContext {
            alt_gr: self.settings.alt_gr,
            ime_composing: self.settings.ime,
            dead_key,
        };
        if context.alt_gr || context.ime_composing || context.dead_key {
            return false;
        }
        if self.settings.pending_at.elapsed() > Duration::from_secs(2) {
            self.settings.pending.clear();
        }
        let logical = match &event.logical_key {
            Key::Character(value) => Some(value.to_uppercase()),
            Key::Named(name) => Some(format!("{name:?}").to_uppercase()),
            _ => None,
        };
        let mut candidates = Vec::new();
        if let PhysicalKey::Code(code) = event.physical_key {
            candidates.push(ChordKey::Physical(format!("{code:?}")));
        }
        if let Some(logical) = logical {
            candidates.push(ChordKey::Logical(logical));
        }
        let field_layer = self.settings.controller.open || self.shortcuts.open;
        let mut field_keymap = Keymap::default();
        if field_layer {
            let bindings = self
                .settings
                .keymap
                .keymap
                .bindings()
                .iter()
                .filter(|binding| {
                    self.app.commands.dispatch(binding.command) == Some(Action::Palette)
                })
                .cloned()
                .collect();
            let _ = field_keymap.replace(bindings, &self.app.commands);
        }
        let active_keymap = if field_layer {
            &field_keymap
        } else {
            &self.settings.keymap.keymap
        };
        let had_pending = !self.settings.pending.is_empty();
        for key in candidates {
            let chord = KeyChord {
                ctrl: self.modifiers.control_key(),
                alt: self.modifiers.alt_key(),
                shift: self.modifiers.shift_key(),
                meta: self.modifiers.super_key(),
                key,
            };
            let mut sequence = self.settings.pending.clone();
            sequence.push(chord.clone());
            match active_keymap.resolve(&sequence, context) {
                KeyResolution::Command(id) => {
                    self.settings.pending.clear();
                    if let Ok(action) = self.app.commands.dispatch_in(id, &self.command_context()) {
                        self.dispatch(el, action);
                    }
                    return true;
                }
                KeyResolution::Pending => {
                    self.settings.pending = sequence;
                    self.settings.pending_at = Instant::now();
                    return true;
                }
                _ => {}
            }
            // A removed/reassigned default shortcut must not fall through to the legacy binding.
            if !field_layer
                && !had_pending
                && matches!(
                    Keymap::defaults(&self.app.commands).resolve(&[chord], context),
                    KeyResolution::Command(_)
                )
            {
                return true;
            }
        }
        self.settings.pending.clear();
        had_pending
    }
}

impl Shell {
    pub(super) fn settings_text_action(&mut self, action: Action) -> bool {
        if !self.settings.controller.open
            || self.palette.open
            || self.shortcuts.open
            || !matches!(
                action,
                Action::Copy
                    | Action::Cut
                    | Action::Paste
                    | Action::SelectAll
                    | Action::Undo
                    | Action::Redo
            )
        {
            return false;
        }
        if self.settings.controller.query_focused {
            let field = &mut self.settings.controller.query;
            match action {
                Action::SelectAll => field.select_all(),
                Action::Undo => field.undo(false),
                Action::Redo => field.undo(true),
                Action::Paste => {
                    if let Some(platform) = &self.platform
                        && let Ok(value) = platform.clipboard_text()
                    {
                        field.commit(&value);
                    }
                }
                Action::Copy | Action::Cut => {
                    if let Some(platform) = &self.platform
                        && platform.set_clipboard_text(field.selected()).is_ok()
                        && action == Action::Cut
                    {
                        field.insert("");
                    }
                }
                _ => {}
            }
            self.settings.controller.query_changed();
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}
