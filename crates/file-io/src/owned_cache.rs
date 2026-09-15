// SPDX-License-Identifier: MPL-2.0
//! Proven-owned temporary cache publication and bounded reclamation.
use bareline_platform::{LocalFileSystem, ProcessLiveness};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const RECORD: &str = ".bareline-cache-owner.json";
const RETAINED_RECORD: &str = ".bareline-cache-cleanup-retained.json";
const VERSION: u32 = 1;
pub const ROOTS: [&str; 5] = [
    "Bareline-owned-spill",
    "Bareline-transcode",
    "Bareline-compare-staging",
    "Bareline-power-staging",
    "Bareline-drag-staging",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheKind {
    ResidentSpill,
    SpillBaseline,
    OwnedSegments,
    OwnedStream,
    Transcode,
}
impl CacheKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::ResidentSpill => "resident-spill",
            Self::SpillBaseline => "spill-baseline",
            Self::OwnedSegments => "owned-segments",
            Self::OwnedStream => "owned-stream",
            Self::Transcode => "bareline-transcode",
        }
    }
}

#[derive(Deserialize, Serialize)]
struct OwnershipRecord {
    version: u32,
    kind: CacheKind,
    directory_identity: String,
    pid: u32,
    process_created: u64,
    retained_recovery: Vec<String>,
}

pub fn publish_ownership(directory: &Path, kind: CacheKind, platform: &dyn LocalFileSystem) -> io::Result<()> {
    let root = directory
        .parent()
        .ok_or_else(|| invalid("cache directory has no root"))?;
    if !registered_root(root) {
        return Ok(());
    }
    if !registered_kind(root, kind) || parse_candidate(directory.file_name(), kind) != Some(std::process::id()) {
        let root_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<non-unicode>");
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "cache root or producer grammar is not registered (root={root_name}, kind={kind:?}, expected-prefix={})",
                kind.prefix()
            ),
        ));
    }
    let owner = platform.cache_process_identity()?;
    if owner.pid != std::process::id() {
        return Err(invalid("cache owner identity does not match this process"));
    }
    let record = OwnershipRecord {
        version: VERSION,
        kind,
        directory_identity: hex(&owner.nonce),
        pid: owner.pid,
        process_created: owner.created,
        retained_recovery: Vec::new(),
    };
    let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    let staged = directory.join(format!("{RECORD}.new"));
    let final_path = directory.join(RECORD);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&staged)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(staged, final_path)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SweepReport {
    pub roots: usize,
    pub candidates: usize,
    pub removed: usize,
    pub removed_entries: usize,
    pub visited_entries: usize,
    pub skipped: BTreeMap<&'static str, usize>,
    pub cancelled: bool,
    pub limit_reached: bool,
    pub authority_restore_failed: bool,
}
impl SweepReport {
    fn skip(&mut self, reason: &'static str) {
        *self.skipped.entry(reason).or_default() += 1;
    }
}

