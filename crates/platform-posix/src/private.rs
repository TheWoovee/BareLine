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
use crate::sys::{self, DIRECTORY, denied};
use rustix::fs::{CWD, Mode};
use std::{
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// The cache root for temporary copies of documents: the per-user cache
/// folder of the current process (the portable `data/cache` for a portable
/// copy), made private by [`private_folder`].
pub fn cache_root() -> io::Result<PathBuf> {
    let root = crate::paths::AppDirectories::for_current_process(crate::paths::APPLICATION)?.cache;
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
}
