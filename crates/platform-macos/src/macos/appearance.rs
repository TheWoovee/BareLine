// SPDX-License-Identifier: MPL-2.0
//! Light or dark, increased contrast, and the user's language.
//!
//! [`MacAppearance::changed`] is the polling hook: the seam calls it when the
//! window is activated and when winit reports `ThemeChanged` (winit observes
//! `effectiveAppearance` on macOS), and it answers only when something
//! changed. Increased contrast has no winit event; activation covers it,
//! because the setting is changed in System Settings, which takes focus.
use crate::types::language_tag;
use objc2_app_kit::{NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSWorkspace};
use objc2_foundation::{MainThreadMarker, NSArray, NSCopying, NSLocale};
use std::{cell::Cell, io};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub dark: bool,
    /// System Settings > Accessibility > Display > Increase contrast.
    pub increased_contrast: bool,
}

/// Main thread only: [`MacAppearance::new`] takes a `MainThreadMarker`.
pub struct MacAppearance {
    mtm: MainThreadMarker,
    last: Cell<Option<Appearance>>,
}

impl MacAppearance {
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            last: Cell::new(None),
        }
    }
    /// The application's effective appearance now.
    pub fn current(&self) -> Appearance {
        let application = NSApplication::sharedApplication(self.mtm);
        let appearance = application.effectiveAppearance();
        // SAFETY: the appearance names are framework constants.
        let (aqua, dark_aqua) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
        let names = NSArray::from_vec(vec![aqua.copy(), dark_aqua.copy()]);
        let dark = appearance
            .bestMatchFromAppearancesWithNames(&names)
            .is_some_and(|name| &*name == dark_aqua);
        Appearance {
            dark,
            increased_contrast: increased_contrast(),
        }
    }
    /// The appearance if it differs from the last answer (the first call
    /// always answers).
    pub fn changed(&self) -> Option<Appearance> {
        let current = self.current();
        (self.last.replace(Some(current)) != Some(current)).then_some(current)
    }
}

fn increased_contrast() -> bool {
    // SAFETY: the shared workspace and its accessibility display options are
    // readable from any thread.
    unsafe { NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast() }
}

/// The seam's `high_contrast_enabled`: macOS's increased-contrast setting.
pub fn high_contrast_enabled() -> io::Result<bool> {
    Ok(increased_contrast())
}

/// The user's first preferred language as a BCP 47 tag, such as `en-US` or
/// `zh-Hans-CN` (System Settings > General > Language & Region). Unlike the
/// POSIX locale variables, this is what macOS applications localize to.
pub fn system_ui_language() -> Option<String> {
    // SAFETY: NSLocale's class properties are thread-safe.
    let languages = unsafe { NSLocale::preferredLanguages() };
    languages.first().and_then(|first| language_tag(&first.to_string()))
}
