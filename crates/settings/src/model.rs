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
    /// A byte quota shown with KB/MB/GB units instead of a raw byte count.
    Bytes(i64, i64),
    /// A font family picked from the installed families.
    Font,
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
    setting!(
        "editor.clipboard.history_enabled",
        "Clipboard history",
        "Keep a bounded in-memory clipboard history for this session.",
        "Advanced",
        SettingKind::Boolean,
        false,
        false
    ),
    setting!(
        "clipboard.history.max_entries",
        "Clipboard history entries",
        "Maximum number of retained clipboard entries.",
        "Advanced",
        SettingKind::Integer(1, 1000),
        false,
        false
    ),
    setting!(
        "clipboard.history.max_total_bytes",
        "Clipboard history size limit",
        "Total memory kept for the clipboard history.",
        "Advanced",
        SettingKind::Bytes(4096, 268435456),
        false,
        false
    ),
    setting!(
        "clipboard.history.max_entry_bytes",
        "Largest clipboard entry to keep",
        "Clipboard entries larger than this are not kept in the history.",
        "Advanced",
        SettingKind::Bytes(1024, 67108864),
        false,
        false
    ),
    setting!(
        "language.policies",
        "Language behavior",
        "Per-language overrides: language ID and field, for example rust.lexer = native or rust.min_chars = 2. Values are strings.",
        "Language",
        SettingKind::Map,
        false,
        false
    ),
    setting!(
        "document.resident_max_bytes",
        "Large-file threshold",
        "Open files larger than this in large-file mode. Applies to files opened from now on.",
        "Advanced",
        SettingKind::Bytes(4096, 1_099_511_627_776),
        false,
        false
    ),
    setting!(
        "document.page_size_bytes",
        "Large-file block size",
        "How much of a large file is read at a time. Applies to files opened from now on.",
        "Advanced",
        SettingKind::Bytes(4096, 16777216),
        false,
        false
    ),
    setting!(
        "document.page_cache_bytes",
        "Memory kept per large file",
        "How much of one large file Bareline keeps in memory. Applies to files opened from now on.",
        "Advanced",
        SettingKind::Bytes(4096, 1073741824),
        false,
        false
    ),
    setting!(
        "document.aggregate_cache_bytes",
        "Document memory limit",
        "Memory shared by all open documents. Lowering it keeps what is already loaded and stops new reads until usage drops.",
        "Advanced",
        SettingKind::Bytes(1048576, 1099511627776),
        false,
        false
    ),
    setting!(
        "undo.aggregate_ram_bytes",
        "Undo memory limit",
        "Memory shared by the undo history of all open documents.",
        "Advanced",
        SettingKind::Bytes(1048576, 1099511627776),
        false,
        false
    ),
    setting!(
        "undo.max_changes",
        "Undo steps kept",
        "How many undo and redo steps to keep per document. The oldest steps are dropped first.",
        "Advanced",
        SettingKind::Integer(1, 1_000_000),
        false,
        false
    ),
    setting!(
        "transcode.temp_quota_bytes",
        "Temporary disk space for encoding conversion",
        "Disk space Bareline may use while converting a file's text encoding. Free disk space still applies.",
        "Advanced",
        SettingKind::Bytes(4096, 1_099_511_627_776),
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
        "workspace.dock.widths",
        "Dock sizes",
        "Remembered widths of the side docks and height of the bottom dock.",
        "Advanced",
        SettingKind::Text,
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
        "Font used by the editor. Monospaced fonts are listed first.",
        "Editor",
        SettingKind::Font,
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
        "Editor",
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
        "Show line numbers in the left margin.",
        "Editor",
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
        "Language used for Bareline's own menus and labels.",
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
        "editor.auto_indent",
        "Auto indent",
        "Match the previous line's indentation when you press Enter.",
        "Editor",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "editor.auto_close_brackets",
        "Close brackets and quotes",
        "Insert the matching closing bracket or quote as you type.",
        "Editor",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "editor.caret.style",
        "Cursor shape",
        "Shape of the text cursor.",
        "Editor",
        SettingKind::Choice(&["line", "block", "underline"]),
        false,
        true
    ),
    setting!(
        "editor.scroll_beyond_last_line",
        "Scroll past the last line",
        "Allow scrolling below the end of the document.",
        "Editor",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "editor.minimap",
        "Document map",
        "Show the document map beside the editor.",
        "Editor",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "files.default_encoding",
        "Encoding for new files",
        "Text encoding used when you create a new file.",
        "Files",
        SettingKind::Choice(&["utf-8", "utf-8-bom", "utf-16le", "utf-16be", "system"]),
        false,
        false
    ),
    setting!(
        "files.default_eol",
        "Line endings for new files",
        "Line ending written when you create a new file.",
        "Files",
        SettingKind::Choice(&["crlf", "lf"]),
        false,
        false
    ),
    setting!(
        "files.autosave_seconds",
        "Auto-save interval",
        "Save changed files automatically after this many seconds. Zero turns auto-save off.",
        "Files",
        SettingKind::Integer(0, 3600),
        false,
        false
    ),
    setting!(
        "files.backup_on_save",
        "Keep a backup copy on save",
        "Write a .bak copy of the previous contents each time you save.",
        "Files",
        SettingKind::Boolean,
        false,
        false
    ),
    setting!(
        "files.external_change",
        "When a file changes outside Bareline",
        "What to do when another program changes an open file.",
        "Files",
        SettingKind::Choice(&["prompt", "reload", "ignore"]),
        false,
        false
    ),
    setting!(
        "files.confirm_close_unsaved",
        "Confirm before closing unsaved files",
        "Ask before closing a document with unsaved changes.",
        "Files",
        SettingKind::Boolean,
        false,
        false
    ),
    setting!(
        "search.match_case",
        "Match case by default",
        "Start Find and Replace with case matching turned on.",
        "Search",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "search.whole_word",
        "Whole word by default",
        "Start Find and Replace matching whole words only.",
        "Search",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "search.regex",
        "Regular expressions by default",
        "Start Find and Replace in regular-expression mode.",
        "Search",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "search.wrap_around",
        "Wrap around at the end",
        "Continue from the top of the document when Find reaches the end.",
        "Search",
        SettingKind::Boolean,
        false,
        true
    ),
    setting!(
        "search.max_results",
        "Maximum results",
        "Stop a workspace search after this many matches.",
        "Search",
        SettingKind::Integer(1, 1_000_000),
        false,
        true
    ),
    setting!(
        "keyboard.chord_timeout_ms",
        "Chord timeout",
        "Milliseconds to wait for the second key of a two-key shortcut.",
        "Keyboard",
        SettingKind::Integer(200, 5000),
        false,
        false
    ),
    setting!(
        "extensions.enabled",
        "Enable extensions",
        "Run installed extensions. Takes effect after a restart.",
        "Extensions",
        SettingKind::Boolean,
        true,
        false
    ),
    setting!(
        "renderer.mode",
        "Drawing mode",
        "Draw with the graphics card or with the processor. Takes effect after a restart.",
        "Advanced",
        SettingKind::Choice(&["hardware", "software"]),
        true,
        false
    ),
];

