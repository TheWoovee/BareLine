// SPDX-License-Identifier: MPL-2.0
//! Bounded per-user, per-profile single-instance handoff over a Unix domain
//! socket in the user's private runtime folder. No document or network reads
//! occur here.
//!
//! The protocol is the Windows adapter's: the client writes a framed request;
//! the owner checks the peer is the same user, validates the request, reserves a
//! queue slot and answers `PREPARED`; the client answers `COMMIT`; only a
//! received commit queues the request, which the owner confirms with `DONE`.
//! The socket workers never wait for the owner's UI thread, so a pending first
//! frame or a modal loop cannot fail a handoff (APP-03), and exactly one process
//! acts on each request (APP-04).
//!
//! Ownership is an exclusive `flock` on a lock file beside the socket, held for
//! the owner's lifetime. The kernel drops it when the owner exits or crashes, so
//! the next launch takes the lock, removes the stale socket and binds afresh;
//! a launch that cannot take it connects to the owner instead, unless the
//! owner's socket is already gone because it is exiting, in which case the
//! launch takes the lock as soon as it is released.
use crate::ipc::{self, Stop};
use rustix::{
    event::PollFlags,
    fs::{FlockOperation, flock},
};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs::File,
    io,
    os::{
        fd::AsFd,
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::{MetadataExt, OpenOptionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

const LIMIT: usize = 64 * 1024;
/// Client budget to connect and be prepared, from launch. A connection's handshake
/// never gets more than this from the moment it connects either.
const TIMEOUT: Duration = Duration::from_secs(2);
/// Further client budget to write the commit, and then again to see it confirmed,
/// so a launch waits at most `TIMEOUT + 2 * COMMIT_WINDOW` for an owner.
const COMMIT_WINDOW: Duration = Duration::from_millis(500);
/// The owner's clock starts when it accepts a client, so it outwaits the latest
/// commit any client writes by a second: a commit that was written is always read
/// (APP-04).
const SERVER_TIMEOUT: Duration = TIMEOUT
    .saturating_add(COMMIT_WINDOW)
    .saturating_add(Duration::from_secs(1));
/// Concurrent handoffs, e.g. a file manager multi-select that starts one process
/// per file. Workers start on demand, so an idle owner keeps one thread (PERF-02).
const INSTANCES: u32 = 4;
/// Committed requests held while the owner's UI thread is busy.
const QUEUE: usize = 64;
const PREPARED: u8 = 1;
const COMMIT: u8 = 2;
const DONE: u8 = 3;
/// The longest path the handoff accepts, in bytes (Linux `PATH_MAX`).
const MAX_PATH_BYTES: usize = 4096;
/// The folder name inside the runtime directory resolution.
const APP: &str = "bareline";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenRequest {
    pub paths: Vec<PathBuf>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub read_only: bool,
    pub monitor: bool,
}
pub enum Outcome {
    Forwarded,
    Primary(UnixInstanceServer),
    Independent(String),
}
/// The shell names the server type `instance::InstanceServer` on every system.
pub use UnixInstanceServer as InstanceServer;

#[derive(Default)]
struct HandoffQueue {
    state: Mutex<QueueState>,
    /// Signalled whenever a reserved slot settles.
    settled: Condvar,
}
#[derive(Default)]
struct QueueState {
    requests: VecDeque<OpenRequest>,
    reserved: usize,
    refusing: bool,
}
impl HandoffQueue {
    fn state(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Refuses further handoffs and waits, at most `limit`, for the prepared ones to
    /// commit or give up. Returns the committed requests still to drain plus any slot
    /// that has not settled; while refusal lasts, no request is added to that count.
    fn quiesce(&self, limit: Duration) -> usize {
        let mut state = self.state();
        state.refusing = true;
        let (state, _) = self
            .settled
            .wait_timeout_while(state, limit, |state| state.reserved > 0)
            .unwrap_or_else(PoisonError::into_inner);
        state.requests.len() + state.reserved
    }
    /// A slot is reserved before `PREPARED`, so a committed request always fits.
    fn reserve(&self) -> bool {
        let mut state = self.state();
        if state.refusing || state.requests.len() + state.reserved >= QUEUE {
            return false;
        }
        state.reserved += 1;
        true
    }
    fn settle(&self, request: Option<OpenRequest>) {
        let mut state = self.state();
        state.reserved -= 1;
        state.requests.extend(request);
        self.settled.notify_all();
    }
}

/// The owner side: one acceptor thread while idle, plus a worker per handoff in
/// progress (at most `INSTANCES`).
pub struct UnixInstanceServer {
    queue: Arc<HandoffQueue>,
    listener: Arc<Listener>,
    acceptor: Option<JoinHandle<()>>,
    socket: PathBuf,
    /// The bound socket's identity, so only this owner's socket is removed.
    socket_node: (u64, u64),
    /// Held while this process owns the scope; released last.
    _ownership: File,
    /// Held while this process owns the profile, so no second owner shares it (APP-14).
    _profile: Option<File>,
}
/// What the acceptor and every worker share.
struct Listener {
    socket: UnixListener,
    uid: u32,
    stop: Stop,
    queue: Arc<HandoffQueue>,
    notify: Arc<dyn Fn() + Send + Sync>,
    workers: Mutex<Workers>,
    /// Signalled whenever a worker finishes a handoff.
    idle: Condvar,
}
#[derive(Default)]
struct Workers {
    handles: Vec<JoinHandle<()>>,
    /// Workers handling a connection; never more than `INSTANCES`.
    busy: u32,
}
impl Listener {
    fn workers(&self) -> MutexGuard<'_, Workers> {
        self.workers.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Accepts connections until stopped. While `INSTANCES` handshakes run, further
    /// launches wait in the socket backlog within their own deadlines, as they
    /// wait for a free pipe instance on Windows.
    fn accept_loop(self: &Arc<Self>) {
        loop {
            {
                let mut workers = self.workers();
                while workers.busy >= INSTANCES {
                    if self.stop.raised() {
                        return;
                    }
                    workers = self
                        .idle
                        .wait_timeout(workers, Duration::from_millis(50))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                // Finished workers are joined here so an idle owner holds no threads.
                let (finished, running): (Vec<_>, Vec<_>) =
                    workers.handles.drain(..).partition(|handle| handle.is_finished());
                workers.handles = running;
                drop(workers);
                for handle in finished {
                    let _ = handle.join();
                }
            }
            match ipc::wait(self.socket.as_fd(), PollFlags::IN, Some(self.stop.fd()), None) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return,
                Err(_) => {
                    // A failed wait must not spin; the stop check above ends the loop.
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
            }
            let stream = match self.socket.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(_) => {
                    // For example out of descriptors: back off instead of spinning.
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            };
            let mut workers = self.workers();
            if self.stop.raised() {
                return;
            }
            workers.busy += 1;
            let listener = self.clone();
            let spawned = std::thread::Builder::new()
                .name("bareline-instance".into())
                .spawn(move || {
                    let _ = receive(&stream, &listener);
                    listener.workers().busy -= 1;
                    listener.idle.notify_all();
                });
            match spawned {
                Ok(handle) => workers.handles.push(handle),
                // The dropped connection makes that launch open its files itself.
                Err(_) => workers.busy -= 1,
            }
        }
    }
}
impl UnixInstanceServer {
    /// Committed requests in arrival order. Each was already acknowledged, so the
    /// owner acts on every one it drains.
    pub fn try_recv(&self) -> Option<OpenRequest> {
        self.queue.state().requests.pop_front()
    }
    /// A refusing owner turns new handoffs away at once, so those launches open
    /// independently instead of queueing behind a close.
    pub fn set_accepting(&self, accepting: bool) {
        self.queue.state().refusing = !accepting;
    }
    /// Acknowledged requests not yet drained, counting those that may still commit.
    pub fn pending(&self) -> usize {
        let state = self.queue.state();
        state.requests.len() + state.reserved
    }
    /// Call before exiting: stops accepting, then waits for the handoffs already
    /// prepared to commit or give up (within the handshake deadline, a few seconds
    /// at worst). Exiting loses the returned number of acknowledged requests, so
    /// the owner may exit only when it is 0 and it has not accepted again since.
    pub fn quiesce(&self) -> usize {
        self.queue
            .quiesce(SERVER_TIMEOUT.saturating_add(Duration::from_secs(1)))
    }
    /// The socket launches connect to; for diagnostics and tests.
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }
}
impl Drop for UnixInstanceServer {
    fn drop(&mut self) {
        self.listener.stop.raise();
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
        // Stopped: no worker starts after the acceptor ended, so every one is joined.
        let workers = std::mem::take(&mut self.listener.workers().handles);
        for worker in workers {
            let _ = worker.join();
        }
        // The ownership lock is still held, so no new owner can have bound a socket
        // at this name; the identity check guards against anything else.
        if std::fs::symlink_metadata(&self.socket)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.socket_node)
        {
            let _ = std::fs::remove_file(&self.socket);
        }
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid instance request")
}

fn encode(request: &OpenRequest) -> io::Result<Vec<u8>> {
    if request.paths.len() > 16
        || request.line == Some(0)
        || request.column == Some(0)
        || (request.column.is_some() && request.line.is_none())
    {
        return Err(invalid());
    }
    let mut data = b"BLI1".to_vec();
    data.push(u8::from(request.read_only) | (u8::from(request.monitor) << 1));
    data.extend(request.line.unwrap_or(0).to_le_bytes());
    data.extend(request.column.unwrap_or(0).to_le_bytes());
    data.push(request.paths.len() as u8);
    for path in &request.paths {
        let bytes = path.as_os_str().as_bytes();
        if !path.is_absolute() || bytes.is_empty() || bytes.contains(&0) || bytes.len() > MAX_PATH_BYTES {
            return Err(invalid());
        }
        data.extend((bytes.len() as u16).to_le_bytes());
        data.extend_from_slice(bytes);
        if data.len() > LIMIT {
            return Err(invalid());
        }
    }
    Ok(data)
}
fn decode(data: &[u8]) -> io::Result<OpenRequest> {
    if data.len() < 22 || data.len() > LIMIT || &data[..4] != b"BLI1" || data[4] & !3 != 0 || data[21] > 16 {
        return Err(invalid());
    }
    let line = u64::from_le_bytes(data[5..13].try_into().map_err(|_| invalid())?);
    let column = u64::from_le_bytes(data[13..21].try_into().map_err(|_| invalid())?);
    let mut request = OpenRequest {
        line: (line != 0).then_some(line),
        column: (column != 0).then_some(column),
        read_only: data[4] & 1 != 0,
        monitor: data[4] & 2 != 0,
        paths: Vec::new(),
    };
    let mut at = 22;
    for _ in 0..data[21] {
        let length = data.get(at..at + 2).ok_or_else(invalid)?;
        at += 2;
        let length = usize::from(u16::from_le_bytes([length[0], length[1]]));
        let bytes = data.get(at..at + length).ok_or_else(invalid)?;
        at += length;
        request
            .paths
            .push(PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec())));
    }
    if at != data.len() {
        return Err(invalid());
    }
    encode(&request)?;
    request.read_only |= request.monitor;
    Ok(request)
}

/// The owner side of one connection. The peer must be this user; anything else
/// is refused before the request is even read.
fn receive(stream: &UnixStream, listener: &Listener) -> io::Result<()> {
    let deadline = Instant::now() + SERVER_TIMEOUT;
    if ipc::peer(stream)?.uid != listener.uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "instance user mismatch",
        ));
    }
    ipc::configure(stream)?;
    let stop = Some(listener.stop.fd());
    let mut size = [0; 4];
    ipc::read_exact(stream, stop, &mut size, deadline)?;
    let size = u32::from_le_bytes(size) as usize;
    if size > LIMIT {
        return Err(invalid());
    }
    let mut data = vec![0; size];
    ipc::read_exact(stream, stop, &mut data, deadline)?;
    let request = decode(&data)?;
    // An unanswered client opens the files itself.
    if !listener.queue.reserve() {
        return Err(io::Error::other("instance queue full or closing"));
    }
    let committed = (|| -> io::Result<()> {
        ipc::write_all(stream, stop, &[PREPARED], deadline)?;
        let mut commit = [0];
        ipc::read_exact(stream, stop, &mut commit, deadline)?;
        if commit == [COMMIT] { Ok(()) } else { Err(invalid()) }
    })();
    // Until the commit arrives the client may still give up and open the files
    // itself, so only a received commit makes this owner act (APP-04).
    listener.queue.settle(committed.is_ok().then_some(request));
    committed?;
    (listener.notify)();
    ipc::write_all(stream, stop, &[DONE], deadline)
}

