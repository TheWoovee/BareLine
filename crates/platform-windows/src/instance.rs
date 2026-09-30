// SPDX-License-Identifier: MPL-2.0
//! Bounded per-user/session/profile handoff. No document or network reads occur here.
//!
//! The client writes a framed request; the owner validates it, reserves a queue slot
//! and answers `PREPARED`; the client answers `COMMIT`; only a received commit queues
//! the request, which the owner confirms with `DONE`. The pipe workers never wait for
//! the owner's UI thread, so a pending first frame or a modal loop cannot fail a
//! handoff (APP-03), and exactly one process acts on each request (APP-04).
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt,
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        Storage::FileSystem::*,
        System::{IO::*, Pipes::*, RemoteDesktop::ProcessIdToSessionId, Threading::*},
    },
    core::{PCWSTR, PWSTR},
};

const LIMIT: usize = 64 * 1024;
/// Client budget to connect and be prepared, from launch. A connection's handshake
/// never gets more than this from the moment it connects either.
const TIMEOUT: Duration = Duration::from_secs(2);
/// Further client budget to write the commit, and then again to see it confirmed,
/// so a launch waits at most `TIMEOUT + 2 * COMMIT_WINDOW` for an owner.
const COMMIT_WINDOW: Duration = Duration::from_millis(500);
/// The owner's clock starts when a client connects, so it outwaits the latest commit
/// any client writes by a second: a commit that was written is always read (APP-04).
const SERVER_TIMEOUT: Duration = TIMEOUT
    .saturating_add(COMMIT_WINDOW)
    .saturating_add(Duration::from_secs(1));
