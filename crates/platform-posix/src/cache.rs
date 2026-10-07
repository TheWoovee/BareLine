// SPDX-License-Identifier: MPL-2.0
//! Owned-cache leases, profile-migration leases and bounded owned-cache removal.
//!
//! POSIX has no sharing modes, so exclusion between processes is cooperative:
//! a cache lease holds a shared `flock` on the folder's descriptor and removal
//! takes the exclusive lock without waiting, so a folder another Bareline
//! process still leases is never deleted under it. Removal walks below retained
//! descriptors with `O_NOFOLLOW` and unlinks names relative to them; links and
//! special files inside a cache refuse the removal, like reparse points on
//! Windows.
use crate::{
    sys::{self, DIRECTORY, Node, READ, changed, denied, invalid_data},
    trust::{self, DirectoryGuard},
};
use bareline_platform::{CacheDirectoryIdentity, CacheDirectoryLease, CacheRemovalOutcome};
use rustix::{
    fs::{AtFlags, FileType, FlockOperation},
    io::Errno,
};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Write},
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

const RECORD: &str = ".bareline-cache-owner.json";
const RETAINED_RECORD: &str = ".bareline-cache-cleanup-retained.json";
const MAX_RECORD: u64 = 4096;

fn identity(node: Node) -> CacheDirectoryIdentity {
    CacheDirectoryIdentity {
        volume: node.dev,
        file: node.ino,
    }
}

/// Take an advisory lock without waiting. A filesystem without `flock` support
/// keeps the lease without exclusion rather than refusing every cache.
pub(crate) fn lock(file: &File, operation: FlockOperation) -> io::Result<()> {
    match rustix::fs::flock(file, operation) {
        Ok(()) => Ok(()),
        Err(Errno::WOULDBLOCK) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "another Bareline process holds this folder",
        )),
        // Some filesystems (and some folders on them) answer EINVAL for flock.
        Err(errno) if [Errno::NOTSUP, Errno::OPNOTSUPP, Errno::NOLCK, Errno::INVAL].contains(&errno) => Ok(()),
        Err(errno) => Err(errno.into()),
    }
}

/// A shared lease on a cache folder: the descriptor and its folder chain.
struct Lease {
    #[allow(dead_code)]
    parent: DirectoryGuard,
    #[allow(dead_code)]
    directory: File,
}

/// `None` for a missing name or one that is not a plain folder.
pub(crate) fn directory_lease(path: &Path) -> io::Result<Option<CacheDirectoryLease>> {
    let (parent, name) = sys::split(path)?;
    let parent = match DirectoryGuard::open(parent) {
        Ok(parent) => parent,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let directory = match sys::open_at(&parent, name, DIRECTORY) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        // A link (ELOOP) or a file (ENOTDIR) at the name is not a plain folder.
        Err(error) if sys::is_errno(&error, Errno::LOOP) || sys::is_errno(&error, Errno::NOTDIR) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    lock(&directory, FlockOperation::NonBlockingLockShared)?;
    let node = Node::of(&directory)?;
    Ok(Some(CacheDirectoryLease {
        path: path.to_path_buf(),
        identity: identity(node),
        guard: Arc::new(Lease { parent, directory }),
        migration_publisher: None,
    }))
}

/// A migration lease holds the entry (file or folder) exclusively against other
/// leases and publishes it with a no-replace rename of the leased object.
struct MigrationEntry {
    parent: DirectoryGuard,
    name: OsString,
    node: Node,
    #[allow(dead_code)]
    entry: File,
}

pub(crate) fn migration_lease(path: &Path) -> io::Result<CacheDirectoryLease> {
    let (parent, name) = sys::split(path)?;
    let parent = DirectoryGuard::open(parent)?;
    let entry = sys::open_at(&parent, name, READ).map_err(sys::no_follow)?;
    let metadata = entry.metadata()?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(denied("migration entries must be files or folders"));
    }
    lock(&entry, FlockOperation::NonBlockingLockExclusive)?;
    let node = Node::of(&entry)?;
    let guard = Arc::new(MigrationEntry {
        parent,
        name: name.to_os_string(),
        node,
        entry,
    });
    let publisher = guard.clone();
    Ok(CacheDirectoryLease {
        path: path.to_path_buf(),
        identity: identity(node),
        guard,
        migration_publisher: Some(Arc::new(move |target: &Path| publisher.publish(target))),
    })
}

