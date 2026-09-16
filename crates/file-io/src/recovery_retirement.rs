// SPDX-License-Identifier: MPL-2.0
//! Durable, content-free recovery discard and retained background cleanup.

use crate::paged_recovery::PagedRecovery;
use bareline_platform::{
    LocalFileSystem,
    executor::{BoundedExecutor, WorkKind},
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

type Drain = Box<dyn FnOnce() -> RecoveryAddition + Send + 'static>;

struct CleanupHoldInner;

impl Drop for CleanupHoldInner {
    fn drop(&mut self) {
        retry_pending_cleanup();
    }
}

/// Keeps a tombstoned source available to a live in-process document owner.
#[derive(Clone)]
pub struct CleanupHold(Arc<CleanupHoldInner>);

impl CleanupHold {
    pub fn new() -> Self {
        Self(Arc::new(CleanupHoldInner))
    }
}

impl Default for CleanupHold {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RecoveryOwnership {
    pub recoveries: Vec<PagedRecovery>,
    pub paths: Vec<PathBuf>,
    pub platform: Arc<dyn LocalFileSystem>,
}

pub(crate) struct RecoveryAddition {
    pub recoveries: Vec<PagedRecovery>,
    pub paths: Vec<PathBuf>,
    pub removed_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscardPoll {
    Pending,
    Durable,
    CleanupPending(String),
    TombstoneFailed(String),
}

enum Phase {
    DiscardRequested,
    WorkersDrained,
    Purging,
    Complete,
    CleanupPending(String),
    TombstoneFailed(String),
}

struct State {
    phase: Phase,
    drain: Option<Drain>,
    ownership: Option<RecoveryOwnership>,
    cleanup_holds: Vec<std::sync::Weak<CleanupHoldInner>>,
    running: bool,
}

struct RunGuard<'a> {
    state: &'a Arc<Mutex<State>>,
    notify: &'a Arc<dyn Fn() + Send + Sync>,
    ownership: Option<RecoveryOwnership>,
    finished: bool,
}
impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        warn("Recovery discard tombstone pending: Recovery retirement worker unwound".into());
        let mut current = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(ownership) = self.ownership.take() {
            current.ownership = Some(ownership);
        }
        current.running = false;
        current.phase = Phase::TombstoneFailed("Recovery retirement worker unwound".into());
        drop(current);
        (self.notify)();
    }
}

struct RetainedCleanup {
    receipts: Vec<crate::paged_recovery::CleanupReceipt>,
    holds: Vec<std::sync::Weak<CleanupHoldInner>>,
    platform: Arc<dyn LocalFileSystem>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

struct RetainedDiscard {
    state: Arc<Mutex<State>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

fn retained() -> &'static Mutex<Vec<RetainedCleanup>> {
    static RETAINED: OnceLock<Mutex<Vec<RetainedCleanup>>> = OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(Vec::new()))
}

fn retained_discards() -> &'static Mutex<Vec<RetainedDiscard>> {
    static RETAINED: OnceLock<Mutex<Vec<RetainedDiscard>>> = OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(Vec::new()))
}

fn warnings() -> &'static Mutex<Vec<String>> {
    static WARNINGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    WARNINGS.get_or_init(|| Mutex::new(Vec::new()))
}

fn retain_cleanup(mut cleanup: RetainedCleanup) {
    cleanup
        .receipts
        .sort_by(|left, right| left.candidate().cmp(right.candidate()));
    cleanup.receipts.dedup_by(|left, right| left.same_owner(right));
    let mut retained = retained().lock().unwrap_or_else(|error| error.into_inner());
    if let Some(existing) = retained.iter_mut().find(|existing| {
        (existing.holds.is_empty() && cleanup.holds.is_empty()
            || existing.holds.len() == cleanup.holds.len()
                && existing
                    .holds
                    .iter()
                    .zip(&cleanup.holds)
                    .all(|(left, right)| left.ptr_eq(right)))
            && Arc::ptr_eq(&existing.platform, &cleanup.platform)
            && Arc::ptr_eq(&existing.notify, &cleanup.notify)
    }) {
        existing.receipts.extend(cleanup.receipts);
        existing
            .receipts
            .sort_by(|left, right| left.candidate().cmp(right.candidate()));
        existing.receipts.dedup_by(|left, right| left.same_owner(right));
    } else {
        retained.push(cleanup);
    }
}

