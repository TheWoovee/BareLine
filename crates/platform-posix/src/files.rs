// SPDX-License-Identifier: MPL-2.0
//! `LocalFileSystem` for Linux and macOS.
use crate::{
    cache, capability, entries, process, resolve,
    sys::{self, READ, denied},
    transaction,
    trust::{self, DirectoryGuard, PosixPathTrustProvider},
};
use bareline_platform::{
    CacheDirectoryIdentity, CacheDirectoryLease, CacheProcessIdentity, CacheRemovalOutcome, CapabilityReport,
    CommitCancellation, CommitMode, CommitReceipt, CommitRecovery, CommitState, FileIdentity, FilesystemCapability,
    LocalFileSystem, PreparedCommit, ProcessLiveness, RemoteReadAccess, SaveStrategy,
};
use rustix::fs::{Access, AtFlags, CWD};
use std::{fs::File, io, os::unix::fs::MetadataExt, path::Path, sync::Arc, time::Duration};

/// Background file operations on Linux and macOS.
///
/// POSIX has no sharing modes, so `release_source_read` and `check_source_read`
/// keep their default no-op and success: a source read never holds other
/// writers off, and changes are detected by identity and hash instead.
#[derive(Clone, Copy, Debug, Default)]
pub struct PosixFileSystem;

impl PosixFileSystem {
    /// Identity for external-change checks. Links resolve first and the final
    /// open never follows a name, so a link swapped in later reads as a change.
    pub fn current_identity(&self, path: &Path) -> io::Result<FileIdentity> {
        let resolved = resolve::resolve(path)?;
        let (_, file) = trust::walk(&resolved.path, READ)?;
        self.identity(&file)
    }

    /// The concrete folder guard behind `guard_directory`.
    pub fn guard_directory_handle(&self, path: &Path) -> io::Result<DirectoryGuard> {
        DirectoryGuard::open(path)
    }

    fn validate_path(&self, path: &Path, writing: bool) -> io::Result<()> {
        if !path.is_absolute() {
            return Err(denied("only absolute local paths are supported"));
        }
        let resolved = resolve::resolve(path)?;
        if !writing {
            return Ok(());
        }
        match std::fs::symlink_metadata(&resolved.path) {
            // No write bit at all reads as read-only even for the superuser, like
            // the Windows read-only attribute; otherwise the caller's access decides.
            Ok(metadata)
                if metadata.mode() & 0o222 == 0
                    || rustix::fs::accessat(CWD, &resolved.path, Access::WRITE_OK, AtFlags::empty()).is_err() =>
            {
                return Err(denied("file is read-only"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        // Weaker filesystems and linked targets choose their save path at commit;
        // only locations without any write path are refused here.
        let report = capability::report_resolved(&resolved)?;
        if report.save == SaveStrategy::CopyOnly {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                report.notice().unwrap_or("saving to this location is unavailable"),
            ));
        }
        Ok(())
    }
}

impl LocalFileSystem for PosixFileSystem {
    fn cache_process_identity(&self) -> io::Result<CacheProcessIdentity> {
        process::current()
    }

    fn cache_process_liveness(&self, pid: u32, created: u64) -> ProcessLiveness {
        process::liveness(pid, created)
    }

    fn cache_directory_guard(&self, path: &Path) -> io::Result<Option<CacheDirectoryLease>> {
        cache::directory_lease(path)
    }

    fn migration_entry_guard(&self, path: &Path) -> io::Result<CacheDirectoryLease> {
        cache::migration_lease(path)
    }

    fn open_migration_read(&self, lease: &CacheDirectoryLease) -> io::Result<File> {
        cache::open_migration_read(lease)
    }

    fn publish_migration_entry(&self, target: &Path, lease: CacheDirectoryLease) -> io::Result<()> {
        let publish = lease
            .migration_publisher
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "migration publication lease unavailable"))?;
        publish(target)
    }

