// SPDX-License-Identifier: MPL-2.0
//! Document-save transactions (FC-04, FIO-07/10/17), mirroring the Windows
//! adapter's protocol: a `.bareline-save-<generation>` folder beside the target
//! holds an immutable manifest, the editor version, state records and, after a
//! replacement, the displaced version. Restart discovery reads only no-follow
//! regular files inside a verified folder, and cleanup unlinks names relative to
//! the retained folder descriptor after checking each one still holds the
//! object the receipt recorded.
//!
//! Replacement swaps the stage onto the name with one exchange rename where the
//! filesystem implements it, so readers never see the name missing and the
//! displaced file keeps its identity; elsewhere the displaced file is moved
//! aside first and the stage published with a no-replace rename. A linked target
//! is rewritten in place after its exact bytes are retained. So is a target whose
//! folder accepts no new entries; its stage, and so its transaction folder, then
//! live in a private folder of the profile instead.
use crate::{
    cache, capability, paths, resolve,
    sys::{self, CREATE, DIRECTORY, Node, READ, changed, denied, invalid_data},
    trust::DirectoryGuard,
};
use bareline_platform::{
    CleanupResponsibility, CommitCancellation, CommitCleanup, CommitMode, CommitReceipt, CommitRecovery, CommitState,
    LocalFileSystem, PreparedCommit, PreservedFile, SaveStrategy,
};
use rustix::fs::{AtFlags, FileType, FlockOperation, Mode, OFlags, RenameFlags};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Seek, SeekFrom, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::Mutex,
};

const MANIFEST_MAGIC: &[u8] = b"bareline-save-manifest-posix-v1\0";
const MANIFEST_LIMIT: u64 = 128 * 1024;
const MANIFEST: &str = "manifest";
const AUTHORITY: &str = "cleanup-authority";
const EDITOR: &str = "editor-version";
const DISPLACED: &str = "displaced-version";
const STATE: &str = "state";
const PREFIX: &str = ".bareline-save-";

/// The folder chosen with [`set_locked_folder_stage`], if any.
static LOCKED_FOLDER_STAGE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Keep stages and transactions of saves into folders that accept no new
/// entries in `folder` instead of the profile's recovery folder.
pub(crate) fn set_locked_folder_stage(folder: PathBuf) {
    *LOCKED_FOLDER_STAGE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(folder);
}

/// Where a save into a folder that accepts no new entries keeps its stage and
/// transaction: `in-place-saves` in the profile's recovery folder (or the folder
/// [`set_locked_folder_stage`] chose), created private on demand. `None` without
/// a usable profile.
pub(crate) fn locked_folder_stage() -> Option<PathBuf> {
    let chosen = LOCKED_FOLDER_STAGE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let folder = chosen.or_else(|| {
        paths::AppDirectories::for_current_process(paths::APPLICATION)
            .ok()
            .map(|folders| folders.data.join("recovery").join("in-place-saves"))
    })?;
    if !folder.is_absolute() {
        return None;
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .ok()?;
    Some(folder)
}

#[cfg(test)]
pub(crate) static FAIL_CLEANUP_BEFORE_MANIFEST: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

fn state_name(state: CommitState) -> OsString {
    bareline_platform::commit_state_path(Path::new(STATE), state).into_os_string()
}

fn manifest_bytes(generation: u128, target: &Path, mode: CommitMode) -> io::Result<Vec<u8>> {
    let target = target.as_os_str().as_bytes();
    let length = u32::try_from(target.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "save target path is too long"))?;
    let mut bytes = Vec::with_capacity(MANIFEST_MAGIC.len() + 21 + target.len());
    bytes.extend_from_slice(MANIFEST_MAGIC);
    bytes.extend_from_slice(&generation.to_le_bytes());
    bytes.push(match mode {
        CommitMode::CreateNew => 0,
        CommitMode::Replace => 1,
    });
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(target);
    Ok(bytes)
}

fn parse_manifest(bytes: &[u8], expected_generation: u128) -> io::Result<(PathBuf, CommitMode)> {
    let header = MANIFEST_MAGIC.len();
    if bytes.len() < header + 21 || &bytes[..header] != MANIFEST_MAGIC {
        return Err(invalid_data("invalid save transaction manifest"));
    }
    let mut generation = [0; 16];
    generation.copy_from_slice(&bytes[header..header + 16]);
    if u128::from_le_bytes(generation) != expected_generation {
        return Err(invalid_data("save transaction generation mismatch"));
    }
    let mode = match bytes[header + 16] {
        0 => CommitMode::CreateNew,
        1 => CommitMode::Replace,
        _ => return Err(invalid_data("invalid save transaction mode")),
    };
    let mut length = [0; 4];
    length.copy_from_slice(&bytes[header + 17..header + 21]);
    let start = header + 21;
    if u32::from_le_bytes(length) as usize != bytes.len() - start {
        return Err(invalid_data("invalid save target path length"));
    }
    Ok((OsString::from_vec(bytes[start..].to_vec()).into(), mode))
}

