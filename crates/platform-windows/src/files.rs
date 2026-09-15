// SPDX-License-Identifier: MPL-2.0
use bareline_platform::{
    CleanupResponsibility, CommitCleanup, CommitMode, CommitReceipt, CommitRecovery, CommitState, FileIdentity,
    LocalFileSystem, PreparedCommit, PreservedFile,
};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    mem::size_of,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
};
use windows::{
    Win32::{Foundation::*, Storage::FileSystem::*, System::Com::CoCreateGuid},
    core::PCWSTR,
};
pub struct WindowsFileSystem;
const MANIFEST_MAGIC: &[u8] = b"bareline-save-manifest-v1\0";
const MANIFEST_LIMIT: u64 = 128 * 1024;
#[cfg(test)]
static FAIL_CLEANUP_BEFORE_MANIFEST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
fn io_error(error: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0 & 0xffff)
}
fn cleanup_handle(path: &Path, allow_write: bool, directory: bool) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(DELETE.0 | FILE_READ_ATTRIBUTES.0)
        .share_mode(FILE_SHARE_READ.0 | if allow_write { FILE_SHARE_WRITE.0 } else { 0 })
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0 | if directory { FILE_FLAG_BACKUP_SEMANTICS.0 } else { 0 })
        .open(path)
}
fn delete_by_handle(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
        .map_err(io_error)
    }
}
fn ordinary_directory_identity(file: &File) -> io::Result<bareline_platform::CacheDirectoryIdentity> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information).map_err(io_error)?;
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
        || information.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0) != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "save transaction is not an ordinary local directory",
        ));
    }
    Ok(bareline_platform::CacheDirectoryIdentity {
        volume: information.dwVolumeSerialNumber as u64,
        file: ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64,
    })
}
fn write_manifest(path: &Path, generation: u128, target: &Path, mode: CommitMode) -> io::Result<()> {
    write_manifest_held(path, generation, target, mode).map(drop)
}
fn write_manifest_held(path: &Path, generation: u128, target: &Path, mode: CommitMode) -> io::Result<File> {
    let target: Vec<u16> = target.as_os_str().encode_wide().collect();
    let target_len = u32::try_from(target.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "save target path is too long"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .access_mode(GENERIC_READ.0 | GENERIC_WRITE.0 | DELETE.0)
        .share_mode(FILE_SHARE_READ.0)
        .create_new(true)
        .open(path)?;
    file.write_all(MANIFEST_MAGIC)?;
    file.write_all(&generation.to_le_bytes())?;
    file.write_all(&[match mode {
        CommitMode::CreateNew => 0,
        CommitMode::Replace => 1,
    }])?;
    file.write_all(&target_len.to_le_bytes())?;
    for unit in target {
        file.write_all(&unit.to_le_bytes())?;
    }
    file.sync_all()?;
    Ok(file)
}
fn open_manifest_cleanup(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(GENERIC_READ.0 | DELETE.0)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}
fn open_manifest_validation(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(GENERIC_READ.0)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}
fn read_manifest(mut file: File, expected_generation: u128) -> io::Result<(PathBuf, CommitMode)> {
    use std::os::windows::ffi::OsStringExt;
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(MANIFEST_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MANIFEST_LIMIT
        || bytes.len() < MANIFEST_MAGIC.len() + 16 + 1 + 4
        || &bytes[..MANIFEST_MAGIC.len()] != MANIFEST_MAGIC
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid save transaction manifest",
        ));
    }
    let mut cursor = MANIFEST_MAGIC.len();
    let generation = u128::from_le_bytes(bytes[cursor..cursor + 16].try_into().unwrap());
    cursor += 16;
    if generation != expected_generation {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "save transaction generation mismatch",
        ));
    }
    let mode = match bytes[cursor] {
        0 => CommitMode::CreateNew,
        1 => CommitMode::Replace,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid save transaction mode",
            ));
        }
    };
    cursor += 1;
    let units = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
    cursor += 4;
    if units > 32_767 || bytes.len() != cursor + units * 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid save target path length",
        ));
    }
    let path = (0..units)
        .map(|index| u16::from_le_bytes([bytes[cursor + index * 2], bytes[cursor + index * 2 + 1]]))
        .collect::<Vec<_>>();
    Ok((std::ffi::OsString::from_wide(&path).into(), mode))
}
fn parse_transaction_generation(name: &std::ffi::OsStr) -> Option<u128> {
    let name = name.to_str()?;
    let generation = name.strip_prefix(".bareline-save-")?;
    (generation.len() == 32)
        .then(|| u128::from_str_radix(generation, 16).ok())
        .flatten()
}
fn state_from_sealed_records(file_system: &WindowsFileSystem, base: &Path) -> CommitState {
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
        match file_system.open_sealed_read(&bareline_platform::commit_state_path(base, state)) {
            Ok(mut file) => {
                let mut bytes = Vec::new();
                match Read::by_ref(&mut file).take(129).read_to_end(&mut bytes) {
                    Ok(_) if bytes == expected && !saw_invalid => return state,
                    Ok(_) if bytes == expected => return CommitState::Unverified,
                    _ => saw_invalid = true,
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => saw_invalid = true,
        }
    }
    CommitState::Unverified
}
struct WindowsCommitCleanup {
    artifacts: Vec<File>,
    authority: Option<File>,
    manifest: Option<File>,
    held: Vec<File>,
    journal: std::path::PathBuf,
    directory: File,
    guards: Vec<std::sync::Arc<dyn Send + Sync>>,
}
struct WindowsPreparedGuards {
    #[allow(dead_code)]
    parent: std::sync::Arc<dyn Send + Sync>,
    transaction: std::sync::Arc<dyn Send + Sync>,
    directory: File,
    manifest: File,
    proposed: File,
}
impl CommitCleanup for WindowsCommitCleanup {
    fn publish_cleanup_authority(&mut self) -> io::Result<()> {
        if self.authority.is_some() {
            return Ok(());
        }
        let directory = self
            .journal
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cleanup journal has no directory"))?;
        let generation = parse_transaction_generation(
            directory
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cleanup generation is missing"))?,
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cleanup generation is invalid"))?;
        let manifest = self
            .manifest
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cleanup manifest is missing"))?;
        let (target, mode) = read_manifest(manifest.try_clone()?, generation)?;
        let authority_path = directory.join("cleanup-authority");
        let authority = match write_manifest_held(&authority_path, generation, &target, mode) {
            Ok(authority) => authority,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let authority = open_manifest_cleanup(&authority_path)?;
                let current = read_manifest(authority.try_clone()?, generation)?;
                if current != (target, mode) {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "cleanup authority changed"));
                }
                authority
            }
            Err(error) => return Err(error),
        };
        self.authority = Some(authority);
        Ok(())
    }

    fn cleanup(&mut self) -> io::Result<()> {
        self.held.clear();
        let mut states = Vec::new();
        for state in [
            CommitState::Precommit,
            CommitState::Created,
            CommitState::Replaced,
            CommitState::Conflict,
            CommitState::CleanupPending,
        ] {
            let path = bareline_platform::commit_state_path(&self.journal, state);
            match cleanup_handle(&path, true, false) {
                Ok(file) => states.push((state, file)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        while let Some(file) = self.artifacts.last() {
            delete_by_handle(file)?;
            self.artifacts.pop();
        }
        for (_, state) in states.iter().filter(|(state, _)| *state != CommitState::CleanupPending) {
            delete_by_handle(state)?;
        }
        #[cfg(test)]
        if FAIL_CLEANUP_BEFORE_MANIFEST.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(io::Error::other("injected cleanup interruption after state retirement"));
        }
        if let Some(manifest) = self.manifest.as_ref() {
            delete_by_handle(manifest)?;
        }
        self.manifest = None;
        if let Some((_, cleanup_state)) = states.iter().find(|(state, _)| *state == CommitState::CleanupPending) {
            delete_by_handle(cleanup_state)?;
        }
        drop(states);
        if let Some(authority) = self.authority.as_ref() {
            delete_by_handle(authority)?;
        }
        self.authority = None;
        delete_by_handle(&self.directory)?;
        self.guards.clear();
        Ok(())
    }
}
impl LocalFileSystem for WindowsFileSystem {
    fn cache_process_identity(&self) -> io::Result<bareline_platform::CacheProcessIdentity> {
        use windows::Win32::{
            Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
            System::Threading::GetCurrentProcess,
        };
        let mut nonce = [0; 16];
        unsafe { BCryptGenRandom(None, &mut nonce, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
            .ok()
            .map_err(io_error)?;
        let created = process_created(unsafe { GetCurrentProcess() })?;
        Ok(bareline_platform::CacheProcessIdentity {
            pid: std::process::id(),
            created,
            nonce,
        })
    }

    fn cache_process_liveness(&self, pid: u32, created: u64) -> bareline_platform::ProcessLiveness {
        use bareline_platform::ProcessLiveness;
        use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let process = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
            Ok(process) => process,
            Err(error) if pid != 0 && (error.code().0 & 0xffff) as u32 == ERROR_INVALID_PARAMETER.0 => {
                return ProcessLiveness::Dead;
            }
            Err(_) => return ProcessLiveness::Unknown,
        };
        let mut code = 0;
        let state = if unsafe { GetExitCodeProcess(process, &mut code) }.is_err() {
            ProcessLiveness::Unknown
        } else if code != STILL_ACTIVE.0 as u32 {
            ProcessLiveness::Dead
        } else {
            match process_created(process) {
                Ok(actual) if actual == created => ProcessLiveness::Alive,
                Ok(_) => ProcessLiveness::Unknown,
                Err(_) => ProcessLiveness::Unknown,
            }
        };
        let _ = unsafe { CloseHandle(process) };
        state
    }

    fn cache_directory_guard(&self, path: &Path) -> io::Result<Option<bareline_platform::CacheDirectoryLease>> {
        let guard = match crate::path_trust::WindowsPathTrustProvider.guard_directory(path) {
            Ok(guard) => guard,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let identity = crate::owned_cache::identity(&guard.file)?;
        Ok(Some(bareline_platform::CacheDirectoryLease {
            path: path.to_path_buf(),
            identity,
            guard: std::sync::Arc::new(guard),
            migration_publisher: None,
        }))
    }

    fn migration_entry_guard(&self, path: &Path) -> io::Result<bareline_platform::CacheDirectoryLease> {
        let guard = std::sync::Arc::new(crate::path_trust::WindowsPathTrustProvider.guard_migration_entry(path)?);
        let identity = crate::owned_cache::identity(&guard.file)?;
        let publisher_guard = guard.clone();
        Ok(bareline_platform::CacheDirectoryLease {
            path: path.to_path_buf(),
            identity,
            guard,
            migration_publisher: Some(std::sync::Arc::new(move |target| {
                crate::owned_cache::rename_guarded(&publisher_guard.file, target)
            })),
        })
    }

    fn publish_migration_entry(&self, target: &Path, lease: bareline_platform::CacheDirectoryLease) -> io::Result<()> {
        let publish = lease
            .migration_publisher
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "migration publication lease unavailable"))?;
        publish(target)
    }

    fn open_migration_read(&self, lease: &bareline_platform::CacheDirectoryLease) -> io::Result<File> {
        use std::os::windows::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_DELETE.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&lease.path)?;
        if crate::owned_cache::identity(&file)? != lease.identity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "migration read identity changed",
            ));
        }
        Ok(file)
    }

    fn remove_owned_cache_directory(
        &self,
        root: &Path,
        candidate: &Path,
        root_identity: bareline_platform::CacheDirectoryIdentity,
        candidate_identity: bareline_platform::CacheDirectoryIdentity,
        proof_name: &str,
        proof_bytes: &[u8],
        max_entries: usize,
        max_time: std::time::Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> bareline_platform::CacheRemovalOutcome {
        crate::owned_cache::remove(
            root,
            candidate,
            root_identity,
            candidate_identity,
            proof_name,
            proof_bytes,
            max_entries,
            max_time,
            cancelled,
        )
    }
    fn scoped_remote_read(
        &self,
        access: bareline_platform::RemoteReadAccess,
    ) -> io::Result<std::sync::Arc<dyn LocalFileSystem>> {
        Ok(std::sync::Arc::new(crate::remote_read::RemoteFileSystem::new(access)))
    }
    fn open_follow_read(&self, path: &Path) -> io::Result<(File, std::sync::Arc<dyn Send + Sync>)> {
        crate::path_trust::WindowsPathTrustProvider.open_follow_read(path)
    }

    fn guard_directory(&self, path: &Path) -> io::Result<std::sync::Arc<dyn Send + Sync>> {
        Ok(std::sync::Arc::new(
            crate::path_trust::WindowsPathTrustProvider.guard_directory(path)?,
        ))
    }

    fn available_space(&self, path: &Path) -> io::Result<u64> {
        let path = wide(path);
        let mut available = 0;
        // SAFETY: NUL-terminated path and output pointer remain valid for this call.
        unsafe {
            GetDiskFreeSpaceExW(PCWSTR(path.as_ptr()), Some(&mut available), None, None).map_err(io_error)?;
        }
        Ok(available)
    }

    fn open_sealed_read(&self, path: &Path) -> io::Result<File> {
        use std::os::windows::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)?;
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the owned file handle and output structure remain valid.
        unsafe {
            GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information).map_err(io_error)?;
        }
        if information.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "sealed source is a reparse/offline file",
            ));
        }
        Ok(file)
    }

    fn create_entry(&self, path: &Path, directory: bool) -> io::Result<()> {
        crate::workspace_files::create(self, path, directory)
    }
    fn rename_entry(&self, source: &Path, target: &Path) -> io::Result<()> {
        crate::workspace_files::rename(self, source, target)
    }
    fn delete_entry(&self, path: &Path) -> io::Result<()> {
        crate::workspace_files::delete(self, path)
    }
    fn identity(&self, file: &File) -> io::Result<FileIdentity> {
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: File owns a valid handle for the entire API call; the API does not take ownership.
        unsafe {
            GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information).map_err(io_error)?;
        }
        Ok(FileIdentity {
            volume: information.dwVolumeSerialNumber as u64,
            file: ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64,
            length: ((information.nFileSizeHigh as u64) << 32) | information.nFileSizeLow as u64,
            modified: ((information.ftLastWriteTime.dwHighDateTime as u64) << 32)
                | information.ftLastWriteTime.dwLowDateTime as u64,
        })
    }
    fn validate_source(&self, path: &Path) -> io::Result<()> {
        self.validate_path(path, false)
    }
    fn validate_target(&self, path: &Path) -> io::Result<()> {
        self.validate_path(path, true)
    }
    fn prepare_commit(
        &self,
        staged: &Path,
        target: &Path,
        mode: CommitMode,
        cancellation: &dyn bareline_platform::CommitCancellation,
    ) -> io::Result<PreparedCommit> {
        let parent = target
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?;
        if staged.parent() != Some(parent) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "save stage and destination must share a directory",
            ));
        }
        let parent_guard = self.guard_directory(parent)?;
        let (directory, generation) = loop {
            let id = unsafe { CoCreateGuid() }.map_err(io_error)?.to_u128();
            let path = parent.join(format!(".bareline-save-{id:032x}"));
            match std::fs::create_dir(&path) {
                Ok(()) => break (path, id),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let transaction_guard = cleanup_handle(&directory, false, true)?;
        let transaction_identity = match ordinary_directory_identity(&transaction_guard) {
            Ok(identity) => identity,
            Err(error) => {
                let _ = delete_by_handle(&transaction_guard);
                return Err(error);
            }
        };
        let transaction_lease = match self.cache_directory_guard(&directory) {
            Ok(Some(lease)) if lease.identity == transaction_identity => lease,
            Ok(_) => {
                let _ = delete_by_handle(&transaction_guard);
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "save transaction directory identity changed",
                ));
            }
            Err(error) => {
                let _ = delete_by_handle(&transaction_guard);
                return Err(error);
            }
        };
        let manifest = directory.join("manifest");
        let proposed = directory.join("editor-version");
        let journal = directory.join("state");
        let prepared = (|| {
            write_manifest(&manifest, generation, target, mode)?;
            let manifest_guard = open_manifest_cleanup(&manifest)?;
            bareline_platform::copy_commit_bytes(staged, &proposed, cancellation)?;
            let proposed_guard = cleanup_handle(&proposed, false, false)?;
            bareline_platform::publish_commit_state(&journal, CommitState::Precommit)?;
            Ok::<_, io::Error>((manifest_guard, proposed_guard))
        })();
        let (manifest_guard, proposed_guard) = match prepared {
            Ok(guards) => guards,
            Err(error) => {
                for path in [
                    proposed.clone(),
                    manifest.clone(),
                    bareline_platform::commit_state_path(&journal, CommitState::Precommit),
                ] {
                    if let Ok(file) = cleanup_handle(&path, true, false) {
                        let _ = delete_by_handle(&file);
                    }
                }
                let _ = delete_by_handle(&transaction_guard);
                return Err(error);
            }
        };
        Ok(PreparedCommit {
            staged: staged.to_path_buf(),
            target: target.to_path_buf(),
            mode,
            displaced_path: (mode == CommitMode::Replace).then(|| directory.join("displaced-version")),
            proposed_path: Some(proposed),
            journal_path: Some(journal),
            guard: Some(Box::new(WindowsPreparedGuards {
                parent: parent_guard,
                transaction: transaction_lease.guard,
                directory: transaction_guard,
                manifest: manifest_guard,
                proposed: proposed_guard,
            })),
        })
    }
    fn commit_transaction(&self, transaction: PreparedCommit) -> io::Result<CommitReceipt> {
        let PreparedCommit {
            staged,
            target,
            mode,
            displaced_path,
            proposed_path,
            journal_path,
            guard,
        } = transaction;
        let guards = guard
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "commit guards missing"))?
            .downcast::<WindowsPreparedGuards>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "commit guards have wrong platform type"))?;
        let WindowsPreparedGuards {
            parent: parent_guard,
            transaction: transaction_guard,
            directory,
            manifest,
            proposed: proposed_guard,
        } = *guards;
        let transaction_directory = journal_path
            .as_ref()
            .and_then(|journal| journal.parent())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "commit journal has no directory"))?
            .to_path_buf();
        let transaction_identity = ordinary_directory_identity(&directory)?;
        // ReplaceFileW must create the displaced entry below this directory. Transfer
        // the candidate lease across that operation while the parent remains pinned,
        // then prove that the pathname still names the prepared directory.
        drop(transaction_guard);
        drop(directory);
        let stage = wide(&staged);
        let target_name = wide(&target);
        #[cfg(test)]
        replacement_faults::hit(mode == CommitMode::Replace, false)?;
        let displaced_path = match mode {
            CommitMode::Replace => Some(displaced_path.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "replacement has no displaced-version path")
            })?),
            CommitMode::CreateNew => None,
        };
        let result = unsafe {
            match &displaced_path {
                Some(displaced) => {
                    let displaced = wide(displaced);
                    // SAFETY: all names are verified same-directory NTFS paths. ReplaceFileW
                    // publishes the stage and moves the actual displaced target into backup
                    // as one filesystem operation; no pathname recheck is treated as CAS.
                    ReplaceFileW(
                        PCWSTR(target_name.as_ptr()),
                        PCWSTR(stage.as_ptr()),
                        PCWSTR(displaced.as_ptr()),
                        REPLACE_FILE_FLAGS(0),
                        None,
                        None,
                    )
                    .map_err(io_error)
                }
                None => {
                    // MOVEFILE_REPLACE_EXISTING is intentionally absent: a create race fails.
                    MoveFileExW(
                        PCWSTR(stage.as_ptr()),
                        PCWSTR(target_name.as_ptr()),
                        MOVEFILE_WRITE_THROUGH,
                    )
                    .map_err(io_error)
                }
            }
        };
        #[cfg(test)]
        replacement_faults::hit(mode == CommitMode::Replace, true)?;
        if let Err(error) = result {
            if mode == CommitMode::CreateNew && error.kind() == io::ErrorKind::AlreadyExists {
                if let Some(journal) = &journal_path {
                    let _ = bareline_platform::publish_commit_state(journal, CommitState::Conflict);
                }
            }
            return Err(error);
        }
        let directory = cleanup_handle(&transaction_directory, false, true)?;
        if ordinary_directory_identity(&directory)? != transaction_identity {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "save transaction directory identity changed during commit",
            ));
        }
        let target_guard = cleanup_handle(&target, false, false)?;
        let target_identity = self.identity(&target_guard)?;
        let journal_path =
            journal_path.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "commit journal missing"))?;
        let mut artifacts = Vec::new();
        let displaced = displaced_path
            .map(|path| -> io::Result<PreservedFile> {
                let file = cleanup_handle(&path, false, false)?;
                let identity = self.identity(&file)?;
                artifacts.push(file);
                Ok(PreservedFile { path, identity })
            })
            .transpose()?;
        let proposed = proposed_path
            .map(|path| -> io::Result<PreservedFile> {
                let identity = self.identity(&proposed_guard)?;
                artifacts.push(proposed_guard);
                Ok(PreservedFile { path, identity })
            })
            .transpose()?;
        let receipt = CommitReceipt {
            target: target_identity,
            displaced,
            proposed,
            journal: Some(journal_path.clone()),
            state: if mode == CommitMode::Replace {
                CommitState::Replaced
            } else {
                CommitState::Created
            },
            cleanup: CleanupResponsibility::Caller,
            cleanup_token: Some(Box::new(WindowsCommitCleanup {
                artifacts,
                authority: None,
                manifest: Some(manifest),
                held: vec![target_guard],
                journal: journal_path.clone(),
                directory,
                guards: vec![parent_guard],
            })),
        };
        self.mark_commit_state(
            &receipt,
            if mode == CommitMode::Replace {
                CommitState::Replaced
            } else {
                CommitState::Created
            },
        )?;
        Ok(receipt)
    }
    fn mark_commit_state(&self, receipt: &CommitReceipt, state: CommitState) -> io::Result<()> {
        let Some(journal) = &receipt.journal else {
            return Ok(());
        };
        if state == CommitState::CleanupPending {
            let directory = journal
                .parent()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cleanup journal has no directory"))?;
            let generation = parse_transaction_generation(
                directory
                    .file_name()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cleanup generation is missing"))?,
            )
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cleanup generation is invalid"))?;
            let authority = directory.join("cleanup-authority");
            let _ = read_manifest(open_manifest_validation(&authority)?, generation)?;
        }
        bareline_platform::publish_commit_state(journal, state)
    }
    fn inspect_commit_transactions(
        &self,
        parent: &Path,
        cancellation: &dyn bareline_platform::CommitCancellation,
    ) -> io::Result<Vec<CommitRecovery>> {
        let _parent_guard = self.guard_directory(parent)?;
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut recoveries = Vec::new();
        for entry in entries.take(4_096) {
            cancellation.check()?;
            let entry = entry?;
            let Some(generation) = parse_transaction_generation(&entry.file_name()) else {
                continue;
            };
            let kind = entry.file_type()?;
            if !kind.is_dir() {
                // A transaction-shaped reparse point or non-directory is itself an
                // orphan requiring inspection. Record only its owned parent/name;
                // never follow it to manufacture artifact or manifest evidence.
                recoveries.push(CommitRecovery {
                    target: None,
                    proposed: entry.path().join("editor-version"),
                    displaced: None,
                    journal: entry.path().join("state"),
                    state: CommitState::Unverified,
                    verified: false,
                });
                continue;
            }
            let directory = entry.path();
            let Ok(directory_guard) = cleanup_handle(&directory, false, true) else {
                recoveries.push(CommitRecovery {
                    target: None,
                    proposed: directory.join("editor-version"),
                    displaced: None,
                    journal: directory.join("state"),
                    state: CommitState::Unverified,
                    verified: false,
                });
                continue;
            };
            let opened_identity = ordinary_directory_identity(&directory_guard);
            let directory_lease = self.cache_directory_guard(&directory);
            let directory_verified = matches!(
                (&opened_identity, &directory_lease),
                (Ok(opened), Ok(Some(lease))) if opened == &lease.identity
            );
            if !directory_verified {
                recoveries.push(CommitRecovery {
                    target: None,
                    proposed: directory.join("editor-version"),
                    displaced: None,
                    journal: directory.join("state"),
                    state: CommitState::Unverified,
                    verified: false,
                });
                continue;
            }
            let _directory_lease = directory_lease.unwrap().unwrap();
            let journal = directory.join("state");
            let mut state = state_from_sealed_records(self, &journal);
            let manifest = self
                .open_sealed_read(&directory.join("manifest"))
                .and_then(|file| read_manifest(file, generation));
            let cleanup_authority = self
                .open_sealed_read(&directory.join("cleanup-authority"))
                .and_then(|file| read_manifest(file, generation));
            let durable_cleanup =
                cleanup_authority.is_ok() && matches!(state, CommitState::CleanupPending | CommitState::Unverified);
            if state == CommitState::Unverified && durable_cleanup {
                state = CommitState::CleanupPending;
            }
            let manifest = manifest.or_else(|_| cleanup_authority);
            let (target, mode) = match manifest {
                Ok((target, mode)) if target.is_absolute() && target.parent() == Some(parent) => {
                    (Some(target), Some(mode))
                }
                _ => (None, None),
            };
            let mut proposed_regular = false;
            let mut displaced_regular = false;
            for child in std::fs::read_dir(&directory)?.take(16) {
                cancellation.check()?;
                let child = child?;
                let child_kind = child.file_type()?;
                if !child_kind.is_file() || self.open_sealed_read(&child.path()).is_err() {
                    continue;
                }
                if child.file_name() == "editor-version" {
                    proposed_regular = true;
                } else if child.file_name() == "displaced-version" {
                    displaced_regular = true;
                }
            }
            let proposed = directory.join("editor-version");
            let displaced =
                (mode == Some(CommitMode::Replace) && displaced_regular).then(|| directory.join("displaced-version"));
            if target.is_none() && !proposed_regular && !displaced_regular && state == CommitState::Unverified {
                continue;
            }
            let verified = target.is_some()
                && state != CommitState::Unverified
                && (proposed_regular || state == CommitState::CleanupPending);
            recoveries.push(CommitRecovery {
                target,
                proposed,
                displaced,
                journal,
                state: if verified { state } else { CommitState::Unverified },
                verified,
            });
        }
        Ok(recoveries)
    }
    fn resume_commit_cleanup(&self, recovery: &CommitRecovery) -> io::Result<Option<CommitReceipt>> {
        if !recovery.verified || recovery.state != CommitState::CleanupPending {
            return Ok(None);
        }
        let directory = recovery
            .journal
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cleanup transaction has no directory"))?;
        let parent = directory
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cleanup transaction has no parent"))?;
        let generation = parse_transaction_generation(
            directory
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cleanup generation is missing"))?,
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cleanup generation is invalid"))?;
        let parent_guard = self.guard_directory(parent)?;
        let directory_guard = cleanup_handle(directory, false, true)?;
        let directory_identity = ordinary_directory_identity(&directory_guard)?;
        let directory_lease = self
            .cache_directory_guard(directory)?
            .filter(|lease| lease.identity == directory_identity)
            .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "cleanup directory identity changed"))?;
        let state = state_from_sealed_records(self, &recovery.journal);
        let authority_path = directory.join("cleanup-authority");
        let authority = open_manifest_cleanup(&authority_path)?;
        let (target, mode) = read_manifest(authority.try_clone()?, generation)?;
        if !matches!(state, CommitState::CleanupPending | CommitState::Unverified) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cleanup authority has an incompatible transaction state",
            ));
        }
        if recovery.target.as_ref() != Some(&target) || target.parent() != Some(parent) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cleanup target no longer matches its manifest",
            ));
        }
        let manifest_path = directory.join("manifest");
        if let Ok(primary) = self.open_sealed_read(&manifest_path) {
            let current = read_manifest(primary, generation)?;
            if current != (target.clone(), mode) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "cleanup manifest and durable authority differ",
                ));
            }
        }
        let manifest = match cleanup_handle(&manifest_path, false, false) {
            Ok(manifest) => Some(manifest),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let target_guard = self.open_sealed_read(&target)?;
        let target_identity = self.identity(&target_guard)?;
        let mut artifacts = Vec::new();
        let proposed = match cleanup_handle(&recovery.proposed, false, false) {
            Ok(file) => {
                let identity = self.identity(&file)?;
                artifacts.push(file);
                Some(PreservedFile {
                    path: recovery.proposed.clone(),
                    identity,
                })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let displaced = match recovery.displaced.as_ref() {
            Some(path) => match cleanup_handle(path, false, false) {
                Ok(file) => {
                    let identity = self.identity(&file)?;
                    artifacts.push(file);
                    Some(PreservedFile {
                        path: path.clone(),
                        identity,
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            },
            None => None,
        };
        Ok(Some(CommitReceipt {
            target: target_identity,
            displaced,
            proposed,
            journal: Some(recovery.journal.clone()),
            state: CommitState::CleanupPending,
            cleanup: CleanupResponsibility::Caller,
            cleanup_token: Some(Box::new(WindowsCommitCleanup {
                artifacts,
                authority: Some(authority),
                manifest,
                held: vec![target_guard],
                journal: recovery.journal.clone(),
                directory: directory_guard,
                guards: vec![parent_guard, directory_lease.guard],
            })),
        }))
    }
    fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
        let stage = wide(staged);
        let target = wide(target);
        #[cfg(test)]
        replacement_faults::hit(existed, false)?;
        // SAFETY: both owned names are same-directory paths; no truncate/in-place fallback.
        let result = unsafe {
            if existed {
                ReplaceFileW(
                    PCWSTR(target.as_ptr()),
                    PCWSTR(stage.as_ptr()),
                    None,
                    REPLACE_FILE_FLAGS(0),
                    None,
                    None,
                )
                .map_err(io_error)
            } else {
                MoveFileExW(PCWSTR(stage.as_ptr()), PCWSTR(target.as_ptr()), MOVEFILE_WRITE_THROUGH).map_err(io_error)
            }
        };
        #[cfg(test)]
        replacement_faults::hit(existed, true)?;
        result
    }
}

fn process_created(process: HANDLE) -> io::Result<u64> {
    use windows::Win32::System::Threading::GetProcessTimes;
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) }.map_err(io_error)?;
    Ok(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
}

impl WindowsFileSystem {
    fn validate_path(&self, path: &Path, writing: bool) -> io::Result<()> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "only absolute local paths are supported",
            ));
        }
        use std::path::{Component, Prefix};
        if !matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
            || path
                .components()
                .any(|c| matches!(c, Component::Normal(s) if s.encode_wide().any(|u| u == b':' as u16)))
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "device and alternate-stream paths are not supported",
            ));
        }
        // Classify the drive root before querying any descendant or volume path.
        // A mapped network drive needs an explicit action-scoped provider instead.
        let drive = match path.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
                _ => return Err(io::Error::new(io::ErrorKind::PermissionDenied, "unsupported drive")),
            },
            _ => return Err(io::Error::new(io::ErrorKind::PermissionDenied, "unsupported drive")),
        };
        let drive_root = [drive as u16, b':' as u16, b'\\' as u16, 0];
        // SAFETY: the terminated drive-root buffer is live through classification.
        if unsafe { GetDriveTypeW(PCWSTR(drive_root.as_ptr())) } != 3 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "network/removable drive requires supported explicit policy",
            ));
        }
        let value = wide(path);
        let mut root = vec![0u16; 32768];
        // SAFETY: owned terminated UTF-16 input and writable output buffers outlive calls.
        unsafe {
            GetVolumePathNameW(PCWSTR(value.as_ptr()), &mut root).map_err(io_error)?;
            if GetDriveTypeW(PCWSTR(root.as_ptr())) != 3 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "save requires a local fixed NTFS volume",
                ));
            }
            let mut filesystem = [0u16; 64];
            GetVolumeInformationW(PCWSTR(root.as_ptr()), None, None, None, None, Some(&mut filesystem))
                .map_err(io_error)?;
            let end = filesystem.iter().position(|c| *c == 0).unwrap_or(filesystem.len());
            if String::from_utf16_lossy(&filesystem[..end]) != "NTFS" {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "filesystem replacement guarantees unavailable",
                ));
            }
            for ancestor in path.ancestors() {
                let name = wide(ancestor);
                let attributes = GetFileAttributesW(PCWSTR(name.as_ptr()));
                if attributes == INVALID_FILE_ATTRIBUTES {
                    if ancestor == path && !path.exists() {
                        continue;
                    }
                    return Err(io::Error::last_os_error());
                }
                if attributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0) != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "reparse/cloud paths need explicit trust integration",
                    ));
                }
                if writing && ancestor == path && attributes & FILE_ATTRIBUTE_READONLY.0 != 0 {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, "file is read-only"));
                }
            }
            if let Ok(file) = File::open(path) {
                let mut info = BY_HANDLE_FILE_INFORMATION::default();
                GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info).map_err(io_error)?;
                if writing && info.nNumberOfLinks > 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "hard-linked targets need explicit replacement approval",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Edit, EditTransaction, TextOffset};
    use bareline_file_io::cancellation::Cancellation;
    use bareline_file_io::lifecycle::{
        FileError, inspect_save_recovery, inspect_save_transactions, open_utf8, save_utf8,
    };
    use std::path::PathBuf;

    struct MigrationFixture(PathBuf);
    impl MigrationFixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "bareline-native-migration-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for MigrationFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn migration_lease_publishes_real_file_and_nested_directory() {
        for directory in [false, true] {
            let fixture = MigrationFixture::new(if directory { "directory" } else { "file" });
            let source = fixture.0.join("source");
            let target = fixture.0.join("target");
            if directory {
                std::fs::create_dir(&source).unwrap();
                std::fs::write(source.join("nested.bin"), b"nested bytes").unwrap();
            } else {
                std::fs::write(&source, b"file bytes").unwrap();
            }
            let lease = WindowsFileSystem.migration_entry_guard(&source).unwrap();
            if !directory {
                let mut read = WindowsFileSystem.open_migration_read(&lease).unwrap();
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut read, &mut bytes).unwrap();
                assert_eq!(bytes, b"file bytes");
            }
            WindowsFileSystem.publish_migration_entry(&target, lease).unwrap();
            assert!(!source.exists());
            assert!(target.exists());
            if directory {
                assert_eq!(std::fs::read(target.join("nested.bin")).unwrap(), b"nested bytes");
            }
        }
    }

    #[test]
    fn migration_lease_retries_after_real_sharing_violation() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = MigrationFixture::new("locked");
        let source = fixture.0.join("source.bin");
        std::fs::write(&source, b"locked bytes").unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(&source)
            .unwrap();
        assert!(WindowsFileSystem.migration_entry_guard(&source).is_err());
        drop(lock);
        let lease = WindowsFileSystem.migration_entry_guard(&source).unwrap();
        WindowsFileSystem
            .publish_migration_entry(&fixture.0.join("published.bin"), lease)
            .unwrap();
    }

    #[test]
    fn native_profile_migration_restarts_after_locked_source() {
        use bareline_file_io::profile_migration::{ItemState, MigrationRequest, migrate};
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = MigrationFixture::new("engine-locked-restart");
        let roaming = fixture.0.join("roaming");
        let local = fixture.0.join("local");
        std::fs::create_dir(&roaming).unwrap();
        let source = roaming.join("settings.toml");
        std::fs::write(&source, b"theme = 'dark'").unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(&source)
            .unwrap();
        let request = || MigrationRequest {
            roaming: &roaming,
            local: &local,
            retire_sources: false,
            max_entries: 256,
            max_io_bytes: 1024 * 1024,
            max_time: std::time::Duration::from_secs(2),
        };
        let first = migrate(request(), &WindowsFileSystem, &|| false).unwrap();
        assert_eq!(first.state("settings.toml"), Some(ItemState::FailedRetryable));
        assert!(source.is_file());
        assert!(!local.join("settings.toml").exists());
        drop(lock);
        let second = migrate(request(), &WindowsFileSystem, &|| false).unwrap();
        assert_eq!(second.state("settings.toml"), Some(ItemState::Published));
        assert_eq!(std::fs::read(local.join("settings.toml")).unwrap(), b"theme = 'dark'");
        assert!(source.is_file());
    }

    #[test]
    fn native_directory_source_is_retained_when_lease_cannot_exclude_new_children() {
        use bareline_file_io::profile_migration::{ItemState, MigrationRequest, migrate, retirement_ready};
        let fixture = MigrationFixture::new("engine-directory-retention");
        let roaming = fixture.0.join("roaming");
        let local = fixture.0.join("local");
        let source = roaming.join("recovery");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("original.bin"), b"original").unwrap();
        let request = |retire_sources| MigrationRequest {
            roaming: &roaming,
            local: &local,
            retire_sources,
            max_entries: 256,
            max_io_bytes: 1024 * 1024,
            max_time: std::time::Duration::from_secs(2),
        };

        let first = migrate(request(false), &WindowsFileSystem, &|| false).unwrap();
        let first_recovery = first.items.iter().find(|item| item.name == "recovery").unwrap();
        assert_eq!(
            first_recovery.state,
            ItemState::Published,
            "recovery migration detail: {:?}",
            first_recovery.detail
        );

        let lease = WindowsFileSystem.migration_entry_guard(&source).unwrap();
        let late_child = source.join("late-child.bin");
        std::fs::write(&late_child, b"late generation").unwrap();
        assert!(late_child.is_file());
        drop(lease);
        std::fs::remove_file(late_child).unwrap();

        let retired = migrate(request(true), &WindowsFileSystem, &|| false).unwrap();
        let recovery = retired.items.iter().find(|item| item.name == "recovery").unwrap();
        assert_eq!(recovery.state, ItemState::Published);
        assert!(
            recovery
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("retained by policy"))
        );
        assert!(!retired.retryable);
        assert!(source.join("original.bin").is_file());
        assert!(local.join("recovery/original.bin").is_file());
        assert!(!local.join(".bareline-migration-retained/recovery").exists());
        assert!(!retirement_ready(&local, &WindowsFileSystem));
    }

    struct FailOneNativePublication(std::sync::atomic::AtomicBool);
    impl LocalFileSystem for FailOneNativePublication {
        fn guard_directory(&self, path: &Path) -> io::Result<std::sync::Arc<dyn Send + Sync>> {
            WindowsFileSystem.guard_directory(path)
        }
        fn migration_entry_guard(&self, path: &Path) -> io::Result<bareline_platform::CacheDirectoryLease> {
            WindowsFileSystem.migration_entry_guard(path)
        }
        fn open_migration_read(&self, lease: &bareline_platform::CacheDirectoryLease) -> io::Result<File> {
            WindowsFileSystem.open_migration_read(lease)
        }
        fn publish_migration_entry(
            &self,
            target: &Path,
            lease: bareline_platform::CacheDirectoryLease,
        ) -> io::Result<()> {
            if self.0.swap(false, std::sync::atomic::Ordering::SeqCst) {
                return Err(io::Error::other("injected interruption after verified staging"));
            }
            WindowsFileSystem.publish_migration_entry(target, lease)
        }
        fn open_sealed_read(&self, path: &Path) -> io::Result<File> {
            WindowsFileSystem.open_sealed_read(path)
        }
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            WindowsFileSystem.identity(file)
        }
        fn validate_target(&self, path: &Path) -> io::Result<()> {
            WindowsFileSystem.validate_target(path)
        }
        fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
            WindowsFileSystem.commit(staged, target, existed)
        }
    }

    #[test]
    fn native_journal_and_guards_resume_after_verified_stage_interruption() {
        use bareline_file_io::profile_migration::{ItemState, MigrationRequest, migrate};
        let fixture = MigrationFixture::new("native-transition-restart");
        let roaming = fixture.0.join("roaming");
        let local = fixture.0.join("local");
        std::fs::create_dir(&roaming).unwrap();
        std::fs::write(roaming.join("settings.toml"), b"theme = 'dark'").unwrap();
        let platform = FailOneNativePublication(std::sync::atomic::AtomicBool::new(true));
        let request = || MigrationRequest {
            roaming: &roaming,
            local: &local,
            retire_sources: false,
            max_entries: 256,
            max_io_bytes: 1024 * 1024,
            max_time: std::time::Duration::from_secs(2),
        };
        let first = migrate(request(), &platform, &|| false).unwrap();
        assert_eq!(first.state("settings.toml"), Some(ItemState::FailedRetryable));
        assert!(local.join(".bareline-migration-stage-settings.toml").is_file());
        assert!(roaming.join("settings.toml").is_file());
        let second = migrate(request(), &platform, &|| false).unwrap();
        assert_eq!(second.state("settings.toml"), Some(ItemState::Published));
        assert_eq!(std::fs::read(local.join("settings.toml")).unwrap(), b"theme = 'dark'");
        assert!(roaming.join("settings.toml").is_file());
    }

    #[test]
    fn migration_lease_rejects_real_junction() {
        let fixture = MigrationFixture::new("junction");
        let external = MigrationFixture::new("junction-external");
        std::fs::write(external.0.join("sentinel"), b"outside").unwrap();
        let junction = fixture.0.join("source");
        let status = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&external.0)
            .status()
            .unwrap();
        assert!(status.success(), "junction fixture creation failed");
        assert!(WindowsFileSystem.migration_entry_guard(&junction).is_err());
        assert_eq!(std::fs::read(external.0.join("sentinel")).unwrap(), b"outside");
        std::fs::remove_dir(&junction).unwrap();
    }
    #[test]
    fn sealed_read_denies_mutation_until_guard_is_dropped() {
        let path = std::env::temp_dir().join(format!("bareline-sealed-{}.txt", std::process::id()));
        let renamed = path.with_extension("moved");
        std::fs::write(&path, b"sealed").unwrap();
        let guard = WindowsFileSystem.open_sealed_read(&path).unwrap();
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::write(&path, b"changed").is_err());
        assert!(std::fs::rename(&path, &renamed).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        drop(guard);
        std::fs::write(&path, b"changed").unwrap();
        std::fs::rename(&path, &renamed).unwrap();
        std::fs::remove_file(renamed).unwrap();
    }
    #[test]
    fn proposed_copy_cancellation_removes_partial_transaction() {
        struct CancelAfterFirstChunk(std::sync::atomic::AtomicUsize);
        impl bareline_platform::CommitCancellation for CancelAfterFirstChunk {
            fn check(&self) -> io::Result<()> {
                if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Ok(())
                } else {
                    Err(io::Error::from(io::ErrorKind::Interrupted))
                }
            }
        }
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-copy-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let stage = directory.join("stage.tmp");
        let target = directory.join("target.txt");
        std::fs::write(&stage, vec![b'x'; 2 * 1024 * 1024]).unwrap();
        let error = WindowsFileSystem
            .prepare_commit(
                &stage,
                &target,
                CommitMode::CreateNew,
                &CancelAfterFirstChunk(std::sync::atomic::AtomicUsize::new(0)),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn readonly_source_can_open_but_cannot_be_replaced() {
        let path = std::env::temp_dir().join(format!("bareline-readonly-{}.txt", std::process::id()));
        std::fs::write(&path, b"read only").unwrap();
        let original_permissions = std::fs::metadata(&path).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let result = open_utf8(&path, &WindowsFileSystem, Budget::new(1024), Budget::new(0));
        let refused = WindowsFileSystem.validate_target(&path).is_err();
        std::fs::set_permissions(&path, original_permissions).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(result.is_ok());
        assert!(refused);
    }
    struct DeniedCommit;
    struct CancellingPlatform {
        cancellation: bareline_file_io::cancellation::Cancellation,
        validations: std::sync::atomic::AtomicUsize,
        during_commit: bool,
    }
    impl LocalFileSystem for CancellingPlatform {
        fn guard_directory(&self, path: &Path) -> io::Result<std::sync::Arc<dyn Send + Sync>> {
            WindowsFileSystem.guard_directory(path)
        }
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            WindowsFileSystem.identity(file)
        }
        fn validate_target(&self, path: &Path) -> io::Result<()> {
            WindowsFileSystem.validate_target(path)?;
            if self.validations.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 1 && !self.during_commit {
                self.cancellation.cancel();
            }
            Ok(())
        }
        fn prepare_commit(
            &self,
            staged: &Path,
            target: &Path,
            mode: CommitMode,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> io::Result<PreparedCommit> {
            WindowsFileSystem.prepare_commit(staged, target, mode, cancellation)
        }
        fn commit_transaction(&self, transaction: PreparedCommit) -> io::Result<CommitReceipt> {
            assert!(self.during_commit, "cancelled save must not reach commit");
            self.cancellation.cancel();
            WindowsFileSystem.commit_transaction(transaction)
        }
        fn cleanup_commit(&self, receipt: &mut CommitReceipt) -> io::Result<()> {
            WindowsFileSystem.cleanup_commit(receipt)
        }
        fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
            assert!(self.during_commit, "cancelled save must not reach commit");
            self.cancellation.cancel();
            WindowsFileSystem.commit(staged, target, existed)
        }
    }
    #[test]
    fn save_cancellation_cleans_before_commit_and_retains_versions_after_commit() {
        use bareline_file_io::{cancellation::Cancellation, lifecycle::save_utf8_cancellable};
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        assert!(directory.canonicalize().unwrap().starts_with(&root));
        let path = directory.join("fixture.txt");
        std::fs::write(&path, b"original").unwrap();
        let opened = open_utf8(&path, &WindowsFileSystem, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let document =
            bareline_document::Document::from_utf8("replacement", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        for during_commit in [false, true] {
            let cancellation = Cancellation::default();
            let platform = CancellingPlatform {
                cancellation: cancellation.clone(),
                validations: 0.into(),
                during_commit,
            };
            let result = save_utf8_cancellable(
                document.snapshot(),
                &path,
                Some(&opened.fingerprint),
                false,
                &platform,
                &cancellation,
            );
            if during_commit {
                let Err(FileError::CancelledAfterCommit {
                    proposed,
                    displaced: Some(displaced),
                    transaction,
                    ..
                }) = result
                else {
                    panic!("late cancellation must report the committed transaction")
                };
                assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
                assert_eq!(std::fs::read(proposed).unwrap(), b"replacement");
                assert_eq!(std::fs::read(displaced).unwrap(), b"original");
                assert_eq!(
                    bareline_platform::read_commit_state(&transaction).unwrap(),
                    CommitState::Conflict
                );
            } else {
                assert!(matches!(result, Err(FileError::Cancelled)));
                assert_eq!(std::fs::read(&path).unwrap(), b"original");
                assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
    impl LocalFileSystem for DeniedCommit {
        fn guard_directory(&self, path: &Path) -> io::Result<std::sync::Arc<dyn Send + Sync>> {
            WindowsFileSystem.guard_directory(path)
        }
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            WindowsFileSystem.identity(file)
        }
        fn validate_target(&self, path: &Path) -> io::Result<()> {
            WindowsFileSystem.validate_target(path)
        }
        fn prepare_commit(
            &self,
            staged: &Path,
            target: &Path,
            mode: CommitMode,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> io::Result<PreparedCommit> {
            WindowsFileSystem.prepare_commit(staged, target, mode, cancellation)
        }
        fn commit_transaction(&self, _: PreparedCommit) -> io::Result<CommitReceipt> {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        }
    }
    #[test]
    fn staged_save_preserves_original_on_failure_and_conflict_and_tracks_captured_state() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-lifecycle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        assert!(directory.canonicalize().unwrap().starts_with(&root));
        let path = directory.join("fixture.txt");
        std::fs::write(&path, b"\xef\xbb\xbfa\r\n").unwrap();
        let mut opened = open_utf8(&path, &WindowsFileSystem, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        assert!(opened.bom);
        let mut insert = |text: &str| {
            let snapshot = opened.document.snapshot();
            opened
                .document
                .apply(EditTransaction {
                    base_revision: snapshot.revision,
                    edits: vec![Edit {
                        range: TextOffset(0)..TextOffset(0),
                        insert: text.into(),
                    }],
                })
                .unwrap();
        };
        insert("b");
        let captured = opened.document.snapshot();
        let failure = save_utf8(captured.clone(), &path, Some(&opened.fingerprint), true, &DeniedCommit);
        let Err(FileError::Commit { staged, proposed, .. }) = failure else {
            panic!("expected injected commit failure")
        };
        assert_eq!(std::fs::read(&path).unwrap(), b"\xef\xbb\xbfa\r\n");
        assert_eq!(std::fs::read(&staged).unwrap(), b"\xef\xbb\xbfba\r\n");
        std::fs::remove_file(staged).unwrap();
        let proposed = proposed.expect("replacement prepares editor recovery");
        assert_eq!(std::fs::read(&proposed).unwrap(), b"\xef\xbb\xbfba\r\n");
        std::fs::remove_dir_all(proposed.parent().unwrap()).unwrap();
        let snapshot = opened.document.snapshot();
        opened
            .document
            .apply(EditTransaction {
                base_revision: snapshot.revision,
                edits: vec![Edit {
                    range: TextOffset(0)..TextOffset(0),
                    insert: "c".into(),
                }],
            })
            .unwrap();
        let saved = save_utf8(captured, &path, Some(&opened.fingerprint), true, &WindowsFileSystem).unwrap();
        opened.document.mark_saved(&saved.captured).unwrap();
        assert!(opened.document.dirty());
        opened.document.undo().unwrap();
        assert!(!opened.document.dirty());
        std::fs::write(&path, b"external").unwrap();
        let failure = save_utf8(
            opened.document.snapshot(),
            &path,
            Some(&saved.fingerprint),
            true,
            &WindowsFileSystem,
        );
        let Err(FileError::Conflict {
            proposed, transaction, ..
        }) = failure
        else {
            panic!("expected conflict")
        };
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        assert_eq!(std::fs::read(&proposed).unwrap(), b"\xef\xbb\xbfba\r\n");
        std::fs::remove_dir_all(transaction.parent().unwrap()).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    struct InterleavingCommit {
        bytes: &'static [u8],
        after_commit: bool,
    }
    struct CleanupFailure;
    impl LocalFileSystem for CleanupFailure {
        fn guard_directory(&self, path: &Path) -> io::Result<std::sync::Arc<dyn Send + Sync>> {
            WindowsFileSystem.guard_directory(path)
        }
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            WindowsFileSystem.identity(file)
        }
        fn validate_target(&self, path: &Path) -> io::Result<()> {
            WindowsFileSystem.validate_target(path)
        }
        fn prepare_commit(
            &self,
            staged: &Path,
            target: &Path,
            mode: CommitMode,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> io::Result<PreparedCommit> {
            WindowsFileSystem.prepare_commit(staged, target, mode, cancellation)
        }
        fn commit_transaction(&self, transaction: PreparedCommit) -> io::Result<CommitReceipt> {
            WindowsFileSystem.commit_transaction(transaction)
        }
        fn cleanup_commit(&self, _: &mut CommitReceipt) -> io::Result<()> {
            Err(io::Error::other("injected cleanup failure"))
        }
        fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
            WindowsFileSystem.commit(staged, target, existed)
        }
    }
    impl LocalFileSystem for InterleavingCommit {
        fn guard_directory(&self, path: &Path) -> io::Result<std::sync::Arc<dyn Send + Sync>> {
            WindowsFileSystem.guard_directory(path)
        }
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            WindowsFileSystem.identity(file)
        }
        fn validate_target(&self, path: &Path) -> io::Result<()> {
            WindowsFileSystem.validate_target(path)
        }
        fn prepare_commit(
            &self,
            staged: &Path,
            target: &Path,
            mode: CommitMode,
            cancellation: &dyn bareline_platform::CommitCancellation,
        ) -> io::Result<PreparedCommit> {
            WindowsFileSystem.prepare_commit(staged, target, mode, cancellation)
        }
        fn commit_transaction(&self, transaction: PreparedCommit) -> io::Result<CommitReceipt> {
            let target = transaction.target.clone();
            if !self.after_commit {
                std::fs::write(&target, self.bytes)?;
            }
            let receipt = WindowsFileSystem.commit_transaction(transaction)?;
            if self.after_commit {
                assert!(
                    std::fs::write(target, self.bytes).is_err(),
                    "receipt guards must block a writer until verification and cleanup"
                );
            }
            Ok(receipt)
        }
        fn cleanup_commit(&self, receipt: &mut CommitReceipt) -> io::Result<()> {
            WindowsFileSystem.cleanup_commit(receipt)
        }
        fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
            WindowsFileSystem.commit(staged, target, existed)
        }
    }

    #[test]
    fn replacement_race_retains_displaced_and_editor_bytes() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-displaced-race-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("target.txt");
        std::fs::write(&target, b"approved bytes").unwrap();
        let opened = open_utf8(&target, &WindowsFileSystem, Budget::new(1024), Budget::new(1024)).unwrap();
        let editor =
            bareline_document::Document::from_utf8("editor bytes", Budget::new(1024), Budget::new(1024)).unwrap();
        let result = save_utf8(
            editor.snapshot(),
            &target,
            Some(&opened.fingerprint),
            false,
            &InterleavingCommit {
                bytes: b"concurrent bytes",
                after_commit: false,
            },
        );
        let (proposed, displaced, actual_displaced) = match result {
            Err(FileError::ConflictAfterCommit {
                proposed,
                displaced,
                actual_displaced,
                ..
            }) => (proposed, displaced, actual_displaced),
            Err(error) => panic!("the actual displaced version must be reported as a conflict: {error:?}"),
            Ok(_) => panic!("the actual displaced version must be reported as a conflict: unexpected success"),
        };
        assert_eq!(std::fs::read(&target).unwrap(), b"editor bytes");
        assert_eq!(std::fs::read(&proposed).unwrap(), b"editor bytes");
        assert_eq!(std::fs::read(&displaced).unwrap(), b"concurrent bytes");
        assert_ne!(actual_displaced, opened.fingerprint);

        std::fs::write(&target, b"approved again").unwrap();
        let reopened = open_utf8(&target, &WindowsFileSystem, Budget::new(1024), Budget::new(1024)).unwrap();
        save_utf8(
            editor.snapshot(),
            &target,
            Some(&reopened.fingerprint),
            false,
            &InterleavingCommit {
                bytes: b"writer after commit",
                after_commit: true,
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"editor bytes");
        std::fs::write(&target, b"writer after receipt").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"writer after receipt");

        let new_target = directory.join("new-target.txt");
        let result = save_utf8(
            editor.snapshot(),
            &new_target,
            None,
            false,
            &InterleavingCommit {
                bytes: b"competing create",
                after_commit: false,
            },
        );
        let proposed = match result {
            Err(FileError::ConflictAfterCreate { proposed, .. }) => proposed,
            Err(error) => panic!("a competing create must fail without replacing either version: {error:?}"),
            Ok(_) => panic!("a competing create must fail without replacing either version: unexpected success"),
        };
        assert_eq!(std::fs::read(&new_target).unwrap(), b"competing create");
        assert_eq!(std::fs::read(&proposed).unwrap(), b"editor bytes");
        let recovered = inspect_save_transactions(&directory, &WindowsFileSystem, &Cancellation::default()).unwrap();
        assert!(
            recovered.iter().any(|transaction| {
                transaction.state == CommitState::Conflict
                    && std::fs::read(&transaction.editor_version).ok().as_deref() == Some(b"editor bytes")
            }),
            "restart discovery must recover a typed conflict with exact editor bytes"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn orphan_reopens_independent_editor_bytes_after_target_changes() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-orphan-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("target.txt");
        let stage = directory.join("stage.tmp");
        std::fs::write(&target, b"prior bytes").unwrap();
        std::fs::write(&stage, b"editor bytes").unwrap();
        let transaction = WindowsFileSystem
            .prepare_commit(
                &stage,
                &target,
                CommitMode::Replace,
                &bareline_file_io::cancellation::Cancellation::default(),
            )
            .unwrap();
        let receipt = WindowsFileSystem.commit_transaction(transaction).unwrap();
        let proposed = receipt.proposed.as_ref().unwrap().path.clone();
        let displaced = receipt.displaced.as_ref().unwrap().path.clone();
        drop(receipt);

        std::fs::write(&target, b"later writer").unwrap();
        assert_eq!(std::fs::read(&proposed).unwrap(), b"editor bytes");
        assert_eq!(std::fs::read(&displaced).unwrap(), b"prior bytes");
        let recovered = inspect_save_transactions(&directory, &WindowsFileSystem, &Cancellation::default()).unwrap();
        assert!(recovered.iter().any(|recovery| {
            recovery.target.as_ref() == Some(&target)
                && recovery.editor_version == proposed
                && recovery.other_version.as_ref() == Some(&displaced)
                && recovery.state == CommitState::Replaced
        }));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn verified_save_returns_cleanup_warning_with_retry_owner() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-cleanup-retry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("target.txt");
        std::fs::write(&target, b"prior bytes").unwrap();
        let opened = open_utf8(&target, &WindowsFileSystem, Budget::new(1024), Budget::new(1024)).unwrap();
        let editor =
            bareline_document::Document::from_utf8("editor bytes", Budget::new(1024), Budget::new(1024)).unwrap();
        let saved = save_utf8(
            editor.snapshot(),
            &target,
            Some(&opened.fingerprint),
            false,
            &CleanupFailure,
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"editor bytes");
        let cleanup = saved.cleanup.expect("verified save must carry cleanup retry ownership");
        assert!(cleanup.editor_version.exists());
        assert!(cleanup.retry(&WindowsFileSystem).unwrap());
        assert!(!cleanup.retry(&WindowsFileSystem).unwrap());
        assert!(!cleanup.editor_version.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn restart_discovery_retains_malformed_transaction_as_unverified() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-malformed-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let transaction = directory.join(".bareline-save-00000000000000000000000000000001");
        std::fs::create_dir(&transaction).unwrap();
        std::fs::write(transaction.join("manifest"), b"partial").unwrap();
        std::fs::write(transaction.join("state.replaced"), vec![b'x'; 256]).unwrap();
        std::fs::write(transaction.join("editor-version"), b"recoverable editor bytes").unwrap();

        let recovered = inspect_save_transactions(&directory, &WindowsFileSystem, &Cancellation::default()).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].state, CommitState::Unverified);
        assert!(recovered[0].target.is_none());
        assert_eq!(
            std::fs::read(&recovered[0].editor_version).unwrap(),
            b"recoverable editor bytes"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn restart_classifies_artifacts_retired_during_cleanup() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-partial-cleanup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("target.txt");
        let stage = directory.join("stage.tmp");
        std::fs::write(&target, b"prior bytes").unwrap();
        std::fs::write(&stage, b"editor bytes").unwrap();
        let transaction = WindowsFileSystem
            .prepare_commit(&stage, &target, CommitMode::Replace, &Cancellation::default())
            .unwrap();
        let mut receipt = WindowsFileSystem.commit_transaction(transaction).unwrap();
        receipt
            .cleanup_token
            .as_mut()
            .unwrap()
            .publish_cleanup_authority()
            .unwrap();
        WindowsFileSystem
            .mark_commit_state(&receipt, CommitState::CleanupPending)
            .unwrap();
        let journal = receipt.journal.clone().unwrap();
        let proposed = receipt.proposed.as_ref().unwrap().path.clone();
        let displaced = receipt.displaced.as_ref().unwrap().path.clone();
        FAIL_CLEANUP_BEFORE_MANIFEST.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(WindowsFileSystem.cleanup_commit(&mut receipt).is_err());
        assert!(!proposed.exists());
        assert!(!displaced.exists());
        assert!(bareline_platform::commit_state_path(&journal, CommitState::CleanupPending).exists());
        assert!(!bareline_platform::commit_state_path(&journal, CommitState::Replaced).exists());
        assert!(journal.parent().unwrap().join("manifest").exists());
        assert!(journal.parent().unwrap().join("cleanup-authority").exists());
        drop(receipt);
        let recovered = inspect_save_recovery(&directory, &WindowsFileSystem, &Cancellation::default()).unwrap();
        assert!(recovered.conflicts.is_empty());
        assert_eq!(recovered.cleanups.len(), 1);
        assert_eq!(recovered.cleanups[0].target, target);
        assert!(recovered.cleanups[0].retry(&WindowsFileSystem).unwrap());
        assert!(!journal.parent().unwrap().exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn substituted_transaction_directory_never_reads_external_sentinel() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-junction-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let external = directory.with_extension("external");
        std::fs::create_dir(&directory).unwrap();
        std::fs::create_dir(&external).unwrap();
        std::fs::write(external.join("manifest"), b"foreign manifest").unwrap();
        std::fs::write(external.join("editor-version"), b"foreign editor bytes").unwrap();
        let junction = directory.join(".bareline-save-00000000000000000000000000000001");
        let status = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&external)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "junction fixture creation failed");
        let recovered = inspect_save_transactions(&directory, &WindowsFileSystem, &Cancellation::default()).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].state, CommitState::Unverified);
        assert!(recovered[0].target.is_none());
        assert_eq!(std::fs::read(external.join("manifest")).unwrap(), b"foreign manifest");
        assert_eq!(
            std::fs::read(external.join("editor-version")).unwrap(),
            b"foreign editor bytes"
        );
        std::fs::remove_dir(&junction).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        std::fs::remove_dir_all(external).unwrap();
    }

    #[test]
    fn sharing_violation_retains_original_stage_and_transaction_recovery() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let directory = root.join(format!(
            "bareline-sharing-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("target.txt");
        std::fs::write(&target, b"original bytes").unwrap();
        let opened = open_utf8(&target, &WindowsFileSystem, Budget::new(1024), Budget::new(1024)).unwrap();
        let held = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(&target)
            .unwrap();
        let editor =
            bareline_document::Document::from_utf8("editor bytes", Budget::new(1024), Budget::new(1024)).unwrap();
        let result = save_utf8(
            editor.snapshot(),
            &target,
            Some(&opened.fingerprint),
            false,
            &WindowsFileSystem,
        );
        let Err(FileError::Commit {
            staged,
            proposed: Some(proposed),
            transaction: Some(transaction),
            ..
        }) = result
        else {
            panic!("sharing violation must retain transaction recovery")
        };
        assert_eq!(std::fs::read(&target).unwrap(), b"original bytes");
        assert_eq!(std::fs::read(staged).unwrap(), b"editor bytes");
        assert_eq!(std::fs::read(proposed).unwrap(), b"editor bytes");
        assert_eq!(
            bareline_platform::read_commit_state(&transaction).unwrap(),
            CommitState::Precommit
        );
        drop(held);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
#[path = "replacement_faults.rs"]
mod replacement_faults;
