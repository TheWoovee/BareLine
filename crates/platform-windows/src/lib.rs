// SPDX-License-Identifier: MPL-2.0
#[cfg(windows)]
mod remote_read;
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub use native::*;
#[cfg(windows)]
mod renderer;
#[cfg(windows)]
pub use renderer::WindowsRenderer;
#[cfg(windows)]
mod files;
#[cfg(windows)]
pub use files::WindowsFileSystem;
#[cfg(windows)]
mod clipboard;
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
pub use process::WindowsProcessLauncher;

#[cfg(windows)]
pub mod extension_transport;
#[cfg(windows)]
pub mod instance;
#[cfg(windows)]
pub mod update;

#[cfg(windows)]
mod workspace_files;
#[cfg(windows)]
pub use process::confirm_external_command;

#[cfg(windows)]
pub use accessibility::high_contrast_enabled;

#[cfg(windows)]
pub mod printing;

#[cfg(windows)]
pub use workspace_files::{WorkspaceDeleteUndo, retain_deleted_entry, restore_deleted_entry};

#[cfg(windows)]
mod rename;

#[cfg(windows)]
pub mod shell_integration;
