// SPDX-License-Identifier: MPL-2.0
//! System preferences the shell reads: contrast, the user's language, the
//! legacy code page and spell checking.
use std::{io, sync::Arc};

pub fn high_contrast_enabled() -> io::Result<bool> {
    Ok(false)
}
pub fn high_contrast_highlight() -> Option<(u32, u32)> {
    None
}
/// The user's language from the POSIX locale variables as a BCP 47 name such as
/// `de-DE`. `None` for the C/POSIX locale or when nothing is set.
pub fn system_ui_language() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty())
        .and_then(|value| locale_name(value.to_str()?))
}
/// `de_DE.UTF-8` or `sr_RS@latin` to `de-DE` or `sr-RS`.
fn locale_name(value: &str) -> Option<String> {
    let name = value.split(['.', '@']).next().unwrap_or_default().replace('_', "-");
    (!name.is_empty() && name != "C" && name != "POSIX").then_some(name)
}
/// The code page legacy text falls back to: UTF-8 on these systems.
pub fn system_code_page() -> u32 {
    65001
}
pub fn spell_checker_factory() -> bareline_platform::spelling::SpellCheckerFactory {
    Arc::new(|| Err("This system does not support spell checking yet".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_names_become_language_tags() {
        assert_eq!(locale_name("de_DE.UTF-8").as_deref(), Some("de-DE"));
        assert_eq!(locale_name("sr_RS@latin").as_deref(), Some("sr-RS"));
        assert_eq!(locale_name("en").as_deref(), Some("en"));
        assert_eq!(locale_name("C.UTF-8"), None);
        assert_eq!(locale_name("POSIX"), None);
        assert_eq!(locale_name(""), None);
    }
}
