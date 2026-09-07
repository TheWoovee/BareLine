// SPDX-License-Identifier: MPL-2.0
//! One ABI boundary for atomic renames of already-open files.
use std::{fs::File, io, mem::{offset_of, size_of}, os::windows::io::AsRawHandle};
use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::*};

pub(crate) fn rename(file: &File, directory: &File, basename: &[u16], replace: bool) -> io::Result<()> {
    if basename.is_empty() || basename.iter().any(|c| matches!(*c, 0 | 47 | 92)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid destination basename"));
    }
    // The caller retains a no-follow directory handle without delete sharing.
    // Resolve its actual path while it is pinned, never reopen the source name.
    let mut name = vec![0u16; 32768];
    let count = unsafe { GetFinalPathNameByHandleW(HANDLE(directory.as_raw_handle()), &mut name, GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0)) } as usize;
    if count == 0 { return Err(io::Error::last_os_error()); }
    if count >= name.len() { return Err(io::Error::new(io::ErrorKind::InvalidInput, "destination path exceeds limit")); }
    name.truncate(count);
    if name.last() != Some(&92) { name.push(92); }
    name.extend_from_slice(basename);
    if name.len() > 32767 { return Err(io::Error::new(io::ErrorKind::InvalidInput, "destination path exceeds limit")); }
    let bytes = (offset_of!(FILE_RENAME_INFO, FileName) + (name.len() + 1) * 2).max(size_of::<FILE_RENAME_INFO>());
    let mut storage = vec![0u64; bytes.div_ceil(8)];
    // Aligned storage includes the structure minimum and a zero UTF-16 tail.
    // Replacement policy and collision checking are one OS operation.
    unsafe {
        let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        (*info).Anonymous.ReplaceIfExists = replace;
        (*info).RootDirectory = HANDLE::default();
        (*info).FileNameLength = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr(), std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(), name.len());
        SetFileInformationByHandle(HANDLE(file.as_raw_handle()), FileRenameInfo, info.cast(), bytes as u32)
            .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))
    }
}
