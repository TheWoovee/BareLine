// SPDX-License-Identifier: MPL-2.0
//! Settings tab controller. File commits run on one bounded worker; rendering never performs I/O.
use bareline_platform::LocalFileSystem;
use bareline_renderer::{Color, DrawOp, LayoutError, LayoutId, Point, Rect, TextBackend};
use bareline_settings::{
    self as config, EffectiveSettings, SaveStatus, Scope, SettingDefinition, SettingKind, SettingValue,
    SettingsDocument, SettingsEditor, SystemAppearance,
};
use bareline_ui::{
    ViewId,
    controls::{Button, ControlAction, ControlState, Key, UiEvent},
    rect, text,
    text_field::TextField,
    widgets::{ItemSource, List, Metrics, SemanticAction, SemanticRole, Semantics},
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
};
const CATEGORIES: &[&str] = &[
    "Editor",
    "Appearance",
    "Files",
    "Search",
    "Keyboard",
    "Language",
    "Extensions",
    "Advanced",
];
#[derive(Clone, Debug, PartialEq)]
pub enum SettingsEffect {
    CopyKey(String),
    OpenToml(Scope),
    Close,
    PreviewChanged,
    /// The "Restart now" action on the restart notice: the shell relaunches the
    /// executable with the same arguments and exits.
    Restart,
}

