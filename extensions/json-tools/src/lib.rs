// SPDX-License-Identifier: MIT OR Apache-2.0
//! Streaming JSON grammar: preserves original string and number lexemes.
pub mod tree;
use std::io::{BufRead, BufReader, Read, Write};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub offset: u64,
    pub line: u64,
    pub message: String,
}
#[derive(Clone, Copy)]
pub enum Layout {
    Validate,
    Minify,
    Pretty,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub start: u64,
    pub end: u64,
    pub kind: &'static str,
}
struct Parser<R: Read, W: Write> {
    input: BufReader<R>,
    out: W,
    layout: Layout,
    offset: u64,
    line: u64,
    depth: usize,
    nodes: Vec<Node>,
}
impl<R: Read, W: Write> Parser<R, W> {
    fn error(&self, message: &str) -> Error {
        Error {
            offset: self.offset,
            line: self.line,
            message: message.into(),
        }
    }
    fn peek(&mut self) -> Result<Option<u8>, Error> {
        self.input
            .fill_buf()
            .map(|b| b.first().copied())
            .map_err(|e| self.error(&e.to_string()))
    }
    fn take(&mut self) -> Result<u8, Error> {
        let b = self
            .peek()?
            .ok_or_else(|| self.error("unexpected end of JSON"))?;
        self.input.consume(1);
        self.offset += 1;
        if b == b'\n' {
            self.line += 1;
        }
        Ok(b)
    }
    fn emit(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if !matches!(self.layout, Layout::Validate) {
            self.out
                .write_all(bytes)
                .map_err(|e| self.error(&e.to_string()))?;
        }
        Ok(())
    }
    fn copy(&mut self) -> Result<u8, Error> {
        let b = self.take()?;
        self.emit(&[b])?;
        Ok(b)
    }
    fn ws(&mut self) -> Result<(), Error> {
        while matches!(self.peek()?, Some(b' ' | b'\r' | b'\n' | b'\t')) {
            self.take()?;
        }
        Ok(())
    }
    fn indent(&mut self) -> Result<(), Error> {
        if matches!(self.layout, Layout::Pretty) {
            self.emit(b"\n")?;
            for _ in 0..self.depth {
                self.emit(b"  ")?;
            }
        }
        Ok(())
    }
    fn expect(&mut self, b: u8) -> Result<(), Error> {
        if self.peek()? != Some(b) {
            return Err(self.error("unexpected JSON token"));
        }
        self.copy()?;
        Ok(())
    }
    fn hex(&mut self) -> Result<u16, Error> {
        let mut value = 0;
        for _ in 0..4 {
            let b = self
                .peek()?
                .ok_or_else(|| self.error("truncated Unicode escape"))?;
            let digit = (b as char)
                .to_digit(16)
                .ok_or_else(|| self.error("invalid Unicode escape"))?;
            self.copy()?;
            value = value * 16 + digit as u16;
        }
        Ok(value)
    }
    fn string(&mut self) -> Result<(), Error> {
        self.expect(b'"')?;
        loop {
            // Ordinary ASCII cannot contain an escape, delimiter, newline or
            // UTF-8 continuation. Consume its buffered span without allocating.
            let span = (|| -> std::io::Result<usize> {
                let bytes = self.input.fill_buf()?;
                let count = bytes
                    .iter()
                    .position(|&b| b < 0x20 || b >= 0x80 || b == b'"' || b == b'\\')
                    .unwrap_or(bytes.len());
                if count != 0 && !matches!(self.layout, Layout::Validate) {
                    self.out.write_all(&bytes[..count])?;
                }
                Ok(count)
            })()
            .map_err(|e| self.error(&e.to_string()))?;
            if span != 0 {
                self.input.consume(span);
                self.offset += span as u64;
                continue;
            }
            let b = self
                .peek()?
                .ok_or_else(|| self.error("unterminated string"))?;
            match b {
                b'"' => {
                    self.copy()?;
                    return Ok(());
                }
                0..=31 => return Err(self.error("unescaped control character")),
                b'\\' => {
                    self.copy()?;
                    let escape = self.copy()?;
                    match escape {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                        b'u' => {
                            let n = self.hex()?;
                            if (0xd800..=0xdbff).contains(&n) {
                                self.expect(b'\\')?;
                                self.expect(b'u')?;
                                if !(0xdc00..=0xdfff).contains(&self.hex()?) {
                                    return Err(self.error("invalid surrogate pair"));
                                }
                            } else if (0xdc00..=0xdfff).contains(&n) {
                                return Err(self.error("unpaired low surrogate"));
                            }
                        }
                        _ => return Err(self.error("invalid string escape")),
                    }
                }
                0x80..=0xff => {
                    let width = match b {
                        0xc2..=0xdf => 2,
                        0xe0..=0xef => 3,
                        0xf0..=0xf4 => 4,
                        _ => return Err(self.error("invalid UTF-8")),
                    };
                    let mut buf = [0; 4];
                    for slot in buf.iter_mut().take(width) {
                        *slot = self.take()?;
                    }
                    std::str::from_utf8(&buf[..width]).map_err(|_| self.error("invalid UTF-8"))?;
                    self.emit(&buf[..width])?;
                }
                _ => {
                    self.copy()?;
                }
            }
        }
    }
    fn digits(&mut self) -> Result<(), Error> {
        if !matches!(self.peek()?, Some(b'0'..=b'9')) {
            return Err(self.error("expected digit"));
        }
        while matches!(self.peek()?, Some(b'0'..=b'9')) {
            self.copy()?;
        }
        Ok(())
    }
    fn number(&mut self) -> Result<(), Error> {
        if self.peek()? == Some(b'-') {
            self.copy()?;
        }
        if self.peek()? == Some(b'0') {
            self.copy()?;
        } else {
            self.digits()?;
        }
        if self.peek()? == Some(b'.') {
            self.copy()?;
            self.digits()?;
        }
        if matches!(self.peek()?, Some(b'e' | b'E')) {
            self.copy()?;
            if matches!(self.peek()?, Some(b'+' | b'-')) {
                self.copy()?;
            }
            self.digits()?;
        }
        Ok(())
    }
    fn value(&mut self) -> Result<(), Error> {
        self.ws()?;
        if self.depth >= 128 {
            return Err(self.error("JSON depth limit (128)"));
        }
        let start = self.offset;
        let kind = match self.peek()? {
            Some(b'{') => "object",
            Some(b'[') => "array",
            Some(b'"') => "string",
            Some(b't' | b'f') => "boolean",
            Some(b'n') => "null",
            _ => "number",
        };
        let node = if self.depth <= 1 && self.nodes.len() < 256 {
            self.nodes.push(Node {
                start,
                end: start,
                kind,
            });
            Some(self.nodes.len() - 1)
        } else {
            None
        };
        match self.peek()? {
            Some(b'{') | Some(b'[') => {
                let open = self.copy()?;
                let close = if open == b'{' { b'}' } else { b']' };
                self.depth += 1;
                self.ws()?;
                if self.peek()? != Some(close) {
                    loop {
                        self.indent()?;
                        if open == b'{' {
                            self.string()?;
                            self.ws()?;
                            self.expect(b':')?;
                            if matches!(self.layout, Layout::Pretty) {
                                self.emit(b" ")?;
                            }
                        }
                        self.value()?;
                        self.ws()?;
                        if self.peek()? != Some(b',') {
                            break;
                        }
                        self.copy()?;
                        self.ws()?;
                    }
                    self.depth -= 1;
                    self.indent()?;
                } else {
                    self.depth -= 1;
                }
                self.expect(close)?;
            }
            Some(b'"') => self.string()?,
            Some(b't') | Some(b'f') | Some(b'n') => {
                let token: &[u8] = match self.peek()? {
                    Some(b't') => b"true",
                    Some(b'f') => b"false",
                    _ => b"null",
                };
                for &b in token {
                    self.expect(b)?;
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number()?,
            _ => return Err(self.error("expected JSON value")),
        }
        if let Some(n) = node {
            self.nodes[n].end = self.offset;
        }
        Ok(())
    }
}
/// Input and output stream in bounded buffers; scalar tokens are never allocated.
/// Tree index retains only the root and first 255 immediate children.
pub fn process(input: impl Read, output: impl Write, layout: Layout) -> Result<Vec<Node>, Error> {
    let mut p = Parser {
        input: BufReader::with_capacity(65536, input),
        out: output,
        layout,
        offset: 0,
        line: 1,
        depth: 0,
        nodes: Vec::new(),
    };
    p.value()?;
    p.ws()?;
    if p.peek()?.is_some() {
        return Err(p.error("trailing JSON data"));
    }
    Ok(p.nodes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_fidelity() {
        let source = r#"{"n":123456789012345678901234567890,"e":1.2300E+009,"s":"é\\n"}"#;
        let mut pretty = Vec::new();
        process(source.as_bytes(), &mut pretty, Layout::Pretty).unwrap();
        let mut compact = Vec::new();
        process(&pretty[..], &mut compact, Layout::Minify).unwrap();
        assert_eq!(compact, source.as_bytes());
    }
    #[test]
    fn malformed_positions_and_limits() {
        let e = process(b"[1,\n!]".as_slice(), std::io::sink(), Layout::Validate).unwrap_err();
        assert_eq!((e.offset, e.line), (4, 2));
        for source in ["01", "[1,]", "{\"a\":true false}", "\"\\uD800\"", "\"\\q\""] {
            assert!(
                process(source.as_bytes(), std::io::sink(), Layout::Validate).is_err(),
                "{source}"
            );
        }
        assert!(
            process(
                "[".repeat(129).as_bytes(),
                std::io::sink(),
                Layout::Validate
            )
            .is_err()
        );
    }
    #[test]
    fn tree_is_bounded() {
        let input = format!("[{}]", vec!["0"; 10000].join(","));
        let nodes = process(input.as_bytes(), std::io::sink(), Layout::Validate).unwrap();
        assert_eq!(nodes.len(), 256);
        assert_eq!(nodes[0].end, input.len() as u64);
    }
    #[test]
    fn read_failure_is_reported() {
        struct Bad;
        impl Read for Bad {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("fixture read failure"))
            }
        }
        assert!(
            process(Bad, std::io::sink(), Layout::Validate)
                .unwrap_err()
                .message
                .contains("fixture")
        );
    }

    struct ShortReads<'a> {
        bytes: &'a [u8],
        limit: usize,
    }
    impl Read for ShortReads<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let count = out.len().min(self.limit).min(self.bytes.len());
            out[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }

