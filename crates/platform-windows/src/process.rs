// SPDX-License-Identifier: MPL-2.0
//! Start suspended, contain in a kill-on-close job, then resume. Never permit breakaway children.
//! The extension host additionally runs under a restricted primary token (SEC-03): the token
//! keeps only the traverse-bypass privilege and carries restricting SIDs, so guest code cannot
//! read files that are granted only to the user's own account (everything under %USERPROFILE%
//! by default). The runtime, component and pipe it legitimately needs are handed a read/execute
//! ACE for the RESTRICTED code SID. The token is also lowered to Low integrity, the host runs on
//! a private desktop instead of the user's interactive one, and its job carries every
//! `JOB_OBJECT_UILIMIT_*` restriction (SEC-06). If any part of that launch cannot be
//! established the host is not started and the caller receives [`SandboxUnavailable`]; there
//! is no weaker fallback (SEC-05). Network access is not blocked: that needs an AppContainer
//! or WFP filter and is deferred until it can be qualified against the pipe transport.
use bareline_macros::process::{ProcessLauncher, ProcessTreeGuard};
use std::{
    ffi::OsStr,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle as StdOwnedHandle},
        process::CommandExt,
    },
    path::Path,
    process::{Child, Command},
    sync::atomic::{AtomicU64, Ordering},
};
use windows::{
    Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
                JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
                JOB_OBJECT_UILIMIT, JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS,
                JOB_OBJECT_UILIMIT_EXITWINDOWS, JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES,
                JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
                JOB_OBJECT_UILIMIT_WRITECLIPBOARD, JOBOBJECT_BASIC_UI_RESTRICTIONS,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject,
            },
            StationsAndDesktops::{
                CloseDesktop, CreateDesktopExW, DESKTOP_CONTROL_FLAGS, GetProcessWindowStation,
                GetUserObjectInformationW, HDESK, UOI_NAME,
            },
            Threading::*,
        },
    },
    core::{PCWSTR, PWSTR},
};

#[derive(Default)]
pub struct WindowsProcessLauncher;
/// Launcher for the extension host: same kill-on-close job plus a hard memory cap and
/// a single-process limit, so a pathological component cannot pressure system RAM or
/// fan out helper processes (SEC-03). It launches only through [`Self::spawn_host`] and
/// deliberately does not implement `ProcessLauncher`, so it cannot start a process
/// with the job limits alone (SEC-05).
#[derive(Clone, Copy, Debug)]
pub struct SandboxedProcessLauncher {
    pub memory_limit_bytes: usize,
    pub active_process_limit: u32,
}
impl Default for SandboxedProcessLauncher {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 512 * 1024 * 1024,
            active_process_limit: 1,
        }
    }
}
struct OwnedHandle(HANDLE);
// HANDLE refers to an owned kernel object and all operations require &self or exclusive ownership.
unsafe impl Send for OwnedHandle {}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct JobGuard(OwnedHandle);
impl ProcessTreeGuard for JobGuard {
    fn terminate(&mut self) -> io::Result<()> {
        unsafe { TerminateJobObject(self.0.0, 1).map_err(io::Error::other) }
    }
}
impl ProcessLauncher for WindowsProcessLauncher {
    fn spawn(&self, command: &mut Command) -> io::Result<(Child, Box<dyn ProcessTreeGuard>)> {
        spawn_in_job(command)
    }
    /// Shell mode's prepared `cmd.exe` line, appended without std's argv quoting (SEC-10).
    fn set_raw_command_line(&self, command: &mut Command, line: &OsStr) -> io::Result<()> {
        command.raw_arg(line);
        Ok(())
    }
}
/// The ordinary kill-on-close job launch. The extension host never comes through here:
/// it has only the restricted-token launch in `spawn_host`, so no memory-limit-only
/// launch can pass for the sandbox (SEC-05).
fn spawn_in_job(command: &mut Command) -> io::Result<(Child, Box<dyn ProcessTreeGuard>)> {
    {
        let job = OwnedHandle(unsafe { CreateJobObjectW(None, PCWSTR::null()).map_err(io::Error::other)? });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )
            .map_err(io::Error::other)?;
        }
        command.creation_flags(CREATE_SUSPENDED.0 | CREATE_NO_WINDOW.0);
        let mut child = command.spawn()?;
        let configure = (|| -> io::Result<()> {
            unsafe {
                AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle())).map_err(io::Error::other)?;
            }
            let snapshot =
                OwnedHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).map_err(io::Error::other)? });
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            unsafe {
                Thread32First(snapshot.0, &mut entry).map_err(io::Error::other)?;
            }
            loop {
                if entry.th32OwnerProcessID == child.id() {
                    let thread = OwnedHandle(unsafe {
                        OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID).map_err(io::Error::other)?
                    });
                    if unsafe { ResumeThread(thread.0) } == u32::MAX {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(());
                }
                if unsafe { Thread32Next(snapshot.0, &mut entry) }.is_err() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "Suspended process main thread not found",
                    ));
                }
            }
        })();
        if let Err(error) = configure {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((child, Box::new(JobGuard(job))))
    }
}

