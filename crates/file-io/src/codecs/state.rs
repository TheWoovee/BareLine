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
    /// Detection's likeliest alternatives when it fell back (FIO-05).
    #[serde(default)]
    pub candidates: [Option<Encoding>; 3],
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
            candidates: detection.candidates,
        }
    }
    /// Alternatives worth offering: detection fell back on an ambiguous sample and
    /// the user has not chosen an interpretation yet.
    pub fn uncertain_candidates(&self) -> impl Iterator<Item = Encoding> + '_ {
        let open = self.user_override.is_none() && self.confidence == Confidence::LegacyFallback;
        self.candidates.iter().flatten().copied().filter(move |_| open)
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

/// Append `text` to `out` with every terminator rewritten to `target`. A trailing CR is
/// carried across chunks in `pending_cr`; `end` flushes it as a lone CR terminator.
pub fn convert_eol(text: &str, target: Eol, pending_cr: &mut bool, end: bool, out: &mut String) {
    let mut plain = 0;
    for (index, byte) in text.bytes().enumerate() {
        if std::mem::take(pending_cr) {
            out.push_str(target.text());
            if byte == b'\n' {
                plain = index + 1;
                continue;
            }
        }
        if matches!(byte, b'\r' | b'\n') {
            out.push_str(&text[plain..index]);
            plain = index + 1;
            if byte == b'\r' {
                *pending_cr = true;
            } else {
                out.push_str(target.text());
            }
        }
    }
    out.push_str(&text[plain..]);
    if end && std::mem::take(pending_cr) {
        out.push_str(target.text());
    }
}