    #[test]
    fn ascii_spans_preserve_lexemes_across_special_byte_boundaries() {
        let source = format!(
            "\"{}\\\"\\\\\\uD83D\\uDE00é🦀\u{7f}tail\"",
            "a".repeat(65534)
        );
        for limit in [1, 2, 3, 7, 65536] {
            for layout in [Layout::Validate, Layout::Minify, Layout::Pretty] {
                let mut output = Vec::new();
                let nodes = process(
                    ShortReads {
                        bytes: source.as_bytes(),
                        limit,
                    },
                    &mut output,
                    layout,
                )
                .unwrap();
                assert_eq!((nodes[0].start, nodes[0].end), (0, source.len() as u64));
                if matches!(layout, Layout::Validate) {
                    assert!(output.is_empty());
                } else {
                    assert_eq!(output, source.as_bytes());
                }
            }
        }
    }

    #[test]
    fn ascii_spans_preserve_invalid_byte_offsets_and_lines() {
        let prefix = format!("[\n\"{}", "a".repeat(65533));
        for (tail, advance, message) in [
            (b"\n\"]".as_slice(), 0, "unescaped control character"),
            (b"\\q\"]".as_slice(), 2, "invalid string escape"),
            (b"\xff\"]".as_slice(), 0, "invalid UTF-8"),
            (b"\xc3x\"]".as_slice(), 2, "invalid UTF-8"),
            (b"".as_slice(), 0, "unterminated string"),
        ] {
            let mut source = prefix.as_bytes().to_vec();
            source.extend_from_slice(tail);
            for limit in [1, 7, 65536] {
                let error = process(
                    ShortReads {
                        bytes: &source,
                        limit,
                    },
                    std::io::sink(),
                    Layout::Validate,
                )
                .unwrap_err();
                assert_eq!(error.offset, prefix.len() as u64 + advance);
                assert_eq!(error.line, 2);
                assert_eq!(error.message, message);
            }
        }
    }
}