/// Concurrent handoffs, e.g. an Explorer multi-select that starts one process per file.
const INSTANCES: u32 = 4;
/// Committed requests held while the owner's UI thread is busy.
const QUEUE: usize = 64;
const PREPARED: u8 = 1;
const COMMIT: u8 = 2;
const DONE: u8 = 3;
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
    Primary(InstanceServer),
    Independent(String),
}
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
pub struct InstanceServer {
    queue: Arc<HandoffQueue>,
    stop: Arc<Handle>,
    workers: Vec<std::thread::JoinHandle<()>>,
    /// Held while this process owns the profile, so no second owner shares it (APP-14).
    _profile: Option<std::fs::File>,
}
impl InstanceServer {
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
}
impl Drop for InstanceServer {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop.0);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
fn err(error: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0 & 0xffff)
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid instance request")
}
fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "instance handoff deadline")
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn sid(process: HANDLE) -> io::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).map_err(err)?;
        let token = Handle(token);
        let mut needed = 0;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut needed);
        if needed == 0 || needed > LIMIT as u32 {
            return Err(invalid());
        }
        let mut data = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        GetTokenInformation(token.0, TokenUser, Some(data.as_mut_ptr().cast()), needed, &mut needed).map_err(err)?;
        let user = &*data.as_ptr().cast::<TOKEN_USER>();
        let mut result = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut result).map_err(err)?;
        let value = result.to_string().map_err(io::Error::other);
        let _ = LocalFree(Some(HLOCAL(result.0.cast())));
        value
    }
}
fn authenticate(pipe: HANDLE, server: bool, user: &str, session: u32) -> io::Result<()> {
    unsafe {
        let mut pid = 0;
        let mut actual_session = 0;
        if server {
            GetNamedPipeClientProcessId(pipe, &mut pid).map_err(err)?;
            GetNamedPipeClientSessionId(pipe, &mut actual_session).map_err(err)?;
        } else {
            GetNamedPipeServerProcessId(pipe, &mut pid).map_err(err)?;
            GetNamedPipeServerSessionId(pipe, &mut actual_session).map_err(err)?;
        }
        if actual_session != session {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "instance session mismatch",
            ));
        }
        let process = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(err)?);
        if sid(process.0)? != user {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "instance user mismatch",
            ));
        }
        Ok(())
    }
}
fn event() -> io::Result<Handle> {
    unsafe { CreateEventW(None, true, false, None).map(Handle).map_err(err) }
}
fn complete(handle: HANDLE, overlapped: &mut OVERLAPPED, event: HANDLE, stop: HANDLE, timeout: u32) -> io::Result<u32> {
    unsafe {
        let result = WaitForMultipleObjects(&[event, stop], false, timeout);
        if result != WAIT_OBJECT_0 {
            let _ = CancelIoEx(handle, Some(overlapped));
            let mut count = 0;
            // The operation may have finished before the cancel; what it moved counts,
            // so a commit that arrived is honoured.
            if GetOverlappedResult(handle, overlapped, &mut count, true).is_ok() && count > 0 {
                return Ok(count);
            }
            return Err(timed_out());
        }
        let mut count = 0;
        GetOverlappedResult(handle, overlapped, &mut count, false).map_err(err)?;
        Ok(count)
    }
}
fn transfer(handle: HANDLE, stop: HANDLE, bytes: &mut [u8], write: bool, deadline: Instant) -> io::Result<()> {
    let mut done = 0;
    while done < bytes.len() {
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u32::MAX as u128) as u32;
        if timeout == 0 {
            return Err(timed_out());
        }
        let event = event()?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let mut count = 0;
        let result = unsafe {
            if write {
                WriteFile(handle, Some(&bytes[done..]), Some(&mut count), Some(&mut overlapped))
            } else {
                ReadFile(
                    handle,
                    Some(&mut bytes[done..]),
                    Some(&mut count),
                    Some(&mut overlapped),
                )
            }
        };
        match result {
            Ok(()) => {}
            Err(error) if error.code().0 as u32 & 0xffff == ERROR_IO_PENDING.0 => {
                count = complete(handle, &mut overlapped, event.0, stop, timeout)?
            }
            Err(error) => return Err(err(error)),
        }
        if count == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "instance disconnected"));
        }
        done += count as usize;
    }
    Ok(())
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
        let units: Vec<u16> = path.as_os_str().encode_wide().collect();
        if !path.is_absolute() || units.is_empty() || units.contains(&0) || units.len() > 32767 {
            return Err(invalid());
        }
        data.extend((units.len() as u16).to_le_bytes());
        for unit in units {
            data.extend(unit.to_le_bytes());
        }
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
    let line = u64::from_le_bytes(data[5..13].try_into().unwrap());
    let column = u64::from_le_bytes(data[13..21].try_into().unwrap());
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
        let length = u16::from_le_bytes(length.try_into().unwrap()) as usize * 2;
        let bytes = data.get(at..at + length).ok_or_else(invalid)?;
        at += length;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
            .collect();
        request.paths.push(PathBuf::from(std::ffi::OsString::from_wide(&units)));
    }
    if at != data.len() {
        return Err(invalid());
    }
    encode(&request)?;
    request.read_only |= request.monitor;
    Ok(request)
}

