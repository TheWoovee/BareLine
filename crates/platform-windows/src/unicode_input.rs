// SPDX-License-Identifier: MPL-2.0
//! Compatibility at winit 0.30's Windows message boundary.
//!
//! SendInput delivers an astral scalar as two VK_PACKET key presses. Winit
//! finalizes text per key press, so each unpaired UTF-16 unit becomes no text.
//! Hold the high unit and dispatch the completed scalar with the low unit's
//! pending key event. Winit's WM_CHAR handler explicitly accepts UTF-32 values.
//! This stays synchronous in the normal keyboard route, before field routing.

use windows::Win32::{
    Foundation::WPARAM,
    UI::WindowsAndMessaging::{DispatchMessageW, MSG, WM_CHAR, WM_KEYDOWN, WM_KEYUP},
};

const VK_PACKET: usize = 0xE7;

#[derive(Debug, PartialEq)]
enum Delivery {
    Pass,
    Hold,
    Scalar(char),
}

/// One bounded decoder per Windows event loop. Input/context changes retire a
/// partial scalar; paint, timer and worker wakes must not split a valid pair.
#[derive(Default)]
pub struct UnicodePacketInput {
    packet_window: Option<isize>,
    high: Option<(isize, u16)>,
    context: (isize, u64),
}

impl UnicodePacketInput {
    fn filter(&mut self, window: isize, message: u32, value: usize) -> Delivery {
        match message {
            WM_KEYDOWN if value == VK_PACKET => {
                if self.high.is_some_and(|(owner, _)| owner != window) {
                    self.high = None;
                }
                self.packet_window = Some(window);
            }
            WM_KEYUP if value == VK_PACKET && self.packet_window == Some(window) => {
                self.packet_window = None;
            }
            WM_CHAR if self.packet_window == Some(window) => {
                if (0xD800..=0xDBFF).contains(&value) {
                    self.high = Some((window, value as u16));
                    return Delivery::Hold;
                }
                let high = self.high.take();
                if (0xDC00..=0xDFFF).contains(&value) {
                    return match high {
                        Some((owner, high)) if owner == window => {
                            let scalar = 0x10000 + ((u32::from(high) - 0xD800) << 10) + (value as u32 - 0xDC00);
                            Delivery::Scalar(char::from_u32(scalar).unwrap())
                        }
                        _ => Delivery::Hold,
                    };
                }
            }
            // Keyboard, command, focus, IME and pointer-button boundaries.
            0x0100..=0x0109 | 0x0111 | 0x0007 | 0x0008 | 0x010D..=0x010F | 0x0201..=0x020E => {
                self.packet_window = None;
                self.high = None;
            }
            _ => {}
        }
        Delivery::Pass
    }

