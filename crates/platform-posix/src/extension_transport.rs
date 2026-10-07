// SPDX-License-Identifier: MPL-2.0
//! Same-user, local-only, nonblocking Unix domain sockets for the extension host,
//! and the verified launch that uses them. Callers run this bounded I/O on their
//! host worker, never on the UI thread.
//!
//! An unguessable name is not sufficient: as on Windows, both endpoints check the
//! kernel's record of the other side, the editor that the connecting process is
//! the host it started (pid) running as this user (uid), and the host that the
//! listening process is its parent editor and the same user. Linux names the
//! socket in the abstract namespace, so nothing is left on disk; macOS, which has
//! no abstract namespace, binds it in the per-user temporary folder and removes
//! it once the host connected.
use crate::ipc;
use bareline_extensions_protocol::{
    BrokerResponse, Envelope, ExecutionBudget, INTERACTIVE_TIMEOUT_MS, Invocation, ProtocolError, encode, read_frame,
    valid_id,
};
use bareline_macros::process::ProcessTreeGuard;
use rustix::event::PollFlags;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::AsFd,
        unix::{
            fs::OpenOptionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    process::{Child, ExitStatus},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Every transport name starts with this; anything else is refused before use.
const NAME_PREFIX: &str = "bareline-exthost-";
/// The abstract-namespace marker in a Linux transport name.
#[cfg(target_os = "linux")]
const ABSTRACT: &str = "@";

type WatchdogRun = Box<dyn FnOnce() + Send + 'static>;
type HostGuard = Box<dyn ProcessTreeGuard>;

fn deadline_error() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "extension IPC deadline")
}
/// Waits for readiness, reporting an expired deadline in the transport's words.
fn ready(stream: &impl AsFd, events: PollFlags, deadline: Instant) -> io::Result<()> {
    ipc::wait(stream.as_fd(), events, None, Some(deadline)).map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            deadline_error()
        } else {
            error
        }
    })
}
fn pause(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        return Err(deadline_error());
    }
    std::thread::sleep(Duration::from_millis(2));
    Ok(())
}
/// Only the expected process of this user is accepted, in either direction.
fn authentic(peer: ipc::Peer, expected_pid: u32, uid: u32) -> bool {
    peer.pid == expected_pid && peer.uid == uid
}
fn valid_name(name: &str) -> bool {
    if name.len() > 100 || name.contains('\0') {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        name.strip_prefix(ABSTRACT)
            .is_some_and(|rest| rest.starts_with(NAME_PREFIX))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let path = Path::new(name);
        path.is_absolute()
            && path
                .file_name()
                .and_then(|file| file.to_str())
                .is_some_and(|file| file.starts_with(NAME_PREFIX))
    }
}
#[cfg(target_os = "linux")]
fn connect_name(name: &str) -> io::Result<UnixStream> {
    use std::os::{linux::net::SocketAddrExt, unix::net::SocketAddr};
    let abstract_name = name.strip_prefix(ABSTRACT).unwrap_or(name);
    UnixStream::connect_addr(&SocketAddr::from_abstract_name(abstract_name.as_bytes())?)
}
#[cfg(not(target_os = "linux"))]
fn connect_name(name: &str) -> io::Result<UnixStream> {
    UnixStream::connect(name)
}

