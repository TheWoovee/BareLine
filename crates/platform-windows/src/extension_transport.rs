// SPDX-License-Identifier: MPL-2.0
//! Same-user, local-only, nonblocking named pipes. Caller runs this bounded I/O on
//! its host worker, never on the UI thread. An unguessable name is not sufficient:
//! both endpoints additionally check the expected process and session identities.
use std::{
    io::{self, Read, Write},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Security::{Authorization::*, Cryptography::*, *},
        Storage::FileSystem::*,
        System::{Pipes::*, RemoteDesktop::ProcessIdToSessionId, Threading::*},
    },
    core::{PCWSTR, PWSTR},
};
type WatchdogRun = Box<dyn FnOnce() + Send + 'static>;
type HostGuard = Box<dyn bareline_macros::process::ProcessTreeGuard>;

fn spawn_transport_watchdog(
    run: WatchdogRun,
    spawn: impl FnOnce(String, WatchdogRun) -> io::Result<std::thread::JoinHandle<()>>,
) -> io::Result<std::thread::JoinHandle<()>> {
    spawn("bareline-extension-transport-watchdog".into(), run)
        .map_err(|error| io::Error::new(error.kind(), format!("extension watchdog unavailable: {error}")))
}

fn transfer_guard_to_watchdog(
    sender: std::sync::mpsc::SyncSender<HostGuard>,
    guard: HostGuard,
    child: &mut crate::SandboxedChild,
) -> io::Result<()> {
    if let Err(error) = sender.send(guard) {
        let mut guard = error.0;
        let _ = guard.terminate();
        // Closing the kill-on-close guard is the fallback if explicit
        // termination itself failed. Reaping is bounded so cleanup cannot park
        // the transport worker indefinitely.
        drop(guard);
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

fn io_error(error: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0 & 0xffff)
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
// The handle is exclusively owned; Windows pipe handles can move across workers.
unsafe impl Send for Handle {}
pub struct PipeServer {
    handle: Handle,
    name: String,
    session: u32,
}
pub struct AuthenticatedPipe {
    handle: Handle,
    deadline: Instant,
}
impl PipeServer {
    pub fn create() -> io::Result<Self> {
        unsafe {
            // Random pipe-name suffix only. There is no launch "nonce": it would be
            // readable by any same-user process, so authentication rests entirely on
            // the client PID/session check below (SEC-05).
            let mut random = [0; 32];
            BCryptGenRandom(None, &mut random, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
                .ok()
                .map_err(io_error)?;
            let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let name = format!(r"\\.\pipe\bareline-exthost-{}-{suffix}", GetCurrentProcessId());
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(io_error)?;
            let token = Handle(token);
            let mut needed = 0;
            let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut needed);
            if needed == 0 || needed > 65536 {
                return Err(io::Error::other("token size"));
            }
            let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )
            .map_err(io_error)?;
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut sid = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut sid).map_err(io_error)?;
            let sid_text = sid.to_string().map_err(io::Error::other);
            let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
            let sid_text = sid_text?;
            // Grant the launching user full access and the RESTRICTED code SID (RC) the same,
            // so the sandboxed host running under a restricted token (SEC-03) can still open
            // this pipe; the PID/session check remains the real authenticator.
            let descriptor_text: Vec<u16> = format!("D:P(A;;GA;;;{sid_text})(A;;GA;;;RC)")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(descriptor_text.as_ptr()),
                1,
                &mut descriptor,
                None,
            )
            .map_err(io_error)?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            let name_wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let raw = CreateNamedPipeW(
                PCWSTR(name_wide.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                65536,
                65536,
                5000,
                Some(&attributes),
            );
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            if raw == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let handle = Handle(raw);
            let mut session = 0;
            ProcessIdToSessionId(GetCurrentProcessId(), &mut session).map_err(io_error)?;
            Ok(Self { handle, name, session })
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn accept(self, expected_pid: u32, timeout: Duration) -> io::Result<AuthenticatedPipe> {
        let deadline = Instant::now() + timeout;
        loop {
            let connected = unsafe { ConnectNamedPipe(self.handle.0, None) };
            match connected {
                Ok(()) => {}
                Err(error) if error.code().0 as u32 & 0xffff == ERROR_PIPE_CONNECTED.0 => {}
                Err(error) if error.code().0 as u32 & 0xffff == ERROR_PIPE_LISTENING.0 => {
                    pause(deadline)?;
                    continue;
                }
                Err(error) => return Err(io_error(error)),
            }
            let mut pid = 0;
            let mut session = 0;
            unsafe {
                GetNamedPipeClientProcessId(self.handle.0, &mut pid).map_err(io_error)?;
                GetNamedPipeClientSessionId(self.handle.0, &mut session).map_err(io_error)?;
            }
            if pid != expected_pid || session != self.session {
                // A same-user squatter must not make the invocation fail: drop it and
                // keep accepting until the deadline (SEC-06).
                unsafe {
                    let _ = DisconnectNamedPipe(self.handle.0);
                }
                pause(deadline)?;
                continue;
            }
            let mut pipe = AuthenticatedPipe {
                handle: self.handle,
                deadline,
            };
            pipe.write_all(&[1])?;
            return Ok(pipe);
        }
    }
}
impl AuthenticatedPipe {
    pub fn connect(name: &str, expected_server: u32, timeout: Duration) -> io::Result<Self> {
        if !name.starts_with(r"\\.\pipe\bareline-exthost-") || name.len() > 256 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid pipe name"));
        }
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let deadline = Instant::now() + timeout;
        let handle = loop {
            match unsafe {
                CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_MODE(0),
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OPEN_REPARSE_POINT | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    None,
                )
            } {
                Ok(h) => break Handle(h),
                Err(error) if error.code().0 as u32 & 0xffff == ERROR_PIPE_BUSY.0 => pause(deadline)?,
                Err(error) => return Err(io_error(error)),
            }
        };
        let mut pid = 0;
        let mut session = 0;
        let mut own_session = 0;
        unsafe {
            GetNamedPipeServerProcessId(handle.0, &mut pid).map_err(io_error)?;
            GetNamedPipeServerSessionId(handle.0, &mut session).map_err(io_error)?;
            ProcessIdToSessionId(GetCurrentProcessId(), &mut own_session).map_err(io_error)?;
        }
        if pid != expected_server || session != own_session {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unexpected editor identity",
            ));
        }
        unsafe {
            SetNamedPipeHandleState(handle.0, Some(&(PIPE_READMODE_BYTE | PIPE_NOWAIT)), None, None)
                .map_err(io_error)?;
        }
        let mut pipe = Self { handle, deadline };
        let mut ack = [0];
        pipe.read_exact(&mut ack)?;
        if ack != [1] {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "invalid handshake"));
        }
        Ok(pipe)
    }
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.deadline = Instant::now() + timeout;
    }
}
fn pause(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "extension IPC deadline"));
    }
    std::thread::sleep(Duration::from_millis(2));
    Ok(())
}
impl Read for AuthenticatedPipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if Instant::now() >= self.deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "extension IPC deadline"));
            }
            let mut count = 0;
            match unsafe { ReadFile(self.handle.0, Some(buffer), Some(&mut count), None) } {
                Ok(()) if count > 0 => return Ok(count as usize),
                Ok(()) => pause(self.deadline)?,
                Err(error) if error.code().0 as u32 & 0xffff == ERROR_NO_DATA.0 => pause(self.deadline)?,
                Err(error) => return Err(io_error(error)),
            }
        }
    }
}
impl Write for AuthenticatedPipe {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        // A framed 64 KiB byte response is larger than the pipe's 64 KiB
        // outbound quota. Offering that whole buffer to a NOWAIT write can make
        // no progress even while the peer is waiting for its body. write_all
        // already handles partial writes; keep each OS offer below that quota.
        let buffer = &buffer[..buffer.len().min(16 * 1024)];
        loop {
            if Instant::now() >= self.deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "extension IPC deadline"));
            }
            let mut count = 0;
            match unsafe { WriteFile(self.handle.0, Some(buffer), Some(&mut count), None) } {
                Ok(()) if count > 0 => return Ok(count as usize),
                Ok(()) => pause(self.deadline)?,
                Err(error) if error.code().0 as u32 & 0xffff == ERROR_NO_DATA.0 => pause(self.deadline)?,
                Err(error) => return Err(io_error(error)),
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framed_responses_larger_than_pipe_quota_make_bounded_progress() {
        use bareline_extensions_protocol::{BrokerResponse, BrokerValue, decode, encode};
        let server = PipeServer::create().unwrap();
        let name = server.name().to_owned();
        let pid = unsafe { GetCurrentProcessId() };
        let worker = std::thread::spawn(move || {
            let mut pipe = server.accept(pid, Duration::from_secs(2)).unwrap();
            for length in [65536usize, 131072] {
                let bytes = encode(&BrokerResponse {
                    request_id: length as u64,
                    result: Ok(BrokerValue::Bytes((0..length).map(|i| (i % 251) as u8).collect())),
                })
                .unwrap();
                assert!(bytes.len() > 65536);
                pipe.write_all(&(bytes.len() as u32).to_le_bytes()).unwrap();
                pipe.write_all(&bytes).unwrap();
                let mut ack = [0];
                pipe.read_exact(&mut ack).unwrap();
                assert_eq!(ack, [1]);
            }
        });
        let mut client = AuthenticatedPipe::connect(&name, pid, Duration::from_secs(2)).unwrap();
        for length in [65536usize, 131072] {
            let mut prefix = [0; 4];
            client.read_exact(&mut prefix).unwrap();
            let count = u32::from_le_bytes(prefix) as usize;
            assert!(count <= bareline_extensions_protocol::MAX_CHUNK_BYTES);
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
    fn same_user_pipe_requires_expected_process() {
        let server = PipeServer::create().unwrap();
        let name = server.name().to_owned();
        let pid = unsafe { GetCurrentProcessId() };
        let worker = std::thread::spawn(move || {
            let mut pipe = server.accept(pid, Duration::from_secs(2)).unwrap();
            let mut data = [0; 3];
            pipe.read_exact(&mut data).unwrap();
            assert_eq!(&data, b"rpc");
        });
        let mut client = AuthenticatedPipe::connect(&name, pid, Duration::from_secs(2)).unwrap();
        client.write_all(b"rpc").unwrap();
        worker.join().unwrap();
    }
    #[test]
    fn missing_client_times_out() {
        let server = PipeServer::create().unwrap();
        assert_eq!(
            server.accept(123, Duration::from_millis(20)).err().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
    }
    #[test]
    fn watchdog_spawn_failure_is_terminal_before_launch() {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_ran = ran.clone();
        let result = spawn_transport_watchdog(
            Box::new(move || worker_ran.store(true, std::sync::atomic::Ordering::SeqCst)),
            |name, _run| {
                assert_eq!(name, "bareline-extension-transport-watchdog");
                Err(io::Error::other("controlled watchdog spawn failure"))
            },
        );
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Other);
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    }
    #[test]
    #[ignore = "controlled contained subprocess fixture"]
    fn contained_child_for_guard_transfer_failure() {
        std::thread::sleep(Duration::from_secs(30));
    }
    #[test]
    fn failed_watchdog_transfer_terminates_and_reaps_contained_child() {
        use bareline_macros::process::ProcessLauncher;
        let executable = std::env::current_exe().unwrap();
        let mut command = std::process::Command::new(executable);
        command.args([
            "--ignored",
            "--exact",
            "extension_transport::tests::contained_child_for_guard_transfer_failure",
        ]);
        let (child, guard) = crate::WindowsProcessLauncher.spawn(&mut command).unwrap();
        let mut child = crate::SandboxedChild::Std(child);
        let (sender, receiver) = std::sync::mpsc::sync_channel::<HostGuard>(1);
        drop(receiver);
        let started = Instant::now();
        let error = transfer_guard_to_watchdog(sender, guard, &mut child).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(child.try_wait().unwrap().is_some());
    }
}

/// Launch evidence is supplied by the verified package manager using owner-pinned
/// publisher policy. Paths remain locked against writes/deletion while executing.
pub struct HostLaunch<'a> {
    pub executable: &'a std::path::Path,
    pub executable_sha256: [u8; 32],
    pub publisher_certificate_sha256: [u8; 32],
    pub component: &'a std::path::Path,
    pub component_sha256: [u8; 32],
    pub invocation: &'a bareline_extensions_protocol::Invocation,
    pub budget: bareline_extensions_protocol::ExecutionBudget,
}
/// Synchronous worker entry. A separate watchdog owns the Job Object and kills the
/// entire tree on cancellation/deadline, including compilation or blocked WASI.
/// Returning drops all host memory/IPC; installed runtime files are untouched.
pub fn run_verified_host(
    launch: HostLaunch<'_>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    broker: impl FnMut(bareline_extensions_protocol::Envelope) -> bareline_extensions_protocol::BrokerResponse,
) -> io::Result<()> {
    run_verified_host_observed(launch, cancelled, |_| {}, broker)
}
#[derive(Clone, Copy, Debug)]
pub enum HostLifecycle {
    Started(u32),
    Authenticated(u32),
    Drained(u32),
}
pub fn run_verified_host_observed(
    launch: HostLaunch<'_>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mut observe: impl FnMut(HostLifecycle),
    mut broker: impl FnMut(bareline_extensions_protocol::Envelope) -> bareline_extensions_protocol::BrokerResponse,
) -> io::Result<()> {
    use sha2::{Digest, Sha256};
    let mut executable = crate::update::open_update_file(launch.executable)?;
    crate::update::verify_authenticode(&executable, &launch.publisher_certificate_sha256)
        .map_err(|e| io::Error::other(format!("runtime publisher: {e:?}")))?;
    fn hash(file: &mut std::fs::File, limit: u64) -> io::Result<[u8; 32]> {
        let mut digest = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0; 65536];
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
    if hash(&mut executable, 256 * 1024 * 1024)? != launch.executable_sha256 {
        return Err(io::Error::other("runtime hash"));
    }
    let mut component = crate::update::open_update_file(launch.component)?;
    if hash(&mut component, 32 * 1024 * 1024)? != launch.component_sha256 {
        return Err(io::Error::other("component hash"));
    }
    run_host_process(launch, cancelled, &mut observe, &mut broker)
}
fn run_host_process(
    launch: HostLaunch<'_>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    observe: &mut impl FnMut(HostLifecycle),
    broker: &mut impl FnMut(bareline_extensions_protocol::Envelope) -> bareline_extensions_protocol::BrokerResponse,
) -> io::Result<()> {
    use bareline_extensions_protocol::*;
    use std::{
        process::{Command, Stdio},
        sync::{atomic::Ordering, mpsc},
    };
    if launch.invocation.arguments.len() > 4096
        || !valid_id(&launch.invocation.extension_id)
        || !valid_id(&launch.invocation.command)
    {
        return Err(io::Error::other("invalid invocation"));
    }
    let server = PipeServer::create()?;
    let mut command = Command::new(launch.executable);
    command
        .args([server.name(), &std::process::id().to_string()])
        .arg(launch.component)
        .arg(
            launch
                .component_sha256
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        )
        .arg(match launch.budget {
            ExecutionBudget::Interactive => "interactive",
            ExecutionBudget::Background => "background",
        })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The host inherits no user environment and no caller working directory, so guest
    // code cannot read %USERPROFILE%/%APPDATA% locations by name (SEC-03).
    command.env_clear();
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let system32 = std::path::Path::new(&system_root).join("System32");
    command.env("SystemRoot", &system_root);
    command.env("windir", &system_root);
    command.env("Path", &system32);
    command.env("PATHEXT", ".COM;.EXE;.BAT;.CMD");
    command.env("NoDefaultCurrentDirectoryInExePath", "1");
    for name in ["SystemDrive", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if let Some(runtime) = launch.executable.parent() {
        command.current_dir(runtime);
    }
    // Grant the runtime executable and component a RESTRICTED-SID read ACE so the sandboxed
    // host (SEC-03) can still load exactly those files, then launch it under the restricted
    // token; a machine that cannot build the token falls back to the job-limited launch.
    let (stop_tx, stop_rx) = mpsc::channel();
    let (guard_tx, guard_rx) = mpsc::sync_channel::<HostGuard>(1);
    let abandoned = cancelled.clone();
    let deadline = Instant::now() + Duration::from_millis(launch.budget.timeout_ms());
    // The watchdog must exist before the untrusted process can be created and
    // resumed. It receives the already-contained job guard after launch.
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
    let spawned =
        crate::SandboxedProcessLauncher::default().spawn_host(&mut command, &[launch.executable, launch.component]);
    let (mut child, guard) = match spawned {
        Ok(spawned) => spawned,
        Err(error) => {
            drop(guard_tx);
            let _ = stop_tx.send(());
            let _ = watchdog.join();
            return Err(error);
        }
    };
    let pid = child.id();
    if let Err(error) = transfer_guard_to_watchdog(guard_tx, guard, &mut child) {
        let _ = stop_tx.send(());
        let _ = watchdog.join();
        return Err(error);
    }
    observe(HostLifecycle::Started(pid));
    let outcome = (|| {
        let mut pipe = server.accept(child.id(), Duration::from_millis(INTERACTIVE_TIMEOUT_MS))?;
        observe(HostLifecycle::Authenticated(pid));
        pipe.set_timeout(Duration::from_millis(launch.budget.timeout_ms()));
        let context = encode(launch.invocation).map_err(|_| io::Error::other("invocation encoding"))?;
        pipe.write_all(&(context.len() as u32).to_le_bytes())?;
        pipe.write_all(&context)?;
        loop {
            match read_frame(&mut pipe) {
                Ok(message) => {
                    // The parent launch identity cannot be replaced by a guest envelope.
                    if message.extension_id != launch.invocation.extension_id
                        || message.context.grant_generation != launch.invocation.grant_generation
                    {
                        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "guest identity"));
                    }
                    let response = broker(message);
                    let bytes = encode(&response).map_err(|_| io::Error::other("broker reply limit"))?;
                    pipe.write_all(&(bytes.len() as u32).to_le_bytes())?;
                    pipe.write_all(&bytes)?;
                }
                Err(ProtocolError::Io) => {
                    // Do not park this worker on a host that closed the pipe and idled:
                    // give it a short grace period, then kill the job (SEC-11).
                    let grace = Instant::now() + Duration::from_millis(2000);
                    let status = loop {
                        match child.try_wait()? {
                            Some(status) => break status,
                            None if Instant::now() >= grace => {
                                abandoned.store(true, Ordering::Release);
                                break child.wait()?;
                            }
                            None => std::thread::sleep(Duration::from_millis(10)),
                        }
                    };
                    if status.success() {
                        return Ok(());
                    }
                    return Err(io::Error::other(format!("extension host stopped ({status})")));
                }
                Err(error) => {
                    return Err(io::Error::other(format!("extension protocol: {error:?}")));
                }
            }
        }
    })();
    // Stop signal drops kill-on-close job before waiting for any surviving child.
    let _ = stop_tx.send(());
    let watchdog_stopped = watchdog.join().is_err();
    let _ = child.wait();
    observe(HostLifecycle::Drained(pid));
    if watchdog_stopped {
        Err(io::Error::other("extension watchdog stopped"))
    } else {
        outcome
    }
}
