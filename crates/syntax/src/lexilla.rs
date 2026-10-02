// SPDX-License-Identifier: MPL-2.0
use crate::{Error, Language, MAX_SPANS, StyleKind, StyleSpan};
use bareline_document::TextOffset;
#[cfg(test)]
thread_local! {
    /// Test-only: fail style mapping on this thread to exercise the native fallback.
    pub(crate) static FAIL_MAPPING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(test)]
fn mapping_failure_injected() -> bool {
    FAIL_MAPPING.with(std::cell::Cell::get)
}
#[cfg(not(test))]
fn mapping_failure_injected() -> bool {
    false
}
pub(crate) fn spans(text: &str, language: Language, styles: &[u8]) -> Result<Vec<StyleSpan>, Error> {
    if styles.len() != text.len() || mapping_failure_injected() {
        return Err(Error::InvalidRange);
    }
    // Resolve the lexer once; every byte then maps through a fixed table.
    let lexer = language.metadata().lexilla;
    let table: [Option<StyleKind>; 256] = std::array::from_fn(|style| kind(lexer, style as u8));
    let mut output: Vec<StyleSpan> = Vec::new();
    for (offset, c) in text.char_indices() {
        let style = styles[offset];
        if !styles[offset..offset + c.len_utf8()].iter().all(|s| *s == style) {
            return Err(Error::InvalidRange);
        }
        if let Some(kind) = table[usize::from(style)] {
            if let Some(last) = output.last_mut().filter(|s| s.kind == kind && s.range.end.0 == offset) {
                last.range.end = TextOffset(offset + c.len_utf8());
            } else {
                if output.len() >= MAX_SPANS {
                    return Err(Error::BudgetExceeded);
                }
                output.push(StyleSpan {
                    range: TextOffset(offset)..TextOffset(offset + c.len_utf8()),
                    kind,
                });
            }
        }
    }
    Ok(output)
}
/// Maps an upstream lexer's style numbers (SciLexer.h `SCE_*`) to theme kinds.
/// Identifiers, variables and plain text stay unstyled.
fn kind(lexer: &str, style: u8) -> Option<StyleKind> {
    use StyleKind::*;
    Some(match lexer {
        "rust" => match style {
            1..=4 => Comment,
            5 => Number,
            6..=12 | 19 => Keyword,
            13..=15 | 21..=25 => String,
            16 => Operator,
            _ => return None,
        },
        "python" => match style {
            1 | 12 => Comment,
            2 => Number,
            3 | 4 | 6 | 7 | 13 | 16..=19 => String,
            5 | 14 => Keyword,
            10 => Operator,
            _ => return None,
        },
        "json" => match style {
            1 => Number,
            2..=5 | 9 | 10 => String,
            6 | 7 => Comment,
            8 => Operator,
            11 | 12 => Keyword,
            _ => return None,
        },
        // SCE_H_* and, for PHP, SCE_HPHP_*.
        "hypertext" | "xml" => match style {
            1..=4 | 11..=14 | 18 | 121 => Keyword,
            5 | 122 => Number,
            6 | 7 | 19 | 24 | 25 | 119 | 120 | 126 => String,
            9 | 20 | 29 | 30 | 124 | 125 => Comment,
            127 => Operator,
            _ => return None,
        },
        "css" => match style {
            1..=4 | 6 | 7 | 10..=12 | 15..=18 => Keyword,
            5 => Operator,
            9 => Comment,
            13 | 14 => String,
            _ => return None,
        },
        "toml" => match style {
            1 => Comment,
            3 | 5 | 6 => Keyword,
            4 | 14 => Number,
            8 => Operator,
            9..=13 | 15 => String,
            _ => return None,
        },
        "sql" => match style {
            1..=3 | 13 | 15 | 17 | 18 => Comment,
            4 => Number,
            5 | 8 | 9 | 16 | 19..=22 => Keyword,
            6 | 7 | 23 => String,
            10 | 24 => Operator,
            _ => return None,
        },
        "cpp" => match style {
            1..=3 | 15 | 17 | 18 | 23 | 24 => Comment,
            4 => Number,
            5 | 9 | 16 | 19 => Keyword,
            6..=8 | 12..=14 | 20..=22 => String,
            10 => Operator,
            _ => return None,
        },
        "powershell" => match style {
            1 | 13 | 16 => Comment,
            2 | 3 | 14 | 15 => String,
            4 => Number,
            6 => Operator,
            8..=12 => Keyword,
            _ => return None,
        },
        "batch" => match style {
            1 | 8 => Comment,
            2 | 3 | 5 => Keyword,
            4 | 7 => Operator,
            _ => return None,
        },
        // 0x40 marks text inside a `$(...)` command substitution.
        "bash" => match style & !0x40 {
            2 => Comment,
            3 => Number,
            4 => Keyword,
            5 | 6 | 11..=13 => String,
            7 => Operator,
            _ => return None,
        },
        "yaml" => match style {
            1 => Comment,
            2 | 3 | 5 => Keyword,
            4 => Number,
            6 | 9 => Operator,
            7 => String,
            _ => return None,
        },
        "markdown" => match style {
            6..=11 => Keyword,
            13 | 14 | 17 => Operator,
            15 | 16 => Comment,
            18..=21 => String,
            _ => return None,
        },
        "props" => match style {
            1 => Comment,
            2 | 4 | 5 => Keyword,
            3 => Operator,
            _ => return None,
        },
        "perl" => match style {
            2 | 3 | 21 | 31 => Comment,
            4 => Number,
            5 | 9 => Keyword,
            6 | 7 | 17..=20 | 22..=30 | 41..=44 | 54 | 55 | 57 | 61 | 62 | 64..=66 => String,
            10 => Operator,
            _ => return None,
        },
        "ruby" => match style {
            2 | 3 | 19 => Comment,
            4 => Number,
            5 => Keyword,
            6 | 7 | 12 | 14 | 18 | 20..=28 | 41..=44 => String,
            10 => Operator,
            _ => return None,
        },
        "lua" => match style {
            1..=3 => Comment,
            4 => Number,
            5 | 9 | 13..=19 => Keyword,
            6..=8 | 12 => String,
            10 => Operator,
            _ => return None,
        },
        "makefile" => match style {
            1 => Comment,
            2 | 5 => Keyword,
            4 => Operator,
            _ => return None,
        },
        "cmake" => match style {
            1 => Comment,
            2..=4 | 13 => String,
            5 | 6 | 8..=12 => Keyword,
            14 => Number,
            _ => return None,
        },
        // Added lines use the string colour and removed lines the number colour.
        "diff" => match style {
            1 | 10 | 11 => Comment,
            2 | 3 | 7 => Keyword,
            4 => Operator,
            5 | 9 => Number,
            6 | 8 => String,
            _ => return None,
        },
        // Recognised tool, compiler and traceback locations; diff-style lines.
        "errorlist" => match style {
            4 => Operator,
            11 => String,
            12 => Number,
            22 => Comment,
            1..=10 | 13..=20 | 25 | 26 => Keyword,
            _ => return None,
        },
        "vb" | "vbscript" => match style {
            1 | 19..=22 => Comment,
            2 | 8 | 17 | 18 => Number,
            3 | 5 | 10..=13 => Keyword,
            4 | 9 => String,
            6 => Operator,
            _ => return None,
        },
        "pascal" => match style {
            2..=4 => Comment,
            5 | 6 | 9 => Keyword,
            7 | 8 => Number,
            10..=12 | 15 => String,
            13 => Operator,
            _ => return None,
        },
        "fortran" | "f77" => match style {
            1 => Comment,
            2 | 13 => Number,
            3..=5 => String,
            6 | 12 => Operator,
            8..=11 => Keyword,
            _ => return None,
        },
        "asm" => match style {
            1 | 11 | 15 => Comment,
            2 => Number,
            3 | 12 | 13 | 16 => String,
            4 => Operator,
            6..=10 | 14 => Keyword,
            _ => return None,
        },
        "latex" => match style {
            1 | 2 | 5 | 9 => Keyword,
            3 | 6 | 8 => String,
            4 | 7 => Comment,
            10 => Operator,
            _ => return None,
        },
        "r" => match style {
            1 => Comment,
            2..=4 => Keyword,
            5 => Number,
            6 | 7 | 12..=15 => String,
            8 | 10 => Operator,
            _ => return None,
        },
        "dart" => match style {
            1..=4 => Comment,
            5..=13 | 15 | 17 | 27 => String,
            16 => Operator,
            20 => Number,
            22..=26 => Keyword,
            _ => return None,
        },
        "haskell" => match style {
            2 | 6 | 7 | 9 | 10 | 12 | 17 | 18 => Keyword,
            3 => Number,
            4 | 5 | 19 => String,
            11 | 20 => Operator,
            13..=16 | 21 => Comment,
            _ => return None,
        },
        "erlang" => match style {
            1 | 14..=17 => Comment,
            3 => Number,
            4 | 10 | 12 | 19 | 22 | 24 => Keyword,
            5 | 9 => String,
            6 => Operator,
            _ => return None,
        },
        "tcl" => match style {
            1 | 2 | 20 | 21 => Comment,
            3 => Number,
            4 | 5 => String,
            6 => Operator,
            11..=19 => Keyword,
            _ => return None,
        },
        "au3" => match style {
            1 | 2 => Comment,
            3 => Number,
            4..=6 | 11 | 12 | 15 => Keyword,
            7 | 10 => String,
            8 => Operator,
            _ => return None,
        },
        "nsis" => match style {
            1 | 18 => Comment,
            2..=4 | 13 => String,
            5 | 8..=12 | 15..=17 => Keyword,
            14 => Number,
            _ => return None,
        },
        "inno" => match style {
            1 | 7 => Comment,
            2..=5 | 8 | 9 => Keyword,
            10 | 11 => String,
            _ => return None,
        },
        "registry" => match style {
            1 => Comment,
            2 | 5 | 6 | 9 => Keyword,
            3 | 8 | 10 | 11 => String,
            4 | 7 => Number,
            12 => Operator,
            _ => return None,
        },
        "ada" => match style {
            1 => Keyword,
            3 => Number,
            4 => Operator,
            5..=8 => String,
            10 => Comment,
            _ => return None,
        },
        "d" => match style {
            1..=4 | 15..=17 => Comment,
            5 => Number,
            6..=9 | 20..=22 => Keyword,
            10..=12 | 18 | 19 => String,
            13 => Operator,
            _ => return None,
        },
        "fsharp" => match style {
            1..=5 | 10 => Keyword,
            8 | 9 => Comment,
            12 => Operator,
            13 => Number,
            14..=17 | 19 => String,
            _ => return None,
        },
        "julia" => match style {
            1 => Comment,
            2 => Number,
            3..=5 | 12 | 20 => Keyword,
            6 | 10 | 13..=17 => String,
            7 | 8 | 21 => Operator,
            _ => return None,
        },
        "lisp" => match style {
            1 | 12 => Comment,
            2 => Number,
            3 | 4 | 11 => Keyword,
            6 | 8 => String,
            10 => Operator,
            _ => return None,
        },
        "matlab" => match style {
            1 => Comment,
            2 | 4 => Keyword,
            3 => Number,
            5 | 8 => String,
            6 => Operator,
            _ => return None,
        },
        "nim" => match style {
            1..=4 => Comment,
            5 => Number,
            6 | 7 | 9 | 10 | 13 => String,
            8 => Keyword,
            15 => Operator,
            _ => return None,
        },
        "caml" => match style {
            3..=5 => Keyword,
            7 => Operator,
            8 => Number,
            9 | 11 => String,
            12..=15 => Comment,
            _ => return None,
        },
        "verilog" => match style {
            1..=3 | 20 => Comment,
            4 => Number,
            5 | 7..=9 | 19 => Keyword,
            6 | 12 => String,
            10 => Operator,
            _ => return None,
        },
        "vhdl" => match style {
            1 | 2 | 15 => Comment,
            3 => Number,
            4 | 7 => String,
            5 | 9 => Operator,
            8 | 10..=14 => Keyword,
            _ => return None,
        },
        "zig" => match style {
            1..=3 => Comment,
            4 => Number,
            5 => Operator,
            6..=9 | 18 => String,
            12..=16 => Keyword,
            _ => return None,
        },
        "coffeescript" => match style {
            1..=3 | 15 | 17 | 18 | 22 | 24 => Comment,
            4 => Number,
            5 | 9 | 16 | 19 => Keyword,
            6 | 7 | 12..=14 | 20 | 21 | 23 => String,
            10 => Operator,
            _ => return None,
        },
        "gdscript" => match style {
            1 | 12 => Comment,
            2 => Number,
            3 | 4 | 6 | 7 | 13 | 16 => String,
            5 | 14 | 15 => Keyword,
            10 => Operator,
            _ => return None,
        },
        _ => return None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_catalog_lexer_is_bundled_and_mapped_to_theme_styles() {
        let bundled = bareline_lexilla_bridge::lexer_names();
        for entry in crate::catalog::CATALOG {
            assert!(
                bundled.contains(&entry.lexilla),
                "{}: lexer {} is not bundled",
                entry.id,
                entry.lexilla
            );
            // At least two theme kinds, so a lexer never renders as one colour.
            let kinds: std::collections::BTreeSet<_> = (0..=u8::MAX)
                .filter_map(|style| kind(entry.lexilla, style))
                .map(|kind| kind as u8)
                .collect();
            assert!(kinds.len() >= 2, "{}: {kinds:?}", entry.id);
            // Style 0 is every lexer's default text.
            assert_eq!(kind(entry.lexilla, 0), None, "{}", entry.id);
        }
        assert!((0..=u8::MAX).all(|style| kind(Language::PlainText.metadata().lexilla, style).is_none()));
    }
}
