// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded structural pages; full grammar validation remains a separate command.
use bareline_first_party_common::{Client, Snapshot, Transport, numeric_arguments};
use std::{cell::RefCell, io::Read, rc::Rc};
const PAGE_BYTES: u64 = 65536;
const PAGE_NODES: usize = 256;
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub start: u64,
    pub end: u64,
    pub offset: u64,
    pub depth: u64,
    pub mode: u64,
    pub escaped: bool,
}
impl Cursor {
    pub fn parse(arguments: &str, length: u64) -> Result<Self, String> {
        let values = numeric_arguments(arguments, &["start", "end", "cursor", "depth", "mode", "escaped"])?;
        let start = *values.get("start").unwrap_or(&0);
        let end = *values.get("end").unwrap_or(&length);
        let offset = *values.get("cursor").unwrap_or(&start);
        let depth = *values.get("depth").unwrap_or(&0);
        let mode = *values.get("mode").unwrap_or(&0);
        let escaped = *values.get("escaped").unwrap_or(&0);
        if start > end
            || end > length
            || offset < start
            || offset > end
            || depth > 128
            || mode > 2
            || escaped > 1
            || (escaped == 1 && mode != 2)
        {
            return Err("invalid JSON tree cursor".into());
        }
        Ok(Self {
            start,
            end,
            offset,
            depth,
            mode,
            escaped: escaped == 1,
        })
    }
    fn arguments(&self) -> String {
        format!(
            "start={}\nend={}\ncursor={}\ndepth={}\nmode={}\nescaped={}",
            self.start,
            self.end,
            self.offset,
            self.depth,
            self.mode,
            u8::from(self.escaped)
        )
    }
}
pub fn page(mut input: impl Read, mut cursor: Cursor) -> Result<String, String> {
    let mut result = String::from(
        "JSON structural tree — bounded page\nOffsets address this document revision. Use Validate for full grammar validation.\n",
    );
    let mut byte = [0];
    let mut consumed = 0;
    let mut nodes = 0;
    let mut finished = false;
    while cursor.offset < cursor.end && consumed < PAGE_BYTES && nodes < PAGE_NODES {
        if input.read(&mut byte).map_err(|e| e.to_string())? == 0 {
            return Err("tree range ended early".into());
        }
        let b = byte[0];
        let at = cursor.offset;
        cursor.offset += 1;
        consumed += 1;
        if cursor.mode == 2 {
            if cursor.escaped {
                cursor.escaped = false;
            } else if b == b'\\' {
                cursor.escaped = true;
            } else if b == b'"' {
                cursor.mode = 0;
            }
            continue;
        }
        if cursor.mode == 1 {
            if !matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}') {
                continue;
            }
            cursor.mode = 0;
        }
        let kind = match b {
            b'{' => "object",
            b'[' => "array",
            b'"' => "string/key",
            b'-' | b'0'..=b'9' => "number",
            b't' | b'f' => "boolean",
            b'n' => "null",
            b'}' | b']' => {
                if cursor.depth == 0 {
                    return Err("unbalanced JSON tree range".into());
                }
                cursor.depth -= 1;
                if cursor.depth == 0 {
                    finished = true;
                    break;
                }
                continue;
            }
            b' ' | b'\t' | b'\r' | b'\n' | b',' | b':' => continue,
            _ => {
                return Err(format!("unexpected JSON byte at TextOffset {at}; run Validate"));
            }
        };
        use std::fmt::Write;
        writeln!(
            result,
            "{}{} at TextOffset {}",
            "  ".repeat(cursor.depth.min(16) as usize),
            kind,
            at
        )
        .unwrap();
        nodes += 1;
        if matches!(b, b'{' | b'[') {
            writeln!(result, "  Expand arguments: start={at}  (one argument per line)").unwrap();
            cursor.depth += 1;
            if cursor.depth > 128 {
                return Err("JSON depth limit (128)".into());
            }
        } else if b == b'"' {
            cursor.mode = 2;
        } else {
            cursor.mode = 1;
        }
    }
    if !finished && cursor.offset < cursor.end {
        result.push_str("\nNext page: copy these arguments and run ext.json.tree:\n");
        result.push_str(&cursor.arguments());
    } else {
        result.push_str("\nEnd of selected structural range.");
    }
    Ok(result)
}
pub fn run<T: Transport>(client: Rc<RefCell<Client<T>>>) -> Result<(), String> {
    let invocation = client.borrow().invocation.clone();
    let cursor = Cursor::parse(&invocation.arguments, invocation.text_length)?;
    let source = Snapshot::range(client.clone(), cursor.offset, cursor.end)?;
    let text = page(source, cursor)?;
    client.borrow_mut().panel("ext.json.tree", text)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pages_bound_reads_and_expose_real_nodes() {
        let source = format!("[{}]", vec!["0"; 10000].join(","));
        let cursor = Cursor::parse("", source.len() as u64).unwrap();
        let text = page(source.as_bytes(), cursor).unwrap();
        assert!(text.contains("array at TextOffset 0"));
        assert!(text.contains("number at TextOffset 1"));
        assert!(text.contains("Next page"));
        assert!(text.len() < 30000);
    }
    #[test]
    fn continuation_inside_large_string_is_explicit() {
        let source = format!("[\"{}\",2]", "a".repeat(70000));
        let cursor = Cursor::parse("", source.len() as u64).unwrap();
        let text = page(source.as_bytes(), cursor).unwrap();
        let args = text.split("run ext.json.tree:\n").nth(1).unwrap();
        let next = Cursor::parse(args, source.len() as u64).unwrap();
        assert_eq!(next.mode, 2);
        let offset = next.offset as usize;
        assert!(
            page(&source.as_bytes()[offset..], next)
                .unwrap()
                .contains("number at TextOffset 70004")
        );
    }
    #[test]
    fn malformed_cursor_rejected() {
        for args in ["depth=129", "mode=3", "cursor=11", "cursor=1\ncursor=2", "wat=1"] {
            assert!(Cursor::parse(args, 10).is_err());
        }
    }
}
