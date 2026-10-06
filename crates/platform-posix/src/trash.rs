// SPDX-License-Identifier: MPL-2.0
//! Moving files and folders to the user's trash, the counterpart of the
//! Windows Recycle Bin for the shell's Delete.
//!
//! Linux follows the freedesktop.org Trash specification for the home trash,
//! `$XDG_DATA_HOME/Trash` (by default `~/.local/share/Trash`): a record
//! `info/<name>.trashinfo`, created exclusively, reserves the name and keeps
//! the original location and deletion time; the entry then moves to
//! `files/<name>`, so file managers can list and restore it. macOS moves the
//! entry into `~/.Trash` under a free name.
//!
//! The entry itself moves, never a link's target. A rename keeps it as it was;
//! from another file system it is copied (links stay links) and the original
//! is removed only once the copy is complete. Per-volume trash folders
//! (`$topdir/.Trash-$uid`, `/Volumes/<name>/.Trashes`) are not used yet.
use crate::{paths::Layout, sys};
use rustix::io::Errno;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

/// Names tried for one entry before the trash counts as full of that name.
const MAX_NAMES: u32 = 10_000;

/// The trash of one user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trash {
    root: PathBuf,
    layout: Layout,
}

impl Trash {
    /// The home trash of the user running this process.
    pub fn for_current_user() -> io::Result<Self> {
        Self::resolve(Layout::native(), &|name| std::env::var_os(name))
    }

    /// The home trash for `layout`, from the environment `var` reads.
    /// Relative values are ignored, as the XDG specification requires.
    pub fn resolve(layout: Layout, var: &dyn Fn(&str) -> Option<OsString>) -> io::Result<Self> {
        let absolute = |name: &str| var(name).map(PathBuf::from).filter(|path| path.is_absolute());
        let home = || {
            absolute("HOME").ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "the home folder is unknown; set HOME to an absolute path",
                )
            })
        };
        let root = match layout {
            Layout::Xdg => match absolute("XDG_DATA_HOME") {
                Some(data) => data,
                None => home()?.join(".local/share"),
            }
            .join("Trash"),
            Layout::MacOs => home()?.join(".Trash"),
        };
        Ok(Self { root, layout })
    }

    /// A trash at `root` with the conventions of `layout`.
    pub fn at(root: PathBuf, layout: Layout) -> Self {
        Self { root, layout }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where trashed entries are kept.
    fn files(&self) -> PathBuf {
        match self.layout {
            Layout::Xdg => self.root.join("files"),
            Layout::MacOs => self.root.clone(),
        }
    }

    /// Move the entry at `path` (a file, a folder with its contents, or a
    /// link) into the trash and return its new location.
    pub fn put(&self, path: &Path) -> io::Result<PathBuf> {
        let invalid = |message: &'static str| io::Error::new(io::ErrorKind::InvalidInput, message);
        if !path.is_absolute() {
            return Err(invalid("only absolute paths can be moved to the trash"));
        }
        let name = path
            .file_name()
            .ok_or_else(|| invalid("this location cannot be moved to the trash"))?;
        // The entry itself, never a link's target, and it must exist.
        let entry = fs::symlink_metadata(path)?;
        if path.starts_with(&self.root) {
            return Err(invalid("this item is already in the trash"));
        }
        if self.root.starts_with(path) {
            return Err(invalid("the folder that holds the trash cannot be moved to it"));
        }
        let files = self.files();
        private_folder(&files)?;
        if self.layout == Layout::Xdg {
            private_folder(&self.root.join("info"))?;
        }
        let (destination, record) = self.reserve(name, path, &files)?;
        match relocate(path, &destination, entry.is_dir(), true) {
            Ok(()) => Ok(destination),
            Err(Relocation::Untouched(error)) => {
                if let Some(record) = record {
                    let _ = fs::remove_file(record);
                }
                Err(error)
            }
            // A complete copy is in the trash; its record stays with it.
            Err(Relocation::OriginalKept(error)) => Err(io::Error::new(
                error.kind(),
                format!("The item was copied to the trash, but the original could not be removed: {error}"),
            )),
        }
    }

    /// A free name in the trash and, on Linux, the record that reserves it.
    fn reserve(&self, name: &OsStr, original: &Path, files: &Path) -> io::Result<(PathBuf, Option<PathBuf>)> {
        for attempt in 1..=MAX_NAMES {
            let candidate = numbered(name, attempt);
            let destination = files.join(&candidate);
            if self.layout == Layout::MacOs {
                if !exists(&destination)? {
                    return Ok((destination, None));
                }
                continue;
            }
            let mut record_name = candidate;
            record_name.push(".trashinfo");
            let record = self.root.join("info").join(record_name);
            let mut file = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&record)
            {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            };
            // A stray entry without a record keeps its name too.
            if exists(&destination)? {
                drop(file);
                let _ = fs::remove_file(&record);
                continue;
            }
            let written = file
                .write_all(&trash_info(original, deletion_date()))
                .and_then(|()| file.sync_all());
            if let Err(error) = written {
                drop(file);
                let _ = fs::remove_file(&record);
                return Err(error);
            }
            return Ok((destination, Some(record)));
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the trash already holds too many items with this name",
        ))
    }
}