#[cfg(test)]
struct CleanupRegistrationHook {
    hold: std::sync::Weak<CleanupHoldInner>,
    entered: std::sync::mpsc::SyncSender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

#[cfg(test)]
fn cleanup_registration_hook() -> &'static Mutex<Option<Arc<CleanupRegistrationHook>>> {
    static HOOK: OnceLock<Mutex<Option<Arc<CleanupRegistrationHook>>>> = OnceLock::new();
    HOOK.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
fn pause_before_cleanup_registration(holds: &[std::sync::Weak<CleanupHoldInner>]) {
    let hook = cleanup_registration_hook()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if let Some(hook) = hook
        && holds.iter().any(|hold| hold.ptr_eq(&hook.hold))
        && hook.entered.try_send(()).is_ok()
    {
        let _ = hook
            .release
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .recv_timeout(Duration::from_secs(5));
    }
}

#[cfg(not(test))]
fn pause_before_cleanup_registration(_: &[std::sync::Weak<CleanupHoldInner>]) {}

fn defer_held_cleanup(cleanup: RetainedCleanup) {
    let holds = cleanup.holds.clone();
    pause_before_cleanup_registration(&holds);
    retain_cleanup(cleanup);
    // Registration precedes this second observation. A final owner drop either
    // sees the receipt itself or happens first and is recovered by this requeue.
    if holds.iter().all(|hold| hold.strong_count() == 0) {
        retry_pending_cleanup();
    }
}

fn warn(message: String) {
    const MAX_WARNINGS: usize = 32;
    let mut warnings = warnings().lock().unwrap_or_else(|error| error.into_inner());
    if warnings.last() == Some(&message) {
        return;
    }
    if warnings.len() == MAX_WARNINGS {
        warnings.remove(0);
    }
    warnings.push(message);
}

pub fn take_cleanup_warning() -> Option<String> {
    warnings().lock().ok().and_then(|mut warnings| warnings.pop())
}

pub fn retry_pending_cleanup() -> usize {
    let cleanups = std::mem::take(&mut *retained().lock().unwrap_or_else(|error| error.into_inner()));
    let discards = std::mem::take(&mut *retained_discards().lock().unwrap_or_else(|error| error.into_inner()));
    let mut count = cleanups.len();
    for cleanup in cleanups {
        schedule_cleanup(cleanup);
    }
    for discard in discards {
        let retry = {
            let mut state = discard.state.lock().unwrap_or_else(|error| error.into_inner());
            if !state.running && matches!(state.phase, Phase::TombstoneFailed(_)) {
                state.phase = Phase::DiscardRequested;
                true
            } else {
                false
            }
        };
        if retry {
            count += 1;
            submit(discard.state, discard.notify);
        } else if discard
            .state
            .lock()
            .is_ok_and(|state| !matches!(state.phase, Phase::Complete | Phase::CleanupPending(_)))
        {
            retain_discard(discard);
        }
    }
    count
}

pub struct DiscardTicket {
    state: Arc<Mutex<State>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

static EXECUTOR: OnceLock<BoundedExecutor> = OnceLock::new();

pub fn executor_stats() -> Option<bareline_platform::executor::ExecutorStats> {
    EXECUTOR.get().map(BoundedExecutor::stats)
}

fn executor() -> &'static BoundedExecutor {
    EXECUTOR.get_or_init(|| BoundedExecutor::new(2, 32, "recovery-retirement"))
}

impl DiscardTicket {
    pub fn request(ownership: RecoveryOwnership, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::request_inner(ownership, None, Vec::new(), notify)
    }

    /// Publish discard intent while an in-process owner such as Undo Close Tab
    /// still needs the tombstoned generation's source bytes.
    pub fn request_with_cleanup_hold(
        ownership: RecoveryOwnership,
        hold: &CleanupHold,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self::request_inner(ownership, None, vec![Arc::downgrade(&hold.0)], notify)
    }

    pub(crate) fn request_after_drain(
        ownership: RecoveryOwnership,
        drain: impl FnOnce() -> RecoveryAddition + Send + 'static,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self::request_inner(ownership, Some(Box::new(drain)), Vec::new(), notify)
    }

    fn request_inner(
        ownership: RecoveryOwnership,
        drain: Option<Drain>,
        cleanup_holds: Vec<std::sync::Weak<CleanupHoldInner>>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let ticket = Self {
            state: Arc::new(Mutex::new(State {
                phase: Phase::DiscardRequested,
                drain,
                ownership: Some(ownership),
                cleanup_holds,
                running: false,
            })),
            notify,
        };
        ticket.submit();
        ticket
    }

