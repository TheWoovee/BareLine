// SPDX-License-Identifier: MPL-2.0
//! Start suspended, contain in a kill-on-close job, then resume. Never permit breakaway children.
//! The extension host additionally runs under a restricted primary token (SEC-03): the token
//! keeps only the traverse-bypass privilege and carries restricting SIDs, so guest code cannot
//! read files that are granted only to the user's own account (everything under %USERPROFILE%
//! by default). The runtime, component and pipe it legitimately needs are handed a read/execute
//! ACE for the RESTRICTED code SID. If a machine cannot build the token the host falls back to
//! the previous job-limited, environment-cleared launch (logged once) rather than failing.
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
    sync::atomic::{AtomicBool, Ordering},
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
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
                TerminateJobObject,
            },
            StationsAndDesktops::{GetProcessWindowStation, GetThreadDesktop},
            Threading::*,
        },
    },
    core::{PCWSTR, PWSTR},
};

#[derive(Default)]
pub struct WindowsProcessLauncher;
/// Launcher for the extension host: same kill-on-close job plus a hard memory cap and
/// a single-process limit, so a pathological component cannot pressure system RAM or
/// fan out helper processes (SEC-03).
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
impl ProcessLauncher for SandboxedProcessLauncher {
    fn spawn(&self, command: &mut Command) -> io::Result<(Child, Box<dyn ProcessTreeGuard>)> {
        spawn_in_job(command, Some(*self))
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
        spawn_in_job(command, None)
    }
}
fn spawn_in_job(
    command: &mut Command,
    sandbox: Option<SandboxedProcessLauncher>,
) -> io::Result<(Child, Box<dyn ProcessTreeGuard>)> {
    {
        let job = OwnedHandle(unsafe { CreateJobObjectW(None, PCWSTR::null()).map_err(io::Error::other)? });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(sandbox) = sandbox {
            limits.BasicLimitInformation.LimitFlags |=
                JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            limits.ProcessMemoryLimit = sandbox.memory_limit_bytes;
            limits.JobMemoryLimit = sandbox.memory_limit_bytes;
            limits.BasicLimitInformation.ActiveProcessLimit = sandbox.active_process_limit;
        }
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
/// tracked by its own handle; `Std` is the job-limited fallback used when a restricted token
/// cannot be built.
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
    /// True when the process actually runs under the restricted token (SEC-03), false when
    /// the launcher fell back to the job-limited, environment-cleared launch.
    pub fn is_restricted(&self) -> bool {
        matches!(self, SandboxedChild::Native { .. })
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

static SANDBOX_FALLBACK_LOGGED: AtomicBool = AtomicBool::new(false);

impl SandboxedProcessLauncher {
    /// Spawn the extension host under a restricted primary token (SEC-03). `grant_targets`
    /// are the files/directories the host must still read (its runtime executable, the
    /// component, and — under test — the deliberately granted directory); each receives a
    /// read/execute ACE for the RESTRICTED code SID. On any failure to build the restricted
    /// launch the host is started with the existing job memory limits and cleared environment
    /// instead (logged once), so a locked-down machine still runs extensions.
    pub fn spawn_host(
        &self,
        command: &mut Command,
        grant_targets: &[&Path],
    ) -> io::Result<(SandboxedChild, Box<dyn ProcessTreeGuard>)> {
        match self.spawn_restricted(command, grant_targets) {
            Ok(result) => Ok(result),
            Err(error) => {
                if !SANDBOX_FALLBACK_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "Bareline: extension host restricted-token sandbox unavailable ({error}); \
                         launching with job memory limits and a cleared environment only (SEC-03)"
                    );
                }
                let (child, guard) = spawn_in_job(command, Some(*self))?;
                Ok((SandboxedChild::Std(child), guard))
            }
        }
    }
    fn spawn_restricted(
        &self,
        command: &mut Command,
        grant_targets: &[&Path],
    ) -> io::Result<(SandboxedChild, Box<dyn ProcessTreeGuard>)> {
        let token = restricted_token()?;
        grant_restricted_to_interactive_objects()?;
        for target in grant_targets {
            grant_restricted_read_execute(target)?;
        }
        let job = configure_host_job(*self)?;
        let (process, thread, pid) = create_restricted_process(command, token.0)?;
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
        Ok((SandboxedChild::Native { process, pid }, Box::new(JobGuard(job))))
    }
}

/// Create the kill-on-close job with the launcher's memory and process-count limits.
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
        // working; RESTRICTED matches the ACEs added to the grant targets and the pipe; the
        // per-session logon SID lets process init reach the window station and desktop (whose
        // DACLs grant it) so the host does not die with STATUS_DLL_INIT_FAILED. None of these
        // is ever named by an ACE on the user's private files under %USERPROFILE%.
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
        Ok(token)
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

/// Grant the RESTRICTED code SID access to the interactive window station and desktop that the
/// child inherits, so a restricted token can finish user32/gdi32 initialization instead of
/// dying with STATUS_DLL_INIT_FAILED. The grant is on the current session's objects and is
/// discarded at logoff.
fn grant_restricted_to_interactive_objects() -> io::Result<()> {
    unsafe {
        let restricted = well_known_sid(WinRestrictedCodeSid)?;
        let winsta = GetProcessWindowStation().map_err(io::Error::other)?;
        let desktop = GetThreadDesktop(GetCurrentThreadId()).map_err(io::Error::other)?;
        grant_object_all_access(HANDLE(winsta.0), &restricted)?;
        grant_object_all_access(HANDLE(desktop.0), &restricted)?;
        Ok(())
    }
}

/// Merge a full-access ACE for `sid` onto a window-object handle (window station or desktop).
fn grant_object_all_access(handle: HANDLE, sid: &[u8]) -> io::Result<()> {
    const GENERIC_ALL: u32 = 0x1000_0000;
    unsafe {
        let mut old_dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let status = GetSecurityInfo(
            handle,
            SE_WINDOW_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut old_dacl),
            None,
            Some(&mut descriptor),
        );
        if status.0 != 0 {
            return Err(io::Error::from_raw_os_error(status.0 as i32));
        }
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
        let status = SetEntriesInAclW(Some(std::slice::from_ref(&entry)), Some(old_dacl), &mut new_dacl);
        let result = if status.0 != 0 {
            Err(io::Error::from_raw_os_error(status.0 as i32))
        } else {
            let status = SetSecurityInfo(
                handle,
                SE_WINDOW_OBJECT,
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

/// Grant the restricted host write access to one pre-created qualification receipt.
/// The parent retains and validates the receipt; no directory write grant is added.
#[doc(hidden)]
pub fn sandbox_grant_restricted_qualification_write(path: &Path) -> io::Result<()> {
    const FILE_APPEND_DATA: u32 = 0x0000_0004;
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "qualification receipt must be a regular file",
        ));
    }
    grant_restricted_file_access(path, FILE_APPEND_DATA, ACE_FLAGS(0))
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

/// Create the host suspended under `token` with `CreateProcessAsUserW`, reconstructing the
/// command line, environment block and working directory from the standard-library `Command`.
fn create_restricted_process(command: &Command, token: HANDLE) -> io::Result<(StdOwnedHandle, OwnedHandle, u32)> {
    let program: Vec<u16> = command.get_program().encode_wide().chain(Some(0)).collect();
    let mut command_line = build_command_line(command);
    command_line.push(0);
    let environment = build_environment(command);
    let current_dir: Option<Vec<u16>> = command
        .get_current_dir()
        .map(|dir| dir.as_os_str().encode_wide().chain(Some(0)).collect());
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    let environment_ptr = environment
        .as_ref()
        .map(|block| block.as_ptr() as *const core::ffi::c_void);
    let flags = CREATE_SUSPENDED
        | CREATE_NO_WINDOW
        | if environment_ptr.is_some() {
            CREATE_UNICODE_ENVIRONMENT
        } else {
            PROCESS_CREATION_FLAGS(0)
        };
    unsafe {
        CreateProcessAsUserW(
            Some(token),
            PCWSTR(program.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            flags,
            environment_ptr,
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
/// variables, matching the caller's `env_clear` + allowlist. Empty means inherit.
fn build_environment(command: &Command) -> Option<Vec<u16>> {
    let entries: Vec<(&OsStr, &OsStr)> = command
        .get_envs()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .collect();
    if entries.is_empty() {
        return None;
    }
    let mut block: Vec<u16> = Vec::new();
    for (key, value) in entries {
        block.extend(key.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    Some(block)
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
    let preview = arguments
        .iter()
        .map(|argument| argument.to_string_lossy().chars().take(2048).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    let message = format!(
        "Run this {} command?\n\n{}\n\nArguments (one per line):\n{}",
        if shell { "shell" } else { "direct executable" },
        program.display(),
        preview
    );
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
