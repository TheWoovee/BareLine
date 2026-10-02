// SPDX-License-Identifier: MPL-2.0
use super::*;
use bareline_app::settings::{SettingsController, SettingsEffect};
use bareline_commands::{CommandRegistry, InputContext, KeyChord, KeyPress, KeyResolution, Keymap, KeymapPreset};
use bareline_renderer::{DrawOp, LayoutError, Rect};
use bareline_settings::{EffectiveSettings, KeymapDocument, Scope, SettingsDocument, SystemAppearance, Theme};
use bareline_ui::controls::{Key as UiKey, UiEvent};
use std::{
    cell::{Cell, RefCell},
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::Instant,
};

/// A winit key press in the keymap's platform-neutral terms. The unmodified
/// key lets bindings written with a base character, such as Ctrl+Shift+/,
/// match although Shift turns the logical key into "?" (WSP-06).
pub(super) fn key_press(modifiers: ModifiersState, event: &winit::event::KeyEvent) -> KeyPress {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    KeyPress {
        ctrl: modifiers.control_key(),
        alt: modifiers.alt_key(),
        shift: modifiers.shift_key(),
        meta: modifiers.super_key(),
        physical: physical_key_name(event.physical_key),
        logical: key_name(&event.logical_key),
        unmodified: key_name(&event.key_without_modifiers()),
    }
}
/// A logical key's keymap name: the typed text, or the named key ("ArrowUp").
pub(super) fn key_name(key: &Key) -> Option<String> {
    match key {
        Key::Character(value) => Some(value.to_string()),
        Key::Named(named) => Some(format!("{named:?}")),
        _ => None,
    }
}
/// A physical key's keymap name, the W3C code ("Slash", "KeyA").
pub(super) fn physical_key_name(key: winit::keyboard::PhysicalKey) -> Option<String> {
    match key {
        winit::keyboard::PhysicalKey::Code(code) => Some(format!("{code:?}")),
        winit::keyboard::PhysicalKey::Unidentified(_) => None,
    }
}
/// Resolved settings and themes for one revision of the settings documents.
/// Resolving costs a document clone, re-validation and a token-map rebuild, so a
/// frame that reads it a dozen times must not pay for it a dozen times (ARCH-04).
struct ResolvedCache {
    revision: u64,
    dark: bool,
    high_contrast: bool,
    settings: EffectiveSettings,
    theme: Theme,
    ui: bareline_ui::theme::UiTheme,
    editor: bareline_ui::theme::EditorTheme,
}

