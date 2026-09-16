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
#[cfg(windows)]
mod remote_read;
#[cfg(windows)]
pub use native::*;
#[cfg(windows)]
mod renderer;
#[cfg(windows)]
pub use renderer::{InstalledFontFamily, WindowsRenderer, installed_font_families};
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
pub use process::{
    HostExit, SandboxedChild, SandboxedProcessLauncher, WindowsProcessLauncher,
    sandbox_grant_restricted_qualification_write, sandbox_lock_to_current_user,
};

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
pub use accessibility::high_contrast_enabled;

#[cfg(windows)]
pub mod printing;

#[cfg(windows)]
pub use workspace_files::{WorkspaceDeleteUndo, restore_deleted_entry, retain_deleted_entry};

#[cfg(windows)]
mod rename;

#[cfg(windows)]
pub mod shell_integration;
