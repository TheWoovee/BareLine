// SPDX-License-Identifier: MPL-2.0
//! Launching the extension host confined; see `isolation_policy` for the
//! launch chain, the profile and why it is shaped this way.
//!
//! [`MacIsolation::spawn_host`] first proves that the exact profile applies,
//! by running the same chain on `/usr/bin/true`. If that fails (no
//! `sandbox-exec`, a profile the system rejects, limits that cannot be set),
//! the host is not started: the error carries [`IsolationUnavailable`] with
//! kind `PermissionDenied`, the counterpart of the Windows adapter's
//! `SandboxUnavailable`, and [`MacIsolation::probe`] reports
//! `isolation=unsupported`. There is no weaker fallback.
//!
//! [`MacHostSandbox`] is the shared Unix launch's `HostSandbox` on macOS: the
//! verified host runs only under this isolation, as Linux's runs only under
//! Landlock.
use crate::isolation_policy::{
    IsolationRequest, LIMITS_FAILED, MECHANISM, PROBE_PROGRAM, ResourceLimits, SANDBOX_EXEC, SHELL, host_request,
    launch_arguments, probe_profile, sandbox_profile, scrubbed_environment,
};
use bareline_macros::process::ProcessTreeGuard;
use bareline_platform_posix::extension_transport::{HostProcess, HostSandbox, HostSpawn, Isolation, SpawnedHost};
use std::{
    collections::HashSet,
    ffi::{CString, OsString},
    io,
    os::{
        fd::{BorrowedFd, RawFd},
        unix::{ffi::OsStrExt, process::ExitStatusExt},
    },
    path::Path,
    process::ExitStatus,
    sync::{Arc, Mutex, PoisonError},
};

/// The confined launch could not be established, so the host was not started.
#[derive(Debug)]
pub struct IsolationUnavailable(String);
impl std::fmt::Display for IsolationUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Extension sandbox unavailable on this system; the extension was not started ({})",
            self.0
        )
    }
}
impl std::error::Error for IsolationUnavailable {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IsolationSupport {
    Sandboxed,
    Unsupported { reason: String },
}
impl IsolationSupport {
    /// The diagnostics field, `isolation=sandboxed` or `isolation=unsupported`.
    pub fn report(&self) -> String {
        match self {
            Self::Sandboxed => "isolation=sandboxed".into(),
            Self::Unsupported { reason } => format!("isolation=unsupported reason=\"{reason}\""),
        }
    }
}

/// Spawns confined extension hosts. Profiles that passed the probe are
/// remembered, so a repeated launch of the same component probes once.
#[derive(Default)]
pub struct MacIsolation {
    proven: Mutex<HashSet<String>>,
}

impl MacIsolation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `request` can run confined on this system, proven by running
    /// the launch chain with its profile on `/usr/bin/true`.
    pub fn probe(&self, request: &IsolationRequest) -> IsolationSupport {
        let unsupported = |reason: String| IsolationSupport::Unsupported { reason };
        let profile = match sandbox_profile(request) {
            Ok(profile) => profile,
            Err(error) => return unsupported(error.to_string()),
        };
        if self
            .proven
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&profile)
        {
            return IsolationSupport::Sandboxed;
        }
        if !Path::new(SANDBOX_EXEC).exists() {
            return unsupported(format!("{SANDBOX_EXEC} is not available"));
        }
        let probe = match probe_profile(request) {
            Ok(probe) => probe,
            Err(error) => return unsupported(error.to_string()),
        };
        let limits = ResourceLimits::for_budget(request.budget);
        let argv = launch_arguments(limits, &probe, Path::new(PROBE_PROGRAM), &[]);
        let status = spawn(&argv, &[]).and_then(|pid| Reaper::new(pid).wait());
        match status {
            Ok(status) if status.success() => {
                self.proven
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(profile);
                IsolationSupport::Sandboxed
            }
            Ok(status) if status.code() == Some(LIMITS_FAILED) => {
                unsupported("the resource limits could not be set".into())
            }
            Ok(status) => unsupported(format!("the sandbox profile could not be applied ({status})")),
            Err(error) => unsupported(format!("the launch failed: {error}")),
        }
    }

    /// Starts the host confined. `inherit` maps descriptors of this process
    /// to the numbers the host receives them as (a transport socket); every
    /// other descriptor is closed in the host.
    pub fn spawn_host(
        &self,
        request: &IsolationRequest,
        inherit: &[(BorrowedFd<'_>, RawFd)],
    ) -> io::Result<(MacHostChild, Box<dyn ProcessTreeGuard>)> {
        if let IsolationSupport::Unsupported { reason } = self.probe(request) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                IsolationUnavailable(reason),
            ));
        }
        let profile = sandbox_profile(request)?;
        let limits = ResourceLimits::for_budget(request.budget);
        let argv = launch_arguments(limits, &profile, &request.runtime, &request.arguments);
        let mapped: Vec<(RawFd, RawFd)> = inherit
            .iter()
            .map(|(source, target)| (std::os::fd::AsRawFd::as_raw_fd(source), *target))
            .collect();
        let pid = spawn(&argv, &mapped)?;
        let reaper = Reaper::new(pid);
        Ok((MacHostChild { reaper: reaper.clone() }, Box::new(HostGroup { reaper })))
    }
}

