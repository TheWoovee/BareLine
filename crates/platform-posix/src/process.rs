// SPDX-License-Identifier: MPL-2.0
//! Process identity, resource counters and the monotonic clock.
//!
//! Owned-cache records keep the pid plus a start stamp that a reused pid cannot
//! repeat: a live process whose stamp differs from a record is a different
//! process, so the recorded owner is gone. Recovery journals compare a wall-clock
//! start time ([`started_unix_nanos`]) with the time in their names instead. The
//! counters ([`resident_bytes`], [`open_descriptors`]) and the clock
//! ([`monotonic_nanos`]) feed diagnostics and performance runs.
use bareline_platform::{CacheProcessIdentity, ProcessLiveness};
use std::io;

/// This process's identity for owned-cache records.
pub fn current() -> io::Result<CacheProcessIdentity> {
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
pub fn liveness(pid: u32, created: u64) -> ProcessLiveness {
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

/// What the system reports about a process id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// A running process and its start stamp, the value records keep in
    /// [`CacheProcessIdentity::created`]. Stamps compare only with stamps.
    Running(u64),
    /// A zombie: exited, not yet reaped.
    Exited,
}

/// The process with this id; `None` when no process has it.
pub fn start(pid: u32) -> io::Result<Option<Start>> {
    platform::start(pid)
}

/// When the running process with this id started, in nanoseconds since the
/// Unix epoch by the system clock, so it compares with times the clock wrote
/// elsewhere (journal names). `None` when no running process has the id.
pub fn started_unix_nanos(pid: u32) -> io::Result<Option<u128>> {
    platform::started_unix_nanos(pid)
}

/// The real user id of this process.
pub fn user_id() -> u32 {
    rustix::process::getuid().as_raw()
}

/// Bytes of this process's memory resident in RAM.
pub fn resident_bytes() -> io::Result<u64> {
    platform::resident_bytes()
}

/// File descriptors this process has open.
pub fn open_descriptors() -> io::Result<u32> {
    platform::open_descriptors()
}

/// Nanoseconds on the system's monotonic clock, which every process reads
/// alike: `CLOCK_MONOTONIC` on Linux and `CLOCK_UPTIME_RAW` (the clock of
/// `mach_absolute_time`) on macOS, the clocks Python's `time.monotonic_ns` and
/// `time.perf_counter_ns` use there. `None` if the clock cannot be read.
pub fn monotonic_nanos() -> Option<u128> {
    #[cfg(target_os = "macos")]
    const CLOCK: libc::clockid_t = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_os = "macos"))]
    const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;
    let mut now = std::mem::MaybeUninit::<libc::timespec>::zeroed();
    // SAFETY: `now` is a live, writable timespec, the only memory the call writes.
    if unsafe { libc::clock_gettime(CLOCK, now.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: clock_gettime succeeded and filled it; all-zero was valid too.
    let now = unsafe { now.assume_init() };
    let seconds = u128::try_from(now.tv_sec).ok()?;
    let nanoseconds = u128::try_from(now.tv_nsec).ok()?;
    Some(seconds * 1_000_000_000 + nanoseconds)
}

#[cfg(target_os = "linux")]
mod platform {
    //! `/proc/<pid>/stat` field 22 is the start time in clock ticks since boot.
    //! Stamps combine it with a hash of this boot's id, so a record from an
    //! earlier boot never matches; wall-clock adjustments do not move either
    //! value. Wall-clock start times add the boot time from `/proc/stat`.
    use super::Start;
    use std::io;

    const TICK_BITS: u32 = 40;

    pub(super) fn start(pid: u32) -> io::Result<Option<Start>> {
        let Some((state, ticks)) = status(pid)? else {
            return Ok(None);
        };
        if exited(state) {
            return Ok(Some(Start::Exited));
        }
        Ok(Some(Start::Running(stamp(boot_tag()?, ticks))))
    }

    pub(super) fn started_unix_nanos(pid: u32) -> io::Result<Option<u128>> {
        let Some((state, ticks)) = status(pid)? else {
            return Ok(None);
        };
        if exited(state) {
            return Ok(None);
        }
        let boot = boot_seconds(&std::fs::read_to_string("/proc/stat")?)
            .ok_or_else(|| io::Error::other("unreadable boot time"))?;
        let hertz = u128::from(rustix::param::clock_ticks_per_second().max(1));
        Ok(Some(
            u128::from(boot) * 1_000_000_000 + u128::from(ticks) * 1_000_000_000 / hertz,
        ))
    }

    /// State and start ticks, or `None` when no process has the id.
    fn status(pid: u32) -> io::Result<Option<(char, u64)>> {
        let text = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        parse_stat(&text)
            .map(Some)
            .ok_or_else(|| io::Error::other("unreadable process status"))
    }

    fn exited(state: char) -> bool {
        matches!(state, 'Z' | 'X' | 'x')
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

    /// The `btime` line of `/proc/stat`: the boot time in Unix seconds.
    pub(super) fn boot_seconds(stat: &str) -> Option<u64> {
        stat.lines()
            .find_map(|line| line.strip_prefix("btime "))?
            .trim()
            .parse()
            .ok()
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

    /// The second field of `/proc/self/statm` counts resident pages.
    pub(super) fn resident_bytes() -> io::Result<u64> {
        let statm = std::fs::read_to_string("/proc/self/statm")?;
        let pages: u64 = statm
            .split_whitespace()
            .nth(1)
            .and_then(|pages| pages.parse().ok())
            .ok_or_else(|| io::Error::other("unreadable memory status"))?;
        Ok(pages.saturating_mul(rustix::param::page_size() as u64))
    }

    /// Entries of `/proc/self/fd`, less the one the listing itself holds open.
    pub(super) fn open_descriptors() -> io::Result<u32> {
        let mut count = 0_u32;
        for entry in std::fs::read_dir("/proc/self/fd")? {
            entry?;
            count = count.saturating_add(1);
        }
        Ok(count.saturating_sub(1))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    //! `proc_pidinfo(PROC_PIDTBSDINFO)` reports the start time recorded at fork,
    //! in microseconds since the Unix epoch; the kernel never rewrites it, so
    //! it serves as both the stamp and the wall-clock start time.
    use super::Start;
    use std::io;

    /// One `proc_pidinfo` query of `flavor` into a `T`; `None` when no process
    /// has the id.
    fn pidinfo<T>(pid: libc::c_int, flavor: libc::c_int) -> io::Result<Option<T>> {
        let size = std::mem::size_of::<T>();
        let mut info = std::mem::MaybeUninit::<T>::zeroed();
        // SAFETY: the buffer is a live, writable `T` of exactly `size` bytes.
        let written = unsafe { libc::proc_pidinfo(pid, flavor, 0, info.as_mut_ptr().cast(), size as libc::c_int) };
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
        // SAFETY: every `T` queried here (`proc_bsdinfo`, `proc_taskinfo`) is
        // plain integers and byte arrays, for which all-zero bytes, and the
        // bytes the kernel wrote over them, are valid values.
        Ok(Some(unsafe { info.assume_init() }))
    }

    fn start_micros(pid: u32) -> io::Result<Option<Start>> {
        let Ok(raw_pid) = libc::c_int::try_from(pid) else {
            return Ok(None);
        };
        let Some(info) = pidinfo::<libc::proc_bsdinfo>(raw_pid, libc::PROC_PIDTBSDINFO)? else {
            return Ok(None);
        };
        if info.pbi_status == libc::SZOMB {
            return Ok(Some(Start::Exited));
        }
        Ok(Some(Start::Running(
            info.pbi_start_tvsec
                .saturating_mul(1_000_000)
                .saturating_add(info.pbi_start_tvusec),
        )))
    }

    pub(super) fn start(pid: u32) -> io::Result<Option<Start>> {
        start_micros(pid)
    }

    pub(super) fn started_unix_nanos(pid: u32) -> io::Result<Option<u128>> {
        Ok(match start_micros(pid)? {
            Some(Start::Running(micros)) => Some(u128::from(micros) * 1_000),
            Some(Start::Exited) | None => None,
        })
    }

    fn own_pid() -> libc::c_int {
        // SAFETY: getpid has no preconditions and cannot fail.
        unsafe { libc::getpid() }
    }

    pub(super) fn resident_bytes() -> io::Result<u64> {
        pidinfo::<libc::proc_taskinfo>(own_pid(), libc::PROC_PIDTASKINFO)?
            .map(|info| info.pti_resident_size)
            .ok_or_else(|| io::Error::other("process information unavailable"))
    }

    /// `PROC_PIDLISTFDS` fills one `proc_fdinfo` per open descriptor. The
    /// buffer has room for more than the sizing call reported, since
    /// descriptors may open in between.
    pub(super) fn open_descriptors() -> io::Result<u32> {
        let record = std::mem::size_of::<libc::proc_fdinfo>();
        // SAFETY: a null buffer of size 0 only asks for the size needed.
        let needed = unsafe { libc::proc_pidinfo(own_pid(), libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if needed < 0 {
            return Err(io::Error::last_os_error());
        }
        let capacity = needed as usize / record + 16;
        let mut records = Vec::<libc::proc_fdinfo>::with_capacity(capacity);
        let bytes = libc::c_int::try_from(capacity * record).map_err(io::Error::other)?;
        // SAFETY: the vector owns writable memory for `capacity` records, which
        // the call fills up to `bytes`; the records are counted, never read.
        let written =
            unsafe { libc::proc_pidinfo(own_pid(), libc::PROC_PIDLISTFDS, 0, records.as_mut_ptr().cast(), bytes) };
        if written < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(u32::try_from(written as usize / record).unwrap_or(u32::MAX))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::Start;
    use std::io;

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "process information unavailable")
    }
    pub(super) fn start(_: u32) -> io::Result<Option<Start>> {
        Err(unsupported())
    }
    pub(super) fn started_unix_nanos(_: u32) -> io::Result<Option<u128>> {
        Err(unsupported())
    }
    pub(super) fn resident_bytes() -> io::Result<u64> {
        Err(unsupported())
    }
    pub(super) fn open_descriptors() -> io::Result<u32> {
        Err(unsupported())
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
        assert!(started_unix_nanos(pid).unwrap().is_some());
        drop(child.stdin.take());
        // `read` fails at end of input; only the reaping matters here.
        child.wait().unwrap();
        let state = liveness(pid, created);
        assert!(
            matches!(state, ProcessLiveness::Dead | ProcessLiveness::Unknown),
            "{state:?}"
        );
    }

    #[test]
    fn own_start_time_is_on_the_wall_clock_before_now() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let started = started_unix_nanos(std::process::id()).unwrap().unwrap();
        // Clock ticks round the start down; a test binary is minutes old at most.
        assert!(started <= now, "{started} > {now}");
        assert!(now - started < 86_400 * 1_000_000_000, "{started} is not recent");
        // No process can have this id (Linux caps ids at 2^22, macOS at 99999).
        assert_eq!(started_unix_nanos(u32::MAX).unwrap(), None);
        assert_eq!(start(u32::MAX).unwrap(), None);
    }

    #[test]
    fn the_monotonic_clock_never_goes_back() {
        let first = monotonic_nanos().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = monotonic_nanos().unwrap();
        assert!(second >= first + 1_000_000, "{first} then {second}");
    }

    #[test]
    fn resource_counters_see_this_process() {
        assert!(resident_bytes().unwrap() > 0);
        // Standard input, output and error are open in every test process.
        let open = open_descriptors().unwrap();
        assert!(open >= 3, "{open}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stat_parsing_survives_odd_command_names() {
        let line = "4242 (a) b (c)) S 1 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10";
        assert_eq!(platform::parse_stat(line), Some(('S', 987_654)));
        assert_eq!(platform::parse_stat("1 (x) Z"), None);
        assert_ne!(platform::stamp(1, 5), platform::stamp(2, 5));
        assert_eq!(
            platform::boot_seconds("cpu 1 2 3\nintr 5\nbtime 1700000000\nprocesses 9\n"),
            Some(1_700_000_000)
        );
        assert_eq!(platform::boot_seconds("cpu 1 2 3\n"), None);
    }
}