/// The scope's owner identity: the canonical path of its nearest existing
/// ancestor plus the not-yet-created tail, so every spelling of one profile
/// (`.` steps, symbolic links, a relative path) names one socket (APP-14).
/// Unix paths are case-sensitive bytes, so no case folding happens here.
fn canonical_scope(scope: &Path) -> PathBuf {
    let absolute = std::path::absolute(scope).unwrap_or_else(|_| scope.to_path_buf());
    let mut current = absolute.as_path();
    let mut tail = Vec::new();
    let mut canonical = loop {
        if let Ok(path) = std::fs::canonicalize(current) {
            break path;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                current = parent;
            }
            _ => break current.to_path_buf(),
        }
    };
    for name in tail.into_iter().rev() {
        canonical.push(name);
    }
    canonical
}
/// The socket and lock names for this user and scope. 96 bits of the digest keep
/// the socket path inside `sun_path` even under macOS's long `TMPDIR`.
fn names(runtime: &Path, uid: u32, scope: &Path) -> (PathBuf, PathBuf) {
    let mut digest = Sha256::new();
    digest.update(uid.to_le_bytes());
    digest.update(canonical_scope(scope).as_os_str().as_bytes());
    let key: String = digest.finalize()[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    (
        runtime.join(format!("instance-{key}.sock")),
        runtime.join(format!("instance-{key}.lock")),
    )
}
fn open_lock(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
}
/// Exclusive and nonblocking; `Ok(None)` when another owner holds it.
fn try_lock(file: File) -> io::Result<Option<File>> {
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(file)),
        Err(errno) if errno == rustix::io::Errno::WOULDBLOCK => Ok(None),
        Err(errno) => Err(errno.into()),
    }
}
/// An owner keeps its profile's lock file locked. Another scope spelling that still
/// hashes apart then finds the profile taken. A profile folder that does not exist
/// yet or cannot be written is not locked, as on Windows.
fn lock_profile(profile: &Path) -> io::Result<Option<File>> {
    let Ok(file) = open_lock(&profile.join("instance.lock")) else {
        return Ok(None);
    };
    match try_lock(file) {
        Ok(Some(file)) => Ok(Some(file)),
        Ok(None) => Err(io::Error::new(io::ErrorKind::WouldBlock, "profile in use")),
        Err(_) => Ok(None),
    }
}
/// Connects before `deadline`, retrying while the owner is between taking its
/// lock and listening. Also returns when the successful attempt began.
fn connect(socket: &Path, deadline: Instant) -> io::Result<(UnixStream, Instant)> {
    loop {
        let attempt = Instant::now();
        match UnixStream::connect(socket) {
            Ok(stream) => return Ok((stream, attempt)),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused | io::ErrorKind::WouldBlock
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(2))
            }
            Err(error) => return Err(error),
        }
    }
}

