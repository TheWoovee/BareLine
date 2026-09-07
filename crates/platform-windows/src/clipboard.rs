// SPDX-License-Identifier: MPL-2.0
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
            .and_then(|end| {
                String::from_utf16(&units[..end]).map_err(|_| Error::from_hresult(E_INVALIDARG))
            });
        let _ = GlobalUnlock(handle);
        result
    }
}
pub fn write(hwnd: HWND, text: &str) -> Result<()> {
    if text.len() > LIMIT || text.contains('\0') {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    if units.len() * 2 > LIMIT {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    // SAFETY: GlobalAlloc ownership passes to Windows only after successful SetClipboardData.
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2)?;
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            let _ = GlobalFree(Some(memory));
            return Err(Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), pointer.cast::<u16>(), units.len());
        let _ = GlobalUnlock(memory);
        if let Err(error) = OpenClipboard(Some(hwnd)) {
            let _ = GlobalFree(Some(memory));
            return Err(error);
        }
        let _open = Open;
        let result =
            EmptyClipboard().and_then(|_| SetClipboardData(13, Some(HANDLE(memory.0))).map(|_| ()));
        if result.is_err() {
            let _ = GlobalFree(Some(memory));
        }
        result
    }
}
