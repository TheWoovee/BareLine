// SPDX-License-Identifier: MPL-2.0
//! Private per-user folders and shared sealed copies (LNX-SEC-002,
//! LNX-UI-006, LNX-DISK-004).
//!
//! Temporary copies of documents live in the user's cache folder
//! (`$XDG_CACHE_HOME/bareline`, `~/Library/Caches/Bareline`), never in the
//! shared `/tmp`. `$XDG_RUNTIME_DIR` was considered and not chosen: it is a
//! small RAM-backed tmpfs (10% of memory by default) cleared at logout, while a
//! transcode copy is as large as the document it holds. The cache folder is on
//! disk and survives a crash or a reboot, so copies a process left behind are
//! swept by the owned-cache sweep at the next start. Either way the folder is
//! created 0700 and used only once it is proven a real folder owned by this
//! user that nobody else can write.
//!
//! A portable copy uses the same per-user cache, not its `data/cache`:
//! portable media may be read-only or (FAT, exFAT) unable to keep a folder
//! private, and copies of documents do not belong on removable media.
use crate::{
    paths::{APPLICATION, AppDirectories, Environment, Layout},
    sys::{self, DIRECTORY, denied},
};
use rustix::fs::{CWD, Mode};
use std::{
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// The cache root for temporary copies of documents: the user's cache folder,
/// made private by [`private_folder`], also for a portable copy.
pub fn cache_root() -> io::Result<PathBuf> {
    select_cache_root(
        Layout::native(),
        &|name| std::env::var_os(name),
        rustix::process::getuid().as_raw(),
    )
}

/// The per-user cache folder of `layout`, resolved without the portable marker
/// (no executable folder is given), so a portable copy never keeps document
/// copies on its media.
fn select_cache_root(
    layout: Layout,
    var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    uid: u32,
) -> io::Result<PathBuf> {
    let root = AppDirectories::resolve(
        APPLICATION,
        layout,
        &Environment {
            var,
            executable_dir: None,
            uid,
        },
    )?
    .cache;
    private_folder(&root, false)?;
    Ok(root)
}

/// Create `path` and its missing ancestors with mode 0700, or check an
/// existing folder. The final name must be a real folder (a link there is
/// refused, `O_NOFOLLOW`) owned by this user. A folder other users can only
/// read or search loses their access through the verified descriptor. One
/// they can write to is refused, since entries planted in it cannot be
/// trusted, unless `adopt_writable`, which the user's own profile folders use:
/// narrowing them is always safer than leaving them open.
pub fn private_folder(path: &Path, adopt_writable: bool) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a private folder needs an absolute path",
        ));
    }
    bareline_platform::private::create_dir_all(path)?;
    let folder = sys::open_at(CWD, path, DIRECTORY).map_err(|error| {
        if sys::is_errno(&error, rustix::io::Errno::LOOP) || sys::is_errno(&error, rustix::io::Errno::NOTDIR) {
            denied("the private folder is a link or not a folder")
        } else {
            error
        }
    })?;
    let metadata = folder.metadata()?;
    if !metadata.is_dir() {
        return Err(denied("the private folder is not a folder"));
    }
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(denied("the private folder belongs to another user"));
    }
    let mode = metadata.mode() & 0o7777;
    if mode & 0o022 != 0 && !adopt_writable {
        return Err(denied("other users can change the private folder"));
    }
    if mode & 0o077 != 0 {
        // Group and other access goes; the owner's own bits (a read-only
        // profile stays read-only) are kept.
        let owner = Mode::from_raw_mode(rustix::fs::fstat(&folder)?.st_mode) & Mode::RWXU;
        rustix::fs::fchmod(&folder, owner)?;
    }
    Ok(())
}

