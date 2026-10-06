// SPDX-License-Identifier: MPL-2.0
//! The editor window's identity and the native message hook.
use super::{platform::CommandMessage, shell_integration::TrayAction};
use bareline_app::task::Wake;
use std::{cell::Cell, rc::Rc, sync::mpsc::Sender};
use winit::{event_loop::EventLoopBuilder, window::Window};

/// An opaque token for the editor window. It holds no native handle; it only
/// keys per-window state the shell keeps.
pub type RawWindow = isize;
pub fn raw_window(window: &Window) -> std::result::Result<RawWindow, String> {
    Ok(u64::from(window.id()) as isize)
}

/// Native window messages do not exist here: menu commands, tray actions and
/// packet input never arrive, so the senders are dropped.
pub fn install_message_hook(
    _builder: &mut EventLoopBuilder<Wake>,
    _input_window: Rc<Cell<(isize, u64)>>,
    _commands: Sender<CommandMessage>,
    _tray: Sender<TrayAction>,
) {
}
