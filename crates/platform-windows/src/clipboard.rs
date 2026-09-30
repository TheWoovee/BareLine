// SPDX-License-Identifier: MPL-2.0
use bareline_platform::clipboard::{
    ClipboardContents, MAX_CLIPBOARD_METADATA_BYTES, clipboard_size_label, decode_clipboard_metadata,
    decode_clipboard_text, encode_clipboard_metadata, valid_clipboard_format,
};
use std::time::Duration;
use windows::{
    Win32::{
        Foundation::*,
        System::{DataExchange::*, Memory::*},
    },
    core::{Error, Result},
};
/// CF_UNICODETEXT.
const UNICODE_TEXT: u32 = 13;
/// Clipboard managers and other applications hold the clipboard only briefly.
const OPEN_ATTEMPTS: u32 = 10;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(15);
struct Open;
impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}
/// Retry a transient failure, returning the last error once attempts run out.
fn retry<T>(attempts: u32, delay: Duration, mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
    let mut remaining = attempts.max(1);
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error) if remaining == 1 => return Err(error),
            Err(_) => {
                remaining -= 1;
                std::thread::sleep(delay);
            }
        }
    }
}
fn open(hwnd: HWND) -> Result<Open> {
    // SAFETY: OpenClipboard has no memory preconditions; `Open` closes it exactly once.
    retry(OPEN_ATTEMPTS, OPEN_RETRY_DELAY, || unsafe { OpenClipboard(Some(hwnd)) })
        .map(|()| Open)
        .map_err(|_| {
            Error::new(
                CLIPBRD_E_CANT_OPEN,
                "Another application is using the clipboard. Try again.",
            )
        })
}
fn over_limit(subject: &str, max_bytes: usize) -> Error {
    Error::new(
        E_OUTOFMEMORY,
        format!(
            "{subject} is larger than the {} clipboard limit.",
            clipboard_size_label(max_bytes)
        ),
    )
}
pub(crate) fn no_text() -> Error {
    Error::new(CLIPBRD_E_BAD_DATA, "The clipboard does not contain text.")
}
fn text_available() -> bool {
    // SAFETY: a format query needs no clipboard ownership.
    unsafe { IsClipboardFormatAvailable(UNICODE_TEXT).is_ok() }
}
/// `None` when the clipboard holds no text, so paste is a no-op rather than an error.
pub fn read(hwnd: HWND, max_bytes: usize) -> Result<Option<String>> {
    // An empty or non-text clipboard is answered without contending for ownership.
    if !text_available() {
        return Ok(None);
    }
    let _open = open(hwnd)?;
    // SAFETY: the clipboard stays open until `_open` drops after the read.
    unsafe { read_text_open(max_bytes) }
}
/// Caller owns the clipboard until all global-memory reads have completed.
unsafe fn read_text_open(max_bytes: usize) -> Result<Option<String>> {
    // The contents may have changed between the unowned check and opening.
    if !text_available() {
        return Ok(None);
    }
    unsafe {
        let handle = GetClipboardData(UNICODE_TEXT)
            .map_err(|_| Error::new(CLIPBRD_E_BAD_DATA, "The clipboard text could not be read."))?;
        read_text_global(HGLOBAL(handle.0), max_bytes)
    }
}
/// `handle` must be a live global allocation that nothing frees during this call.
unsafe fn read_text_global(handle: HGLOBAL, max_bytes: usize) -> Result<Option<String>> {
    unsafe {
        let units = GlobalSize(handle) / 2;
        // Text within `max_bytes` UTF-8 bytes never needs more UTF-16 units, so the
        // bound round-trips; allow for the terminator and allocation padding.
        if units > max_bytes.saturating_add(16) {
            return Err(over_limit("The clipboard text", max_bytes));
        }
        if units == 0 {
            return Ok(None);
        }
        let pointer = GlobalLock(handle);
        if pointer.is_null() {
            return Err(Error::from_thread());
        }
        let text = decode_clipboard_text(std::slice::from_raw_parts(pointer.cast::<u16>(), units));
        let _ = GlobalUnlock(handle);
        if text.len() > max_bytes {
            return Err(over_limit("The clipboard text", max_bytes));
        }
        Ok((!text.is_empty()).then_some(text))
    }
}
pub fn write(hwnd: HWND, text: &str, max_bytes: usize) -> Result<()> {
    write_inner(hwnd, text, max_bytes, None)
}
struct OwnedGlobal(HGLOBAL);
impl Drop for OwnedGlobal {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalFree(Some(self.0));
        }
    }
}
impl OwnedGlobal {
    fn copy(bytes: &[u8]) -> Result<Self> {
        unsafe {
            let memory = Self(GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len())?);
            let pointer = GlobalLock(memory.0);
            if pointer.is_null() {
                return Err(Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
            let _ = GlobalUnlock(memory.0);
            Ok(memory)
        }
    }
    /// Encode NUL-terminated UTF-16 in place; large selections get no intermediate copies.
    fn text(text: &str) -> Result<Self> {
        let units = text.encode_utf16().count() + 1;
        let bytes = units.checked_mul(2).ok_or_else(|| Error::from_hresult(E_OUTOFMEMORY))?;
        unsafe {
            let memory = Self(
                GlobalAlloc(GMEM_MOVEABLE, bytes)
                    .map_err(|_| Error::new(E_OUTOFMEMORY, "Not enough memory to place the text on the clipboard."))?,
            );
            let pointer = GlobalLock(memory.0);
            if pointer.is_null() {
                return Err(Error::from_thread());
            }
            let target = std::slice::from_raw_parts_mut(pointer.cast::<u16>(), units);
            for (slot, unit) in target.iter_mut().zip(text.encode_utf16().chain(Some(0))) {
                *slot = unit;
            }
            let _ = GlobalUnlock(memory.0);
            Ok(memory)
        }
    }
    unsafe fn publish(self, format: u32) -> Result<()> {
        unsafe {
            SetClipboardData(format, Some(HANDLE(self.0.0)))?;
        }
        std::mem::forget(self);
        Ok(())
    }
}
fn registered(format: &str) -> Result<u32> {
    if !valid_clipboard_format(format) {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    let name: Vec<u16> = format.encode_utf16().chain(Some(0)).collect();
    let id = unsafe { RegisterClipboardFormatW(windows::core::PCWSTR(name.as_ptr())) };
    if id == 0 { Err(Error::from_thread()) } else { Ok(id) }
}
pub fn write_with_metadata(hwnd: HWND, text: &str, max_bytes: usize, format: &str, bytes: &[u8]) -> Result<()> {
    let envelope = encode_clipboard_metadata(bytes).ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
    let id = registered(format)?;
    write_inner(hwnd, text, max_bytes, Some((id, &envelope)))
}
fn write_inner(hwnd: HWND, text: &str, max_bytes: usize, metadata: Option<(u32, &[u8])>) -> Result<()> {
    if text.len() > max_bytes {
        return Err(over_limit("The text", max_bytes));
    }
    if text.contains('\0') {
        // CF_UNICODETEXT ends at the first NUL; other applications would silently lose the rest.
        return Err(Error::new(
            E_INVALIDARG,
            "Text containing NUL characters cannot be copied to the clipboard.",
        ));
    }
    // Prepare every fallible allocation, and wait out other clipboard users,
    // before clearing the user's clipboard.
    let memory = OwnedGlobal::text(text)?;
    let private = metadata
        .map(|(id, bytes)| OwnedGlobal::copy(bytes).map(|memory| (id, memory)))
        .transpose()?;
    let _open = open(hwnd)?;
    // SAFETY: ownership transfers only after each successful SetClipboardData.
    // Win32 requires EmptyClipboard before SetClipboardData for this window to
    // own the new data, so emptying is the last step before publishing.
    unsafe {
        EmptyClipboard().map_err(|_| Error::new(CLIPBRD_E_CANT_EMPTY, "The clipboard could not be replaced."))?;
        memory
            .publish(UNICODE_TEXT)
            .map_err(|_| Error::new(CLIPBRD_E_CANT_SET, "The text could not be placed on the clipboard."))?;
        // Private metadata is optional: a rejected extra format leaves valid text.
        if let Some((id, memory)) = private {
            let _ = memory.publish(id);
        }
    }
    Ok(())
}

unsafe fn metadata_open(id: u32, max_bytes: usize) -> Option<Vec<u8>> {
    unsafe {
        if IsClipboardFormatAvailable(id).is_err() {
            return None;
        }
        let handle = HGLOBAL(GetClipboardData(id).ok()?.0);
        let size = GlobalSize(handle);
        // The allocation may have alignment padding. Bound the entire allocation
        // before borrowing it, then enforce the declared payload bound as well.
        if !(8..=MAX_CLIPBOARD_METADATA_BYTES + 64).contains(&size) {
            return None;
        }
        let pointer = GlobalLock(handle);
        if pointer.is_null() {
            return None;
        }
        let result = decode_clipboard_metadata(std::slice::from_raw_parts(pointer.cast::<u8>(), size), max_bytes);
        let _ = GlobalUnlock(handle);
        result
    }
}
pub fn metadata(hwnd: HWND, format: &str, max_bytes: usize) -> Result<Option<Vec<u8>>> {
    let id = registered(format)?;
    let _open = open(hwnd)?;
    // SAFETY: the clipboard stays open while the metadata is copied out.
    unsafe { Ok(metadata_open(id, max_bytes)) }
}
/// `None` when the clipboard holds no text; metadata never travels without text.
pub fn read_with_metadata(
    hwnd: HWND,
    text_limit: usize,
    format: &str,
    max_bytes: usize,
) -> Result<Option<ClipboardContents>> {
    let id = registered(format)?;
    if !text_available() {
        return Ok(None);
    }
    let _open = open(hwnd)?;
    // SAFETY: both formats are read under one ownership lock.
    unsafe {
        let Some(text) = read_text_open(text_limit)? else {
            return Ok(None);
        };
        Ok(Some(ClipboardContents {
            text,
            metadata: metadata_open(id, max_bytes),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::clipboard::{DEFAULT_CLIPBOARD_MAX_BYTES, RECTANGLE_CLIPBOARD_FORMAT};
    use std::sync::{Mutex, MutexGuard, mpsc};
    use windows::{
        Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE},
        core::w,
    };
    /// Tests that open the system clipboard must not contend with one another.
    fn serial() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn sample(bytes: usize) -> String {
        let line = "Tab\tseparated, CJK \u{754c}, emoji \u{1f980}, ZWJ \u{1f469}\u{200d}\u{1f4bb}, RTL \u{05e9}\u{05dc}\u{05d5}\u{05dd}\r\n";
        let mut text = String::with_capacity(bytes + line.len());
        while text.len() < bytes {
            text.push_str(line);
        }
        text
    }
    /// Runs `f` while another thread keeps the clipboard open for about 50 ms.
    fn while_another_thread_holds_clipboard<T>(f: impl FnOnce() -> T) -> T {
        let (held, wait) = mpsc::channel();
        let holder = std::thread::spawn(move || {
            let guard = open(HWND::default()).expect("holder opens the clipboard");
            held.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            drop(guard);
        });
        wait.recv().expect("holder opened the clipboard");
        let result = f();
        holder.join().unwrap();
        result
    }
    struct OwnerWindow(HWND);
    impl OwnerWindow {
        fn new() -> Self {
            // SAFETY: a message-only window created and destroyed on this test thread.
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("Bareline clipboard owner"),
                    WINDOW_STYLE::default(),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    None,
                    None,
                )
            }
            .expect("message-only clipboard owner");
            Self(hwnd)
        }
    }
    impl Drop for OwnerWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
    #[test]
    fn private_payload_preparation_round_trips_without_touching_clipboard() {
        let payload = b"versioned selection payload";
        let envelope = encode_clipboard_metadata(payload).unwrap();
        let memory = OwnedGlobal::copy(&envelope).unwrap();
        // Exercise the native movable allocation and lock contract without
        // opening or replacing the user's clipboard in an automated test.
        unsafe {
            let size = GlobalSize(memory.0);
            assert!(size >= envelope.len());
            let pointer = GlobalLock(memory.0);
            assert!(!pointer.is_null());
            let result =
                decode_clipboard_metadata(std::slice::from_raw_parts(pointer.cast::<u8>(), size), payload.len());
            let _ = GlobalUnlock(memory.0);
            assert_eq!(result.as_deref(), Some(payload.as_slice()));
        }
        assert!(registered("CF_UNICODETEXT").is_err());
    }
    #[test]
    fn text_larger_than_the_history_limit_round_trips_through_global_memory() {
        // 6 MB exceeds the 4 MiB history entry limit that used to cap the clipboard.
        let text = sample(6_000_000);
        let memory = OwnedGlobal::text(&text).unwrap();
        // SAFETY: the test owns this allocation until `memory` drops.
        unsafe {
            let pasted = read_text_global(memory.0, DEFAULT_CLIPBOARD_MAX_BYTES).unwrap();
            assert!(pasted.as_deref() == Some(text.as_str()), "6 MB text changed in transit");
            assert!(read_text_global(memory.0, text.len() - 1).is_err());
        }
        assert!(write_inner(HWND::default(), &text, text.len() - 1, None).is_err());
        assert!(write_inner(HWND::default(), "a\0b", DEFAULT_CLIPBOARD_MAX_BYTES, None).is_err());
    }
    #[test]
    fn foreign_text_without_terminator_or_content_is_accepted() {
        let units: Vec<u16> = "no terminator".encode_utf16().collect();
        let bytes: Vec<u8> = units.iter().flat_map(|unit| unit.to_ne_bytes()).collect();
        let foreign = OwnedGlobal::copy(&bytes).unwrap();
        let empty = OwnedGlobal::text("").unwrap();
        // SAFETY: the test owns both allocations until they drop.
        unsafe {
            assert_eq!(
                read_text_global(foreign.0, DEFAULT_CLIPBOARD_MAX_BYTES)
                    .unwrap()
                    .as_deref(),
                Some("no terminator")
            );
            assert_eq!(read_text_global(empty.0, DEFAULT_CLIPBOARD_MAX_BYTES).unwrap(), None);
        }
    }
    #[test]
    fn open_retries_until_the_holder_releases() {
        let mut calls = 0;
        let opened = retry(OPEN_ATTEMPTS, Duration::ZERO, || {
            calls += 1;
            if calls < 4 {
                Err(Error::from_hresult(CLIPBRD_E_CANT_OPEN))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(opened.unwrap(), 4);
        let mut calls = 0;
        let failed = retry(OPEN_ATTEMPTS, Duration::ZERO, || -> Result<()> {
            calls += 1;
            Err(Error::from_hresult(CLIPBRD_E_CANT_OPEN))
        });
        assert_eq!(failed.unwrap_err().code(), CLIPBRD_E_CANT_OPEN);
        assert_eq!(calls, OPEN_ATTEMPTS);
    }
    #[test]
    fn paste_succeeds_while_another_thread_briefly_holds_the_clipboard() {
        let _serial = serial();
        // Opening and reading only: the user's clipboard contents are left untouched.
        let opened = while_another_thread_holds_clipboard(|| open(HWND::default()).map(drop));
        assert!(opened.is_ok(), "{opened:?}");
        let pasted = while_another_thread_holds_clipboard(|| read(HWND::default(), DEFAULT_CLIPBOARD_MAX_BYTES));
        assert!(pasted.is_ok(), "{:?}", pasted.map(|text| text.map(|text| text.len())));
    }
    fn system_round_trip(bytes: usize) {
        let _serial = serial();
        let owner = OwnerWindow::new();
        let text = sample(bytes);
        write(owner.0, &text, DEFAULT_CLIPBOARD_MAX_BYTES).unwrap();
        let hwnd = owner.0;
        let pasted = while_another_thread_holds_clipboard(|| read(hwnd, DEFAULT_CLIPBOARD_MAX_BYTES)).unwrap();
        assert!(
            pasted.as_deref() == Some(text.as_str()),
            "{bytes}-byte clipboard round trip changed the text"
        );
    }
    #[test]
    #[ignore = "replaces the user's system clipboard; run with --ignored in a disposable session"]
    fn six_megabyte_clipboard_round_trip_is_byte_exact() {
        system_round_trip(6_000_000);
    }
    #[test]
    #[ignore = "replaces the user's system clipboard and needs about 600 MB of memory"]
    fn hundred_megabyte_clipboard_round_trip_is_byte_exact() {
        system_round_trip(100_000_000);
    }
    #[test]
    #[ignore = "replaces the user's system clipboard; run with --ignored in a disposable session"]
    fn non_text_clipboard_paste_is_a_no_op() {
        let _serial = serial();
        let owner = OwnerWindow::new();
        let id = registered(RECTANGLE_CLIPBOARD_FORMAT).unwrap();
        let private = OwnedGlobal::copy(&encode_clipboard_metadata(b"opaque").unwrap()).unwrap();
        let guard = open(owner.0).unwrap();
        // SAFETY: this window owns the open clipboard; ownership transfers on success.
        unsafe {
            EmptyClipboard().unwrap();
            private.publish(id).unwrap();
        }
        drop(guard);
        assert_eq!(read(owner.0, DEFAULT_CLIPBOARD_MAX_BYTES).unwrap(), None);
        assert_eq!(
            read_with_metadata(owner.0, DEFAULT_CLIPBOARD_MAX_BYTES, RECTANGLE_CLIPBOARD_FORMAT, 64).unwrap(),
            None
        );
    }
}