/// Settings that exist in the schema but are not worth showing yet. `language.locale`
/// stays hidden until a second locale ships (UX-54i).
pub const HIDDEN_KEYS: &[&str] = &["language.locale", "workspace.dock.widths"];

/// True when the settings page should not list this key.
pub fn is_hidden(key: &str) -> bool {
    HIDDEN_KEYS.contains(&key)
}

/// True when a key/value setting holds theme colors, so the page shows it as
/// color rows with swatches rather than a plain name/value list.
pub fn is_color_map(key: &str) -> bool {
    key == "theme.overrides"
}

/// Format a theme color back to the `#RRGGBB` / `#RRGGBBAA` text a color row
/// edits, so a swatch and its stored value round-trip through the same string.
pub fn format_color_hex(color: &crate::ThemeColor) -> String {
    if color.alpha == 255 {
        format!("#{:06X}", color.rgb & 0xFF_FFFF)
    } else {
        format!("#{:06X}{:02X}", color.rgb & 0xFF_FFFF, color.alpha)
    }
}

/// Format a byte count for display: "64 MB", "1.5 GB", "4 KB".
pub fn format_bytes(bytes: i64) -> String {
    const UNITS: [(&str, i64); 4] = [
        ("GB", 1024 * 1024 * 1024),
        ("MB", 1024 * 1024),
        ("KB", 1024),
        ("bytes", 1),
    ];
    for (unit, scale) in UNITS {
        if bytes >= scale {
            if bytes % scale == 0 {
                return format!("{} {unit}", bytes / scale);
            }
            let value = bytes as f64 / scale as f64;
            let text = format!("{value:.1}");
            return format!("{} {unit}", text.trim_end_matches(".0"));
        }
    }
    format!("{bytes} bytes")
}

