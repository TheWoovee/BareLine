// SPDX-License-Identifier: MPL-2.0
//! One sleeping worker for up to 63 shared directory watches; bounded queues and buffers.
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
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
};
use windows::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{IO::*, Threading::*},
};
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
struct Directory {
    file: File,
    event: File,
    pending: Box<OVERLAPPED>,
    bytes: Vec<u32>,
    path: PathBuf,
    armed: bool,
}
impl Directory {
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
    /// Setup does filesystem I/O; invoke on a bounded background worker.
    pub fn start_notifying(
        mut paths: Vec<PathBuf>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Self> {
        paths.sort();
        paths.dedup();
        if paths.is_empty() || paths.len() > 63 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "watch requires 1..63 directories",
            ));
        }
        let mut canonical = Vec::new();
        for p in paths {
            canonical.push(
                crate::WindowsPathTrustProvider
                    .canonicalize(&p, PathOrigin::User)?
                    .canonical,
            );
        }
        canonical.sort();
        canonical.dedup();
        let cancel = Arc::new(event()?);
        let stop = cancel.clone();
        let overflow = Arc::new(AtomicBool::new(false));
        let lost = overflow.clone();
        let (sender, receiver) = mpsc::sync_channel(256);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("bareline-watch".into())
            .spawn(move || {
                let setup = || -> io::Result<Vec<Directory>> {
                    let mut dirs = Vec::new();
                    for path in &canonical {
                        let _trust =
                            crate::WindowsPathTrustProvider.open_read(path, PathOrigin::User)?;
                        let file = OpenOptions::new()
                            .access_mode(FILE_LIST_DIRECTORY.0)
                            .share_mode(
                                FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0,
                            )
                            .custom_flags(
                                FILE_FLAG_BACKUP_SEMANTICS.0
                                    | FILE_FLAG_OVERLAPPED.0
                                    | FILE_FLAG_OPEN_REPARSE_POINT.0,
                            )
                            .open(path)?;
                        let event = event()?;
                        let mut pending = Box::new(OVERLAPPED::default());
                        pending.hEvent = handle(&event);
                        let mut d = Directory {
                            file,
                            event,
                            pending,
                            bytes: vec![0; 16384],
                            path: path.clone(),
                            armed: false,
                        };
                        d.arm()?;
                        dirs.push(d);
                    }
                    Ok(dirs)
                };
                let mut first = true;
                loop {
                    let mut dirs = match setup() {
                        Ok(d) => {
                            if first {
                                let _ = ready_tx.send(Ok(()));
                                first = false;
                            } else {
                                lost.store(true, Ordering::Release);
                                notify();
                            }
                            d
                        }
                        Err(e) => {
                            if first {
                                let _ = ready_tx.send(Err(e));
                                return;
                            }
                            if unsafe { WaitForSingleObject(handle(&stop), 250) } == WAIT_OBJECT_0 {
                                return;
                            }
                            continue;
                        }
                    };
                    let mut pending_events: Vec<WatchEvent> = Vec::new();
                    let mut debounce: Option<std::time::Instant> = None;
                    loop {
                        if debounce
                            .is_some_and(|t| t.elapsed() >= std::time::Duration::from_millis(50))
                        {
                            for e in pending_events.drain(..) {
                                if sender.try_send(e).is_err() {
                                    lost.store(true, Ordering::Release);
                                    notify();
                                }
                            }
                            debounce = None;
                            notify();
                        }
                        let timeout = debounce.map_or(INFINITE, |t| {
                            50u32.saturating_sub(t.elapsed().as_millis() as u32)
                        });
                        let handles: Vec<_> = std::iter::once(handle(&stop))
                            .chain(dirs.iter().map(|d| handle(&d.event)))
                            .collect();
                        let result = unsafe { WaitForMultipleObjects(&handles, false, timeout) };
                        if result == WAIT_OBJECT_0 {
                            return;
                        }
                        if result == WAIT_TIMEOUT {
                            continue;
                        }
                        let index = result.0.wrapping_sub(WAIT_OBJECT_0.0 + 1) as usize;
                        if index >= dirs.len() {
                            lost.store(true, Ordering::Release);
                            notify();
                            break;
                        }
                        let d = &mut dirs[index];
                        let mut count = 0;
                        let completion = unsafe {
                            GetOverlappedResult(handle(&d.file), &*d.pending, &mut count, false)
                        };
                        d.armed = false;
                        if completion.is_err() {
                            lost.store(true, Ordering::Release);
                            notify();
                            break;
                        }
                        let bytes = unsafe {
                            std::slice::from_raw_parts(
                                d.bytes.as_ptr().cast::<u8>(),
                                count as usize,
                            )
                        };
                        for e in parse(&d.path, bytes) {
                            if pending_events.len() == 256 {
                                pending_events.clear();
                                lost.store(true, Ordering::Release);
                                notify();
                            }
                            if e.kind != WatchKind::Modified || pending_events.last() != Some(&e) {
                                pending_events.push(e);
                            }
                            if debounce.is_none() {
                                debounce = Some(std::time::Instant::now());
                            }
                        }
                        if d.arm().is_err() {
                            lost.store(true, Ordering::Release);
                            notify();
                            break;
                        }
                    }
                    drop(dirs);
                    if unsafe { WaitForSingleObject(handle(&stop), 250) } == WAIT_OBJECT_0 {
                        return;
                    }
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
            assert_eq!(
                parse(std::path::Path::new("x"), &b)[0].kind,
                WatchKind::RescanNeeded
            );
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
        let start = std::time::Instant::now();
        drop(service);
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        std::fs::remove_file(dir.join("probe")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn deleted_directory_is_rearmed_after_recreation() {
        let dir =
            std::env::temp_dir().join(format!("bareline-watch-recreate-{}", std::process::id()));
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
            if service
                .try_recv()
                .is_some_and(|e| e.name == PathBuf::from("after"))
            {
                found = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        drop(service);
        std::fs::remove_file(dir.join("after")).unwrap();
        std::fs::remove_dir(dir).unwrap();
        assert!(
            found,
            "watch must recover its handle after directory recreation"
        );
    }
    #[test]
    fn nonexistent_directory_fails_without_worker_leak() {
        assert!(
            WindowsWatchService::start(vec![
                std::env::temp_dir().join("bareline-watch-missing-directory-12345")
            ])
            .is_err()
        );
    }
}

impl WindowsWatchService {
    /// Consent text is constructed without opening, classifying, or querying the destination.
    pub fn confirm_remote_read(path:&std::path::Path,action:bareline_platform::RemoteReadAction)->bool{
        use windows::{core::PCWSTR,Win32::UI::WindowsAndMessaging::*};
        let action=match action{bareline_platform::RemoteReadAction::Open=>"open",bareline_platform::RemoteReadAction::Reload=>"reload",bareline_platform::RemoteReadAction::Follow=>"follow"};
        let destination=path.to_string_lossy();if destination.len()>8192||destination.contains('\0'){return false;}
        let destination:String=destination.chars().flat_map(char::escape_debug).collect();
        let text=format!("Allow Bareline to {action} this exact remote file?\n\n{destination}\n\nWindows may send your account credentials to the remote host. This read-only permission applies only to this request; it does not authorize saved-session or extension access.");
        let message:Vec<u16>=text.encode_utf16().chain(Some(0)).collect();let title:Vec<u16>="Remote file permission".encode_utf16().chain(Some(0)).collect();
        // SAFETY: both strings remain NUL-terminated for the synchronous dialog call.
        unsafe{MessageBoxW(None,PCWSTR(message.as_ptr()),PCWSTR(title.as_ptr()),MB_YESNO|MB_DEFBUTTON2|MB_ICONWARNING)==IDYES}
    }
}
