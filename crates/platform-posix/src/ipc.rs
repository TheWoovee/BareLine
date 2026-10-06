// SPDX-License-Identifier: MPL-2.0
//! Local stream sockets shared by the single-instance handoff and the extension
//! host transport: peer credentials, deadline-bounded nonblocking transfers that
//! never raise SIGPIPE, and the private folder the instance sockets live in.
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
};
use std::{
    io::{self, Read},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::{fs::DirBuilderExt, fs::MetadataExt, fs::PermissionsExt, net::UnixStream},
    },
    path::Path,
    time::{Duration, Instant},
};

/// The process on the other end of a connected local socket, as the kernel
/// recorded it when the connection (or the listening socket) was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Peer {
    pub pid: u32,
    pub uid: u32,
}

/// `SO_PEERCRED`: the peer's process and effective user at connect (client)
/// or listen (server) time.
#[cfg(target_os = "linux")]
pub(crate) fn peer(stream: &UnixStream) -> io::Result<Peer> {
    let credentials = rustix::net::sockopt::socket_peercred(stream)?;
    Ok(Peer {
        pid: credentials.pid.as_raw_nonzero().get().unsigned_abs(),
        uid: credentials.uid.as_raw(),
    })
}
/// `getpeereid` for the user and `LOCAL_PEERPID` for the process.
#[cfg(target_os = "macos")]
pub(crate) fn peer(stream: &UnixStream) -> io::Result<Peer> {
    use std::os::fd::AsRawFd;
    let fd = stream.as_raw_fd();
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: `fd` is a live socket for this call; both out-pointers are valid.
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: the option value is a `pid_t` sized buffer and `length` says so.
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Peer {
        pid: pid.unsigned_abs(),
        uid,
    })
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn peer(_stream: &UnixStream) -> io::Result<Peer> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "peer credentials are not available on this system",
    ))
}

/// The effective user every peer must match.
pub(crate) fn own_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

pub(crate) fn timed_out(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, what)
}

/// Nonblocking, and on macOS without SIGPIPE (Linux passes `MSG_NOSIGNAL` per send).
pub(crate) fn configure(stream: &UnixStream) -> io::Result<()> {
    stream.set_nonblocking(true)?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        let on: libc::c_int = 1;
        // SAFETY: a valid socket and an `int` option value of the stated size.
        let result = unsafe {
            libc::setsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_NOSIGPIPE,
                (&raw const on).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// A stop signal several threads wait on: once raised it stays readable. A
/// socket pair rather than a pipe, because macOS has no `pipe2` to create a
/// pipe close-on-exec atomically.
pub(crate) struct Stop {
    read: UnixStream,
    write: UnixStream,
}
impl Stop {
    pub fn new() -> io::Result<Self> {
        let (read, write) = UnixStream::pair()?;
        write.set_nonblocking(true)?;
        Ok(Self { read, write })
    }
    pub fn raise(&self) {
        // The pair is never drained, so one byte keeps it readable for every waiter.
        let _ = send(&self.write, &[1]);
    }
    pub fn raised(&self) -> bool {
        let mut fds = [PollFd::new(&self.read, PollFlags::IN)];
        matches!(poll(&mut fds, Some(&Timespec::default())), Ok(count) if count > 0)
    }
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.read.as_fd()
    }
}

fn timespec(duration: Duration) -> Timespec {
    Timespec {
        tv_sec: duration.as_secs().min(i64::MAX as u64) as i64,
        tv_nsec: i64::from(duration.subsec_nanos()),
    }
}

/// Waits until `fd` is ready for `events`, the deadline passes (`TimedOut`) or
/// `stop` is raised (`Interrupted`). Hang-ups count as ready so the following
/// read or write reports them.
pub(crate) fn wait(
    fd: BorrowedFd<'_>,
    events: PollFlags,
    stop: Option<BorrowedFd<'_>>,
    deadline: Option<Instant>,
) -> io::Result<()> {
    loop {
        let timeout = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(timed_out("local socket deadline"));
                }
                Some(timespec(remaining))
            }
            None => None,
        };
        let mut fds = [
            PollFd::from_borrowed_fd(fd, events),
            PollFd::from_borrowed_fd(fd, events),
        ];
        let count = match stop {
            Some(stop) => {
                fds[1] = PollFd::from_borrowed_fd(stop, PollFlags::IN);
                2
            }
            None => 1,
        };
        match poll(&mut fds[..count], timeout.as_ref()) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(errno) => return Err(errno.into()),
        }
        if count == 2 && !fds[1].revents().is_empty() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "stopping"));
        }
        if !fds[0].revents().is_empty() {
            return Ok(());
        }
    }
}