/// Move `path` to the current user's trash; see [`Trash::put`].
pub fn trash(path: &Path) -> io::Result<PathBuf> {
    Trash::for_current_user()?.put(path)
}

/// Why an entry did not move.
enum Relocation {
    /// The original is where it was and nothing is left in the trash.
    Untouched(io::Error),
    /// A complete copy is in the trash but the original was not (fully) removed.
    OriginalKept(io::Error),
}

/// Rename `source` onto the free name `destination`, or copy it there when it
/// is on another file system (or when `rename` is false, which tests use) and
/// then remove the original.
fn relocate(source: &Path, destination: &Path, directory: bool, rename: bool) -> Result<(), Relocation> {
    if rename {
        match rename_entry(source, destination) {
            Ok(()) => return Ok(()),
            Err(error) if sys::is_errno(&error, Errno::XDEV) => {}
            Err(error) => return Err(Relocation::Untouched(error)),
        }
    }
    if let Err(error) = copy_tree(source, destination) {
        let _ = remove_tree(destination);
        return Err(Relocation::Untouched(error));
    }
    let removed = if directory {
        fs::remove_dir_all(source)
    } else {
        fs::remove_file(source)
    };
    removed.map_err(Relocation::OriginalKept)
}

fn rename_entry(source: &Path, destination: &Path) -> io::Result<()> {
    let (from_parent, from_name) = sys::split(source)?;
    let (to_parent, to_name) = sys::split(destination)?;
    let (from, to) = (File::open(from_parent)?, File::open(to_parent)?);
    sys::rename_no_replace(&from, from_name, &to, to_name)?;
    // The rename has happened; failing to make it durable is not worth undoing it.
    let _ = sys::sync_directory(&from);
    let _ = sys::sync_directory(&to);
    Ok(())
}

/// Copy an entry without following links: folders recursively, links as links
/// and regular files with their permissions. Other kinds are refused.
fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(source)?, destination)
    } else if kind.is_file() {
        let mut from = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(source)?;
        let mut to = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)?;
        io::copy(&mut from, &mut to)?;
        to.set_permissions(metadata.permissions())?;
        to.sync_all()
    } else if kind.is_dir() {
        fs::DirBuilder::new().mode(0o700).create(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, metadata.permissions())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "special files cannot be moved to the trash from another drive",
        ))
    }
}

fn remove_tree(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// A folder only the user can read, created with its parents if missing.
fn private_folder(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
}

/// `name` for the first attempt, then `name.2.ext`, `name.3.ext` and so on,
/// keeping the extension last so the copy still opens with the same program.
fn numbered(name: &OsStr, attempt: u32) -> OsString {
    if attempt == 1 {
        return name.to_owned();
    }
    let path = Path::new(name);
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(extension)) if !stem.is_empty() => {
            let mut numbered = stem.to_owned();
            numbered.push(format!(".{attempt}."));
            numbered.push(extension);
            numbered
        }
        _ => {
            let mut numbered = name.to_owned();
            numbered.push(format!(".{attempt}"));
            numbered
        }
    }
}

/// The `.trashinfo` record: the original path, percent-encoded as a URI path,
/// and the deletion time.
fn trash_info(original: &Path, deleted: String) -> Vec<u8> {
    let mut path = Vec::new();
    for &byte in original.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            path.push(byte);
        } else {
            path.extend_from_slice(format!("%{byte:02X}").as_bytes());
        }
    }
    let mut record = b"[Trash Info]\nPath=".to_vec();
    record.extend_from_slice(&path);
    record.extend_from_slice(format!("\nDeletionDate={deleted}\n").as_bytes());
    record
}

/// Now as `YYYY-MM-DDThh:mm:ss` in local time, as the specification asks.
fn deletion_date() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    local_date(seconds).unwrap_or_else(|| utc_date(seconds))
}

fn local_date(seconds: i64) -> Option<String> {
    let time = libc::time_t::try_from(seconds).ok()?;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: both pointers are valid for the call; localtime_r is the
    // reentrant form and writes only into `tm`.
    let filled = unsafe { libc::localtime_r(&time, tm.as_mut_ptr()) };
    if filled.is_null() {
        return None;
    }
    // SAFETY: localtime_r succeeded, so it filled `tm`; all-zero bytes (its
    // integers and null zone pointer) were a valid `tm` before that, too.
    let tm = unsafe { tm.assume_init() };
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        i64::from(tm.tm_year) + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    ))
}

