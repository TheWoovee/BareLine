// SPDX-License-Identifier: MPL-2.0
//! Filesystem capability classification (FIO-18) for Linux and macOS. The mount
//! facts come from `fstatfs`/`fstatvfs` on a descriptor of the resolved folder,
//! and the save policy is a pure function of the gathered facts.
//!
//! A replacement swaps the stage onto the name with `renameat2(RENAME_EXCHANGE)`
//! (Linux) or `renamex_np(RENAME_SWAP)` (APFS/HFS+), which keeps the displaced
//! file's identity like NTFS `ReplaceFileW`; those filesystems report a
//! transactional save without a notice. Elsewhere the displaced file is moved
//! aside first and the stage published with a no-replace rename.
use crate::{
    resolve::{self, Resolved},
    sys::{self, WALK},
    trust,
};
use bareline_platform::{CapabilityReport, FilesystemCapability, SaveStrategy, StorageKind, Support};
use std::{
    fs::File,
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// What the mount of a location supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mount {
    pub storage: StorageKind,
    /// Atomic exchange rename is implemented by the filesystem.
    pub exchange: bool,
    pub hard_links: Support,
    pub read_only: bool,
    /// A cloud-sync client owns the location.
    pub cloud: bool,
}

/// Facts the probe gathers for one location; tests inject them directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Facts {
    pub mount: Mount,
    /// Link count of the existing target file; zero when it is absent.
    pub links: u64,
    pub redirected: bool,
    pub final_link: bool,
}

/// Save policy: exchange-capable filesystems keep the transactional replace,
/// others replace through a same-directory stage, linked targets are rewritten
/// in place so every link sees the change, read-only mounts offer Save Copy.
pub(crate) fn classify(facts: &Facts) -> CapabilityReport {
    let known = facts.mount.storage != StorageKind::Unknown;
    let save = if !known || facts.mount.read_only {
        SaveStrategy::CopyOnly
    } else if facts.links > 1 || facts.final_link {
        SaveStrategy::InPlace
    } else if facts.mount.exchange {
        SaveStrategy::Transactional
    } else {
        SaveStrategy::RenameReplace
    };
    CapabilityReport {
        atomic_replace: match (known, facts.mount.exchange) {
            (false, _) => Support::Unknown,
            (true, true) => Support::Supported,
            (true, false) => Support::Unsupported,
        },
        acl: Support::Unknown,
        ads: Support::Unsupported,
        hard_links: if facts.links > 1 {
            Support::Supported
        } else {
            facts.mount.hard_links
        },
        storage: facts.mount.storage,
        save,
        redirected: facts.redirected,
        cloud: facts.mount.cloud,
    }
}

/// Mount facts of an open descriptor whose link-free path is `canonical`.
pub(crate) fn mount_of(file: &File, canonical: &Path) -> io::Result<Mount> {
    let read_only = rustix::fs::fstatvfs(file)?
        .f_flag
        .contains(rustix::fs::StatVfsMountFlags::RDONLY);
    let mut mount = platform::mount(file, canonical)?;
    mount.read_only |= read_only;
    Ok(mount)
}

