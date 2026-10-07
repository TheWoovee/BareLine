// SPDX-License-Identifier: MPL-2.0
//! The POSIX file system, path trust and trash (PR-030, `bareline_platform_posix`).
//!
//! Document saves run the same transaction protocol as on Windows: the stage
//! is swapped onto the name in one rename where the file system supports it,
//! the displaced version is kept in the transaction folder beside the file
//! until the save is verified, and an interrupted save is found again on the
//! next start. Capability notices come from `FileSystem.report`, external
//! change checks from `FileSystem.current_identity`.
use super::window::RawWindow;
use bareline_platform::LocalFileSystem;
use std::{io, path::Path};

pub use bareline_platform_posix::{
    PosixFileSystem as FileSystem, PosixPathTrustProvider as PathTrust,
    PosixSessionPathTrustProvider as SessionPathTrust,
};

/// Moves a file or folder to the user's trash, where file managers can restore
/// it (the freedesktop.org trash on Linux, `~/.Trash` on macOS). The explorer
/// policy of rename and delete applies first, as on Windows: an absolute
/// normalized path, `fs.validate_target`, no linked folder on the way and no
/// link as the entry ("linked entries require separate authorization").
pub fn recycle_entry(fs: &dyn LocalFileSystem, path: &Path, _owner: RawWindow) -> io::Result<()> {
    bareline_platform_posix::trash::trash(fs, path).map(|_| ())
}
/// What the workspace says after `recycle_entry`, in the words of this system.
pub const RECYCLED: &str = "Moved to the Trash";

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::{FilesystemCapability, PathOrigin, PathTrustProvider};
    use std::path::PathBuf;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            // Canonical: macOS reaches the temporary folder through the /var link,
            // and path trust refuses paths that cross links.
            let root = std::fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!("bareline-native-files-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_seam_file_system_guards_folders_reports_capabilities_and_trusts_user_paths() {
        let scratch = Scratch::new("seam");
        let file = scratch.0.join("notes.txt");
        std::fs::write(&file, "text").unwrap();
        // The placeholder refused these ("directory guards unavailable").
        assert!(FileSystem.guard_directory(&scratch.0).is_ok());
        assert!(FileSystem.available_space(&scratch.0).unwrap() > 0);
        let sealed = FileSystem.open_sealed_read(&file).unwrap();
        assert_eq!(
            FileSystem.identity(&sealed).unwrap(),
            FileSystem.current_identity(&file).unwrap()
        );
        assert!(FileSystem.report(&file).is_ok());
        assert!(PathTrust.open_read(&file, PathOrigin::User).is_ok());
    }

    #[test]
    fn recycling_refuses_paths_it_cannot_place_in_the_trash() {
        let fs = FileSystem;
        let error = recycle_entry(&fs, Path::new("relative.txt"), 0).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let scratch = Scratch::new("recycle");
        let missing = scratch.0.join("missing.txt");
        // A missing entry is reported before the user's trash is touched.
        assert_eq!(
            recycle_entry(&fs, &missing, 0).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn recycling_refuses_what_rename_refuses_before_the_trash_is_touched() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fs = FileSystem;
        let scratch = Scratch::new("recycle-policy");
        let folder = scratch.0.join("real");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("notes.txt");
        std::fs::write(&file, "text").unwrap();
        let link = scratch.0.join("link.txt");
        symlink(&file, &link).unwrap();
        let linked_folder = scratch.0.join("via");
        symlink(&folder, &linked_folder).unwrap();
        let read_only = folder.join("read-only.txt");
        std::fs::write(&read_only, "kept").unwrap();
        std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o444)).unwrap();
        let stepped = folder.join("..").join("real").join("notes.txt");
        // Each refusal comes from the explorer policy before anything in the
        // user's trash is created or moved, so this test never changes it.
        // The linked folder fails its no-follow open (a refusal, or "not a
        // folder" where the system reports the link that way).
        let linked = [io::ErrorKind::PermissionDenied, io::ErrorKind::NotADirectory];
        let denied = [io::ErrorKind::PermissionDenied];
        let invalid = [io::ErrorKind::InvalidInput];
        for (path, kinds) in [
            (&link, &denied[..]),
            (&linked_folder.join("notes.txt"), &linked[..]),
            (&read_only, &denied[..]),
            (&stepped, &invalid[..]),
        ] {
            let recycled = recycle_entry(&fs, path, 0).unwrap_err();
            assert!(kinds.contains(&recycled.kind()), "{}: {recycled}", path.display());
            let renamed = fs.rename_entry(path, &scratch.0.join("renamed.txt")).unwrap_err();
            assert_eq!(renamed.kind(), recycled.kind(), "{}: {renamed}", path.display());
        }
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "text");
        assert_eq!(std::fs::read_to_string(&read_only).unwrap(), "kept");
    }
}
