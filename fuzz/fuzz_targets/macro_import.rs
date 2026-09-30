// SPDX-License-Identifier: MPL-2.0
//! Macro and external-command TOML imports, output-link parsing and placeholder
//! expansion never panic; accepted definitions validate and re-import losslessly.
#![no_main]
use bareline_macros::{
    Macro,
    process::{ExternalDefinition, PlaceholderContext, PlaceholderSafety, expand_argument_for, parse_output_link},
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let registry = bareline_commands::shell_commands();
    if let Ok(value) = Macro::import_toml(text, &registry) {
        value.validate(&registry).expect("accepted macro must validate");
        let exported = value.export_toml();
        assert_eq!(Macro::import_toml(&exported, &registry).as_ref(), Ok(&value));
    }
    if let Ok(value) = ExternalDefinition::import_toml(text) {
        assert!(!value.name.trim().is_empty());
        assert_eq!(ExternalDefinition::import_toml(&value.export_toml()), Ok(value));
    }
    if let Some(link) = parse_output_link(text) {
        assert!(link.line > 0 && link.column > 0);
    }
    let context = PlaceholderContext {
        file: Some(std::env::temp_dir().join("fuzz dir").join("file & name.txt")),
        workspace: Some(std::env::temp_dir()),
        selection: text.chars().rev().collect(),
        line: data.len() as u64,
        column: 7,
    };
    let direct = expand_argument_for(text, &context, PlaceholderSafety::Argument);
    if !text.contains("${") {
        assert_eq!(
            direct,
            Ok(std::ffi::OsString::from(text)),
            "literal templates expand to themselves"
        );
    }
    let _ = expand_argument_for(text, &context, PlaceholderSafety::CommandShell);
});