/// Give `target`, a new name, the bytes of the sealed regular file `source`
/// without copying them: a copy-on-write clone (`FICLONE`) where the Linux
/// file system has one, else a hard link to the same file. A failure leaves
/// no `target`.
pub fn share_sealed_file(source: &Path, target: &Path) -> io::Result<()> {
    let input = sys::open_at(CWD, source, sys::READ).map_err(sys::no_follow)?;
    let metadata = input.metadata()?;
    if !metadata.is_file() {
        return Err(denied("a sealed file must be a regular file"));
    }
    #[cfg(target_os = "linux")]
    {
        let output = sys::open_at(CWD, target, sys::CREATE)?;
        // A clone is a new file whose extents are durable only once synced; a
        // manifest naming it is published right after (the copy it replaces
        // was synced too).
        match rustix::fs::ioctl_ficlone(&output, &input).and_then(|()| rustix::fs::fsync(&output)) {
            Ok(()) => return Ok(()),
            Err(_) => {
                drop(output);
                rustix::fs::unlinkat(CWD, target, rustix::fs::AtFlags::empty())?;
            }
        }
    }
    // Without AT_SYMLINK_FOLLOW the link names `source` itself; it must still
    // be the file opened and checked above.
    rustix::fs::linkat(CWD, source, CWD, target, rustix::fs::AtFlags::empty())?;
    let linked = std::fs::symlink_metadata(target)?;
    if linked.dev() != metadata.dev() || linked.ino() != metadata.ino() {
        let _ = rustix::fs::unlinkat(CWD, target, rustix::fs::AtFlags::empty());
        return Err(sys::changed());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("bareline-private-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn private_folders_are_created_0700_with_their_missing_ancestors() {
        let scratch = Scratch::new("create");
        let root = scratch.0.join("cache").join("bareline");
        private_folder(&root, false).unwrap();
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&scratch.0.join("cache")), 0o700);
        private_folder(&root, false).unwrap();
        assert!(private_folder(Path::new("relative/cache"), false).is_err());
    }

    #[test]
    fn readable_folders_are_narrowed_and_writable_or_linked_ones_refused() {
        let scratch = Scratch::new("refuse");
        let readable = scratch.0.join("readable");
        std::fs::create_dir(&readable).unwrap();
        std::fs::set_permissions(&readable, std::fs::Permissions::from_mode(0o755)).unwrap();
        private_folder(&readable, false).unwrap();
        assert_eq!(mode(&readable), 0o700);

        let writable = scratch.0.join("writable");
        std::fs::create_dir(&writable).unwrap();
        std::fs::set_permissions(&writable, std::fs::Permissions::from_mode(0o777)).unwrap();
        let refused = private_folder(&writable, false).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(mode(&writable), 0o777, "a refused folder is left as it was");
        // The user's own profile folder is adopted and narrowed instead.
        private_folder(&writable, true).unwrap();
        assert_eq!(mode(&writable), 0o700);

        let target = scratch.0.join("target");
        std::fs::create_dir(&target).unwrap();
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            private_folder(&link, true).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let file = scratch.0.join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(private_folder(&file, true).is_err());
    }

    /// A portable copy keeps its document copies in the user's cache folder:
    /// its own `data/cache` may be on read-only media, or on a file system
    /// (FAT, exFAT) whose modes cannot be made private.
    #[test]
    fn portable_copies_keep_document_copies_in_the_user_cache() {
        let scratch = Scratch::new("portable");
        let (home, cache) = (scratch.0.join("home"), scratch.0.join("xdg-cache"));
        let executable = scratch.0.join("stick");
        std::fs::create_dir_all(executable.join("data").join("cache")).unwrap();
        std::fs::write(executable.join(crate::paths::PORTABLE_MARKER), b"").unwrap();
        let var = |name: &str| match name {
            "HOME" => Some(home.clone().into_os_string()),
            "XDG_CACHE_HOME" => Some(cache.clone().into_os_string()),
            _ => None,
        };
        // The portable layout really does name the media's own cache folder.
        let portable = AppDirectories::resolve(
            APPLICATION,
            Layout::Xdg,
            &Environment {
                var: &var,
                executable_dir: Some(&executable),
                uid: 1000,
            },
        )
        .unwrap();
        assert!(portable.portable);
        assert_eq!(portable.cache, executable.join("data").join("cache"));

        // Media whose cache folder others can write (as FAT and exFAT report),
        // then read-only media: neither is used.
        std::fs::set_permissions(
            executable.join("data").join("cache"),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        for media_mode in [0o777, 0o555] {
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(media_mode)).unwrap();
            let root = select_cache_root(Layout::Xdg, &var, 1000).unwrap();
            assert_eq!(root, cache.join(APPLICATION));
            assert_eq!(mode(&root), 0o700);
            assert_eq!(mode(&executable.join("data").join("cache")), 0o777);
            assert_eq!(
                std::fs::read_dir(executable.join("data").join("cache"))
                    .unwrap()
                    .count(),
                0
            );
        }
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        // macOS: ~/Library/Caches/Bareline, whatever the marker says.
        let macos = select_cache_root(Layout::MacOs, &var, 1000).unwrap();
        assert_eq!(macos, home.join("Library").join("Caches").join(APPLICATION));
        // Without a home folder there is no private cache, never a fallback to the media.
        assert!(select_cache_root(Layout::Xdg, &|_| None, 1000).is_err());
        // A per-user cache folder others can write is refused, not narrowed.
        std::fs::set_permissions(cache.join(APPLICATION), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(
            select_cache_root(Layout::Xdg, &var, 1000).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn shared_sealed_files_hold_the_same_bytes_without_a_second_write() {
        let scratch = Scratch::new("share");
        let source = scratch.0.join("original.raw");
        std::fs::write(&source, b"sealed bytes").unwrap();
        let target = scratch.0.join("text.utf8");
        share_sealed_file(&source, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"sealed bytes");
        let (source_meta, target_meta) = (std::fs::metadata(&source).unwrap(), std::fs::metadata(&target).unwrap());
        // A hard link shares the file; a clone is a new file of the same length.
        assert!(source_meta.ino() == target_meta.ino() || target_meta.len() == source_meta.len());
        // An existing target is never replaced, and a link is never shared.
        assert!(share_sealed_file(&source, &target).is_err());
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert!(share_sealed_file(&link, &scratch.0.join("other")).is_err());
        assert!(!scratch.0.join("other").exists());
    }
}
