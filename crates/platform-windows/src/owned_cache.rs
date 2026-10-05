// SPDX-License-Identifier: MPL-2.0
use bareline_platform::{CacheDirectoryIdentity, CacheRemovalOutcome, TrustedRead};
use std::{
    fs,
    io::{self, Read, Write},
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::*};

const RECORD: &str = ".bareline-cache-owner.json";
const RETAINED_RECORD: &str = ".bareline-cache-cleanup-retained.json";
const MAX_RECORD: usize = 4096;

pub(crate) fn identity(file: &fs::File) -> io::Result<CacheDirectoryIdentity> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: File owns the handle and the output structure remains valid for the call.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(|error| io::Error::from_raw_os_error(error.code().0 & 0xffff))?;
    Ok(CacheDirectoryIdentity {
        volume: info.dwVolumeSerialNumber as u64,
        file: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    })
}

pub(crate) fn rename_guarded(entry: &fs::File, target: &Path) -> io::Result<()> {
    let target_parent = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "migration target parent required"))?;
    let target_parent_guard = crate::path_trust::WindowsPathTrustProvider.guard_directory(target_parent)?;
    let name: Vec<u16> = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "migration target name required"))?
        .encode_wide()
        .collect();
    if name.is_empty() || name.contains(&0) || name.len() > 32767 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid migration target name",
        ));
    }
    crate::rename::rename(entry, &target_parent_guard.file, &name, false)
}

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
    remove_with_finalize_hook(
        root,
        candidate,
        expected_root,
        expected_candidate,
        proof_name,
        proof_bytes,
        max_entries,
        max_time,
        cancelled,
        &|_, _| Ok(()),
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FinalizePoint {
    CandidateGuardReleased,
    ProofRemoved,
}

#[allow(clippy::too_many_arguments)]
fn remove_with_finalize_hook(
    root: &Path,
    candidate: &Path,
    expected_root: CacheDirectoryIdentity,
    expected_candidate: CacheDirectoryIdentity,
    proof_name: &str,
    proof_bytes: &[u8],
    max_entries: usize,
    max_time: Duration,
    cancelled: &dyn Fn() -> bool,
    finalize_hook: &dyn Fn(FinalizePoint, &Path) -> io::Result<()>,
) -> CacheRemovalOutcome {
    let mut visited = 0;
    let mut retry_authority_retained = true;
    let result = remove_inner(
        root,
        candidate,
        expected_root,
        expected_candidate,
        proof_name,
        proof_bytes,
        max_entries,
        max_time,
        cancelled,
        &mut visited,
        &mut retry_authority_retained,
        finalize_hook,
    );
    CacheRemovalOutcome {
        visited,
        retry_authority_retained,
        result,
    }
}

#[allow(clippy::too_many_arguments)]
fn remove_inner(
    root: &Path,
    candidate: &Path,
    expected_root: CacheDirectoryIdentity,
    expected_candidate: CacheDirectoryIdentity,
    proof_name: &str,
    proof_bytes: &[u8],
    max_entries: usize,
    max_time: Duration,
    cancelled: &dyn Fn() -> bool,
    visited: &mut usize,
    retry_authority_retained: &mut bool,
    finalize_hook: &dyn Fn(FinalizePoint, &Path) -> io::Result<()>,
) -> io::Result<usize> {
    if !root.is_absolute() || candidate.parent() != Some(root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cache candidate escapes its root",
        ));
    }
    let root_guard = crate::path_trust::WindowsPathTrustProvider.guard_directory(root)?;
    if identity(&root_guard.file)? != expected_root {
        return Err(changed());
    }
    let candidate_guard = crate::path_trust::WindowsPathTrustProvider.guard_directory(candidate)?;
    if identity(&candidate_guard.file)? != expected_candidate {
        return Err(changed());
    }

    let started = Instant::now();
    let mut directories = Vec::new();
    let mut files = Vec::new();
    *visited = 1;
    collect(
        candidate,
        Some(candidate_guard),
        max_entries,
        max_time,
        started,
        cancelled,
        visited,
        &mut directories,
        &mut files,
    )?;

    if proof_name != RECORD && proof_name != RETAINED_RECORD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid ownership proof name",
        ));
    }
    let record_path = candidate.join(proof_name);
    let record_bytes = read_bounded(&record_path)?;
    if record_bytes != proof_bytes {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "ownership proof changed",
        ));
    }
    let mut removed = 0;
    for path in files.iter().filter(|path| *path != &record_path) {
        check_budget(started, max_time, cancelled)?;
        fs::remove_file(path)?;
        removed += 1;
    }
    let nested_count = directories.len().saturating_sub(1);
    for (path, guard) in directories[..nested_count].iter_mut() {
        check_budget(started, max_time, cancelled)?;
        guard.take();
        fs::remove_dir(path)?;
        removed += 1;
    }

    check_budget(started, max_time, cancelled)?;
    let (_, candidate_guard) = directories.last_mut().expect("candidate guard collected");
    candidate_guard.take();
    finalize_hook(FinalizePoint::CandidateGuardReleased, candidate)?;
    let final_handle = open_delete_directory(candidate)?;
    if identity(&final_handle)? != expected_candidate {
        return Err(changed());
    }
    fs::remove_file(&record_path)?;
    let disposition = (|| {
        finalize_hook(FinalizePoint::ProofRemoved, candidate)?;
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: the verified no-follow directory handle stays live through disposition.
        unsafe {
            SetFileInformationByHandle(
                HANDLE(final_handle.as_raw_handle()),
                FileDispositionInfo,
                (&info as *const FILE_DISPOSITION_INFO).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
            .map_err(|error| io::Error::from_raw_os_error(error.code().0 & 0xffff))
        }
    })();
    match disposition {
        Ok(()) => {
            drop(final_handle);
            Ok(removed + 2)
        }
        Err(error) => {
            if let Err(restore) = restore_authority(candidate, &record_bytes) {
                *retry_authority_retained = false;
                return Err(io::Error::other(format!(
                    "cache directory removal failed ({error}); cleanup authority restoration failed ({restore})"
                )));
            }
            Err(error)
        }
    }
}