    pub fn poll(&self) -> DiscardPoll {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match &state.phase {
            Phase::DiscardRequested | Phase::WorkersDrained => DiscardPoll::Pending,
            Phase::Purging | Phase::Complete => DiscardPoll::Durable,
            Phase::CleanupPending(error) => DiscardPoll::CleanupPending(error.clone()),
            Phase::TombstoneFailed(error) => DiscardPoll::TombstoneFailed(error.clone()),
        }
    }

    /// Retry only after the UI has surfaced the prior failure and invokes discard again.
    pub fn retry(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.running || !matches!(state.phase, Phase::TombstoneFailed(_)) {
                return;
            }
            state.phase = Phase::DiscardRequested;
        }
        self.submit();
    }

    fn submit(&self) {
        submit(self.state.clone(), self.notify.clone());
    }
}

impl Drop for DiscardTicket {
    fn drop(&mut self) {
        if self.state.lock().is_ok_and(|state| {
            matches!(
                state.phase,
                Phase::DiscardRequested | Phase::WorkersDrained | Phase::TombstoneFailed(_)
            )
        }) {
            retain_discard(RetainedDiscard {
                state: self.state.clone(),
                notify: self.notify.clone(),
            });
        }
    }
}

fn submit(state: Arc<Mutex<State>>, notify: Arc<dyn Fn() + Send + Sync>) {
    retain_discard(RetainedDiscard {
        state: state.clone(),
        notify: notify.clone(),
    });
    {
        let mut current = state.lock().unwrap_or_else(|error| error.into_inner());
        if current.running {
            return;
        }
        current.running = true;
    }
    let job_state = state.clone();
    let job_notify = notify.clone();
    let submitted = executor().submit(WorkKind::Maintenance, Box::new(move || run(job_state, job_notify)));
    if let Err(error) = submitted {
        warn(format!("Recovery discard admission pending: {error:?}"));
        let mut current = state.lock().unwrap_or_else(|failure| failure.into_inner());
        current.running = false;
        current.phase = Phase::TombstoneFailed(format!("Recovery cleanup admission failed: {error:?}"));
        drop(current);
        notify();
    }
}

fn retain_discard(discard: RetainedDiscard) {
    let mut retained = retained_discards().lock().unwrap_or_else(|error| error.into_inner());
    if !retained
        .iter()
        .any(|retained| Arc::ptr_eq(&retained.state, &discard.state))
    {
        retained.push(discard);
    }
}