impl MigrationEntry {
    fn publish(&self, target: &Path) -> io::Result<()> {
        let (target_parent, target_name) = sys::split(target)?;
        let target_parent = DirectoryGuard::open(target_parent)?;
        if sys::stat_name(&self.parent, &self.name)?.node != self.node {
            return Err(changed());
        }
        sys::rename_no_replace(&self.parent, &self.name, &target_parent, target_name)?;
        sys::sync_directory(&target_parent.directory)?;
        sys::sync_directory(&self.parent.directory)
    }
}

pub(crate) fn open_migration_read(lease: &CacheDirectoryLease) -> io::Result<File> {
    let (parent, name) = sys::split(&lease.path)?;
    let parent = DirectoryGuard::open(parent)?;
    let file = sys::open_at(&parent, name, READ).map_err(sys::no_follow)?;
    if identity(Node::of(&file)?) != lease.identity {
        return Err(invalid_data("migration read identity changed"));
    }
    Ok(file)
}

/// One folder of the removal walk: its descriptor and the names it holds.
struct Folder {
    directory: File,
    files: Vec<OsString>,
    /// Child folders as (name, index into the walk), removed bottom-up.
    folders: Vec<(OsString, usize)>,
}

struct Walk<'a> {
    started: Instant,
    max_entries: usize,
    max_time: Duration,
    cancelled: &'a dyn Fn() -> bool,
    visited: usize,
    folders: Vec<Folder>,
}
impl Walk<'_> {
    fn budget(&self) -> io::Result<()> {
        if (self.cancelled)() {
            Err(io::Error::new(io::ErrorKind::Interrupted, "cache cleanup cancelled"))
        } else if self.started.elapsed() >= self.max_time || self.visited > self.max_entries {
            Err(limit())
        } else {
            Ok(())
        }
    }

    fn collect(&mut self, directory: File) -> io::Result<usize> {
        self.budget()?;
        let index = self.folders.len();
        self.folders.push(Folder {
            directory,
            files: Vec::new(),
            folders: Vec::new(),
        });
        let entries = rustix::fs::Dir::read_from(&self.folders[index].directory)?;
        for entry in entries {
            let entry = entry?;
            let name = OsStr::from_bytes(entry.file_name().to_bytes());
            if name == "." || name == ".." {
                continue;
            }
            self.visited = self.visited.checked_add(1).ok_or_else(limit)?;
            self.budget()?;
            let name = name.to_os_string();
            match sys::stat_name(&self.folders[index].directory, &name)?.kind {
                FileType::RegularFile => self.folders[index].files.push(name),
                FileType::Directory => {
                    let child =
                        sys::open_at(&self.folders[index].directory, &name, DIRECTORY).map_err(sys::no_follow)?;
                    let child_index = self.collect(child)?;
                    self.folders[index].folders.push((name, child_index));
                }
                _ => return Err(denied("cache contains a link or an unsupported entry")),
            }
        }
        Ok(index)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn remove(
    root: &Path,
    candidate: &Path,
    expected_root: CacheDirectoryIdentity,
    expected_candidate: CacheDirectoryIdentity,
    proof_name: &str,
    proof_bytes: &[u8],
    max_entries: usize,
    max_time: Duration,
    cancelled: &dyn Fn() -> bool,
) -> CacheRemovalOutcome {
    remove_with_hook(
        root,
        candidate,
        (expected_root, expected_candidate),
        (proof_name, proof_bytes),
        (max_entries, max_time),
        cancelled,
        &|_| Ok(()),
    )
}

/// Test hook: runs after the walk released the candidate, before finalization.
type Hook<'a> = &'a dyn Fn(&Path) -> io::Result<()>;

fn remove_with_hook(
    root: &Path,
    candidate: &Path,
    (expected_root, expected_candidate): (CacheDirectoryIdentity, CacheDirectoryIdentity),
    (proof_name, proof_bytes): (&str, &[u8]),
    (max_entries, max_time): (usize, Duration),
    cancelled: &dyn Fn() -> bool,
    hook: Hook<'_>,
) -> CacheRemovalOutcome {
    let mut walk = Walk {
        started: Instant::now(),
        max_entries,
        max_time,
        cancelled,
        visited: 0,
        folders: Vec::new(),
    };
    let mut retry_authority_retained = true;
    let result = remove_inner(
        root,
        candidate,
        (expected_root, expected_candidate),
        (proof_name, proof_bytes),
        &mut walk,
        &mut retry_authority_retained,
        hook,
    );
    CacheRemovalOutcome {
        visited: walk.visited,
        retry_authority_retained,
        result,
    }
}

fn remove_inner(
    root: &Path,
    candidate: &Path,
    (expected_root, expected_candidate): (CacheDirectoryIdentity, CacheDirectoryIdentity),
    (proof_name, proof_bytes): (&str, &[u8]),
    walk: &mut Walk<'_>,
    retry_authority_retained: &mut bool,
    hook: Hook<'_>,
) -> io::Result<usize> {
    if !root.is_absolute() || candidate.parent() != Some(root) {
        return Err(denied("cache candidate escapes its root"));
    }
    if proof_name != RECORD && proof_name != RETAINED_RECORD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid ownership proof name",
        ));
    }
    let name = candidate
        .file_name()
        .ok_or_else(|| denied("cache candidate has no name"))?;
    let root_guard = trust::DirectoryGuard::open(root)?;
    if root_guard.identity()? != expected_root {
        return Err(changed());
    }
    let directory = sys::open_at(&root_guard, name, DIRECTORY).map_err(sys::no_follow)?;
    let node = Node::of(&directory)?;
    if identity(node) != expected_candidate {
        return Err(changed());
    }
    // A cancelled request answers Interrupted before it ever contends for the
    // lease: a child another thread is spawning holds duplicates of every
    // descriptor until it execs, so the lock can be busy for a moment even
    // when no other owner exists.
    walk.budget()?;
    // A lease held anywhere keeps the folder; the sweep retries later.
    lock(&directory, FlockOperation::NonBlockingLockExclusive)?;
    walk.visited = 1;
    walk.collect(directory)?;

    let mut proof = sys::open_at(&walk.folders[0].directory, proof_name, READ).map_err(sys::no_follow)?;
    if !proof.metadata()?.is_file() || sys::read_bounded(&mut proof, MAX_RECORD)? != proof_bytes {
        return Err(denied("ownership proof changed"));
    }
    drop(proof);
    let mut removed = 0;
    // Children come after their parent in the walk, so reverse order is bottom-up.
    for index in (0..walk.folders.len()).rev() {
        let folder = &walk.folders[index];
        for file in folder
            .files
            .iter()
            .filter(|file| index != 0 || file.as_os_str() != proof_name)
        {
            walk.budget()?;
            rustix::fs::unlinkat(&folder.directory, file, AtFlags::empty())?;
            removed += 1;
        }
        for (child, _) in &folder.folders {
            walk.budget()?;
            rustix::fs::unlinkat(&folder.directory, child, AtFlags::REMOVEDIR)?;
            removed += 1;
        }
    }
    walk.budget()?;
    hook(candidate)?;
    // The name must still hold the walked folder before its proof goes.
    if sys::stat_name(&root_guard, name)?.node != node {
        return Err(changed());
    }
    let candidate_directory = walk.folders.swap_remove(0).directory;
    rustix::fs::unlinkat(&candidate_directory, proof_name, AtFlags::empty())?;
    match rustix::fs::unlinkat(&root_guard, name, AtFlags::REMOVEDIR) {
        Ok(()) => {
            sys::sync_directory(&root_guard.directory)?;
            Ok(removed + 2)
        }
        Err(errno) => {
            let error = io::Error::from(errno);
            if let Err(restore) = restore_authority(&candidate_directory, proof_bytes) {
                *retry_authority_retained = false;
                return Err(io::Error::other(format!(
                    "cache directory removal failed ({error}); cleanup authority restoration failed ({restore})"
                )));
            }
            Err(error)
        }
    }
}

