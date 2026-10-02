// SPDX-License-Identifier: MPL-2.0
//! Filesystem capability classification (FIO-18). The probe resolves junction,
//! mount-point and symbolic-link names one hop at a time and classifies every
//! destination before opening anything below it, so a link never leads it to a
//! network host. The save policy is a pure function of the gathered facts.
use bareline_platform::{CapabilityReport, SaveStrategy, StorageKind, Support};
use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io,
    mem::size_of,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Component, Path, PathBuf, Prefix},
};
use windows::{
    Win32::{Foundation::HANDLE, Storage::FileSystem::*},
    core::PCWSTR,
};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;
const DRIVE_RAMDISK: u32 = 6;
const FILE_PERSISTENT_ACLS: u32 = 0x0000_0008;
const FILE_NAMED_STREAMS: u32 = 0x0004_0000;
const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;
const FILE_SUPPORTS_HARD_LINKS: u32 = 0x0040_0000;
/// IsReparseTagNameSurrogate: the reparse point names another location.
const NAME_SURROGATE: u32 = 0x2000_0000;
/// IO_REPARSE_TAG_CLOUD..IO_REPARSE_TAG_CLOUD_F differ only in bits 12..15.
const CLOUD_TAG: u32 = 0x9000_001A;
const CLOUD_TAG_MASK: u32 = 0xFFFF_0FFF;
const MAX_REDIRECTIONS: usize = 31;
const NETWORK: &str = "This location is on the network. Open it with Open Remote File with Permission, or use Save Copy to keep edits in a local folder.";

fn io_error(error: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0 & 0xffff)
}
fn network() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, NETWORK)
}
fn unsupported(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason)
}

/// Facts the probe gathers for one location; tests inject them directly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct VolumeFacts {
    /// GetDriveTypeW of the resolved volume root.
    pub drive_type: u32,
    /// Filesystem name, for example "NTFS", "exFAT" or "ReFS".
    pub filesystem: String,
    /// GetVolumeInformation filesystem flags.
    pub flags: u32,
    /// Link count of the existing target file; zero when it is absent.
    pub links: u32,
    /// Reached through a junction, mount point or symbolic link.
    pub redirected: bool,
    /// The final name itself is a symbolic link to a local file.
    pub final_link: bool,
    pub cloud: bool,
}

/// Save policy: NTFS keeps the transactional replace. Other local filesystems
/// replace through a same-directory stage. Linked targets are rewritten in place
/// so the link survives. Network and read-only volumes offer Save Copy only.
pub(crate) fn classify(facts: &VolumeFacts) -> CapabilityReport {
    let storage = match facts.drive_type {
        DRIVE_FIXED | DRIVE_RAMDISK => StorageKind::Local,
        DRIVE_REMOVABLE | DRIVE_CDROM => StorageKind::Removable,
        DRIVE_REMOTE => StorageKind::Network,
        _ => StorageKind::Unknown,
    };
    let known = matches!(storage, StorageKind::Local | StorageKind::Removable);
    let flag = |bit: u32| match (known, facts.flags & bit != 0) {
        (false, _) => Support::Unknown,
        (true, true) => Support::Supported,
        (true, false) => Support::Unsupported,
    };
    let ntfs = facts.filesystem.eq_ignore_ascii_case("NTFS");
    let save = if !known || facts.flags & FILE_READ_ONLY_VOLUME != 0 {
        // Remote writes would need a write-scoped consent; none exists yet.
        SaveStrategy::CopyOnly
    } else if facts.links > 1 || facts.final_link {
        SaveStrategy::InPlace
    } else if ntfs {
        SaveStrategy::Transactional
    } else {
        SaveStrategy::RenameReplace
    };
    CapabilityReport {
        atomic_replace: match (known, ntfs) {
            (false, _) => Support::Unknown,
            (true, true) => Support::Supported,
            (true, false) => Support::Unsupported,
        },
        acl: flag(FILE_PERSISTENT_ACLS),
        ads: flag(FILE_NAMED_STREAMS),
        hard_links: flag(FILE_SUPPORTS_HARD_LINKS),
        storage,
        save,
        redirected: facts.redirected,
        cloud: facts.cloud,
    }
}

pub(crate) fn is_cloud_tag(tag: u32) -> bool {
    tag & CLOUD_TAG_MASK == CLOUD_TAG
}

