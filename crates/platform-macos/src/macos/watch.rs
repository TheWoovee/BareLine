// SPDX-License-Identifier: MPL-2.0
//! Directory watching with kqueue, with the Windows service's vocabulary and
//! failure behaviour: one sleeping worker for up to 63 directories, bounded
//! queues, a debounce, per-directory retry with backoff, and polling of a
//! directory that exists but cannot be watched.
//!
//! Each directory is opened `O_EVTONLY` and registered for `EVFILT_VNODE`.
//! A change to its entries wakes the worker, which reads the directory again
//! and reports the difference from its last snapshot (`watch_snapshot`). A
//! write into an existing file does not change the directory, so regular
//! files are watched too, the most recently modified first, up to 64 per
//! directory and a total budget derived from the descriptor limit. When a
//! directory has more files than it may watch, its snapshot is also compared
//! every five seconds so writes to the rest are still reported; a directory
//! too large to snapshot (over 4096 entries) reports RescanNeeded on change.
//!
//! Deleting, renaming or revoking a watched directory ends its watch: it
//! reports RescanNeeded and is reopened with the Windows backoff, quick at
//! first, so a directory that is deleted and recreated recovers.
use crate::watch_snapshot::{Snapshot, Stamp, diff, rescan};
use bareline_platform::{WatchEvent, WatchKind};
use rustix::{
    buffer::spare_capacity,
    event::kqueue::{self, Event, EventFilter, EventFlags, UserDefinedFlags, UserFlags, VnodeEvents},
    fd::{AsFd, AsRawFd, OwnedFd},
    io::Errno,
    process::{Resource, getrlimit},
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::OpenOptions,
    io,
    mem::MaybeUninit,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
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
/// A directory that exists but cannot be watched, or has more files than it
/// may watch, is compared on this cadence.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const MAX_DIRECTORIES: usize = 63;
const QUEUE_LIMIT: usize = 256;
const FILES_PER_DIRECTORY: usize = 64;
/// At most this many file descriptors go to file watches in total, and never
/// more than a quarter of the soft descriptor limit (256 by default for a
/// macOS application).
const FILE_BUDGET_MAX: usize = 256;
/// The `EVFILT_USER` event that stops the worker.
const STOP_IDENT: isize = 1;
const DIRECTORY_EVENTS: VnodeEvents = VnodeEvents::WRITE
    .union(VnodeEvents::DELETE)
    .union(VnodeEvents::RENAME)
    .union(VnodeEvents::REVOKE)
    .union(VnodeEvents::LINK)
    .union(VnodeEvents::EXTEND)
    .union(VnodeEvents::ATTRIBUTES);
const FILE_EVENTS: VnodeEvents = VnodeEvents::WRITE
    .union(VnodeEvents::EXTEND)
    .union(VnodeEvents::ATTRIBUTES)
    .union(VnodeEvents::DELETE)
    .union(VnodeEvents::RENAME);
/// Any of these ends a directory's watch.
const DIRECTORY_GONE: VnodeEvents = VnodeEvents::DELETE
    .union(VnodeEvents::RENAME)
    .union(VnodeEvents::REVOKE);

fn retry_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(RETRY_FAST_ATTEMPTS).min(5);
    (RETRY_FAST * (1u32 << doublings)).min(RETRY_MAX)
}
fn file_budget() -> usize {
    let soft = getrlimit(Resource::Nofile).current.unwrap_or(u64::MAX);
    usize::try_from(soft / 4).unwrap_or(usize::MAX).min(FILE_BUDGET_MAX)
}

/// `O_EVTONLY` asks for event notifications only: the descriptor neither
/// reads the object nor keeps its volume from being unmounted.
fn open_event_only(path: &Path, directory: bool) -> io::Result<OwnedFd> {
    let flags = if directory {
        libc::O_EVTONLY | libc::O_DIRECTORY
    } else {
        libc::O_EVTONLY | libc::O_NOFOLLOW
    };
    Ok(OpenOptions::new().read(true).custom_flags(flags).open(path)?.into())
}

