// SPDX-License-Identifier: MPL-2.0
//! Explicit shell/tray integration. Document paths never enter a command shell.
use std::{os::windows::ffi::OsStrExt, path::Path};
use windows::{
    Win32::{
        Foundation::*,
        System::{
            Environment::SetCurrentDirectoryW,
            LibraryLoader::{LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, SetDefaultDllDirectories},
        },
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::PCWSTR,
};
fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn result(value: windows::Win32::Foundation::HINSTANCE) -> Result<(), String> {
    if value.0 as isize <= 32 {
        Err(format!("Windows shell error {}", value.0 as isize))
    } else {
        Ok(())
    }
}
pub fn reveal(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Absolute document path required".into());
    }
    // /select uses Explorer's direct argument parser; this is not cmd.exe.
    if path.as_os_str().encode_wide().any(|c| c == 0 || c == 34) {
        return Err("Invalid document path".into());
    }
    let mut arguments = std::ffi::OsString::from("/select,\"");
    arguments.push(path.as_os_str());
    arguments.push("\"");
    let args = wide(&arguments);
    // Never let an unqualified name resolve against the current directory (SEC-01).
    let explorer = wide(system_root()?.join("explorer.exe").as_os_str());
    unsafe {
        result(ShellExecuteW(
            None,
            PCWSTR::null(),
            PCWSTR(explorer.as_ptr()),
            PCWSTR(args.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        ))
    }
}
fn system_root() -> Result<std::path::PathBuf, String> {
    let system = std::env::var_os("SystemRoot").ok_or("Windows directory unavailable")?;
    let root = std::path::PathBuf::from(system);
    if !root.is_absolute() {
        return Err("Windows directory unavailable".into());
    }
    Ok(root)
}
/// Pins process-wide search paths so neither an executable nor a DLL can be picked
/// up from the directory Bareline happened to be started in (SEC-01). Call once at
/// startup, after relative command line paths have been resolved against the launch
/// directory.
pub fn harden_process_search_paths() -> Result<(), String> {
    let system32 = system_root()?.join("System32");
    let wide = wide(system32.as_os_str());
    unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS).map_err(|e| e.to_string())?;
        if !SetCurrentDirectoryW(PCWSTR(wide.as_ptr())).as_bool() {
            return Err("Cannot pin the working directory".into());
        }
    }
    Ok(())
}
pub fn open_terminal(directory: &Path) -> Result<(), String> {
    if !directory.is_absolute() {
        return Err("Absolute folder required".into());
    }
    // Fixed PowerShell executable/no command script; document folder is CreateProcess cwd.
    let system = std::env::var_os("SystemRoot").ok_or("Windows directory unavailable")?;
    let executable = std::path::PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let exe = wide(executable.as_os_str());
    let cwd = wide(directory.as_os_str());
    let args = wide(std::ffi::OsStr::new("-NoLogo -NoProfile"));
    unsafe {
        result(ShellExecuteW(
            None,
            PCWSTR::null(),
            PCWSTR(exe.as_ptr()),
            PCWSTR(args.as_ptr()),
            PCWSTR(cwd.as_ptr()),
            SW_SHOWNORMAL,
        ))
    }
}
pub fn add_recent(path: &Path, portable: bool) {
    if portable || !path.is_absolute() {
        return;
    }
    let name = wide(path.as_os_str());
    unsafe {
        SHAddToRecentDocs(SHARD_PATHW.0 as u32, Some(name.as_ptr().cast()));
    }
}
pub fn initialize_jump_list(portable: bool) -> Result<(), String> {
    if portable {
        return Ok(());
    }
    let id = wide(std::ffi::OsStr::new("Bareline.Editor"));
    unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(id.as_ptr())).map_err(|e| e.to_string()) }
}
const MESSAGE: u32 = WM_APP + 0x42;
#[derive(Clone, Copy, Debug)]
pub enum TrayAction {
    Restore,
    New,
    Open,
    Find,
    Exit,
}
/// The product mark embedded as resource 1 by `build.rs`. Falls back to the
/// system application icon when the resource is absent (unpackaged builds).
fn tray_icon_handle() -> HICON {
    unsafe {
        let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
        if let Ok(icon) = LoadIconW(Some(module.into()), PCWSTR(1 as *const u16)) {
            return icon;
        }
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    }
}
pub struct TrayIcon {
    data: NOTIFYICONDATAW,
}
impl TrayIcon {
    pub fn new(window: isize) -> Result<Self, String> {
        unsafe {
            let mut data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: HWND(window as *mut _),
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: MESSAGE,
                hIcon: tray_icon_handle(),
                ..Default::default()
            };
            for (out, value) in data.szTip.iter_mut().zip("Bareline".encode_utf16()) {
                *out = value;
            }
            if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                return Err("Cannot add notification icon".into());
            }
            Ok(Self { data })
        }
    }
}
impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data);
        }
    }
}
/// Called from the native message hook. Returns only product actions, no Windows types.
/// # Safety
/// `message` points to the live MSG provided by winit's Windows message hook.
pub unsafe fn tray_message(message: *const std::ffi::c_void) -> Option<TrayAction> {
    unsafe {
        let msg = &*message.cast::<MSG>();
        if msg.message != MESSAGE {
            return None;
        }
        match msg.lParam.0 as u32 {
            WM_LBUTTONDBLCLK => Some(TrayAction::Restore),
            WM_RBUTTONUP => {
                let menu = CreatePopupMenu().ok()?;
                for (id, label) in [(1, "Restore"), (2, "New"), (3, "Open…"), (4, "Find"), (5, "Exit")] {
                    let text = wide(std::ffi::OsStr::new(label));
                    let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(text.as_ptr()));
                }
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                let _ = SetForegroundWindow(msg.hwnd);
                let selected = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    point.x,
                    point.y,
                    Some(0),
                    msg.hwnd,
                    None,
                )
                .0;
                let _ = DestroyMenu(menu);
                match selected {
                    1 => Some(TrayAction::Restore),
                    2 => Some(TrayAction::New),
                    3 => Some(TrayAction::Open),
                    4 => Some(TrayAction::Find),
                    5 => Some(TrayAction::Exit),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
