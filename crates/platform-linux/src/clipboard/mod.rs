// SPDX-License-Identifier: MPL-2.0
//! The system clipboard on Linux, with the same contract as the Windows adapter:
//! text within a byte limit, plus Bareline's optional private metadata, which
//! travels as a private MIME type beside `text/plain;charset=utf-8` in the same
//! selection and is read back only together with that text.
//!
//! Two backends serve it:
//! - **Wayland data control** (`wl-clipboard-rs`) when `WAYLAND_DISPLAY` names a
//!   compositor that offers `ext-data-control` or `wlr-data-control` (wlroots
//!   compositors, KDE). Copies are served from a thread the crate starts.
//! - **X11** (`x11rb`) when `DISPLAY` is set, including XWayland on Wayland
//!   sessions whose compositor offers no data-control protocol (GNOME, Weston and
//!   WSLg): the compositor bridges the X11 `CLIPBOARD` to Wayland clients. The
//!   selection owner runs on its own thread, so serving another application's
//!   paste never waits on (or blocks) the UI thread.
//!
//! Column-block markers of Windows editors (`MSDEVColumnSelect`) have no Linux
//! counterpart, so `rectangular` is always false for foreign text here.
mod wayland;
mod x11;

use bareline_platform::clipboard::{
    ClipboardContents, MULTISELECTION_CLIPBOARD_FORMAT, RECTANGLE_CLIPBOARD_FORMAT, clipboard_size_label,
    decode_clipboard_metadata, encode_clipboard_metadata,
};
use std::{io, time::Duration};

/// How long a paste waits for another application to deliver its data.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(3);
/// The MIME type of plain UTF-8 text.
const TEXT_PLAIN_UTF8: &str = "text/plain;charset=utf-8";

/// Which system clipboard a [`LinuxClipboard`] talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardBackend {
    WaylandDataControl,
    X11,
}

/// The private MIME type that carries one metadata format.
pub fn metadata_mime_type(format: &str) -> Option<&'static str> {
    match format {
        RECTANGLE_CLIPBOARD_FORMAT => Some("application/x-bareline-rectangle-v1"),
        MULTISELECTION_CLIPBOARD_FORMAT => Some("application/x-bareline-multiselection-v1"),
        _ => None,
    }
}
fn metadata_mime(format: &str) -> io::Result<&'static str> {
    metadata_mime_type(format).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unknown clipboard format"))
}
fn over_limit(subject: &str, max_bytes: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::OutOfMemory,
        format!(
            "{subject} is larger than the {} clipboard limit.",
            clipboard_size_label(max_bytes)
        ),
    )
}
fn busy() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "Another application is using the clipboard. Try again.",
    )
}
/// The checks every copy makes before replacing the user's clipboard.
fn check_text(text: &str, max_bytes: usize) -> io::Result<()> {
    if text.len() > max_bytes {
        return Err(over_limit("The text", max_bytes));
    }
    if text.contains('\0') {
        // Windows text ends at the first NUL, and so do many Linux readers; refuse
        // rather than let another application silently lose the rest.
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Text containing NUL characters cannot be copied to the clipboard.",
        ));
    }
    Ok(())
}
/// Clipboard bytes as text, up to the first NUL as on Windows. Invalid UTF-8
/// becomes U+FFFD rather than blocking the paste.
fn decode_text(mut bytes: Vec<u8>, max_bytes: usize) -> io::Result<Option<String>> {
    if let Some(end) = bytes.iter().position(|byte| *byte == 0) {
        bytes.truncate(end);
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    };
    if text.len() > max_bytes {
        return Err(over_limit("The clipboard text", max_bytes));
    }
    Ok((!text.is_empty()).then_some(text))
}
/// Latin-1 (`STRING`) bytes as text.
fn decode_latin1(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|byte| char::from(*byte))
        .collect::<String>()
        .into_bytes()
}