/// Registers `fd` with the queue. The descriptor stays owned by the caller;
/// closing it removes the registration.
fn register(queue: &OwnedFd, fd: &OwnedFd, flags: VnodeEvents) -> io::Result<()> {
    let change = Event::new(
        EventFilter::Vnode {
            vnode: fd.as_raw_fd(),
            flags,
        },
        EventFlags::ADD | EventFlags::CLEAR,
        std::ptr::null_mut(),
    );
    let mut none: [MaybeUninit<Event>; 0] = [];
    // SAFETY: the registered descriptor is owned by the worker's slot and is
    // closed (which drops the registration) before the slot goes away.
    unsafe { kqueue::kevent(queue, &[change], &mut none, Some(Duration::ZERO)) }?;
    Ok(())
}
fn user_event(flags: UserFlags, action: EventFlags) -> Event {
    Event::new(
        EventFilter::User {
            ident: STOP_IDENT,
            flags,
            user_flags: UserDefinedFlags::new(0),
        },
        action,
        std::ptr::null_mut(),
    )
}

struct FileWatch {
    fd: OwnedFd,
    inode: u64,
}

struct Directory {
    path: PathBuf,
    fd: OwnedFd,
    snapshot: Snapshot,
    files: BTreeMap<OsString, FileWatch>,
    /// Set when some regular files are not watched individually.
    poll_at: Option<Instant>,
}
impl Directory {
    fn open(queue: &OwnedFd, path: &Path, budget: usize, now: Instant) -> io::Result<Self> {
        let fd = open_event_only(path, true)?;
        register(queue, &fd, DIRECTORY_EVENTS)?;
        // Registered before the first read, so no change between them is lost.
        let snapshot = Snapshot::read(path)?;
        let mut directory = Self {
            path: path.to_owned(),
            fd,
            snapshot,
            files: BTreeMap::new(),
            poll_at: None,
        };
        directory.watch_files(queue, budget, now);
        Ok(directory)
    }

    /// Watches the most recently modified regular files within `budget`, and
    /// polls when that leaves some unwatched.
    fn watch_files(&mut self, queue: &OwnedFd, budget: usize, now: Instant) {
        let mut regular: Vec<(&OsString, &Stamp)> = self
            .snapshot
            .entries
            .iter()
            .filter(|(_, stamp)| stamp.regular)
            .collect();
        regular.sort_by(|a, b| b.1.modified_ns.cmp(&a.1.modified_ns).then_with(|| a.0.cmp(b.0)));
        let limit = budget.min(FILES_PER_DIRECTORY);
        let wanted: BTreeMap<OsString, u64> = regular
            .iter()
            .take(limit)
            .map(|(name, stamp)| ((*name).clone(), stamp.inode))
            .collect();
        // Closing a descriptor removes its registration.
        self.files
            .retain(|name, watch| wanted.get(name).is_some_and(|inode| *inode == watch.inode));
        for (name, inode) in &wanted {
            if self.files.contains_key(name) {
                continue;
            }
            let opened = open_event_only(&self.path.join(name), false)
                .and_then(|fd| register(queue, &fd, FILE_EVENTS).map(|()| fd));
            // A file that vanished or cannot be opened is left to the snapshot.
            if let Ok(fd) = opened {
                self.files.insert(name.clone(), FileWatch { fd, inode: *inode });
            }
        }
        let unwatched = regular.len() > self.files.len();
        self.poll_at = (self.snapshot.complete && unwatched).then(|| now + POLL_INTERVAL);
    }

    /// Reads the directory again and reports the difference.
    fn refresh(&mut self, queue: &OwnedFd, budget: usize, now: Instant, out: &mut Vec<WatchEvent>) -> io::Result<()> {
        let snapshot = Snapshot::read(&self.path)?;
        out.extend(diff(&self.path, &self.snapshot, &snapshot));
        self.snapshot = snapshot;
        self.watch_files(queue, budget, now);
        Ok(())
    }

    /// A watched file was written: report it and record its new stamp, so the
    /// next directory comparison does not report it again.
    fn file_written(&mut self, name: &OsString, out: &mut Vec<WatchEvent>) {
        if let Ok(metadata) = std::fs::symlink_metadata(self.path.join(name)) {
            self.snapshot.entries.insert(name.clone(), Stamp::of(&metadata));
        }
        out.push(WatchEvent {
            directory: self.path.clone(),
            name: PathBuf::from(name),
            kind: WatchKind::Modified,
        });
    }
}