/// A spawned, job-contained host process. `Native` is created from a restricted primary
/// token via `CreateProcessAsUserW`, which the standard library cannot represent, so it is
/// tracked by its own handle. `spawn_host` only ever returns `Native`; `Std` wraps a child from
/// the ordinary job launcher for callers that share the host plumbing.
pub enum SandboxedChild {
    Std(Child),
    Native { process: StdOwnedHandle, pid: u32 },
}
impl SandboxedChild {
    pub fn id(&self) -> u32 {
        match self {
            SandboxedChild::Std(child) => child.id(),
            SandboxedChild::Native { pid, .. } => *pid,
        }
    }
    /// True when the process was created from the restricted token (SEC-03).
    pub fn is_restricted(&self) -> bool {
        matches!(self, SandboxedChild::Native { .. })
    }
    /// Read the containment the running (or exited, still referenced) process's primary token
    /// really carries, independently of how the launcher classified it (SEC-05).
    pub fn token_state(&self) -> io::Result<SandboxTokenState> {
        let process = match self {
            SandboxedChild::Std(child) => HANDLE(child.as_raw_handle()),
            SandboxedChild::Native { process, .. } => HANDLE(process.as_raw_handle()),
        };
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(process, TOKEN_QUERY, &mut token).map_err(io::Error::other)?;
            let token = OwnedHandle(token);
            token_state(token.0)
        }
    }
    pub fn try_wait(&mut self) -> io::Result<Option<HostExit>> {
        match self {
            SandboxedChild::Std(child) => Ok(child.try_wait()?.map(HostExit::from_status)),
            SandboxedChild::Native { process, .. } => {
                let handle = HANDLE(process.as_raw_handle());
                match unsafe { WaitForSingleObject(handle, 0) } {
                    WAIT_TIMEOUT => Ok(None),
                    WAIT_OBJECT_0 => Ok(Some(native_exit(handle)?)),
                    _ => Err(io::Error::last_os_error()),
                }
            }
        }
    }
    pub fn wait(&mut self) -> io::Result<HostExit> {
        match self {
            SandboxedChild::Std(child) => Ok(HostExit::from_status(child.wait()?)),
            SandboxedChild::Native { process, .. } => {
                let handle = HANDLE(process.as_raw_handle());
                if unsafe { WaitForSingleObject(handle, u32::MAX) } != WAIT_OBJECT_0 {
                    return Err(io::Error::last_os_error());
                }
                native_exit(handle)
            }
        }
    }
}
/// Exit outcome that spans both the standard-library child and the restricted native process.
pub struct HostExit {
    success: bool,
    code: i64,
}
impl HostExit {
    pub fn success(&self) -> bool {
        self.success
    }
    fn from_status(status: std::process::ExitStatus) -> Self {
        Self {
            success: status.success(),
            code: status.code().map(i64::from).unwrap_or(-1),
        }
    }
}
impl std::fmt::Display for HostExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit code: {}", self.code)
    }
}
fn native_exit(process: HANDLE) -> io::Result<HostExit> {
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(process, &mut code).map_err(io::Error::other)? };
    Ok(HostExit {
        success: code == 0,
        code: code as i64,
    })
}

/// What a process's primary token actually carries: restricting SIDs, and the RID of its
/// mandatory integrity label. Lets tests prove the host is contained (SEC-05).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SandboxTokenState {
    pub restricted: bool,
    pub integrity_rid: u32,
}
impl SandboxTokenState {
    /// `SECURITY_MANDATORY_LOW_RID`.
    pub const LOW_INTEGRITY_RID: u32 = 0x1000;
}