fn identity() -> io::Result<(String, u32)> {
    let user = sid(unsafe { GetCurrentProcess() })?;
    let mut session = 0;
    unsafe {
        ProcessIdToSessionId(GetCurrentProcessId(), &mut session).map_err(err)?;
    }
    Ok((user, session))
}
fn final_path(path: &Path) -> io::Result<Vec<u16>> {
    // No access is needed to name an object; backup semantics also opens folders.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)?;
    let mut name = vec![0u16; 32768];
    let count = unsafe {
        GetFinalPathNameByHandleW(
            HANDLE(file.as_raw_handle()),
            &mut name,
            GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0),
        )
    } as usize;
    if count == 0 || count >= name.len() {
        return Err(io::Error::last_os_error());
    }
    name.truncate(count);
    Ok(name)
}
fn lowercase(units: &[u16]) -> Vec<u16> {
    let mut folded = Vec::with_capacity(units.len());
    for unit in char::decode_utf16(units.iter().copied()) {
        match unit {
            Ok(character) => {
                for lower in character.to_lowercase() {
                    folded.extend_from_slice(lower.encode_utf16(&mut [0; 2]));
                }
            }
            Err(error) => folded.push(error.unpaired_surrogate()),
        }
    }
    folded
}
/// The profile identity behind one spelling of a scope: the final path of its nearest
/// existing ancestor, which resolves case, 8.3 names, `subst` drives and links, then
/// the not-yet-created tail, all lowercased (APP-14).
fn canonical_scope(scope: &Path) -> Vec<u16> {
    let absolute = std::path::absolute(scope).unwrap_or_else(|_| scope.to_path_buf());
    let mut current = absolute.as_path();
    let mut tail = Vec::new();
    let mut units = loop {
        if let Ok(units) = final_path(current) {
            break units;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                current = parent;
            }
            _ => break current.as_os_str().encode_wide().collect(),
        }
    };
    for name in tail.into_iter().rev() {
        if units.last() != Some(&92) {
            units.push(92);
        }
        units.extend(name.encode_wide());
    }
    lowercase(&units)
}
fn pipe_name(user: &str, session: u32, scope: &Path) -> Vec<u16> {
    let mut digest = Sha256::new();
    digest.update(user.as_bytes());
    digest.update(session.to_le_bytes());
    for unit in canonical_scope(scope) {
        digest.update(unit.to_le_bytes());
    }
    wide(&format!(r"\\.\pipe\bareline-instance-{:x}", digest.finalize()))
}
/// An owner keeps its profile's lock file open without sharing. Another session, or a
/// spelling that still hashes apart, then finds the profile taken (sharing violation).
/// A profile folder that does not exist yet or cannot be written is not locked.
fn lock_profile(profile: &Path) -> io::Result<Option<std::fs::File>> {
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .share_mode(0)
        .open(profile.join("instance.lock"))
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION.0 as i32) => Err(error),
        Err(_) => Ok(None),
    }
}
fn create_instance(name: &[u16], first: bool, attributes: Option<*const SECURITY_ATTRIBUTES>) -> HANDLE {
    let first = if first {
        FILE_FLAG_FIRST_PIPE_INSTANCE
    } else {
        FILE_FLAGS_AND_ATTRIBUTES(0)
    };
    unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX | first | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            INSTANCES,
            LIMIT as u32,
            LIMIT as u32,
            0,
            attributes,
        )
    }
}
/// Also returns when the successful attempt began, which precedes the owner's clock.
fn connect(name: &[u16], deadline: Instant) -> io::Result<(Handle, Instant)> {
    loop {
        let attempt = Instant::now();
        match unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        } {
            Ok(handle) => return Ok((Handle(handle), attempt)),
            // Every instance is serving another launch, or the owner is between instances.
            Err(error) if matches!(error.code().0 as u32 & 0xffff, 2 | 231) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2))
            }
            Err(error) => return Err(err(error)),
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
enum Listen {
    Connected,
    Retry,
    Stop,
}
fn listen(pipe: HANDLE, stop: HANDLE) -> Listen {
    let Ok(ready) = event() else {
        return Listen::Stop;
    };
    let mut overlapped = OVERLAPPED {
        hEvent: ready.0,
        ..Default::default()
    };
    let code = match unsafe { ConnectNamedPipe(pipe, Some(&mut overlapped)) } {
        Ok(()) => return Listen::Connected,
        Err(error) => error.code().0 as u32 & 0xffff,
    };
    if code == ERROR_PIPE_CONNECTED.0 {
        Listen::Connected
    } else if code == ERROR_NO_DATA.0 {
        // A client connected and closed before this call. Disconnecting frees the
        // instance for the next client; it never ends the worker (APP-04).
        Listen::Retry
    } else if code != ERROR_IO_PENDING.0 {
        Listen::Stop
    } else if complete(pipe, &mut overlapped, ready.0, stop, INFINITE).is_ok() {
        Listen::Connected
    } else if unsafe { WaitForSingleObject(stop, 0) } == WAIT_OBJECT_0 {
        Listen::Stop
    } else {
        Listen::Retry
    }
}
fn receive(
    pipe: HANDLE,
    stop: HANDLE,
    queue: &HandoffQueue,
    user: &str,
    session: u32,
    notify: &(dyn Fn() + Send + Sync),
) -> io::Result<()> {
    let deadline = Instant::now() + SERVER_TIMEOUT;
    authenticate(pipe, true, user, session)?;
    let mut size = [0; 4];
    transfer(pipe, stop, &mut size, false, deadline)?;
    let size = u32::from_le_bytes(size) as usize;
    if size > LIMIT {
        return Err(invalid());
    }
    let mut data = vec![0; size];
    transfer(pipe, stop, &mut data, false, deadline)?;
    let request = decode(&data)?;
    // An unanswered client opens the files itself.
    if !queue.reserve() {
        return Err(io::Error::other("instance queue full or closing"));
    }
    let committed = (|| -> io::Result<()> {
        transfer(pipe, stop, &mut [PREPARED], true, deadline)?;
        let mut commit = [0];
        transfer(pipe, stop, &mut commit, false, deadline)?;
        if commit == [COMMIT] { Ok(()) } else { Err(invalid()) }
    })();
    // Until the commit arrives the client may still give up and open the files
    // itself, so only a received commit makes this owner act (APP-04).
    queue.settle(committed.is_ok().then_some(request));
    committed?;
    notify();
    transfer(pipe, stop, &mut [DONE], true, deadline)
}
fn serve(
    pipe: Handle,
    stop: &Handle,
    queue: &HandoffQueue,
    user: &str,
    session: u32,
    notify: &(dyn Fn() + Send + Sync),
) {
    loop {
        match listen(pipe.0, stop.0) {
            Listen::Connected => {
                let _ = receive(pipe.0, stop.0, queue, user, session, notify);
            }
            Listen::Retry => {}
            Listen::Stop => break,
        }
        unsafe {
            let _ = DisconnectNamedPipe(pipe.0);
        }
        if unsafe { WaitForSingleObject(stop.0, 0) } == WAIT_OBJECT_0 {
            break;
        }
    }
}

