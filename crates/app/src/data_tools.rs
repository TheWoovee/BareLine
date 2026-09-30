// SPDX-License-Identifier: MPL-2.0
//! Built-in JSON, XML and Hex tools. Bareline 1.0 ships without third-party
//! plugins (BIZ-04), so these run natively on the utility worker and reuse the
//! pure logic of the first-party json-tools, xml-tools and hex-view components.
//! Nothing here touches the UI thread or mutates a document: edits come back as
//! replacement text that the caller applies as one undoable transaction.
use bareline_document::{DocumentSnapshot, Edit, EditTransaction, Revision, TextOffset};
use std::{io::Write, ops::Range};

/// Largest text a JSON or XML tool reads; the XML parser has the same ceiling.
pub const MAX_INPUT_BYTES: usize = bareline_xml_tools::MAX_INPUT as usize;
/// Largest formatted text staged as one edit.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
/// Original file bytes one Hex View shows.
pub const MAX_HEX_BYTES: u64 = 1024 * 1024;
/// Opens the XPath prompt.
pub const XPATH_COMMAND: &str = "utilities.xpathQuery";
/// Runs the query the XPath prompt collected.
pub const XPATH_RUN_COMMAND: &str = "utilities.xpathRun";
/// Opens the original file bytes as a read-only hex tab.
pub const HEX_COMMAND: &str = "utilities.hexView";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataTool {
    JsonFormat,
    JsonMinify,
    JsonValidate,
    XmlFormat,
    XmlValidate,
}
impl DataTool {
    pub fn from_command(id: &str) -> Option<Self> {
        Some(match id {
            "utilities.jsonFormat" => Self::JsonFormat,
            "utilities.jsonMinify" => Self::JsonMinify,
            "utilities.jsonValidate" => Self::JsonValidate,
            "utilities.xmlFormat" => Self::XmlFormat,
            "utilities.xmlValidate" => Self::XmlValidate,
            _ => return None,
        })
    }
    /// True for the tools that replace the text rather than report on it.
    pub fn edits(self) -> bool {
        matches!(self, Self::JsonFormat | Self::JsonMinify | Self::XmlFormat)
    }
    /// What a successful edit did, for the status line.
    pub fn done(self) -> &'static str {
        match self {
            Self::JsonFormat => "Formatted the JSON",
            Self::JsonMinify => "Minified the JSON",
            Self::XmlFormat => "Formatted the XML",
            Self::JsonValidate | Self::XmlValidate => "Checked the text",
        }
    }
    fn language(self) -> &'static str {
        match self {
            Self::JsonFormat | Self::JsonMinify | Self::JsonValidate => "JSON",
            Self::XmlFormat | Self::XmlValidate => "XML",
        }
    }
}

/// True for every command this module runs, including the XPath and Hex ones.
pub fn is_tool_command(id: &str) -> bool {
    DataTool::from_command(id).is_some() || matches!(id, XPATH_COMMAND | XPATH_RUN_COMMAND | HEX_COMMAND)
}

/// One indentation level for formatted output, from the editor's Tab settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Spaces(u8),
    Tab,
}
impl Indent {
    pub fn from_settings(tab_width: u8, insert_spaces: bool) -> Self {
        if insert_spaces {
            Self::Spaces(tab_width.clamp(1, 16))
        } else {
            Self::Tab
        }
    }
    fn unit(self) -> (u8, usize) {
        match self {
            Self::Spaces(count) => (b' ', count as usize),
            Self::Tab => (b'\t', 1),
        }
    }
}

/// What a tool produced for a document range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolOutput {
    /// Replacement for the whole range, applied as one undoable edit.
    Replace(String),
    /// A sentence for the result panel; the document is unchanged.
    Report(String),
    /// The text is invalid at `offset` (a document offset, where the caret
    /// goes); `message` names the line and column.
    Invalid { offset: TextOffset, message: String },
}

/// The one-step edit that replaces `range` with a tool's output.
pub fn replacement(base_revision: Revision, range: Range<TextOffset>, text: String) -> EditTransaction {
    EditTransaction {
        base_revision,
        edits: vec![Edit { range, insert: text }],
    }
}

