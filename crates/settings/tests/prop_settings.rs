// SPDX-License-Identifier: MPL-2.0
//! Seeded property tests (QA-05): settings TOML parsing never panics on arbitrary bytes,
//! accepted documents survive a save/reload cycle, and valid values round-trip through
//! the document model and the inline value editor.
use bareline_settings::{
    DEFINITIONS, Diagnostic, Scope, SettingDefinition, SettingKind, SettingValue, Settings, SettingsDocument,
    TOKEN_NAMES, format_setting_input, parse_setting_input,
};
use std::collections::BTreeMap;

/// SplitMix64. Report the seed and case of a failure to reproduce it exactly.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    fn text(&mut self, pool: &[&str], max: usize) -> String {
        (0..1 + self.below(max)).map(|_| *self.pick(pool)).collect()
    }
}

/// No control characters, as required for Text and Font settings.
const TEXT: &[&str] = &[
    "a", "Z", " ", "\"", "'", "\\", "#", "=", "[", "]", ".", "é", "中", "😀", "\u{feff}", "\u{2028}",
];
/// Array and map payloads may contain escaped control characters, but never NUL.
const PAYLOAD: &[&str] = &[
    "a", " ", "\"", "'''", "\\", "\n", "\t", "\r\n", "\u{7f}", "é", "😀", "*.rs", "$",
];
const POLICIES: &[(&str, &[&str])] = &[
    ("lexer", &["primary", "native"]),
    ("min_chars", &["0", "3", "16"]),
    ("completion", &["true", "false"]),
    ("include_open_documents", &["true", "false"]),
    ("smart_pairs", &["true", "false"]),
    ("smart_indent", &["true", "false"]),
    ("parameter_hints", &["true", "false"]),
];

