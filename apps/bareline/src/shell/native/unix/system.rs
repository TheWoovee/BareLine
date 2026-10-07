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
//! setting through AppKit.
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
/// AppKit's effective appearance of the application.
#[cfg(target_os = "macos")]
pub fn window_theme(window: &Window) -> Option<Theme> {
    use bareline_platform_macos::{MacAppearance, MainThreadMarker};
    thread_local! {
        static APPEARANCE: std::cell::OnceCell<MacAppearance> = const { std::cell::OnceCell::new() };
    }
    let dark = MainThreadMarker::new()
        .map(|mtm| APPEARANCE.with(|appearance| appearance.get_or_init(|| MacAppearance::new(mtm)).current().dark));
    theme(dark).or_else(|| window.theme())
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
/// macOS reports appearance changes through winit's `ThemeChanged` and window
/// activation, which the shell already follows.
#[cfg(target_os = "macos")]
pub fn appearance_changed() -> bool {
    false
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
}
