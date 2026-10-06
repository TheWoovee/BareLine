// SPDX-License-Identifier: MPL-2.0
//! Clipboard text and Bareline's private metadata on an `NSPasteboard`,
//! mirroring the Windows adapter's contract: a byte limit on text in both
//! directions, NUL-free text, an optional metadata envelope that never
//! travels without text, and "no text" answered as `None` rather than an error.
//!
//! The metadata goes under a reverse-DNS pasteboard type
//! ([`crate::types::pasteboard_type`]) next to the plain text, in the same
//! length envelope as on Windows. The Windows column-block markers
//! (`MSDEVColumnSelect`, `Borland IDE Block Type`) are not published or read:
//! no Mac editor uses them, so a foreign paste is never rectangular here.
//!
//! An `NSPasteboard` is safe to use from any thread; the seam uses it from
//! the UI thread like every other platform service.
use super::ns_string;
use crate::types::pasteboard_type;
use bareline_platform::clipboard::{
    ClipboardContents, MAX_CLIPBOARD_METADATA_BYTES, clipboard_size_label, decode_clipboard_metadata,
    encode_clipboard_metadata, valid_clipboard_format,
};
use objc2::rc::Retained;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::{MainThreadMarker, NSData};
use std::io;

/// Reads retried when another application changes the pasteboard between the
/// text and the metadata.
const CONSISTENT_READ_ATTEMPTS: u32 = 3;

fn over_limit(subject: &str, max_bytes: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::OutOfMemory,
        format!(
            "{subject} is larger than the {} clipboard limit.",
            clipboard_size_label(max_bytes)
        ),
    )
}
pub(crate) fn no_text() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "The clipboard does not contain text.")
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

pub struct MacClipboard {
    pasteboard: Retained<NSPasteboard>,
    /// A private pasteboard made by [`Self::private`], released on drop.
    private: bool,
}

impl MacClipboard {
    /// The general (system) pasteboard.
    pub fn new(_mtm: MainThreadMarker) -> Self {
        Self {
            // SAFETY: the shared general pasteboard; always available in a GUI session.
            pasteboard: unsafe { NSPasteboard::generalPasteboard() },
            private: false,
        }
    }
    /// A pasteboard of its own, which no other application reads or replaces:
    /// for tests and diagnostics that must leave the person's clipboard alone.
    pub fn private(_mtm: MainThreadMarker) -> Self {
        Self {
            // SAFETY: creates a uniquely named pasteboard released in `drop`.
            pasteboard: unsafe { NSPasteboard::pasteboardWithUniqueName() },
            private: true,
        }
    }
    /// Increments whenever any application replaces the contents.
    pub fn change_count(&self) -> isize {
        // SAFETY: a plain property read.
        unsafe { self.pasteboard.changeCount() }
    }

    /// The text, or `None` when the pasteboard holds none (or only empty
    /// text). Text over `max_bytes` UTF-8 bytes fails before it is copied
    /// into a Rust string.
    pub fn text_within(&self, max_bytes: usize) -> io::Result<Option<String>> {
        // SAFETY: reading the string flavor; the type is a framework constant.
        let Some(text) = (unsafe { self.pasteboard.stringForType(NSPasteboardTypeString) }) else {
            return Ok(None);
        };
        if text.len() > max_bytes {
            return Err(over_limit("The clipboard text", max_bytes));
        }
        let text = text.to_string();
        Ok((!text.is_empty()).then_some(text))
    }
    /// Like [`Self::text_within`], but no text is an error.
    pub fn text(&self, max_bytes: usize) -> io::Result<String> {
        self.text_within(max_bytes)?.ok_or_else(no_text)
    }

    pub fn set_text(&self, text: &str, max_bytes: usize) -> io::Result<()> {
        self.write(text, max_bytes, None)
    }

