// SPDX-License-Identifier: MPL-2.0
//! The Wayland clipboard through a data-control protocol (`wl-clipboard-rs`),
//! which lets a client read and own the selection without keyboard focus.
//! Copies are served from a thread the crate starts for each copy, which ends
//! when another copy replaces the selection; pastes read the other client's pipe
//! with a deadline, so a stalled source cannot hang the caller.
use super::{TEXT_PLAIN_UTF8, TRANSFER_TIMEOUT, busy, decode_text, over_limit};
use bareline_platform::clipboard::MAX_CLIPBOARD_METADATA_BYTES;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::{
    collections::HashSet,
    io::{self, Read},
    os::fd::AsFd,
    time::Instant,
};
use wl_clipboard_rs::{copy, paste};

fn wayland_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("Wayland clipboard: {error}"))
}
/// Reads at most `limit` bytes before the deadline; more is `OutOfMemory`.
fn read_bounded(mut pipe: impl Read + AsFd, limit: usize) -> io::Result<Vec<u8>> {
    rustix::io::ioctl_fionbio(&pipe, true)?;
    let deadline = Instant::now() + TRANSFER_TIMEOUT;
    let mut data = Vec::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(data),
            Ok(count) => {
                if data.len() + count > limit {
                    return Err(io::Error::new(io::ErrorKind::OutOfMemory, "clipboard data too large"));
                }
                data.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(busy());
                }
                let timeout = Timespec {
                    tv_sec: remaining.as_secs() as i64,
                    tv_nsec: i64::from(remaining.subsec_nanos()),
                };
                let mut fds = [PollFd::new(&pipe, PollFlags::IN)];
                match poll(&mut fds, Some(&timeout)) {
                    Ok(_) | Err(rustix::io::Errno::INTR) => {}
                    Err(errno) => return Err(errno.into()),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}
/// An empty clipboard or a seat without one is "no text", not a failure.
fn empty(error: &paste::Error) -> bool {
    matches!(
        error,
        paste::Error::ClipboardEmpty | paste::Error::NoSeats | paste::Error::NoMimeType
    )
}

pub(super) struct WaylandClipboard;
impl WaylandClipboard {
    /// `Unsupported` when the compositor offers no data-control protocol.
    pub fn probe() -> io::Result<Self> {
        match paste::get_mime_types(paste::ClipboardType::Regular, paste::Seat::Unspecified) {
            Ok(_) => Ok(Self),
            Err(error) if empty(&error) => Ok(Self),
            Err(paste::Error::MissingProtocol { name, .. }) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("the compositor does not offer {name}"),
            )),
            Err(error) => Err(wayland_error(error)),
        }
    }
    pub fn write(&self, text: &str, metadata: Option<(&str, Vec<u8>)>) -> io::Result<()> {
        let mut sources = vec![copy::MimeSource {
            source: copy::Source::Bytes(text.as_bytes().into()),
            mime_type: copy::MimeType::Text,
        }];
        if let Some((mime, envelope)) = metadata {
            sources.push(copy::MimeSource {
                source: copy::Source::Bytes(envelope.into()),
                mime_type: copy::MimeType::Specific(mime.to_owned()),
            });
        }
        let mut options = copy::Options::new();
        // Served from the crate's own thread until another copy replaces it.
        options.clipboard(copy::ClipboardType::Regular).foreground(false);
        options.copy_multi(sources).map_err(wayland_error)
    }
    fn types(&self) -> io::Result<Option<HashSet<String>>> {
        match paste::get_mime_types(paste::ClipboardType::Regular, paste::Seat::Unspecified) {
            Ok(types) => Ok(Some(types)),
            Err(error) if empty(&error) => Ok(None),
            Err(error) => Err(wayland_error(error)),
        }
    }
    fn contents(&self, mime: paste::MimeType<'_>, limit: usize) -> io::Result<Option<Vec<u8>>> {
        match paste::get_contents(paste::ClipboardType::Regular, paste::Seat::Unspecified, mime) {
            Ok((pipe, _)) => read_bounded(pipe, limit).map(Some),
            Err(error) if empty(&error) => Ok(None),
            Err(error) => Err(wayland_error(error)),
        }
    }
    fn text(&self, types: &HashSet<String>, max_bytes: usize) -> io::Result<Option<String>> {
        // Prefer the explicitly UTF-8 type; otherwise let the crate pick a text type.
        let mime = if types.contains(TEXT_PLAIN_UTF8) {
            paste::MimeType::Specific(TEXT_PLAIN_UTF8)
        } else {
            paste::MimeType::Text
        };
        match self.contents(mime, max_bytes) {
            Ok(Some(bytes)) => decode_text(bytes, max_bytes),
            Ok(None) => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::OutOfMemory => {
                Err(over_limit("The clipboard text", max_bytes))
            }
            Err(error) => Err(error),
        }
    }
    fn envelope(&self, types: &HashSet<String>, mime: &str) -> io::Result<Option<Vec<u8>>> {
        if !types.contains(mime) {
            return Ok(None);
        }
        match self.contents(paste::MimeType::Specific(mime), MAX_CLIPBOARD_METADATA_BYTES + 64) {
            Err(error) if error.kind() == io::ErrorKind::OutOfMemory => Ok(None),
            other => other,
        }
    }
    pub fn read_text(&self, max_bytes: usize) -> io::Result<Option<String>> {
        match self.types()? {
            Some(types) => self.text(&types, max_bytes),
            None => Ok(None),
        }
    }
    pub fn read_metadata(&self, mime: &str) -> io::Result<Option<Vec<u8>>> {
        match self.types()? {
            Some(types) => self.envelope(&types, mime),
            None => Ok(None),
        }
    }
    /// Text and metadata of one offer: the type list is read once and both reads
    /// must find the same list afterwards, or the read is retried.
    pub fn read_with_metadata(&self, text_limit: usize, mime: &str) -> io::Result<Option<(String, Option<Vec<u8>>)>> {
        for _ in 0..3 {
            let Some(types) = self.types()? else {
                return Ok(None);
            };
            let Some(text) = self.text(&types, text_limit)? else {
                return Ok(None);
            };
            let metadata = self.envelope(&types, mime)?;
            if self.types()?.as_ref() == Some(&types) {
                return Ok(Some((text, metadata)));
            }
        }
        Err(busy())
    }
}
