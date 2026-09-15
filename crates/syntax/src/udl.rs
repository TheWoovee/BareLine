// SPDX-License-Identifier: MPL-2.0
//! Data-only versioned UDL. Replacement validates completely before publishing.
use crate::{Cancellation, Error, MAX_REQUEST_BYTES, MAX_SPANS, StyleKind, StyleSpan};
use bareline_document::TextOffset;
use serde::{Deserialize, Serialize};
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
/// Bounded XML subset importer. DTDs/entities/includes are forbidden; no filesystem/network access.
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
        strings: vec!['"', '\''],
        fold_pairs: vec![('{', '}')],
    };
    let mut report = Vec::new();
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
                let value = xml_unescape(&xml[begin..start])?;
                if name.starts_with("Keywords") {
                    definition.keywords.extend(value.split_whitespace().map(str::to_owned));
                    report.push(Mapping {
                        field: name,
                        kind: MappingKind::Imported,
                        reason: "Keyword set imported".into(),
                    });
                } else if name == "Operators1" {
                    definition.operators = value.split_whitespace().collect();
                    report.push(Mapping {
                        field: name,
                        kind: MappingKind::Approximated,
                        reason: "Single-character operator styling".into(),
                    });
                } else {
                    report.push(Mapping {
                        field: name,
                        kind: MappingKind::Unsupported,
                        reason: "Encoded delimiter/comment/folding rules require manual mapping".into(),
                    });
                }
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
            if name == "UserLang" {
                if found {
                    return Err(Error::InvalidRange);
                }
                found = true;
                if let Some(name) = attrs.get("name") {
                    definition.name = name.clone();
                    definition.id = name
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
                }
                if let Some(ext) = attrs.get("ext") {
                    definition.extensions = ext.split_whitespace().map(str::to_owned).collect();
                }
            }
            if name == "Keywords" {
                if keyword.is_some() {
                    return Err(Error::InvalidRange);
                }
                keyword = Some((attrs.get("name").cloned().unwrap_or_default(), end + 1));
            }
            if matches!(name, "Global" | "Prefix" | "WordsStyle") {
                report.push(Mapping {
                    field: name.into(),
                    kind: MappingKind::Unsupported,
                    reason: "Theme/keyword-prefix options require manual mapping".into(),
                });
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
    definition.validate()?;
    report.push(Mapping {
        field: "strings/folding".into(),
        kind: MappingKind::Approximated,
        reason: "Default quote pairs and brace folds; inspect imported sample".into(),
    });
    Ok((definition, report))
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
    #[test]
    fn xml_reports_loss_and_rejects_unsafe_input() {
        let xml = r#"<?xml version="1.0"?><NotepadPlus><UserLang name="Demo" ext="demo"><KeywordLists><Keywords name="Keywords1">hello world</Keywords><Keywords name="Comments">00//</Keywords></KeywordLists></UserLang></NotepadPlus>"#;
        let (d, report) = import_notepad_xml(xml).unwrap();
        assert_eq!(d.keywords, vec!["hello", "world"]);
        assert!(report.iter().any(|r| r.kind == MappingKind::Unsupported));
        assert!(import_notepad_xml("<!DOCTYPE x><UserLang/>").is_err());
        assert!(import_notepad_xml("<UserLang><Keywords></UserLang>").is_err());
    }
}