#[derive(Clone, Copy)]
struct Backoff {
    failures: u32,
    retry_at: Instant,
    poll_at: Option<Instant>,
}
impl Backoff {
    fn after(failures: u32, now: Instant, poll: bool) -> Self {
        Self {
            failures,
            retry_at: now + retry_delay(failures),
            poll_at: poll.then(|| now + POLL_INTERVAL),
        }
    }
}
enum Slot {
    Watching(Directory),
    Failed(Backoff),
}
/// A missing directory recovers when it is recreated. Anything else that stops
/// a watch (permissions, a file where a directory was) leaves the path in
/// place, so it is polled.
fn open_slot(queue: &OwnedFd, path: &Path, failures: u32, budget: usize, now: Instant) -> Slot {
    match Directory::open(queue, path, budget, now) {
        Ok(directory) => Slot::Watching(directory),
        Err(error) => Slot::Failed(Backoff::after(
            failures + 1,
            now,
            error.kind() != io::ErrorKind::NotFound,
        )),
    }
}

struct Queue {
    pending: Vec<WatchEvent>,
    debounce: Option<Instant>,
}
impl Queue {
    fn push(&mut self, event: WatchEvent, lost: &AtomicBool, notify: &(dyn Fn() + Send + Sync)) {
        if self.pending.len() == QUEUE_LIMIT {
            self.pending.clear();
            lost.store(true, Ordering::Release);
            notify();
        }
        if event.kind != WatchKind::Modified || self.pending.last() != Some(&event) {
            self.pending.push(event);
        }
        if self.debounce.is_none() {
            self.debounce = Some(Instant::now());
        }
    }
}