/// Cloud-sync placeholders keep the object at its own name; once their data is
/// local they behave as ordinary files and directories. Every other reparse
/// point, and any object whose data would first be recalled, is not ordinary.
pub(crate) fn ordinary_object(attributes: u32, tag: u32) -> bool {
    attributes & (FILE_ATTRIBUTE_OFFLINE.0 | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0) == 0
        && (attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 || is_cloud_tag(tag))
}

/// Attributes and reparse tag (zero without a reparse point) of an open handle.
pub(crate) fn handle_attribute_tag(file: &File) -> io::Result<(u32, u32)> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the file owns a valid handle and the output structure is live.
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information).map_err(io_error)?;
    }
    let attributes = information.dwFileAttributes;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 {
        // FAT-family filesystems need not answer the tag query; they have no tags.
        return Ok((attributes, 0));
    }
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: the file owns a valid handle; the output structure matches the class.
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
        .map_err(io_error)?;
    }
    Ok((attributes, info.ReparseTag))
}

/// Open the name itself: never follows a link and never recalls cloud data.
fn open_name(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}

fn is_volume_guid(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.len() == 44 && name.starts_with("Volume{") && name.ends_with('}'))
}

/// Classify the root by drive letter or volume name only; no remote query.
fn drive_type(path: &Path) -> io::Result<u32> {
    let root: Vec<u16> = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => vec![drive as u16, b':' as u16, b'\\' as u16, 0],
            // Mount points name their volume as \\?\Volume{GUID}\.
            Prefix::Verbatim(name) if is_volume_guid(name) => r"\\?\"
                .encode_utf16()
                .chain(name.encode_wide())
                .chain([b'\\' as u16, 0])
                .collect(),
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) => return Err(network()),
            _ => return Err(unsupported("device paths are not supported")),
        },
        _ => return Err(unsupported("only absolute local paths are supported")),
    };
    // SAFETY: root is a terminated drive or volume root, not a remote destination.
    Ok(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) })
}

/// True for UNC names and mapped network drives, decided without contacting them.
pub(crate) fn is_network_name(path: &Path) -> bool {
    match path.components().next() {
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..)) => true,
        Some(Component::Prefix(_)) => drive_type(path).is_ok_and(|kind| kind == DRIVE_REMOTE),
        _ => false,
    }
}

pub(crate) struct Resolved {
    pub path: PathBuf,
    pub drive_type: u32,
    pub redirected: bool,
    pub final_link: bool,
    pub cloud: bool,
}

/// Replace every junction, mount point and symbolic link on `path` by its target.
/// Each destination is classified before anything below it is opened; network
/// destinations are refused because they keep needing the remote-read consent.
pub(crate) fn resolve(path: &Path) -> io::Result<Resolved> {
    let mut current = path.to_path_buf();
    let mut redirected = false;
    let mut final_link = false;
    let mut cloud = false;
    for _ in 0..=MAX_REDIRECTIONS {
        let kind = drive_type(&current)?;
        match kind {
            DRIVE_REMOVABLE | DRIVE_FIXED | DRIVE_CDROM | DRIVE_RAMDISK => {}
            DRIVE_REMOTE => return Err(network()),
            _ => return Err(unsupported("unsupported drive")),
        }
        match resolve_step(&current, &mut cloud)? {
            None => {
                return Ok(Resolved {
                    path: current,
                    drive_type: kind,
                    redirected,
                    final_link,
                    cloud,
                });
            }
            Some((next, last)) => {
                redirected = true;
                final_link |= last;
                current = next;
            }
        }
    }
    Err(unsupported("too many junctions or symbolic links"))
}

/// Walk root first and replace the first link found; `None` when there is none.
fn resolve_step(path: &Path, cloud: &mut bool) -> io::Result<Option<(PathBuf, bool)>> {
    let components: Vec<Component<'_>> = path.components().collect();
    let mut walked = PathBuf::new();
    for (index, component) in components.iter().enumerate() {
        walked.push(component);
        if !matches!(component, Component::Normal(_)) {
            continue;
        }
        let last = index + 1 == components.len();
        let (attributes, tag) = match open_name(&walked).and_then(|file| handle_attribute_tag(&file)) {
            Ok(info) => info,
            // A new file below an existing directory.
            Err(error) if last && error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 {
            continue;
        }
        if tag & NAME_SURROGATE == 0 {
            *cloud |= is_cloud_tag(tag);
            continue;
        }
        // Reads the reparse data of the name itself; nothing is followed.
        let target = std::fs::read_link(&walked)?;
        let mut next = if target.is_relative() {
            lexical_join(walked.parent().unwrap_or(&walked), &target)?
        } else {
            target
        };
        for rest in &components[index + 1..] {
            next.push(rest);
        }
        return Ok(Some((next, last)));
    }
    Ok(None)
}

/// Relative link targets are relative to the link's directory. Verbatim paths do
/// not interpret "..", so normalize lexically.
fn lexical_join(base: &Path, relative: &Path) -> io::Result<PathBuf> {
    let mut joined = base.to_path_buf();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !joined.pop() {
                    return Err(unsupported("symbolic link leaves its volume root"));
                }
            }
            Component::Normal(name) => joined.push(name),
            Component::RootDir | Component::Prefix(_) => {
                return Err(unsupported("unsupported symbolic link target"));
            }
        }
    }
    Ok(joined)
}

