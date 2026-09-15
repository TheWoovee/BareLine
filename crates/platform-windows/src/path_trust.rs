// SPDX-License-Identifier: MPL-2.0
//! Conservative path classification before any destination access. Grants are action-scoped.
use bareline_platform::{PathOperation, PathOrigin, PathTrust, PathTrustProvider, StorageKind, TrustedRead};
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Component, Path, Prefix},
};
use windows::{
    Win32::{Foundation::HANDLE, Storage::FileSystem::*},
    core::PCWSTR,
};

#[derive(Default)]
pub struct WindowsPathTrustProvider;
impl WindowsPathTrustProvider {
    /// Pin a destination directory without requesting directory-list access.
    /// All ancestors still reject reparse/offline paths and deny write/delete sharing.
    pub(crate) fn guard_directory(&self, path: &Path) -> io::Result<TrustedRead> {
        let guard = self.open_pinned(path, PathOrigin::User, FILE_READ_ATTRIBUTES.0)?;
        if !guard.file.metadata()?.is_dir() {
            return Err(denied());
        }
        Ok(guard)
    }
    pub(crate) fn guard_migration_entry(&self, path: &Path) -> io::Result<TrustedRead> {
        self.open_pinned_shared(
            path,
            PathOrigin::User,
            FILE_READ_ATTRIBUTES.0 | DELETE.0,
            FILE_SHARE_READ.0,
            FILE_SHARE_READ.0 | FILE_SHARE_DELETE.0,
        )
    }
    fn open_pinned(&self, path: &Path, origin: PathOrigin, final_access: u32) -> io::Result<TrustedRead> {
        self.open_pinned_shared(path, origin, final_access, FILE_SHARE_READ.0, FILE_SHARE_READ.0)
    }
    fn open_pinned_shared(
        &self,
        path: &Path,
        origin: PathOrigin,
        final_access: u32,
        final_sharing: u32,
        ancestor_sharing: u32,
    ) -> io::Result<TrustedRead> {
        // Reject metadata-supplied paths before canonicalization, metadata, shell or network calls.
        if origin != PathOrigin::User || !path.is_absolute() {
            return Err(denied());
        }
        let drive = match path.components().next() {
            Some(Component::Prefix(p)) => match p.kind() {
                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => d,
                _ => return Err(denied()),
            },
            _ => return Err(denied()),
        };
        if path.components().any(|c| matches!(c, Component::ParentDir) || matches!(c, Component::Normal(s) if (reserved_device(s) || s.encode_wide().any(|u| u == 0 || u == b':' as u16)))) { return Err(denied()); }
        let root = [drive as u16, b':' as u16, b'\\' as u16, 0];
        // SAFETY: root is a terminated drive root, not a remote destination.
        let storage = match unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } {
            3 => StorageKind::Local,
            2 => StorageKind::Removable,
            _ => return Err(denied()),
        };
        // Walk root first: never query a descendant through an unchecked junction.
        // Holding each handle without delete/write sharing blocks ordinary concurrent retargeting.
        let mut held: Vec<File> = Vec::new();
        for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let file = OpenOptions::new()
                .access_mode(if ancestor == path {
                    final_access
                } else {
                    FILE_READ_ATTRIBUTES.0
                })
                .share_mode(if ancestor == path {
                    final_sharing
                } else {
                    ancestor_sharing
                })
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
                .open(ancestor)?;
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            // SAFETY: owned handle and valid output structure.
            unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
                .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))?;
            if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0) != 0 {
                return Err(denied());
            }
            held.push(file);
        }
        let canonical = path.canonicalize()?;
        let trust = PathTrust {
            canonical,
            storage,
            origin,
            traverses_reparse_point: false,
        };
        let file = held.pop().ok_or_else(denied)?;
        Ok(TrustedRead {
            trust,
            file,
            ancestors: held,
        })
    }

    /// A follow reader pins the approved directory chain but permits writers and
    /// rotation of the final file. OPEN_REPARSE_POINT prevents a final-name race from
    /// authenticating to a substituted remote target.
    pub fn open_follow_read(&self, path: &Path) -> io::Result<(File, std::sync::Arc<dyn Send + Sync>)> {
        let name = path.file_name().ok_or_else(denied)?;
        if reserved_device(name) || name.encode_wide().any(|u| u == 0 || u == b':' as u16) {
            return Err(denied());
        }
        let parent = path.parent().ok_or_else(denied)?;
        let guard = self.open_read(parent, PathOrigin::User)?;
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: file owns the handle and the output buffer lives through the call.
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))?;
        if info.dwFileAttributes
            & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0 | FILE_ATTRIBUTE_DIRECTORY.0)
            != 0
        {
            return Err(denied());
        }
        Ok((file, std::sync::Arc::new(guard)))
    }
}
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "path requires explicit supported local access",
    )
}
fn reserved_device(name: &std::ffi::OsStr) -> bool {
    let units: Vec<u16> = name.encode_wide().take_while(|u| *u != b'.' as u16).collect();
    let base = String::from_utf16_lossy(&units)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|prefix| {
            base.strip_prefix(prefix).is_some_and(|n| {
                n.chars().count() == 1 && n.chars().all(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
            })
        })
}
impl PathTrustProvider for WindowsPathTrustProvider {
    fn canonicalize(&self, path: &Path, origin: PathOrigin) -> io::Result<PathTrust> {
        Ok(self.open_read(path, origin)?.trust)
    }
    fn open_read(&self, path: &Path, origin: PathOrigin) -> io::Result<TrustedRead> {
        self.open_pinned(path, origin, FILE_GENERIC_READ.0)
    }
    fn permits(&self, trust: &PathTrust, operation: PathOperation) -> bool {
        trust.origin == PathOrigin::User
            && !trust.traverses_reparse_point
            && matches!(trust.storage, StorageKind::Local | StorageKind::Removable)
            && matches!(operation, PathOperation::Read | PathOperation::Write)
    }
}
/// Explicit application-owned restore policy. Instantiate only after validating
/// manifest provenance as the application's own persisted session and approving
/// local restore. Imported manifests cannot select this policy. It grants reads
/// only; UNC, mapped drives, reparse points and extension origins remain denied.
pub struct WindowsSessionPathTrustProvider;
impl PathTrustProvider for WindowsSessionPathTrustProvider {
    fn canonicalize(&self, path: &Path, origin: PathOrigin) -> io::Result<PathTrust> {
        Ok(self.open_read(path, origin)?.trust)
    }
    fn open_read(&self, path: &Path, origin: PathOrigin) -> io::Result<TrustedRead> {
        if origin != PathOrigin::Session {
            return Err(denied());
        }
        let mut opened = WindowsPathTrustProvider.open_read(path, PathOrigin::User)?;
        opened.trust.origin = PathOrigin::Session;
        Ok(opened)
    }
    fn permits(&self, trust: &PathTrust, operation: PathOperation) -> bool {
        trust.origin == PathOrigin::Session
            && !trust.traverses_reparse_point
            && matches!(trust.storage, StorageKind::Local | StorageKind::Removable)
            && matches!(operation, PathOperation::Read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_and_metadata_paths_rejected_without_access() {
        for p in [
            r"\\never-contact.invalid\share\x",
            r"\\?\UNC\never-contact.invalid\share\x",
            r"\\.\pipe\x",
            r"C:\x:stream",
            r"C:\NUL.txt",
            r"C:\CON",
            r"C:\a\..\x",
        ] {
            assert!(
                WindowsPathTrustProvider
                    .canonicalize(Path::new(p), PathOrigin::User)
                    .is_err()
            );
        }
        for origin in [PathOrigin::Session, PathOrigin::Extension] {
            assert!(
                WindowsPathTrustProvider
                    .canonicalize(Path::new(r"C:\"), origin)
                    .is_err()
            );
        }
    }
    #[test]
    fn retained_read_prevents_retarget_and_reads_approved_object() {
        use std::io::Read;
        let path = std::env::temp_dir().join(format!("bareline-trust-retain-{}", std::process::id()));
        std::fs::write(&path, b"approved").unwrap();
        let mut opened = WindowsPathTrustProvider.open_read(&path, PathOrigin::User).unwrap();
        assert!(std::fs::rename(&path, path.with_extension("moved")).is_err());
        let mut bytes = Vec::new();
        opened.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"approved");
        drop(opened);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn approved_local_session_is_read_only_and_remote_stays_blocked() {
        let p = WindowsSessionPathTrustProvider;
        let trust = p
            .canonicalize(&std::env::current_dir().unwrap(), PathOrigin::Session)
            .unwrap();
        assert!(p.permits(&trust, PathOperation::Read));
        assert!(!p.permits(&trust, PathOperation::Write));
        assert!(!p.permits(&trust, PathOperation::Execute));
        assert!(
            p.open_read(Path::new(r"\\never-contact.invalid\share\x"), PathOrigin::Session)
                .is_err()
        );
        assert!(p.open_read(&trust.canonical, PathOrigin::Extension).is_err());
    }
    #[test]
    fn real_local_path_classified_and_execute_denied() {
        let trust = WindowsPathTrustProvider
            .canonicalize(&std::env::current_dir().unwrap(), PathOrigin::User)
            .unwrap();
        assert_eq!(trust.storage, StorageKind::Local);
        assert!(trust.canonical.is_absolute());
        assert!(WindowsPathTrustProvider.permits(&trust, PathOperation::Read));
        assert!(!WindowsPathTrustProvider.permits(&trust, PathOperation::Execute));
    }
}

impl WindowsPathTrustProvider {
    /// This method is reachable only with an admitted exact-path read capability.
    /// Every ancestor is opened no-follow and pinned before its descendant.
    pub(crate) fn open_remote_read(
        &self,
        path: &Path,
        access: &bareline_platform::RemoteReadAccess,
    ) -> io::Result<TrustedRead> {
        access.check(path)?;
        if !path.is_absolute() {
            return Err(denied());
        }
        match path.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                    if server.is_empty()
                        || share.is_empty()
                        || [server, share]
                            .iter()
                            .any(|part| part.encode_wide().any(|u| u == 0 || u == b':' as u16))
                    {
                        return Err(denied());
                    }
                }
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                    let root = [drive as u16, b':' as u16, b'\\' as u16, 0];
                    if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != 4 {
                        return Err(denied());
                    }
                }
                _ => return Err(denied()),
            },
            _ => return Err(denied()),
        }
        if path.components().any(|part|matches!(part,Component::ParentDir)|matches!(part,Component::Normal(name) if reserved_device(name)||name.encode_wide().any(|u|u==0||u==b':' as u16))){return Err(denied());}
        let mut held = Vec::new();
        for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
            access.check(path)?;
            let final_file = ancestor == path;
            let sharing = if final_file && access.action() == bareline_platform::RemoteReadAction::Follow {
                FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0
            } else {
                FILE_SHARE_READ.0
            };
            let file = OpenOptions::new()
                .access_mode(if final_file {
                    FILE_GENERIC_READ.0
                } else {
                    FILE_READ_ATTRIBUTES.0
                })
                .share_mode(sharing)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
                .open(ancestor)?;
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            // SAFETY: the file retains its handle and the output buffer is live.
            unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
                .map_err(|error| io::Error::from_raw_os_error(error.code().0 & 0xffff))?;
            if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0) != 0
                || final_file && info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0
            {
                return Err(denied());
            }
            held.push(file);
        }
        access.check(path)?;
        let file = held.pop().ok_or_else(denied)?;
        Ok(TrustedRead {
            trust: PathTrust {
                canonical: path.into(),
                storage: StorageKind::Network,
                origin: PathOrigin::User,
                traverses_reparse_point: false,
            },
            file,
            ancestors: held,
        })
    }
}

#[cfg(test)]
mod remote_rejection_tests {
    use super::*;
    #[test]
    fn unsafe_remote_components_fail_before_destination_access() {
        use bareline_platform::{RemoteReadAction, RemoteReadGrant};
        for path in [
            r"\\never-contact.invalid\share\..\file",
            r"\\never-contact.invalid\share\file:stream",
            r"\\never-contact.invalid\share\NUL",
        ] {
            let path = Path::new(path);
            let grant =
                RemoteReadGrant::after_consent(path.into(), RemoteReadAction::Open, std::time::Duration::from_secs(1))
                    .unwrap();
            let access = grant
                .claim(path, RemoteReadAction::Open, std::sync::Arc::new(|| false))
                .unwrap();
            assert!(WindowsPathTrustProvider.open_remote_read(path, &access).is_err());
        }
    }
}
