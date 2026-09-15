// SPDX-License-Identifier: MPL-2.0
//! Streaming codec contracts. Production codec implementations arrive in PR-007.
pub mod cancellation;
pub mod codecs;
pub mod lifecycle;
pub mod owned_cache;
mod owned_read;
pub mod owned_store;
pub mod paged_recovery;
pub mod paged_service;
pub mod profile_migration;
pub mod recovery;
pub mod recovery_retirement;
pub mod resident_recovery;
pub mod session;
pub mod source;
use std::ops::Range;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextOffset(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawOffset(pub u64);
#[derive(Clone, Copy, Debug)]
pub enum BomPolicy {
    Preserve,
    Emit,
    Omit,
}
#[derive(Clone, Debug)]
pub struct DecodedSpan<'a> {
    pub text: &'a str,
    pub original: Range<RawOffset>,
    /// Invalid spans display U+FFFD but retain exact original bytes outside text.
    pub opaque_bytes: Option<&'a [u8]>,
}
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub consumed: usize,
    pub produced: usize,
    pub needs_output: bool,
}
#[derive(Debug)]
pub enum CodecError {
    InvalidSequence,
    Unrepresentable,
    UnresolvedOpaqueBytes,
    Output(std::io::Error),
}
pub trait DecodedSink {
    fn remaining_capacity(&self) -> usize;
    fn write(&mut self, span: DecodedSpan<'_>) -> Result<(), CodecError>;
}
pub trait ByteSink {
    fn remaining_capacity(&self) -> usize;
    fn write(&mut self, bytes: &[u8]) -> Result<(), CodecError>;
}
pub trait StreamingDecoder {
    /// Consume only the input fitting the sink; retain incomplete code units across pushes.
    fn push(&mut self, raw: &[u8], end: bool, out: &mut dyn DecodedSink) -> Result<Progress, CodecError>;
}
pub trait StreamingEncoder {
    fn push(&mut self, spans: &[DecodedSpan<'_>], end: bool, out: &mut dyn ByteSink) -> Result<Progress, CodecError>;
}
pub trait Codec {
    fn bom_policy(&self) -> BomPolicy;
    fn decoder(&self) -> Box<dyn StreamingDecoder>;
    fn encoder(&self, bom: BomPolicy) -> Box<dyn StreamingEncoder>;
}

pub mod tail;
