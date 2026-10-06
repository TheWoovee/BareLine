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
/// it (the freedesktop.org trash on Linux, `~/.Trash` on macOS). A link moves
/// as itself; its target stays.
pub fn recycle_entry(_fs: &dyn LocalFileSystem, path: &Path, _owner: RawWindow) -> io::Result<()> {
    bareline_platform_posix::trash::trash(path).map(|_| ())
}

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
}
