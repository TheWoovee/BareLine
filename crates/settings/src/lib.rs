// SPDX-License-Identifier: MPL-2.0
//! User configuration only; workspace policy belongs to PR-015.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RendererMode {
    #[default]
    Hardware,
    Software,
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
            None => RendererMode::Hardware,
            Some(item) => {
                let table = item.as_table_like().ok_or(SettingsError::InvalidRenderer)?;
                match table.get("mode").and_then(|value| value.as_str()) {
                    None if table.get("mode").is_none() => RendererMode::Hardware,
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
}

mod keymap;
mod localization;
mod model;
mod persistence;
mod theme;
pub use keymap::*;
pub use localization::*;
pub use model::*;
pub use persistence::*;
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
    Ok(())
}