/// `profile` is the data folder whose lock file marks its single owner. The
/// socket lives in the user's runtime folder (`paths::runtime_dir`).
pub fn coordinate(
    scope: &Path,
    profile: Option<&Path>,
    request: OpenRequest,
    new_instance: bool,
    notify: Arc<dyn Fn() + Send + Sync>,
) -> io::Result<Outcome> {
    let runtime = match crate::paths::runtime_dir(APP) {
        Ok(runtime) => runtime,
        Err(error) => {
            encode(&request)?;
            if new_instance {
                return Ok(independent_by_choice());
            }
            return Ok(unreachable_owners(&error));
        }
    };
    coordinate_in(&runtime, scope, profile, request, new_instance, notify)
}
fn independent_by_choice() -> Outcome {
    Outcome::Independent(
        "This window runs separately from other Bareline windows; its tabs are not restored next time.".into(),
    )
}
fn unreachable_owners(error: &io::Error) -> Outcome {
    Outcome::Independent(format!(
        "Could not connect to other Bareline windows ({error}), so this window runs separately; its tabs are not restored next time."
    ))
}
/// [`coordinate`] with an explicit runtime folder, which must be (or become) a
/// private folder of this user.
pub fn coordinate_in(
    runtime: &Path,
    scope: &Path,
    profile: Option<&Path>,
    request: OpenRequest,
    new_instance: bool,
    notify: Arc<dyn Fn() + Send + Sync>,
) -> io::Result<Outcome> {
    let payload = encode(&request)?;
    if new_instance {
        return Ok(independent_by_choice());
    }
    if let Err(error) = ipc::private_directory(runtime) {
        return Ok(unreachable_owners(&error));
    }
    let uid = ipc::own_uid();
    let (socket, lock) = names(runtime, uid, scope);
    let connect_by = Instant::now() + TIMEOUT;
    let ownership = loop {
        match open_lock(&lock).and_then(try_lock) {
            Ok(Some(ownership)) => break ownership,
            Ok(None) => {}
            Err(error) => return Ok(unreachable_owners(&error)),
        }
        // The owner listens once its socket exists. Until then it is starting,
        // or exiting: it removes its socket before its lock goes, and a process
        // it spawned holds the lock until that process execs. Taking the lock
        // again succeeds an exiting owner instead of waiting on it in vain.
        if std::fs::symlink_metadata(&socket).is_ok() || Instant::now() >= connect_by {
            return Ok(forwarded(forward(&socket, &payload, uid, connect_by)));
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let Ok(profile) = profile.map_or(Ok(None), lock_profile) else {
        return Ok(Outcome::Independent(
            "Another Bareline window is using this profile, so this window runs separately; its tabs are not restored next time.".into(),
        ));
    };
    // Whatever is at the socket name belongs to an owner that is gone: its lock
    // was free. Remove it so this owner can bind (stale socket after a crash).
    match std::fs::remove_file(&socket) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Ok(unreachable_owners(&error)),
    }
    let listener = match UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(error) => return Ok(unreachable_owners(&error)),
    };
    listener.set_nonblocking(true)?;
    let metadata = std::fs::symlink_metadata(&socket)?;
    let queue = Arc::new(HandoffQueue::default());
    let listener = Arc::new(Listener {
        socket: listener,
        uid,
        stop: Stop::new()?,
        queue: queue.clone(),
        notify,
        workers: Mutex::default(),
        idle: Condvar::new(),
    });
    let accepting = listener.clone();
    let acceptor = std::thread::Builder::new()
        .name("bareline-instance".into())
        .spawn(move || accepting.accept_loop())?;
    Ok(Outcome::Primary(UnixInstanceServer {
        queue,
        listener,
        acceptor: Some(acceptor),
        socket,
        socket_node: (metadata.dev(), metadata.ino()),
        _ownership: ownership,
        _profile: profile,
    }))
}