enum Backend {
    Wayland(wayland::WaylandClipboard),
    X11(Box<x11::X11Clipboard>),
}
pub struct LinuxClipboard {
    backend: Backend,
}
impl LinuxClipboard {
    /// The session's clipboard: Wayland data control when the compositor offers
    /// it, otherwise X11 (XWayland on Wayland sessions). `Unsupported` when the
    /// session has neither.
    pub fn new() -> io::Result<Self> {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|name| !name.is_empty());
        let mut reason = None;
        if wayland {
            match wayland::WaylandClipboard::probe() {
                Ok(clipboard) => {
                    return Ok(Self {
                        backend: Backend::Wayland(clipboard),
                    });
                }
                Err(error) => reason = Some(error),
            }
        }
        if std::env::var_os("DISPLAY").is_some_and(|name| !name.is_empty()) {
            return Self::x11();
        }
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            match reason {
                Some(error) => {
                    format!("This system does not support the clipboard here ({error}) and no X11 display is available")
                }
                None => "This system does not support the clipboard without a Wayland or X11 display".into(),
            },
        ))
    }
    /// The X11 clipboard of `DISPLAY`.
    pub fn x11() -> io::Result<Self> {
        Ok(Self {
            backend: Backend::X11(Box::new(x11::X11Clipboard::connect()?)),
        })
    }
    /// The Wayland data-control clipboard of `WAYLAND_DISPLAY`; `Unsupported`
    /// when the compositor does not offer it.
    pub fn wayland() -> io::Result<Self> {
        Ok(Self {
            backend: Backend::Wayland(wayland::WaylandClipboard::probe()?),
        })
    }
    pub fn backend(&self) -> ClipboardBackend {
        match self.backend {
            Backend::Wayland(_) => ClipboardBackend::WaylandDataControl,
            Backend::X11(_) => ClipboardBackend::X11,
        }
    }
    /// `None` when the clipboard holds no text, so paste is a no-op rather than an error.
    pub fn read(&self, max_bytes: usize) -> io::Result<Option<String>> {
        match &self.backend {
            Backend::Wayland(clipboard) => clipboard.read_text(max_bytes),
            Backend::X11(clipboard) => clipboard.read_text(max_bytes),
        }
    }
    pub fn write(&self, text: &str, max_bytes: usize) -> io::Result<()> {
        check_text(text, max_bytes)?;
        match &self.backend {
            Backend::Wayland(clipboard) => clipboard.write(text, None),
            Backend::X11(clipboard) => clipboard.write(text, None),
        }
    }
    /// Text plus private metadata in one selection. A backend that rejects the
    /// extra type still publishes the text, as the trait allows.
    pub fn write_with_metadata(&self, text: &str, max_bytes: usize, format: &str, bytes: &[u8]) -> io::Result<()> {
        let envelope = encode_clipboard_metadata(bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "clipboard metadata too large"))?;
        let mime = metadata_mime(format)?;
        check_text(text, max_bytes)?;
        match &self.backend {
            Backend::Wayland(clipboard) => clipboard.write(text, Some((mime, envelope))),
            Backend::X11(clipboard) => clipboard.write(text, Some((mime, envelope))),
        }
    }
    pub fn metadata(&self, format: &str, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
        let mime = metadata_mime(format)?;
        let envelope = match &self.backend {
            Backend::Wayland(clipboard) => clipboard.read_metadata(mime),
            Backend::X11(clipboard) => clipboard.read_metadata(mime),
        }?;
        Ok(envelope.and_then(|envelope| decode_clipboard_metadata(&envelope, max_bytes)))
    }
    /// `None` when the clipboard holds no text; metadata never travels without text.
    pub fn read_with_metadata(
        &self,
        text_limit: usize,
        format: &str,
        max_bytes: usize,
    ) -> io::Result<Option<ClipboardContents>> {
        let mime = metadata_mime(format)?;
        let read = match &self.backend {
            Backend::Wayland(clipboard) => clipboard.read_with_metadata(text_limit, mime),
            Backend::X11(clipboard) => clipboard.read_with_metadata(text_limit, mime),
        }?;
        Ok(read.map(|(text, envelope)| ClipboardContents {
            text,
            metadata: envelope.and_then(|envelope| decode_clipboard_metadata(&envelope, max_bytes)),
            rectangular: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::clipboard::{MAX_CLIPBOARD_METADATA_BYTES, valid_clipboard_format};

    #[test]
    fn every_metadata_format_has_its_own_private_type() {
        for format in [RECTANGLE_CLIPBOARD_FORMAT, MULTISELECTION_CLIPBOARD_FORMAT] {
            assert!(valid_clipboard_format(format));
            let mime = metadata_mime_type(format).unwrap();
            assert!(mime.starts_with("application/x-bareline-"), "{mime}");
        }
        assert_ne!(
            metadata_mime_type(RECTANGLE_CLIPBOARD_FORMAT),
            metadata_mime_type(MULTISELECTION_CLIPBOARD_FORMAT)
        );
        assert_eq!(
            metadata_mime("CF_UNICODETEXT").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
    #[test]
    fn copies_refuse_oversized_and_nul_text_before_touching_the_clipboard() {
        assert!(check_text("abc", 3).is_ok());
        let error = check_text("abcd", 3).unwrap_err();
        assert!(error.to_string().contains("clipboard limit"), "{error}");
        assert!(check_text("a\0b", 16).is_err());
        assert!(encode_clipboard_metadata(&vec![0; MAX_CLIPBOARD_METADATA_BYTES + 1]).is_none());
    }
    #[test]
    fn foreign_text_ends_at_nul_tolerates_bad_utf8_and_respects_the_limit() {
        assert_eq!(
            decode_text(b"tab\tend\0junk".to_vec(), 64).unwrap().as_deref(),
            Some("tab\tend")
        );
        assert_eq!(
            decode_text(b"a\xffb".to_vec(), 64).unwrap().as_deref(),
            Some("a\u{fffd}b")
        );
        assert_eq!(decode_text(Vec::new(), 64).unwrap(), None);
        assert_eq!(decode_text(b"\0rest".to_vec(), 64).unwrap(), None);
        // Small text in an over-allocated buffer fits a small limit; long text does not.
        assert_eq!(
            decode_text(b"find me\0\0\0\0\0\0\0\0\0\0".to_vec(), 7)
                .unwrap()
                .as_deref(),
            Some("find me")
        );
        assert!(decode_text(b"too long".to_vec(), 3).is_err());
        assert_eq!(decode_latin1(b"caf\xe9"), "caf\u{e9}".as_bytes());
    }
}
