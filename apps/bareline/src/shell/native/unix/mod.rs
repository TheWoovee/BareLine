// SPDX-License-Identifier: MPL-2.0
//! Linux and macOS backend of the shell seam.
//!
//! Real today: the portable software renderer presenting into the editor
//! window (`renderer`), the POSIX file system, path trust and trash (`files`),
//! the per-user data folders (`launch`), and process identity, counters and the
//! monotonic clock (`process`). Everything the adapters for these systems do
//! not provide yet answers with the existing `Unsupported` wording ("This
//! system does not support ..."), a plain error the shell already shows, or a
//! logged no-op: nothing here panics.
//!
//! One file per concern, so the services that replace the stand-ins (dialogs
//! and menus in `platform`, `watch`, `instance`, `session`, `printing`, ...)
//! each change only their own file. This module re-exports every item under
//! the names the Windows seam (`../windows.rs`) uses.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("the Bareline shell supports Windows, Linux and macOS");

mod accessibility;
mod error;
mod files;
mod launch;
mod platform;
mod process;
mod renderer;
mod session;
mod system;
mod watch;
mod window;

pub mod cli;
pub mod extension_transport;
pub mod instance;
pub mod printing;
pub mod shell_integration;
pub mod update;

#[cfg(test)]
mod tests;

pub use accessibility::Accessibility;
pub use error::Result;
pub use files::{FileSystem, PathTrust, SessionPathTrust, recycle_entry};
pub use launch::{installed_folders, user_home, valid_launch_path};
pub use platform::{AboutAction, CommandMessage, Platform, SaveChoice, SavePromptOutcome, command_window};
pub use process::{ProcessLauncher, alive, handle_counters, monotonic_ns, private_bytes, resolve_program};
pub use renderer::{Renderer, create_renderer, installed_font_families};
pub use session::{SessionEndMonitor, SessionEndSignal, register_application_restart};
// Types the shell reaches only through values and methods (as on Windows), or
// only in its tests.
#[allow(
    unused_imports,
    reason = "the names complete the seam; the shell uses the types through values"
)]
pub use self::{
    error::{Error, ErrorCode},
    launch::InstalledFolders,
    renderer::InstalledFontFamily,
};
#[cfg(test)]
pub use session::{SessionEndHost, SessionEndMessage};
pub use system::{
    high_contrast_enabled, high_contrast_highlight, spell_checker_factory, system_code_page, system_ui_language,
};
pub use watch::WatchService;
pub use window::{RawWindow, install_message_hook, raw_window};
