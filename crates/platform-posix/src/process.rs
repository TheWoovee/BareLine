// SPDX-License-Identifier: MPL-2.0
//! Process identity for owned-cache records: the pid plus a start stamp that a
//! reused pid cannot repeat. A live process whose stamp differs from a record
//! is a different process, so the recorded owner is gone.
use bareline_platform::{CacheProcessIdentity, ProcessLiveness};
use std::io;

pub(crate) fn current() -> io::Result<CacheProcessIdentity> {
    let pid = std::process::id();
    let created = match platform::start(pid)? {
        Some(Start::Running(created)) => created,
        _ => return Err(io::Error::other("the current process has no start record")),
    };
    Ok(CacheProcessIdentity {
        pid,
        created,
        nonce: crate::sys::random_bytes()?,
    })
}

/// A zero `created` asks for pid-only liveness: only a missing or exited
/// process reports Dead, and a live or reused id is never treated as gone.
pub(crate) fn liveness(pid: u32, created: u64) -> ProcessLiveness {
    if pid == 0 {
        return ProcessLiveness::Unknown;
    }
    match platform::start(pid) {
        Ok(None | Some(Start::Exited)) => ProcessLiveness::Dead,
        Ok(Some(Start::Running(_))) if created == 0 => ProcessLiveness::Unknown,
        Ok(Some(Start::Running(actual))) if actual == created => ProcessLiveness::Alive,
        // The pid now belongs to a process started at another time.
        Ok(Some(Start::Running(_))) => ProcessLiveness::Dead,
        Err(_) => ProcessLiveness::Unknown,
    }
}

pub(crate) enum Start {
    Running(u64),
    /// A zombie: exited, not yet reaped.
    Exited,
}

#[cfg(target_os = "linux")]
mod platform {
    //! `/proc/<pid>/stat` field 22 is the start time in clock ticks since boot.
    //! It is stamped with a hash of this boot's id, so a record from an earlier
    //! boot never matches; wall-clock adjustments do not move either value.
    use super::Start;
    use std::io;

    const TICK_BITS: u32 = 40;

    pub(super) fn start(pid: u32) -> io::Result<Option<Start>> {
        let text = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let (state, ticks) = parse_stat(&text).ok_or_else(|| io::Error::other("unreadable process status"))?;
        if matches!(state, 'Z' | 'X' | 'x') {
            return Ok(Some(Start::Exited));
        }
        Ok(Some(Start::Running(stamp(boot_tag()?, ticks))))
    }

    pub(super) fn stamp(boot: u64, ticks: u64) -> u64 {
        (boot << TICK_BITS) | (ticks & ((1 << TICK_BITS) - 1))
    }

    /// State and start ticks. The command name may contain spaces and
    /// parentheses, so fields are counted after its last `)`.
    pub(super) fn parse_stat(text: &str) -> Option<(char, u64)> {
        let mut fields = text.get(text.rfind(')')? + 1..)?.split_whitespace();
        let state = fields.next()?.chars().next()?;
        // Field 3 is the state; field 22 is 19 fields later.
        let ticks = fields.nth(18)?.parse().ok()?;
        Some((state, ticks))
    }

    /// 24-bit FNV-1a hash of the boot id.
    fn boot_tag() -> io::Result<u64> {
        let id = std::fs::read("/proc/sys/kernel/random/boot_id")?;
        let hash = id
            .iter()
            .filter(|byte| !byte.is_ascii_whitespace())
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        Ok(hash & 0x00ff_ffff)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    //! `proc_pidinfo(PROC_PIDTBSDINFO)` reports the start time recorded at fork,
    //! in microseconds; the kernel never rewrites it.
    use super::Start;
    use std::io;

    pub(super) fn start(pid: u32) -> io::Result<Option<Start>> {
        let Ok(raw_pid) = libc::c_int::try_from(pid) else {
            return Ok(None);
        };
        let size = std::mem::size_of::<libc::proc_bsdinfo>();
        // SAFETY: proc_bsdinfo is plain integers and byte arrays; all-zero is valid.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        // SAFETY: the buffer is a live, writable proc_bsdinfo of exactly `size` bytes.
        let written = unsafe {
            libc::proc_pidinfo(
                raw_pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size as libc::c_int,
            )
        };
        if written <= 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ESRCH) => Ok(None),
                _ => Err(error),
            };
        }
        if written as usize != size {
            return Err(io::Error::other("short process information"));
        }
        if info.pbi_status == libc::SZOMB {
            return Ok(Some(Start::Exited));
        }
        Ok(Some(Start::Running(
            info.pbi_start_tvsec
                .saturating_mul(1_000_000)
                .saturating_add(info.pbi_start_tvusec),
        )))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::Start;
    use std::io;

    pub(super) fn start(_: u32) -> io::Result<Option<Start>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "process start time unavailable",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn own_process_is_alive_and_identity_is_stable() {
        let first = current().unwrap();
        let second = current().unwrap();
        assert_eq!(first.pid, std::process::id());
        assert_eq!(first.created, second.created);
        assert_ne!(first.nonce, second.nonce);
        assert_eq!(liveness(first.pid, first.created), ProcessLiveness::Alive);
        // Pid-only liveness never calls a live process gone.
        assert_eq!(liveness(first.pid, 0), ProcessLiveness::Unknown);
        // Same pid, other start time: the recorded process is not this one.
        assert_eq!(liveness(first.pid, first.created ^ 1), ProcessLiveness::Dead);
        assert_eq!(liveness(0, 1), ProcessLiveness::Unknown);
    }

    #[test]
    fn reaped_child_is_not_alive() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "read line"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let created = match platform::start(pid).unwrap() {
            Some(Start::Running(created)) => created,
            _ => panic!("the child must be running while its stdin is open"),
        };
        assert_eq!(liveness(pid, created), ProcessLiveness::Alive);
        drop(child.stdin.take());
        // `read` fails at end of input; only the reaping matters here.
        child.wait().unwrap();
        let state = liveness(pid, created);
        assert!(
            matches!(state, ProcessLiveness::Dead | ProcessLiveness::Unknown),
            "{state:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stat_parsing_survives_odd_command_names() {
        let line = "4242 (a) b (c)) S 1 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10";
        assert_eq!(platform::parse_stat(line), Some(('S', 987_654)));
        assert_eq!(platform::parse_stat("1 (x) Z"), None);
        assert_ne!(platform::stamp(1, 5), platform::stamp(2, 5));
    }
}
