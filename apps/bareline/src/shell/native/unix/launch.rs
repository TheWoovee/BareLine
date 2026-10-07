// SPDX-License-Identifier: MPL-2.0
//! Launch inputs: the per-user folders of an installed Bareline, the home
//! folder, and the limits on command-line paths.
//!
//! Linux follows the XDG base directories (`~/.local/share/bareline` for the
//! profile, `~/.local/state/bareline/logs` for diagnostics); macOS uses
//! `~/Library/Application Support/Bareline` and `~/Library/Logs/Bareline`. A
//! portable copy (a `bareline.portable` file next to the executable) keeps
//! everything in its own `data` folder; the shell detects that itself and asks
//! for these folders only when it is installed.
use bareline_platform_posix::paths::{AppDirectories, Environment, Layout};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The application's folder name: lowercase by the XDG convention, the
/// product name on macOS.
const APPLICATION: &str = bareline_platform_posix::paths::APPLICATION;

/// Where an installed Bareline keeps its profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstalledFolders {
    /// The roaming profile of earlier Windows releases, migrated into `local`;
    /// these systems have none.
    pub roaming: Option<PathBuf>,
    /// Settings, session, recovery journals, extensions and macros.
    pub local: Option<PathBuf>,
    /// Diagnostics, when they live outside the profile folder.
    pub logs: Option<PathBuf>,
}

/// The installed folders for this user. Without a usable home folder there is
/// no profile: the shell then runs without settings, session or recovery, as
/// it does on Windows without `%LOCALAPPDATA%`.
pub fn installed_folders() -> InstalledFolders {
    match folders(&|name| std::env::var_os(name), Layout::native()) {
        Ok(folders) => InstalledFolders {
            roaming: None,
            local: Some(folders.data),
            logs: Some(folders.logs),
        },
        Err(error) => {
            eprintln!("event=profile_folders_unavailable reason={error}");
            InstalledFolders::default()
        }
    }
}

/// The per-user folders, never the portable ones: the shell reads the
/// portable marker itself.
fn folders(var: &dyn Fn(&str) -> Option<OsString>, layout: Layout) -> std::io::Result<AppDirectories> {
    AppDirectories::resolve(
        APPLICATION,
        layout,
        &Environment {
            var,
            executable_dir: None,
            uid: bareline_platform_posix::process::user_id(),
        },
    )
}

/// The user's home folder, where Run starts when nothing else names a folder.
pub fn user_home() -> Option<OsString> {
    std::env::var_os("HOME")
}

/// The longest path the handoff accepts, in bytes (Linux `PATH_MAX`).
const MAX_PATH_BYTES: usize = 4096;
/// The same limits the instance handoff enforces for every forwarded path.
pub fn valid_launch_path(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    path.is_absolute() && !bytes.is_empty() && !bytes.contains(&0) && bytes.len() <= MAX_PATH_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_paths_follow_posix_limits() {
        assert!(valid_launch_path(Path::new("/home/user/notes.txt")));
        assert!(!valid_launch_path(Path::new("notes.txt")));
        assert!(!valid_launch_path(&Path::new("/").join("a".repeat(MAX_PATH_BYTES))));
    }

    #[test]
    fn installed_folders_follow_the_platform_convention_and_ignore_the_portable_marker() {
        let var = |name: &str| match name {
            "HOME" => Some(OsString::from("/home/ada")),
            "XDG_STATE_HOME" => Some(OsString::from("/state")),
            _ => None,
        };
        let xdg = folders(&var, Layout::Xdg).unwrap();
        assert_eq!(xdg.data, Path::new("/home/ada/.local/share").join(APPLICATION));
        assert_eq!(xdg.logs, Path::new("/state").join(APPLICATION).join("logs"));
        assert!(!xdg.portable);
        let macos = folders(&var, Layout::MacOs).unwrap();
        assert_eq!(
            macos.data,
            Path::new("/home/ada/Library/Application Support").join(APPLICATION)
        );
        assert_eq!(macos.logs, Path::new("/home/ada/Library/Logs").join(APPLICATION));
        assert!(folders(&|_| None, Layout::Xdg).is_err());
    }
}
