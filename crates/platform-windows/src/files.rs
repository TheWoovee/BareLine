// SPDX-License-Identifier: MPL-2.0
use bareline_platform::{FileIdentity, LocalFileSystem};
use std::{
    fs::File,
    io,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::Path,
};
use windows::{
    Win32::{Foundation::*, Storage::FileSystem::*},
    core::PCWSTR,
};
pub struct WindowsFileSystem;
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
fn io_error(error: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0 & 0xffff)
}
impl LocalFileSystem for WindowsFileSystem {
    fn open_follow_read(&self,path:&Path)->io::Result<(File,std::sync::Arc<dyn Send+Sync>)> {
        crate::path_trust::WindowsPathTrustProvider.open_follow_read(path)
    }

    fn guard_directory(&self, path: &Path) -> io::Result<std::sync::Arc<dyn Send + Sync>> {
        use bareline_platform::PathTrustProvider;
        Ok(std::sync::Arc::new(crate::path_trust::WindowsPathTrustProvider.open_read(path, bareline_platform::PathOrigin::User)?))
    }

    fn available_space(&self, path: &Path) -> io::Result<u64> {
        let path = wide(path);
        let mut available = 0;
        // SAFETY: NUL-terminated path and output pointer remain valid for this call.
        unsafe { GetDiskFreeSpaceExW(PCWSTR(path.as_ptr()), Some(&mut available), None, None).map_err(io_error)?; }
        Ok(available)
    }

    fn open_sealed_read(&self, path: &Path) -> io::Result<File> {
        use std::os::windows::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ.0).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0).open(path)?;
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the owned file handle and output structure remain valid.
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information).map_err(io_error)?; }
        if information.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0) != 0 { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "sealed source is a reparse/offline file")); }
        Ok(file)
    }

    fn create_entry(&self, path: &Path, directory: bool) -> io::Result<()> {
        crate::workspace_files::create(self, path, directory)
    }
    fn rename_entry(&self, source: &Path, target: &Path) -> io::Result<()> {
        crate::workspace_files::rename(self, source, target)
    }
    fn delete_entry(&self, path: &Path) -> io::Result<()> {
        crate::workspace_files::delete(self, path)
    }
    fn identity(&self, file: &File) -> io::Result<FileIdentity> {
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: File owns a valid handle for the entire API call; the API does not take ownership.
        unsafe {
            GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information)
                .map_err(io_error)?;
        }
        Ok(FileIdentity {
            volume: information.dwVolumeSerialNumber as u64,
            file: ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64,
            length: ((information.nFileSizeHigh as u64) << 32) | information.nFileSizeLow as u64,
            modified: ((information.ftLastWriteTime.dwHighDateTime as u64) << 32)
                | information.ftLastWriteTime.dwLowDateTime as u64,
        })
    }
    fn validate_source(&self, path: &Path) -> io::Result<()> {
        self.validate_path(path, false)
    }
    fn validate_target(&self, path: &Path) -> io::Result<()> {
        self.validate_path(path, true)
    }
    fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
        let stage = wide(staged);
        let target = wide(target);
        #[cfg(test)] replacement_faults::hit(existed, false)?;
        // SAFETY: both owned names are same-directory paths; no truncate/in-place fallback.
        let result = unsafe {
            if existed {
                ReplaceFileW(PCWSTR(target.as_ptr()), PCWSTR(stage.as_ptr()), None, REPLACE_FILE_FLAGS(0), None, None).map_err(io_error)
            } else {
                MoveFileExW(PCWSTR(stage.as_ptr()), PCWSTR(target.as_ptr()), MOVEFILE_WRITE_THROUGH).map_err(io_error)
            }
        };
        #[cfg(test)] replacement_faults::hit(existed, true)?;
        result
    }
}

