// SPDX-License-Identifier: MPL-2.0
//! Bounded per-user/session/profile handoff. No document or network reads occur here.
use sha2::{Digest, Sha256};
use std::{
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
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
const TIMEOUT: Duration = Duration::from_millis(200);
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
pub struct PendingOpen {
    pub request: OpenRequest,
    accepted: mpsc::SyncSender<bool>,
    deadline: Instant,
}
impl PendingOpen {
    pub fn live(&self) -> bool {
        Instant::now() < self.deadline
    }
    pub fn accept(self) -> bool {
        self.accepted.send(true).is_ok()
    }
}
pub struct InstanceServer {
    requests: mpsc::Receiver<PendingOpen>,
    stop: Arc<Handle>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl InstanceServer {
    pub fn try_recv(&self) -> Option<PendingOpen> {
        self.requests.try_recv().ok()
    }
}
impl Drop for InstanceServer {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop.0);
        }
        if let Some(worker) = self.worker.take() {
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
            let mut ignored = 0;
            let _ = GetOverlappedResult(handle, overlapped, &mut ignored, true);
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

pub fn coordinate(
    scope: &Path,
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
    let user = sid(unsafe { GetCurrentProcess() })?;
    let mut session = 0;
    unsafe {
        ProcessIdToSessionId(GetCurrentProcessId(), &mut session).map_err(err)?;
    }
    let mut digest = Sha256::new();
    digest.update(user.as_bytes());
    digest.update(session.to_le_bytes());
    for unit in scope.as_os_str().encode_wide() {
        digest.update(unit.to_le_bytes());
    }
    let name = wide(&format!(r"\\.\pipe\bareline-instance-{:x}", digest.finalize()));
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
    let raw = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            LIMIT as u32,
            LIMIT as u32,
            0,
            Some(&attributes),
        )
    };
    let create_error = io::Error::last_os_error();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    if raw != INVALID_HANDLE_VALUE {
        let handle = Handle(raw);
        let stop = Arc::new(event()?);
        let worker_stop = stop.clone();
        let (tx, requests) = mpsc::sync_channel(16);
        let worker = std::thread::Builder::new()
            .name("bareline-instance".into())
            .spawn(move || {
                let handle = handle;
                loop {
                    let Ok(ready) = event() else {
                        break;
                    };
                    let mut overlapped = OVERLAPPED {
                        hEvent: ready.0,
                        ..Default::default()
                    };
                    let connect = unsafe { ConnectNamedPipe(handle.0, Some(&mut overlapped)) };
                    let connected = match connect {
                        Ok(()) => true,
                        Err(error) if error.code().0 as u32 & 0xffff == ERROR_PIPE_CONNECTED.0 => true,
                        Err(error) if error.code().0 as u32 & 0xffff == ERROR_IO_PENDING.0 => {
                            complete(handle.0, &mut overlapped, ready.0, worker_stop.0, INFINITE).is_ok()
                        }
                        Err(_) => false,
                    };
                    if !connected {
                        break;
                    }
                    let deadline = Instant::now() + TIMEOUT;
                    let accepted = (|| -> io::Result<()> {
                        authenticate(handle.0, true, &user, session)?;
                        let mut size = [0; 4];
                        transfer(handle.0, worker_stop.0, &mut size, false, deadline)?;
                        let size = u32::from_le_bytes(size) as usize;
                        if size > LIMIT {
                            return Err(invalid());
                        }
                        let mut data = vec![0; size];
                        transfer(handle.0, worker_stop.0, &mut data, false, deadline)?;
                        let request = decode(&data)?;
                        let (accepted, result) = mpsc::sync_channel(1);
                        tx.try_send(PendingOpen {
                            request,
                            accepted,
                            deadline,
                        })
                        .map_err(|_| io::Error::other("instance queue full"))?;
                        notify();
                        if result
                            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                            .ok()
                            != Some(true)
                        {
                            return Err(timed_out());
                        }
                        transfer(handle.0, worker_stop.0, &mut [1], true, deadline)?;
                        let mut received = [0];
                        transfer(handle.0, worker_stop.0, &mut received, false, deadline)
                    })();
                    let _ = accepted;
                    unsafe {
                        let _ = DisconnectNamedPipe(handle.0);
                    }
                    if unsafe { WaitForSingleObject(worker_stop.0, 0) } == WAIT_OBJECT_0 {
                        break;
                    }
                }
            })?;
        return Ok(Outcome::Primary(InstanceServer {
            requests,
            stop,
            worker: Some(worker),
        }));
    }
    if !matches!(create_error.raw_os_error(), Some(5 | 231)) {
        return Ok(Outcome::Independent(format!(
            "Instance coordination unavailable: {create_error}"
        )));
    }
    let deadline = Instant::now() + TIMEOUT;
    let stop = event()?;
    let result = (|| -> io::Result<()> {
        let handle = loop {
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
                Ok(handle) => break Handle(handle),
                Err(error) if matches!(error.code().0 as u32 & 0xffff, 2 | 231) && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(error) => return Err(err(error)),
            }
        };
        authenticate(handle.0, false, &user, session)?;
        let mut data = (payload.len() as u32).to_le_bytes().to_vec();
        data.extend(payload);
        transfer(handle.0, stop.0, &mut data, true, deadline)?;
        let mut ack = [0];
        transfer(handle.0, stop.0, &mut ack, false, deadline)?;
        if ack != [1] {
            return Err(invalid());
        }
        transfer(handle.0, stop.0, &mut [2], true, deadline)?;
        Ok(())
    })();
    Ok(match result {
        Ok(()) => Outcome::Forwarded,
        Err(error) => Outcome::Independent(format!(
            "Existing instance did not accept the request: {error}. This window has an independent session."
        )),
    })
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
    #[test]
    fn abandoned_sender_cannot_be_reported_as_accepted() {
        let (accepted, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        let pending = PendingOpen {
            request: OpenRequest::default(),
            accepted,
            deadline: Instant::now() + Duration::from_secs(1),
        };
        assert!(!pending.accept());
    }
    #[test]
    fn handoff_ack_and_hung_owner_fallback_are_bounded() {
        let scope = std::env::temp_dir().join(format!(
            "bareline-instance-test-{}-{:?}",
            std::process::id(),
            Instant::now()
        ));
        let Outcome::Primary(server) = coordinate(&scope, OpenRequest::default(), false, Arc::new(|| {})).unwrap()
        else {
            panic!("primary");
        };
        let scope2 = scope.clone();
        let client =
            std::thread::spawn(move || coordinate(&scope2, OpenRequest::default(), false, Arc::new(|| {})).unwrap());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(request) = server.try_recv() {
                assert!(request.accept());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(matches!(client.join().unwrap(), Outcome::Forwarded));
        let started = Instant::now();
        assert!(matches!(
            coordinate(&scope, OpenRequest::default(), false, Arc::new(|| {})).unwrap(),
            Outcome::Independent(_)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            coordinate(&scope, OpenRequest::default(), true, Arc::new(|| {})).unwrap(),
            Outcome::Independent(_)
        ));
    }
}