/// The restricted extension-host launch could not be established, so the host was not started
/// (SEC-05). There is deliberately no weaker fallback; the editor reports this to the user.
#[derive(Debug)]
pub struct SandboxUnavailable(io::Error);
impl std::fmt::Display for SandboxUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Extension sandbox unavailable on this system; the extension was not started ({})",
            self.0
        )
    }
}
impl std::error::Error for SandboxUnavailable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl SandboxedProcessLauncher {
    /// Spawn the extension host under a restricted, Low-integrity primary token on a private
    /// desktop (SEC-03, SEC-06). `grant_targets` are the files/directories the host must still
    /// read (its runtime executable, the component, and — under test — the deliberately
    /// granted directory); each receives a read/execute ACE for the RESTRICTED code SID. If any
    /// step fails nothing is started and the error (kind `PermissionDenied`) wraps
    /// [`SandboxUnavailable`] (SEC-05).
    pub fn spawn_host(
        &self,
        command: &mut Command,
        grant_targets: &[&Path],
    ) -> io::Result<(SandboxedChild, Box<dyn ProcessTreeGuard>)> {
        self.spawn_restricted(command, grant_targets)
            .map_err(|error| io::Error::new(io::ErrorKind::PermissionDenied, SandboxUnavailable(error)))
    }
    fn spawn_restricted(
        &self,
        command: &mut Command,
        grant_targets: &[&Path],
    ) -> io::Result<(SandboxedChild, Box<dyn ProcessTreeGuard>)> {
        let token = restricted_token()?;
        let desktop = HostDesktop::create()?;
        for target in grant_targets {
            grant_restricted_read_execute(target)?;
        }
        let job = configure_host_job(*self)?;
        let (process, thread, pid) = create_restricted_process(command, token.0, &desktop.path)?;
        let handle = HANDLE(process.as_raw_handle());
        let configure = (|| -> io::Result<()> {
            unsafe {
                AssignProcessToJobObject(job.0, handle).map_err(io::Error::other)?;
                if ResumeThread(thread.0) == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        })();
        if let Err(error) = configure {
            unsafe {
                let _ = TerminateProcess(handle, 1);
            }
            return Err(error);
        }
        Ok((
            SandboxedChild::Native { process, pid },
            Box::new(HostContainment { job, _desktop: desktop }),
        ))
    }
}

/// The host's kill-on-close job plus the private desktop it runs on. The desktop stays open for
/// as long as the host is contained; fields drop in order, so the job kills the host first.
struct HostContainment {
    job: OwnedHandle,
    _desktop: HostDesktop,
}
impl ProcessTreeGuard for HostContainment {
    fn terminate(&mut self) -> io::Result<()> {
        unsafe { TerminateJobObject(self.job.0, 1).map_err(io::Error::other) }
    }
}

/// Every UI restriction a job can carry (SEC-06): no desktop creation or switching, display
/// settings, logoff/shutdown, global atoms, USER handles owned outside the job, clipboard
/// reads or writes, or system parameters.
fn host_ui_limits() -> JOB_OBJECT_UILIMIT {
    JOB_OBJECT_UILIMIT_DESKTOP
        | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
        | JOB_OBJECT_UILIMIT_EXITWINDOWS
        | JOB_OBJECT_UILIMIT_GLOBALATOMS
        | JOB_OBJECT_UILIMIT_HANDLES
        | JOB_OBJECT_UILIMIT_READCLIPBOARD
        | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
        | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
}

/// Create the kill-on-close job with the launcher's memory and process-count limits and every
/// UI restriction.
fn configure_host_job(sandbox: SandboxedProcessLauncher) -> io::Result<OwnedHandle> {
    unsafe {
        let job = OwnedHandle(CreateJobObjectW(None, PCWSTR::null()).map_err(io::Error::other)?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.ProcessMemoryLimit = sandbox.memory_limit_bytes;
        limits.JobMemoryLimit = sandbox.memory_limit_bytes;
        limits.BasicLimitInformation.ActiveProcessLimit = sandbox.active_process_limit;
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        )
        .map_err(io::Error::other)?;
        let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: host_ui_limits(),
        };
        SetInformationJobObject(
            job.0,
            JobObjectBasicUIRestrictions,
            &ui as *const _ as *const _,
            std::mem::size_of_val(&ui) as u32,
        )
        .map_err(io::Error::other)?;
        Ok(job)
    }
}

/// Build a restricted, primary token derived from this process: all privileges except the
/// benign traverse-bypass are removed, and the restricting SIDs deliberately exclude the
/// user's own account so the second access check fails on user-private files.
fn restricted_token() -> io::Result<OwnedHandle> {
    unsafe {
        let mut process_token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            &mut process_token,
        )
        .map_err(io::Error::other)?;
        let process_token = OwnedHandle(process_token);
        // Everyone + Users keep system DLL loading and other Everyone/Users-readable files
        // working; RESTRICTED matches the ACEs added to the grant targets, the pipe and the
        // host's private desktop; the per-session logon SID lets process init reach the
        // interactive window station (whose DACL grants it) without editing that DACL, so the
        // host does not die with STATUS_DLL_INIT_FAILED. None of these is ever named by an ACE
        // on the user's private files under %USERPROFILE%.
        let everyone = well_known_sid(WinWorldSid)?;
        let users = well_known_sid(WinBuiltinUsersSid)?;
        let restricted = well_known_sid(WinRestrictedCodeSid)?;
        let logon = logon_sid(process_token.0)?;
        let restricting = [
            SID_AND_ATTRIBUTES {
                Sid: PSID(everyone.as_ptr() as *mut _),
                Attributes: 0,
            },
            SID_AND_ATTRIBUTES {
                Sid: PSID(users.as_ptr() as *mut _),
                Attributes: 0,
            },
            SID_AND_ATTRIBUTES {
                Sid: PSID(restricted.as_ptr() as *mut _),
                Attributes: 0,
            },
            SID_AND_ATTRIBUTES {
                Sid: PSID(logon.as_ptr() as *mut _),
                Attributes: 0,
            },
        ];
        let mut token = HANDLE::default();
        CreateRestrictedToken(
            process_token.0,
            DISABLE_MAX_PRIVILEGE,
            None,
            None,
            Some(&restricting),
            &mut token,
        )
        .map_err(io::Error::other)?;
        let token = OwnedHandle(token);
        // A restricted token's default DACL grants only the user, so objects the process
        // creates during initialization (heap sections, CRT sync objects) cannot be reopened
        // under the restricting SIDs and the process dies with STATUS_DLL_INIT_FAILED. Add the
        // RESTRICTED code SID to the default DACL so the process can access its own objects.
        set_restricted_default_dacl(token.0, &restricted)?;
        set_low_integrity(token.0)?;
        Ok(token)
    }
}

