// SPDX-License-Identifier: MPL-2.0
//! Path trust bound to opened objects. Links are resolved by `lstat` first and
//! recorded; the resulting link-free path is then opened from `/` one component
//! at a time with `openat(O_NOFOLLOW)`, keeping every ancestor descriptor. A
//! link swapped in after the resolution fails the open instead of redirecting
//! it, and the retained descriptors keep naming the approved objects.
use crate::{
    capability,
    resolve::{self, Resolved},
    sys::{self, DIRECTORY, Node, READ, WALK, denied},
};
use bareline_platform::{
    CacheDirectoryIdentity, PathOperation, PathOrigin, PathTrust, PathTrustProvider, StorageKind, TrustedRead,
};
use rustix::fs::{CWD, OFlags};
use std::{
    fs::File,
    io,
    os::fd::{AsFd, BorrowedFd},
    path::{Path, PathBuf},
};

/// Open a link-free absolute path from the root, retaining each ancestor. The
/// final component is opened with `last` plus `O_NOFOLLOW`.
pub(crate) fn walk(canonical: &Path, last: OFlags) -> io::Result<(Vec<File>, File)> {
    let names = resolve::names(canonical)?;
    let mut current = sys::open_at(CWD, "/", if names.is_empty() { last } else { WALK })?;
    let mut ancestors = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        let final_name = index + 1 == names.len();
        let flags = if final_name { last | OFlags::NOFOLLOW } else { WALK };
        let next = sys::open_at(&current, name, flags).map_err(sys::no_follow)?;
        // `O_PATH | O_NOFOLLOW` opens a link itself; it must still be a folder.
        if !final_name && !next.metadata()?.is_dir() {
            return Err(sys::changed());
        }
        ancestors.push(std::mem::replace(&mut current, next));
    }
    Ok((ancestors, current))
}

/// A folder pinned by descriptor, with its ancestors. Entries created through it
/// land in the approved folder even if its name is moved or replaced meanwhile.
pub struct DirectoryGuard {
    pub(crate) path: PathBuf,
    pub(crate) directory: File,
    #[allow(dead_code)]
    pub(crate) ancestors: Vec<File>,
}
impl DirectoryGuard {
    /// Pin `path`, resolving links on the way (system links such as macOS
    /// `/tmp` and `/var` are ordinary there); the final folder is held open.
    pub fn open(path: &Path) -> io::Result<Self> {
        let resolved = resolve::resolve(path)?;
        Self::open_resolved(&resolved.path)
    }
    pub(crate) fn open_resolved(canonical: &Path) -> io::Result<Self> {
        let (ancestors, directory) = walk(canonical, DIRECTORY)?;
        if !directory.metadata()?.is_dir() {
            return Err(denied("path is not a folder"));
        }
        Ok(Self {
            path: canonical.to_path_buf(),
            directory,
            ancestors,
        })
    }
    /// The link-free path the guard was opened at.
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn identity(&self) -> io::Result<CacheDirectoryIdentity> {
        let node = Node::of(&self.directory)?;
        Ok(CacheDirectoryIdentity {
            volume: node.dev,
            file: node.ino,
        })
    }
    /// Whether the guarded path still names the pinned folder.
    pub fn is_current(&self) -> bool {
        match (std::fs::symlink_metadata(&self.path), Node::of(&self.directory)) {
            (Ok(metadata), Ok(node)) => {
                use std::os::unix::fs::MetadataExt;
                !metadata.file_type().is_symlink() && metadata.dev() == node.dev && metadata.ino() == node.ino
            }
            _ => false,
        }
    }
}
impl AsFd for DirectoryGuard {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.directory.as_fd()
    }
}

#[derive(Default)]
pub struct PosixPathTrustProvider;
impl PosixPathTrustProvider {
    fn open_trusted(&self, path: &Path, origin: PathOrigin) -> io::Result<TrustedRead> {
        // Reject metadata-supplied paths before any filesystem access.
        if origin != PathOrigin::User || !path.is_absolute() || resolve::has_parent_step(path) {
            return Err(denied("path requires explicit supported local access"));
        }
        let Resolved {
            path: canonical,
            redirected,
            ..
        } = resolve::resolve(path)?;
        let (ancestors, file) = walk(&canonical, READ)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(denied("only files and folders can be opened"));
        }
        let storage = capability::mount_of(&file, &canonical)?.storage;
        Ok(TrustedRead {
            trust: PathTrust {
                canonical,
                storage,
                origin,
                traverses_reparse_point: redirected,
            },
            file,
            ancestors,
        })
    }

    /// A follow reader pins the folder chain but lets writers and rotation change
    /// the final file. The file is opened below the pinned parent descriptor
    /// without following a link, so a renamed-in link is refused.
    pub fn open_follow_read(&self, path: &Path) -> io::Result<(File, std::sync::Arc<dyn Send + Sync>)> {
        let (parent, name) = sys::split(path)?;
        let guard = self.open_trusted(parent, PathOrigin::User)?;
        if !guard.file.metadata()?.is_dir() {
            return Err(denied("the parent is not a folder"));
        }
        let file = sys::open_at(&guard.file, name, READ).map_err(sys::no_follow)?;
        if !file.metadata()?.is_file() {
            return Err(denied("only regular files can be followed"));
        }
        Ok((file, std::sync::Arc::new(guard)))
    }
}
impl PathTrustProvider for PosixPathTrustProvider {
    fn canonicalize(&self, path: &Path, origin: PathOrigin) -> io::Result<PathTrust> {
        Ok(self.open_trusted(path, origin)?.trust)
    }
    fn open_read(&self, path: &Path, origin: PathOrigin) -> io::Result<TrustedRead> {
        self.open_trusted(path, origin)
    }
    /// Mounted network filesystems are reachable like local ones on POSIX; there
    /// is no implicit credential exchange with a named host as with UNC paths.
    fn permits(&self, trust: &PathTrust, operation: PathOperation) -> bool {
        trust.origin == PathOrigin::User
            && !trust.traverses_reparse_point
            && matches!(
                trust.storage,
                StorageKind::Local | StorageKind::Removable | StorageKind::Network
            )
            && matches!(operation, PathOperation::Read | PathOperation::Write)
    }
}

