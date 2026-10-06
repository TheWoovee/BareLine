// SPDX-License-Identifier: MPL-2.0
//! The desktop's appearance (light or dark, higher contrast), the user's
//! language and spell checking.
//!
//! The appearance comes from the XDG desktop portal's Settings interface
//! (`org.freedesktop.appearance` `color-scheme` and `contrast`), which GNOME,
//! KDE and other desktops implement. A worker thread reads it, keeps the cached
//! value current from the portal's `SettingChanged` signal and calls the change
//! hook, so the UI thread only ever reads the cache. Without a portal the cache
//! keeps "no preference" and normal contrast.
use crate::portal::{DesktopPortal, PortalError, Settings};
use std::{
    ffi::OsString,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

const NAMESPACE: &str = "org.freedesktop.appearance";
const COLOR_SCHEME: &str = "color-scheme";
const CONTRAST: &str = "contrast";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorScheme {
    #[default]
    NoPreference,
    Dark,
    Light,
}
impl ColorScheme {
    /// The portal's value: 1 prefers dark, 2 prefers light, anything else none.
    fn from_portal(value: u32) -> Self {
        match value {
            1 => Self::Dark,
            2 => Self::Light,
            _ => Self::NoPreference,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    pub color_scheme: ColorScheme,
    pub high_contrast: bool,
    /// Whether a portal answered; false means the defaults above are assumed.
    pub portal: bool,
}

type SettingsSource = Box<dyn FnOnce() -> Result<Arc<dyn Settings>, PortalError> + Send>;

/// The cached appearance, kept current by a worker thread.
pub struct LinuxAppearance {
    state: Arc<Mutex<Appearance>>,
    stopped: Arc<AtomicBool>,
}
impl LinuxAppearance {
    /// Follows the session's portal. `on_change` runs on the worker whenever the
    /// cached value changes, including once the first answer arrives.
    pub fn start(on_change: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::start_with(
            Box::new(|| DesktopPortal::session().map(|portal| portal.settings())),
            on_change,
        )
    }
    /// Follows a given Settings source (another bus, or a test double).
    pub fn with_settings(settings: Arc<dyn Settings>, on_change: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::start_with(Box::new(move || Ok(settings)), on_change)
    }
    fn start_with(source: SettingsSource, on_change: Arc<dyn Fn() + Send + Sync>) -> Self {
        let state = Arc::new(Mutex::new(Appearance::default()));
        let stopped = Arc::new(AtomicBool::new(false));
        let (worker_state, worker_stopped) = (state.clone(), stopped.clone());
        // The worker blocks on the bus for the life of the session; it ends with
        // the next signal after this value is dropped, or with the connection.
        let _ = std::thread::Builder::new()
            .name("bareline-appearance".into())
            .spawn(move || follow(source, &worker_state, &worker_stopped, &*on_change));
        Self { state, stopped }
    }
    pub fn current(&self) -> Appearance {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// `Some(true)` for a dark preference, `Some(false)` for light, `None` without one.
    pub fn dark(&self) -> Option<bool> {
        match self.current().color_scheme {
            ColorScheme::Dark => Some(true),
            ColorScheme::Light => Some(false),
            ColorScheme::NoPreference => None,
        }
    }
    pub fn high_contrast(&self) -> bool {
        self.current().high_contrast
    }
}
impl Drop for LinuxAppearance {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}
fn update(state: &Mutex<Appearance>, key: &str, value: u32) -> bool {
    let mut current = state.lock().unwrap_or_else(PoisonError::into_inner);
    let before = *current;
    match key {
        COLOR_SCHEME => current.color_scheme = ColorScheme::from_portal(value),
        CONTRAST => current.high_contrast = value == 1,
        _ => return false,
    }
    current.portal = true;
    *current != before
}
fn follow(
    source: SettingsSource,
    state: &Mutex<Appearance>,
    stopped: &AtomicBool,
    on_change: &(dyn Fn() + Send + Sync),
) {
    let Ok(settings) = source() else {
        return;
    };
    let mut changed = false;
    for key in [COLOR_SCHEME, CONTRAST] {
        match settings.read(NAMESPACE, key) {
            Ok(Some(value)) => changed |= update(state, key, value),
            // Known to the portal but unset: it answered, so record that.
            Ok(None) => {
                let mut current = state.lock().unwrap_or_else(PoisonError::into_inner);
                changed |= !current.portal;
                current.portal = true;
            }
            // No portal or no Settings interface: keep the defaults.
            Err(_) => return,
        }
    }
    if changed {
        on_change();
    }
    let _ = settings.watch(&mut |namespace, key, value| {
        if stopped.load(Ordering::Acquire) {
            return false;
        }
        if namespace == NAMESPACE && update(state, key, value) {
            on_change();
        }
        true
    });
}

/// The user's language as a BCP 47 name such as `de-DE`, by the gettext rules:
/// `LC_ALL`, then `LC_MESSAGES`, then `LANG` name the locale; when that locale
/// is not C/POSIX, the first usable entry of the `LANGUAGE` priority list wins.
/// `None` for the C/POSIX locale or when nothing is set.
pub fn system_ui_language() -> Option<String> {
    ui_language(&|name| std::env::var_os(name))
}
fn ui_language(var: &dyn Fn(&str) -> Option<OsString>) -> Option<String> {
    let value = |name: &str| {
        var(name)
            .and_then(|value| value.into_string().ok())
            .filter(|value| !value.is_empty())
    };
    let locale = value("LC_ALL")
        .or_else(|| value("LC_MESSAGES"))
        .or_else(|| value("LANG"))?;
    let tag = locale_name(&locale)?;
    let preferred = value("LANGUAGE").and_then(|list| list.split(':').find_map(locale_name));
    Some(preferred.unwrap_or(tag))
}
/// `de_DE.UTF-8` or `sr_RS@latin` to `de-DE` or `sr-RS`.
fn locale_name(value: &str) -> Option<String> {
    let name = value.split(['.', '@']).next().unwrap_or_default().replace('_', "-");
    (!name.is_empty() && name != "C" && name != "POSIX").then_some(name)
}

/// No spell checker is integrated on Linux yet (Hunspell or Enchant would be
/// the candidates), so building one reports why in words for a person.
pub fn spell_checker_factory() -> bareline_platform::spelling::SpellCheckerFactory {
    Arc::new(|| Err("This system does not support spell checking yet".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::mpsc::{self, Receiver, Sender},
        time::{Duration, Instant},
    };

    /// A portal whose settings and change signals the test controls.
    struct Fake {
        values: HashMap<&'static str, u32>,
        changes: Mutex<Receiver<(String, String, u32)>>,
        missing: bool,
    }
    impl Settings for Fake {
        fn read(&self, namespace: &str, key: &str) -> Result<Option<u32>, PortalError> {
            if self.missing {
                return Err(PortalError::Unavailable("ServiceUnknown".into()));
            }
            assert_eq!(namespace, NAMESPACE);
            Ok(self.values.get(key).copied())
        }
        fn watch(&self, changed: &mut dyn FnMut(&str, &str, u32) -> bool) -> Result<(), PortalError> {
            let changes = self.changes.lock().unwrap();
            while let Ok((namespace, key, value)) = changes.recv() {
                if !changed(&namespace, &key, value) {
                    break;
                }
            }
            Ok(())
        }
    }
    fn fake(values: &[(&'static str, u32)], missing: bool) -> (Arc<Fake>, Sender<(String, String, u32)>) {
        let (sender, receiver) = mpsc::channel();
        let fake = Arc::new(Fake {
            values: values.iter().copied().collect(),
            changes: Mutex::new(receiver),
            missing,
        });
        (fake, sender)
    }
    fn until(mut done: impl FnMut() -> bool) {
        let watchdog = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < watchdog, "watchdog expired");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn the_portal_preference_and_its_changes_reach_the_cache_and_the_hook() {
        let (settings, changes) = fake(&[(COLOR_SCHEME, 1), (CONTRAST, 0)], false);
        let (notified, notifications) = mpsc::channel();
        let appearance = LinuxAppearance::with_settings(settings, Arc::new(move || notified.send(()).unwrap()));
        notifications.recv().unwrap();
        assert_eq!(appearance.dark(), Some(true));
        assert!(!appearance.high_contrast());
        assert!(appearance.current().portal);
        changes.send((NAMESPACE.into(), COLOR_SCHEME.into(), 2)).unwrap();
        notifications.recv().unwrap();
        assert_eq!(appearance.dark(), Some(false));
        changes.send((NAMESPACE.into(), CONTRAST.into(), 1)).unwrap();
        notifications.recv().unwrap();
        assert!(appearance.high_contrast());
        // Other namespaces and unchanged values do not wake the UI.
        changes
            .send(("org.gnome.desktop.interface".into(), COLOR_SCHEME.into(), 1))
            .unwrap();
        changes.send((NAMESPACE.into(), CONTRAST.into(), 1)).unwrap();
        changes.send((NAMESPACE.into(), COLOR_SCHEME.into(), 0)).unwrap();
        notifications.recv().unwrap();
        assert_eq!(appearance.dark(), None);
        assert!(notifications.try_recv().is_err());
    }
    #[test]
    fn without_a_portal_the_defaults_hold_and_nothing_is_announced() {
        let (settings, _changes) = fake(&[], true);
        let (notified, notifications) = mpsc::channel();
        let appearance = LinuxAppearance::with_settings(settings, Arc::new(move || notified.send(()).unwrap()));
        until(|| Arc::strong_count(&appearance.state) == 1);
        assert_eq!(appearance.current(), Appearance::default());
        assert_eq!(appearance.dark(), None);
        assert!(notifications.try_recv().is_err());
        // A portal without the keys still counts as answering.
        let (settings, _changes) = fake(&[], false);
        let appearance = LinuxAppearance::with_settings(settings, Arc::new(|| {}));
        until(|| appearance.current().portal);
        assert_eq!(appearance.current().color_scheme, ColorScheme::NoPreference);
    }
    #[test]
    fn languages_follow_the_gettext_rules() {
        let resolve = |vars: &[(&str, &str)]| {
            let vars: HashMap<String, OsString> =
                vars.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
            ui_language(&|name| vars.get(name).cloned())
        };
        assert_eq!(resolve(&[("LANG", "de_DE.UTF-8")]).as_deref(), Some("de-DE"));
        assert_eq!(
            resolve(&[("LANG", "de_DE.UTF-8"), ("LC_MESSAGES", "fr_FR")]).as_deref(),
            Some("fr-FR")
        );
        assert_eq!(
            resolve(&[("LC_ALL", "sr_RS@latin"), ("LC_MESSAGES", "fr_FR")]).as_deref(),
            Some("sr-RS")
        );
        assert_eq!(
            resolve(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "pt_BR:pt:en")]).as_deref(),
            Some("pt-BR")
        );
        // LANGUAGE is ignored under the C locale, as gettext does.
        assert_eq!(resolve(&[("LANG", "C.UTF-8"), ("LANGUAGE", "pt_BR")]), None);
        assert_eq!(resolve(&[("LANG", ""), ("LC_ALL", "POSIX")]), None);
        assert_eq!(resolve(&[]), None);
        assert_eq!(locale_name("en").as_deref(), Some("en"));
    }
    #[test]
    fn spell_checking_reports_that_it_is_unavailable() {
        let error = spell_checker_factory()().err().unwrap();
        assert!(error.starts_with("This system does not support"), "{error}");
    }
}