/// `profile` is the data folder whose lock file marks its single owner.
pub fn coordinate(
    scope: &Path,
    profile: Option<&Path>,
    request: OpenRequest,
    new_instance: bool,
    notify: Arc<dyn Fn() + Send + Sync>,
) -> io::Result<Outcome> {
    let payload = encode(&request)?;
    if new_instance {
        return Ok(Outcome::Independent(
            "Explicit new instance uses no shared session writer.".into(),
        ));
    }
    let (user, session) = identity()?;
    let name = pipe_name(&user, session, scope);
    let descriptor_text = wide(&format!("D:P(A;;GA;;;{user})"));
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(descriptor_text.as_ptr()),
            1,
            &mut descriptor,
            None,
        )
        .map_err(err)?;
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let raw = create_instance(&name, true, Some(&attributes));
    let create_error = io::Error::last_os_error();
    let mut pipes = Vec::new();
    if raw != INVALID_HANDLE_VALUE {
        pipes.push(Handle(raw));
        // Further instances let several launches hand off at once (APP-03).
        while pipes.len() < INSTANCES as usize {
            let raw = create_instance(&name, false, Some(&attributes));
            if raw == INVALID_HANDLE_VALUE {
                break;
            }
            pipes.push(Handle(raw));
        }
    }
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    if !pipes.is_empty() {
        let Ok(lock) = profile.map_or(Ok(None), lock_profile) else {
            return Ok(Outcome::Independent(
                "Another Bareline window owns this profile. This window has an independent session.".into(),
            ));
        };
        let stop = Arc::new(event()?);
        let queue = Arc::new(HandoffQueue::default());
        let mut workers = Vec::new();
        for pipe in pipes {
            let (stop, queue, user, notify) = (stop.clone(), queue.clone(), user.clone(), notify.clone());
            match std::thread::Builder::new()
                .name("bareline-instance".into())
                .spawn(move || serve(pipe, &stop, &queue, &user, session, &*notify))
            {
                Ok(worker) => workers.push(worker),
                Err(error) if workers.is_empty() => return Err(error),
                Err(_) => break,
            }
        }
        return Ok(Outcome::Primary(InstanceServer {
            queue,
            stop,
            workers,
            _profile: lock,
        }));
    }
    if !matches!(create_error.raw_os_error(), Some(5 | 231)) {
        return Ok(Outcome::Independent(format!(
            "Instance coordination unavailable: {create_error}"
        )));
    }
    Ok(forwarded(forward(
        &name,
        &payload,
        &user,
        session,
        Instant::now() + TIMEOUT,
    )))
}
/// Client side up to `PREPARED`. Returns the pipe and the deadline for its commit.
fn prepare(
    name: &[u16],
    payload: &[u8],
    user: &str,
    session: u32,
    stop: HANDLE,
    connect_by: Instant,
) -> io::Result<(Handle, Instant)> {
    let (pipe, connecting) = connect(name, connect_by)?;
    // The owner's clock starts once this attempt connects, so the commit deadline
    // below always falls inside `SERVER_TIMEOUT` of it.
    let deadline = connect_by.min(connecting + TIMEOUT);
    authenticate(pipe.0, false, user, session)?;
    let mut data = (payload.len() as u32).to_le_bytes().to_vec();
    data.extend_from_slice(payload);
    transfer(pipe.0, stop, &mut data, true, deadline)?;
    let mut ack = [0];
    transfer(pipe.0, stop, &mut ack, false, deadline)?;
    if ack != [PREPARED] {
        return Err(invalid());
    }
    Ok((pipe, deadline + COMMIT_WINDOW))
}
/// Any error means the owner never got the commit, and it acts only on a received
/// commit, so this launch opens the files itself. A written commit is always read,
/// because the owner outwaits `deadline` (APP-04).
fn commit(pipe: &Handle, stop: HANDLE, deadline: Instant) -> io::Result<()> {
    transfer(pipe.0, stop, &mut [COMMIT], true, deadline)?;
    // Waiting for `DONE` keeps the pipe open until the owner has read the commit.
    let _ = transfer(pipe.0, stop, &mut [0], false, deadline + COMMIT_WINDOW);
    Ok(())
}
fn forward(name: &[u16], payload: &[u8], user: &str, session: u32, connect_by: Instant) -> io::Result<()> {
    let stop = event()?;
    let (pipe, deadline) = prepare(name, payload, user, session, stop.0, connect_by)?;
    commit(&pipe, stop.0, deadline)
}
fn forwarded(result: io::Result<()>) -> Outcome {
    match result {
        Ok(()) => Outcome::Forwarded,
        Err(error) => Outcome::Independent(format!(
            "Existing instance did not accept the request: {error}. This window has an independent session."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lossless_payload_and_revalidation() {
        let request = OpenRequest {
            paths: vec![PathBuf::from(std::ffi::OsString::from_wide(&[
                67, 58, 92, 0xd800, 32, 0x00e9,
            ]))],
            line: Some(2),
            column: Some(3),
            read_only: true,
            ..Default::default()
        };
        let data = encode(&request).unwrap();
        assert_eq!(decode(&data).unwrap(), request);
        assert!(decode(&data[..data.len() - 1]).is_err());
        let mut bad = data;
        bad[4] = 128;
        assert!(decode(&bad).is_err());
    }
    fn nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
    fn scope(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("bareline-instance-{label}-{}-{}", std::process::id(), nanos()))
    }
    fn open(path: &str) -> OpenRequest {
        OpenRequest {
            paths: vec![PathBuf::from(path)],
            ..Default::default()
        }
    }
    fn hand_off(scope: &Path, profile: Option<&Path>, request: OpenRequest) -> Outcome {
        coordinate(scope, profile, request, false, Arc::new(|| {})).unwrap()
    }
    fn primary(scope: &Path, profile: Option<&Path>) -> InstanceServer {
        let Outcome::Primary(server) = hand_off(scope, profile, OpenRequest::default()) else {
            panic!("primary");
        };
        server
    }
    /// A hang guard only: no assertion depends on how long a handoff takes.
    fn wait_until(mut done: impl FnMut() -> bool) {
        let watchdog = Instant::now() + Duration::from_secs(60);
        while !done() {
            assert!(Instant::now() < watchdog, "watchdog expired");
            std::thread::yield_now();
        }
    }
    fn drain(server: &InstanceServer, count: usize) -> Vec<OpenRequest> {
        let mut received = Vec::new();
        wait_until(|| {
            received.extend(server.try_recv());
            received.len() >= count
        });
        received
    }
    /// A client that was answered `PREPARED` and has not committed yet.
    struct Prepared {
        pipe: Handle,
        deadline: Instant,
        stop: Handle,
    }
    impl Prepared {
        fn commit(&self) -> io::Result<()> {
            commit(&self.pipe, self.stop.0, self.deadline)
        }
    }
    fn prepared(scope: &Path, path: &str) -> Prepared {
        let (user, session) = identity().unwrap();
        let stop = event().unwrap();
        let payload = encode(&open(path)).unwrap();
        let name = pipe_name(&user, session, scope);
        let (pipe, deadline) = prepare(&name, &payload, &user, session, stop.0, Instant::now() + TIMEOUT).unwrap();
        Prepared { pipe, deadline, stop }
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
        queue.settle(Some(open(r"C:\queue\a.txt")));
        assert!(!queue.reserve());
        assert_eq!(queue.state().requests.pop_front(), Some(open(r"C:\queue\a.txt")));
        assert!(queue.reserve());
    }
    #[test]
    fn owner_acknowledges_a_queued_request_without_its_ui() {
        let scope = scope("ack");
        let server = primary(&scope, None);
        // Nothing drains the queue during the handoff, as before the owner's first
        // frame or while its UI thread runs a modal loop (APP-03).
        assert!(matches!(
            hand_off(&scope, None, open(r"C:\ack\a.txt")),
            Outcome::Forwarded
        ));
        assert_eq!(drain(&server, 1), vec![open(r"C:\ack\a.txt")]);
        assert_eq!(server.try_recv(), None);
        server.set_accepting(false);
        assert!(matches!(
            hand_off(&scope, None, open(r"C:\ack\b.txt")),
            Outcome::Independent(_)
        ));
        assert_eq!(server.try_recv(), None);
        assert!(matches!(
            coordinate(&scope, None, OpenRequest::default(), true, Arc::new(|| {})).unwrap(),
            Outcome::Independent(_)
        ));
    }
    #[test]
    fn concurrent_clients_are_each_forwarded_exactly_once() {
        let scope = scope("many");
        let server = primary(&scope, None);
        let (user, session) = identity().unwrap();
        let name = pipe_name(&user, session, &scope);
        let expected: Vec<_> = (0..12).map(|index| open(&format!(r"C:\many\{index:02}.txt"))).collect();
        let clients: Vec<_> = expected
            .iter()
            .map(|request| {
                let (name, user, payload) = (name.clone(), user.clone(), encode(request).unwrap());
                // Clients wait for one of the instances as long as they need to, so a
                // loaded machine cannot fail this test; each handshake keeps its deadline.
                std::thread::spawn(move || {
                    forwarded(forward(
                        &name,
                        &payload,
                        &user,
                        session,
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
    }
    #[test]
    fn uncommitted_request_is_never_acted_on() {
        let scope = scope("abort");
        let server = primary(&scope, None);
        let client = prepared(&scope, r"C:\abort\a.txt");
        assert_eq!(server.pending(), 1);
        // The client gives up before committing, as one past its deadline does, and
        // then opens the file itself; the owner must not open it too (APP-04).
        drop(client);
        wait_until(|| server.queue.state().reserved == 0);
        assert_eq!(server.try_recv(), None);
        assert!(matches!(
            hand_off(&scope, None, open(r"C:\abort\b.txt")),
            Outcome::Forwarded
        ));
        assert_eq!(drain(&server, 1), vec![open(r"C:\abort\b.txt")]);
    }
    #[test]
    fn commit_the_owner_no_longer_reads_opens_independently() {
        let scope = scope("gave-up");
        let server = primary(&scope, None);
        let client = prepared(&scope, r"C:\gave-up\a.txt");
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
            queue.settle(Some(open(r"C:\quiesce\a.txt")));
            assert_eq!(exiting.join().unwrap(), 1);
        });
        assert!(!queue.reserve());
        assert_eq!(queue.state().requests.pop_front(), Some(open(r"C:\quiesce\a.txt")));
        assert_eq!(queue.quiesce(Duration::ZERO), 0);
        // A slot that never settles keeps counting, so the owner does not exit on it.
        queue.state().refusing = false;
        assert!(queue.reserve());
        assert_eq!(queue.quiesce(Duration::ZERO), 1);
    }
    #[test]
    fn exiting_owner_leaves_no_acknowledged_request_behind() {
        let scope = scope("quiesce");
        let server = primary(&scope, None);
        let client = prepared(&scope, r"C:\quiesce\a.txt");
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
            hand_off(&scope, None, open(r"C:\quiesce\b.txt")),
            Outcome::Independent(_)
        ));
        assert_eq!(server.quiesce(), 1);
        assert_eq!(server.try_recv(), Some(open(r"C:\quiesce\a.txt")));
        assert_eq!(server.pending(), 0);
        assert_eq!(server.quiesce(), 0);
    }
    #[test]
    fn client_that_closed_before_listen_does_not_end_the_worker() {
        let name = wide(&format!(
            r"\\.\pipe\bareline-instance-nodata-{}-{}",
            std::process::id(),
            nanos()
        ));
        let pipe = Handle(create_instance(&name, true, None));
        assert!(pipe.0 != INVALID_HANDLE_VALUE);
        let stop = event().unwrap();
        // The client connects and closes before the owner calls ConnectNamedPipe,
        // which then reports ERROR_NO_DATA.
        drop(connect(&name, Instant::now() + TIMEOUT).unwrap());
        assert_eq!(listen(pipe.0, stop.0), Listen::Retry);
        unsafe { DisconnectNamedPipe(pipe.0).unwrap() };
        let client = std::thread::spawn(move || connect(&name, Instant::now() + TIMEOUT).is_ok());
        assert_eq!(listen(pipe.0, stop.0), Listen::Connected);
        assert!(client.join().unwrap());
    }
    #[test]
    fn scope_spellings_of_one_profile_share_one_identity() {
        let root = scope("Spelling");
        std::fs::create_dir_all(&root).unwrap();
        let settings = root.join("settings.toml");
        let before = canonical_scope(&settings);
        let upper = PathBuf::from(root.to_string_lossy().to_ascii_uppercase());
        assert_eq!(canonical_scope(&upper.join("SETTINGS.TOML")), before);
        assert_eq!(canonical_scope(&root.join(".").join("settings.toml")), before);
        let mut short = vec![0u16; 32768];
        let long = wide(&root.to_string_lossy());
        let length = unsafe { GetShortPathNameW(PCWSTR(long.as_ptr()), Some(&mut short)) } as usize;
        // 8.3 names exist only where the volume keeps them.
        if length > 0 && length < short.len() {
            short.truncate(length);
            let short = PathBuf::from(std::ffi::OsString::from_wide(&short));
            assert_eq!(canonical_scope(&short.join("settings.toml")), before);
        }
        // Creating the file keeps the identity its folder already gave it.
        std::fs::write(&settings, "").unwrap();
        assert_eq!(canonical_scope(&settings), before);
        assert_ne!(canonical_scope(&root.join("other.toml")), before);
        assert_eq!(lowercase(&[0x41, 0xd800, 0xc9]), vec![0x61, 0xd800, 0xe9]);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn profile_lock_refuses_a_second_owner() {
        let root = scope("lock");
        std::fs::create_dir_all(&root).unwrap();
        let server = primary(&root.join("settings.toml"), Some(&root));
        // A spelling that differs only in case reaches the same owner.
        let upper = PathBuf::from(root.to_string_lossy().to_ascii_uppercase());
        assert!(matches!(
            hand_off(&upper.join("SETTINGS.TOML"), Some(&upper), open(r"C:\lock\a.txt")),
            Outcome::Forwarded
        ));
        assert_eq!(drain(&server, 1), vec![open(r"C:\lock\a.txt")]);
        // Another session names another pipe, yet cannot own the same profile.
        assert!(matches!(
            hand_off(&root.join("session-2"), Some(&root), OpenRequest::default()),
            Outcome::Independent(_)
        ));
        drop(server);
        drop(primary(&root.join("session-2"), Some(&root)));
        std::fs::remove_dir_all(root).unwrap();
    }
}
