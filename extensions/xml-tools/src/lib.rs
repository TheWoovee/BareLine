// SPDX-License-Identifier: MIT OR Apache-2.0
//! XML tools use a patched streaming parser with explicit resource limits.
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
use std::{
    collections::BTreeMap,
    io::{BufReader, Read, Write},
};
pub const MAX_INPUT: u64 = 16 * 1024 * 1024;
pub const MAX_NODES: usize = 10000;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub offset: u64,
    pub message: String,
}
#[derive(Debug, Clone)]
pub struct Node {
    pub namespace: String,
    pub name: String,
    pub attributes: BTreeMap<(String, String), String>,
    pub text: String,
    pub text_nodes: Vec<String>,
    text_open: bool,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}
pub struct Document {
    pub nodes: Vec<Node>,
}
fn err(offset: u64, message: impl ToString) -> Error {
    Error {
        offset,
        message: message.to_string(),
    }
}
fn ns(result: ResolveResult<'_>) -> Result<String, String> {
    match result {
        ResolveResult::Bound(n) => std::str::from_utf8(n.as_ref())
            .map(str::to_owned)
            .map_err(|_| "invalid namespace UTF-8".into()),
        ResolveResult::Unbound => Ok(String::new()),
        ResolveResult::Unknown(_) => Err("unbound namespace prefix".into()),
    }
}
fn xml_chars(text: &str) -> bool {
    text.chars().all(|c|matches!(c,'\u{9}'|'\u{a}'|'\u{d}'|'\u{20}'..='\u{d7ff}'|'\u{e000}'..='\u{fffd}'|'\u{10000}'..='\u{10ffff}'))
}
/// Limits are checked before tree insertion. Reader has a hard input ceiling,
/// bounding even a single malicious unterminated token before parser allocation.
pub fn parse(input: impl Read) -> Result<Document, Error> {
    let mut reader = NsReader::from_reader(BufReader::new(input.take(MAX_INPUT + 1)));
    reader.config_mut().check_end_names = true;
    reader.config_mut().check_comments = true;
    let mut buf = Vec::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut roots = 0;
    let mut text_bytes = 0usize;
    let mut first_event = true;
    loop {
        let was_first = first_event;
        first_event = false;
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| err(reader.error_position(), e))?;
        let offset = reader.buffer_position();
        if offset > MAX_INPUT {
            return Err(err(offset, "XML input limit (16 MiB)"));
        }
        if !matches!(
            event,
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_)
        ) {
            if let Some(&index) = stack.last() {
                nodes[index].text_open = false;
            }
        }
        match event {
            Event::Decl(ref declaration) => {
                if !was_first
                    || declaration.version().map_err(|e| err(offset, e))?.as_ref() != b"1.0"
                {
                    return Err(err(offset, "XML declaration must be first and version 1.0"));
                }
                if let Some(encoding) = declaration.encoding() {
                    let encoding = encoding.map_err(|e| err(offset, e))?;
                    // Snapshot bytes are the editor's already-decoded UTF-8 text
                    // view. The declaration describes its original encoding and
                    // must be preserved, not used to transcode the snapshot again.
                    if !encoding.first().is_some_and(u8::is_ascii_alphabetic)
                        || !encoding.iter().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-')
                        })
                    {
                        return Err(err(offset, "invalid XML encoding declaration"));
                    }
                }
            }
            Event::PI(ref pi) => {
                if !qname(pi.target()) || pi.target().eq_ignore_ascii_case(b"xml") {
                    return Err(err(offset, "invalid processing instruction"));
                }
            }
            Event::DocType(_) => return Err(err(offset, "DTD and external entities are disabled")),
            Event::Start(ref e) | Event::Empty(ref e) => {
                if stack.len() >= 128 || nodes.len() >= MAX_NODES {
                    return Err(err(offset, "XML depth/node limit"));
                }
                if !qname(e.name().as_ref()) {
                    return Err(err(offset, "invalid XML qualified name"));
                }
                let (namespace, local) = reader.resolver().resolve_element(e.name());
                let namespace = ns(namespace).map_err(|e| err(offset, e))?;
                let name = std::str::from_utf8(local.as_ref())
                    .map_err(|e| err(offset, e))?
                    .to_owned();
                if namespace == "http://www.w3.org/2001/XInclude" {
                    return Err(err(offset, "XInclude is disabled"));
                }
                let mut attributes = BTreeMap::new();
                for (index, attribute) in e.attributes().enumerate() {
                    if index >= 256 {
                        return Err(err(offset, "XML attribute limit"));
                    }
                    let attribute = attribute.map_err(|e| err(offset, e))?;
                    let raw = attribute.key.as_ref();
                    if !qname(raw) {
                        return Err(err(offset, "invalid XML attribute name"));
                    }
                    if raw == b"xmlns" || raw.starts_with(b"xmlns:") {
                        continue;
                    }
                    let (ans, aname) = reader.resolver().resolve_attribute(attribute.key);
                    let key = (
                        ns(ans).map_err(|e| err(offset, e))?,
                        std::str::from_utf8(aname.as_ref())
                            .map_err(|e| err(offset, e))?
                            .to_owned(),
                    );
                    let value = attribute
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(|e| err(offset, e))?
                        .into_owned();
                    if !xml_chars(&value) || attributes.insert(key, value).is_some() {
                        return Err(err(offset, "invalid or duplicate expanded attribute"));
                    }
                }
                let parent = stack.last().copied();
                let index = nodes.len();
                if let Some(parent) = parent {
                    nodes[parent].children.push(index);
                } else {
                    roots += 1;
                    if roots > 1 {
                        return Err(err(offset, "multiple XML roots"));
                    }
                }
                nodes.push(Node {
                    namespace,
                    name,
                    attributes,
                    text: String::new(),
                    text_nodes: Vec::new(),
                    text_open: false,
                    parent,
                    children: Vec::new(),
                });
                if matches!(event, Event::Start(_)) {
                    stack.push(index);
                }
            }
            Event::End(_) => {
                stack
                    .pop()
                    .ok_or_else(|| err(offset, "unexpected closing element"))?;
            }
            Event::Text(ref text) => {
                let decoded = text
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|e| err(offset, e))?;
                let value = quick_xml::escape::unescape(&decoded).map_err(|e| err(offset, e))?;
                append_text(&mut nodes, &stack, &value, &mut text_bytes, offset)?;
            }
            Event::CData(ref text) => {
                let value = text.decode().map_err(|e| err(offset, e))?;
                if stack.is_empty() {
                    return Err(err(offset, "CDATA outside root"));
                }
                append_text(&mut nodes, &stack, &value, &mut text_bytes, offset)?;
            }
            Event::GeneralRef(ref reference) => {
                let name = reference.decode().map_err(|e| err(offset, e))?;
                let escaped = format!("&{name};");
                let value = quick_xml::escape::unescape(&escaped).map_err(|e| err(offset, e))?;
                append_text(&mut nodes, &stack, &value, &mut text_bytes, offset)?;
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if roots != 1 || !stack.is_empty() {
        return Err(err(
            reader.buffer_position(),
            "missing or unclosed XML root",
        ));
    }
    Ok(Document { nodes })
}
fn append_text(
    nodes: &mut [Node],
    stack: &[usize],
    text: &str,
    total: &mut usize,
    offset: u64,
) -> Result<(), Error> {
    if !xml_chars(text) {
        return Err(err(offset, "invalid XML character"));
    }
    *total = total.saturating_add(text.len());
    if *total > 4 * 1024 * 1024 {
        return Err(err(offset, "XML expanded text limit"));
    }
    if let Some(&index) = stack.last() {
        nodes[index].text.push_str(text);
        if !nodes[index].text_open {
            nodes[index].text_nodes.push(String::new());
            nodes[index].text_open = true;
        }
        nodes[index].text_nodes.last_mut().unwrap().push_str(text);
    } else if !text.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n')) {
        return Err(err(offset, "text outside root"));
    }
    Ok(())
}
/// Whitespace-preserving canonical layout. Mixed content retains its original
/// bytes; element-only documents receive indentation through the vetted writer.
pub fn format(input: &[u8], out: impl Write) -> Result<(), Error> {
    let document = parse(input)?;
    let mixed = document.nodes.iter().any(|n| {
        !n.text.is_empty()
            || n.attributes
                .get(&(
                    "http://www.w3.org/XML/1998/namespace".into(),
                    "space".into(),
                ))
                .is_some_and(|v| v == "preserve")
    });
    let mut reader = quick_xml::Reader::from_reader(input);
    let mut writer = if mixed {
        quick_xml::Writer::new(out)
    } else {
        quick_xml::Writer::new_with_indent(out, b' ', 2)
    };
    if !mixed {
        reader.config_mut().trim_text(true);
    }
    loop {
        let event = reader
            .read_event()
            .map_err(|e| err(reader.error_position(), e))?;
        if matches!(event, Event::Eof) {
            break;
        }
        writer
            .write_event(event)
            .map_err(|e| err(reader.buffer_position(), e))?;
    }
    Ok(())
}
#[derive(Clone)]
enum Predicate {
    Position(usize),
    Attribute(String, String),
    Text(String),
}
struct Step {
    descendant: bool,
    name: String,
    predicate: Option<Predicate>,
}
fn expanded(name: &str, namespaces: &BTreeMap<String, String>) -> Result<(String, String), Error> {
    if !qname(name.as_bytes()) {
        return Err(err(0, "unsupported XPath qualified name"));
    }
    if let Some((prefix, local)) = name.split_once(':') {
        Ok((
            namespaces
                .get(prefix)
                .ok_or_else(|| err(0, "unknown XPath namespace prefix"))?
                .clone(),
            local.into(),
        ))
    } else {
        Ok((String::new(), name.into()))
    }
}
/// Deliberately limited XPath v1: child/descendant paths, namespace bindings,
/// attributes/text(), equality and one-based positional predicates.
pub fn query(
    document: &Document,
    expression: &str,
    namespaces: &BTreeMap<String, String>,
) -> Result<Vec<String>, Error> {
    if expression.len() > 4096
        || !expression.starts_with('/')
        || expression.contains("::")
        || expression.contains('|')
    {
        return Err(err(0, "unsupported XPath syntax"));
    }
    let mut rest = expression;
    let mut steps = Vec::new();
    let mut selection = None;
    let mut selection_descendant = false;
    while !rest.is_empty() {
        let descendant = rest.starts_with("//");
        rest = &rest[if descendant { 2 } else { 1 }..];
        let mut end = rest.len();
        let (mut quote, mut bracket) = (None, 0);
        for (i, c) in rest.char_indices() {
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
                continue;
            }
            match c {
                '\'' | '"' => quote = Some(c),
                '[' => bracket += 1,
                ']' => bracket -= 1,
                '/' if bracket == 0 => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        let token = &rest[..end];
        rest = &rest[end..];
        if token == "text()" || token.starts_with('@') {
            if let Some(attribute) = token.strip_prefix('@') {
                expanded(attribute, namespaces)?;
            }
            if !rest.is_empty() {
                return Err(err(0, "unsupported XPath selection path"));
            }
            selection = Some(token.to_owned());
            selection_descendant = descendant;
            break;
        }
        let (name, predicate) = if let Some((name, pred)) = token.split_once('[') {
            let pred = pred
                .strip_suffix(']')
                .ok_or_else(|| err(0, "unsupported XPath predicate"))?;
            let predicate = if let Ok(n) = pred.parse::<usize>() {
                if n == 0 {
                    return Err(err(0, "XPath positions begin at one"));
                }
                Predicate::Position(n)
            } else if let Some((left, right)) = pred.split_once('=') {
                let right = right.trim();
                let value = right
                    .strip_prefix('\'')
                    .and_then(|v| v.strip_suffix('\''))
                    .or_else(|| right.strip_prefix('"').and_then(|v| v.strip_suffix('"')))
                    .ok_or_else(|| err(0, "unsupported XPath equality"))?
                    .to_owned();
                if let Some(attribute) = left.trim().strip_prefix('@') {
                    Predicate::Attribute(attribute.into(), value)
                } else if left.trim() == "text()" {
                    Predicate::Text(value)
                } else {
                    return Err(err(0, "unsupported XPath predicate"));
                }
            } else {
                return Err(err(0, "unsupported XPath predicate"));
            };
            (name, Some(predicate))
        } else {
            (token, None)
        };
        if name.is_empty()
            || name
                .chars()
                .any(|c| !(c.is_alphanumeric() || matches!(c, ':' | '_' | '-' | '.' | '*')))
        {
            return Err(err(0, "unsupported XPath function or name"));
        }
        steps.push(Step {
            descendant,
            name: name.into(),
            predicate,
        });
        if steps.len() > 128 {
            return Err(err(0, "XPath depth limit"));
        }
    }
    let mut work = 0usize;
    let mut current = vec![None];
    for step in steps {
        let mut next = Vec::new();
        let expected = if step.name == "*" {
            None
        } else {
            Some(expanded(&step.name, namespaces)?)
        };
        for parent in current {
            work = work.saturating_add(document.nodes.len().saturating_mul(128));
            if work > 8_000_000 {
                return Err(err(0, "XPath work limit"));
            }
            let candidates: Vec<usize> = document
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    if step.descendant {
                        let mut ancestor = n.parent;
                        loop {
                            if ancestor == parent {
                                break true;
                            }
                            match ancestor {
                                Some(a) => ancestor = document.nodes[a].parent,
                                None => break false,
                            }
                        }
                    } else {
                        n.parent == parent
                    }
                })
                .map(|(i, _)| i)
                .collect();
            let mut positions = BTreeMap::new();
            for index in candidates {
                let node = &document.nodes[index];
                if expected
                    .as_ref()
                    .is_some_and(|(ns, name)| &node.namespace != ns || &node.name != name)
                {
                    continue;
                }
                let position = positions.entry(node.parent).or_insert(0usize);
                *position += 1;
                let keep = match &step.predicate {
                    None => true,
                    Some(Predicate::Position(p)) => *position == *p,
                    Some(Predicate::Text(value)) => {
                        node.text_nodes.iter().any(|text| text == value)
                    }
                    Some(Predicate::Attribute(name, value)) => {
                        node.attributes.get(&expanded(name, namespaces)?) == Some(value)
                    }
                };
                if keep && !next.contains(&Some(index)) {
                    next.push(Some(index));
                }
                if next.len() > 1000 {
                    return Err(err(0, "XPath result limit"));
                }
            }
        }
        current = next;
    }
    if selection_descendant {
        let roots: std::collections::BTreeSet<_> = current.iter().flatten().copied().collect();
        let mut selected = Vec::new();
        for (index, _) in document.nodes.iter().enumerate() {
            let mut ancestor = Some(index);
            while let Some(node) = ancestor {
                if roots.contains(&node) {
                    selected.push(Some(index));
                    if selected.len() > 1000 {
                        return Err(err(0, "XPath result limit"));
                    }
                    break;
                }
                ancestor = document.nodes[node].parent;
            }
        }
        current = selected;
    }
    let mut output = Vec::new();
    let mut bytes = 0;
    for index in current.into_iter().flatten() {
        let node = &document.nodes[index];
        if selection.as_deref() == Some("text()") {
            for text in &node.text_nodes {
                bytes += text.len() + 1;
                if bytes > 1024 * 1024 || output.len() >= 1000 {
                    return Err(err(0, "XPath output limit"));
                }
                output.push(text.clone());
            }
            continue;
        }
        let value = match selection.as_deref() {
            Some("text()") => unreachable!(),
            Some(attribute) => match node.attributes.get(&expanded(&attribute[1..], namespaces)?) {
                Some(value) => value.clone(),
                None => continue,
            },
            None => format!("{{{}}}{}: {}", node.namespace, node.name, node.text),
        };
        bytes += value.len() + 1;
        if bytes > 1024 * 1024 {
            return Err(err(0, "XPath output limit"));
        }
        output.push(value);
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn security_and_well_formedness() {
        for input in [
            "<!DOCTYPE x SYSTEM 'https://bad'><x/>",
            "<x xmlns:i='http://www.w3.org/2001/XInclude'><i:include/></x>",
            "<x>&unknown;</x>",
            "<x a='1' a='2'/>",
            "<x></y>",
            "<x/><y/>",
        ] {
            assert!(parse(input.as_bytes()).is_err(), "{input}");
        }
        assert!(parse("<x>".repeat(129).as_bytes()).is_err());
    }
    #[test]
    fn xpath_namespace_predicates() {
        let doc = parse(
            b"<r xmlns:a='urn:x'><a:n k='one'>alpha</a:n><a:n k='two'>beta</a:n></r>".as_slice(),
        )
        .unwrap();
        let ns = BTreeMap::from([("a".into(), "urn:x".into())]);
        assert_eq!(query(&doc, "/r/a:n[2]/text()", &ns).unwrap(), ["beta"]);
        assert_eq!(query(&doc, "//a:n[@k='one']/@k", &ns).unwrap(), ["one"]);
        assert!(query(&doc, "//a:n[contains(text(),'x')]", &ns).is_err());
        assert!(query(&doc, "/r/parent::x", &ns).is_err());
    }
    #[test]
    fn inherited_space_and_whitespace_nodes_preserved() {
        for input in [
            "<r xml:space='preserve'><a/><b/></r>",
            "<r xml:space='preserve'><a xml:space='default'><b/></a></r>",
            "<p><b/> <i/></p>",
            "<p><![CDATA[ ]]><b/></p>",
            "<p>&#32;<b/></p>",
            "<r>\n  <a/>\n</r>",
        ] {
            let mut output = Vec::new();
            format(input.as_bytes(), &mut output).unwrap();
            assert_eq!(output, input.as_bytes(), "{input}");
        }
    }
    #[test]
    fn mixed_content_preserved() {
        let input = b"<p>hello <b>world</b>!</p>";
        let mut output = Vec::new();
        format(input, &mut output).unwrap();
        assert_eq!(output, input);
    }
}

