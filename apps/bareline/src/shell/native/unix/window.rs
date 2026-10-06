// SPDX-License-Identifier: MPL-2.0
//! The editor window's identity, the native message hook and the event-loop
//! wake the native services call when an answer or command arrives.
use super::{platform::CommandMessage, shell_integration::TrayAction};
use bareline_app::task::Wake;
use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError, mpsc::Sender},
};
use winit::{event::WindowEvent, event_loop::EventLoopBuilder, window::Window};

/// An opaque token for the editor window. It holds no native handle; it only
/// keys per-window state the shell keeps.
pub type RawWindow = isize;
pub fn raw_window(window: &Window) -> std::result::Result<RawWindow, String> {
    Ok(u64::from(window.id()) as isize)
}

type Notify = Arc<dyn Fn() + Send + Sync>;
/// The shell's generic wake, which runs every pump once.
static EVENT_NOTIFY: Mutex<Option<Notify>> = Mutex::new(None);

/// Keeps the shell's wake for services that answer from another thread: a
/// portal dialog's worker, the appearance worker, the macOS menu target.
pub fn set_event_notify(notify: Notify) {
    *EVENT_NOTIFY.lock().unwrap_or_else(PoisonError::into_inner) = Some(notify);
}
/// The shell's wake, or one that does nothing before the shell set it.
pub(super) fn event_notify() -> Notify {
    EVENT_NOTIFY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| Arc::new(|| {}))
}

#[cfg(target_os = "macos")]
thread_local! {
    /// The channel the shell drains for menu commands. AppKit objects live on
    /// the main thread, which is the thread that installs the hook.
    static COMMANDS: std::cell::RefCell<Option<Sender<CommandMessage>>> = const { std::cell::RefCell::new(None) };
}
/// The sender the menu bar posts chosen commands to. Before the hook ran (unit
/// tests), a channel nobody reads.
#[cfg(target_os = "macos")]
pub(super) fn command_sender() -> Sender<CommandMessage> {
    COMMANDS
        .with(|commands| commands.borrow().clone())
        .unwrap_or_else(|| std::sync::mpsc::channel().0)
}

/// Native window messages do not exist here. On macOS the menu bar's commands
/// come through `commands` (see `Platform::new`); tray actions and packet input
/// never arrive, so their senders are dropped.
pub fn install_message_hook(
    _builder: &mut EventLoopBuilder<Wake>,
    _input_window: Rc<Cell<(isize, u64)>>,
    commands: Sender<CommandMessage>,
    _tray: Sender<TrayAction>,
) {
    #[cfg(target_os = "macos")]
    COMMANDS.with(|kept| *kept.borrow_mut() = Some(commands));
    #[cfg(not(target_os = "macos"))]
    drop(commands);
}

/// The event as the shell's keymap reads it. The keymap's semantic modifiers
/// are read from winit's state by their Windows and Linux keys (Primary is
/// Control, Super is the logo key); on macOS each comes from the key
/// `keys::macos_modifier` assigns it (Primary is Command, which winit reports
/// as Super, and Super is Control), so every shell handler sees the same
/// chords on every system.
#[cfg(target_os = "macos")]
pub fn translate_event(event: WindowEvent) -> WindowEvent {
    use bareline_platform::{PhysicalModifier, SemanticModifier};
    use bareline_platform_macos::keys::macos_modifier;
    use winit::keyboard::ModifiersState;
    let WindowEvent::ModifiersChanged(modifiers) = event else {
        return event;
    };
    let state = modifiers.state();
    let held = |modifier: PhysicalModifier| match modifier {
        PhysicalModifier::Control => state.control_key(),
        PhysicalModifier::Alt => state.alt_key(),
        PhysicalModifier::Shift => state.shift_key(),
        PhysicalModifier::Super => state.super_key(),
    };
    let mut shell = ModifiersState::empty();
    for (semantic, bit) in [
        (SemanticModifier::Primary, ModifiersState::CONTROL),
        (SemanticModifier::Alt, ModifiersState::ALT),
        (SemanticModifier::Shift, ModifiersState::SHIFT),
        (SemanticModifier::Super, ModifiersState::SUPER),
    ] {
        if held(macos_modifier(semantic)) {
            shell |= bit;
        }
    }
    WindowEvent::ModifiersChanged(shell.into())
}
/// Linux keyboards already report Control as the keymap's primary modifier.
#[cfg(not(target_os = "macos"))]
pub fn translate_event(event: WindowEvent) -> WindowEvent {
    event
}
