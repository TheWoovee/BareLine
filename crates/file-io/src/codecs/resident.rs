// SPDX-License-Identifier: MPL-2.0
//! Explicitly caller-bounded Resident provenance. Retained baseline segments make
//! address-range identity valid even after piece splits, edits and undo.
use super::{
    Decoder, Encoder, Encoding, detect,
    state::{EncodingState, EolState},
};
use crate::{CodecError, DecodedSink, DecodedSpan, StreamingDecoder};
use bareline_document::{Budget, BudgetClaim, Document, DocumentBuilder, DocumentSnapshot, TextOffset};
use std::{io::Write, ops::Range, sync::Arc};

#[derive(Clone, Debug)]
struct Mapping {
    text: Range<usize>,
    raw: Range<usize>,
    opaque: bool,
    text_unit: usize,
    raw_unit: usize,
}
/// Text of each opaque unit under UTF-8 identity provenance: one U+FFFD.
const REPLACEMENT_LEN: usize = '\u{fffd}'.len_utf8();

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Edit, EditTransaction};
    fn open(raw: Vec<u8>, e: Encoding) -> (Document, ResidentEncoding) {
        ResidentEncoding::open(
            raw,
            Some(e),
            Budget::new(4 * 1024 * 1024),
            Budget::new(4 * 1024 * 1024),
            1024 * 1024,
            64 * 1024 * 1024,
        )
        .unwrap()
    }
    fn save(d: &Document, p: &ResidentEncoding, e: Encoding) -> Result<Vec<u8>, ResidentError> {
        let mut out = vec![];
        p.write_snapshot(&d.snapshot(), e, p.state.bom, &mut out)?;
        Ok(out)
    }
    #[test]
    fn failures_use_current_text_offsets_after_prefix_edits() {
        let (mut document, codec) = open(vec![b'A', 255, b'B'], Encoding::Utf8);
        document
            .apply(EditTransaction {
                base_revision: document.snapshot().revision,
                edits: vec![Edit {
                    range: TextOffset(0)..TextOffset(0),
                    insert: "prefix".into(),
                }],
            })
            .unwrap();
        let Err(ResidentError::At { range, .. }) = save(&document, &codec, Encoding::Utf16Le) else {
            panic!("expected opaque range")
        };
        assert_eq!(range, 7..10);
        let (mut document, codec) = open(b"A".to_vec(), Encoding::Utf8);
        document
            .apply(EditTransaction {
                base_revision: document.snapshot().revision,
                edits: vec![Edit {
                    range: TextOffset(1)..TextOffset(1),
                    insert: "x😀z".into(),
                }],
            })
            .unwrap();
        let Err(ResidentError::At { range, .. }) = save(&document, &codec, Encoding::Latin1) else {
            panic!("expected scalar range")
        };
        assert_eq!(range, 2..6);
    }
    #[test]
    fn all_catalog_raw_bytes_survive_unchanged_and_unrelated_edits() {
        for e in [
            Encoding::Utf8,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
            Encoding::Utf32Le,
            Encoding::Utf32Be,
            Encoding::Latin1,
            Encoding::Windows1250,
            Encoding::Windows1251,
            Encoding::Windows1252,
            Encoding::Windows1253,
            Encoding::Windows1254,
            Encoding::Windows1255,
            Encoding::Windows1256,
            Encoding::Windows1257,
            Encoding::Windows1258,
            Encoding::ShiftJis,
            Encoding::Gbk,
            Encoding::Big5,
            Encoding::EucJp,
            Encoding::EucKr,
        ] {
            let mut raw = e.bom().to_vec();
            raw.extend(0..=255);
            let (mut d, p) = open(raw.clone(), e);
            assert_eq!(save(&d, &p, e).unwrap(), raw);
            let end = d.snapshot().len();
            d.apply(EditTransaction {
                base_revision: d.snapshot().revision,
                edits: vec![Edit {
                    range: TextOffset(end)..TextOffset(end),
                    insert: "X".into(),
                }],
            })
            .unwrap();
            let mut expected = raw.clone();
            expected.extend(Encoder::new(e, false).encode_text("X").unwrap());
            assert_eq!(save(&d, &p, e).unwrap(), expected);
            d.undo().unwrap();
            assert_eq!(save(&d, &p, e).unwrap(), raw);
        }
    }
    #[test]
    fn opaque_conversion_refuses_but_removed_opaque_does_not_poison_document() {
        let (mut d, p) = open(vec![b'A', 255, b'B'], Encoding::Utf8);
        assert!(matches!(save(&d, &p, Encoding::Utf16Le), Err(ResidentError::At { .. })));
        d.apply(EditTransaction {
            base_revision: d.snapshot().revision,
            edits: vec![Edit {
                range: TextOffset(1)..TextOffset(4),
                insert: "�".into(),
            }],
        })
        .unwrap();
        assert!(save(&d, &p, Encoding::Utf16Le).is_ok());
        assert!(matches!(
            p.interpret(
                Encoding::Latin1,
                true,
                false,
                Budget::new(100000),
                Budget::new(100000),
                1000
            ),
            Err(ResidentError::DirtyInterpret)
        ));
        let (interpreted, _) = p
            .interpret(
                Encoding::Latin1,
                true,
                true,
                Budget::new(100000),
                Budget::new(100),
                1000,
            )
            .unwrap();
        assert_eq!(
            interpreted.snapshot().read(TextOffset(0)..TextOffset(4), 100).unwrap(),
            "AÿB"
        );
    }
    #[test]
    fn legacy_multiscalar_unit_at_document_piece_boundary_roundtrips() {
        let mut raw = vec![b'a'; 65534];
        raw.extend([0x88, 0x62, b'z']);
        let (d, p) = open(raw.clone(), Encoding::Big5);
        assert_eq!(save(&d, &p, Encoding::Big5).unwrap(), raw);
    }
    #[test]
    fn utf8_identity_provenance_retains_only_opaque_bytes() {
        let mut raw = Encoding::Utf8.bom().to_vec();
        raw.extend("aé中😀\r\n".repeat(1000).as_bytes());
        raw.extend([0xff, b'x', 0xc3]);
        let (d, p) = open(raw.clone(), Encoding::Utf8);
        // Mixed scalar widths record no per-run provenance and no raw copy.
        assert_eq!(*p.raw, [0xff, 0xc3]);
        assert_eq!(p.mapping.len(), 2);
        assert_eq!(p.original_len(), raw.len());
        assert_eq!(p.read_original(0..raw.len()).unwrap(), raw);
        // Bounded reads split scalars, the BOM and opaque units exactly.
        for range in [0..1, 1..5, 4..9, raw.len() - 4..raw.len() - 1, raw.len() - 1..raw.len()] {
            assert_eq!(p.read_original(range.clone()).unwrap(), raw[range]);
        }
        assert!(p.read_original(0..raw.len() + 1).is_err());
        assert_eq!(save(&d, &p, Encoding::Utf8).unwrap(), raw);
        // Bounded reads start at a searched unit: every short window, prefix and
        // suffix across many opaque units of one to three bytes is exact.
        let mut many: Vec<u8> = Vec::new();
        for i in 0..40 {
            many.extend("é中a".as_bytes());
            many.extend_from_slice(match i % 3 {
                0 => &[0xff][..],
                1 => &[0xe4, 0xb8],
                _ => &[0xf0, 0x9f, 0x98],
            });
            many.push(b'x');
        }
        let (_, q) = open(many.clone(), Encoding::Utf8);
        assert_eq!(q.mapping.len(), 40);
        for start in 0..=many.len() {
            for end in (start..=many.len().min(start + 9)).chain([many.len()]) {
                assert_eq!(q.read_original(start..end).unwrap(), many[start..end], "{start}..{end}");
            }
            assert_eq!(q.read_original(0..start).unwrap(), many[..start]);
        }
        let (latin, l) = p
            .interpret(
                Encoding::Latin1,
                false,
                false,
                Budget::new(4 * 1024 * 1024),
                Budget::new(1024),
                1024 * 1024,
            )
            .unwrap();
        assert_eq!(save(&latin, &l, Encoding::Latin1).unwrap(), raw);
    }
    #[test]
    fn provenance_quota_and_budget_exhaustion_report_limit() {
        let raw = "aé".repeat(1000).into_bytes();
        let quota = ResidentEncoding::open(
            raw.clone(),
            Some(Encoding::Windows1252),
            Budget::new(1024 * 1024),
            Budget::new(1024),
            raw.len(),
            100 * std::mem::size_of::<Mapping>(),
        );
        assert!(matches!(quota, Err(ResidentError::Limit)));
        let budget = ResidentEncoding::open(
            vec![b'a'; 200_000],
            Some(Encoding::Utf8),
            Budget::new(100_000),
            Budget::new(1024),
            200_000,
            1024 * 1024,
        );
        assert!(matches!(budget, Err(ResidentError::Limit)));
    }
    #[test]
    fn mutable_policy_cannot_relabel_original_provenance() {
        let (d, mut p) = open(vec![b'A'], Encoding::Utf8);
        p.state.user_override = Some(Encoding::Utf16Le);
        assert_eq!(save(&d, &p, Encoding::Utf16Le).unwrap(), [65, 0]);
        let (d, mut p) = open(vec![255], Encoding::Utf8);
        p.state.user_override = Some(Encoding::Utf16Le);
        assert!(matches!(save(&d, &p, Encoding::Utf16Le), Err(ResidentError::At { .. })));
    }
}
#[derive(Clone)]
pub struct ResidentEncoding {
    original_encoding: Encoding,
    /// Whole original bytes, or under UTF-8 identity provenance only the opaque
    /// (invalid) units that `mapping` indexes; valid UTF-8 bytes equal their text.
    raw: Arc<Vec<u8>>,
    original_bom: bool,
    original_len: usize,
    baseline: DocumentSnapshot,
    mapping: Arc<Vec<Mapping>>,
    _claims: Arc<Vec<BudgetClaim>>,
    pub state: EncodingState,
    pub eol: EolState,
}
#[derive(Debug)]
pub enum ResidentError {
    At {
        range: std::ops::Range<usize>,
        reason: String,
    },
    Cancelled,
    Limit,
    Document(bareline_document::Error),
    Codec(CodecError),
    DirtyInterpret,
    WrongDocument,
}
/// Plain-language reason shown to the user (UI-03); `Debug` stays for diagnostics.
impl std::fmt::Display for ResidentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::At { range, reason } => write!(f, "{reason} at bytes {}..{}", range.start, range.end),
            Self::Cancelled => f.write_str("the conversion was cancelled"),
            Self::Limit => {
                f.write_str("the text is larger than the configured memory limit allows (Settings > Advanced)")
            }
            Self::Document(error) => std::fmt::Display::fmt(error, f),
            Self::Codec(error) => std::fmt::Display::fmt(error, f),
            Self::DirtyInterpret => f.write_str("save or undo your changes before reinterpreting the encoding"),
            Self::WrongDocument => f.write_str(
                "the document was reloaded or replaced during the conversion; nothing was changed, try again",
            ),
        }
    }
}
impl From<CodecError> for ResidentError {
    fn from(e: CodecError) -> Self {
        Self::Codec(e)
    }
}
impl From<bareline_document::Error> for ResidentError {
    fn from(e: bareline_document::Error) -> Self {
        Self::Document(e)
    }
}
struct Collector {
    text: String,
    mapping: Vec<Mapping>,
    state: EncodingState,
    eol: EolState,
    text_offset: usize,
    builder: DocumentBuilder,
    budget: Budget,
    claims: Vec<BudgetClaim>,
    mapping_limit: usize,
    /// UTF-8 identity provenance (FIO-01): valid spans record no mapping and keep
    /// no raw copy; only opaque units are mapped, into `opaque_raw`.
    identity: bool,
    opaque_raw: Vec<u8>,
    /// A quota or budget refused growth: a size outcome, not a decoding failure.
    exhausted: bool,
}
impl DecodedSink for Collector {
    fn remaining_capacity(&self) -> usize {
        usize::MAX
    }
    fn write(&mut self, span: DecodedSpan<'_>) -> Result<(), CodecError> {
        if span.text.is_empty() {
            return Ok(());
        }
        let raw_len = (span.original.end.0 - span.original.start.0) as usize;
        let identity = self.identity;
        let recorded = !identity || span.opaque_bytes.is_some();
        let coalesce = recorded
            && self.mapping.last().is_some_and(|m| {
                !m.opaque
                    && span.opaque_bytes.is_none()
                    && m.text_unit == span.text.len()
                    && m.raw_unit == raw_len
                    && m.raw.end == span.original.start.0 as usize
            });
        if recorded && !coalesce && self.mapping.len() >= self.mapping_limit {
            return Err(self.exhaust("resident provenance quota"));
        }
        // Exact growth prevents Vec/String's geometric reserve from exceeding the
        // caller's explicit temporary limits near a quota boundary.
        if recorded && !coalesce && self.mapping.len() == self.mapping.capacity() {
            let additional = (self.mapping_limit - self.mapping.len()).min(1024);
            let claim = self
                .budget
                .claim(additional * std::mem::size_of::<Mapping>())
                .map_err(|e| self.exhaust(format!("budget: {e}")))?;
            self.claims.push(claim);
            self.mapping
                .try_reserve_exact(additional)
                .map_err(|e| self.exhaust(e))?;
        }
        if let Some(bytes) = span.opaque_bytes.filter(|_| identity)
            && self.opaque_raw.capacity() - self.opaque_raw.len() < bytes.len()
        {
            let additional = bytes.len().max(self.opaque_raw.len().clamp(4096, 1 << 20));
            let claim = self
                .budget
                .claim(additional)
                .map_err(|e| self.exhaust(format!("budget: {e}")))?;
            self.claims.push(claim);
            self.opaque_raw
                .try_reserve_exact(additional)
                .map_err(|e| self.exhaust(e))?;
        }
        if self.text.len() + span.text.len() > 65536 {
            self.flush()?;
        }
        let start = self.text_offset + self.text.len();
        self.text.push_str(span.text);
        self.eol.push(span.text, false);
        if let Some(bytes) = span.opaque_bytes {
            self.state.record_invalid(bytes.len());
        }
        if !recorded {
            return Ok(());
        }
        if coalesce {
            let m = self.mapping.last_mut().unwrap();
            m.text.end = self.text_offset + self.text.len();
            m.raw.end = span.original.end.0 as usize;
        } else {
            let raw = match span.opaque_bytes.filter(|_| identity) {
                Some(bytes) => {
                    let at = self.opaque_raw.len();
                    self.opaque_raw.extend_from_slice(bytes);
                    at..self.opaque_raw.len()
                }
                None => span.original.start.0 as usize..span.original.end.0 as usize,
            };
            self.mapping.push(Mapping {
                text: start..self.text_offset + self.text.len(),
                raw,
                opaque: span.opaque_bytes.is_some(),
                text_unit: span.text.len(),
                raw_unit: raw_len,
            });
        }
        Ok(())
    }
}
impl Collector {
    fn exhaust(&mut self, reason: impl std::fmt::Display) -> CodecError {
        self.exhausted = true;
        CodecError::Output(std::io::Error::other(reason.to_string()))
    }
    /// Quota and budget exhaustion report `Limit`, so callers fall back to paged
    /// storage instead of presenting an encoding failure (FIO-01).
    fn error(&self, error: CodecError) -> ResidentError {
        if self.exhausted {
            ResidentError::Limit
        } else {
            ResidentError::Codec(error)
        }
    }
    fn flush(&mut self) -> Result<(), CodecError> {
        if !self.text.is_empty() {
            self.builder.append(&self.text).map_err(|e| match e {
                bareline_document::Error::BudgetExceeded => self.exhaust("document budget"),
                e => CodecError::Output(std::io::Error::other(format!("document: {e}"))),
            })?;
            self.text_offset += self.text.len();
            self.text.clear();
        }
        Ok(())
    }
}
/// Streaming Resident transcoder. Raw baseline, mapping capacity, scratch buffer and
/// decoded document all charge the same aggregate byte budget before allocation.
/// UTF-8 keeps no raw baseline: its valid bytes are the decoded text itself.
pub struct ResidentBuilder {
    decoder: Decoder,
    collector: Collector,
    raw: Vec<u8>,
    raw_limit: usize,
    received: usize,
    _scratch: BudgetClaim,
}
impl ResidentBuilder {
    pub fn new(
        sample: &[u8],
        interpret: Option<Encoding>,
        bytes: Budget,
        history: Budget,
        raw_limit: usize,
        max_mapping_bytes: usize,
    ) -> Result<Self, ResidentError> {
        let mut state = EncodingState::new(detect(sample));
        state.user_override = interpret;
        state.save_target = state.interpreted();
        state.bom = !state.interpreted().bom().is_empty() && sample.starts_with(state.interpreted().bom());
        let identity = state.interpreted() == Encoding::Utf8;
        let raw_claim = bytes.claim(if identity { 0 } else { raw_limit })?;
        let scratch = bytes.claim(65536)?;
        Ok(Self {
            decoder: Decoder::new(state.interpreted()),
            collector: Collector {
                text: String::with_capacity(65536),
                mapping: vec![],
                state,
                eol: EolState::default(),
                text_offset: 0,
                builder: DocumentBuilder::new(bytes.clone(), history)?,
                budget: bytes,
                claims: vec![raw_claim],
                mapping_limit: max_mapping_bytes / std::mem::size_of::<Mapping>(),
                identity,
                opaque_raw: Vec::new(),
                exhausted: false,
            },
            raw: if identity {
                Vec::new()
            } else {
                Vec::with_capacity(raw_limit)
            },
            raw_limit,
            received: 0,
            _scratch: scratch,
        })
    }
    pub fn push(&mut self, raw: &[u8]) -> Result<(), ResidentError> {
        if raw.len() > self.raw_limit - self.received {
            return Err(ResidentError::Limit);
        }
        self.received += raw.len();
        if !self.collector.identity {
            self.raw.extend_from_slice(raw);
        }
        let p = self
            .decoder
            .push(raw, false, &mut self.collector)
            .map_err(|e| self.collector.error(e))?;
        if p.needs_output || p.consumed != raw.len() {
            return Err(ResidentError::Limit);
        }
        Ok(())
    }
    pub fn prefix(&mut self) -> Result<DocumentSnapshot, ResidentError> {
        self.collector.flush().map_err(|e| self.collector.error(e))?;
        Ok(self.collector.builder.prefix())
    }
    pub fn finish(mut self) -> Result<(Document, ResidentEncoding), ResidentError> {
        self.decoder
            .push(&[], true, &mut self.collector)
            .map_err(|e| self.collector.error(e))?;
        self.collector.flush().map_err(|e| self.collector.error(e))?;
        self.collector.eol.push("", true);
        let document = self.collector.builder.finish();
        let raw = if self.collector.identity {
            // Identity provenance must reconstruct every received byte: BOM, valid
            // text and opaque units. Anything else keeps the exact raw copy paged.
            let bom = if self.collector.state.bom {
                Encoding::Utf8.bom().len()
            } else {
                0
            };
            // One U+FFFD per opaque unit also lets `visit_original` locate a unit
            // by binary search.
            let one_unit = self.collector.mapping.iter().all(|m| m.text.len() == REPLACEMENT_LEN);
            let opaque_text = self.collector.mapping.len() * REPLACEMENT_LEN;
            let rebuilt = (bom + document.snapshot().len() + self.collector.opaque_raw.len()).checked_sub(opaque_text);
            if !one_unit || rebuilt != Some(self.received) {
                return Err(ResidentError::Limit);
            }
            self.collector.opaque_raw
        } else {
            self.raw
        };
        let result = ResidentEncoding {
            original_encoding: self.collector.state.interpreted(),
            raw: Arc::new(raw),
            original_bom: self.collector.state.bom,
            original_len: self.received,
            baseline: document.snapshot(),
            mapping: Arc::new(self.collector.mapping),
            _claims: Arc::new(self.collector.claims),
            state: self.collector.state,
            eol: self.collector.eol,
        };
        Ok((document, result))
    }
}
impl ResidentEncoding {
    /// UTF-8 identity provenance: unmapped original text is its own raw bytes.
    fn identity(&self) -> bool {
        self.original_encoding == Encoding::Utf8
    }
    /// Exact original byte length, including any BOM.
    pub fn original_len(&self) -> usize {
        self.original_len
    }
    /// Stream the exact original bytes in `range`, in order, without materializing
    /// them. Identity provenance reads valid spans from the retained baseline and
    /// opaque units from `raw`; a baseline read failure is reported, never skipped.
    pub fn visit_original<E: From<ResidentError>>(
        &self,
        range: Range<usize>,
        mut visit: impl FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        if range.start > range.end || range.end > self.original_len {
            return Err(ResidentError::Limit.into());
        }
        if !self.identity() {
            for chunk in self.raw[range.clone()].chunks(65536) {
                visit(chunk)?;
            }
            return Ok(());
        }
        // The part of segment `at..at + len` inside `range`, relative to `at`.
        let clip = |at: usize, len: usize| range.start.clamp(at, at + len) - at..range.end.clamp(at, at + len) - at;
        let bom: &[u8] = if self.original_bom { Encoding::Utf8.bom() } else { &[] };
        let part = clip(0, bom.len());
        if !part.is_empty() {
            visit(&bom[part])?;
        }
        // Unit `k` starts after the BOM, its predecessors' opaque bytes and the
        // valid text before it (`finish` checks each unit is one U+FFFD), so the
        // walk starts at the first unit ending past `range.start`.
        let unit_start = |k: usize| {
            let m = &self.mapping[k];
            bom.len() + m.raw.start + m.text.start - k * REPLACEMENT_LEN
        };
        let first = {
            let (mut low, mut high) = (0, self.mapping.len());
            while low < high {
                let mid = low + (high - low) / 2;
                if unit_start(mid) + self.mapping[mid].raw.len() <= range.start {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            low
        };
        let (mut at, mut cursor) = match first.checked_sub(1) {
            Some(k) => (unit_start(k) + self.mapping[k].raw.len(), self.mapping[k].text.end),
            None => (bom.len(), 0),
        };
        let tail = self.baseline.len()..self.baseline.len();
        let spans = self.mapping[first..]
            .iter()
            .map(|m| (m.text.clone(), Some(m.raw.clone())));
        for (text, raw) in spans.chain([(tail, None)]) {
            if at >= range.end {
                break;
            }
            // Valid UTF-8 before this opaque unit: its text bytes are the original.
            let part = clip(at, text.start - cursor);
            if !part.is_empty() {
                let (a, b) = (cursor + part.start, cursor + part.end);
                let mut start = a;
                while !self.baseline.is_boundary(TextOffset(start)) {
                    start -= 1;
                }
                let mut end = b;
                while !self.baseline.is_boundary(TextOffset(end)) {
                    end += 1;
                }
                let mut position = start;
                for chunk in self
                    .baseline
                    .chunks(TextOffset(start)..TextOffset(end))
                    .map_err(ResidentError::from)?
                {
                    let bytes = chunk.as_bytes();
                    let next = position + bytes.len();
                    let local = a.clamp(position, next) - position..b.clamp(position, next) - position;
                    if !local.is_empty() {
                        visit(&bytes[local])?;
                    }
                    position = next;
                }
            }
            at += text.start - cursor;
            if let Some(raw) = raw {
                let bytes = &self.raw[raw];
                let part = clip(at, bytes.len());
                if !part.is_empty() {
                    visit(&bytes[part])?;
                }
                at += bytes.len();
            }
            cursor = text.end;
        }
        Ok(())
    }
    /// Bounded exact read of original bytes, e.g. for an extension's raw view.
    pub fn read_original(&self, range: Range<usize>) -> Result<Vec<u8>, ResidentError> {
        if range.start > range.end || range.end > self.original_len {
            return Err(ResidentError::Limit);
        }
        let mut bytes = Vec::with_capacity(range.end - range.start);
        self.visit_original(range, |part| {
            bytes.extend_from_slice(part);
            Ok::<(), ResidentError>(())
        })?;
        Ok(bytes)
    }
    pub fn has_opaque_original(&self) -> bool {
        self.mapping.iter().any(|span| span.opaque)
    }
    pub fn original_encoding(&self) -> Encoding {
        self.original_encoding
    }
    /// Pointer identity distinguishes retained original allocation from typed equal text.
    pub fn spill_original_range(&self, text: &str) -> Option<std::ops::Range<u64>> {
        let mut offset = 0u64;
        for chunk in self
            .baseline
            .chunks(TextOffset(0)..TextOffset(self.baseline.len()))
            .ok()?
        {
            if let Some(local) = (text.as_ptr() as usize).checked_sub(chunk.as_ptr() as usize) {
                if local <= chunk.len() && text.len() <= chunk.len() - local {
                    let start = offset + local as u64;
                    return Some(start..start + text.len() as u64);
                }
            }
            offset += chunk.len() as u64;
        }
        None
    }
    pub fn recovery_pieces(
        &self,
        snapshot: &DocumentSnapshot,
    ) -> Result<Vec<bareline_document::paged::RestoredPiece>, ResidentError> {
        if !snapshot.same_document(&self.baseline) {
            return Err(ResidentError::WrongDocument);
        }
        let mut originals = Vec::new();
        let mut offset = 0;
        for text in self.baseline.chunks(TextOffset(0)..TextOffset(self.baseline.len()))? {
            originals.push((text.as_ptr() as usize, text.len(), offset));
            offset += text.len();
        }
        snapshot
            .chunks(TextOffset(0)..TextOffset(snapshot.len()))?
            .map(|text| {
                let origin = originals.iter().find_map(|&(base, len, offset)| {
                    (text.as_ptr() as usize)
                        .checked_sub(base)
                        .filter(|&n| n <= len && text.len() <= len - n)
                        .map(|n| offset + n)
                });
                Ok(match origin {
                    Some(start) => {
                        bareline_document::paged::RestoredPiece::Original(start as u64..(start + text.len()) as u64)
                    }
                    None => bareline_document::paged::RestoredPiece::Inserted(text.to_owned()),
                })
            })
            .collect()
    }

    /// No source file writes. All temporary allocation is bounded by explicit limits;
    /// text storage additionally participates in the document byte budget.
    pub fn open(
        raw: Vec<u8>,
        interpret: Option<Encoding>,
        bytes: Budget,
        history: Budget,
        max_raw_bytes: usize,
        max_mapping_bytes: usize,
    ) -> Result<(Document, Self), ResidentError> {
        if raw.len() > max_raw_bytes {
            return Err(ResidentError::Limit);
        }
        let mut builder = ResidentBuilder::new(&raw, interpret, bytes, history, raw.len(), max_mapping_bytes)?;
        for chunk in raw.chunks(65536) {
            builder.push(chunk)?;
        }
        drop(raw);
        builder.finish()
    }
    pub fn interpret(
        &self,
        encoding: Encoding,
        dirty: bool,
        discard_confirmed: bool,
        bytes: Budget,
        history: Budget,
        max_mapping_bytes: usize,
    ) -> Result<(Document, Self), ResidentError> {
        if dirty && !discard_confirmed {
            return Err(ResidentError::DirtyInterpret);
        }
        self.reinterpret_streaming(encoding, bytes, history, max_mapping_bytes, || Ok(()))
    }
    pub fn reinterpret_streaming(
        &self,
        encoding: Encoding,
        bytes: Budget,
        history: Budget,
        max_mapping_bytes: usize,
        mut checkpoint: impl FnMut() -> Result<(), ResidentError>,
    ) -> Result<(Document, Self), ResidentError> {
        checkpoint()?;
        // Detection reads at most 64 KiB; the rest streams without a whole copy.
        let sample = self.read_original(0..self.original_len.min(65536))?;
        let mut builder = ResidentBuilder::new(
            &sample,
            Some(encoding),
            bytes,
            history,
            self.original_len,
            max_mapping_bytes,
        )?;
        drop(sample);
        self.visit_original(0..self.original_len, |chunk| {
            checkpoint()?;
            builder.push(chunk)
        })?;
        checkpoint()?;
        builder.finish()
    }
    /// Streams immutable pieces. Unchanged original units copy exact bytes, including
    /// noncanonical mappings; inserted or partially edited multi-scalar units encode.
    pub fn write_snapshot(
        &self,
        snapshot: &DocumentSnapshot,
        target: Encoding,
        bom: bool,
        out: &mut dyn Write,
    ) -> Result<(), ResidentError> {
        let policy = super::state::metadata_encoding(snapshot.metadata());
        let (target, bom) = policy.map_or((target, bom), |state| (state.save_target, state.bom));
        if !snapshot.same_document(&self.baseline) {
            return Err(ResidentError::WrongDocument);
        }
        if !snapshot.is_complete() {
            return Err(ResidentError::Document(bareline_document::Error::IncompleteSource));
        }
        let write = |out: &mut dyn Write, b: &[u8]| {
            for chunk in b.chunks(65536) {
                out.write_all(chunk)
                    .map_err(|e| ResidentError::Codec(CodecError::Output(e)))?;
            }
            Ok::<(), ResidentError>(())
        };
        if bom {
            write(out, target.bom())?;
        }
        let encoder = Encoder::new(target, false);
        // Addresses are identity keys only: never dereferenced. The retained baseline
        // owns every segment for this operation's lifetime, preventing allocator reuse.
        let mut original_chunks = Vec::new();
        let mut offset = 0;
        for s in self.baseline.chunks(TextOffset(0)..TextOffset(self.baseline.len()))? {
            original_chunks.push((s.as_ptr() as usize, s.len(), offset));
            offset += s.len();
        }
        let origin_of = |chunk: &str| {
            let address = chunk.as_ptr() as usize;
            original_chunks.iter().find_map(|&(base, len, offset)| {
                address
                    .checked_sub(base)
                    .filter(|&n| n <= len && chunk.len() <= len - n)
                    .map(|n| offset + n)
            })
        };
        let mut chunks = snapshot.chunks(TextOffset(0)..TextOffset(snapshot.len()))?.peekable();
        let mut document_offset = 0usize;
        while let Some(chunk) = chunks.next() {
            if let Some(start) = origin_of(chunk) {
                let mut end = start + chunk.len();
                while let Some(next) = chunks.peek() {
                    if origin_of(next) != Some(end) {
                        break;
                    }
                    end += next.len();
                    chunks.next();
                }
                let encode_range = |a, b, out: &mut dyn Write| -> Result<(), ResidentError> {
                    let mut at = document_offset + a - start;
                    for text in self.baseline.chunks(TextOffset(a)..TextOffset(b))? {
                        for (local, part) in super::failure::bounded_chunks(text) {
                            let encoded = encoder.encode_text(part).map_err(|error| ResidentError::At {
                                range: super::failure::rejected_range(part, target, at + local),
                                reason: error.to_string(),
                            })?;
                            write(out, &encoded)?;
                        }
                        at += text.len();
                    }
                    Ok(())
                };
                // Identity provenance leaves valid UTF-8 unmapped: those gaps are
                // text whose UTF-8 bytes are exactly the original bytes.
                let gap = |a, b, out: &mut dyn Write| -> Result<(), ResidentError> {
                    if target != self.original_encoding || !self.identity() {
                        return encode_range(a, b, out);
                    }
                    for text in self.baseline.chunks(TextOffset(a)..TextOffset(b))? {
                        write(out, text.as_bytes())?;
                    }
                    Ok(())
                };
                let mut cursor = start;
                let first = self.mapping.partition_point(|m| m.text.end <= start);
                for m in self.mapping[first..].iter().take_while(|m| m.text.start < end) {
                    let a = m.text.start.max(start);
                    let b = m.text.end.min(end);
                    if cursor < a {
                        gap(cursor, a, out)?;
                    }
                    cursor = b;
                    if m.opaque && target != self.original_encoding {
                        return Err(ResidentError::At {
                            range: document_offset + a - start..document_offset + b - start,
                            reason: "Unresolved original bytes cannot be converted".into(),
                        });
                    }
                    if target == self.original_encoding {
                        let aligned_a = (m.text.start + (a - m.text.start).div_ceil(m.text_unit) * m.text_unit).min(b);
                        let aligned_b = (m.text.start + (b - m.text.start) / m.text_unit * m.text_unit).max(aligned_a);
                        encode_range(a, aligned_a, out)?;
                        let ra = m.raw.start + (aligned_a - m.text.start) / m.text_unit * m.raw_unit;
                        let rb = m.raw.start + (aligned_b - m.text.start) / m.text_unit * m.raw_unit;
                        write(out, &self.raw[ra..rb])?;
                        encode_range(aligned_b, b, out)?;
                    } else {
                        encode_range(a, b, out)?;
                    }
                }
                if cursor < end {
                    gap(cursor, end, out)?;
                }
                document_offset += end - start;
            } else {
                for (local, part) in super::failure::bounded_chunks(chunk) {
                    let encoded = encoder.encode_text(part).map_err(|error| ResidentError::At {
                        range: super::failure::rejected_range(part, target, document_offset + local),
                        reason: error.to_string(),
                    })?;
                    write(out, &encoded)?;
                }
                document_offset += chunk.len();
            }
        }
        Ok(())
    }
}
