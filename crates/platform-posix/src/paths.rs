// SPDX-License-Identifier: MPL-2.0
//! Per-user data folders on Linux and macOS.
//!
//! Linux follows the XDG Base Directory specification: `XDG_DATA_HOME`,
//! `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME` and `XDG_RUNTIME_DIR`
//! override `~/.local/share`, `~/.config`, `~/.local/state` and `~/.cache`;
//! relative values are invalid and ignored. Logs live in the state folder.
//! macOS uses `~/Library/Application Support/<app>` for the profile,
//! `~/Library/Caches/<app>`, `~/Library/Logs/<app>` and the per-user `TMPDIR`.
//!
//! Portable mode mirrors Windows (README "Portable mode"): a file named
//! `bareline.portable` next to the executable keeps the profile in a `data`
//! folder there, with `cache` and `diagnostics` (logs) below it.
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

pub const PORTABLE_MARKER: &str = "bareline.portable";

/// Folder conventions of the host OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Xdg,
    MacOs,
}
impl Layout {
    pub fn native() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Xdg
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDirectories {
    /// Durable profile data: sessions, recovery journals, macros.
    pub data: PathBuf,
    /// Settings and keymaps.
    pub config: PathBuf,
    /// State that survives restarts but is not worth backing up.
    pub state: PathBuf,
    pub cache: PathBuf,
    pub logs: PathBuf,
    /// Sockets and locks of the running session; never on removable media.
    pub runtime: PathBuf,
    pub portable: bool,
}

/// The process environment a resolution reads; tests supply their own.
pub struct Environment<'a> {
    pub var: &'a dyn Fn(&str) -> Option<OsString>,
    /// Folder of the running executable, for the portable marker.
    pub executable_dir: Option<&'a Path>,
    /// Real user id, for the runtime fallback name.
    pub uid: u32,
}

impl AppDirectories {
    /// Resolve the folders of `app` for this process.
    pub fn for_current_process(app: &str) -> io::Result<Self> {
        let executable = std::env::current_exe().ok();
        Self::resolve(
            app,
            Layout::native(),
            &Environment {
                var: &|name| std::env::var_os(name),
                executable_dir: executable.as_deref().and_then(Path::parent),
                uid: rustix::process::getuid().as_raw(),
            },
        )
    }

    pub fn resolve(app: &str, layout: Layout, environment: &Environment<'_>) -> io::Result<Self> {
        if app.is_empty() || app.contains('/') || app == "." || app == ".." {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid application name"));
        }
        let absolute = |name: &str| {
            (environment.var)(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let runtime = match layout {
            Layout::Xdg => absolute("XDG_RUNTIME_DIR").map(|runtime| runtime.join(app)),
            Layout::MacOs => absolute("TMPDIR").map(|runtime| runtime.join(app)),
        }
        .unwrap_or_else(|| {
            absolute("TMPDIR")
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(format!("{app}-{}", environment.uid))
        });
        if let Some(folder) = environment.executable_dir
            && folder.join(PORTABLE_MARKER).is_file()
        {
            let data = folder.join("data");
            return Ok(Self {
                config: data.clone(),
                state: data.clone(),
                cache: data.join("cache"),
                logs: data.join("diagnostics"),
                data,
                runtime,
                portable: true,
            });
        }
        let home = absolute("HOME").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "the home folder is unknown; set HOME to an absolute path",
            )
        })?;
        Ok(match layout {
            Layout::Xdg => {
                let base = |variable: &str, default: &str| absolute(variable).unwrap_or_else(|| home.join(default));
                let state = base("XDG_STATE_HOME", ".local/state").join(app);
                Self {
                    data: base("XDG_DATA_HOME", ".local/share").join(app),
                    config: base("XDG_CONFIG_HOME", ".config").join(app),
                    logs: state.join("logs"),
                    state,
                    cache: base("XDG_CACHE_HOME", ".cache").join(app),
                    runtime,
                    portable: false,
                }
            }
            Layout::MacOs => {
                let library = home.join("Library");
                let profile = library.join("Application Support").join(app);
                Self {
                    data: profile.clone(),
                    config: profile.clone(),
                    state: profile,
                    cache: library.join("Caches").join(app),
                    logs: library.join("Logs").join(app),
                    runtime,
                    portable: false,
                }
            }
        })
    }
}

