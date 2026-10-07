// SPDX-License-Identifier: MPL-2.0
//! Owner-only creation of the folders and files that hold document text:
//! recovery journals, transcode, spill and staging caches (LNX-SEC-002,
//! LNX-UI-006).
//!
//! On Unix every folder is created with mode 0700 and every file with mode
//! 0600, whatever the umask, so other local users never read unsaved text.
//! The mode is given at creation (the umask can only narrow it), never
//! applied afterwards, so no window exists in which the entry is wider. On
//! Windows these are the ordinary calls: the profile under `%LOCALAPPDATA%`
//! and `%TEMP%` below it carry a user-only ACL that new entries inherit.
use std::{fs, io, path::Path};

/// Mode of the private folders this module creates on Unix.
pub const FOLDER_MODE: u32 = 0o700;
/// Mode of the private files this module creates on Unix.
pub const FILE_MODE: u32 = 0o600;

/// `OpenOptions` whose created files are private to the user.
pub fn file_options() -> fs::OpenOptions {
    #[allow(unused_mut)]
    let mut options = fs::OpenOptions::new();
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, FILE_MODE);
    options
}

/// `fs::create_dir` for a private folder.
pub fn create_dir(path: &Path) -> io::Result<()> {
    builder().create(path)
}

/// `fs::create_dir_all` whose missing folders, ancestors included, are private.
/// Folders that already exist keep their mode.
pub fn create_dir_all(path: &Path) -> io::Result<()> {
    builder().recursive(true).create(path)
}

fn builder() -> fs::DirBuilder {
    #[allow(unused_mut)]
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, FOLDER_MODE);
    builder
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{io::Write, os::unix::fs::PermissionsExt};

    #[test]
    fn folders_and_files_are_private_whatever_the_umask_allows() {
        let root = std::env::temp_dir().join(format!("bareline-private-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let nested = root.join("a").join("b");
        create_dir_all(&nested).unwrap();
        create_dir(&nested.join("c")).unwrap();
        let mut file = file_options()
            .write(true)
            .create_new(true)
            .open(nested.join("c").join("journal.bin"))
            .unwrap();
        file.write_all(b"unsaved text").unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        for folder in [root.join("a"), nested.clone(), nested.join("c")] {
            assert_eq!(mode(&folder), FOLDER_MODE, "{}", folder.display());
        }
        assert_eq!(mode(&nested.join("c").join("journal.bin")), FILE_MODE);
        // An existing folder is accepted as it is.
        create_dir_all(&nested).unwrap();
        fs::remove_dir_all(&root).unwrap();
    }
}