pub fn sweep(
    temp: &Path,
    referenced: &HashSet<PathBuf>,
    platform: &dyn LocalFileSystem,
    cancelled: &dyn Fn() -> bool,
    max_entries: usize,
    max_time: Duration,
) -> SweepReport {
    let mut report = SweepReport::default();
    let started = Instant::now();
    for name in ROOTS {
        if report.cancelled || report.limit_reached || report.authority_restore_failed {
            break;
        }
        if cancelled() {
            report.cancelled = true;
            break;
        }
        if report.candidates.saturating_add(report.visited_entries) >= max_entries || started.elapsed() >= max_time {
            report.limit_reached = true;
            break;
        }
        let root = temp.join(name);
        let Ok(Some(root_lease)) = platform.cache_directory_guard(&root) else {
            if root.exists() {
                report.skip("root-not-plain");
            }
            continue;
        };
        report.roots += 1;
        let Ok(entries) = fs::read_dir(&root) else {
            report.skip("root-unreadable");
            continue;
        };
        for entry in entries.flatten() {
            if report.cancelled || report.limit_reached || report.authority_restore_failed {
                break;
            }
            if cancelled() {
                report.cancelled = true;
                break;
            }
            if report.candidates.saturating_add(report.visited_entries) >= max_entries || started.elapsed() >= max_time
            {
                report.limit_reached = true;
                break;
            }
            report.candidates += 1;
            let candidate = entry.path();
            if referenced.contains(&candidate) {
                report.skip("referenced");
                continue;
            }
            let Ok(Some(candidate_lease)) = platform.cache_directory_guard(&candidate) else {
                report.skip("candidate-not-plain");
                continue;
            };
            let Ok(proof) = read_record(&candidate) else {
                report.skip("unproven");
                continue;
            };
            let record = &proof.record;
            if !registered_kind(&root, record.kind)
                || parse_candidate(candidate.file_name(), record.kind) != Some(record.pid)
            {
                report.skip("grammar-mismatch");
                continue;
            }
            if !record.retained_recovery.is_empty() {
                report.skip("recovery-referenced");
                continue;
            }
            match platform.cache_process_liveness(record.pid, record.process_created) {
                ProcessLiveness::Alive => {
                    report.skip("live");
                    continue;
                }
                ProcessLiveness::Unknown => {
                    report.skip("unknown-liveness");
                    continue;
                }
                ProcessLiveness::Dead => {}
            }
            let consumed = report.candidates.saturating_add(report.visited_entries);
            let remaining_entries = max_entries.saturating_sub(consumed);
            let remaining_time = max_time.saturating_sub(started.elapsed());
            if remaining_entries == 0 || remaining_time.is_zero() {
                report.limit_reached = true;
                break;
            }
            let root_identity = root_lease.identity;
            let candidate_identity = candidate_lease.identity;
            drop(candidate_lease);
            let outcome = platform.remove_owned_cache_directory(
                &root,
                &candidate,
                root_identity,
                candidate_identity,
                proof.name,
                &proof.bytes,
                remaining_entries,
                remaining_time,
                cancelled,
            );
            report.visited_entries = report.visited_entries.saturating_add(outcome.visited);
            let budget_exhausted = report.candidates.saturating_add(report.visited_entries) >= max_entries;
            if !outcome.retry_authority_retained {
                report.skip("authority-restore-failed");
                report.authority_restore_failed = true;
                break;
            }
            match outcome.result {
                Ok(entries) => {
                    report.removed += 1;
                    report.removed_entries += entries;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => report.cancelled = true,
                Err(error) if error.kind() == io::ErrorKind::InvalidData => report.limit_reached = true,
                Err(_) => report.skip("deletion-failed"),
            }
            if budget_exhausted {
                report.limit_reached = true;
            }
        }
    }
    report
}

struct SelectedProof {
    record: OwnershipRecord,
    name: &'static str,
    bytes: Vec<u8>,
}

fn read_record(directory: &Path) -> io::Result<SelectedProof> {
    let mut last_error = io::Error::new(io::ErrorKind::NotFound, "ownership record missing");
    for name in [RECORD, RETAINED_RECORD] {
        match read_record_named(directory, name) {
            Ok(proof) => return Ok(proof),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn read_record_named(directory: &Path, name: &'static str) -> io::Result<SelectedProof> {
    let path = directory.join(name);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
        return Err(invalid("invalid ownership record"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(path)?.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid("ownership record limit"));
    }
    let record: OwnershipRecord = serde_json::from_slice(&bytes).map_err(|_| invalid("invalid ownership record"))?;
    if record.version != VERSION
        || record.directory_identity.len() != 32
        || !record.directory_identity.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("unsupported ownership record"));
    }
    Ok(SelectedProof { record, name, bytes })
}

pub fn registered_root(root: &Path) -> bool {
    root.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| ROOTS.contains(&name))
}

/// Resolve a producer to an exact registered root beside an already registered root.
/// Ambiguous kinds and unregistered callers fail closed without granting authority.
pub fn registered_producer_root(current_root: &Path, kind: CacheKind) -> Option<PathBuf> {
    if !registered_root(current_root) {
        return None;
    }
    if registered_kind(current_root, kind) {
        return Some(current_root.to_path_buf());
    }
    let parent = current_root.parent()?;
    let mut matches = ROOTS
        .into_iter()
        .map(|name| parent.join(name))
        .filter(|root| registered_kind(root, kind));
    let root = matches.next()?;
    matches.next().is_none().then_some(root)
}
fn registered_kind(root: &Path, kind: CacheKind) -> bool {
    match root.file_name().and_then(|name| name.to_str()) {
        Some("Bareline-owned-spill") => matches!(
            kind,
            CacheKind::ResidentSpill | CacheKind::SpillBaseline | CacheKind::OwnedSegments | CacheKind::OwnedStream
        ),
        Some("Bareline-transcode") => kind == CacheKind::Transcode,
        Some("Bareline-compare-staging" | "Bareline-power-staging" | "Bareline-drag-staging") => {
            kind == CacheKind::OwnedStream
        }
        _ => false,
    }
}
fn parse_candidate(name: Option<&std::ffi::OsStr>, kind: CacheKind) -> Option<u32> {
    let name = name?.to_str()?;
    let rest = name.strip_prefix(kind.prefix())?.strip_prefix('-')?;
    let mut pieces = rest.split('-');
    let pid = pieces.next()?.parse().ok()?;
    pieces.next()?.parse::<u64>().ok()?;
    pieces.next().is_none().then_some(pid)
}
fn hex(bytes: &[u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(32);
    for byte in bytes {
        value.push(HEX[(byte >> 4) as usize] as char);
        value.push(HEX[(byte & 15) as usize] as char);
    }
    value
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::{
        CacheDirectoryIdentity, CacheDirectoryLease, CacheProcessIdentity, CacheRemovalOutcome, FileIdentity,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Platform {
        created: u64,
        fail_delete: bool,
        swap_on_remove: bool,
    }
    impl LocalFileSystem for Platform {
        fn cache_process_identity(&self) -> io::Result<CacheProcessIdentity> {
            Ok(CacheProcessIdentity {
                pid: std::process::id(),
                created: self.created,
                nonce: [0x5a; 16],
            })
        }
        fn cache_process_liveness(&self, _: u32, created: u64) -> ProcessLiveness {
            match created {
                7 | 10 => ProcessLiveness::Dead,
                8 => ProcessLiveness::Alive,
                _ => ProcessLiveness::Unknown,
            }
        }
        fn cache_directory_guard(&self, path: &Path) -> io::Result<Option<CacheDirectoryLease>> {
            let metadata = fs::symlink_metadata(path)?;
            Ok(
                (metadata.is_dir() && !metadata.file_type().is_symlink()).then_some(CacheDirectoryLease {
                    path: path.to_path_buf(),
                    identity: CacheDirectoryIdentity {
                        volume: 1,
                        file: path
                            .to_string_lossy()
                            .bytes()
                            .fold(0u64, |hash, byte| hash.wrapping_mul(31).wrapping_add(byte as u64)),
                    },
                    guard: std::sync::Arc::new(()),
                    migration_publisher: None,
                }),
            )
        }
        fn remove_owned_cache_directory(
            &self,
            root: &Path,
            candidate: &Path,
            root_identity: CacheDirectoryIdentity,
            candidate_identity: CacheDirectoryIdentity,
            _: &str,
            _: &[u8],
            max_entries: usize,
            _: Duration,
            cancelled: &dyn Fn() -> bool,
        ) -> CacheRemovalOutcome {
            let mut visited = 0;
            let result = (|| {
                if cancelled() {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                if self.fail_delete {
                    return Err(io::Error::other("injected deletion failure"));
                }
                if self.swap_on_remove {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected directory swap",
                    ));
                }
                if self.cache_directory_guard(root)?.map(|lease| lease.identity) != Some(root_identity)
                    || self.cache_directory_guard(candidate)?.map(|lease| lease.identity) != Some(candidate_identity)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "cache directory changed",
                    ));
                }
                let count = fs::read_dir(candidate)?.count() + 1;
                visited = count;
                if count > max_entries {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "entry budget"));
                }
                fs::remove_dir_all(candidate)?;
                Ok(count)
            })();
            CacheRemovalOutcome {
                visited,
                retry_authority_retained: true,
                result,
            }
        }
        fn identity(&self, _: &fs::File) -> io::Result<FileIdentity> {
            Err(io::ErrorKind::Unsupported.into())
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
            Err(io::ErrorKind::Unsupported.into())
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bareline-owned-cleanup-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn candidate(&self, serial: u64, platform: &Platform) -> PathBuf {
            let root = self.0.join("Bareline-owned-spill");
            fs::create_dir_all(&root).unwrap();
            let path = root.join(format!("owned-stream-{}-{serial}", std::process::id()));
            fs::create_dir(&path).unwrap();
            publish_ownership(&path, CacheKind::OwnedStream, platform).unwrap();
            fs::write(path.join("payload"), b"owned").unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn exact_registry_and_producer_grammar_are_required() {
        let fixture = Fixture::new();
        let platform = Platform {
            created: 7,
            fail_delete: false,
            swap_on_remove: false,
        };
        let foreign = fixture
            .0
            .join("Bareline-foreign")
            .join(format!("owned-stream-{}-1", std::process::id()));
        fs::create_dir_all(&foreign).unwrap();
        publish_ownership(&foreign, CacheKind::OwnedStream, &platform).unwrap();
        assert!(!foreign.join(RECORD).exists());
        let malformed = fixture
            .0
            .join("Bareline-owned-spill")
            .join(format!("owned-stream-{}-1-extra", std::process::id()));
        fs::create_dir_all(&malformed).unwrap();
        assert!(publish_ownership(&malformed, CacheKind::OwnedStream, &platform).is_err());
        let wrong_kind = fixture
            .0
            .join("Bareline-transcode")
            .join(format!("owned-stream-{}-1", std::process::id()));
        fs::create_dir_all(&wrong_kind).unwrap();
        assert!(publish_ownership(&wrong_kind, CacheKind::OwnedStream, &platform).is_err());
    }

    #[test]
    fn producer_root_resolution_requires_exact_registered_authority() {
        let temp = std::env::temp_dir().join("bareline-owned-cache-root-resolution");
        let owned = temp.join("Bareline-owned-spill");
        let transcode = temp.join("Bareline-transcode");
        assert_eq!(
            registered_producer_root(&owned, CacheKind::Transcode),
            Some(transcode.clone())
        );
        assert_eq!(
            registered_producer_root(&transcode, CacheKind::Transcode),
            Some(transcode)
        );
        assert_eq!(
            registered_producer_root(&temp.join("custom-cache"), CacheKind::Transcode),
            None
        );
        assert_eq!(
            registered_producer_root(&temp.join("Bareline-owned-spill-lookalike"), CacheKind::Transcode),
            None
        );
        assert_eq!(
            registered_producer_root(&temp.join("Bareline-transcode"), CacheKind::OwnedStream),
            None
        );
    }

    #[test]
    fn sweep_removes_only_proven_unreferenced_definitely_dead_cache() {
        let fixture = Fixture::new();
        let dead = Platform {
            created: 7,
            fail_delete: false,
            swap_on_remove: false,
        };
        let removed = fixture.candidate(1, &dead);
        let referenced = fixture.candidate(2, &dead);
        let live = fixture.candidate(
            3,
            &Platform {
                created: 8,
                fail_delete: false,
                swap_on_remove: false,
            },
        );
        let reused_pid = fixture.candidate(
            5,
            &Platform {
                created: 11,
                fail_delete: false,
                swap_on_remove: false,
            },
        );
        let recovery = fixture.candidate(6, &dead);
        let mut recovery_record = read_record(&recovery).unwrap().record;
        recovery_record.retained_recovery.push("checkpoint-1".into());
        fs::write(recovery.join(RECORD), serde_json::to_vec(&recovery_record).unwrap()).unwrap();
        let root = fixture.0.join("Bareline-owned-spill");
        let unmarked = root.join(format!("owned-stream-{}-4", std::process::id()));
        fs::create_dir(&unmarked).unwrap();
        let references = HashSet::from([referenced.clone()]);
        let report = sweep(&fixture.0, &references, &dead, &|| false, 32, Duration::from_secs(1));
        assert_eq!(report.removed, 1);
        assert!(!removed.exists());
        assert!(referenced.exists() && live.exists() && reused_pid.exists() && recovery.exists() && unmarked.exists());
        assert_eq!(report.skipped.get("unknown-liveness"), Some(&1));
        assert_eq!(report.skipped.get("referenced"), Some(&1));
        assert_eq!(report.skipped.get("recovery-referenced"), Some(&1));
        assert_eq!(report.skipped.get("unproven"), Some(&1));
    }

    #[test]
    fn unknown_failure_and_cancellation_retain_record_for_retry() {
        let fixture = Fixture::new();
        let unknown = Platform {
            created: 9,
            fail_delete: false,
            swap_on_remove: false,
        };
        let unknown_path = fixture.candidate(1, &unknown);
        let report = sweep(
            &fixture.0,
            &HashSet::new(),
            &unknown,
            &|| false,
            8,
            Duration::from_secs(1),
        );
        assert_eq!(report.skipped.get("unknown-liveness"), Some(&1));
        assert!(unknown_path.join(RECORD).is_file());

        let failing = Platform {
            created: 10,
            fail_delete: true,
            swap_on_remove: false,
        };
        let failed_path = fixture.candidate(2, &failing);
        let report = sweep(
            &fixture.0,
            &HashSet::new(),
            &failing,
            &|| false,
            8,
            Duration::from_secs(1),
        );
        assert!(report.skipped.get("deletion-failed").is_some());
        assert!(failed_path.join(RECORD).is_file());

        let cancelled = AtomicBool::new(true);
        let report = sweep(
            &fixture.0,
            &HashSet::new(),
            &failing,
            &|| cancelled.load(Ordering::Acquire),
            8,
            Duration::from_secs(1),
        );
        assert!(report.cancelled);
        assert!(failed_path.join(RECORD).is_file());
    }

    #[test]
    fn directory_identity_change_retains_owner_record_and_payload() {
        let fixture = Fixture::new();
        let publisher = Platform {
            created: 7,
            fail_delete: false,
            swap_on_remove: false,
        };
        let candidate = fixture.candidate(1, &publisher);
        let swapped = Platform {
            created: 7,
            fail_delete: false,
            swap_on_remove: true,
        };
        let report = sweep(
            &fixture.0,
            &HashSet::new(),
            &swapped,
            &|| false,
            8,
            Duration::from_secs(1),
        );
        assert_eq!(report.skipped.get("deletion-failed"), Some(&1));
        assert!(candidate.join(RECORD).is_file());
        assert!(candidate.join("payload").is_file());
    }

    #[test]
    fn recursive_entry_budget_retains_owner_record() {
        let fixture = Fixture::new();
        let platform = Platform {
            created: 7,
            fail_delete: false,
            swap_on_remove: false,
        };
        let candidate = fixture.candidate(1, &platform);
        fs::write(candidate.join("second-payload"), b"owned").unwrap();
        let report = sweep(
            &fixture.0,
            &HashSet::new(),
            &platform,
            &|| false,
            2,
            Duration::from_secs(1),
        );
        assert!(report.limit_reached);
        assert!(candidate.join(RECORD).is_file());
    }

    #[test]
    fn retained_cleanup_receipt_remains_valid_retry_authority() {
        let fixture = Fixture::new();
        let platform = Platform {
            created: 7,
            fail_delete: false,
            swap_on_remove: false,
        };
        let candidate = fixture.candidate(1, &platform);
        fs::rename(candidate.join(RECORD), candidate.join(RETAINED_RECORD)).unwrap();
        let report = sweep(
            &fixture.0,
            &HashSet::new(),
            &platform,
            &|| false,
            16,
            Duration::from_secs(1),
        );
        assert_eq!(report.removed, 1);
        assert!(!candidate.exists());
    }

    #[test]
    fn invalid_primary_without_valid_fallback_is_retained() {
        let fixture = Fixture::new();
        let platform = Platform {
            created: 7,
            fail_delete: false,
            swap_on_remove: false,
        };
        let root = fixture.0.join("Bareline-owned-spill");
        fs::create_dir(&root).unwrap();
        let candidate = root.join(format!("owned-stream-{}-1", std::process::id()));
        fs::create_dir(&candidate).unwrap();
        fs::write(candidate.join(RECORD), b"{").unwrap();
        fs::write(candidate.join("payload"), b"foreign").unwrap();
        let report = sweep(
            &fixture.0,
            &HashSet::new(),
            &platform,
            &|| false,
            16,
            Duration::from_secs(1),
        );
        assert_eq!(report.skipped.get("unproven"), Some(&1));
        assert_eq!(fs::read(candidate.join("payload")).unwrap(), b"foreign");
    }

    #[test]
    fn deletion_visits_and_later_skips_share_one_entry_cap() {
        let fixture = Fixture::new();
        let platform = Platform {
            created: 7,
            fail_delete: false,
            swap_on_remove: false,
        };
        let removed = fixture.candidate(1, &platform);
        let later_root = fixture.0.join("Bareline-transcode");
        fs::create_dir(&later_root).unwrap();
        for serial in 0..8 {
            fs::create_dir(later_root.join(format!("bareline-transcode-999999-{serial}"))).unwrap();
        }
        let report = sweep(
            &fixture.0,
            &HashSet::new(),
            &platform,
            &|| false,
            7,
            Duration::from_secs(1),
        );
        assert_eq!(report.removed, 1);
        assert!(!removed.exists());
        assert_eq!(report.visited_entries, 3);
        assert_eq!(report.candidates, 4);
        assert_eq!(report.skipped.get("unproven"), Some(&3));
        assert!(report.limit_reached);
        assert!(fs::read_dir(later_root).unwrap().count() > 3);
    }
}
