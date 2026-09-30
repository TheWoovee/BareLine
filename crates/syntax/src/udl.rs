// SPDX-License-Identifier: MPL-2.0
//! Data-only versioned UDL. Replacement validates completely before publishing.
use crate::{Cancellation, Error, MAX_REQUEST_BYTES, MAX_SPANS, StyleKind, StyleSpan};
use bareline_document::TextOffset;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub extensions: Vec<String>,
    pub keywords: Vec<String>,
    pub operators: String,
    pub line_comment: Option<String>,
    pub block_comment: Option<(String, String)>,
    pub strings: Vec<char>,
    pub fold_pairs: Vec<(char, char)>,
}
impl Definition {
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != 1
            || self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || self.name.len() > 128
            || self.extensions.len() > 64
            || self.keywords.len() > 8192
            || self.operators.len() > 256
            || self.strings.len() > 8
            || self.fold_pairs.len() > 16
        {
            return Err(Error::BudgetExceeded);
        }
        let mut bytes = 0;
        for word in self.keywords.iter().chain(&self.extensions) {
            bytes += word.len();
            if word.is_empty() || word.len() > 128 || bytes > 64 << 10 {
                return Err(Error::BudgetExceeded);
            }
        }
        for token in self
            .line_comment
            .iter()
            .chain(self.block_comment.iter().flat_map(|(a, b)| [a, b]))
        {
            if token.is_empty() || token.len() > 32 || token.contains(['\n', '\r']) {
                return Err(Error::InvalidRange);
            }
        }
        let mut delimiters = std::collections::BTreeSet::new();
        for &(open, close) in &self.fold_pairs {
            if open == close
                || open.is_control()
                || close.is_control()
                || !delimiters.insert(open)
                || !delimiters.insert(close)
            {
                return Err(Error::InvalidRange);
            }
        }
        if self.strings.iter().any(|c| c.is_control()) || self.operators.contains(['\r', '\n', '\0']) {
            return Err(Error::InvalidRange);
        }
        Ok(())
    }
    pub fn from_json(text: &str) -> Result<Self, Error> {
        if text.len() > 128 << 10 {
            return Err(Error::BudgetExceeded);
        }
        let result: Self = serde_json::from_str(text).map_err(|_| Error::InvalidRange)?;
        result.validate()?;
        Ok(result)
    }
    pub fn to_json(&self) -> Result<String, Error> {
        self.validate()?;
        serde_json::to_string_pretty(self).map_err(|_| Error::InvalidRange)
    }
}
#[derive(Default)]
pub struct Registry {
    definitions: std::collections::BTreeMap<String, Definition>,
}
impl Registry {
    pub fn get(&self, id: &str) -> Option<&Definition> {
        self.definitions.get(id)
    }
    pub fn replace_json(&mut self, text: &str) -> Result<(), Error> {
        let definition = Definition::from_json(text)?;
        if self.definitions.len() >= 128 && !self.definitions.contains_key(&definition.id) {
            return Err(Error::BudgetExceeded);
        }
        self.definitions.insert(definition.id.clone(), definition);
        Ok(())
    }
}
pub fn lex(text: &str, definition: &Definition, cancel: &Cancellation) -> Result<Vec<StyleSpan>, Error> {
    definition.validate()?;
    if text.len() > MAX_REQUEST_BYTES {
        return Err(Error::BudgetExceeded);
    }
    let keywords: std::collections::BTreeSet<_> = definition.keywords.iter().map(String::as_str).collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < text.len() {
        cancel.check()?;
        let start = i;
        let rest = &text[i..];
        let c = rest.chars().next().unwrap();
        let kind = if let Some(token) = definition
            .line_comment
            .as_ref()
            .filter(|t| rest.starts_with(t.as_str()))
        {
            i += token.len();
            while i < text.len() && !matches!(text.as_bytes()[i], b'\r' | b'\n') {
                i += text[i..].chars().next().unwrap().len_utf8();
            }
            Some(StyleKind::Comment)
        } else if let Some((open, close)) = definition
            .block_comment
            .as_ref()
            .filter(|(a, _)| rest.starts_with(a.as_str()))
        {
            i += open.len();
            i += text[i..].find(close).map_or(text.len() - i, |n| n + close.len());
            Some(StyleKind::Comment)
        } else if definition.strings.contains(&c) {
            i += c.len_utf8();
            let mut escaped = false;
            while i < text.len() {
                let next = text[i..].chars().next().unwrap();
                i += next.len_utf8();
                if !escaped && next == c {
                    break;
                }
                escaped = !escaped && next == '\\';
            }
            Some(StyleKind::String)
        } else if c == '_' || c.is_alphabetic() {
            i += c.len_utf8();
            while i < text.len() {
                let next = text[i..].chars().next().unwrap();
                if next != '_' && !next.is_alphanumeric() {
                    break;
                }
                i += next.len_utf8();
            }
            keywords.contains(&text[start..i]).then_some(StyleKind::Keyword)
        } else if c.is_ascii_digit() {
            i += 1;
            while i < text.len() && (text.as_bytes()[i].is_ascii_alphanumeric() || b"._".contains(&text.as_bytes()[i]))
            {
                i += 1;
            }
            Some(StyleKind::Number)
        } else {
            i += c.len_utf8();
            definition.operators.contains(c).then_some(StyleKind::Operator)
        };
        if let Some(kind) = kind {
            if spans.len() >= MAX_SPANS {
                return Err(Error::BudgetExceeded);
            }
            spans.push(StyleSpan {
                range: TextOffset(start)..TextOffset(i),
                kind,
            });
        }
    }
    Ok(spans)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_replacement_preserves_definition_and_utf8_lex() {
        let definition = Definition {
            version: 1,
            id: "custom".into(),
            name: "Custom".into(),
            extensions: vec!["mine".into()],
            keywords: vec!["hello".into()],
            operators: "{}".into(),
            line_comment: Some("#".into()),
            block_comment: None,
            strings: vec!['"'],
            fold_pairs: vec![('{', '}')],
        };
        let mut registry = Registry::default();
        registry.replace_json(&definition.to_json().unwrap()).unwrap();
        assert!(registry.replace_json(r#"{"version":2}"#).is_err());
        let d = registry.get("custom").unwrap();
        let text = "hello 🦀 # comment";
        let spans = lex(text, d, &Cancellation::default()).unwrap();
        assert_eq!(spans[0].kind, StyleKind::Keyword);
        assert_eq!(spans.last().unwrap().kind, StyleKind::Comment);
        assert!(
            spans
                .iter()
                .all(|s| text.is_char_boundary(s.range.start.0) && text.is_char_boundary(s.range.end.0))
        );
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MappingKind {
    Imported,
    Approximated,
    Unsupported,
}
#[derive(Clone, Debug)]
pub struct Mapping {
    pub field: String,
    pub kind: MappingKind,
    pub reason: String,
}
impl Mapping {
    fn new(field: &str, kind: MappingKind, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            kind,
            reason: reason.into(),
        }
    }
}
/// Report field of the note [`resolve_collisions`] adds when an import replaces
/// or is renamed around an installed or built-in language.
pub const NAME_FIELD: &str = "name";
/// Bounded XML subset importer. DTDs/entities/includes are forbidden; no filesystem/network access.
/// Every UDL list is mapped onto the definition model or reported as approximated
/// or unsupported; nothing is dropped without a report entry (SRC-19).
pub fn import_notepad_xml(xml: &str) -> Result<(Definition, Vec<Mapping>), Error> {
    if xml.len() > 128 << 10 {
        return Err(Error::BudgetExceeded);
    }
    if xml.contains("<!DOCTYPE") || xml.contains("<!ENTITY") {
        return Err(Error::InvalidRange);
    }
    let mut definition = Definition {
        version: 1,
        id: "imported".into(),
        name: "Imported language".into(),
        extensions: Vec::new(),
        keywords: Vec::new(),
        operators: String::new(),
        line_comment: None,
        block_comment: None,
        strings: Vec::new(),
        fold_pairs: Vec::new(),
    };
    let mut lists = Vec::<(String, String)>::new();
    let mut global = BTreeMap::new();
    let mut prefix = BTreeMap::new();
    let mut styles = 0;
    let mut stack = Vec::<String>::new();
    let mut cursor = 0;
    let mut keyword: Option<(String, usize)> = None;
    let mut found = false;
    while cursor < xml.len() {
        let Some(relative) = xml[cursor..].find('<') else {
            if !xml[cursor..].trim().is_empty() {
                return Err(Error::InvalidRange);
            }
            break;
        };
        let start = cursor + relative;
        if xml[start..].starts_with("<!--") {
            cursor = start + 4 + xml[start + 4..].find("-->").ok_or(Error::InvalidRange)? + 3;
            continue;
        }
        if xml[start..].starts_with("<?") {
            cursor = start + 2 + xml[start + 2..].find("?>").ok_or(Error::InvalidRange)? + 2;
            continue;
        }
        let end = start + xml[start..].find('>').ok_or(Error::InvalidRange)?;
        let tag = xml[start + 1..end].trim();
        if let Some(closing) = tag.strip_prefix('/') {
            let closing = closing.trim();
            if stack.pop().as_deref() != Some(closing) {
                return Err(Error::InvalidRange);
            }
            if closing == "Keywords"
                && let Some((name, begin)) = keyword.take()
            {
                lists.push((name, xml_unescape(&xml[begin..start])?));
            }
        } else {
            let self_closing = tag.ends_with('/');
            let tag = tag.trim_end_matches('/').trim();
            let split = tag.find(char::is_whitespace).unwrap_or(tag.len());
            let name = &tag[..split];
            if name.is_empty() || name.starts_with('!') {
                return Err(Error::InvalidRange);
            }
            let attrs = xml_attributes(&tag[split..])?;
            match name {
                "UserLang" => {
                    if found {
                        return Err(Error::InvalidRange);
                    }
                    found = true;
                    if let Some(name) = attrs.get("name") {
                        definition.name = name.clone();
                        definition.id = derive_id(name);
                    }
                    if let Some(ext) = attrs.get("ext") {
                        definition.extensions = ext.split_whitespace().map(str::to_owned).collect();
                    }
                }
                "Keywords" => {
                    if keyword.is_some() {
                        return Err(Error::InvalidRange);
                    }
                    keyword = Some((attrs.get("name").cloned().unwrap_or_default(), end + 1));
                }
                "Global" => global = attrs,
                "Prefix" => prefix = attrs,
                "WordsStyle" => styles += 1,
                _ => {}
            }
            if !self_closing {
                if stack.len() >= 64 {
                    return Err(Error::BudgetExceeded);
                }
                stack.push(name.into());
            }
        }
        cursor = end + 1;
    }
    if !found || !stack.is_empty() || keyword.is_some() {
        return Err(Error::InvalidRange);
    }
    let mut report = Vec::new();
    map_lists(&mut definition, &lists, &mut report);
    map_settings(&global, &prefix, styles, &mut report);
    definition.validate()?;
    Ok((definition, report))
}
/// End-of-line marker Notepad++ writes as a delimiter or comment close.
const EOL: &str = "((EOL))";
fn map_lists(definition: &mut Definition, lists: &[(String, String)], report: &mut Vec<Mapping>) {
    for (name, value) in lists {
        if value.trim().is_empty() {
            continue;
        }
        if name.starts_with("Keywords") || name.starts_with("Words") {
            map_keywords(definition, name, value, report);
        } else if name.starts_with("Operators") {
            let mut dropped = Vec::new();
            for c in udl_words(value)
                .iter()
                .flat_map(|word| ungroup(word).chars().collect::<Vec<_>>())
            {
                if c.is_whitespace() || c.is_control() || definition.operators.contains(c) {
                    continue;
                }
                if definition.operators.len() + c.len_utf8() > 256 {
                    dropped.push(c.to_string());
                } else {
                    definition.operators.push(c);
                }
            }
            report.push(Mapping::new(
                name,
                MappingKind::Approximated,
                "Operators are styled one character at a time",
            ));
            if !dropped.is_empty() {
                report.push(Mapping::new(
                    name,
                    MappingKind::Unsupported,
                    format!("Operator budget reached; not imported: {}", sample(&dropped)),
                ));
            }
        } else if name == "Comments" {
            map_comments(definition, value, report);
        } else if name == "Delimiters" {
            map_delimiters(definition, value, report);
        } else if !name.starts_with("Folder") && !name.starts_with("Numbers") {
            report.push(Mapping::new(
                name,
                MappingKind::Unsupported,
                "Unrecognised keyword list; not imported",
            ));
        }
    }
    if !lists.iter().any(|(name, _)| name == "Delimiters") {
        definition.strings = vec!['"', '\''];
        report.push(Mapping::new(
            "Delimiters",
            MappingKind::Approximated,
            "No delimiter list; double and single quotes delimit strings",
        ));
    }
    if !map_folders(definition, lists, report) {
        definition.fold_pairs = vec![('{', '}')];
        report.push(Mapping::new(
            "Folders",
            MappingKind::Approximated,
            "No folding markers; braces fold",
        ));
    }
    map_numbers(lists, report);
}
fn map_keywords(definition: &mut Definition, name: &str, value: &str, report: &mut Vec<Mapping>) {
    let mut count = 0;
    let mut phrases = Vec::new();
    let mut symbolic = Vec::new();
    for word in udl_words(value) {
        let word = ungroup(&word);
        if word.contains(char::is_whitespace) {
            phrases.push(word.to_owned());
            continue;
        }
        let mut chars = word.chars();
        if !chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
            || !chars.all(|c| c == '_' || c.is_alphanumeric())
        {
            symbolic.push(word.to_owned());
        }
        definition.keywords.push(word.to_owned());
        count += 1;
    }
    if count > 0 {
        report.push(Mapping::new(
            name,
            MappingKind::Imported,
            format!("Keyword set imported ({count} words)"),
        ));
    }
    if !phrases.is_empty() {
        report.push(Mapping::new(
            name,
            MappingKind::Unsupported,
            format!("Keywords containing spaces are not supported: {}", sample(&phrases)),
        ));
    }
    if !symbolic.is_empty() {
        report.push(Mapping::new(
            name,
            MappingKind::Approximated,
            format!(
                "Keywords with symbols are offered for completion but not highlighted: {}",
                sample(&symbolic)
            ),
        ));
    }
}
/// `Comments` slots: 00 line open, 01 line continuation, 02 line close,
/// 03 block open, 04 block close.
fn map_comments(definition: &mut Definition, value: &str, report: &mut Vec<Mapping>) {
    const FIELD: &str = "Comments";
    let Some(entries) = coded_words(value) else {
        report.push(Mapping::new(
            FIELD,
            MappingKind::Unsupported,
            "Unrecognised comment encoding; comments not imported",
        ));
        return;
    };
    let mut lost = Vec::new();
    for token in slot(&entries, 0) {
        if definition.line_comment.is_none() && comment_token(token) {
            definition.line_comment = Some(token.into());
            report.push(Mapping::new(
                FIELD,
                MappingKind::Imported,
                format!("Line comment {token}"),
            ));
        } else {
            lost.push(format!("line comment {token}"));
        }
    }
    let (opens, closes) = (slot(&entries, 3), slot(&entries, 4));
    for (index, open) in opens.iter().enumerate() {
        match closes.get(index).copied() {
            Some(EOL) if definition.line_comment.is_none() && comment_token(open) => {
                definition.line_comment = Some((*open).into());
                report.push(Mapping::new(
                    FIELD,
                    MappingKind::Imported,
                    format!("Comment {open} to the end of the line"),
                ));
            }
            Some(close) if definition.block_comment.is_none() && comment_token(open) && comment_token(close) => {
                definition.block_comment = Some(((*open).into(), close.into()));
                report.push(Mapping::new(
                    FIELD,
                    MappingKind::Imported,
                    format!("Block comment {open} … {close}"),
                ));
            }
            close => lost.push(format!("block comment {open} … {}", close.unwrap_or("(no close)"))),
        }
    }
    lost.extend(
        closes
            .iter()
            .skip(opens.len())
            .map(|close| format!("block comment close {close}")),
    );
    lost.extend(
        entries
            .iter()
            .filter(|(slot, _)| *slot > 4)
            .map(|(slot, token)| format!("slot {slot:02} {token}")),
    );
    for token in slot(&entries, 1) {
        report.push(Mapping::new(
            FIELD,
            MappingKind::Unsupported,
            format!("Line comment continuation {token} is not supported"),
        ));
    }
    for token in slot(&entries, 2).into_iter().filter(|token| *token != EOL) {
        report.push(Mapping::new(
            FIELD,
            MappingKind::Approximated,
            format!("Line comments end at the end of the line; close {token} ignored"),
        ));
    }
    if !lost.is_empty() {
        report.push(Mapping::new(
            FIELD,
            MappingKind::Approximated,
            format!(
                "One line and one block comment are supported; not imported: {}",
                sample(&lost)
            ),
        ));
    }
}
/// `Delimiters` slots: three per delimiter (open, escape, close) for eight delimiters.
fn map_delimiters(definition: &mut Definition, value: &str, report: &mut Vec<Mapping>) {
    let Some(entries) = coded_words(value) else {
        report.push(Mapping::new(
            "Delimiters",
            MappingKind::Unsupported,
            "Unrecognised delimiter encoding; delimiters not imported",
        ));
        return;
    };
    for group in 0..8 {
        let opens = slot(&entries, group * 3);
        let escapes = slot(&entries, group * 3 + 1);
        let closes = slot(&entries, group * 3 + 2);
        if opens.is_empty() && escapes.is_empty() && closes.is_empty() {
            continue;
        }
        let field = format!("Delimiters {}", group + 1);
        let mut quotes = String::new();
        let mut rejected = Vec::new();
        for (index, open) in opens.iter().enumerate() {
            let close = closes.get(index).copied().filter(|_| opens.len() == closes.len());
            match symbol_char(open) {
                Some(c)
                    if close == Some(*open) && (definition.strings.contains(&c) || definition.strings.len() < 8) =>
                {
                    if !definition.strings.contains(&c) {
                        definition.strings.push(c);
                    }
                    quotes.push(c);
                }
                _ => rejected.push(format!("{open} … {}", close.unwrap_or("(unpaired)"))),
            }
        }
        rejected.extend(
            closes
                .iter()
                .skip(opens.len())
                .map(|close| format!("(unpaired) … {close}")),
        );
        if !quotes.is_empty() {
            let (kind, reason) = match escapes.as_slice() {
                ["\\"] => (
                    MappingKind::Imported,
                    format!("Strings quoted by {quotes} with \\ escapes"),
                ),
                [] => (
                    MappingKind::Approximated,
                    format!("Strings quoted by {quotes}; \\ escapes are always recognised"),
                ),
                other => (
                    MappingKind::Approximated,
                    format!("Strings quoted by {quotes}; escape {} is read as \\", other.join(" ")),
                ),
            };
            report.push(Mapping::new(&field, kind, reason));
        }
        if !rejected.is_empty() {
            report.push(Mapping::new(
                &field,
                MappingKind::Unsupported,
                format!(
                    "Strings must open and close with the same single symbol; not imported: {}",
                    sample(&rejected)
                ),
            ));
        }
    }
    let unknown: Vec<_> = entries
        .iter()
        .filter(|(slot, _)| *slot >= 24)
        .map(|(slot, token)| format!("slot {slot:02} {token}"))
        .collect();
    if !unknown.is_empty() {
        report.push(Mapping::new(
            "Delimiters",
            MappingKind::Unsupported,
            format!("Unknown delimiter slots: {}", sample(&unknown)),
        ));
    }
}
/// Maps code folding markers onto single-character fold pairs. Returns whether
/// the file declared any code folding list, even an empty one.
fn map_folders(definition: &mut Definition, lists: &[(String, String)], report: &mut Vec<Mapping>) -> bool {
    let list = |name: &str| -> Vec<String> {
        lists
            .iter()
            .filter(|(candidate, _)| candidate == name)
            .flat_map(|(_, value)| udl_words(value))
            .map(|word| ungroup(&word).to_owned())
            .collect()
    };
    let mut declared = false;
    for (field, open, middle, close) in [
        (
            "Folders in code1",
            "Folders in code1, open",
            Some("Folders in code1, middle"),
            "Folders in code1, close",
        ),
        (
            "Folders in code2",
            "Folders in code2, open",
            Some("Folders in code2, middle"),
            "Folders in code2, close",
        ),
        ("Folder+/Folder-", "Folder+", None, "Folder-"),
    ] {
        declared |= lists.iter().any(|(name, _)| name == open || name == close);
        let (opens, closes) = (list(open), list(close));
        let mut imported = Vec::new();
        let mut rejected = Vec::new();
        for (index, open) in opens.iter().enumerate() {
            let close = closes.get(index).filter(|_| opens.len() == closes.len());
            match close {
                Some(close) if add_fold(&mut definition.fold_pairs, open, close) => {
                    imported.push(format!("{open} … {close}"));
                }
                _ => rejected.push(format!("{open} … {}", close.map_or("(unpaired)", String::as_str))),
            }
        }
        rejected.extend(
            closes
                .iter()
                .skip(opens.len())
                .map(|close| format!("(unpaired) … {close}")),
        );
        if !imported.is_empty() {
            report.push(Mapping::new(
                field,
                MappingKind::Imported,
                format!("Fold pairs {}", imported.join(", ")),
            ));
        }
        if !rejected.is_empty() {
            report.push(Mapping::new(
                field,
                MappingKind::Unsupported,
                format!(
                    "Only single-symbol fold pairs are supported; not imported: {}",
                    sample(&rejected)
                ),
            ));
        }
        let middles = middle.map(list).unwrap_or_default();
        if !middles.is_empty() {
            report.push(Mapping::new(
                field,
                MappingKind::Unsupported,
                format!("Middle fold markers are not supported: {}", sample(&middles)),
            ));
        }
    }
    let comment: Vec<String> = [
        "Folders in comment, open",
        "Folders in comment, middle",
        "Folders in comment, close",
    ]
    .into_iter()
    .flat_map(list)
    .collect();
    if !comment.is_empty() {
        report.push(Mapping::new(
            "Folders in comment",
            MappingKind::Unsupported,
            format!("Folding inside comments is not supported: {}", sample(&comment)),
        ));
    }
    declared
}
fn add_fold(pairs: &mut Vec<(char, char)>, open: &str, close: &str) -> bool {
    let (Some(open), Some(close)) = (symbol_char(open), symbol_char(close)) else {
        return false;
    };
    if pairs.contains(&(open, close)) {
        return true;
    }
    if open == close
        || pairs.len() >= 16
        || pairs
            .iter()
            .any(|&(a, b)| [a, b].contains(&open) || [a, b].contains(&close))
    {
        return false;
    }
    pairs.push((open, close));
    true
}
/// Numbers always use the built-in style: a digit followed by letters, digits,
/// `.` or `_`. Custom prefixes, extras and suffixes are reported against it.
fn map_numbers(lists: &[(String, String)], report: &mut Vec<Mapping>) {
    let tail = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_';
    let mut covered = Vec::new();
    let mut missed = Vec::new();
    for (name, value) in lists.iter().filter(|(name, _)| name.starts_with("Numbers")) {
        for word in udl_words(value) {
            let word = ungroup(&word).to_owned();
            let recognised = if name.contains("range") {
                false
            } else if name.contains("prefix") {
                word.starts_with(|c: char| c.is_ascii_digit()) && word.chars().all(tail)
            } else {
                word.chars().all(tail)
            };
            if recognised {
                covered.push(word);
            } else {
                missed.push(word);
            }
        }
    }
    if !covered.is_empty() {
        report.push(Mapping::new(
            "Numbers",
            MappingKind::Imported,
            format!("Covered by the built-in number style: {}", sample(&covered)),
        ));
    }
    if !missed.is_empty() {
        report.push(Mapping::new(
            "Numbers",
            MappingKind::Approximated,
            format!(
                "Numbers start with a digit and continue with letters, digits, . or _; not recognised: {}",
                sample(&missed)
            ),
        ));
    }
}
fn map_settings(
    global: &BTreeMap<String, String>,
    prefix: &BTreeMap<String, String>,
    styles: usize,
    report: &mut Vec<Mapping>,
) {
    let set = |key: &str, default: &str| global.get(key).is_some_and(|value| value != default);
    for (key, default, kind, reason) in [
        (
            "caseIgnored",
            "no",
            MappingKind::Approximated,
            "Keywords match case-sensitively",
        ),
        (
            "allowFoldOfComments",
            "no",
            MappingKind::Unsupported,
            "Folding of comments is not supported",
        ),
        (
            "foldCompact",
            "no",
            MappingKind::Approximated,
            "Compact folding is not configurable",
        ),
        (
            "forcePureLC",
            "0",
            MappingKind::Approximated,
            "Line comments are recognised anywhere on a line",
        ),
        (
            "decimalSeparator",
            "0",
            MappingKind::Approximated,
            "Only . is recognised as a decimal separator",
        ),
    ] {
        if set(key, default) {
            report.push(Mapping::new(&format!("Global {key}"), kind, reason));
        }
    }
    let prefixed: Vec<String> = prefix
        .iter()
        .filter(|(_, value)| *value == "yes")
        .map(|(key, _)| key.clone())
        .collect();
    if !prefixed.is_empty() {
        report.push(Mapping::new(
            "Prefix",
            MappingKind::Unsupported,
            format!(
                "Prefix matching is not supported; these sets match whole words: {}",
                sample(&prefixed)
            ),
        ));
    }
    if styles > 0 {
        report.push(Mapping::new(
            "WordsStyle",
            MappingKind::Approximated,
            format!("{styles} styles: colours and fonts follow the Bareline theme"),
        ));
    }
}
/// Splits a UDL list on whitespace. `((a b))` groups keep their spaces:
/// Notepad++ writes multi-word entries and the `((EOL))` marker that way.
fn udl_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut group: Option<String> = None;
    for piece in value.split_whitespace() {
        if let Some(open) = group.as_mut() {
            open.push(' ');
            open.push_str(piece);
            if piece.ends_with("))") {
                words.extend(group.take());
            }
        } else if piece.contains("((") && !piece.ends_with("))") {
            group = Some(piece.to_owned());
        } else {
            words.push(piece.to_owned());
        }
    }
    words.extend(group);
    words
}
fn ungroup(word: &str) -> &str {
    word.strip_prefix("((")
        .and_then(|inner| inner.strip_suffix("))"))
        .unwrap_or(word)
}
/// Words of the `Comments` and `Delimiters` lists, each prefixed with its two-digit slot.
fn coded_words(value: &str) -> Option<Vec<(usize, String)>> {
    let mut entries = Vec::new();
    for word in udl_words(value) {
        let slot = word
            .get(..2)
            .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))?
            .parse()
            .ok()?;
        let token = &word[2..];
        if !token.is_empty() {
            entries.push((slot, (if token == EOL { token } else { ungroup(token) }).to_owned()));
        }
    }
    Some(entries)
}
fn slot(entries: &[(usize, String)], slot: usize) -> Vec<&str> {
    entries
        .iter()
        .filter(|(n, _)| *n == slot)
        .map(|(_, token)| token.as_str())
        .collect()
}
fn comment_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 32 && token != EOL && !token.contains(['\n', '\r'])
}
/// A lone symbol usable as a quote or fold delimiter; the lexers read letters,
/// digits and `_` as words.
fn symbol_char(token: &str) -> Option<char> {
    let mut chars = token.chars();
    let c = chars.next()?;
    (chars.next().is_none() && !c.is_control() && !c.is_whitespace() && !c.is_alphanumeric() && c != '_').then_some(c)
}
fn sample(items: &[String]) -> String {
    let mut text = items.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > 8 {
        text.push_str(&format!(" and {} more", items.len() - 8));
    }
    text
}
/// Name identity for collisions: full Unicode case folding (the folding search
/// uses) with whitespace runs collapsed, so `QA  Test` and `qa test` are one language.
pub fn name_key(name: &str) -> String {
    bareline_unicode_fold::fold(name)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
/// ASCII names keep their historical IDs. A name with other characters also
/// gets a stable hash of its [`name_key`], so distinct non-ASCII names no longer
/// map to the same dashes and replace each other.
fn derive_id(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    if name.is_ascii() {
        return slug;
    }
    // FNV-1a: fixed, so the ID is the same on every run and machine.
    let hash = name_key(name).bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    let stem = slug.trim_matches('-');
    let stem = if stem.is_empty() {
        "udl"
    } else {
        &stem[..stem.len().min(55)]
    };
    format!("{stem}-{hash:08x}")
}
/// Fits an import into the catalog before it is installed. An installed language
/// with the same [`name_key`] is replaced (the same language imported again).
/// A name matching a built-in language, or an ID used by a built-in or another
/// installed language (IDs compare case-insensitively, like the file names that
/// store them), gets a numeric suffix instead of silently replacing it. Returns
/// the note to show the user, if anything happened.
pub fn resolve_collisions(definition: &mut Definition, installed: &[&Definition]) -> Result<Option<Mapping>, Error> {
    let builtins = || {
        crate::catalog::CATALOG
            .iter()
            .chain([crate::Language::PlainText.metadata()])
    };
    let requested_name = definition.name.clone();
    let requested_id = definition.id.clone();
    let mut name = requested_name.clone();
    let mut key = name_key(&name);
    let mut n = 1;
    while builtins().any(|builtin| name_key(builtin.label) == key) {
        n += 1;
        if n > 999 {
            return Err(Error::BudgetExceeded);
        }
        name = with_suffix(&requested_name, &format!(" ({n})"), 128);
        key = name_key(&name);
    }
    let renamed = name != requested_name;
    if let Some(existing) = installed.iter().find(|installed| name_key(&installed.name) == key) {
        definition.id = existing.id.clone();
        definition.name = name;
        let reason = if renamed {
            format!(
                "{requested_name} is a built-in language; replaces the installed {}",
                existing.name
            )
        } else {
            format!("Replaces the installed {} ({})", existing.name, existing.id)
        };
        return Ok(Some(Mapping::new(NAME_FIELD, MappingKind::Imported, reason)));
    }
    let taken = |id: &str| {
        builtins().any(|builtin| builtin.id.eq_ignore_ascii_case(id))
            || installed.iter().any(|installed| installed.id.eq_ignore_ascii_case(id))
    };
    let mut id = requested_id.clone();
    let mut n = 1;
    while taken(&id) {
        n += 1;
        if n > 999 {
            return Err(Error::BudgetExceeded);
        }
        id = with_suffix(&requested_id, &format!("-{n}"), 64);
    }
    if !renamed && id == requested_id {
        return Ok(None);
    }
    let clash = if renamed {
        "a built-in language".to_owned()
    } else {
        installed
            .iter()
            .find(|installed| installed.id.eq_ignore_ascii_case(&requested_id))
            .map_or_else(
                || "a built-in language".to_owned(),
                |installed| format!("the installed {}", installed.name),
            )
    };
    definition.name = name;
    definition.id = id;
    Ok(Some(Mapping::new(
        NAME_FIELD,
        MappingKind::Approximated,
        format!(
            "{requested_name} collides with {clash}; installed as {} ({})",
            definition.name, definition.id
        ),
    )))
}
fn with_suffix(base: &str, suffix: &str, limit: usize) -> String {
    let mut end = base.len().min(limit.saturating_sub(suffix.len()));
    while !base.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &base[..end])
}
fn xml_attributes(mut text: &str) -> Result<std::collections::BTreeMap<String, String>, Error> {
    let mut attrs = std::collections::BTreeMap::new();
    while !text.trim().is_empty() {
        text = text.trim_start();
        let end = text.find('=').ok_or(Error::InvalidRange)?;
        let name = text[..end].trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':' || c == '-')
        {
            return Err(Error::InvalidRange);
        }
        text = text[end + 1..].trim_start();
        let quote = text
            .chars()
            .next()
            .filter(|c| *c == '"' || *c == '\'')
            .ok_or(Error::InvalidRange)?;
        text = &text[1..];
        let end = text.find(quote).ok_or(Error::InvalidRange)?;
        if attrs.insert(name.into(), xml_unescape(&text[..end])?).is_some() {
            return Err(Error::InvalidRange);
        }
        text = &text[end + 1..];
    }
    Ok(attrs)
}
fn xml_unescape(text: &str) -> Result<String, Error> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let end = start + rest[start..].find(';').ok_or(Error::InvalidRange)?;
        let entity = &rest[start + 1..end];
        let c = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    entity.strip_prefix('#').and_then(|n| n.parse::<u32>().ok())
                };
                code.and_then(char::from_u32).ok_or(Error::InvalidRange)?
            }
        };
        out.push(c);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}
