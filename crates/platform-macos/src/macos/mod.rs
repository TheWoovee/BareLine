// SPDX-License-Identifier: MPL-2.0
//! The macOS-only services: AppKit menus, pasteboard, panels, alerts and
//! appearance, the kqueue watcher, sandboxed process launch and Unix-socket
//! peer identity.
//!
//! Retina: winit reports the window's `backingScaleFactor` (2.0 on a Retina
//! display) as its scale factor, and the software renderer draws pixels at
//! DIPs times that scale, so text is drawn at the display's full resolution
//! without anything from this crate. When a window moves between displays of
//! different scales winit sends `ScaleFactorChanged`; the shell resizes the
//! renderer as it does on Windows.
pub(crate) mod appearance;
pub(crate) mod clipboard;
pub(crate) mod dialogs;
pub(crate) mod isolation;
pub(crate) mod menu;
pub mod peer;
pub(crate) mod watch;

use objc2::rc::Retained;
use objc2_foundation::NSString;

fn ns_string(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}
