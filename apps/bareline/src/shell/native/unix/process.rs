// SPDX-License-Identifier: MPL-2.0
//! Processes: whether another Bareline still runs, this process's counters and
//! clock (`bareline_platform_posix::process`), and the program launcher, which
//! runs nothing until process containment exists for these systems.
use super::error::{Error, unsupported, unsupported_io};
use bareline_platform::Capability;
use bareline_platform_posix::process;
use std::{io, path::PathBuf};

/// Runs no program: process containment for these systems is not built yet.
pub struct ProcessLauncher;
impl bareline_app::macros::model::process::ProcessLauncher for ProcessLauncher {
    fn spawn(
        &self,
        _command: &mut std::process::Command,
    ) -> io::Result<(
        std::process::Child,
        Box<dyn bareline_app::macros::model::process::ProcessTreeGuard>,
    )> {
        Err(unsupported_io(Capability::Shell))
    }
}
pub fn resolve_program(_name: &str) -> std::result::Result<PathBuf, String> {
    Err(unsupported(Capability::Shell).to_string())
}

/// Nanoseconds on the system's monotonic clock: the clock the performance
/// driver's `time.perf_counter_ns` reads on these systems, as QPC is on
/// Windows, so launch-to-present times can span the two processes.
pub fn monotonic_ns() -> Option<u128> {
    process::monotonic_nanos()
}

/// The process's resident memory, the counter performance runs and the idle
/// log record (Windows records private bytes).
pub fn private_bytes() -> super::Result<u64> {
    process::resident_bytes().map_err(|error| Error::other(format!("Process memory is unavailable: {error}")))
}

/// Open file descriptors for `--diag handles`. GDI and USER objects do not
/// exist on these systems, so those two counters stay 0.
pub fn handle_counters() -> (u32, u32, u32) {
    (process::open_descriptors().unwrap_or(0), 0, 0)
}

pub mod alive {
    //! Whether the process that owns a recovery journal or a replace job still
    //! runs. Unknown owners count as running, as on Windows, so a doubtful case
    //! never sweeps or offers another window's files; that matters here because
    //! every window runs as its own instance (see `instance`).
    use bareline_platform_posix::process::{self, Start};

    /// True unless the process is known to be gone (no such id, or exited).
    pub fn running(id: u32) -> bool {
        id == std::process::id() || !matches!(process::start(id), Ok(None | Some(Start::Exited)))
    }
    /// Start time of the running process with this id, in Unix nanoseconds by
    /// the system clock; `None` when it is gone or cannot be queried.
    pub fn started(id: u32) -> Option<u128> {
        process::started_unix_nanos(id).ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_owners_are_running_until_known_gone() {
        let own = std::process::id();
        assert!(alive::running(own));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        assert!(alive::started(own).is_some_and(|started| started <= now));
        // No process can have this id on Linux (ids stop at 2^22) or macOS.
        let gone = u32::MAX;
        assert!(!alive::running(gone));
        assert_eq!(alive::started(gone), None);
        // A child that exited and was reaped is gone too.
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        if !alive::running(pid) {
            assert_eq!(alive::started(pid), None);
        }
    }

    #[test]
    fn counters_and_the_clock_describe_this_process() {
        assert!(private_bytes().unwrap() > 0);
        let (descriptors, gdi, user) = handle_counters();
        assert!(descriptors >= 3, "{descriptors}");
        assert_eq!((gdi, user), (0, 0));
        let first = monotonic_ns().unwrap();
        assert!(monotonic_ns().unwrap() >= first);
    }
}