/// The editor's listening end for one host launch.
pub struct UnixTransportServer {
    listener: UnixListener,
    name: String,
    /// The socket file to remove, where the name is a file (macOS).
    path: Option<PathBuf>,
}
/// A connected, authenticated, nonblocking socket. Every read and write fails
/// with `TimedOut` once the deadline passes.
pub struct AuthenticatedSocket {
    stream: UnixStream,
    deadline: Instant,
}
impl UnixTransportServer {
    pub fn create() -> io::Result<Self> {
        // Random name suffix only. There is no launch "nonce": it would be
        // readable by any same-user process, so authentication rests entirely on
        // the peer pid/uid check (SEC-05).
        // 96 bits keep a socket file name inside macOS's 104-byte `sun_path`.
        let suffix = ipc::random_hex::<12>()?;
        let label = format!("{NAME_PREFIX}{}-{suffix}", std::process::id());
        #[cfg(target_os = "linux")]
        let (listener, name, path) = {
            use std::os::{linux::net::SocketAddrExt, unix::net::SocketAddr};
            let listener = UnixListener::bind_addr(&SocketAddr::from_abstract_name(label.as_bytes())?)?;
            (listener, format!("{ABSTRACT}{label}"), None)
        };
        #[cfg(not(target_os = "linux"))]
        let (listener, name, path) = {
            // The per-user temporary folder is closed to other users.
            let path = std::env::temp_dir().join(&label);
            let listener = UnixListener::bind(&path)?;
            let name = path
                .to_str()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "temporary folder name"))?
                .to_owned();
            (listener, name, Some(path))
        };
        listener.set_nonblocking(true)?;
        let server = Self { listener, name, path };
        if !valid_name(&server.name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "transport name too long"));
        }
        Ok(server)
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn accept(self, expected_pid: u32, timeout: Duration) -> io::Result<AuthenticatedSocket> {
        let deadline = Instant::now() + timeout;
        let uid = ipc::own_uid();
        loop {
            let stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    ready(&self.listener, PollFlags::IN, deadline)?;
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if !ipc::peer(&stream).is_ok_and(|peer| authentic(peer, expected_pid, uid)) {
                // A same-user squatter must not make the invocation fail: drop it
                // and keep accepting until the deadline (SEC-06).
                drop(stream);
                continue;
            }
            ipc::configure(&stream)?;
            let mut socket = AuthenticatedSocket { stream, deadline };
            socket.write_all(&[1])?;
            return Ok(socket);
        }
    }
}
impl Drop for UnixTransportServer {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}
impl AuthenticatedSocket {
    pub fn connect(name: &str, expected_server: u32, timeout: Duration) -> io::Result<Self> {
        if !valid_name(name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid transport name"));
        }
        let deadline = Instant::now() + timeout;
        let stream = loop {
            match connect_name(name) {
                Ok(stream) => break stream,
                // Not listening yet, or its backlog is full.
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::WouldBlock | io::ErrorKind::NotFound
                    ) =>
                {
                    pause(deadline)?
                }
                Err(error) => return Err(error),
            }
        };
        if !authentic(ipc::peer(&stream)?, expected_server, ipc::own_uid()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unexpected editor identity",
            ));
        }
        ipc::configure(&stream)?;
        let mut socket = Self { stream, deadline };
        let mut ack = [0];
        socket.read_exact(&mut ack)?;
        if ack != [1] {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "invalid handshake"));
        }
        Ok(socket)
    }
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.deadline = Instant::now() + timeout;
    }
}
impl Read for AuthenticatedSocket {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if Instant::now() >= self.deadline {
                return Err(deadline_error());
            }
            match (&self.stream).read(buffer) {
                Ok(count) => return Ok(count),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    ready(&self.stream, PollFlags::IN, self.deadline)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}
impl Write for AuthenticatedSocket {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if Instant::now() >= self.deadline {
                return Err(deadline_error());
            }
            match ipc::send(&self.stream, buffer) {
                Ok(count) => return Ok(count),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    ready(&self.stream, PollFlags::OUT, self.deadline)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Launch evidence is supplied by the verified package manager using owner-pinned
/// publisher policy. The same structure as on Windows; there is no platform code
/// signature to check here, so `signer` is not consulted: the SHA-256 pins, which
/// come from signed metadata, bind the exact executable and component.
pub struct HostLaunch<'a> {
    pub executable: &'a Path,
    pub executable_sha256: [u8; 32],
    pub signer: &'a bareline_distribution::update::PublisherPin,
    pub component: &'a Path,
    pub component_sha256: [u8; 32],
    pub invocation: &'a Invocation,
    pub budget: ExecutionBudget,
}
#[derive(Clone, Copy, Debug)]
pub enum HostLifecycle {
    Started(u32),
    Authenticated(u32),
    Drained(u32),
}
/// How far a platform sandbox could confine the host. `Unsupported` is never a
/// silent downgrade: a sandbox that reports it either refused to start the host
/// or was explicitly configured to run it with the remaining measures only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Isolation {
    /// The mechanism that confines filesystem access, for example "Landlock ABI 6".
    Enforced(String),
    /// Why filesystem confinement is not available on this system.
    Unsupported(String),
}
impl std::fmt::Display for Isolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Enforced(mechanism) => write!(f, "isolation={mechanism}"),
            Self::Unsupported(reason) => write!(f, "isolation=Unsupported ({reason})"),
        }
    }
}
/// What a platform sandbox needs to start the verified host.
pub struct HostSpawn<'a> {
    pub executable: &'a Path,
    /// The executable that was hashed, still open. Launch from this descriptor
    /// where the system allows, so a path swapped after the check is not run.
    pub executable_file: &'a File,
    pub component: &'a Path,
    pub arguments: Vec<OsString>,
    pub budget: ExecutionBudget,
    /// The transport's socket file, which the host must be allowed to connect
    /// to (macOS); `None` for Linux's abstract socket, which has no file.
    pub socket: Option<&'a Path>,
}
/// The started host as the launch waits for it: a `std::process::Child`, or
/// the child of a sandbox that spawns it another way (macOS `posix_spawn`).
pub trait HostProcess {
    fn id(&self) -> u32;
    /// The exit status once the host exited, reaping it; `None` while it runs.
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    fn wait(&mut self) -> io::Result<ExitStatus>;
}
impl HostProcess for Child {
    fn id(&self) -> u32 {
        Child::id(self)
    }
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }
    fn wait(&mut self) -> io::Result<ExitStatus> {
        Child::wait(self)
    }
}
pub struct SpawnedHost {
    pub child: Box<dyn HostProcess>,
    /// Terminates the host and anything it started.
    pub guard: HostGuard,
    pub isolation: Isolation,
}
/// The platform half of a host launch (Linux: Landlock, rlimits, no-new-privs).
pub trait HostSandbox {
    /// Starts the host contained, or fails without starting it.
    fn spawn_host(&self, spawn: &HostSpawn<'_>) -> io::Result<SpawnedHost>;
}

fn spawn_transport_watchdog(
    run: WatchdogRun,
    spawn: impl FnOnce(String, WatchdogRun) -> io::Result<std::thread::JoinHandle<()>>,
) -> io::Result<std::thread::JoinHandle<()>> {
    spawn("bareline-extension-transport-watchdog".into(), run)
        .map_err(|error| io::Error::new(error.kind(), format!("extension watchdog unavailable: {error}")))
}
fn transfer_guard_to_watchdog(
    sender: mpsc::SyncSender<HostGuard>,
    guard: HostGuard,
    child: &mut dyn HostProcess,
) -> io::Result<()> {
    if let Err(error) = sender.send(guard) {
        let mut guard = error.0;
        let _ = guard.terminate();
        drop(guard);
        // Reaping is bounded so cleanup cannot park the transport worker indefinitely.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "contained extension child did not stop",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "extension watchdog stopped before containment transfer",
        ));
    }
    Ok(())
}
/// Opens a runtime file without following a final symbolic link and requires a
/// regular file, so the hash below describes exactly what is launched or read.
fn open_verified(path: &Path) -> io::Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("runtime file type"));
    }
    Ok(file)
}
fn hash(file: &mut File, limit: u64) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > limit {
            return Err(io::Error::other("runtime file size"));
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finalize().into())
}

