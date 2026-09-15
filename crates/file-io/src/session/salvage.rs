// SPDX-License-Identifier: MPL-2.0
//! Streaming, bounded entry salvage. Global JSON/version errors remain fatal.
use super::*;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::fmt;
const MAX_DIAGNOSTICS: usize = 128;
const MAX_VALUE_NODES: usize = 400_128;
const MAX_VALUE_STRINGS: usize = 2 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionIssue {
    InvalidEntry,
    ResourceLimit,
    DuplicateField,
    DuplicateIdentity,
    InvalidReference,
    InvalidPath,
    InvalidView,
    PinOrder,
    InvalidLayout,
    InvalidComparison,
    ActiveFallback,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionDiagnostic {
    pub section: &'static str,
    pub index: Option<usize>,
    pub issue: SessionIssue,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionDiagnostics {
    pub skipped_documents: usize,
    pub skipped: usize,
    pub repaired: usize,
    pub omitted: usize,
    pub entries: Vec<SessionDiagnostic>,
}
impl SessionDiagnostics {
    pub fn is_empty(&self) -> bool {
        self.skipped == 0 && self.repaired == 0
    }
    pub fn summary(&self) -> String {
        let mut reasons = Vec::new();
        for diagnostic in &self.entries {
            let label = format!("{:?}", diagnostic.issue);
            if !reasons.contains(&label) && reasons.len() < 4 {
                reasons.push(label);
            }
        }
        format!(
            "Session recovered: {} entries skipped, {} repairs. Reasons: {}.",
            self.skipped,
            self.repaired,
            reasons.join(", ")
        )
    }
    pub(super) fn merge(&mut self, other: Self) {
        self.skipped_documents += other.skipped_documents;
        self.skipped += other.skipped;
        self.repaired += other.repaired;
        self.omitted += other.omitted;
        for entry in other.entries {
            if self.entries.len() < MAX_DIAGNOSTICS {
                self.entries.push(entry)
            } else {
                self.omitted += 1
            }
        }
    }
    fn note(&mut self, section: &'static str, index: Option<usize>, issue: SessionIssue, skipped: bool) {
        if skipped {
            self.skipped += 1;
            if section == "documents" {
                self.skipped_documents += 1;
            }
        } else {
            self.repaired += 1
        }
        if self.entries.len() < MAX_DIAGNOSTICS {
            self.entries.push(SessionDiagnostic { section, index, issue });
        } else {
            self.omitted += 1
        }
    }
}
#[derive(Debug)]
pub struct DecodedSession {
    pub manifest: SessionManifest,
    pub diagnostics: SessionDiagnostics,
}
#[derive(Default)]
struct ValueBudget {
    nodes: usize,
    strings: usize,
    exceeded: bool,
    ambiguous: bool,
}
struct ValueSeed<'a>(&'a mut ValueBudget);
impl<'de> DeserializeSeed<'de> for ValueSeed<'_> {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<Value, D::Error> {
        self.0.nodes += 1;
        if self.0.nodes > MAX_VALUE_NODES {
            self.0.exceeded = true;
        }
        if self.0.exceeded {
            IgnoredAny::deserialize(de)?;
            return Ok(Value::Null);
        }
        de.deserialize_any(ValueVisitor(self.0))
    }
}
struct ValueVisitor<'a>(&'a mut ValueBudget);
impl ValueVisitor<'_> {
    fn string(&mut self, value: &str) -> bool {
        self.0.strings = self.0.strings.saturating_add(value.len());
        self.0.exceeded |= self.0.strings > MAX_VALUE_STRINGS;
        !self.0.exceeded
    }
}
impl<'de> Visitor<'de> for ValueVisitor<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("bounded JSON entry")
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
        Ok(serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number))
    }
    fn visit_str<E: serde::de::Error>(mut self, v: &str) -> Result<Value, E> {
        Ok(if self.string(v) {
            Value::String(v.into())
        } else {
            Value::Null
        })
    }
    fn visit_string<E: serde::de::Error>(mut self, v: String) -> Result<Value, E> {
        Ok(if self.string(&v) { Value::String(v) } else { Value::Null })
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(ValueSeed(&mut *self.0))? {
            if !self.0.exceeded {
                values.push(value);
            }
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if !self.string(&key) {
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            // Ambiguous duplicate fields invalidate this entry, never silently overwrite identity.
            if values.contains_key(&key) {
                self.0.exceeded = true;
                self.0.ambiguous = true;
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            let value = map.next_value_seed(ValueSeed(&mut *self.0))?;
            if !self.0.exceeded {
                values.insert(key, value);
            }
        }
        Ok(Value::Object(values))
    }
}
struct Entry(Option<Value>, bool);
impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let mut budget = ValueBudget::default();
        let value = ValueSeed(&mut budget).deserialize(de)?;
        Ok(Entry((!budget.exceeded).then_some(value), budget.ambiguous))
    }
}
trait SessionEntry: Sized {
    fn parse(value: Value, diagnostics: &mut SessionDiagnostics, section: &'static str, index: usize) -> Option<Self>;
}
macro_rules! plain_entry {
    ($ty:ty) => {
        impl SessionEntry for $ty {
            fn parse(
                value: Value,
                diagnostics: &mut SessionDiagnostics,
                section: &'static str,
                index: usize,
            ) -> Option<Self> {
                match serde_json::from_value(value) {
                    Ok(value) => Some(value),
                    Err(_) => {
                        diagnostics.note(section, Some(index), SessionIssue::InvalidEntry, true);
                        None
                    }
                }
            }
        }
    };
}
plain_entry!(SessionDocument);
plain_entry!(SerializedPath);
plain_entry!(u64);
impl SessionEntry for SessionTab {
    fn parse(value: Value, diagnostics: &mut SessionDiagnostics, section: &'static str, index: usize) -> Option<Self> {
        let raw = match serde_json::from_value::<RawTab>(value) {
            Ok(raw) => raw,
            Err(_) => {
                diagnostics.note(section, Some(index), SessionIssue::InvalidEntry, true);
                return None;
            }
        };
        let view = serde_json::from_value::<ViewState>(raw.view).ok().filter(|view| {
            view.folds.len() <= 100_000
                && view.folds.iter().all(|range| range.start < range.end)
                && f64::from_bits(view.scroll_y_bits).is_finite()
                && f64::from_bits(view.scroll_y_bits) >= 0.0
        });
        let view = view.unwrap_or_else(|| {
            diagnostics.note(section, Some(index), SessionIssue::InvalidView, false);
            ViewState::default()
        });
        Some(Self {
            id: raw.id,
            document_id: raw.document_id,
            pinned: raw.pinned,
            view,
        })
    }
}
struct Entries<'a, T> {
    section: &'static str,
    diagnostics: &'a mut SessionDiagnostics,
    kind: std::marker::PhantomData<T>,
}
impl<'de, T: SessionEntry> DeserializeSeed<'de> for Entries<'_, T> {
    type Value = Vec<(usize, T)>;
    fn deserialize<D: serde::Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        de.deserialize_seq(self)
    }
}
impl<'de, T: SessionEntry> Visitor<'de> for Entries<'_, T> {
    type Value = Vec<(usize, T)>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("session entry array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        let mut index = 0;
        while index < MAX_ENTRIES {
            let Some(entry) = seq.next_element::<Entry>()? else {
                return Ok(values);
            };
            match entry.0 {
                Some(value) => {
                    if let Some(value) = T::parse(value, self.diagnostics, self.section, index) {
                        values.push((index, value));
                    }
                }
                None => self.diagnostics.note(
                    self.section,
                    Some(index),
                    if entry.1 {
                        SessionIssue::DuplicateField
                    } else {
                        SessionIssue::ResourceLimit
                    },
                    true,
                ),
            }
            index += 1;
        }
        while seq.next_element::<IgnoredAny>()?.is_some() {
            self.diagnostics
                .note(self.section, Some(index), SessionIssue::ResourceLimit, true);
            index += 1;
        }
        Ok(values)
    }
}
struct RawSession {
    version: u32,
    documents: Vec<(usize, SessionDocument)>,
    tabs: Vec<(usize, SessionTab)>,
    active: Value,
    mru: Vec<(usize, u64)>,
    recent: Vec<(usize, SerializedPath)>,
    layout: Option<Value>,
    compare: Option<Value>,
    window: Option<Value>,
    diagnostics: SessionDiagnostics,
}
impl<'de> Deserialize<'de> for RawSession {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        de.deserialize_map(RawVisitor)
    }
}
struct RawVisitor;
impl<'de> Visitor<'de> for RawVisitor {
    type Value = RawSession;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("versioned session object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<RawSession, A::Error> {
        let mut seen = HashSet::new();
        let mut version = None;
        let mut documents = None;
        let mut tabs = None;
        let mut active = Value::Null;
        let mut mru = Vec::new();
        let mut recent = Vec::new();
        let mut layout = None;
        let mut compare = None;
        let mut window = None;
        let mut diagnostics = SessionDiagnostics::default();
        while let Some(key) = map.next_key::<String>()? {
            if !matches!(
                key.as_str(),
                "version" | "documents" | "tabs" | "active_tab" | "mru" | "recent" | "layout" | "compare" | "window"
            ) || !seen.insert(key.clone())
            {
                return Err(serde::de::Error::custom("unknown or duplicate session field"));
            }
            match key.as_str() {
                "version" => {
                    let value = map.next_value::<u32>()?;
                    if value != 0 && value != SESSION_VERSION {
                        return Err(serde::de::Error::custom("unsupported session version"));
                    }
                    version = Some(value);
                }
                "documents" => {
                    documents = Some(map.next_value_seed(Entries {
                        section: "documents",
                        diagnostics: &mut diagnostics,
                        kind: std::marker::PhantomData,
                    })?)
                }
                "tabs" => {
                    tabs = Some(map.next_value_seed(Entries {
                        section: "tabs",
                        diagnostics: &mut diagnostics,
                        kind: std::marker::PhantomData,
                    })?)
                }
                "mru" => {
                    mru = map.next_value_seed(Entries {
                        section: "mru",
                        diagnostics: &mut diagnostics,
                        kind: std::marker::PhantomData,
                    })?
                }
                "recent" => {
                    recent = map.next_value_seed(Entries {
                        section: "recent",
                        diagnostics: &mut diagnostics,
                        kind: std::marker::PhantomData,
                    })?
                }
                "active_tab" => {
                    let entry = map.next_value::<Entry>()?;
                    active = entry.0.unwrap_or_else(|| {
                        diagnostics.note("active_tab", None, SessionIssue::ResourceLimit, false);
                        Value::String(String::new())
                    });
                }
                "layout" => {
                    let entry = map.next_value::<Entry>()?;
                    layout = entry.0;
                    if layout.is_none() {
                        diagnostics.note("layout", None, SessionIssue::ResourceLimit, false);
                    }
                }
                "compare" => {
                    let entry = map.next_value::<Entry>()?;
                    compare = entry.0;
                    if compare.is_none() {
                        diagnostics.note("compare", None, SessionIssue::ResourceLimit, true);
                    }
                }
                "window" => {
                    let entry = map.next_value::<Entry>()?;
                    window = entry.0;
                }
                _ => unreachable!(),
            }
        }
        Ok(RawSession {
            version: version.ok_or_else(|| serde::de::Error::custom("missing session version"))?,
            documents: documents.ok_or_else(|| serde::de::Error::custom("missing session documents"))?,
            tabs: tabs.ok_or_else(|| serde::de::Error::custom("missing session tabs"))?,
            active,
            mru,
            recent,
            layout,
            compare,
            window,
            diagnostics,
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTab {
    id: u64,
    document_id: u64,
    pinned: bool,
    #[serde(default)]
    view: Value,
}

pub fn decode_report(bytes: &[u8]) -> io::Result<DecodedSession> {
    if bytes.len() > MAX_SESSION_BYTES {
        return Err(invalid("session byte limit"));
    }
    let raw: RawSession = serde_json::from_slice(bytes).map_err(|_| invalid("invalid session JSON or structure"))?;
    if raw.version != 0 && raw.version != SESSION_VERSION {
        return Err(invalid("unsupported session version"));
    }
    let mut diagnostics = raw.diagnostics;
    let mut manifest = SessionManifest::default();
    let mut docs = HashSet::new();
    for (index, doc) in raw.documents {
        if doc.title.len() > 4096 {
            diagnostics.note("documents", Some(index), SessionIssue::ResourceLimit, true);
            continue;
        }
        if doc.path.as_ref().is_some_and(|path| validate_path(path).is_err()) {
            diagnostics.note("documents", Some(index), SessionIssue::InvalidPath, true);
            continue;
        }
        if !docs.insert(doc.id) {
            diagnostics.note("documents", Some(index), SessionIssue::DuplicateIdentity, true);
            continue;
        }
        manifest.documents.push(doc);
    }
    let mut tabs = HashSet::new();
    let mut unpinned = false;
    for (index, mut tab) in raw.tabs {
        if !docs.contains(&tab.document_id) {
            diagnostics.note("tabs", Some(index), SessionIssue::InvalidReference, true);
            continue;
        }
        if !tabs.insert(tab.id) {
            diagnostics.note("tabs", Some(index), SessionIssue::DuplicateIdentity, true);
            continue;
        }
        if tab.pinned && unpinned {
            tab.pinned = false;
            diagnostics.note("tabs", Some(index), SessionIssue::PinOrder, false);
        }
        unpinned |= !tab.pinned;
        manifest.tabs.push(tab);
    }
    manifest.active_tab = raw.active.as_u64().filter(|id| tabs.contains(id));
    if !raw.active.is_null() && manifest.active_tab.is_none() {
        manifest.active_tab = manifest.tabs.first().map(|tab| tab.id);
        diagnostics.note("active_tab", None, SessionIssue::ActiveFallback, false);
    }
    let mut mru = HashSet::new();
    for (index, id) in raw.mru {
        if docs.contains(&id) && mru.insert(id) {
            manifest.mru.push(id)
        } else {
            diagnostics.note("mru", Some(index), SessionIssue::InvalidReference, true)
        }
    }
    for (index, path) in raw.recent {
        if validate_path(&path).is_ok() {
            manifest.recent.push(path)
        } else {
            diagnostics.note("recent", Some(index), SessionIssue::InvalidPath, true)
        }
    }
    if let Some(value) = raw.layout {
        match serde_json::from_value::<SessionLayout>(value) {
            Ok(layout) => manifest.layout = layout,
            Err(_) => diagnostics.note("layout", None, SessionIssue::InvalidLayout, false),
        }
    }
    if !manifest.valid_layout() {
        manifest.layout = SessionLayout::default();
        manifest.layout.active_tabs[0] = manifest.active_tab;
        for tab in &mut manifest.tabs {
            tab.view.split = 0;
        }
        diagnostics.note("layout", None, SessionIssue::InvalidLayout, false);
    }
    let colors = manifest.layout.tab_colors.len();
    manifest
        .layout
        .tab_colors
        .retain(|id, color| tabs.contains(id) && *color <= 0xff_ffff);
    if manifest.layout.tab_colors.len() != colors {
        diagnostics.note("layout", None, SessionIssue::InvalidReference, false);
    }
    if let Some(value) = raw.compare.filter(|value| !value.is_null()) {
        match serde_json::from_value::<SessionCompare>(value) {
            Ok(compare)
                if compare.version == 1
                    && compare.options_json.len() <= 65536
                    && compare.left_document != compare.right_document
                    && docs.contains(&compare.left_document)
                    && docs.contains(&compare.right_document) =>
            {
                manifest.compare = Some(compare)
            }
            _ => diagnostics.note("compare", None, SessionIssue::InvalidComparison, true),
        }
    }
    if let Some(value) = raw.window.filter(|value| !value.is_null()) {
        match serde_json::from_value::<SessionWindow>(value) {
            Ok(window) => manifest.window = Some(window),
            Err(_) => diagnostics.note("window", None, SessionIssue::InvalidEntry, false),
        }
    }
    manifest.validate()?;
    Ok(DecodedSession { manifest, diagnostics })
}