pub(crate) fn report_resolved(resolved: &Resolved) -> io::Result<CapabilityReport> {
    // The nearest existing folder answers for its mount; a file adds its links.
    let (folder, links) = match std::fs::symlink_metadata(&resolved.path) {
        Ok(metadata) if metadata.is_dir() => (resolved.path.clone(), 0),
        Ok(metadata) => (parent(&resolved.path)?, metadata.nlink()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (parent(&resolved.path)?, 0),
        Err(error) => return Err(error),
    };
    let (_, directory) = trust::walk(&folder, WALK)?;
    Ok(classify(&Facts {
        mount: mount_of(&directory, &folder)?,
        links,
        redirected: resolved.redirected,
        final_link: resolved.final_link,
    }))
}

fn parent(path: &Path) -> io::Result<PathBuf> {
    Ok(sys::split(path)?.0.to_path_buf())
}

pub(crate) fn report(path: &Path) -> io::Result<CapabilityReport> {
    report_resolved(&resolve::resolve(path)?)
}

/// Capability reports for Linux and macOS locations.
#[derive(Clone, Copy, Debug, Default)]
pub struct PosixFilesystemCapability;
impl FilesystemCapability for PosixFilesystemCapability {
    fn report(&self, path: &Path) -> io::Result<CapabilityReport> {
        report(path)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    //! `f_type` magic numbers from `linux/magic.h`; FUSE mounts are told apart by
    //! the subtype `/proc/self/mountinfo` lists for the descriptor's device.
    use super::Mount;
    use bareline_platform::{StorageKind, Support};
    use std::{fs::File, io, io::Read, os::unix::fs::MetadataExt, path::Path};

    const EXT: u32 = 0xEF53;
    const BTRFS: u32 = 0x9123_683E;
    const XFS: u32 = 0x5846_5342;
    const TMPFS: u32 = 0x0102_1994;
    const F2FS: u32 = 0xF2F5_2010;
    const BCACHEFS: u32 = 0xCA45_1A4E;
    const MSDOS: u32 = 0x4D44;
    const EXFAT: u32 = 0x2011_BAB0;
    const ISOFS: u32 = 0x9660;
    const FUSE: u32 = 0x6573_5546;
    const NETWORK: [u32; 12] = [
        0x6969,      // NFS
        0x517B,      // SMB
        0xFF53_4D42, // CIFS
        0xFE53_4D42, // SMB2
        0x564C,      // NCP
        0x7375_7245, // Coda
        0x5346_414F, // AFS
        0x6B41_4653, // kAFS
        0x00C3_6400, // Ceph
        0x0102_1997, // 9P (also WSL drvfs)
        0x0BD0_0BD0, // Lustre
        0x4750_4653, // GPFS
    ];
    /// FUSE subtypes that serve local data.
    const LOCAL_FUSE: [&str; 14] = [
        "bindfs",
        "encfs",
        "gocryptfs",
        "cryfs",
        "securefs",
        "portal",
        "ntfs-3g",
        "exfat",
        "lxcfs",
        "squashfuse",
        "fuse-overlayfs",
        "mergerfs",
        "unionfs",
        "appimagelauncherfs",
    ];
    /// FUSE subtypes of cloud-sync clients.
    const CLOUD_FUSE: [&str; 8] = [
        "rclone",
        "google-drive-ocamlfuse",
        "onedriver",
        "s3fs",
        "gcsfuse",
        "goofys",
        "dropbox",
        "onedrive",
    ];
    const MOUNTINFO_LIMIT: u64 = 8 * 1024 * 1024;

    /// Mount-table answers per device and magic, reused briefly so a folder
    /// search does not reread `mountinfo` for every file it opens.
    struct Cached {
        dev: u64,
        magic: u32,
        at: std::time::Instant,
        fstype: Option<String>,
        removable: bool,
    }
    static CACHE: std::sync::Mutex<Vec<Cached>> = std::sync::Mutex::new(Vec::new());
    const CACHE_AGE: std::time::Duration = std::time::Duration::from_secs(5);

    pub(super) fn mount(file: &File, canonical: &Path) -> io::Result<Mount> {
        // Magic numbers fit 32 bits; the kernel word is wider on 64-bit targets.
        let magic = rustix::fs::fstatfs(file)?.f_type as u32;
        let dev = file.metadata()?.dev();
        let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.retain(|entry| entry.at.elapsed() < CACHE_AGE);
        if let Some(entry) = cache.iter().find(|entry| entry.dev == dev && entry.magic == magic) {
            return Ok(classify(magic, entry.fstype.as_deref(), entry.removable));
        }
        let (major, minor) = (rustix::fs::major(dev), rustix::fs::minor(dev));
        let entry = mount_entry(major, minor, canonical);
        let removable = entry
            .as_ref()
            .is_some_and(|entry| removable_mount_point(&entry.mount_point))
            || removable_device(major, minor);
        let fstype = entry.map(|entry| entry.fstype);
        let mount = classify(magic, fstype.as_deref(), removable);
        if cache.len() >= 64 {
            cache.remove(0);
        }
        cache.push(Cached {
            dev,
            magic,
            at: std::time::Instant::now(),
            fstype,
            removable,
        });
        Ok(mount)
    }

    pub(super) fn classify(magic: u32, fstype: Option<&str>, removable: bool) -> Mount {
        let fuse_subtype = fstype.map(|fstype| fstype.strip_prefix("fuse.").unwrap_or(fstype));
        let cloud = magic == FUSE && fuse_subtype.is_some_and(|subtype| CLOUD_FUSE.contains(&subtype));
        let local_fuse =
            magic == FUSE && fuse_subtype.is_some_and(|subtype| subtype == "fuseblk" || LOCAL_FUSE.contains(&subtype));
        let network = NETWORK.contains(&magic) || (magic == FUSE && !local_fuse);
        let storage = match (network, removable) {
            (true, _) => StorageKind::Network,
            (false, true) => StorageKind::Removable,
            (false, false) => StorageKind::Local,
        };
        Mount {
            storage,
            exchange: [EXT, BTRFS, XFS, TMPFS, F2FS, BCACHEFS].contains(&magic),
            hard_links: if [MSDOS, EXFAT, ISOFS].contains(&magic) {
                Support::Unsupported
            } else if network {
                Support::Unknown
            } else {
                Support::Supported
            },
            read_only: false,
            cloud,
        }
    }

    pub(super) struct MountEntry {
        pub mount_point: std::path::PathBuf,
        pub fstype: String,
    }

    /// The `mountinfo` entry of a device; bind mounts share a device, so the
    /// longest mount point containing the path wins.
    fn mount_entry(major: u32, minor: u32, canonical: &Path) -> Option<MountEntry> {
        let mut text = String::new();
        File::open("/proc/self/mountinfo")
            .ok()?
            .take(MOUNTINFO_LIMIT)
            .read_to_string(&mut text)
            .ok()?;
        parse_mountinfo(&text, major, minor, canonical)
    }

    pub(super) fn parse_mountinfo(text: &str, major: u32, minor: u32, canonical: &Path) -> Option<MountEntry> {
        let device = format!("{major}:{minor}");
        let mut best: Option<MountEntry> = None;
        for line in text.lines() {
            let mut fields = line.split(' ');
            let (Some(_), Some(_), Some(entry_device), Some(_), Some(mount_point)) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                continue;
            };
            if entry_device != device {
                continue;
            }
            // Optional fields end at a lone "-", followed by the filesystem type.
            let Some(fstype) = fields.skip_while(|field| *field != "-").nth(1) else {
                continue;
            };
            let mount_point = std::path::PathBuf::from(unescape(mount_point));
            let better = best
                .as_ref()
                .is_none_or(|best| mount_point.as_os_str().len() > best.mount_point.as_os_str().len());
            if canonical.starts_with(&mount_point) && better {
                best = Some(MountEntry {
                    mount_point,
                    fstype: unescape(fstype),
                });
            }
        }
        best
    }

    /// `mountinfo` escapes space, tab, newline and backslash as `\ooo`.
    fn unescape(field: &str) -> String {
        let bytes = field.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            let octal = bytes.get(index + 1..index + 4).and_then(|digits| {
                std::str::from_utf8(digits)
                    .ok()
                    .and_then(|digits| u8::from_str_radix(digits, 8).ok())
            });
            match (bytes[index], octal) {
                (b'\\', Some(byte)) => {
                    out.push(byte);
                    index += 4;
                }
                (byte, _) => {
                    out.push(byte);
                    index += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn removable_mount_point(mount_point: &Path) -> bool {
        mount_point.starts_with("/media") || mount_point.starts_with("/run/media")
    }

    /// The kernel's removable-media flag of the block device or, for a
    /// partition, of the disk that holds it.
    fn removable_device(major: u32, minor: u32) -> bool {
        if major == 0 {
            return false;
        }
        let device = format!("/sys/dev/block/{major}:{minor}");
        ["removable", "../removable"]
            .iter()
            .any(|flag| std::fs::read(format!("{device}/{flag}")).is_ok_and(|value| value.first() == Some(&b'1')))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    //! `f_fstypename` and mount flags from `statfs`; iCloud Drive and File
    //! Provider folders under the home folder are cloud locations.
    use super::Mount;
    use bareline_platform::{StorageKind, Support};
    use std::{fs::File, io, path::Path};

    // <sys/mount.h>
    const MNT_RDONLY: u32 = 0x0000_0001;
    const MNT_REMOVABLE: u32 = 0x0000_0200;
    const MNT_LOCAL: u32 = 0x0000_1000;

    pub(super) fn mount(file: &File, canonical: &Path) -> io::Result<Mount> {
        let stat = rustix::fs::fstatfs(file)?;
        let name: Vec<u8> = stat
            .f_fstypename
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let cloud = home.is_some_and(|home| {
            home.is_absolute()
                && ["Library/Mobile Documents", "Library/CloudStorage"]
                    .iter()
                    .any(|folder| canonical.starts_with(home.join(folder)))
        });
        Ok(classify(&String::from_utf8_lossy(&name), stat.f_flags, cloud))
    }

    pub(super) fn classify(fstype: &str, flags: u32, cloud: bool) -> Mount {
        let network = matches!(fstype, "nfs" | "smbfs" | "afpfs" | "webdav" | "cifs" | "ftp") || flags & MNT_LOCAL == 0;
        let storage = if network {
            StorageKind::Network
        } else if flags & MNT_REMOVABLE != 0 {
            StorageKind::Removable
        } else {
            StorageKind::Local
        };
        Mount {
            storage,
            exchange: matches!(fstype, "apfs" | "hfs"),
            hard_links: if matches!(fstype, "msdos" | "exfat" | "cd9660") {
                Support::Unsupported
            } else if network {
                Support::Unknown
            } else {
                Support::Supported
            },
            read_only: flags & MNT_RDONLY != 0,
            cloud,
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    //! Other Unix systems: rename semantics are POSIX, mount kinds are unknown.
    use super::Mount;
    use bareline_platform::{StorageKind, Support};
    use std::{fs::File, io, path::Path};

    pub(super) fn mount(_: &File, _: &Path) -> io::Result<Mount> {
        Ok(Mount {
            storage: StorageKind::Local,
            exchange: false,
            hard_links: Support::Unknown,
            read_only: false,
            cloud: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(exchange: bool) -> Mount {
        Mount {
            storage: StorageKind::Local,
            exchange,
            hard_links: Support::Supported,
            read_only: false,
            cloud: false,
        }
    }
    fn facts(mount: Mount) -> Facts {
        Facts {
            mount,
            links: 1,
            redirected: false,
            final_link: false,
        }
    }

    #[test]
    fn save_strategy_follows_mount_capability() {
        let report = classify(&facts(local(true)));
        assert_eq!(report.save, SaveStrategy::Transactional);
        assert_eq!(report.atomic_replace, Support::Supported);
        assert_eq!(report.ads, Support::Unsupported);
        assert_eq!(report.acl, Support::Unknown);
        assert!(report.notice().is_none(), "a plain local save needs no notice");

        let report = classify(&facts(local(false)));
        assert_eq!(report.save, SaveStrategy::RenameReplace);
        assert_eq!(report.atomic_replace, Support::Unsupported);
        assert!(report.notice().is_some());

        let report = classify(&Facts {
            links: 2,
            ..facts(local(true))
        });
        assert_eq!(report.save, SaveStrategy::InPlace);
        assert_eq!(report.hard_links, Support::Supported);

        let report = classify(&Facts {
            final_link: true,
            redirected: true,
            ..facts(local(true))
        });
        assert_eq!(report.save, SaveStrategy::InPlace);
        assert!(report.redirected);

        let read_only = classify(&facts(Mount {
            read_only: true,
            ..local(true)
        }));
        assert_eq!(read_only.save, SaveStrategy::CopyOnly);
        assert!(read_only.notice().is_some_and(|notice| notice.contains("Save Copy")));

        let unknown = classify(&facts(Mount {
            storage: StorageKind::Unknown,
            ..local(true)
        }));
        assert_eq!(unknown.save, SaveStrategy::CopyOnly);
        assert_eq!(unknown.atomic_replace, Support::Unknown);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_mounts_are_classified_by_magic_and_subtype() {
        let ext4 = platform::classify(0xEF53, Some("ext4"), false);
        assert_eq!((ext4.storage, ext4.exchange), (StorageKind::Local, true));
        let usb = platform::classify(0x4D44, Some("vfat"), true);
        assert_eq!(usb.storage, StorageKind::Removable);
        assert_eq!(usb.hard_links, Support::Unsupported);
        assert!(!usb.exchange);
        for (magic, fstype) in [
            (0x6969, "nfs4"),
            (0xFF53_4D42, "cifs"),
            (0x0102_1997, "9p"),
            (0x6573_5546, "fuse.sshfs"),
        ] {
            let mount = platform::classify(magic, Some(fstype), false);
            assert_eq!(mount.storage, StorageKind::Network, "{fstype}");
            assert!(!mount.exchange);
        }
        let drive = platform::classify(0x6573_5546, Some("fuse.rclone"), false);
        assert!(drive.cloud && drive.storage == StorageKind::Network);
        let ntfs = platform::classify(0x6573_5546, Some("fuseblk"), false);
        assert_eq!(ntfs.storage, StorageKind::Local);
        assert!(!platform::classify(0x6573_5546, None, false).cloud);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_picks_the_longest_mount_of_the_device() {
        let text = "22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n\
                    40 22 8:1 /srv /srv\\040data rw shared:2 - ext4 /dev/sda1 rw\n\
                    41 22 0:50 / /mnt/share rw - nfs4 server:/export rw\n";
        let entry = platform::parse_mountinfo(text, 8, 1, Path::new("/srv data/doc.txt")).unwrap();
        assert_eq!(entry.mount_point, Path::new("/srv data"));
        assert_eq!(entry.fstype, "ext4");
        let root = platform::parse_mountinfo(text, 8, 1, Path::new("/home/user/doc.txt")).unwrap();
        assert_eq!(root.mount_point, Path::new("/"));
        let nfs = platform::parse_mountinfo(text, 0, 50, Path::new("/mnt/share/x")).unwrap();
        assert_eq!(nfs.fstype, "nfs4");
        assert!(platform::parse_mountinfo(text, 9, 9, Path::new("/x")).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_mounts_are_classified_by_type_and_flags() {
        let apfs = platform::classify("apfs", 0x1000, false);
        assert_eq!((apfs.storage, apfs.exchange), (StorageKind::Local, true));
        let stick = platform::classify("msdos", 0x1000 | 0x200, false);
        assert_eq!(stick.storage, StorageKind::Removable);
        assert_eq!(stick.hard_links, Support::Unsupported);
        for fstype in ["smbfs", "nfs", "afpfs", "webdav"] {
            assert_eq!(platform::classify(fstype, 0, false).storage, StorageKind::Network);
        }
        assert!(platform::classify("apfs", 0x1000 | 0x1, false).read_only);
        assert!(platform::classify("apfs", 0x1000, true).cloud);
    }

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("bareline-capability-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn real_locations_report_links_and_redirection() {
        let root = scratch("real");
        let target = root.join("doc.txt");
        std::fs::write(&target, b"x").unwrap();
        let plain = report(&target).unwrap();
        assert!(!plain.redirected);
        assert_ne!(plain.save, SaveStrategy::InPlace);
        assert_ne!(plain.save, SaveStrategy::CopyOnly);
        // A missing file is reported through its folder.
        assert_eq!(report(&root.join("new.txt")).unwrap().save, plain.save);

        std::fs::hard_link(&target, root.join("alias.txt")).unwrap();
        let linked = report(&target).unwrap();
        assert_eq!(linked.save, SaveStrategy::InPlace);
        assert_eq!(linked.hard_links, Support::Supported);
        std::fs::remove_file(root.join("alias.txt")).unwrap();

        std::os::unix::fs::symlink(&target, root.join("link.txt")).unwrap();
        let symbolic = report(&root.join("link.txt")).unwrap();
        assert!(symbolic.redirected);
        assert_eq!(symbolic.save, SaveStrategy::InPlace);
        std::fs::create_dir(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("folder-link")).unwrap();
        let through = report(&root.join("folder-link/new.txt")).unwrap();
        assert!(through.redirected);
        assert_eq!(through.save, plain.save);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A read-only mount offers Save Copy only. The fixture needs a read-only
    /// mount the test can see; the test reports why it is skipped otherwise.
    #[test]
    fn read_only_mount_yields_copy_only() {
        let Some(folder) = read_only_folder() else {
            eprintln!("skipped: no read-only mount is visible to this test process");
            return;
        };
        let report = report(&folder).unwrap();
        assert_eq!(report.save, SaveStrategy::CopyOnly, "{}", folder.display());
    }

    #[cfg(target_os = "macos")]
    fn read_only_folder() -> Option<PathBuf> {
        // The sealed system volume is mounted read-only on macOS 11 and later.
        let folder = PathBuf::from("/usr/bin");
        rustix::fs::statvfs(&folder)
            .ok()
            .filter(|stat| stat.f_flag.contains(rustix::fs::StatVfsMountFlags::RDONLY))
            .map(|_| folder)
    }

    #[cfg(not(target_os = "macos"))]
    fn read_only_folder() -> Option<PathBuf> {
        let text = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
        text.lines()
            .filter_map(|line| line.split(' ').nth(4))
            .filter(|mount_point| !mount_point.contains('\\'))
            .map(PathBuf::from)
            .find(|mount_point| {
                std::fs::symlink_metadata(mount_point).is_ok_and(|metadata| metadata.is_dir())
                    && resolve::resolve(mount_point).is_ok_and(|resolved| &resolved.path == mount_point)
                    && rustix::fs::statvfs(mount_point)
                        .is_ok_and(|stat| stat.f_flag.contains(rustix::fs::StatVfsMountFlags::RDONLY))
            })
    }
}
