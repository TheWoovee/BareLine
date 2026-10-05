// SPDX-License-Identifier: MPL-2.0
//! Notepad++ UDL XML import and JSON registry replacement never panic; accepted
//! definitions validate, persist losslessly and lex only in-bounds UTF-8 ranges.
#![no_main]
use bareline_syntax::{Cancellation, udl};
use libfuzzer_sys::fuzz_target;

const SAMPLE: &str = "hello world // note\n/* block */ \"str\\\"\" 'c' 12.5e3 _x9 é 中 😀 + - { } <tag>";

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut registry = udl::Registry::default();
    if registry.replace_json(text).is_err() {
        assert!(
            registry.get("imported").is_none(),
            "failed replacement published a definition"
        );
    }
    let Ok((definition, _report)) = udl::import_notepad_xml(text) else {
        return;
    };
    definition.validate().expect("accepted definition must validate");
    let json = definition.to_json().expect("validated definition serializes");
    // from_json caps its input at 128 KiB; a pretty-printed export of a maximal
    // import can exceed that, which is a size asymmetry rather than a parse failure.
    if json.len() <= 128 << 10 {
        let reloaded = udl::Definition::from_json(&json).expect("export must reload");
        assert_eq!(reloaded.to_json().unwrap(), json);
    }
    let cancel = Cancellation::default();
    for sample in [SAMPLE, text] {
        let Ok(spans) = udl::lex(sample, &definition, &cancel) else {
            continue;
        };
        let mut previous_end = 0;
        for span in &spans {
            let (start, end) = (span.range.start.0, span.range.end.0);
            assert!((previous_end..end).contains(&start) && end <= sample.len());
            assert!(sample.is_char_boundary(start) && sample.is_char_boundary(end));
            previous_end = end;
        }
    }
});
