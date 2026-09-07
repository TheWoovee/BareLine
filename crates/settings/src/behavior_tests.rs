// SPDX-License-Identifier: MPL-2.0
use crate::*;
use bareline_commands::{CommandId, KeyBinding, KeyChord, shell_commands};
use bareline_platform::{FileIdentity, LocalFileSystem};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
fn parse(text: &str, scope: Scope) -> SettingsDocument {
    SettingsDocument::parse(text.as_bytes(), scope).unwrap()
}
#[test]
fn resource_quotas_are_bounded_user_preferences_and_preserve_other_values() {
    let user = parse(
        "[document]\nresident_max_bytes=4096\n[transcode]\ntemp_quota_bytes=21474836480\n",
        Scope::User,
    );
    let workspace = parse(
        "[document]\nresident_max_bytes=8192\n[transcode]\ntemp_quota_bytes=8192\n",
        Scope::Workspace,
    );
    let resolved = resolve(&user, Some(&workspace), true, None);
    assert_eq!(resolved.values.resident_max_bytes, 4096);
    assert_eq!(resolved.values.transcode_quota_bytes, 21_474_836_480);
    let invalid = parse(
        "[document]\nresident_max_bytes=-1\n[editor.tab]\nwidth=8\n",
        Scope::User,
    );
    let resolved = resolve(&invalid, None, false, None);
    assert_eq!(resolved.values.resident_max_bytes, 268_435_456);
    assert_eq!(resolved.values.tab_width, 8);
    assert!(!resolved.diagnostics.is_empty());
}
#[test]
fn toolbar_order_roundtrips_and_invalid_edits_preserve_document() {
    let mut document = SettingsDocument::empty(Scope::User);
    let commands = vec!["file.open".into(), "file.new".into()];
    document
        .set("toolbar.commands", SettingValue::Strings(commands.clone()))
        .unwrap();
    let reloaded = SettingsDocument::parse(document.to_toml().as_bytes(), Scope::User).unwrap();
    assert_eq!(
        resolve(&reloaded, None, false, None)
            .values
            .toolbar_commands,
        commands
    );
    let original = document.to_toml();
    assert!(
        document
            .set(
                "toolbar.commands",
                SettingValue::Strings(vec!["file.open".into(), "file.open".into()])
            )
            .is_err()
    );
    assert_eq!(document.to_toml(), original);
}
#[test]
fn single_bad_values_do_not_reset_other_settings_and_unknown_comments_survive() {
    let mut doc = parse(
        "# profile\nfuture = 'retained'\n[editor]\nfont_size_pt = 14.0 # size\ntab_width = 'bad'\nword_wrap = true\n",
        Scope::User,
    );
    let resolved = resolve(&doc, None, false, None);
    assert_eq!(resolved.values.editor_font_size_pt, 14.0);
    assert_eq!(resolved.values.tab_width, 4);
    assert!(resolved.values.word_wrap);
    assert_eq!(resolved.diagnostics.len(), 1);
    doc.set("editor.font_size_pt", SettingValue::Number(12.0))
        .unwrap();
    let text = doc.to_toml();
    assert!(text.contains("# profile"));
    assert!(text.contains("# size"));
    assert!(text.contains("future = 'retained'"));
    let original = text.clone();
    assert!(
        doc.set("editor.font_size_pt", SettingValue::Number(f64::NAN))
            .is_err()
    );
    assert_eq!(doc.to_toml(), original);
}
#[test]
fn opt_in_workspace_cannot_override_policy_even_when_mislabeled_as_user() {
    let user = parse("[editor]\ntab_width=8\n[theme]\nmode='dark'", Scope::User);
    let workspace = parse(
        "[editor]\ntab_width=2\ninsert_spaces=false\n[network]\nallow=true\n[process]\ncommand='evil'\n[extensions]\ngrants=['all']\n[theme]\nmode='light'",
        Scope::User,
    );
    let ignored = resolve(&user, Some(&workspace), false, None);
    assert_eq!(ignored.values.tab_width, 8);
    assert!(ignored.diagnostics.is_empty());
    let allowed = resolve(&user, Some(&workspace), true, None);
    assert_eq!(allowed.values.tab_width, 2);
    assert!(!allowed.values.insert_spaces);
    assert_eq!(allowed.values.theme, ThemeMode::Dark);
    assert_eq!(allowed.diagnostics.len(), 4);
    let mut scoped = parse("", Scope::Workspace);
    assert!(
        scoped
            .set("renderer.mode", SettingValue::Text("software".into()))
            .is_err()
    );
    scoped
        .set("editor.tab_width", SettingValue::Integer(3))
        .unwrap();
}
#[test]
fn session_layer_has_precedence_and_invalid_workspace_falls_back_per_key() {
    let user = parse("[editor]\nfont_size_pt=15\ntab_width=8", Scope::User);
    let workspace = parse("[editor]\nfont_size_pt=100\ntab_width=2", Scope::Workspace);
    let session = parse("[editor]\ntab_width=6", Scope::Session);
    let result = resolve(&user, Some(&workspace), true, Some(&session));
    assert_eq!(result.values.editor_font_size_pt, 15.0);
    assert_eq!(result.values.tab_width, 6);
    assert_eq!(result.diagnostics.len(), 1);
}
#[test]
fn migration_and_font_points_remain_stable_at_mixed_dpi() {
    let migrated = parse(
        "schema_version=0\nfuture='keep'\n[editor]\nfont_size_px=16",
        Scope::User,
    );
    let restored = parse(&migrated.to_toml(), Scope::User);
    assert!(restored.to_toml().contains("future='keep'"));
    let settings = resolve(&restored, None, false, None).values;
    assert_eq!(settings.editor_font_size_pt, 12.0);
    for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
        assert_eq!(
            pt_to_physical_px(settings.editor_font_size_pt, scale).unwrap(),
            16.0 * scale
        );
    }
    assert!(pt_to_physical_px(12.0, f64::INFINITY).is_err());
    assert!(SettingsDocument::parse(b"schema_version=2", Scope::User).is_err());
    assert!(SettingsDocument::parse(b"[broken", Scope::User).is_err());
}
#[test]
fn reset_targets_scope_and_keeps_unrelated_values() {
    let mut doc = parse(
        "[editor]\nfont_size_pt=14\nfuture=42\n[theme]\nmode='dark'",
        Scope::User,
    );
    let keys = doc.reset_section("Editor");
    assert!(keys.contains(&"editor.font.size"));
    assert!(doc.to_toml().contains("future=42"));
    assert_eq!(
        resolve(&doc, None, false, None).values.theme,
        ThemeMode::Dark
    );
    assert_eq!(search_definitions("font points")[0].key, "editor.font.size");
}
#[test]
fn built_in_themes_and_high_contrast_meet_functional_ratios() {
    for dark in [false, true] {
        for high_contrast in [false, true] {
            let theme = Theme::resolve(
                ThemeMode::System,
                SystemAppearance {
                    dark,
                    high_contrast,
                },
                &BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(theme.dark, dark);
            assert_eq!(theme.high_contrast, high_contrast);
            for token in TOKEN_NAMES {
                assert!(theme.color(token).is_some(), "{token}");
            }
        }
    }
    let theme = Theme::resolve(
        ThemeMode::Light,
        SystemAppearance {
            dark: true,
            high_contrast: false,
        },
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(!theme.dark);
}
#[test]
fn theme_override_persistence_and_composited_contrast_rejection() {
    let mut doc = SettingsDocument::empty(Scope::User);
    doc.set("theme.mode", SettingValue::Text("light".into()))
        .unwrap();
    doc.set(
        "theme.overrides",
        SettingValue::Map(BTreeMap::from([("text".into(), "#202020".into())])),
    )
    .unwrap();
    let restored = parse(&doc.to_toml(), Scope::User);
    let effective = resolve(&restored, None, false, None).values;
    let theme = Theme::resolve(
        effective.theme,
        SystemAppearance::default(),
        &effective.theme_overrides,
    )
    .unwrap();
    assert_eq!(theme.color("text").unwrap().rgb, 0x202020);
    let bad = BTreeMap::from([("text".into(), "#FFFFFF".into())]);
    assert!(Theme::resolve(ThemeMode::Light, SystemAppearance::default(), &bad).is_err());
    let bad = BTreeMap::from([("selection".into(), "#23272BFF".into())]);
    assert!(Theme::resolve(ThemeMode::Light, SystemAppearance::default(), &bad).is_err());
    assert!(ThemeColor::parse("#xxxxxx").is_err());
    assert_eq!(
        ThemeColor::opaque(0xffffff).contrast(ThemeColor::opaque(0)),
        21.0
    );
}
#[test]
fn locale_switch_is_data_only_parameterized_and_falls_back_per_message() {
    let pack=LocalePack::parse("version=1\nlocale='ar'\ndirection='rtl'\n[messages]\n'settings.reset'='إعدادات {scope}: {section}؟'\n".as_bytes()).unwrap();
    let mut localizer = Localizer::default();
    let change = localizer.switch(pack).unwrap();
    assert!(change.rebuild_native_menus);
    assert!(!change.restart_required);
    assert_eq!(localizer.direction(), TextDirection::RightToLeft);
    assert_eq!(
        localizer.format("settings.saved", &[]).unwrap(),
        "All changes saved"
    );
    assert_eq!(
        localizer
            .format(
                "settings.reset",
                &[("scope", "User"), ("section", "Editor")]
            )
            .unwrap(),
        "إعدادات User: Editor؟"
    );
    assert!(localizer.format("settings.reset", &[]).is_err());
    let bad = LocalePack::parse(
        b"version=1\nlocale='de'\ndirection='ltr'\n[messages]\n'settings.reset'='{execute}'",
    )
    .unwrap();
    assert!(localizer.switch(bad).is_err());
    assert_eq!(localizer.locale(), "ar");
    assert!(
        LocalePack::parse(
            b"version=1\nlocale='de'\ndirection='ltr'\n[messages]\n'settings.reset'='{execute()}'"
        )
        .is_err()
    );
}
#[test]
fn keymap_edits_preserve_comments_and_conflicts_leave_previous_state() {
    let registry = shell_commands();
    let mut doc=KeymapDocument::parse("# shortcut profile\nversion=1\n[[bindings]]\ncommand='file.new'\nkeys=['Ctrl+N'] # new\n[[bindings]]\ncommand='file.open'\nkeys=['Ctrl+O'] # open\n",&registry).unwrap();
    doc.set_binding(
        KeyBinding {
            command: CommandId("file.new"),
            sequence: vec![KeyChord::parse("Ctrl+J").unwrap()],
        },
        &registry,
    )
    .unwrap();
    let text = doc.to_toml();
    for comment in ["# shortcut profile", "# new", "# open"] {
        assert!(text.contains(comment));
    }
    assert!(
        doc.set_binding(
            KeyBinding {
                command: CommandId("file.new"),
                sequence: vec![KeyChord::parse("Ctrl+O").unwrap()]
            },
            &registry
        )
        .is_err()
    );
    assert_eq!(doc.to_toml(), text);
    assert!(doc.import("version=99", &registry).is_err());
    assert_eq!(doc.to_toml(), text);
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bareline-settings-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct TestFs {
    reject: bool,
}
impl LocalFileSystem for TestFs {
    fn identity(&self, _: &fs::File) -> io::Result<FileIdentity> {
        Err(io::ErrorKind::Unsupported.into())
    }
    fn validate_target(&self, _: &Path) -> io::Result<()> {
        Ok(())
    }
    fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
        if self.reject {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        fs::rename(staged, target)
    }
}
#[test]
fn failed_atomic_commit_preserves_file_and_allows_retry() {
    let fixture = Fixture::new();
    let path = fixture.0.join("settings.toml");
    fs::write(&path, "# existing\n[editor]\nfont_size_pt=11\n").unwrap();
    let mut editor = SettingsEditor::new(SettingsDocument::load(&path, Scope::User).unwrap());
    editor
        .set("editor.font_size_pt", SettingValue::Number(12.0))
        .unwrap();
    assert!(editor.save(&path, &TestFs { reject: true }).is_err());
    assert!(matches!(editor.status, SaveStatus::Failed(_)));
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("font_size_pt=11")
    );
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    editor.save(&path, &TestFs { reject: false }).unwrap();
    assert_eq!(editor.status, SaveStatus::Saved);
    assert_eq!(
        resolve(
            &SettingsDocument::load(&path, Scope::User).unwrap(),
            None,
            false,
            None
        )
        .values
        .editor_font_size_pt,
        12.0
    );
    editor
        .set("editor.font_size_pt", SettingValue::Number(20.0))
        .unwrap();
    editor.revert();
    assert_eq!(
        resolve(&editor.document, None, false, None)
            .values
            .editor_font_size_pt,
        12.0
    );
}
#[test]
fn bounded_storage_and_session_persistence_rejections() {
    let fixture = Fixture::new();
    let path = fixture.0.join("oversized.toml");
    fs::write(&path, vec![b' '; MAX_CONFIG_BYTES + 1]).unwrap();
    assert_eq!(
        read_config(&path).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(
        SettingsDocument::empty(Scope::Session)
            .save(&path, &TestFs { reject: false })
            .is_err()
    );
    assert!(
        atomic_write_config(
            &fixture.0.join("missing/settings.toml"),
            b"schema_version=1",
            &TestFs { reject: false }
        )
        .is_err()
    );
}

#[test]
fn keymap_default_create_and_failed_rebind_preserve_existing_bytes() {
    let fixture = Fixture::new();
    let path = fixture.0.join("keymap.toml");
    let registry = shell_commands();
    let original = KeymapDocument::defaults(&registry);
    atomic_create_config(
        &path,
        original.to_toml().as_bytes(),
        &TestFs { reject: false },
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    atomic_create_config(&path, b"not a replacement", &TestFs { reject: true }).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut changed = original.clone();
    changed
        .set_binding(
            KeyBinding {
                command: CommandId("file.save"),
                sequence: vec![
                    KeyChord::parse("Ctrl+K").unwrap(),
                    KeyChord::parse("Ctrl+S").unwrap(),
                ],
            },
            &registry,
        )
        .unwrap();
    assert!(changed.save(&path, &TestFs { reject: true }).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
}
