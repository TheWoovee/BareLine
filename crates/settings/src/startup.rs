// SPDX-License-Identifier: MPL-2.0
//! Startup recovery for the user settings file (APP-01). A damaged file never
//! stops the editor from opening: its content is set aside, defaults apply in
//! memory, and the caller shows the returned notice until the user dismisses it.
use crate::{ParseError, Scope, SettingsDocument, atomic_write_config, decode_config_text, is_utf16_config};
use bareline_platform::LocalFileSystem;
use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupNotice {
    /// Unusable content was renamed to `backup`; defaults are in use.
    Quarantined { reason: String, backup: PathBuf },
    /// The file was left untouched and defaults are in use. It must not be
    /// replaced by an automatic save before the user acts on it.
    Retained { reason: String },
    /// A UTF-16 file was rewritten as UTF-8; the original bytes are in `backup`.
    Converted { backup: PathBuf },
}

pub struct StartupSettings {
    pub document: SettingsDocument,
    pub notice: Option<StartupNotice>,
    /// False when the file was left in place and saving over it would destroy it.
    pub writable: bool,
}

/// Turns the bounded startup read of `path` (`Ok(None)` when it is absent) into
/// the document to run with. `repair` allows renaming or rewriting `path`; a
/// legacy file that profile migration still reads passes false and is only reported.
pub fn recover_startup_settings(
    path: &Path,
    read: io::Result<Option<Vec<u8>>>,
    stamp: u64,
    repair: bool,
    platform: &dyn LocalFileSystem,
) -> Option<StartupSettings> {
    let bytes = match read {
        Ok(None) => return None,
        Ok(Some(bytes)) => bytes,
        // The bounded reader reports oversized content as invalid data.
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            return Some(set_aside(path, ParseError::TooLarge.to_string(), stamp, repair));
        }
        Err(error) => {
            return Some(defaults(StartupNotice::Retained {
                reason: error.to_string(),
            }));
        }
    };
    match SettingsDocument::parse_classified(&bytes, Scope::User) {
        Ok(document) if repair && is_utf16_config(&bytes) => Some(match convert_utf16(path, &bytes, stamp, platform) {
            Ok(backup) => StartupSettings {
                document,
                notice: Some(StartupNotice::Converted { backup }),
                writable: true,
            },
            // The UTF-16 original stays authoritative until it is safely copied.
            Err(error) => StartupSettings {
                document,
                notice: Some(StartupNotice::Retained {
                    reason: format!("UTF-16 settings could not be converted to UTF-8: {error}"),
                }),
                writable: false,
            },
        }),
        Ok(document) => Some(StartupSettings {
            document,
            notice: None,
            writable: true,
        }),
        // A newer Bareline owns this file; renaming it would lose its settings there.
        Err(ParseError::UnsupportedVersion) => Some(defaults(StartupNotice::Retained {
            reason: ParseError::UnsupportedVersion.to_string(),
        })),
        Err(error) => Some(set_aside(path, error.to_string(), stamp, repair)),
    }
}

fn defaults(notice: StartupNotice) -> StartupSettings {
    StartupSettings {
        document: SettingsDocument::empty(Scope::User),
        writable: matches!(notice, StartupNotice::Quarantined { .. }),
        notice: Some(notice),
    }
}

fn set_aside(path: &Path, reason: String, stamp: u64, repair: bool) -> StartupSettings {
    if !repair {
        return defaults(StartupNotice::Retained { reason });
    }
    let renamed = unused_sibling(path, &format!("invalid-{stamp}"))
        .and_then(|backup| std::fs::rename(path, &backup).map(|()| backup));
    defaults(match renamed {
        Ok(backup) => StartupNotice::Quarantined { reason, backup },
        Err(error) => StartupNotice::Retained {
            reason: format!("{reason}; the file could not be renamed: {error}"),
        },
    })
}

fn convert_utf16(path: &Path, bytes: &[u8], stamp: u64, platform: &dyn LocalFileSystem) -> io::Result<PathBuf> {
    let text = decode_config_text(bytes).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    let backup = unused_sibling(path, &format!("utf16-{stamp}"))?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(&backup)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    atomic_write_config(path, text.as_bytes(), platform)?;
    Ok(backup)
}

/// `<name>.<suffix>`, or `<name>.<suffix>-N`, whichever does not exist yet.
fn unused_sibling(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    for attempt in 0..100u32 {
        let mut candidate = name.to_os_string();
        candidate.push(format!(".{suffix}"));
        if attempt != 0 {
            candidate.push(format!("-{attempt}"));
        }
        let candidate = path.with_file_name(candidate);
        match std::fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}
