// SPDX-License-Identifier: MPL-2.0
//! Dialogs, prompts, menus and the clipboard of the editor window (`Platform`),
//! with the answer types the Windows dialogs return. Linux and macOS differ
//! here more than anywhere else: Linux asks through the desktop portal and the
//! shell's own prompts, which answer later; macOS has a native menu bar and
//! AppKit panels and alerts, which answer at once.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;
