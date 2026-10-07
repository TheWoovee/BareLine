// SPDX-License-Identifier: MPL-2.0
//! Windows backend of the shell seam: the Windows adapter, re-exported under the
//! neutral names the shell uses, plus the few Win32 calls the shell made itself.
//! Nothing here changes behaviour; each item is the code the shell called before.
use bareline_app::task::Wake;
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc::Sender,
};
use winit::{
    event_loop::EventLoopBuilder,
    platform::windows::EventLoopBuilderExtWindows,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

pub use bareline_platform_windows::{
    AboutAction, CommandMessage, SaveChoice, SavePromptOutcome, SessionEndMonitor, SessionEndSignal,
    WindowsAccessibility as Accessibility, WindowsFileSystem as FileSystem, WindowsPathTrustProvider as PathTrust,
    WindowsPlatform as Platform, WindowsProcessLauncher as ProcessLauncher, WindowsRenderer as Renderer,
    WindowsSessionPathTrustProvider as SessionPathTrust, WindowsWatchService as WatchService, cli,
    high_contrast_enabled, high_contrast_highlight, installed_font_families, instance, monotonic_ns, private_bytes,
    recycle_entry, register_application_restart, resolve_program, shell_integration, spell_checker_factory,
    system_code_page, system_ui_language, update,
};
#[cfg(test)]
pub use bareline_platform_windows::{SessionEndHost, SessionEndMessage};

pub mod printing {
    pub use bareline_platform_windows::printing::{WindowsPrintJob as PrintJob, choose_printer};
}

pub mod extension_transport {
    pub use bareline_platform_windows::extension_transport::*;
    /// The Extensions page names the host's confinement only where it can be
    /// missing; on Windows the job object and AppContainer always apply.
    pub fn isolation() -> Option<String> {
        None
    }
}

/// What the workspace says after `recycle_entry`.
pub const RECYCLED: &str = "Moved to the Recycle Bin";

/// Every Windows save keeps its transaction beside the file, so no recovery
/// folder is needed (see the Linux seam's `files::use_recovery_folder`).
pub fn use_recovery_folder(_recovery: Option<&Path>) {}

/// Every Windows prompt and dialog is modal: the call returns the answer, so
/// nothing is ever deferred, replayed or drawn by the shell (see the Linux
/// seam's `interaction` for the systems where that is needed).
pub struct InteractionScope;
pub fn interaction_scope<R: 'static>(_platform: Option<&Platform>, _owner: impl FnOnce() -> R) -> InteractionScope {
    InteractionScope
}
pub fn interaction_waiting(_platform: Option<&Platform>) -> bool {
    false
}
pub fn interaction_replay<R: 'static>(_platform: Option<&Platform>) -> Option<R> {
    None
}
pub fn interaction_settle(_platform: Option<&Platform>) {}
/// Nothing of the Windows save dialog carries over to the destination check.
pub fn save_destination_settled(_platform: Option<&Platform>) {}
pub fn in_app_prompt(_platform: Option<&Platform>) -> Option<super::PromptView> {
    None
}
pub fn answer_prompt(_platform: Option<&Platform>, _id: i32) {}

/// The Windows services wake the loop through their own window messages.
pub fn set_event_notify(_notify: std::sync::Arc<dyn Fn() + Send + Sync>) {}
/// winit's events already carry the keymap's modifiers on Windows.
pub fn translate_event(event: winit::event::WindowEvent) -> winit::event::WindowEvent {
    event
}
/// The `AppsUseLightTheme` preference, as winit reads it.
pub fn window_theme(window: &Window) -> Option<winit::window::Theme> {
    window.theme()
}
/// Windows reports appearance changes as `ThemeChanged` window events.
pub fn appearance_changed() -> bool {
    false
}
/// Nothing to ask again on activation: `ThemeChanged` covers Windows.
pub fn appearance_activated() {}

/// The native handle of the editor window: its HWND.
pub type RawWindow = isize;

/// The editor window's HWND, which the platform, accessibility and session-end
/// adapters attach to.
pub fn raw_window(window: &Window) -> Result<RawWindow, String> {
    let handle = window.window_handle().map_err(|error| error.to_string())?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("Expected Win32 window".into());
    };
    Ok(handle.hwnd.get())
}

/// Direct2D/DirectWrite, created for the platform's window.
pub fn create_renderer(
    platform: &Platform,
    _window: Option<&Window>,
    software: bool,
) -> windows::core::Result<Renderer> {
    platform.renderer(software)
}