/// Plan a normal undoable text transaction. Up to `max_edits` changed terminators
/// are separate edits; past that cap the changed span becomes one coalesced edit, so
/// any number of line endings converts and planning memory stays bounded by the span.
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
    // Validates the requested range before it is widened below.
    let _ = snapshot.chunks(range.clone())?;
    // FIO-16: an edge inside a CRLF must convert the whole pair, never yield `\r\r\n`
    // or `\n\n`. Scan one byte beyond each edge (a non-boundary neighbor is part of a
    // multibyte scalar, so never CR or LF) and keep terminators overlapping the range.
    let requested = range.start.0..range.end.0;
    let scan_start = if requested.start > 0 && snapshot.is_boundary(TextOffset(requested.start - 1)) {
        requested.start - 1
    } else {
        requested.start
    };
    let scan_end = if requested.end < snapshot.len() && snapshot.is_boundary(TextOffset(requested.end + 1)) {
        requested.end + 1
    } else {
        requested.end
    };
    let mut edits = Vec::new();
    let mut changed: Option<std::ops::Range<usize>> = None;
    let mut coalesce = false;
    let mut pending = None;
    let mut offset = scan_start;
    let mut terminator = |start: usize, len: usize, current: Eol| {
        if current != target && !requested.is_empty() && start < requested.end && start + len > requested.start {
            changed.get_or_insert(start..start).end = start + len;
            if !coalesce && edits.len() >= max_edits {
                coalesce = true;
                edits = Vec::new();
            }
            if !coalesce {
                edits.push(Edit {
                    range: TextOffset(start)..TextOffset(start + len),
                    insert: target.text().into(),
                });
            }
        }
    };
    for chunk in snapshot.chunks(TextOffset(scan_start)..TextOffset(scan_end))? {
        for b in chunk.bytes() {
            if let Some(cr) = pending.take() {
                if b == b'\n' {
                    terminator(cr, 2, Eol::CrLf);
                    offset += 1;
                    continue;
                }
                terminator(cr, 1, Eol::Cr);
            }
            match b {
                b'\r' => pending = Some(offset),
                b'\n' => terminator(offset, 1, Eol::Lf),
                _ => {}
            }
            offset += 1;
        }
    }
    if let Some(cr) = pending {
        terminator(cr, 1, Eol::Cr);
    }
    if let Some(span) = changed.filter(|_| coalesce) {
        let mut insert = String::with_capacity(span.end - span.start);
        let mut cr = false;
        for chunk in snapshot.chunks(TextOffset(span.start)..TextOffset(span.end))? {
            convert_eol(&chunk, target, &mut cr, false, &mut insert);
        }
        convert_eol("", target, &mut cr, true, &mut insert);
        edits.push(Edit {
            range: TextOffset(span.start)..TextOffset(span.end),
            insert,
        });
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
    fn ambiguity_candidates_persist_and_yield_to_an_interpretation() {
        let mut s = EncodingState::new(Detection {
            encoding: Encoding::Windows1252,
            confidence: Confidence::LegacyFallback,
            bom: false,
            binary_warning: false,
            candidates: [Some(Encoding::Gbk), Some(Encoding::Big5), None],
        });
        assert_eq!(
            s.uncertain_candidates().collect::<Vec<_>>(),
            [Encoding::Gbk, Encoding::Big5]
        );
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<EncodingState>(&json).unwrap(), s);
        // Metadata written before candidates existed still reads.
        let older = json.replace(r#","candidates":["Gbk","Big5",null]"#, "");
        assert_ne!(older, json);
        assert_eq!(
            serde_json::from_str::<EncodingState>(&older).unwrap().candidates,
            [None; 3]
        );
        s.user_override = Some(Encoding::Gbk);
        assert_eq!(s.uncertain_candidates().count(), 0);
        assert_eq!(super::super::detect(b"hello").candidates, [None; 3]);
    }
    #[test]
    fn selection_edges_inside_crlf_convert_the_whole_pair() {
        use bareline_document::{Budget, Document, TextOffset};
        // Bytes: a0 \r1 \n2 b3 \r4 \n5 c6 \r7 d8; the range 2..5 splits both CRLFs.
        for (target, expected) in [
            (Eol::Lf, "a\nb\nc\rd"),
            (Eol::Cr, "a\rb\rc\rd"),
            (Eol::CrLf, "a\r\nb\r\nc\rd"),
        ] {
            let mut d = Document::from_utf8("a\r\nb\r\nc\rd", Budget::new(10000), Budget::new(10000)).unwrap();
            let s = d.snapshot();
            d.apply(plan_eol_conversion(&s, TextOffset(2)..TextOffset(5), target, 10).unwrap())
                .unwrap();
            let s = d.snapshot();
            assert_eq!(s.read(TextOffset(0)..TextOffset(s.len()), 100).unwrap(), expected);
        }
        // Whole terminators just outside the range stay unchanged.
        let d = Document::from_utf8("é\r\nx\né", Budget::new(10000), Budget::new(10000)).unwrap();
        let s = d.snapshot();
        let edit = plan_eol_conversion(&s, TextOffset(4)..TextOffset(5), Eol::CrLf, 10).unwrap();
        assert!(edit.edits.is_empty());
    }
    #[test]
    fn coalesced_conversion_keeps_split_crlf_pairs_whole() {
        use bareline_document::{Budget, Document, TextOffset};
        // Past the edit cap the coalesced span also starts and ends on whole pairs.
        let mut d = Document::from_utf8("a\r\nb\r\nc\rd", Budget::new(10000), Budget::new(10000)).unwrap();
        let s = d.snapshot();
        let edit = plan_eol_conversion(&s, TextOffset(2)..TextOffset(5), Eol::Lf, 1).unwrap();
        assert_eq!(edit.edits.len(), 1);
        assert_eq!(edit.edits[0].range, TextOffset(1)..TextOffset(6));
        assert_eq!(edit.edits[0].insert, "\nb\n");
        d.apply(edit).unwrap();
        let s = d.snapshot();
        assert_eq!(s.read(TextOffset(0)..TextOffset(s.len()), 100).unwrap(), "a\nb\nc\rd");
    }
    #[test]
    fn eol_conversion_is_one_undoable_transaction_and_cap_coalesces() {
        use bareline_document::{Budget, Document, TextOffset};
        let mut d = Document::from_utf8("é\r\na\rb\nc", Budget::new(10000), Budget::new(10000)).unwrap();
        let s = d.snapshot();
        let range = TextOffset(0)..TextOffset(s.len());
        let coalesced = plan_eol_conversion(&s, range.clone(), Eol::Lf, 1).unwrap();
        assert_eq!(coalesced.edits.len(), 1);
        assert_eq!(coalesced.edits[0].range, TextOffset(2)..TextOffset(6));
        assert_eq!(coalesced.edits[0].insert, "\na\n");
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
    #[test]
    fn eol_conversion_past_the_edit_cap_converts_300k_crlf_lines() {
        use bareline_document::{Budget, Document, TextOffset};
        let text = "x\r\n".repeat(300_000);
        let mut d = Document::from_utf8(&text, Budget::new(64 << 20), Budget::new(64 << 20)).unwrap();
        let s = d.snapshot();
        let edit = plan_eol_conversion(&s, TextOffset(0)..TextOffset(s.len()), Eol::Lf, 131_072).unwrap();
        assert_eq!(edit.edits.len(), 1);
        d.apply(edit).unwrap();
        let after = d.snapshot();
        assert_eq!(
            after.read(TextOffset(0)..TextOffset(after.len()), after.len()).unwrap(),
            "x\n".repeat(300_000)
        );
        d.undo().unwrap();
        let undone = d.snapshot();
        assert_eq!(
            undone
                .read(TextOffset(0)..TextOffset(undone.len()), undone.len())
                .unwrap(),
            text
        );
    }
    #[test]
    fn streamed_conversion_carries_cr_across_chunks() {
        let mut out = String::new();
        let mut cr = false;
        for chunk in ["é\r", "\na\r", "b\n", "c\r"] {
            convert_eol(chunk, Eol::CrLf, &mut cr, false, &mut out);
        }
        convert_eol("", Eol::CrLf, &mut cr, true, &mut out);
        assert_eq!(out, "é\r\na\r\nb\r\nc\r\n");
    }
}

pub fn metadata_encoding(metadata: &bareline_document::DocumentMetadata) -> Option<EncodingState> {
    metadata
        .get("file.encoding")
        .and_then(|value| serde_json::from_str(value).ok())
}
pub fn with_encoding(
    metadata: &bareline_document::DocumentMetadata,
    state: &EncodingState,
) -> Result<bareline_document::DocumentMetadata, bareline_document::Error> {
    let mut values = metadata.values().clone();
    values.insert(
        "file.encoding".into(),
        serde_json::to_string(state).map_err(|_| bareline_document::Error::BudgetExceeded)?,
    );
    bareline_document::DocumentMetadata::new(values)
}