/// Runs `tool` over a range of a resident document. The error is a sentence
/// for the user, such as the size cap.
pub fn run_on_snapshot(
    snapshot: &DocumentSnapshot,
    range: Range<TextOffset>,
    tool: DataTool,
    indent: Indent,
) -> Result<ToolOutput, String> {
    let input = read_capped(snapshot, range.clone())?;
    let eol = detect_eol(&input).unwrap_or(snapshot.insertion_eol());
    run(&input, Some(snapshot), range.start.0, tool, indent, eol)
}

/// Runs `tool` over text already read from a document at `base`, such as a
/// paged selection. `fallback_eol` is used when the text has no line break.
pub fn run_on_text(
    input: &str,
    base: TextOffset,
    tool: DataTool,
    indent: Indent,
    fallback_eol: &str,
) -> Result<ToolOutput, String> {
    check_size(input.len(), MAX_INPUT_BYTES)?;
    run(
        input,
        None,
        base.0,
        tool,
        indent,
        detect_eol(input).unwrap_or(fallback_eol),
    )
}

/// Runs a bounded XPath query over a range of a resident document.
pub fn xpath_on_snapshot(
    snapshot: &DocumentSnapshot,
    range: Range<TextOffset>,
    prompt: &str,
) -> Result<ToolOutput, String> {
    let input = read_capped(snapshot, range.clone())?;
    xpath(&input, Some(snapshot), range.start.0, prompt)
}

/// Runs a bounded XPath query over text already read from a document at `base`.
pub fn xpath_on_text(input: &str, base: TextOffset, prompt: &str) -> Result<ToolOutput, String> {
    check_size(input.len(), MAX_INPUT_BYTES)?;
    xpath(input, None, base.0, prompt)
}

/// Refuses `bytes` of input above `limit` with a sentence naming the limit.
pub fn check_size(bytes: usize, limit: usize) -> Result<(), String> {
    if bytes > limit {
        return Err(format!(
            "JSON and XML tools work on up to {} MiB of text. Select a smaller range and try again.",
            limit / (1024 * 1024)
        ));
    }
    Ok(())
}

fn read_capped(snapshot: &DocumentSnapshot, range: Range<TextOffset>) -> Result<String, String> {
    if !snapshot.is_complete() {
        return Err("The document is still loading. Try again when it finishes.".into());
    }
    check_size(range.end.0.saturating_sub(range.start.0), MAX_INPUT_BYTES)?;
    snapshot
        .read(range, MAX_INPUT_BYTES)
        .map_err(|_| "The selected text is no longer available. Select it again and retry.".into())
}

fn run(
    input: &str,
    snapshot: Option<&DocumentSnapshot>,
    base: usize,
    tool: DataTool,
    indent: Indent,
    eol: &str,
) -> Result<ToolOutput, String> {
    let language = tool.language();
    let (unit, size) = indent.unit();
    let mut output = Limited::new(MAX_OUTPUT_BYTES);
    let failure = match tool {
        DataTool::JsonFormat | DataTool::JsonMinify | DataTool::JsonValidate => {
            let layout = match tool {
                DataTool::JsonFormat => bareline_json_tools::Layout::Pretty,
                DataTool::JsonMinify => bareline_json_tools::Layout::Minify,
                _ => bareline_json_tools::Layout::Validate,
            };
            bareline_json_tools::process_with_indent(input.as_bytes(), &mut output, layout, &vec![unit; size])
                .err()
                .map(|error| (error.offset, error.message))
        }
        DataTool::XmlFormat => bareline_xml_tools::format_with_indent(input.as_bytes(), &mut output, unit, size)
            .err()
            .map(|error| (error.offset, error.message)),
        DataTool::XmlValidate => bareline_xml_tools::parse(input.as_bytes())
            .err()
            .map(|error| (error.offset, error.message)),
    };
    if output.exceeded {
        return Err(output_too_large());
    }
    if let Some((offset, detail)) = failure {
        return Ok(invalid(input, snapshot, base, language, offset, &detail));
    }
    match tool {
        DataTool::JsonValidate => return Ok(ToolOutput::Report("Valid JSON.".into())),
        DataTool::XmlValidate => {
            return Ok(ToolOutput::Report(
                "Well-formed XML. DTDs and external entities are never loaded.".into(),
            ));
        }
        _ => {}
    }
    let formatted =
        String::from_utf8(output.bytes).map_err(|_| "The result is not valid text; the document was not changed.")?;
    let mut text = normalize_eol(&formatted, eol);
    // Keep the final line break the input ended with.
    if let Some(end) = trailing_break(input)
        && !text.ends_with(['\r', '\n'])
    {
        text.push_str(end);
    }
    if text.len() > MAX_OUTPUT_BYTES {
        return Err(output_too_large());
    }
    if text == input {
        return Ok(ToolOutput::Report(
            match tool {
                DataTool::JsonMinify => "The JSON is already minified; nothing changed.",
                DataTool::XmlFormat => {
                    "The XML is already formatted, or its whitespace is significant; nothing changed."
                }
                _ => "The JSON is already formatted; nothing changed.",
            }
            .into(),
        ));
    }
    Ok(ToolOutput::Replace(text))
}

