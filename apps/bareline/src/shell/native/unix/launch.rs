// SPDX-License-Identifier: MPL-2.0
//! Launch inputs: the per-user folders of an installed Bareline, the home
//! folder, and the limits on command-line paths.
//!
//! Linux follows the XDG base directories: settings and keymaps in
//! `~/.config/bareline`, the profile (session, recovery journals, recent
//! files, macros, extensions) in `~/.local/share/bareline`, diagnostics in
//! `~/.local/state/bareline/logs`, and temporary copies of documents in
//! `~/.cache/bareline` (the file system's `private_cache_root`). macOS uses
//! `~/Library/Application Support/Bareline` for settings and profile and
//! `~/Library/Logs/Bareline`. A portable copy (a `bareline.portable` file next
//! to the executable) keeps everything in its own `data` folder; the shell
//! detects that itself and asks for these folders only when it is installed.
//!
//! The folders are private to the user (0700, LNX-UI-006): they hold unsaved
//! text. Settings that earlier builds wrote to the data folder move to the
//! configuration folder once (LNX-XDG-006).
use bareline_platform_posix::paths::{APPLICATION, AppDirectories, Environment, Layout};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Where an installed Bareline keeps its profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstalledFolders {
    /// The roaming profile of earlier Windows releases, migrated into `local`;
    /// these systems have none.
    pub roaming: Option<PathBuf>,
    /// Session, recovery journals, recent files, extensions and macros: the
    /// XDG data folder.
    pub local: Option<PathBuf>,
    /// Settings and keymaps: the XDG configuration folder (the same folder as
    /// `local` on macOS).
    pub config: Option<PathBuf>,
    /// Diagnostics, when they live outside the profile folder.
    pub logs: Option<PathBuf>,
}

/// The installed folders for this user. Without a usable home folder there is
/// no profile: the shell then runs without settings, session or recovery, as
/// it does on Windows without `%LOCALAPPDATA%`.
pub fn installed_folders() -> InstalledFolders {
    match folders(&|name| std::env::var_os(name), Layout::native()) {
        Ok(folders) => {
            prepare_profile(&folders);
            InstalledFolders {
                roaming: None,
                local: Some(folders.data),
                config: Some(folders.config),
                logs: Some(folders.logs),
            }
        }
        Err(error) => {
            eprintln!("event=profile_folders_unavailable reason={error}");
            InstalledFolders::default()
        }
    }
}

/// Makes the profile folders private, creating them 0700 or narrowing ones
/// earlier builds created with the umask (LNX-UI-006), and moves settings
/// earlier builds kept in the data folder to the configuration folder
/// (LNX-XDG-006). Best effort: a profile that cannot be changed still opens,
/// as it did before.
fn prepare_profile(folders: &AppDirectories) {
    let recovery = folders.data.join("recovery");
    let mut private = vec![&folders.data, &folders.config, &folders.state, &folders.logs];
    if recovery.is_dir() {
        private.push(&recovery);
    }
    private.dedup();
    for folder in private {
        if let Err(error) = bareline_platform_posix::private::private_folder(folder, true) {
            eprintln!(
                "event=profile_folder_not_private folder={} reason={error}",
                folder.display()
            );
        }
    }
    for name in ["settings.toml", "keymap.toml"] {
        match migrate_config_file(&folders.data, &folders.config, name) {
            Ok(true) => eprintln!("event=settings_migrated file={name} to={}", folders.config.display()),
            Ok(false) => {}
            Err(error) => eprintln!("event=settings_migration_failed file={name} reason={error}"),
        }
    }
}

