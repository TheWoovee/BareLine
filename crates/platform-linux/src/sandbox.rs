// SPDX-License-Identifier: MPL-2.0
//! Isolation of the extension host on Linux (SEC-03), the counterpart of the
//! Windows restricted token and job object.
//!
//! The host starts with no inherited environment, no inherited descriptors
//! beyond standard input and output (which are `/dev/null`), an empty private
//! working directory, `PR_SET_NO_NEW_PRIVS`, its own process group with a
//! parent-death signal, and resource limits sized from its execution budget:
//! CPU seconds, address space, data segment (the memory cap, as the job's
//! process memory limit), descriptors, tasks and no core dumps. Landlock then
//! confines the filesystem to executing the host and its system libraries and
//! reading the component and any explicit grants; on ABI 4 and later TCP is
//! denied, and on ABI 6 and later signals to processes outside the sandbox.
//!
//! Landlock needs Linux 5.13 or later with the LSM enabled. Where it is missing
//! the probe reports `Isolation::Unsupported` with the reason. The default
//! sandbox then refuses to start the host, as Windows does when its sandbox
//! cannot be established (SEC-05); a sandbox built with
//! [`LinuxHostSandbox::allow_without_landlock`] starts it with the remaining
//! measures and reports the same `Unsupported` isolation to its caller. It is
//! never skipped silently.
use bareline_extensions_protocol::ExecutionBudget;
use bareline_macros::process::ProcessTreeGuard;
use bareline_platform_posix::extension_transport::{HostSandbox, HostSpawn, Isolation, SpawnedHost};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    process::{Pid, Resource, Rlimit, Signal},
};
use std::{
    io,
    os::{
        fd::{AsRawFd, OwnedFd, RawFd},
        unix::{
            fs::{DirBuilderExt, MetadataExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Raw Landlock system calls; the kernel ABI is stable and versioned.
mod landlock {
    use std::{
        io,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        path::Path,
    };

    const CREATE_RULESET_VERSION: u32 = 1;
    const RULE_PATH_BENEATH: libc::c_int = 1;
    pub const EXECUTE: u64 = 1 << 0;
    pub const WRITE_FILE: u64 = 1 << 1;
    pub const READ_FILE: u64 = 1 << 2;
    pub const READ_DIR: u64 = 1 << 3;
    const REFER: u64 = 1 << 13;
    const TRUNCATE: u64 = 1 << 14;
    const IOCTL_DEV: u64 = 1 << 15;
    /// The rights that apply to a file rather than a directory.
    const FILE_RIGHTS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;
    const NET_BIND_TCP: u64 = 1 << 0;
    const NET_CONNECT_TCP: u64 = 1 << 1;
    const SCOPE_SIGNAL: u64 = 1 << 1;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
        scoped: u64,
    }
    #[repr(C, packed)]
    struct PathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    /// The kernel's Landlock ABI version, or why there is none.
    pub fn abi() -> Result<u32, String> {
        // SAFETY: the version query takes no attribute pointer.
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<RulesetAttr>(),
                0usize,
                CREATE_RULESET_VERSION,
            )
        };
        if result > 0 {
            return Ok(result as u32);
        }
        let error = io::Error::last_os_error();
        Err(match error.raw_os_error() {
            Some(libc::ENOSYS) => "this kernel has no Landlock (Linux 5.13 or later is needed)".into(),
            Some(libc::EOPNOTSUPP) => "Landlock is disabled in this kernel's boot configuration".into(),
            _ => format!("Landlock is unavailable ({error})"),
        })
    }
    /// Every filesystem right the ABI knows, so anything not granted is denied.
    fn handled(abi: u32) -> u64 {
        let mut rights = (1 << 13) - 1;
        if abi >= 2 {
            rights |= REFER;
        }
        if abi >= 3 {
            rights |= TRUNCATE;
        }
        if abi >= 5 {
            rights |= IOCTL_DEV;
        }
        rights
    }
    pub struct Ruleset {
        fd: OwnedFd,
        handled: u64,
    }
    impl Ruleset {
        pub fn new(abi: u32) -> io::Result<Self> {
            let attr = RulesetAttr {
                handled_access_fs: handled(abi),
                // No TCP at all: the host talks to the editor over a Unix socket.
                handled_access_net: if abi >= 4 { NET_BIND_TCP | NET_CONNECT_TCP } else { 0 },
                scoped: if abi >= 6 { SCOPE_SIGNAL } else { 0 },
            };
            // SAFETY: a valid attribute of the stated size; older kernels accept
            // the larger structure because the fields they do not know are zero.
            let fd = unsafe {
                libc::syscall(
                    libc::SYS_landlock_create_ruleset,
                    &raw const attr,
                    std::mem::size_of::<RulesetAttr>(),
                    0u32,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                // SAFETY: the kernel returned a new descriptor that nothing else owns.
                fd: unsafe { OwnedFd::from_raw_fd(fd as i32) },
                handled: attr.handled_access_fs,
            })
        }
        /// Allows `access` beneath `path` (on the file itself for a file).
        pub fn allow(&self, path: &Path, access: u64) -> io::Result<()> {
            let target = rustix::fs::open(
                path,
                rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )?;
            let is_directory = rustix::fs::fstat(&target)?.st_mode & libc::S_IFMT == libc::S_IFDIR;
            let mut allowed = access & self.handled;
            if !is_directory {
                allowed &= FILE_RIGHTS;
            }
            let attr = PathBeneathAttr {
                allowed_access: allowed,
                parent_fd: target.as_raw_fd(),
            };
            // SAFETY: a valid rule attribute and our ruleset descriptor.
            let result = unsafe {
                libc::syscall(
                    libc::SYS_landlock_add_rule,
                    self.fd.as_raw_fd(),
                    RULE_PATH_BENEATH,
                    &raw const attr,
                    0u32,
                )
            };
            if result != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        pub fn fd(&self) -> &OwnedFd {
            &self.fd
        }
    }
}

/// Folders the host's dynamic loader and C library read and map executable.
const SYSTEM_LIBRARIES: [&str; 7] = [
    "/lib",
    "/lib32",
    "/lib64",
    "/usr/lib",
    "/usr/lib32",
    "/usr/lib64",
    "/usr/libexec",
];
/// Files the dynamic loader reads.
const SYSTEM_FILES: [&str; 1] = ["/etc/ld.so.cache"];

/// The kernel's filesystem confinement for the host.
pub fn probe() -> Isolation {
    match landlock::abi() {
        Ok(abi) => Isolation::Enforced(format!("Landlock ABI {abi}")),
        Err(reason) => Isolation::Unsupported(reason),
    }
}

/// How the extension host is confined. The defaults mirror the Windows
/// launcher: 512 MiB of memory and a single process's worth of tasks.
#[derive(Clone, Debug)]
pub struct LinuxHostSandbox {
    /// `RLIMIT_DATA`: private writable memory, the analogue of the job's
    /// process memory limit.
    pub memory_limit_bytes: u64,
    /// `RLIMIT_AS`: address space. Wasmtime reserves guard regions far larger
    /// than it commits, so this bounds reservations, not use.
    pub address_space_limit_bytes: u64,
    /// `RLIMIT_NOFILE`.
    pub open_files: u64,
    /// Threads the host may start beyond the tasks this user runs at launch.
    /// `RLIMIT_NPROC` counts every task of the user, so it is set to that count
    /// plus this headroom: enough for the runtime's threads, not for a fork bomb.
    pub task_headroom: u64,
    /// Refuse to start the host without Landlock (the default, SEC-05).
    pub require_landlock: bool,
    /// Further files or folders the host may read, beyond its component.
    pub read_grants: Vec<PathBuf>,
}
impl Default for LinuxHostSandbox {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 512 * 1024 * 1024,
            address_space_limit_bytes: 64 * 1024 * 1024 * 1024,
            open_files: 64,
            task_headroom: 64,
            require_landlock: true,
            read_grants: Vec::new(),
        }
    }
}
impl LinuxHostSandbox {
    /// Starts the host with the remaining measures where Landlock is missing; the
    /// launch then reports `Isolation::Unsupported`.
    pub fn allow_without_landlock(mut self) -> Self {
        self.require_landlock = false;
        self
    }
    pub fn with_read_grant(mut self, path: impl Into<PathBuf>) -> Self {
        self.read_grants.push(path.into());
        self
    }
    fn limits(&self, budget: ExecutionBudget) -> io::Result<Vec<(Resource, Rlimit)>> {
        let exact = |value: u64| Rlimit {
            current: Some(value),
            maximum: Some(value),
        };
        // The watchdog stops the host at its wall-clock deadline; CPU time is a
        // backstop one second later (SIGXCPU, then SIGKILL at the hard limit).
        let cpu = budget.timeout_ms().div_ceil(1000) + 1;
        Ok(vec![
            (
                Resource::Cpu,
                Rlimit {
                    current: Some(cpu),
                    maximum: Some(cpu + 1),
                },
            ),
            (Resource::As, exact(self.address_space_limit_bytes)),
            (Resource::Data, exact(self.memory_limit_bytes)),
            (Resource::Nofile, exact(self.open_files)),
            (Resource::Nproc, exact(user_tasks()?.saturating_add(self.task_headroom))),
            (Resource::Core, exact(0)),
        ])
    }
    /// The rules for one launch.
    fn ruleset(&self, abi: u32, spawn: &HostSpawn<'_>) -> io::Result<landlock::Ruleset> {
        let ruleset = landlock::Ruleset::new(abi)?;
        ruleset.allow(spawn.executable, landlock::EXECUTE | landlock::READ_FILE)?;
        ruleset.allow(spawn.component, landlock::READ_FILE)?;
        for grant in &self.read_grants {
            ruleset.allow(grant, landlock::READ_FILE | landlock::READ_DIR)?;
        }
        for library in SYSTEM_LIBRARIES {
            match ruleset.allow(Path::new(library), landlock::EXECUTE | landlock::READ_FILE) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                other => other?,
            }
        }
        for file in SYSTEM_FILES {
            match ruleset.allow(Path::new(file), landlock::READ_FILE) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                other => other?,
            }
        }
        Ok(ruleset)
    }
}
impl HostSandbox for LinuxHostSandbox {
    fn spawn_host(&self, spawn: &HostSpawn<'_>) -> io::Result<SpawnedHost> {
        let isolation = probe();
        let ruleset = match &isolation {
            Isolation::Enforced(_) => Some(self.ruleset(landlock::abi().map_err(io::Error::other)?, spawn)?),
            Isolation::Unsupported(_) if self.require_landlock => {
                return Err(io::Error::new(io::ErrorKind::Unsupported, isolation.to_string()));
            }
            Isolation::Unsupported(_) => None,
        };
        let limits = self.limits(spawn.budget)?;
        let mut command = host_command(spawn)?;
        let ruleset_fd: Option<RawFd> = ruleset.as_ref().map(|ruleset| ruleset.fd().as_raw_fd());
        let parent = rustix::process::getpid();
        // SAFETY: the closure runs between fork and exec and makes only
        // async-signal-safe system calls; it allocates nothing.
        unsafe {
            command.pre_exec(move || contain(parent, &limits, ruleset_fd));
        }
        let child = command.spawn()?;
        // The rules live on in the child; the parent's copy can go.
        drop(ruleset);
        let guard = HostTree::new(child.id());
        Ok(SpawnedHost {
            child,
            guard: Box::new(guard),
            isolation,
        })
    }
}

