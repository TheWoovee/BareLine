// SPDX-License-Identifier: MPL-2.0
//! Explicit explorer mutations: retained no-follow ancestors and handle-based
//! rename/delete. Rename never replaces a destination; delete is nonrecursive.
use bareline_platform::LocalFileSystem;
use std::{
    fs::{File, OpenOptions},
    io,
    mem::{offset_of, size_of},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Component, Path},
};
use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::*};
fn error(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0 & 0xffff)
}
fn nofollow(path: &Path, access: u32) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0 | FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)?;
    if file.metadata()?.file_attributes()
        & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0)
        != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "linked/offline entries require separate authorization",
        ));
    }
    Ok(file)
}
fn parents(fs: &dyn LocalFileSystem, path: &Path) -> io::Result<Vec<File>> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "absolute normalized path required",
        ));
    }
    fs.validate_target(path)?;
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "volume root cannot be mutated",
        )
    })?;
    let paths: Vec<_> = parent.ancestors().collect();
    let mut retained = Vec::new();
    for ancestor in paths.into_iter().rev() {
        retained.push(nofollow(ancestor, FILE_READ_ATTRIBUTES.0)?);
    }
    Ok(retained)
}
pub fn create(fs: &dyn LocalFileSystem, path: &Path, directory: bool) -> io::Result<()> {
    let _parents = parents(fs, path)?;
    if directory {
        std::fs::create_dir(path)
    } else {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)?
            .sync_all()
    }
}
pub fn rename(fs: &dyn LocalFileSystem, source: &Path, target: &Path) -> io::Result<()> {
    let _source_parents = parents(fs, source)?;
    let _target_parents = parents(fs, target)?;
    let file = nofollow(source, DELETE.0 | FILE_READ_ATTRIBUTES.0)?;
    let name: Vec<u16> = target.as_os_str().encode_wide().collect();
    if name.contains(&0) || name.len() > 32767 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid destination name",
        ));
    }
    let bytes = offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2;
    let mut storage = vec![0u64; bytes.max(size_of::<FILE_RENAME_INFO>()).div_ceil(8)];
    // SAFETY: u64 allocation aligns FILE_RENAME_INFO on Windows x64; sufficient
    // bytes include the variable UTF-16 tail. File and ancestor handles remain
    // alive throughout the call. ReplaceIfExists stays false (zero initialized).
    unsafe {
        let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        (*info).FileNameLength = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            name.len(),
        );
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileRenameInfo,
            info.cast(),
            bytes as u32,
        )
        .map_err(error)
    }
}
pub fn delete(fs: &dyn LocalFileSystem, path: &Path) -> io::Result<()> {
    let _parents = parents(fs, path)?;
    let file = nofollow(path, DELETE.0 | FILE_READ_ATTRIBUTES.0)?;
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: live no-follow handle; the OS refuses nonempty directory deletion.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
        .map_err(error)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::WindowsFileSystem;
    #[test]
    fn no_replace_and_nonrecursive_deletion() {
        let root =
            std::env::temp_dir().join(format!("bareline-workspace-ops-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        create(&WindowsFileSystem, &a, false).unwrap();
        create(&WindowsFileSystem, &b, false).unwrap();
        assert!(rename(&WindowsFileSystem, &a, &b).is_err());
        assert!(a.exists() && b.exists());
        assert!(delete(&WindowsFileSystem, &root).is_err());
        let c = root.join("c");
        rename(&WindowsFileSystem, &a, &c).unwrap();
        assert!(!a.exists() && c.exists());
        delete(&WindowsFileSystem, &c).unwrap();
        delete(&WindowsFileSystem, &b).unwrap();
        delete(&WindowsFileSystem, &root).unwrap();
    }
}