/// One nonblocking send; never raises SIGPIPE on a closed peer.
pub(crate) fn send(stream: &UnixStream, bytes: &[u8]) -> io::Result<usize> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        rustix::net::send(stream, bytes, rustix::net::SendFlags::NOSIGNAL).map_err(io::Error::from)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        use std::io::Write;
        // SO_NOSIGPIPE was set by `configure`.
        (&*stream).write(bytes)
    }
}

/// Reads exactly `bytes` before `deadline`; EOF is `UnexpectedEof`.
pub(crate) fn read_exact(
    stream: &UnixStream,
    stop: Option<BorrowedFd<'_>>,
    bytes: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    let mut done = 0;
    while done < bytes.len() {
        match (&*stream).read(&mut bytes[done..]) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "peer disconnected")),
            Ok(count) => done += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait(stream.as_fd(), PollFlags::IN, stop, Some(deadline))?
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Writes all of `bytes` before `deadline`.
pub(crate) fn write_all(
    stream: &UnixStream,
    stop: Option<BorrowedFd<'_>>,
    bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    let mut done = 0;
    while done < bytes.len() {
        match send(stream, &bytes[done..]) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "peer disconnected")),
            Ok(count) => done += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait(stream.as_fd(), PollFlags::OUT, stop, Some(deadline))?
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Creates `path` (and its parents) and accepts it only as a real folder that
/// this user owns and nobody else can enter: the sockets and locks inside
/// decide who may hand files to this user's editor. A `/tmp` fallback folder
/// another user created first is refused rather than shared.
pub(crate) fn private_directory(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != own_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the runtime folder belongs to someone else",
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        // Ours but too open (for example created by an older umask): tighten it.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Random bytes from the kernel, for socket names nobody can guess.
pub(crate) fn random_hex<const N: usize>() -> io::Result<String> {
    let bytes = crate::sys::random_bytes::<N>()?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peers_of_a_socket_pair_are_this_process_and_user() {
        let (left, right) = UnixStream::pair().unwrap();
        let expected = Peer {
            pid: std::process::id(),
            uid: own_uid(),
        };
        assert_eq!(peer(&left).unwrap(), expected);
        assert_eq!(peer(&right).unwrap(), expected);
    }

    #[test]
    fn transfers_honour_deadlines_stops_and_closed_peers() {
        let (left, right) = UnixStream::pair().unwrap();
        configure(&left).unwrap();
        configure(&right).unwrap();
        let mut byte = [0];
        // Nothing arrives: the deadline ends the wait.
        let error = read_exact(&left, None, &mut byte, Instant::now() + Duration::from_millis(20)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        // A raised stop ends it at once, however far away the deadline is.
        let stop = Stop::new().unwrap();
        assert!(!stop.raised());
        stop.raise();
        assert!(stop.raised());
        let far = Instant::now() + Duration::from_secs(3600);
        let error = read_exact(&left, Some(stop.fd()), &mut byte, far).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        write_all(&right, None, b"ok", far).unwrap();
        let mut two = [0; 2];
        read_exact(&left, None, &mut two, far).unwrap();
        assert_eq!(&two, b"ok");
        // A closed peer is an error, not a signal that kills the process.
        drop(left);
        assert!(write_all(&right, None, b"lost", far).is_err());
    }

    #[test]
    fn private_directories_are_owned_and_closed_to_others() {
        let root = std::env::temp_dir().join(format!(
            "bareline-ipc-private-{}-{}",
            std::process::id(),
            random_hex::<8>().unwrap()
        ));
        let folder = root.join("runtime");
        private_directory(&folder).unwrap();
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        private_directory(&folder).unwrap();
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
        // A link in its place is not a folder of ours.
        let link = root.join("link");
        std::os::unix::fs::symlink(&folder, &link).unwrap();
        assert_eq!(
            private_directory(&link).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