/// Lower the token to Low mandatory integrity (SEC-06). The host can then write only objects
/// labelled Low (its pipe, its private desktop, a qualification receipt) and cannot write the
/// user's Medium-labelled files, even where a DACL grants a restricting SID. Reading is
/// unaffected, and lowering needs no privilege, so the token stays a child of this one.
fn set_low_integrity(token: HANDLE) -> io::Result<()> {
    const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;
    unsafe {
        let low = well_known_sid(WinLowLabelSid)?;
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: PSID(low.as_ptr() as *mut _),
                Attributes: SE_GROUP_INTEGRITY,
            },
        };
        SetTokenInformation(
            token,
            TokenIntegrityLevel,
            &label as *const _ as *const core::ffi::c_void,
            (std::mem::size_of::<TOKEN_MANDATORY_LABEL>() + low.len()) as u32,
        )
        .map_err(io::Error::other)
    }
}

/// Read whether `token` carries restricting SIDs and the RID of its integrity label.
fn token_state(token: HANDLE) -> io::Result<SandboxTokenState> {
    unsafe {
        let restricted = IsTokenRestricted(token).is_ok();
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed);
        if needed == 0 || needed > 1024 {
            return Err(io::Error::other("integrity label size"));
        }
        let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .map_err(io::Error::other)?;
        let label = &*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        if count == 0 {
            return Err(io::Error::other("integrity label SID"));
        }
        Ok(SandboxTokenState {
            restricted,
            integrity_rid: *GetSidSubAuthority(label.Label.Sid, u32::from(count) - 1),
        })
    }
}

fn well_known_sid(kind: WELL_KNOWN_SID_TYPE) -> io::Result<Vec<u8>> {
    unsafe {
        let mut size = 0u32;
        let _ = CreateWellKnownSid(kind, None, None, &mut size);
        if size == 0 || size > 1024 {
            return Err(io::Error::other("well-known SID size"));
        }
        let mut buffer = vec![0u8; size as usize];
        CreateWellKnownSid(kind, None, Some(PSID(buffer.as_mut_ptr().cast())), &mut size).map_err(io::Error::other)?;
        Ok(buffer)
    }
}

/// Add a full-access ACE for `sid` to the token's default DACL, which is applied to objects the
/// process creates, so a restricted process can reopen its own objects.
fn set_restricted_default_dacl(token: HANDLE, sid: &[u8]) -> io::Result<()> {
    const GENERIC_ALL: u32 = 0x1000_0000;
    unsafe {
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenDefaultDacl, None, 0, &mut needed);
        if needed == 0 || needed > 65536 {
            return Err(io::Error::other("default DACL size"));
        }
        let mut buffer = vec![0u8; needed as usize];
        GetTokenInformation(
            token,
            TokenDefaultDacl,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .map_err(io::Error::other)?;
        let current = &*buffer.as_ptr().cast::<TOKEN_DEFAULT_DACL>();
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_ALL,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: ACE_FLAGS(0),
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: PWSTR(sid.as_ptr() as *mut _),
            },
        };
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let status = SetEntriesInAclW(
            Some(std::slice::from_ref(&entry)),
            Some(current.DefaultDacl as *const _),
            &mut new_dacl,
        );
        let result = if status.0 != 0 {
            Err(io::Error::from_raw_os_error(status.0 as i32))
        } else {
            let default = TOKEN_DEFAULT_DACL { DefaultDacl: new_dacl };
            SetTokenInformation(
                token,
                TokenDefaultDacl,
                &default as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<TOKEN_DEFAULT_DACL>() as u32,
            )
            .map_err(io::Error::other)
        };
        if !new_dacl.is_null() {
            let _ = LocalFree(Some(HLOCAL(new_dacl.cast())));
        }
        result
    }
}

/// Copy the per-session logon SID out of a token's groups.
fn logon_sid(token: HANDLE) -> io::Result<Vec<u8>> {
    const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;
    unsafe {
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenGroups, None, 0, &mut needed);
        if needed == 0 || needed > 1024 * 1024 {
            return Err(io::Error::other("token groups size"));
        }
        let mut buffer = vec![0u8; needed as usize];
        GetTokenInformation(
            token,
            TokenGroups,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .map_err(io::Error::other)?;
        let groups = &*buffer.as_ptr().cast::<TOKEN_GROUPS>();
        let entries = std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize);
        for entry in entries {
            if entry.Attributes & SE_GROUP_LOGON_ID != 0 {
                let len = GetLengthSid(entry.Sid) as usize;
                if len == 0 || len > 1024 {
                    return Err(io::Error::other("logon SID length"));
                }
                return Ok(std::slice::from_raw_parts(entry.Sid.0 as *const u8, len).to_vec());
            }
        }
        Err(io::Error::other("logon SID not present in token"))
    }
}

static HOST_DESKTOP_SERIAL: AtomicU64 = AtomicU64::new(0);

