// SPDX-License-Identifier: MPL-2.0
use crate::{RendererMode, ThemeMode};
use std::collections::BTreeMap;
use toml_edit::{DocumentMut, Item, Table, Value};
pub const MAX_CONFIG_BYTES: usize = 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    Workspace,
    Session,
}
#[derive(Clone, Debug, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    Integer(i64),
    Number(f64),
    Text(String),
    Strings(Vec<String>),
    Map(BTreeMap<String, String>),
}
#[derive(Clone, Copy, Debug)]
pub enum SettingKind {
    Boolean,
    Integer(i64, i64),
    Number(f64, f64),
    Text,
    Choice(&'static [&'static str]),
    Strings,
    Map,
}
#[derive(Clone, Copy, Debug)]
pub struct SettingDefinition {
    pub key: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub description_id: &'static str,
    pub category: &'static str,
    pub kind: SettingKind,
    pub restart_required: bool,
    pub workspace_allowed: bool,
}
macro_rules! setting {
    ($key:literal, $title:literal, $description:literal, $category:literal, $kind:expr, $restart:expr, $workspace:expr) => {
        SettingDefinition {
            key: $key,
            title: $title,
            description: $description,
            description_id: concat!("setting.", $key),
            category: $category,
            kind: $kind,
            restart_required: $restart,
            workspace_allowed: $workspace,
        }
    };
}
pub static DEFINITIONS: &[SettingDefinition] = &[
    setting!("language.policies", "Language behavior", "Per-language overrides: language ID and field, for example rust.lexer = native or rust.min_chars = 2. Values are strings.", "Language", SettingKind::Map, false, false),
    setting!(
        "document.resident_max_bytes",
        "Resident document limit",
        "Maximum bytes opened in memory; larger files use paged storage. Applies to newly opened files.",
        "Advanced",
        SettingKind::Integer(4096, 1_099_511_627_776),
        false,
        false
    ),
    setting!(
        "transcode.temp_quota_bytes",
        "Transcode temporary storage limit",
        "Maximum disk bytes used for temporary transcoding, also limited by available disk space. Applies to newly opened files.",
        "Advanced",
        SettingKind::Integer(4096, 1_099_511_627_776),
        false,
        false
    ),
    setting!(
        "session.restore",
        "Restore previous session",
        "Restore Bareline's own saved local session at startup.",
        "Files",
        SettingKind::Boolean,
        false,
        false
    ),
    setting!(
        "workspace.preferences_enabled",
        "Enable workspace preferences",
        "Allow only display, indentation, language associations and search exclusions from this workspace.",
        "Advanced",
        SettingKind::Boolean,
        false,
        false
    ),
    setting!(
        "editor.font.size",
        "Font size",
        "Editor font size in points; 12 pt equals 16 logical pixels.",
        "Editor",
        SettingKind::Number(6.0, 72.0),
        false,
        true
    ),
    setting!(
        "editor.font.family",
        "Font family",
        "Monospace font family, resolved with system fallback.",
        "Editor",
        SettingKind::Text,
        false,
        true
    ),
    setting!(
        "editor.tab.width",
        "Tab width",
        "Columns occupied by a tab character.",
        "Editor",
        SettingKind::Integer(1, 16),
        false,
        true
    ),
    setting!(
        "editor.insert_spaces",
        "Insert spaces",
        "Insert spaces when the Tab key indents text.",
        "Advanced",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "editor.wrap.mode",
        "Word wrap",
        "Wrap long lines within the visible editor.",
        "Editor",
        SettingKind::Choice(&["off", "viewport"]),
        false,
        true
    ),
    setting!(
        "editor.render.whitespace",
        "Show whitespace",
        "Show whitespace characters in the selection or throughout the editor.",
        "Editor",
        SettingKind::Choice(&["none", "selection", "all"]),
        false,
        true
    ),
    setting!(
        "editor.currentLine.highlight",
        "Highlight current line",
        "Highlight the background of the current line.",
        "Editor",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "editor.line_numbers",
        "Line numbers",
        "Show the line-number gutter.",
        "Advanced",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "theme.mode",
        "Color mode",
        "Use system appearance, light, or dark colors.",
        "Appearance",
        SettingKind::Choice(&["system", "light", "dark"]),
        false,
        false
    ),
    setting!(
        "theme.overrides",
        "Theme color overrides",
        "Semantic token colors, validated for functional contrast.",
        "Appearance",
        SettingKind::Map,
        false,
        false
    ),
    setting!(
        "toolbar.visible",
        "Show toolbar",
        "Show the optional command toolbar.",
        "Appearance",
        SettingKind::Boolean,
        false,
        false
    ),
    setting!(
        "toolbar.commands",
        "Toolbar commands",
        "Ordered stable command IDs for the optional toolbar.",
        "Appearance",
        SettingKind::Strings,
        false,
        false
    ),
    setting!(
        "tabs.pinned_first",
        "Pinned tabs first",
        "Keep pinned documents at the start of the tab strip.",
        "Appearance",
        SettingKind::Boolean,
        false,
        false
    ),
    setting!(
        "language.locale",
        "Display language",
        "Changes apply to custom UI immediately; rebuild native menu labels.",
        "Language",
        SettingKind::Text,
        false,
        false
    ),
    setting!(
        "language.associations",
        "Language associations",
        "Map file patterns to language IDs.",
        "Language",
        SettingKind::Map,
        false,
        true
    ),
    setting!(
        "search.excludes",
        "Search exclusions",
        "File patterns excluded from workspace search.",
        "Search",
        SettingKind::Strings,
        false,
        true
    ),
    setting!(
        "renderer.mode",
        "Renderer",
        "Hardware or software rendering; restart required.",
        "Advanced",
        SettingKind::Choice(&["hardware", "software"]),
        true,
        false
    ),
];
pub fn search_definitions(query: &str) -> Vec<&'static SettingDefinition> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    DEFINITIONS
        .iter()
        .filter(|definition| {
            let haystack = format!(
                "{} {} {} {}",
                definition.key, definition.title, definition.description, definition.category
            )
            .to_lowercase();
            words.iter().all(|word| haystack.contains(word))
        })
        .collect()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub key: String,
    pub message: String,
}
#[derive(Clone, Debug)]
pub struct SettingsDocument {
    pub(crate) document: DocumentMut,
    pub scope: Scope,
}
impl SettingsDocument {
    pub fn empty(scope: Scope) -> Self {
        let mut document = DocumentMut::new();
        document["schema_version"] = toml_edit::value(1);
        Self { document, scope }
    }
    pub fn parse(bytes: &[u8], scope: Scope) -> Result<Self, String> {
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err("Settings exceed 1 MiB".into());
        }
        let text = std::str::from_utf8(bytes).map_err(|_| "Settings must be UTF-8")?;
        let mut document = text
            .parse::<DocumentMut>()
            .map_err(|error| error.to_string())?;
        match document.get("schema_version").and_then(Item::as_integer) {
            None if document.get("schema_version").is_none() => {
                document["schema_version"] = toml_edit::value(1);
            }
            Some(0) => {
                // Version 0's explicitly pixel-named value migrates to points. Unknown keys survive.
                if let Some(size) = lookup(&document, "editor.font_size_px").and_then(number)
                    && lookup(&document, "editor.font.size").is_none()
                {
                    put(
                        &mut document,
                        "editor.font.size",
                        SettingValue::Number(size * 72.0 / 96.0),
                    )?;
                }
                document["schema_version"] = toml_edit::value(1);
            }
            Some(1) => {}
            _ => return Err("Unsupported settings schema version".into()),
        }
        Ok(Self { document, scope })
    }
    pub fn to_toml(&self) -> String {
        self.document.to_string()
    }
    pub fn set(&mut self, key: &str, value: SettingValue) -> Result<(), String> {
        let key = canonical_key(key);
        let value = if key == "editor.wrap.mode" {
            match value {
                SettingValue::Bool(v) => {
                    SettingValue::Text(if v { "viewport" } else { "off" }.into())
                }
                v => v,
            }
        } else {
            value
        };
        let definition = DEFINITIONS
            .iter()
            .find(|definition| definition.key == key)
            .ok_or("Unknown setting")?;
        if self.scope == Scope::Workspace && !definition.workspace_allowed {
            return Err("Workspace cannot override this user setting".into());
        }
        validate_value(definition, &value)?;
        let mut next = self.document.clone();
        put(&mut next, key, value)?;
        if next.to_string().len() > MAX_CONFIG_BYTES {
            return Err("Settings exceed 1 MiB".into());
        }
        self.document = next;
        Ok(())
    }
    pub fn reset_section(&mut self, category: &str) -> Vec<&'static str> {
        let keys: Vec<_> = DEFINITIONS
            .iter()
            .filter(|d| {
                d.category == category && (self.scope != Scope::Workspace || d.workspace_allowed)
            })
            .map(|d| d.key)
            .collect();
        for key in &keys {
            remove_key(
                self.document.as_table_mut(),
                &key.split('.').collect::<Vec<_>>(),
            );
            for (legacy, canonical) in ALIASES {
                if canonical == key {
                    remove_key(
                        self.document.as_table_mut(),
                        &legacy.split('.').collect::<Vec<_>>(),
                    );
                }
            }
        }
        keys
    }
    pub fn values(&self) -> (BTreeMap<String, SettingValue>, Vec<Diagnostic>) {
        let mut values = BTreeMap::new();
        let mut diagnostics = Vec::new();
        for definition in DEFINITIONS {
            let Some(item) = lookup(&self.document, definition.key) else {
                continue;
            };
            let parsed = parse_value(definition, item).and_then(|value| {
                if self.scope == Scope::Workspace && !definition.workspace_allowed {
                    return Err("Workspace cannot grant or override user policy".into());
                }
                validate_value(definition, &value)?;
                Ok(value)
            });
            match parsed {
                Ok(value) => {
                    values.insert(definition.key.into(), value);
                }
                Err(message) => diagnostics.push(Diagnostic {
                    key: definition.key.into(),
                    message,
                }),
            }
        }
        // Reject every unknown workspace leaf, including process/network/extension grants.
        if self.scope == Scope::Workspace {
            let mut leaves = Vec::new();
            collect_leaves(self.document.as_table(), "", &mut leaves);
            for key in leaves {
                if key != "schema_version"
                    && !DEFINITIONS.iter().any(|d| {
                        canonical_key(&key) == d.key
                            || (matches!(d.kind, SettingKind::Map)
                                && key.starts_with(&format!("{}.", d.key)))
                    })
                {
                    diagnostics.push(Diagnostic {
                        key,
                        message: "Workspace setting is outside the preference allowlist".into(),
                    });
                }
            }
        }
        // A malformed section does not hide diagnostics for every affected key.
        for section in [
            "editor", "theme", "toolbar", "tabs", "language", "search", "renderer",
        ] {
            if self
                .document
                .get(section)
                .is_some_and(|item| item.as_table_like().is_none())
            {
                diagnostics.push(Diagnostic {
                    key: section.into(),
                    message: "Expected a settings table".into(),
                });
            }
        }
        (values, diagnostics)
    }
}
fn collect_leaves(table: &dyn toml_edit::TableLike, prefix: &str, result: &mut Vec<String>) {
    for (key, item) in table.iter() {
        let path = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };
        if let Some(table) = item.as_table_like() {
            collect_leaves(table, &path, result);
        } else {
            result.push(path);
        }
    }
}
fn lookup<'a>(document: &'a DocumentMut, key: &str) -> Option<&'a Item> {
    lookup_exact(document, key).or_else(|| {
        ALIASES
            .iter()
            .find(|(_, canonical)| *canonical == key)
            .and_then(|(legacy, _)| lookup_exact(document, legacy))
    })
}
const ALIASES: &[(&str, &str)] = &[
    ("editor.font_size_pt", "editor.font.size"),
    ("editor.font_family", "editor.font.family"),
    ("editor.tab_width", "editor.tab.width"),
    ("editor.word_wrap", "editor.wrap.mode"),
];
fn canonical_key(key: &str) -> &str {
    ALIASES
        .iter()
        .find(|(legacy, _)| *legacy == key)
        .map_or(key, |(_, canonical)| canonical)
}
fn lookup_exact<'a>(document: &'a DocumentMut, key: &str) -> Option<&'a Item> {
    let mut parts = key.split('.');
    let mut item = document.get(parts.next()?)?;
    for part in parts {
        item = item.as_table_like()?.get(part)?;
    }
    Some(item)
}
fn remove_key(table: &mut dyn toml_edit::TableLike, path: &[&str]) {
    if path.len() == 1 {
        table.remove(path[0]);
    } else if let Some(child) = table.get_mut(path[0]).and_then(Item::as_table_like_mut) {
        remove_key(child, &path[1..]);
    }
}
fn number(item: &Item) -> Option<f64> {
    item.as_float()
        .or_else(|| item.as_integer().map(|n| n as f64))
}
/// Parse the single-line value editor using the same schema as persisted TOML.
/// Text and choices are plain text; arrays/maps use TOML value syntax.
pub fn parse_setting_input(key: &str, input: &str) -> Result<SettingValue, String> {
    if input.len() > 16 * 1024 { return Err("Value exceeds the inline editor limit; edit the TOML file".into()); }
    let definition = DEFINITIONS.iter().find(|d| d.key == key).ok_or("Unknown setting")?;
    let value = match definition.kind {
        SettingKind::Text | SettingKind::Choice(_) => SettingValue::Text(input.to_owned()),
        _ => {
            let document = format!("value = {input}").parse::<DocumentMut>().map_err(|_| "Enter a valid TOML value")?;
            if document.len() != 1 { return Err("Enter one value only".into()); }
            parse_value(definition, document.get("value").ok_or("Missing value")?)?
        }
    };
    validate_value(definition, &value)?;
    Ok(value)
}
pub fn format_setting_input(value: &SettingValue) -> String {
    match value {
        SettingValue::Text(value) => value.clone(),
        SettingValue::Bool(value) => value.to_string(),
        SettingValue::Integer(value) => value.to_string(),
        SettingValue::Number(value) => value.to_string(),
        SettingValue::Strings(values) => {
            let mut array = toml_edit::Array::new();
            for value in values { array.push(value.as_str()); }
            array.to_string()
        }
        SettingValue::Map(values) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in values { table.insert(key, Value::from(value.as_str())); }
            table.to_string()
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexerPreference { Primary, Native }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguagePolicy {
    pub lexer: LexerPreference,
    pub completion: bool,
    pub min_chars: u8,
    pub include_open_documents: bool,
    pub smart_pairs: bool,
    pub smart_indent: bool,
    pub parameter_hints: bool,
}
impl Default for LanguagePolicy {
    fn default() -> Self { Self { lexer: LexerPreference::Primary, completion: true, min_chars: 0,
        include_open_documents: true, smart_pairs: true, smart_indent: true, parameter_hints: true } }
}
fn validate_language_policy(key: &str, value: &str) -> Result<(), String> {
    let (language, field) = key.rsplit_once('.').ok_or("Use language-id.field for a policy key")?;
    if language.is_empty() || language.len() > 64 || !language.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)) {
        return Err("Invalid stable language ID".into());
    }
    let valid = match field {
        "lexer" => matches!(value, "primary" | "native"),
        "min_chars" => value.parse::<u8>().is_ok_and(|n| n <= 16),
        "completion" | "include_open_documents" | "smart_pairs" | "smart_indent" | "parameter_hints" => matches!(value, "true" | "false"),
        _ => false,
    };
    if valid { Ok(()) } else { Err(format!("Invalid language policy {key}")) }
}
impl EffectiveSettings {
    pub fn setting_value(&self, key: &str) -> Option<SettingValue> {
        Some(match key {
            "session.restore" => SettingValue::Bool(self.restore_session),
            "workspace.preferences_enabled" => SettingValue::Bool(self.workspace_preferences_enabled),
            "document.resident_max_bytes" => SettingValue::Integer(self.resident_max_bytes as i64),
            "transcode.temp_quota_bytes" => SettingValue::Integer(self.transcode_quota_bytes as i64),
            "editor.font.family" => SettingValue::Text(self.editor_font_family.clone()),
            "editor.font.size" => SettingValue::Number(self.editor_font_size_pt),
            "editor.tab.width" => SettingValue::Integer(self.tab_width as i64),
            "editor.insert_spaces" => SettingValue::Bool(self.insert_spaces),
            "editor.wrap.mode" => SettingValue::Text(if self.word_wrap { "viewport" } else { "off" }.into()),
            "editor.line_numbers" => SettingValue::Bool(self.line_numbers),
            "editor.render.whitespace" => SettingValue::Text(self.whitespace.clone()),
            "editor.currentLine.highlight" => SettingValue::Bool(self.highlight_current_line),
            "theme.mode" => SettingValue::Text(match self.theme { ThemeMode::System => "system", ThemeMode::Light => "light", ThemeMode::Dark => "dark" }.into()),
            "theme.overrides" => SettingValue::Map(self.theme_overrides.clone()),
            "toolbar.visible" => SettingValue::Bool(self.toolbar_visible),
            "toolbar.commands" => SettingValue::Strings(self.toolbar_commands.clone()),
            "tabs.pinned_first" => SettingValue::Bool(self.tabs_pinned_first),
            "language.locale" => SettingValue::Text(self.locale.clone()),
            "language.associations" => SettingValue::Map(self.language_associations.clone()),
            "language.policies" => SettingValue::Map(self.language_policies.clone()),
            "search.excludes" => SettingValue::Strings(self.search_excludes.clone()),
            "renderer.mode" => SettingValue::Text(match self.renderer { RendererMode::Hardware => "hardware", RendererMode::Software => "software" }.into()),
            _ => return None,
        })
    }
    pub fn language_policy(&self, stable_id: &str) -> LanguagePolicy {
        let mut policy = LanguagePolicy::default();
        let get = |field: &str| self.language_policies.get(&format!("{stable_id}.{field}")).map(String::as_str);
        if get("lexer") == Some("native") { policy.lexer = LexerPreference::Native; }
        policy.min_chars = get("min_chars").and_then(|n| n.parse().ok()).filter(|n| *n <= 16).unwrap_or(0);
        for (field, target) in [("completion", &mut policy.completion), ("include_open_documents", &mut policy.include_open_documents),
            ("smart_pairs", &mut policy.smart_pairs), ("smart_indent", &mut policy.smart_indent), ("parameter_hints", &mut policy.parameter_hints)] {
            if let Some(value) = get(field) { *target = value == "true"; }
        }
        policy
    }
}
fn parse_value(definition: &SettingDefinition, item: &Item) -> Result<SettingValue, String> {
    if definition.key == "editor.wrap.mode"
        && let Some(value) = item.as_bool()
    {
        return Ok(SettingValue::Text(
            if value { "viewport" } else { "off" }.into(),
        ));
    }
    let invalid = || "Invalid setting type; using the lower-priority value".to_string();
    Ok(match definition.kind {
        SettingKind::Boolean => SettingValue::Bool(item.as_bool().ok_or_else(invalid)?),
        SettingKind::Integer(_, _) => SettingValue::Integer(item.as_integer().ok_or_else(invalid)?),
        SettingKind::Number(_, _) => SettingValue::Number(number(item).ok_or_else(invalid)?),
        SettingKind::Text | SettingKind::Choice(_) => {
            SettingValue::Text(item.as_str().ok_or_else(invalid)?.into())
        }
        SettingKind::Strings => SettingValue::Strings(
            item.as_array()
                .ok_or_else(invalid)?
                .iter()
                .map(|value| value.as_str().map(str::to_owned).ok_or_else(invalid))
                .collect::<Result<_, _>>()?,
        ),
        SettingKind::Map => SettingValue::Map(
            item.as_table_like()
                .ok_or_else(invalid)?
                .iter()
                .map(|(key, value)| {
                    value
                        .as_str()
                        .map(|v| (key.to_owned(), v.to_owned()))
                        .ok_or_else(invalid)
                })
                .collect::<Result<_, _>>()?,
        ),
    })
}
fn validate_value(definition: &SettingDefinition, value: &SettingValue) -> Result<(), String> {
    if definition.key == "language.locale" {
        if let SettingValue::Text(locale) = value {
            if locale.is_empty() || locale.len() > 64 || !locale.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                return Err("Locale must be a language ID containing letters, digits or hyphens".into());
            }
        }
    }
    if definition.key == "language.policies" {
        if let SettingValue::Map(values) = value {
            if values.len() > 512 { return Err("At most 512 language policy entries are allowed".into()); }
            for (key, value) in values { validate_language_policy(key, value)?; }
        }
    }
    let valid = match (definition.kind, value) {
        (SettingKind::Boolean, SettingValue::Bool(_)) => true,
        (SettingKind::Integer(min, max), SettingValue::Integer(n)) => (min..=max).contains(n),
        (SettingKind::Number(min, max), SettingValue::Number(n)) => {
            n.is_finite() && (min..=max).contains(n)
        }
        (SettingKind::Text, SettingValue::Text(text)) => {
            !text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control)
        }
        (SettingKind::Choice(choices), SettingValue::Text(text)) => {
            choices.contains(&text.as_str())
        }
        (SettingKind::Strings, SettingValue::Strings(values)) => {
            values.len() <= 256 && values.iter().all(|v| v.len() <= 1024 && !v.contains('\0'))
        }
        (SettingKind::Map, SettingValue::Map(values)) => {
            values.len() <= 256
                && values.iter().all(|(k, v)| {
                    !k.is_empty()
                        && k.len() <= 256
                        && v.len() <= 1024
                        && !k.contains('\0')
                        && !v.contains('\0')
                })
        }
        _ => false,
    };
    if !valid {
        return Err("Value is outside the permitted type or range".into());
    }
    if definition.key == "toolbar.commands"
        && let SettingValue::Strings(values) = value
    {
        let mut unique = std::collections::BTreeSet::new();
        if values.len() > 32
            || values.iter().any(|id| {
                id.is_empty()
                    || id.len() > 128
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                    || !unique.insert(id)
            })
        {
            return Err("Toolbar requires at most 32 unique stable command IDs".into());
        }
    }
    if definition.key == "theme.overrides"
        && let SettingValue::Map(values) = value
    {
        for (key, value) in values {
            if !crate::TOKEN_NAMES.contains(&key.as_str()) {
                return Err(format!("Unknown theme token: {key}"));
            }
            crate::ThemeColor::parse(value)?;
        }
    }
    Ok(())
}
fn put(document: &mut DocumentMut, key: &str, value: SettingValue) -> Result<(), String> {
    let mut parts = key.split('.').collect::<Vec<_>>();
    let key = parts.pop().ok_or("Invalid setting key")?;
    let mut table: &mut dyn toml_edit::TableLike = document.as_table_mut();
    for section in parts {
        if table.get(section).is_none() {
            table.insert(section, Item::Table(Table::new()));
        }
        table = table
            .get_mut(section)
            .and_then(Item::as_table_like_mut)
            .ok_or("Section is not a table; repair it before editing")?;
    }
    let mut item = match value {
        SettingValue::Bool(v) => toml_edit::value(v),
        SettingValue::Integer(v) => toml_edit::value(v),
        SettingValue::Number(v) => toml_edit::value(v),
        SettingValue::Text(v) => toml_edit::value(v),
        SettingValue::Strings(values) => Item::Value(Value::Array(values.into_iter().collect())),
        SettingValue::Map(values) => {
            let mut table = Table::new();
            for (key, value) in values {
                table.insert(&key, toml_edit::value(value));
            }
            Item::Table(table)
        }
    };
    if let (Some(old), Some(new)) = (table.get(key).and_then(Item::as_value), item.as_value_mut()) {
        *new.decor_mut() = old.decor().clone();
    }
    table.insert(key, item);
    Ok(())
}
#[derive(Clone, Debug, PartialEq)]
pub struct EffectiveSettings {
    pub language_policies: BTreeMap<String, String>,
    pub resident_max_bytes: u64,
    pub transcode_quota_bytes: u64,
    pub restore_session: bool,
    pub workspace_preferences_enabled: bool,
    pub renderer: RendererMode,
    pub editor_font_size_pt: f64,
    pub editor_font_family: String,
    pub theme: ThemeMode,
    pub theme_overrides: BTreeMap<String, String>,
    pub tab_width: u8,
    pub insert_spaces: bool,
    pub word_wrap: bool,
    pub line_numbers: bool,
    pub whitespace: String,
    pub highlight_current_line: bool,
    pub toolbar_visible: bool,
    pub toolbar_commands: Vec<String>,
    pub tabs_pinned_first: bool,
    pub locale: String,
    pub language_associations: BTreeMap<String, String>,
    pub search_excludes: Vec<String>,
}
impl Default for EffectiveSettings {
    fn default() -> Self {
        Self {
            language_policies: BTreeMap::new(),
            resident_max_bytes: 268_435_456,
            transcode_quota_bytes: 21_474_836_480,
            restore_session: true,
            workspace_preferences_enabled: false,
            renderer: RendererMode::Hardware,
            editor_font_size_pt: 12.0,
            editor_font_family: "Cascadia Mono".into(),
            theme: ThemeMode::System,
            theme_overrides: BTreeMap::new(),
            tab_width: 4,
            insert_spaces: true,
            word_wrap: false,
            line_numbers: true,
            whitespace: "selection".into(),
            highlight_current_line: true,
            toolbar_visible: false,
            toolbar_commands: [
                "file.new",
                "file.open",
                "file.save",
                "edit.undo",
                "search.find",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            tabs_pinned_first: true,
            locale: "en".into(),
            language_associations: BTreeMap::new(),
            search_excludes: Vec::new(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct ResolvedSettings {
    pub values: EffectiveSettings,
    pub diagnostics: Vec<Diagnostic>,
}
/// Workspace is ignored unless explicitly opted in. Session overrides are never persisted.
pub fn resolve(
    user: &SettingsDocument,
    workspace: Option<&SettingsDocument>,
    workspace_opted_in: bool,
    session: Option<&SettingsDocument>,
) -> ResolvedSettings {
    let mut resolved = ResolvedSettings {
        values: EffectiveSettings::default(),
        diagnostics: Vec::new(),
    };
    for document in std::iter::once(user)
        .chain(workspace.filter(|_| workspace_opted_in))
        .chain(session)
    {
        let mut scoped = document.clone();
        // Caller cannot bypass workspace restrictions by supplying a user-scoped object.
        if workspace_opted_in && workspace.is_some_and(|w| std::ptr::eq(w, document)) {
            scoped.scope = Scope::Workspace;
        }
        let (values, diagnostics) = scoped.values();
        resolved.diagnostics.extend(diagnostics);
        for (key, value) in values {
            apply(&mut resolved.values, &key, value);
        }
    }
    resolved
}
fn apply(settings: &mut EffectiveSettings, key: &str, value: SettingValue) {
    match (key, value) {
        ("language.policies", SettingValue::Map(v)) => settings.language_policies.extend(v),
        ("document.resident_max_bytes", SettingValue::Integer(v)) => {
            settings.resident_max_bytes = v as u64
        }
        ("transcode.temp_quota_bytes", SettingValue::Integer(v)) => {
            settings.transcode_quota_bytes = v as u64
        }
        ("session.restore", SettingValue::Bool(v)) => settings.restore_session = v,
        ("workspace.preferences_enabled", SettingValue::Bool(v)) => {
            settings.workspace_preferences_enabled = v
        }
        ("editor.font.size", SettingValue::Number(v)) => settings.editor_font_size_pt = v,
        ("editor.font.family", SettingValue::Text(v)) => settings.editor_font_family = v,
        ("editor.tab.width", SettingValue::Integer(v)) => settings.tab_width = v as u8,
        ("editor.insert_spaces", SettingValue::Bool(v)) => settings.insert_spaces = v,
        ("editor.wrap.mode", SettingValue::Text(v)) => settings.word_wrap = v == "viewport",
        ("editor.render.whitespace", SettingValue::Text(v)) => settings.whitespace = v,
        ("editor.currentLine.highlight", SettingValue::Bool(v)) => {
            settings.highlight_current_line = v
        }
        ("editor.line_numbers", SettingValue::Bool(v)) => settings.line_numbers = v,
        ("theme.mode", SettingValue::Text(v)) => {
            settings.theme = match v.as_str() {
                "light" => ThemeMode::Light,
                "dark" => ThemeMode::Dark,
                _ => ThemeMode::System,
            }
        }
        ("theme.overrides", SettingValue::Map(v)) => settings.theme_overrides.extend(v),
        ("toolbar.visible", SettingValue::Bool(v)) => settings.toolbar_visible = v,
        ("toolbar.commands", SettingValue::Strings(v)) => settings.toolbar_commands = v,
        ("tabs.pinned_first", SettingValue::Bool(v)) => settings.tabs_pinned_first = v,
        ("language.locale", SettingValue::Text(v)) => settings.locale = v,
        ("language.associations", SettingValue::Map(v)) => settings.language_associations.extend(v),
        ("search.excludes", SettingValue::Strings(v)) => settings.search_excludes = v,
        ("renderer.mode", SettingValue::Text(v)) => {
            settings.renderer = if v == "software" {
                RendererMode::Software
            } else {
                RendererMode::Hardware
            }
        }
        _ => {}
    }
}
/// Points remain persisted; renderer receives logical pixels and applies monitor scale once.
pub fn pt_to_logical_px(points: f64) -> f64 {
    points * 96.0 / 72.0
}
pub fn pt_to_physical_px(points: f64, scale: f64) -> Result<f64, String> {
    if !points.is_finite()
        || !(6.0..=72.0).contains(&points)
        || !scale.is_finite()
        || !(0.5..=8.0).contains(&scale)
    {
        return Err("Invalid font size or DPI scale".into());
    }
    Ok(pt_to_logical_px(points) * scale)
}
