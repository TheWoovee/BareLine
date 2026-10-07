// SPDX-License-Identifier: MPL-2.0
//! Explicit explorer mutations below a retained, link-free folder chain. Rename
//! never replaces a destination; delete removes one entry and refuses a folder
//! that is not empty. Linked entries need separate authorization, as on Windows.
use crate::{
    resolve,
    sys::{self, CREATE, denied},
    trust::DirectoryGuard,
};
use bareline_platform::LocalFileSystem;
use rustix::fs::{AtFlags, FileType, Mode};
use std::{
    ffi::OsStr,
    io,
    path::{Component, Path},
};

/// The pinned parent of `path` and its final name. A linked folder on the way
/// is refused unless the link is part of the system layout (macOS `/var` and
/// `/tmp`); the resolved, link-free chain is then opened without following
/// links, so a link swapped in meanwhile is refused too.
pub(crate) fn parent<'a>(fs: &dyn LocalFileSystem, path: &'a Path) -> io::Result<(DirectoryGuard, &'a OsStr)> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "absolute normalized path required",
        ));
    }
    fs.validate_target(path)?;
    let (parent, name) = sys::split(path).map_err(|_| denied("the root folder cannot be changed"))?;
    resolve::names(parent)?;
    let resolved = resolve::resolve(parent)?;
    if resolved.redirected {
        return Err(denied("symbolic links are not followed here"));
    }
    Ok((DirectoryGuard::open_resolved(&resolved.path)?, name))
}

pub(crate) fn create(fs: &dyn LocalFileSystem, path: &Path, directory: bool) -> io::Result<()> {
    let (parent, name) = parent(fs, path)?;
    if directory {
        rustix::fs::mkdirat(&parent, name, Mode::from_bits_truncate(0o777))?;
    } else {
        let file = std::fs::File::from(rustix::fs::openat(
            &parent,
            name,
            CREATE,
            Mode::from_bits_truncate(0o666),
        )?);
        file.sync_all()?;
    }
    sys::sync_directory(&parent.directory)
}

pub(crate) fn unlinked_entry(parent: &DirectoryGuard, name: &OsStr) -> io::Result<FileType> {
    let kind = sys::stat_name(parent, name)?.kind;
    if kind == FileType::Symlink {
        return Err(denied("linked entries require separate authorization"));
    }
    Ok(kind)
}

pub(crate) fn rename(fs: &dyn LocalFileSystem, source: &Path, target: &Path) -> io::Result<()> {
    let (source_parent, source_name) = parent(fs, source)?;
    let (target_parent, target_name) = parent(fs, target)?;
    unlinked_entry(&source_parent, source_name)?;
    sys::rename_no_replace(&source_parent, source_name, &target_parent, target_name)?;
    sys::sync_directory(&target_parent.directory)?;
    sys::sync_directory(&source_parent.directory)
}

pub(crate) fn delete(fs: &dyn LocalFileSystem, path: &Path) -> io::Result<()> {
    let (parent, name) = parent(fs, path)?;
    let flags = if unlinked_entry(&parent, name)? == FileType::Directory {
        AtFlags::REMOVEDIR
    } else {
        AtFlags::empty()
    };
    rustix::fs::unlinkat(&parent, name, flags)?;
    sys::sync_directory(&parent.directory)
}

#[cfg(test)]
mod tests {
    use crate::PosixFileSystem;
    use bareline_platform::LocalFileSystem;

    #[test]
    fn no_replace_and_nonrecursive_deletion() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("bareline-workspace-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        PosixFileSystem.create_entry(&root, true).unwrap();
        let (a, b, c) = (root.join("a"), root.join("b"), root.join("c"));
        PosixFileSystem.create_entry(&a, false).unwrap();
        PosixFileSystem.create_entry(&b, false).unwrap();
        assert!(PosixFileSystem.create_entry(&a, false).is_err());
        std::fs::write(&a, b"source content").unwrap();
        std::fs::write(&b, b"destination content").unwrap();
        assert!(PosixFileSystem.rename_entry(&a, &b).is_err());
        assert_eq!(std::fs::read(&a).unwrap(), b"source content");
        assert_eq!(std::fs::read(&b).unwrap(), b"destination content");
        assert!(PosixFileSystem.delete_entry(&root).is_err());
        PosixFileSystem.rename_entry(&a, &c).unwrap();
        assert!(!a.exists() && c.exists());
        std::os::unix::fs::symlink(&c, root.join("link")).unwrap();
        assert!(PosixFileSystem.delete_entry(&root.join("link")).is_err());
        std::fs::remove_file(root.join("link")).unwrap();
        PosixFileSystem.delete_entry(&c).unwrap();
        PosixFileSystem.delete_entry(&b).unwrap();
        PosixFileSystem.delete_entry(&root).unwrap();
        assert!(!root.exists());
    }

    /// macOS keeps the temporary folder behind the system's `/var` and `/tmp`
    /// links (`/var/lock` on Linux): entries there are created, renamed and
    /// deleted, while a folder the person linked on the way is still refused.
    #[test]
    fn system_links_on_the_way_are_followed_and_others_refused() {
        let unique = format!("bareline-entry-links-{}", std::process::id());
        let mut checked = Vec::new();
        for base in [
            std::env::temp_dir(),
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/var/lock"),
        ] {
            // Only a base reached through the system's links exercises the rule.
            if !crate::trust::tests::behind_root_links(&base) || checked.contains(&base) {
                continue;
            }
            let root = base.join(&unique);
            let _ = std::fs::remove_dir_all(&root);
            PosixFileSystem.create_entry(&root, true).unwrap();
            let (a, b) = (root.join("a.txt"), root.join("b.txt"));
            PosixFileSystem.create_entry(&a, false).unwrap();
            PosixFileSystem.rename_entry(&a, &b).unwrap();
            assert!(!a.exists() && b.exists());
            PosixFileSystem.delete_entry(&b).unwrap();
            PosixFileSystem.delete_entry(&root).unwrap();
            assert!(!root.exists());
            checked.push(base);
        }
        eprintln!("bases behind system links: {checked:?}");

        let root = std::env::temp_dir().canonicalize().unwrap().join(&unique);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join("real/a.txt"), b"kept").unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let error = PosixFileSystem
            .rename_entry(&root.join("link/a.txt"), &root.join("link/b.txt"))
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(PosixFileSystem.delete_entry(&root.join("link/a.txt")).is_err());
        assert_eq!(std::fs::read(root.join("real/a.txt")).unwrap(), b"kept");
        std::fs::remove_dir_all(root).unwrap();
    }
}