/// The `HostSandbox` of the shared verified launch (`run_verified_host_in`):
/// the host starts under the sandbox profile and limits, after the probe
/// proved they apply, or it does not start (SEC-05).
#[derive(Default)]
pub struct MacHostSandbox {
    isolation: MacIsolation,
}
impl MacHostSandbox {
    /// The Extensions page's line, without starting anything: `sandbox_init`
    /// where `sandbox-exec` exists. Every launch still proves the profile first.
    pub fn isolation() -> Isolation {
        if Path::new(SANDBOX_EXEC).exists() {
            Isolation::Enforced(MECHANISM.into())
        } else {
            Isolation::Unsupported(format!("{SANDBOX_EXEC} is not available"))
        }
    }
}
impl HostSandbox for MacHostSandbox {
    fn spawn_host(&self, spawn: &HostSpawn<'_>) -> io::Result<SpawnedHost> {
        let request = host_request(spawn)?;
        if let IsolationSupport::Unsupported { reason } = self.isolation.probe(&request) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                Isolation::Unsupported(reason).to_string(),
            ));
        }
        let (child, guard) = self.isolation.spawn_host(&request, &[])?;
        Ok(SpawnedHost {
            child: Box::new(child),
            guard,
            isolation: Isolation::Enforced(MECHANISM.into()),
        })
    }
}
impl HostProcess for MacHostChild {
    fn id(&self) -> u32 {
        MacHostChild::id(self)
    }
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        MacHostChild::try_wait(self)
    }
    fn wait(&mut self) -> io::Result<ExitStatus> {
        MacHostChild::wait(self)
    }
}

/// The spawned pid and whether it was reaped. A reaped pid may be reused by
/// an unrelated process, so signals are sent only under the lock and only
/// while the pid is still unreaped (running or a zombie).
#[derive(Clone)]
struct Reaper {
    pid: libc::pid_t,
    reaped: Arc<Mutex<Option<ExitStatus>>>,
}
impl Reaper {
    fn new(pid: libc::pid_t) -> Self {
        Self {
            pid,
            reaped: Arc::new(Mutex::new(None)),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<ExitStatus>> {
        self.reaped.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Reaps without blocking; `None` while the process runs.
    fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
        let mut reaped = self.lock();
        if let Some(status) = *reaped {
            return Ok(Some(status));
        }
        let mut raw = 0;
        // SAFETY: waits for our own child; `raw` is a plain integer.
        match unsafe { libc::waitpid(self.pid, &mut raw, libc::WNOHANG) } {
            0 => Ok(None),
            -1 => Err(io::Error::last_os_error()),
            _ => {
                let status = ExitStatus::from_raw(raw);
                *reaped = Some(status);
                Ok(Some(status))
            }
        }
    }
    /// Blocks until the process exits, without holding the lock while it runs.
    fn wait(&self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = *self.lock() {
                return Ok(status);
            }
            // SAFETY: an all-zero siginfo_t is a valid output buffer.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // WNOWAIT leaves the exited child unreaped, so its pid cannot be
            // reused before `try_wait` reaps it under the lock.
            // SAFETY: waits for our own child; `info` is writable.
            let waited = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            };
            if waited == -1 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    // Already reaped elsewhere (ECHILD) shows up below.
                    if let Some(status) = *self.lock() {
                        return Ok(status);
                    }
                    return Err(error);
                }
                continue;
            }
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
        }
    }
    /// Kills the host's process group while it is unreaped.
    fn kill(&self) -> io::Result<()> {
        let reaped = self.lock();
        if reaped.is_some() {
            return Ok(());
        }
        // SAFETY: signals our own child's process group (its pgid is its pid).
        if unsafe { libc::killpg(self.pid, libc::SIGKILL) } == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }
}