fn open_delete_directory(path: &Path) -> io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .access_mode(DELETE.0 | FILE_READ_ATTRIBUTES.0)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0 | FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)?;
    let attributes = file.metadata()?.file_attributes();
    if attributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0) != 0
        || attributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cache finalization target is linked, offline, or not a directory",
        ));
    }
    Ok(file)
}

#[allow(clippy::too_many_arguments)]
fn collect(
    directory: &Path,
    guard: Option<TrustedRead>,
    max_entries: usize,
    max_time: Duration,
    started: Instant,
    cancelled: &dyn Fn() -> bool,
    visited: &mut usize,
    directories: &mut Vec<(PathBuf, Option<TrustedRead>)>,
    files: &mut Vec<PathBuf>,
) -> io::Result<()> {
    check_budget(started, max_time, cancelled)?;
    if *visited > max_entries {
        return Err(limit());
    }
    let guard = match guard {
        Some(guard) => guard,
        None => crate::path_trust::WindowsPathTrustProvider.guard_directory(directory)?,
    };
    for entry in fs::read_dir(directory)? {
        check_budget(started, max_time, cancelled)?;
        *visited = visited.checked_add(1).ok_or_else(limit)?;
        if *visited > max_entries {
            return Err(limit());
        }
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cache contains a reparse point",
            ));
        }
        if metadata.is_dir() {
            collect(
                &entry.path(),
                None,
                max_entries,
                max_time,
                started,
                cancelled,
                visited,
                directories,
                files,
            )?;
        } else if metadata.is_file() {
            files.push(entry.path());
        } else {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cache contains an unsupported entry",
            ));
        }
    }
    directories.push((directory.to_owned(), Some(guard)));
    Ok(())
}

fn restore_record(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn restore_authority(candidate: &Path, bytes: &[u8]) -> io::Result<()> {
    match restore_record(&candidate.join(RECORD), bytes) {
        Ok(()) => Ok(()),
        Err(primary) => restore_record(&candidate.join(RETAINED_RECORD), bytes)
            .map_err(|retained| io::Error::other(format!("owner record: {primary}; retained receipt: {retained}"))),
    }
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_RECORD as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid ownership record"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(path)?
        .take((MAX_RECORD + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_RECORD {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "ownership record limit"));
    }
    Ok(bytes)
}

fn check_budget(started: Instant, max_time: Duration, cancelled: &dyn Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::new(io::ErrorKind::Interrupted, "cache cleanup cancelled"))
    } else if started.elapsed() >= max_time {
        Err(limit())
    } else {
        Ok(())
    }
}

