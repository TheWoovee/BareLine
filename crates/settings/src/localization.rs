// SPDX-License-Identifier: MPL-2.0
use crate::{DEFINITIONS, MAX_CONFIG_BYTES};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextDirection {
    LeftToRight,
    RightToLeft,
}
#[derive(Clone, Debug)]
pub struct LocalePack {
    pub locale: String,
    pub direction: TextDirection,
    messages: BTreeMap<String, String>,
}
impl LocalePack {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err("Locale exceeds 1 MiB".into());
        }
        let text = std::str::from_utf8(bytes).map_err(|_| "Locale must be UTF-8")?;
        let document = text.parse::<toml_edit::DocumentMut>().map_err(|e| e.to_string())?;
        if document.get("version").and_then(toml_edit::Item::as_integer) != Some(1) {
            return Err("Unsupported locale version".into());
        }
        let locale = document
            .get("locale")
            .and_then(toml_edit::Item::as_str)
            .ok_or("Locale ID is required")?;
        if locale.is_empty() || locale.len() > 64 || !locale.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("Invalid locale ID".into());
        }
        let direction = match document.get("direction").and_then(toml_edit::Item::as_str) {
            Some("ltr") => TextDirection::LeftToRight,
            Some("rtl") => TextDirection::RightToLeft,
            _ => return Err("Locale direction must be ltr or rtl".into()),
        };
        let mut messages = BTreeMap::new();
        for (key, value) in document
            .get("messages")
            .and_then(toml_edit::Item::as_table_like)
            .ok_or("Locale messages are required")?
            .iter()
        {
            let text = value.as_str().ok_or("Locale messages must be plain strings")?;
            if key.is_empty() || key.len() > 256 || text.len() > 16 * 1024 || text.contains('\0') {
                return Err("Locale message exceeds limits".into());
            }
            placeholders(text)?;
            messages.insert(key.into(), text.into());
        }
        Ok(Self {
            locale: locale.into(),
            direction,
            messages,
        })
    }
    pub fn english() -> Self {
        let mut pack = Self::parse(include_bytes!("../locales/en.toml")).expect("valid built-in English");
        for definition in DEFINITIONS {
            pack.messages
                .insert(format!("setting.{}.title", definition.key), definition.title.into());
            pack.messages
                .insert(definition.description_id.into(), definition.description.into());
        }
        for category in [
            "Editor",
            "Appearance",
            "Files",
            "Search",
            "Keyboard",
            "Language",
            "Extensions",
            "Advanced",
        ] {
            pack.messages.insert(
                format!("settings.category.{category}"),
                if category == "Language" { "Languages" } else { category }.into(),
            );
        }
        // Command titles and menu captions are keyed resources in en.toml
        // ("command.<id>", "menu.<caption>"); see `unresourced_commands` (BIZ-30).
        pack
    }
    /// A message with its `{{`/`}}` escapes resolved; `None` when the ID is unknown
    /// or the message takes parameters.
    fn plain(&self, id: &str) -> Option<String> {
        self.messages.get(id).and_then(|text| format_message(text, &[]).ok())
    }
}
/// `language.locale` value that follows the Windows display language. It is the
/// default; only English ships, so it resolves to English until a pack is installed.
pub const SYSTEM_LOCALE: &str = "system";
/// The locale compiled into the binary.
pub const ENGLISH_LOCALE: &str = "en";
/// The pack to load for a `language.locale` value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocaleRequest {
    pub locale: String,
    /// Chosen explicitly. A missing pack for an explicit locale is reported; for
    /// the system language it quietly falls back to English.
    pub explicit: bool,
}
/// Resolves `setting` (the `language.locale` value) against the Windows display
/// language `system` (a BCP 47 name such as `de-DE`, when Windows can name it).
pub fn requested_locale(setting: &str, system: Option<&str>) -> LocaleRequest {
    let english =
        |locale: &str| locale.eq_ignore_ascii_case(ENGLISH_LOCALE) || locale.to_ascii_lowercase().starts_with("en-");
    if !setting.is_empty() && !setting.eq_ignore_ascii_case(SYSTEM_LOCALE) {
        return LocaleRequest {
            locale: setting.into(),
            explicit: true,
        };
    }
    let locale = system
        .filter(|locale| {
            !locale.is_empty() && locale.len() <= 64 && locale.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .filter(|locale| !english(*locale))
        .unwrap_or(ENGLISH_LOCALE);
    LocaleRequest {
        locale: locale.into(),
        explicit: false,
    }
}
/// Codec commands are titled with their encoding's standard name, which is not translated.
fn resource_exempt(id: &str) -> bool {
    id.starts_with("encoding.interpret.") || id.starts_with("encoding.convert.")
}
fn resource_line(key: &str, value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('{', "{{")
        .replace('}', "}}");
    format!("\"{key}\" = \"{escaped}\"")
}
/// Commands in `registry` without an English resource equal to their registered
/// title (BIZ-30), as the `crates/settings/locales/en.toml` lines that add them.
/// Every user-visible command title must be a keyed resource; codec commands are
/// the one exemption.
pub fn unresourced_commands(registry: &bareline_commands::CommandRegistry) -> Vec<String> {
    let english = LocalePack::english();
    registry
        .entries()
        .filter(|spec| !resource_exempt(spec.id.0))
        .filter_map(|spec| {
            let key = format!("command.{}", spec.id.0);
            (english.plain(&key).as_deref() != Some(spec.title)).then(|| resource_line(&key, spec.title))
        })
        .collect()
}
/// Submenus in `items` (recursively) whose caption has no `menu.<caption>`
/// English resource, as the `en.toml` lines that add them.
pub fn unresourced_menus(items: &[bareline_commands::MenuItem]) -> Vec<String> {
    fn walk(items: &[bareline_commands::MenuItem], english: &LocalePack, out: &mut Vec<String>) {
        for item in items {
            if let bareline_commands::MenuItem::Submenu { title, items } = item {
                let key = format!("menu.{title}");
                if english.plain(&key).as_deref() != Some(title.as_str()) {
                    out.push(resource_line(&key, title));
                }
                walk(items, english, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(items, &LocalePack::english(), &mut out);
    out.sort();
    out.dedup();
    out
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageChange {
    pub revision: u64,
    pub invalidate_visible_layout: bool,
    pub rebuild_native_menus: bool,
    pub restart_required: bool,
}
#[derive(Clone, Debug)]
pub struct Localizer {
    english: LocalePack,
    active: LocalePack,
    revision: u64,
}
impl Default for Localizer {
    fn default() -> Self {
        let english = LocalePack::english();
        Self {
            active: english.clone(),
            english,
            revision: 0,
        }
    }
}
impl Localizer {
    pub fn locale(&self) -> &str {
        &self.active.locale
    }
    /// Advances on every successful switch, so caches of localized text can
    /// tell when to rebuild.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn direction(&self) -> TextDirection {
        self.active.direction
    }
    /// Packs are data-only. Parameter sets must match English before any active state changes.
    pub fn switch(&mut self, pack: LocalePack) -> Result<LanguageChange, String> {
        for (id, text) in &pack.messages {
            if let Some(english) = self.english.messages.get(id)
                && placeholders(english)? != placeholders(text)?
            {
                return Err(format!("Locale parameters differ for {id}"));
            }
        }
        self.active = pack;
        self.revision = self.revision.wrapping_add(1);
        Ok(LanguageChange {
            revision: self.revision,
            invalidate_visible_layout: true,
            rebuild_native_menus: true,
            restart_required: false,
        })
    }
    pub fn format(&self, id: &str, args: &[(&str, &str)]) -> Result<String, String> {
        let text = self
            .active
            .messages
            .get(id)
            .or_else(|| self.english.messages.get(id))
            .ok_or_else(|| format!("Unknown message ID: {id}"))?;
        format_message(text, args)
    }
}
fn placeholders(text: &str) -> Result<BTreeSet<String>, String> {
    let mut result = BTreeSet::new();
    visit_message(text, |key| {
        result.insert(key.into());
        Ok(String::new())
    })?;
    Ok(result)
}
fn format_message(text: &str, args: &[(&str, &str)]) -> Result<String, String> {
    visit_message(text, |key| {
        args.iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| (*value).to_owned())
            .ok_or_else(|| format!("Missing message parameter: {key}"))
    })
}
fn visit_message(text: &str, mut parameter: impl FnMut(&str) -> Result<String, String>) -> Result<String, String> {
    let mut output = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if rest.starts_with("{{") {
            output.push('{');
            rest = &rest[2..];
        } else if rest.starts_with("}}") {
            output.push('}');
            rest = &rest[2..];
        } else if rest.starts_with('{') {
            let end = rest.find('}').ok_or("Unclosed message parameter")?;
            let key = &rest[1..end];
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err("Invalid message parameter".into());
            }
            output.push_str(&parameter(key)?);
            rest = &rest[end + 1..];
        } else if rest.starts_with('}') {
            return Err("Unexpected closing message brace".into());
        } else {
            let length = rest.find(['{', '}']).unwrap_or(rest.len());
            output.push_str(&rest[..length]);
            rest = &rest[length..];
        }
        if output.len() > 64 * 1024 {
            return Err("Formatted message exceeds 64 KiB".into());
        }
    }
    Ok(output)
}
