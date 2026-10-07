// SPDX-License-Identifier: MPL-2.0
//! System preferences the shell reads: light or dark, contrast, the user's
//! language, the legacy code page and spell checking.
//!
//! Linux follows the desktop portal's appearance through one
//! `LinuxAppearance` per process, started the first time the shell asks; its
//! worker wakes the event loop when the desktop changes, and the shell then
//! reads the cached value again (`appearance_changed`). Without a portal (WSL,
//! a bare X server) the defaults apply: no preference, normal contrast. macOS
//! reads the application's effective appearance and increased-contrast
//! setting through AppKit, and asks `MacAppearance::changed` again whenever the
//! window is activated (`appearance_activated`): increased contrast has no
//! event, and System Settings, where both are changed, takes focus.
#[cfg(target_os = "linux")]
use std::io;
use winit::window::{Theme, Window};

#[cfg(target_os = "linux")]
mod desktop {
    use bareline_platform_linux::LinuxAppearance;
    use std::sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    };

    static APPEARANCE: OnceLock<LinuxAppearance> = OnceLock::new();
    /// Set by the appearance worker; taken by the event loop.
    static CHANGED: AtomicBool = AtomicBool::new(false);

    pub(super) fn appearance() -> &'static LinuxAppearance {
        APPEARANCE.get_or_init(|| {
            LinuxAppearance::start(Arc::new(|| {
                CHANGED.store(true, Ordering::Release);
                // The shell's wake at the time of the change: the appearance
                // may start before the event loop exists.
                super::super::window::event_notify()();
            }))
        })
    }
    pub(super) fn take_changed() -> bool {
        APPEARANCE.get().is_some() && CHANGED.swap(false, Ordering::AcqRel)
    }
}

/// The light or dark preference the first frame and later changes follow: the
/// portal's preference on Linux, where winit reports none on X11.
#[cfg(target_os = "linux")]
pub fn window_theme(window: &Window) -> Option<Theme> {
    theme(desktop::appearance().dark()).or_else(|| window.theme())
}
#[cfg(target_os = "macos")]
mod appkit {
    use bareline_platform_macos::{MacAppearance, MainThreadMarker};
    use std::cell::OnceCell;

    thread_local! {
        static APPEARANCE: OnceCell<MacAppearance> = const { OnceCell::new() };
        static ACTIVATION: super::Activation = const { super::Activation(std::cell::Cell::new(false)) };
    }
    /// The main thread's reader. Its first answer is the baseline `changed`
    /// compares with, so only a later change counts.
    pub(super) fn with<R>(read: impl FnOnce(&MacAppearance) -> R) -> Option<R> {
        let mtm = MainThreadMarker::new()?;
        Some(APPEARANCE.with(|appearance| {
            read(appearance.get_or_init(|| {
                let appearance = MacAppearance::new(mtm);
                let _ = appearance.changed();
                appearance
            }))
        }))
    }
    pub(super) fn activated() {
        ACTIVATION.with(super::Activation::arm);
    }
    pub(super) fn take_changed() -> bool {
        ACTIVATION
            .with(|activation| activation.changed(|| with(|appearance| appearance.changed().is_some()) == Some(true)))
    }
}
/// AppKit's effective appearance of the application.
#[cfg(target_os = "macos")]
pub fn window_theme(window: &Window) -> Option<Theme> {
    theme(appkit::with(|appearance| appearance.current().dark)).or_else(|| window.theme())
}
fn theme(dark: Option<bool>) -> Option<Theme> {
    dark.map(|dark| if dark { Theme::Dark } else { Theme::Light })
}
/// Whether the desktop's appearance changed since the last call, so the shell
/// reads the theme and contrast again.
#[cfg(target_os = "linux")]
pub fn appearance_changed() -> bool {
    desktop::take_changed()
}
/// Whether AppKit's appearance or increased contrast changed since it was last
/// asked, which happens once after each activation of the window.
#[cfg(target_os = "macos")]
pub fn appearance_changed() -> bool {
    appkit::take_changed()
}
/// The window was activated (it became key, as when the person returns from
/// System Settings). macOS asks AppKit at the next `appearance_changed`; the
/// Linux portal reports its changes itself.
pub fn appearance_activated() {
    #[cfg(target_os = "macos")]
    appkit::activated();
}
/// Asks a system without change events again only once per activation of the
/// window, so the event loop does not query it on every turn.
#[cfg(any(target_os = "macos", test))]
#[derive(Default)]
struct Activation(std::cell::Cell<bool>);
#[cfg(any(target_os = "macos", test))]
impl Activation {
    fn arm(&self) {
        self.0.set(true);
    }
    /// `changed` answers whether the appearance differs from its last answer.
    fn changed(&self, changed: impl FnOnce() -> bool) -> bool {
        self.0.take() && changed()
    }
}
#[cfg(target_os = "linux")]
pub fn high_contrast_enabled() -> io::Result<bool> {
    Ok(desktop::appearance().high_contrast())
}
/// System Settings > Accessibility > Display > Increase contrast.
#[cfg(target_os = "macos")]
pub use bareline_platform_macos::high_contrast_enabled;
pub fn high_contrast_highlight() -> Option<(u32, u32)> {
    None
}
/// The user's language as a BCP 47 name such as `de-DE`: from the gettext
/// variables (`LANGUAGE` first, then the POSIX locale).
#[cfg(target_os = "linux")]
pub use bareline_platform_linux::system_ui_language;
/// The first preferred language in System Settings, from NSLocale.
#[cfg(target_os = "macos")]
pub use bareline_platform_macos::system_ui_language;
/// The code page legacy text falls back to: UTF-8 on these systems.
pub fn system_code_page() -> u32 {
    65001
}
#[cfg(target_os = "linux")]
pub use bareline_platform_linux::spell_checker_factory;
#[cfg(target_os = "macos")]
pub fn spell_checker_factory() -> bareline_platform::spelling::SpellCheckerFactory {
    std::sync::Arc::new(|| Err("This system does not support spell checking yet".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_preferences_answer_without_a_desktop() {
        // Answers come from a cache (Linux) or AppKit (macOS), never an error.
        assert!(high_contrast_enabled().is_ok());
        assert!(high_contrast_highlight().is_none());
        assert_eq!(system_code_page(), 65001);
        assert!(system_ui_language().is_none_or(|language| !language.is_empty()));
        assert!(spell_checker_factory()().is_err());
    }

    /// macOS consults `MacAppearance::changed` after each activation of the
    /// window, once, and never between activations.
    #[test]
    fn appearance_is_asked_again_once_per_activation() {
        let activation = Activation::default();
        let asked = std::cell::Cell::new(0);
        let ask = |answer: bool| {
            asked.set(asked.get() + 1);
            answer
        };
        assert!(!activation.changed(|| ask(true)));
        assert_eq!(asked.get(), 0, "not asked before an activation");
        activation.arm();
        assert!(activation.changed(|| ask(true)));
        assert!(!activation.changed(|| ask(true)));
        assert_eq!(asked.get(), 1, "asked once per activation");
        activation.arm();
        assert!(!activation.changed(|| ask(false)));
        assert_eq!(asked.get(), 2);
        // The seam answers without a window or AppKit main thread.
        appearance_activated();
        let _ = appearance_changed();
    }
}
