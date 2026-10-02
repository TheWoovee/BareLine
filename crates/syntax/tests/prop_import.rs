// SPDX-License-Identifier: MPL-2.0
//! Seeded property tests (QA-05): Notepad++ UDL and function-list XML imports never
//! panic on arbitrary input; accepted definitions validate, persist losslessly and lex
//! or extract only in-bounds, UTF-8 aligned ranges.
use bareline_document::{Budget, Document, TextOffset};
use bareline_syntax::{
    Cancellation,
    outline::{self, OutlineJob},
    udl,
};

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

/// Structural mutation: insert, delete or duplicate a short span at char boundaries.
fn mutate(rng: &mut Rng, text: &mut String, pool: &[&str]) {
    for _ in 0..rng.below(3) {
        let points: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(Some(text.len())).collect();
        let (a, b) = (*rng.pick(&points), *rng.pick(&points));
        let (a, b) = (a.min(b), a.max(b));
        match rng.below(3) {
            0 => text.insert_str(a, rng.pick(pool)),
            1 => text.replace_range(a..b, ""),
            _ => {
                let copy = text[a..b].to_owned();
                text.insert_str(b, &copy);
            }
        }
    }
}
const SAMPLE_TEXT: &[&str] = &[
    "hello", "world", " ", "\n", "\r\n", "\"q\"", "'s'", "\\", "//", "/*", "*/", "#", "{", "}", "12.5e3", "_x9", "é",
    "中", "😀", "+", "-", "<", ">", "&",
];
fn sample(rng: &mut Rng) -> String {
    rng.text(SAMPLE_TEXT, 24)
}

const UDL_PIECES: &[&str] = &[
    "<?xml version=\"1.0\"?>",
    "<!-- note -->",
    "<NotepadPlus>",
    "</NotepadPlus>",
    "<UserLang name=\"Demo Lang\" ext=\"demo dm\">",
    "<UserLang name=\"&amp;&#x41;&#66;\" ext=''>",
    "<UserLang>",
    "</UserLang>",
    "<Settings><Global caseIgnored=\"no\"/><Prefix Keywords1=\"no\"/></Settings>",
    "<KeywordLists>",
    "</KeywordLists>",
    "<Keywords name=\"Keywords1\">hello world &lt;tag&gt;</Keywords>",
    "<Keywords name=\"Keywords2\">_x9 é 中</Keywords>",
    "<Keywords name=\"Operators1\">+ - &amp; { }</Keywords>",
    "<Keywords name=\"Comments\">00// 01 02/* 03 04*/</Keywords>",
    "<Keywords name=\"Delimiters\">00\" 01 02\"</Keywords>",
    "<Keywords name=\"Keywords3\">&bogus;</Keywords>",
    "<Keywords name=\"Keywords4\">",
    "</Keywords>",
    "<Styles><WordsStyle name=\"DEFAULT\" fgColor=\"000000\"/></Styles>",
    "<Unclosed attr=\"x\"",
    "text outside",
    "<>",
    "< / >",
];
fn udl_xml(rng: &mut Rng) -> String {
    let mut xml = if rng.chance(60) {
        // Mostly well-formed skeleton with random inner elements.
        let inner = rng.text(&UDL_PIECES[8..20], 6);
        let name = rng.pick(&["Demo", "a b", "x&amp;y", "é", ""]);
        format!(
            "<NotepadPlus><UserLang name=\"{name}\" ext=\"demo\"><KeywordLists>{inner}</KeywordLists></UserLang></NotepadPlus>"
        )
    } else {
        rng.text(UDL_PIECES, 12)
    };
    if rng.chance(50) {
        mutate(rng, &mut xml, UDL_PIECES);
    }
    xml
}

#[test]
fn udl_import_never_panics_and_accepted_definitions_persist_and_lex() {
    let mut rng = Rng(0x0d11_0001);
    let mut accepted = 0;
    for case in 0..2000 {
        let xml = if rng.chance(15) {
            String::from_utf8_lossy(&(0..rng.below(64)).map(|_| rng.next_u64() as u8).collect::<Vec<_>>()).into_owned()
        } else {
            udl_xml(&mut rng)
        };
        let context = format!("case {case}: {xml:?}");
        let Ok((definition, _report)) = udl::import_notepad_xml(&xml) else {
            continue;
        };
        accepted += 1;
        definition
            .validate()
            .unwrap_or_else(|error| panic!("{context}: accepted invalid: {error:?}"));
        let json = definition.to_json().unwrap();
        let reloaded = udl::Definition::from_json(&json).unwrap_or_else(|error| panic!("{context}: {error:?}"));
        assert_eq!(reloaded.to_json().unwrap(), json, "{context}: JSON round trip");
        let mut registry = udl::Registry::default();
        registry.replace_json(&json).unwrap();
        assert_eq!(registry.get(&definition.id).unwrap().to_json().unwrap(), json);
        for _ in 0..3 {
            let text = sample(&mut rng);
            let spans = udl::lex(&text, &definition, &Cancellation::default())
                .unwrap_or_else(|error| panic!("{context}: lex {text:?}: {error:?}"));
            let mut previous_end = 0;
            for span in &spans {
                let (start, end) = (span.range.start.0, span.range.end.0);
                assert!(
                    (previous_end..end).contains(&start) && end <= text.len(),
                    "{context}: span {span:?} in {text:?}"
                );
                assert!(
                    text.is_char_boundary(start) && text.is_char_boundary(end),
                    "{context}: {span:?}"
                );
                previous_end = end;
            }
        }
    }
    assert!(
        accepted > 50,
        "generator produced too few importable definitions: {accepted}"
    );
    for hostile in ["<!DOCTYPE x><NotepadPlus/>", "<!ENTITY e SYSTEM 'x'><NotepadPlus/>"] {
        assert!(udl::import_notepad_xml(hostile).is_err());
    }
}