pub(super) struct SettingsRuntime {
    pub controller: SettingsController,
    pub keymap: KeymapDocument,
    path: Option<PathBuf>,
    keymap_path: Option<PathBuf>,
    notify: Arc<dyn Fn() + Send + Sync>,
    keymap_result: Option<Receiver<Result<KeymapDocument, String>>>,
    keymap_loaded: bool,
    /// First run: once settings storage is ready, save the default shortcut
    /// preset so the preset notice is offered only once (BIZ-08).
    pub(super) record_keymap_preset: bool,
    /// The preset a keymap switch was last attempted for, so a failing switch
    /// is not retried every frame.
    preset_requested: Option<KeymapPreset>,
    /// Startup left an unusable settings file in place; saving would replace it.
    storage_blocked: bool,
    locale_requested: String,
    locale_result: Option<Receiver<Result<bareline_settings::LocalePack, String>>>,
    pub language_change: Option<bareline_settings::LanguageChange>,
    workspace_requested: Option<PathBuf>,
    workspace_loaded: Option<PathBuf>,
    workspace_result: Option<Receiver<(PathBuf, Result<SettingsDocument, String>)>>,
    fonts_result: Option<Receiver<Vec<(String, bool)>>>,
    /// The installed font families changed; the shell refreshes the renderer.
    pub fonts_changed: bool,
    cache: RefCell<Option<ResolvedCache>>,
    /// The single cached default `Keymap`. Building it walks all ~444 commands,
    /// so the keystroke resolver (both the chord path in `window_event` and the
    /// user-keymap fallback here) shares this one copy instead of rebuilding it
    /// per key press (ARCH-05). Rebuilt only when the command set or the loaded
    /// keymap changes; `keymap_rebuilds` counts each rebuild.
    default_keymap: RefCell<Option<Keymap>>,
    default_keymap_key: Cell<Option<(usize, u64)>>,
    keymap_rebuilds: Cell<usize>,
    keymap_revision: u64,
    pending: Vec<KeyChord>,
    pending_at: Instant,
    alt_gr: bool,
    ime: bool,
    dead_key: bool,
}
/// Read and parse the user settings file that profile migration just published.
/// Runs on the profile-initialization worker, never on the UI thread (APP-12).
pub(super) fn read_migrated_user(path: &std::path::Path) -> Result<SettingsDocument, String> {
    use bareline_platform::LocalFileSystem;
    use std::io::Read;
    let platform = bareline_platform_windows::WindowsFileSystem;
    let lease = platform
        .migration_entry_guard(path)
        .map_err(|error| error.to_string())?;
    let file = platform
        .open_migration_read(&lease)
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    let limit = bareline_settings::MAX_CONFIG_BYTES;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err("Migrated settings file is invalid".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err("Migrated settings file exceeds its size limit".into());
    }
    SettingsDocument::parse(&bytes, Scope::User).map_err(|error| error.to_string())
}
impl Default for SettingsRuntime {
    fn default() -> Self {
        Self::new(SettingsDocument::empty(Scope::User), None, Arc::new(|| {}))
    }
}
impl SettingsRuntime {
    pub fn new(document: SettingsDocument, path: Option<PathBuf>, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let controller = SettingsController::new(
            document,
            None,
            SystemAppearance {
                dark: true,
                high_contrast: bareline_platform_windows::high_contrast_enabled().unwrap_or(false),
                highlight: bareline_platform_windows::high_contrast_highlight(),
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
            record_keymap_preset: false,
            preset_requested: None,
            storage_blocked: false,
            locale_requested: String::new(),
            locale_result: None,
            language_change: None,
            workspace_requested: None,
            workspace_loaded: None,
            workspace_result: None,
            fonts_result: None,
            fonts_changed: false,
            pending: Vec::new(),
            cache: RefCell::new(None),
            default_keymap: RefCell::new(None),
            default_keymap_key: Cell::new(None),
            keymap_rebuilds: Cell::new(0),
            keymap_revision: 0,
            pending_at: Instant::now(),
            alt_gr: false,
            ime: false,
            dead_key: false,
        }
    }
    /// Adopt the settings document profile migration moved into place, unless the
    /// live settings changed since `expected_revision`. The profile-initialization
    /// worker read it (read_migrated_user), so the UI thread does no file I/O here.
    pub(super) fn reconcile_migrated_user(&mut self, document: SettingsDocument, expected_revision: u64) -> bool {
        if self.controller.revision != expected_revision || self.controller.editing_value() || self.path.is_none() {
            return false;
        }
        if !self.controller.reconcile_user_document(document, expected_revision) {
            return false;
        }
        self.invalidate_cache();
        true
    }
    /// Keeps saves off a settings file that startup could not use and left in
    /// place (APP-01): an automatic save would silently replace the user's file.
    pub(super) fn block_user_storage(&mut self) {
        self.storage_blocked = true;
    }
    /// Drop the cached resolution; the next reader rebuilds it once.
    pub fn invalidate_cache(&self) {
        *self.cache.borrow_mut() = None;
    }
    /// Follow the OS light/dark preference as winit reports it (on Windows, the
    /// AppsUseLightTheme value). Applied when the window is created, so a
    /// light-mode desktop gets a light first frame (APP-15); `None` keeps dark.
    pub(super) fn apply_window_theme(&mut self, theme: Option<winit::window::Theme>) {
        let Some(theme) = theme else {
            return;
        };
        self.controller.system.dark = theme == winit::window::Theme::Dark;
        self.invalidate_cache();
    }
    /// Resolve `sequence` against the shared, cached default keymap (ARCH-05).
    /// The keymap is built lazily and only when the command set or the loaded
    /// keymap has changed since the last build, so a burst of keystrokes shares
    /// a single build.
    pub fn resolve_default(
        &self,
        registry: &CommandRegistry,
        sequence: &[KeyChord],
        context: InputContext,
    ) -> KeyResolution {
        self.ensure_default_keymap(registry);
        self.default_keymap
            .borrow()
            .as_ref()
            .map_or(KeyResolution::NoMatch, |keymap| keymap.resolve(sequence, context))
    }
    fn ensure_default_keymap(&self, registry: &CommandRegistry) {
        let key = (registry.entries().count(), self.keymap_revision);
        if self.default_keymap_key.get() == Some(key) && self.default_keymap.borrow().is_some() {
            return;
        }
        *self.default_keymap.borrow_mut() = Some(Keymap::defaults(registry));
        self.default_keymap_key.set(Some(key));
        self.keymap_rebuilds.set(self.keymap_rebuilds.get() + 1);
    }
    /// Number of times the cached default keymap has been rebuilt: one across
    /// any number of keystrokes, plus one more per keymap change. Consumed by
    /// the resolver rebuild test.
    #[cfg(test)]
    pub fn keymap_rebuilds(&self) -> usize {
        self.keymap_rebuilds.get()
    }
    /// Bump the keymap revision so the cached default keymap rebuilds once on
    /// the next resolve. Called whenever the loaded keymap document changes.
    fn bump_keymap_revision(&mut self) {
        self.keymap_revision = self.keymap_revision.wrapping_add(1);
    }
    fn refresh_cache(&self) {
        let (dark, high_contrast) = (self.controller.system.dark, self.controller.system.high_contrast);
        let revision = self.controller.revision;
        if self
            .cache
            .borrow()
            .as_ref()
            .is_some_and(|c| c.revision == revision && c.dark == dark && c.high_contrast == high_contrast)
        {
            return;
        }
        let settings = self.controller.effective();
        let theme = Theme::resolve(settings.theme, self.controller.system, &settings.theme_overrides)
            .unwrap_or_else(|_| Theme::builtin(dark));
        let ui = bareline_ui::theme::UiTheme::from_tokens(|key| theme.color(key).map(|c| (c.rgb, c.alpha)))
            .unwrap_or_default();
        let editor = bareline_ui::theme::EditorTheme::from_tokens(|key| theme.color(key).map(|c| (c.rgb, c.alpha)))
            .unwrap_or_default();
        *self.cache.borrow_mut() = Some(ResolvedCache {
            revision,
            dark,
            high_contrast,
            settings,
            theme,
            ui,
            editor,
        });
    }
    pub fn effective(&self) -> EffectiveSettings {
        self.refresh_cache();
        self.cache
            .borrow()
            .as_ref()
            .map(|c| c.settings.clone())
            .unwrap_or_default()
    }
    pub fn workspace_root(&self) -> Option<&std::path::Path> {
        self.workspace_requested.as_deref()
    }
    pub fn set_workspace_root(&mut self, root: PathBuf) {
        if self.workspace_requested.as_ref() == Some(&root) {
            return;
        }
        self.controller.clear_workspace_document();
        self.workspace_requested = Some(root);
        (self.notify)();
    }
    pub fn ui_theme(&self) -> bareline_ui::theme::UiTheme {
        self.refresh_cache();
        self.cache.borrow().as_ref().map(|c| c.ui).unwrap_or_default()
    }
    pub fn editor_theme(&self) -> bareline_ui::theme::EditorTheme {
        self.refresh_cache();
        self.cache.borrow().as_ref().map(|c| c.editor).unwrap_or_default()
    }
    pub fn theme_color(&self, key: &str) -> Option<bareline_renderer::Color> {
        self.refresh_cache();
        let cache = self.cache.borrow();
        let theme = &cache.as_ref()?.theme;
        let color = theme.color(key)?;
        let background = theme.color("surface.editor")?.rgb;
        let mut rgb = 0;
        for shift in [16, 8, 0] {
            let foreground = (color.rgb >> shift) & 255;
            let behind = (background >> shift) & 255;
            rgb |= ((foreground * color.alpha as u32 + behind * (255 - color.alpha as u32) + 127) / 255) << shift;
        }
        Some(bareline_renderer::Color(rgb))
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = self.controller.poll();
        if let Some((root, result)) = self.workspace_result.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.workspace_result = None;
            if self.workspace_requested.as_ref() == Some(&root) {
                match result {
                    Ok(document) => self
                        .controller
                        .set_workspace_document(root.join(".bareline").join("settings.toml"), document),
                    Err(error) => self.controller.error = Some(error),
                }
                self.workspace_loaded = Some(root);
                changed = true;
            }
        }
        if self.keymap_loaded && self.workspace_result.is_none() && self.workspace_requested != self.workspace_loaded {
            if let Some(root) = self.workspace_requested.clone() {
                let wake = self.notify.clone();
                let (tx, rx) = mpsc::sync_channel(1);
                self.workspace_result = Some(rx);
                if let Err(error) = std::thread::Builder::new()
                    .name("bareline-workspace-settings".into())
                    .spawn(move || {
                        let path = root.join(".bareline").join("settings.toml");
                        let result = match bareline_settings::read_config(&path) {
                            Ok(bytes) => SettingsDocument::parse(&bytes, Scope::Workspace),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                Ok(SettingsDocument::empty(Scope::Workspace))
                            }
                            Err(error) => Err(error.to_string()),
                        };
                        let _ = tx.send((root, result));
                        wake();
                    })
                {
                    self.workspace_result = None;
                    self.controller.error = Some(error.to_string());
                    self.workspace_loaded = self.workspace_requested.clone();
                    changed = true;
                }
            }
        }
        if self.locale_result.is_some() {
            if let Some(result) = self.locale_result.as_ref().and_then(|rx| rx.try_recv().ok()) {
                self.locale_result = None;
                if self.locale_requested == self.effective().locale {
                    match result.and_then(|pack| self.controller.localizer.switch(pack)) {
                        Ok(change) => {
                            self.language_change = Some(change);
                            changed = true;
                        }
                        Err(error) => {
                            self.controller.error = Some(error);
                            changed = true;
                        }
                    }
                }
            }
        }
        if self.keymap_loaded && self.locale_result.is_none() {
            let locale = self.effective().locale;
            if self.locale_requested != locale {
                self.locale_requested = locale.clone();
                if locale == "en" {
                    self.language_change = self
                        .controller
                        .localizer
                        .switch(bareline_settings::LocalePack::english())
                        .ok();
                    changed = true;
                } else if let Some(parent) = self.path.as_ref().and_then(|path| path.parent()) {
                    let path = parent.join("locales").join(format!("{locale}.toml"));
                    let (tx, rx) = mpsc::sync_channel(1);
                    self.locale_result = Some(rx);
                    let wake = self.notify.clone();
                    if let Err(error) =
                        std::thread::Builder::new()
                            .name("bareline-locale-load".into())
                            .spawn(move || {
                                use std::io::Read;
                                let result = (|| {
                                    let mut bytes = Vec::new();
                                    std::fs::File::open(path)
                                        .map_err(|e| e.to_string())?
                                        .take(bareline_settings::MAX_CONFIG_BYTES as u64 + 1)
                                        .read_to_end(&mut bytes)
                                        .map_err(|e| e.to_string())?;
                                    let pack = bareline_settings::LocalePack::parse(&bytes)?;
                                    if pack.locale != locale {
                                        return Err("Locale pack ID does not match the selected language".into());
                                    }
                                    Ok(pack)
                                })();
                                let _ = tx.send(result);
                                wake();
                            })
                    {
                        self.locale_result = None;
                        self.controller.error = Some(error.to_string());
                    }
                } else {
                    self.controller.error = Some("Locale pack folder is unavailable".into());
                    changed = true;
                }
            }
        }
        if let Some(families) = self.fonts_result.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.fonts_result = None;
            // An empty enumeration means DirectWrite failed; keep the last list.
            if !families.is_empty() && self.controller.set_font_families(families) {
                self.fonts_changed = true;
                changed = true;
            }
        }
        if let Some(result) = self.keymap_result.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.keymap_result = None;
            changed = true;
            match result {
                Ok(document) => {
                    self.keymap = document;
                    self.pending.clear();
                    self.preset_requested = None;
                    self.bump_keymap_revision();
                }
                Err(error) => self.controller.error = Some(error),
            }
        }
        if changed {
            self.invalidate_cache();
        }
        changed
    }
    pub fn load_keymap(&mut self, registry: &CommandRegistry) {
        if !self.keymap_loaded {
            self.keymap_loaded = true;
            // Called after the first frame (or on explicit Settings open).
            if self.storage_blocked {
                self.controller.error = Some(
                    "Settings changes are not saved: the settings file could not be read and was left unchanged."
                        .into(),
                );
            } else if let Some(path) = &self.path
                && let Err(error) = self.controller.configure_storage(
                    path.clone(),
                    None,
                    Arc::new(bareline_platform_windows::WindowsFileSystem),
                    self.notify.clone(),
                )
            {
                self.controller.error = Some(error.to_string());
            } else if std::mem::take(&mut self.record_keymap_preset)
                && self.path.is_some()
                && !self.controller.user.document.values().0.contains_key("keyboard.preset")
            {
                self.choose_keymap_preset(KeymapPreset::Bareline);
            }
            self.keymap = KeymapDocument::defaults(registry);
            self.bump_keymap_revision();
            if let Some(path) = self.keymap_path.clone()
                && path.exists()
            {
                self.import_keymap(path, registry, false);
            }
        }
        self.sync_keymap_preset(registry);
    }
    /// Lay the keymap over the `keyboard.preset` choice (BIZ-08) once the keymap
    /// file has loaded and no keymap load or save is running. Shortcuts the
    /// person changed stay on top. The switch is made in memory only: the file
    /// keeps the preset it names and is switched the same way on every load, and
    /// the next shortcut edit saves the switched map, so a keymap file is never
    /// rewritten behind the person's back.
    fn sync_keymap_preset(&mut self, registry: &CommandRegistry) {
        if !self.keymap_loaded || self.keymap_result.is_some() {
            return;
        }
        self.refresh_cache();
        let Some(wanted) = self.cache.borrow().as_ref().map(|cache| cache.settings.keymap_preset) else {
            return;
        };
        if wanted == self.keymap.preset() {
            self.preset_requested = None;
            return;
        }
        if self.preset_requested == Some(wanted) {
            return;
        }
        self.preset_requested = Some(wanted);
        match self.keymap.with_preset(wanted, registry) {
            Ok(document) => {
                self.keymap = document;
                self.pending.clear();
                self.bump_keymap_revision();
            }
            Err(error) => {
                self.controller.error = Some(format!(
                    "The {} shortcuts could not be applied: {error}",
                    wanted.title()
                ))
            }
        }
    }
    /// Save `preset` as the person's shortcut preset; the keymap follows on the
    /// next frame (`sync_keymap_preset`). Returns the line for the status bar.
    pub(super) fn choose_keymap_preset(&mut self, preset: KeymapPreset) -> String {
        let prior = self.controller.scope;
        self.controller.scope = Scope::User;
        let result = self.controller.edit(
            "keyboard.preset",
            bareline_settings::SettingValue::Text(preset.id().into()),
        );
        self.controller.scope = prior;
        // An explicit choice retries a switch that failed before.
        self.preset_requested = None;
        match result {
            Ok(()) => format!(
                "{} shortcuts are in use. Shortcuts you changed yourself are kept.",
                preset.title()
            ),
            Err(error) => format!("The shortcut preset could not be changed: {error}"),
        }
    }
    /// Check the menu's radio item for the preset the keymap is laid out from.
    pub(super) fn annotate_keymap_preset(&self, context: &mut bareline_commands::CommandContext) {
        let active = self.keymap.preset();
        for (id, _, preset) in bareline_settings::KEYMAP_PRESET_COMMANDS {
            context.states.insert(
                bareline_commands::CommandId(id),
                bareline_commands::CommandState {
                    checked: preset == active,
                    radio: true,
                    ..Default::default()
                },
            );
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
                    let document = KeymapDocument::load(&path, &owned_registry).map_err(|e| e.to_string())?;
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
    /// Re-enumerate installed font families off the UI thread (UI-20). The
    /// enumeration checks for updates, so a font installed since the last open
    /// is listed; walking every family can take a noticeable moment on machines
    /// with many fonts, so Settings opens with the last list and `poll` applies
    /// the fresh one when it arrives.
    pub(super) fn refresh_font_families(&mut self) {
        if self.fonts_result.is_some() {
            return;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.fonts_result = Some(rx);
        let wake = self.notify.clone();
        if std::thread::Builder::new()
            .name("bareline-font-enumeration".into())
            .spawn(move || {
                let families: Vec<(String, bool)> = bareline_platform_windows::installed_font_families()
                    .into_iter()
                    .map(|family| (family.name, family.monospace))
                    .collect();
                let _ = tx.send(families);
                wake();
            })
            .is_err()
        {
            // Keep the last list; the next open tries again.
            self.fonts_result = None;
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
    /// Draws the page below the tab strip, which starts at `top` (under the
    /// toolbar when it is shown), so the strip stays visible (UI-05).
    pub fn draw(
        &mut self,
        renderer: &mut WindowsRenderer,
        width: f32,
        height: f32,
        top: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        if !self.controller.open {
            return Ok(None);
        }
        let bounds = bareline_ui::rect(
            0.0,
            top + bareline_ui::TAB_HEIGHT,
            width,
            (height - top - bareline_ui::TAB_HEIGHT - bareline_ui::STATUS_HEIGHT).max(0.0),
        );
        self.controller.draw(bounds, renderer, ops)?;
        Ok(Some(bounds))
    }
}
impl Shell {
    /// First run, with no settings file yet: say once that Notepad++'s
    /// shortcuts are one step away (BIZ-08). The default preset is saved when
    /// settings storage is ready, so the next launch does not repeat this.
    pub(super) fn offer_keymap_preset(&mut self) {
        self.settings.record_keymap_preset = true;
        self.startup_notice(
            "startup:keymap-preset",
            bareline_ui::theme::ToastLevel::Info,
            "Coming from Notepad++? Bareline can use its keyboard shortcuts.".into(),
            "Choose Settings > Import from Notepad++ > Use Notepad++ Shortcuts, or set Shortcut preset on the Keyboard page of Settings. Shortcuts you change yourself are kept when you switch presets.".into(),
        );
    }
    /// The installed font families changed (UI-20): re-resolve families that
    /// fell back while missing and reshape visible text with the new faces.
    pub(super) fn apply_font_refresh(&mut self) {
        if !std::mem::take(&mut self.settings.fonts_changed) {
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.refresh_fonts();
        if let Some(workspace) = self.workspace.as_mut() {
            for editor in &mut workspace.editors {
                editor.release_layouts(renderer);
            }
        }
        if let Some(peer) = self.views.secondary.as_mut() {
            peer.release_layouts(renderer);
        }
    }
    pub(super) fn settings_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        if let Some(preset) = bareline_settings::keymap_preset_command(id) {
            let message = self.settings.choose_keymap_preset(preset);
            if let Some(workspace) = &mut self.workspace {
                workspace.message = Some(message);
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return true;
        }
        match id {
            "settings.open" => {
                // Pages are tabs: showing Settings hides Extensions, which keeps
                // its tab in the strip (UI-05).
                if self.extensions.open {
                    self.extensions.open = false;
                    self.views.park_page(super::views::PageTab::Extensions);
                }
                self.settings.controller.show();
                self.palette.dismiss();
                // Re-enumerate on every open (UI-20), off the UI thread, so a font
                // installed since the last open is listed and no longer reported
                // missing; `apply_font_refresh` runs when the list changes.
                self.settings.refresh_font_families();
                // The toolbar chip editor picks commands by title, so give it the
                // registry's (ID, title) pairs sorted by title.
                let mut catalog: Vec<(String, String)> = self
                    .app
                    .commands
                    .entries()
                    .map(|spec| (spec.id.0.to_string(), spec.title.to_string()))
                    .collect();
                catalog.sort_by(|a, b| a.1.cmp(&b.1));
                self.settings.controller.toolbar_catalog = catalog;
                self.settings.load_keymap(&self.app.commands);
            }
            "settings.close" => self.settings.controller.dismiss(),
            "settings.retry" => self.settings.controller.retry_save(),
            "settings.revert" => {
                if self.settings.controller.revert_changes() {
                    self.settings_effect(el, Some(SettingsEffect::PreviewChanged));
                }
            }
            "settings.external_reload" => {
                if self.settings.controller.reload_external_change() {
                    self.settings_effect(el, Some(SettingsEffect::PreviewChanged));
                }
            }
            "settings.external_keep" => {
                self.settings.controller.keep_after_external_change();
            }
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
                        Ok(Some(path)) => self.settings.import_keymap(path, &self.app.commands, true),
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
                self.settings_effect(el, effect);
            }
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn settings_effect(&mut self, el: &ActiveEventLoop, effect: Option<SettingsEffect>) {
        match effect {
            Some(SettingsEffect::Restart) => self.relaunch(el),
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
                } else if let Some(root) = self.settings.workspace_root() {
                    let path = root.join(".bareline").join("settings.toml");
                    if let Some(workspace) = &mut self.workspace {
                        workspace.open(path);
                        self.settings.controller.dismiss();
                    }
                } else {
                    self.settings.controller.error = Some("Workspace settings path is unavailable".into());
                }
            }
            Some(SettingsEffect::Close) => self.settings.controller.dismiss(),
            Some(SettingsEffect::PreviewChanged) | None => {}
        }
    }
    pub(super) fn settings_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if let WindowEvent::ThemeChanged(theme) = event {
            self.settings.controller.system.high_contrast =
                bareline_platform_windows::high_contrast_enabled().unwrap_or(false);
            self.settings.controller.system.highlight = bareline_platform_windows::high_contrast_highlight();
            // Sets the dark flag and invalidates the resolved theme cache.
            self.settings.apply_window_theme(Some(*theme));
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }
        if matches!(event, WindowEvent::Focused(true)) {
            self.settings.controller.system.high_contrast =
                bareline_platform_windows::high_contrast_enabled().unwrap_or(false);
            self.settings.controller.system.highlight = bareline_platform_windows::high_contrast_highlight();
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
                    effect = self.settings.controller.event(UiEvent::PointerMove(self.pointer));
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                // The settings page starts below the tab strip. A press in the
                // strip row belongs to the tabs (e.g. the Settings tab ×), so let
                // it fall through to the tab-strip handler. The strip sits under
                // the toolbar when that is shown.
                if *state == ElementState::Pressed && self.pointer.y < self.editor_bounds().y + bareline_ui::TAB_HEIGHT
                {
                    return false;
                }
                effect = self.settings.controller.event(if *state == ElementState::Pressed {
                    UiEvent::PointerDown(self.pointer)
                } else {
                    UiEvent::PointerUp(self.pointer)
                });
                if *state == ElementState::Pressed
                    && let Some(renderer) = &self.renderer
                    && let Some(field) = self.settings.controller.text_field_mut()
                {
                    let _ = field.click(renderer, self.pointer, self.modifiers.shift_key());
                }
            }
            WindowEvent::Ime(Ime::Preedit(value, cursor)) => {
                if let Some(field) = self.settings.controller.text_field_mut() {
                    field.preedit(value.clone(), *cursor);
                }
            }
            WindowEvent::Ime(Ime::Commit(value)) => {
                if let Some(field) = self.settings.controller.text_field_mut() {
                    field.commit(value);
                    self.settings.controller.text_changed();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let ctrl = self.modifiers.control_key();
                let shift = self.modifiers.shift_key();
                if let Some(field) = self.settings.controller.text_field_mut()
                    && !field.composing()
                {
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
                                        && let Ok(Some(value)) =
                                            platform.clipboard_text_within(bareline_ui::text_field::LIMIT)
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
                        Key::Unidentified(_) if !ctrl || self.modifiers.alt_key() => {
                            // VK_PACKET has no logical key label. Its decoded
                            // Unicode text still belongs to the focused field.
                            if let Some(value) = &event.text {
                                field.insert(value);
                            }
                        }
                        _ => {}
                    }
                    self.settings.controller.text_changed();
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
                if normalized == Some(UiKey::Tab) {
                    self.settings.controller.traverse_focus(shift);
                } else if let Some(key) = normalized {
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
        self.settings_effect(el, effect);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    /// Relaunch the executable with the same arguments and exit, so a
    /// restart-required setting can be applied without the user hunting for the
    /// process. The save-changes prompt still runs first, so no unsaved work is
    /// lost; nothing happens if the user cancels it.
    fn relaunch(&mut self, el: &ActiveEventLoop) {
        // As for a close, no acknowledged launch may be lost with this process.
        if !self.instance_exit_ready() {
            self.instance_exit_cancelled(el);
            return;
        }
        if !self.confirm_exit() {
            self.instance_resume();
            return;
        }
        let spawned = std::env::current_exe().and_then(|exe| {
            let mut args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
            // Force a fresh process, otherwise it would just forward this launch
            // to the instance that is about to exit and do nothing.
            if !args.iter().any(|arg| arg == "--new-instance") {
                args.push(std::ffi::OsString::from("--new-instance"));
            }
            std::process::Command::new(exe).args(&args).spawn()
        });
        if let Err(error) = spawned {
            if let Some(platform) = &self.platform {
                platform.operation_failed(&format!("Could not restart Bareline: {error}"));
            }
            self.instance_resume();
            return;
        }
        el.exit();
    }
    /// Call before legacy shortcuts, but after focused palette/settings text handling.
    pub(super) fn settings_keymap_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
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
        // AltGr only claims keys that type text; `Keymap::resolve` decides that
        // per chord, so AltGr+arrow still reaches Ctrl+Alt+arrow (WSP-06).
        if context.ime_composing || context.dead_key {
            return false;
        }
        if self.settings.pending_at.elapsed() > Duration::from_secs(2) {
            self.settings.pending.clear();
        }
        let candidates = key_press(self.modifiers, event).candidates();
        let field_layer = self.settings.controller.open
            || self.shortcuts.open
            || self.palette.open
            || self.search_modal()
            || self.language.controller.open
            || self.extensions.open
            || self.power.open
            || self.macros.controller.manager.open
            || self.utilities.has_input_focus()
            || self.workspace.as_ref().is_some_and(|workspace| {
                workspace.find.has_focus() || (workspace.search_panel.open && workspace.search_focus)
            });
        let mut field_keymap = Keymap::default();
        let routed_text_actions = self.palette.open
            || self.search_folder_open()
            || self.workspace.as_ref().is_some_and(|workspace| {
                workspace.find.has_focus() || (workspace.search_panel.open && workspace.search_focus)
            });
        if field_layer {
            let bindings = self
                .settings
                .keymap
                .keymap
                .bindings()
                .iter()
                .filter(|binding| {
                    let action = self.app.commands.dispatch(binding.command);
                    action == Some(Action::Palette)
                        || (routed_text_actions
                            && matches!(
                                action,
                                Some(
                                    Action::SelectAll
                                        | Action::Copy
                                        | Action::Cut
                                        | Action::Paste
                                        | Action::Undo
                                        | Action::Redo
                                )
                            ))
                })
                .cloned()
                .collect();
            let _ = field_keymap.replace(bindings, &self.app.commands);
        }
        let had_pending = !self.settings.pending.is_empty();
        for chord in candidates {
            let mut sequence = self.settings.pending.clone();
            sequence.push(chord.clone());
            let resolution = if field_layer {
                field_keymap.resolve(&sequence, context)
            } else {
                self.settings.keymap.keymap.resolve(&sequence, context)
            };
            match resolution {
                KeyResolution::Command(id) => {
                    self.settings.pending.clear();
                    if !field_layer && !had_pending && self.insert_tab_shortcut(id, &event.logical_key) {
                        return true;
                    }
                    match self.app.commands.dispatch_in(id, &self.command_state_context(id)) {
                        Ok(action) => self.dispatch(el, action),
                        // Surface why a bound key did nothing instead of dropping it.
                        Err(bareline_commands::DispatchError::Disabled(reason)) => {
                            if let Some(workspace) = self.workspace.as_mut() {
                                workspace.message = Some(reason);
                            }
                        }
                        Err(bareline_commands::DispatchError::Unknown(_)) => {}
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
                    self.settings.resolve_default(&self.app.commands, &[chord], context),
                    KeyResolution::Command(_)
                )
            {
                return true;
            }
        }
        self.settings.pending.clear();
        had_pending
    }

    pub(super) fn insert_tab_shortcut(&mut self, id: bareline_commands::CommandId, key: &Key) -> bool {
        if id.0 != "editor.indent"
            || *key != Key::Named(NamedKey::Tab)
            || !self.modifiers.is_empty()
            || self.modal.is_some()
            || self.palette.open
            || self.panels_accessibility_focus().is_some()
            || self.views_accessibility_focus().is_some()
            || self
                .workspace
                .as_ref()
                .is_none_or(|workspace| workspace.find.has_focus() || workspace.search_focus)
        {
            return false;
        }
        let handled = self
            .workspace
            .as_mut()
            .is_some_and(|workspace| self.views.insert_tab_at_carets(workspace, self.app.active));
        if handled && let Some(window) = &self.window {
            window.request_redraw();
        }
        handled
    }
}

impl Shell {
    pub(super) fn settings_text_action(&mut self, action: Action) -> bool {
        if !self.settings.controller.open
            || self.palette.open
            || self.shortcuts.open
            || !matches!(
                action,
                Action::Copy | Action::Cut | Action::Paste | Action::SelectAll | Action::Undo | Action::Redo
            )
        {
            return false;
        }
        if let Some(field) = self.settings.controller.text_field_mut() {
            match action {
                Action::SelectAll => field.select_all(),
                Action::Undo => field.undo(false),
                Action::Redo => field.undo(true),
                Action::Paste => {
                    if let Some(platform) = &self.platform
                        && let Ok(Some(value)) = platform.clipboard_text_within(bareline_ui::text_field::LIMIT)
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
            self.settings.controller.text_changed();
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

#[cfg(test)]
mod keymap_cache_tests {
    use super::*;
    use bareline_commands::shell_commands;

    /// APP-15: a light Windows theme gives a light first frame, and a window
    /// that reports no theme keeps the dark default.
    #[test]
    fn window_theme_sets_the_first_frame_palette() {
        let mut runtime = SettingsRuntime::default();
        runtime.controller.system.high_contrast = false;
        runtime.invalidate_cache();
        let dark = runtime.ui_theme();
        runtime.apply_window_theme(None);
        assert_eq!(runtime.ui_theme().editor, dark.editor);
        runtime.apply_window_theme(Some(winit::window::Theme::Light));
        assert!(!runtime.controller.system.dark);
        assert_ne!(
            runtime.ui_theme().editor,
            dark.editor,
            "light Windows must not start dark"
        );
        runtime.apply_window_theme(Some(winit::window::Theme::Dark));
        assert_eq!(runtime.ui_theme().editor, dark.editor);
    }

    #[test]
    fn default_keymap_is_built_once_per_keymap_change() {
        let mut runtime = SettingsRuntime::default();
        let registry = shell_commands();
        let chord = [KeyChord::parse("Ctrl+N").unwrap()];
        let context = InputContext::default();
        // A burst of keystrokes shares a single build (ARCH-05): the resolver no
        // longer rebuilds `Keymap::defaults` per key press.
        for _ in 0..25 {
            let _ = runtime.resolve_default(&registry, &chord, context);
        }
        assert_eq!(runtime.keymap_rebuilds(), 1);
        // One keymap change invalidates the cache; the next resolve rebuilds
        // exactly once, and further keystrokes reuse that build.
        runtime.bump_keymap_revision();
        for _ in 0..25 {
            let _ = runtime.resolve_default(&registry, &chord, context);
        }
        assert_eq!(runtime.keymap_rebuilds(), 2);
        // Resolution itself is unchanged: Ctrl+N still maps to a command.
        assert!(matches!(
            runtime.resolve_default(&registry, &chord, context),
            KeyResolution::Command(_)
        ));
    }

    /// BIZ-08: choosing a preset lays the loaded keymap over it on the next
    /// sync, keeps the person's own shortcuts, and switching back restores the
    /// Bareline bindings under them.
    #[test]
    fn choosing_a_keymap_preset_switches_the_keymap_and_keeps_user_shortcuts() {
        use bareline_commands::{CommandId, KeyBinding};
        let registry = shell_commands();
        let mut runtime = SettingsRuntime::default();
        runtime.keymap_loaded = true;
        let mut keymap = KeymapDocument::defaults(&registry);
        keymap
            .set_binding(
                KeyBinding {
                    command: CommandId("file.new"),
                    sequence: vec![KeyChord::parse("Ctrl+Alt+N").unwrap()],
                },
                &registry,
            )
            .unwrap();
        runtime.keymap = keymap;
        let label = |runtime: &SettingsRuntime, id| runtime.keymap.keymap.shortcut_label(CommandId(id));

        runtime.choose_keymap_preset(KeymapPreset::NotepadPlusPlus);
        assert_eq!(runtime.effective().keymap_preset, KeymapPreset::NotepadPlusPlus);
        runtime.sync_keymap_preset(&registry);
        assert_eq!(runtime.keymap.preset(), KeymapPreset::NotepadPlusPlus);
        assert_eq!(label(&runtime, "file.new"), "Ctrl+Alt+N");
        assert_eq!(label(&runtime, "file.save_as"), "Ctrl+Alt+S");
        assert!(runtime.keymap.to_toml().contains("preset = \"notepad++\""));

        runtime.choose_keymap_preset(KeymapPreset::Bareline);
        runtime.sync_keymap_preset(&registry);
        assert_eq!(runtime.keymap.preset(), KeymapPreset::Bareline);
        assert_eq!(label(&runtime, "file.new"), "Ctrl+Alt+N");
        assert_eq!(label(&runtime, "file.save_as"), "Ctrl+Shift+S");
    }

    #[test]
    fn migration_receipt_does_not_overwrite_a_newer_live_settings_revision() {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let root = std::env::temp_dir().join(format!(
            "bareline-settings-reconcile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("settings.toml");
        let mut runtime = SettingsRuntime::new(
            SettingsDocument::empty(Scope::User),
            Some(path.clone()),
            Arc::new(|| {}),
        );
        let bootstrap_revision = runtime.controller.revision;
        runtime.controller.revision = runtime.controller.revision.wrapping_add(1);
        std::fs::write(&path, b"[editor]\nfont_size = 17\n").unwrap();
        let migrated = read_migrated_user(&path).unwrap();

        assert!(!runtime.reconcile_migrated_user(migrated, bootstrap_revision));
        assert_eq!(runtime.controller.revision, bootstrap_revision.wrapping_add(1));
        // A damaged migrated file is reported by the worker's read, not applied.
        std::fs::write(&path, b"not = [valid").unwrap();
        assert!(read_migrated_user(&path).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migrated_settings_reconciliation_preserves_the_open_tab() {
        let root = std::env::temp_dir().join(format!(
            "bareline-settings-open-reconcile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.toml");
        std::fs::write(&path, b"[editor]\nfont_size = 17\n").unwrap();
        let migrated = read_migrated_user(&path).unwrap();
        let mut runtime = SettingsRuntime::new(SettingsDocument::empty(Scope::User), Some(path), Arc::new(|| {}));
        runtime.controller.open = true;
        let revision = runtime.controller.revision;

        assert!(runtime.reconcile_migrated_user(migrated, revision));
        assert!(runtime.controller.open);
        assert!(runtime.controller.revision > revision);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn resolved_tab_uses_caret_input_but_other_commands_modifiers_and_fields_do_not() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        super::super::views::accessibility_test_setup(&mut shell, "open");
        let tab = Key::Named(NamedKey::Tab);
        let chord = KeyChord::parse("Tab").unwrap();
        let keymap = Keymap::defaults(&shell.app.commands);
        let KeyResolution::Command(id) = keymap.resolve(&[chord], InputContext::default()) else {
            panic!("Tab must resolve to the editor's effective command");
        };
        assert_eq!(id.0, "editor.indent");
        shell.modifiers = winit::keyboard::ModifiersState::SHIFT;
        assert!(!shell.insert_tab_shortcut(id, &tab));
        shell.modifiers = winit::keyboard::ModifiersState::empty();
        assert!(!shell.insert_tab_shortcut(bareline_commands::CommandId("editor.unindent"), &tab));
        assert!(!shell.insert_tab_shortcut(id, &Key::Named(NamedKey::Enter)));
        shell.workspace.as_mut().unwrap().find.focused = true;
        shell.workspace.as_mut().unwrap().find.open = true;
        assert!(!shell.insert_tab_shortcut(id, &tab));
        shell.workspace.as_mut().unwrap().find.focused = false;
        shell.workspace.as_mut().unwrap().find.open = false;
        assert!(shell.insert_tab_shortcut(id, &tab));
        let workspace = shell.workspace.as_mut().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while workspace.editors[0].busy() {
            assert!(Instant::now() < deadline);
            workspace.pump();
            std::thread::yield_now();
        }
        assert_eq!(
            workspace.editors[0]
                .snapshot()
                .read(bareline_document::TextOffset(0)..bareline_document::TextOffset(1), 1)
                .unwrap(),
            "\t"
        );
    }
}