pub fn run<T: bareline_first_party_common::Transport>(
    client: std::rc::Rc<std::cell::RefCell<bareline_first_party_common::Client<T>>>,
) -> Result<(), String> {
    use bareline_first_party_common::{Counter, Snapshot, Staged};
    let invocation = client.borrow().invocation.clone();
    if invocation.text_length > MAX_INPUT {
        return Err("XML operation input limit is 16 MiB".into());
    }
    match invocation.command.as_str() {
        "ext.xml.validate" => {
            parse(Snapshot::new(client.clone()))
                .map_err(|e| format!("TextOffset {}: {}", e.offset, e.message))?;
            client.borrow_mut().panel(
                "ext.xml.xpath",
                "Valid XML; external resolution disabled".into(),
            )
        }
        "ext.xml.format" => {
            let mut input = Vec::new();
            Snapshot::new(client.clone())
                .read_to_end(&mut input)
                .map_err(|e| e.to_string())?;
            let mut count = Counter::default();
            format(&input, &mut count).map_err(|e| e.message)?;
            let mut stage = Staged::new(client.clone(), count.0)?;
            format(&input, &mut stage).map_err(|e| e.message)?;
            stage.finish()
        }
        "ext.xml.xpath" => {
            let doc = parse(Snapshot::new(client.clone()))
                .map_err(|e| format!("TextOffset {}: {}", e.offset, e.message))?; // First line is XPath; later lines bind prefix=namespace URI.
            let (expression, namespaces) = parse_arguments(&invocation.arguments)?;
            let values = query(&doc, &expression, &namespaces).map_err(|e| e.message)?;
            client
                .borrow_mut()
                .panel("ext.xml.xpath", values.join("\n"))
        }
        _ => Err("unsupported XML command".into()),
    }
}
#[cfg(target_arch = "wasm32")]
struct Component;
#[cfg(target_arch = "wasm32")]
impl bareline_extension_sdk::bindings::Guest for Component {
    fn run() {
        if let Ok(client) = bareline_first_party_common::wasm_client() {
            if let Err(error) = run(client.clone()) {
                let _ = client.borrow_mut().panel("ext.xml.xpath", error);
            }
        }
    }
}
#[cfg(target_arch = "wasm32")]
bareline_extension_sdk::bindings::export!(Component with_types_in bareline_extension_sdk::bindings);

