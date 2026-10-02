// SPDX-License-Identifier: MPL-2.0
//! Visible command-line and startup-failure output (APP-01, APP-02). The shipping
//! executable uses the GUI subsystem, so it has no console of its own: output goes
//! to a redirected handle, else to the console of the invoking shell, else to a
//! message box. A launch never ends without saying why.
use std::io::Write;
use windows::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, WriteConsoleW,
};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW};
use windows::core::PCWSTR;

/// Prints `text` for `--help`, `--version` or an argument error (`error`).
pub fn report(text: &str, error: bool) {
    if !write_console(text, error) {
        message_box(text, if error { MB_ICONERROR } else { MB_ICONINFORMATION });
    }
}

/// Explains a startup failure that leaves no window to show it in.
pub fn show_startup_error(message: &str) {
    message_box(message, MB_ICONERROR);
}

/// Whether standard error goes to a file or pipe that the launching process reads.
pub fn stderr_redirected() -> bool {
    redirected(STD_ERROR_HANDLE)
}

/// Whether standard input is a file or pipe. A console is never read: it would
/// wait for typing that nobody knows is expected (APP-09).
pub fn stdin_redirected() -> bool {
    redirected(STD_INPUT_HANDLE)
}

fn redirected(id: STD_HANDLE) -> bool {
    // SAFETY: GetStdHandle and GetFileType only inspect this process's handle table.
    unsafe { GetStdHandle(id) }
        .is_ok_and(|handle| matches!(unsafe { GetFileType(handle) }, FILE_TYPE_DISK | FILE_TYPE_PIPE))
}

fn write_console(text: &str, error: bool) -> bool {
    if redirected(if error { STD_ERROR_HANDLE } else { STD_OUTPUT_HANDLE }) {
        let mut out: Box<dyn Write> = if error {
            Box::new(std::io::stderr())
        } else {
            Box::new(std::io::stdout())
        };
        return writeln!(out, "{text}").and_then(|()| out.flush()).is_ok();
    }
    // Fails harmlessly when a console is already attached (console-subsystem
    // builds) or when there is no parent console (Explorer, shortcuts).
    // SAFETY: plain Win32 call without caller-owned memory.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    let Ok(console) = std::fs::OpenOptions::new().read(true).write(true).open("CONOUT$") else {
        return false;
    };
    use std::os::windows::io::AsRawHandle;
    // The shell has already printed its prompt; start on a fresh line.
    let wide: Vec<u16> = format!("\r\n{text}\r\n").encode_utf16().collect();
    // SAFETY: `console` owns a live console output handle for the duration of the call.
    unsafe {
        WriteConsoleW(
            windows::Win32::Foundation::HANDLE(console.as_raw_handle()),
            &wide,
            None,
            None,
        )
    }
    .is_ok()
}

fn message_box(text: &str, icon: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) {
    let wide: Vec<u16> = text.encode_utf16().filter(|unit| *unit != 0).chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the modal call; no owner window exists yet.
    unsafe {
        MessageBoxW(None, PCWSTR(wide.as_ptr()), windows::core::w!("Bareline"), MB_OK | icon);
    }
}