/// A private desktop for one host launch (SEC-06). The host starts on it rather than on the
/// user's interactive desktop, so no ACE is ever added to the shared window station or desktop
/// DACL, and nothing the host could do on a desktop reaches the user's windows. Its DACL grants
/// the user and SYSTEM full access and the RESTRICTED code SID everything except switching,
/// re-permissioning or deleting it; its Low label lets the Low-integrity host use it.
struct HostDesktop {
    handle: HDESK,
    /// NUL-terminated `WindowStation\Desktop` for `STARTUPINFOW::lpDesktop`.
    path: Vec<u16>,
}
// HDESK refers to an owned desktop handle that is only closed on drop.
unsafe impl Send for HostDesktop {}
impl Drop for HostDesktop {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseDesktop(self.handle);
        }
    }
}
impl HostDesktop {
    fn create() -> io::Result<Self> {
        const GENERIC_ALL: u32 = 0x1000_0000;
        // The host creates no windows; keep its desktop heap small (in KiB).
        const HEAP_KIB: u32 = 1024;
        unsafe {
            // The process window-station handle is borrowed and must not be closed.
            let station = GetProcessWindowStation().map_err(io::Error::other)?;
            let station = user_object_name(HANDLE(station.0))?;
            let name = format!(
                "bareline-exthost-{}-{}",
                std::process::id(),
                HOST_DESKTOP_SERIAL.fetch_add(1, Ordering::Relaxed)
            );
            // 0x200ff: DESKTOP_READOBJECTS through DESKTOP_WRITEOBJECTS plus READ_CONTROL; no
            // DESKTOP_SWITCHDESKTOP, WRITE_DAC, WRITE_OWNER or DELETE.
            let sddl: Vec<u16> = format!(
                "D:P(A;;GA;;;{})(A;;GA;;;SY)(A;;0x200ff;;;RC)",
                current_user_sid_string()?
            )
            .encode_utf16()
            .chain(Some(0))
            .collect();
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .map_err(io::Error::other)?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            let wide_name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let created = CreateDesktopExW(
                PCWSTR(wide_name.as_ptr()),
                PCWSTR::null(),
                None,
                DESKTOP_CONTROL_FLAGS(0),
                GENERIC_ALL,
                Some(&attributes),
                HEAP_KIB,
                None,
            );
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            let mut path = station;
            path.push(u16::from(b'\\'));
            path.extend(name.encode_utf16());
            path.push(0);
            let desktop = Self {
                handle: created.map_err(io::Error::other)?,
                path,
            };
            let handle = HANDLE(desktop.handle.0);
            with_low_integrity_sacl(|sacl| {
                SetSecurityInfo(
                    handle,
                    SE_WINDOW_OBJECT,
                    LABEL_SECURITY_INFORMATION,
                    None,
                    None,
                    None,
                    Some(sacl),
                )
            })?;
            Ok(desktop)
        }
    }
}

/// The name of a window station or desktop, without its terminating NUL.
fn user_object_name(object: HANDLE) -> io::Result<Vec<u16>> {
    unsafe {
        let mut needed = 0u32;
        let _ = GetUserObjectInformationW(object, UOI_NAME, None, 0, Some(&mut needed));
        if needed == 0 || needed > 4096 {
            return Err(io::Error::other("window station name size"));
        }
        let mut name = vec![0u16; (needed as usize).div_ceil(2)];
        GetUserObjectInformationW(
            object,
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            needed,
            Some(&mut needed),
        )
        .map_err(io::Error::other)?;
        let len = name.iter().position(|&unit| unit == 0).unwrap_or(name.len());
        name.truncate(len);
        Ok(name)
    }
}

/// Run `apply` with a SACL holding a Low mandatory label with no-write-up, the label a
/// Low-integrity host needs on anything it must write.
fn with_low_integrity_sacl(apply: impl FnOnce(*const ACL) -> WIN32_ERROR) -> io::Result<()> {
    unsafe {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            windows::core::w!("S:(ML;;NW;;;LW)"),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .map_err(io::Error::other)?;
        let mut present = windows::core::BOOL(0);
        let mut sacl: *mut ACL = std::ptr::null_mut();
        let mut defaulted = windows::core::BOOL(0);
        let result = GetSecurityDescriptorSacl(descriptor, &mut present, &mut sacl, &mut defaulted)
            .map_err(io::Error::other)
            .and_then(|()| {
                if !present.as_bool() || sacl.is_null() {
                    return Err(io::Error::other("low integrity label"));
                }
                let status = apply(sacl as *const ACL);
                if status.0 != 0 {
                    Err(io::Error::from_raw_os_error(status.0 as i32))
                } else {
                    Ok(())
                }
            });
        if !descriptor.0.is_null() {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        result
    }
}

/// Merge a read/execute ACE for the RESTRICTED code SID onto a file or directory so the
/// restricted host can still read it without granting anything to other user accounts.
fn grant_restricted_read_execute(path: &Path) -> io::Result<()> {
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_EXECUTE: u32 = 0x2000_0000;
    grant_restricted_file_access(
        path,
        GENERIC_READ | GENERIC_EXECUTE,
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
    )
}

/// Grant the restricted host write access to one pre-created qualification receipt, and label
/// it Low so the Low-integrity host may append to it (SEC-06). The parent retains and
/// validates the receipt; no directory write grant or label is added.
#[doc(hidden)]
pub fn sandbox_grant_restricted_qualification_write(path: &Path) -> io::Result<()> {
    const FILE_APPEND_DATA: u32 = 0x0000_0004;
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "qualification receipt must be a regular file",
        ));
    }
    grant_restricted_file_access(path, FILE_APPEND_DATA, ACE_FLAGS(0))?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    with_low_integrity_sacl(|sacl| unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            LABEL_SECURITY_INFORMATION,
            None,
            None,
            None,
            Some(sacl),
        )
    })
}