    fn remove_owned_cache_directory(
        &self,
        root: &Path,
        candidate: &Path,
        root_identity: CacheDirectoryIdentity,
        candidate_identity: CacheDirectoryIdentity,
        proof_name: &str,
        proof_bytes: &[u8],
        max_entries: usize,
        max_time: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> CacheRemovalOutcome {
        cache::remove(
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

    /// Mounted remote filesystems need no separate consent on POSIX, so the
    /// scoped reader is this filesystem itself.
    fn scoped_remote_read(&self, _: RemoteReadAccess) -> io::Result<Arc<dyn LocalFileSystem>> {
        Ok(Arc::new(PosixFileSystem))
    }

    fn open_follow_read(&self, path: &Path) -> io::Result<(File, Arc<dyn Send + Sync>)> {
        PosixPathTrustProvider.open_follow_read(path)
    }

    fn guard_directory(&self, path: &Path) -> io::Result<Arc<dyn Send + Sync>> {
        Ok(Arc::new(DirectoryGuard::open(path)?))
    }

    fn available_space(&self, path: &Path) -> io::Result<u64> {
        let stat = rustix::fs::statvfs(path)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }

    /// The final name is never followed and only a regular file is returned.
    /// POSIX cannot keep other writers off it; readers verify by hash.
    fn open_sealed_read(&self, path: &Path) -> io::Result<File> {
        let file = sys::open_at(CWD, path, READ).map_err(sys::no_follow)?;
        if !file.metadata()?.is_file() {
            return Err(denied("sealed source is not a regular file"));
        }
        Ok(file)
    }

    fn create_entry(&self, path: &Path, directory: bool) -> io::Result<()> {
        entries::create(self, path, directory)
    }

    fn rename_entry(&self, source: &Path, target: &Path) -> io::Result<()> {
        entries::rename(self, source, target)
    }

    fn delete_entry(&self, path: &Path) -> io::Result<()> {
        entries::delete(self, path)
    }

    fn validate_source(&self, path: &Path) -> io::Result<()> {
        self.validate_path(path, false)
    }

    fn identity(&self, file: &File) -> io::Result<FileIdentity> {
        let metadata = file.metadata()?;
        Ok(FileIdentity {
            volume: metadata.dev(),
            file: metadata.ino(),
            length: metadata.size(),
            modified: metadata
                .mtime()
                .wrapping_mul(1_000_000_000)
                .wrapping_add(metadata.mtime_nsec()) as u64,
        })
    }

    fn validate_target(&self, path: &Path) -> io::Result<()> {
        self.validate_path(path, true)
    }

    fn prepare_commit(
        &self,
        staged: &Path,
        target: &Path,
        mode: CommitMode,
        cancellation: &dyn CommitCancellation,
    ) -> io::Result<PreparedCommit> {
        transaction::prepare(staged, target, mode, cancellation)
    }

    fn commit_transaction(&self, transaction: PreparedCommit) -> io::Result<CommitReceipt> {
        transaction::commit(self, transaction)
    }

    fn abort_commit(&self, transaction: PreparedCommit) -> io::Result<()> {
        transaction::abort(transaction)
    }

    fn mark_commit_state(&self, receipt: &CommitReceipt, state: CommitState) -> io::Result<()> {
        transaction::mark_state(receipt, state)
    }

    fn inspect_commit_transactions(
        &self,
        parent: &Path,
        cancellation: &dyn CommitCancellation,
    ) -> io::Result<Vec<CommitRecovery>> {
        transaction::inspect(parent, cancellation)
    }

    fn resume_commit_cleanup(&self, recovery: &CommitRecovery) -> io::Result<Option<CommitReceipt>> {
        transaction::resume(self, recovery)
    }

    /// Compatibility primitive for callers outside document saves: publish a
    /// same-folder stage. A new name never replaces a file created in a race.
    fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
        let (parent, target_name) = sys::split(target)?;
        let (stage_parent, stage_name) = sys::split(staged)?;
        if parent != stage_parent {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "save stage and destination must share a directory",
            ));
        }
        let folder = DirectoryGuard::open(parent)?;
        if existed {
            rustix::fs::renameat(&folder, stage_name, &folder, target_name)?;
        } else {
            sys::rename_no_replace(&folder, stage_name, &folder, target_name)?;
        }
        sys::sync_directory(&folder.directory)
    }
}

