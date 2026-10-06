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

/// The pinned parent of `path` and its final name. Every ancestor is opened
/// without following links, so a linked folder on the way is refused.
fn parent<'a>(fs: &dyn LocalFileSystem, path: &'a Path) -> io::Result<(DirectoryGuard, &'a OsStr)> {
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
    Ok((DirectoryGuard::open_resolved(parent)?, name))
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

fn unlinked_entry(parent: &DirectoryGuard, name: &OsStr) -> io::Result<FileType> {
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
}
