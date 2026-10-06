// SPDX-License-Identifier: MPL-2.0
//! One sleeping worker for up to 63 directory watches on one inotify instance;
//! bounded queues and buffers. The same contract as the Windows service: each
//! directory fails and recovers on its own, a failing one is retried with
//! backoff and polled when it exists but cannot be watched (for example when
//! the user's inotify watch limit is exhausted), while the others keep watching.
use bareline_platform::{FilesystemCapability, PathOrigin, PathTrustProvider, StorageKind, WatchEvent, WatchKind};
use bareline_platform_posix::{PosixFilesystemCapability, PosixPathTrustProvider};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags},
    io::Errno,
};
use std::{
    ffi::OsStr,
    io::{self, Write},
    mem::MaybeUninit,
    os::{
        fd::OwnedFd,
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const DEBOUNCE: Duration = Duration::from_millis(50);
/// Quick retries cover a directory that is deleted and recreated; exponential
/// backoff then keeps a dead path cheap.
const RETRY_FAST: Duration = Duration::from_millis(250);
const RETRY_FAST_ATTEMPTS: u32 = 4;
const RETRY_MAX: Duration = Duration::from_secs(8);
/// A directory that exists but cannot be watched is polled: its owners recheck
/// their files on this cadence.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// The most directories one service watches, as on Windows (one wait slot each there).
const MAX_DIRECTORIES: usize = 63;
const QUEUE: usize = 256;

/// The cadence of one service; tests shorten it.
#[derive(Clone, Copy, Debug)]
struct Timing {
    debounce: Duration,
    retry_fast: Duration,
    poll_interval: Duration,
}
const TIMING: Timing = Timing {
    debounce: DEBOUNCE,
    retry_fast: RETRY_FAST,
    poll_interval: POLL_INTERVAL,
};
fn retry_delay(timing: &Timing, failures: u32) -> Duration {
    let doublings = failures.saturating_sub(RETRY_FAST_ATTEMPTS).min(5);
    (timing.retry_fast * (1u32 << doublings)).min(RETRY_MAX)
}
/// Changes inside a watched directory, plus the directory itself going away.
fn watch_flags() -> WatchFlags {
    WatchFlags::CREATE
        | WatchFlags::DELETE
        | WatchFlags::MODIFY
        | WatchFlags::ATTRIB
        | WatchFlags::CLOSE_WRITE
        | WatchFlags::MOVED_FROM
        | WatchFlags::MOVED_TO
        | WatchFlags::DELETE_SELF
        | WatchFlags::MOVE_SELF
        | WatchFlags::ONLYDIR
        | WatchFlags::DONT_FOLLOW
        | WatchFlags::EXCL_UNLINK
}

/// What one inotify record means for the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Record {
    /// A change to an entry of the directory.
    Entry(WatchKind),
    /// The directory was deleted, moved, unmounted or its watch dropped: its
    /// path must be watched afresh.
    Lost,
    /// The kernel queue overflowed; changes to every directory are unknown.
    Overflow,
    /// Nothing the service reports (for example a change to the directory itself).
    Ignored,
}
fn classify(flags: ReadFlags, named: bool) -> Record {
    if flags.contains(ReadFlags::QUEUE_OVERFLOW) {
        Record::Overflow
    } else if flags.intersects(ReadFlags::DELETE_SELF | ReadFlags::MOVE_SELF | ReadFlags::UNMOUNT | ReadFlags::IGNORED)
    {
        Record::Lost
    } else if !named {
        Record::Ignored
    } else if flags.contains(ReadFlags::MOVED_FROM) {
        Record::Entry(WatchKind::RenameFrom)
    } else if flags.contains(ReadFlags::MOVED_TO) {
        Record::Entry(WatchKind::RenameTo)
    } else if flags.contains(ReadFlags::CREATE) {
        Record::Entry(WatchKind::Created)
    } else if flags.contains(ReadFlags::DELETE) {
        Record::Entry(WatchKind::Removed)
    } else if flags.intersects(ReadFlags::MODIFY | ReadFlags::CLOSE_WRITE | ReadFlags::ATTRIB) {
        Record::Entry(WatchKind::Modified)
    } else {
        Record::Ignored
    }
}

#[derive(Clone, Copy)]
struct Backoff {
    failures: u32,
    retry_at: Instant,
    poll_at: Option<Instant>,
}
impl Backoff {
    fn after(timing: &Timing, failures: u32, now: Instant, poll: bool) -> Self {
        Self {
            failures,
            retry_at: now + retry_delay(timing, failures),
            poll_at: poll.then(|| now + timing.poll_interval),
        }
    }
}
enum Slot {
    /// The watch descriptor and the canonical path events are reported under.
    Watching {
        wd: i32,
        path: PathBuf,
    },
    Failed(Backoff),
}
/// Network mounts are never polled: rechecking them can stall on an unreachable
/// server, so their owners wait for the retry instead, as on Windows.
fn is_network(path: &Path) -> bool {
    PosixFilesystemCapability
        .report(path)
        .is_ok_and(|report| report.storage == StorageKind::Network)
}
fn open_directory(inotify: &OwnedFd, path: &Path) -> io::Result<Slot> {
    let canonical = PosixPathTrustProvider.canonicalize(path, PathOrigin::User)?.canonical;
    let wd = inotify::add_watch(inotify, &canonical, watch_flags())?;
    Ok(Slot::Watching { wd, path: canonical })
}
/// A missing directory recovers when it is recreated. Anything else that stops a
/// watch (trust refusal, access, the watch limit) leaves the directory in place,
/// so poll it.
fn open_slot(timing: &Timing, inotify: &OwnedFd, path: &Path, failures: u32, now: Instant) -> Slot {
    match open_directory(inotify, path) {
        Ok(slot) => slot,
        Err(error) => Slot::Failed(Backoff::after(
            timing,
            failures + 1,
            now,
            error.kind() != io::ErrorKind::NotFound && !is_network(path),
        )),
    }
}
fn rescan(directory: &Path) -> WatchEvent {
    WatchEvent {
        directory: directory.into(),
        name: PathBuf::new(),
        kind: WatchKind::RescanNeeded,
    }
}
struct Queue {
    pending: Vec<WatchEvent>,
    debounce: Option<Instant>,
}
impl Queue {
    fn push(&mut self, e: WatchEvent, lost: &AtomicBool, notify: &(dyn Fn() + Send + Sync)) {
        if self.pending.len() == QUEUE {
            self.pending.clear();
            lost.store(true, Ordering::Release);
            notify();
        }
        if e.kind != WatchKind::Modified || self.pending.last() != Some(&e) {
            self.pending.push(e);
        }
        if self.debounce.is_none() {
            self.debounce = Some(Instant::now());
        }
    }
}

/// The worker's state; everything it touches lives on its thread.
struct Worker {
    timing: Timing,
    inotify: OwnedFd,
    slots: Vec<(PathBuf, Slot)>,
    queue: Queue,
    sender: SyncSender<WatchEvent>,
    lost: Arc<AtomicBool>,
    notify: Arc<dyn Fn() + Send + Sync>,
}
impl Worker {
    fn overflowed(&self) {
        self.lost.store(true, Ordering::Release);
        (self.notify)();
    }
    /// Retries and polls the failed directories that are due.
    fn recover(&mut self, now: Instant) {
        let Self {
            timing,
            inotify,
            slots,
            queue,
            lost,
            notify,
            ..
        } = self;
        for (path, slot) in slots.iter_mut() {
            let Slot::Failed(mut backoff) = *slot else {
                continue;
            };
            if backoff.poll_at.is_some_and(|at| now >= at) {
                queue.push(rescan(path), lost, &**notify);
                backoff.poll_at = Some(now + timing.poll_interval);
            }
            *slot = if now < backoff.retry_at {
                Slot::Failed(backoff)
            } else {
                match open_slot(timing, inotify, path, backoff.failures, now) {
                    // Changes made while unwatched are unknown: rescan once.
                    Slot::Watching { wd, path: canonical } => {
                        queue.push(rescan(&canonical), lost, &**notify);
                        Slot::Watching { wd, path: canonical }
                    }
                    // Keep an existing poll cadence instead of restarting it.
                    Slot::Failed(next) => Slot::Failed(Backoff {
                        poll_at: next.poll_at.and(backoff.poll_at).or(next.poll_at),
                        ..next
                    }),
                }
            };
        }
    }
    /// Delivers the debounced batch.
    fn flush(&mut self) {
        if !self.queue.debounce.is_some_and(|t| t.elapsed() >= self.timing.debounce) {
            return;
        }
        for e in self.queue.pending.drain(..) {
            if self.sender.try_send(e).is_err() {
                self.lost.store(true, Ordering::Release);
                (self.notify)();
            }
        }
        self.queue.debounce = None;
        (self.notify)();
    }
    /// How long the worker may sleep before something is due.
    fn timeout(&self) -> Option<Duration> {
        let mut deadline = self.queue.debounce.map(|t| t + self.timing.debounce);
        for (_, slot) in &self.slots {
            if let Slot::Failed(backoff) = slot {
                for at in [Some(backoff.retry_at), backoff.poll_at].into_iter().flatten() {
                    deadline = Some(deadline.map_or(at, |current| current.min(at)));
                }
            }
        }
        deadline.map(|at| {
            at.saturating_duration_since(Instant::now())
                .min(Duration::from_secs(60))
        })
    }
    /// One inotify record.
    fn record(&mut self, wd: i32, flags: ReadFlags, name: Option<&OsStr>, now: Instant) {
        match classify(flags, name.is_some_and(|name| !name.is_empty())) {
            Record::Ignored => {}
            Record::Overflow => self.overflowed(),
            Record::Entry(kind) => {
                let Self {
                    slots,
                    queue,
                    lost,
                    notify,
                    ..
                } = self;
                for (_, slot) in slots.iter() {
                    if let Slot::Watching { wd: watched, path } = slot
                        && *watched == wd
                    {
                        let e = WatchEvent {
                            directory: path.clone(),
                            name: PathBuf::from(name.unwrap_or_default()),
                            kind,
                        };
                        queue.push(e, lost, &**notify);
                    }
                }
            }
            Record::Lost => {
                let Self {
                    timing,
                    inotify,
                    slots,
                    queue,
                    lost,
                    notify,
                    ..
                } = self;
                for (_, slot) in slots.iter_mut() {
                    if let Slot::Watching { wd: watched, path } = slot
                        && *watched == wd
                    {
                        // Only this directory stops watching; it is retried with backoff.
                        if !flags.contains(ReadFlags::IGNORED) {
                            let _ = inotify::remove_watch(&*inotify, wd);
                        }
                        queue.push(rescan(path), lost, &**notify);
                        *slot = Slot::Failed(Backoff::after(timing, 1, now, false));
                    }
                }
            }
        }
    }
    /// Reads every pending record. False when the inotify instance failed.
    fn drain(&mut self, buffer: &mut [MaybeUninit<u8>]) -> bool {
        let now = Instant::now();
        let mut records = Vec::new();
        {
            let mut reader = inotify::Reader::new(&self.inotify, buffer);
            loop {
                match reader.next() {
                    Ok(event) => records.push((
                        event.wd(),
                        event.events(),
                        event
                            .file_name()
                            .map(|name| OsStr::from_bytes(name.to_bytes()).to_owned()),
                    )),
                    Err(Errno::AGAIN) => break,
                    Err(Errno::INTR) => {}
                    Err(_) => return false,
                }
                if reader.is_buffer_empty() && records.len() >= QUEUE * 4 {
                    // Bounded work per wake; the rest is read on the next one.
                    break;
                }
            }
        }
        for (wd, flags, name) in records {
            self.record(wd, flags, name.as_deref(), now);
        }
        true
    }
    /// A broken inotify instance: start over with a fresh one; everything rescans.
    fn reset(&mut self) -> io::Result<()> {
        self.inotify = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
        let now = Instant::now();
        for (path, slot) in &mut self.slots {
            if matches!(slot, Slot::Watching { .. }) {
                self.queue.push(rescan(path), &self.lost, &*self.notify);
                *slot = Slot::Failed(Backoff::after(&self.timing, 1, now, false));
            }
        }
        Ok(())
    }
    fn run(mut self, stop: UnixStream) {
        let mut buffer = vec![MaybeUninit::<u8>::uninit(); 64 * 1024];
        loop {
            self.recover(Instant::now());
            self.flush();
            let timeout = self.timeout().map(|duration| Timespec {
                tv_sec: duration.as_secs() as i64,
                tv_nsec: i64::from(duration.subsec_nanos()),
            });
            let mut fds = [
                PollFd::new(&stop, PollFlags::IN),
                PollFd::new(&self.inotify, PollFlags::IN),
            ];
            match poll(&mut fds, timeout.as_ref()) {
                Ok(_) | Err(Errno::INTR) => {}
                Err(_) => {
                    // A failed wait must not spin; consumers rescan everything.
                    self.overflowed();
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
            if !fds[0].revents().is_empty() {
                return;
            }
            if !fds[1].revents().is_empty() && !self.drain(&mut buffer) {
                self.overflowed();
                if self.reset().is_err() {
                    thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }
}

pub struct LinuxWatchService {
    cancel: UnixStream,
    worker: Option<JoinHandle<()>>,
    receiver: Receiver<WatchEvent>,
    overflow: Arc<AtomicBool>,
}
impl LinuxWatchService {
    pub fn start(paths: Vec<PathBuf>) -> io::Result<Self> {
        Self::start_notifying(paths, Arc::new(|| {}))
    }
    /// Setup does filesystem I/O; invoke on a bounded background worker. A directory
    /// that cannot be watched yet never fails the service; it is retried and polled.
    pub fn start_notifying(paths: Vec<PathBuf>, notify: Arc<dyn Fn() + Send + Sync>) -> io::Result<Self> {
        Self::start_with(paths, notify, TIMING)
    }
    fn start_with(mut paths: Vec<PathBuf>, notify: Arc<dyn Fn() + Send + Sync>, timing: Timing) -> io::Result<Self> {
        paths.sort();
        paths.dedup();
        if paths.is_empty() || paths.len() > MAX_DIRECTORIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "watch requires 1..63 directories",
            ));
        }
        let inotify = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
        let (cancel, stop) = UnixStream::pair()?;
        let overflow = Arc::new(AtomicBool::new(false));
        let lost = overflow.clone();
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<io::Result<()>>(1);
        let worker = thread::Builder::new().name("bareline-watch".into()).spawn(move || {
            let now = Instant::now();
            let slots: Vec<(PathBuf, Slot)> = paths
                .into_iter()
                .map(|path| {
                    let slot = open_slot(&timing, &inotify, &path, 0, now);
                    (path, slot)
                })
                .collect();
            let _ = ready_tx.send(Ok(()));
            Worker {
                timing,
                inotify,
                slots,
                queue: Queue {
                    pending: Vec::new(),
                    debounce: None,
                },
                sender,
                lost,
                notify,
            }
            .run(stop);
        })?;
        if let Err(e) = ready_rx
            .recv()
            .unwrap_or_else(|_| Err(io::Error::other("watch worker failed")))
        {
            let _ = worker.join();
            return Err(e);
        }
        Ok(Self {
            cancel,
            worker: Some(worker),
            receiver,
            overflow,
        })
    }
    pub fn try_recv(&self) -> Option<WatchEvent> {
        if self.overflow.swap(false, Ordering::AcqRel) {
            return Some(WatchEvent {
                directory: PathBuf::new(),
                name: PathBuf::new(),
                kind: WatchKind::RescanNeeded,
            });
        }
        self.receiver.try_recv().ok()
    }
}
impl Drop for LinuxWatchService {
    fn drop(&mut self) {
        let _ = (&self.cancel).write(&[1]);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!("bareline-watch-{label}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            Self(std::fs::canonicalize(root).unwrap())
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    const FAST: Timing = Timing {
        debounce: Duration::from_millis(10),
        retry_fast: Duration::from_millis(20),
        poll_interval: Duration::from_millis(100),
    };
    fn fast(paths: Vec<PathBuf>) -> LinuxWatchService {
        LinuxWatchService::start_with(paths, Arc::new(|| {}), FAST).unwrap()
    }
    /// Waits (hang guard only) for an event matching `wanted`, keeping what came before.
    fn wait_for(service: &LinuxWatchService, seen: &mut Vec<WatchEvent>, wanted: impl Fn(&WatchEvent) -> bool) -> bool {
        wait_within(service, seen, wanted, Duration::from_secs(10))
    }
    fn wait_within(
        service: &LinuxWatchService,
        seen: &mut Vec<WatchEvent>,
        wanted: impl Fn(&WatchEvent) -> bool,
        limit: Duration,
    ) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            match service.try_recv() {
                Some(e) => {
                    let matched = wanted(&e);
                    seen.push(e);
                    if matched {
                        return true;
                    }
                }
                None => thread::sleep(Duration::from_millis(5)),
            }
        }
        false
    }
    fn is(kind: WatchKind, name: &'static str) -> impl Fn(&WatchEvent) -> bool {
        move |e| e.kind == kind && e.name == Path::new(name)
    }

    #[test]
    fn records_map_to_the_windows_vocabulary() {
        assert_eq!(classify(ReadFlags::CREATE, true), Record::Entry(WatchKind::Created));
        assert_eq!(
            classify(ReadFlags::DELETE | ReadFlags::ISDIR, true),
            Record::Entry(WatchKind::Removed)
        );
        assert_eq!(
            classify(ReadFlags::CLOSE_WRITE, true),
            Record::Entry(WatchKind::Modified)
        );
        assert_eq!(classify(ReadFlags::ATTRIB, true), Record::Entry(WatchKind::Modified));
        assert_eq!(
            classify(ReadFlags::MOVED_FROM, true),
            Record::Entry(WatchKind::RenameFrom)
        );
        assert_eq!(classify(ReadFlags::MOVED_TO, true), Record::Entry(WatchKind::RenameTo));
        assert_eq!(classify(ReadFlags::QUEUE_OVERFLOW, false), Record::Overflow);
        for gone in [
            ReadFlags::DELETE_SELF,
            ReadFlags::MOVE_SELF,
            ReadFlags::UNMOUNT,
            ReadFlags::IGNORED,
        ] {
            assert_eq!(classify(gone, false), Record::Lost);
        }
        // A change to the watched directory itself names no entry.
        assert_eq!(classify(ReadFlags::ATTRIB | ReadFlags::ISDIR, false), Record::Ignored);
    }
    #[test]
    fn retry_backoff_is_quick_then_exponential_and_capped() {
        assert_eq!(retry_delay(&TIMING, 1), RETRY_FAST);
        assert_eq!(retry_delay(&TIMING, RETRY_FAST_ATTEMPTS), RETRY_FAST);
        assert_eq!(retry_delay(&TIMING, RETRY_FAST_ATTEMPTS + 1), RETRY_FAST * 2);
        assert_eq!(retry_delay(&TIMING, u32::MAX), RETRY_MAX);
        assert!((1..40).all(|failures| retry_delay(&TIMING, failures) <= retry_delay(&TIMING, failures + 1)));
    }
    #[test]
    fn creates_modifies_deletes_and_renames_are_reported_in_order() {
        let scratch = Scratch::new("events");
        let service = fast(vec![scratch.0.clone()]);
        let mut seen = Vec::new();
        std::fs::write(scratch.0.join("probe"), b"one").unwrap();
        assert!(
            wait_for(&service, &mut seen, is(WatchKind::Created, "probe")),
            "{seen:?}"
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(scratch.0.join("probe"))
            .unwrap();
        file.write_all(b"two").unwrap();
        drop(file);
        assert!(
            wait_for(&service, &mut seen, is(WatchKind::Modified, "probe")),
            "{seen:?}"
        );
        std::fs::rename(scratch.0.join("probe"), scratch.0.join("renamed")).unwrap();
        assert!(
            wait_for(&service, &mut seen, is(WatchKind::RenameTo, "renamed")),
            "{seen:?}"
        );
        // The halves stay ordered: the old name directly precedes the new one.
        let to = seen.len() - 1;
        assert_eq!(seen[to - 1].kind, WatchKind::RenameFrom, "{seen:?}");
        assert_eq!(seen[to - 1].name, Path::new("probe"));
        std::fs::remove_file(scratch.0.join("renamed")).unwrap();
        assert!(
            wait_for(&service, &mut seen, is(WatchKind::Removed, "renamed")),
            "{seen:?}"
        );
        assert!(seen.iter().all(|e| e.directory == scratch.0), "{seen:?}");
        // No event will arrive, so returning at all proves the idle wait was cancelled.
        drop(service);
    }
    #[test]
    fn deleted_directory_is_rearmed_after_recreation() {
        let scratch = Scratch::new("recreate");
        let dir = scratch.0.join("watched");
        std::fs::create_dir(&dir).unwrap();
        let service = fast(vec![dir.clone()]);
        let mut seen = Vec::new();
        std::fs::remove_dir(&dir).unwrap();
        assert!(wait_for(&service, &mut seen, |e| e.kind == WatchKind::RescanNeeded
            && e.directory == dir));
        std::fs::create_dir(&dir).unwrap();
        // Once rearmed it asks for a rescan of the changes it could not see.
        assert!(wait_for(&service, &mut seen, |e| e.kind == WatchKind::RescanNeeded
            && e.directory == dir));
        std::fs::write(dir.join("after"), b"new").unwrap();
        assert!(
            wait_for(&service, &mut seen, is(WatchKind::Created, "after")),
            "watch must recover after directory recreation: {seen:?}"
        );
    }
    #[test]
    fn moved_directory_is_watched_again_at_its_path() {
        let scratch = Scratch::new("moved");
        let (dir, away) = (scratch.0.join("watched"), scratch.0.join("away"));
        std::fs::create_dir(&dir).unwrap();
        let service = fast(vec![dir.clone()]);
        let mut seen = Vec::new();
        std::fs::rename(&dir, &away).unwrap();
        assert!(wait_for(&service, &mut seen, |e| e.kind == WatchKind::RescanNeeded
            && e.directory == dir));
        // Changes in the moved folder no longer belong to the watched path.
        std::fs::write(away.join("elsewhere"), b"").unwrap();
        std::fs::create_dir(&dir).unwrap();
        assert!(wait_for(&service, &mut seen, |e| e.kind == WatchKind::RescanNeeded
            && e.directory == dir));
        std::fs::write(dir.join("here"), b"").unwrap();
        assert!(
            wait_for(&service, &mut seen, is(WatchKind::Created, "here")),
            "{seen:?}"
        );
        assert!(seen.iter().all(|e| e.name != Path::new("elsewhere")), "{seen:?}");
    }
    #[test]
    fn other_directories_keep_watching_when_one_is_deleted() {
        let scratch = Scratch::new("independent");
        let (doomed, kept) = (scratch.0.join("doomed"), scratch.0.join("kept"));
        std::fs::create_dir_all(&doomed).unwrap();
        std::fs::create_dir_all(&kept).unwrap();
        let service = fast(vec![doomed.clone(), kept.clone()]);
        let mut seen = Vec::new();
        std::fs::remove_dir(&doomed).unwrap();
        // Order the proof: the worker has seen this directory fail before the probe.
        assert!(
            wait_for(&service, &mut seen, |e| e.kind == WatchKind::RescanNeeded
                && e.directory == doomed),
            "the deleted directory must request a rescan of itself"
        );
        std::fs::write(kept.join("probe"), b"after").unwrap();
        assert!(
            wait_for(&service, &mut seen, |e| e.name == Path::new("probe")
                && e.directory == kept),
            "a deleted sibling must not stop watching this directory"
        );
    }
    #[test]
    fn unwatchable_directory_is_retried_without_failing_the_service() {
        assert!(LinuxWatchService::start(Vec::new()).is_err());
        let too_many: Vec<PathBuf> = (0..=MAX_DIRECTORIES)
            .map(|n| PathBuf::from(format!("/tmp/{n}")))
            .collect();
        assert!(LinuxWatchService::start(too_many).is_err());
        let scratch = Scratch::new("missing");
        let missing = scratch.0.join("not-yet");
        let service = fast(vec![missing.clone()]);
        let mut seen = Vec::new();
        // A missing directory is not polled; it is watched once it appears.
        std::fs::create_dir(&missing).unwrap();
        assert!(wait_for(&service, &mut seen, |e| e.kind == WatchKind::RescanNeeded
            && e.directory == missing));
        std::fs::write(missing.join("late"), b"").unwrap();
        assert!(
            wait_for(&service, &mut seen, is(WatchKind::Created, "late")),
            "{seen:?}"
        );
        // Dropping joins the worker; a leaked or stuck retry loop would hang here.
        drop(service);
    }
    #[test]
    fn existing_directory_that_cannot_be_watched_is_polled() {
        if rustix::process::geteuid().is_root() {
            // Permissions do not stop the superuser, so there is nothing to poll.
            return;
        }
        let scratch = Scratch::new("polled");
        let closed = scratch.0.join("closed");
        std::fs::create_dir(&closed).unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        let service = fast(vec![closed.clone()]);
        let mut seen = Vec::new();
        for _ in 0..2 {
            assert!(wait_for(&service, &mut seen, |e| e.kind == WatchKind::RescanNeeded
                && e.directory == closed));
        }
        // Once it can be watched again, a retry arms it (when is up to the backoff),
        // and later changes arrive as events rather than polls.
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();
        let armed = (0..20).any(|attempt| {
            let name = format!("open-{attempt}");
            std::fs::write(closed.join(&name), b"").unwrap();
            wait_within(
                &service,
                &mut seen,
                |e| e.kind == WatchKind::Created && e.name == Path::new(&name),
                Duration::from_secs(1),
            )
        });
        assert!(armed, "{seen:?}");
    }
}
