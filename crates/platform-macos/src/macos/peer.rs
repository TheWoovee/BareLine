// SPDX-License-Identifier: MPL-2.0
//! Peer identity of a connected Unix-domain socket on macOS, for the shared
//! socket transport and single-instance handoff: macOS has no
//! `SO_PEERCRED`; the peer's pid comes from `LOCAL_PEERPID` at level
//! `SOL_LOCAL` and its user and group from `getpeereid`.
use std::{
    io,
    os::fd::{AsFd, AsRawFd},
};

/// `SOL_LOCAL` and `LOCAL_PEERPID` from `<sys/un.h>`.
pub const SOL_LOCAL: libc::c_int = libc::SOL_LOCAL;
pub const LOCAL_PEERPID: libc::c_int = libc::LOCAL_PEERPID;

/// The pid of the process at the other end of a connected socket, as recorded
/// when it connected.
pub fn peer_pid(socket: impl AsFd) -> io::Result<u32> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `pid` and `length` describe a writable buffer of the option's size.
    let status = unsafe {
        libc::getsockopt(
            socket.as_fd().as_raw_fd(),
            SOL_LOCAL,
            LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &mut length,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    u32::try_from(pid).map_err(|_| io::Error::other("the peer reported no process id"))
}

/// The effective user and group of the peer.
pub fn peer_credentials(socket: impl AsFd) -> io::Result<(u32, u32)> {
    let (mut user, mut group): (libc::uid_t, libc::gid_t) = (0, 0);
    // SAFETY: both outputs are plain integers owned by this frame.
    if unsafe { libc::getpeereid(socket.as_fd().as_raw_fd(), &mut user, &mut group) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((user, group))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_pair_reports_this_process_and_user() {
        let (left, right) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(peer_pid(&left).unwrap(), std::process::id());
        assert_eq!(peer_pid(&right).unwrap(), std::process::id());
        // SAFETY: getuid has no preconditions.
        let user = unsafe { libc::getuid() };
        assert_eq!(peer_credentials(&left).unwrap().0, user);
        let file = std::fs::File::open("/dev/null").unwrap();
        assert!(peer_pid(&file).is_err());
    }
}