fn restore_record(directory: &File, name: &str, bytes: &[u8]) -> io::Result<()> {
    let mut file = sys::open_at(directory, name, sys::CREATE)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn restore_authority(directory: &File, bytes: &[u8]) -> io::Result<()> {
    match restore_record(directory, RECORD, bytes) {
        Ok(()) => Ok(()),
        Err(primary) => restore_record(directory, RETAINED_RECORD, bytes)
            .map_err(|retained| io::Error::other(format!("owner record: {primary}; retained receipt: {retained}"))),
    }
}

fn limit() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "cache deletion budget exhausted")
}

/// Child-process side of the cross-process lease test: hold a lease on the
/// folder named by the environment until stdin closes.
#[cfg(test)]
pub(crate) fn hold_lease_for_parent(path: &Path) {
    use std::io::Read;
    let lease = directory_lease(path).unwrap().unwrap();
    println!("leased");
    std::io::stdout().flush().unwrap();
    let mut rest = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut rest);
    drop(lease);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        path::PathBuf,
        process::{Command, Stdio},
        sync::atomic::{AtomicBool, Ordering},
    };

    const HELPER: &str = "BARELINE_POSIX_LEASE_HELPER";

    struct Fixture(PathBuf);
    impl Fixture {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("bareline-cache-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            Self(base)
        }
        fn tree(&self) -> (PathBuf, PathBuf) {
            let root = self.0.join("Bareline-owned-spill");
            let candidate = root.join(format!("owned-stream-{}-1", std::process::id()));
            std::fs::create_dir_all(&candidate).unwrap();
            std::fs::write(candidate.join(RECORD), b"owner-proof").unwrap();
            (root, candidate)
        }
        fn ids(root: &Path, candidate: &Path) -> (CacheDirectoryIdentity, CacheDirectoryIdentity) {
            (
                directory_lease(root).unwrap().unwrap().identity,
                directory_lease(candidate).unwrap().unwrap().identity,
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn purge(root: &Path, candidate: &Path, proof: &str, bytes: &[u8]) -> CacheRemovalOutcome {
        let (root_id, candidate_id) = Fixture::ids(root, candidate);
        remove(
            root,
            candidate,
            root_id,
            candidate_id,
            proof,
            bytes,
            16,
            Duration::from_secs(60),
            &|| false,
        )
    }

    #[test]
    fn lease_is_none_for_links_files_and_missing_names() {
        let fixture = Fixture::new("plain");
        let folder = fixture.0.join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(fixture.0.join("file"), b"x").unwrap();
        std::os::unix::fs::symlink(&folder, fixture.0.join("link")).unwrap();
        assert!(directory_lease(&folder).unwrap().is_some());
        assert!(directory_lease(&fixture.0.join("file")).unwrap().is_none());
        assert!(directory_lease(&fixture.0.join("link")).unwrap().is_none());
        assert!(directory_lease(&fixture.0.join("missing")).unwrap().is_none());
        assert!(directory_lease(&fixture.0.join("missing/child")).unwrap().is_none());
    }

    #[test]
    fn nested_purge_removes_everything() {
        let fixture = Fixture::new("nested");
        let (root, candidate) = fixture.tree();
        std::fs::create_dir(candidate.join("nested")).unwrap();
        std::fs::write(candidate.join("nested/payload"), b"owned").unwrap();
        let outcome = purge(&root, &candidate, RECORD, b"owner-proof");
        assert_eq!(outcome.result.unwrap(), 4);
        assert!(!candidate.exists());
        assert!(root.is_dir());
    }

    #[test]
    fn nested_link_is_refused_and_external_sentinel_survives() {
        let fixture = Fixture::new("link");
        let (root, candidate) = fixture.tree();
        let external = fixture.0.join("external");
        std::fs::create_dir(&external).unwrap();
        std::fs::write(external.join("sentinel"), b"foreign").unwrap();
        std::os::unix::fs::symlink(&external, candidate.join("escape")).unwrap();
        assert!(purge(&root, &candidate, RECORD, b"owner-proof").result.is_err());
        assert_eq!(std::fs::read(external.join("sentinel")).unwrap(), b"foreign");
        assert!(candidate.join(RECORD).is_file());
    }

    #[test]
    fn changed_proof_and_replaced_root_are_refused() {
        let fixture = Fixture::new("proof");
        let (root, candidate) = fixture.tree();
        assert!(purge(&root, &candidate, RECORD, b"other-proof").result.is_err());
        assert!(candidate.join(RECORD).is_file());

        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        std::fs::rename(&root, fixture.0.join("original-root")).unwrap();
        let replacement = root.join(candidate.file_name().unwrap());
        std::fs::create_dir_all(&replacement).unwrap();
        std::fs::write(replacement.join(RECORD), b"owner-proof").unwrap();
        let outcome = remove(
            &root,
            &replacement,
            root_id,
            candidate_id,
            RECORD,
            b"owner-proof",
            16,
            Duration::from_secs(60),
            &|| false,
        );
        assert!(outcome.result.is_err());
        assert!(replacement.join(RECORD).is_file());
    }

    #[test]
    fn swapped_candidate_is_refused_before_its_proof_is_removed() {
        let fixture = Fixture::new("swap");
        let (root, candidate) = fixture.tree();
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        let original = root.join("original-candidate");
        let swapped = AtomicBool::new(false);
        let outcome = remove_with_hook(
            &root,
            &candidate,
            (root_id, candidate_id),
            (RECORD, b"owner-proof"),
            (16, Duration::from_secs(60)),
            &|| false,
            &|path| {
                if !swapped.swap(true, Ordering::AcqRel) {
                    std::fs::rename(path, &original)?;
                    std::fs::create_dir(path)?;
                    std::fs::write(path.join("replacement-sentinel"), b"foreign")?;
                }
                Ok(())
            },
        );
        assert!(outcome.result.is_err());
        assert_eq!(
            std::fs::read(candidate.join("replacement-sentinel")).unwrap(),
            b"foreign"
        );
        assert!(original.join(RECORD).is_file());
    }

    #[test]
    fn failed_final_removal_restores_the_authority() {
        let fixture = Fixture::new("restore");
        let (root, candidate) = fixture.tree();
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        let outcome = remove_with_hook(
            &root,
            &candidate,
            (root_id, candidate_id),
            (RECORD, b"owner-proof"),
            (16, Duration::from_secs(60)),
            &|| false,
            // A late entry makes the final rmdir fail with ENOTEMPTY.
            &|path| std::fs::write(path.join("late"), b"late"),
        );
        assert!(outcome.result.is_err());
        assert!(outcome.retry_authority_retained);
        assert_eq!(std::fs::read(candidate.join(RECORD)).unwrap(), b"owner-proof");
    }

    #[test]
    fn budget_and_cancellation_stop_the_walk() {
        let fixture = Fixture::new("budget");
        let (root, candidate) = fixture.tree();
        for index in 0..8 {
            std::fs::write(candidate.join(format!("f{index}")), b"x").unwrap();
        }
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        let limited = remove(
            &root,
            &candidate,
            root_id,
            candidate_id,
            RECORD,
            b"owner-proof",
            4,
            Duration::from_secs(60),
            &|| false,
        );
        assert_eq!(limited.result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        let cancelled = remove(
            &root,
            &candidate,
            root_id,
            candidate_id,
            RECORD,
            b"owner-proof",
            64,
            Duration::from_secs(60),
            &|| true,
        );
        assert_eq!(cancelled.result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(candidate.join(RECORD).is_file());
    }

    /// Re-entered in a child process by `lease_excludes_removal_across_processes`.
    #[test]
    fn lease_helper_child() {
        if let Some(path) = std::env::var_os(HELPER) {
            hold_lease_for_parent(Path::new(&path));
        }
    }

    #[test]
    fn lease_excludes_removal_across_processes() {
        let fixture = Fixture::new("cross-process");
        let (root, candidate) = fixture.tree();
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cache::tests::lease_helper_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(HELPER, &candidate)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.contains("leased") {
            line.clear();
            assert!(stdout.read_line(&mut line).unwrap() > 0, "lease helper exited early");
        }
        let blocked = remove(
            &root,
            &candidate,
            root_id,
            candidate_id,
            RECORD,
            b"owner-proof",
            16,
            Duration::from_secs(60),
            &|| false,
        );
        let leased_elsewhere = blocked.result.as_ref().map_err(io::Error::kind).err();
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        if leased_elsewhere != Some(io::ErrorKind::WouldBlock) && blocked.result.is_ok() {
            // Only a filesystem without flock support can get here.
            eprintln!("skipped: this filesystem does not support advisory folder locks");
            return;
        }
        assert_eq!(leased_elsewhere, Some(io::ErrorKind::WouldBlock));
        assert!(candidate.join(RECORD).is_file());
        let removed = remove(
            &root,
            &candidate,
            root_id,
            candidate_id,
            RECORD,
            b"owner-proof",
            16,
            Duration::from_secs(60),
            &|| false,
        );
        assert_eq!(removed.result.unwrap(), 2);
        assert!(!candidate.exists());
    }

    #[test]
    fn migration_lease_is_exclusive_and_publishes_files_and_folders() {
        let fixture = Fixture::new("migration");
        for directory in [false, true] {
            let source = fixture.0.join(format!("source-{directory}"));
            let target = fixture.0.join(format!("target-{directory}"));
            if directory {
                std::fs::create_dir(&source).unwrap();
                std::fs::write(source.join("nested.bin"), b"nested bytes").unwrap();
            } else {
                std::fs::write(&source, b"file bytes").unwrap();
            }
            let lease = migration_lease(&source).unwrap();
            let second = migration_lease(&source).err().unwrap();
            assert_eq!(second.kind(), io::ErrorKind::WouldBlock);
            if !directory {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut open_migration_read(&lease).unwrap(), &mut bytes).unwrap();
                assert_eq!(bytes, b"file bytes");
            }
            let publish = lease.migration_publisher.clone().unwrap();
            std::fs::write(&target, b"occupied").unwrap();
            assert_eq!(publish(&target).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
            std::fs::remove_file(&target).unwrap();
            publish(&target).unwrap();
            assert!(!source.exists());
            if directory {
                assert_eq!(std::fs::read(target.join("nested.bin")).unwrap(), b"nested bytes");
            } else {
                assert_eq!(std::fs::read(&target).unwrap(), b"file bytes");
            }
        }
        let link = fixture.0.join("link");
        std::os::unix::fs::symlink(fixture.0.join("target-true"), &link).unwrap();
        assert!(migration_lease(&link).is_err());
    }
}