fn integer(rng: &mut Rng, min: i64, max: i64) -> i64 {
    match rng.below(4) {
        0 => min,
        1 => max,
        _ => min + (rng.next_u64() % ((max - min) as u64 + 1)) as i64,
    }
}
fn valid_value(rng: &mut Rng, definition: &SettingDefinition) -> SettingValue {
    match definition.key {
        "language.locale" => SettingValue::Text((*rng.pick(&["en", "en-US", "fr-CA", "zh-Hans-CN"])).into()),
        "language.policies" => SettingValue::Map(
            (0..rng.below(4))
                .map(|_| {
                    let (field, values) = *rng.pick(POLICIES);
                    let language = *rng.pick(&["rust", "toml", "c_cpp", "plain-text"]);
                    (format!("{language}.{field}"), (*rng.pick(values)).to_owned())
                })
                .collect(),
        ),
        "theme.overrides" => SettingValue::Map(
            (0..rng.below(4))
                .map(|_| {
                    let color = rng.next_u64() as u32;
                    let color = if rng.chance(50) {
                        format!("#{:06X}", color & 0x00ff_ffff)
                    } else {
                        format!("#{color:08x}")
                    };
                    ((*rng.pick(TOKEN_NAMES)).to_owned(), color)
                })
                .collect(),
        ),
        "toolbar.commands" => {
            let mut ids = vec![
                "file.new",
                "file.open",
                "file.save",
                "edit.undo",
                "search.find",
                "view_zoom-in",
            ];
            ids.truncate(rng.below(ids.len() + 1));
            SettingValue::Strings(ids.into_iter().map(str::to_owned).collect())
        }
        _ => match definition.kind {
            SettingKind::Boolean => SettingValue::Bool(rng.chance(50)),
            SettingKind::Integer(min, max) | SettingKind::Bytes(min, max) => {
                SettingValue::Integer(integer(rng, min, max))
            }
            SettingKind::Number(min, max) => {
                SettingValue::Number(min + (max - min) * (rng.below(1001) as f64 / 1000.0))
            }
            SettingKind::Text | SettingKind::Font => SettingValue::Text(rng.text(TEXT, 40)),
            SettingKind::Choice(choices) => SettingValue::Text((*rng.pick(choices)).to_owned()),
            SettingKind::Strings => SettingValue::Strings((0..rng.below(4)).map(|_| rng.text(PAYLOAD, 8)).collect()),
            SettingKind::Map => SettingValue::Map(
                (0..rng.below(4))
                    .map(|_| (rng.text(PAYLOAD, 6), rng.text(PAYLOAD, 8)))
                    .collect(),
            ),
        },
    }
}
fn sorted(mut diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diagnostics.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.message.cmp(&b.message)));
    diagnostics
}
fn utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let mut bytes = if little_endian {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

#[test]
fn valid_settings_round_trip_through_toml_and_every_config_encoding() {
    let mut rng = Rng(0x5e77_0001);
    for case in 0..60 {
        let mut document = SettingsDocument::empty(Scope::User);
        let mut expected: BTreeMap<String, SettingValue> = BTreeMap::new();
        for _ in 0..1 + rng.below(30) {
            if rng.chance(8) {
                let category = rng.pick(DEFINITIONS).category;
                for key in document.reset_section(category) {
                    expected.remove(key);
                }
                continue;
            }
            let definition = rng.pick(DEFINITIONS);
            let value = valid_value(&mut rng, definition);
            document
                .set(definition.key, value.clone())
                .unwrap_or_else(|error| panic!("case {case}: {} = {value:?}: {error}", definition.key));
            expected.insert(definition.key.to_owned(), value);
        }
        let toml = document.to_toml();
        let context = format!("case {case}:\n{toml}");
        let (values, diagnostics) = document.values();
        assert!(diagnostics.is_empty(), "{context}: {diagnostics:?}");
        assert_eq!(values, expected, "{context}: in-memory values");
        let encodings = [
            toml.clone().into_bytes(),
            [&[0xef, 0xbb, 0xbf][..], toml.as_bytes()].concat(),
            utf16(&toml, true),
            utf16(&toml, false),
        ];
        for bytes in encodings {
            let reloaded =
                SettingsDocument::parse(&bytes, Scope::User).unwrap_or_else(|error| panic!("{context}: {error}"));
            assert_eq!(
                reloaded.values(),
                (expected.clone(), Vec::new()),
                "{context}: reloaded values"
            );
        }
        // Workspace scope keeps allowlisted keys only and reports every other one.
        let workspace = SettingsDocument::parse(toml.as_bytes(), Scope::Workspace).unwrap();
        let (values, diagnostics) = workspace.values();
        for (key, value) in &expected {
            let allowed = DEFINITIONS
                .iter()
                .any(|definition| definition.key == key && definition.workspace_allowed);
            if allowed {
                assert_eq!(values.get(key), Some(value), "{context}: workspace {key}");
            } else {
                assert!(!values.contains_key(key), "{context}: workspace granted {key}");
                assert!(
                    diagnostics.iter().any(|diagnostic| diagnostic.key == *key),
                    "{context}: workspace did not report {key}"
                );
            }
        }
    }
}

#[test]
fn inline_editor_round_trips_every_accepted_value() {
    let mut rng = Rng(0x5e77_0002);
    let inputs = [
        "",
        " ",
        "true",
        "false",
        "0",
        "1",
        "-1",
        "16",
        "4096",
        "64 MB",
        "1.5 GB",
        "12.5",
        "6",
        "72",
        "nan",
        "inf",
        "1e1",
        "0x10",
        "1_000",
        "\"text\"",
        "text",
        "off",
        "viewport",
        "dark",
        "[\"a\", 'b']",
        "[]",
        "{ a = \"b\" }",
        "{}",
        "5 # comment",
        "5\nx = 1",
        "[1, 2]",
        "{ \"rust.lexer\" = \"native\" }",
        "#112233",
    ];
    for definition in DEFINITIONS {
        for case in 0..inputs.len() + 8 {
            let input = match inputs.get(case) {
                Some(input) => (*input).to_owned(),
                None => format_setting_input(&valid_value(&mut rng, definition)),
            };
            let context = format!("{} input {input:?}", definition.key);
            let Ok(value) = parse_setting_input(definition.key, &input) else {
                assert!(case < inputs.len(), "{context}: formatted valid value was rejected");
                continue;
            };
            let formatted = format_setting_input(&value);
            assert_eq!(
                parse_setting_input(definition.key, &formatted),
                Ok(value.clone()),
                "{context}: reformatted as {formatted:?}"
            );
            let mut document = SettingsDocument::empty(Scope::User);
            document
                .set(definition.key, value.clone())
                .unwrap_or_else(|error| panic!("{context}: accepted input rejected by set: {error}"));
            assert_eq!(document.values().0.get(definition.key), Some(&value), "{context}");
        }
    }
}

const HEADERS: &[&str] = &[
    "[editor]",
    "[editor.font]",
    "[theme]",
    "[theme.overrides]",
    "[renderer]",
    "[language]",
    "[language.policies]",
    "[toolbar]",
    "[search]",
    "[[editor]]",
    "[files]",
];
const KEYS: &[&str] = &[
    "schema_version",
    "size",
    "family",
    "mode",
    "font_size_px",
    "font_size_pt",
    "tab_width",
    "word_wrap",
    "commands",
    "excludes",
    "\"rust.lexer\"",
    "editor.font.size",
    "editor.font_size_px",
    "editor",
    "font",
    "renderer.mode",
    "'quoted key'",
    "background",
];
const VALUES: &[&str] = &[
    "0",
    "1",
    "2",
    "-1",
    "12.5",
    "nan",
    "-inf",
    "true",
    "\"text\"",
    "'lit'",
    "[\"a\", 'b']",
    "{ a = \"b\" }",
    "[]",
    "{}",
    "1979-05-27",
    "0x10",
    "\"\"\"multi\nline\"\"\"",
    "\"#112233\"",
    "\"software\"",
];
/// TOML-shaped text (headers, dotted keys, inline tables, schema versions 0/1/2) with
/// optional byte mutations, so parsing reaches migration and value validation.
fn toml_like(rng: &mut Rng) -> Vec<u8> {
    let mut text = String::new();
    if rng.chance(40) {
        text.push_str(rng.pick(&["schema_version = 0\n", "schema_version = 1\n", "schema_version = 2\n"]));
    }
    for _ in 0..rng.below(10) {
        match rng.below(5) {
            0 => text.push_str(rng.pick(HEADERS)),
            1 => text.push_str(rng.pick(&["# comment", "", "=", "\u{feff}", "key ="])),
            _ => {
                text.push_str(rng.pick(KEYS));
                text.push_str(" = ");
                text.push_str(rng.pick(VALUES));
            }
        }
        text.push('\n');
    }
    let mut bytes = text.into_bytes();
    for _ in 0..rng.below(3) {
        if bytes.is_empty() {
            break;
        }
        let at = rng.below(bytes.len());
        match rng.below(3) {
            0 => bytes[at] = rng.next_u64() as u8,
            1 => {
                bytes.remove(at);
            }
            _ => bytes.insert(at, *rng.pick(b"[]=\"'.\n{},")),
        }
    }
    match rng.below(10) {
        0 => [&[0xef, 0xbb, 0xbf][..], bytes.as_slice()].concat(),
        1 => String::from_utf8(bytes.clone()).map_or(bytes, |text| utf16(&text, rng.chance(50))),
        _ => bytes,
    }
}

#[test]
fn arbitrary_bytes_never_panic_and_accepted_documents_reload_identically() {
    let mut rng = Rng(0x5e77_0003);
    let mut accepted = 0;
    for case in 0..800 {
        let bytes = if rng.chance(30) {
            (0..rng.below(96)).map(|_| rng.next_u64() as u8).collect()
        } else {
            toml_like(&mut rng)
        };
        let _ = Settings::parse(&bytes);
        for scope in [Scope::User, Scope::Workspace, Scope::Session] {
            let Ok(document) = SettingsDocument::parse_classified(&bytes, scope) else {
                continue;
            };
            accepted += 1;
            let (values, diagnostics) = document.values();
            let saved = document.to_toml();
            let context = format!("case {case} {scope:?} input {bytes:02x?}\nsaved:\n{saved}");
            let reloaded = SettingsDocument::parse(saved.as_bytes(), scope)
                .unwrap_or_else(|error| panic!("{context}: saved settings do not reload: {error}"));
            let (reloaded_values, reloaded_diagnostics) = reloaded.values();
            assert_eq!(reloaded_values, values, "{context}: values changed on reload");
            assert_eq!(
                sorted(reloaded_diagnostics),
                sorted(diagnostics),
                "{context}: diagnostics changed on reload"
            );
        }
    }
    assert!(
        accepted > 30,
        "generator produced too few parseable documents: {accepted}"
    );
}
