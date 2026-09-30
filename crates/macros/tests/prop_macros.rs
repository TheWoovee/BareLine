// SPDX-License-Identifier: MPL-2.0
//! Seeded property tests (QA-05): macro and external-command imports never panic on
//! arbitrary text, exports re-import losslessly, and placeholder expansion and output
//! links follow their literal contracts.
use bareline_macros::{
    Macro, MacroEvent,
    process::{
        ExternalDefinition, OutputLink, PlaceholderContext, PlaceholderSafety, expand_argument_for, parse_output_link,
    },
};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

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
        (0..self.below(max + 1)).map(|_| *self.pick(pool)).collect()
    }
}

const COMMANDS: &[&str] = &[
    "file.new",
    "file.open",
    "file.save",
    "edit.copy",
    "edit.paste",
    "search.find",
];
/// Printable text for names; payloads add every escape the exporter must emit.
const PRINTABLE: &[&str] = &[
    "a", "Z", " ", "\"", "'", "\\", "#", "=", "[", "]", "{", "}", ",", "é", "中", "😀", "\u{feff}",
];
const PAYLOAD: &[&str] = &[
    "a", " ", "\"", "'''", "\\", "\\u0041", "\n", "\r", "\t", "\u{1}", "\u{7f}", "\u{85}", "\u{2028}", "é", "😀", "=",
];

fn random_macro(rng: &mut Rng) -> Macro {
    let mut name = rng.text(PRINTABLE, 12);
    if name.trim().is_empty() {
        name.push('m');
    }
    let events = (0..1 + rng.below(6))
        .map(|_| {
            let id = (*rng.pick(COMMANDS)).to_owned();
            if rng.chance(50) {
                let arguments: BTreeMap<String, String> = (0..rng.below(4))
                    .map(|_| (rng.text(PAYLOAD, 4), rng.text(PAYLOAD, 8)))
                    .collect();
                MacroEvent::Command { id, arguments }
            } else {
                let mut text = rng.text(PAYLOAD, 8);
                if text.is_empty() {
                    text.push('t');
                }
                let interval_ms = *rng.pick(&[0, 1, 30, 60_000]);
                MacroEvent::TypeText { id, text, interval_ms }
            }
        })
        .collect();
    Macro { name, events }
}

#[test]
fn exported_macros_reimport_losslessly() {
    let registry = bareline_commands::shell_commands();
    let mut rng = Rng(0x3ac0_0001);
    for case in 0..300 {
        let original = random_macro(&mut rng);
        original
            .validate(&registry)
            .unwrap_or_else(|error| panic!("case {case}: generator produced invalid macro: {error}"));
        let exported = original.export_toml();
        let imported = Macro::import_toml(&exported, &registry)
            .unwrap_or_else(|error| panic!("case {case}: {exported}\nre-import failed: {error}"));
        assert_eq!(imported, original, "case {case}: {exported}");
        assert_eq!(
            imported.export_toml(),
            exported,
            "case {case}: export is not a fixed point"
        );
    }
}

#[test]
fn external_definitions_reimport_losslessly_and_never_gain_fields() {
    let mut rng = Rng(0x3ac0_0002);
    // Absolute on every platform, so the import's absolute-program rule holds.
    let program = std::env::temp_dir().join("tool").to_string_lossy().into_owned();
    for case in 0..200 {
        let mut name = rng.text(PRINTABLE, 12);
        if name.trim().is_empty() {
            name.push('n');
        }
        let original = ExternalDefinition {
            name,
            program: program.clone(),
            arguments: (0..rng.below(5)).map(|_| rng.text(PAYLOAD, 8)).collect(),
            shell: rng.chance(50),
            capture: rng.chance(50),
        };
        let exported = original.export_toml();
        let imported = ExternalDefinition::import_toml(&exported)
            .unwrap_or_else(|error| panic!("case {case}: {exported}\nre-import failed: {error}"));
        assert_eq!(imported, original, "case {case}");
        let granted = format!("{exported}allow = true\n");
        assert!(
            ExternalDefinition::import_toml(&granted).is_err(),
            "case {case}: unknown field accepted"
        );
    }
}

const TOML_PIECES: &[&str] = &[
    "format_version = 1\n",
    "format_version = 2\n",
    "name = \"n\"\n",
    "name = ''\n",
    "program = \"/bin/tool\"\n",
    "program = 'C:\\tool.exe'\n",
    "args = [\"${file}\", '${selection}']\n",
    "mode = 'shell'\n",
    "capture = false\n",
    "[[events]]\n",
    "kind = 'command'\n",
    "kind = 'type_text'\n",
    "command = \"file.new\"\n",
    "command = \"no.such\"\n",
    "arguments = { a = \"b\", \"\" = \"\\u0000\" }\n",
    "arguments = { a = 1 }\n",
    "text = \"hi\"\n",
    "text = ''\n",
    "interval_ms = 5\n",
    "interval_ms = -1\n",
    "interval_ms = 60001\n",
    "events = []\n",
    "extra = true\n",
    "[events]\n",
    "\"\"\"\n",
    "{",
    "]\n",
];

