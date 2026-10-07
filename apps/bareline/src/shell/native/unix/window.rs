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
/// (their emulated button press is the flagged, dropped half), nor do
/// high-resolution wheels and touchpads, which report each notch or step once
/// as a valuator delta. winit reports both kinds alike, by the master pointer.
///
/// So a device's echoes are learned, never assumed (LNX-UI-011; winit itself is
/// left as published). Every line delta of a doubling device arrives as a
/// whole notch (±1.0 on one axis) followed in the same batch of events by its
/// equal echo; a device that sends a lone delta (unpaired when another delta
/// or the end of the batch comes) or a fractional one reports each notch once
/// and is never deduplicated again this session. A device whose batch ends
/// with all its deltas in equal notch pairs is doubling, and from its next
/// batch the second of each pair is dropped. Limits: a doubling device's
/// first batch scrolls twice as far; a device that reports single notches but
/// whose first scrolling batch holds only equal pairs (two quick notches) loses
/// every second notch until its first lone notch, usually the next slow one;
/// and as XI2 events carry the master pointer, an XTEST pointer stops being
/// deduplicated once a real wheel on the same master is seen. Wayland reports
/// each notch once.
#[cfg(not(target_os = "macos"))]
pub fn translate_event(event_loop: &ActiveEventLoop, event: WindowEvent) -> Option<WindowEvent> {
    use winit::platform::x11::ActiveEventLoopExtX11;
    (!(event_loop.is_x11() && wheel_echo(&event))).then_some(event)
}
/// What a device's line deltas have shown about it (see `translate_event`).
#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy, PartialEq)]
enum WheelReport {
    /// Nothing yet: no delta is dropped.
    Unknown,
    /// Each notch arrives with its echo, which is dropped.
    Doubled,
    /// Each notch arrives once; nothing is dropped again.
    Single,
}
#[cfg(not(target_os = "macos"))]
struct WheelDevice {
    id: winit::event::DeviceId,
    report: WheelReport,
    /// The notch that opened a possible notch-and-echo pair in this batch.
    open: Option<(f32, f32)>,
    /// Whether a pair closed in this batch.
    paired: bool,
}
#[cfg(not(target_os = "macos"))]
thread_local! {
    static WHEELS: std::cell::RefCell<Vec<WheelDevice>> = const { std::cell::RefCell::new(Vec::new()) };
}
/// Whether `event` is the echo of the wheel notch before it from a doubling
/// device (see `translate_event`), and what it shows about its device.
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
    WHEELS.with_borrow_mut(|devices| {
        let index = devices
            .iter()
            .position(|device| device.id == device_id)
            .unwrap_or_else(|| {
                devices.push(WheelDevice {
                    id: device_id,
                    report: WheelReport::Unknown,
                    open: None,
                    paired: false,
                });
                devices.len() - 1
            });
        let device = &mut devices[index];
        if device.report == WheelReport::Single {
            return false;
        }
        let notch = matches!((x.abs(), y.abs()), (0.0, 1.0) | (1.0, 0.0));
        match device.open.take() {
            Some(open) if notch && open == (x, y) => {
                device.paired = true;
                device.report == WheelReport::Doubled
            }
            None if notch => {
                device.open = Some((x, y));
                false
            }
            // A notch with no echo, or a fraction of one.
            _ => {
                device.report = WheelReport::Single;
                false
            }
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
    WHEELS.with_borrow_mut(|devices| {
        for device in devices {
            if device.open.take().is_some() {
                // A notch whose echo did not follow.
                device.report = WheelReport::Single;
            } else if device.paired && device.report == WheelReport::Unknown {
                device.report = WheelReport::Doubled;
            }
            device.paired = false;
        }
    });
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
    /// Which of `events` scroll, as one batch of events.
    fn batch(events: &[WindowEvent]) -> Vec<bool> {
        let kept = events.iter().map(|event| !wheel_echo(event)).collect();
        end_event_batch();
        kept
    }
    /// A fresh session: no device has been seen.
    fn forget_devices() {
        WHEELS.with_borrow_mut(Vec::clear);
    }

    /// LNX-UI-011: an X11 button wheel's notch arrives as a button press and
    /// an emulated valuator motion (XTEST, evdev, remote desktops); once the
    /// device shows it, only one of them scrolls.
    #[test]
    fn a_doubling_wheel_scrolls_once_per_notch_once_learned() {
        forget_devices();
        let down = notch(-1.0);
        // The first batch is the evidence: a notch and its echo.
        assert_eq!(batch(&[down.clone(), down.clone()]), [true, true]);
        // From then on the echo is dropped, two notches in a batch included.
        assert_eq!(batch(&[down.clone(), down.clone()]), [true, false]);
        assert_eq!(
            batch(&[down.clone(), down.clone(), down.clone(), down.clone()]),
            [true, false, true, false]
        );
        assert_eq!(
            batch(&[notch(1.0), notch(1.0), down.clone(), down]),
            [true, false, true, false]
        );
        // Other events are never echoes.
        assert!(!wheel_echo(&WindowEvent::RedrawRequested));
    }

    /// LNX-UI-011 review: libinput, high-resolution and touchpad wheels report
    /// each notch once; equal notches queued in one batch all scroll.
    #[test]
    fn a_single_reporting_wheel_never_loses_a_notch() {
        let down = notch(-1.0);
        // A lone notch shows the device reports each notch once.
        forget_devices();
        assert_eq!(batch(std::slice::from_ref(&down)), [true]);
        assert_eq!(batch(&[down.clone(), down.clone()]), [true, true]);
        assert_eq!(
            batch(&[down.clone(), down.clone(), down.clone(), down.clone()]),
            [true, true, true, true]
        );
        // So does an odd run in the first batch.
        forget_devices();
        assert_eq!(batch(&[down.clone(), down.clone(), down.clone()]), [true; 3]);
        assert_eq!(batch(&[down.clone(), down.clone()]), [true, true]);
        // And a direction change that leaves a notch without an echo.
        forget_devices();
        assert_eq!(batch(&[down.clone(), notch(1.0)]), [true, true]);
        assert_eq!(batch(&[down.clone(), down.clone()]), [true, true]);
        // High-resolution wheels and touchpads send fractions, equal or not.
        forget_devices();
        let step = notch(-0.125);
        assert_eq!(batch(&[step.clone(), step.clone(), step.clone(), step]), [true; 4]);
        assert_eq!(batch(&[down.clone(), down.clone()]), [true, true]);
        // Two quick notches in the first batch look like a notch and its echo;
        // the next lone notch corrects that for the rest of the session.
        forget_devices();
        assert_eq!(batch(&[down.clone(), down.clone()]), [true, true]);
        assert_eq!(batch(std::slice::from_ref(&down)), [true]);
        assert_eq!(
            batch(&[down.clone(), down.clone(), down.clone(), down.clone()]),
            [true; 4]
        );
        // Pixel deltas (Wayland smooth scrolling) are never touched.
        forget_devices();
        let pixels = WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, 3.0)),
            phase: TouchPhase::Moved,
        };
        assert_eq!(batch(&[pixels.clone(), pixels]), [true, true]);
        assert_eq!(batch(&[down.clone(), down.clone()]), [true, true]);
        assert_eq!(batch(&[down.clone(), down]), [true, false]);
    }
}