/// Coordinated universal time, when the local time zone cannot be applied
/// (days to civil dates as in Howard Hinnant's `civil_from_days`).
fn utc_date(seconds: i64) -> String {
    let (days, second) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        second / 3600,
        second / 60 % 60,
        second % 60
    )
}

/// The path of a `.trashinfo` record's `Path=` value, decoded.
#[cfg(test)]
fn recorded_path(record: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let text = std::str::from_utf8(record).ok()?;
    let encoded = text.lines().find_map(|line| line.strip_prefix("Path="))?.as_bytes();
    let mut bytes = Vec::new();
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'%' {
            let hex = std::str::from_utf8(encoded.get(index + 1..index + 3)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            bytes.push(encoded[index]);
            index += 1;
        }
    }
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("bareline-trash-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("work")).unwrap();
            Self(root)
        }
        fn work(&self) -> PathBuf {
            self.0.join("work")
        }
        fn trash(&self, layout: Layout) -> Trash {
            Trash::at(self.0.join("Trash"), layout)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn files_move_into_the_home_trash_with_a_restorable_record() {
        let scratch = Scratch::new("record");
        let trash = scratch.trash(Layout::Xdg);
        let original = scratch.work().join("notes.txt");
        fs::write(&original, "keep me").unwrap();
        let trashed = trash.put(&original).unwrap();
        assert_eq!(trashed, trash.root().join("files/notes.txt"));
        assert!(!original.exists());
        assert_eq!(fs::read_to_string(&trashed).unwrap(), "keep me");
        let record = fs::read(trash.root().join("info/notes.txt.trashinfo")).unwrap();
        assert!(record.starts_with(b"[Trash Info]\nPath=/"));
        assert_eq!(recorded_path(&record).unwrap(), original);
        let text = String::from_utf8(record).unwrap();
        let date = text
            .lines()
            .find_map(|line| line.strip_prefix("DeletionDate="))
            .unwrap();
        assert_eq!(date.len(), "2026-10-06T12:00:00".len());
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[10..11], "T");
    }

    #[test]
    fn a_name_already_in_the_trash_gets_the_next_number_before_its_extension() {
        let scratch = Scratch::new("numbered");
        let trash = scratch.trash(Layout::Xdg);
        let original = scratch.work().join("notes.txt");
        for (expected, contents) in [("notes.txt", "one"), ("notes.2.txt", "two"), ("notes.3.txt", "three")] {
            fs::write(&original, contents).unwrap();
            let trashed = trash.put(&original).unwrap();
            assert_eq!(trashed, trash.root().join("files").join(expected));
            assert_eq!(fs::read_to_string(trashed).unwrap(), contents);
            assert!(trash.root().join(format!("info/{expected}.trashinfo")).is_file());
        }
        assert_eq!(numbered(OsStr::new(".bashrc"), 2), ".bashrc.2");
        assert_eq!(numbered(OsStr::new("Makefile"), 4), "Makefile.4");
        // A stray entry without a record also keeps its name.
        fs::write(trash.root().join("files/stray"), "").unwrap();
        fs::write(scratch.work().join("stray"), "new").unwrap();
        let trashed = trash.put(&scratch.work().join("stray")).unwrap();
        assert_eq!(trashed, trash.root().join("files/stray.2"));
        assert!(!trash.root().join("info/stray.trashinfo").exists());
    }

    #[test]
    fn links_move_as_links_and_their_targets_stay() {
        let scratch = Scratch::new("link");
        let trash = scratch.trash(Layout::Xdg);
        let target = scratch.work().join("target.txt");
        fs::write(&target, "target").unwrap();
        let link = scratch.work().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let trashed = trash.put(&link).unwrap();
        assert!(fs::symlink_metadata(&trashed).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "target");
        assert!(fs::symlink_metadata(&link).is_err());
    }

    #[test]
    fn a_copy_from_another_drive_keeps_contents_links_and_modes_and_removes_the_original() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("copy");
        let outside = scratch.work().join("outside.txt");
        fs::write(&outside, "outside").unwrap();
        let folder = scratch.work().join("folder");
        fs::create_dir_all(folder.join("nested")).unwrap();
        fs::write(folder.join("nested/data.bin"), [0, 1, 2]).unwrap();
        fs::set_permissions(folder.join("nested/data.bin"), fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("link")).unwrap();
        let destination = scratch.0.join("copied");
        // Forced copy: the path a rename across file systems takes.
        assert!(relocate(&folder, &destination, true, false).is_ok());
        assert!(!folder.exists());
        assert_eq!(fs::read(destination.join("nested/data.bin")).unwrap(), [0, 1, 2]);
        let mode = fs::metadata(destination.join("nested/data.bin"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
        let link = destination.join("link");
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), outside);
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
    }

    #[test]
    fn a_failed_copy_leaves_the_original_and_no_partial_entry() {
        let scratch = Scratch::new("failed-copy");
        let folder = scratch.work().join("folder");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.txt"), "a").unwrap();
        // A FIFO cannot be copied, so the copy fails part way.
        let made = std::process::Command::new("mkfifo")
            .arg(folder.join("pipe"))
            .status()
            .unwrap();
        assert!(made.success());
        let destination = scratch.0.join("copied");
        match relocate(&folder, &destination, true, false) {
            Err(Relocation::Untouched(error)) => assert_eq!(error.kind(), io::ErrorKind::Unsupported),
            _ => panic!("the copy must fail before the original is touched"),
        }
        assert!(folder.join("a.txt").is_file());
        assert!(!destination.exists());
    }

    #[test]
    fn macos_trash_keeps_entries_directly_under_its_folder() {
        let scratch = Scratch::new("macos");
        let trash = scratch.trash(Layout::MacOs);
        let folder = scratch.work().join("Project");
        fs::create_dir_all(folder.join("src")).unwrap();
        fs::write(folder.join("src/main.rs"), "fn main() {}").unwrap();
        assert_eq!(trash.put(&folder).unwrap(), trash.root().join("Project"));
        fs::create_dir_all(&folder).unwrap();
        assert_eq!(trash.put(&folder).unwrap(), trash.root().join("Project.2"));
        assert!(trash.root().join("Project/src/main.rs").is_file());
        assert!(!trash.root().join("info").exists());
    }

    #[test]
    fn invalid_or_missing_entries_and_the_trash_itself_are_refused() {
        let scratch = Scratch::new("refused");
        let trash = scratch.trash(Layout::Xdg);
        let kind = |path: &Path| trash.put(path).unwrap_err().kind();
        assert_eq!(kind(Path::new("relative.txt")), io::ErrorKind::InvalidInput);
        assert_eq!(kind(Path::new("/")), io::ErrorKind::InvalidInput);
        assert_eq!(kind(&scratch.work().join("missing.txt")), io::ErrorKind::NotFound);
        fs::create_dir_all(trash.root().join("files")).unwrap();
        fs::write(trash.root().join("files/old.txt"), "").unwrap();
        assert_eq!(kind(&trash.root().join("files/old.txt")), io::ErrorKind::InvalidInput);
        assert_eq!(kind(&scratch.0), io::ErrorKind::InvalidInput);
        assert!(scratch.0.exists());
    }

    #[test]
    fn records_percent_encode_the_original_path() {
        let record = trash_info(Path::new("/tmp/a b/\u{fc}%.txt"), "2026-10-06T12:00:00".into());
        assert_eq!(
            record,
            b"[Trash Info]\nPath=/tmp/a%20b/%C3%BC%25.txt\nDeletionDate=2026-10-06T12:00:00\n"
        );
        assert_eq!(recorded_path(&record).unwrap(), Path::new("/tmp/a b/\u{fc}%.txt"));
        assert_eq!(utc_date(0), "1970-01-01T00:00:00");
        assert_eq!(utc_date(1_700_000_000), "2023-11-14T22:13:20");
        assert_eq!(utc_date(951_782_400), "2000-02-29T00:00:00");
        assert!(local_date(1_700_000_000).is_some());
    }

    #[test]
    fn the_home_trash_follows_xdg_data_home_and_macos_uses_dot_trash() {
        let resolve = |layout: Layout, vars: &[(&str, &str)]| {
            let vars: HashMap<String, OsString> = vars
                .iter()
                .map(|(name, value)| (name.to_string(), OsString::from(value)))
                .collect();
            Trash::resolve(layout, &|name| vars.get(name).cloned())
        };
        let home = [("HOME", "/home/ada")];
        assert_eq!(
            resolve(Layout::Xdg, &home).unwrap().root(),
            Path::new("/home/ada/.local/share/Trash")
        );
        assert_eq!(
            resolve(Layout::Xdg, &[("HOME", "/home/ada"), ("XDG_DATA_HOME", "/data")])
                .unwrap()
                .root(),
            Path::new("/data/Trash")
        );
        assert_eq!(
            resolve(Layout::Xdg, &[("HOME", "/home/ada"), ("XDG_DATA_HOME", "data")])
                .unwrap()
                .root(),
            Path::new("/home/ada/.local/share/Trash")
        );
        assert_eq!(
            resolve(Layout::MacOs, &[("HOME", "/Users/ada"), ("XDG_DATA_HOME", "/data")])
                .unwrap()
                .root(),
            Path::new("/Users/ada/.Trash")
        );
        assert_eq!(resolve(Layout::Xdg, &[]).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
