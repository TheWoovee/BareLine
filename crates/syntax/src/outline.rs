// SPDX-License-Identifier: MPL-2.0
//! Bounded data-only function-list definitions. Regex execution shares the search
//! engine's PCRE2 limits; definitions never execute code or resolve XML entities.
use bareline_document::{Budget, Document, DocumentSnapshot, TextOffset};
use bareline_search::{Completeness, SearchJob, SearchMode, SearchQuery, scan};
pub use bareline_search::SearchJob as OutlineJob;
use std::{collections::BTreeMap, ops::Range};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub kind: String,
    pub pattern: String,
    pub names: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub version: u32,
    pub id: String,
    pub rules: Vec<Rule>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MappingKind { Imported, Approximated, Unsupported }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping { pub field: String, pub kind: MappingKind, pub reason: String }
#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub range: Range<TextOffset>,
    pub name_range: Range<TextOffset>,
    pub depth: usize,
}
pub struct Projection {
    pub source: DocumentSnapshot,
    pub symbols: Vec<Symbol>,
    pub partial: bool,
}
fn regex_ranges(source: &DocumentSnapshot, pattern: &str, bounds: Option<Range<TextOffset>>, job: &SearchJob) -> Result<Vec<Range<TextOffset>>, String> {
    if let Some(bounds) = bounds {
        let text = source.read(bounds.clone(), crate::MAX_REQUEST_BYTES).map_err(|e| format!("{e:?}"))?;
        let document = Document::from_utf8(&text, Budget::new(crate::MAX_REQUEST_BYTES * 4), Budget::new(4096)).map_err(|e| format!("{e:?}"))?;
        return regex_ranges(&document.snapshot(), pattern, None, job).map(|ranges| ranges.into_iter().map(|range| TextOffset(bounds.start.0 + range.start.0)..TextOffset(bounds.start.0 + range.end.0)).collect());
    }
    let mut query = SearchQuery::literal(pattern);
    query.mode = SearchMode::Regex;
    query.results_ram_bytes = 1024 * 1024;
    let result = scan(source, &query, job, |_| {});
    if result.completeness() != Completeness::Complete {
        return Err(format!("Regex unavailable: {:?}", result.completeness()));
    }
    Ok(result.matches().iter().map(|m| m.range.clone()).collect())
}
impl Definition {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with_job(&SearchJob::default())
    }
    fn validate_with_job(&self, job: &SearchJob) -> Result<(), String> {
        if self.version != 1 || self.id.is_empty() || self.id.len() > 128 || self.rules.len() > 128 {
            return Err("Invalid outline definition/version or budget exceeded".into());
        }
        if self.rules.iter().flat_map(|r| std::iter::once(&r.pattern).chain(&r.names)).try_fold(0usize, |sum, pattern| sum.checked_add(pattern.len())).is_none_or(|bytes| bytes > 256 * 1024) {
            return Err("Outline definition aggregate budget exceeded".into());
        }
        let empty = Document::from_utf8("", Budget::new(4096), Budget::new(4096)).map_err(|e| format!("{e:?}"))?.snapshot();
        for rule in &self.rules {
            if !matches!(rule.kind.as_str(), "function" | "class") || rule.names.len() > 16 {
                return Err("Invalid outline rule".into());
            }
            for pattern in std::iter::once(&rule.pattern).chain(&rule.names) {
                if pattern.is_empty() || pattern.len() > 16 * 1024 { return Err("Pattern budget exceeded".into()); }
                if job.is_cancelled() { return Err("Outline import cancelled".into()); }
                regex_ranges(&empty, pattern, None, job)?;
            }
        }
        Ok(())
    }
    pub fn to_toml(&self) -> Result<String, String> {
        self.validate()?;
        let mut doc = toml_edit::DocumentMut::new();
        doc["version"] = toml_edit::value(1);
        doc["id"] = toml_edit::value(self.id.clone());
        let mut rules = toml_edit::ArrayOfTables::new();
        for rule in &self.rules {
            let mut table = toml_edit::Table::new();
            table["kind"] = toml_edit::value(rule.kind.clone());
            table["pattern"] = toml_edit::value(rule.pattern.clone());
            let mut names = toml_edit::Array::new();
            for name in &rule.names { names.push(name.as_str()); }
            table["names"] = toml_edit::value(names);
            rules.push(table);
        }
        doc["rules"] = toml_edit::Item::ArrayOfTables(rules);
        Ok(doc.to_string())
    }
    pub fn from_toml(text: &str) -> Result<Self, String> {
        if text.len() > 256 * 1024 { return Err("Definition budget exceeded".into()); }
        let doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| e.to_string())?;
        if doc.iter().any(|(k, _)| !matches!(k, "version" | "id" | "rules")) { return Err("Unknown outline key".into()); }
        let version = doc.get("version").and_then(|v| v.as_integer()).ok_or("Missing version")?;
        let id = doc.get("id").and_then(|v| v.as_str()).ok_or("Missing id")?.to_owned();
        let mut rules = Vec::new();
        if doc.get("rules").is_some_and(|v| v.as_array_of_tables().is_none()) { return Err("Rules must be an array of tables".into()); }
        if let Some(tables) = doc.get("rules").and_then(|v| v.as_array_of_tables()) {
            for table in tables {
                if table.iter().any(|(k, _)| !matches!(k, "kind" | "pattern" | "names")) { return Err("Unknown rule key".into()); }
                let kind = table.get("kind").and_then(|v| v.as_str()).ok_or("Missing kind")?.into();
                let pattern = table.get("pattern").and_then(|v| v.as_str()).ok_or("Missing pattern")?.into();
                let names = table.get("names").and_then(|v| v.as_array()).ok_or("Missing names")?.iter().map(|v| v.as_str().map(str::to_owned).ok_or("Invalid name rule")).collect::<Result<Vec<_>, _>>()?;
                rules.push(Rule { kind, pattern, names });
            }
        }
        let result = Self { version: u32::try_from(version).map_err(|_| "Invalid version")?, id, rules };
        result.validate()?;
        Ok(result)
    }
    /// Caller supplies a bounded, sealed chunk. Ranges remain in that chunk's
    /// text domain; the panel rebases them to its revisioned source snapshot.
    pub fn extract(&self, source: &DocumentSnapshot, job: &SearchJob) -> Result<Projection, String> {
        if source.len() > crate::MAX_REQUEST_BYTES { return Err("Outline chunk budget exceeded".into()); }
        let mut symbols = Vec::new();
        let mut partial = false;
        for rule in &self.rules {
            if job.is_cancelled() { return Err("Outline cancelled".into()); }
            let ranges = regex_ranges(source, &rule.pattern, None, job)?;
            for range in ranges {
                if symbols.len() == 8192 { partial = true; break; }
                let mut name_range = range.clone();
                let mut valid = true;
                for pattern in &rule.names {
                    if let Some(found) = regex_ranges(source, pattern, Some(name_range.clone()), job)?.first() {
                        name_range = found.clone();
                    } else { valid = false; break; }
                }
                if !valid || name_range.is_empty() || name_range.end.0 - name_range.start.0 > 4096 { continue; }
                let name = source.read(name_range.clone(), 4096).map_err(|e| format!("{e:?}"))?;
                symbols.push(Symbol { name, kind: rule.kind.clone(), range, name_range, depth: 0 });
            }
        }
        symbols.sort_by_key(|s| (s.range.start, std::cmp::Reverse(s.range.end)));
        for i in 0..symbols.len() {
            symbols[i].depth = symbols[..i].iter().filter(|parent| parent.kind == "class" && parent.range.start <= symbols[i].range.start && parent.range.end >= symbols[i].range.end && parent.range != symbols[i].range).count().min(64);
        }
        Ok(Projection { source: source.clone(), symbols, partial })
    }
}

