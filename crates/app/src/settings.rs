// SPDX-License-Identifier: MPL-2.0
//! Settings tab controller. File commits run on one bounded worker; rendering never performs I/O.
use bareline_platform::LocalFileSystem;
use bareline_renderer::{Color, DrawOp, LayoutError, Point, Rect, TextBackend};
use bareline_settings::{
    self as config, EffectiveSettings, SaveStatus, Scope, SettingDefinition, SettingKind,
    SettingValue, SettingsDocument, SettingsEditor, SystemAppearance,
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
}
struct SaveJob {
    scope: Scope,
    snapshot: SettingsDocument,
    path: PathBuf,
}
struct SaveCompletion {
    scope: Scope,
    snapshot: SettingsDocument,
    error: Option<String>,
    path: PathBuf,
}
struct Storage {
    user: PathBuf,
    workspace: Option<PathBuf>,
    sender: SyncSender<SaveJob>,
    receiver: Receiver<SaveCompletion>,
    active: bool,
    pending: VecDeque<SaveJob>,
}
struct Choice {
    key: &'static str,
    labels: Vec<String>,
    values: Vec<SettingValue>,
    list: List,
}
struct ValueEdit {
    key: &'static str,
    field: TextField,
    bounds: Rect,
    invoker: ViewId,
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
    retired_fields: Vec<TextField>,
    focus: bareline_ui::focus::FocusChain,
    pub localizer: config::Localizer,
    scope_user: Rect,
    scope_workspace: Rect,
    opt_in: Rect,
    reset: Rect,
    retry: Rect,
    revert: Rect,
}
impl SettingsController {
    pub fn label(&self, id: &str, fallback: &str) -> String {
        self.localizer.format(id, &[]).unwrap_or_else(|_| fallback.into())
    }
    pub fn status_description(&self) -> String {
        if let Some(error) = &self.error { return error.clone(); }
        match &self.current().status {
            SaveStatus::Saved => self.label("settings.saved", "All changes saved"),
            SaveStatus::Pending => self.label("settings.unsaved", "Changes not saved"),
            SaveStatus::Failed(reason) => format!("{}: {reason}", self.label("settings.unsaved", "Changes not saved")),
        }
    }
    fn reset_dialog_bounds(&self) -> Rect {
        let sidebar = 164.0_f32.min(self.bounds.width*0.26);
        rect(self.bounds.x+sidebar+38.0, self.bounds.y+120.0, (self.bounds.width-sidebar-76.0).max(100.0),112.0)
    }
    pub fn new(
        user: SettingsDocument,
        workspace: Option<SettingsDocument>,
        system: SystemAppearance,
    ) -> Self {
        let workspace_opted_in = config::resolve(&user, None, false, None)
            .values
            .workspace_preferences_enabled;
        let mut query = TextField::default();
        query.set_placeholder("Search settings");
        Self {
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
            retired_fields: Vec::new(),
            focus: Default::default(),
            localizer: Default::default(),
            scope_user: Rect::default(),
            scope_workspace: Rect::default(),
            opt_in: Rect::default(),
            reset: Rect::default(),
            retry: Rect::default(),
            revert: Rect::default(),
        }
    }
    pub fn configure_storage(
        &mut self,
        user: PathBuf,
        workspace: Option<PathBuf>,
        platform: Arc<dyn LocalFileSystem>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<()> {
        let (sender, jobs) = mpsc::sync_channel::<SaveJob>(1);
        let (completed, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("bareline-settings-save".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let prepare = if job.scope == Scope::Workspace { job.path.parent().map_or(Ok(()), std::fs::create_dir_all) } else { Ok(()) };
                    let error = prepare.and_then(|_| job.snapshot.save(&job.path, platform.as_ref()))
                        .err()
                        .map(|e| e.to_string());
                    if completed
                        .send(SaveCompletion {
                            scope: job.scope,
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
            active: false,
            pending: VecDeque::new(),
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
        if let Some(storage) = &mut self.storage {
            storage.workspace = Some(path);
            storage.pending.retain(|job| job.scope != Scope::Workspace);
        }
        self.workspace = Some(SettingsEditor::new(document));
        self.popup = None;
        if let Some(edit) = self.value_edit.take() { self.retired_fields.push(edit.field); self.focus.close_layer(); }
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
    pub fn saving(&self) -> bool {
        self.storage
            .as_ref()
            .is_some_and(|storage| storage.active || !storage.pending.is_empty())
    }
    pub fn edit(&mut self, key: &str, value: SettingValue) -> Result<(), String> {
        if self.scope == Scope::Workspace && self.workspace.is_none() {
            return Err("No workspace is open".into());
        }
        let previous = self.current().clone();
        self.current_mut().set(key, value)?;
        let effective = self.effective();
        if let Err(error) =
            config::Theme::resolve(effective.theme, self.system, &effective.theme_overrides)
        {
            *self.current_mut() = previous;
            self.error = Some(error.clone());
            return Err(error);
        }
        self.error = None;
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
        self.open = true;
        self.query_focused = true;
    }
    pub fn dismiss(&mut self) {
        self.open = false;
        self.popup = None;
        if let Some(edit) = self.value_edit.take() { self.retired_fields.push(edit.field); self.focus.close_layer(); }
        self.query.cancel();
    }
    pub fn revert_changes(&mut self) {
        self.current_mut().revert();
        self.error = None;
        self.popup = None;
        self.queue_save();
    }
    pub fn request_reset(&mut self) {
        if self.reset_pending { return; }
        self.reset_pending = true;
        self.focus.open_layer(ViewId(8003), [8011,8012].into_iter().map(|id| bareline_ui::focus::FocusTarget { id:ViewId(id),enabled:true }).collect());
    }
    pub fn confirm_reset(&mut self, confirmed: bool) {
        if !self.reset_pending { return; }
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
        self.current_mut().status = SaveStatus::Pending;
        let scope = self.scope;
        let snapshot = self.current().document.clone();
        let Some(storage) = self.storage.as_mut() else {
            self.current_mut().status =
                SaveStatus::Failed("Settings storage is not configured".into());
            return;
        };
        let path = if scope == Scope::Workspace {
            storage.workspace.clone()
        } else {
            Some(storage.user.clone())
        };
        let Some(path) = path else {
            self.current_mut().status =
                SaveStatus::Failed("Workspace settings path is unavailable".into());
            return;
        };
        storage.pending.retain(|job| job.scope != scope);
        storage.pending.push_back(SaveJob {
            scope,
            snapshot,
            path,
        });
        self.start_save();
    }
    fn start_save(&mut self) {
        if let Some(storage) = self.storage.as_mut() {
            if !storage.active {
                if let Some(job) = storage.pending.pop_front() {
                    match storage.sender.try_send(job) {
                        Ok(()) => storage.active = true,
                        Err(error) => {
                            self.error = Some(format!("Settings worker unavailable: {error}"));
                        }
                    }
                }
            }
        }
    }
    pub fn poll(&mut self) -> bool {
        let completion = self
            .storage
            .as_mut()
            .and_then(|storage| storage.receiver.try_recv().ok());
        let Some(completion) = completion else {
            return false;
        };
        if let Some(storage) = self.storage.as_mut() {
            storage.active = false;
        }
        let same_destination = self.storage.as_ref().is_some_and(|storage|
            if completion.scope == Scope::Workspace { storage.workspace.as_ref() == Some(&completion.path) } else { storage.user == completion.path });
        if !same_destination { self.start_save(); return true; }
        let editor = if completion.scope == Scope::Workspace {
            self.workspace.as_mut()
        } else {
            Some(&mut self.user)
        };
        if let Some(editor) = editor {
            match completion.error {
                None => editor.acknowledge_saved(completion.snapshot),
                Some(error) => {
                    if editor.document.to_toml() == completion.snapshot.to_toml() {
                        editor.status = SaveStatus::Failed(error);
                    }
                }
            }
        }
        self.start_save();
        true
    }
    pub fn release(&mut self, backend: &mut impl TextBackend) {
        self.query.release(backend);
        if let Some(edit) = &mut self.value_edit { edit.field.release(backend); }
        for mut field in self.retired_fields.drain(..) { field.release(backend); }
    }
    pub fn text_field_mut(&mut self) -> Option<&mut TextField> {
        if let Some(edit) = &mut self.value_edit { return (self.focus.focused() == Some(ViewId(8007))).then_some(&mut edit.field); }
        self.query_focused.then_some(&mut self.query)
    }
    pub fn text_changed(&mut self) {
        if let Some(edit) = &mut self.value_edit { edit.field.set_validation(None); }
        else { self.query_changed(); }
    }
    pub fn editing_value(&self) -> bool { self.value_edit.is_some() }
    pub fn focused_id(&self) -> Option<ViewId> {
        if !self.open { return None; }
        if self.reset_pending { return self.focus.focused(); }
        if self.value_edit.is_some() { return self.focus.focused(); }
        if let Some(popup) = &self.popup { return popup.list.selected.map(|i| ViewId(8500+i as u64)); }
        if self.query_focused { Some(ViewId(8000)) } else { self.focus.focused() }
    }
    pub fn traverse_focus(&mut self, backwards: bool) {
        if self.value_edit.is_some() { self.focus.traverse(backwards); return; }
        if self.reset_pending { self.focus.traverse(backwards); return; }
        if self.popup.is_some() { self.popup = None; }
        self.refresh_focus();
        if let Some(id) = self.focus.traverse(backwards) { self.accessibility_action(id.0, false); }
    }
    fn refresh_focus(&mut self) {
        let mut nodes = self.semantics();
        nodes.sort_by_key(|node| match node.id.0 { 8000 => (0,0),8001|8002|8006 => (1,node.id.0),8100..=8199 => (2,node.id.0),2000..=3999 => (3,node.id.0), _ => (4,node.id.0) });
        let targets = nodes.into_iter().filter(|node| node.actions.contains(&SemanticAction::Focus))
            .map(|node| bareline_ui::focus::FocusTarget { id: node.id, enabled: !node.disabled }).collect();
        self.focus.set_targets(targets);
        if self.query_focused { self.focus.focus(ViewId(8000)); }
    }
    fn begin_value_edit(&mut self, index: usize) -> Option<SettingsEffect> {
        let row = self.rows.get(index)?;
        let value = self.effective().setting_value(row.definition.key)?;
        let input = config::format_setting_input(&value);
        if input.len() > 16 * 1024 { return Some(SettingsEffect::OpenToml(self.scope)); }
        let mut field = TextField::default(); field.insert(&input); field.select_all();
        field.set_placeholder("Enter value · Enter applies · Escape cancels");
        self.value_edit = Some(ValueEdit { key: row.definition.key, field, bounds: row.value.bounds, invoker: row.value.id });
        self.focus.open_layer(row.value.id, [8007,8009,8010].into_iter().map(|id| bareline_ui::focus::FocusTarget { id: ViewId(id), enabled: true }).collect());
        self.query_focused = false;
        None
    }
    fn finish_value_edit(&mut self, commit: bool) -> Option<SettingsEffect> {
        let edit = self.value_edit.as_ref()?;
        if edit.field.composing() { return None; }
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
        if let Some(edit) = self.value_edit.take() { self.retired_fields.push(edit.field); }
        self.focus.close_layer();
        self.focus.focus(invoker);
        Some(SettingsEffect::PreviewChanged)
    }
    fn definitions(&self) -> Vec<&'static SettingDefinition> {
        let mut definitions: Vec<_> = config::search_definitions(self.query.value())
            .into_iter()
            .filter(|definition| {
                !self.query.value().is_empty() || definition.category == self.category
            })
            .collect();
        if self.query.value().is_empty() && self.category == "Editor" {
            let order = [
                "editor.font.family",
                "editor.font.size",
                "editor.wrap.mode",
                "editor.render.whitespace",
                "editor.tab.width",
                "editor.currentLine.highlight",
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
        match key {
            "document.resident_max_bytes" => effective.resident_max_bytes.to_string(),
            "transcode.temp_quota_bytes" => effective.transcode_quota_bytes.to_string(),
            "editor.font.family" => effective.editor_font_family,
            "editor.font.size" => format!("{} pt", effective.editor_font_size_pt),
            "editor.tab.width" => effective.tab_width.to_string(),
            "editor.wrap.mode" => if effective.word_wrap {
                "Viewport"
            } else {
                "Off"
            }
            .into(),
            "editor.render.whitespace" => effective.whitespace,
            "editor.currentLine.highlight" => on_off(effective.highlight_current_line),
            "editor.insert_spaces" => on_off(effective.insert_spaces),
            "editor.line_numbers" => on_off(effective.line_numbers),
            "toolbar.visible" => on_off(effective.toolbar_visible),
            "tabs.pinned_first" => on_off(effective.tabs_pinned_first),
            "theme.mode" => format!("{:?}", effective.theme),
            "renderer.mode" => format!("{:?}", effective.renderer),
            "language.locale" => effective.locale,
            _ => "Edit TOML…".into(),
        }
    }
    fn choose(&mut self, index: usize) -> Option<SettingsEffect> {
        let row = self.rows.get(index)?;
        let definition = row.definition;
        if self.scope == Scope::Workspace && !definition.workspace_allowed {
            self.error = Some("This setting is controlled by User scope".into());
            return None;
        }
        if matches!(definition.kind, SettingKind::Text | SettingKind::Integer(_, _) | SettingKind::Number(_, _) | SettingKind::Strings | SettingKind::Map) {
            return self.begin_value_edit(index);
        }
        let values: Vec<SettingValue> = match definition.kind {
            SettingKind::Boolean => vec![SettingValue::Bool(false), SettingValue::Bool(true)],
            SettingKind::Choice(choices) => choices
                .iter()
                .map(|v| SettingValue::Text((*v).into()))
                .collect(),
            SettingKind::Integer(min, max) if max.saturating_sub(min) <= 256 => {
                (min..=max).map(SettingValue::Integer).collect()
            }
            SettingKind::Integer(min, max) => {
                // Byte quotas must never allocate one choice per byte.
                let mut choices = vec![min, max];
                for power in 12..=40 {
                    let value = 1i64 << power;
                    if value >= min && value <= max {
                        choices.push(value);
                    }
                }
                if definition.key == "transcode.temp_quota_bytes" {
                    choices.push(21_474_836_480);
                }
                choices.sort_unstable();
                choices.dedup();
                choices.into_iter().map(SettingValue::Integer).collect()
            }
            SettingKind::Number(_, _) => [
                8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 16.0, 18.0, 20.0, 24.0, 32.0, 48.0, 72.0,
            ]
            .into_iter()
            .map(SettingValue::Number)
            .collect(),
            SettingKind::Text if definition.key == "editor.font.family" => {
                ["Cascadia Mono", "Consolas", "monospace"]
                    .into_iter()
                    .map(|v| SettingValue::Text(v.into()))
                    .collect()
            }
            SettingKind::Text if definition.key == "language.locale" => {
                vec![SettingValue::Text("en".into())]
            }
            _ => return Some(SettingsEffect::OpenToml(self.scope)),
        };
        let labels = values
            .iter()
            .map(|value| match value {
                SettingValue::Bool(v) => on_off(*v),
                SettingValue::Number(v) => format!("{v} pt"),
                SettingValue::Integer(v) => v.to_string(),
                SettingValue::Text(v) => v.clone(),
                _ => String::new(),
            })
            .collect::<Vec<_>>();
        let height = (labels.len().min(6) as f32) * 28.0;
        let y = (row.value.bounds.y + row.value.bounds.height)
            .min(self.bounds.y + self.bounds.height - height - 44.0)
            .max(self.bounds.y);
        self.popup = Some(Choice {
            key: definition.key,
            values,
            labels,
            list: List {
                bounds: rect(row.value.bounds.x, y, row.value.bounds.width, height),
                state: ControlState {
                    focused: true,
                    ..Default::default()
                },
                selected: Some(0),
                offset: 0.0,
                metrics: Metrics::COMPACT,
            },
        });
        None
    }
    pub fn event(&mut self, event: UiEvent) -> Option<SettingsEffect> {
        if self.value_edit.is_some() {
            return match event {
                UiEvent::Key(Key::Tab) => { self.traverse_focus(false); None },
                UiEvent::Key(Key::Enter | Key::Space) if self.focus.focused() == Some(ViewId(8010)) => self.finish_value_edit(false),
                UiEvent::Key(Key::Enter) => self.finish_value_edit(true),
                UiEvent::Key(Key::Space) if self.focus.focused() == Some(ViewId(8009)) => self.finish_value_edit(true),
                UiEvent::Key(Key::Escape) => {
                    if self.value_edit.as_ref().unwrap().field.composing() {
                        self.value_edit.as_mut().unwrap().field.cancel(); None
                    } else { self.finish_value_edit(false) }
                }
                UiEvent::Focus(false) => { self.value_edit.as_mut().unwrap().field.cancel(); None },
                UiEvent::PointerDown(point) => {
                    let bounds = self.value_edit.as_ref().unwrap().bounds;
                    if bounds.contains(point) { self.focus.focus(ViewId(8007)); }
                    else if rect(bounds.x, bounds.y + bounds.height + 24.0, 72.0, 28.0).contains(point) { return self.finish_value_edit(true); }
                    else if rect(bounds.x + 80.0, bounds.y + bounds.height + 24.0, 72.0, 28.0).contains(point) { return self.finish_value_edit(false); }
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
                    if let Some(node) = self.semantics().into_iter().find(|node| node.actions.contains(&SemanticAction::Invoke) && node.bounds.contains(point)) {
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
            if let Some(ControlAction::Selected(index)) = popup.list.event(event, &source) {
                if matches!(
                    event,
                    UiEvent::PointerUp(_) | UiEvent::Key(Key::Enter) | UiEvent::Key(Key::Space)
                ) {
                    let key = popup.key;
                    let value = popup.values[index].clone();
                    if let Err(error) = self.edit(key, value) {
                        self.error = Some(error);
                    }
                    return Some(SettingsEffect::PreviewChanged);
                }
            }
            if matches!(event, UiEvent::Key(Key::Enter) | UiEvent::Key(Key::Space))
                || matches!(event,UiEvent::PointerUp(point) if popup.list.bounds.contains(point))
            {
                if let Some(index) = popup.list.selected {
                    let key = popup.key;
                    let value = popup.values[index].clone();
                    if let Err(error) = self.edit(key, value) {
                        self.error = Some(error);
                    }
                    return Some(SettingsEffect::PreviewChanged);
                }
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
            if let Some(node) = self.semantics().into_iter().rev().find(|node| !node.disabled && node.bounds.contains(point)) {
                self.focus.focus(node.id);
            }
        }
        match event {
            UiEvent::PointerDown(point) => {
                self.query_focused = self.search_bounds.contains(point);
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
                if self.revert.contains(point) {
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
                if let Some(id) = self.focus.focused() { return self.accessibility_action(id.0, true); }
                return self.choose(self.selected.saturating_sub(self.first));
            }
            _ => {}
        }
        for index in 0..self.rows.len() {
            if self.rows[index].copy.event(event) == Some(ControlAction::Activated) {
                return Some(SettingsEffect::CopyKey(
                    self.rows[index].definition.key.into(),
                ));
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
        for mut field in self.retired_fields.drain(..) { field.release(backend); }
        self.query.set_placeholder(&self.label("settings.search", "Search settings"));
        self.bounds = bounds;
        let effective = self.effective();
        let theme =
            config::Theme::resolve(effective.theme, self.system, &effective.theme_overrides)
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
        let sidebar = 164.0_f32.min(bounds.width * 0.26);
        ops.push(DrawOp::Fill(
            rect(bounds.x, bounds.y, sidebar, bounds.height),
            chrome,
        ));
        for (index, category) in CATEGORIES.iter().enumerate() {
            let y = bounds.y + index as f32 * 38.0;
            if self.category == *category {
                ops.push(DrawOp::Fill(
                    rect(bounds.x, y, sidebar, 38.0),
                    color("surface.elevated"),
                ));
                ops.push(DrawOp::Fill(rect(bounds.x, y, 3.0, 38.0), focus));
            }
            text(ops, bounds.x + 18.0, y + 10.0, self.label(&format!("settings.category.{category}"), category), 13.0, foreground);
        }
        let x = bounds.x + sidebar + 18.0;
        let width = (bounds.width - sidebar - 36.0).max(1.0);
        self.search_bounds = rect(x, bounds.y + 12.0, width, 32.0);
        self.query.draw_with_theme(
            backend,
            self.search_bounds,
            self.query_focused,
            bareline_ui::theme::UiTheme::from_tokens(|key| {
                theme.color(key).map(|color| (color.rgb, color.alpha))
            })
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
                self.label(if scope == Scope::User { "settings.scope.user" } else { "settings.scope.workspace" }, label),
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
        text(ops, x, header, self.label("settings.effective", "Effective values"), 13.0, muted);
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
        let top = header + 30.0;
        let count = ((bounds.y + bounds.height - 60.0 - top) / row_height).max(0.0) as usize;
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
            text(ops, x + 10.0, y + 4.0, self.label(&format!("setting.{}.title",definition.key), definition.title), 13.0, foreground);
            text(ops, x + 10.0, y + 27.0, definition.key, 11.0, muted);
            if !compact {
                ops.push(DrawOp::PushClip(rect(
                    x + width * 0.29,
                    y,
                    width * 0.39,
                    row_height,
                )));
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
            ops.push(DrawOp::StrokeRounded(
                control,
                if self.selected == self.first + index && !self.query_focused {
                    focus
                } else {
                    color("border.interactive")
                },
                3.0,
                1.0,
            ));
            ops.push(DrawOp::PushClip(control));
            text(
                ops,
                control.x + 12.0,
                control.y + 8.0,
                value,
                13.0,
                if disabled { muted } else { foreground },
            );
            text(
                ops,
                control.x + control.width - 20.0,
                control.y + 8.0,
                "⌄",
                13.0,
                muted,
            );
            ops.push(DrawOp::PopClip);
            if definition.restart_required {
                text(ops, x + 10.0, y + 44.0, "Restart required", 10.0, muted);
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
                    id: ViewId(2000 + config::DEFINITIONS.iter().position(|d| d.key == definition.key).unwrap() as u64 * 2),
                    label: definition.title.into(),
                    bounds: control,
                    toggle: false,
                    state: ControlState { disabled, ..state },
                },
                copy: Button {
                    id: ViewId(2001 + config::DEFINITIONS.iter().position(|d| d.key == definition.key).unwrap() as u64 * 2),
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
        let status = self.status_description();
        ops.push(DrawOp::PushClip(rect(
            x,
            bottom,
            (width - 290.0).max(0.0),
            36.0,
        )));
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
            "Revert",
            12.0,
            foreground,
        );
        if matches!(self.current().status, SaveStatus::Failed(_)) {
            text(ops, self.retry.x, self.retry.y + 8.0, "Retry", 12.0, focus);
        }
        ops.push(DrawOp::StrokeRounded(
            self.reset,
            color("border.interactive"),
            3.0,
            1.0,
        ));
        text(
            ops,
            self.reset.x + 12.0,
            self.reset.y + 8.0,
            "Reset section",
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
                format!("Reset {} in {:?} settings?", self.category, self.scope),
                14.0,
                foreground,
            );
            text(
                ops,
                dialog.x + 12.0,
                dialog.y + 54.0,
                "Enter: reset section   Escape: cancel",
                13.0,
                muted,
            );
            for (id,label,x) in [(8011,"Reset",dialog.x+12.0),(8012,"Cancel",dialog.x+92.0)] {
                let button = rect(x,dialog.y+76.0,72.0,28.0);
                ops.push(DrawOp::StrokeRounded(button,if self.focus.focused()==Some(ViewId(id)) { focus } else {color("border.interactive")},4.0,1.0));
                text(ops,x+8.0,button.y+6.0,label,13.0,foreground);
            }
        }
        if let Some(popup) = &self.popup {
            let source = Labels(&popup.labels);
            ops.push(DrawOp::FillRounded(
                popup.list.bounds,
                color("surface.elevated"),
                4.0,
            ));
            ops.push(DrawOp::StrokeRounded(
                popup.list.bounds,
                color("border.interactive"),
                4.0,
                1.0,
            ));
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
        if let Some(edit) = &mut self.value_edit {
            if let Some(row) = self.rows.iter().find(|row| row.definition.key == edit.key) { edit.bounds = row.value.bounds; }
            let panel = rect(edit.bounds.x - 4.0, edit.bounds.y - 4.0, edit.bounds.width + 8.0, edit.bounds.height + 64.0);
            ops.push(DrawOp::FillRounded(panel, color("surface.elevated"), 4.0));
            ops.push(DrawOp::StrokeRounded(panel, color("border.interactive"), 4.0, 1.0));
            edit.field.draw_with_theme(backend, edit.bounds, self.focus.focused() == Some(ViewId(8007)),
                bareline_ui::theme::UiTheme::from_tokens(|key| theme.color(key).map(|c| (c.rgb,c.alpha))).unwrap(), ops)?;
            if let Some(reason) = edit.field.validation() {
                ops.push(DrawOp::PushClip(rect(edit.bounds.x, edit.bounds.y + edit.bounds.height, edit.bounds.width, 22.0)));
                text(ops, edit.bounds.x, edit.bounds.y + edit.bounds.height + 2.0, reason, 11.0, color("danger"));
                ops.push(DrawOp::PopClip);
            }
            for (id, label, x) in [(8009,"Apply",edit.bounds.x),(8010,"Cancel",edit.bounds.x+80.0)] {
                let bounds = rect(x, edit.bounds.y + edit.bounds.height + 24.0, 72.0, 28.0);
                ops.push(DrawOp::StrokeRounded(bounds, if self.focus.focused() == Some(ViewId(id)) { focus } else { color("border.interactive") }, 4.0, 1.0));
                text(ops, x+8.0, bounds.y+6.0, label, 13.0, foreground);
            }
        }
        if self.value_edit.is_none() && self.popup.is_none() && !self.reset_pending {
            if let Some(id) = self.focused_id().filter(|id| *id != ViewId(8000)) {
                if let Some(node) = self.semantics().into_iter().find(|node| node.id == id) { ops.push(DrawOp::Stroke(node.bounds,focus,2.0)); }
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
            return [(8011,"Reset section",dialog.x+12.0),(8012,"Cancel reset",dialog.x+92.0)].into_iter().map(|(id,name,x)|
                Semantics::new(ViewId(id),SemanticRole::Button,name,"settings.reset_section",rect(x,dialog.y+76.0,72.0,28.0),
                    ControlState { focused:self.focus.focused()==Some(ViewId(id)), ..Default::default() })
                    .action(SemanticAction::Focus).action(SemanticAction::Invoke)).collect();
        }
        if let Some(edit) = &self.value_edit {
            let name = config::DEFINITIONS.iter().find(|d| d.key == edit.key).map_or(edit.key, |d| d.title);
            let mut nodes = vec![edit.field.semantics(ViewId(8007), name, "settings.edit_value", edit.bounds,
                ControlState { focused: self.focus.focused() == Some(ViewId(8007)), ..Default::default() })];
            for (id,label,x) in [(8009,"Apply value",edit.bounds.x),(8010,"Cancel value edit",edit.bounds.x+80.0)] {
                nodes.push(Semantics::new(ViewId(id),SemanticRole::Button,label,"settings.edit_value",
                    rect(x,edit.bounds.y+edit.bounds.height+24.0,72.0,28.0),
                    ControlState { focused:self.focus.focused()==Some(ViewId(id)), ..Default::default() })
                    .action(SemanticAction::Focus).action(SemanticAction::Invoke));
            }
            return nodes;
        }
        let mut nodes = vec![self.query.semantics(
            ViewId(8000),
            "Search settings",
            "settings.search",
            self.search_bounds,
            ControlState {
                focused: self.query_focused && self.popup.is_none(),
                ..Default::default()
            },
        )];
        for (id, label, command, bounds) in [
            (
                8001,
                "User settings",
                "settings.scope_user",
                self.scope_user,
            ),
            (
                8002,
                "Workspace settings",
                "settings.scope_workspace",
                self.scope_workspace,
            ),
            (8003, "Reset section", "settings.reset_section", self.reset),
            (8004, "Revert changes", "settings.revert", self.revert),
        ] {
            nodes.push(
                Semantics::new(
                    ViewId(id),
                    SemanticRole::Button,
                    label,
                    command,
                    bounds,
                    ControlState::default(),
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke),
            );
        }
        if self.error.is_some() {
            nodes.push(
                Semantics::new(
                    ViewId(8005),
                    SemanticRole::Button,
                    "Retry saving",
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
                category,
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
                    "Enable workspace preferences",
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
        for row in &self.rows {
            let mut value = row.value.semantic().into_settings(row.value.bounds);
            value.name = self.label(&format!("setting.{}.title",row.definition.key), row.definition.title);
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
                if let Some(index) = id
                    .checked_sub(8500)
                    .filter(|i| *i < popup.labels.len() as u64)
                {
                    popup.list.selected = Some(index as usize);
                }
            }
            return None;
        }
        if self.reset_pending { self.confirm_reset(id == 8011); return Some(SettingsEffect::PreviewChanged); }
        if self.value_edit.is_some() {
            return match id { 8009 => self.finish_value_edit(true), 8010 => self.finish_value_edit(false), _ => None };
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
mod visual_contract_tests {
    use super::*;
    use bareline_renderer_recording::RecordingBackend;
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
            controller
                .rows
                .iter()
                .map(|row| row.definition.key)
                .collect::<Vec<_>>(),
            [
                "editor.font.family",
                "editor.font.size",
                "editor.wrap.mode",
                "editor.render.whitespace",
                "editor.tab.width",
                "editor.currentLine.highlight"
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
        controller.choose(2);
        let popup_bounds = controller.popup.as_ref().unwrap().list.bounds;
        ops.clear();
        controller
            .draw(rect(0.0, 34.0, 1200.0, 660.0), &mut backend, &mut ops)
            .unwrap();
        assert!(
            ops.iter().any(
                |op| matches!(op, DrawOp::FillRounded(bounds, _, _) if *bounds == popup_bounds)
            )
        );
        assert!(bareline_renderer::balanced_clips(&ops));
    }
}