/// `prompt` is the XPath expression, optionally followed by `|` and
/// space-separated `prefix=URI` namespace bindings.
fn xpath(input: &str, snapshot: Option<&DocumentSnapshot>, base: usize, prompt: &str) -> Result<ToolOutput, String> {
    let (expression, bindings) = split_bindings(prompt);
    let arguments = std::iter::once(expression.trim())
        .chain(bindings.split_whitespace())
        .collect::<Vec<_>>()
        .join("\n");
    let (expression, namespaces) = bareline_xml_tools::parse_arguments(&arguments).map_err(|error| {
        format!("The namespace bindings are not valid ({error}). Write them after a bar: | prefix=URI.")
    })?;
    let document = match bareline_xml_tools::parse(input.as_bytes()) {
        Ok(document) => document,
        Err(error) => return Ok(invalid(input, snapshot, base, "XML", error.offset, &error.message)),
    };
    let found = bareline_xml_tools::query_located(&document, &expression, &namespaces).map_err(|error| {
        format!(
            "This XPath query is not supported ({}). Use paths such as /root/item, //item[@id='1'], //item[2]/text() or //item/@name.",
            error.message
        )
    })?;
    if found.is_empty() {
        return Ok(ToolOutput::Report(format!("No matches for {expression}")));
    }
    let mut report = format!(
        "{} {} for {expression}",
        found.len(),
        if found.len() == 1 { "match" } else { "matches" }
    );
    for (offset, value) in found {
        let offset = floor_boundary(input, usize::try_from(offset).unwrap_or(usize::MAX));
        let long = value.chars().nth(500).is_some();
        let mut value: String = value.replace(['\r', '\n'], " ").chars().take(500).collect();
        if long {
            value.push('\u{2026}');
        }
        let (line, column, of_selection) = position(input, snapshot, base, offset);
        report.push_str(&format!("\nLine {line}, column {column}{of_selection}: {value}"));
    }
    Ok(ToolOutput::Report(report))
}

/// Splits `//a:item | a=urn:x` into the expression and its bindings. A bar
/// whose tail is not all `prefix=URI` tokens stays part of the expression.
fn split_bindings(prompt: &str) -> (&str, &str) {
    match prompt.rsplit_once('|') {
        Some((expression, tail))
            if !tail.trim().is_empty() && tail.split_whitespace().all(|token| token.contains('=')) =>
        {
            (expression, tail)
        }
        _ => (prompt, ""),
    }
}

fn invalid(
    input: &str,
    snapshot: Option<&DocumentSnapshot>,
    base: usize,
    language: &str,
    offset: u64,
    detail: &str,
) -> ToolOutput {
    let offset = floor_boundary(input, usize::try_from(offset).unwrap_or(usize::MAX));
    let (line, column, of_selection) = position(input, snapshot, base, offset);
    ToolOutput::Invalid {
        offset: TextOffset(base + offset),
        message: format!("{language} is not valid at line {line}, column {column}{of_selection}: {detail}."),
    }
}

/// One-based line and column of an input offset. A resident document gives
/// document positions; otherwise positions count from the start of the input.
fn position(
    input: &str,
    snapshot: Option<&DocumentSnapshot>,
    base: usize,
    offset: usize,
) -> (usize, usize, &'static str) {
    if let Some((line, column)) =
        snapshot.and_then(|snapshot| document_line_column(snapshot, TextOffset(base + offset)))
    {
        return (line, column, "");
    }
    let (line, column) = line_column(input, offset);
    (line, column, if base == 0 { "" } else { " of the selection" })
}

