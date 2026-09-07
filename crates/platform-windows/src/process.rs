// SPDX-License-Identifier: MPL-2.0
//! Start suspended, contain in a kill-on-close job, then resume. Never permit breakaway children.
use bareline_macros::process::{ProcessLauncher, ProcessTreeGuard};
use std::{
    io,
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command},
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject,
            },
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
            },
        },
    },
    core::PCWSTR,
};

#[derive(Default)]
pub struct WindowsProcessLauncher;
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
        let job = OwnedHandle(unsafe {
            CreateJobObjectW(None, PCWSTR::null()).map_err(io::Error::other)?
        });
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
                AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle()))
                    .map_err(io::Error::other)?;
            }
            let snapshot = OwnedHandle(unsafe {
                CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).map_err(io::Error::other)?
            });
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
                        OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)
                            .map_err(io::Error::other)?
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
        let mut command =
            Command::new(std::env::temp_dir().join("bareline-missing-process-fixture-71455.exe"));
        assert!(WindowsProcessLauncher.spawn(&mut command).is_err());
    }
}

/// Called on the UI thread before granting a configured command execution rights.
pub fn confirm_external_command(
    program: &std::path::Path,
    arguments: &[std::ffi::OsString],
    shell: bool,
) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO, MessageBoxW,
    };
    let preview = arguments
        .iter()
        .map(|argument| {
            argument
                .to_string_lossy()
                .chars()
                .take(2048)
                .collect::<String>()
        })
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
            None,
            PCWSTR(wide.as_ptr()),
            windows::core::w!("Bareline — Run External Command"),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING,
        ) == IDYES
    }
}
