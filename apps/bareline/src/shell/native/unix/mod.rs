// SPDX-License-Identifier: MPL-2.0
//! Linux and macOS backend of the shell seam.
//!
//! Real here: the portable software renderer presenting into the editor
//! window (`renderer`), the POSIX file system, path trust and trash (`files`),
//! the per-user data folders (`launch`), process identity, counters and the
//! monotonic clock (`process`), single-instance handoff (`instance`), file
//! watching (`watch`), the desktop's appearance and language (`system`), and
//! the dialogs, prompts, clipboard and (macOS) menu bar of the editor window
//! (`platform`). Linux dialogs and prompts answer later; `interaction` keeps
//! the shell's synchronous call sites working with them. Linux also runs the
//! extension host contained by Landlock (`extension_transport`).
//!
//! Everything these systems do not provide yet answers with the existing
//! `Unsupported` wording ("This system does not support ..."), a plain error
//! the shell already shows, or a logged no-op: nothing here panics.
//!
//! One file per concern; where Linux and macOS differ, the file splits by
//! `target_os`. This module re-exports every item under the names the Windows
//! seam (`../windows.rs`) uses.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("the Bareline shell supports Windows, Linux and macOS");

mod accessibility;
mod error;
mod files;
#[cfg(target_os = "linux")]
mod interaction;
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
pub use files::{FileSystem, PathTrust, RECYCLED, SessionPathTrust, recycle_entry, use_recovery_folder};
pub use launch::{installed_folders, prepare_installed_folders, user_home, valid_launch_path};
pub use platform::{
    AboutAction, CommandMessage, Platform, SaveChoice, SavePromptOutcome, answer_prompt, answer_prompt_text,
    command_window, in_app_prompt, interaction_replay, interaction_scope, interaction_settle, interaction_waiting,
    save_destination_prompt, save_destination_settled, save_prompt_failed,
};
pub use process::{ProcessLauncher, alive, handle_counters, monotonic_ns, private_bytes, resolve_program};
pub use renderer::{Renderer, create_renderer, installed_font_families};
pub use session::{SessionEndMonitor, SessionEndSignal, register_application_restart, session_end_signalled};
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
pub use session::{SessionEndHost, SessionEndMessage, session_end_monitor_for_tests};
pub use system::{
    appearance_activated, appearance_changed, high_contrast_enabled, high_contrast_highlight, spell_checker_factory,
    system_code_page, system_ui_language, window_theme,
};
pub use watch::WatchService;
pub use window::{RawWindow, install_message_hook, raw_window, set_event_notify, translate_event};
