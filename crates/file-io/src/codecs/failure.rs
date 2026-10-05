// SPDX-License-Identifier: MPL-2.0
use super::Encoding;
use bareline_document::TextOffset;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodingFailure {
    pub revision: u64,
    pub range: std::ops::Range<TextOffset>,
    pub reason: String,
    identity: u64,
}
impl EncodingFailure {
    pub fn new(identity: (u64, u64), range: std::ops::Range<usize>, reason: impl Into<String>) -> Self {
        Self {
            identity: identity.0,
            revision: identity.1,
            range: TextOffset(range.start)..TextOffset(range.end),
            reason: reason.into(),
        }
    }
    pub fn same_document(&self, identity: (u64, u64)) -> bool {
        self.identity == identity.0
    }
    pub fn matches(&self, identity: (u64, u64)) -> bool {
        self.identity == identity.0 && self.revision == identity.1
    }
}
/// Only used after the bounded chunk encoder rejects text. Supported encoders
/// are stateless, so the first rejected scalar identifies the exact UTF-8 span.
pub(crate) fn rejected_range(text: &str, target: Encoding, base: usize) -> std::ops::Range<usize> {
    let encoder = super::Encoder::new(target, false);
    for (offset, scalar) in text.char_indices() {
        let mut bytes = [0; 4];
        if encoder.encode_text(scalar.encode_utf8(&mut bytes)).is_err() {
            return base + offset..base + offset + scalar.len_utf8();
        }
    }
    base..base + text.len()
}

pub(crate) fn bounded_chunks(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start == text.len() {
            return None;
        }
        let mut end = (start + 64 * 1024).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let result = (start, &text[start..end]);
        start = end;
        Some(result)
    })
}