impl FilesystemCapability for PosixFileSystem {
    fn report(&self, path: &Path) -> io::Result<CapabilityReport> {
        capability::report(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::FAIL_CLEANUP_BEFORE_MANIFEST;
    use bareline_platform::CleanupResponsibility;
    use std::{os::unix::fs::PermissionsExt, path::PathBuf};

    struct Never;
    impl CommitCancellation for Never {
        fn check(&self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let directory = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("bareline-posix-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).unwrap();
            Self(directory)
        }
        fn entries(&self) -> usize {
            std::fs::read_dir(&self.0).unwrap().count()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn save(target: &Path, bytes: &[u8], mode: CommitMode) -> io::Result<CommitReceipt> {
        let stage = target.with_file_name(".bareline-test-stage.tmp");
        std::fs::write(&stage, bytes).unwrap();
        let prepared = PosixFileSystem.prepare_commit(&stage, target, mode, &Never)?;
        let receipt = PosixFileSystem.commit_transaction(prepared);
        let _ = std::fs::remove_file(&stage);
        receipt
    }

    #[test]
    fn replacement_keeps_displaced_bytes_where_the_receipt_says() {
        let scratch = Scratch::new("replace");
        let target = scratch.0.join("doc.txt");
        std::fs::write(&target, b"prior bytes").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        let before = PosixFileSystem.identity(&File::open(&target).unwrap()).unwrap();

        let mut receipt = save(&target, b"editor bytes", CommitMode::Replace).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"editor bytes");
        assert_eq!(receipt.state, CommitState::Replaced);
        assert_eq!(receipt.cleanup, CleanupResponsibility::Caller);
        let displaced = receipt.displaced.clone().unwrap();
        let proposed = receipt.proposed.clone().unwrap();
        assert_eq!(std::fs::read(&displaced.path).unwrap(), b"prior bytes");
        assert_eq!(std::fs::read(&proposed.path).unwrap(), b"editor bytes");
        assert_eq!(displaced.path.parent(), receipt.journal.as_ref().unwrap().parent());
        assert_eq!(
            PosixFileSystem.identity(&File::open(&target).unwrap()).unwrap(),
            receipt.target
        );
        // The private mode of the replaced file carries over to the new one.
        assert_eq!(std::fs::metadata(&target).unwrap().mode() & 0o777, 0o640);
        match receipt.strategy {
            // An exchange keeps the displaced file object itself.
            SaveStrategy::Transactional => assert_eq!(displaced.identity, before),
            SaveStrategy::RenameReplace => assert_eq!(displaced.identity.length, before.length),
            other => panic!("unexpected strategy {other:?}"),
        }
        assert_eq!(
            bareline_platform::read_commit_state(receipt.journal.as_ref().unwrap()).unwrap(),
            CommitState::Replaced
        );

        receipt
            .cleanup_token
            .as_mut()
            .unwrap()
            .publish_cleanup_authority()
            .unwrap();
        PosixFileSystem
            .mark_commit_state(&receipt, CommitState::CleanupPending)
            .unwrap();
        PosixFileSystem.cleanup_commit(&mut receipt).unwrap();
        assert!(!displaced.path.exists());
        assert_eq!(scratch.entries(), 1, "only the saved file remains");
    }

    #[test]
    fn exchange_capable_local_folder_saves_transactionally() {
        let scratch = Scratch::new("transactional");
        let report = PosixFileSystem.report(&scratch.0.join("doc.txt")).unwrap();
        if report.save != SaveStrategy::Transactional {
            eprintln!("skipped: the temporary folder's filesystem has no exchange rename ({report:?})");
            return;
        }
        assert!(report.notice().is_none());
        let target = scratch.0.join("doc.txt");
        std::fs::write(&target, b"prior").unwrap();
        let receipt = save(&target, b"next", CommitMode::Replace).unwrap();
        assert_eq!(receipt.strategy, SaveStrategy::Transactional);
    }

    #[test]
    fn create_new_never_replaces_and_records_a_conflict() {
        let scratch = Scratch::new("create");
        let target = scratch.0.join("new.txt");
        let stage = scratch.0.join(".bareline-test-stage.tmp");
        std::fs::write(&stage, b"editor bytes").unwrap();
        let prepared = PosixFileSystem
            .prepare_commit(&stage, &target, CommitMode::CreateNew, &Never)
            .unwrap();
        assert!(prepared.displaced_path.is_none());
        let journal = prepared.journal_path.clone().unwrap();
        std::fs::write(&target, b"competing create").unwrap();
        let error = PosixFileSystem.commit_transaction(prepared).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&target).unwrap(), b"competing create");
        assert_eq!(
            bareline_platform::read_commit_state(&journal).unwrap(),
            CommitState::Conflict
        );
        let recovered = PosixFileSystem.inspect_commit_transactions(&scratch.0, &Never).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].state, CommitState::Conflict);
        assert_eq!(std::fs::read(&recovered[0].proposed).unwrap(), b"editor bytes");

        std::fs::remove_file(&target).unwrap();
        let receipt = save(&target, b"created", CommitMode::CreateNew).unwrap();
        assert_eq!(receipt.state, CommitState::Created);
        assert!(receipt.displaced.is_none());
        assert_eq!(std::fs::read(&target).unwrap(), b"created");
    }