// SPDX-License-Identifier: MIT OR Apache-2.0
fn name_start(c: char) -> bool {
    matches!(c,'A'..='Z'|'a'..='z'|'_'|'\u{c0}'..='\u{d6}'|'\u{d8}'..='\u{f6}'|'\u{f8}'..='\u{2ff}'|'\u{370}'..='\u{37d}'|'\u{37f}'..='\u{1fff}'|'\u{200c}'..='\u{200d}'|'\u{2070}'..='\u{218f}'|'\u{2c00}'..='\u{2fef}'|'\u{3001}'..='\u{d7ff}'|'\u{f900}'..='\u{fdcf}'|'\u{fdf0}'..='\u{fffd}'|'\u{10000}'..='\u{effff}')
}
fn qname(bytes: &[u8]) -> bool {
    let Ok(name) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut parts = name.split(':');
    let mut count = 0;
    for part in &mut parts {
        count += 1;
        let mut chars = part.chars();
        if !chars.next().is_some_and(name_start)||!chars.all(|c|name_start(c)||matches!(c,'0'..='9'|'-'|'.'|'\u{b7}'|'\u{300}'..='\u{36f}'|'\u{203f}'..='\u{2040}')){return false;}
    }
    count <= 2
}

/// First line is the XPath expression; later lines bind namespace prefixes.
pub fn parse_arguments(arguments: &str) -> Result<(String, BTreeMap<String, String>), String> {
    if arguments.len() > 4096 {
        return Err("XPath arguments exceed 4096 bytes".into());
    }
    let mut lines = arguments.lines();
    let first = lines.next().unwrap_or("").trim();
    let expression = if first.is_empty() { "/*" } else { first }.to_owned();
    let mut namespaces = BTreeMap::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let (prefix, uri) = line
            .split_once('=')
            .ok_or("namespace bindings use prefix=URI")?;
        let prefix = prefix.trim();
        let uri = uri.trim();
        if prefix.contains(':')
            || !qname(prefix.as_bytes())
            || uri.is_empty()
            || prefix == "xmlns"
            || (prefix == "xml" && uri != "http://www.w3.org/XML/1998/namespace")
        {
            return Err("invalid XPath namespace binding".into());
        }
        if namespaces.insert(prefix.into(), uri.into()).is_some() {
            return Err("duplicate XPath namespace prefix".into());
        }
        if namespaces.len() > 64 {
            return Err("XPath namespace limit".into());
        }
    }
    Ok((expression, namespaces))
}
#[cfg(test)]
mod argument_tests {
    use super::*;
    #[test]
    fn arguments_and_descendant_positions() {
        assert!(parse_arguments("//a:x\na=urn:x\na=urn:y").is_err());
        assert!(parse_arguments("/x\nbad:prefix=urn:x").is_err());
        let doc =
            parse(b"<r><g><n>a</n><n>b</n></g><g><n>c</n><n>d</n></g></r>".as_slice()).unwrap();
        assert_eq!(
            query(&doc, "//n[2]/text()", &BTreeMap::new()).unwrap(),
            ["b", "d"]
        );
        assert!(query(&doc, "/r/@x[1]", &BTreeMap::new()).is_err());
    }
    #[test]
    fn mixed_text_nodes_are_not_concatenated_for_xpath() {
        let doc = parse(b"<r>one<x/>two&amp;three</r>".as_slice()).unwrap();
        assert_eq!(
            query(&doc, "/r/text()", &BTreeMap::new()).unwrap(),
            ["one", "two&three"]
        );
    }
}

#[cfg(test)]
mod terminal_selection_tests {
    use super::*;
    #[test]
    fn descendant_terminal_selection_and_missing_attributes() {
        let doc = parse(b"<r id='root'><n id='child'>x</n><n>y</n></r>".as_slice()).unwrap();
        let ns = BTreeMap::new();
        assert_eq!(query(&doc, "/r//text()", &ns).unwrap(), ["x", "y"]);
        assert_eq!(query(&doc, "/r//@id", &ns).unwrap(), ["root", "child"]);
        assert!(query(&doc, "/r/@missing", &ns).unwrap().is_empty());
    }
}

#[cfg(test)]
mod decoded_view_tests {
    use super::*;
    #[test]
    fn original_encoding_declaration_survives_decoded_text_format() {
        let source = "<?xml version='1.0' encoding='UTF-16'?><r>é</r>";
        assert!(parse(source.as_bytes()).is_ok());
        let mut out = Vec::new();
        format(source.as_bytes(), &mut out).unwrap();
        assert_eq!(out, source.as_bytes());
        assert!(parse(b"<?xml version='1.0' encoding='123'?><r/>".as_slice()).is_err());
    }
}