/// Routes the window messages winit does not forward: packet Unicode input,
/// native menu commands and notification-area icon actions.
pub fn install_message_hook(
    builder: &mut EventLoopBuilder<Wake>,
    input_window: Rc<Cell<(isize, u64)>>,
    commands: Sender<CommandMessage>,
    tray: Sender<shell_integration::TrayAction>,
) {
    let mut unicode_input = bareline_platform_windows::unicode_input::UnicodePacketInput::default();
    builder.with_msg_hook(move |message| {
        // SAFETY: winit supplies a live MSG on this window's event-loop thread.
        if unsafe { unicode_input.process_message(message, input_window.get()) } {
            return true;
        }
        // SAFETY: winit supplies a valid MSG pointer during the hook invocation.
        if let Some(command) = unsafe { Platform::command_message(message) } {
            let _ = commands.send(command);
        }
        // SAFETY: as above, the MSG pointer is valid for the hook invocation.
        if let Some(action) = unsafe { shell_integration::tray_message(message) } {
            let _ = tray.send(action);
        }
        false
    });
}

/// The window a native menu command was sent to, for the command trace.
pub fn command_window(message: &CommandMessage) -> RawWindow {
    message.hwnd
}

/// Where an installed Bareline keeps its profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstalledFolders {
    /// `%APPDATA%\Bareline`, the roaming profile of earlier releases, which the
    /// profile migration moves into `local`.
    pub roaming: Option<PathBuf>,
    /// `%LOCALAPPDATA%\Bareline`: settings, session, recovery journals,
    /// extensions and macros.
    pub local: Option<PathBuf>,
    /// Diagnostics live in the profile folder on Windows.
    pub logs: Option<PathBuf>,
}
/// The installed profile folders, from the environment.
pub fn installed_folders() -> InstalledFolders {
    InstalledFolders {
        roaming: std::env::var_os("APPDATA").map(|root| PathBuf::from(root).join("Bareline")),
        local: std::env::var_os("LOCALAPPDATA").map(|root| PathBuf::from(root).join("Bareline")),
        logs: None,
    }
}

/// The user's profile folder (`%USERPROFILE%`).
pub fn user_home() -> Option<std::ffi::OsString> {
    std::env::var_os("USERPROFILE")
}

/// The same limits the instance handoff enforces for every forwarded path.
pub fn valid_launch_path(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    path.is_absolute() && !units.is_empty() && !units.contains(&0) && units.len() <= 32767
}

/// Process handle, GDI and USER object counts for `--diag handles`.
pub fn handle_counters() -> (u32, u32, u32) {
    use windows::Win32::System::Threading::{
        GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources, GetProcessHandleCount,
    };
    // SAFETY: pseudo handle for the current process; counters are plain outputs.
    unsafe {
        let process = GetCurrentProcess();
        let mut handles = 0u32;
        let _ = GetProcessHandleCount(process, &mut handles);
        (
            handles,
            GetGuiResources(process, GR_GDIOBJECTS),
            GetGuiResources(process, GR_USEROBJECTS),
        )
    }
}

/// Whether a recovery journal's owning process still runs.
pub mod alive {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_INVALID_PARAMETER: u32 = 87;
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, id: u32) -> isize;
        fn GetExitCodeProcess(process: isize, code: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetLastError() -> u32;
        fn GetProcessTimes(process: isize, creation: *mut u64, exit: *mut u64, kernel: *mut u64, user: *mut u64)
        -> i32;
    }
    /// FILETIME (100 ns ticks since 1601) of the Unix epoch.
    const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;
    /// Start time, in nanoseconds since the Unix epoch, of the running process with
    /// this id. `None` when it cannot be opened or queried, or has exited.
    pub fn started(id: u32) -> Option<u128> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, id);
            if process == 0 {
                return None;
            }
            let mut code = 0u32;
            let active = GetExitCodeProcess(process, &mut code) != 0 && code == STILL_ACTIVE;
            // A FILETIME is two little-endian u32 halves; a u64 has the same layout
            // and at least its alignment.
            let (mut creation, mut exit, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
            let timed = active && GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
            CloseHandle(process);
            timed.then(|| u128::from(creation.saturating_sub(UNIX_EPOCH_FILETIME)) * 100)
        }
    }
    /// `OpenProcess` reports a process id that names no process as an invalid
    /// parameter. Any other failure (for example access denied for an elevated or
    /// another user's process) leaves the owner possibly alive.
    pub fn alive_after_open_failure(error: u32) -> bool {
        error != ERROR_INVALID_PARAMETER
    }
    /// True when a process with this id is still running. Unknown ids are reported as
    /// running so a doubtful case never deletes someone else's recovery data.
    pub fn running(id: u32) -> bool {
        if id == std::process::id() {
            return true;
        }
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, id);
            if process == 0 {
                return alive_after_open_failure(GetLastError());
            }
            let mut code = 0u32;
            let queried = GetExitCodeProcess(process, &mut code) != 0;
            CloseHandle(process);
            !queried || code == STILL_ACTIVE
        }
    }
}