/// The host command: launched from the verified descriptor where `/proc` allows,
/// with nothing of this process's environment, working directory or stdio.
fn host_command(spawn: &HostSpawn<'_>) -> io::Result<Command> {
    let descriptor = PathBuf::from(format!("/proc/self/fd/{}", spawn.executable_file.as_raw_fd()));
    let program = if descriptor.exists() {
        descriptor
    } else {
        spawn.executable.to_path_buf()
    };
    let mut command = Command::new(program);
    command
        .arg0(spawn.executable)
        .args(&spawn.arguments)
        .env_clear()
        .current_dir(working_directory()?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}
/// An empty folder only this user can enter, inside the runtime folder. The
/// host gets no Landlock right to it, so it stays empty.
fn working_directory() -> io::Result<PathBuf> {
    let runtime = bareline_platform_posix::paths::runtime_dir("bareline")?;
    let folder = runtime.join("extension-host");
    std::fs::create_dir_all(&runtime)?;
    match std::fs::DirBuilder::new().mode(0o700).create(&folder) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(&folder)?;
    if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the extension host folder belongs to someone else",
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(folder)
}
/// The tasks (threads) this user runs, which `RLIMIT_NPROC` counts.
fn user_tasks() -> io::Result<u64> {
    let uid = rustix::process::getuid().as_raw();
    let mut total = 0u64;
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        // Processes come and go while the list is read; a vanished one counts nothing.
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        let field = |name: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok())
        };
        if field("Uid:") == Some(u64::from(uid)) {
            total += field("Threads:").unwrap_or(1);
        }
    }
    Ok(total.max(1))
}
/// Runs in the child between fork and exec: only async-signal-safe calls.
fn contain(parent: Pid, limits: &[(Resource, Rlimit)], ruleset: Option<RawFd>) -> io::Result<()> {
    // Its own process group, so the guard reaches anything it starts.
    rustix::process::setpgid(None, None)?;
    rustix::process::set_parent_process_death_signal(Some(Signal::KILL))?;
    // The editor may have died between fork and the line above.
    if rustix::process::getppid() != Some(parent) {
        return Err(io::Error::from(rustix::io::Errno::SRCH));
    }
    rustix::thread::set_no_new_privs(true)?;
    for (resource, limit) in limits {
        rustix::process::setrlimit(*resource, *limit)?;
    }
    if let Some(ruleset) = ruleset {
        // SAFETY: a plain system call on the inherited ruleset descriptor.
        if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0u32) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    close_inherited_on_exec()
}
/// Marks every descriptor above standard error close-on-exec, so nothing the
/// editor opened without that flag (by a library, for example) reaches the host.
fn close_inherited_on_exec() -> io::Result<()> {
    // SAFETY: a plain system call; CLOSE_RANGE_CLOEXEC (Linux 5.11) only sets flags.
    let result = unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) };
    if result == 0 {
        return Ok(());
    }
    // Older kernels: set the flag one descriptor at a time.
    for raw in 3..65_536 {
        // SAFETY: `fcntl` on a number that may not be open fails with EBADF.
        let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(raw) };
        if let Ok(flags) = rustix::io::fcntl_getfd(fd) {
            rustix::io::fcntl_setfd(fd, flags | rustix::io::FdFlags::CLOEXEC)?;
        }
    }
    Ok(())
}