/// One-based line and column (in characters) of `offset` in a document.
pub fn document_line_column(snapshot: &DocumentSnapshot, offset: TextOffset) -> Option<(usize, usize)> {
    let line = snapshot.line_at(offset).ok()?;
    let start = snapshot.line_range(line).ok()?.start;
    let column = snapshot
        .chunks(start..offset)
        .ok()?
        .map(|chunk| chunk.chars().count())
        .sum::<usize>();
    Some((line + 1, column + 1))
}

/// One-based line and column (in characters) of `offset` in `text`; CR, LF
/// and CRLF each end a line.
pub fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..floor_boundary(text, offset)];
    let bytes = before.as_bytes();
    let (mut line, mut start, mut index) = (1, 0, 0);
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            index += 1;
        }
        if matches!(bytes[index], b'\r' | b'\n') {
            line += 1;
            start = index + 1;
        }
        index += 1;
    }
    (line, before[start..].chars().count() + 1)
}

fn floor_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn detect_eol(text: &str) -> Option<&'static str> {
    let index = text.find(['\r', '\n'])?;
    Some(match &text[index..] {
        rest if rest.starts_with("\r\n") => "\r\n",
        rest if rest.starts_with('\r') => "\r",
        _ => "\n",
    })
}

fn trailing_break(text: &str) -> Option<&'static str> {
    if text.ends_with("\r\n") {
        Some("\r\n")
    } else if text.ends_with('\n') {
        Some("\n")
    } else if text.ends_with('\r') {
        Some("\r")
    } else {
        None
    }
}

/// Formatters write `\n`; the document keeps its own line ending.
fn normalize_eol(text: &str, eol: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(['\r', '\n']) {
        output.push_str(&rest[..index]);
        output.push_str(eol);
        let width = if rest[index..].starts_with("\r\n") { 2 } else { 1 };
        rest = &rest[index + width..];
    }
    output.push_str(rest);
    output
}

fn output_too_large() -> String {
    format!(
        "The result would be larger than {} MiB. Select a smaller range and try again.",
        MAX_OUTPUT_BYTES / (1024 * 1024)
    )
}