fn parse_generation(name: &OsStr) -> Option<u128> {
    let generation = name.to_str()?.strip_prefix(PREFIX)?;
    (generation.len() == 32)
        .then(|| u128::from_str_radix(generation, 16).ok())
        .flatten()
}

/// A regular file below `directory`, never through a link or into a FIFO.
fn open_regular(directory: &File, name: impl AsRef<OsStr>) -> io::Result<File> {
    let file = sys::open_at(directory, name, READ).map_err(sys::no_follow)?;
    if !file.metadata()?.is_file() {
        return Err(denied("transaction entry is not a regular file"));
    }
    Ok(file)
}

fn read_manifest(directory: &File, name: &str, generation: u128) -> io::Result<(PathBuf, CommitMode)> {
    parse_manifest(
        &sys::read_bounded(&mut open_regular(directory, name)?, MANIFEST_LIMIT)?,
        generation,
    )
}

fn write_record(directory: &File, name: &str, bytes: &[u8]) -> io::Result<()> {
    let mut file = sys::open_at(directory, name, CREATE)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn unlink_present(directory: &File, name: impl AsRef<OsStr>) -> io::Result<()> {
    match rustix::fs::unlinkat(directory, name.as_ref(), AtFlags::empty()) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(errno) => Err(errno.into()),
    }
}

