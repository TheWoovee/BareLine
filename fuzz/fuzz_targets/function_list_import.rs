// SPDX-License-Identifier: MPL-2.0
//! Notepad++ function-list XML and outline TOML imports never panic; accepted
//! definitions validate, persist losslessly and extract ordered, in-bounds symbols.
#![no_main]
use bareline_document::{Budget, Document};
use bareline_syntax::outline::{self, OutlineJob};
use libfuzzer_sys::fuzz_target;

const SAMPLE: &str = "class Shape {\n  fn area(&self) {}\n}\ndef hello():\n  pass\nfunction world() {}\nfn main() {}\n";

fn check(definition: &outline::Definition) {
    definition.validate().expect("accepted definition must validate");
    let toml = definition.to_toml().expect("validated definition serializes");
    assert_eq!(outline::Definition::from_toml(&toml).as_ref(), Ok(definition));
    let document = Document::from_utf8(SAMPLE, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
    let Ok(projection) = definition.extract(&document.snapshot(), &OutlineJob::default()) else {
        return;
    };
    let mut previous = None;
    for symbol in &projection.symbols {
        let key = (symbol.range.start, std::cmp::Reverse(symbol.range.end));
        assert!(previous.is_none_or(|previous| previous <= key), "unsorted symbols");
        previous = Some(key);
        let (range, name) = (&symbol.range, &symbol.name_range);
        assert!(range.start <= name.start && name.end <= range.end && range.end.0 <= SAMPLE.len());
        assert_eq!(symbol.name, SAMPLE[name.start.0..name.end.0]);
        assert!(symbol.depth <= 64);
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok((definition, report)) = outline::import_function_list(text) {
        assert_eq!(outline::import_function_list(text).map(|(_, again)| again), Ok(report));
        check(&definition);
    }
    if let Ok(definition) = outline::Definition::from_toml(text) {
        check(&definition);
    }
});