fn changed() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "cache directory changed")
}

fn limit() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "cache deletion budget exhausted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::windows::fs::OpenOptionsExt,
        process::Command,
        sync::atomic::{AtomicBool, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;

    struct Fixture {
        base: PathBuf,
        junction: Option<PathBuf>,
    }

    impl Fixture {
        fn new() -> Self {
            let base = std::env::temp_dir().join(format!(
                "bareline-cache-guard-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
            ));
            fs::create_dir(&base).unwrap();
            Self { base, junction: None }
        }

        fn tree(&self) -> (PathBuf, PathBuf) {
            let root = self.base.join("Bareline-owned-spill");
            let candidate = root.join(format!("owned-stream-{}-1", std::process::id()));
            fs::create_dir_all(&candidate).unwrap();
            fs::write(candidate.join(RECORD), b"owner-proof").unwrap();
            (root, candidate)
        }

        fn ids(root: &Path, candidate: &Path) -> (CacheDirectoryIdentity, CacheDirectoryIdentity) {
            let root_guard = crate::path_trust::WindowsPathTrustProvider
                .guard_directory(root)
                .unwrap();
            let candidate_guard = crate::path_trust::WindowsPathTrustProvider
                .guard_directory(candidate)
                .unwrap();
            (
                identity(&root_guard.file).unwrap(),
                identity(&candidate_guard.file).unwrap(),
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(junction) = self.junction.take() {
                let _ = fs::remove_dir(junction);
            }
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn nested_junction_is_rejected_and_external_sentinel_survives() {
        let mut fixture = Fixture::new();
        let (root, candidate) = fixture.tree();
        let external = fixture.base.join("external");
        fs::create_dir(&external).unwrap();
        let sentinel = external.join("sentinel");
        fs::write(&sentinel, b"foreign").unwrap();
        let junction = candidate.join("escape");
        let status = Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&external)
            .status()
            .unwrap();
        assert!(status.success(), "junction fixture creation failed");
        fixture.junction = Some(junction);
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        assert!(
            remove(
                &root,
                &candidate,
                root_id,
                candidate_id,
                RECORD,
                b"owner-proof",
                16,
                Duration::from_secs(1),
                &|| false
            )
            .result
            .is_err()
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"foreign");
        assert!(candidate.join(RECORD).is_file());
    }

    #[test]
    fn replaced_root_identity_is_rejected() {
        let fixture = Fixture::new();
        let (root, candidate) = fixture.tree();
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        let original = fixture.base.join("original-root");
        fs::rename(&root, &original).unwrap();
        let replacement = root.join(candidate.file_name().unwrap());
        fs::create_dir_all(&replacement).unwrap();
        fs::write(replacement.join(RECORD), b"replacement-proof").unwrap();
        let sentinel = replacement.join("sentinel");
        fs::write(&sentinel, b"replacement").unwrap();
        assert!(
            remove(
                &root,
                &replacement,
                root_id,
                candidate_id,
                RECORD,
                b"owner-proof",
                16,
                Duration::from_secs(1),
                &|| false
            )
            .result
            .is_err()
        );
        assert_eq!(fs::read(sentinel).unwrap(), b"replacement");
    }

    #[test]
    fn failed_partial_purge_preserves_owner_record() {
        let fixture = Fixture::new();
        let (root, candidate) = fixture.tree();
        let locked = candidate.join("locked");
        fs::write(&locked, b"busy").unwrap();
        let _lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .open(&locked)
            .unwrap();
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        assert!(
            remove(
                &root,
                &candidate,
                root_id,
                candidate_id,
                RECORD,
                b"owner-proof",
                16,
                Duration::from_secs(1),
                &|| false
            )
            .result
            .is_err()
        );
        assert!(candidate.join(RECORD).is_file());
    }

    #[test]
    fn successful_nested_purge_uses_handle_finalization() {
        let fixture = Fixture::new();
        let (root, candidate) = fixture.tree();
        let nested = candidate.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("payload"), b"owned").unwrap();
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        let outcome = remove(
            &root,
            &candidate,
            root_id,
            candidate_id,
            RECORD,
            b"owner-proof",
            16,
            Duration::from_secs(1),
            &|| false,
        );
        assert_eq!(outcome.result.unwrap(), 4);
        assert!(!candidate.exists());
    }

    #[test]
    fn retained_cleanup_receipt_removes_candidate_after_manifest_is_gone() {
        let fixture = Fixture::new();
        let (root, candidate) = fixture.tree();
        fs::remove_file(candidate.join(RECORD)).unwrap();
        fs::write(candidate.join(RETAINED_RECORD), b"retained-proof").unwrap();
        let manifest = candidate.join("manifest.json");
        fs::write(&manifest, b"discarded recovery metadata").unwrap();
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);

        fs::remove_file(manifest).unwrap();
        let outcome = remove(
            &root,
            &candidate,
            root_id,
            candidate_id,
            RETAINED_RECORD,
            b"retained-proof",
            16,
            Duration::from_secs(1),
            &|| false,
        );

        assert!(outcome.retry_authority_retained);
        assert!(outcome.result.is_ok());
        assert!(!candidate.exists());
    }

    #[test]
    fn finalization_replacement_is_rejected_before_proof_removal() {
        let fixture = Fixture::new();
        let (root, candidate) = fixture.tree();
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        let original = root.join("original-candidate");
        let replaced = AtomicBool::new(false);
        let outcome = remove_with_finalize_hook(
            &root,
            &candidate,
            root_id,
            candidate_id,
            RECORD,
            b"owner-proof",
            16,
            Duration::from_secs(1),
            &|| false,
            &|point, path| {
                if point == FinalizePoint::CandidateGuardReleased && !replaced.swap(true, Ordering::AcqRel) {
                    fs::rename(path, &original)?;
                    fs::create_dir(path)?;
                    fs::write(path.join("replacement-sentinel"), b"foreign")?;
                }
                Ok(())
            },
        );
        assert!(outcome.result.is_err());
        assert_eq!(fs::read(candidate.join("replacement-sentinel")).unwrap(), b"foreign");
        assert!(original.join(RECORD).is_file());
        assert!(!candidate.join(RECORD).exists());
    }

    #[test]
    fn failed_disposition_uses_retained_receipt_when_owner_name_is_blocked() {
        let fixture = Fixture::new();
        let (root, initial) = fixture.tree();
        fs::remove_file(initial.join(RECORD)).unwrap();
        bareline_file_io::owned_cache::publish_ownership(
            &initial,
            bareline_file_io::owned_cache::CacheKind::OwnedStream,
            &crate::files::WindowsFileSystem,
        )
        .unwrap();
        let current_pid = std::process::id();
        let dead_pid = u32::MAX;
        let proof = fs::read_to_string(initial.join(RECORD))
            .unwrap()
            .replace(&format!("\"pid\":{current_pid}"), &format!("\"pid\":{dead_pid}"));
        fs::write(initial.join(RECORD), proof.as_bytes()).unwrap();
        let candidate = root.join(format!("owned-stream-{dead_pid}-1"));
        fs::rename(initial, &candidate).unwrap();
        let (root_id, candidate_id) = Fixture::ids(&root, &candidate);
        let outcome = remove_with_finalize_hook(
            &root,
            &candidate,
            root_id,
            candidate_id,
            RECORD,
            proof.as_bytes(),
            16,
            Duration::from_secs(1),
            &|| false,
            &|point, path| {
                if point == FinalizePoint::ProofRemoved {
                    fs::create_dir(path.join(RECORD))?;
                }
                Ok(())
            },
        );
        assert!(outcome.result.is_err());
        assert!(outcome.retry_authority_retained);
        assert!(candidate.join(RETAINED_RECORD).is_file());
        let report = bareline_file_io::owned_cache::sweep(
            &fixture.base,
            &std::collections::HashSet::new(),
            &crate::files::WindowsFileSystem,
            &|| false,
            16,
            Duration::from_secs(1),
        );
        assert_eq!(report.removed, 1);
        assert!(!candidate.exists());
    }
}
