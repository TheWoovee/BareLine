// SPDX-License-Identifier: MPL-2.0
//! The shell's single seam to the operating system (ADR-B).
//!
//! Everything else under `shell/` is platform-neutral and names native services
//! only through this module: the renderer and platform services and their
//! constructors, the local file system and path trust providers, file watching,
//! clipboard, dialogs and menus (through `Platform`), single-instance handoff,
//! the extension host transport, updates, printing, shell integration,
//! accessibility, session end, the profile folders and startup failure
//! reporting. Exactly one backend is compiled: `windows.rs` re-exports the
//! Windows adapter unchanged, and `unix/` (Linux and macOS, one file per
//! concern) wires the portable renderer and the POSIX, Linux and macOS
//! services. `prompt` is the one data type both sides share: a question the
//! shell draws itself where the system has no modal dialog it can wait on.
#[cfg(windows)]
#[path = "windows.rs"]
mod backend;
#[cfg(not(windows))]
#[path = "unix/mod.rs"]
mod backend;
mod prompt;
pub use backend::*;
#[cfg_attr(
    not(test),
    allow(
        unused_imports,
        reason = "the shell reads buttons through the view; its tests build them"
    )
)]
pub use prompt::PromptButtonView;
pub use prompt::{PromptLevel, PromptView};
