// SPDX-License-Identifier: MPL-2.0
//! Explicitly caller-bounded Resident provenance. Retained baseline segments make
//! address-range identity valid even after piece splits, edits and undo.
use super::{
    Decoder, Encoder, Encoding, detect,
    state::{EncodingState, EolState},
};
use crate::{CodecError, DecodedSink, DecodedSpan, StreamingDecoder};
use bareline_document::{
    Budget, BudgetClaim, Document, DocumentBuilder, DocumentSnapshot, TextOffset,
};
use std::{io::Write, ops::Range, sync::Arc};

#[derive(Clone, Debug)]
struct Mapping {
    text: Range<usize>,
    raw: Range<usize>,
    opaque: bool,
    text_unit: usize,
    raw_unit: usize,
}

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
        assert!(matches!(
            save(&d, &p, Encoding::Utf16Le),
            Err(ResidentError::Codec(CodecError::UnresolvedOpaqueBytes))
        ));
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
            interpreted
                .snapshot()
                .read(TextOffset(0)..TextOffset(4), 100)
                .unwrap(),
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
    fn mutable_policy_cannot_relabel_original_provenance() {
        let (d, mut p) = open(vec![b'A'], Encoding::Utf8);
        p.state.user_override = Some(Encoding::Utf16Le);
        assert_eq!(save(&d, &p, Encoding::Utf16Le).unwrap(), [65, 0]);
        let (d, mut p) = open(vec![255], Encoding::Utf8);
        p.state.user_override = Some(Encoding::Utf16Le);
        assert!(matches!(
            save(&d, &p, Encoding::Utf16Le),
            Err(ResidentError::Codec(CodecError::UnresolvedOpaqueBytes))
        ));
    }
}
#[derive(Clone)]
pub struct ResidentEncoding {
    original_encoding: Encoding,
    raw: Arc<Vec<u8>>,
    baseline: DocumentSnapshot,
    mapping: Arc<Vec<Mapping>>,
    _claims: Arc<Vec<BudgetClaim>>,
    pub state: EncodingState,
    pub eol: EolState,
}
#[derive(Debug)]
pub enum ResidentError {
    Cancelled,
    Limit,
    Document(bareline_document::Error),
    Codec(CodecError),
    DirtyInterpret,
    WrongDocument,
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
        let coalesce = self.mapping.last().is_some_and(|m| {
            !m.opaque
                && span.opaque_bytes.is_none()
                && m.text_unit == span.text.len()
                && m.raw_unit == raw_len
                && m.raw.end == span.original.start.0 as usize
        });
        if !coalesce && self.mapping.len() >= self.mapping_limit {
            return Err(CodecError::Output(std::io::Error::other(
                "resident provenance quota",
            )));
        }
        // Exact growth prevents Vec/String's geometric reserve from exceeding the
        // caller's explicit temporary limits near a quota boundary.
        if !coalesce && self.mapping.len() == self.mapping.capacity() {
            let additional = (self.mapping_limit - self.mapping.len()).min(1024);
            self.claims.push(
                self.budget
                    .claim(additional * std::mem::size_of::<Mapping>())
                    .map_err(|e| {
                        CodecError::Output(std::io::Error::other(format!("budget: {e:?}")))
                    })?,
            );
            self.mapping
                .try_reserve_exact(additional)
                .map_err(|e| CodecError::Output(std::io::Error::other(e)))?;
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
        if coalesce {
            let m = self.mapping.last_mut().unwrap();
            m.text.end = self.text_offset + self.text.len();
            m.raw.end = span.original.end.0 as usize;
        } else {
            self.mapping.push(Mapping {
                text: start..self.text_offset + self.text.len(),
                raw: span.original.start.0 as usize..span.original.end.0 as usize,
                opaque: span.opaque_bytes.is_some(),
                text_unit: span.text.len(),
                raw_unit: raw_len,
            });
        }
        Ok(())
    }
}
impl Collector {
    fn flush(&mut self) -> Result<(), CodecError> {
        if !self.text.is_empty() {
            self.builder.append(&self.text).map_err(|e| {
                CodecError::Output(std::io::Error::other(format!("document: {e:?}")))
            })?;
            self.text_offset += self.text.len();
            self.text.clear();
        }
        Ok(())
    }
}
/// Streaming Resident transcoder. Raw baseline, mapping capacity, scratch buffer and
/// decoded document all charge the same aggregate byte budget before allocation.
pub struct ResidentBuilder {
    decoder: Decoder,
    collector: Collector,
    raw: Vec<u8>,
    raw_limit: usize,
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
        let raw_claim = bytes.claim(raw_limit)?;
        let scratch = bytes.claim(65536)?;
        let mut state = EncodingState::new(detect(sample));
        state.user_override = interpret;
        state.save_target = state.interpreted();
        state.bom =
            !state.interpreted().bom().is_empty() && sample.starts_with(state.interpreted().bom());
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
            },
            raw: Vec::with_capacity(raw_limit),
            raw_limit,
            _scratch: scratch,
        })
    }
    pub fn push(&mut self, raw: &[u8]) -> Result<(), ResidentError> {
        if raw.len() > self.raw_limit - self.raw.len() {
            return Err(ResidentError::Limit);
        }
        self.raw.extend_from_slice(raw);
        let p = self.decoder.push(raw, false, &mut self.collector)?;
        if p.needs_output || p.consumed != raw.len() {
            return Err(ResidentError::Limit);
        }
        Ok(())
    }
    pub fn prefix(&mut self) -> Result<DocumentSnapshot, ResidentError> {
        self.collector.flush()?;
        Ok(self.collector.builder.prefix())
    }
    pub fn finish(mut self) -> Result<(Document, ResidentEncoding), ResidentError> {
        self.decoder.push(&[], true, &mut self.collector)?;
        self.collector.flush()?;
        self.collector.eol.push("", true);
        let document = self.collector.builder.finish();
        let result = ResidentEncoding {
            original_encoding: self.collector.state.interpreted(),
            raw: Arc::new(self.raw),
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
    pub fn original_bytes(&self) -> Arc<Vec<u8>> { self.raw.clone() }
    pub fn has_opaque_original(&self) -> bool { self.mapping.iter().any(|span| span.opaque) }
    pub fn original_encoding(&self) -> Encoding { self.original_encoding }
    pub fn recovery_pieces(&self, snapshot: &DocumentSnapshot) -> Result<Vec<bareline_document::paged::RestoredPiece>, ResidentError> {
        if !snapshot.same_document(&self.baseline) { return Err(ResidentError::WrongDocument); }
        let mut originals=Vec::new();let mut offset=0;
        for text in self.baseline.chunks(TextOffset(0)..TextOffset(self.baseline.len()))? { originals.push((text.as_ptr() as usize,text.len(),offset));offset+=text.len(); }
        snapshot.chunks(TextOffset(0)..TextOffset(snapshot.len()))?.map(|text| {
            let origin=originals.iter().find_map(|&(base,len,offset)| (text.as_ptr() as usize).checked_sub(base).filter(|&n| n<=len && text.len()<=len-n).map(|n|offset+n));
            Ok(match origin {Some(start)=>bareline_document::paged::RestoredPiece::Original(start as u64..(start+text.len()) as u64),None=>bareline_document::paged::RestoredPiece::Inserted(text.to_owned())})
        }).collect()
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
        let mut builder = ResidentBuilder::new(
            &raw,
            interpret,
            bytes,
            history,
            raw.len(),
            max_mapping_bytes,
        )?;
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
        let mut builder = ResidentBuilder::new(
            &self.raw,
            Some(encoding),
            bytes,
            history,
            self.raw.len(),
            max_mapping_bytes,
        )?;
        for chunk in self.raw.chunks(65536) {
            checkpoint()?;
            builder.push(chunk)?;
        }
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
        if !snapshot.same_document(&self.baseline) {
            return Err(ResidentError::WrongDocument);
        }
        if !snapshot.is_complete() {
            return Err(ResidentError::Document(
                bareline_document::Error::IncompleteSource,
            ));
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
        for s in self
            .baseline
            .chunks(TextOffset(0)..TextOffset(self.baseline.len()))?
        {
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
        let mut chunks = snapshot
            .chunks(TextOffset(0)..TextOffset(snapshot.len()))?
            .peekable();
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
                let first = self.mapping.partition_point(|m| m.text.end <= start);
                for m in self.mapping[first..]
                    .iter()
                    .take_while(|m| m.text.start < end)
                {
                    let a = m.text.start.max(start);
                    let b = m.text.end.min(end);
                    if m.opaque && target != self.original_encoding {
                        return Err(ResidentError::Codec(CodecError::UnresolvedOpaqueBytes));
                    }
                    let encode_range = |a, b, out: &mut dyn Write| -> Result<(), ResidentError> {
                        for text in self.baseline.chunks(TextOffset(a)..TextOffset(b))? {
                            write(out, &encoder.encode_text(text)?)?;
                        }
                        Ok(())
                    };
                    if target == self.original_encoding {
                        let aligned_a = (m.text.start
                            + (a - m.text.start).div_ceil(m.text_unit) * m.text_unit)
                            .min(b);
                        let aligned_b = (m.text.start
                            + (b - m.text.start) / m.text_unit * m.text_unit)
                            .max(aligned_a);
                        encode_range(a, aligned_a, out)?;
                        let ra =
                            m.raw.start + (aligned_a - m.text.start) / m.text_unit * m.raw_unit;
                        let rb =
                            m.raw.start + (aligned_b - m.text.start) / m.text_unit * m.raw_unit;
                        write(out, &self.raw[ra..rb])?;
                        encode_range(aligned_b, b, out)?;
                    } else {
                        encode_range(a, b, out)?;
                    }
                }
            } else {
                write(out, &encoder.encode_text(chunk)?)?;
            }
        }
        Ok(())
    }
}