#[test]
fn arbitrary_text_imports_never_panic_and_accepted_macros_validate() {
    let registry = bareline_commands::shell_commands();
    let mut rng = Rng(0x3ac0_0003);
    let mut imported = 0;
    for case in 0..1500 {
        let text = match rng.below(3) {
            0 => String::from_utf8_lossy(&(0..rng.below(80)).map(|_| rng.next_u64() as u8).collect::<Vec<_>>())
                .into_owned(),
            1 => rng.text(TOML_PIECES, 14),
            _ => {
                // A valid export, sometimes with one structural mutation.
                let mut text = random_macro(&mut rng).export_toml();
                let at = rng.below(text.len() + 1);
                if rng.chance(40) && text.is_char_boundary(at) {
                    text.insert_str(at, rng.pick(TOML_PIECES));
                }
                text
            }
        };
        if let Ok(value) = Macro::import_toml(&text, &registry) {
            imported += 1;
            value
                .validate(&registry)
                .unwrap_or_else(|error| panic!("case {case}: accepted invalid macro: {error}\n{text}"));
            let again = Macro::import_toml(&value.export_toml(), &registry);
            assert_eq!(
                again.as_ref(),
                Ok(&value),
                "case {case}: accepted macro does not round-trip\n{text}"
            );
        }
        if let Ok(value) = ExternalDefinition::import_toml(&text) {
            assert!(!value.name.trim().is_empty() && PathBuf::from(&value.program).is_absolute());
            assert_eq!(
                ExternalDefinition::import_toml(&value.export_toml()),
                Ok(value),
                "case {case}"
            );
        }
        let _ = parse_output_link(&text);
    }
    assert!(
        imported > 100,
        "generator produced too few importable macros: {imported}"
    );
}

#[test]
fn argument_placeholders_expand_literally_and_output_links_parse_exactly() {
    let mut rng = Rng(0x3ac0_0004);
    let file = std::env::temp_dir().join("dir").join("file & name.txt");
    for case in 0..500 {
        let selection = rng.text(PAYLOAD, 6);
        let context = PlaceholderContext {
            file: Some(file.clone()),
            workspace: None,
            selection: selection.clone(),
            line: rng.next_u64() % 1000,
            column: rng.next_u64() % 1000,
        };
        let mut template = String::new();
        let mut expected = OsString::new();
        for _ in 0..rng.below(6) {
            match rng.below(5) {
                0 => {
                    template.push_str("${selection}");
                    expected.push(&selection);
                }
                1 => {
                    template.push_str("${line}");
                    expected.push(context.line.to_string());
                }
                2 => {
                    template.push_str("${file}");
                    expected.push(&file);
                }
                3 => {
                    template.push_str("${column}");
                    expected.push(context.column.to_string());
                }
                _ => {
                    // Literal text never forms `${`, even next to the following piece.
                    let mut literal = rng
                        .text(&["a", " ", "$", "{", "}", "\"", "&", "é"], 4)
                        .replace("${", "$ {");
                    if literal.ends_with('$') {
                        literal.push('.');
                    }
                    template.push_str(&literal);
                    expected.push(literal);
                }
            }
        }
        assert_eq!(
            expand_argument_for(&template, &context, PlaceholderSafety::Argument),
            Ok(expected),
            "case {case}: {template:?}"
        );
        let _ = expand_argument_for(&template, &context, PlaceholderSafety::CommandShell);
        assert!(expand_argument_for("${workspace}", &context, PlaceholderSafety::Argument).is_err());
        assert!(expand_argument_for("${unknown}", &context, PlaceholderSafety::Argument).is_err());
        assert!(expand_argument_for("${file", &context, PlaceholderSafety::Argument).is_err());

        let path = format!(
            "{}{}",
            rng.text(&["a", "b", ":", "\\", "/", " ", "é"], 6),
            rng.pick(&["x", "y.rs"])
        );
        let (line, column) = (1 + rng.next_u64() % 10_000, 1 + rng.next_u64() % 500);
        let link = format!("{path}:{line}:{column}");
        let expected = (!path.contains("://")).then(|| OutputLink {
            path: PathBuf::from(&path),
            line,
            column,
        });
        assert_eq!(parse_output_link(&link), expected, "case {case}: {link:?}");
        assert_eq!(parse_output_link(&format!("{path}:0:{column}")), None);
        assert_eq!(parse_output_link(&format!("{path}:{line}:{column}\n")), None);
    }
}
