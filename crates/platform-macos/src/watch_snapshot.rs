// SPDX-License-Identifier: MPL-2.0
//! Directory snapshots and their differences in the Windows watch vocabulary.
//!
//! kqueue reports that a directory changed, not what changed in it. The
//! watcher keeps a snapshot of each directory's entries (device, inode, size,
//! modification time) and turns the difference between two snapshots into the
//! events `ReadDirectoryChangesW` delivers on Windows: Created, Removed,
//! Modified, and RenameFrom/RenameTo pairs for an inode that moved to another
//! name. A directory too large to snapshot reports RescanNeeded instead.
use bareline_platform::{WatchEvent, WatchKind};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Entries tracked per directory; a larger directory is watched for "something
/// changed" only, and consumers rescan it.
pub(crate) const SNAPSHOT_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) length: u64,
    pub(crate) modified_ns: i128,
    pub(crate) regular: bool,
}
impl Stamp {
    pub(crate) fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified_ns: i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec()),
            regular: metadata.file_type().is_file(),
        }
    }
    fn node(&self) -> (u64, u64) {
        (self.device, self.inode)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) entries: BTreeMap<OsString, Stamp>,
    /// False when the directory held more than [`SNAPSHOT_LIMIT`] entries.
    pub(crate) complete: bool,
}
impl Snapshot {
    /// Entries are read without following links. An entry that disappears
    /// while the directory is read is simply not listed.
    pub(crate) fn read(directory: &Path) -> io::Result<Self> {
        let mut entries = BTreeMap::new();
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entries.len() == SNAPSHOT_LIMIT {
                return Ok(Self {
                    entries,
                    complete: false,
                });
            }
            match std::fs::symlink_metadata(entry.path()) {
                Ok(metadata) => {
                    entries.insert(entry.file_name(), Stamp::of(&metadata));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(Self {
            entries,
            complete: true,
        })
    }
}

pub(crate) fn rescan(directory: &Path) -> WatchEvent {
    WatchEvent {
        directory: directory.into(),
        name: PathBuf::new(),
        kind: WatchKind::RescanNeeded,
    }
}

/// Events that turn `before` into `after`, in this order: rename pairs, then
/// removals, creations and modifications, each by name.
pub(crate) fn diff(directory: &Path, before: &Snapshot, after: &Snapshot) -> Vec<WatchEvent> {
    if !before.complete || !after.complete {
        return vec![rescan(directory)];
    }
    let event = |name: &OsString, kind| WatchEvent {
        directory: directory.into(),
        name: PathBuf::from(name),
        kind,
    };
    let after_by_node: BTreeMap<(u64, u64), &OsString> =
        after.entries.iter().map(|(name, stamp)| (stamp.node(), name)).collect();
    let mut events = Vec::new();
    let mut renamed_from = BTreeSet::new();
    let mut renamed_to = BTreeSet::new();
    for (name, stamp) in &before.entries {
        let stayed = after.entries.get(name).is_some_and(|now| now.node() == stamp.node());
        if stayed {
            continue;
        }
        // The same object under another name that did not hold it before.
        if let Some(&target) = after_by_node.get(&stamp.node())
            && target != name
            && before.entries.get(target).is_none_or(|old| old.node() != stamp.node())
            && renamed_to.insert(target.clone())
        {
            renamed_from.insert(name.clone());
            events.push(event(name, WatchKind::RenameFrom));
            events.push(event(target, WatchKind::RenameTo));
        }
    }
    for name in before.entries.keys() {
        if !after.entries.contains_key(name) && !renamed_from.contains(name) {
            events.push(event(name, WatchKind::Removed));
        }
    }
    for (name, stamp) in &after.entries {
        match before.entries.get(name) {
            None if !renamed_to.contains(name) => events.push(event(name, WatchKind::Created)),
            None => {}
            // The old object moved elsewhere and a new one took the name; one
            // that arrived by rename was already reported as RenameTo.
            Some(old) if old.node() != stamp.node() && renamed_from.contains(name) => {
                if !renamed_to.contains(name) {
                    events.push(event(name, WatchKind::Created));
                }
            }
            // Replaced in place (an atomic save) or rewritten. Directories
            // report only their own creation, removal and renaming.
            Some(old) if old != stamp && (stamp.regular || old.node() != stamp.node()) => {
                events.push(event(name, WatchKind::Modified));
            }
            Some(_) => {}
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(inode: u64, length: u64, modified_ns: i128) -> Stamp {
        Stamp {
            device: 7,
            inode,
            length,
            modified_ns,
            regular: true,
        }
    }
    fn snapshot(entries: &[(&str, Stamp)]) -> Snapshot {
        Snapshot {
            entries: entries
                .iter()
                .map(|(name, stamp)| (OsString::from(name), *stamp))
                .collect(),
            complete: true,
        }
    }
    fn kinds(events: &[WatchEvent]) -> Vec<(String, WatchKind)> {
        events
            .iter()
            .map(|event| (event.name.to_string_lossy().into_owned(), event.kind))
            .collect()
    }

    #[test]
    fn creations_removals_and_modifications_are_named() {
        let before = snapshot(&[
            ("kept", stamp(1, 3, 10)),
            ("gone", stamp(2, 3, 10)),
            ("same", stamp(4, 1, 1)),
        ]);
        let after = snapshot(&[
            ("kept", stamp(1, 9, 20)),
            ("new", stamp(3, 0, 30)),
            ("same", stamp(4, 1, 1)),
        ]);
        let events = diff(Path::new("/w"), &before, &after);
        assert_eq!(
            kinds(&events),
            [
                ("gone".into(), WatchKind::Removed),
                ("kept".into(), WatchKind::Modified),
                ("new".into(), WatchKind::Created),
            ]
        );
        assert!(events.iter().all(|event| event.directory == Path::new("/w")));
        assert!(diff(Path::new("/w"), &after, &after).is_empty());
    }

    #[test]
    fn a_moved_inode_is_an_ordered_rename_pair() {
        let before = snapshot(&[("draft.txt", stamp(5, 3, 10))]);
        let after = snapshot(&[("final.txt", stamp(5, 3, 10))]);
        assert_eq!(
            kinds(&diff(Path::new("/w"), &before, &after)),
            [
                ("draft.txt".into(), WatchKind::RenameFrom),
                ("final.txt".into(), WatchKind::RenameTo),
            ]
        );
    }

    #[test]
    fn atomic_saves_and_backups_keep_their_meaning() {
        // An editor wrote a temporary file and renamed it over the name.
        let before = snapshot(&[("notes.txt", stamp(1, 3, 10))]);
        let after = snapshot(&[("notes.txt", stamp(9, 4, 20))]);
        assert_eq!(
            kinds(&diff(Path::new("/w"), &before, &after)),
            [("notes.txt".into(), WatchKind::Modified)]
        );
        // The old file moved to a backup name and a new one took its name.
        let after = snapshot(&[("notes.txt", stamp(9, 4, 20)), ("notes.txt~", stamp(1, 3, 10))]);
        assert_eq!(
            kinds(&diff(Path::new("/w"), &before, &after)),
            [
                ("notes.txt".into(), WatchKind::RenameFrom),
                ("notes.txt~".into(), WatchKind::RenameTo),
                ("notes.txt".into(), WatchKind::Created),
            ]
        );
    }

    #[test]
    fn swapped_names_and_directories_are_reported_once() {
        // Two names exchanged their objects (renamex_np RENAME_SWAP).
        let before = snapshot(&[("a", stamp(1, 1, 1)), ("b", stamp(2, 2, 2))]);
        let after = snapshot(&[("a", stamp(2, 2, 2)), ("b", stamp(1, 1, 1))]);
        let events = kinds(&diff(Path::new("/w"), &before, &after));
        assert_eq!(
            events,
            [
                ("a".into(), WatchKind::RenameFrom),
                ("b".into(), WatchKind::RenameTo),
                ("b".into(), WatchKind::RenameFrom),
                ("a".into(), WatchKind::RenameTo),
            ]
        );
        // A subdirectory whose contents changed is not itself modified.
        let folder = |modified_ns| Stamp {
            regular: false,
            ..stamp(3, 64, modified_ns)
        };
        let before = snapshot(&[("src", folder(1))]);
        let after = snapshot(&[("src", folder(2))]);
        assert!(diff(Path::new("/w"), &before, &after).is_empty());
    }

    #[test]
    fn incomplete_snapshots_ask_for_a_rescan() {
        let mut large = snapshot(&[("a", stamp(1, 1, 1))]);
        large.complete = false;
        let small = snapshot(&[]);
        for (before, after) in [(&large, &small), (&small, &large)] {
            let events = diff(Path::new("/w"), before, after);
            assert_eq!(events, [rescan(Path::new("/w"))]);
        }
    }

    #[test]
    fn snapshots_read_entries_without_following_links() {
        let root = std::env::temp_dir().join(format!(
            "bareline-macos-snapshot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("folder")).unwrap();
        std::fs::write(root.join("file.txt"), b"one").unwrap();
        std::os::unix::fs::symlink(root.join("file.txt"), root.join("link")).unwrap();
        let first = Snapshot::read(&root).unwrap();
        assert!(first.complete);
        assert_eq!(first.entries.len(), 3);
        assert!(first.entries[&OsString::from("file.txt")].regular);
        assert!(!first.entries[&OsString::from("folder")].regular);
        // The link is its own entry, not the file it names.
        assert!(!first.entries[&OsString::from("link")].regular);
        std::fs::write(root.join("file.txt"), b"longer contents").unwrap();
        std::fs::rename(root.join("link"), root.join("renamed-link")).unwrap();
        let second = Snapshot::read(&root).unwrap();
        assert_eq!(
            kinds(&diff(&root, &first, &second)),
            [
                ("link".into(), WatchKind::RenameFrom),
                ("renamed-link".into(), WatchKind::RenameTo),
                ("file.txt".into(), WatchKind::Modified),
            ]
        );
        std::fs::remove_dir_all(&root).unwrap();
        assert!(Snapshot::read(&root).is_err());
    }
}