/// The host and its process group. Terminating, or dropping the guard (as the
/// Windows job object's kill-on-close does), kills both.
struct HostTree {
    pid: Option<Pid>,
    /// Identifies the host itself even after its pid is reused (Linux 5.3+).
    pidfd: Option<OwnedFd>,
}
impl HostTree {
    fn new(pid: u32) -> Self {
        let pid = i32::try_from(pid).ok().and_then(Pid::from_raw);
        let pidfd = pid.and_then(|pid| rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).ok());
        Self { pid, pidfd }
    }
    /// Whether the host has exited; unknown counts as running.
    fn exited(&self) -> bool {
        let Some(pidfd) = &self.pidfd else {
            return false;
        };
        let mut fds = [PollFd::new(pidfd, PollFlags::IN)];
        matches!(poll(&mut fds, Some(&Timespec::default())), Ok(count) if count > 0)
    }
}
impl ProcessTreeGuard for HostTree {
    fn terminate(&mut self) -> io::Result<()> {
        let Some(pid) = self.pid else {
            return Ok(());
        };
        // The group is the host's only while the host runs: once it exited, its
        // number may name someone else's group, so only the host is signalled.
        if !self.exited() {
            match rustix::process::kill_process_group(pid, Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                Err(errno) => return Err(errno.into()),
            }
        }
        let result = match &self.pidfd {
            Some(pidfd) => rustix::process::pidfd_send_signal(pidfd, Signal::KILL),
            None => rustix::process::kill_process(pid, Signal::KILL),
        };
        match result {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(errno) => Err(errno.into()),
        }
    }
}
impl Drop for HostTree {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_follow_the_budget_and_count_this_users_tasks() {
        let sandbox = LinuxHostSandbox::default();
        let limits = sandbox.limits(ExecutionBudget::Interactive).unwrap();
        let limit = |resource| limits.iter().find(|(kind, _)| *kind == resource).unwrap().1;
        assert_eq!(limit(Resource::Cpu).current, Some(6));
        assert_eq!(limit(Resource::Cpu).maximum, Some(7));
        assert_eq!(limit(Resource::Data).maximum, Some(512 * 1024 * 1024));
        assert_eq!(limit(Resource::Nofile).current, Some(64));
        assert_eq!(limit(Resource::Core).current, Some(0));
        let background = sandbox.limits(ExecutionBudget::Background).unwrap();
        assert_eq!(background[0].1.current, Some(121));
        // This test's own threads are among the user's tasks, and the limit leaves
        // the headroom above them.
        assert!(user_tasks().unwrap() >= 2);
        assert!(limit(Resource::Nproc).current.unwrap() > sandbox.task_headroom);
    }
    #[test]
    fn the_probe_names_the_mechanism_or_the_reason() {
        match probe() {
            Isolation::Enforced(mechanism) => assert!(mechanism.starts_with("Landlock ABI "), "{mechanism}"),
            Isolation::Unsupported(reason) => assert!(reason.contains("Landlock"), "{reason}"),
        }
        assert!(
            Isolation::Unsupported("no Landlock".into())
                .to_string()
                .starts_with("isolation=Unsupported")
        );
    }
    #[test]
    fn the_guard_kills_a_running_host_and_spares_a_finished_one() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let mut guard = HostTree::new(child.id());
        assert!(!guard.exited());
        guard.terminate().unwrap();
        assert!(child.wait().unwrap().code().is_none(), "killed by a signal");
        // Terminating again (or dropping) after the host is gone is harmless.
        guard.terminate().unwrap();
        drop(guard);
    }
}
