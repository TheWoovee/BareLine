// SPDX-License-Identifier: MPL-2.0
//! Optional, untrusted private clipboard data travels with a plain-text fallback.
pub const MAX_CLIPBOARD_METADATA_BYTES: usize = 256 * 1024;
pub const RECTANGLE_CLIPBOARD_FORMAT: &str = "Bareline.Rectangle.v1";
pub const MULTISELECTION_CLIPBOARD_FORMAT: &str = "Bareline.Multiselection.v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardContents {
    pub text: String,
    /// Consumers must validate their payload version and its relation to `text`.
    pub metadata: Option<Vec<u8>>,
}

pub fn valid_clipboard_format(format: &str) -> bool {
    matches!(format, RECTANGLE_CLIPBOARD_FORMAT | MULTISELECTION_CLIPBOARD_FORMAT)
}

/// Length envelope avoids exposing GlobalAlloc padding as application metadata.
pub fn encode_clipboard_metadata(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() > MAX_CLIPBOARD_METADATA_BYTES {
        return None;
    }
    let mut envelope = Vec::with_capacity(8 + bytes.len());
    envelope.extend_from_slice(b"BLM1");
    envelope.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    envelope.extend_from_slice(bytes);
    Some(envelope)
}
pub fn decode_clipboard_metadata(envelope: &[u8], max_bytes: usize) -> Option<Vec<u8>> {
    if envelope.len() < 8 || &envelope[..4] != b"BLM1" {
        return None;
    }
    let length = u32::from_le_bytes(envelope[4..8].try_into().ok()?) as usize;
    if length > max_bytes.min(MAX_CLIPBOARD_METADATA_BYTES) {
        return None;
    }
    Some(envelope.get(8..8usize.checked_add(length)?)?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn external_envelopes_reject_truncation_and_untrusted_lengths() {
        let mut data = encode_clipboard_metadata(b"opaque").unwrap();
        data.extend_from_slice(&[0; 8]);
        assert_eq!(decode_clipboard_metadata(&data, 6), Some(b"opaque".to_vec()));
        assert_eq!(decode_clipboard_metadata(&data, 5), None);
        assert_eq!(decode_clipboard_metadata(&data[..10], 100), None);
        data[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode_clipboard_metadata(&data, usize::MAX), None);
        assert!(!valid_clipboard_format("CF_UNICODETEXT"));
    }
}
