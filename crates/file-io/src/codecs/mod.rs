// SPDX-License-Identifier: MPL-2.0
//! Stateless encoding catalog and bounded streaming adapters. Original source ranges
//! must be copied on unchanged same-encoding saves; encoding is not a bijection.
pub mod disk;
pub mod failure;
pub mod resident;
pub mod state;
use crate::{
    ByteSink, CodecError, DecodedSink, DecodedSpan, Progress, RawOffset, StreamingDecoder,
    StreamingEncoder,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
    Latin1,
    Windows1250,
    Windows1251,
    Windows1252,
    Windows1253,
    Windows1254,
    Windows1255,
    Windows1256,
    Windows1257,
    Windows1258,
    ShiftJis,
    Gbk,
    Big5,
    EucJp,
    EucKr,
}
impl Encoding {
    /// Catalog version 1. ISO-8859-1 aliases deliberately bypass encoding_rs.
    pub fn from_label(label: &str) -> Option<Self> {
        let label = label.trim().to_ascii_lowercase().replace('_', "-");
        Some(match label.as_str() {
            "utf-8" | "utf8" => Self::Utf8,
            "utf-16le" | "utf16le" => Self::Utf16Le,
            "utf-16be" | "utf16be" => Self::Utf16Be,
            "utf-32le" | "utf32le" => Self::Utf32Le,
            "utf-32be" | "utf32be" => Self::Utf32Be,
            "iso-8859-1" | "latin1" | "latin-1" => Self::Latin1,
            "shift-jis" | "sjis" | "windows-31j" => Self::ShiftJis,
            "gbk" | "cp936" => Self::Gbk,
            "big5" | "big-5" => Self::Big5,
            "euc-jp" => Self::EucJp,
            "euc-kr" | "windows-949" => Self::EucKr,
            "windows-1250" | "cp1250" => Self::Windows1250,
            "windows-1251" | "cp1251" => Self::Windows1251,
            "windows-1252" | "cp1252" => Self::Windows1252,
            "windows-1253" | "cp1253" => Self::Windows1253,
            "windows-1254" | "cp1254" => Self::Windows1254,
            "windows-1255" | "cp1255" => Self::Windows1255,
            "windows-1256" | "cp1256" => Self::Windows1256,
            "windows-1257" | "cp1257" => Self::Windows1257,
            "windows-1258" | "cp1258" => Self::Windows1258,
            _ => return None,
        })
    }
    pub fn bom(self) -> &'static [u8] {
        match self {
            Self::Utf8 => &[239, 187, 191],
            Self::Utf16Le => &[255, 254],
            Self::Utf16Be => &[254, 255],
            Self::Utf32Le => &[255, 254, 0, 0],
            Self::Utf32Be => &[0, 0, 254, 255],
            _ => &[],
        }
    }
    fn legacy(self) -> Option<&'static encoding_rs::Encoding> {
        use encoding_rs::*;
        Some(match self {
            Self::Windows1250 => WINDOWS_1250,
            Self::Windows1251 => WINDOWS_1251,
            Self::Windows1252 => WINDOWS_1252,
            Self::Windows1253 => WINDOWS_1253,
            Self::Windows1254 => WINDOWS_1254,
            Self::Windows1255 => WINDOWS_1255,
            Self::Windows1256 => WINDOWS_1256,
            Self::Windows1257 => WINDOWS_1257,
            Self::Windows1258 => WINDOWS_1258,
            Self::ShiftJis => SHIFT_JIS,
            Self::Gbk => GBK,
            Self::Big5 => BIG5,
            Self::EucJp => EUC_JP,
            Self::EucKr => EUC_KR,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Confidence {
    Bom,
    Utf8Sample,
    LegacySample,
    LegacyFallback,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Detection {
    pub encoding: Encoding,
    pub confidence: Confidence,
    pub bom: bool,
    pub binary_warning: bool,
}
/// At most 64 KiB are inspected. An incomplete final UTF-8 sequence remains optimistic.
pub fn detect(raw: &[u8]) -> Detection {
    let raw = &raw[..raw.len().min(65536)];
    let binary_warning = !raw.is_empty()
        && raw
            .iter()
            .filter(|&&b| b == 0 || (b < 32 && !matches!(b, 9 | 10 | 13)))
            .count()
            * 10
            > raw.len();
    for encoding in [
        Encoding::Utf32Le,
        Encoding::Utf32Be,
        Encoding::Utf8,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
    ] {
        if raw.starts_with(encoding.bom()) {
            return Detection {
                encoding,
                confidence: Confidence::Bom,
                bom: true,
                binary_warning: false,
            };
        }
    }
    let utf8 = match std::str::from_utf8(raw) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    };
    if !utf8 {
        // Conservative bounded trial decoding: script evidence is advisory, never
        // grounds to reinterpret later chunks. Ambiguous Chinese/code pages fall back.
        for e in [Encoding::EucJp, Encoding::ShiftJis, Encoding::EucKr] {
            if let Some(s) = e
                .legacy()
                .unwrap()
                .decode_without_bom_handling_and_without_replacement(raw)
            {
                let evidence = s
                    .chars()
                    .filter(|&c| match e {
                        Encoding::EucKr => ('\u{ac00}'..='\u{d7af}').contains(&c),
                        _ => ('\u{3040}'..='\u{30ff}').contains(&c),
                    })
                    .count();
                if evidence >= 2 {
                    return Detection {
                        encoding: e,
                        confidence: Confidence::LegacySample,
                        bom: false,
                        binary_warning,
                    };
                }
            }
        }
    }
    Detection {
        encoding: if utf8 {
            Encoding::Utf8
        } else {
            Encoding::Windows1252
        },
        confidence: if utf8 {
            Confidence::Utf8Sample
        } else {
            Confidence::LegacyFallback
        },
        bom: false,
        binary_warning,
    }
}

#[derive(Clone)]
pub struct Decoder {
    encoding: Encoding,
    pending: Vec<u8>,
    offset: u64,
    start: bool,
}
impl Decoder {
    pub fn new(encoding: Encoding) -> Self {
        Self {
            encoding,
            pending: Vec::with_capacity(4),
            offset: 0,
            start: true,
        }
    }
    // None means one more byte is needed. Invalid units retain their exact bytes.
    fn unit(&self, end: bool) -> Option<(usize, String, bool)> {
        let p = &self.pending;
        if p.is_empty() {
            return None;
        }
        let invalid = |n| Some((n, "\u{fffd}".to_owned(), true));
        let scalar = |n, c: Option<char>| match c {
            Some(c) => Some((n, c.to_string(), false)),
            None => invalid(n),
        };
        match self.encoding {
            Encoding::Utf8 => match std::str::from_utf8(p) {
                Ok(s) => {
                    let c = s.chars().next().unwrap();
                    scalar(c.len_utf8(), Some(c))
                }
                Err(e) if e.valid_up_to() > 0 => {
                    let s = std::str::from_utf8(&p[..e.valid_up_to()]).unwrap();
                    let c = s.chars().next().unwrap();
                    scalar(c.len_utf8(), Some(c))
                }
                Err(e) => match e.error_len() {
                    Some(n) => invalid(n),
                    None if end => invalid(p.len()),
                    None => None,
                },
            },
            Encoding::Latin1 => scalar(1, Some(char::from(p[0]))),
            Encoding::Utf16Le | Encoding::Utf16Be => {
                if p.len() < 2 {
                    return if end { invalid(p.len()) } else { None };
                }
                let word = |b: &[u8]| {
                    if self.encoding == Encoding::Utf16Le {
                        u16::from_le_bytes([b[0], b[1]])
                    } else {
                        u16::from_be_bytes([b[0], b[1]])
                    }
                };
                let w = word(p);
                if (0xd800..=0xdbff).contains(&w) {
                    if p.len() < 4 {
                        return if end { invalid(2) } else { None };
                    }
                    let low = word(&p[2..]);
                    if !(0xdc00..=0xdfff).contains(&low) {
                        return invalid(2);
                    }
                    scalar(
                        4,
                        char::from_u32(0x10000 + ((w as u32 - 0xd800) << 10) + low as u32 - 0xdc00),
                    )
                } else {
                    scalar(2, char::from_u32(w as u32))
                }
            }
            Encoding::Utf32Le | Encoding::Utf32Be => {
                if p.len() < 4 {
                    return if end { invalid(p.len()) } else { None };
                }
                let b = [p[0], p[1], p[2], p[3]];
                scalar(
                    4,
                    char::from_u32(if self.encoding == Encoding::Utf32Le {
                        u32::from_le_bytes(b)
                    } else {
                        u32::from_be_bytes(b)
                    }),
                )
            }
            e => {
                let width = match e {
                    Encoding::ShiftJis if matches!(p[0],0x81..=0x9f|0xe0..=0xfc) => 2,
                    Encoding::Gbk | Encoding::Big5 | Encoding::EucKr if p[0] >= 0x81 => 2,
                    Encoding::EucJp if p[0] == 0x8f => 3,
                    Encoding::EucJp if p[0] >= 0x80 => 2,
                    _ => 1,
                };
                if p.len() < width && !end {
                    return None;
                }
                let n = width.min(p.len());
                match e
                    .legacy()
                    .unwrap()
                    .decode_without_bom_handling_and_without_replacement(&p[..n])
                {
                    Some(s) => Some((n, s.into_owned(), false)),
                    None => invalid(1),
                }
            }
        }
    }
}
impl StreamingDecoder for Decoder {
    fn push(
        &mut self,
        raw: &[u8],
        end: bool,
        out: &mut dyn DecodedSink,
    ) -> Result<Progress, CodecError> {
        let mut consumed = 0;
        let mut produced = 0;
        loop {
            let final_unit = end && consumed == raw.len();
            if self.start {
                let bom = self.encoding.bom();
                if !bom.is_empty()
                    && bom.starts_with(&self.pending)
                    && self.pending.len() < bom.len()
                    && !final_unit
                {
                    if consumed == raw.len() {
                        break;
                    }
                    self.pending.push(raw[consumed]);
                    consumed += 1;
                    continue;
                }
                if !bom.is_empty() && self.pending == bom {
                    out.write(DecodedSpan {
                        text: "",
                        original: RawOffset(0)..RawOffset(bom.len() as u64),
                        opaque_bytes: None,
                    })?;
                    self.offset += bom.len() as u64;
                    self.pending.clear();
                }
                self.start = false;
            }
            if let Some((n, text, invalid)) = self.unit(final_unit) {
                if out.remaining_capacity() < text.len() {
                    return Ok(Progress {
                        consumed,
                        produced,
                        needs_output: true,
                    });
                }
                out.write(DecodedSpan {
                    text: &text,
                    original: RawOffset(self.offset)..RawOffset(self.offset + n as u64),
                    opaque_bytes: invalid.then_some(&self.pending[..n]),
                })?;
                self.pending.drain(..n);
                self.offset += n as u64;
                produced += text.len();
            } else if consumed < raw.len() {
                self.pending.push(raw[consumed]);
                consumed += 1;
            } else {
                break;
            }
        }
        Ok(Progress {
            consumed,
            produced,
            needs_output: false,
        })
    }
}

pub struct Encoder {
    encoding: Encoding,
    bom: bool,
    span_offset: usize,
}
impl Encoder {
    pub fn new(encoding: Encoding, bom: bool) -> Self {
        Self {
            encoding,
            bom,
            span_offset: 0,
        }
    }
    /// Convenience for caller-bounded chunks. Streaming push never allocates by span size.
    pub fn encode_text(&self, text: &str) -> Result<Vec<u8>, CodecError> {
        let mut result = Vec::new();
        for c in text.chars() {
            match self.encoding {
                Encoding::Utf8 => {
                    let mut b = [0; 4];
                    result.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                }
                Encoding::Latin1 => {
                    if c as u32 > 255 {
                        return Err(CodecError::Unrepresentable);
                    }
                    result.push(c as u8);
                }
                Encoding::Utf16Le | Encoding::Utf16Be => {
                    let mut b = [0; 2];
                    for w in c.encode_utf16(&mut b) {
                        result.extend_from_slice(&if self.encoding == Encoding::Utf16Le {
                            w.to_le_bytes()
                        } else {
                            w.to_be_bytes()
                        });
                    }
                }
                Encoding::Utf32Le => result.extend_from_slice(&(c as u32).to_le_bytes()),
                Encoding::Utf32Be => result.extend_from_slice(&(c as u32).to_be_bytes()),
                e => {
                    let mut b = [0; 4];
                    let (bytes, _, errors) = e.legacy().unwrap().encode(c.encode_utf8(&mut b));
                    if errors {
                        return Err(CodecError::Unrepresentable);
                    }
                    result.extend_from_slice(&bytes);
                }
            }
        }
        Ok(result)
    }
}
impl StreamingEncoder for Encoder {
    /// Progress.consumed counts complete input spans. On backpressure pass the
    /// unconsumed spans again unchanged: the partial scalar offset is retained.
    fn push(
        &mut self,
        spans: &[DecodedSpan<'_>],
        _end: bool,
        out: &mut dyn ByteSink,
    ) -> Result<Progress, CodecError> {
        let mut produced = 0;
        if self.bom {
            let bom = self.encoding.bom();
            if out.remaining_capacity() < bom.len() {
                return Ok(Progress {
                    consumed: 0,
                    produced: 0,
                    needs_output: true,
                });
            }
            out.write(bom)?;
            produced += bom.len();
            self.bom = false;
        }
        for (consumed, span) in spans.iter().enumerate() {
            if span.opaque_bytes.is_some() {
                return Err(CodecError::UnresolvedOpaqueBytes);
            }
            let remaining = span
                .text
                .get(self.span_offset..)
                .ok_or(CodecError::InvalidSequence)?;
            for c in remaining.chars() {
                let mut utf8 = [0; 4];
                let bytes = self.encode_text(c.encode_utf8(&mut utf8))?;
                if out.remaining_capacity() < bytes.len() {
                    return Ok(Progress {
                        consumed,
                        produced,
                        needs_output: true,
                    });
                }
                out.write(&bytes)?;
                produced += bytes.len();
                self.span_offset += c.len_utf8();
            }
            self.span_offset = 0;
        }
        Ok(Progress {
            consumed: spans.len(),
            produced,
            needs_output: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Sink {
        text: String,
        invalid: Vec<(u64, Vec<u8>)>,
        ranges: Vec<(u64, u64)>,
    }
    impl DecodedSink for Sink {
        fn remaining_capacity(&self) -> usize {
            usize::MAX
        }
        fn write(&mut self, s: DecodedSpan<'_>) -> Result<(), CodecError> {
            self.text.push_str(s.text);
            self.ranges.push((s.original.start.0, s.original.end.0));
            if let Some(b) = s.opaque_bytes {
                self.invalid.push((s.original.start.0, b.to_vec()));
            }
            Ok(())
        }
    }
    fn decode(e: Encoding, raw: &[u8], chunk: usize) -> Sink {
        let mut d = Decoder::new(e);
        let mut s = Sink::default();
        d.push(&[], false, &mut s).unwrap();
        for part in raw.chunks(chunk) {
            let p = d.push(part, false, &mut s).unwrap();
            assert_eq!(p.consumed, part.len());
        }
        d.push(&[], true, &mut s).unwrap();
        s
    }
    #[test]
    fn unicode_boundaries_bom_and_mixed_eol() {
        for e in [
            Encoding::Utf8,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
            Encoding::Utf32Le,
            Encoding::Utf32Be,
        ] {
            let text = "A\r\n😀\r漢\n\u{fffd}\u{e000}";
            let mut raw = e.bom().to_vec();
            raw.extend(Encoder::new(e, false).encode_text(text).unwrap());
            for n in 1..=5 {
                let s = decode(e, &raw, n);
                assert_eq!(s.text, text);
                assert!(s.invalid.is_empty());
                assert_eq!(s.ranges.last().unwrap().1, raw.len() as u64);
            }
            assert_eq!(detect(&raw).encoding, e);
        }
    }
    #[test]
    fn malformed_sequences_retain_raw_ranges_without_unicode_collisions() {
        let raw = b"\xef\xbf\xbd\xee\x80\x80\xf0\x9fX\xff";
        for n in 1..=5 {
            let s = decode(Encoding::Utf8, raw, n);
            assert_eq!(s.text, "\u{fffd}\u{e000}\u{fffd}X\u{fffd}");
            assert_eq!(s.invalid, vec![(6, vec![0xf0, 0x9f]), (9, vec![0xff])]);
        }
        let s = decode(Encoding::Utf16Le, &[0, 0xd8, 65, 0, 1], 1);
        assert_eq!(s.text, "�A�");
        assert_eq!(s.invalid, vec![(0, vec![0, 0xd8]), (4, vec![1])]);
        let s = decode(Encoding::Utf32Be, &[0, 0x11, 0, 0, 1], 1);
        assert_eq!(s.invalid.len(), 2);
    }
    #[test]
    fn legacy_catalog_and_true_latin1() {
        for e in [
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
            let raw = Encoder::new(e, false).encode_text("A\r\n").unwrap();
            assert_eq!(decode(e, &raw, 1).text, "A\r\n");
        }
        for (e, text) in [
            (Encoding::ShiftJis, "日本"),
            (Encoding::Gbk, "中文"),
            (Encoding::Big5, "中文"),
            (Encoding::EucJp, "日本"),
            (Encoding::EucKr, "한국"),
            (Encoding::Windows1252, "€é"),
        ] {
            let raw = Encoder::new(e, false).encode_text(text).unwrap();
            for n in 1..=3 {
                assert_eq!(decode(e, &raw, n).text, text);
            }
        }
        assert_eq!(decode(Encoding::Latin1, &[0x80, 0xff], 1).text, "\u{80}ÿ");
        assert_eq!(
            Encoder::new(Encoding::Latin1, false)
                .encode_text("\u{80}ÿ")
                .unwrap(),
            [0x80, 0xff]
        );
        assert!(matches!(
            Encoder::new(Encoding::Latin1, false).encode_text("€"),
            Err(CodecError::Unrepresentable)
        ));
    }
    #[test]
    fn detection_is_bounded_and_advisory() {
        let mut raw = vec![b'a'; 65537];
        raw[65536] = 255;
        assert_eq!(detect(&raw).encoding, Encoding::Utf8);
        assert_eq!(detect(&[255]).confidence, Confidence::LegacyFallback);
        assert!(detect(&[0; 100]).binary_warning);
    }
    #[test]
    fn decoder_backpressure_retains_token() {
        struct Full;
        impl DecodedSink for Full {
            fn remaining_capacity(&self) -> usize {
                0
            }
            fn write(&mut self, _: DecodedSpan<'_>) -> Result<(), CodecError> {
                panic!("full")
            }
        }
        let mut d = Decoder::new(Encoding::Utf8);
        let p = d.push(b"a", true, &mut Full).unwrap();
        assert!(p.needs_output);
        assert_eq!(p.consumed, 1);
        let mut s = Sink::default();
        d.push(&[], true, &mut s).unwrap();
        assert_eq!(s.text, "a");
    }

    #[test]
    fn encoder_backpressure_refusal_and_output_errors() {
        struct Bytes {
            bytes: Vec<u8>,
            capacity: usize,
            fail: bool,
        }
        impl ByteSink for Bytes {
            fn remaining_capacity(&self) -> usize {
                self.capacity
            }
            fn write(&mut self, b: &[u8]) -> Result<(), CodecError> {
                if self.fail {
                    return Err(CodecError::Output(std::io::Error::other(
                        "injected disk full",
                    )));
                }
                self.bytes.extend_from_slice(b);
                self.capacity -= b.len();
                Ok(())
            }
        }
        let spans = [DecodedSpan {
            text: "A😀B",
            original: RawOffset(0)..RawOffset(0),
            opaque_bytes: None,
        }];
        let mut e = Encoder::new(Encoding::Utf16Le, true);
        let mut out = Bytes {
            bytes: vec![],
            capacity: 4,
            fail: false,
        };
        let p = e.push(&spans, true, &mut out).unwrap();
        assert_eq!(p.consumed, 0);
        assert!(p.needs_output);
        out.capacity = 4;
        let p = e.push(&spans, true, &mut out).unwrap();
        assert!(p.needs_output);
        out.capacity = 2;
        let p = e.push(&spans, true, &mut out).unwrap();
        assert_eq!(p.consumed, 1);
        assert!(!p.needs_output);
        assert_eq!(decode(Encoding::Utf16Le, &out.bytes, 1).text, "A😀B");
        out.capacity = 100;
        out.fail = true;
        assert!(matches!(
            Encoder::new(Encoding::Utf8, false).push(&spans, true, &mut out),
            Err(CodecError::Output(_))
        ));
        let opaque = [DecodedSpan {
            text: "�",
            original: RawOffset(0)..RawOffset(1),
            opaque_bytes: Some(&[255]),
        }];
        assert!(matches!(
            Encoder::new(Encoding::Utf8, false).push(&opaque, true, &mut out),
            Err(CodecError::UnresolvedOpaqueBytes)
        ));
        assert_eq!(Encoding::from_label("ISO-8859-1"), Some(Encoding::Latin1));
        assert_eq!(Encoding::from_label("shift_jis"), Some(Encoding::ShiftJis));
        assert_eq!(Encoding::from_label("iso-2022-jp"), None);
    }
}