/// Application-owned restore policy, as on Windows: instantiate only for the
/// application's own persisted session. It grants reads only.
pub struct PosixSessionPathTrustProvider;
impl PathTrustProvider for PosixSessionPathTrustProvider {
    fn canonicalize(&self, path: &Path, origin: PathOrigin) -> io::Result<PathTrust> {
        Ok(self.open_read(path, origin)?.trust)
    }
    fn open_read(&self, path: &Path, origin: PathOrigin) -> io::Result<TrustedRead> {
        if origin != PathOrigin::Session {
            return Err(denied("path requires explicit supported local access"));
        }
        let mut opened = PosixPathTrustProvider.open_trusted(path, PathOrigin::User)?;
        opened.trust.origin = PathOrigin::Session;
        Ok(opened)
    }
    fn permits(&self, trust: &PathTrust, operation: PathOperation) -> bool {
        trust.origin == PathOrigin::Session
            && !trust.traverses_reparse_point
            && matches!(
                trust.storage,
                StorageKind::Local | StorageKind::Removable | StorageKind::Network
            )
            && matches!(operation, PathOperation::Read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, os::unix::fs::symlink};

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("bareline-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn metadata_paths_and_other_origins_are_refused_before_access() {
        for path in ["relative.txt", "/tmp/../etc/passwd"] {
            assert!(
                PosixPathTrustProvider
                    .canonicalize(Path::new(path), PathOrigin::User)
                    .is_err(),
                "{path}"
            );
        }
        for origin in [PathOrigin::Session, PathOrigin::Extension] {
            assert!(PosixPathTrustProvider.canonicalize(Path::new("/"), origin).is_err());
        }
    }

    #[test]
    fn local_file_is_classified_and_execute_is_denied() {
        let root = scratch("local");
        let path = root.join("doc.txt");
        std::fs::write(&path, b"approved").unwrap();
        let trust = PosixPathTrustProvider.canonicalize(&path, PathOrigin::User).unwrap();
        assert_eq!(trust.canonical, path);
        assert!(!trust.traverses_reparse_point);
        assert!(matches!(trust.storage, StorageKind::Local | StorageKind::Removable));
        assert!(PosixPathTrustProvider.permits(&trust, PathOperation::Read));
        assert!(PosixPathTrustProvider.permits(&trust, PathOperation::Write));
        assert!(!PosixPathTrustProvider.permits(&trust, PathOperation::Execute));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinked_path_is_recorded_and_not_permitted() {
        let root = scratch("linked");
        std::fs::create_dir(root.join("real")).unwrap();
        std::fs::write(root.join("real/doc.txt"), b"behind a link").unwrap();
        symlink(root.join("real"), root.join("link")).unwrap();
        let mut opened = PosixPathTrustProvider
            .open_read(&root.join("link/doc.txt"), PathOrigin::User)
            .unwrap();
        assert!(opened.trust.traverses_reparse_point);
        assert_eq!(opened.trust.canonical, root.join("real/doc.txt"));
        assert!(!PosixPathTrustProvider.permits(&opened.trust, PathOperation::Read));
        let mut bytes = Vec::new();
        opened.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"behind a link");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retained_read_keeps_the_approved_object_after_the_name_is_replaced() {
        let root = scratch("retarget");
        let path = root.join("doc.txt");
        std::fs::write(&path, b"approved").unwrap();
        let mut opened = PosixPathTrustProvider.open_read(&path, PathOrigin::User).unwrap();
        std::fs::rename(&path, root.join("moved.txt")).unwrap();
        std::fs::write(&path, b"substituted").unwrap();
        let mut bytes = Vec::new();
        opened.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"approved");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_policy_reads_only() {
        let root = scratch("session");
        let provider = PosixSessionPathTrustProvider;
        let trust = provider.canonicalize(&root, PathOrigin::Session).unwrap();
        assert!(provider.permits(&trust, PathOperation::Read));
        assert!(!provider.permits(&trust, PathOperation::Write));
        assert!(provider.open_read(&root, PathOrigin::User).is_err());
        assert!(provider.open_read(&root, PathOrigin::Extension).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn follow_reader_refuses_a_final_link() {
        let root = scratch("follow");
        std::fs::write(root.join("log.txt"), b"line\n").unwrap();
        symlink(root.join("log.txt"), root.join("alias.txt")).unwrap();
        let (mut file, _guard) = PosixPathTrustProvider.open_follow_read(&root.join("log.txt")).unwrap();
        let mut bytes = String::new();
        file.read_to_string(&mut bytes).unwrap();
        assert_eq!(bytes, "line\n");
        let error = PosixPathTrustProvider
            .open_follow_read(&root.join("alias.txt"))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        std::fs::remove_dir_all(root).unwrap();
    }
}
