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
use winit::{
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoopBuilder},
    window::{Window, WindowAttributes},
};

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
pub fn translate_event(_event_loop: &ActiveEventLoop, event: WindowEvent) -> Option<WindowEvent> {
    use bareline_platform::{PhysicalModifier, SemanticModifier};
    use bareline_platform_macos::keys::macos_modifier;
    use winit::keyboard::ModifiersState;
    let WindowEvent::ModifiersChanged(modifiers) = event else {
        return Some(event);
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
    Some(WindowEvent::ModifiersChanged(shell.into()))
}
/// Linux keyboards already report Control as the keymap's primary modifier.
///
/// On X11, winit 0.30.13 reports a notch of a wheel without smooth scrolling
/// twice: once from the XI2 button press (buttons 4 to 7) and once from the
/// scroll-valuator motion the server emulates for it, whose XIPointerEmulated
/// flag `xinput2_mouse_motion` does not check (only the button path does).
/// XTEST, evdev button wheels and remote desktops send such pairs, so each
/// notch scrolled twice as far; smooth-scroll wheels under libinput do not
/// (their emulated button press is the flagged, dropped half). The echo is
/// the second of two equal line deltas from one device in one batch of
/// events, and is dropped here; winit itself is left as published
/// (LNX-UI-011). Wayland reports each notch once.
#[cfg(not(target_os = "macos"))]
pub fn translate_event(event_loop: &ActiveEventLoop, event: WindowEvent) -> Option<WindowEvent> {
    use winit::platform::x11::ActiveEventLoopExtX11;
    (!(event_loop.is_x11() && wheel_echo(&event))).then_some(event)
}
#[cfg(not(target_os = "macos"))]
thread_local! {
    /// The line delta that opened a possible button-and-valuator pair in this
    /// batch of events, by device.
    static WHEEL_PAIR: Cell<Option<(winit::event::DeviceId, (f32, f32))>> = const { Cell::new(None) };
}
/// Whether `event` repeats the wheel notch that came before it in this batch
/// from the same device (see `translate_event`). Pairs close as they match, so
/// two notches in one batch (four events) still scroll twice.
#[cfg(not(target_os = "macos"))]
fn wheel_echo(event: &WindowEvent) -> bool {
    let WindowEvent::MouseWheel {
        device_id,
        delta: winit::event::MouseScrollDelta::LineDelta(x, y),
        ..
    } = *event
    else {
        return false;
    };
    WHEEL_PAIR.with(|pair| {
        if pair.get() == Some((device_id, (x, y))) {
            pair.set(None);
            true
        } else {
            pair.set(Some((device_id, (x, y))));
            false
        }
    })
}
/// The application id desktops match the window to its `.desktop` entry by
/// (icon, task bar grouping): the Wayland xdg_toplevel app_id and the X11
/// WM_CLASS, both "bareline" (LNX-UI-004). winit sends no app_id without it.
#[cfg(target_os = "linux")]
pub const APPLICATION_ID: &str = "bareline";
/// The editor window's attributes with its identity on this system.
pub fn identify_window(attributes: WindowAttributes) -> WindowAttributes {
    #[cfg(target_os = "linux")]
    {
        use winit::platform::{wayland::WindowAttributesExtWayland, x11::WindowAttributesExtX11};
        let attributes = WindowAttributesExtWayland::with_name(attributes, APPLICATION_ID, APPLICATION_ID);
        WindowAttributesExtX11::with_name(attributes, APPLICATION_ID, APPLICATION_ID)
    }
    #[cfg(not(target_os = "linux"))]
    attributes
}
/// The event loop has delivered a batch of events and is about to wait.
pub fn end_event_batch() {
    #[cfg(not(target_os = "macos"))]
    WHEEL_PAIR.with(|pair| pair.set(None));
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use winit::event::{DeviceId, MouseScrollDelta, TouchPhase};

    fn notch(y: f32) -> WindowEvent {
        WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::LineDelta(0.0, y),
            phase: TouchPhase::Moved,
        }
    }

    /// LNX-UI-011: an X11 button wheel's notch arrives as a button press and
    /// an emulated valuator motion; only one of them scrolls.
    #[test]
    fn a_wheel_notch_and_its_emulated_echo_scroll_once() {
        end_event_batch();
        // Two notches in one batch: press, echo, press, echo.
        let kept: Vec<_> = [notch(-1.0), notch(-1.0), notch(-1.0), notch(-1.0)]
            .iter()
            .map(|event| !wheel_echo(event))
            .collect();
        assert_eq!(kept, [true, false, true, false]);
        // Other deltas and other events are never echoes.
        end_event_batch();
        assert!(!wheel_echo(&notch(1.0)));
        assert!(!wheel_echo(&notch(-1.0)));
        assert!(!wheel_echo(&WindowEvent::RedrawRequested));
        // A pair never spans two batches.
        end_event_batch();
        assert!(!wheel_echo(&notch(1.0)));
        end_event_batch();
        assert!(!wheel_echo(&notch(1.0)));
        // Pixel deltas (touchpads, Wayland smooth scrolling) pass untouched.
        let pixels = WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, 3.0)),
            phase: TouchPhase::Moved,
        };
        assert!(!wheel_echo(&pixels) && !wheel_echo(&pixels));
    }
}
