// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only original byte panes. Raw offsets never address decoded text.
use bareline_extension_sdk::{RawRange, Request};
pub const ROW_BYTES: u64 = 16;
pub const MAX_ROWS: u64 = 256;
#[derive(Debug, PartialEq, Eq)]
pub struct Viewport {
    pub document: u64,
    pub generation: u64,
    pub original_length: u64,
    pub offset: u64,
    pub rows: u64,
}
impl Viewport {
    pub fn goto(&mut self, offset: u64) -> Result<(), &'static str> {
        if offset > self.original_length {
            return Err("raw offset is beyond original file bytes");
        }
        self.offset = offset / ROW_BYTES * ROW_BYTES;
        Ok(())
    }
    pub fn request(&self) -> Result<Request, &'static str> {
        if self.rows == 0 || self.rows > MAX_ROWS || self.offset > self.original_length {
            return Err("invalid Hex viewport");
        }
        let start = self.offset.saturating_sub(ROW_BYTES * 2);
        let end = self
            .offset
            .saturating_add((self.rows + 2) * ROW_BYTES)
            .min(self.original_length);
        Ok(Request::ReadOriginalBytes {
            document: self.document,
            generation: self.generation,
            range: RawRange { start, end },
        })
    }
    /// A missing original range is rendered as gaps, never substituted with edits.
    pub fn render(&self, range_start: u64, bytes: Option<&[u8]>) -> Result<String, &'static str> {
        let Request::ReadOriginalBytes { range, .. } = self.request()? else {
            unreachable!()
        };
        if range_start != range.start
            || bytes.is_some_and(|b| b.len() as u64 != range.end - range.start)
        {
            return Err("original byte response range mismatch");
        }
        let mut text = format!(
            "Original file bytes — disk generation {}\nUnsaved text edits are excluded. Read only.\nRaw offset       Bytes                                            Text\n",
            self.generation
        );
        use std::fmt::Write;
        let end = self
            .offset
            .saturating_add(self.rows * ROW_BYTES)
            .min(self.original_length);
        for offset in (self.offset..end).step_by(ROW_BYTES as usize) {
            write!(text, "{offset:016X}  ").unwrap();
            let mut ascii = String::new();
            for col in 0..ROW_BYTES {
                if offset + col >= self.original_length {
                    text.push_str("   ");
                    continue;
                }
                let byte = bytes
                    .and_then(|b| b.get((offset + col - range_start) as usize))
                    .copied();
                match byte {
                    Some(b) => {
                        write!(text, "{b:02X} ").unwrap();
                        ascii.push(if b.is_ascii_graphic() || b == b' ' {
                            b as char
                        } else {
                            '.'
                        });
                    }
                    None => {
                        text.push_str("?? ");
                        ascii.push('?');
                    }
                }
            }
            writeln!(text, " {ascii}").unwrap();
        }
        Ok(text)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn five_gb_view_is_bounded_and_goto_constant_work() {
        let mut view = Viewport {
            document: 1,
            generation: 7,
            original_length: 5 * 1024 * 1024 * 1024,
            offset: 0,
            rows: 32,
        };
        view.goto(4 * 1024 * 1024 * 1024 + 3).unwrap();
        let Request::ReadOriginalBytes {
            range, generation, ..
        } = view.request().unwrap()
        else {
            panic!()
        };
        assert_eq!(generation, 7);
        assert!(range.end - range.start <= 36 * 16);
        let text = view
            .render(
                range.start,
                Some(&vec![0xff; (range.end - range.start) as usize]),
            )
            .unwrap();
        assert!(text.contains("disk generation 7"));
        assert!(text.contains("Unsaved text edits are excluded"));
        assert!(view.goto(u64::MAX).is_err());
    }
    #[test]
    fn gaps_and_utf16_are_original_bytes() {
        let view = Viewport {
            document: 9,
            generation: 42,
            original_length: 4,
            offset: 0,
            rows: 1,
        };
        assert!(
            view.render(0, Some(&[0xff, 0xfe, 0x41, 0]))
                .unwrap()
                .contains("FF FE 41 00")
        );
        assert!(view.render(0, None).unwrap().contains("?? ?? ?? ??"));
        assert!(view.render(0, Some(&[0])).is_err());
    }
}

pub fn run<T: bareline_first_party_common::Transport>(
    client: std::rc::Rc<std::cell::RefCell<bareline_first_party_common::Client<T>>>,
) -> Result<(), String> {
    let invocation = client.borrow().invocation.clone();
    if !matches!(invocation.command.as_str(), "ext.hex.open" | "ext.hex.goto") {
        return Err("unsupported Hex command".into());
    }
    let mut view = Viewport {
        document: invocation.document,
        generation: invocation.source_generation,
        original_length: invocation.raw_length,
        offset: 0,
        rows: 32,
    };
    let args = if !invocation.arguments.trim().is_empty() && !invocation.arguments.contains('=') {
        format!("offset={}", invocation.arguments.trim())
    } else {
        invocation.arguments.clone()
    };
    let values = bareline_first_party_common::numeric_arguments(&args, &["offset", "rows"])?;
    if let Some(rows) = values.get("rows") {
        if *rows == 0 || *rows > MAX_ROWS {
            return Err("rows must be between 1 and 256".into());
        }
        view.rows = *rows;
    }
    if let Some(offset) = values.get("offset") {
        view.goto(*offset)?;
    }
    let request = view.request()?;
    let Request::ReadOriginalBytes { ref range, .. } = request else {
        unreachable!()
    };
    let start = range.start;
    let response = client.borrow_mut().call(request);
    let text = match response {
        Ok(bareline_extension_sdk::BrokerValue::Bytes(bytes)) => {
            view.render(start, Some(&bytes))?
        }
        Err(error) => format!(
            "{}\nOriginal range unavailable: {error}",
            view.render(start, None)?
        ),
        _ => return Err("unexpected original-byte response".into()),
    };
    client.borrow_mut().panel("ext.hex.view", text)
}
#[cfg(target_arch = "wasm32")]
struct Component;
#[cfg(target_arch = "wasm32")]
impl bareline_extension_sdk::bindings::Guest for Component {
    fn run() {
        if let Ok(client) = bareline_first_party_common::wasm_client() {
            if let Err(error) = run(client.clone()) {
                let _ = client.borrow_mut().panel("ext.hex.view", error);
            }
        }
    }
}
#[cfg(target_arch = "wasm32")]
bareline_extension_sdk::bindings::export!(Component with_types_in bareline_extension_sdk::bindings);
