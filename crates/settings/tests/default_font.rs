// SPDX-License-Identifier: MPL-2.0
//! The default editor font comes from the running system (LNX-UI-003). It is
//! process-wide state, so it is exercised in its own test binary.
use bareline_settings::{
    EffectiveSettings, Scope, SettingValue, SettingsDocument, default_font_family, resolve, set_default_font_family,
};

#[test]
fn the_default_editor_font_is_the_one_the_system_draws() {
    // Before the shell names one, the default is Windows's Cascadia Mono.
    assert_eq!(default_font_family(), "Cascadia Mono");
    // Linux: the software renderer's resolved monospace face.
    assert!(set_default_font_family(Some("DejaVu Sans Mono".into())));
    assert!(!set_default_font_family(Some("DejaVu Sans Mono".into())));
    assert_eq!(EffectiveSettings::default().editor_font_family, "DejaVu Sans Mono");
    let empty = SettingsDocument::empty(Scope::User);
    let values = resolve(&empty, None, false, None).values;
    assert_eq!(values.editor_font_family, "DejaVu Sans Mono");
    assert_eq!(
        values.setting_value("editor.font.family"),
        Some(SettingValue::Text("DejaVu Sans Mono".into()))
    );
    // A family the user chose wins over the system's.
    let mut user = SettingsDocument::empty(Scope::User);
    user.set("editor.font.family", SettingValue::Text("Consolas".into()))
        .unwrap();
    assert_eq!(resolve(&user, None, false, None).values.editor_font_family, "Consolas");
    // A blank name is no name.
    assert!(set_default_font_family(Some("  ".into())));
    assert_eq!(default_font_family(), "Cascadia Mono");
}