/// The confined host process.
pub struct MacHostChild {
    reaper: Reaper,
}
impl MacHostChild {
    pub fn id(&self) -> u32 {
        self.reaper.pid as u32
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.reaper.try_wait()
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.reaper.wait()
    }
    pub fn kill(&mut self) -> io::Result<()> {
        self.reaper.kill()
    }
}
impl Drop for MacHostChild {
    /// A host is never left running or as a zombie behind its handle.
    fn drop(&mut self) {
        if matches!(self.reaper.try_wait(), Ok(None)) {
            let _ = self.reaper.kill();
            let _ = self.reaper.wait();
        }
    }
}

/// The host's process group. The profile denies `fork`, so the group is the
/// host alone; killing the group still covers anything that escaped that rule.
struct HostGroup {
    reaper: Reaper,
}
impl ProcessTreeGuard for HostGroup {
    fn terminate(&mut self) -> io::Result<()> {
        self.reaper.kill()
    }
}

fn c_string(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "an argument contains a NUL byte"))
}

/// Owns spawn attributes and file actions until the call returns.
struct SpawnSetup {
    attributes: libc::posix_spawnattr_t,
    actions: libc::posix_spawn_file_actions_t,
}
impl Drop for SpawnSetup {
    fn drop(&mut self) {
        // SAFETY: both were initialized by `SpawnSetup::new` and are destroyed once.
        unsafe {
            libc::posix_spawn_file_actions_destroy(&mut self.actions);
            libc::posix_spawnattr_destroy(&mut self.attributes);
        }
    }
}
fn check(status: libc::c_int) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status))
    }
}

