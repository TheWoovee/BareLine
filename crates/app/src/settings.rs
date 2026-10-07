// SPDX-License-Identifier: MPL-2.0
//! Settings tab controller. File commits run on one bounded worker; rendering never performs I/O.
//!
//! The controller is split by concern: `storage` owns the save worker and
//! queue, `conflict` resolves external edits and document handoffs,
//! `semantics` builds the accessibility tree and focus order, and `draw`
//! paints the page and records its hit rects.
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
        mpsc::{self, Receiver, Sender},
    },
};
mod conflict;
mod draw;
mod semantics;
mod storage;
use conflict::{DeferredWorkspace, ExternalChange};
use storage::Storage;
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
    /// The top of the open page's footer (its status line and Revert, Retry
    /// and Reset, with the external-change row above them when shown), as
    /// last drawn; notifications stay above it.
    pub fn footer_top(&self) -> Option<f32> {
        if !self.open || self.revert == Rect::default() {
            return None;
        }
        Some(if self.external_reload == Rect::default() {
            self.revert.y
        } else {
            self.external_reload.y.min(self.revert.y)
        })
    }
    /// True when the saved font family is not among the installed families.
    pub fn missing_font(&self) -> Option<String> {
        let family = self.effective().editor_font_family;
        if self.font_families.is_empty() || self.font_families.iter().any(|(name, _)| *name == family) {
            return None;
        }
        Some(family)
    }
    /// Replace the installed families with a fresh enumeration. The shell calls
    /// this every time Settings opens so a font installed mid-session is listed
    /// (UI-20). Returns true when the set of families changed.
    pub fn set_font_families(&mut self, families: Vec<(String, bool)>) -> bool {
        if self.font_families == families {
            return false;
        }
        self.font_families = families;
        true
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
    pub fn effective(&self) -> EffectiveSettings {
        config::resolve(
            &self.user.document,
            self.workspace.as_ref().map(|w| &w.document),
            self.workspace_opted_in,
            None,
        )
        .values
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
    pub fn editing_value(&self) -> bool {
        self.value_edit.is_some()
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
    fn reenumerated_fonts_replace_the_list_and_clear_the_missing_notice() {
        let mut controller = controller();
        let before = vec![("Consolas".to_owned(), true), ("Segoe UI".to_owned(), false)];
        assert!(controller.set_font_families(before.clone()));
        // The default editor font is not installed yet.
        assert_eq!(controller.missing_font().as_deref(), Some("Cascadia Mono"));
        assert!(
            !controller.set_font_families(before.clone()),
            "an unchanged list is not a change"
        );
        // Settings reopens after Cascadia Mono was installed mid-session.
        let mut after = before;
        after.insert(0, ("Cascadia Mono".to_owned(), true));
        assert!(controller.set_font_families(after));
        assert_eq!(controller.missing_font(), None);
        let names = controller.choice_list("editor.font.family").unwrap();
        assert!(
            names
                .iter()
                .any(|(_, value)| *value == SettingValue::Text("Cascadia Mono".into()))
        );
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