fn grant_restricted_file_access(path: &Path, access: u32, inheritance: ACE_FLAGS) -> io::Result<()> {
    unsafe {
        let restricted = well_known_sid(WinRestrictedCodeSid)?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut old_dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let status = GetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut old_dacl),
            None,
            &mut descriptor,
        );
        if status.0 != 0 {
            return Err(io::Error::from_raw_os_error(status.0 as i32));
        }
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: access,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: inheritance,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: PWSTR(restricted.as_ptr() as *mut _),
            },
        };
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let status = SetEntriesInAclW(Some(std::slice::from_ref(&entry)), Some(old_dacl), &mut new_dacl);
        let result = if status.0 != 0 {
            Err(io::Error::from_raw_os_error(status.0 as i32))
        } else {
            let status = SetNamedSecurityInfoW(
                PCWSTR(wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(new_dacl),
                None,
            );
            if status.0 != 0 {
                Err(io::Error::from_raw_os_error(status.0 as i32))
            } else {
                Ok(())
            }
        };
        if !new_dacl.is_null() {
            let _ = LocalFree(Some(HLOCAL(new_dacl.cast())));
        }
        if !descriptor.0.is_null() {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        result
    }
}

/// Lock a file to the current user only: a protected DACL that grants no restricting SID, so a
/// restricted token's second access check is denied while the user can still read it. Test
/// support for the SEC-03 isolation check.
#[doc(hidden)]
pub fn sandbox_lock_to_current_user(path: &Path) -> io::Result<()> {
    unsafe {
        let sid = current_user_sid_string()?;
        let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .map_err(io::Error::other)?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut present = windows::core::BOOL(0);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut defaulted = windows::core::BOOL(0);
        let result = (|| {
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted).map_err(io::Error::other)?;
            let status = SetNamedSecurityInfoW(
                PCWSTR(wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl),
                None,
            );
            if status.0 != 0 {
                return Err(io::Error::from_raw_os_error(status.0 as i32));
            }
            Ok(())
        })();
        if !descriptor.0.is_null() {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        result
    }
}

fn current_user_sid_string() -> io::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(io::Error::other)?;
        let token = OwnedHandle(token);
        let mut needed = 0u32;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut needed);
        if needed == 0 || needed > 65536 {
            return Err(io::Error::other("token size"));
        }
        let mut buffer = vec![0u8; needed as usize];
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .map_err(io::Error::other)?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid).map_err(io::Error::other)?;
        let text = sid.to_string().map_err(io::Error::other);
        let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
        text
    }
}