/// Moves `name` from the data folder, where earlier builds kept it, to the
/// configuration folder, only while the configuration folder has none: a
/// settings file written there is never replaced. Returns whether it moved.
/// Links and folders under the old name are left alone.
fn migrate_config_file(data: &Path, config: &Path, name: &str) -> io::Result<bool> {
    if data == config {
        return Ok(false);
    }
    let (from, to) = (data.join(name), config.join(name));
    match std::fs::symlink_metadata(&to) {
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    match std::fs::symlink_metadata(&from) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    std::fs::create_dir_all(config)?;
    // A new link never replaces a file another process created meanwhile.
    match std::fs::hard_link(&from, &to) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        // Another file system: copy under a temporary name, then publish it
        // the same way.
        Err(_) => {
            let staged = config.join(format!(".{name}.migrating-{}", std::process::id()));
            let copied = std::fs::read(&from).and_then(|bytes| {
                let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&staged)?;
                file.write_all(&bytes)?;
                file.sync_all()
            });
            let published = copied.and_then(|()| std::fs::hard_link(&staged, &to));
            let _ = std::fs::remove_file(&staged);
            match published {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
                Err(error) => return Err(error),
            }
        }
    }
    std::fs::remove_file(&from)?;
    Ok(true)
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

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("bareline-launch-{name}-{}", std::process::id()));
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

    #[test]
    fn xdg_settings_live_in_the_config_folder_and_the_profile_in_the_data_folder() {
        let var = |name: &str| match name {
            "HOME" => Some(OsString::from("/home/ada")),
            "XDG_CONFIG_HOME" => Some(OsString::from("/config")),
            "XDG_CACHE_HOME" => Some(OsString::from("/cache")),
            _ => None,
        };
        let xdg = folders(&var, Layout::Xdg).unwrap();
        assert_eq!(xdg.config, Path::new("/config").join(APPLICATION));
        assert_eq!(xdg.data, Path::new("/home/ada/.local/share").join(APPLICATION));
        assert_eq!(xdg.cache, Path::new("/cache").join(APPLICATION));
        assert_eq!(
            xdg.logs,
            Path::new("/home/ada/.local/state").join(APPLICATION).join("logs")
        );
        // macOS keeps settings beside the profile, as before.
        let macos = folders(&var, Layout::MacOs).unwrap();
        assert_eq!(macos.config, macos.data);
        assert_eq!(macos.cache, Path::new("/home/ada/Library/Caches").join(APPLICATION));
    }

    #[test]
    fn settings_move_from_the_data_folder_once_and_never_replace_newer_ones() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("migrate");
        let (data, config) = (scratch.0.join("data/bareline"), scratch.0.join("config/bareline"));
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("settings.toml"), "[editor]\nfont_size = 17\n").unwrap();
        std::fs::write(data.join("keymap.toml"), "# keys\n").unwrap();
        std::fs::write(data.join("session.json"), "{}").unwrap();
        assert!(migrate_config_file(&data, &config, "settings.toml").unwrap());
        assert!(migrate_config_file(&data, &config, "keymap.toml").unwrap());
        assert_eq!(
            std::fs::read_to_string(config.join("settings.toml")).unwrap(),
            "[editor]\nfont_size = 17\n"
        );
        assert_eq!(std::fs::read_to_string(config.join("keymap.toml")).unwrap(), "# keys\n");
        assert!(!data.join("settings.toml").exists() && !data.join("keymap.toml").exists());
        // The profile stays in the data folder.
        assert!(data.join("session.json").is_file());
        // Nothing left to move: a second start changes nothing.
        assert!(!migrate_config_file(&data, &config, "settings.toml").unwrap());
        // A settings file already in the config folder is never replaced.
        std::fs::write(data.join("settings.toml"), "stale").unwrap();
        assert!(!migrate_config_file(&data, &config, "settings.toml").unwrap());
        assert_eq!(
            std::fs::read_to_string(config.join("settings.toml")).unwrap(),
            "[editor]\nfont_size = 17\n"
        );
        assert_eq!(std::fs::read_to_string(data.join("settings.toml")).unwrap(), "stale");
        // A link under the old name is not followed or moved.
        let other = scratch.0.join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::os::unix::fs::symlink(data.join("settings.toml"), other.join("settings.toml")).unwrap();
        let fresh = scratch.0.join("fresh-config");
        assert!(!migrate_config_file(&other, &fresh, "settings.toml").unwrap());
        assert!(!fresh.join("settings.toml").exists());
        // The same folder (macOS) has nothing to move.
        assert!(!migrate_config_file(&data, &data, "settings.toml").unwrap());

        // The whole profile is made private, and settings move with it.
        let profile = AppDirectories {
            data: scratch.0.join("p/data"),
            config: scratch.0.join("p/config"),
            state: scratch.0.join("p/state"),
            cache: scratch.0.join("p/cache"),
            logs: scratch.0.join("p/state/logs"),
            runtime: scratch.0.join("p/run"),
            portable: false,
        };
        std::fs::create_dir_all(profile.data.join("recovery")).unwrap();
        for folder in [&profile.data, &profile.data.join("recovery")] {
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(profile.data.join("settings.toml"), "x = 1\n").unwrap();
        prepare_profile(&profile);
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        for folder in [
            &profile.data,
            &profile.data.join("recovery"),
            &profile.config,
            &profile.state,
            &profile.logs,
        ] {
            assert_eq!(mode(folder), 0o700, "{}", folder.display());
        }
        assert!(profile.config.join("settings.toml").is_file());
        assert!(!profile.data.join("settings.toml").exists());
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