const PATTERNS: &[&str] = &[
    r"fn\s+\w+",
    r"def\s+\w+",
    r"class\s+\w+",
    r"\w+(?=\()",
    r"[a-z]+",
    r"^\s*#.*$",
    r"\w+$",
    r"(",
    r"[",
    r"(?<",
    r"\",
    r"a{2,1}",
    r"(a+)+$",
    r"é+",
    r"&lt;\w+&gt;",
    r"&quot;[^&]*&quot;",
];
fn function_list_xml(rng: &mut Rng) -> String {
    let mut rules = String::new();
    for _ in 0..rng.below(4) {
        let element = if rng.chance(60) { "function" } else { "classRange" };
        let mut names = String::new();
        for _ in 0..rng.below(3) {
            names.push_str("<nameExpr expr=\"");
            names.push_str(rng.pick(PATTERNS));
            names.push_str("\"/>");
        }
        rules.push_str(&format!(
            "<{element} mainExpr=\"{}\"><functionName>{names}</functionName></{element}>",
            rng.pick(PATTERNS)
        ));
    }
    let id = rng.pick(&[
        "id=\"rust\"",
        "displayName=\"Py\"",
        "id='x' displayName='y'",
        "id=\"\"",
        "",
    ]);
    let mut xml = format!("<NotepadPlus><functionList><parser {id}>{rules}</parser></functionList></NotepadPlus>");
    if rng.chance(50) {
        let pool = [
            "<parser id='p'>",
            "</parser>",
            "<function mainExpr='x'/>",
            "<nameExpr expr='y'/>",
            "<!-- c -->",
            "&amp;",
            "\"",
        ];
        mutate(rng, &mut xml, &pool);
    }
    xml
}

#[test]
fn function_list_import_never_panics_and_accepted_definitions_persist_and_extract() {
    let mut rng = Rng(0x0d11_0002);
    let mut accepted = 0;
    for case in 0..400 {
        let xml = function_list_xml(&mut rng);
        let context = format!("case {case}: {xml:?}");
        let Ok((definition, report)) = outline::import_function_list(&xml) else {
            continue;
        };
        accepted += 1;
        assert_eq!(
            outline::import_function_list(&xml).map(|(_, again)| again),
            Ok(report),
            "{context}"
        );
        definition
            .validate()
            .unwrap_or_else(|error| panic!("{context}: accepted invalid: {error}"));
        let toml = definition.to_toml().unwrap();
        assert_eq!(
            outline::Definition::from_toml(&toml),
            Ok(definition.clone()),
            "{context}: {toml}"
        );
        let text = sample(&mut rng);
        let document = Document::from_utf8(&text, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let Ok(projection) = definition.extract(&document.snapshot(), &OutlineJob::default()) else {
            continue;
        };
        assert!(!projection.partial, "{context}");
        let mut previous = None;
        for symbol in &projection.symbols {
            let key = (symbol.range.start, std::cmp::Reverse(symbol.range.end));
            assert!(
                previous.is_none_or(|previous| previous <= key),
                "{context}: unsorted symbols"
            );
            previous = Some(key);
            let (range, name) = (&symbol.range, &symbol.name_range);
            assert!(
                range.start <= name.start && name.end <= range.end && range.end.0 <= text.len(),
                "{context}: {symbol:?} in {text:?}"
            );
            assert_eq!(symbol.name, text[name.start.0..name.end.0], "{context}");
            assert!(!symbol.name.is_empty() && symbol.depth <= 64, "{context}");
            assert!(matches!(symbol.kind.as_str(), "function" | "class"), "{context}");
        }
        assert!(
            projection
                .symbols
                .iter()
                .all(|symbol| symbol.range.end <= TextOffset(text.len()))
        );
    }
    assert!(
        accepted > 40,
        "generator produced too few importable definitions: {accepted}"
    );
}