/// Client side up to `PREPARED`. Returns the connection and the deadline for its commit.
fn prepare(socket: &Path, payload: &[u8], uid: u32, connect_by: Instant) -> io::Result<(UnixStream, Instant)> {
    let (stream, connecting) = connect(socket, connect_by)?;
    // The owner's clock starts once it accepts this connection, which is after
    // this attempt began, so the commit deadline falls inside `SERVER_TIMEOUT` of it.
    let deadline = connect_by.min(connecting + TIMEOUT);
    if ipc::peer(&stream)?.uid != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "instance user mismatch",
        ));
    }
    ipc::configure(&stream)?;
    let mut data = (payload.len() as u32).to_le_bytes().to_vec();
    data.extend_from_slice(payload);
    ipc::write_all(&stream, None, &data, deadline)?;
    let mut ack = [0];
    ipc::read_exact(&stream, None, &mut ack, deadline)?;
    if ack != [PREPARED] {
        return Err(invalid());
    }
    Ok((stream, deadline + COMMIT_WINDOW))
}
/// Any error means the owner never got the commit, and it acts only on a received
/// commit, so this launch opens the files itself. A written commit is always read,
/// because the owner outwaits `deadline` (APP-04).
fn commit(stream: &UnixStream, deadline: Instant) -> io::Result<()> {
    ipc::write_all(stream, None, &[COMMIT], deadline)?;
    // Waiting for `DONE` keeps the connection open until the owner has read the commit.
    let _ = ipc::read_exact(stream, None, &mut [0], deadline + COMMIT_WINDOW);
    Ok(())
}
fn forward(socket: &Path, payload: &[u8], uid: u32, connect_by: Instant) -> io::Result<()> {
    let (stream, deadline) = prepare(socket, payload, uid, connect_by)?;
    commit(&stream, deadline)
}
fn forwarded(result: io::Result<()>) -> Outcome {
    match result {
        Ok(()) => Outcome::Forwarded,
        Err(error) => Outcome::Independent(format!(
            "The running Bareline window did not respond ({error}), so this window opened the files itself; its tabs are not restored next time."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(label: &str) -> Self {
            // Short and under /tmp: macOS's TMPDIR is long and a socket path must
            // fit in 104 bytes.
            let root = PathBuf::from(format!(
                "/tmp/bli-{label}-{}-{}",
                std::process::id(),
                nanos() % 1_000_000_000
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn runtime(&self) -> PathBuf {
            self.0.join("run")
        }
        fn scope(&self) -> PathBuf {
            self.0.join("profile").join("settings.toml")
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn open(path: &str) -> OpenRequest {
        OpenRequest {
            paths: vec![PathBuf::from(path)],
            ..Default::default()
        }
    }
    fn hand_off(scratch: &Scratch, scope: &Path, profile: Option<&Path>, request: OpenRequest) -> Outcome {
        coordinate_in(&scratch.runtime(), scope, profile, request, false, Arc::new(|| {})).unwrap()
    }
    fn primary(scratch: &Scratch) -> UnixInstanceServer {
        match hand_off(scratch, &scratch.scope(), None, OpenRequest::default()) {
            Outcome::Primary(server) => server,
            Outcome::Forwarded => panic!("primary: forwarded"),
            Outcome::Independent(reason) => panic!("primary: {reason}"),
        }
    }
    /// A hang guard only: no assertion depends on how long a handoff takes.
    fn wait_until(mut done: impl FnMut() -> bool) {
        let watchdog = Instant::now() + Duration::from_secs(60);
        while !done() {
            assert!(Instant::now() < watchdog, "watchdog expired");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn drain(server: &UnixInstanceServer, count: usize) -> Vec<OpenRequest> {
        let mut received = Vec::new();
        wait_until(|| {
            received.extend(server.try_recv());
            received.len() >= count
        });
        received
    }
    /// A client that was answered `PREPARED` and has not committed yet.
    struct Prepared {
        stream: UnixStream,
        deadline: Instant,
    }
    impl Prepared {
        fn commit(&self) -> io::Result<()> {
            commit(&self.stream, self.deadline)
        }
    }
    fn prepared(scratch: &Scratch, path: &str) -> Prepared {
        let (socket, _) = names(&scratch.runtime(), ipc::own_uid(), &scratch.scope());
        let payload = encode(&open(path)).unwrap();
        let (stream, deadline) = prepare(&socket, &payload, ipc::own_uid(), Instant::now() + TIMEOUT).unwrap();
        Prepared { stream, deadline }
    }

    #[test]
    fn lossless_payload_and_revalidation() {
        let request = OpenRequest {
            paths: vec![PathBuf::from(std::ffi::OsString::from_vec(
                b"/tmp/\xff caf\xc3\xa9".to_vec(),
            ))],
            line: Some(2),
            column: Some(3),
            read_only: true,
            ..Default::default()
        };
        let data = encode(&request).unwrap();
        assert_eq!(decode(&data).unwrap(), request);
        assert!(decode(&data[..data.len() - 1]).is_err());
        let mut bad = data.clone();
        bad[4] = 128;
        assert!(decode(&bad).is_err());
        // Relative, empty, NUL-carrying and oversized paths never cross.
        for path in [&b"notes.txt"[..], b"", b"/a\0b"] {
            let request = OpenRequest {
                paths: vec![PathBuf::from(std::ffi::OsString::from_vec(path.to_vec()))],
                ..Default::default()
            };
            assert!(encode(&request).is_err());
        }
        assert!(encode(&open(&format!("/{}", "a".repeat(MAX_PATH_BYTES)))).is_err());
        // A monitored file always opens read-only.
        let monitored = decode(
            &encode(&OpenRequest {
                monitor: true,
                ..open("/var/log/syslog")
            })
            .unwrap(),
        )
        .unwrap();
        assert!(monitored.read_only);
    }
    #[test]
    fn queue_slots_are_reserved_before_prepared() {
        let queue = HandoffQueue::default();
        for _ in 0..QUEUE {
            assert!(queue.reserve());
        }
        assert!(!queue.reserve());
        queue.settle(None);
        assert!(queue.reserve());
        // A committed request keeps its slot until the owner drains it.
        queue.settle(Some(open("/queue/a.txt")));
        assert!(!queue.reserve());
        assert_eq!(queue.state().requests.pop_front(), Some(open("/queue/a.txt")));
        assert!(queue.reserve());
    }
    #[test]
    fn owner_acknowledges_a_queued_request_without_its_ui() {
        let scratch = Scratch::new("ack");
        let server = primary(&scratch);
        // Nothing drains the queue during the handoff, as before the owner's first
        // frame or while its UI thread runs a modal loop (APP-03).
        assert!(matches!(
            hand_off(&scratch, &scratch.scope(), None, open("/ack/a.txt")),
            Outcome::Forwarded
        ));
        assert_eq!(drain(&server, 1), vec![open("/ack/a.txt")]);
        assert_eq!(server.try_recv(), None);
        server.set_accepting(false);
        assert!(matches!(
            hand_off(&scratch, &scratch.scope(), None, open("/ack/b.txt")),
            Outcome::Independent(_)
        ));
        assert_eq!(server.try_recv(), None);
        assert!(matches!(
            coordinate_in(
                &scratch.runtime(),
                &scratch.scope(),
                None,
                OpenRequest::default(),
                true,
                Arc::new(|| {})
            )
            .unwrap(),
            Outcome::Independent(_)
        ));
    }
    #[test]
    fn concurrent_clients_are_each_forwarded_exactly_once() {
        let scratch = Scratch::new("many");
        let server = primary(&scratch);
        let (socket, _) = names(&scratch.runtime(), ipc::own_uid(), &scratch.scope());
        let expected: Vec<_> = (0..12).map(|index| open(&format!("/many/{index:02}.txt"))).collect();
        let clients: Vec<_> = expected
            .iter()
            .map(|request| {
                let (socket, payload) = (socket.clone(), encode(request).unwrap());
                // Clients wait for a worker as long as they need to, so a loaded
                // machine cannot fail this test; each handshake keeps its deadline.
                std::thread::spawn(move || {
                    forwarded(forward(
                        &socket,
                        &payload,
                        ipc::own_uid(),
                        Instant::now() + Duration::from_secs(60),
                    ))
                })
            })
            .collect();
        for client in clients {
            assert!(matches!(client.join().unwrap(), Outcome::Forwarded));
        }
        let mut received = drain(&server, expected.len());
        received.sort_by(|left, right| left.paths.cmp(&right.paths));
        assert_eq!(received, expected);
        assert_eq!(server.try_recv(), None);
        assert!(server.listener.workers().busy <= INSTANCES);
    }
    /// PERF-02: an idle owner serves from one thread; workers exist only while a
    /// handoff is in progress.
    #[test]
    fn idle_owner_keeps_only_its_acceptor() {
        let scratch = Scratch::new("lazy");
        let server = primary(&scratch);
        assert_eq!(server.listener.workers().busy, 0);
        let client = prepared(&scratch, "/lazy/a.txt");
        assert_eq!(server.listener.workers().busy, 1, "one worker per handoff in progress");
        client.commit().unwrap();
        drop(client);
        assert_eq!(drain(&server, 1), vec![open("/lazy/a.txt")]);
        wait_until(|| server.listener.workers().busy == 0);
    }
    #[test]
    fn uncommitted_request_is_never_acted_on() {
        let scratch = Scratch::new("abort");
        let server = primary(&scratch);
        let client = prepared(&scratch, "/abort/a.txt");
        assert_eq!(server.pending(), 1);
        // The client gives up before committing, as one past its deadline does, and
        // then opens the file itself; the owner must not open it too (APP-04).
        drop(client);
        wait_until(|| server.queue.state().reserved == 0);
        assert_eq!(server.try_recv(), None);
        assert!(matches!(
            hand_off(&scratch, &scratch.scope(), None, open("/abort/b.txt")),
            Outcome::Forwarded
        ));
        assert_eq!(drain(&server, 1), vec![open("/abort/b.txt")]);
    }
    #[test]
    fn commit_the_owner_no_longer_reads_opens_independently() {
        let scratch = Scratch::new("gave-up");
        let server = primary(&scratch);
        let client = prepared(&scratch, "/gave-up/a.txt");
        // The owner stops waiting for this commit, as at its own deadline, and
        // disconnects; the commit then cannot be written.
        drop(server);
        let result = client.commit();
        assert!(result.is_err());
        // Nobody else acts on the request, so this launch opens the file itself.
        assert!(matches!(forwarded(result), Outcome::Independent(_)));
    }
    #[test]
    fn quiesce_waits_for_prepared_slots_and_counts_their_commits() {
        let queue = HandoffQueue::default();
        assert!(queue.reserve());
        std::thread::scope(|threads| {
            let exiting = threads.spawn(|| queue.quiesce(Duration::from_secs(60)));
            wait_until(|| queue.state().refusing);
            assert!(!queue.reserve());
            // The prepared client commits while the owner waits to exit.
            queue.settle(Some(open("/quiesce/a.txt")));
            assert_eq!(exiting.join().unwrap(), 1);
        });
        assert!(!queue.reserve());
        assert_eq!(queue.state().requests.pop_front(), Some(open("/quiesce/a.txt")));
        assert_eq!(queue.quiesce(Duration::ZERO), 0);
        // A slot that never settles keeps counting, so the owner does not exit on it.
        queue.state().refusing = false;
        assert!(queue.reserve());
        assert_eq!(queue.quiesce(Duration::ZERO), 1);
    }
    #[test]
    fn exiting_owner_leaves_no_acknowledged_request_behind() {
        let scratch = Scratch::new("quiesce");
        let server = primary(&scratch);
        let client = prepared(&scratch, "/quiesce/a.txt");
        assert_eq!(server.pending(), 1);
        std::thread::scope(|threads| {
            // The owner decides to exit while this client is prepared...
            let exiting = threads.spawn(|| server.quiesce());
            wait_until(|| server.queue.state().refusing);
            // ...and the commit still lands, so the owner must not exit yet (APP-03).
            client.commit().unwrap();
            assert_eq!(exiting.join().unwrap(), 1);
        });
        // Refusal holds: a later launch opens by itself and nothing is added.
        assert!(matches!(
            hand_off(&scratch, &scratch.scope(), None, open("/quiesce/b.txt")),
            Outcome::Independent(_)
        ));
        assert_eq!(server.quiesce(), 1);
        assert_eq!(server.try_recv(), Some(open("/quiesce/a.txt")));
        assert_eq!(server.pending(), 0);
        assert_eq!(server.quiesce(), 0);
    }
    #[test]
    fn stale_socket_of_a_vanished_owner_is_replaced() {
        let scratch = Scratch::new("stale");
        ipc::private_directory(&scratch.runtime()).unwrap();
        let (socket, _) = names(&scratch.runtime(), ipc::own_uid(), &scratch.scope());
        // A crashed owner leaves its socket file behind but no listener or lock.
        drop(UnixListener::bind(&socket).unwrap());
        assert!(socket.exists());
        let server = primary(&scratch);
        assert!(matches!(
            hand_off(&scratch, &scratch.scope(), None, open("/stale/a.txt")),
            Outcome::Forwarded
        ));
        assert_eq!(drain(&server, 1), vec![open("/stale/a.txt")]);
        // A clean exit removes the socket, and the next launch owns the scope.
        drop(server);
        assert!(!socket.exists());
        drop(primary(&scratch));
    }
    /// An owner that is exiting has removed its socket but still holds its
    /// lock for a moment (or a process it spawned holds it until that process
    /// execs). A launch in that moment becomes the next owner instead of
    /// failing to reach a listener and running separately.
    #[test]
    fn a_launch_during_the_owners_exit_becomes_the_owner() {
        let scratch = Scratch::new("exiting");
        ipc::private_directory(&scratch.runtime()).unwrap();
        let (socket, lock) = names(&scratch.runtime(), ipc::own_uid(), &scratch.scope());
        let held = try_lock(open_lock(&lock).unwrap()).unwrap().unwrap();
        assert!(!socket.exists());
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(held);
        });
        let server = primary(&scratch);
        release.join().unwrap();
        assert!(socket.exists());
        assert!(matches!(
            hand_off(&scratch, &scratch.scope(), None, open("/exiting/a.txt")),
            Outcome::Forwarded
        ));
        assert_eq!(drain(&server, 1), vec![open("/exiting/a.txt")]);
    }
    #[test]
    fn scope_spellings_of_one_profile_share_one_identity() {
        let scratch = Scratch::new("spelling");
        let root = scratch.0.join("Profile");
        std::fs::create_dir_all(&root).unwrap();
        let settings = root.join("settings.toml");
        let before = canonical_scope(&settings);
        assert_eq!(canonical_scope(&root.join(".").join("settings.toml")), before);
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert_eq!(canonical_scope(&link.join("settings.toml")), before);
        // Creating the file keeps the identity its folder already gave it.
        std::fs::write(&settings, "").unwrap();
        assert_eq!(canonical_scope(&settings), before);
        assert_ne!(canonical_scope(&root.join("other.toml")), before);
        // Linux names are case-sensitive: another case is another scope. (macOS
        // volumes usually fold case, and then both spellings name one folder.)
        #[cfg(target_os = "linux")]
        assert_ne!(
            canonical_scope(&scratch.0.join("PROFILE").join("settings.toml")),
            before
        );
    }
    #[test]
    fn profile_lock_refuses_a_second_owner() {
        let scratch = Scratch::new("lock");
        let profile = scratch.0.join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        let Outcome::Primary(server) = hand_off(&scratch, &scratch.scope(), Some(&profile), OpenRequest::default())
        else {
            panic!("primary");
        };
        // A spelling through a link reaches the same owner.
        let link = scratch.0.join("alias");
        std::os::unix::fs::symlink(&profile, &link).unwrap();
        assert!(matches!(
            hand_off(&scratch, &link.join("settings.toml"), Some(&link), open("/lock/a.txt")),
            Outcome::Forwarded
        ));
        assert_eq!(drain(&server, 1), vec![open("/lock/a.txt")]);
        // Another scope names another socket, yet cannot own the same profile.
        assert!(matches!(
            hand_off(
                &scratch,
                &profile.join("session-2"),
                Some(&profile),
                OpenRequest::default()
            ),
            Outcome::Independent(_)
        ));
        drop(server);
        let Outcome::Primary(_second) = hand_off(
            &scratch,
            &profile.join("session-2"),
            Some(&profile),
            OpenRequest::default(),
        ) else {
            panic!("the profile is free once its owner exits");
        };
    }
    #[test]
    fn a_runtime_folder_of_another_shape_is_never_used() {
        let scratch = Scratch::new("shape");
        // A file where the runtime folder should be: run independently, never fail.
        std::fs::write(scratch.runtime(), "").unwrap();
        assert!(matches!(
            hand_off(&scratch, &scratch.scope(), None, OpenRequest::default()),
            Outcome::Independent(_)
        ));
    }
}