/// `posix_spawn` of [`SHELL`] with `argv`, the scrubbed environment, its own
/// process group, default SIGPIPE handling and only `inherit` plus standard
/// input, output and error (on `/dev/null`) open.
fn spawn(argv: &[OsString], inherit: &[(RawFd, RawFd)]) -> io::Result<libc::pid_t> {
    let arguments: Vec<CString> = argv
        .iter()
        .map(|argument| c_string(argument.as_bytes()))
        .collect::<io::Result<_>>()?;
    let environment: Vec<CString> = scrubbed_environment()
        .iter()
        .map(|(name, value)| c_string(&[name.as_bytes(), b"=", value.as_bytes()].concat()))
        .collect::<io::Result<_>>()?;
    let pointers = |strings: &[CString]| -> Vec<*mut libc::c_char> {
        strings
            .iter()
            .map(|string| string.as_ptr().cast_mut())
            .chain(std::iter::once(std::ptr::null_mut()))
            .collect()
    };
    let (argv, envp) = (pointers(&arguments), pointers(&environment));
    let program = c_string(SHELL.as_bytes())?;
    let null_device = c_string(b"/dev/null")?;
    // SAFETY: every pointer handed to posix_spawn refers to a CString or a
    // pointer array that lives until the call returns; the attribute and
    // file-action objects are initialized before use and destroyed by
    // `SpawnSetup`.
    unsafe {
        let mut setup = SpawnSetup {
            attributes: std::ptr::null_mut(),
            actions: std::ptr::null_mut(),
        };
        check(libc::posix_spawnattr_init(&mut setup.attributes))?;
        if let Err(error) = check(libc::posix_spawn_file_actions_init(&mut setup.actions)) {
            libc::posix_spawnattr_destroy(&mut setup.attributes);
            std::mem::forget(setup);
            return Err(error);
        }
        let flags = libc::POSIX_SPAWN_CLOEXEC_DEFAULT
            | libc::POSIX_SPAWN_SETPGROUP
            | libc::POSIX_SPAWN_SETSIGDEF
            | libc::POSIX_SPAWN_SETSIGMASK;
        check(libc::posix_spawnattr_setflags(
            &mut setup.attributes,
            flags as libc::c_short,
        ))?;
        check(libc::posix_spawnattr_setpgroup(&mut setup.attributes, 0))?;
        // Rust ignores SIGPIPE; ignored signals survive exec, so restore it.
        let mut defaults: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut defaults);
        libc::sigaddset(&mut defaults, libc::SIGPIPE);
        check(libc::posix_spawnattr_setsigdefault(&mut setup.attributes, &defaults))?;
        let mut mask: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        check(libc::posix_spawnattr_setsigmask(&mut setup.attributes, &mask))?;
        for (fd, mode) in [(0, libc::O_RDONLY), (1, libc::O_WRONLY), (2, libc::O_WRONLY)] {
            check(libc::posix_spawn_file_actions_addopen(
                &mut setup.actions,
                fd,
                null_device.as_ptr(),
                mode,
                0,
            ))?;
        }
        for (source, target) in inherit {
            check(libc::posix_spawn_file_actions_adddup2(
                &mut setup.actions,
                *source,
                *target,
            ))?;
        }
        let mut pid: libc::pid_t = 0;
        check(libc::posix_spawn(
            &mut pid,
            program.as_ptr(),
            &setup.actions,
            &setup.attributes,
            argv.as_ptr(),
            envp.as_ptr(),
        ))?;
        Ok(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_extensions_protocol::ExecutionBudget;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // The sandbox matches real paths: /var/folders is /private/var/folders.
        let path = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("bareline-isolation-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    fn cat(readable: Vec<PathBuf>, target: &Path) -> IsolationRequest {
        IsolationRequest {
            runtime: PathBuf::from("/bin/cat"),
            arguments: vec![target.into()],
            readable,
            socket: None,
            budget: ExecutionBudget::Interactive,
        }
    }

    #[test]
    fn the_host_reads_its_granted_file_and_nothing_else() {
        let root = scratch("grant");
        let (granted, secret) = (root.join("granted.txt"), root.join("secret.txt"));
        std::fs::write(&granted, b"granted").unwrap();
        std::fs::write(&secret, b"secret").unwrap();
        let isolation = MacIsolation::new();
        let request = cat(vec![granted.clone()], &granted);
        assert_eq!(
            isolation.probe(&request),
            IsolationSupport::Sandboxed,
            "{}",
            isolation.probe(&request).report()
        );
        let (mut child, _guard) = isolation.spawn_host(&request, &[]).unwrap();
        assert!(child.wait().unwrap().success());
        // The same program may not read a file it was not granted.
        let (mut child, _guard) = isolation.spawn_host(&cat(vec![granted.clone()], &secret), &[]).unwrap();
        assert!(!child.wait().unwrap().success());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_host_cannot_fork_and_its_group_can_be_killed() {
        let isolation = MacIsolation::new();
        // /bin/sh would need fork for a pipeline; under the profile it may not
        // even run, because only the runtime itself may be executed.
        let request = IsolationRequest {
            runtime: PathBuf::from("/bin/sleep"),
            arguments: vec!["30".into()],
            readable: Vec::new(),
            socket: None,
            budget: ExecutionBudget::Interactive,
        };
        let (mut child, mut guard) = isolation.spawn_host(&request, &[]).unwrap();
        assert_eq!(child.try_wait().unwrap(), None);
        guard.terminate().unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        // Terminating again after the reap must not signal a reused pid.
        guard.terminate().unwrap();
        let denied = IsolationRequest {
            runtime: PathBuf::from("/bin/sh"),
            arguments: vec!["-c".into(), "/usr/bin/true | /usr/bin/true".into()],
            readable: Vec::new(),
            socket: None,
            budget: ExecutionBudget::Interactive,
        };
        let (mut child, _guard) = isolation.spawn_host(&denied, &[]).unwrap();
        assert!(!child.wait().unwrap().success());
    }

    /// The shared launch's sandbox starts the verified runtime confined and
    /// names the mechanism; the child is waited for through the shared trait.
    #[test]
    fn the_host_sandbox_starts_the_runtime_confined() {
        let root = scratch("adapter");
        let component = root.join("c.wasm");
        std::fs::write(&component, b"component").unwrap();
        // Spelled through /var (a link), as the shared launch names files.
        let spelled = std::env::temp_dir().join(root.file_name().unwrap()).join("c.wasm");
        let executable = Path::new("/bin/cat");
        let file = std::fs::File::open(executable).unwrap();
        let spawn = HostSpawn {
            executable,
            executable_file: &file,
            component: &spelled,
            arguments: vec![spelled.clone().into()],
            budget: ExecutionBudget::Interactive,
            socket: None,
        };
        assert_eq!(MacHostSandbox::isolation(), Isolation::Enforced(MECHANISM.into()));
        let SpawnedHost {
            mut child,
            guard: _guard,
            isolation,
        } = MacHostSandbox::default().spawn_host(&spawn).unwrap();
        assert_eq!(isolation.to_string(), "isolation=sandbox_init");
        assert!(child.wait().unwrap().success(), "the granted component is readable");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unconfinable_requests_are_refused_before_anything_runs() {
        let isolation = MacIsolation::new();
        let mut request = cat(Vec::new(), Path::new("/etc/hosts"));
        request.readable = vec![PathBuf::from("relative/component.wasm")];
        assert!(matches!(
            isolation.probe(&request),
            IsolationSupport::Unsupported { .. }
        ));
        let error = isolation.spawn_host(&request, &[]).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("the extension was not started"), "{error}");
        assert!(isolation.probe(&request).report().starts_with("isolation=unsupported"));
    }
}
