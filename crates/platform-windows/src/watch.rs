// SPDX-License-Identifier: MPL-2.0
//! One sleeping worker for up to 63 shared directory watches; bounded queues and buffers.
//! Each directory fails and recovers on its own: a failing one is retried with backoff,
//! and polled when it exists but cannot be watched, while the others keep watching.
use bareline_platform::{PathOrigin, PathTrustProvider, WatchEvent, WatchKind};
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io,
    os::windows::{
        ffi::OsStringExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{IO::*, Threading::*},
};
const DEBOUNCE: Duration = Duration::from_millis(50);
/// Quick retries cover a directory that is deleted and recreated; exponential
/// backoff then keeps a dead path cheap.
const RETRY_FAST: Duration = Duration::from_millis(250);
const RETRY_FAST_ATTEMPTS: u32 = 4;
const RETRY_MAX: Duration = Duration::from_secs(8);
/// A directory that exists but cannot be watched (for example an untrusted reparse
/// path) is polled: its owners recheck their files on this cadence.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
fn err(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0 & 0xffff)
}
fn handle(f: &File) -> HANDLE {
    HANDLE(f.as_raw_handle())
}
fn event() -> io::Result<File> {
    // SAFETY: unnamed event, ownership transferred exactly once to File.
    let h = unsafe { CreateEventW(None, true, false, None) }.map_err(err)?;
    Ok(unsafe { File::from_raw_handle(h.0) })
}
fn retry_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(RETRY_FAST_ATTEMPTS).min(5);
    (RETRY_FAST * (1u32 << doublings)).min(RETRY_MAX)
}
struct Directory {
    file: File,
    event: File,
    pending: Box<OVERLAPPED>,
    bytes: Vec<u32>,
    path: PathBuf,
    armed: bool,
}
impl Directory {
    fn open(path: &Path) -> io::Result<Self> {
        let canonical = crate::WindowsPathTrustProvider
            .canonicalize(path, PathOrigin::User)?
            .canonical;
        let _trust = crate::WindowsPathTrustProvider.open_read(&canonical, PathOrigin::User)?;
        let file = OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY.0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OVERLAPPED.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&canonical)?;
        let event = event()?;
        let mut pending = Box::new(OVERLAPPED::default());
        pending.hEvent = handle(&event);
        let mut d = Directory {
            file,
            event,
            pending,
            bytes: vec![0; 16384],
            path: canonical,
            armed: false,
        };
        d.arm()?;
        Ok(d)
    }
    fn arm(&mut self) -> io::Result<()> {
        // SAFETY: stable boxed OVERLAPPED and aligned fixed-size buffer survive until drained.
        unsafe {
            ResetEvent(handle(&self.event)).map_err(err)?;
            ReadDirectoryChangesW(
                handle(&self.file),
                self.bytes.as_mut_ptr().cast(),
                (self.bytes.len() * 4) as u32,
                false,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_ATTRIBUTES
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE,
                None,
                Some(&mut *self.pending),
                None,
            )
            .map_err(err)?;
            self.armed = true;
            Ok(())
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: cancel and drain before freeing the pending operation's storage.
        if !self.armed {
            return;
        }
        unsafe {
            let _ = CancelIoEx(handle(&self.file), Some(&*self.pending));
            let mut n = 0;
            let _ = GetOverlappedResult(handle(&self.file), &*self.pending, &mut n, true);
        }
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
/// A missing directory recovers when it is recreated. Anything else that stops a
/// watch (trust refusal, access) leaves the directory in place, so poll it; network
/// names are never polled because checking them needs an explicit remote grant.
fn open_slot(path: &Path, failures: u32, now: Instant) -> Slot {
    match Directory::open(path) {
        Ok(directory) => Slot::Watching(directory),
        Err(error) => Slot::Failed(Backoff::after(
            failures + 1,
            now,
            error.kind() != io::ErrorKind::NotFound && !crate::capability::is_network_name(path),
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
        if self.pending.len() == 256 {
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
pub struct WindowsWatchService {
    cancel: Arc<File>,
    worker: Option<JoinHandle<()>>,
    receiver: Receiver<WatchEvent>,
    overflow: Arc<AtomicBool>,
}
impl WindowsWatchService {
    pub fn start(paths: Vec<PathBuf>) -> io::Result<Self> {
        Self::start_notifying(paths, Arc::new(|| {}))
    }
    /// Setup does filesystem I/O; invoke on a bounded background worker. A directory
    /// that cannot be watched yet never fails the service; it is retried and polled.
    pub fn start_notifying(mut paths: Vec<PathBuf>, notify: Arc<dyn Fn() + Send + Sync>) -> io::Result<Self> {
        paths.sort();
        paths.dedup();
        if paths.is_empty() || paths.len() > 63 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "watch requires 1..63 directories",
            ));
        }
        let cancel = Arc::new(event()?);
        let stop = cancel.clone();
        let overflow = Arc::new(AtomicBool::new(false));
        let lost = overflow.clone();
        let (sender, receiver) = mpsc::sync_channel(256);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<io::Result<()>>(1);
        let worker = thread::Builder::new().name("bareline-watch".into()).spawn(move || {
            let now = Instant::now();
            let mut slots: Vec<(PathBuf, Slot)> = paths
                .into_iter()
                .map(|path| {
                    let slot = open_slot(&path, 0, now);
                    (path, slot)
                })
                .collect();
            let _ = ready_tx.send(Ok(()));
            let mut queue = Queue {
                pending: Vec::new(),
                debounce: None,
            };
            let mut rotation = 0usize;
            loop {
                let now = Instant::now();
                for (path, slot) in &mut slots {
                    let Slot::Failed(mut backoff) = *slot else {
                        continue;
                    };
                    if backoff.poll_at.is_some_and(|at| now >= at) {
                        queue.push(rescan(path), &lost, &*notify);
                        backoff.poll_at = Some(now + POLL_INTERVAL);
                    }
                    *slot = if now < backoff.retry_at {
                        Slot::Failed(backoff)
                    } else {
                        match open_slot(path, backoff.failures, now) {
                            // Changes made while unwatched are unknown: rescan once.
                            Slot::Watching(d) => {
                                queue.push(rescan(&d.path), &lost, &*notify);
                                Slot::Watching(d)
                            }
                            // Keep an existing poll cadence instead of restarting it.
                            Slot::Failed(next) => Slot::Failed(Backoff {
                                poll_at: next.poll_at.and(backoff.poll_at).or(next.poll_at),
                                ..next
                            }),
                        }
                    };
                }
                if queue.debounce.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
                    for e in queue.pending.drain(..) {
                        if sender.try_send(e).is_err() {
                            lost.store(true, Ordering::Release);
                            notify();
                        }
                    }
                    queue.debounce = None;
                    notify();
                }
                let mut deadline = queue.debounce.map(|t| t + DEBOUNCE);
                for (_, slot) in &slots {
                    if let Slot::Failed(backoff) = slot {
                        for at in [Some(backoff.retry_at), backoff.poll_at].into_iter().flatten() {
                            deadline = Some(deadline.map_or(at, |current| current.min(at)));
                        }
                    }
                }
                let timeout = deadline.map_or(INFINITE, |at| {
                    at.saturating_duration_since(Instant::now()).as_millis().min(60_000) as u32
                });
                let watching: Vec<(usize, HANDLE)> = slots
                    .iter()
                    .enumerate()
                    .filter_map(|(index, (_, slot))| match slot {
                        Slot::Watching(d) => Some((index, handle(&d.event))),
                        Slot::Failed(_) => None,
                    })
                    .collect();
                let handles: Vec<_> = std::iter::once(handle(&stop))
                    .chain(watching.iter().map(|(_, signal)| *signal))
                    .collect();
                let result = unsafe { WaitForMultipleObjects(&handles, false, timeout) };
                if result == WAIT_OBJECT_0 {
                    return;
                }
                if result == WAIT_TIMEOUT {
                    continue;
                }
                if result.0.wrapping_sub(WAIT_OBJECT_0.0 + 1) as usize >= watching.len() {
                    // A failed wait must not spin; consumers rescan everything.
                    lost.store(true, Ordering::Release);
                    notify();
                    if unsafe { WaitForSingleObject(handle(&stop), 250) } == WAIT_OBJECT_0 {
                        return;
                    }
                    continue;
                }
                // Service every signalled directory once per wake, starting from a
                // rotating slot, so a busy folder cannot starve the others.
                let now = Instant::now();
                for step in 0..watching.len() {
                    let (index, signal) = watching[rotation.wrapping_add(step) % watching.len()];
                    if unsafe { WaitForSingleObject(signal, 0) } != WAIT_OBJECT_0 {
                        continue;
                    }
                    let Slot::Watching(d) = &mut slots[index].1 else {
                        continue;
                    };
                    let mut count = 0;
                    let completion = unsafe { GetOverlappedResult(handle(&d.file), &*d.pending, &mut count, false) };
                    d.armed = false;
                    let healthy = completion.is_ok() && {
                        let bytes =
                            unsafe { std::slice::from_raw_parts(d.bytes.as_ptr().cast::<u8>(), count as usize) };
                        for e in parse(&d.path, bytes) {
                            queue.push(e, &lost, &*notify);
                        }
                        d.arm().is_ok()
                    };
                    if !healthy {
                        // Only this directory stops watching; it is retried with backoff.
                        queue.push(rescan(&d.path), &lost, &*notify);
                        slots[index].1 = Slot::Failed(Backoff::after(1, now, false));
                    }
                }
                rotation = rotation.wrapping_add(1);
            }
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
impl Drop for WindowsWatchService {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(handle(&self.cancel));
        }
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
#[allow(clippy::chunks_exact_to_as_chunks)] // Preserve Rust 1.85 compatibility.
fn parse(directory: &std::path::Path, bytes: &[u8]) -> Vec<WatchEvent> {
    let rescan = || {
        vec![WatchEvent {
            directory: directory.into(),
            name: PathBuf::new(),
            kind: WatchKind::RescanNeeded,
        }]
    };
    if bytes.is_empty() {
        return rescan();
    }
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        if bytes.len() - pos < 12 {
            return rescan();
        }
        let u = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let next = u(pos);
        let action = u(pos + 4);
        let size = u(pos + 8);
        if size % 2 != 0 || size > bytes.len() - pos - 12 {
            return rescan();
        }
        let name: Vec<u16> = bytes[pos + 12..pos + 12 + size]
            .chunks_exact(2)
            .map(|v| u16::from_le_bytes([v[0], v[1]]))
            .collect();
        let kind = match action {
            1 => WatchKind::Created,
            2 => WatchKind::Removed,
            3 => WatchKind::Modified,
            4 => WatchKind::RenameFrom,
            5 => WatchKind::RenameTo,
            _ => return rescan(),
        };
        let e = WatchEvent {
            directory: directory.into(),
            name: PathBuf::from(OsString::from_wide(&name)),
            kind,
        };
        if kind != WatchKind::Modified || out.last() != Some(&e) {
            out.push(e);
        }
        if next == 0 {
            break;
        }
        if next < 12 + size || next > bytes.len() - pos {
            return rescan();
        }
        pos += next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_and_overflow_request_rescan() {
        for b in [vec![], vec![0; 11], vec![255; 16]] {
            assert_eq!(parse(std::path::Path::new("x"), &b)[0].kind, WatchKind::RescanNeeded);
        }
    }
    #[test]
    fn watch_changes_and_idle_cancellation() {
        let dir = std::env::temp_dir().join(format!("bareline-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let service = WindowsWatchService::start(vec![dir.clone()]).unwrap();
        std::fs::write(dir.join("probe"), b"one").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            if let Some(e) = service.try_recv() {
                if e.name == PathBuf::from("probe") {
                    found = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(found);
        // No event will arrive, so returning at all proves the idle wait was
        // cancelled; its latency is not asserted against the wall clock (QA-07).
        drop(service);
        std::fs::remove_file(dir.join("probe")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn deleted_directory_is_rearmed_after_recreation() {
        let dir = std::env::temp_dir().join(format!("bareline-watch-recreate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let service = WindowsWatchService::start(vec![dir.clone()]).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        std::fs::create_dir(&dir).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        std::fs::write(dir.join("after"), b"new").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            if service.try_recv().is_some_and(|e| e.name == PathBuf::from("after")) {
                found = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        drop(service);
        std::fs::remove_file(dir.join("after")).unwrap();
        std::fs::remove_dir(dir).unwrap();
        assert!(found, "watch must recover its handle after directory recreation");
    }
    #[test]
    fn unwatchable_directory_is_retried_without_failing_the_service() {
        assert!(WindowsWatchService::start(Vec::new()).is_err());
        let service = WindowsWatchService::start(vec![
            std::env::temp_dir().join("bareline-watch-missing-directory-12345"),
        ])
        .unwrap();
        // Dropping joins the worker; a leaked or stuck retry loop would hang here.
        drop(service);
    }
    #[test]
    fn retry_backoff_is_quick_then_exponential_and_capped() {
        assert_eq!(retry_delay(1), RETRY_FAST);
        assert_eq!(retry_delay(RETRY_FAST_ATTEMPTS), RETRY_FAST);
        assert_eq!(retry_delay(RETRY_FAST_ATTEMPTS + 1), RETRY_FAST * 2);
        assert_eq!(retry_delay(u32::MAX), RETRY_MAX);
        assert!((1..40).all(|failures| retry_delay(failures) <= retry_delay(failures + 1)));
    }
    #[test]
    fn other_directories_keep_watching_when_one_is_deleted() {
        let root = std::env::temp_dir().join(format!("bareline-watch-independent-{}", std::process::id()));
        let (doomed, kept) = (root.join("doomed"), root.join("kept"));
        std::fs::create_dir_all(&doomed).unwrap();
        std::fs::create_dir_all(&kept).unwrap();
        let service = WindowsWatchService::start(vec![doomed.clone(), kept.clone()]).unwrap();
        let wait_for = |wanted: &dyn Fn(&WatchEvent) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                match service.try_recv() {
                    Some(e) if wanted(&e) => return true,
                    Some(_) => {}
                    None => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            }
            false
        };
        std::fs::remove_dir(&doomed).unwrap();
        // Order the proof: the worker has seen this directory fail before the probe.
        let failed = wait_for(&|e: &WatchEvent| {
            e.kind == WatchKind::RescanNeeded && e.directory.file_name() == Some(std::ffi::OsStr::new("doomed"))
        });
        std::fs::write(kept.join("probe"), b"after").unwrap();
        let delivered = wait_for(&|e: &WatchEvent| e.name == PathBuf::from("probe"));
        drop(service);
        std::fs::remove_dir_all(&root).unwrap();
        assert!(failed, "the deleted directory must request a rescan of itself");
        assert!(delivered, "a deleted sibling must not stop watching this directory");
    }
}

impl WindowsWatchService {
    /// Consent text is constructed without opening, classifying, or querying the destination.
    /// `owner` keeps the prompt modal to the editor window (UI-18).
    pub fn confirm_remote_read(
        owner: Option<windows::Win32::Foundation::HWND>,
        path: &std::path::Path,
        action: bareline_platform::RemoteReadAction,
    ) -> bool {
        use windows::{Win32::UI::WindowsAndMessaging::*, core::PCWSTR};
        let action = match action {
            bareline_platform::RemoteReadAction::Open => "open",
            bareline_platform::RemoteReadAction::Reload => "reload",
            bareline_platform::RemoteReadAction::Follow => "follow",
        };
        let destination = path.to_string_lossy();
        if destination.len() > 8192 || destination.contains('\0') {
            return false;
        }
        let destination: String = destination.chars().flat_map(char::escape_debug).collect();
        let text = format!(
            "Allow Bareline to {action} this exact remote file?\n\n{destination}\n\nWindows may send your account credentials to the remote host. This read-only permission applies only to this request; it does not authorize saved-session or extension access."
        );
        let message: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "Remote file permission".encode_utf16().chain(Some(0)).collect();
        // SAFETY: both strings remain NUL-terminated for the synchronous dialog call.
        unsafe {
            MessageBoxW(
                owner,
                PCWSTR(message.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING,
            ) == IDYES
        }
    }
}
