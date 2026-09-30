// SPDX-License-Identifier: MPL-2.0
//! Encoding detection is total and its verdict is consistent with the sampled evidence;
//! decoding with the detected encoding tiles the input.
#![no_main]
use bareline_file_io::{
    CodecError, DecodedSink, DecodedSpan, StreamingDecoder,
    codecs::{Confidence, Decoder, Encoding, detect},
};
use libfuzzer_sys::fuzz_target;

struct Tiling {
    next: u64,
}
impl DecodedSink for Tiling {
    fn remaining_capacity(&self) -> usize {
        usize::MAX
    }
    fn write(&mut self, span: DecodedSpan<'_>) -> Result<(), CodecError> {
        assert_eq!(span.original.start.0, self.next, "span gap");
        assert!(span.original.end.0 > span.original.start.0, "empty span");
        self.next = span.original.end.0;
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    let detection = detect(data);
    let sample = &data[..data.len().min(65536)];
    let bom_owner = [
        Encoding::Utf32Le,
        Encoding::Utf32Be,
        Encoding::Utf8,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
    ]
    .into_iter()
    .find(|encoding| data.starts_with(encoding.bom()));
    let optimistic_utf8 = match std::str::from_utf8(sample) {
        Ok(_) => true,
        Err(error) => error.error_len().is_none(),
    };
    match detection.confidence {
        Confidence::Bom => {
            assert_eq!(Some(detection.encoding), bom_owner);
            assert!(detection.bom && !detection.binary_warning);
        }
        Confidence::Utf8Sample => {
            assert!(bom_owner.is_none() && optimistic_utf8 && detection.encoding == Encoding::Utf8);
        }
        Confidence::LegacySample => {
            assert!(bom_owner.is_none() && !optimistic_utf8);
            assert!(matches!(
                detection.encoding,
                Encoding::EucJp | Encoding::ShiftJis | Encoding::EucKr
            ));
        }
        Confidence::LegacyFallback => {
            assert!(bom_owner.is_none() && !optimistic_utf8 && detection.encoding == Encoding::Windows1252);
        }
    }
    let mut tiling = Tiling { next: 0 };
    let progress = Decoder::new(detection.encoding)
        .push(data, true, &mut tiling)
        .expect("unbounded sink");
    assert_eq!(progress.consumed, data.len());
    assert_eq!(tiling.next, data.len() as u64, "spans do not cover the input");
});