fn run(state: Arc<Mutex<State>>, notify: Arc<dyn Fn() + Send + Sync>) {
    let drain = {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.drain.take()
    };
    let addition = match drain.map(|drain| std::panic::catch_unwind(std::panic::AssertUnwindSafe(drain))) {
        Some(Ok(addition)) => addition,
        Some(Err(_)) => {
            terminal_failure(&state, &notify, "Recovery ownership drain failed".into());
            return;
        }
        None => RecoveryAddition {
            recoveries: Vec::new(),
            paths: Vec::new(),
            removed_paths: Vec::new(),
        },
    };
    let ownership = {
        let mut current = state.lock().unwrap_or_else(|error| error.into_inner());
        current.ownership.take().expect("discard ownership")
    };
    let mut guard = RunGuard {
        state: &state,
        notify: &notify,
        ownership: Some(ownership),
        finished: false,
    };
    let tombstone = {
        let ownership = guard.ownership.as_mut().unwrap();
        ownership.recoveries.extend(addition.recoveries);
        ownership.paths.extend(addition.paths);
        ownership.paths.retain(|path| !addition.removed_paths.contains(path));
        {
            let mut current = state.lock().unwrap_or_else(|error| error.into_inner());
            current.phase = Phase::WorkersDrained;
        }
        ownership.paths.extend(
            ownership
                .recoveries
                .iter()
                .map(|recovery| recovery.directory().to_path_buf()),
        );
        ownership.paths.sort();
        ownership.paths.dedup();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ownership
                .recoveries
                .iter_mut()
                .try_for_each(PagedRecovery::tombstone)
                .and_then(|_| {
                    ownership
                        .paths
                        .iter()
                        .map(|path| crate::paged_recovery::publish_cleanup_receipt(path, ownership.platform.as_ref()))
                        .collect::<Result<Vec<_>, _>>()
                        .map(|receipts| receipts.into_iter().flatten().collect::<Vec<_>>())
                })
        }))
        .unwrap_or_else(|_| Err("Recovery tombstone worker failed".into()))
    };
    let receipts = match tombstone {
        Ok(receipts) => receipts,
        Err(error) => {
            warn(format!("Recovery discard tombstone pending: {error}"));
            let mut current = state.lock().unwrap_or_else(|failure| failure.into_inner());
            current.ownership = guard.ownership.take();
            current.running = false;
            current.phase = Phase::TombstoneFailed(error);
            drop(current);
            guard.finished = true;
            notify();
            return;
        }
    };
    {
        let mut current = state.lock().unwrap_or_else(|error| error.into_inner());
        current.phase = Phase::Purging;
    }
    notify();
    let platform = {
        let ownership = guard.ownership.as_mut().unwrap();
        ownership.recoveries.clear();
        ownership.platform.clone()
    };
    let holds = state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .cleanup_holds
        .clone();
    if holds.iter().any(|hold| hold.strong_count() != 0) {
        defer_held_cleanup(RetainedCleanup {
            receipts,
            holds,
            platform,
            notify: notify.clone(),
        });
        let mut current = state.lock().unwrap_or_else(|error| error.into_inner());
        current.running = false;
        current.phase = Phase::Complete;
        drop(current);
        guard.finished = true;
        prune_retained_discard(&state);
        notify();
        return;
    }
    let mut failure = None;
    let mut unresolved = Vec::new();
    for receipt in receipts {
        let mut removed = false;
        let mut path_failure = None;
        for attempt in 0..3 {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::paged_recovery::purge_receipt(&receipt, platform.as_ref())
            }))
            .unwrap_or_else(|_| Err("Recovery purge worker failed".into()));
            match result {
                Ok(()) => {
                    removed = true;
                    break;
                }
                Err(error) => {
                    path_failure = Some(format!(
                        "Recovery cleanup pending for {}: {error}",
                        receipt.candidate().display()
                    ));
                    if attempt < 2 {
                        std::thread::sleep(Duration::from_millis(25 * (attempt + 1)));
                    }
                }
            }
        }
        if !removed {
            if failure.is_none() {
                failure = path_failure;
            }
            unresolved.push(receipt);
        }
    }
    if let Some(error) = &failure {
        warn(error.clone());
        retain_cleanup(RetainedCleanup {
            receipts: unresolved,
            holds: Vec::new(),
            platform,
            notify: notify.clone(),
        });
    }
    let mut current = state.lock().unwrap_or_else(|error| error.into_inner());
    current.running = false;
    current.phase = failure.map_or(Phase::Complete, Phase::CleanupPending);
    drop(current);
    guard.finished = true;
    prune_retained_discard(&state);
    notify();
}

fn terminal_failure(state: &Arc<Mutex<State>>, notify: &Arc<dyn Fn() + Send + Sync>, error: String) {
    warn(format!("Recovery discard tombstone pending: {error}"));
    let mut current = state.lock().unwrap_or_else(|failure| failure.into_inner());
    current.running = false;
    current.phase = Phase::TombstoneFailed(error);
    drop(current);
    notify();
}

fn prune_retained_discard(state: &Arc<Mutex<State>>) {
    retained_discards()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|retained| !Arc::ptr_eq(&retained.state, state));
}

