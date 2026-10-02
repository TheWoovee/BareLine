// SPDX-License-Identifier: MPL-2.0
//! User configuration only; workspace policy belongs to PR-015.
/// Software drawing is the default: it measured 24 MB idle and a 109 ms first
/// frame against 57 MB and 282 ms for Direct2D hardware (ADR-32, PERF-02).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RendererMode {
    Hardware,
    #[default]
    Software,
}
impl RendererMode {
    /// ADR-32 selection: a `--software` or `--hardware` launch flag wins over the
    /// `renderer.mode` setting, which already carries the default. Launch parsing
    /// refuses both flags together; were both set, the setting would apply.
    pub fn select(setting: RendererMode, software_flag: bool, hardware_flag: bool) -> RendererMode {
        match (software_flag, hardware_flag) {
            (true, false) => RendererMode::Software,
            (false, true) => RendererMode::Hardware,
            _ => setting,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub renderer: RendererMode,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsError {
    InvalidUtf8,
    InvalidToml,
    UnsupportedVersion,
    InvalidRenderer,
}
impl Settings {
    pub fn parse(bytes: &[u8]) -> Result<Self, SettingsError> {
        let text = std::str::from_utf8(bytes).map_err(|_| SettingsError::InvalidUtf8)?;
        let document = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| SettingsError::InvalidToml)?;
        if let Some(version) = document.get("schema_version")
            && version.as_integer() != Some(1)
        {
            return Err(SettingsError::UnsupportedVersion);
        }
        let renderer = match document.get("renderer") {
            None => RendererMode::default(),
            Some(item) => {
                let table = item.as_table_like().ok_or(SettingsError::InvalidRenderer)?;
                match table.get("mode").and_then(|value| value.as_str()) {
                    None if table.get("mode").is_none() => RendererMode::default(),
                    Some("hardware") => RendererMode::Hardware,
                    Some("software") => RendererMode::Software,
                    _ => return Err(SettingsError::InvalidRenderer),
                }
            }
        };
        Ok(Self { renderer })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_accepts_toml_and_rejects_ambiguous_modes() {
        assert_eq!(
            Settings::parse(b"schema_version = 1\n[renderer]\nmode = 'software' # comment")
                .unwrap()
                .renderer,
            RendererMode::Software
        );
        for input in [
            "[renderer]\nmode = 42",
            "renderer = 'software'",
            "[renderer]\nmode = 'auto'",
            "[renderer]\nmode = 'software'\nmode = 'hardware'",
        ] {
            assert!(Settings::parse(input.as_bytes()).is_err());
        }
        assert_eq!(
            Settings::parse(b"schema_version=2"),
            Err(SettingsError::UnsupportedVersion)
        );
    }
    #[test]
    fn software_renderer_is_the_default_and_launch_flags_override_the_setting() {
        // ADR-32, PERF-02: nothing configured draws in software.
        assert_eq!(RendererMode::default(), RendererMode::Software);
        for input in ["", "schema_version = 1", "[renderer]"] {
            assert_eq!(
                Settings::parse(input.as_bytes()).unwrap().renderer,
                RendererMode::Software
            );
        }
        assert_eq!(
            Settings::parse(b"[renderer]\nmode = 'hardware'").unwrap().renderer,
            RendererMode::Hardware
        );
        let empty = SettingsDocument::empty(Scope::User);
        let resolved = resolve(&empty, None, false, None).values;
        assert_eq!(resolved.renderer, RendererMode::Software);
        assert_eq!(EffectiveSettings::default().renderer, RendererMode::Software);
        assert_eq!(
            resolved.setting_value("renderer.mode"),
            Some(SettingValue::Text("software".into()))
        );
        let mut chosen = SettingsDocument::empty(Scope::User);
        chosen
            .set("renderer.mode", SettingValue::Text("hardware".into()))
            .unwrap();
        let hardware = resolve(&chosen, None, false, None).values.renderer;
        assert_eq!(hardware, RendererMode::Hardware);
        // No flag: the setting (or its software default) applies.
        assert_eq!(
            RendererMode::select(RendererMode::Software, false, false),
            RendererMode::Software
        );
        assert_eq!(RendererMode::select(hardware, false, false), RendererMode::Hardware);
        // A flag wins over the setting in either direction.
        assert_eq!(RendererMode::select(hardware, true, false), RendererMode::Software);
        assert_eq!(
            RendererMode::select(RendererMode::Software, false, true),
            RendererMode::Hardware
        );
        assert_eq!(RendererMode::select(hardware, false, true), RendererMode::Hardware);
    }
}

mod keymap;
mod localization;
mod model;
mod persistence;
mod startup;
mod theme;
pub use keymap::*;
pub use localization::*;
pub use model::*;
pub use persistence::*;
pub use startup::*;
pub use theme::*;

#[cfg(test)]
mod behavior_tests;

/// Feature-owned stable commands, handled by the app's contributed command dispatcher.
pub fn register_commands(
    registry: &mut bareline_commands::CommandRegistry,
) -> Result<(), bareline_commands::CommandId> {
    use bareline_commands::{Action, CommandId, CommandPresentation, CommandSpec};
    for (key, title) in [
        ("settings.open", "Settings"),
        ("settings.close", "Close Settings"),
        ("settings.retry", "Retry Settings Save"),
        ("settings.revert", "Revert Settings Changes"),
        ("settings.external_reload", "Reload Changed Settings"),
        ("settings.external_keep", "Keep My Settings"),
        ("settings.reset_section", "Reset Settings Section"),
        ("settings.copy_key", "Copy Setting TOML Key"),
        ("settings.change", "Change Setting"),
    ] {
        let id = CommandId(key);
        registry.register(CommandSpec {
            id,
            title,
            category: "Settings",
            shortcut: "",
            action: Action::Contributed(id),
        })?;
        // Only the page opener belongs in menus and the palette; the rest are
        // actions the settings surface itself invokes by ID.
        if key != "settings.open" {
            registry.set_presentation(
                id,
                CommandPresentation {
                    menu_path: "Settings".into(),
                    internal: true,
                    ..Default::default()
                },
            )?;
        }
    }
    // One-step switches between the keymap presets (BIZ-08).
    for (key, title, _) in KEYMAP_PRESET_COMMANDS {
        let id = CommandId(key);
        registry.register(CommandSpec {
            id,
            title,
            category: "Settings",
            shortcut: "",
            action: Action::Contributed(id),
        })?;
        registry.set_presentation(
            id,
            CommandPresentation {
                menu_path: "Settings".into(),
                keywords: vec![
                    "keymap".into(),
                    "keyboard".into(),
                    "shortcut preset".into(),
                    "notepad++".into(),
                ],
                ..Default::default()
            },
        )?;
    }
    Ok(())
}
