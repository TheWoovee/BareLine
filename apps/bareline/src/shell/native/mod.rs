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
//! concern) wires the portable renderer and POSIX services and reports what
//! this system does not support yet.
#[cfg(windows)]
#[path = "windows.rs"]
mod backend;
#[cfg(not(windows))]
#[path = "unix/mod.rs"]
mod backend;
pub use backend::*;