fn schedule_cleanup(cleanup: RetainedCleanup) {
    if cleanup.holds.iter().any(|hold| hold.strong_count() != 0) {
        defer_held_cleanup(cleanup);
        return;
    }
    let retained_cleanup = Arc::new(Mutex::new(Some(cleanup)));
    let job_cleanup = retained_cleanup.clone();
    let submitted = executor().submit(
        WorkKind::Maintenance,
        Box::new(move || {
            let mut cleanup = job_cleanup
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
                .expect("retained cleanup");
            let platform = cleanup.platform.clone();
            let mut failure = None;
            let mut unresolved = Vec::new();
            for receipt in cleanup.receipts.drain(..) {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::paged_recovery::purge_receipt(&receipt, platform.as_ref())
                }))
                .unwrap_or_else(|_| Err("Recovery cleanup worker failed".into()));
                if let Err(error) = result {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                    unresolved.push(receipt);
                }
            }
            cleanup.receipts = unresolved;
            let notify = cleanup.notify.clone();
            if let Some(error) = failure {
                warn(format!("Recovery cleanup retry pending: {error}"));
                retain_cleanup(cleanup);
            }
            notify();
        }),
    );
    if submitted.is_err()
        && let Some(cleanup) = retained_cleanup
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    {
        warn("Recovery cleanup retry admission pending".into());
        retain_cleanup(cleanup);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::{RecoveryMetadata, RecoveryStatus, RecoveryWriter};
    use bareline_platform::{CacheDirectoryIdentity, CacheDirectoryLease, CacheRemovalOutcome};
    use std::{
        collections::hash_map::DefaultHasher,
        fs,
        hash::{Hash, Hasher},
        io,
        path::Path,
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    #[derive(Default)]
    struct Platform {
        deny_cleanup: AtomicBool,
        panic_cleanup: AtomicBool,
        partial_cleanup: AtomicBool,
    }
    fn directory_identity(path: &Path) -> io::Result<CacheDirectoryIdentity> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not a plain directory"));
        }
        let mut hash = DefaultHasher::new();
        path.hash(&mut hash);
        Ok(CacheDirectoryIdentity {
            volume: 1,
            file: hash.finish(),
        })
    }
    impl LocalFileSystem for Platform {
        fn identity(&self, file: &fs::File) -> io::Result<bareline_platform::FileIdentity> {
            let metadata = file.metadata()?;
            Ok(bareline_platform::FileIdentity {
                volume: 1,
                file: 1,
                length: metadata.len(),
                modified: metadata
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
            })
        }
        fn guard_directory(&self, _: &Path) -> io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn cache_directory_guard(&self, path: &Path) -> io::Result<Option<CacheDirectoryLease>> {
            match directory_identity(path) {
                Ok(identity) => Ok(Some(CacheDirectoryLease {
                    path: path.to_path_buf(),
                    identity,
                    guard: Arc::new(()),
                    migration_publisher: None,
                })),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            }
        }
        fn remove_owned_cache_directory(
            &self,
            root: &Path,
            candidate: &Path,
            expected_root: CacheDirectoryIdentity,
            expected_candidate: CacheDirectoryIdentity,
            proof_name: &str,
            proof_bytes: &[u8],
            max_entries: usize,
            _: Duration,
            _: &dyn Fn() -> bool,
        ) -> CacheRemovalOutcome {
            if self.panic_cleanup.swap(false, Ordering::SeqCst) {
                panic!("injected cleanup unwind");
            }
            let result = (|| {
                if self.partial_cleanup.swap(false, Ordering::SeqCst) {
                    fs::remove_file(candidate.join("manifest.json"))?;
                    self.deny_cleanup.store(true, Ordering::SeqCst);
                }
                if self.deny_cleanup.load(Ordering::SeqCst) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected cleanup denial",
                    ));
                }
                if candidate.parent() != Some(root)
                    || directory_identity(root)? != expected_root
                    || directory_identity(candidate)? != expected_candidate
                    || fs::read(candidate.join(proof_name))? != proof_bytes
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "cleanup identity changed",
                    ));
                }
                let mut entries: Vec<_> = fs::read_dir(candidate)?.collect::<Result<_, _>>()?;
                if entries.len() > max_entries
                    || entries
                        .iter()
                        .any(|entry| !entry.file_type().is_ok_and(|k| k.is_file()))
                {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "cleanup entry limit"));
                }
                entries.sort_by_key(|entry| entry.file_name() == proof_name);
                for entry in entries {
                    fs::remove_file(entry.path())?;
                }
                fs::remove_dir(candidate)?;
                Ok(1)
            })();
            CacheRemovalOutcome {
                visited: 1,
                retry_authority_retained: true,
                result,
            }
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
            fs::rename(staged, target)
        }
        fn available_space(&self, _: &Path) -> io::Result<u64> {
            Ok(1 << 40)
        }
        fn open_sealed_read(&self, path: &Path) -> io::Result<fs::File> {
            fs::File::open(path)
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bareline-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn journal(path: &Path, platform: &Platform) {
        drop(
            RecoveryWriter::create(
                path,
                RecoveryMetadata {
                    original_path: None,
                    source_generation: "test".into(),
                    codec_catalog_version: "test".into(),
                    original_len: 0,
                },
                platform,
            )
            .unwrap(),
        );
    }

    fn wait_for(ticket: &DiscardTicket, predicate: impl Fn(&DiscardPoll) -> bool) -> DiscardPoll {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let outcome = ticket.poll();
            if predicate(&outcome) {
                return outcome;
            }
            assert!(Instant::now() < deadline, "discard stayed at {outcome:?}");
            std::thread::yield_now();
        }
    }

    fn registry_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn final_cleanup_hold_drop_cannot_miss_receipt_registration() {
        let _registry = registry_test_lock().lock().unwrap_or_else(|error| error.into_inner());
        retained().lock().unwrap().clear();
        retained_discards().lock().unwrap().clear();
        warnings().lock().unwrap().clear();
        let platform = Arc::new(Platform::default());
        let root = scratch("discard-cleanup-hold");
        let path = root.join("journal");
        journal(&path, platform.as_ref());
        let source = path.join("source.bin");
        fs::write(&source, b"exact undo source").unwrap();

        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let hold = CleanupHold::new();
        *cleanup_registration_hook().lock().unwrap() = Some(Arc::new(CleanupRegistrationHook {
            hold: Arc::downgrade(&hold.0),
            entered: entered_tx,
            release: Mutex::new(release_rx),
        }));
        let ticket = DiscardTicket::request_with_cleanup_hold(
            RecoveryOwnership {
                recoveries: Vec::new(),
                paths: vec![path.clone()],
                platform,
            },
            &hold,
            Arc::new(|| {}),
        );

        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(ticket.poll(), DiscardPoll::Durable);
        assert_eq!(
            crate::recovery::inspect(&path, &Default::default()).unwrap().status,
            RecoveryStatus::Discarded
        );
        assert_eq!(fs::read(&source).unwrap(), b"exact undo source");

        // Force the former lost-wake order: the final owner retries while the
        // receipt has not yet entered the retained registry.
        *cleanup_registration_hook().lock().unwrap() = None;
        drop(hold);
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while path.exists() {
            assert!(Instant::now() < deadline, "final hold release left cleanup dormant");
            std::thread::yield_now();
        }
        assert_eq!(ticket.poll(), DiscardPoll::Durable);
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn delayed_drain_failed_tombstone_and_retained_cleanup_are_truthful() {
        let _registry = registry_test_lock().lock().unwrap_or_else(|error| error.into_inner());
        retained().lock().unwrap().clear();
        retained_discards().lock().unwrap().clear();
        warnings().lock().unwrap().clear();
        let platform = Arc::new(Platform::default());

        // Reaching the gate proves request returned without running the drain inline.
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let delayed = DiscardTicket::request_after_drain(
            RecoveryOwnership {
                recoveries: Vec::new(),
                paths: Vec::new(),
                platform: platform.clone(),
            },
            move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                RecoveryAddition {
                    recoveries: Vec::new(),
                    paths: Vec::new(),
                    removed_paths: Vec::new(),
                }
            },
            Arc::new(|| {}),
        );
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(delayed.poll(), DiscardPoll::Pending);
        release_tx.send(()).unwrap();
        assert_eq!(
            wait_for(&delayed, |outcome| *outcome == DiscardPoll::Durable),
            DiscardPoll::Durable
        );

        let unwind = scratch("discard-unwind").join("journal");
        journal(&unwind, platform.as_ref());
        let unwound = DiscardTicket::request_after_drain(
            RecoveryOwnership {
                recoveries: Vec::new(),
                paths: vec![unwind.clone()],
                platform: platform.clone(),
            },
            || -> RecoveryAddition { panic!("injected drain unwind") },
            Arc::new(|| {}),
        );
        assert!(matches!(
            wait_for(&unwound, |outcome| matches!(outcome, DiscardPoll::TombstoneFailed(_))),
            DiscardPoll::TombstoneFailed(_)
        ));
        assert_ne!(
            crate::recovery::inspect(&unwind, &Default::default()).unwrap().status,
            RecoveryStatus::Discarded
        );
        drop(unwound);
        assert_eq!(retry_pending_cleanup(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while unwind.exists() {
            assert!(Instant::now() < deadline, "unwound discard lost retry ownership");
            std::thread::yield_now();
        }

        // Failure retains ownership in the ticket; repairing the journal lets the
        // same request retry and finish without resurrecting a separate owner.
        let invalid = scratch("discard-invalid").join("journal");
        fs::create_dir_all(&invalid).unwrap();
        let failed = DiscardTicket::request(
            RecoveryOwnership {
                recoveries: Vec::new(),
                paths: vec![invalid.clone()],
                platform: platform.clone(),
            },
            Arc::new(|| {}),
        );
        assert!(matches!(
            wait_for(&failed, |outcome| matches!(outcome, DiscardPoll::TombstoneFailed(_))),
            DiscardPoll::TombstoneFailed(_)
        ));
        let failed_state = failed.state.clone();
        drop(failed);
        assert_eq!(retry_pending_cleanup(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = failed_state.lock().unwrap();
            if !state.running && matches!(state.phase, Phase::TombstoneFailed(_)) {
                break;
            }
            drop(state);
            assert!(
                Instant::now() < deadline,
                "second tombstone failure did not become terminal"
            );
            std::thread::yield_now();
        }
        fs::remove_dir(&invalid).unwrap();
        journal(&invalid, platform.as_ref());
        assert_eq!(retry_pending_cleanup(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while invalid.exists() {
            assert!(Instant::now() < deadline, "retained discard did not remove journal");
            std::thread::yield_now();
        }

        // A T01 removal can fail after deleting journal content. Retry must use
        // the retained identity/proof receipt instead of trying to reread manifest.json.
        let partial = scratch("discard-partial").join("journal");
        journal(&partial, platform.as_ref());
        platform.partial_cleanup.store(true, Ordering::SeqCst);
        let partial_ticket = DiscardTicket::request(
            RecoveryOwnership {
                recoveries: Vec::new(),
                paths: vec![partial.clone()],
                platform: platform.clone(),
            },
            Arc::new(|| {}),
        );
        assert!(matches!(
            wait_for(&partial_ticket, |outcome| matches!(
                outcome,
                DiscardPoll::CleanupPending(_)
            )),
            DiscardPoll::CleanupPending(_)
        ));
        assert!(!partial.join("manifest.json").exists());
        assert!(partial.join(crate::paged_recovery::CLEANUP_PROOF_NAME).exists());
        platform.deny_cleanup.store(false, Ordering::SeqCst);
        assert_eq!(retry_pending_cleanup(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while partial.exists() {
            assert!(
                Instant::now() < deadline,
                "proof-only retry did not remove partial journal"
            );
            std::thread::yield_now();
        }

        // An unexpected entry deterministically blocks purge after the tombstone.
        // Restart discovery sees Discarded until the retained cleanup actually wins.
        let root = scratch("discard-retained");
        let path = root.join("journal");
        journal(&path, platform.as_ref());
        platform.deny_cleanup.store(true, Ordering::SeqCst);
        let pending = DiscardTicket::request(
            RecoveryOwnership {
                recoveries: Vec::new(),
                paths: vec![path.clone()],
                platform: platform.clone(),
            },
            Arc::new(|| {}),
        );
        assert!(matches!(
            wait_for(&pending, |outcome| matches!(outcome, DiscardPoll::CleanupPending(_))),
            DiscardPoll::CleanupPending(_)
        ));
        assert_eq!(
            crate::recovery::inspect(&path, &Default::default()).unwrap().status,
            RecoveryStatus::Discarded
        );
        assert!(take_cleanup_warning().is_some());
        platform.deny_cleanup.store(false, Ordering::SeqCst);
        platform.panic_cleanup.store(true, Ordering::SeqCst);
        assert_eq!(retry_pending_cleanup(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if take_cleanup_warning().is_some_and(|warning| warning.contains("worker failed")) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "cleanup unwind did not reach terminal warning"
            );
            std::thread::yield_now();
        }
        assert!(path.exists(), "cleanup unwind lost retained ownership");
        assert_eq!(retry_pending_cleanup(), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while path.exists() {
            assert!(Instant::now() < deadline, "retained cleanup did not remove journal");
            std::thread::yield_now();
        }

        let restart = root.join(format!("paged-{}-restart", std::process::id()));
        journal(&restart, platform.as_ref());
        crate::paged_recovery::tombstone_directory(&restart, platform.as_ref()).unwrap();
        let removed =
            crate::paged_recovery::sweep(&root, &Default::default(), &|_| false, usize::MAX, platform.as_ref())
                .unwrap();
        assert_eq!(removed, vec![restart.clone()]);
        assert!(!restart.exists(), "restart janitor retained a discarded draft");
        fs::remove_dir_all(root).unwrap();
    }
}
