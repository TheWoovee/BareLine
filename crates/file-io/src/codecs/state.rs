// SPDX-License-Identifier: MPL-2.0
//! Encoding and newline policy are independent from the document's UTF-8 text.
use super::{Confidence, Detection, Encoding};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EncodingState {
    pub binary_warning: bool,
    pub detected: Encoding,
    pub confidence: Confidence,
    pub bom: bool,
    pub user_override: Option<Encoding>,
    pub save_target: Encoding,
    pub had_decode_errors: bool,
    pub invalid_span_count: u64,
    pub invalid_byte_count: u64,
}
impl EncodingState {
    pub fn new(detection: Detection) -> Self {
        Self {
            binary_warning: detection.binary_warning,
            detected: detection.encoding,
            confidence: detection.confidence,
            bom: detection.bom,
            user_override: None,
            save_target: detection.encoding,
            had_decode_errors: false,
            invalid_span_count: 0,
            invalid_byte_count: 0,
        }
    }
    pub fn interpreted(&self) -> Encoding {
        self.user_override.unwrap_or(self.detected)
    }
    /// Returns the previous policy for the command layer's metadata undo record.
    /// This never changes decoded text or silently resolves opaque bytes.
    pub fn convert_to(&mut self, target: Encoding) -> Encoding {
        let previous = self.save_target;
        self.save_target = target;
        previous
    }
    pub fn record_invalid(&mut self, bytes: usize) {
        self.had_decode_errors = true;
        self.invalid_span_count += 1;
        self.invalid_byte_count += bytes as u64;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Lf,
    CrLf,
    Cr,
}
impl Eol {
    pub fn text(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
            Self::Cr => "\r",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EolState {
    pub lf: u64,
    pub crlf: u64,
    pub cr: u64,
    pending_cr: bool,
}
impl EolState {
    /// Stream decoded text, retaining a CR across chunk boundaries. Open preserves bytes.
    pub fn push(&mut self, text: &str, end: bool) {
        for c in text.chars() {
            if self.pending_cr {
                self.pending_cr = false;
                if c == '\n' {
                    self.crlf += 1;
                    continue;
                }
                self.cr += 1;
            }
            match c {
                '\r' => self.pending_cr = true,
                '\n' => self.lf += 1,
                _ => {}
            }
        }
        if end && self.pending_cr {
            self.cr += 1;
            self.pending_cr = false;
        }
    }
    pub fn mixed(self) -> bool {
        u8::from(self.lf > 0) + u8::from(self.crlf > 0) + u8::from(self.cr > 0) > 1
    }
    pub fn dominant(self) -> Eol {
        if self.crlf > self.lf && self.crlf >= self.cr {
            Eol::CrLf
        } else if self.cr > self.lf {
            Eol::Cr
        } else {
            Eol::Lf
        }
    }
    pub fn label(self) -> &'static str {
        if self.mixed() {
            "Mixed"
        } else {
            match self.dominant() {
                Eol::Lf => "LF",
                Eol::CrLf => "CRLF",
                Eol::Cr => "CR",
            }
        }
    }
}

/// Plan a normal undoable text transaction. The explicit edit cap bounds temporary
/// planning memory; exhaustion returns no transaction and never partly edits text.
pub fn plan_eol_conversion(
    snapshot: &bareline_document::DocumentSnapshot,
    range: std::ops::Range<bareline_document::TextOffset>,
    target: Eol,
    max_edits: usize,
) -> Result<bareline_document::EditTransaction, bareline_document::Error> {
    use bareline_document::{Edit, EditTransaction, Error, TextOffset};
    if !snapshot.is_complete() {
        return Err(Error::IncompleteSource);
    }
    let mut edits = Vec::new();
    let mut pending = None;
    let mut offset = range.start.0;
    let mut terminator = |start: usize, len: usize, current: Eol| -> Result<(), Error> {
        if current != target {
            if edits.len() >= max_edits {
                return Err(Error::BudgetExceeded);
            }
            edits.push(Edit {
                range: TextOffset(start)..TextOffset(start + len),
                insert: target.text().into(),
            });
        }
        Ok(())
    };
    for chunk in snapshot.chunks(range)? {
        for b in chunk.bytes() {
            if let Some(cr) = pending.take() {
                if b == b'\n' {
                    terminator(cr, 2, Eol::CrLf)?;
                    offset += 1;
                    continue;
                }
                terminator(cr, 1, Eol::Cr)?;
            }
            match b {
                b'\r' => pending = Some(offset),
                b'\n' => terminator(offset, 1, Eol::Lf)?,
                _ => {}
            }
            offset += 1;
        }
    }
    if let Some(cr) = pending {
        terminator(cr, 1, Eol::Cr)?;
    }
    Ok(EditTransaction {
        base_revision: snapshot.revision,
        edits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_newlines_span_chunk_boundaries() {
        let mut s = EolState::default();
        s.push("a\r", false);
        s.push("\nb\rc\n", false);
        s.push("d\r", true);
        assert_eq!((s.lf, s.crlf, s.cr), (1, 1, 2));
        assert!(s.mixed());
        assert_eq!(s.dominant(), Eol::Cr);
        assert_eq!(s.label(), "Mixed");
    }
    #[test]
    fn conversion_only_changes_save_target() {
        let mut s = EncodingState::new(super::super::detect(b"hello"));
        s.record_invalid(2);
        let original = s.clone();
        assert_eq!(s.convert_to(Encoding::Utf16Le), Encoding::Utf8);
        assert_eq!(s.interpreted(), Encoding::Utf8);
        assert_eq!(s.invalid_byte_count, 2);
        s.convert_to(Encoding::Utf8);
        assert_eq!(s, original);
    }
    #[test]
    fn eol_conversion_is_one_undoable_transaction_and_quota_refuses() {
        use bareline_document::{Budget, Document, TextOffset};
        let mut d =
            Document::from_utf8("é\r\na\rb\nc", Budget::new(10000), Budget::new(10000)).unwrap();
        let s = d.snapshot();
        let range = TextOffset(0)..TextOffset(s.len());
        assert!(plan_eol_conversion(&s, range.clone(), Eol::Lf, 1).is_err());
        let edit = plan_eol_conversion(&s, range, Eol::Lf, 10).unwrap();
        assert_eq!(edit.edits.len(), 2);
        d.apply(edit).unwrap();
        assert_eq!(
            d.snapshot()
                .read(TextOffset(0)..TextOffset(d.snapshot().len()), 100)
                .unwrap(),
            "é\na\nb\nc"
        );
        d.undo().unwrap();
        assert_eq!(
            d.snapshot()
                .read(TextOffset(0)..TextOffset(d.snapshot().len()), 100)
                .unwrap(),
            "é\r\na\rb\nc"
        );
    }
}