    /// Text plus private metadata for one of the formats the shared contract
    /// accepts. The metadata is optional: if it cannot be added, the text is
    /// still placed, as on Windows.
    pub fn set_text_with_metadata(&self, text: &str, max_bytes: usize, format: &str, bytes: &[u8]) -> io::Result<()> {
        let kind = metadata_type(format)?;
        let envelope =
            encode_clipboard_metadata(bytes).ok_or_else(|| invalid("The clipboard metadata is too large."))?;
        self.write(text, max_bytes, Some((kind, envelope)))
    }

    fn write(&self, text: &str, max_bytes: usize, metadata: Option<(&str, Vec<u8>)>) -> io::Result<()> {
        if text.len() > max_bytes {
            return Err(over_limit("The text", max_bytes));
        }
        if text.contains('\0') {
            // Other applications read pasteboard text as C strings and would
            // silently lose everything after the NUL, as on Windows.
            return Err(invalid(
                "Text containing NUL characters cannot be copied to the clipboard.",
            ));
        }
        // Prepare every object before the person's clipboard is cleared.
        let string = ns_string(text);
        let extra = metadata.map(|(kind, envelope)| (ns_string(kind), NSData::from_vec(envelope)));
        // SAFETY: clearing and writing flavors of a live pasteboard; the type
        // names are framework constants or our own reverse-DNS strings.
        unsafe {
            self.pasteboard.clearContents();
            if !self.pasteboard.setString_forType(&string, NSPasteboardTypeString) {
                return Err(io::Error::other("The text could not be placed on the clipboard."));
            }
            if let Some((kind, data)) = extra {
                // Private metadata is optional: a rejected flavor leaves valid text.
                let _ = self.pasteboard.setData_forType(Some(&data), &kind);
            }
        }
        Ok(())
    }

    /// The metadata for `format`, if the pasteboard holds a valid envelope
    /// within `max_bytes`.
    pub fn metadata(&self, format: &str, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
        let kind = metadata_type(format)?;
        Ok(self.metadata_of(kind, max_bytes))
    }

    fn metadata_of(&self, kind: &str, max_bytes: usize) -> Option<Vec<u8>> {
        // SAFETY: reading one data flavor of a live pasteboard.
        let data = unsafe { self.pasteboard.dataForType(&ns_string(kind)) }?;
        let bytes = data.bytes();
        // Bound the whole payload before decoding, then the declared length.
        if !(8..=MAX_CLIPBOARD_METADATA_BYTES + 64).contains(&bytes.len()) {
            return None;
        }
        decode_clipboard_metadata(bytes, max_bytes)
    }

    /// Text and metadata that belong together: both are read again if another
    /// application replaced the pasteboard in between, and the metadata is
    /// dropped (the text kept) if it keeps changing. `None` when there is no text.
    pub fn text_with_metadata(
        &self,
        text_limit: usize,
        format: &str,
        max_bytes: usize,
    ) -> io::Result<Option<ClipboardContents>> {
        let kind = metadata_type(format)?;
        for _ in 0..CONSISTENT_READ_ATTEMPTS {
            let before = self.change_count();
            let Some(text) = self.text_within(text_limit)? else {
                return Ok(None);
            };
            let metadata = self.metadata_of(kind, max_bytes);
            if self.change_count() == before {
                return Ok(Some(ClipboardContents {
                    text,
                    metadata,
                    rectangular: false,
                }));
            }
        }
        Ok(self.text_within(text_limit)?.map(|text| ClipboardContents {
            text,
            metadata: None,
            rectangular: false,
        }))
    }
}

fn metadata_type(format: &str) -> io::Result<&'static str> {
    if !valid_clipboard_format(format) {
        return Err(invalid("Unknown clipboard format."));
    }
    pasteboard_type(format).ok_or_else(|| invalid("Unknown clipboard format."))
}

impl Drop for MacClipboard {
    fn drop(&mut self) {
        if self.private {
            // SAFETY: releases only the uniquely named pasteboard this value made.
            let _: () = unsafe { objc2::msg_send![&self.pasteboard, releaseGlobally] };
        }
    }
}
