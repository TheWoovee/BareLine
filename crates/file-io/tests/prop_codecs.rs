// SPDX-License-Identifier: MPL-2.0
//! Seeded property tests (QA-05): the public streaming decoder and encoder across random
//! chunk splits and sink backpressure, for every catalog encoding, plus detection.
use bareline_file_io::{
    ByteSink, CodecError, DecodedSink, DecodedSpan, RawOffset, StreamingDecoder, StreamingEncoder,
    codecs::{Confidence, Decoder, Encoder, Encoding, detect},
};
use std::ops::Range;

/// SplitMix64. Report the seed and case of a failure to reproduce it exactly.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        (0..self.below(max + 1)).map(|_| self.next_u64() as u8).collect()
    }
}

const ENCODINGS: &[Encoding] = &[
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
];
/// Unicode and ISO-8859-1 are bijective per scalar; legacy code pages may alias.
fn exact_unit_encoding(encoding: Encoding) -> bool {
    matches!(
        encoding,
        Encoding::Utf8
            | Encoding::Utf16Le
            | Encoding::Utf16Be
            | Encoding::Utf32Le
            | Encoding::Utf32Be
            | Encoding::Latin1
    )
}
const POOL: &[char] = &[
    'a',
    'Z',
    '0',
    ' ',
    '\t',
    '\r',
    '\n',
    '~',
    '\u{1}',
    '\u{7f}',
    '\u{80}',
    'é',
    'ÿ',
    '\u{100}',
    'Ω',
    'Ж',
    'ع',
    'א',
    'ก',
    '€',
    '‚',
    '¥',
    'Ê',
    '\u{304}',
    '\u{203e}',
    '\u{2212}',
    '\u{2028}',
    'あ',
    'ア',
    'ｱ',
    '中',
    '文',
    '한',
    '\u{feff}',
    '\u{fffd}',
    '\u{e000}',
    '\u{ffff}',
    '😀',
    '𝄞',
    '\u{10ffff}',
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct Span {
    text: String,
    original: Range<u64>,
    opaque: Option<Vec<u8>>,
}
struct Spans {
    spans: Vec<Span>,
    capacity: usize,
}
impl DecodedSink for Spans {
    fn remaining_capacity(&self) -> usize {
        self.capacity
    }
    fn write(&mut self, span: DecodedSpan<'_>) -> Result<(), CodecError> {
        assert!(span.text.len() <= self.capacity, "decoder overran sink capacity");
        self.capacity -= span.text.len();
        self.spans.push(Span {
            text: span.text.to_owned(),
            original: span.original.start.0..span.original.end.0,
            opaque: span.opaque_bytes.map(<[u8]>::to_vec),
        });
        Ok(())
    }
}
struct Bytes {
    bytes: Vec<u8>,
    capacity: usize,
}
impl ByteSink for Bytes {
    fn remaining_capacity(&self) -> usize {
        self.capacity
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), CodecError> {
        assert!(bytes.len() <= self.capacity, "encoder overran sink capacity");
        self.capacity -= bytes.len();
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

/// One push when `rng` is None; otherwise 0–5 byte chunks and a 0–8 byte sink that is
/// refilled (to at least one maximal unit) only after the decoder reports backpressure.
fn decode(encoding: Encoding, bytes: &[u8], mut rng: Option<&mut Rng>) -> Vec<Span> {
    fn draw(rng: &mut Option<&mut Rng>, n: usize, unsplit: usize) -> usize {
        match rng {
            Some(rng) => rng.below(n),
            None => unsplit,
        }
    }
    let mut decoder = Decoder::new(encoding);
    let mut sink = Spans {
        spans: Vec::new(),
        capacity: draw(&mut rng, 9, usize::MAX),
    };
    let mut offset: usize = 0;
    loop {
        let end = offset.saturating_add(draw(&mut rng, 6, bytes.len())).min(bytes.len());
        let last = end == bytes.len();
        let mut chunk = &bytes[offset..end];
        for attempt in 0.. {
            assert!(attempt < 64, "decoder made no progress");
            let progress = decoder.push(chunk, last, &mut sink).unwrap();
            chunk = &chunk[progress.consumed..];
            if !progress.needs_output {
                break;
            }
            sink.capacity = 4 + draw(&mut rng, 9, 0);
        }
        assert!(chunk.is_empty(), "decoder left input unconsumed without backpressure");
        offset = end;
        if last {
            return sink.spans;
        }
    }
}
fn decoded_text(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}
/// Spans tile the input exactly; opaque spans keep their bytes; exact encodings re-encode.
fn check_spans(encoding: Encoding, bytes: &[u8], spans: &[Span], context: &str) {
    let mut cursor = 0u64;
    for (index, span) in spans.iter().enumerate() {
        assert_eq!(span.original.start, cursor, "{context}: span {index} gap");
        assert!(span.original.end > span.original.start, "{context}: empty span {index}");
        cursor = span.original.end;
        let raw = &bytes[span.original.start as usize..span.original.end as usize];
        match &span.opaque {
            Some(opaque) => {
                assert_eq!(opaque.as_slice(), raw, "{context}: opaque bytes {index}");
                assert_eq!(span.text, "\u{fffd}", "{context}: opaque display {index}");
            }
            None if span.text.is_empty() => {
                assert_eq!(index, 0, "{context}: BOM span after text");
                assert_eq!(raw, encoding.bom(), "{context}: BOM range");
            }
            None if exact_unit_encoding(encoding) => assert_eq!(
                Encoder::new(encoding, false).encode_text(&span.text).unwrap(),
                raw,
                "{context}: span {index} does not re-encode to its source bytes"
            ),
            None => {}
        }
    }
    assert_eq!(cursor, bytes.len() as u64, "{context}: spans do not cover the input");
}
/// Independent lossy oracles for the Unicode decoders (maximal-subpart replacement).
fn lossy_oracle(encoding: Encoding, bytes: &[u8]) -> Option<String> {
    let body = bytes.strip_prefix(encoding.bom()).unwrap_or(bytes);
    let units = move |width: usize| body.chunks(width).filter(move |unit| unit.len() == width);
    let tail = move |width: usize| (!body.len().is_multiple_of(width)).then_some('\u{fffd}');
    Some(match encoding {
        Encoding::Utf8 => String::from_utf8_lossy(body).into_owned(),
        Encoding::Latin1 => body.iter().map(|byte| char::from(*byte)).collect(),
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let words = units(2).map(|unit| {
                let pair = [unit[0], unit[1]];
                if encoding == Encoding::Utf16Le {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            });
            char::decode_utf16(words)
                .map(|scalar| scalar.unwrap_or('\u{fffd}'))
                .chain(tail(2))
                .collect()
        }
        Encoding::Utf32Le | Encoding::Utf32Be => units(4)
            .map(|unit| {
                let quad = [unit[0], unit[1], unit[2], unit[3]];
                let value = if encoding == Encoding::Utf32Le {
                    u32::from_le_bytes(quad)
                } else {
                    u32::from_be_bytes(quad)
                };
                char::from_u32(value).unwrap_or('\u{fffd}')
            })
            .chain(tail(4))
            .collect(),
        _ => return None,
    })
}

#[test]
fn arbitrary_bytes_decode_identically_across_chunk_splits_and_backpressure() {
    let mut rng = Rng(0xc0de_c001);
    for case in 0..600 {
        let encoding = *rng.pick(ENCODINGS);
        let mut bytes = rng.bytes(40);
        if rng.chance(25) {
            // Start with this encoding's (possibly truncated) BOM.
            let bom = encoding.bom();
            let keep = rng.below(bom.len() + 1);
            bytes = [&bom[..keep], bytes.as_slice()].concat();
        }
        let context = format!("case {case} {encoding:?} {bytes:02x?}");
        let whole = decode(encoding, &bytes, None);
        check_spans(encoding, &bytes, &whole, &context);
        for _ in 0..3 {
            assert_eq!(
                decode(encoding, &bytes, Some(&mut rng)),
                whole,
                "{context}: split decode differs"
            );
        }
        if let Some(expected) = lossy_oracle(encoding, &bytes) {
            assert_eq!(decoded_text(&whole), expected, "{context}: lossy oracle");
        }
    }
}

fn encode_stream(encoding: Encoding, bom: bool, text: &str, rng: &mut Rng) -> Result<Vec<u8>, CodecError> {
    let mut cuts: Vec<usize> = (0..rng.below(6))
        .map(|_| rng.below(text.len() + 1))
        .filter(|at| text.is_char_boundary(*at))
        .chain([0, text.len()])
        .collect();
    cuts.sort_unstable();
    let spans: Vec<DecodedSpan<'_>> = cuts
        .windows(2)
        .map(|cut| DecodedSpan {
            text: &text[cut[0]..cut[1]],
            original: RawOffset(0)..RawOffset(0),
            opaque_bytes: None,
        })
        .collect();
    let mut encoder = Encoder::new(encoding, bom);
    let mut sink = Bytes {
        bytes: Vec::new(),
        capacity: rng.below(9),
    };
    let mut next = 0;
    for _ in 0..4 * text.len() + 16 {
        let progress = encoder.push(&spans[next..], true, &mut sink)?;
        next += progress.consumed;
        if !progress.needs_output {
            assert_eq!(next, spans.len(), "encoder finished without consuming every span");
            return Ok(sink.bytes);
        }
        // Resubmit the unconsumed spans unchanged; the encoder remembers partial spans.
        sink.capacity = 4 + rng.below(9);
    }
    panic!("encoder made no progress");
}
/// Scalars that survive this encoding after an ASCII prefix (so U+FEFF is not a BOM).
fn round_trip_pool(encoding: Encoding) -> Vec<char> {
    POOL.iter()
        .copied()
        .filter(|scalar| {
            let text = format!("a{scalar}");
            Encoder::new(encoding, false)
                .encode_text(&text)
                .is_ok_and(|bytes| decoded_text(&decode(encoding, &bytes, None)) == text)
        })
        .collect()
}

#[test]
fn representable_text_round_trips_through_streaming_encoder_and_decoder() {
    let mut rng = Rng(0xc0de_c002);
    for &encoding in ENCODINGS {
        let pool = round_trip_pool(encoding);
        assert!(pool.len() >= 8, "{encoding:?}: too few representable scalars: {pool:?}");
        for case in 0..40 {
            let text: String = (0..rng.below(24)).map(|_| *rng.pick(&pool)).collect();
            let bom = rng.chance(50);
            let context = format!("{encoding:?} case {case} bom={bom} {text:?}");
            let expected_bytes = [
                if bom { encoding.bom() } else { &[] },
                Encoder::new(encoding, false).encode_text(&text).unwrap().as_slice(),
            ]
            .concat();
            let streamed = encode_stream(encoding, bom, &text, &mut rng).unwrap();
            assert_eq!(streamed, expected_bytes, "{context}: streaming encoder differs");
            let spans = decode(encoding, &streamed, Some(&mut rng));
            check_spans(encoding, &streamed, &spans, &context);
            assert!(
                spans.iter().all(|span| span.opaque.is_none()),
                "{context}: opaque spans"
            );
            // Only the selected encoding's leading BOM is metadata; a BOM-less U+FEFF
            // first scalar is indistinguishable from it and is consumed by design.
            let expected = match text.strip_prefix('\u{feff}') {
                Some(rest) if !bom && !encoding.bom().is_empty() => rest,
                _ => text.as_str(),
            };
            assert_eq!(decoded_text(&spans), expected, "{context}: round trip");
        }
    }
}

#[test]
fn encoder_refuses_unrepresentable_and_opaque_input() {
    let mut rng = Rng(0xc0de_c003);
    assert!(matches!(
        encode_stream(Encoding::Latin1, false, "ok\u{100}", &mut rng),
        Err(CodecError::Unrepresentable)
    ));
    assert!(matches!(
        Encoder::new(Encoding::Windows1252, false).encode_text("中"),
        Err(CodecError::Unrepresentable)
    ));
    let mut sink = Bytes {
        bytes: Vec::new(),
        capacity: usize::MAX,
    };
    let opaque = [DecodedSpan {
        text: "\u{fffd}",
        original: RawOffset(0)..RawOffset(1),
        opaque_bytes: Some(&[0xff]),
    }];
    assert!(matches!(
        Encoder::new(Encoding::Utf8, false).push(&opaque, true, &mut sink),
        Err(CodecError::UnresolvedOpaqueBytes)
    ));
    assert!(sink.bytes.is_empty());
}

fn binary_warning(sample: &[u8]) -> bool {
    !sample.is_empty()
        && sample
            .iter()
            .filter(|&&byte| byte == 0 || (byte < 32 && !matches!(byte, 9 | 10 | 13)))
            .count()
            * 10
            > sample.len()
}
fn bom_owner(bytes: &[u8]) -> Option<Encoding> {
    [
        Encoding::Utf32Le,
        Encoding::Utf32Be,
        Encoding::Utf8,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
    ]
    .into_iter()
    .find(|encoding| bytes.starts_with(encoding.bom()))
}

#[test]
fn detection_is_total_and_consistent_with_its_evidence() {
    let mut rng = Rng(0xc0de_c004);
    for case in 0..800 {
        let bytes = match rng.below(4) {
            0 => rng.bytes(64),
            1 => {
                let text: String = (0..rng.below(24)).map(|_| *rng.pick(POOL)).collect();
                let mut bytes = text.into_bytes();
                // A truncated final scalar is still an optimistic UTF-8 sample.
                bytes.truncate(bytes.len().saturating_sub(rng.below(3)));
                bytes
            }
            2 => {
                let encoding = *rng.pick(&ENCODINGS[..5]);
                let pool = ['a', 'é', '中', '😀', '\r', '\n', 'あ', '한', '\u{feff}', '\u{fffd}'];
                let text: String = (0..rng.below(16)).map(|_| *rng.pick(&pool)).collect();
                [
                    encoding.bom(),
                    Encoder::new(encoding, false).encode_text(&text).unwrap().as_slice(),
                ]
                .concat()
            }
            _ => {
                // Legacy CJK samples exercise the bounded trial decoders.
                let encoding = *rng.pick(&[Encoding::ShiftJis, Encoding::EucJp, Encoding::EucKr, Encoding::Gbk]);
                let pool = ['あ', 'い', 'ア', 'カ', '한', '국', '中', 'a', ' '];
                let text: String = (0..rng.below(16)).map(|_| *rng.pick(&pool)).collect();
                let encoder = Encoder::new(encoding, false);
                text.chars()
                    .filter_map(|scalar| encoder.encode_text(&scalar.to_string()).ok())
                    .flatten()
                    .collect()
            }
        };
        let context = format!("case {case} {bytes:02x?}");
        let detection = detect(&bytes);
        if let Some(encoding) = bom_owner(&bytes) {
            assert_eq!(detection.encoding, encoding, "{context}");
            assert_eq!(detection.confidence, Confidence::Bom, "{context}");
            assert!(detection.bom && !detection.binary_warning, "{context}");
            continue;
        }
        assert!(!detection.bom, "{context}");
        if detection.confidence == Confidence::Utf16Sample {
            // BOM-less UTF-16 is recognised by its NUL pattern, which would
            // otherwise count as binary; detection never flags it as binary.
            assert!(!detection.binary_warning, "{context}");
            assert!(
                matches!(detection.encoding, Encoding::Utf16Le | Encoding::Utf16Be),
                "{context}"
            );
            continue;
        }
        assert_eq!(detection.binary_warning, binary_warning(&bytes), "{context}");
        let optimistic_utf8 = match std::str::from_utf8(&bytes) {
            Ok(_) => true,
            Err(error) => error.error_len().is_none(),
        };
        match detection.confidence {
            Confidence::Utf8Sample => {
                assert!(optimistic_utf8, "{context}");
                assert_eq!(detection.encoding, Encoding::Utf8, "{context}");
            }
            Confidence::LegacySample => {
                assert!(!optimistic_utf8, "{context}");
                assert!(
                    matches!(
                        detection.encoding,
                        Encoding::EucJp | Encoding::ShiftJis | Encoding::EucKr | Encoding::Gbk | Encoding::Big5
                    ),
                    "{context}"
                );
            }
            Confidence::LegacyFallback => {
                assert!(!optimistic_utf8, "{context}");
                assert_eq!(detection.encoding, Encoding::Windows1252, "{context}");
            }
            Confidence::Bom => panic!("{context}: BOM confidence without a BOM"),
            Confidence::Utf16Sample => unreachable!("{context}: handled above"),
        }
    }
}
