// SPDX-License-Identifier: MPL-2.0
//! Explicit explorer mutations: retained no-follow ancestors and handle-based
//! rename/delete. Rename never replaces a destination; delete is nonrecursive;
//! explorer deletion moves the entry to the Recycle Bin.
use bareline_platform::LocalFileSystem;
use std::{
    fs::{File, OpenOptions},
    io,
    mem::size_of,
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
    if file.metadata()?.file_attributes() & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0) != 0 {
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
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "volume root cannot be mutated"))?;
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
    let target_parents = parents(fs, target)?;
    let file = nofollow(source, DELETE.0 | FILE_READ_ATTRIBUTES.0)?;
    let target_parent = target_parents
        .last()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination parent required"))?;
    let name: Vec<u16> = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination filename required"))?
        .encode_wide()
        .collect();
    if name.contains(&0) || name.len() > 32767 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid destination name"));
    }
    crate::rename::rename(&file, target_parent, &name, false)
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
/// Reversible explorer deletion: the shell moves the whole entry to the Recycle
/// Bin, where Explorer restores it, instead of leaving hidden siblings in the
/// folder. The path and its ancestors pass the same no-follow checks first, and
/// an entry too large to recycle is only destroyed after the shell's warning.
pub fn recycle_entry(fs: &dyn LocalFileSystem, path: &Path) -> io::Result<()> {
    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            UI::Shell::{
                FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, FOFX_EARLYFAILURE,
                FOFX_RECYCLEONDELETE, FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName,
            },
        },
        core::PCWSTR,
    };
    let _parents = parents(fs, path)?;
    drop(nofollow(path, FILE_READ_ATTRIBUTES.0)?);
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if wide[..wide.len() - 1].contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid path"));
    }
    /// Returns whether the shell aborted the operation (for example after its warning).
    fn shell_recycle(wide: &[u16]) -> windows::core::Result<bool> {
        // SAFETY: `wide` is NUL-terminated and outlives the calls; the COM objects
        // are released when this function returns, inside the caller's apartment.
        unsafe {
            let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
            operation.SetOperationFlags(
                FOF_ALLOWUNDO
                    | FOFX_RECYCLEONDELETE
                    | FOF_NOCONFIRMATION
                    | FOF_WANTNUKEWARNING
                    | FOF_SILENT
                    | FOF_NOERRORUI
                    | FOFX_EARLYFAILURE,
            )?;
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)?;
            operation.DeleteItem(&item, None)?;
            operation.PerformOperations()?;
            Ok(operation.GetAnyOperationsAborted()?.as_bool())
        }
    }
    // SAFETY: this worker thread initializes its own apartment for the call and
    // balances a successful initialization after every COM object is released.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }.is_ok();
    let result = shell_recycle(&wide);
    if initialized {
        // SAFETY: paired with the successful CoInitializeEx above on this thread.
        unsafe { CoUninitialize() };
    }
    match result {
        Ok(false) => Ok(()),
        Ok(true) => Err(io::Error::new(io::ErrorKind::Interrupted, "Deletion was cancelled")),
        Err(error) => Err(io::Error::other(format!("Could not move to the Recycle Bin: {error}"))),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::WindowsFileSystem;
    #[test]
    fn no_replace_and_nonrecursive_deletion() {
        let root = std::env::temp_dir().join(format!("bareline-workspace-ops-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        create(&WindowsFileSystem, &a, false).unwrap();
        create(&WindowsFileSystem, &b, false).unwrap();
        std::fs::write(&a, b"source content").unwrap();
        std::fs::write(&b, b"destination content").unwrap();
        assert!(rename(&WindowsFileSystem, &a, &b).is_err());
        assert!(a.exists() && b.exists());
        assert_eq!(std::fs::read(&a).unwrap(), b"source content");
        assert_eq!(std::fs::read(&b).unwrap(), b"destination content");
        assert!(delete(&WindowsFileSystem, &root).is_err());
        let c = root.join("c");
        rename(&WindowsFileSystem, &a, &c).unwrap();
        assert!(!a.exists() && c.exists());
        delete(&WindowsFileSystem, &c).unwrap();
        delete(&WindowsFileSystem, &b).unwrap();
        delete(&WindowsFileSystem, &root).unwrap();
    }
    #[test]
    fn recycling_checks_the_path_before_the_shell_sees_it() {
        let kind = |path: &Path| recycle_entry(&WindowsFileSystem, path).unwrap_err().kind();
        assert_eq!(kind(Path::new("relative.txt")), io::ErrorKind::InvalidInput);
        let missing = std::env::temp_dir().join(format!("bareline-recycle-missing-{}", std::process::id()));
        assert_eq!(kind(&missing), io::ErrorKind::NotFound);
    }
    #[test]
    #[ignore = "moves a scratch file into the signed-in user's real Recycle Bin"]
    fn explorer_delete_recycles_without_hidden_siblings() {
        let root = std::env::temp_dir().join(format!("bareline-recycle-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let entry = root.join("recycled.txt");
        std::fs::write(&entry, b"restorable from the Recycle Bin").unwrap();
        recycle_entry(&WindowsFileSystem, &entry).unwrap();
        assert!(!entry.exists());
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            0,
            "a hidden sibling was left behind"
        );
        std::fs::remove_dir(&root).unwrap();
    }
}