pub fn data_dir(app: &str) -> io::Result<PathBuf> {
    Ok(AppDirectories::for_current_process(app)?.data)
}
pub fn config_dir(app: &str) -> io::Result<PathBuf> {
    Ok(AppDirectories::for_current_process(app)?.config)
}
pub fn state_dir(app: &str) -> io::Result<PathBuf> {
    Ok(AppDirectories::for_current_process(app)?.state)
}
pub fn cache_dir(app: &str) -> io::Result<PathBuf> {
    Ok(AppDirectories::for_current_process(app)?.cache)
}
pub fn logs_dir(app: &str) -> io::Result<PathBuf> {
    Ok(AppDirectories::for_current_process(app)?.logs)
}
pub fn runtime_dir(app: &str) -> io::Result<PathBuf> {
    Ok(AppDirectories::for_current_process(app)?.runtime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resolve(layout: Layout, vars: &[(&str, &str)], executable_dir: Option<&Path>) -> io::Result<AppDirectories> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(name, value)| (name.to_string(), OsString::from(value)))
            .collect();
        AppDirectories::resolve(
            "bareline",
            layout,
            &Environment {
                var: &|name| vars.get(name).cloned(),
                executable_dir,
                uid: 1000,
            },
        )
    }

    #[test]
    fn xdg_defaults_follow_home() {
        let dirs = resolve(Layout::Xdg, &[("HOME", "/home/ada")], None).unwrap();
        assert_eq!(dirs.data, Path::new("/home/ada/.local/share/bareline"));
        assert_eq!(dirs.config, Path::new("/home/ada/.config/bareline"));
        assert_eq!(dirs.state, Path::new("/home/ada/.local/state/bareline"));
        assert_eq!(dirs.logs, Path::new("/home/ada/.local/state/bareline/logs"));
        assert_eq!(dirs.cache, Path::new("/home/ada/.cache/bareline"));
        assert_eq!(dirs.runtime, Path::new("/tmp/bareline-1000"));
        assert!(!dirs.portable);
    }

    #[test]
    fn xdg_overrides_apply_and_relative_values_are_ignored() {
        let dirs = resolve(
            Layout::Xdg,
            &[
                ("HOME", "/home/ada"),
                ("XDG_DATA_HOME", "/data"),
                ("XDG_CONFIG_HOME", "/config"),
                ("XDG_STATE_HOME", "/state"),
                ("XDG_CACHE_HOME", "relative/cache"),
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ],
            None,
        )
        .unwrap();
        assert_eq!(dirs.data, Path::new("/data/bareline"));
        assert_eq!(dirs.config, Path::new("/config/bareline"));
        assert_eq!(dirs.state, Path::new("/state/bareline"));
        assert_eq!(dirs.logs, Path::new("/state/bareline/logs"));
        assert_eq!(dirs.cache, Path::new("/home/ada/.cache/bareline"));
        assert_eq!(dirs.runtime, Path::new("/run/user/1000/bareline"));
    }

    #[test]
    fn macos_defaults_use_the_library_folders() {
        let dirs = resolve(
            Layout::MacOs,
            &[("HOME", "/Users/ada"), ("TMPDIR", "/var/folders/xy/T/")],
            None,
        )
        .unwrap();
        let support = Path::new("/Users/ada/Library/Application Support/bareline");
        assert_eq!(dirs.data, support);
        assert_eq!(dirs.config, support);
        assert_eq!(dirs.state, support);
        assert_eq!(dirs.cache, Path::new("/Users/ada/Library/Caches/bareline"));
        assert_eq!(dirs.logs, Path::new("/Users/ada/Library/Logs/bareline"));
        assert_eq!(dirs.runtime, Path::new("/var/folders/xy/T/bareline"));
        // XDG variables do not move macOS folders.
        let ignored = resolve(
            Layout::MacOs,
            &[("HOME", "/Users/ada"), ("XDG_DATA_HOME", "/data")],
            None,
        )
        .unwrap();
        assert_eq!(ignored.data, support);
        assert_eq!(ignored.runtime, Path::new("/tmp/bareline-1000"));
    }

    #[test]
    fn missing_home_and_bad_names_are_errors() {
        assert_eq!(
            resolve(Layout::Xdg, &[], None).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(resolve(Layout::Xdg, &[("HOME", "relative")], None).is_err());
        let environment = Environment {
            var: &|_| None,
            executable_dir: None,
            uid: 0,
        };
        for app in ["", "a/b", ".."] {
            assert!(AppDirectories::resolve(app, Layout::Xdg, &environment).is_err());
        }
    }

    #[test]
    fn portable_marker_keeps_everything_next_to_the_executable() {
        let folder = std::env::temp_dir().join(format!("bareline-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let installed = resolve(Layout::Xdg, &[("HOME", "/home/ada")], Some(&folder)).unwrap();
        assert!(!installed.portable);
        // Only presence as a file matters; its content never does.
        std::fs::write(folder.join(PORTABLE_MARKER), b"anything\n").unwrap();
        for layout in [Layout::Xdg, Layout::MacOs] {
            let dirs = resolve(layout, &[], Some(&folder)).unwrap();
            assert!(dirs.portable);
            assert_eq!(dirs.data, folder.join("data"));
            assert_eq!(dirs.config, folder.join("data"));
            assert_eq!(dirs.state, folder.join("data"));
            assert_eq!(dirs.cache, folder.join("data/cache"));
            assert_eq!(dirs.logs, folder.join("data/diagnostics"));
        }
        std::fs::remove_file(folder.join(PORTABLE_MARKER)).unwrap();
        std::fs::create_dir(folder.join(PORTABLE_MARKER)).unwrap();
        assert!(
            !resolve(Layout::Xdg, &[("HOME", "/home/ada")], Some(&folder))
                .unwrap()
                .portable
        );
        std::fs::remove_dir_all(folder).unwrap();
    }
}
