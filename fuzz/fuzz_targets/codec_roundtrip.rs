// SPDX-License-Identifier: MPL-2.0
//! Streaming decode of arbitrary bytes is independent of chunking and sink
//! backpressure, spans tile the input, and exact Unicode/Latin-1 text round-trips
//! through the encoder. Input: [encoding][control length][control bytes][payload].
#![no_main]
use bareline_file_io::{
    CodecError, DecodedSink, DecodedSpan, StreamingDecoder,
    codecs::{Decoder, Encoder, Encoding},
};
use libfuzzer_sys::fuzz_target;

const ENCODINGS: [Encoding; 20] = [
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

#[derive(Debug, PartialEq)]
struct Span {
    text: String,
    start: u64,
    end: u64,
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
            start: span.original.start.0,
            end: span.original.end.0,
            opaque: span.opaque_bytes.map(<[u8]>::to_vec),
        });
        Ok(())
    }
}

/// Adjacent valid spans joined: bulk decoding groups whole valid units per push,
/// so only this form is independent of chunking.
fn merged(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for span in spans {
        if let Some(last) = out.last_mut()
            && last.opaque.is_none()
            && span.opaque.is_none()
            && !last.text.is_empty()
            && !span.text.is_empty()
            && last.end == span.start
        {
            last.text.push_str(&span.text);
            last.end = span.end;
            continue;
        }
        out.push(span);
    }
    out
}

/// Each control byte picks a chunk length (low nibble % 6) and the sink capacity
/// granted after backpressure (high nibble, at least one maximal unit).
fn decode(encoding: Encoding, bytes: &[u8], control: &[u8]) -> Vec<Span> {
    let mut decoder = Decoder::new(encoding);
    let mut steps = control.iter().copied();
    let mut sink = Spans {
        spans: Vec::new(),
        capacity: if control.is_empty() { usize::MAX } else { 0 },
    };
    let mut offset = 0;
    loop {
        let end = match steps.next() {
            Some(step) => (offset + usize::from(step & 0x0f) % 6).min(bytes.len()),
            None => bytes.len(),
        };
        let last = end == bytes.len();
        let mut chunk = &bytes[offset..end];
        for attempt in 0.. {
            assert!(attempt < 64, "decoder made no progress");
            let progress = decoder.push(chunk, last, &mut sink).expect("sink never fails");
            chunk = &chunk[progress.consumed..];
            if !progress.needs_output {
                break;
            }
            sink.capacity = 4 + usize::from(steps.next().unwrap_or(0xf0) >> 4);
        }
        assert!(chunk.is_empty(), "input left unconsumed without backpressure");
        offset = end;
        if last {
            return sink.spans;
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let [selector, control_len, rest @ ..] = data else {
        return;
    };
    let encoding = ENCODINGS[usize::from(*selector) % ENCODINGS.len()];
    let (control, bytes) = rest.split_at(rest.len().min(usize::from(*control_len % 64)));
    let whole = decode(encoding, bytes, &[]);
    let mut cursor = 0;
    for (index, span) in whole.iter().enumerate() {
        assert_eq!(span.start, cursor, "span gap");
        assert!(span.end > span.start, "empty span");
        cursor = span.end;
        let raw = &bytes[span.start as usize..span.end as usize];
        match &span.opaque {
            Some(opaque) => assert_eq!((opaque.as_slice(), span.text.as_str()), (raw, "\u{fffd}")),
            None if span.text.is_empty() => assert!(index == 0 && raw == encoding.bom(), "misplaced BOM span"),
            None => {}
        }
    }
    assert_eq!(cursor, bytes.len() as u64, "spans do not cover the input");
    assert_eq!(
        merged(decode(encoding, bytes, control)),
        merged(decode(encoding, bytes, &[])),
        "chunked decode differs"
    );

    let exact = matches!(
        encoding,
        Encoding::Utf8
            | Encoding::Utf16Le
            | Encoding::Utf16Be
            | Encoding::Utf32Le
            | Encoding::Utf32Be
            | Encoding::Latin1
    );
    if !exact {
        return;
    }
    let bom = whole
        .first()
        .is_some_and(|span| span.text.is_empty() && span.opaque.is_none());
    let text: String = whole
        .iter()
        .filter(|span| span.opaque.is_none())
        .map(|span| span.text.as_str())
        .collect();
    let encoder = Encoder::new(encoding, false);
    for span in whole
        .iter()
        .filter(|span| span.opaque.is_none() && !span.text.is_empty())
    {
        let raw = &bytes[span.start as usize..span.end as usize];
        assert_eq!(
            encoder.encode_text(&span.text).expect("decoded scalar"),
            raw,
            "span re-encoding"
        );
    }
    let prefix: &[u8] = if bom { encoding.bom() } else { &[] };
    let encoded = [prefix, encoder.encode_text(&text).expect("decoded text").as_slice()].concat();
    let again = decode(encoding, &encoded, control);
    assert!(
        again.iter().all(|span| span.opaque.is_none()),
        "valid text decoded as opaque"
    );
    let decoded: String = again.iter().map(|span| span.text.as_str()).collect();
    // A BOM-less leading U+FEFF is indistinguishable from the encoding's BOM.
    let expected = match text.strip_prefix('\u{feff}') {
        Some(rest) if !bom && !encoding.bom().is_empty() => rest,
        _ => text.as_str(),
    };
    assert_eq!(decoded, expected, "round trip");
});