/// Volume facts for a resolved local path; the nearest existing object answers.
fn facts(resolved: &Resolved) -> io::Result<VolumeFacts> {
    let (file, links) = match open_name(&resolved.path) {
        Ok(file) => {
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            // SAFETY: the file owns a valid handle and the output structure is live.
            unsafe {
                GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info).map_err(io_error)?;
            }
            let links = if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
                info.nNumberOfLinks
            } else {
                0
            };
            (file, links)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = resolved
                .path
                .parent()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
            (open_name(parent)?, 0)
        }
        Err(error) => return Err(error),
    };
    let mut flags = 0u32;
    let mut name = [0u16; 64];
    // SAFETY: the file owns a valid handle; both output buffers outlive the call.
    unsafe {
        GetVolumeInformationByHandleW(
            HANDLE(file.as_raw_handle()),
            None,
            None,
            None,
            Some(&mut flags),
            Some(&mut name),
        )
        .map_err(io_error)?;
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    Ok(VolumeFacts {
        drive_type: resolved.drive_type,
        filesystem: String::from_utf16_lossy(&name[..end]),
        flags,
        links,
        redirected: resolved.redirected,
        final_link: resolved.final_link,
        cloud: resolved.cloud,
    })
}

pub(crate) fn report_resolved(resolved: &Resolved) -> io::Result<CapabilityReport> {
    Ok(classify(&facts(resolved)?))
}