/// The host's exit status once it has exited, without reaping it on Linux: the
/// guard signals the host's process group, and the group's number stays the
/// host's only while the host is unreaped, so the host is reaped (by the final
/// `wait`) only after the watchdog dropped the guard. Elsewhere the guard does
/// not rely on the pid, and `try_wait` reaps as usual.
#[cfg(target_os = "linux")]
fn host_exit(child: &mut dyn HostProcess) -> io::Result<Option<ExitStatus>> {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    use std::os::unix::process::ExitStatusExt;
    let pid = i32::try_from(child.id())
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::other("host pid"))?;
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    let Some(status) = waitid(WaitId::Pid(pid), options)? else {
        return Ok(None);
    };
    // The raw wait status `std` decodes: the code in the second byte, or the signal.
    let raw = match (status.exit_status(), status.terminating_signal()) {
        (Some(code), _) => (code & 0xff) << 8,
        (None, Some(signal)) => signal | if status.dumped() { 0x80 } else { 0 },
        (None, None) => return Ok(None),
    };
    Ok(Some(ExitStatus::from_raw(raw)))
}
#[cfg(not(target_os = "linux"))]
fn host_exit(child: &mut dyn HostProcess) -> io::Result<Option<ExitStatus>> {
    child.try_wait()
}
/// Synchronous worker entry. A separate watchdog owns the host's process tree
/// guard and kills it on cancellation or deadline, including compilation or
/// blocked WASI. Returns how the host was isolated. Returning drops all host
/// memory and IPC; installed runtime files are untouched.
pub fn run_verified_host_in(
    sandbox: &dyn HostSandbox,
    launch: HostLaunch<'_>,
    cancelled: Arc<AtomicBool>,
    mut observe: impl FnMut(HostLifecycle),
    mut broker: impl FnMut(Envelope) -> BrokerResponse,
) -> io::Result<Isolation> {
    let mut executable = open_verified(launch.executable)?;
    if hash(&mut executable, 256 * 1024 * 1024)? != launch.executable_sha256 {
        return Err(io::Error::other("runtime hash"));
    }
    let mut component = open_verified(launch.component)?;
    if hash(&mut component, 32 * 1024 * 1024)? != launch.component_sha256 {
        return Err(io::Error::other("component hash"));
    }
    drop(component);
    if launch.invocation.arguments.len() > 4096
        || !valid_id(&launch.invocation.extension_id)
        || !valid_id(&launch.invocation.command)
    {
        return Err(io::Error::other("invalid invocation"));
    }
    let server = UnixTransportServer::create()?;
    let spawn = HostSpawn {
        executable: launch.executable,
        executable_file: &executable,
        component: launch.component,
        arguments: vec![
            server.name().into(),
            std::process::id().to_string().into(),
            launch.component.as_os_str().to_owned(),
            launch
                .component_sha256
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                .into(),
            match launch.budget {
                ExecutionBudget::Interactive => "interactive",
                ExecutionBudget::Background => "background",
            }
            .into(),
        ],
        budget: launch.budget,
        socket: server.path.as_deref(),
    };
    let (stop_tx, stop_rx) = mpsc::channel();
    let (guard_tx, guard_rx) = mpsc::sync_channel::<HostGuard>(1);
    let abandoned = cancelled.clone();
    let deadline = Instant::now() + Duration::from_millis(launch.budget.timeout_ms());
    // The watchdog must exist before the untrusted process can run. It receives
    // the process tree guard after launch.
    let watchdog = spawn_transport_watchdog(
        Box::new(move || {
            let Ok(mut guard) = guard_rx.recv() else {
                return;
            };
            loop {
                if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                    let _ = guard.terminate();
                    return;
                }
                match stop_rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }),
        |name, run| std::thread::Builder::new().name(name).spawn(run),
    )?;
    if abandoned.load(Ordering::Acquire) {
        drop(guard_tx);
        let _ = stop_tx.send(());
        let _ = watchdog.join();
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "extension cancelled before launch",
        ));
    }
    let SpawnedHost {
        mut child,
        guard,
        isolation,
    } = match sandbox.spawn_host(&spawn) {
        Ok(spawned) => spawned,
        Err(error) => {
            drop(guard_tx);
            let _ = stop_tx.send(());
            let _ = watchdog.join();
            return Err(error);
        }
    };
    drop(executable);
    let pid = child.id();
    if let Err(error) = transfer_guard_to_watchdog(guard_tx, guard, &mut *child) {
        let _ = stop_tx.send(());
        let _ = watchdog.join();
        return Err(error);
    }
    observe(HostLifecycle::Started(pid));
    let outcome = (|| {
        let mut socket = server.accept(pid, Duration::from_millis(INTERACTIVE_TIMEOUT_MS))?;
        observe(HostLifecycle::Authenticated(pid));
        socket.set_timeout(Duration::from_millis(launch.budget.timeout_ms()));
        let context = encode(launch.invocation).map_err(|_| io::Error::other("invocation encoding"))?;
        socket.write_all(&(context.len() as u32).to_le_bytes())?;
        socket.write_all(&context)?;
        loop {
            match read_frame(&mut socket) {
                Ok(message) => {
                    // The parent launch identity cannot be replaced by a guest envelope.
                    if message.extension_id != launch.invocation.extension_id
                        || message.context.grant_generation != launch.invocation.grant_generation
                    {
                        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "guest identity"));
                    }
                    let response = broker(message);
                    let bytes = encode(&response).map_err(|_| io::Error::other("broker reply limit"))?;
                    socket.write_all(&(bytes.len() as u32).to_le_bytes())?;
                    socket.write_all(&bytes)?;
                }
                Err(ProtocolError::Io) => {
                    // Do not park this worker on a host that closed the socket and
                    // idled: give it a short grace period, then kill it (SEC-11).
                    // Cancelling makes the watchdog kill the host's tree; the
                    // exit is then observed like any other.
                    let grace = Instant::now() + Duration::from_millis(2000);
                    let status = loop {
                        if let Some(status) = host_exit(&mut *child)? {
                            break status;
                        }
                        if Instant::now() >= grace {
                            abandoned.store(true, Ordering::Release);
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    };
                    if status.success() {
                        return Ok(());
                    }
                    return Err(io::Error::other(format!("extension host stopped ({status})")));
                }
                Err(error) => {
                    return Err(io::Error::other(format!("extension protocol: {error}")));
                }
            }
        }
    })();
    // Stopping the watchdog drops the guard, which kills what is left of the host.
    let _ = stop_tx.send(());
    let watchdog_stopped = watchdog.join().is_err();
    let _ = child.wait();
    observe(HostLifecycle::Drained(pid));
    if watchdog_stopped {
        Err(io::Error::other("extension watchdog stopped"))
    } else {
        outcome.map(|()| isolation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_extensions_protocol::{BrokerValue, MAX_CHUNK_BYTES, decode};

    #[test]
    fn framed_responses_larger_than_the_socket_buffer_make_bounded_progress() {
        let server = UnixTransportServer::create().unwrap();
        let name = server.name().to_owned();
        let pid = std::process::id();
        let worker = std::thread::spawn(move || {
            let mut socket = server.accept(pid, Duration::from_secs(5)).unwrap();
            for length in [65536usize, 131072] {
                let bytes = encode(&BrokerResponse {
                    request_id: length as u64,
                    result: Ok(BrokerValue::Bytes((0..length).map(|i| (i % 251) as u8).collect())),
                })
                .unwrap();
                assert!(bytes.len() > 65536);
                socket.write_all(&(bytes.len() as u32).to_le_bytes()).unwrap();
                socket.write_all(&bytes).unwrap();
                let mut ack = [0];
                socket.read_exact(&mut ack).unwrap();
                assert_eq!(ack, [1]);
            }
        });
        let mut client = AuthenticatedSocket::connect(&name, pid, Duration::from_secs(5)).unwrap();
        for length in [65536usize, 131072] {
            let mut prefix = [0; 4];
            client.read_exact(&mut prefix).unwrap();
            let count = u32::from_le_bytes(prefix) as usize;
            assert!(count <= MAX_CHUNK_BYTES);
            let mut bytes = vec![0; count];
            client.read_exact(&mut bytes).unwrap();
            let response: BrokerResponse = decode(&bytes).unwrap();
            assert_eq!(response.request_id, length as u64);
            match response.result.unwrap() {
                BrokerValue::Bytes(bytes) => {
                    assert_eq!(bytes, (0..length).map(|i| (i % 251) as u8).collect::<Vec<_>>())
                }
                _ => panic!("expected byte response"),
            }
            client.write_all(&[1]).unwrap();
        }
        worker.join().unwrap();
    }
    #[test]
    fn same_user_socket_requires_the_expected_process() {
        let server = UnixTransportServer::create().unwrap();
        let name = server.name().to_owned();
        let pid = std::process::id();
        let worker = std::thread::spawn(move || {
            let mut socket = server.accept(pid, Duration::from_secs(5)).unwrap();
            let mut data = [0; 3];
            socket.read_exact(&mut data).unwrap();
            assert_eq!(&data, b"rpc");
        });
        let mut client = AuthenticatedSocket::connect(&name, pid, Duration::from_secs(5)).unwrap();
        client.write_all(b"rpc").unwrap();
        worker.join().unwrap();
    }
    #[test]
    fn a_wrong_process_is_refused_on_both_ends() {
        // The editor expects another pid: this process is a squatter, dropped
        // without a handshake, and the editor keeps waiting until its deadline.
        let server = UnixTransportServer::create().unwrap();
        let name = server.name().to_owned();
        let squatter = std::thread::spawn(move || {
            AuthenticatedSocket::connect(&name, std::process::id(), Duration::from_millis(500))
                .err()
                .map(|error| error.kind())
        });
        let error = server.accept(1, Duration::from_millis(300)).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        // The squatter never received the handshake byte.
        assert!(squatter.join().unwrap().is_some());
        // The host expects another editor pid: refused before any byte is read.
        let server = UnixTransportServer::create().unwrap();
        let error = AuthenticatedSocket::connect(server.name(), 1, Duration::from_secs(2))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }
    #[test]
    fn identity_requires_both_the_process_and_the_user() {
        let uid = ipc::own_uid();
        let pid = std::process::id();
        assert!(authentic(ipc::Peer { pid, uid }, pid, uid));
        assert!(!authentic(ipc::Peer { pid: pid + 1, uid }, pid, uid));
        // Another user's process with the right pid (a pid namespace, a reused
        // pid) is still refused.
        assert!(!authentic(ipc::Peer { pid, uid: uid ^ 1 }, pid, uid));
    }
    #[test]
    fn missing_client_and_silent_peer_time_out() {
        let server = UnixTransportServer::create().unwrap();
        assert_eq!(
            server.accept(123, Duration::from_millis(20)).err().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
        // Nobody listens under this name: the connect gives up at its deadline.
        let unused = format!(
            "{}{NAME_PREFIX}{}-unused",
            if cfg!(target_os = "linux") { "@" } else { "/tmp/" },
            std::process::id()
        );
        assert_eq!(
            AuthenticatedSocket::connect(&unused, 1, Duration::from_millis(20))
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::TimedOut
        );
        // A connected peer that never writes: reads end at the socket's deadline.
        let server = UnixTransportServer::create().unwrap();
        let name = server.name().to_owned();
        let pid = std::process::id();
        let editor = std::thread::spawn(move || {
            let socket = server.accept(pid, Duration::from_secs(5)).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            drop(socket);
        });
        let mut client = AuthenticatedSocket::connect(&name, pid, Duration::from_secs(5)).unwrap();
        client.set_timeout(Duration::from_millis(30));
        assert_eq!(client.read(&mut [0; 4]).unwrap_err().kind(), io::ErrorKind::TimedOut);
        editor.join().unwrap();
    }
    #[test]
    fn names_outside_the_transport_are_refused() {
        for name in ["", "bareline-exthost-1", "/tmp/other.sock", "@other"] {
            assert_eq!(
                AuthenticatedSocket::connect(name, 1, Duration::from_millis(10))
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidInput,
                "{name}"
            );
        }
        let server = UnixTransportServer::create().unwrap();
        assert!(valid_name(server.name()), "{}", server.name());
    }
    #[test]
    fn watchdog_spawn_failure_is_terminal_before_launch() {
        let ran = Arc::new(AtomicBool::new(false));
        let worker_ran = ran.clone();
        let result = spawn_transport_watchdog(
            Box::new(move || worker_ran.store(true, Ordering::SeqCst)),
            |name, _run| {
                assert_eq!(name, "bareline-extension-transport-watchdog");
                Err(io::Error::other("controlled watchdog spawn failure"))
            },
        );
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Other);
        assert!(!ran.load(Ordering::SeqCst));
    }
    #[test]
    fn failed_watchdog_transfer_terminates_and_reaps_the_child() {
        struct Kill(u32);
        impl ProcessTreeGuard for Kill {
            fn terminate(&mut self) -> io::Result<()> {
                let pid = rustix::process::Pid::from_raw(self.0 as i32).ok_or_else(|| io::Error::other("pid"))?;
                rustix::process::kill_process(pid, rustix::process::Signal::KILL).map_err(io::Error::from)
            }
        }
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let guard: HostGuard = Box::new(Kill(child.id()));
        let (sender, receiver) = mpsc::sync_channel::<HostGuard>(1);
        drop(receiver);
        let error = transfer_guard_to_watchdog(sender, guard, &mut child).unwrap_err();
        // The child sleeps far past the bounded reap, which reports TimedOut if it
        // outlives it; BrokenPipe proves it was stopped.
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(child.try_wait().unwrap().is_some());
    }
    #[test]
    fn verification_refuses_links_and_other_bytes_before_any_launch() {
        struct NeverSpawn;
        impl HostSandbox for NeverSpawn {
            fn spawn_host(&self, _spawn: &HostSpawn<'_>) -> io::Result<SpawnedHost> {
                panic!("an unverified host must never be started");
            }
        }
        let root = std::env::temp_dir().join(format!(
            "bl-verify-{}-{}",
            std::process::id(),
            ipc::random_hex::<8>().unwrap()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (executable, component, link) = (root.join("host"), root.join("c.wasm"), root.join("link"));
        std::fs::write(&executable, b"host").unwrap();
        std::fs::write(&component, b"component").unwrap();
        std::os::unix::fs::symlink(&executable, &link).unwrap();
        let signer = bareline_distribution::update::PublisherPin {
            subject: String::new(),
            issuers: Vec::new(),
        };
        let invocation = Invocation {
            extension_id: "fixture".into(),
            command: "fixture.run".into(),
            arguments: String::new(),
            document: 1,
            revision: 1,
            source_generation: 1,
            text_length: 0,
            raw_length: 0,
            grant_generation: 1,
        };
        let digest = |bytes: &[u8]| -> [u8; 32] { Sha256::digest(bytes).into() };
        let run = |executable: &Path, executable_sha256: [u8; 32], component_sha256: [u8; 32]| {
            run_verified_host_in(
                &NeverSpawn,
                HostLaunch {
                    executable,
                    executable_sha256,
                    signer: &signer,
                    component: &component,
                    component_sha256,
                    invocation: &invocation,
                    budget: ExecutionBudget::Interactive,
                },
                Arc::new(AtomicBool::new(false)),
                |_| panic!("nothing starts"),
                |_| panic!("nothing is brokered"),
            )
            .unwrap_err()
            .to_string()
        };
        assert_eq!(run(&executable, digest(b"other"), digest(b"component")), "runtime hash");
        assert_eq!(run(&executable, digest(b"host"), digest(b"other")), "component hash");
        // The right bytes behind a link are still refused: the path itself must be the file.
        assert!(!run(&link, digest(b"host"), digest(b"component")).contains("hash"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
