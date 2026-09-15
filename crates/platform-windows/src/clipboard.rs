// SPDX-License-Identifier: MPL-2.0
use bareline_platform::clipboard::{
    ClipboardContents, MAX_CLIPBOARD_METADATA_BYTES, decode_clipboard_metadata, encode_clipboard_metadata,
    valid_clipboard_format,
};
use windows::{
    Win32::{
        Foundation::*,
        System::{DataExchange::*, Memory::*},
    },
    core::{Error, Result},
};
const LIMIT: usize = 4 * 1024 * 1024;
struct Open;
impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}
pub fn read(hwnd: HWND) -> Result<String> {
    // SAFETY: the clipboard and global memory stay locked for the entire borrowed slice.
    unsafe {
        OpenClipboard(Some(hwnd))?;
        let _open = Open;
        read_text_open()
    }
}
/// Caller owns the clipboard until all global-memory reads have completed.
unsafe fn read_text_open() -> Result<String> {
    unsafe {
        let handle = HGLOBAL(GetClipboardData(13)?.0);
        let size = GlobalSize(handle);
        if !(2..=LIMIT).contains(&size) || !size.is_multiple_of(2) {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        let pointer = GlobalLock(handle);
        if pointer.is_null() {
            return Err(Error::from_thread());
        }
        let units = std::slice::from_raw_parts(pointer.cast::<u16>(), size / 2);
        let result = units
            .iter()
            .position(|u| *u == 0)
            .ok_or_else(|| Error::from_hresult(E_INVALIDARG))
            .and_then(|end| String::from_utf16(&units[..end]).map_err(|_| Error::from_hresult(E_INVALIDARG)));
        let _ = GlobalUnlock(handle);
        result
    }
}
pub fn write(hwnd: HWND, text: &str) -> Result<()> {
    write_inner(hwnd, text, None)
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
pub fn write_with_metadata(hwnd: HWND, text: &str, format: &str, bytes: &[u8]) -> Result<()> {
    let envelope = encode_clipboard_metadata(bytes).ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
    let id = registered(format)?;
    write_inner(hwnd, text, Some((id, &envelope)))
}
fn write_inner(hwnd: HWND, text: &str, metadata: Option<(u32, &[u8])>) -> Result<()> {
    if text.len() > LIMIT || text.contains('\0') {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    if units.len() * 2 > LIMIT {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    let text_bytes: Vec<u8> = units.iter().flat_map(|u| u.to_ne_bytes()).collect();
    // Prepare every fallible allocation before clearing the user's clipboard.
    let memory = OwnedGlobal::copy(&text_bytes)?;
    let private = metadata
        .map(|(id, bytes)| OwnedGlobal::copy(bytes).map(|memory| (id, memory)))
        .transpose()?;
    // SAFETY: ownership transfers only after each successful SetClipboardData.
    unsafe {
        OpenClipboard(Some(hwnd))?;
        let _open = Open;
        EmptyClipboard()?;
        memory.publish(13)?;
        // Private metadata is optional: a rejected extra format leaves valid text.
        if let Some((id, memory)) = private {
            let _ = memory.publish(id);
        }
        Ok(())
    }
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
    unsafe {
        OpenClipboard(Some(hwnd))?;
        let _open = Open;
        Ok(metadata_open(id, max_bytes))
    }
}
pub fn read_with_metadata(hwnd: HWND, format: &str, max_bytes: usize) -> Result<ClipboardContents> {
    let id = registered(format)?;
    unsafe {
        OpenClipboard(Some(hwnd))?;
        let _open = Open;
        let text = read_text_open()?;
        Ok(ClipboardContents {
            text,
            metadata: metadata_open(id, max_bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
