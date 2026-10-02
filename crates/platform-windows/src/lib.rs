// SPDX-License-Identifier: MPL-2.0
#![cfg(windows)]

#[cfg(windows)]
mod dark_mode;
mod menu_bar;
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub mod unicode_input;

/// The Windows ANSI code page selected by the OS (including UTF-8 mode).
#[cfg(windows)]
pub fn system_code_page() -> u32 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetACP() -> u32;
    }
    // SAFETY: GetACP has no arguments or caller-owned memory.
    unsafe { GetACP() }
}

/// The Windows display language as a BCP 47 name such as `de-DE`: the default
/// Bareline locale (BIZ-30). `None` when Windows cannot name it.
#[cfg(windows)]
pub fn system_ui_language() -> Option<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
        fn LCIDToLocaleName(locale: u32, name: *mut u16, capacity: i32, flags: u32) -> i32;
    }
    // LOCALE_NAME_MAX_LENGTH, including the terminator.
    let mut name = [0u16; 85];
    // SAFETY: GetUserDefaultUILanguage takes no arguments; LCIDToLocaleName writes
    // at most `capacity` UTF-16 units into `name`, which outlives the call.
    let written = unsafe { LCIDToLocaleName(u32::from(GetUserDefaultUILanguage()), name.as_mut_ptr(), 85, 0) };
    // The count includes the terminator; zero means failure.
    let length = usize::try_from(written).ok()?.checked_sub(1)?;
    String::from_utf16(name.get(..length)?)
        .ok()
        .filter(|name| !name.is_empty())
}
#[cfg(windows)]
mod remote_read;
#[cfg(windows)]
pub use native::*;
#[cfg(windows)]
mod renderer;
#[cfg(windows)]
pub use renderer::{InstalledFontFamily, WindowsRenderer, installed_font_families};
#[cfg(windows)]
mod capability;
#[cfg(windows)]
mod files;
#[cfg(windows)]
pub use files::WindowsFileSystem;
#[cfg(windows)]
mod clipboard;
mod owned_cache;
#[cfg(windows)]
mod path_trust;
#[cfg(windows)]
pub use path_trust::{WindowsPathTrustProvider, WindowsSessionPathTrustProvider};

#[cfg(windows)]
mod watch;
#[cfg(windows)]
pub use watch::WindowsWatchService;

#[cfg(windows)]
pub mod accessibility;
#[cfg(windows)]
pub use accessibility::WindowsAccessibility;
#[cfg(windows)]
mod process;
#[cfg(windows)]
pub use process::resolve_program;
#[cfg(windows)]
pub use process::{
    HostExit, SandboxTokenState, SandboxUnavailable, SandboxedChild, SandboxedProcessLauncher, WindowsProcessLauncher,
    sandbox_grant_restricted_qualification_write, sandbox_lock_to_current_user,
};

#[cfg(windows)]
pub mod cli;
#[cfg(windows)]
pub mod extension_transport;
#[cfg(windows)]
pub mod instance;
#[cfg(windows)]
pub mod update;

#[cfg(windows)]
mod workspace_files;
#[cfg(windows)]
#[cfg(windows)]
pub use accessibility::{high_contrast_enabled, high_contrast_highlight};

#[cfg(windows)]
pub mod printing;

#[cfg(windows)]
pub use workspace_files::recycle_entry;

#[cfg(windows)]
mod rename;

#[cfg(windows)]
mod session_end;
#[cfg(windows)]
pub use session_end::{
    SessionEndHost, SessionEndMessage, SessionEndMonitor, SessionEndSignal, register_application_restart,
};

#[cfg(windows)]
pub mod shell_integration;

#[cfg(windows)]
pub mod spelling;
#[cfg(windows)]
pub use spelling::spell_checker_factory;