/// Runs through the exact capability-scoped broker used by the component host.
pub fn run<T: bareline_first_party_common::Transport>(
    client: std::rc::Rc<std::cell::RefCell<bareline_first_party_common::Client<T>>>,
) -> Result<(), String> {
    use bareline_first_party_common::{Counter, Snapshot, Staged};
    let command = client.borrow().invocation.command.clone();
    match command.as_str() {
        "ext.json.format" | "ext.json.minify" => {
            let layout = if command.ends_with("minify") {
                Layout::Minify
            } else {
                Layout::Pretty
            };
            let mut count = Counter::default();
            process(Snapshot::new(client.clone()), &mut count, layout)
                .map_err(|e| format!("TextOffset {}, line {}: {}", e.offset, e.line, e.message))?;
            let mut stage = Staged::new(client.clone(), count.0)?;
            process(Snapshot::new(client.clone()), &mut stage, layout).map_err(|e| e.message)?;
            stage.finish()
        }
        "ext.json.validate" => {
            process(
                Snapshot::new(client.clone()),
                std::io::sink(),
                Layout::Validate,
            )
            .map_err(|e| format!("TextOffset {}, line {}: {}", e.offset, e.line, e.message))?;
            client
                .borrow_mut()
                .panel("ext.json.tree", "Valid JSON".into())
        }
        "ext.json.tree" => tree::run(client.clone()),
        _ => Err("unsupported JSON command".into()),
    }
}
#[cfg(target_arch = "wasm32")]
struct Component;
#[cfg(target_arch = "wasm32")]
impl bareline_extension_sdk::bindings::Guest for Component {
    fn run() {
        if let Ok(client) = bareline_first_party_common::wasm_client() {
            if let Err(error) = run(client.clone()) {
                let _ = client.borrow_mut().panel("ext.json.tree", error);
            }
        }
    }
}
#[cfg(target_arch = "wasm32")]
bareline_extension_sdk::bindings::export!(Component with_types_in bareline_extension_sdk::bindings);
