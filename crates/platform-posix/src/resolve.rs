// SPDX-License-Identifier: MPL-2.0
//! Lexical-and-`lstat` path resolution. Every component is examined with
//! `lstat` from the root down; a symbolic link is replaced by its target and the
//! walk continues, so the result names no link and records whether one was
//! crossed. `..` after a link refers to the link target's parent, as the kernel
//! resolves it. A missing final name is kept, so new files resolve too.
//!
//! Links of the system layout are not redirections: one owned by the superuser
//! in a folder only the superuser can change (macOS `/var`, `/tmp` and `/etc`,
//! merged-usr `/bin`, Linux `/var/run`) names the same object for everyone, as
//! a plain folder does. Every other link, such as one in a cloned repository or
//! an extracted archive, still counts as crossed.
use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    io,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

/// Linux and macOS both stop at 40 links per lookup (`MAXSYMLINKS`/ELOOP).
const MAX_LINKS: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub path: PathBuf,
    /// A symbolic link that is not part of the system layout was crossed
    /// anywhere on the way.
    pub redirected: bool,
    /// The final name itself was a symbolic link.
    pub final_link: bool,
}

enum Step {
    Name(OsString),
    Parent,
}

fn queue(steps: &mut VecDeque<Step>, path: &Path, front: bool) {
    let parts: Vec<Step> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(Step::Name(name.to_os_string())),
            Component::ParentDir => Some(Step::Parent),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => None,
        })
        .collect();
    if front {
        for part in parts.into_iter().rev() {
            steps.push_front(part);
        }
    } else {
        steps.extend(parts);
    }
}

pub(crate) fn resolve(path: &Path) -> io::Result<Resolved> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "only absolute local paths are supported",
        ));
    }
    let mut steps = VecDeque::new();
    queue(&mut steps, path, false);
    let mut resolved = PathBuf::from("/");
    let (mut redirected, mut final_link, mut links) = (false, false, 0);
    while let Some(step) = steps.pop_front() {
        let name = match step {
            Step::Parent => {
                resolved.pop();
                continue;
            }
            Step::Name(name) => name,
        };
        let last = steps.is_empty();
        let candidate = resolved.join(&name);
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                links += 1;
                if links > MAX_LINKS {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "too many symbolic links",
                    ));
                }
                let target = std::fs::read_link(&candidate)?;
                redirected |= !system_link(&metadata, &resolved);
                final_link |= last;
                if target.is_absolute() {
                    resolved = PathBuf::from("/");
                }
                queue(&mut steps, &target, true);
            }
            Ok(metadata) => {
                if !last && !metadata.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "a path component is a file",
                    ));
                }
                resolved = candidate;
            }
            // A new file below an existing folder.
            Err(error) if last && error.kind() == io::ErrorKind::NotFound => resolved = candidate,
            Err(error) => return Err(error),
        }
    }
    Ok(Resolved {
        path: resolved,
        redirected,
        final_link,
    })
}

/// Whether only the superuser can change where `link`, an entry of the
/// link-free `folder`, points: the link is root's, and so is the folder, which
/// nobody else may write to or which is sticky (only an entry's owner may then
/// replace it). Ownership proves nothing when the editor itself runs as the
/// superuser, so then every link counts as a redirection.
fn system_link(link: &std::fs::Metadata, folder: &Path) -> bool {
    const OTHERS_WRITE: u32 = 0o022;
    const STICKY: u32 = 0o1000;
    !rustix::process::geteuid().is_root()
        && link.uid() == 0
        && std::fs::symlink_metadata(folder).is_ok_and(|folder| {
            folder.is_dir() && folder.uid() == 0 && (folder.mode() & OTHERS_WRITE == 0 || folder.mode() & STICKY != 0)
        })
}

/// Paths that spell `..` are refused where trust is granted, as on Windows.
pub(crate) fn has_parent_step(path: &Path) -> bool {
    path.components().any(|component| component == Component::ParentDir)
}

/// Normal components of an absolute, link-free path.
pub(crate) fn names(path: &Path) -> io::Result<Vec<&OsStr>> {
    path.components()
        .filter(|component| *component != Component::RootDir)
        .map(|component| match component {
            Component::Normal(name) => Ok(name),
            _ => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "path requires a normalized absolute spelling",
            )),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("bareline-resolve-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn links_are_replaced_and_recorded() {
        let root = scratch("links");
        std::fs::create_dir(root.join("real")).unwrap();
        std::fs::write(root.join("real/doc.txt"), b"x").unwrap();
        symlink(root.join("real"), root.join("dir-link")).unwrap();
        symlink("doc.txt", root.join("real/file-link")).unwrap();

        let plain = resolve(&root.join("real/doc.txt")).unwrap();
        assert_eq!(plain.path, root.join("real/doc.txt"));
        assert!(!plain.redirected && !plain.final_link);

        let through = resolve(&root.join("dir-link/doc.txt")).unwrap();
        assert_eq!(through.path, root.join("real/doc.txt"));
        assert!(through.redirected && !through.final_link);

        let final_link = resolve(&root.join("dir-link/file-link")).unwrap();
        assert_eq!(final_link.path, root.join("real/doc.txt"));
        assert!(final_link.redirected && final_link.final_link);

        // `..` after a link climbs from the link target, not from the spelling.
        let parent = resolve(&root.join("dir-link/../real/doc.txt")).unwrap();
        assert_eq!(parent.path, root.join("real/doc.txt"));

        let missing = resolve(&root.join("dir-link/new.txt")).unwrap();
        assert_eq!(missing.path, root.join("real/new.txt"));
        assert!(resolve(&root.join("absent/new.txt")).is_err());
        assert!(resolve(Path::new("relative.txt")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The system's own links (macOS `/var` and `/tmp`, merged-usr `/bin`,
    /// `/var/run`) resolve without counting as a redirection, so documents in
    /// the temporary folder are trusted like any other; a link the person made
    /// still counts.
    #[test]
    fn system_layout_links_are_not_redirections() {
        let system: Vec<&str> = ["/var", "/tmp", "/etc", "/bin", "/lib", "/var/run", "/var/lock"]
            .into_iter()
            .filter(|name| {
                let path = Path::new(name);
                std::fs::symlink_metadata(path)
                    .is_ok_and(|link| link.file_type().is_symlink() && system_link(&link, path.parent().unwrap()))
            })
            .collect();
        for name in &system {
            let resolved = resolve(&Path::new(name).join("bareline-new.txt")).unwrap();
            assert!(!resolved.redirected && !resolved.final_link, "{name}");
            assert_eq!(
                resolved.path,
                std::fs::canonicalize(name).unwrap().join("bareline-new.txt"),
                "{name}"
            );
            // Named itself, the link is still reported as the final link.
            let itself = resolve(Path::new(name)).unwrap();
            assert!(!itself.redirected && itself.final_link, "{name}");
        }
        // Ubuntu (`/bin`, `/var/run`) and macOS (`/var`, `/tmp`) have several.
        eprintln!("system links checked: {system:?}");
        let root = scratch("own");
        std::fs::create_dir(root.join("real")).unwrap();
        symlink(root.join("real"), root.join("mine")).unwrap();
        assert!(resolve(&root.join("mine/doc.txt")).unwrap().redirected);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn link_cycles_stop() {
        let root = scratch("cycle");
        symlink(root.join("b"), root.join("a")).unwrap();
        symlink(root.join("a"), root.join("b")).unwrap();
        assert!(resolve(&root.join("a/x")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