struct Worker {
    queue: Arc<OwnedFd>,
    slots: Vec<(PathBuf, Slot)>,
    budget: usize,
    pending: Queue,
    sender: mpsc::SyncSender<WatchEvent>,
    lost: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    notify: Arc<dyn Fn() + Send + Sync>,
}
impl Worker {
    fn files_in_use(&self, except: usize) -> usize {
        self.slots
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != except)
            .map(|(_, (_, slot))| match slot {
                Slot::Watching(directory) => directory.files.len(),
                Slot::Failed(_) => 0,
            })
            .sum()
    }
    fn push_all(&mut self, events: Vec<WatchEvent>) {
        for event in events {
            self.pending.push(event, &self.lost, &*self.notify);
        }
    }
    /// Ends one directory's watch after a failure; the others keep watching.
    fn fail(&mut self, index: usize, now: Instant) {
        let path = self.slots[index].0.clone();
        self.pending.push(rescan(&path), &self.lost, &*self.notify);
        self.slots[index].1 = Slot::Failed(Backoff::after(1, now, false));
    }
    fn refresh(&mut self, index: usize, now: Instant) {
        let budget = self.budget.saturating_sub(self.files_in_use(index));
        let mut events = Vec::new();
        let healthy = match &mut self.slots[index].1 {
            Slot::Watching(directory) => directory.refresh(&self.queue, budget, now, &mut events).is_ok(),
            Slot::Failed(_) => true,
        };
        self.push_all(events);
        if !healthy {
            self.fail(index, now);
        }
    }

    /// Retries and polls failed directories, and polls partly watched ones.
    fn service_timers(&mut self, now: Instant) {
        for index in 0..self.slots.len() {
            match &self.slots[index].1 {
                Slot::Watching(directory) => {
                    if directory.poll_at.is_some_and(|at| now >= at) {
                        self.refresh(index, now);
                    }
                }
                Slot::Failed(backoff) => {
                    let mut backoff = *backoff;
                    let path = self.slots[index].0.clone();
                    if backoff.poll_at.is_some_and(|at| now >= at) {
                        self.pending.push(rescan(&path), &self.lost, &*self.notify);
                        backoff.poll_at = Some(now + POLL_INTERVAL);
                    }
                    self.slots[index].1 = if now < backoff.retry_at {
                        Slot::Failed(backoff)
                    } else {
                        let budget = self.budget.saturating_sub(self.files_in_use(index));
                        match open_slot(&self.queue, &path, backoff.failures, budget, now) {
                            // Changes made while unwatched are unknown: rescan once.
                            Slot::Watching(directory) => {
                                self.pending.push(rescan(&path), &self.lost, &*self.notify);
                                Slot::Watching(directory)
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
        }
    }

    fn flush(&mut self) {
        if !self.pending.debounce.is_some_and(|at| at.elapsed() >= DEBOUNCE) {
            return;
        }
        for event in self.pending.pending.drain(..) {
            if self.sender.try_send(event).is_err() {
                self.lost.store(true, Ordering::Release);
                (self.notify)();
            }
        }
        self.pending.debounce = None;
        (self.notify)();
    }

    fn deadline(&self) -> Option<Instant> {
        let mut deadline = self.pending.debounce.map(|at| at + DEBOUNCE);
        for (_, slot) in &self.slots {
            let times = match slot {
                Slot::Watching(directory) => [directory.poll_at, None],
                Slot::Failed(backoff) => [Some(backoff.retry_at), backoff.poll_at],
            };
            for at in times.into_iter().flatten() {
                deadline = Some(deadline.map_or(at, |current| current.min(at)));
            }
        }
        deadline
    }

    /// Handles one vnode event; `false` when the stop event arrived.
    fn handle(&mut self, event: &Event, now: Instant) -> bool {
        let (fd, flags) = match event.filter() {
            EventFilter::User { .. } => return false,
            EventFilter::Vnode { vnode, flags } => (vnode, flags),
            _ => return true,
        };
        for index in 0..self.slots.len() {
            let Slot::Watching(directory) = &mut self.slots[index].1 else {
                continue;
            };
            if directory.fd.as_raw_fd() == fd {
                if flags.intersects(DIRECTORY_GONE) {
                    self.fail(index, now);
                } else {
                    self.refresh(index, now);
                }
                return true;
            }
            let file = directory
                .files
                .iter()
                .find(|(_, watch)| watch.fd.as_raw_fd() == fd)
                .map(|(name, _)| name.clone());
            if let Some(name) = file {
                if flags.intersects(VnodeEvents::DELETE | VnodeEvents::RENAME) {
                    // The directory reports the removal or rename itself.
                    self.refresh(index, now);
                } else {
                    let mut events = Vec::new();
                    directory.file_written(&name, &mut events);
                    self.push_all(events);
                }
                return true;
            }
        }
        true
    }

    fn run(mut self, ready: mpsc::SyncSender<io::Result<()>>) {
        let now = Instant::now();
        for index in 0..self.slots.len() {
            let budget = self.budget.saturating_sub(self.files_in_use(index));
            let path = self.slots[index].0.clone();
            self.slots[index].1 = open_slot(&self.queue, &path, 0, budget, now);
        }
        let _ = ready.send(Ok(()));
        let mut events: Vec<Event> = Vec::with_capacity(64);
        loop {
            if self.stop.load(Ordering::Acquire) {
                return;
            }
            self.service_timers(Instant::now());
            self.flush();
            let timeout = self.deadline().map(|at| {
                at.saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(60))
            });
            events.clear();
            // SAFETY: every registered descriptor is owned by a slot and closed
            // (dropping its registration) before the slot is replaced.
            let waited = unsafe { kqueue::kevent(self.queue.as_fd(), &[], spare_capacity(&mut events), timeout) };
            match waited {
                Ok(_) => {}
                Err(Errno::INTR) => continue,
                Err(_) => {
                    // A failed wait must not spin; consumers rescan everything.
                    self.lost.store(true, Ordering::Release);
                    (self.notify)();
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
            let now = Instant::now();
            for event in &events {
                if !self.handle(event, now) {
                    return;
                }
            }
        }
    }
}

pub struct MacWatchService {
    queue: Arc<OwnedFd>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    receiver: Receiver<WatchEvent>,
    overflow: Arc<AtomicBool>,
}

impl MacWatchService {
    pub fn start(paths: Vec<PathBuf>) -> io::Result<Self> {
        Self::start_notifying(paths, Arc::new(|| {}))
    }
    /// Setup does filesystem I/O; call it on a bounded background worker, as
    /// the shell does. A directory that cannot be watched yet never fails the
    /// service; it is retried and polled. `notify` wakes the shell's event
    /// loop whenever events are ready or were lost.
    pub fn start_notifying(mut paths: Vec<PathBuf>, notify: Arc<dyn Fn() + Send + Sync>) -> io::Result<Self> {
        paths.sort();
        paths.dedup();
        if paths.is_empty() || paths.len() > MAX_DIRECTORIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "watch requires 1..63 directories",
            ));
        }
        let queue = Arc::new(kqueue::kqueue()?);
        let mut none: [MaybeUninit<Event>; 0] = [];
        // SAFETY: a user event has no descriptor.
        unsafe {
            kqueue::kevent(
                queue.as_fd(),
                &[user_event(UserFlags::empty(), EventFlags::ADD | EventFlags::CLEAR)],
                &mut none,
                Some(Duration::ZERO),
            )
        }?;
        let stop = Arc::new(AtomicBool::new(false));
        let overflow = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(QUEUE_LIMIT);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<io::Result<()>>(1);
        let worker = Worker {
            queue: queue.clone(),
            slots: paths
                .into_iter()
                .map(|path| (path, Slot::Failed(Backoff::after(0, Instant::now(), false))))
                .collect(),
            budget: file_budget(),
            pending: Queue {
                pending: Vec::new(),
                debounce: None,
            },
            sender,
            lost: overflow.clone(),
            stop: stop.clone(),
            notify,
        };
        let worker = thread::Builder::new()
            .name("bareline-watch".into())
            .spawn(move || worker.run(ready_tx))?;
        if let Err(error) = ready_rx
            .recv()
            .unwrap_or_else(|_| Err(io::Error::other("watch worker failed")))
        {
            let _ = worker.join();
            return Err(error);
        }
        Ok(Self {
            queue,
            stop,
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

impl Drop for MacWatchService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let mut none: [MaybeUninit<Event>; 0] = [];
        // SAFETY: triggering the user event registered at start.
        let _ = unsafe {
            kqueue::kevent(
                self.queue.as_fd(),
                &[user_event(UserFlags::TRIGGER, EventFlags::empty())],
                &mut none,
                Some(Duration::ZERO),
            )
        };
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("bareline-kqueue-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    fn wait_for(service: &MacWatchService, wanted: &dyn Fn(&WatchEvent) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match service.try_recv() {
                Some(event) if wanted(&event) => return true,
                Some(_) => {}
                None => thread::sleep(Duration::from_millis(10)),
            }
        }
        false
    }

    #[test]
    fn retry_backoff_is_quick_then_exponential_and_capped() {
        assert_eq!(retry_delay(1), RETRY_FAST);
        assert_eq!(retry_delay(RETRY_FAST_ATTEMPTS), RETRY_FAST);
        assert_eq!(retry_delay(RETRY_FAST_ATTEMPTS + 1), RETRY_FAST * 2);
        assert_eq!(retry_delay(u32::MAX), RETRY_MAX);
        assert!(file_budget() <= FILE_BUDGET_MAX);
    }

    #[test]
    fn creations_writes_and_renames_are_reported_by_name() {
        let directory = scratch("events");
        std::fs::write(directory.join("existing.txt"), b"one").unwrap();
        let service = MacWatchService::start(vec![directory.clone()]).unwrap();
        std::fs::write(directory.join("probe"), b"new").unwrap();
        assert!(wait_for(&service, &|event| event.name == *"probe"
            && event.kind == WatchKind::Created));
        // A write into an existing file does not touch the directory.
        std::fs::write(directory.join("existing.txt"), b"rewritten").unwrap();
        assert!(wait_for(&service, &|event| event.name == *"existing.txt"
            && event.kind == WatchKind::Modified));
        std::fs::rename(directory.join("probe"), directory.join("renamed")).unwrap();
        assert!(wait_for(&service, &|event| event.name == *"renamed"
            && event.kind == WatchKind::RenameTo));
        // Dropping joins the worker; a stuck wait would hang here.
        drop(service);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn deleted_directory_is_rearmed_after_recreation_and_siblings_keep_watching() {
        let root = scratch("recreate");
        let (doomed, kept) = (root.join("doomed"), root.join("kept"));
        std::fs::create_dir_all(&doomed).unwrap();
        std::fs::create_dir_all(&kept).unwrap();
        let service = MacWatchService::start(vec![doomed.clone(), kept.clone()]).unwrap();
        std::fs::remove_dir(&doomed).unwrap();
        assert!(wait_for(&service, &|event| event.kind == WatchKind::RescanNeeded
            && event.directory == doomed));
        std::fs::write(kept.join("probe"), b"after").unwrap();
        assert!(wait_for(&service, &|event| event.name == *"probe"));
        std::fs::create_dir(&doomed).unwrap();
        // The recreated directory is reopened by the quick retries.
        assert!(wait_for(&service, &|event| event.kind == WatchKind::RescanNeeded
            && event.directory == doomed));
        std::fs::write(doomed.join("after"), b"new").unwrap();
        assert!(wait_for(&service, &|event| event.name == *"after"));
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unwatchable_directories_never_fail_the_service() {
        assert!(MacWatchService::start(Vec::new()).is_err());
        let too_many = (0..64).map(|n| PathBuf::from(format!("/nonexistent/{n}"))).collect();
        assert!(MacWatchService::start(too_many).is_err());
        let service = MacWatchService::start(vec![std::env::temp_dir().join("bareline-kqueue-missing-12345")]).unwrap();
        assert_eq!(service.try_recv(), None);
        drop(service);
    }
}