/// Parse a byte count written with or without a unit: "64 MB", "64mb", "65536".
pub fn parse_bytes(input: &str) -> Result<i64, String> {
    let trimmed = input.trim();
    let split = trimmed
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '_')
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let number = number.replace('_', "");
    if number.is_empty() {
        return Err("Enter a size, for example 64 MB".into());
    }
    let scale = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" | "byte" | "bytes" => 1_i64,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        "t" | "tb" | "tib" => 1024_i64 * 1024 * 1024 * 1024,
        _ => return Err("Use bytes, KB, MB, GB or TB".into()),
    };
    let value: f64 = number
        .parse()
        .map_err(|_| "Enter a size, for example 64 MB".to_string())?;
    let scaled = value * scale as f64;
    if !scaled.is_finite() || scaled < 0.0 || scaled > i64::MAX as f64 {
        return Err("Size is outside the permitted range".into());
    }
    Ok(scaled.round() as i64)
}

/// Sensible byte steps offered for a `Bytes` setting, always inside the range.
pub fn byte_steps(min: i64, max: i64) -> Vec<i64> {
    let mut steps = vec![min, max];
    for power in 12..=41 {
        let value = 1_i64 << power;
        if value > min && value < max {
            steps.push(value);
        }
    }
    steps.sort_unstable();
    steps.dedup();
    steps
}