    /// Return true only when this hook has consumed or synchronously dispatched
    /// the message; all other messages follow winit's ordinary dispatch.
    ///
    /// # Safety
    /// raw is the valid MSG supplied by winit's Windows message hook, on the
    /// owning event-loop thread. target contains the live winit HWND (zero
    /// before creation) and an epoch advanced on native/UIA focus changes.
    pub unsafe fn process_message(&mut self, raw: *const std::ffi::c_void, target: (isize, u64)) -> bool {
        // SAFETY: the caller supplies the live message for this invocation.
        let message = unsafe { &*(raw as *const MSG) };
        if self.context != target {
            self.packet_window = None;
            self.high = None;
            self.context = target;
        }
        if target.0 == 0 || message.hwnd.0 as isize != target.0 {
            if matches!(message.message, 0x0100..=0x010F | 0x0111 | 0x0007 | 0x0008 | 0x0201..=0x020E) {
                self.packet_window = None;
                self.high = None;
            }
            return false;
        }
        match self.filter(message.hwnd.0 as isize, message.message, message.wParam.0) {
            Delivery::Pass => false,
            Delivery::Hold => true,
            Delivery::Scalar(character) => {
                // Copy the message: never mutate winit's borrowed queue entry.
                let mut completed = *message;
                completed.wParam = WPARAM(character as usize);
                // SAFETY: dispatch once to the original window on its owner
                // thread. WM_CHAR needs no TranslateMessage step. Winit still
                // has the low packet's key event and delivers normal event.text.
                unsafe { DispatchMessageW(&completed) };
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(input: &mut UnicodePacketInput, window: isize, unit: u16) -> Delivery {
        assert_eq!(input.filter(window, WM_KEYDOWN, VK_PACKET), Delivery::Pass);
        let delivered = input.filter(window, WM_CHAR, usize::from(unit));
        assert_eq!(input.filter(window, WM_KEYUP, VK_PACKET), Delivery::Pass);
        delivered
    }

    #[test]
    fn separate_packet_key_presses_preserve_one_astral_scalar() {
        let mut input = UnicodePacketInput::default();
        assert_eq!(packet(&mut input, 1, 0xD83C), Delivery::Hold);
        assert_eq!(packet(&mut input, 1, 0xDF89), Delivery::Scalar('🎉'));
        assert_eq!(packet(&mut input, 1, 0xDF89), Delivery::Hold);
    }

    #[test]
    fn bmp_and_combining_input_pass_without_duplication() {
        let mut input = UnicodePacketInput::default();
        for unit in "Ae\u{301}文".encode_utf16() {
            assert_eq!(packet(&mut input, 1, unit), Delivery::Pass);
        }
    }

    #[test]
    fn malformed_high_does_not_swallow_following_bmp_text() {
        let mut input = UnicodePacketInput::default();
        assert_eq!(packet(&mut input, 1, 0xD83C), Delivery::Hold);
        assert_eq!(packet(&mut input, 1, b'A' as u16), Delivery::Pass);
        assert_eq!(packet(&mut input, 1, 0xDF89), Delivery::Hold);
    }

    #[test]
    fn latest_high_surrogate_replaces_abandoned_high() {
        let mut input = UnicodePacketInput::default();
        assert_eq!(packet(&mut input, 1, 0xD83C), Delivery::Hold);
        assert_eq!(packet(&mut input, 1, 0xD83D), Delivery::Hold);
        assert_eq!(packet(&mut input, 1, 0xDE00), Delivery::Scalar('😀'));
    }

    #[test]
    fn partial_scalar_cannot_cross_windows_or_queued_context_changes() {
        let mut input = UnicodePacketInput::default();
        packet(&mut input, 1, 0xD83C);
        assert_eq!(packet(&mut input, 2, 0xDF89), Delivery::Hold);
        for message in [0x0008, 0x0111, 0x0201] {
            packet(&mut input, 1, 0xD83C);
            assert_eq!(input.filter(1, message, 0), Delivery::Pass);
            assert_eq!(packet(&mut input, 1, 0xDF89), Delivery::Hold);
        }
    }

    #[test]
    fn ordinary_keys_and_ime_messages_keep_the_existing_route() {
        let mut input = UnicodePacketInput::default();
        assert_eq!(input.filter(1, WM_KEYDOWN, b'A' as usize), Delivery::Pass);
        assert_eq!(input.filter(1, WM_CHAR, 0xD83C), Delivery::Pass);
        assert_eq!(input.filter(1, WM_CHAR, 0xDF89), Delivery::Pass);
        packet(&mut input, 1, 0xD83C);
        input.filter(1, WM_KEYDOWN, b'A' as usize);
        assert_eq!(packet(&mut input, 1, 0xDF89), Delivery::Hold);
    }

    #[test]
    fn native_dialog_and_unregistered_window_messages_are_untouched() {
        let mut input = UnicodePacketInput::default();
        packet(&mut input, 1, 0xD83C);
        let message = MSG {
            message: WM_CHAR,
            wParam: WPARAM(0xDF89),
            ..Default::default()
        };
        // Neither call may dispatch to this unrelated/null window.
        for target in [(0, 0), (1, 1)] {
            assert!(!unsafe { input.process_message(&message as *const _ as _, target) });
        }
        assert_eq!(packet(&mut input, 1, 0xDF89), Delivery::Hold);
    }

    #[test]
    fn redraw_timer_and_worker_wakes_do_not_split_a_scalar() {
        for message in [0x000F, 0x0113, 0x8000] {
            let mut input = UnicodePacketInput::default();
            packet(&mut input, 1, 0xD83C);
            assert_eq!(input.filter(1, message, 0), Delivery::Pass);
            assert_eq!(packet(&mut input, 1, 0xDF89), Delivery::Scalar('🎉'));
        }
    }

    #[test]
    fn focus_epoch_retires_a_partial_scalar_even_on_the_same_window() {
        let mut input = UnicodePacketInput {
            context: (1, 1),
            ..Default::default()
        };
        packet(&mut input, 1, 0xD83C);
        let message = MSG::default();
        assert!(!unsafe { input.process_message(&message as *const _ as _, (1, 2)) });
        assert_eq!(packet(&mut input, 1, 0xDF89), Delivery::Hold);
    }
}