    #[test]
    fn hard_linked_target_is_rewritten_in_place_and_both_links_see_it() {
        let scratch = Scratch::new("hard-link");
        let target = scratch.0.join("doc.txt");
        let alias = scratch.0.join("alias.txt");
        std::fs::write(&target, b"prior bytes").unwrap();
        std::fs::hard_link(&target, &alias).unwrap();
        assert_eq!(PosixFileSystem.report(&target).unwrap().save, SaveStrategy::InPlace);
        let inode = std::fs::metadata(&target).unwrap().ino();
        let receipt = save(&target, b"editor bytes", CommitMode::Replace).unwrap();
        assert_eq!(receipt.strategy, SaveStrategy::InPlace);
        assert_eq!(std::fs::read(&target).unwrap(), b"editor bytes");
        assert_eq!(std::fs::read(&alias).unwrap(), b"editor bytes");
        assert_eq!(std::fs::metadata(&target).unwrap().ino(), inode);
        assert_eq!(
            std::fs::read(&receipt.displaced.as_ref().unwrap().path).unwrap(),
            b"prior bytes"
        );
    }

    #[test]
    fn symlinked_target_is_redirected_and_saved_through_the_link() {
        let scratch = Scratch::new("symlink");
        let real = scratch.0.join("real.txt");
        let link = scratch.0.join("link.txt");
        std::fs::write(&real, b"prior bytes").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let report = PosixFileSystem.report(&link).unwrap();
        assert!(report.redirected);
        assert_eq!(report.save, SaveStrategy::InPlace);
        let refused = PosixFileSystem.open_sealed_read(&link).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
        assert!(PosixFileSystem.open_sealed_read(&real).is_ok());

        save(&link, b"editor bytes", CommitMode::Replace).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&real).unwrap(), b"editor bytes");
    }

    #[test]
    fn sealed_read_refuses_fifos_and_folders_without_blocking() {
        let scratch = Scratch::new("fifo");
        let fifo = scratch.0.join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(PosixFileSystem.open_sealed_read(&fifo).is_err());
        assert!(PosixFileSystem.open_sealed_read(&scratch.0).is_err());
    }

    #[test]
    fn abort_removes_the_whole_transaction() {
        let scratch = Scratch::new("abort");
        let target = scratch.0.join("doc.txt");
        let stage = scratch.0.join(".bareline-test-stage.tmp");
        std::fs::write(&target, b"prior").unwrap();
        std::fs::write(&stage, b"editor").unwrap();
        let prepared = PosixFileSystem
            .prepare_commit(&stage, &target, CommitMode::Replace, &Never)
            .unwrap();
        let proposed = prepared.proposed_path.clone().unwrap();
        assert_eq!(std::fs::read(&proposed).unwrap(), b"editor");
        PosixFileSystem.abort_commit(prepared).unwrap();
        assert!(!proposed.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"prior");
        assert_eq!(scratch.entries(), 2, "target and the caller's stage remain");
    }

    #[test]
    fn cancelled_copy_leaves_no_transaction() {
        struct CancelAfterFirstChunk(std::sync::atomic::AtomicUsize);
        impl CommitCancellation for CancelAfterFirstChunk {
            fn check(&self) -> io::Result<()> {
                if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Ok(())
                } else {
                    Err(io::Error::from(io::ErrorKind::Interrupted))
                }
            }
        }
        let scratch = Scratch::new("cancel");
        let stage = scratch.0.join("stage.tmp");
        std::fs::write(&stage, vec![b'x'; 2 * 1024 * 1024]).unwrap();
        let error = PosixFileSystem
            .prepare_commit(
                &stage,
                &scratch.0.join("target.txt"),
                CommitMode::CreateNew,
                &CancelAfterFirstChunk(0.into()),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(scratch.entries(), 1);
    }

    #[test]
    fn interrupted_cleanup_is_rediscovered_and_resumed() {
        let scratch = Scratch::new("resume");
        let target = scratch.0.join("doc.txt");
        std::fs::write(&target, b"prior bytes").unwrap();
        let mut receipt = save(&target, b"editor bytes", CommitMode::Replace).unwrap();
        receipt
            .cleanup_token
            .as_mut()
            .unwrap()
            .publish_cleanup_authority()
            .unwrap();
        PosixFileSystem
            .mark_commit_state(&receipt, CommitState::CleanupPending)
            .unwrap();
        let journal = receipt.journal.clone().unwrap();
        let proposed = receipt.proposed.as_ref().unwrap().path.clone();
        let displaced = receipt.displaced.as_ref().unwrap().path.clone();
        *FAIL_CLEANUP_BEFORE_MANIFEST.lock().unwrap() = Some(journal.clone());
        assert!(PosixFileSystem.cleanup_commit(&mut receipt).is_err());
        assert!(!proposed.exists() && !displaced.exists());
        assert!(bareline_platform::commit_state_path(&journal, CommitState::CleanupPending).exists());
        assert!(journal.parent().unwrap().join("cleanup-authority").exists());
        drop(receipt);

        let recovered = PosixFileSystem.inspect_commit_transactions(&scratch.0, &Never).unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].verified);
        assert_eq!(recovered[0].state, CommitState::CleanupPending);
        assert_eq!(recovered[0].target.as_deref(), Some(target.as_path()));
        let mut resumed = PosixFileSystem.resume_commit_cleanup(&recovered[0]).unwrap().unwrap();
        PosixFileSystem
            .mark_commit_state(&resumed, CommitState::CleanupPending)
            .unwrap();
        PosixFileSystem.cleanup_commit(&mut resumed).unwrap();
        assert!(!journal.parent().unwrap().exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"editor bytes");
    }

    #[test]
    fn restart_discovery_retains_malformed_and_linked_transactions_as_unverified() {
        let scratch = Scratch::new("malformed");
        let transaction = scratch.0.join(".bareline-save-00000000000000000000000000000001");
        std::fs::create_dir(&transaction).unwrap();
        std::fs::write(transaction.join("manifest"), b"partial").unwrap();
        std::fs::write(transaction.join("state.replaced"), vec![b'x'; 256]).unwrap();
        std::fs::write(transaction.join("editor-version"), b"recoverable editor bytes").unwrap();
        let external = Scratch::new("malformed-external");
        std::fs::write(external.0.join("manifest"), b"foreign manifest").unwrap();
        std::os::unix::fs::symlink(
            &external.0,
            scratch.0.join(".bareline-save-00000000000000000000000000000002"),
        )
        .unwrap();

        let mut recovered = PosixFileSystem.inspect_commit_transactions(&scratch.0, &Never).unwrap();
        recovered.sort_by(|a, b| a.journal.cmp(&b.journal));
        assert_eq!(recovered.len(), 2);
        assert!(recovered.iter().all(|recovery| {
            recovery.state == CommitState::Unverified && !recovery.verified && recovery.target.is_none()
        }));
        assert_eq!(
            std::fs::read(&recovered[0].proposed).unwrap(),
            b"recoverable editor bytes"
        );
        assert_eq!(std::fs::read(external.0.join("manifest")).unwrap(), b"foreign manifest");
        let missing = scratch.0.join("missing/child");
        assert!(
            PosixFileSystem
                .inspect_commit_transactions(&missing, &Never)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn read_only_files_open_but_cannot_be_replaced() {
        let scratch = Scratch::new("read-only");
        let path = scratch.0.join("doc.txt");
        std::fs::write(&path, b"read only").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        PosixFileSystem.validate_source(&path).unwrap();
        assert_eq!(
            PosixFileSystem.validate_target(&path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(PosixFileSystem.validate_target(Path::new("relative.txt")).is_err());
        PosixFileSystem.validate_target(&scratch.0.join("new.txt")).unwrap();
    }

    #[test]
    fn guard_binds_to_the_folder_object_not_its_name() {
        let scratch = Scratch::new("guard");
        let folder = scratch.0.join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("child.txt"), b"child").unwrap();
        let guard = PosixFileSystem.guard_directory_handle(&folder).unwrap();
        assert!(guard.is_current());
        // Moving the name away keeps the guarded folder usable through the guard.
        std::fs::rename(&folder, scratch.0.join("moved")).unwrap();
        assert!(!guard.is_current());
        let child = sys::open_at(&guard, "child.txt", READ).unwrap();
        assert_eq!(std::io::read_to_string(child).unwrap(), "child");
        // A folder recreated at the old name is not the guarded one.
        std::fs::create_dir(&folder).unwrap();
        assert!(!guard.is_current());
        assert!(sys::open_at(&guard, "child.txt", READ).is_ok());
        assert!(!folder.join("child.txt").exists());
        // After the guarded folder is removed, nothing can be created through it.
        std::fs::remove_file(scratch.0.join("moved/child.txt")).unwrap();
        std::fs::remove_dir(scratch.0.join("moved")).unwrap();
        assert!(sys::open_at(&guard, "late.txt", sys::CREATE).is_err());
        assert!(PosixFileSystem.guard_directory(&scratch.0.join("absent")).is_err());
        assert!(PosixFileSystem.available_space(&scratch.0).unwrap() > 0);
    }

    #[test]
    fn compatibility_commit_publishes_without_replacing_new_names() {
        let scratch = Scratch::new("compat");
        let stage = scratch.0.join("stage.tmp");
        let target = scratch.0.join("doc.txt");
        std::fs::write(&stage, b"one").unwrap();
        PosixFileSystem.commit(&stage, &target, false).unwrap();
        std::fs::write(&stage, b"two").unwrap();
        assert_eq!(
            PosixFileSystem.commit(&stage, &target, false).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        PosixFileSystem.commit(&stage, &target, true).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"two");
    }

    #[test]
    fn remote_scope_is_this_filesystem() {
        use bareline_platform::{RemoteReadAction, RemoteReadGrant};
        let path = PathBuf::from("/never/contacted.txt");
        let grant =
            RemoteReadGrant::after_consent(path.clone(), RemoteReadAction::Open, Duration::from_secs(1)).unwrap();
        let access = grant.claim(&path, RemoteReadAction::Open, Arc::new(|| false)).unwrap();
        assert!(PosixFileSystem.scoped_remote_read(access).is_ok());
        assert!(PosixFileSystem.check_source_read(&path).is_ok());
    }
}
