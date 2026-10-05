// SPDX-License-Identifier: MPL-2.0
//! Optional, untrusted private clipboard data travels with a plain-text fallback.
pub const MAX_CLIPBOARD_METADATA_BYTES: usize = 256 * 1024;
pub const RECTANGLE_CLIPBOARD_FORMAT: &str = "Bareline.Rectangle.v1";
pub const MULTISELECTION_CLIPBOARD_FORMAT: &str = "Bareline.Multiselection.v1";
/// Registered format Visual Studio and Scintilla editors (Notepad++) publish
/// beside column-block text. Only its presence matters; the payload is ignored.
pub const MSDEV_COLUMN_SELECT_FORMAT: &str = "MSDEVColumnSelect";
/// Registered format Borland-lineage IDEs and Scintilla publish beside block
/// text: one byte, [`BORLAND_COLUMN_BLOCK`] for a column block.
pub const BORLAND_BLOCK_TYPE_FORMAT: &str = "Borland IDE Block Type";
pub const BORLAND_COLUMN_BLOCK: u8 = 0x02;
/// Default ceiling for system clipboard text. The 4 MiB entry limit applies only
/// to clipboard-history admission, never to the system clipboard itself.
pub const DEFAULT_CLIPBOARD_MAX_BYTES: usize = 1 << 30;
/// Copies above this size still succeed but warn about their memory cost.
pub const CLIPBOARD_WARNING_BYTES: usize = 256 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardContents {
    pub text: String,
    /// Consumers must validate their payload version and its relation to `text`.
    pub metadata: Option<Vec<u8>>,
    /// Another editor marked `text` as a column block (see [`foreign_rectangle`]).
    pub rectangular: bool,
}

/// Markers published beside a rectangular copy, as (format, payload), so other
/// editors paste it as a column block too.
pub fn rectangle_interop_markers() -> [(&'static str, &'static [u8]); 2] {
    [
        (MSDEV_COLUMN_SELECT_FORMAT, &[0u8]),
        (BORLAND_BLOCK_TYPE_FORMAT, &[BORLAND_COLUMN_BLOCK]),
    ]
}
/// Whether another editor marked the clipboard text as a column block: the
/// `MSDEVColumnSelect` format is present, or the first byte of the
/// `Borland IDE Block Type` payload says column.
pub fn foreign_rectangle(msdev_column_select: bool, borland_block_type: Option<u8>) -> bool {
    msdev_column_select || borland_block_type == Some(BORLAND_COLUMN_BLOCK)
}
/// The rows of foreign column-block text. Visual Studio and Scintilla end every
/// row with a line break, the last one included; that final break does not
/// start another row. `\r\n`, `\n` and `\r` each end one row.
pub fn foreign_rectangle_rows(text: &str) -> (&str, usize) {
    let body = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix(['\n', '\r']))
        .unwrap_or(text);
    let bytes = body.as_bytes();
    let breaks = bytes
        .iter()
        .enumerate()
        .filter(|&(at, &byte)| byte == b'\n' || (byte == b'\r' && bytes.get(at + 1) != Some(&b'\n')))
        .count();
    (body, breaks + 1)
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
/// Decode clipboard UTF-16 up to its first NUL. Another application's data may
/// lack the terminator or contain unpaired surrogates; neither blocks paste.
/// `None` only when memory for the decoded text cannot be reserved, so a huge
/// paste fails readably instead of aborting the process.
pub fn decode_clipboard_text(units: &[u16]) -> Option<String> {
    let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
    let chars = || char::decode_utf16(units[..end].iter().copied()).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER));
    let mut text = String::new();
    text.try_reserve_exact(chars().map(char::len_utf8).sum()).ok()?;
    text.extend(chars());
    Some(text)
}
/// Advisory for a successful copy large enough to strain memory wherever it is pasted.
pub fn large_clipboard_warning(bytes: usize) -> Option<String> {
    (bytes > CLIPBOARD_WARNING_BYTES).then(|| {
        format!(
            "Copied {} to the clipboard. Text this large uses a lot of memory wherever it is pasted.",
            clipboard_size_label(bytes)
        )
    })
}
/// Readable size for clipboard limits and warnings.
pub fn clipboard_size_label(bytes: usize) -> String {
    const MIB: usize = 1 << 20;
    if bytes >= 1 << 30 && bytes.is_multiple_of(1 << 30) {
        format!("{} GiB", bytes >> 30)
    } else if bytes >= MIB {
        format!("{} MiB", bytes.div_ceil(MIB))
    } else {
        format!("{} KiB", bytes.div_ceil(1024))
    }
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
    #[test]
    fn foreign_clipboard_text_decodes_without_terminator_or_valid_surrogates() {
        let units: Vec<u16> = "tab\tend".encode_utf16().collect();
        assert_eq!(decode_clipboard_text(&units).as_deref(), Some("tab\tend"));
        let mut padded = units.clone();
        padded.extend_from_slice(&[0, 0x41, 0x42]);
        assert_eq!(decode_clipboard_text(&padded).as_deref(), Some("tab\tend"));
        assert_eq!(
            decode_clipboard_text(&[0x61, 0xd800, 0x62]).as_deref(),
            Some("a\u{fffd}b")
        );
        assert_eq!(decode_clipboard_text(&[0xd83e, 0xdd80]).as_deref(), Some("\u{1f980}"));
        assert_eq!(decode_clipboard_text(&[]).as_deref(), Some(""));
        assert_eq!(decode_clipboard_text(&[0, 0x61]).as_deref(), Some(""));
    }
    #[test]
    fn foreign_column_blocks_are_recognized_and_split_into_rows() {
        assert!(foreign_rectangle(true, None));
        assert!(foreign_rectangle(false, Some(BORLAND_COLUMN_BLOCK)));
        assert!(!foreign_rectangle(false, Some(0x01)));
        assert!(!foreign_rectangle(false, None));
        let markers = rectangle_interop_markers();
        assert_eq!(markers[0].0, "MSDEVColumnSelect");
        assert_eq!(markers[1], ("Borland IDE Block Type", &[0x02u8][..]));
        assert!(markers.iter().all(|(format, _)| !valid_clipboard_format(format)));
        assert_eq!(foreign_rectangle_rows("ab\r\ncd\r\n"), ("ab\r\ncd", 2));
        assert_eq!(foreign_rectangle_rows("ab\ncd"), ("ab\ncd", 2));
        assert_eq!(foreign_rectangle_rows("ab\rcd\r"), ("ab\rcd", 2));
        assert_eq!(foreign_rectangle_rows("x\r\n\r\ny\r\n"), ("x\r\n\r\ny", 3));
        assert_eq!(foreign_rectangle_rows("single"), ("single", 1));
        assert_eq!(foreign_rectangle_rows(""), ("", 1));
    }
    #[test]
    fn system_clipboard_limit_is_independent_of_history_entries() {
        assert_eq!(clipboard_size_label(DEFAULT_CLIPBOARD_MAX_BYTES), "1 GiB");
        assert_eq!(clipboard_size_label(CLIPBOARD_WARNING_BYTES), "256 MiB");
        assert_eq!(clipboard_size_label(6_000_000), "6 MiB");
        assert_eq!(clipboard_size_label(1), "1 KiB");
        assert_eq!(large_clipboard_warning(6_000_000), None);
        assert_eq!(large_clipboard_warning(CLIPBOARD_WARNING_BYTES), None);
        assert!(large_clipboard_warning(300 << 20).is_some_and(|warning| warning.contains("300 MiB")));
    }
}