impl WindowsFileSystem {
    fn validate_path(&self, path: &Path, writing: bool) -> io::Result<()> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "only absolute local paths are supported",
            ));
        }
        use std::path::{Component, Prefix};
        if !matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
            || path.components().any(
                |c| matches!(c, Component::Normal(s) if s.encode_wide().any(|u| u == b':' as u16)),
            )
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "device and alternate-stream paths are not supported",
            ));
        }
        let value = wide(path);
        let mut root = vec![0u16; 32768];
        // SAFETY: owned terminated UTF-16 input and writable output buffers outlive calls.
        unsafe {
            GetVolumePathNameW(PCWSTR(value.as_ptr()), &mut root).map_err(io_error)?;
            if GetDriveTypeW(PCWSTR(root.as_ptr())) != 3 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "save requires a local fixed NTFS volume",
                ));
            }
            let mut filesystem = [0u16; 64];
            GetVolumeInformationW(
                PCWSTR(root.as_ptr()),
                None,
                None,
                None,
                None,
                Some(&mut filesystem),
            )
            .map_err(io_error)?;
            let end = filesystem
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(filesystem.len());
            if String::from_utf16_lossy(&filesystem[..end]) != "NTFS" {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "filesystem replacement guarantees unavailable",
                ));
            }
            for ancestor in path.ancestors() {
                let name = wide(ancestor);
                let attributes = GetFileAttributesW(PCWSTR(name.as_ptr()));
                if attributes == INVALID_FILE_ATTRIBUTES {
                    if ancestor == path && !path.exists() {
                        continue;
                    }
                    return Err(io::Error::last_os_error());
                }
                if attributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0) != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "reparse/cloud paths need explicit trust integration",
                    ));
                }
                if writing && ancestor == path && attributes & FILE_ATTRIBUTE_READONLY.0 != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "file is read-only",
                    ));
                }
            }
            if let Ok(file) = File::open(path) {
                let mut info = BY_HANDLE_FILE_INFORMATION::default();
                GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)
                    .map_err(io_error)?;
                if writing && info.nNumberOfLinks > 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "hard-linked targets need explicit replacement approval",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Edit, EditTransaction, TextOffset};
    use bareline_file_io::lifecycle::{FileError, open_utf8, save_utf8};
    #[test]
    fn sealed_read_denies_mutation_until_guard_is_dropped() {
        let path = std::env::temp_dir().join(format!("bareline-sealed-{}.txt", std::process::id()));
        let renamed = path.with_extension("moved");
        std::fs::write(&path, b"sealed").unwrap();
        let guard = WindowsFileSystem.open_sealed_read(&path).unwrap();
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::write(&path, b"changed").is_err());
        assert!(std::fs::rename(&path, &renamed).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        drop(guard);
        std::fs::write(&path, b"changed").unwrap();
        std::fs::rename(&path, &renamed).unwrap();
        std::fs::remove_file(renamed).unwrap();
    }
    #[test]
    fn readonly_source_can_open_but_cannot_be_replaced() {
        let path =
            std::env::temp_dir().join(format!("bareline-readonly-{}.txt", std::process::id()));
        std::fs::write(&path, b"read only").unwrap();
        let original_permissions = std::fs::metadata(&path).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let result = open_utf8(&path, &WindowsFileSystem, Budget::new(1024), Budget::new(0));
        let refused = WindowsFileSystem.validate_target(&path).is_err();
        std::fs::set_permissions(&path, original_permissions).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(result.is_ok());
        assert!(refused);
    }
    struct DeniedCommit;
    struct CancellingPlatform {
        cancellation: bareline_file_io::cancellation::Cancellation,
        validations: std::sync::atomic::AtomicUsize,
        during_commit: bool,
    }
    impl LocalFileSystem for CancellingPlatform {
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            WindowsFileSystem.identity(file)
        }
        fn validate_target(&self, path: &Path) -> io::Result<()> {
            WindowsFileSystem.validate_target(path)?;
            if self
                .validations
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                == 1
                && !self.during_commit
            {
                self.cancellation.cancel();
            }
            Ok(())
        }
        fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
            assert!(self.during_commit, "cancelled save must not reach commit");
            self.cancellation.cancel();
            WindowsFileSystem.commit(staged, target, existed)
        }
    }
    #[test]
    fn save_cancellation_cleans_stage_before_commit_and_reports_success_after_commit() {
        use bareline_file_io::{cancellation::Cancellation, lifecycle::save_utf8_cancellable};
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        assert!(directory.canonicalize().unwrap().starts_with(&root));
        let path = directory.join("fixture.txt");
        std::fs::write(&path, b"original").unwrap();
        let opened = open_utf8(
            &path,
            &WindowsFileSystem,
            Budget::new(1 << 20),
            Budget::new(1 << 20),
        )
        .unwrap();
        let document = bareline_document::Document::from_utf8(
            "replacement",
            Budget::new(1 << 20),
            Budget::new(1 << 20),
        )
        .unwrap();
        for during_commit in [false, true] {
            let cancellation = Cancellation::default();
            let platform = CancellingPlatform {
                cancellation: cancellation.clone(),
                validations: 0.into(),
                during_commit,
            };
            let result = save_utf8_cancellable(
                document.snapshot(),
                &path,
                Some(&opened.fingerprint),
                false,
                &platform,
                &cancellation,
            );
            if during_commit {
                assert!(
                    result.is_ok(),
                    "commit outcome must override late cancellation"
                );
                assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
            } else {
                assert!(matches!(result, Err(FileError::Cancelled)));
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
            }
            assert_eq!(
                std::fs::read_dir(&directory).unwrap().count(),
                1,
                "no abandoned staging file"
            );
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
    impl LocalFileSystem for DeniedCommit {
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            WindowsFileSystem.identity(file)
        }
        fn validate_target(&self, path: &Path) -> io::Result<()> {
            WindowsFileSystem.validate_target(path)
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        }
    }
    #[test]
    fn staged_save_preserves_original_on_failure_and_conflict_and_tracks_captured_state() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-lifecycle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        assert!(directory.canonicalize().unwrap().starts_with(&root));
        let path = directory.join("fixture.txt");
        std::fs::write(&path, b"\xef\xbb\xbfa\r\n").unwrap();
        let mut opened = open_utf8(
            &path,
            &WindowsFileSystem,
            Budget::new(1 << 20),
            Budget::new(1 << 20),
        )
        .unwrap();
        assert!(opened.bom);
        let mut insert = |text: &str| {
            let snapshot = opened.document.snapshot();
            opened
                .document
                .apply(EditTransaction {
                    base_revision: snapshot.revision,
                    edits: vec![Edit {
                        range: TextOffset(0)..TextOffset(0),
                        insert: text.into(),
                    }],
                })
                .unwrap();
        };
        insert("b");
        let captured = opened.document.snapshot();
        let failure = save_utf8(
            captured.clone(),
            &path,
            Some(&opened.fingerprint),
            true,
            &DeniedCommit,
        );
        let Err(FileError::Commit { staged, .. }) = failure else {
            panic!("expected injected commit failure")
        };
        assert_eq!(std::fs::read(&path).unwrap(), b"\xef\xbb\xbfa\r\n");
        assert_eq!(std::fs::read(&staged).unwrap(), b"\xef\xbb\xbfba\r\n");
        std::fs::remove_file(staged).unwrap();
        let snapshot = opened.document.snapshot();
        opened
            .document
            .apply(EditTransaction {
                base_revision: snapshot.revision,
                edits: vec![Edit {
                    range: TextOffset(0)..TextOffset(0),
                    insert: "c".into(),
                }],
            })
            .unwrap();
        let saved = save_utf8(
            captured,
            &path,
            Some(&opened.fingerprint),
            true,
            &WindowsFileSystem,
        )
        .unwrap();
        opened.document.mark_saved(&saved.captured).unwrap();
        assert!(opened.document.dirty());
        opened.document.undo().unwrap();
        assert!(!opened.document.dirty());
        std::fs::write(&path, b"external").unwrap();
        let failure = save_utf8(
            opened.document.snapshot(),
            &path,
            Some(&saved.fingerprint),
            true,
            &WindowsFileSystem,
        );
        let Err(FileError::Conflict { staged }) = failure else {
            panic!("expected conflict")
        };
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        std::fs::remove_file(staged).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}

#[cfg(test)]
#[path = "replacement_faults.rs"]
mod replacement_faults;