/// Capability of `path` for the local provider. Network names are classified
/// from the name or drive letter alone; nothing is sent to a remote host.
pub(crate) fn report(path: &Path) -> io::Result<CapabilityReport> {
    if is_network_name(path) {
        return Ok(classify(&VolumeFacts {
            drive_type: DRIVE_REMOTE,
            ..VolumeFacts::default()
        }));
    }
    report_resolved(&resolve(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(drive_type: u32, filesystem: &str) -> VolumeFacts {
        VolumeFacts {
            drive_type,
            filesystem: filesystem.into(),
            flags: FILE_PERSISTENT_ACLS | FILE_NAMED_STREAMS | FILE_SUPPORTS_HARD_LINKS,
            ..VolumeFacts::default()
        }
    }

    #[test]
    fn save_strategy_follows_volume_capability() {
        assert_eq!(classify(&volume(DRIVE_FIXED, "NTFS")).save, SaveStrategy::Transactional);
        assert_eq!(
            classify(&volume(DRIVE_REMOVABLE, "NTFS")).save,
            SaveStrategy::Transactional
        );
        for (drive, filesystem) in [
            (DRIVE_REMOVABLE, "exFAT"),
            (DRIVE_REMOVABLE, "FAT32"),
            (DRIVE_FIXED, "ReFS"),
            (DRIVE_RAMDISK, "FAT"),
        ] {
            let report = classify(&volume(drive, filesystem));
            assert_eq!(report.save, SaveStrategy::RenameReplace, "{filesystem}");
            assert_eq!(report.atomic_replace, Support::Unsupported);
            assert!(report.notice().is_some(), "weaker saves must surface a notice");
        }
        let report = classify(&volume(DRIVE_REMOVABLE, "exFAT"));
        assert_eq!(report.storage, StorageKind::Removable);
    }

    #[test]
    fn linked_targets_are_rewritten_in_place() {
        let hard_linked = VolumeFacts {
            links: 2,
            ..volume(DRIVE_FIXED, "NTFS")
        };
        assert_eq!(classify(&hard_linked).save, SaveStrategy::InPlace);
        let symbolic = VolumeFacts {
            final_link: true,
            redirected: true,
            ..volume(DRIVE_FIXED, "NTFS")
        };
        let report = classify(&symbolic);
        assert_eq!(report.save, SaveStrategy::InPlace);
        assert!(report.redirected);
        assert!(report.notice().is_some_and(|notice| notice.contains("in place")));
        let junction_only = VolumeFacts {
            redirected: true,
            ..volume(DRIVE_FIXED, "NTFS")
        };
        let report = classify(&junction_only);
        assert_eq!(report.save, SaveStrategy::Transactional);
        assert!(report.notice().is_some());
    }

    #[test]
    fn network_and_read_only_volumes_offer_save_copy() {
        let network = classify(&VolumeFacts {
            drive_type: DRIVE_REMOTE,
            ..VolumeFacts::default()
        });
        assert_eq!(network.storage, StorageKind::Network);
        assert_eq!(network.save, SaveStrategy::CopyOnly);
        assert_eq!(network.acl, Support::Unknown);
        assert!(network.notice().is_some_and(|notice| notice.contains("Save Copy")));
        let optical = classify(&VolumeFacts {
            flags: FILE_READ_ONLY_VOLUME,
            ..volume(DRIVE_CDROM, "UDF")
        });
        assert_eq!(optical.save, SaveStrategy::CopyOnly);
        assert!(optical.notice().is_some_and(|notice| notice.contains("Save Copy")));
        let unknown = classify(&volume(0, "NTFS"));
        assert_eq!(unknown.save, SaveStrategy::CopyOnly);
    }

    #[test]
    fn plain_local_ntfs_has_no_notice() {
        let report = classify(&volume(DRIVE_FIXED, "ntfs"));
        assert_eq!(report.save, SaveStrategy::Transactional);
        assert_eq!(report.hard_links, Support::Supported);
        assert!(report.notice().is_none());
    }

    #[test]
    fn cloud_placeholders_are_ordinary_only_while_local() {
        let cloud = FILE_ATTRIBUTE_REPARSE_POINT.0;
        assert!(ordinary_object(0, 0));
        assert!(ordinary_object(cloud, 0x9000_301A));
        assert!(!ordinary_object(cloud | FILE_ATTRIBUTE_OFFLINE.0, 0x9000_301A));
        assert!(!ordinary_object(
            cloud | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS.0,
            0x9000_001A
        ));
        // Junctions and symbolic links are never ordinary objects.
        assert!(!ordinary_object(cloud, 0xA000_0003));
        assert!(!ordinary_object(cloud, 0xA000_000C));
    }

    #[test]
    fn network_names_are_classified_without_access() {
        for path in [
            r"\\never-contact.invalid\share\x",
            r"\\?\UNC\never-contact.invalid\share\x",
        ] {
            let path = Path::new(path);
            assert!(is_network_name(path));
            assert_eq!(
                resolve(path).err().map(|error| error.kind()),
                Some(io::ErrorKind::PermissionDenied)
            );
            let report = report(path).unwrap();
            assert_eq!(report.storage, StorageKind::Network);
            assert_eq!(report.save, SaveStrategy::CopyOnly);
        }
    }

    #[test]
    fn relative_link_targets_stay_below_the_volume_root() {
        let base = Path::new(r"C:\a\b");
        assert_eq!(
            lexical_join(base, Path::new(r"..\c\.\d")).unwrap(),
            PathBuf::from(r"C:\a\c\d")
        );
        assert!(lexical_join(Path::new(r"C:\"), Path::new(r"..\x")).is_err());
        assert!(lexical_join(base, Path::new(r"\x")).is_err());
    }

    #[test]
    fn junction_ancestor_resolves_to_its_local_target() {
        let root = std::env::temp_dir().join(format!(
            "bareline-capability-junction-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("doc.txt"), b"through junction").unwrap();
        let junction = root.join("link");
        let status = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&real)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "junction fixture creation failed");
        let resolved = resolve(&junction.join("doc.txt")).unwrap();
        assert!(resolved.redirected);
        assert!(!resolved.final_link);
        assert_eq!(
            resolved.path.canonicalize().unwrap(),
            real.join("doc.txt").canonicalize().unwrap()
        );
        let report = report(&junction.join("doc.txt")).unwrap();
        assert!(report.redirected);
        assert_eq!(report.storage, StorageKind::Local);
        let missing = resolve(&junction.join("new.txt")).unwrap();
        assert_eq!(missing.path.file_name(), Some(OsStr::new("new.txt")));
        std::fs::remove_dir(&junction).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