/// Create the host suspended under `token` on `desktop` (a NUL-terminated
/// `WindowStation\Desktop`) with `CreateProcessAsUserW`, reconstructing the command line,
/// environment block and working directory from the standard-library `Command`.
fn create_restricted_process(
    command: &Command,
    token: HANDLE,
    desktop: &[u16],
) -> io::Result<(StdOwnedHandle, OwnedHandle, u32)> {
    let program: Vec<u16> = command.get_program().encode_wide().chain(Some(0)).collect();
    let mut command_line = build_command_line(command);
    command_line.push(0);
    let environment = build_environment(command);
    let current_dir: Option<Vec<u16>> = command
        .get_current_dir()
        .map(|dir| dir.as_os_str().encode_wide().chain(Some(0)).collect());
    let mut desktop = desktop.to_vec();
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    // Always an explicit Unicode block, never NULL (which would inherit this process's).
    let flags = CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT;
    unsafe {
        CreateProcessAsUserW(
            Some(token),
            PCWSTR(program.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            flags,
            Some(environment.as_ptr() as *const core::ffi::c_void),
            current_dir
                .as_ref()
                .map(|dir| PCWSTR(dir.as_ptr()))
                .unwrap_or(PCWSTR::null()),
            &startup,
            &mut info,
        )
        .map_err(io::Error::other)?;
        Ok((
            StdOwnedHandle::from_raw_handle(info.hProcess.0),
            OwnedHandle(info.hThread),
            info.dwProcessId,
        ))
    }
}

fn build_command_line(command: &Command) -> Vec<u16> {
    let mut line: Vec<u16> = Vec::new();
    append_quoted(command.get_program(), &mut line);
    for arg in command.get_args() {
        line.push(u16::from(b' '));
        append_quoted(arg, &mut line);
    }
    line
}

/// Quote one argument per the rules `CommandLineToArgvW` (and the host's `args_os`) parse.
fn append_quoted(arg: &OsStr, out: &mut Vec<u16>) {
    const SPACE: u16 = b' ' as u16;
    const TAB: u16 = b'\t' as u16;
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    let units: Vec<u16> = arg.encode_wide().collect();
    let needs_quotes = units.is_empty() || units.iter().any(|&c| c == SPACE || c == TAB || c == QUOTE);
    if !needs_quotes {
        out.extend_from_slice(&units);
        return;
    }
    out.push(QUOTE);
    let mut backslashes = 0usize;
    for &c in &units {
        if c == BACKSLASH {
            backslashes += 1;
        } else if c == QUOTE {
            for _ in 0..(backslashes * 2 + 1) {
                out.push(BACKSLASH);
            }
            out.push(QUOTE);
            backslashes = 0;
        } else {
            for _ in 0..backslashes {
                out.push(BACKSLASH);
            }
            backslashes = 0;
            out.push(c);
        }
    }
    for _ in 0..(backslashes * 2) {
        out.push(BACKSLASH);
    }
    out.push(QUOTE);
}

/// Build a double-null-terminated UTF-16 environment block from the command's explicitly set
/// variables, matching the caller's `env_clear` + allowlist. The restricted host never inherits
/// the parent environment: with no variables set the block is empty (two NULs) (SEC-21).
fn build_environment(command: &Command) -> Vec<u16> {
    let mut block: Vec<u16> = Vec::new();
    for (key, value) in command
        .get_envs()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
    {
        block.extend(key.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        process::Stdio,
        time::{Duration, Instant},
    };
    const LITERAL: &str = "a folder \"quoted\" & ; $(literal) | >";
    #[test]
    fn sandbox_failure_refuses_to_start_the_host() {
        // A grant target that does not exist makes the restricted launch impossible; the host
        // must not then be started with a weaker token (SEC-05).
        let missing = std::env::temp_dir().join(format!("bareline-sec05-missing-grant-{}", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "process::tests::descendant_fixture", "--ignored"]);
        let error = match SandboxedProcessLauncher::default().spawn_host(&mut command, &[missing.as_path()]) {
            Ok((mut child, mut guard)) => {
                let _ = guard.terminate();
                let _ = child.wait();
                panic!("host started although its restricted launch could not be established");
            }
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            error
                .get_ref()
                .is_some_and(|inner| inner.downcast_ref::<SandboxUnavailable>().is_some())
        );
    }
    #[test]
    fn host_token_is_restricted_and_low_integrity() {
        let token = restricted_token().unwrap();
        let state = token_state(token.0).unwrap();
        assert!(state.restricted);
        assert_eq!(state.integrity_rid, SandboxTokenState::LOW_INTEGRITY_RID);
    }
    #[test]
    fn host_job_carries_every_ui_limit() {
        let job = configure_host_job(SandboxedProcessLauncher::default()).unwrap();
        let mut ui = JOBOBJECT_BASIC_UI_RESTRICTIONS::default();
        unsafe {
            windows::Win32::System::JobObjects::QueryInformationJobObject(
                Some(job.0),
                JobObjectBasicUIRestrictions,
                &mut ui as *mut _ as *mut _,
                std::mem::size_of_val(&ui) as u32,
                None,
            )
            .unwrap();
        }
        assert_eq!(ui.UIRestrictionsClass, host_ui_limits());
        assert_eq!(ui.UIRestrictionsClass.0, 0xff);
    }
    #[test]
    fn host_desktop_is_private_to_this_launch() {
        let first = HostDesktop::create().unwrap();
        let second = HostDesktop::create().unwrap();
        assert_ne!(first.path, second.path);
        let text = String::from_utf16(&first.path[..first.path.len() - 1]).unwrap();
        let (_, desktop) = text.split_once('\\').unwrap();
        assert!(desktop.starts_with("bareline-exthost-"), "{text}");
    }
    #[test]
    fn restricted_launch_environment_never_inherits_the_parent() {
        // Without env_clear and without variables, an empty block (not NULL = inherit) (SEC-21).
        assert_eq!(build_environment(&Command::new("host.exe")), vec![0, 0]);
        let mut cleared = Command::new("host.exe");
        cleared.env_clear();
        assert_eq!(build_environment(&cleared), vec![0, 0]);
        cleared.env("A", "b");
        assert_eq!(
            build_environment(&cleared),
            "A=b\0\0".encode_utf16().collect::<Vec<_>>()
        );
    }
    #[test]
    #[ignore = "Controlled subprocess fixture, launched only by job_contains_and_terminates_test_process"]
    fn child_fixture() {
        assert!(std::env::args().any(|arg| arg == LITERAL));
        println!("{LITERAL}");
        let mut descendant = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "process::tests::descendant_fixture", "--ignored"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .unwrap();
        println!("DESCENDANT={}", descendant.id());
        std::io::stdout().flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = descendant.kill();
        let _ = descendant.wait();
    }
    #[test]
    #[ignore = "Controlled descendant fixture, launched only by child_fixture"]
    fn descendant_fixture() {
        std::thread::sleep(Duration::from_secs(10));
    }
    #[test]
    fn job_contains_and_terminates_test_process() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "process::tests::child_fixture",
                "--ignored",
                "--nocapture",
                "--skip",
                LITERAL,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (mut child, mut guard) = WindowsProcessLauncher.spawn(&mut command).unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut text = String::new();
        let descendant_id = loop {
            let mut line = String::new();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "fixture exited before descendant was ready"
            );
            text.push_str(&line);
            if let Some(id) = line.trim().strip_prefix("DESCENDANT=") {
                break id.parse::<u32>().unwrap();
            }
        };
        let descendant = OwnedHandle(unsafe {
            windows::Win32::System::Threading::OpenProcess(
                windows::Win32::System::Threading::PROCESS_SYNCHRONIZE,
                false,
                descendant_id,
            )
            .unwrap()
        });
        guard.terminate().unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
        reader.read_to_string(&mut text).unwrap();
        assert!(text.contains(LITERAL));
        assert_eq!(
            unsafe { windows::Win32::System::Threading::WaitForSingleObject(descendant.0, 5000) },
            windows::Win32::Foundation::WAIT_OBJECT_0
        );
    }
    #[test]
    fn missing_program_fails_without_running_an_uncontained_child() {
        let mut command = Command::new(std::env::temp_dir().join("bareline-missing-process-fixture-71455.exe"));
        assert!(WindowsProcessLauncher.spawn(&mut command).is_err());
    }
}