/// Turn a stored enum value into a title-cased display name. Known values get a
/// hand-written label; anything else is title-cased word by word.
pub fn display_name(value: &str) -> String {
    match value {
        "utf-8" => return "UTF-8".into(),
        "utf-8-bom" => return "UTF-8 with BOM".into(),
        "utf-16le" => return "UTF-16 LE".into(),
        "utf-16be" => return "UTF-16 BE".into(),
        "crlf" => return "Windows (CRLF)".into(),
        "lf" => return "Unix (LF)".into(),
        "viewport" => return "On".into(),
        "off" => return "Off".into(),
        "none" => return "None".into(),
        "prompt" => return "Ask me".into(),
        _ => {}
    }
    let mut out = String::with_capacity(value.len());
    for (index, word) in value.split(['_', '-', '.', ' ']).enumerate() {
        if word.is_empty() {
            continue;
        }
        if index > 0 {
            out.push(' ');
            out.push_str(&word.to_lowercase());
            continue;
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(&chars.as_str().to_lowercase());
        }
    }
    if out.is_empty() { value.into() } else { out }
}

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
        let mut document = text.parse::<DocumentMut>().map_err(|error| error.to_string())?;
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
                SettingValue::Bool(v) => SettingValue::Text(if v { "viewport" } else { "off" }.into()),
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
            .filter(|d| d.category == category && (self.scope != Scope::Workspace || d.workspace_allowed))
            .map(|d| d.key)
            .collect();
        for key in &keys {
            remove_key(self.document.as_table_mut(), &key.split('.').collect::<Vec<_>>());
            for (legacy, canonical) in ALIASES {
                if canonical == key {
                    remove_key(self.document.as_table_mut(), &legacy.split('.').collect::<Vec<_>>());
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
                            || (matches!(d.kind, SettingKind::Map) && key.starts_with(&format!("{}.", d.key)))
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
        for section in ["editor", "theme", "toolbar", "tabs", "language", "search", "renderer"] {
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
    item.as_float().or_else(|| item.as_integer().map(|n| n as f64))
}
/// Parse the single-line value editor using the same schema as persisted TOML.
/// Text and choices are plain text; arrays/maps use TOML value syntax.
pub fn parse_setting_input(key: &str, input: &str) -> Result<SettingValue, String> {
    if input.len() > 16 * 1024 {
        return Err("Value exceeds the inline editor limit; edit the TOML file".into());
    }
    let definition = DEFINITIONS.iter().find(|d| d.key == key).ok_or("Unknown setting")?;
    let value = match definition.kind {
        SettingKind::Text | SettingKind::Font | SettingKind::Choice(_) => SettingValue::Text(input.to_owned()),
        SettingKind::Bytes(_, _) => SettingValue::Integer(parse_bytes(input)?),
        _ => {
            let document = format!("value = {input}")
                .parse::<DocumentMut>()
                .map_err(|_| "Enter a valid TOML value")?;
            if document.len() != 1 {
                return Err("Enter one value only".into());
            }
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
            for value in values {
                array.push(value.as_str());
            }
            array.to_string()
        }
        SettingValue::Map(values) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in values {
                table.insert(key, Value::from(value.as_str()));
            }
            table.to_string()
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexerPreference {
    Primary,
    Native,
}
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
    fn default() -> Self {
        Self {
            lexer: LexerPreference::Primary,
            completion: true,
            min_chars: 0,
            include_open_documents: true,
            smart_pairs: true,
            smart_indent: true,
            parameter_hints: true,
        }
    }
}
fn validate_language_policy(key: &str, value: &str) -> Result<(), String> {
    let (language, field) = key.rsplit_once('.').ok_or("Use language-id.field for a policy key")?;
    if language.is_empty()
        || language.len() > 64
        || !language
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err("Invalid stable language ID".into());
    }
    let valid = match field {
        "lexer" => matches!(value, "primary" | "native"),
        "min_chars" => value.parse::<u8>().is_ok_and(|n| n <= 16),
        "completion" | "include_open_documents" | "smart_pairs" | "smart_indent" | "parameter_hints" => {
            matches!(value, "true" | "false")
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!("Invalid language policy {key}"))
    }
}
impl EffectiveSettings {
    pub fn setting_value(&self, key: &str) -> Option<SettingValue> {
        Some(match key {
            "editor.clipboard.history_enabled" => SettingValue::Bool(self.clipboard_history_enabled),
            "clipboard.history.max_entries" => SettingValue::Integer(self.clipboard_history_max_entries as i64),
            "clipboard.history.max_total_bytes" => SettingValue::Integer(self.clipboard_history_max_total_bytes as i64),
            "clipboard.history.max_entry_bytes" => SettingValue::Integer(self.clipboard_history_max_entry_bytes as i64),
            "session.restore" => SettingValue::Bool(self.restore_session),
            "workspace.preferences_enabled" => SettingValue::Bool(self.workspace_preferences_enabled),
            "document.resident_max_bytes" => SettingValue::Integer(self.resident_max_bytes as i64),
            "undo.max_changes" => SettingValue::Integer(self.undo_max_changes as i64),
            "document.page_size_bytes" => SettingValue::Integer(self.page_size_bytes as i64),
            "document.page_cache_bytes" => SettingValue::Integer(self.page_cache_bytes as i64),
            "document.aggregate_cache_bytes" => SettingValue::Integer(self.aggregate_cache_bytes as i64),
            "undo.aggregate_ram_bytes" => SettingValue::Integer(self.undo_aggregate_ram_bytes as i64),
            "transcode.temp_quota_bytes" => SettingValue::Integer(self.transcode_quota_bytes as i64),
            "editor.font.family" => SettingValue::Text(self.editor_font_family.clone()),
            "editor.font.size" => SettingValue::Number(self.editor_font_size_pt),
            "editor.tab.width" => SettingValue::Integer(self.tab_width as i64),
            "editor.insert_spaces" => SettingValue::Bool(self.insert_spaces),
            "editor.wrap.mode" => SettingValue::Text(if self.word_wrap { "viewport" } else { "off" }.into()),
            "editor.line_numbers" => SettingValue::Bool(self.line_numbers),
            "editor.render.whitespace" => SettingValue::Text(self.whitespace.clone()),
            "editor.currentLine.highlight" => SettingValue::Bool(self.highlight_current_line),
            "theme.mode" => SettingValue::Text(
                match self.theme {
                    ThemeMode::System => "system",
                    ThemeMode::Light => "light",
                    ThemeMode::Dark => "dark",
                }
                .into(),
            ),
            "theme.overrides" => SettingValue::Map(self.theme_overrides.clone()),
            "toolbar.visible" => SettingValue::Bool(self.toolbar_visible),
            "toolbar.commands" => SettingValue::Strings(self.toolbar_commands.clone()),
            "tabs.pinned_first" => SettingValue::Bool(self.tabs_pinned_first),
            "language.locale" => SettingValue::Text(self.locale.clone()),
            "language.associations" => SettingValue::Map(self.language_associations.clone()),
            "language.policies" => SettingValue::Map(self.language_policies.clone()),
            "search.excludes" => SettingValue::Strings(self.search_excludes.clone()),
            "editor.auto_indent" => SettingValue::Bool(self.auto_indent),
            "editor.auto_close_brackets" => SettingValue::Bool(self.auto_close_brackets),
            "editor.caret.style" => SettingValue::Text(self.caret_style.clone()),
            "editor.scroll_beyond_last_line" => SettingValue::Bool(self.scroll_beyond_last_line),
            "editor.minimap" => SettingValue::Bool(self.minimap),
            "files.default_encoding" => SettingValue::Text(self.default_encoding.clone()),
            "files.default_eol" => SettingValue::Text(self.default_eol.clone()),
            "files.autosave_seconds" => SettingValue::Integer(self.autosave_seconds as i64),
            "files.backup_on_save" => SettingValue::Bool(self.backup_on_save),
            "files.external_change" => SettingValue::Text(self.external_change.clone()),
            "files.confirm_close_unsaved" => SettingValue::Bool(self.confirm_close_unsaved),
            "search.match_case" => SettingValue::Bool(self.search_match_case),
            "search.whole_word" => SettingValue::Bool(self.search_whole_word),
            "search.regex" => SettingValue::Bool(self.search_regex),
            "search.wrap_around" => SettingValue::Bool(self.search_wrap_around),
            "search.max_results" => SettingValue::Integer(self.search_max_results as i64),
            "keyboard.chord_timeout_ms" => SettingValue::Integer(self.chord_timeout_ms as i64),
            "extensions.enabled" => SettingValue::Bool(self.extensions_enabled),
            "workspace.dock.widths" => SettingValue::Text(self.dock_widths.clone()),
            "renderer.mode" => SettingValue::Text(
                match self.renderer {
                    RendererMode::Hardware => "hardware",
                    RendererMode::Software => "software",
                }
                .into(),
            ),
            _ => return None,
        })
    }
    pub fn language_policy(&self, stable_id: &str) -> LanguagePolicy {
        let mut policy = LanguagePolicy::default();
        let get = |field: &str| {
            self.language_policies
                .get(&format!("{stable_id}.{field}"))
                .map(String::as_str)
        };
        if get("lexer") == Some("native") {
            policy.lexer = LexerPreference::Native;
        }
        policy.min_chars = get("min_chars")
            .and_then(|n| n.parse().ok())
            .filter(|n| *n <= 16)
            .unwrap_or(0);
        for (field, target) in [
            ("completion", &mut policy.completion),
            ("include_open_documents", &mut policy.include_open_documents),
            ("smart_pairs", &mut policy.smart_pairs),
            ("smart_indent", &mut policy.smart_indent),
            ("parameter_hints", &mut policy.parameter_hints),
        ] {
            if let Some(value) = get(field) {
                *target = value == "true";
            }
        }
        policy
    }
}
fn parse_value(definition: &SettingDefinition, item: &Item) -> Result<SettingValue, String> {
    if definition.key == "editor.wrap.mode"
        && let Some(value) = item.as_bool()
    {
        return Ok(SettingValue::Text(if value { "viewport" } else { "off" }.into()));
    }
    let invalid = || "Invalid setting type; using the lower-priority value".to_string();
    Ok(match definition.kind {
        SettingKind::Boolean => SettingValue::Bool(item.as_bool().ok_or_else(invalid)?),
        SettingKind::Integer(_, _) | SettingKind::Bytes(_, _) => {
            SettingValue::Integer(item.as_integer().ok_or_else(invalid)?)
        }
        SettingKind::Number(_, _) => SettingValue::Number(number(item).ok_or_else(invalid)?),
        SettingKind::Text | SettingKind::Font | SettingKind::Choice(_) => {
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
    if matches!(
        definition.key,
        "document.page_size_bytes"
            | "document.page_cache_bytes"
            | "document.aggregate_cache_bytes"
            | "undo.aggregate_ram_bytes"
    ) && matches!(value, SettingValue::Integer(bytes) if usize::try_from(*bytes).is_err())
    {
        return Err("Resource limit exceeds this platform's address range".into());
    }
    if definition.key == "language.locale" {
        if let SettingValue::Text(locale) = value {
            if locale.is_empty() || locale.len() > 64 || !locale.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err("Locale must be a language ID containing letters, digits or hyphens".into());
            }
        }
    }
    if definition.key == "language.policies" {
        if let SettingValue::Map(values) = value {
            if values.len() > 512 {
                return Err("At most 512 language policy entries are allowed".into());
            }
            for (key, value) in values {
                validate_language_policy(key, value)?;
            }
        }
    }
    let valid = match (definition.kind, value) {
        (SettingKind::Boolean, SettingValue::Bool(_)) => true,
        (SettingKind::Integer(min, max) | SettingKind::Bytes(min, max), SettingValue::Integer(n)) => {
            (min..=max).contains(n)
        }
        (SettingKind::Number(min, max), SettingValue::Number(n)) => n.is_finite() && (min..=max).contains(n),
        (SettingKind::Text | SettingKind::Font, SettingValue::Text(text)) => {
            !text.is_empty() && text.len() <= 256 && !text.chars().any(char::is_control)
        }
        (SettingKind::Choice(choices), SettingValue::Text(text)) => choices.contains(&text.as_str()),
        (SettingKind::Strings, SettingValue::Strings(values)) => {
            values.len() <= 256 && values.iter().all(|v| v.len() <= 1024 && !v.contains('\0'))
        }
        (SettingKind::Map, SettingValue::Map(values)) => {
            values.len() <= 256
                && values.iter().all(|(k, v)| {
                    !k.is_empty() && k.len() <= 256 && v.len() <= 1024 && !k.contains('\0') && !v.contains('\0')
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
                    || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
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
    pub clipboard_history_enabled: bool,
    pub clipboard_history_max_entries: usize,
    pub clipboard_history_max_total_bytes: usize,
    pub clipboard_history_max_entry_bytes: usize,
    pub language_policies: BTreeMap<String, String>,
    pub resident_max_bytes: u64,
    pub undo_max_changes: usize,
    pub page_size_bytes: usize,
    pub page_cache_bytes: usize,
    pub aggregate_cache_bytes: usize,
    pub undo_aggregate_ram_bytes: usize,
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
    pub auto_indent: bool,
    pub auto_close_brackets: bool,
    pub caret_style: String,
    pub scroll_beyond_last_line: bool,
    pub minimap: bool,
    pub default_encoding: String,
    pub default_eol: String,
    pub autosave_seconds: u32,
    pub backup_on_save: bool,
    pub external_change: String,
    pub confirm_close_unsaved: bool,
    pub search_match_case: bool,
    pub search_whole_word: bool,
    pub search_regex: bool,
    pub search_wrap_around: bool,
    pub search_max_results: usize,
    pub chord_timeout_ms: u32,
    pub extensions_enabled: bool,
    /// Persisted dock geometry in the DockWidths serialized form (from
    /// bareline-ui); empty means "use defaults". Never shown as a settings row.
    pub dock_widths: String,
}
impl Default for EffectiveSettings {
    fn default() -> Self {
        Self {
            clipboard_history_enabled: false,
            clipboard_history_max_entries: 20,
            clipboard_history_max_total_bytes: 16 * 1024 * 1024,
            clipboard_history_max_entry_bytes: 4 * 1024 * 1024,
            language_policies: BTreeMap::new(),
            resident_max_bytes: 268_435_456,
            undo_max_changes: 100_000,
            page_size_bytes: 1048576,
            page_cache_bytes: 67108864,
            aggregate_cache_bytes: 268435456,
            undo_aggregate_ram_bytes: 134217728,
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
            toolbar_commands: ["file.new", "file.open", "file.save", "edit.undo", "search.find"]
                .into_iter()
                .map(String::from)
                .collect(),
            tabs_pinned_first: true,
            locale: "en".into(),
            language_associations: BTreeMap::new(),
            search_excludes: Vec::new(),
            auto_indent: true,
            auto_close_brackets: true,
            caret_style: "line".into(),
            scroll_beyond_last_line: false,
            minimap: false,
            default_encoding: "utf-8".into(),
            default_eol: "crlf".into(),
            autosave_seconds: 0,
            backup_on_save: false,
            external_change: "prompt".into(),
            confirm_close_unsaved: true,
            search_match_case: false,
            search_whole_word: false,
            search_regex: false,
            search_wrap_around: true,
            search_max_results: 10_000,
            chord_timeout_ms: 1500,
            extensions_enabled: true,
            dock_widths: String::new(),
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
    // A cache must admit at least one page; display the effective bounded value.
    resolved.values.page_cache_bytes = resolved
        .values
        .page_cache_bytes
        .min(resolved.values.aggregate_cache_bytes);
    resolved.values.page_size_bytes = resolved.values.page_size_bytes.min(resolved.values.page_cache_bytes);
    resolved
}
fn apply(settings: &mut EffectiveSettings, key: &str, value: SettingValue) {
    match (key, value) {
        ("undo.max_changes", SettingValue::Integer(v)) => settings.undo_max_changes = v as usize,
        ("document.page_size_bytes", SettingValue::Integer(v)) => settings.page_size_bytes = v as usize,
        ("document.page_cache_bytes", SettingValue::Integer(v)) => settings.page_cache_bytes = v as usize,
        ("document.aggregate_cache_bytes", SettingValue::Integer(v)) => settings.aggregate_cache_bytes = v as usize,
        ("undo.aggregate_ram_bytes", SettingValue::Integer(v)) => settings.undo_aggregate_ram_bytes = v as usize,
        ("editor.clipboard.history_enabled", SettingValue::Bool(v)) => settings.clipboard_history_enabled = v,
        ("clipboard.history.max_entries", SettingValue::Integer(v)) => {
            settings.clipboard_history_max_entries = v as usize
        }
        ("clipboard.history.max_total_bytes", SettingValue::Integer(v)) => {
            settings.clipboard_history_max_total_bytes = v as usize
        }
        ("clipboard.history.max_entry_bytes", SettingValue::Integer(v)) => {
            settings.clipboard_history_max_entry_bytes = v as usize
        }
        ("language.policies", SettingValue::Map(v)) => settings.language_policies.extend(v),
        ("document.resident_max_bytes", SettingValue::Integer(v)) => settings.resident_max_bytes = v as u64,
        ("transcode.temp_quota_bytes", SettingValue::Integer(v)) => settings.transcode_quota_bytes = v as u64,
        ("session.restore", SettingValue::Bool(v)) => settings.restore_session = v,
        ("workspace.preferences_enabled", SettingValue::Bool(v)) => settings.workspace_preferences_enabled = v,
        ("editor.font.size", SettingValue::Number(v)) => settings.editor_font_size_pt = v,
        ("editor.font.family", SettingValue::Text(v)) => settings.editor_font_family = v,
        ("editor.tab.width", SettingValue::Integer(v)) => settings.tab_width = v as u8,
        ("editor.insert_spaces", SettingValue::Bool(v)) => settings.insert_spaces = v,
        ("editor.wrap.mode", SettingValue::Text(v)) => settings.word_wrap = v == "viewport",
        ("editor.render.whitespace", SettingValue::Text(v)) => settings.whitespace = v,
        ("editor.currentLine.highlight", SettingValue::Bool(v)) => settings.highlight_current_line = v,
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
        ("editor.auto_indent", SettingValue::Bool(v)) => settings.auto_indent = v,
        ("editor.auto_close_brackets", SettingValue::Bool(v)) => settings.auto_close_brackets = v,
        ("editor.caret.style", SettingValue::Text(v)) => settings.caret_style = v,
        ("editor.scroll_beyond_last_line", SettingValue::Bool(v)) => settings.scroll_beyond_last_line = v,
        ("editor.minimap", SettingValue::Bool(v)) => settings.minimap = v,
        ("files.default_encoding", SettingValue::Text(v)) => settings.default_encoding = v,
        ("files.default_eol", SettingValue::Text(v)) => settings.default_eol = v,
        ("files.autosave_seconds", SettingValue::Integer(v)) => settings.autosave_seconds = v.clamp(0, 3600) as u32,
        ("files.backup_on_save", SettingValue::Bool(v)) => settings.backup_on_save = v,
        ("files.external_change", SettingValue::Text(v)) => settings.external_change = v,
        ("files.confirm_close_unsaved", SettingValue::Bool(v)) => settings.confirm_close_unsaved = v,
        ("search.match_case", SettingValue::Bool(v)) => settings.search_match_case = v,
        ("search.whole_word", SettingValue::Bool(v)) => settings.search_whole_word = v,
        ("search.regex", SettingValue::Bool(v)) => settings.search_regex = v,
        ("search.wrap_around", SettingValue::Bool(v)) => settings.search_wrap_around = v,
        ("search.max_results", SettingValue::Integer(v)) => settings.search_max_results = v.max(1) as usize,
        ("keyboard.chord_timeout_ms", SettingValue::Integer(v)) => {
            settings.chord_timeout_ms = v.clamp(200, 5000) as u32
        }
        ("extensions.enabled", SettingValue::Bool(v)) => settings.extensions_enabled = v,
        ("workspace.dock.widths", SettingValue::Text(v)) => settings.dock_widths = v,
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

#[cfg(test)]
mod input_contract_tests {
    use super::*;
    #[test]
    fn resource_limits_are_user_owned_and_effective_pages_fit_the_shared_cap() {
        let mut user = SettingsDocument::empty(Scope::User);
        assert_eq!(EffectiveSettings::default().undo_max_changes, 100_000);
        user.set("undo.max_changes", SettingValue::Integer(37)).unwrap();
        assert!(user.set("undo.max_changes", SettingValue::Integer(0)).is_err());
        user.set("document.page_size_bytes", SettingValue::Integer(8 << 20))
            .unwrap();
        user.set("document.page_cache_bytes", SettingValue::Integer(16 << 20))
            .unwrap();
        user.set("document.aggregate_cache_bytes", SettingValue::Integer(4 << 20))
            .unwrap();
        user.set("undo.aggregate_ram_bytes", SettingValue::Integer(2 << 20))
            .unwrap();
        assert!(user.set("document.page_size_bytes", SettingValue::Integer(0)).is_err());
        let mut workspace = SettingsDocument::empty(Scope::Workspace);
        assert!(workspace.set("undo.max_changes", SettingValue::Integer(2)).is_err());
        assert!(
            workspace
                .set("undo.aggregate_ram_bytes", SettingValue::Integer(8 << 20))
                .is_err()
        );
        let values = resolve(&user, Some(&workspace), true, None).values;
        assert_eq!(values.undo_max_changes, 37);
        assert_eq!(values.page_size_bytes, 4 << 20);
        assert_eq!(values.page_cache_bytes, 4 << 20);
        assert_eq!(values.undo_aggregate_ram_bytes, 2 << 20);
        assert_eq!(
            values.setting_value("document.page_size_bytes"),
            Some(SettingValue::Integer(4 << 20))
        );
    }
    #[test]
    fn policy_and_clipboard_inputs_roundtrip_and_invalid_input_preserves_document() {
        let mut document = SettingsDocument::empty(Scope::User);
        let policies = parse_setting_input(
            "language.policies",
            r#"{ "rust.lexer" = "native", "rust.min_chars" = "2" }"#,
        )
        .unwrap();
        document.set("language.policies", policies.clone()).unwrap();
        assert_eq!(
            parse_setting_input("language.policies", &format_setting_input(&policies)).unwrap(),
            policies
        );
        document
            .set("editor.clipboard.history_enabled", SettingValue::Bool(true))
            .unwrap();
        let before = document.to_toml();
        assert!(
            document
                .set("clipboard.history.max_entries", SettingValue::Integer(0))
                .is_err()
        );
        assert_eq!(document.to_toml(), before);
        assert!(parse_setting_input("language.policies", r#"{ "rust.min_chars" = "17" }"#).is_err());
        let effective = resolve(&document, None, false, None).values;
        assert_eq!(effective.language_policy("rust").lexer, LexerPreference::Native);
        assert_eq!(effective.language_policy("rust").min_chars, 2);
        assert!(effective.clipboard_history_enabled);
        assert_eq!(effective.clipboard_history_max_entries, 20);
    }
}
pub fn pt_to_physical_px(points: f64, scale: f64) -> Result<f64, String> {
    if !points.is_finite() || !(6.0..=72.0).contains(&points) || !scale.is_finite() || !(0.5..=8.0).contains(&scale) {
        return Err("Invalid font size or DPI scale".into());
    }
    Ok(pt_to_logical_px(points) * scale)
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    #[test]
    fn byte_sizes_round_trip_through_display_units() {
        assert_eq!(format_bytes(64 * 1024 * 1024), "64 MB");
        assert_eq!(format_bytes(4096), "4 KB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1 GB");
        assert_eq!(format_bytes(1536 * 1024 * 1024), "1.5 GB");
        assert_eq!(format_bytes(512), "512 bytes");
        assert_eq!(parse_bytes("64 MB").unwrap(), 64 * 1024 * 1024);
        assert_eq!(parse_bytes("64mb").unwrap(), 64 * 1024 * 1024);
        assert_eq!(parse_bytes("65536").unwrap(), 65536);
        assert_eq!(parse_bytes("1.5 GB").unwrap(), 1536 * 1024 * 1024);
        assert!(parse_bytes("64 parsecs").is_err());
        assert!(parse_bytes("MB").is_err());
        let value = parse_setting_input("document.resident_max_bytes", "64 MB").unwrap();
        assert_eq!(value, SettingValue::Integer(64 * 1024 * 1024));
    }
    #[test]
    fn choice_values_display_title_cased() {
        assert_eq!(display_name("system"), "System");
        assert_eq!(display_name("utf-8"), "UTF-8");
        assert_eq!(display_name("crlf"), "Windows (CRLF)");
        assert_eq!(display_name("whole_word"), "Whole word");
        assert_eq!(display_name("viewport"), "On");
        assert_eq!(display_name("hardware"), "Hardware");
    }
    #[test]
    fn color_rows_round_trip_hex_through_parse_and_format() {
        use crate::ThemeColor;
        for text in ["#000000", "#FFFFFF", "#1E1E1E", "#00000080", "#12345678"] {
            let color = ThemeColor::parse(text).unwrap();
            assert_eq!(format_color_hex(&color), text, "{text} did not survive parse/format");
            // Formatting is stable under a second round-trip too.
            let reparsed = ThemeColor::parse(&format_color_hex(&color)).unwrap();
            assert_eq!(reparsed, color);
        }
        // A fully opaque #RRGGBBAA canonicalises to the shorter opaque form.
        assert_eq!(format_color_hex(&ThemeColor::parse("#23272BFF").unwrap()), "#23272B");
        // A lower-case input normalises to the canonical upper-case hex.
        assert_eq!(format_color_hex(&ThemeColor::parse("#abcdef").unwrap()), "#ABCDEF");
        assert!(is_color_map("theme.overrides"));
        assert!(!is_color_map("language.associations"));
    }
    #[test]
    fn every_listed_category_owns_at_least_one_visible_setting() {
        let categories = [
            "Editor",
            "Appearance",
            "Files",
            "Search",
            "Keyboard",
            "Language",
            "Extensions",
            "Advanced",
        ];
        for category in categories {
            assert!(
                DEFINITIONS.iter().any(|d| d.category == category && !is_hidden(d.key)),
                "category {category} has no visible setting"
            );
        }
        for definition in DEFINITIONS {
            assert!(
                categories.contains(&definition.category),
                "{} uses an unlisted category {}",
                definition.key,
                definition.category
            );
        }
    }
}