/// Unlink `name` only while it still holds the recorded object.
fn remove_verified(directory: &File, name: &str, node: Node) -> io::Result<()> {
    match sys::stat_name(directory, OsStr::new(name)) {
        Ok(stat) if stat.node == node => Ok(rustix::fs::unlinkat(directory, name, AtFlags::empty())?),
        Ok(_) => Err(changed()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// The newest valid record wins; an invalid or conflicting record makes the
/// state unverified, as on Windows.
fn state_from_records(directory: &File) -> CommitState {
    let records = [
        (
            CommitState::CleanupPending,
            b"bareline-save-v1\ncleanup-pending\n".as_slice(),
        ),
        (CommitState::Conflict, b"bareline-save-v1\nconflict\n".as_slice()),
        (
            CommitState::Replaced,
            b"bareline-save-v1\ncommitted-unacknowledged\n".as_slice(),
        ),
        (CommitState::Created, b"bareline-save-v1\ncreated\n".as_slice()),
        (CommitState::Precommit, b"bareline-save-v1\nprecommit\n".as_slice()),
    ];
    let mut saw_invalid = false;
    for (state, expected) in records {
        match open_regular(directory, state_name(state)) {
            Ok(mut file) => match sys::read_bounded(&mut file, 128) {
                Ok(bytes) if bytes == expected && !saw_invalid => return state,
                Ok(bytes) if bytes == expected => return CommitState::Unverified,
                _ => saw_invalid = true,
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => saw_invalid = true,
        }
    }
    CommitState::Unverified
}

/// The transaction folder as created: its descriptor (holding a shared lease)
/// and the object its name must keep naming.
struct Transaction {
    directory: File,
    name: OsString,
    node: Node,
    generation: u128,
}
impl Transaction {
    fn still_named(&self, parent: &DirectoryGuard) -> io::Result<()> {
        if sys::stat_name(parent, &self.name)?.node == self.node {
            Ok(())
        } else {
            Err(changed())
        }
    }
    fn remove(&self, parent: &DirectoryGuard) -> io::Result<()> {
        self.still_named(parent)?;
        rustix::fs::unlinkat(parent, &self.name, AtFlags::REMOVEDIR)?;
        sys::sync_directory(&parent.directory)
    }
}

pub(crate) struct Prepared {
    parent: DirectoryGuard,
    /// The folder holding the transaction when it is not `parent`.
    store: Option<DirectoryGuard>,
    transaction: Transaction,
    proposed: File,
    strategy: SaveStrategy,
}

fn create_transaction(parent: &DirectoryGuard) -> io::Result<Transaction> {
    loop {
        let generation = u128::from_le_bytes(sys::random_bytes()?);
        let name = OsString::from(format!("{PREFIX}{generation:032x}"));
        match rustix::fs::mkdirat(parent, &name, Mode::from_bits_truncate(0o700)) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => continue,
            Err(errno) => return Err(errno.into()),
        }
        let opened = sys::open_at(parent, &name, DIRECTORY).and_then(|directory| {
            cache::lock(&directory, FlockOperation::NonBlockingLockShared)?;
            let node = Node::of(&directory)?;
            Ok(Transaction {
                directory,
                name: name.clone(),
                node,
                generation,
            })
        });
        if opened.is_err() {
            let _ = rustix::fs::unlinkat(parent, &name, AtFlags::REMOVEDIR);
        }
        return opened;
    }
}

pub(crate) fn prepare(
    staged: &Path,
    target: &Path,
    mode: CommitMode,
    cancellation: &dyn CommitCancellation,
) -> io::Result<PreparedCommit> {
    let apart = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "save stage and destination must share a directory",
        )
    };
    if target.parent().is_none() || staged.parent().is_none() {
        return Err(apart());
    }
    // Resolve links once so the transaction pins the physical location instead
    // of re-traversing a link that could be retargeted meanwhile.
    let resolved = resolve::resolve(target)?;
    let strategy = capability::report_resolved(&resolved)?.save;
    if strategy == SaveStrategy::CopyOnly {
        return Err(denied("saving to this location is unavailable; use Save Copy"));
    }
    // A folder that accepts no new entries holds neither the stage nor the
    // transaction: both live in the folder the stage was created in.
    let locked = strategy == SaveStrategy::InPlaceLockedFolder;
    if !locked && staged.parent() != target.parent() {
        return Err(apart());
    }
    let target = resolved.path;
    let staged = resolve::resolve(staged)?.path;
    let (parent_path, _) = sys::split(&target)?;
    if locked && mode == CommitMode::CreateNew {
        return Err(denied(
            "this folder is not writable, so no new file can be created in it",
        ));
    }
    // An in-place rewrite may reach a symbolic link's target in another folder.
    if !matches!(strategy, SaveStrategy::InPlace | SaveStrategy::InPlaceLockedFolder)
        && staged.parent() != Some(parent_path)
    {
        return Err(apart());
    }
    let parent = DirectoryGuard::open_resolved(parent_path)?;
    let store = match staged.parent() {
        Some(folder) if locked => Some(DirectoryGuard::open_resolved(folder)?),
        _ => None,
    };
    let home = store.as_ref().unwrap_or(&parent);
    let transaction = create_transaction(home)?;
    let directory_path = home.path().join(&transaction.name);
    let journal = directory_path.join(STATE);
    let prepared = (|| {
        write_record(
            &transaction.directory,
            MANIFEST,
            &manifest_bytes(transaction.generation, &target, mode)?,
        )?;
        let mut proposed = sys::open_at(&transaction.directory, EDITOR, CREATE)?;
        // POSIX cannot keep other writers off the copy, so its hash is not
        // offered as proof (`proposed_sha256`); the save reads it again instead.
        bareline_platform::copy_commit_bytes_into(&staged, &mut proposed, cancellation, &mut |_: &[u8]| {})?;
        bareline_platform::publish_commit_state(&journal, CommitState::Precommit)?;
        sys::sync_directory(&transaction.directory)?;
        sys::sync_directory(&parent.directory)?;
        Ok::<_, io::Error>(proposed)
    })();
    let proposed = match prepared {
        Ok(proposed) => proposed,
        Err(error) => {
            for name in [
                OsString::from(EDITOR),
                OsString::from(MANIFEST),
                state_name(CommitState::Precommit),
            ] {
                let _ = unlink_present(&transaction.directory, name);
            }
            let _ = transaction.remove(home);
            return Err(error);
        }
    };
    Ok(PreparedCommit {
        staged,
        target,
        mode,
        displaced_path: (mode == CommitMode::Replace).then(|| directory_path.join(DISPLACED)),
        proposed_path: Some(directory_path.join(EDITOR)),
        journal_path: Some(journal),
        guard: Some(Box::new(Prepared {
            parent,
            store,
            transaction,
            proposed,
            strategy,
        })),
        proposed_sha256: None,
    })
}

fn prepared_guards(transaction: PreparedCommit) -> io::Result<(PreparedCommit, Prepared)> {
    let mut transaction = transaction;
    let guards = transaction
        .guard
        .take()
        .ok_or_else(|| invalid_data("commit guards missing"))?
        .downcast::<Prepared>()
        .map_err(|_| invalid_data("commit guards have wrong platform type"))?;
    Ok((transaction, *guards))
}

/// Discard a prepared transaction: the target was never touched and the caller
/// still owns its stage. Only entries `prepare` created are removed.
pub(crate) fn abort(transaction: PreparedCommit) -> io::Result<()> {
    let (_, prepared) = prepared_guards(transaction)?;
    let Prepared {
        parent,
        store,
        transaction,
        proposed,
        strategy: _,
    } = prepared;
    remove_verified(&transaction.directory, EDITOR, Node::of(&proposed)?)?;
    drop(proposed);
    for state in [CommitState::Precommit, CommitState::Conflict] {
        unlink_present(&transaction.directory, state_name(state))?;
    }
    unlink_present(&transaction.directory, MANIFEST)?;
    transaction.remove(store.as_ref().unwrap_or(&parent))
}

/// Give the stage the displaced file's permissions and, where allowed, owner,
/// so a private file stays private after the replacement. Set-id bits survive
/// only when the owner does.
fn carry_metadata(parent: &DirectoryGuard, stage: &OsStr, target: &OsStr) -> io::Result<()> {
    let target = std::fs::symlink_metadata(parent.path.join(target))?;
    let stage = sys::open_at(parent, stage, READ).map_err(sys::no_follow)?;
    let current = stage.metadata()?;
    let owned = (current.uid(), current.gid()) == (target.uid(), target.gid())
        || std::os::unix::fs::fchown(&stage, Some(target.uid()), Some(target.gid())).is_ok();
    if !owned {
        // Keep at least the group where this user may set it.
        let _ = std::os::unix::fs::fchown(&stage, None, Some(target.gid()));
    }
    let mut mode = target.mode() & 0o7777;
    if !owned {
        mode &= !0o6000;
    }
    stage.set_permissions(std::fs::Permissions::from_mode(mode))
}

/// Move the displaced version into the transaction, then publish the stage with
/// a no-replace rename. A file created on the name in between is a conflict that
/// keeps every version; the approved version is put back when the name is free.
fn replace_by_rename(parent: &DirectoryGuard, stage: &OsStr, target: &OsStr, transaction: &File) -> io::Result<()> {
    sys::rename_no_replace(parent, target, transaction, OsStr::new(DISPLACED))?;
    if let Err(error) = sys::rename_no_replace(parent, stage, parent, target) {
        let _ = sys::rename_no_replace(transaction, OsStr::new(DISPLACED), parent, target);
        return Err(error);
    }
    Ok(())
}

/// Swap the stage onto the name in one operation; the stage's name then holds
/// exactly the version that was displaced, which moves into the transaction.
fn replace_by_exchange(
    parent: &DirectoryGuard,
    stage: &OsStr,
    target: &OsStr,
    transaction: &File,
) -> io::Result<SaveStrategy> {
    if let Err(errno) = rustix::fs::renameat_with(parent, stage, parent, target, RenameFlags::EXCHANGE) {
        let error = io::Error::from(errno);
        if !sys::flag_unsupported(&error) {
            return Err(error);
        }
        replace_by_rename(parent, stage, target, transaction)?;
        return Ok(SaveStrategy::RenameReplace);
    }
    if let Err(error) = sys::rename_no_replace(parent, stage, transaction, OsStr::new(DISPLACED)) {
        // Swap back so a failed save leaves the approved version in place.
        let _ = rustix::fs::renameat_with(parent, stage, parent, target, RenameFlags::EXCHANGE);
        return Err(error);
    }
    Ok(SaveStrategy::Transactional)
}

/// Rewrite the existing file object so hard and symbolic links keep pointing at
/// it. The exact prior bytes are retained first, and a failed rewrite restores
/// them.
fn rewrite_in_place(staged: &Path, parent: &DirectoryGuard, target: &OsStr, transaction: &File) -> io::Result<()> {
    let mut file = sys::open_at(
        parent,
        target,
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
    )
    .map_err(sys::no_follow)?;
    if !file.metadata()?.is_file() {
        return Err(denied("only regular files can be rewritten in place"));
    }
    let mut previous = sys::open_at(transaction, DISPLACED, CREATE)?;
    io::copy(&mut file, &mut previous)?;
    previous.sync_all()?;
    drop(previous);
    let rewrite = |file: &mut File, source: &mut File| -> io::Result<()> {
        file.seek(SeekFrom::Start(0))?;
        let length = io::copy(source, file)?;
        file.set_len(length)?;
        file.sync_all()
    };
    if let Err(error) = File::open(staged).and_then(|mut source| rewrite(&mut file, &mut source)) {
        return match open_regular(transaction, DISPLACED).and_then(|mut previous| rewrite(&mut file, &mut previous)) {
            Ok(()) => Err(error),
            Err(restore) => Err(io::Error::new(
                error.kind(),
                format!("{error}; restoring the previous version also failed: {restore}"),
            )),
        };
    }
    Ok(())
}

pub(crate) fn commit(file_system: &dyn LocalFileSystem, transaction: PreparedCommit) -> io::Result<CommitReceipt> {
    let (transaction, prepared) = prepared_guards(transaction)?;
    let PreparedCommit {
        staged,
        target,
        mode,
        displaced_path,
        proposed_path,
        journal_path,
        ..
    } = transaction;
    let Prepared {
        parent,
        store,
        transaction,
        proposed,
        strategy,
    } = prepared;
    let journal = journal_path.ok_or_else(|| invalid_data("commit journal missing"))?;
    transaction.still_named(store.as_ref().unwrap_or(&parent))?;
    let (_, target_name) = sys::split(&target)?;
    let (stage_parent, stage_name) = sys::split(&staged)?;
    let published = match (mode, strategy) {
        (CommitMode::Replace, SaveStrategy::InPlace | SaveStrategy::InPlaceLockedFolder) => {
            rewrite_in_place(&staged, &parent, target_name, &transaction.directory).map(|()| strategy)
        }
        (CommitMode::Replace, _) => carry_metadata(&parent, stage_name, target_name).and_then(|()| {
            if strategy == SaveStrategy::Transactional {
                replace_by_exchange(&parent, stage_name, target_name, &transaction.directory)
            } else {
                replace_by_rename(&parent, stage_name, target_name, &transaction.directory)
                    .map(|()| SaveStrategy::RenameReplace)
            }
        }),
        (CommitMode::CreateNew, _) if stage_parent == parent.path() => {
            sys::rename_no_replace(&parent, stage_name, &parent, target_name).map(|()| strategy)
        }
        // A dangling final link: the new file is created at the link's target.
        (CommitMode::CreateNew, _) => DirectoryGuard::open_resolved(stage_parent)
            .and_then(|stage_folder| sys::rename_no_replace(&stage_folder, stage_name, &parent, target_name))
            .map(|()| strategy),
    };
    let strategy = match published {
        Ok(strategy) => strategy,
        Err(error) => {
            if mode == CommitMode::CreateNew && error.kind() == io::ErrorKind::AlreadyExists {
                let _ = bareline_platform::publish_commit_state(&journal, CommitState::Conflict);
            }
            return Err(error);
        }
    };
    sys::sync_directory(&parent.directory)?;
    sys::sync_directory(&transaction.directory)?;
    transaction.still_named(store.as_ref().unwrap_or(&parent))?;
    let target_identity = file_system.identity(&open_regular(&parent.directory, target_name)?)?;
    let mut artifacts = Vec::new();
    let displaced = displaced_path
        .map(|path| -> io::Result<PreservedFile> {
            let file = open_regular(&transaction.directory, DISPLACED)?;
            artifacts.push((DISPLACED, Node::of(&file)?));
            Ok(PreservedFile {
                path,
                identity: file_system.identity(&file)?,
            })
        })
        .transpose()?;
    let proposed = proposed_path
        .map(|path| -> io::Result<PreservedFile> {
            artifacts.push((EDITOR, Node::of(&proposed)?));
            Ok(PreservedFile {
                path,
                identity: file_system.identity(&proposed)?,
            })
        })
        .transpose()?;
    let state = if mode == CommitMode::Replace {
        CommitState::Replaced
    } else {
        CommitState::Created
    };
    let receipt = CommitReceipt {
        target: target_identity,
        displaced,
        proposed,
        journal: Some(journal.clone()),
        state,
        cleanup: CleanupResponsibility::Caller,
        cleanup_token: Some(Box::new(Cleanup {
            parent: store.unwrap_or(parent),
            transaction,
            artifacts,
            journal,
            authority: false,
        })),
        strategy,
    };
    file_system.mark_commit_state(&receipt, state)?;
    Ok(receipt)
}

struct Cleanup {
    parent: DirectoryGuard,
    transaction: Transaction,
    artifacts: Vec<(&'static str, Node)>,
    /// Names the transaction for test fault injection.
    #[cfg_attr(not(test), allow(dead_code))]
    journal: PathBuf,
    authority: bool,
}
impl CommitCleanup for Cleanup {
    fn publish_cleanup_authority(&mut self) -> io::Result<()> {
        if self.authority {
            return Ok(());
        }
        let directory = &self.transaction.directory;
        let bytes = sys::read_bounded(&mut open_regular(directory, MANIFEST)?, MANIFEST_LIMIT)?;
        parse_manifest(&bytes, self.transaction.generation)?;
        match write_record(directory, AUTHORITY, &bytes) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if sys::read_bounded(&mut open_regular(directory, AUTHORITY)?, MANIFEST_LIMIT)? != bytes {
                    return Err(invalid_data("cleanup authority changed"));
                }
            }
            Err(error) => return Err(error),
        }
        sys::sync_directory(directory)?;
        self.authority = true;
        Ok(())
    }

    fn cleanup(&mut self) -> io::Result<()> {
        self.transaction.still_named(&self.parent)?;
        let directory = &self.transaction.directory;
        while let Some((name, node)) = self.artifacts.last() {
            remove_verified(directory, name, *node)?;
            self.artifacts.pop();
        }
        for state in [
            CommitState::Precommit,
            CommitState::Created,
            CommitState::Replaced,
            CommitState::Conflict,
        ] {
            unlink_present(directory, state_name(state))?;
        }
        #[cfg(test)]
        {
            let mut fail = FAIL_CLEANUP_BEFORE_MANIFEST
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if fail.as_ref() == Some(&self.journal) {
                *fail = None;
                return Err(io::Error::other("injected cleanup interruption after state retirement"));
            }
        }
        unlink_present(directory, MANIFEST)?;
        unlink_present(directory, state_name(CommitState::CleanupPending))?;
        unlink_present(directory, AUTHORITY)?;
        self.authority = false;
        self.transaction.remove(&self.parent)
    }
}

