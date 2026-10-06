// SPDX-License-Identifier: MPL-2.0
//! Small descriptor-relative helpers shared by the services. Every name below a
//! retained directory is opened with `O_NOFOLLOW`, so a link swapped in between
//! two calls fails instead of being followed.
use rustix::{
    fs::{AtFlags, FileType, Mode, OFlags, RenameFlags},
    io::Errno,
};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read},
    os::{fd::AsFd, unix::fs::MetadataExt},
    path::Path,
};

/// Directories that are only walked through. `O_PATH` on Linux needs no read
/// permission, so search-only folders on the way stay traversable.
#[cfg(target_os = "linux")]
pub(crate) const WALK: OFlags = OFlags::PATH
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
#[cfg(not(target_os = "linux"))]
pub(crate) const WALK: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
/// A directory whose entries are created, renamed, synced or locked.
pub(crate) const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
/// Read an existing entry. `O_NONBLOCK` keeps a FIFO swapped onto the name from
/// blocking the open; it has no effect on regular files or directories.
pub(crate) const READ: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);
/// Create a private file that must not exist yet.
pub(crate) const CREATE: OFlags = OFlags::WRONLY
    .union(OFlags::CREATE)
    .union(OFlags::EXCL)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// Device and inode: the identity of a directory entry's object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    pub dev: u64,
    pub ino: u64,
}
impl Node {
    pub fn of(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
}

/// `lstat` of a name below a retained directory.
pub(crate) struct NameStat {
    pub node: Node,
    pub kind: FileType,
}

// Field widths differ between Linux (u64) and macOS (i32 device numbers); the
// casts match `MetadataExt::dev`, so both sides of an identity compare agree.
#[allow(clippy::unnecessary_cast)]
pub(crate) fn stat_name(directory: impl AsFd, name: &OsStr) -> io::Result<NameStat> {
    let stat = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
    Ok(NameStat {
        node: Node {
            dev: stat.st_dev as u64,
            ino: stat.st_ino as u64,
        },
        kind: FileType::from_raw_mode(stat.st_mode),
    })
}

pub(crate) fn open_at(directory: impl AsFd, name: impl AsRef<OsStr>, flags: OFlags) -> io::Result<File> {
    Ok(File::from(rustix::fs::openat(
        directory,
        name.as_ref(),
        flags,
        Mode::from_bits_truncate(0o600),
    )?))
}

pub(crate) fn split(path: &Path) -> io::Result<(&Path, &OsStr)> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Ok((parent, name)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "path has no parent folder")),
    }
}

pub(crate) fn denied(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
pub(crate) fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub(crate) fn changed() -> io::Error {
    denied("a folder or file changed while it was in use")
}

pub(crate) fn is_errno(error: &io::Error, errno: Errno) -> bool {
    error.raw_os_error() == Some(errno.raw_os_error())
}

/// The filesystem does not implement a rename flag (exchange or no-replace).
pub(crate) fn flag_unsupported(error: &io::Error) -> bool {
    [Errno::INVAL, Errno::NOSYS, Errno::NOTSUP, Errno::OPNOTSUPP]
        .into_iter()
        .any(|errno| is_errno(error, errno))
}

/// A link found where none may be followed reads as a refusal, not a loop.
pub(crate) fn no_follow(error: io::Error) -> io::Error {
    if is_errno(&error, Errno::LOOP) {
        denied("symbolic links are not followed here")
    } else {
        error
    }
}

/// Rename that never replaces an existing destination, including in a race,
/// where the filesystem supports it; otherwise checked immediately before.
pub(crate) fn rename_no_replace(
    from_directory: impl AsFd,
    from: &OsStr,
    to_directory: impl AsFd,
    to: &OsStr,
) -> io::Result<()> {
    match rustix::fs::renameat_with(&from_directory, from, &to_directory, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(errno) => {
            let error = io::Error::from(errno);
            if !flag_unsupported(&error) {
                return Err(error);
            }
            match stat_name(&to_directory, to) {
                Ok(_) => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    Ok(rustix::fs::renameat(from_directory, from, to_directory, to)?)
                }
                Err(error) => Err(error),
            }
        }
    }
}

/// Make a directory's entry changes durable. Filesystems that cannot sync a
/// directory report it as unsupported; their entries are as durable as they get.
pub(crate) fn sync_directory(directory: &File) -> io::Result<()> {
    match rustix::fs::fsync(directory) {
        Ok(()) => Ok(()),
        Err(errno) if errno == Errno::INVAL || errno == Errno::NOTSUP || errno == Errno::OPNOTSUPP => Ok(()),
        Err(errno) => Err(errno.into()),
    }
}

pub(crate) fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn read_bounded(file: &mut File, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid_data("record is larger than allowed"));
    }
    Ok(bytes)
}