#[cfg(test)]
mod import_tests {
    use super::*;
    /// A UDL 2.1 file written for these tests, exercising every list the importer maps.
    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<NotepadPlus>
    <UserLang name="Fixture Lang" ext="fxl fx2" udlVersion="2.1">
        <Settings>
            <Global caseIgnored="yes" allowFoldOfComments="no" foldCompact="no" forcePureLC="0" decimalSeparator="0" />
            <Prefix Keywords1="no" Keywords2="yes" />
        </Settings>
        <KeywordLists>
            <Keywords name="Comments">00// 00# 01 02((EOL)) 03/* 04*/</Keywords>
            <Keywords name="Numbers, prefix1">0x</Keywords>
            <Keywords name="Numbers, prefix2">$</Keywords>
            <Keywords name="Numbers, extras1">A B C D E F</Keywords>
            <Keywords name="Numbers, suffix1">L</Keywords>
            <Keywords name="Numbers, range"></Keywords>
            <Keywords name="Operators1">+ - :=</Keywords>
            <Keywords name="Operators2"></Keywords>
            <Keywords name="Folders in code1, open">{ begin</Keywords>
            <Keywords name="Folders in code1, middle">else</Keywords>
            <Keywords name="Folders in code1, close">} end</Keywords>
            <Keywords name="Folders in code2, open">(</Keywords>
            <Keywords name="Folders in code2, middle"></Keywords>
            <Keywords name="Folders in code2, close">)</Keywords>
            <Keywords name="Folders in comment, open">region</Keywords>
            <Keywords name="Folders in comment, middle"></Keywords>
            <Keywords name="Folders in comment, close">endregion</Keywords>
            <Keywords name="Keywords1">if else ((end if)) #include</Keywords>
            <Keywords name="Keywords2">print</Keywords>
            <Keywords name="Delimiters">00" 01\ 02" 03' 04 05' 06[ 07 08] 09` 10^ 11`</Keywords>
        </KeywordLists>
        <Styles>
            <WordsStyle name="DEFAULT" fgColor="000000" bgColor="FFFFFF" fontStyle="0" nesting="0" />
            <WordsStyle name="COMMENTS" fgColor="008000" bgColor="FFFFFF" fontStyle="0" nesting="0" />
        </Styles>
    </UserLang>
</NotepadPlus>"#;
    fn noted(report: &[Mapping], kind: MappingKind, needle: &str) -> bool {
        report.iter().any(|r| r.kind == kind && r.reason.contains(needle))
    }
    fn definition(id: &str, name: &str) -> Definition {
        Definition {
            version: 1,
            id: id.into(),
            name: name.into(),
            extensions: Vec::new(),
            keywords: Vec::new(),
            operators: String::new(),
            line_comment: None,
            block_comment: None,
            strings: Vec::new(),
            fold_pairs: Vec::new(),
        }
    }
    fn imported_id(name: &str) -> String {
        import_notepad_xml(&format!(r#"<UserLang name="{name}"/>"#))
            .unwrap()
            .0
            .id
    }
    #[test]
    fn udl_import_maps_comments_delimiters_folders_and_numbers() {
        let (d, report) = import_notepad_xml(FIXTURE).unwrap();
        assert_eq!((d.id.as_str(), d.name.as_str()), ("fixture-lang", "Fixture Lang"));
        assert_eq!(d.extensions, ["fxl", "fx2"]);
        assert_eq!(d.line_comment.as_deref(), Some("//"));
        assert_eq!(d.block_comment, Some(("/*".to_owned(), "*/".to_owned())));
        assert_eq!(d.strings, ['"', '\'', '`']);
        assert_eq!(d.fold_pairs, [('{', '}'), ('(', ')')]);
        assert_eq!(d.keywords, ["if", "else", "#include", "print"]);
        assert_eq!(d.operators, "+-:=");
        // What was imported, and every rule the model cannot express, is reported.
        for (kind, needle) in [
            (MappingKind::Imported, "Line comment //"),
            (MappingKind::Imported, "Block comment /* … */"),
            (MappingKind::Approximated, "line comment #"),
            (MappingKind::Imported, "Strings quoted by \" with \\ escapes"),
            (MappingKind::Approximated, "Strings quoted by '; \\ escapes are always"),
            (MappingKind::Approximated, "escape ^ is read as \\"),
            (MappingKind::Unsupported, "[ … ]"),
            (MappingKind::Imported, "Fold pairs { … }"),
            (MappingKind::Imported, "Fold pairs ( … )"),
            (MappingKind::Unsupported, "begin … end"),
            (MappingKind::Unsupported, "Middle fold markers are not supported: else"),
            (MappingKind::Unsupported, "region, endregion"),
            (MappingKind::Imported, "0x"),
            (MappingKind::Approximated, "not recognised: $"),
            (MappingKind::Unsupported, "end if"),
            (MappingKind::Approximated, "#include"),
            (MappingKind::Unsupported, "Keywords2"),
            (MappingKind::Approximated, "case-sensitively"),
            (MappingKind::Approximated, "2 styles"),
        ] {
            assert!(noted(&report, kind.clone(), needle), "{kind:?} {needle}: {report:#?}");
        }
        let text = "if 'a' # x // note\n`y^` 0x1F";
        let spans = lex(text, &d, &Cancellation::default()).unwrap();
        let kinds: Vec<_> = spans
            .iter()
            .map(|s| (&text[s.range.start.0..s.range.end.0], s.kind))
            .collect();
        assert!(kinds.contains(&("if", StyleKind::Keyword)));
        assert!(kinds.contains(&("'a'", StyleKind::String)));
        assert!(kinds.contains(&("// note", StyleKind::Comment)));
        assert!(kinds.contains(&("0x1F", StyleKind::Number)));
    }
    #[test]
    fn xml_without_lists_reports_defaults_and_rejects_unsafe_input() {
        let xml = r#"<?xml version="1.0"?><NotepadPlus><UserLang name="Demo" ext="demo"><KeywordLists><Keywords name="Keywords1">hello world</Keywords><Keywords name="Comments">00//</Keywords></KeywordLists></UserLang></NotepadPlus>"#;
        let (d, report) = import_notepad_xml(xml).unwrap();
        assert_eq!(d.keywords, vec!["hello", "world"]);
        assert_eq!(d.line_comment.as_deref(), Some("//"));
        assert_eq!(
            (d.strings.as_slice(), d.fold_pairs.as_slice()),
            (&['"', '\''][..], &[('{', '}')][..])
        );
        assert!(noted(&report, MappingKind::Approximated, "No delimiter list"));
        assert!(noted(&report, MappingKind::Approximated, "No folding markers"));
        assert!(import_notepad_xml("<!DOCTYPE x><UserLang/>").is_err());
        assert!(import_notepad_xml("<UserLang><Keywords></UserLang>").is_err());
    }
    #[test]
    fn non_ascii_names_get_distinct_stable_ids() {
        // ASCII IDs are unchanged, so installed languages keep matching.
        assert_eq!(imported_id("QA Test"), "qa-test");
        let (japanese, chinese) = (imported_id("日本語"), imported_id("中文"));
        assert_ne!(japanese, chinese);
        assert!(japanese.starts_with("udl-"), "{japanese}");
        assert_eq!(japanese, imported_id("日本語"));
        // Case folding decides identity: these are one language.
        assert_eq!(imported_id("Ärger"), imported_id("ärger"));
        assert_eq!(name_key("Straße  Deluxe"), name_key("STRASSE deluxe"));
        let long = imported_id(&format!("{}{}", "a".repeat(60), "语".repeat(20)));
        assert!(long.len() <= 64 && long.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    }
    #[test]
    fn collisions_are_renamed_with_a_suffix_and_same_name_replaces() {
        let demo = definition("demo", "Demo");
        // The same language imported again replaces the installed copy.
        let mut again = definition("demo", "DEMO");
        let note = resolve_collisions(&mut again, &[&demo]).unwrap().unwrap();
        assert_eq!((again.id.as_str(), note.kind), ("demo", MappingKind::Imported));
        assert!(note.reason.contains("Replaces"), "{}", note.reason);
        // Another language with the same ID never replaces Demo.
        let mut other = definition("demo", "Other");
        let note = resolve_collisions(&mut other, &[&demo]).unwrap().unwrap();
        assert_eq!((other.id.as_str(), other.name.as_str()), ("demo-2", "Other"));
        assert_eq!(note.kind, MappingKind::Approximated);
        assert!(
            note.reason.contains("Demo") && note.reason.contains("demo-2"),
            "{}",
            note.reason
        );
        // IDs name files, which Windows compares case-insensitively.
        let mut upper = definition("DEMO", "Upper");
        resolve_collisions(&mut upper, &[&demo]).unwrap().unwrap();
        assert_eq!(upper.id, "DEMO-2");
        // A built-in name is suffixed; importing it again replaces the renamed copy.
        let mut rust = definition("rust", "Rust");
        resolve_collisions(&mut rust, &[&demo]).unwrap().unwrap();
        assert_eq!((rust.id.as_str(), rust.name.as_str()), ("rust-2", "Rust (2)"));
        let mut rust_again = definition("rust", "Rust");
        let note = resolve_collisions(&mut rust_again, &[&demo, &rust]).unwrap().unwrap();
        assert_eq!((rust_again.id.as_str(), note.kind), ("rust-2", MappingKind::Imported));
        // Distinct non-ASCII names no longer meet at all.
        let (japanese, _) = import_notepad_xml(r#"<UserLang name="日本語"/>"#).unwrap();
        let (mut chinese, _) = import_notepad_xml(r#"<UserLang name="中文"/>"#).unwrap();
        assert!(resolve_collisions(&mut chinese, &[&japanese]).unwrap().is_none());
        assert!(chinese.validate().is_ok() && other.validate().is_ok() && rust.validate().is_ok());
    }
}