/// Called on the UI thread before granting a configured command execution rights.
pub(crate) fn confirm_external_command(
    owner: windows::Win32::Foundation::HWND,
    program: &std::path::Path,
    arguments: &[std::ffi::OsString],
    shell: bool,
) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO, MessageBoxW};
    // Shows the resolved absolute program and marks any truncation visibly (SEC-10).
    let message = bareline_macros::process::consent_text(program, arguments, shell);
    let wide: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(wide.as_ptr()),
            windows::core::w!("Bareline — Run External Command"),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING,
        ) == IDYES
    }
}

/// Resolves a Run (F5) program the way Notepad++ users expect: an absolute path is
/// used as typed and a bare name is searched on `PATH` with `PATHEXT`. The current
/// directory, relative paths and relative `PATH` entries are never searched (SEC-04),
/// and only launchable types (.com, .exe, .bat, .cmd) are returned. A bare name checks
/// up to five files in each `PATH` folder, and a slow or disconnected network folder
/// can block each check for seconds, so the Run prompt resolves bare names on a worker
/// thread (an absolute path returns without touching the file system).
pub fn resolve_program(name: &str) -> Result<std::path::PathBuf, String> {
    resolve_program_in(
        name,
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("PATHEXT").as_deref(),
        &|candidate: &Path| candidate.is_file(),
    )
}
fn resolve_program_in(
    name: &str,
    path: Option<&OsStr>,
    pathext: Option<&OsStr>,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<std::path::PathBuf, String> {
    const LAUNCHABLE: [&str; 4] = [".com", ".exe", ".bat", ".cmd"];
    if Path::new(name).is_absolute() {
        return Ok(name.into());
    }
    if name.is_empty() || name.contains(['\\', '/', ':']) || name.trim_matches('.').is_empty() {
        return Err(format!(
            "{name} is not an absolute path or a program name; relative paths are not searched"
        ));
    }
    let extensions: Vec<String> = pathext
        .and_then(OsStr::to_str)
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .map(|extension| extension.trim().to_ascii_lowercase())
        .filter(|extension| LAUNCHABLE.contains(&extension.as_str()))
        .collect();
    let typed = Path::new(name)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extensions.contains(&format!(".{}", extension.to_ascii_lowercase())));
    for directory in std::env::split_paths(path.unwrap_or_default()) {
        if !directory.is_absolute() {
            continue;
        }
        let candidates = typed.then(|| directory.join(name)).into_iter().chain(
            extensions
                .iter()
                .map(|extension| directory.join(format!("{name}{extension}"))),
        );
        for candidate in candidates {
            if exists(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err(format!("{name} was not found on PATH; enter its absolute path"))
}

#[cfg(test)]
mod external_command_tests {
    use super::*;

    #[test]
    fn run_programs_resolve_on_absolute_path_entries_with_pathext() {
        let files = [
            r"C:\Windows\System32\where.exe",
            r"C:\tools\build.cmd",
            r"C:\tools\notes.txt",
            r".\where.exe",
            r"C:\first\python.exe",
            r"C:\second\python.exe",
        ];
        let exists = |candidate: &Path| files.iter().any(|file| candidate == Path::new(file));
        let path = OsStr::new(r".;relative;C:\first;C:\Windows\System32\;C:\tools;C:\second");
        let resolve =
            |name: &str| resolve_program_in(name, Some(path), Some(OsStr::new(".COM;.EXE;.BAT;.CMD;.VBS")), &exists);
        assert_eq!(resolve("where").unwrap(), Path::new(r"C:\Windows\System32\where.exe"));
        assert_eq!(
            resolve("where.exe").unwrap(),
            Path::new(r"C:\Windows\System32\where.exe")
        );
        assert_eq!(resolve("build").unwrap(), Path::new(r"C:\tools\build.cmd"));
        // PATH order wins; relative entries (including ".") are skipped.
        assert_eq!(resolve("python").unwrap(), Path::new(r"C:\first\python.exe"));
        // Absolute paths are used as typed; relative paths are never searched.
        assert_eq!(resolve(r"D:\x\tool.exe").unwrap(), Path::new(r"D:\x\tool.exe"));
        for refused in [
            r".\where.exe",
            r"sub\where.exe",
            "C:where.exe",
            "..",
            "",
            "notes.txt",
            "notes",
            "missing",
        ] {
            assert!(resolve(refused).is_err(), "{refused:?}");
        }
    }
}
