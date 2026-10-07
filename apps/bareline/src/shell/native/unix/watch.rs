// SPDX-License-Identifier: MPL-2.0
//! File watching with the Windows service's contract (the same `WatchEvent`
//! vocabulary, debounce, bounded queues and per-directory recovery): inotify on
//! Linux, kqueue on macOS. The shell drops a replaced service on its setup
//! thread; both stop and join their worker there.
#[cfg(target_os = "linux")]
pub use bareline_platform_linux::LinuxWatchService as WatchService;
#[cfg(target_os = "macos")]
pub use bareline_platform_macos::MacWatchService as WatchService;