/// The durable cleanup authority must exist before cleanup is announced.
pub(crate) fn mark_state(receipt: &CommitReceipt, state: CommitState) -> io::Result<()> {
    let Some(journal) = &receipt.journal else {
        return Ok(());
    };
    if state == CommitState::CleanupPending {
        let directory = journal
            .parent()
            .ok_or_else(|| invalid_data("cleanup journal has no directory"))?;
        let generation = directory
            .file_name()
            .and_then(parse_generation)
            .ok_or_else(|| invalid_data("cleanup generation is invalid"))?;
        let (parent, name) = sys::split(directory)?;
        let parent = DirectoryGuard::open_resolved(parent)?;
        let folder = sys::open_at(&parent, name, DIRECTORY).map_err(sys::no_follow)?;
        read_manifest(&folder, AUTHORITY, generation)?;
    }
    bareline_platform::publish_commit_state(journal, state)
}

fn unverified(directory: &Path) -> CommitRecovery {
    CommitRecovery {
        target: None,
        proposed: directory.join(EDITOR),
        displaced: None,
        journal: directory.join(STATE),
        state: CommitState::Unverified,
        verified: false,
    }
}

fn regular_child(directory: &File, name: &str) -> bool {
    open_regular(directory, name).is_ok()
}

pub(crate) fn inspect(parent: &Path, cancellation: &dyn CommitCancellation) -> io::Result<Vec<CommitRecovery>> {
    // A folder that does not exist holds no interrupted save (APP-21).
    let guard = match resolve::resolve(parent).and_then(|resolved| DirectoryGuard::open_resolved(&resolved.path)) {
        Ok(guard) => guard,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut recoveries = Vec::new();
    for entry in rustix::fs::Dir::read_from(&guard)?.take(4_096) {
        cancellation.check()?;
        let entry = entry?;
        let name = OsStr::from_bytes(entry.file_name().to_bytes()).to_os_string();
        let Some(generation) = parse_generation(&name) else {
            continue;
        };
        let directory_path = guard.path().join(&name);
        // A transaction-shaped link or file is itself an orphan to inspect; its
        // target is never followed to manufacture evidence.
        let opened = sys::stat_name(&guard, &name)
            .and_then(|stat| match stat.kind {
                FileType::Directory => sys::open_at(&guard, &name, DIRECTORY),
                _ => Err(denied("transaction entry is not a folder")),
            })
            .and_then(|directory| {
                cache::lock(&directory, FlockOperation::NonBlockingLockShared)?;
                Ok(directory)
            });
        let Ok(directory) = opened else {
            recoveries.push(unverified(&directory_path));
            continue;
        };
        let journal = directory_path.join(STATE);
        let mut state = state_from_records(&directory);
        let manifest = read_manifest(&directory, MANIFEST, generation);
        let cleanup_authority = read_manifest(&directory, AUTHORITY, generation);
        let durable_cleanup =
            cleanup_authority.is_ok() && matches!(state, CommitState::CleanupPending | CommitState::Unverified);
        if state == CommitState::Unverified && durable_cleanup {
            state = CommitState::CleanupPending;
        }
        let (target, mode) = match manifest.or(cleanup_authority) {
            Ok((target, mode)) if target.is_absolute() && target.parent() == Some(guard.path()) => {
                (Some(target), Some(mode))
            }
            _ => (None, None),
        };
        let proposed_regular = regular_child(&directory, EDITOR);
        let displaced_regular = regular_child(&directory, DISPLACED);
        let displaced =
            (mode == Some(CommitMode::Replace) && displaced_regular).then(|| directory_path.join(DISPLACED));
        if target.is_none() && !proposed_regular && !displaced_regular && state == CommitState::Unverified {
            continue;
        }
        let verified = target.is_some()
            && state != CommitState::Unverified
            && (proposed_regular || state == CommitState::CleanupPending);
        recoveries.push(CommitRecovery {
            target,
            proposed: directory_path.join(EDITOR),
            displaced,
            journal,
            state: if verified { state } else { CommitState::Unverified },
            verified,
        });
    }
    Ok(recoveries)
}

/// Only these names, a regular editor version among them, are in `directory`.
/// The listing stops at the first other name, so it stays bounded.
fn holds_only(directory: &File, names: &[&str]) -> io::Result<bool> {
    for entry in rustix::fs::Dir::read_from(directory)? {
        let entry = entry?;
        let name = OsStr::from_bytes(entry.file_name().to_bytes());
        if name == "." || name == ".." {
            continue;
        }
        if !names.iter().any(|known| name == *known) {
            return Ok(false);
        }
    }
    match open_regular(directory, EDITOR) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Remove the transactions in `parent` that a process left while it was still
/// copying the editor version (LNX-FILE-007): a valid manifest naming a file in
/// `parent`, at most a partial editor version beside it, no state record, and no
/// process holding the folder's lease. Names are unlinked relative to the
/// folder's descriptor, and the folder only while its name still holds it.
pub(crate) fn reclaim(parent: &Path) -> io::Result<Vec<PathBuf>> {
    let guard = match resolve::resolve(parent).and_then(|resolved| DirectoryGuard::open_resolved(&resolved.path)) {
        Ok(guard) => guard,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut removed = Vec::new();
    for entry in rustix::fs::Dir::read_from(&guard)?.take(4_096) {
        let entry = entry?;
        let name = OsStr::from_bytes(entry.file_name().to_bytes()).to_os_string();
        let Some(generation) = parse_generation(&name) else {
            continue;
        };
        if !sys::stat_name(&guard, &name).is_ok_and(|stat| stat.kind == FileType::Directory) {
            continue;
        }
        let Ok(directory) = sys::open_at(&guard, &name, DIRECTORY) else {
            continue;
        };
        // Its creator holds the folder's shared lease before it writes the
        // manifest, so a folder with a manifest and no lease outlived its process.
        if !read_manifest(&directory, MANIFEST, generation)
            .is_ok_and(|(target, _)| target.parent() == Some(guard.path()))
            || rustix::fs::flock(&directory, FlockOperation::NonBlockingLockExclusive).is_err()
            || !holds_only(&directory, &[MANIFEST, EDITOR]).unwrap_or(false)
        {
            continue;
        }
        let transaction = Transaction {
            node: Node::of(&directory)?,
            directory,
            name,
            generation,
        };
        let reclaimed = unlink_present(&transaction.directory, EDITOR)
            .and_then(|()| unlink_present(&transaction.directory, MANIFEST))
            .and_then(|()| transaction.remove(&guard));
        if reclaimed.is_ok() {
            removed.push(guard.path().join(&transaction.name));
        }
    }
    Ok(removed)
}

pub(crate) fn resume(
    file_system: &dyn LocalFileSystem,
    recovery: &CommitRecovery,
) -> io::Result<Option<CommitReceipt>> {
    if !recovery.verified || recovery.state != CommitState::CleanupPending {
        return Ok(None);
    }
    let directory_path = recovery
        .journal
        .parent()
        .ok_or_else(|| invalid_data("cleanup transaction has no directory"))?;
    let (parent_path, name) = sys::split(directory_path)?;
    let generation = parse_generation(name).ok_or_else(|| invalid_data("cleanup generation is invalid"))?;
    let parent = DirectoryGuard::open_resolved(parent_path)?;
    let directory = sys::open_at(&parent, name, DIRECTORY).map_err(sys::no_follow)?;
    cache::lock(&directory, FlockOperation::NonBlockingLockShared)?;
    let transaction = Transaction {
        node: Node::of(&directory)?,
        directory,
        name: name.to_os_string(),
        generation,
    };
    let state = state_from_records(&transaction.directory);
    let (target, mode) = read_manifest(&transaction.directory, AUTHORITY, generation)?;
    if !matches!(state, CommitState::CleanupPending | CommitState::Unverified) {
        return Err(denied("cleanup authority has an incompatible transaction state"));
    }
    if recovery.target.as_ref() != Some(&target) || target.parent() != Some(parent_path) {
        return Err(denied("cleanup target no longer matches its manifest"));
    }
    match read_manifest(&transaction.directory, MANIFEST, generation) {
        Ok(primary) if primary != (target.clone(), mode) => {
            return Err(denied("cleanup manifest and durable authority differ"));
        }
        _ => {}
    }
    let (_, target_name) = sys::split(&target)?;
    let target_identity = file_system.identity(&open_regular(&parent.directory, target_name)?)?;
    let mut artifacts = Vec::new();
    let mut preserved = |name: &'static str, path: &Path| -> io::Result<Option<PreservedFile>> {
        match open_regular(&transaction.directory, name) {
            Ok(file) => {
                artifacts.push((name, Node::of(&file)?));
                Ok(Some(PreservedFile {
                    path: path.to_path_buf(),
                    identity: file_system.identity(&file)?,
                }))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    };
    let proposed = preserved(EDITOR, &recovery.proposed)?;
    let displaced = match &recovery.displaced {
        Some(path) => preserved(DISPLACED, path)?,
        None => None,
    };
    Ok(Some(CommitReceipt {
        target: target_identity,
        displaced,
        proposed,
        journal: Some(recovery.journal.clone()),
        state: CommitState::CleanupPending,
        cleanup: CleanupResponsibility::Caller,
        cleanup_token: Some(Box::new(Cleanup {
            parent,
            transaction,
            artifacts,
            journal: recovery.journal.clone(),
            authority: true,
        })),
        // This receipt carries cleanup authority only; nothing is verified against it.
        strategy: SaveStrategy::Transactional,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips_and_rejects_damage() {
        let target = Path::new("/home/user/notes \u{e9}.txt");
        let bytes = manifest_bytes(7, target, CommitMode::Replace).unwrap();
        assert_eq!(
            parse_manifest(&bytes, 7).unwrap(),
            (target.to_path_buf(), CommitMode::Replace)
        );
        assert!(parse_manifest(&bytes, 8).is_err());
        assert!(parse_manifest(&bytes[..bytes.len() - 1], 7).is_err());
        assert!(parse_manifest(b"partial", 7).is_err());
        assert_eq!(
            parse_generation(OsStr::new(".bareline-save-0000000000000000000000000000000a")),
            Some(10)
        );
        assert_eq!(parse_generation(OsStr::new(".bareline-save-1")), None);
        assert_eq!(state_name(CommitState::Replaced), OsString::from("state.replaced"));
    }

    /// LNX-FILE-007: only a transaction an ended process left while copying its
    /// editor version is reclaimed. A complete editor version, a transaction of a
    /// file in another folder, one a process holds, and foreign entries stay.
    #[test]
    fn reclaim_removes_only_interrupted_transactions_nobody_holds() {
        let folder = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("bareline-posix-reclaim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let target = folder.join("big.log");
        std::fs::write(&target, b"old").unwrap();
        let make = |generation: u128, target: &Path, entries: &[(&str, &str)]| {
            let directory = folder.join(format!("{PREFIX}{generation:032x}"));
            std::fs::create_dir(&directory).unwrap();
            std::fs::write(
                directory.join(MANIFEST),
                manifest_bytes(generation, target, CommitMode::Replace).unwrap(),
            )
            .unwrap();
            for (name, bytes) in entries {
                std::fs::write(directory.join(name), bytes).unwrap();
            }
            directory
        };
        let partial = make(1, &target, &[(EDITOR, "half of the ed")]);
        let complete = make(
            2,
            &target,
            &[(EDITOR, "whole"), ("state.precommit", "bareline-save-v1\nprecommit\n")],
        );
        let foreign = make(3, Path::new("/elsewhere/big.log"), &[(EDITOR, "x")]);
        let held = make(4, &target, &[(EDITOR, "live")]);
        let stranger = make(5, &target, &[(EDITOR, "x"), ("notes", "user file")]);
        let lease = File::open(&held).unwrap();
        cache::lock(&lease, FlockOperation::NonBlockingLockShared).unwrap();

        assert_eq!(reclaim(&folder).unwrap(), vec![partial.clone()]);
        assert!(!partial.exists());
        for kept in [&complete, &foreign, &held, &stranger] {
            assert!(kept.join(EDITOR).exists(), "{}", kept.display());
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        drop(lease);
        assert!(reclaim(&folder.join("absent")).unwrap().is_empty());
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