/// The theme cards shown at the top of Appearance, replacing the plain
/// `theme.mode` picker row: display label paired with its stored value.
const THEME_CARDS: [(&str, &str); 3] = [("Light", "light"), ("Dark", "dark"), ("System", "system")];
/// Byte-quota and font pickers plus "Custom…" live here so the settings page and
/// its tests agree on what a row offers.
impl SettingsController {
    /// The list a row opens, or `None` when the row has no list and falls back to
    /// typed entry. The last entry is always `Custom…` for open-ended kinds.
    pub fn choice_list(&self, key: &str) -> Option<Vec<(String, SettingValue)>> {
        let definition = config::DEFINITIONS.iter().find(|d| d.key == key)?;
        let mut entries: Vec<(String, SettingValue)> = match definition.kind {
            SettingKind::Boolean => vec![
                (on_off(false), SettingValue::Bool(false)),
                (on_off(true), SettingValue::Bool(true)),
            ],
            SettingKind::Choice(choices) => choices
                .iter()
                .map(|v| (config::display_name(v), SettingValue::Text((*v).into())))
                .collect(),
            SettingKind::Integer(min, max) if max.saturating_sub(min) <= 256 => {
                (min..=max).map(|v| (v.to_string(), SettingValue::Integer(v))).collect()
            }
            SettingKind::Integer(_, _) => Vec::new(),
            SettingKind::Bytes(min, max) => {
                let mut steps = config::byte_steps(min, max);
                if key == "transcode.temp_quota_bytes" && !steps.contains(&21_474_836_480) {
                    steps.push(21_474_836_480);
                    steps.sort_unstable();
                }
                steps
                    .into_iter()
                    .map(|v| (config::format_bytes(v), SettingValue::Integer(v)))
                    .collect()
            }
            SettingKind::Number(min, max) => [
                8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0,
            ]
            .into_iter()
            .filter(|v: &f64| (min..=max).contains(v))
            .map(|v| (format!("{v} pt"), SettingValue::Number(v)))
            .collect(),
            SettingKind::Font => {
                let mut families: Vec<(String, SettingValue)> = self
                    .font_families
                    .iter()
                    .map(|(name, monospace)| {
                        let label = if *monospace {
                            format!("{name}  ·  monospaced")
                        } else {
                            name.clone()
                        };
                        (label, SettingValue::Text(name.clone()))
                    })
                    .collect();
                if families.is_empty() {
                    families = ["Cascadia Mono", "Consolas", "Courier New", "Segoe UI"]
                        .into_iter()
                        .map(|name| (name.to_owned(), SettingValue::Text(name.into())))
                        .collect();
                }
                families
            }
            SettingKind::Text | SettingKind::Strings | SettingKind::Map => return None,
        };
        if matches!(
            definition.kind,
            SettingKind::Integer(_, _) | SettingKind::Number(_, _) | SettingKind::Bytes(_, _)
        ) && entries.is_empty()
        {
            // A range too wide to list still gets typed entry only.
            return None;
        }
        if !matches!(definition.kind, SettingKind::Boolean | SettingKind::Choice(_)) {
            entries.push((CUSTOM_ENTRY.into(), SettingValue::Text(String::new())));
        }
        Some(entries)
    }
    /// True when the saved font family is not among the installed families.
    pub fn missing_font(&self) -> Option<String> {
        let family = self.effective().editor_font_family;
        if self.font_families.is_empty() || self.font_families.iter().any(|(name, _)| *name == family) {
            return None;
        }
        Some(family)
    }
}
struct SaveJob {
    scope: Scope,
    owner_epoch: u64,
    generation: u64,
    snapshot: SettingsDocument,
    path: PathBuf,
    expected: DiskVersion,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum DiskVersion {
    Absent,
    Bytes(Vec<u8>),
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
struct Storage {
    user: PathBuf,
    workspace: Option<PathBuf>,
    sender: SyncSender<SaveJob>,
    receiver: Receiver<SaveCompletion>,
    active: Option<SaveOwner>,
    pending: VecDeque<SaveJob>,
    user_disk: DiskVersion,
    workspace_disk: Option<DiskVersion>,
    user_epoch: u64,
    workspace_epoch: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct SaveOwner {
    scope: Scope,
    owner_epoch: u64,
    path: PathBuf,
}
struct DeferredWorkspace {
    awaited: SaveOwner,
    path: PathBuf,
    requested: SettingsDocument,
}
struct ExternalChange {
    scope: Scope,
    owner_epoch: u64,
    path: PathBuf,
    disk: DiskVersion,
}
fn read_disk_version(path: &std::path::Path) -> std::io::Result<DiskVersion> {
    match config::read_config(path) {
        Ok(bytes) => Ok(DiskVersion::Bytes(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DiskVersion::Absent),
        Err(error) => Err(error),
    }
}
struct Choice {
    key: &'static str,
    labels: Vec<String>,
    values: Vec<SettingValue>,
    list: List,
    /// When set, each row is drawn in the font family it names (falling back to
    /// the UI font if that family cannot be shaped) so the list previews faces.
    font_preview: bool,
}
struct ValueEdit {
    key: &'static str,
    field: TextField,
    bounds: Rect,
    invoker: ViewId,
}
/// Typed editor for the list and key/value settings, so no one has to type raw
/// TOML into a one-line field (UX-54h).
struct CollectionEdit {
    key: &'static str,
    map: bool,
    /// True when the map values are theme colors, so entries paint a swatch and
    /// the field parses `token = #RRGGBB`.
    colors: bool,
    entries: Vec<(String, String)>,
    field: TextField,
    invoker: ViewId,
    bounds: Rect,
    remove: Vec<Rect>,
    add: Rect,
    apply: Rect,
    cancel: Rect,
    error: Option<String>,
    /// (id, title) commands offered by the toolbar picker; empty for other keys.
    catalog: Vec<(String, String)>,
    /// Hit rects for the visible command-picker rows and the catalog index each
    /// one adds; rebuilt every frame in `draw`.
    picker: Vec<(Rect, usize)>,
}
impl CollectionEdit {
    fn value(&self) -> SettingValue {
        if self.map {
            SettingValue::Map(self.entries.iter().cloned().collect())
        } else {
            SettingValue::Strings(self.entries.iter().map(|(k, _)| k.clone()).collect())
        }
    }
    /// True when this editor picks toolbar commands from a catalog of titles.
    fn command_picker(&self) -> bool {
        !self.catalog.is_empty()
    }
    /// Add the catalog command at `index` (a picker click) as an ID/title pair.
    fn add_command(&mut self, index: usize) {
        let Some((id, title)) = self.catalog.get(index).cloned() else {
            return;
        };
        self.entries.retain(|(existing, _)| *existing != id);
        self.entries.push((id, title));
        self.error = None;
        self.field.select_all();
        self.field.insert("");
    }
    fn add_entry(&mut self) {
        let text = self.field.value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        // The toolbar editor only adds real commands, so a typed entry must match
        // a catalog title or ID; picker clicks go through `add_command`.
        if self.command_picker() {
            if let Some(index) = self
                .catalog
                .iter()
                .position(|(id, title)| id.eq_ignore_ascii_case(&text) || title.eq_ignore_ascii_case(&text))
            {
                self.add_command(index);
            } else {
                self.error = Some("Choose a command from the list".into());
            }
            return;
        }
        let entry = if self.map {
            match text.split_once('=') {
                Some((key, value)) => (key.trim().to_owned(), value.trim().to_owned()),
                None => {
                    self.error = Some("Enter the entry as name = value".into());
                    return;
                }
            }
        } else {
            (text, String::new())
        };
        if entry.0.is_empty() {
            self.error = Some("Enter a name".into());
            return;
        }
        // A color map validates the value early so a bad hex never becomes a chip.
        if self.colors
            && let Err(reason) = config::ThemeColor::parse(&entry.1)
        {
            self.error = Some(reason);
            return;
        }
        self.entries.retain(|(key, _)| *key != entry.0);
        self.entries.push(entry);
        self.error = None;
        self.field.select_all();
        self.field.insert("");
    }
}
impl ItemSource for Choice {
    fn len(&self) -> Option<usize> {
        Some(self.labels.len())
    }
    fn discovered(&self) -> usize {
        self.labels.len()
    }
    fn label(&self, index: usize) -> &str {
        self.labels.get(index).map(String::as_str).unwrap_or("")
    }
    fn enabled(&self, _: usize) -> bool {
        true
    }
}
struct Row {
    definition: &'static SettingDefinition,
    value: Button,
    copy: Button,
}
pub struct SettingsController {
    /// Bumped whenever the resolved settings could have changed. The shell keys
    /// its settings/theme cache on this instead of re-resolving every frame.
    pub revision: u64,
    pub open: bool,
    pub scope: Scope,
    pub category: String,
    pub query: TextField,
    pub query_focused: bool,
    pub user: SettingsEditor,
    pub workspace: Option<SettingsEditor>,
    pub workspace_opted_in: bool,
    pub system: SystemAppearance,
    pub error: Option<String>,
    pub reset_pending: bool,
    storage: Option<Storage>,
    bounds: Rect,
    search_bounds: Rect,
    rows: Vec<Row>,
    selected: usize,
    first: usize,
    popup: Option<Choice>,
    value_edit: Option<ValueEdit>,
    collection_edit: Option<CollectionEdit>,
    /// Installed font families, monospaced first, supplied by the shell.
    pub font_families: Vec<(String, bool)>,
    /// (stable ID, title) of every command the toolbar picker may add, supplied
    /// by the shell so chips read as titles and added IDs are always real.
    pub toolbar_catalog: Vec<(String, String)>,
    /// Per-face font-preview layouts shaped last frame, released at the top of
    /// the next `draw` once they have been painted.
    retired_layouts: Vec<LayoutId>,
    /// Hit rects for the Appearance theme cards (Light, Dark, System).
    theme_cards: [Rect; 3],
    /// Set when a restart-required setting was changed in this session.
    pub restart_pending: bool,
    restart_dismiss: Rect,
    restart_now: Rect,
    close_button: Rect,
    open_toml: Rect,
    retired_fields: Vec<TextField>,
    focus: bareline_ui::focus::FocusChain,
    pub localizer: config::Localizer,
    scope_user: Rect,
    scope_workspace: Rect,
    opt_in: Rect,
    reset: Rect,
    retry: Rect,
    revert: Rect,
    external_reload: Rect,
    external_keep: Rect,
    external_change: Option<ExternalChange>,
    deferred_workspace: Option<DeferredWorkspace>,
}
/// Sentinel appended to a picker so a value outside the list can still be typed.
const CUSTOM_ENTRY: &str = "Custom…";
impl SettingsController {
    pub fn label(&self, id: &str, fallback: &str) -> String {
        self.localizer.format(id, &[]).unwrap_or_else(|_| fallback.into())
    }
    pub fn status_description(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        let resolved = config::resolve(
            &self.user.document,
            self.workspace.as_ref().map(|w| &w.document),
            self.workspace_opted_in,
            None,
        );
        if let Some(diagnostic) = resolved.diagnostics.first() {
            return self
                .localizer
                .format(
                    "settings.invalid",
                    &[("key", &diagnostic.key), ("reason", &diagnostic.message)],
                )
                .unwrap_or_else(|_| format!("{}: {}", diagnostic.key, diagnostic.message));
        }
        match &self.current().status {
            SaveStatus::Saved => self.label("settings.saved", "All changes saved"),
            SaveStatus::Pending => self.label("settings.unsaved", "Changes not saved"),
            SaveStatus::Failed(reason) => format!("{}: {reason}", self.label("settings.unsaved", "Changes not saved")),
        }
    }
    fn reset_dialog_bounds(&self) -> Rect {
        let sidebar = 164.0_f32.min(self.bounds.width * 0.26);
        rect(
            self.bounds.x + sidebar + 38.0,
            self.bounds.y + 120.0,
            (self.bounds.width - sidebar - 76.0).max(100.0),
            112.0,
        )
    }
    pub fn new(user: SettingsDocument, workspace: Option<SettingsDocument>, system: SystemAppearance) -> Self {
        let workspace_opted_in = config::resolve(&user, None, false, None)
            .values
            .workspace_preferences_enabled;
        let mut query = TextField::default();
        query.set_placeholder("Search settings");
        Self {
            revision: 1,
            open: false,
            scope: Scope::User,
            category: "Editor".into(),
            query,
            query_focused: true,
            user: SettingsEditor::new(user),
            workspace: workspace.map(SettingsEditor::new),
            workspace_opted_in,
            system,
            error: None,
            reset_pending: false,
            storage: None,
            bounds: Rect::default(),
            search_bounds: Rect::default(),
            rows: Vec::new(),
            selected: 0,
            first: 0,
            popup: None,
            value_edit: None,
            collection_edit: None,
            font_families: Vec::new(),
            toolbar_catalog: Vec::new(),
            retired_layouts: Vec::new(),
            theme_cards: [Rect::default(); 3],
            restart_pending: false,
            restart_dismiss: Rect::default(),
            restart_now: Rect::default(),
            close_button: Rect::default(),
            open_toml: Rect::default(),
            retired_fields: Vec::new(),
            focus: Default::default(),
            localizer: Default::default(),
            scope_user: Rect::default(),
            scope_workspace: Rect::default(),
            opt_in: Rect::default(),
            reset: Rect::default(),
            retry: Rect::default(),
            revert: Rect::default(),
            external_reload: Rect::default(),
            external_keep: Rect::default(),
            external_change: None,
            deferred_workspace: None,
        }
    }
    pub fn configure_storage(
        &mut self,
        user: PathBuf,
        workspace: Option<PathBuf>,
        platform: Arc<dyn LocalFileSystem>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<()> {
        let user_disk = read_disk_version(&user)?;
        let workspace_disk = workspace
            .as_ref()
            .zip(self.workspace.as_ref())
            .map(|(path, _)| read_disk_version(path))
            .transpose()?;
        let (sender, jobs) = mpsc::sync_channel::<SaveJob>(1);
        let (completed, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("bareline-settings-save".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let current = read_disk_version(&job.path);
                    let prepare = if job.scope == Scope::Workspace {
                        job.path.parent().map_or(Ok(()), std::fs::create_dir_all)
                    } else {
                        Ok(())
                    };
                    let error = match current {
                        Ok(current) if current != job.expected => Some(SaveFailure::ExternalChange(current)),
                        Ok(_) => prepare
                            .and_then(|_| job.snapshot.save(&job.path, platform.as_ref()))
                            .err()
                            .map(|error| SaveFailure::Io(error.to_string())),
                        Err(error) => Some(SaveFailure::Io(error.to_string())),
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
        self.storage = Some(Storage {
            user,
            workspace,
            sender,
            receiver,
            active: None,
            pending: VecDeque::new(),
            user_disk,
            workspace_disk,
            user_epoch: 1,
            workspace_epoch: 1,
        });
        Ok(())
    }
    pub fn effective(&self) -> EffectiveSettings {
        config::resolve(
            &self.user.document,
            self.workspace.as_ref().map(|w| &w.document),
            self.workspace_opted_in,
            None,
        )
        .values
    }
    pub fn set_workspace_document(&mut self, path: PathBuf, document: SettingsDocument) {
        let active = self
            .storage
            .as_ref()
            .and_then(|storage| storage.active.as_ref())
            .filter(|owner| owner.scope == Scope::Workspace && owner.path == path)
            .cloned();
        if let Some(awaited) = active {
            self.deferred_workspace = Some(DeferredWorkspace {
                awaited,
                path,
                requested: document,
            });
            self.error = Some("Workspace settings are waiting for the current save to finish".into());
            return;
        }
        self.deferred_workspace = None;
        self.error = None;
        self.install_workspace_document(path, document);
    }
    fn install_workspace_document(&mut self, path: PathBuf, document: SettingsDocument) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(storage) = &mut self.storage {
            storage.workspace_epoch = storage.workspace_epoch.wrapping_add(1);
            storage.workspace_disk =
                Some(read_disk_version(&path).unwrap_or_else(|_| DiskVersion::Bytes(document.to_toml().into_bytes())));
            storage.workspace = Some(path);
            storage.pending.retain(|job| job.scope != Scope::Workspace);
        }
        if self
            .external_change
            .as_ref()
            .is_some_and(|change| change.scope == Scope::Workspace)
        {
            self.external_change = None;
        }
        let mut editor = SettingsEditor::new(document);
        if self.open {
            editor.begin_session();
        }
        self.workspace = Some(editor);
        self.popup = None;
        if let Some(edit) = self.value_edit.take() {
            self.retired_fields.push(edit.field);
            self.focus.close_layer();
        }
    }
    fn apply_deferred_workspace(&mut self, owner: &SaveOwner, settled: SettingsDocument) -> bool {
        let matches = self.deferred_workspace.as_ref().is_some_and(|deferred| {
            deferred.awaited == *owner
                && !self.storage.as_ref().is_some_and(|storage| {
                    storage.pending.iter().any(|job| {
                        job.scope == owner.scope && job.owner_epoch == owner.owner_epoch && job.path == owner.path
                    })
                })
        });
        if !matches {
            return false;
        }
        let deferred = self.deferred_workspace.take().unwrap();
        debug_assert_eq!(deferred.path, owner.path);
        let _requested = deferred.requested;
        self.error = None;
        self.install_workspace_document(deferred.path, settled);
        true
    }
    /// Install a newly available user document without replacing the live tab,
    /// its search/navigation state, workspace settings, or storage worker.
    pub fn reconcile_user_document(&mut self, document: SettingsDocument, expected_revision: u64) -> bool {
        if self.revision != expected_revision || self.editing_value() {
            return false;
        }
        if self.storage.as_ref().is_some_and(|storage| {
            storage.active.as_ref().is_some_and(|owner| owner.scope == Scope::User)
                || storage.pending.iter().any(|job| job.scope == Scope::User)
        }) {
            return false;
        }
        self.revision = self.revision.wrapping_add(1);
        self.workspace_opted_in = config::resolve(&document, None, false, None)
            .values
            .workspace_preferences_enabled;
        if let Some(storage) = &mut self.storage {
            storage.user_epoch = storage.user_epoch.wrapping_add(1);
            storage.user_disk = read_disk_version(&storage.user)
                .unwrap_or_else(|_| DiskVersion::Bytes(document.to_toml().into_bytes()));
        }
        if self
            .external_change
            .as_ref()
            .is_some_and(|change| change.scope == Scope::User)
        {
            self.external_change = None;
        }
        let mut editor = SettingsEditor::new(document);
        if self.open {
            editor.begin_session();
        }
        self.user = editor;
        self.popup = None;
        if let Some(edit) = self.value_edit.take() {
            self.retired_fields.push(edit.field);
            self.focus.close_layer();
        }
        true
    }
    pub fn clear_workspace_document(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.workspace = None;
        self.scope = Scope::User;
        self.popup = None;
        self.deferred_workspace = None;
        self.error = None;
        if let Some(storage) = &mut self.storage {
            storage.workspace_epoch = storage.workspace_epoch.wrapping_add(1);
            storage.workspace = None;
            storage.workspace_disk = None;
            storage.pending.retain(|job| job.scope != Scope::Workspace);
        }
        if self
            .external_change
            .as_ref()
            .is_some_and(|change| change.scope == Scope::Workspace)
        {
            self.external_change = None;
        }
        if let Some(edit) = self.value_edit.take() {
            self.retired_fields.push(edit.field);
            self.focus.close_layer();
        }
    }
    pub fn set_workspace_opt_in(&mut self, enabled: bool) -> Result<(), String> {
        let scope = self.scope;
        self.scope = Scope::User;
        let result = self.edit("workspace.preferences_enabled", SettingValue::Bool(enabled));
        self.scope = scope;
        if result.is_ok() {
            self.workspace_opted_in = enabled;
        }
        result
    }
    fn current(&self) -> &SettingsEditor {
        if self.scope == Scope::Workspace {
            self.workspace.as_ref().unwrap_or(&self.user)
        } else {
            &self.user
        }
    }
    fn current_mut(&mut self) -> &mut SettingsEditor {
        if self.scope == Scope::Workspace {
            self.workspace.as_mut().unwrap_or(&mut self.user)
        } else {
            &mut self.user
        }
    }
    fn editor_for_scope(&self, scope: Scope) -> Option<&SettingsEditor> {
        match scope {
            Scope::User => Some(&self.user),
            Scope::Workspace => self.workspace.as_ref(),
            Scope::Session => None,
        }
    }
    fn editor_for_scope_mut(&mut self, scope: Scope) -> Option<&mut SettingsEditor> {
        match scope {
            Scope::User => Some(&mut self.user),
            Scope::Workspace => self.workspace.as_mut(),
            Scope::Session => None,
        }
    }
    pub fn saving(&self) -> bool {
        self.storage
            .as_ref()
            .is_some_and(|storage| storage.active.is_some() || !storage.pending.is_empty())
    }
    pub fn edit(&mut self, key: &str, value: SettingValue) -> Result<(), String> {
        if self.scope == Scope::Workspace && self.workspace.is_none() {
            return Err("No workspace is open".into());
        }
        if self.scope == Scope::Workspace && self.deferred_workspace.is_some() {
            return Err("Wait for the current workspace settings save to finish".into());
        }
        let previous = self.current().clone();
        self.current_mut().set(key, value)?;
        let effective = self.effective();
        if let Err(error) = config::Theme::resolve(effective.theme, self.system, &effective.theme_overrides) {
            *self.current_mut() = previous;
            self.error = Some(error.clone());
            return Err(error);
        }
        self.error = None;
        self.revision = self.revision.wrapping_add(1);
        if config::DEFINITIONS.iter().any(|d| d.key == key && d.restart_required) {
            self.restart_pending = true;
        }
        self.queue_save();
        Ok(())
    }
    pub fn set_query(&mut self, value: &str) -> bool {
        self.query.select_all();
        let changed = self.query.insert(value);
        if changed {
            self.first = 0;
            self.selected = 0;
            self.popup = None;
        }
        changed
    }
    pub fn insert(&mut self, value: &str) -> bool {
        if !self.query_focused {
            return false;
        }
        let changed = self.query.insert(value);
        if changed {
            self.first = 0;
            self.selected = 0;
            self.popup = None;
        }
        changed
    }
    pub fn query_changed(&mut self) {
        self.first = 0;
        self.selected = 0;
        self.popup = None;
    }
    pub fn show(&mut self) {
        if !self.open {
            self.user.begin_session();
            if let Some(workspace) = &mut self.workspace {
                workspace.begin_session();
            }
        }
        self.open = true;
        self.query_focused = true;
    }
    pub fn dismiss(&mut self) {
        self.confirm_reset(false);
        self.open = false;
        self.popup = None;
        if let Some(edit) = self.value_edit.take() {
            self.retired_fields.push(edit.field);
            self.focus.close_layer();
        }
        self.query.cancel();
        self.user.end_session();
        if let Some(workspace) = &mut self.workspace {
            workspace.end_session();
        }
    }
    pub fn can_revert(&self) -> bool {
        self.current().changed_from_opening()
    }
    pub fn has_external_change(&self) -> bool {
        self.external_change.is_some()
    }
    pub fn revert_changes(&mut self) -> bool {
        if !self.can_revert() {
            return false;
        }
        let scope = self.scope;
        self.revision = self.revision.wrapping_add(1);
        if !self.current_mut().revert() {
            return false;
        }
        self.error = None;
        self.popup = None;
        if scope == Scope::User {
            self.workspace_opted_in = config::resolve(&self.user.document, None, false, None)
                .values
                .workspace_preferences_enabled;
        }
        self.queue_save();
        true
    }
    pub fn request_reset(&mut self) {
        if self.reset_pending {
            return;
        }
        self.reset_pending = true;
        self.focus.open_layer(
            ViewId(8003),
            [8011, 8012]
                .into_iter()
                .map(|id| bareline_ui::focus::FocusTarget {
                    id: ViewId(id),
                    enabled: true,
                })
                .collect(),
        );
    }
    pub fn confirm_reset(&mut self, confirmed: bool) {
        self.revision = self.revision.wrapping_add(1);
        if !self.reset_pending {
            return;
        }
        if self.reset_pending && confirmed {
            let category = self.category.clone();
            self.current_mut().reset_section(&category);
            self.queue_save();
        }
        self.reset_pending = false;
        self.focus.close_layer();
    }
    pub fn retry_save(&mut self) {
        self.queue_save();
    }
    fn queue_save(&mut self) {
        self.queue_scope(self.scope);
    }
    fn queue_scope(&mut self, scope: Scope) {
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
                    match storage.sender.try_send(job) {
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
    fn owns_external_change(&self, change: &ExternalChange) -> bool {
        self.storage.as_ref().is_some_and(|storage| {
            if change.scope == Scope::Workspace {
                storage.workspace.as_ref() == Some(&change.path) && storage.workspace_epoch == change.owner_epoch
            } else {
                storage.user == change.path && storage.user_epoch == change.owner_epoch
            }
        }) && self.editor_for_scope(change.scope).is_some()
    }
    pub fn reload_external_change(&mut self) -> bool {
        let Some(change) = self.external_change.take() else {
            return false;
        };
        if !self.owns_external_change(&change) {
            self.error = Some("The settings file awaiting a decision is no longer open".into());
            self.start_save();
            return false;
        }
        let document = match &change.disk {
            DiskVersion::Absent => SettingsDocument::empty(change.scope),
            DiskVersion::Bytes(bytes) => match SettingsDocument::parse(bytes, change.scope) {
                Ok(document) => document,
                Err(error) => {
                    self.error = Some(format!("Cannot reload changed settings: {error}"));
                    self.external_change = Some(change);
                    return false;
                }
            },
        };
        let settled = document.clone();
        let Some(editor) = self.editor_for_scope_mut(change.scope) else {
            self.error = Some("The changed settings scope is no longer available".into());
            return false;
        };
        editor.replace_from_disk(document);
        if let Some(storage) = &mut self.storage {
            storage.pending.retain(|job| job.scope != change.scope);
            if change.scope == Scope::Workspace {
                storage.workspace_disk = Some(change.disk);
            } else {
                storage.user_disk = change.disk;
            }
        }
        if change.scope == Scope::User {
            self.workspace_opted_in = config::resolve(&self.user.document, None, false, None)
                .values
                .workspace_preferences_enabled;
        }
        if change.scope == Scope::Workspace {
            self.apply_deferred_workspace(
                &SaveOwner {
                    scope: change.scope,
                    owner_epoch: change.owner_epoch,
                    path: change.path.clone(),
                },
                settled,
            );
        }
        self.revision = self.revision.wrapping_add(1);
        self.error = None;
        self.start_save();
        true
    }
    pub fn keep_after_external_change(&mut self) -> bool {
        let Some(change) = self.external_change.take() else {
            return false;
        };
        if !self.owns_external_change(&change) {
            self.error = Some("The settings file awaiting a decision is no longer open".into());
            self.start_save();
            return false;
        }
        if let Some(storage) = &mut self.storage {
            storage.pending.retain(|job| job.scope != change.scope);
            if change.scope == Scope::Workspace {
                storage.workspace_disk = Some(change.disk);
            } else {
                storage.user_disk = change.disk;
            }
        }
        self.error = None;
        self.queue_scope(change.scope);
        true
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
    pub fn release(&mut self, backend: &mut impl TextBackend) {
        self.query.release(backend);
        if let Some(edit) = &mut self.value_edit {
            edit.field.release(backend);
        }
        for mut field in self.retired_fields.drain(..) {
            field.release(backend);
        }
    }
    pub fn text_field_mut(&mut self) -> Option<&mut TextField> {
        if let Some(edit) = &mut self.value_edit {
            return (self.focus.focused() == Some(ViewId(8007))).then_some(&mut edit.field);
        }
        self.query_focused.then_some(&mut self.query)
    }
    pub fn text_changed(&mut self) {
        if let Some(edit) = &mut self.value_edit {
            edit.field.set_validation(None);
        } else {
            self.query_changed();
        }
    }
    /// Replaces the committed value of an exposed text field without applying a setting.
    pub fn accessibility_set_value(&mut self, id: u64, value: &str) -> bool {
        if !self.open || !matches!(id, 8000 | 8007) || value.len() > 16 * 1024 || value.chars().any(char::is_control) {
            return false;
        }
        if !self.semantics().iter().any(|node| node.id.0 == id && !node.disabled) {
            return false;
        }
        self.accessibility_action(id, false);
        let Some(field) = self.text_field_mut() else {
            return false;
        };
        field.cancel();
        field.select_all();
        field.insert(value);
        self.text_changed();
        true
    }
    pub fn editing_value(&self) -> bool {
        self.value_edit.is_some()
    }
    pub fn focused_id(&self) -> Option<ViewId> {
        if !self.open {
            return None;
        }
        if self.reset_pending {
            return self.focus.focused();
        }
        if self.value_edit.is_some() {
            return self.focus.focused();
        }
        if let Some(popup) = &self.popup {
            return popup.list.selected.map(|i| ViewId(8500 + i as u64));
        }
        if self.query_focused {
            Some(ViewId(8000))
        } else {
            self.focus.focused()
        }
    }
    pub fn traverse_focus(&mut self, backwards: bool) {
        if self.value_edit.is_some() {
            self.focus.traverse(backwards);
            return;
        }
        if self.reset_pending {
            self.focus.traverse(backwards);
            return;
        }
        if self.popup.is_some() {
            self.popup = None;
        }
        self.refresh_focus();
        if let Some(id) = self.focus.traverse(backwards) {
            self.accessibility_action(id.0, false);
        }
    }
    fn refresh_focus(&mut self) {
        let mut nodes = self.semantics();
        nodes.sort_by_key(|node| match node.id.0 {
            8000 => (0, 0),
            8001 | 8002 | 8006 => (1, node.id.0),
            8100..=8199 => (2, node.id.0),
            2000..=3999 => (3, node.id.0),
            _ => (4, node.id.0),
        });
        let targets = nodes
            .into_iter()
            .filter(|node| node.actions.contains(&SemanticAction::Focus))
            .map(|node| bareline_ui::focus::FocusTarget {
                id: node.id,
                enabled: !node.disabled,
            })
            .collect();
        self.focus.set_targets(targets);
        if self.query_focused {
            self.focus.focus(ViewId(8000));
        }
    }
    fn begin_value_edit(&mut self, index: usize) -> Option<SettingsEffect> {
        let row = self.rows.get(index)?;
        let value = self.effective().setting_value(row.definition.key)?;
        let input = config::format_setting_input(&value);
        if input.len() > 16 * 1024 {
            return Some(SettingsEffect::OpenToml(self.scope));
        }
        let mut field = TextField::default();
        field.insert(&input);
        field.select_all();
        field.set_placeholder("Enter value · Enter applies · Escape cancels");
        self.value_edit = Some(ValueEdit {
            key: row.definition.key,
            field,
            bounds: row.value.bounds,
            invoker: row.value.id,
        });
        self.focus.open_layer(
            row.value.id,
            [8007, 8009, 8010]
                .into_iter()
                .map(|id| bareline_ui::focus::FocusTarget {
                    id: ViewId(id),
                    enabled: true,
                })
                .collect(),
        );
        self.query_focused = false;
        None
    }
    fn finish_value_edit(&mut self, commit: bool) -> Option<SettingsEffect> {
        let edit = self.value_edit.as_ref()?;
        if edit.field.composing() {
            return None;
        }
        let invoker = edit.invoker;
        let key = edit.key;
        if commit {
            let parsed = config::parse_setting_input(key, edit.field.value());
            let result = parsed.and_then(|value| self.edit(key, value));
            if let Err(reason) = result {
                self.value_edit.as_mut().unwrap().field.set_validation(Some(reason));
                return None;
            }
        }
        if let Some(edit) = self.value_edit.take() {
            self.retired_fields.push(edit.field);
        }
        self.focus.close_layer();
        self.focus.focus(invoker);
        Some(SettingsEffect::PreviewChanged)
    }
    fn definitions(&self) -> Vec<&'static SettingDefinition> {
        let query = self.query.value().to_lowercase();
        let mut definitions: Vec<_> = config::DEFINITIONS
            .iter()
            .filter(|definition| {
                query.is_empty()
                    || format!(
                        "{} {} {}",
                        definition.key,
                        self.label(&format!("setting.{}.title", definition.key), definition.title),
                        self.label(definition.description_id, definition.description)
                    )
                    .to_lowercase()
                    .contains(&query)
            })
            .filter(|definition| !self.query.value().is_empty() || definition.category == self.category)
            // A hidden setting still resolves; it is simply not worth a row yet.
            .filter(|definition| !config::is_hidden(definition.key))
            // Workspace scope lists only what a workspace may set, so switching
            // scope can never produce "controlled by User scope" on click.
            .filter(|definition| self.scope != Scope::Workspace || definition.workspace_allowed)
            // The color mode is shown as theme cards at the top of Appearance, so
            // it no longer needs its own picker row there.
            .filter(|definition| {
                definition.key != "theme.mode" || !(self.query.value().is_empty() && self.category == "Appearance")
            })
            .collect();
        if self.query.value().is_empty() && self.category == "Editor" {
            let order = [
                "editor.font.family",
                "editor.font.size",
                "editor.line_numbers",
                "editor.wrap.mode",
                "editor.render.whitespace",
                "editor.tab.width",
                "editor.insert_spaces",
                "editor.auto_indent",
                "editor.auto_close_brackets",
                "editor.currentLine.highlight",
                "editor.caret.style",
                "editor.scroll_beyond_last_line",
                "editor.minimap",
            ];
            definitions.sort_by_key(|definition| {
                order
                    .iter()
                    .position(|key| *key == definition.key)
                    .unwrap_or(order.len())
            });
        }
        definitions
    }
    fn value_text(&self, key: &str) -> String {
        let effective = self.effective();
        let kind = config::DEFINITIONS.iter().find(|d| d.key == key).map(|d| d.kind);
        let Some(value) = effective.setting_value(key) else {
            return String::new();
        };
        match (kind, &value) {
            (Some(SettingKind::Bytes(_, _)), SettingValue::Integer(bytes)) => config::format_bytes(*bytes),
            (Some(SettingKind::Number(_, _)), SettingValue::Number(size)) => format!("{size} pt"),
            (_, SettingValue::Bool(on)) => on_off(*on),
            (Some(SettingKind::Choice(_)), SettingValue::Text(text)) => config::display_name(text),
            (_, SettingValue::Strings(values)) => match values.len() {
                0 => "None".into(),
                1 => values[0].clone(),
                count => format!("{} · {count} items", values[0]),
            },
            (_, SettingValue::Map(values)) => match values.len() {
                0 => "None".into(),
                count => format!("{count} entries"),
            },
            _ => config::format_setting_input(&value),
        }
    }
    /// Open the typed editor for a list or key/value setting.
    fn begin_collection_edit(&mut self, index: usize) -> Option<SettingsEffect> {
        let row = self.rows.get(index)?;
        let (key, invoker) = (row.definition.key, row.value.id);
        self.start_collection_edit(key, invoker)
    }
    /// Set up the list/key-value editor for `key`. Split out from
    /// `begin_collection_edit` so it does not depend on a drawn row.
    fn start_collection_edit(&mut self, key: &'static str, invoker: ViewId) -> Option<SettingsEffect> {
        let value = self.effective().setting_value(key)?;
        let (map, entries) = match value {
            SettingValue::Strings(values) => (false, values.into_iter().map(|v| (v, String::new())).collect()),
            SettingValue::Map(values) => (true, values.into_iter().collect::<Vec<_>>()),
            _ => return None,
        };
        let colors = config::is_color_map(key);
        // The toolbar editor is a command picker: chips read as titles and the
        // catalog supplies the pickable commands.
        let catalog = if key == "toolbar.commands" {
            self.toolbar_catalog.clone()
        } else {
            Vec::new()
        };
        let mut entries = entries;
        if key == "toolbar.commands" {
            // Store the command title alongside each ID so chips read as titles.
            for entry in &mut entries {
                if let Some((_, title)) = catalog.iter().find(|(id, _)| *id == entry.0) {
                    entry.1 = title.clone();
                }
            }
        }
        let mut field = TextField::default();
        field.set_placeholder(if !catalog.is_empty() {
            "Type to find a command, then pick it"
        } else if colors {
            "token = #RRGGBB, then Add"
        } else if map {
            "name = value, then Add"
        } else {
            "Pattern, for example target/**"
        });
        self.collection_edit = Some(CollectionEdit {
            key,
            map,
            colors,
            entries,
            field,
            invoker,
            bounds: Rect::default(),
            remove: Vec::new(),
            add: Rect::default(),
            apply: Rect::default(),
            cancel: Rect::default(),
            error: None,
            catalog,
            picker: Vec::new(),
        });
        self.query_focused = false;
        None
    }
    fn finish_collection_edit(&mut self, commit: bool) -> Option<SettingsEffect> {
        let edit = self.collection_edit.as_ref()?;
        let invoker = edit.invoker;
        if commit {
            let (key, value) = (edit.key, edit.value());
            if let Err(reason) = self.edit(key, value) {
                if let Some(edit) = self.collection_edit.as_mut() {
                    edit.error = Some(reason);
                }
                return None;
            }
        }
        if let Some(edit) = self.collection_edit.take() {
            self.retired_fields.push(edit.field);
        }
        self.focus.focus(invoker);
        Some(SettingsEffect::PreviewChanged)
    }
    fn choose(&mut self, index: usize) -> Option<SettingsEffect> {
        let row = self.rows.get(index)?;
        let definition = row.definition;
        if self.scope == Scope::Workspace && !definition.workspace_allowed {
            self.error = Some("This setting is controlled by User scope".into());
            return None;
        }
        // A boolean is a switch, not a two-item list.
        if matches!(definition.kind, SettingKind::Boolean) {
            let key = definition.key;
            let current = matches!(self.effective().setting_value(key), Some(SettingValue::Bool(true)));
            if let Err(error) = self.edit(key, SettingValue::Bool(!current)) {
                self.error = Some(error);
            }
            return Some(SettingsEffect::PreviewChanged);
        }
        if matches!(definition.kind, SettingKind::Strings | SettingKind::Map) {
            return self.begin_collection_edit(index);
        }
        // The list is consulted first; typed entry is the fallback, never the
        // shortcut that hides every picker (UX-54c).
        let Some(entries) = self.choice_list(definition.key) else {
            return self.begin_value_edit(index);
        };
        let (labels, values): (Vec<String>, Vec<SettingValue>) = entries.into_iter().unzip();
        let height = (labels.len().min(8) as f32) * 28.0;
        let bounds = self.rows.get(index)?.value.bounds;
        let y = (bounds.y + bounds.height)
            .min(self.bounds.y + self.bounds.height - height - 44.0)
            .max(self.bounds.y);
        self.popup = Some(Choice {
            key: definition.key,
            values,
            labels,
            list: List {
                bounds: rect(bounds.x, y, bounds.width.max(220.0), height),
                state: ControlState {
                    focused: true,
                    ..Default::default()
                },
                selected: Some(0),
                offset: 0.0,
                metrics: Metrics::COMPACT,
            },
            font_preview: definition.key == "editor.font.family",
        });
        None
    }
    /// Apply a popup selection. `Custom…` hands the row to the text editor.
    fn commit_choice(&mut self, key: &'static str, index: usize) -> Option<SettingsEffect> {
        let popup = self.popup.take()?;
        if popup.labels.get(index).map(String::as_str) == Some(CUSTOM_ENTRY) {
            let row = self.rows.iter().position(|r| r.definition.key == key)?;
            return self.begin_value_edit(row);
        }
        let value = popup.values.get(index)?.clone();
        if let Err(error) = self.edit(key, value) {
            self.error = Some(error);
        }
        Some(SettingsEffect::PreviewChanged)
    }
    /// The stored `theme.mode` value ("system", "light" or "dark").
    fn theme_mode_value(&self) -> String {
        match self.effective().setting_value("theme.mode") {
            Some(SettingValue::Text(mode)) => mode,
            _ => "system".into(),
        }
    }
    /// Apply the theme card at `index` (Light, Dark, System), mapping the card to
    /// its `theme.mode` value.
    fn select_theme_card(&mut self, index: usize) -> Option<SettingsEffect> {
        let (_, value) = THEME_CARDS.get(index)?;
        if let Err(error) = self.edit("theme.mode", SettingValue::Text((*value).into())) {
            self.error = Some(error);
        }
        Some(SettingsEffect::PreviewChanged)
    }
    /// True while the Appearance theme cards stand in for the `theme.mode` row.
    fn theme_cards_visible(&self) -> bool {
        self.query.value().is_empty() && self.category == "Appearance"
    }
    pub fn event(&mut self, event: UiEvent) -> Option<SettingsEffect> {
        if self.collection_edit.is_some() {
            match event {
                UiEvent::Key(Key::Escape) => return self.finish_collection_edit(false),
                UiEvent::Key(Key::Enter) => {
                    self.collection_edit.as_mut().unwrap().add_entry();
                    return None;
                }
                UiEvent::PointerDown(point) => {
                    let edit = self.collection_edit.as_ref().unwrap();
                    if let Some(&(_, catalog_index)) = edit.picker.iter().find(|(r, _)| r.contains(point)) {
                        self.collection_edit.as_mut().unwrap().add_command(catalog_index);
                        return None;
                    }
                    if let Some(index) = edit.remove.iter().position(|r| r.contains(point)) {
                        let edit = self.collection_edit.as_mut().unwrap();
                        if index < edit.entries.len() {
                            edit.entries.remove(index);
                            edit.error = None;
                        }
                        return None;
                    }
                    if edit.add.contains(point) {
                        self.collection_edit.as_mut().unwrap().add_entry();
                        return None;
                    }
                    if edit.apply.contains(point) {
                        return self.finish_collection_edit(true);
                    }
                    if edit.cancel.contains(point) || !edit.bounds.contains(point) {
                        return self.finish_collection_edit(false);
                    }
                    return None;
                }
                _ => return None,
            }
        }
        if self.value_edit.is_some() {
            return match event {
                UiEvent::Key(Key::Tab) => {
                    self.traverse_focus(false);
                    None
                }
                UiEvent::Key(Key::Enter | Key::Space) if self.focus.focused() == Some(ViewId(8010)) => {
                    self.finish_value_edit(false)
                }
                UiEvent::Key(Key::Enter) => self.finish_value_edit(true),
                UiEvent::Key(Key::Space) if self.focus.focused() == Some(ViewId(8009)) => self.finish_value_edit(true),
                UiEvent::Key(Key::Escape) => {
                    if self.value_edit.as_ref().unwrap().field.composing() {
                        self.value_edit.as_mut().unwrap().field.cancel();
                        None
                    } else {
                        self.finish_value_edit(false)
                    }
                }
                UiEvent::Focus(false) => {
                    self.value_edit.as_mut().unwrap().field.cancel();
                    None
                }
                UiEvent::PointerDown(point) => {
                    let bounds = self.value_edit.as_ref().unwrap().bounds;
                    if bounds.contains(point) {
                        self.focus.focus(ViewId(8007));
                    } else if rect(bounds.x, bounds.y + bounds.height + 24.0, 72.0, 28.0).contains(point) {
                        return self.finish_value_edit(true);
                    } else if rect(bounds.x + 80.0, bounds.y + bounds.height + 24.0, 72.0, 28.0).contains(point) {
                        return self.finish_value_edit(false);
                    }
                    None
                }
                _ => None,
            };
        }
        if self.reset_pending {
            match event {
                UiEvent::Key(Key::Enter) => {
                    self.confirm_reset(self.focus.focused() != Some(ViewId(8012)));
                    return Some(SettingsEffect::PreviewChanged);
                }
                UiEvent::Key(Key::Escape) => self.confirm_reset(false),
                UiEvent::Key(Key::Tab) => self.traverse_focus(false),
                UiEvent::PointerDown(point) => {
                    if let Some(node) = self
                        .semantics()
                        .into_iter()
                        .find(|node| node.actions.contains(&SemanticAction::Invoke) && node.bounds.contains(point))
                    {
                        self.confirm_reset(node.id == ViewId(8011));
                        return Some(SettingsEffect::PreviewChanged);
                    }
                }
                _ => {}
            }
            return None;
        }
        if let Some(mut popup) = self.popup.take() {
            if matches!(event, UiEvent::Key(Key::Escape)) {
                return None;
            }
            let source = Labels(&popup.labels);
            let key = popup.key;
            let action = popup.list.event(event, &source);
            let chosen = match action {
                Some(ControlAction::Selected(index))
                    if matches!(
                        event,
                        UiEvent::PointerUp(_) | UiEvent::Key(Key::Enter) | UiEvent::Key(Key::Space)
                    ) =>
                {
                    Some(index)
                }
                _ if matches!(event, UiEvent::Key(Key::Enter) | UiEvent::Key(Key::Space))
                    || matches!(event, UiEvent::PointerUp(point) if popup.list.bounds.contains(point)) =>
                {
                    popup.list.selected
                }
                _ => None,
            };
            if let Some(index) = chosen {
                self.popup = Some(popup);
                return self.commit_choice(key, index);
            }
            if let UiEvent::PointerDown(point) = event {
                if !popup.list.bounds.contains(point) {
                    return None;
                }
            }
            self.popup = Some(popup);
            return None;
        }
        if let UiEvent::PointerDown(point) = event {
            if let Some(node) = self
                .semantics()
                .into_iter()
                .rev()
                .find(|node| !node.disabled && node.bounds.contains(point))
            {
                self.focus.focus(node.id);
            }
        }
        match event {
            UiEvent::PointerDown(point) => {
                self.query_focused = self.search_bounds.contains(point);
                if self.close_button.contains(point) {
                    self.dismiss();
                    return Some(SettingsEffect::Close);
                }
                if self.open_toml.contains(point) {
                    return Some(SettingsEffect::OpenToml(self.scope));
                }
                if self.restart_pending && self.restart_dismiss.contains(point) {
                    self.restart_pending = false;
                    return None;
                }
                if self.restart_pending && self.restart_now.contains(point) {
                    self.restart_pending = false;
                    return Some(SettingsEffect::Restart);
                }
                if self.theme_cards_visible()
                    && let Some(index) = self.theme_cards.iter().position(|card| card.contains(point))
                {
                    return self.select_theme_card(index);
                }
                if self.scope_user.contains(point) {
                    self.scope = Scope::User;
                    self.error = None;
                    return None;
                }
                if self.scope_workspace.contains(point) {
                    if self.workspace.is_some() {
                        self.scope = Scope::Workspace;
                        self.error = None;
                    } else {
                        self.error = Some("No workspace is open".into());
                    }
                    return None;
                }
                if self.opt_in.contains(point) && self.scope == Scope::Workspace {
                    if let Err(error) = self.set_workspace_opt_in(!self.workspace_opted_in) {
                        self.error = Some(error);
                    }
                    return Some(SettingsEffect::PreviewChanged);
                }
                if self.external_change.is_some() && self.external_reload.contains(point) {
                    return self.reload_external_change().then_some(SettingsEffect::PreviewChanged);
                }
                if self.external_change.is_some() && self.external_keep.contains(point) {
                    self.keep_after_external_change();
                    return None;
                }
                if point.x < self.bounds.x + 164.0 {
                    let index = ((point.y - self.bounds.y) / 38.0) as usize;
                    if let Some(category) = CATEGORIES.get(index) {
                        self.category = (*category).into();
                        self.first = 0;
                        self.selected = 0;
                    }
                    return None;
                }
                if self.reset.contains(point) {
                    self.request_reset();
                    return None;
                }
                if self.retry.contains(point) {
                    self.retry_save();
                    return None;
                }
                if self.revert.contains(point) && self.can_revert() {
                    self.revert_changes();
                    return Some(SettingsEffect::PreviewChanged);
                }
            }
            UiEvent::Key(Key::Escape) => {
                if self.reset_pending {
                    self.confirm_reset(false);
                    return None;
                }
                self.dismiss();
                return Some(SettingsEffect::Close);
            }
            UiEvent::Key(Key::Tab) => {
                self.traverse_focus(false);
                return None;
            }
            UiEvent::Key(Key::Down) if !self.query_focused => {
                self.selected = (self.selected + 1).min(self.definitions().len().saturating_sub(1));
                self.reveal();
                return None;
            }
            UiEvent::Key(Key::Up) if !self.query_focused => {
                self.selected = self.selected.saturating_sub(1);
                self.reveal();
                return None;
            }
            UiEvent::Key(Key::Enter) if self.reset_pending => {
                self.confirm_reset(true);
                return Some(SettingsEffect::PreviewChanged);
            }
            UiEvent::Key(Key::Enter) | UiEvent::Key(Key::Space) if !self.query_focused => {
                if let Some(id) = self.focus.focused() {
                    return self.accessibility_action(id.0, true);
                }
                return self.choose(self.selected.saturating_sub(self.first));
            }
            _ => {}
        }
        for index in 0..self.rows.len() {
            if self.rows[index].copy.event(event) == Some(ControlAction::Activated) {
                return Some(SettingsEffect::CopyKey(self.rows[index].definition.key.into()));
            }
            if self.rows[index].value.event(event) == Some(ControlAction::Activated) {
                self.selected = self.first + index;
                return self.choose(index);
            }
        }
        None
    }
    fn reveal(&mut self) {
        let visible = self.rows.len().max(1);
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + visible {
            self.first = self.selected + 1 - visible;
        }
    }
    pub fn copy_selected_key(&self) -> Option<String> {
        self.definitions().get(self.selected).map(|d| d.key.into())
    }
    pub fn draw(
        &mut self,
        bounds: Rect,
        backend: &mut impl TextBackend,
        ops: &mut Vec<DrawOp>,
    ) -> Result<(), LayoutError> {
        for mut field in self.retired_fields.drain(..) {
            field.release(backend);
        }
        // Per-face preview layouts shaped last frame have now been painted, so
        // release them before shaping this frame's set.
        for layout in std::mem::take(&mut self.retired_layouts) {
            backend.release_layout(layout);
        }
        self.query
            .set_placeholder(&self.label("settings.search", "Search settings"));
        self.bounds = bounds;
        let effective = self.effective();
        let theme = config::Theme::resolve(effective.theme, self.system, &effective.theme_overrides)
            .unwrap_or_else(|_| config::Theme::builtin(self.system.dark));
        let color = |key: &str| Color(theme.color(key).unwrap().rgb);
        let bg = color("surface.editor");
        let chrome = color("surface.chrome");
        let foreground = color("text");
        let muted = color("text.muted");
        let border = color("border");
        let focus = color("focus.ring");
        ops.push(DrawOp::PushClip(bounds));
        ops.push(DrawOp::Fill(bounds, bg));
        // Persistent header: title, a way to open the file, a close button and the
        // Escape hint, so the page can be closed with the mouse alone (UX-54b).
        const HEADER: f32 = 44.0;
        ops.push(DrawOp::Fill(rect(bounds.x, bounds.y, bounds.width, HEADER), chrome));
        text(
            ops,
            bounds.x + 18.0,
            bounds.y + 12.0,
            self.label("settings.title", "Settings"),
            16.0,
            foreground,
        );
        text(
            ops,
            bounds.x + 100.0,
            bounds.y + 15.0,
            self.label("settings.close_hint", "Esc to close"),
            11.0,
            muted,
        );
        self.close_button = rect(bounds.x + bounds.width - 44.0, bounds.y + 8.0, 30.0, 28.0);
        self.open_toml = rect(bounds.x + bounds.width - 228.0, bounds.y + 8.0, 172.0, 28.0);
        ops.push(DrawOp::StrokeRounded(
            self.open_toml,
            color("border.interactive"),
            4.0,
            1.0,
        ));
        text(
            ops,
            self.open_toml.x + 12.0,
            self.open_toml.y + 6.0,
            self.label("settings.open_toml", "Open settings.toml"),
            12.0,
            foreground,
        );
        ops.push(DrawOp::StrokeRounded(
            self.close_button,
            color("border.interactive"),
            4.0,
            1.0,
        ));
        text(
            ops,
            self.close_button.x + 10.0,
            self.close_button.y + 5.0,
            "\u{00d7}",
            16.0,
            foreground,
        );
        ops.push(DrawOp::Line {
            from: Point {
                x: bounds.x,
                y: bounds.y + HEADER,
            },
            to: Point {
                x: bounds.x + bounds.width,
                y: bounds.y + HEADER,
            },
            color: border,
            width: 1.0,
        });
        let bounds = rect(
            bounds.x,
            bounds.y + HEADER,
            bounds.width,
            (bounds.height - HEADER).max(1.0),
        );
        self.bounds = bounds;
        let sidebar = 164.0_f32.min(bounds.width * 0.26);
        ops.push(DrawOp::Fill(rect(bounds.x, bounds.y, sidebar, bounds.height), chrome));
        for (index, category) in CATEGORIES.iter().enumerate() {
            let y = bounds.y + index as f32 * 38.0;
            if self.category == *category {
                ops.push(DrawOp::Fill(
                    rect(bounds.x, y, sidebar, 38.0),
                    color("surface.elevated"),
                ));
                ops.push(DrawOp::Fill(rect(bounds.x, y, 3.0, 38.0), focus));
            }
            text(
                ops,
                bounds.x + 18.0,
                y + 10.0,
                self.label(&format!("settings.category.{category}"), category),
                13.0,
                foreground,
            );
        }
        let x = bounds.x + sidebar + 18.0;
        let width = (bounds.width - sidebar - 36.0).max(1.0);
        self.search_bounds = rect(x, bounds.y + 12.0, width, 32.0);
        self.query.draw_with_theme(
            backend,
            self.search_bounds,
            self.query_focused,
            bareline_ui::theme::UiTheme::from_tokens(|key| theme.color(key).map(|color| (color.rgb, color.alpha)))
                .unwrap(),
            ops,
        )?;
        self.scope_user = rect(x, bounds.y + 52.0, 78.0, 28.0);
        self.scope_workspace = rect(x + 86.0, bounds.y + 52.0, 130.0, 28.0);
        self.opt_in = rect(x + 230.0, bounds.y + 52.0, (width - 230.0).max(0.0), 28.0);
        for (target, label, scope) in [
            (self.scope_user, "User", Scope::User),
            (self.scope_workspace, "Workspace", Scope::Workspace),
        ] {
            text(
                ops,
                target.x,
                target.y + 4.0,
                if self.scope == scope { "◉" } else { "○" },
                15.0,
                if self.scope == scope { focus } else { muted },
            );
            text(
                ops,
                target.x + 24.0,
                target.y + 4.0,
                self.label(
                    if scope == Scope::User {
                        "settings.scope.user"
                    } else {
                        "settings.scope.workspace"
                    },
                    label,
                ),
                13.0,
                foreground,
            );
        }
        if self.scope == Scope::Workspace {
            text(
                ops,
                self.opt_in.x,
                self.opt_in.y + 4.0,
                if self.workspace_opted_in {
                    "Workspace preferences enabled"
                } else {
                    "Enable workspace preferences"
                },
                12.0,
                focus,
            );
        }
        let header = bounds.y + 88.0;
        text(
            ops,
            x,
            header,
            self.label("settings.effective", "Effective values"),
            13.0,
            muted,
        );
        ops.push(DrawOp::Line {
            from: Point {
                x: bounds.x + sidebar,
                y: header + 24.0,
            },
            to: Point {
                x: bounds.x + bounds.width,
                y: header + 24.0,
            },
            color: border,
            width: 1.0,
        });
        let row_height = if width < 620.0 { 96.0 } else { 58.0 };
        // Appearance shows the theme cards where the color-mode picker row used
        // to be; the settings rows start below the card band.
        let cards_visible = self.theme_cards_visible();
        let card_band = if cards_visible { 108.0 } else { 0.0 };
        if cards_visible {
            let mode = self.theme_mode_value();
            let card_y = header + 30.0;
            let gap = 12.0;
            let card_width = ((width - gap * 2.0) / 3.0).max(1.0);
            let mut cards = [Rect::default(); 3];
            for (index, (title, value)) in THEME_CARDS.iter().enumerate() {
                let card = rect(x + index as f32 * (card_width + gap), card_y, card_width, 92.0);
                cards[index] = card;
                // Resolve the card's own theme so it previews that mode's colors.
                let card_theme = config::Theme::resolve(
                    match *value {
                        "light" => config::ThemeMode::Light,
                        "dark" => config::ThemeMode::Dark,
                        _ => config::ThemeMode::System,
                    },
                    self.system,
                    &effective.theme_overrides,
                )
                .unwrap_or_else(|_| config::Theme::builtin(*value == "dark"));
                let pick = |key: &str, fallback: Color| card_theme.color(key).map(|c| Color(c.rgb)).unwrap_or(fallback);
                let card_bg = pick("surface.editor", bg);
                let card_fg = pick("text", foreground);
                let accent = pick("accent", focus);
                let selected = mode == *value;
                ops.push(DrawOp::FillRounded(card, card_bg, 6.0));
                ops.push(DrawOp::StrokeRounded(
                    card,
                    if selected { focus } else { border },
                    6.0,
                    if selected { 2.0 } else { 1.0 },
                ));
                // Accent swatch, so each card advertises its accent color.
                ops.push(DrawOp::FillRounded(
                    rect(card.x + 12.0, card.y + 12.0, 40.0, 20.0),
                    accent,
                    4.0,
                ));
                text(
                    ops,
                    card.x + 12.0,
                    card.y + 48.0,
                    self.label(&format!("settings.theme.{value}"), title),
                    14.0,
                    card_fg,
                );
                if selected {
                    text(
                        ops,
                        card.x + 12.0,
                        card.y + 68.0,
                        self.label("settings.theme.current", "Selected"),
                        11.0,
                        accent,
                    );
                }
            }
            self.theme_cards = cards;
        } else {
            self.theme_cards = [Rect::default(); 3];
        }
        let top = header + 30.0 + card_band;
        let footer_height = if self.external_change.is_some() { 98.0 } else { 60.0 };
        let count = ((bounds.y + bounds.height - footer_height - top) / row_height).max(0.0) as usize;
        let definitions = self.definitions();
        self.first = self.first.min(definitions.len().saturating_sub(1));
        let old = std::mem::take(&mut self.rows);
        for (index, definition) in definitions.iter().skip(self.first).take(count).enumerate() {
            let y = top + index as f32 * row_height;
            let compact = width < 620.0;
            let control = if compact {
                rect(x, y + 52.0, width, 30.0)
            } else {
                rect(x + width * 0.70, y + 5.0, width * 0.30, 34.0)
            };
            // The visible TOML key itself is the copy target; its accessible
            // name and command expose the action without extra row clutter.
            let copy = rect(x + 10.0, y + 25.0, (width * 0.29 - 10.0).max(0.0), 22.0);
            let disabled = self.scope == Scope::Workspace && !definition.workspace_allowed;
            let state = old
                .iter()
                .find(|r| r.definition.key == definition.key)
                .map(|r| r.value.state)
                .unwrap_or_default();
            let copy_state = old
                .iter()
                .find(|r| r.definition.key == definition.key)
                .map(|r| r.copy.state)
                .unwrap_or_default();
            let value = self.value_text(definition.key);
            text(
                ops,
                x + 10.0,
                y + 4.0,
                self.label(&format!("setting.{}.title", definition.key), definition.title),
                13.0,
                foreground,
            );
            text(ops, x + 10.0, y + 27.0, definition.key, 11.0, muted);
            if !compact {
                ops.push(DrawOp::PushClip(rect(x + width * 0.29, y, width * 0.39, row_height)));
                text(
                    ops,
                    x + width * 0.29,
                    y + 16.0,
                    self.label(definition.description_id, definition.description),
                    12.0,
                    muted,
                );
                ops.push(DrawOp::PopClip);
            }
            let selected_row = self.selected == self.first + index && !self.query_focused;
            let outline = if selected_row {
                focus
            } else {
                color("border.interactive")
            };
            if matches!(definition.kind, SettingKind::Boolean) {
                // Booleans are switches, not two-item dropdowns (UX-54k).
                let on = value == on_off(true);
                let track = rect(control.x + 8.0, control.y + 7.0, 40.0, 20.0);
                ops.push(DrawOp::FillRounded(
                    track,
                    if on && !disabled {
                        focus
                    } else {
                        color("surface.chrome")
                    },
                    10.0,
                ));
                ops.push(DrawOp::StrokeRounded(track, outline, 10.0, 1.0));
                ops.push(DrawOp::FillRounded(
                    rect(
                        if on { track.x + 21.0 } else { track.x + 2.0 },
                        track.y + 2.0,
                        16.0,
                        16.0,
                    ),
                    color("surface.editor"),
                    8.0,
                ));
                text(
                    ops,
                    track.x + 52.0,
                    control.y + 8.0,
                    value,
                    13.0,
                    if disabled { muted } else { foreground },
                );
            } else {
                let has_list = self.choice_list(definition.key).is_some();
                ops.push(DrawOp::StrokeRounded(control, outline, 3.0, 1.0));
                ops.push(DrawOp::PushClip(control));
                text(
                    ops,
                    control.x + 12.0,
                    control.y + 8.0,
                    value,
                    13.0,
                    if disabled { muted } else { foreground },
                );
                // A color map previews its overrides as swatches beside the count.
                if config::is_color_map(definition.key) {
                    let mut swatch_x = control.x + 90.0;
                    for hex in effective.theme_overrides.values().take(6) {
                        if let Ok(parsed) = config::ThemeColor::parse(hex) {
                            let swatch = rect(swatch_x, control.y + 9.0, 16.0, 16.0);
                            ops.push(DrawOp::FillRounded(swatch, Color(parsed.rgb), 3.0));
                            ops.push(DrawOp::StrokeRounded(swatch, border, 3.0, 1.0));
                            swatch_x += 20.0;
                        }
                    }
                }
                if has_list {
                    // Only rows that actually open a list draw a chevron (UX-54a).
                    text(
                        ops,
                        control.x + control.width - 20.0,
                        control.y + 8.0,
                        "\u{2304}",
                        13.0,
                        muted,
                    );
                } else if matches!(definition.kind, SettingKind::Strings | SettingKind::Map) {
                    text(
                        ops,
                        control.x + control.width - 26.0,
                        control.y + 8.0,
                        self.label("settings.edit", "Edit\u{2026}"),
                        11.0,
                        muted,
                    );
                }
                ops.push(DrawOp::PopClip);
            }
            if definition.key == "editor.font.family"
                && let Some(missing) = self.missing_font()
            {
                text(
                    ops,
                    x + 10.0,
                    y + 44.0,
                    format!("\u{26a0} \u{201c}{missing}\u{201d} is not installed on this PC."),
                    11.0,
                    color("danger"),
                );
            }
            if definition.restart_required {
                text(
                    ops,
                    x + 10.0,
                    y + 44.0,
                    self.label("settings.restart", "Restart required"),
                    10.0,
                    muted,
                );
            }
            ops.push(DrawOp::Line {
                from: Point {
                    x: bounds.x + sidebar,
                    y: y + row_height,
                },
                to: Point {
                    x: bounds.x + bounds.width,
                    y: y + row_height,
                },
                color: border,
                width: 1.0,
            });
            self.rows.push(Row {
                definition,
                value: Button {
                    id: ViewId(
                        2000 + config::DEFINITIONS
                            .iter()
                            .position(|d| d.key == definition.key)
                            .unwrap() as u64
                            * 2,
                    ),
                    label: definition.title.into(),
                    bounds: control,
                    toggle: false,
                    state: ControlState { disabled, ..state },
                },
                copy: Button {
                    id: ViewId(
                        2001 + config::DEFINITIONS
                            .iter()
                            .position(|d| d.key == definition.key)
                            .unwrap() as u64
                            * 2,
                    ),
                    label: format!("Copy {}", definition.key),
                    bounds: copy,
                    toggle: false,
                    state: copy_state,
                },
            });
        }
        if definitions.is_empty() {
            text(
                ops,
                x,
                top + 12.0,
                if self.query.value().is_empty() {
                    "Edit this category in settings.toml"
                } else {
                    "No matching settings"
                },
                13.0,
                muted,
            );
        }
        let bottom = bounds.y + bounds.height - 44.0;
        self.reset = rect(x + width - 132.0, bottom, 132.0, 30.0);
        self.retry = rect(x + width - 204.0, bottom, 64.0, 30.0);
        self.revert = rect(x + width - 278.0, bottom, 68.0, 30.0);
        self.external_reload = Rect::default();
        self.external_keep = Rect::default();
        if let Some(change) = &self.external_change {
            let conflict_y = bottom - 38.0;
            self.external_reload = rect(x + width - 212.0, conflict_y, 96.0, 30.0);
            self.external_keep = rect(x + width - 108.0, conflict_y, 108.0, 30.0);
            let scope = if change.scope == Scope::Workspace {
                "Workspace"
            } else {
                "User"
            };
            text(
                ops,
                x,
                conflict_y + 8.0,
                &format!("{scope} settings changed on disk"),
                13.0,
                color("danger"),
            );
            for (bounds, label) in [(self.external_reload, "Reload disk"), (self.external_keep, "Keep mine")] {
                ops.push(DrawOp::StrokeRounded(bounds, color("border.interactive"), 3.0, 1.0));
                text(ops, bounds.x + 10.0, bounds.y + 8.0, label, 12.0, foreground);
            }
        }
        let status = self.status_description();
        ops.push(DrawOp::PushClip(rect(x, bottom, (width - 290.0).max(0.0), 36.0)));
        text(
            ops,
            x,
            bottom + 8.0,
            status,
            13.0,
            if self.error.is_some() {
                color("danger")
            } else {
                foreground
            },
        );
        ops.push(DrawOp::PopClip);
        text(
            ops,
            self.revert.x,
            self.revert.y + 8.0,
            self.label("settings.revert", "Revert"),
            12.0,
            if self.can_revert() { foreground } else { muted },
        );
        if matches!(self.current().status, SaveStatus::Failed(_)) {
            text(
                ops,
                self.retry.x,
                self.retry.y + 8.0,
                self.label("settings.retry", "Retry"),
                12.0,
                focus,
            );
        }
        ops.push(DrawOp::StrokeRounded(self.reset, color("border.interactive"), 3.0, 1.0));
        text(
            ops,
            self.reset.x + 12.0,
            self.reset.y + 8.0,
            self.label("settings.reset_section", "Reset section"),
            12.0,
            foreground,
        );
        if self.reset_pending {
            let dialog = self.reset_dialog_bounds();
            ops.push(DrawOp::Fill(dialog, color("surface.elevated")));
            ops.push(DrawOp::Stroke(dialog, focus, 2.0));
            text(
                ops,
                dialog.x + 12.0,
                dialog.y + 18.0,
                self.localizer
                    .format(
                        "settings.reset",
                        &[
                            (
                                "section",
                                &self.label(&format!("settings.category.{}", self.category), &self.category),
                            ),
                            (
                                "scope",
                                &self.label(
                                    if self.scope == Scope::User {
                                        "settings.scope.user"
                                    } else {
                                        "settings.scope.workspace"
                                    },
                                    if self.scope == Scope::User { "User" } else { "Workspace" },
                                ),
                            ),
                        ],
                    )
                    .unwrap_or_else(|_| format!("Reset {} in {:?} settings?", self.category, self.scope)),
                14.0,
                foreground,
            );
            text(
                ops,
                dialog.x + 12.0,
                dialog.y + 54.0,
                self.label("settings.reset_hint", "Enter: reset section   Escape: cancel"),
                13.0,
                muted,
            );
            for (id, label, x) in [(8011, "Reset", dialog.x + 12.0), (8012, "Cancel", dialog.x + 92.0)] {
                let button = rect(x, dialog.y + 76.0, 72.0, 28.0);
                ops.push(DrawOp::StrokeRounded(
                    button,
                    if self.focus.focused() == Some(ViewId(id)) {
                        focus
                    } else {
                        color("border.interactive")
                    },
                    4.0,
                    1.0,
                ));
                text(
                    ops,
                    x + 8.0,
                    button.y + 6.0,
                    self.label(
                        if id == 8011 {
                            "settings.reset_button"
                        } else {
                            "settings.cancel"
                        },
                        label,
                    ),
                    13.0,
                    foreground,
                );
            }
        }
        let mut shaped_previews: Vec<LayoutId> = Vec::new();
        if let Some(popup) = &self.popup {
            let source = Labels(&popup.labels);
            ops.push(DrawOp::FillRounded(popup.list.bounds, color("surface.elevated"), 4.0));
            ops.push(DrawOp::StrokeRounded(
                popup.list.bounds,
                color("border.interactive"),
                4.0,
                1.0,
            ));
            if popup.font_preview {
                // Each entry is shaped in the family it names so the list previews
                // faces; a family that will not shape falls back to the UI font.
                let selection = color("surface.currentLine");
                let padding = Metrics::COMPACT.padding;
                let size = Metrics::COMPACT.font_size;
                ops.push(DrawOp::PushClip(popup.list.bounds));
                for index in popup.list.visible(&source) {
                    let row = popup.list.row_bounds(index);
                    if popup.list.selected == Some(index) {
                        ops.push(DrawOp::Fill(row, selection));
                    }
                    let label = &popup.labels[index];
                    let family = match popup.values.get(index) {
                        Some(SettingValue::Text(name)) if !name.is_empty() => Some(name.as_str()),
                        _ => None,
                    };
                    let origin = Point {
                        x: row.x + padding,
                        y: row.y + 6.0,
                    };
                    let width = (row.width - padding * 2.0).max(1.0);
                    let shaped =
                        family.and_then(|family| backend.shape_with_font_family(label, size, width, family).ok());
                    if let Some(layout) = shaped {
                        ops.push(DrawOp::Layout {
                            origin,
                            layout,
                            color: foreground,
                        });
                        shaped_previews.push(layout);
                    } else {
                        text(ops, origin.x, origin.y, label.clone(), size, foreground);
                    }
                }
                ops.push(DrawOp::PopClip);
                if popup.list.state.focused {
                    ops.push(DrawOp::Stroke(popup.list.bounds, focus, 2.0));
                }
            } else {
                popup.list.paint(
                    &source,
                    bareline_ui::widgets::Theme {
                        surface: color("surface.elevated"),
                        text: foreground,
                        muted,
                        selection: color("surface.currentLine"),
                        border: color("border.interactive"),
                        focus,
                    },
                    ops,
                );
            }
        }
        // Retire this frame's preview layouts once the frame has been painted.
        self.retired_layouts.append(&mut shaped_previews);
        if let Some(edit) = &mut self.value_edit {
            if let Some(row) = self.rows.iter().find(|row| row.definition.key == edit.key) {
                edit.bounds = row.value.bounds;
            }
            let panel = rect(
                edit.bounds.x - 4.0,
                edit.bounds.y - 4.0,
                edit.bounds.width + 8.0,
                edit.bounds.height + 64.0,
            );
            ops.push(DrawOp::FillRounded(panel, color("surface.elevated"), 4.0));
            ops.push(DrawOp::StrokeRounded(panel, color("border.interactive"), 4.0, 1.0));
            edit.field.draw_with_theme(
                backend,
                edit.bounds,
                self.focus.focused() == Some(ViewId(8007)),
                bareline_ui::theme::UiTheme::from_tokens(|key| theme.color(key).map(|c| (c.rgb, c.alpha))).unwrap(),
                ops,
            )?;
            if let Some(reason) = edit.field.validation() {
                ops.push(DrawOp::PushClip(rect(
                    edit.bounds.x,
                    edit.bounds.y + edit.bounds.height,
                    edit.bounds.width,
                    22.0,
                )));
                text(
                    ops,
                    edit.bounds.x,
                    edit.bounds.y + edit.bounds.height + 2.0,
                    reason,
                    11.0,
                    color("danger"),
                );
                ops.push(DrawOp::PopClip);
            }
            for (id, label, x) in [(8009, "Apply", edit.bounds.x), (8010, "Cancel", edit.bounds.x + 80.0)] {
                let bounds = rect(x, edit.bounds.y + edit.bounds.height + 24.0, 72.0, 28.0);
                ops.push(DrawOp::StrokeRounded(
                    bounds,
                    if self.focus.focused() == Some(ViewId(id)) {
                        focus
                    } else {
                        color("border.interactive")
                    },
                    4.0,
                    1.0,
                ));
                text(
                    ops,
                    x + 8.0,
                    bounds.y + 6.0,
                    self.localizer
                        .format(
                            if id == 8009 {
                                "settings.apply"
                            } else {
                                "settings.cancel"
                            },
                            &[],
                        )
                        .unwrap_or_else(|_| label.into()),
                    13.0,
                    foreground,
                );
            }
        }
        if let Some(edit) = self.collection_edit.as_ref() {
            let panel = rect(
                self.bounds.x + sidebar + 30.0,
                self.bounds.y + 60.0,
                (self.bounds.width - sidebar - 60.0).max(240.0),
                (self.bounds.height - 120.0).max(180.0),
            );
            ops.push(DrawOp::FillRounded(panel, color("surface.elevated"), 6.0));
            ops.push(DrawOp::StrokeRounded(panel, focus, 6.0, 2.0));
            let (title, map, colors, picker_mode, entries, catalog, filter) = {
                (
                    config::DEFINITIONS
                        .iter()
                        .find(|d| d.key == edit.key)
                        .map_or(edit.key, |d| d.title)
                        .to_owned(),
                    edit.map,
                    edit.colors,
                    edit.command_picker(),
                    edit.entries.clone(),
                    edit.catalog.clone(),
                    edit.field.value().to_lowercase(),
                )
            };
            text(ops, panel.x + 14.0, panel.y + 12.0, title, 15.0, foreground);
            let mut remove = Vec::new();
            let rows_top = panel.y + 44.0;
            // A command picker keeps the chip list short so the picker below has
            // room; other editors fill the panel with chips.
            let visible = if picker_mode {
                3
            } else {
                (((panel.height - 150.0) / 28.0).max(0.0)) as usize
            };
            for (index, (key, value)) in entries.iter().take(visible).enumerate() {
                let row_y = rows_top + index as f32 * 28.0;
                let chip = rect(panel.x + 14.0, row_y, panel.width - 60.0, 24.0);
                ops.push(DrawOp::FillRounded(chip, color("surface.chrome"), 12.0));
                ops.push(DrawOp::PushClip(chip));
                if colors {
                    // Color rows carry a swatch of the value they set.
                    if let Ok(parsed) = config::ThemeColor::parse(value) {
                        let swatch = rect(chip.x + 8.0, chip.y + 5.0, 14.0, 14.0);
                        ops.push(DrawOp::FillRounded(swatch, Color(parsed.rgb), 3.0));
                        ops.push(DrawOp::StrokeRounded(swatch, border, 3.0, 1.0));
                    }
                    text(
                        ops,
                        chip.x + 30.0,
                        chip.y + 4.0,
                        format!("{key}  {value}"),
                        12.0,
                        foreground,
                    );
                } else {
                    let label = if picker_mode {
                        // Chips read as command titles, not raw IDs.
                        if value.is_empty() { key.clone() } else { value.clone() }
                    } else if map {
                        format!("{key} = {value}")
                    } else {
                        key.clone()
                    };
                    text(ops, chip.x + 10.0, chip.y + 4.0, label, 12.0, foreground);
                }
                ops.push(DrawOp::PopClip);
                let button = rect(panel.x + panel.width - 40.0, row_y, 24.0, 24.0);
                ops.push(DrawOp::StrokeRounded(button, color("border.interactive"), 4.0, 1.0));
                text(ops, button.x + 8.0, button.y + 3.0, "\u{00d7}", 13.0, muted);
                remove.push(button);
            }
            if entries.len() > visible {
                text(
                    ops,
                    panel.x + 14.0,
                    rows_top + visible as f32 * 28.0 + 4.0,
                    format!("{} more not shown", entries.len() - visible),
                    11.0,
                    muted,
                );
            }
            // The field sits below the chips in picker mode (so the picker list
            // can fill the middle), and near the bottom otherwise.
            let field_y = if picker_mode {
                rows_top + visible as f32 * 28.0 + 20.0
            } else {
                panel.y + panel.height - 76.0
            };
            let field_bounds = rect(panel.x + 14.0, field_y, panel.width - 110.0, 30.0);
            let add = rect(panel.x + panel.width - 88.0, field_y, 74.0, 30.0);
            let apply = rect(panel.x + 14.0, panel.y + panel.height - 38.0, 78.0, 28.0);
            let cancel = rect(panel.x + 100.0, panel.y + panel.height - 38.0, 78.0, 28.0);
            // The command picker: catalog commands matching the filter and not
            // already added, each a clickable row that adds its ID.
            let mut picker = Vec::new();
            if picker_mode {
                let picker_top = field_y + 40.0;
                let picker_bottom = panel.y + panel.height - 46.0;
                let mut row_y = picker_top;
                for (index, (id, cmd_title)) in catalog.iter().enumerate() {
                    if row_y + 24.0 > picker_bottom {
                        break;
                    }
                    if entries.iter().any(|(existing, _)| existing == id) {
                        continue;
                    }
                    if !filter.is_empty()
                        && !cmd_title.to_lowercase().contains(&filter)
                        && !id.to_lowercase().contains(&filter)
                    {
                        continue;
                    }
                    let row = rect(panel.x + 14.0, row_y, panel.width - 28.0, 24.0);
                    ops.push(DrawOp::PushClip(row));
                    text(ops, row.x + 8.0, row.y + 4.0, cmd_title.clone(), 12.0, foreground);
                    ops.push(DrawOp::PopClip);
                    picker.push((row, index));
                    row_y += 26.0;
                }
                if picker.is_empty() {
                    text(
                        ops,
                        panel.x + 14.0,
                        picker_top + 4.0,
                        "No matching commands",
                        12.0,
                        muted,
                    );
                }
            }
            let ui_theme =
                bareline_ui::theme::UiTheme::from_tokens(|key| theme.color(key).map(|c| (c.rgb, c.alpha))).unwrap();
            if let Some(edit) = self.collection_edit.as_mut() {
                edit.bounds = panel;
                edit.remove = remove;
                edit.add = add;
                edit.apply = apply;
                edit.cancel = cancel;
                edit.picker = picker;
                edit.field.draw_with_theme(backend, field_bounds, true, ui_theme, ops)?;
            }
            for (bounds, label) in [(add, "Add"), (apply, "Apply"), (cancel, "Cancel")] {
                ops.push(DrawOp::StrokeRounded(bounds, color("border.interactive"), 4.0, 1.0));
                text(ops, bounds.x + 12.0, bounds.y + 6.0, label, 12.0, foreground);
            }
            if let Some(reason) = self.collection_edit.as_ref().and_then(|edit| edit.error.clone()) {
                text(
                    ops,
                    panel.x + 190.0,
                    panel.y + panel.height - 32.0,
                    reason,
                    11.0,
                    color("danger"),
                );
            }
        }
        if self.restart_pending {
            // Restart-required settings say so and offer to relaunch right away
            // instead of silently doing nothing.
            let toast = rect(x, bounds.y + bounds.height - 90.0, width.min(520.0), 38.0);
            ops.push(DrawOp::FillRounded(toast, color("surface.elevated"), 6.0));
            ops.push(DrawOp::StrokeRounded(toast, focus, 6.0, 1.0));
            text(
                ops,
                toast.x + 12.0,
                toast.y + 11.0,
                self.label("settings.restart_toast", "Restart Bareline to apply this change."),
                12.0,
                foreground,
            );
            self.restart_now = rect(toast.x + toast.width - 184.0, toast.y + 5.0, 96.0, 28.0);
            ops.push(DrawOp::StrokeRounded(self.restart_now, focus, 4.0, 1.0));
            text(
                ops,
                self.restart_now.x + 12.0,
                self.restart_now.y + 6.0,
                self.label("settings.restart_now", "Restart now"),
                12.0,
                focus,
            );
            self.restart_dismiss = rect(toast.x + toast.width - 80.0, toast.y + 5.0, 70.0, 28.0);
            ops.push(DrawOp::StrokeRounded(
                self.restart_dismiss,
                color("border.interactive"),
                4.0,
                1.0,
            ));
            text(
                ops,
                self.restart_dismiss.x + 14.0,
                self.restart_dismiss.y + 6.0,
                self.label("settings.restart_later", "Later"),
                12.0,
                foreground,
            );
        }
        if self.value_edit.is_none() && self.popup.is_none() && !self.reset_pending {
            if let Some(id) = self.focused_id().filter(|id| *id != ViewId(8000)) {
                if let Some(node) = self.semantics().into_iter().find(|node| node.id == id) {
                    ops.push(DrawOp::Stroke(node.bounds, focus, 2.0));
                }
            }
        }
        ops.push(DrawOp::PopClip);
        self.refresh_focus();
        Ok(())
    }
    pub fn semantics(&self) -> Vec<Semantics> {
        if !self.open {
            return Vec::new();
        }
        if self.reset_pending {
            let dialog = self.reset_dialog_bounds();
            return [
                (8011, "Reset section", dialog.x + 12.0),
                (8012, "Cancel reset", dialog.x + 92.0),
            ]
            .into_iter()
            .map(|(id, name, x)| {
                Semantics::new(
                    ViewId(id),
                    SemanticRole::Button,
                    &self.label(
                        if id == 8011 {
                            "settings.reset_section"
                        } else {
                            "settings.cancel"
                        },
                        name,
                    ),
                    "settings.reset_section",
                    rect(x, dialog.y + 76.0, 72.0, 28.0),
                    ControlState {
                        focused: self.focus.focused() == Some(ViewId(id)),
                        ..Default::default()
                    },
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke)
            })
            .collect();
        }
        if let Some(edit) = &self.value_edit {
            let name = config::DEFINITIONS
                .iter()
                .find(|d| d.key == edit.key)
                .map_or(edit.key, |d| d.title);
            let mut nodes = vec![edit.field.semantics(
                ViewId(8007),
                name,
                "settings.edit_value",
                edit.bounds,
                ControlState {
                    focused: self.focus.focused() == Some(ViewId(8007)),
                    ..Default::default()
                },
            )];
            for (id, label, x) in [
                (8009, "Apply value", edit.bounds.x),
                (8010, "Cancel value edit", edit.bounds.x + 80.0),
            ] {
                nodes.push(
                    Semantics::new(
                        ViewId(id),
                        SemanticRole::Button,
                        &self.label(
                            if id == 8009 {
                                "settings.apply"
                            } else {
                                "settings.cancel"
                            },
                            label,
                        ),
                        "settings.edit_value",
                        rect(x, edit.bounds.y + edit.bounds.height + 24.0, 72.0, 28.0),
                        ControlState {
                            focused: self.focus.focused() == Some(ViewId(id)),
                            ..Default::default()
                        },
                    )
                    .action(SemanticAction::Focus)
                    .action(SemanticAction::Invoke),
                );
            }
            return nodes;
        }
        let mut nodes = vec![self.query.semantics(
            ViewId(8000),
            &self.label("settings.search", "Search settings"),
            "settings.search",
            self.search_bounds,
            ControlState {
                focused: self.query_focused && self.popup.is_none(),
                ..Default::default()
            },
        )];
        for (id, label, command, bounds) in [
            (8001, "User settings", "settings.scope_user", self.scope_user),
            (
                8002,
                "Workspace settings",
                "settings.scope_workspace",
                self.scope_workspace,
            ),
            (8003, "Reset section", "settings.reset_section", self.reset),
            (8004, "Revert changes", "settings.revert", self.revert),
        ] {
            let revert = id == 8004;
            let disabled = revert && !self.can_revert();
            let label = if revert {
                let scope = if self.scope == Scope::Workspace {
                    "Workspace"
                } else {
                    "User"
                };
                if disabled {
                    format!("Revert unavailable: {scope} settings match the values from when Settings opened")
                } else {
                    format!("Revert {scope} settings to the values from when Settings opened")
                }
            } else {
                self.label(command, label)
            };
            nodes.push(
                Semantics::new(
                    ViewId(id),
                    SemanticRole::Button,
                    &label,
                    command,
                    bounds,
                    ControlState {
                        disabled,
                        ..Default::default()
                    },
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke),
            );
        }
        if let Some(change) = &self.external_change {
            let scope = if change.scope == Scope::Workspace {
                "Workspace"
            } else {
                "User"
            };
            for (id, label, command, bounds) in [
                (
                    8013,
                    format!("Reload {scope} settings from disk and discard my current changes"),
                    "settings.external_reload",
                    self.external_reload,
                ),
                (
                    8014,
                    format!("Keep my {scope} settings and replace the changed disk file"),
                    "settings.external_keep",
                    self.external_keep,
                ),
            ] {
                nodes.push(
                    Semantics::new(
                        ViewId(id),
                        SemanticRole::Button,
                        &label,
                        command,
                        bounds,
                        ControlState::default(),
                    )
                    .action(SemanticAction::Focus)
                    .action(SemanticAction::Invoke),
                );
            }
        }
        if matches!(self.current().status, SaveStatus::Failed(_)) {
            nodes.push(
                Semantics::new(
                    ViewId(8005),
                    SemanticRole::Button,
                    &self.label("settings.retry", "Retry saving"),
                    "settings.retry",
                    self.retry,
                    ControlState::default(),
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke),
            );
        }
        for (index, category) in CATEGORIES.iter().enumerate() {
            let mut node = Semantics::new(
                ViewId(8100 + index as u64),
                SemanticRole::ListItem,
                &self.label(&format!("settings.category.{category}"), category),
                "settings.category",
                rect(
                    self.bounds.x,
                    self.bounds.y + index as f32 * 38.0,
                    164.0_f32.min(self.bounds.width * 0.26),
                    38.0,
                ),
                ControlState::default(),
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::Invoke);
            node.selected = self.category == *category;
            nodes.push(node);
        }
        if self.scope == Scope::Workspace {
            nodes.push(
                Semantics::new(
                    ViewId(8006),
                    SemanticRole::Checkbox,
                    &self.label("settings.workspace_opt_in", "Enable workspace preferences"),
                    "settings.workspace_opt_in",
                    self.opt_in,
                    ControlState {
                        checked: self.workspace_opted_in,
                        ..Default::default()
                    },
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke),
            );
        }
        if self.theme_cards_visible() {
            let mode = self.theme_mode_value();
            for (index, (title, value)) in THEME_CARDS.iter().enumerate() {
                let mut node = Semantics::new(
                    ViewId(8020 + index as u64),
                    SemanticRole::ListItem,
                    &self.label(&format!("settings.theme.{value}"), title),
                    "theme.mode",
                    self.theme_cards[index],
                    ControlState::default(),
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke);
                node.selected = mode == *value;
                nodes.push(node);
            }
        }
        if self.restart_pending {
            nodes.push(
                Semantics::new(
                    ViewId(8023),
                    SemanticRole::Button,
                    &self.label("settings.restart_now", "Restart now"),
                    "settings.restart_now",
                    self.restart_now,
                    ControlState::default(),
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke),
            );
        }
        for row in &self.rows {
            let mut value = row.value.semantic().into_settings(row.value.bounds);
            value.name = self.label(&format!("setting.{}.title", row.definition.key), row.definition.title);
            value.focused = !self.query_focused
                && self.popup.is_none()
                && self
                    .rows
                    .iter()
                    .position(|r| r.value.id == row.value.id)
                    .is_some_and(|index| self.first + index == self.selected);
            value.value = Some(self.value_text(row.definition.key));
            nodes.push(value);
            nodes.push(
                Semantics::new(
                    row.copy.id,
                    SemanticRole::Button,
                    &row.copy.label,
                    "settings.copy_key",
                    row.copy.bounds,
                    row.copy.state,
                )
                .action(SemanticAction::Invoke),
            );
        }
        if let Some(popup) = &self.popup {
            nodes.extend(
                bareline_ui::semantics::list(
                    &popup.list,
                    popup,
                    ViewId(1),
                    |i| (ViewId(8500 + i as u64), popup.labels[i].clone()),
                    "settings.choose",
                )
                .into_iter()
                .map(|entry| entry.node),
            );
        }
        nodes
    }
    pub fn accessibility_action(&mut self, id: u64, invoke: bool) -> Option<SettingsEffect> {
        if !self.open {
            return None;
        }
        let node = self
            .semantics()
            .into_iter()
            .find(|node| node.id.0 == id && !node.disabled)?;
        if !invoke {
            self.focus.focus(ViewId(id));
            self.query_focused = id == 8000;
            if let Some(index) = self
                .rows
                .iter()
                .position(|row| row.value.id.0 == id || row.copy.id.0 == id)
            {
                self.selected = self.first + index;
            }
            if let Some(popup) = &mut self.popup {
                if let Some(index) = id.checked_sub(8500).filter(|i| *i < popup.labels.len() as u64) {
                    popup.list.selected = Some(index as usize);
                }
            }
            return None;
        }
        if self.reset_pending {
            self.confirm_reset(id == 8011);
            return Some(SettingsEffect::PreviewChanged);
        }
        if self.value_edit.is_some() {
            return match id {
                8009 => self.finish_value_edit(true),
                8010 => self.finish_value_edit(false),
                _ => None,
            };
        }
        let point = Point {
            x: node.bounds.x + node.bounds.width / 2.0,
            y: node.bounds.y + node.bounds.height / 2.0,
        };
        let down = self.event(UiEvent::PointerDown(point));
        self.event(UiEvent::PointerUp(point)).or(down)
    }
}
struct Labels<'a>(&'a [String]);
impl ItemSource for Labels<'_> {
    fn len(&self) -> Option<usize> {
        Some(self.0.len())
    }
    fn discovered(&self) -> usize {
        self.0.len()
    }
    fn label(&self, index: usize) -> &str {
        self.0.get(index).map(String::as_str).unwrap_or("")
    }
    fn enabled(&self, _: usize) -> bool {
        true
    }
}
fn on_off(value: bool) -> String {
    if value { "On" } else { "Off" }.into()
}
trait SettingSemantic {
    fn into_settings(self, bounds: Rect) -> Semantics;
}
impl SettingSemantic for bareline_ui::controls::SemanticNode {
    fn into_settings(self, bounds: Rect) -> Semantics {
        Semantics::new(
            self.id,
            SemanticRole::Combo,
            &self.name,
            "settings.change",
            bounds,
            ControlState {
                disabled: self.disabled,
                focused: self.focused,
                ..Default::default()
            },
        )
        .action(SemanticAction::Focus)
        .action(SemanticAction::Invoke)
    }
}

#[cfg(test)]
mod picker_tests {
    use super::*;
    fn controller() -> SettingsController {
        SettingsController::new(SettingsDocument::empty(Scope::User), None, SystemAppearance::default())
    }
    #[test]
    fn open_ended_rows_offer_a_list_that_ends_with_custom() {
        let controller = controller();
        for key in [
            "editor.font.size",
            "editor.tab.width",
            "editor.font.family",
            "document.resident_max_bytes",
        ] {
            let entries = controller
                .choice_list(key)
                .unwrap_or_else(|| panic!("{key} should open a list"));
            assert!(entries.len() > 1, "{key} list is too short");
            assert_eq!(
                entries.last().map(|(label, _)| label.as_str()),
                Some(CUSTOM_ENTRY),
                "{key} must still allow a typed value"
            );
        }
        let sizes = controller.choice_list("editor.font.size").unwrap();
        assert!(sizes.iter().any(|(label, _)| label == "12 pt"));
        let widths = controller.choice_list("editor.tab.width").unwrap();
        assert_eq!(widths.len(), 17);
        assert!(widths.iter().any(|(label, _)| label == "4"));
        let quota = controller.choice_list("document.resident_max_bytes").unwrap();
        assert!(quota.iter().any(|(label, _)| label == "64 MB"));
    }
    #[test]
    fn closed_kinds_use_named_values_and_lists_fall_back_to_typed_entry() {
        let controller = controller();
        let modes = controller.choice_list("theme.mode").unwrap();
        assert_eq!(modes[0].0, "System");
        assert!(modes.iter().all(|(label, _)| label != CUSTOM_ENTRY));
        assert!(controller.choice_list("search.excludes").is_none());
        assert!(controller.choice_list("theme.overrides").is_none());
    }
    #[test]
    fn theme_cards_map_to_theme_mode_values() {
        let mut controller = controller();
        controller.show();
        controller.category = "Appearance".into();
        // The Appearance list drops the color-mode row; the cards replace it.
        assert!(
            controller.definitions().iter().all(|d| d.key != "theme.mode"),
            "theme.mode should be shown as cards, not a picker row"
        );
        for (index, (_, expected)) in THEME_CARDS.iter().enumerate() {
            assert_eq!(
                controller.select_theme_card(index),
                Some(SettingsEffect::PreviewChanged)
            );
            assert_eq!(&controller.theme_mode_value(), expected);
            assert_eq!(
                controller.effective().setting_value("theme.mode"),
                Some(SettingValue::Text((*expected).into()))
            );
        }
    }
    #[test]
    fn toolbar_chip_editor_add_and_remove_keeps_ids_valid() {
        let mut controller = controller();
        controller.toolbar_catalog = vec![
            ("file.save".into(), "Save".into()),
            ("file.open".into(), "Open".into()),
            ("edit.undo".into(), "Undo".into()),
        ];
        let valid: Vec<String> = controller.toolbar_catalog.iter().map(|(id, _)| id.clone()).collect();
        // Start from an empty toolbar so the test controls the whole list.
        controller
            .edit("toolbar.commands", SettingValue::Strings(Vec::new()))
            .unwrap();
        // Open the toolbar editor and add commands by picking them from the catalog.
        controller.start_collection_edit("toolbar.commands", ViewId(0));
        {
            let edit = controller.collection_edit.as_mut().unwrap();
            assert!(edit.command_picker());
            edit.add_command(0); // Save
            edit.add_command(2); // Undo
            // A typed title resolves too; a duplicate is de-duplicated.
            edit.field.insert("Open");
            edit.add_entry();
            edit.field.insert("Save");
            edit.add_entry();
            let SettingValue::Strings(ids) = edit.value() else {
                panic!("toolbar value should be a string list");
            };
            assert_eq!(ids, vec!["edit.undo", "file.open", "file.save"]);
            assert!(
                ids.iter().all(|id| valid.contains(id)),
                "every toolbar ID must be a real command"
            );
            // Chips carry titles, not raw IDs.
            assert!(
                edit.entries
                    .iter()
                    .any(|(id, title)| id == "file.save" && title == "Save")
            );
            // Remove one and confirm the rest stay valid.
            edit.entries.remove(0);
        }
        let SettingValue::Strings(ids) = controller.collection_edit.as_ref().unwrap().value() else {
            panic!("toolbar value should be a string list");
        };
        assert_eq!(ids, vec!["file.open", "file.save"]);
        assert!(ids.iter().all(|id| valid.contains(id)));
        // Applying commits a value that validates against the schema.
        assert_eq!(
            controller.finish_collection_edit(true),
            Some(SettingsEffect::PreviewChanged)
        );
        assert_eq!(
            controller.effective().setting_value("toolbar.commands"),
            Some(SettingValue::Strings(vec!["file.open".into(), "file.save".into()]))
        );
    }
}
#[cfg(test)]
mod visual_contract_tests {
    use super::*;
    use bareline_renderer_recording::RecordingBackend;
    #[test]
    fn failed_save_exposes_retry_even_without_an_unrelated_controller_error() {
        let mut controller =
            SettingsController::new(SettingsDocument::empty(Scope::User), None, SystemAppearance::default());
        controller.show();
        controller.current_mut().status = SaveStatus::Failed("Disk full".into());
        assert!(controller.error.is_none());
        assert!(
            controller
                .semantics()
                .iter()
                .any(|node| node.id == ViewId(8005) && node.actions.contains(&SemanticAction::Invoke))
        );
        assert!(controller.status_description().contains("Disk full"));
    }
    #[test]
    fn malformed_persisted_value_reports_its_key_without_resetting_other_values() {
        let document = SettingsDocument::parse(
            b"schema_version=1\n[editor.font]\nsize=999\nfamily='Consolas'\n",
            Scope::User,
        )
        .unwrap();
        let controller = SettingsController::new(document, None, SystemAppearance::default());
        assert!(controller.status_description().contains("editor.font.size"));
        assert_eq!(controller.effective().editor_font_size_pt, 12.0);
        assert_eq!(controller.effective().editor_font_family, "Consolas");
    }
    #[test]
    fn invalid_value_keeps_draft_and_cancel_restores_row_focus() {
        let mut controller = SettingsController::new(
            SettingsDocument::empty(Scope::User),
            None,
            SystemAppearance {
                dark: false,
                high_contrast: false,
            },
        );
        controller.show();
        controller
            .draw(
                rect(0.0, 34.0, 1200.0, 660.0),
                &mut RecordingBackend::default(),
                &mut Vec::new(),
            )
            .unwrap();
        let invoker = controller.rows[1].value.id;
        // Row 1 (editor.font.size) now opens a size picker; the "Custom…" entry
        // hands the row to the free-text value editor, where an out-of-range
        // draft must be kept rather than silently applied.
        controller.choose(1);
        let custom = controller
            .popup
            .as_ref()
            .unwrap()
            .labels
            .iter()
            .position(|label| label.as_str() == CUSTOM_ENTRY)
            .unwrap();
        controller.commit_choice("editor.font.size", custom);
        assert_eq!(controller.focused_id(), Some(ViewId(8007)));
        assert!(controller.accessibility_set_value(8007, "999"));
        controller.accessibility_action(8009, true);
        assert!(controller.editing_value());
        assert_eq!(controller.effective().editor_font_size_pt, 12.0);
        assert_eq!(controller.text_field_mut().unwrap().value(), "999");
        controller.accessibility_action(8010, true);
        assert!(!controller.editing_value());
        assert_eq!(controller.focused_id(), Some(invoker));
        controller.request_reset();
        controller.dismiss();
        controller.show();
        assert!(!controller.reset_pending);
        assert_eq!(controller.focused_id(), Some(ViewId(8000)));
    }
    #[test]
    fn migration_reconciliation_retains_an_uncommitted_value_draft() {
        let mut controller =
            SettingsController::new(SettingsDocument::empty(Scope::User), None, SystemAppearance::default());
        controller.show();
        controller
            .draw(
                rect(0.0, 34.0, 1200.0, 660.0),
                &mut RecordingBackend::default(),
                &mut Vec::new(),
            )
            .unwrap();
        controller.choose(1);
        let custom = controller
            .popup
            .as_ref()
            .unwrap()
            .labels
            .iter()
            .position(|label| label.as_str() == CUSTOM_ENTRY)
            .unwrap();
        controller.commit_choice("editor.font.size", custom);
        assert!(controller.accessibility_set_value(8007, "19"));
        let revision = controller.revision;

        assert!(!controller.reconcile_user_document(SettingsDocument::empty(Scope::User), revision));
        assert_eq!(controller.revision, revision);
        assert!(controller.editing_value());
        assert_eq!(controller.text_field_mut().unwrap().value(), "19");
    }
    #[test]
    fn settings_reference_order_copy_target_and_opaque_popup() {
        let mut controller = SettingsController::new(
            SettingsDocument::empty(Scope::User),
            None,
            SystemAppearance {
                dark: true,
                high_contrast: false,
            },
        );
        controller.show();
        let mut backend = RecordingBackend::default();
        let mut ops = Vec::new();
        controller
            .draw(rect(0.0, 34.0, 1200.0, 660.0), &mut backend, &mut ops)
            .unwrap();
        assert_eq!(
            controller.rows.iter().map(|row| row.definition.key).collect::<Vec<_>>(),
            [
                "editor.font.family",
                "editor.font.size",
                "editor.line_numbers",
                "editor.wrap.mode",
                "editor.render.whitespace",
                "editor.tab.width",
                "editor.insert_spaces"
            ]
        );
        let copy = controller.rows[0].copy.bounds;
        assert_eq!(
            controller.event(UiEvent::PointerDown(Point {
                x: copy.x + 1.0,
                y: copy.y + 1.0
            })),
            None
        );
        assert_eq!(
            controller.event(UiEvent::PointerUp(Point {
                x: copy.x + 1.0,
                y: copy.y + 1.0
            })),
            Some(SettingsEffect::CopyKey("editor.font.family".into()))
        );
        // Row 3 (editor.wrap.mode) is a choice picker; rows 2/6 are now booleans
        // that toggle in place without raising a popup.
        controller.choose(3);
        let popup_bounds = controller.popup.as_ref().unwrap().list.bounds;
        ops.clear();
        controller
            .draw(rect(0.0, 34.0, 1200.0, 660.0), &mut backend, &mut ops)
            .unwrap();
        assert!(
            ops.iter()
                .any(|op| matches!(op, DrawOp::FillRounded(bounds, _, _) if *bounds == popup_bounds))
        );
        assert!(bareline_renderer::balanced_clips(&ops));
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

    fn attach_storage(
        controller: &mut SettingsController,
        path: PathBuf,
    ) -> (mpsc::Receiver<SaveJob>, mpsc::SyncSender<SaveCompletion>) {
        let (jobs, job_receiver) = mpsc::sync_channel(4);
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
}
