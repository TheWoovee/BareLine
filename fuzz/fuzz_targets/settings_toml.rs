// SPDX-License-Identifier: MPL-2.0
//! Settings bytes never panic the parser; any accepted document reloads from its own
//! saved TOML with identical values and diagnostics; inline-editor input accepted for
//! a setting (selected by the first byte) reformats to the same value.
#![no_main]
use bareline_settings::{
    DEFINITIONS, Diagnostic, Scope, Settings, SettingsDocument, format_setting_input, parse_setting_input,
};
use libfuzzer_sys::fuzz_target;

fn sorted(mut diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diagnostics.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.message.cmp(&b.message)));
    diagnostics
}

fuzz_target!(|data: &[u8]| {
    let _ = Settings::parse(data);
    for scope in [Scope::User, Scope::Workspace, Scope::Session] {
        let Ok(document) = SettingsDocument::parse_classified(data, scope) else {
            continue;
        };
        let (values, diagnostics) = document.values();
        let saved = document.to_toml();
        let reloaded = SettingsDocument::parse(saved.as_bytes(), scope).expect("saved settings must reload");
        let (reloaded_values, reloaded_diagnostics) = reloaded.values();
        assert_eq!(reloaded_values, values);
        assert_eq!(sorted(reloaded_diagnostics), sorted(diagnostics));
    }
    let Some((&selector, input)) = data.split_first() else {
        return;
    };
    let Ok(input) = std::str::from_utf8(input) else {
        return;
    };
    let definition = &DEFINITIONS[usize::from(selector) % DEFINITIONS.len()];
    if let Ok(value) = parse_setting_input(definition.key, input) {
        let formatted = format_setting_input(&value);
        assert_eq!(parse_setting_input(definition.key, &formatted), Ok(value.clone()));
        let mut document = SettingsDocument::empty(Scope::User);
        document
            .set(definition.key, value.clone())
            .expect("accepted input must be settable");
        assert_eq!(document.values().0.get(definition.key), Some(&value));
    }
});