pub fn import_function_list(xml: &str) -> Result<(Definition, Vec<Mapping>), String> {
    import_function_list_with_job(xml, &SearchJob::default())
}
pub fn import_function_list_with_job(xml: &str, job: &SearchJob) -> Result<(Definition, Vec<Mapping>), String> {
    if xml.len() > 256 * 1024 { return Err("XML budget exceeded".into()); }
    let mut definition = Definition { version: 1, id: String::new(), rules: Vec::new() };
    let mut report = Vec::new();
    let mut stack: Vec<(String, Option<usize>)> = Vec::new();
    let mut cursor = 0;
    let mut parser_seen = false;
    while cursor < xml.len() {
        if job.is_cancelled() { return Err("Outline import cancelled".into()); }
        let remaining = &xml[cursor..];
        if remaining.trim().is_empty() { break; }
        let start = cursor + remaining.find('<').ok_or("Malformed XML")?;
        if !xml[cursor..start].trim().is_empty() { return Err("Unexpected XML text".into()); }
        if xml[start..].starts_with("<!--") {
            cursor = start + 4 + xml[start + 4..].find("-->").ok_or("Unclosed comment")? + 3; continue;
        }
        if xml[start..].starts_with("<?xml ") {
            cursor = start + 2 + xml[start + 2..].find("?>").ok_or("Unclosed declaration")? + 2; continue;
        }
        let mut quote = None;
        let end = xml[start+1..].char_indices().find_map(|(i,c)| {
            if let Some(q) = quote { if c == q { quote = None; } }
            else if c == '\'' || c == '"' { quote = Some(c); }
            else if c == '>' { return Some(start + 1 + i); }
            None
        }).ok_or("Unclosed XML tag")?;
        let tag = xml[start+1..end].trim();
        if let Some(name) = tag.strip_prefix('/') {
            if stack.pop().map(|s| s.0).as_deref() != Some(name.trim()) { return Err("Mismatched XML tag".into()); }
        } else {
            let closed = tag.ends_with('/');
            let tag = tag.strip_suffix('/').unwrap_or(tag).trim();
            let split = tag.find(char::is_whitespace).unwrap_or(tag.len());
            let name = &tag[..split];
            if name.is_empty() || name.starts_with(['!', '?']) { return Err("XML declarations/entities are forbidden".into()); }
            let attrs = attributes(&tag[split..])?;
            let mut current = stack.last().and_then(|s| s.1);
            match name {
                "parser" => {
                    if parser_seen { return Err("Only one parser per definition".into()); }
                    parser_seen = true;
                    definition.id = attrs.get("id").or_else(|| attrs.get("displayName")).cloned().ok_or("Missing parser id")?;
                }
                "function" | "classRange" => {
                    if !parser_seen || definition.rules.len() >= 128 { return Err("Invalid parser/rule budget".into()); }
                    let pattern = attrs.get("mainExpr").cloned().ok_or("Missing mainExpr")?;
                    current = Some(definition.rules.len());
                    definition.rules.push(Rule { kind: if name == "function" { "function" } else { "class" }.into(), pattern, names: Vec::new() });
                }
                "nameExpr" => {
                    let index = current.ok_or("Name expression outside rule")?;
                    definition.rules[index].names.push(attrs.get("expr").cloned().ok_or("Missing name expression")?);
                }
                "NotepadPlus" | "functionList" | "functionName" | "className" => {}
                _ => report.push(Mapping { field: name.into(), kind: MappingKind::Unsupported, reason: "Element is not part of the outline schema".into() }),
            }
            for key in attrs.keys() {
                if !matches!((name,key.as_str()), ("parser","id" | "displayName") | ("function" | "classRange","mainExpr") | ("nameExpr","expr")) {
                    report.push(Mapping { field: format!("{name}.{key}"), kind: MappingKind::Unsupported, reason: "Attribute requires manual mapping".into() });
                }
            }
            if !closed {
                if stack.len() >= 64 { return Err("XML nesting budget exceeded".into()); }
                stack.push((name.into(), current));
            }
        }
        cursor = end + 1;
    }
    if !parser_seen || !stack.is_empty() { return Err("Incomplete parser XML".into()); }
    let mut accepted = Vec::new();
    for (index, rule) in definition.rules.drain(..).enumerate() {
        let candidate = Definition { version: 1, id: definition.id.clone(), rules: vec![rule.clone()] };
        if job.is_cancelled() { return Err("Outline import cancelled".into()); }
        match candidate.validate_with_job(job) {
            Ok(()) => {
                report.push(Mapping { field: format!("rule.{index}"), kind: if rule.kind == "class" { MappingKind::Approximated } else { MappingKind::Imported }, reason: if rule.kind == "class" { "Class extent is mainExpr; delimiter nesting requires manual mapping" } else { "PCRE2 function and chained name expressions" }.into() });
                accepted.push(rule);
            }
            Err(error) => report.push(Mapping { field: format!("rule.{index}"), kind: MappingKind::Unsupported, reason: error }),
        }
    }
    definition.rules = accepted;
    definition.validate_with_job(job)?;
    report.push(Mapping { field: "chunk-boundaries".into(), kind: MappingKind::Approximated, reason: "Progressive extraction uses bounded chunks; cross-chunk expressions may be omitted".into() });
    Ok((definition, report))
}
fn attributes(mut text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut attrs = BTreeMap::new();
    while !text.trim().is_empty() {
        text = text.trim_start();
        let split = text.find('=').ok_or("Missing attribute value")?;
        let name = text[..split].trim();
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_:-".contains(&b)) { return Err("Invalid attribute".into()); }
        text = text[split+1..].trim_start();
        let quote = text.chars().next().filter(|c| *c == '\'' || *c == '"').ok_or("Unquoted attribute")?;
        text = &text[1..];
        let end = text.find(quote).ok_or("Unclosed attribute")?;
        let mut decoded = String::new();
        let mut value = &text[..end];
        if value.contains('<') { return Err("Unescaped XML attribute delimiter".into()); }
        while let Some(at) = value.find('&') {
            decoded.push_str(&value[..at]);
            let finish = at + value[at..].find(';').ok_or("Unclosed entity")?;
            let entity = &value[at+1..finish];
            let c = match entity {
                "amp" => '&', "lt" => '<', "gt" => '>', "quot" => '"', "apos" => '\'',
                _ => { let code = if let Some(hex) = entity.strip_prefix("#x") { u32::from_str_radix(hex,16).ok() } else { entity.strip_prefix('#').and_then(|n| n.parse().ok()) }; code.and_then(char::from_u32).ok_or("Unknown entity")? }
            };
            decoded.push(c); value = &value[finish+1..];
        }
        decoded.push_str(value);
        if attrs.insert(name.into(), decoded).is_some() { return Err("Duplicate attribute".into()); }
        text = &text[end+1..];
    }
    Ok(attrs)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_roundtrip_and_extracts_name_ranges() {
        let xml = r#"<NotepadPlus><functionList><parser id="rust"><function mainExpr="fn\s+\w+"><functionName><nameExpr expr="\w+$"/></functionName></function></parser></functionList></NotepadPlus>"#;
        let (definition, report) = import_function_list(xml).unwrap();
        assert_eq!(report, import_function_list(xml).unwrap().1);
        assert_eq!(definition, Definition::from_toml(&definition.to_toml().unwrap()).unwrap());
        let doc = Document::from_utf8("fn main() {}", Budget::new(4096), Budget::new(4096)).unwrap();
        let projection = definition.extract(&doc.snapshot(), &SearchJob::default()).unwrap();
        assert_eq!(projection.symbols[0].name, "main");
        assert_eq!(projection.symbols[0].name_range, TextOffset(3)..TextOffset(7));
    }
    #[test]
    fn rejects_malformed_entities_and_reports_invalid_regex() {
        for xml in ["<!DOCTYPE x><parser id='x'/>", "<parser id='x'></function>", "<parser id='&external;'/>"] { assert!(import_function_list(xml).is_err()); }
        let (definition, report) = import_function_list("<parser id='x'><function mainExpr='('/></parser>").unwrap();
        assert!(definition.rules.is_empty());
        assert!(report.iter().any(|m| m.kind == MappingKind::Unsupported));
    }
    #[test]
    fn cancellation_and_three_language_mapping_shapes_are_deterministic() {
        let cancelled = SearchJob::default();
        cancelled.cancel();
        assert!(import_function_list_with_job("<parser id='x'/>", &cancelled).is_err());
        for (id, pattern, sample, expected) in [
            ("python", r"def\s+\w+", "def hello():", "hello"),
            ("javascript", r"function\s+\w+", "function world() {}", "world"),
            ("rust", r"fn\s+\w+", "fn main() {}", "main"),
        ] {
            let xml = format!(r#"<parser id="{id}"><function mainExpr="{pattern}"><functionName><nameExpr expr="\w+$"/></functionName></function></parser>"#);
            let (definition, report) = import_function_list(&xml).unwrap();
            assert_eq!(report, import_function_list(&xml).unwrap().1);
            let doc = Document::from_utf8(sample, Budget::new(4096), Budget::new(4096)).unwrap();
            assert_eq!(definition.extract(&doc.snapshot(), &SearchJob::default()).unwrap().symbols[0].name, expected);
            assert!(definition.extract(&doc.snapshot(), &cancelled).is_err());
        }
        assert!(Definition::from_toml("version=1\nid='x'\nrules=5").is_err());
    }
}

use crate::StyleKind;
#[derive(Clone, Debug)]
pub struct LexicalSymbol {
    pub name: String,
    pub kind: &'static str,
    pub offset: TextOffset,
    pub end: TextOffset,
    pub depth: usize,
}
pub fn rust_symbols(text: &str, base: usize, spans: &[crate::StyleSpan]) -> Vec<LexicalSymbol> {
    let mut result = Vec::new();
    for span in spans.iter().filter(|s| s.kind == StyleKind::Keyword) {
        let start = span.range.start.0 - base;
        let end = span.range.end.0 - base;
        let Some(kind) = text.get(start..end) else {
            continue;
        };
        let kind = match kind {
            "fn" => "fn",
            "struct" => "struct",
            "enum" => "enum",
            "trait" => "trait",
            "mod" => "mod",
            "const" => "const",
            "type" => "type",
            _ => continue,
        };
        let rest = &text[end..];
        let skip = rest.len() - rest.trim_start().len();
        let name_start = end + skip;
        let len = text[name_start..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum::<usize>();
        if len == 0 || len > 4096 {
            continue;
        }
        if spans[spans.partition_point(|s| s.range.end.0 <= base + name_start)..]
            .iter()
            .take_while(|s| s.range.start.0 < base + name_start + len)
            .any(|s| {
                s.range.start.0 < base + name_start + len
                    && s.range.end.0 > base + name_start
                    && matches!(s.kind, StyleKind::Comment | StyleKind::String)
            })
        {
            continue;
        }
        result.push(LexicalSymbol {
            name: text[name_start..name_start + len].into(),
            kind,
            offset: TextOffset(base + name_start),
            end: TextOffset(base + name_start + len),
            depth: 0,
        });
    }
    // Brace scopes use only lexically valid bytes. Comments and strings cannot
    // create nesting. An unfinished scope ends at this bounded projection edge.
    let mut scopes: Vec<Option<usize>> = Vec::new();
    let mut next = 0;
    let mut pending = None;
    let mut span_index = 0;
    for (at, byte) in text.bytes().enumerate() {
        while next < result.len() && result[next].offset.0 <= base + at {
            result[next].depth = scopes.iter().filter(|s| s.is_some()).count();
            pending = Some(next);
            next += 1;
        }
        while span_index < spans.len() && spans[span_index].range.end.0 <= base + at { span_index += 1; }
        if spans.get(span_index).is_some_and(|s| s.range.start.0 <= base + at && matches!(s.kind, StyleKind::Comment | StyleKind::String)) { continue; }
        match byte {
            b'{' => scopes.push(pending.take()),
            b'}' => {
                if let Some(Some(index)) = scopes.pop() { result[index].end = TextOffset(base + at + 1); }
                pending = None;
            }
            b';' => { if let Some(index) = pending.take() { result[index].end = TextOffset(base + at + 1); } }
            _ => {}
        }
    }
    for index in scopes.into_iter().flatten() { result[index].end = TextOffset(base + text.len()); }
    result
}
pub fn toml_symbols(text: &str, base: usize, multiline: &mut Option<u8>) -> Vec<LexicalSymbol> {
    let mut offset = base;
    let mut out = Vec::new();
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if multiline.is_none()
            && trimmed.starts_with('[')
            && let Some(end) = trimmed.find(']')
        {
            let skip = if trimmed.starts_with("[[") { 2 } else { 1 };
            if end > skip {
                let name = &trimmed[skip..end];
                if name.len() <= 4096 {
                    out.push(LexicalSymbol {
                        name: name.into(),
                        kind: "table",
                        offset: TextOffset(offset + line.len() - trimmed.len() + skip),
                        end: TextOffset(offset + line.len() - trimmed.len() + end),
                        depth: name.bytes().filter(|b| *b == b'.').count().min(64),
                    });
                }
            }
        }
        // Track multiline strings across worker chunks. Quotes/comments in ordinary
        // strings do not start or finish a multiline literal.
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if let Some(quote) = *multiline {
                if bytes
                    .get(i..i + 3)
                    .is_some_and(|s| s.iter().all(|b| *b == quote))
                {
                    *multiline = None;
                    i += 3;
                } else if quote == b'"' && bytes[i] == b'\\' {
                    i += 2;
                } else {
                    i += 1;
                }
            } else if bytes[i] == b'#' {
                break;
            } else if bytes[i] == b'"' || bytes[i] == b'\'' {
                let quote = bytes[i];
                if bytes
                    .get(i..i + 3)
                    .is_some_and(|s| s.iter().all(|b| *b == quote))
                {
                    *multiline = Some(quote);
                    i += 3;
                } else {
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == quote {
                            i += 1;
                            break;
                        }
                        if quote == b'"' && bytes[i] == b'\\' {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                }
            } else {
                i += 1;
            }
        }
        offset += line.len();
    }
    out
}