/// Collects formatter output and refuses to grow past its limit.
struct Limited {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl Limited {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }
}
impl Write for Limited {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(data.len()) > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::other("output limit"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Renders the first [`MAX_HEX_BYTES`] of a file's original bytes as a
/// read-only hex dump, one bounded hex-view page at a time. `read` returns
/// exactly the original bytes of the range it is given.
pub fn hex_dump(
    title: &str,
    total: u64,
    mut read: impl FnMut(Range<u64>) -> Result<Vec<u8>, String>,
) -> Result<String, String> {
    use bareline_hex_view::{MAX_ROWS, ROW_BYTES, Viewport};
    let shown = total.min(MAX_HEX_BYTES);
    let mut text = format!("{title} \u{2014} original file bytes ({total} bytes)\n");
    let mut offset = 0;
    loop {
        let view = Viewport {
            document: 0,
            generation: 0,
            original_length: shown,
            offset,
            rows: MAX_ROWS,
        };
        let bareline_extensions_protocol::Request::ReadOriginalBytes { range, .. } = view.request()? else {
            return Err("Hex View could not plan its read.".into());
        };
        let bytes = read(range.start..range.end)?;
        let page = view.render(range.start, Some(&bytes))?;
        // Keep the component's read-only notice and column header once; its
        // generation line is replaced by the title above.
        for line in page.lines().skip(if offset == 0 { 1 } else { 3 }) {
            text.push_str(line);
            text.push('\n');
        }
        offset += ROW_BYTES * MAX_ROWS;
        if offset >= shown {
            break;
        }
    }
    if total > shown {
        text.push_str(&format!(
            "Showing the first {} MiB of {total} bytes. Hex View stays bounded to keep the editor responsive.\n",
            MAX_HEX_BYTES / (1024 * 1024)
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};

    fn document(text: &str) -> Document {
        Document::from_utf8(text, Budget::new(64 << 20), Budget::new(64 << 20)).unwrap()
    }
    fn whole(document: &Document) -> Range<TextOffset> {
        TextOffset(0)..TextOffset(document.snapshot().len())
    }
    fn text(document: &Document) -> String {
        document.snapshot().read(whole(document), 1 << 20).unwrap()
    }

    #[test]
    fn json_format_and_minify_round_trip_with_settings_indent_and_eol() {
        let source = r#"{"a":[1,2],"b":{"c":"é"}}"#;
        let ToolOutput::Replace(pretty) =
            run_on_text(source, TextOffset(0), DataTool::JsonFormat, Indent::Spaces(4), "\r\n").unwrap()
        else {
            panic!("format must replace");
        };
        assert_eq!(
            pretty,
            "{\r\n    \"a\": [\r\n        1,\r\n        2\r\n    ],\r\n    \"b\": {\r\n        \"c\": \"é\"\r\n    }\r\n}"
        );
        assert_eq!(
            run_on_text(&pretty, TextOffset(0), DataTool::JsonMinify, Indent::Tab, "\n").unwrap(),
            ToolOutput::Replace(source.into())
        );
        // Formatting formatted text is a report, not an edit.
        assert!(matches!(
            run_on_text(&pretty, TextOffset(0), DataTool::JsonFormat, Indent::Spaces(4), "\n").unwrap(),
            ToolOutput::Report(_)
        ));
        assert!(matches!(
            run_on_text(source, TextOffset(0), DataTool::JsonMinify, Indent::Tab, "\n").unwrap(),
            ToolOutput::Report(_)
        ));
        // The document's own line ending and final break survive.
        assert_eq!(
            run_on_text("[1]\n", TextOffset(0), DataTool::JsonFormat, Indent::Tab, "\r\n").unwrap(),
            ToolOutput::Replace("[\n\t1\n]\n".into())
        );
        assert_eq!(Indent::from_settings(2, true), Indent::Spaces(2));
        assert_eq!(Indent::from_settings(0, true), Indent::Spaces(1));
        assert_eq!(Indent::from_settings(4, false), Indent::Tab);
    }

    #[test]
    fn json_validate_reports_line_column_and_caret_offset() {
        assert_eq!(
            run_on_text("{\"a\": [1]}", TextOffset(0), DataTool::JsonValidate, Indent::Tab, "\n").unwrap(),
            ToolOutput::Report("Valid JSON.".into())
        );
        let ToolOutput::Invalid { offset, message } = run_on_text(
            "{\n  \"a\": tru\n}",
            TextOffset(0),
            DataTool::JsonValidate,
            Indent::Tab,
            "\n",
        )
        .unwrap() else {
            panic!("invalid JSON must be located");
        };
        assert_eq!(offset, TextOffset(12));
        assert!(message.contains("line 2, column 11"), "{message}");
        // Format refuses invalid JSON the same way instead of editing.
        assert!(matches!(
            run_on_text("[1,]", TextOffset(0), DataTool::JsonFormat, Indent::Tab, "\n").unwrap(),
            ToolOutput::Invalid {
                offset: TextOffset(3),
                ..
            }
        ));
        // A selection reports document positions and a document caret offset.
        let doc = document("// note\n[1,\n!]");
        let ToolOutput::Invalid { offset, message } = run_on_snapshot(
            &doc.snapshot(),
            TextOffset(8)..TextOffset(14),
            DataTool::JsonValidate,
            Indent::Tab,
        )
        .unwrap() else {
            panic!("invalid selection must be located");
        };
        assert_eq!(offset, TextOffset(12));
        assert!(message.contains("line 3, column 1:"), "{message}");
        // Text read from a paged selection counts from the selection.
        let ToolOutput::Invalid { offset, message } =
            run_on_text("[1,\n!]", TextOffset(8), DataTool::JsonValidate, Indent::Tab, "\n").unwrap()
        else {
            panic!("invalid paged selection must be located");
        };
        assert_eq!(offset, TextOffset(12));
        assert!(message.contains("line 2, column 1 of the selection"), "{message}");
    }

    #[test]
    fn xml_format_validate_and_xpath_positions() {
        assert_eq!(
            run_on_text(
                "<r><a x='1'/><b/></r>",
                TextOffset(0),
                DataTool::XmlFormat,
                Indent::Tab,
                "\n"
            )
            .unwrap(),
            ToolOutput::Replace("<r>\n\t<a x='1'/>\n\t<b/>\n</r>".into())
        );
        assert!(matches!(
            run_on_text(
                "<p>keep <b>this</b></p>",
                TextOffset(0),
                DataTool::XmlFormat,
                Indent::Tab,
                "\n"
            )
            .unwrap(),
            ToolOutput::Report(_)
        ));
        assert!(matches!(
            run_on_text("<r/>", TextOffset(0), DataTool::XmlValidate, Indent::Tab, "\n").unwrap(),
            ToolOutput::Report(_)
        ));
        let ToolOutput::Invalid { offset, message } = run_on_text(
            "<r>\n  <a></b>\n</r>",
            TextOffset(0),
            DataTool::XmlValidate,
            Indent::Tab,
            "\n",
        )
        .unwrap() else {
            panic!("mismatched tags must be located");
        };
        assert!((4..=13).contains(&offset.0), "{offset:?}");
        assert!(message.contains("line 2,"), "{message}");
        assert!(matches!(
            run_on_text(
                "<!DOCTYPE x SYSTEM 'https://bad'><x/>",
                TextOffset(0),
                DataTool::XmlValidate,
                Indent::Tab,
                "\n"
            )
            .unwrap(),
            ToolOutput::Invalid { .. }
        ));

        let source = "<r>\n  <n k='a'>x</n>\n  <n>y</n>\n</r>";
        let ToolOutput::Report(report) = xpath_on_text(source, TextOffset(0), "//n/text()").unwrap() else {
            panic!("query must report");
        };
        assert!(report.starts_with("2 matches for //n/text()"), "{report}");
        assert!(report.contains("Line 2, column 3: x"), "{report}");
        assert!(report.contains("Line 3, column 3: y"), "{report}");
        let doc = document(source);
        let ToolOutput::Report(report) = xpath_on_snapshot(&doc.snapshot(), whole(&doc), "/r/n[2]").unwrap() else {
            panic!("query must report");
        };
        assert!(report.contains("Line 3, column 3"), "{report}");
        let ToolOutput::Report(report) = xpath_on_text(
            "<r xmlns:a='urn:x'><a:n>v</a:n></r>",
            TextOffset(0),
            "//a:n/text() | a=urn:x",
        )
        .unwrap() else {
            panic!("bound prefix must query");
        };
        assert!(report.starts_with("1 match for"), "{report}");
        assert_eq!(
            xpath_on_text(source, TextOffset(0), "//missing").unwrap(),
            ToolOutput::Report("No matches for //missing".into())
        );
        assert!(
            xpath_on_text(source, TextOffset(0), "//n[contains(text(),'x')]")
                .unwrap_err()
                .contains("not supported")
        );
        // A bar inside a predicate is part of the expression, not a binding.
        assert_eq!(split_bindings("//n[@k='a|b']"), ("//n[@k='a|b']", ""));
        assert_eq!(split_bindings("//a:n | a=urn:x"), ("//a:n ", " a=urn:x"));
        // Malformed XML is located instead of queried.
        assert!(matches!(
            xpath_on_text("<r>\n<n>", TextOffset(0), "//n").unwrap(),
            ToolOutput::Invalid { .. }
        ));
    }

    #[test]
    fn format_is_one_undoable_edit() {
        let original = "{\"a\":1,\"b\":[true,null]}\n";
        let mut doc = document(original);
        let snapshot = doc.snapshot();
        let range = whole(&doc);
        let ToolOutput::Replace(formatted) =
            run_on_snapshot(&snapshot, range.clone(), DataTool::JsonFormat, Indent::Spaces(2)).unwrap()
        else {
            panic!("format must replace");
        };
        assert_eq!(text(&doc), original, "staging must not mutate");
        doc.apply(replacement(snapshot.revision, range, formatted)).unwrap();
        assert_eq!(text(&doc), "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null\n  ]\n}\n");
        doc.undo().unwrap();
        assert_eq!(text(&doc), original);
        assert!(!doc.dirty());
        assert!(doc.undo().is_err(), "format must be exactly one undo step");
    }

    #[test]
    fn inputs_above_the_cap_are_refused_with_a_clear_message() {
        assert!(check_size(MAX_INPUT_BYTES, MAX_INPUT_BYTES).is_ok());
        let error = check_size(MAX_INPUT_BYTES + 1, MAX_INPUT_BYTES).unwrap_err();
        assert!(error.contains("16 MiB") && error.contains("smaller range"), "{error}");
        assert!(check_size(2 << 20, 1 << 20).unwrap_err().contains("1 MiB"));
        let big = " ".repeat(MAX_INPUT_BYTES + 1);
        for tool in [DataTool::JsonFormat, DataTool::XmlValidate] {
            let error = run_on_text(&big, TextOffset(0), tool, Indent::Tab, "\n").unwrap_err();
            assert!(error.contains("16 MiB"), "{error}");
        }
        assert!(xpath_on_text(&big, TextOffset(0), "/r").unwrap_err().contains("16 MiB"));
        let doc = document(&big);
        let error = run_on_snapshot(&doc.snapshot(), whole(&doc), DataTool::JsonMinify, Indent::Tab).unwrap_err();
        assert!(error.contains("16 MiB"), "{error}");
        // A selection within the cap of that same document is processed.
        assert!(matches!(
            run_on_snapshot(
                &doc.snapshot(),
                TextOffset(0)..TextOffset(8),
                DataTool::JsonValidate,
                Indent::Tab
            ),
            Ok(ToolOutput::Invalid { .. })
        ));
        // Output is bounded too; a full collector fails the write.
        let mut limited = Limited::new(4);
        assert!(limited.write_all(b"1234").is_ok());
        assert!(limited.write_all(b"5").is_err());
        assert!(limited.exceeded);
    }

    #[test]
    fn hex_dump_is_paged_bounded_and_shows_original_bytes() {
        let bytes: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        let mut reads = 0;
        let dump = hex_dump("a.bin", bytes.len() as u64, |range| {
            reads += 1;
            assert!(range.end - range.start <= 4 * 1024 + 64, "one bounded page per read");
            Ok(bytes[range.start as usize..range.end as usize].to_vec())
        })
        .unwrap();
        assert_eq!(reads, 2);
        assert!(dump.starts_with("a.bin \u{2014} original file bytes (5000 bytes)\n"));
        assert_eq!(dump.matches("Raw offset").count(), 1);
        assert!(dump.contains("0000000000000000  00 01 02 03"));
        assert!(dump.contains("0000000000001000  00 01 02 03"));
        assert!(dump.contains("0000000000001380  80 81 82 83 84 85 86 87"));
        assert!(!dump.contains("Showing the first"));

        let mut furthest = 0;
        let dump = hex_dump("big.bin", 5 << 30, |range| {
            furthest = furthest.max(range.end);
            Ok(vec![0x41; (range.end - range.start) as usize])
        })
        .unwrap();
        assert_eq!(furthest, MAX_HEX_BYTES);
        assert_eq!(dump.lines().filter(|line| line.starts_with("00000000")).count(), 65536);
        assert!(dump.ends_with("Hex View stays bounded to keep the editor responsive.\n"));

        let empty = hex_dump("empty.bin", 0, |range| {
            assert!(range.is_empty());
            Ok(Vec::new())
        })
        .unwrap();
        assert_eq!(empty.matches("Raw offset").count(), 1);
        assert!(hex_dump("gone.bin", 10, |_| Err("Original disk generation changed".into())).is_err());
    }

    #[test]
    fn positions_count_every_line_ending_and_characters() {
        assert_eq!(line_column("a\r\nb\rc\nd", 7), (4, 1));
        assert_eq!(line_column("a\r\nb\rc\nd", 8), (4, 2));
        assert_eq!(line_column("é\nxé!", 6), (2, 3));
        assert_eq!(line_column("abc", 99), (1, 4));
        assert_eq!(normalize_eol("a\nb\r\nc\rd", "\r\n"), "a\r\nb\r\nc\r\nd");
        assert_eq!(DataTool::from_command("utilities.xmlFormat"), Some(DataTool::XmlFormat));
        assert_eq!(DataTool::from_command(XPATH_COMMAND), None);
        assert!(is_tool_command(HEX_COMMAND) && is_tool_command("utilities.jsonMinify"));
        assert!(!is_tool_command("utilities.base64Encode"));
    }
}
