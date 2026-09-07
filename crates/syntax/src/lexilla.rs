// SPDX-License-Identifier: MPL-2.0
use crate::{Error, Language, MAX_SPANS, StyleKind, StyleSpan};
use bareline_document::TextOffset;
pub(crate) fn spans(
    text: &str,
    language: Language,
    styles: &[u8],
) -> Result<Vec<StyleSpan>, Error> {
    if styles.len() != text.len() {
        return Err(Error::InvalidRange);
    }
    let mut output: Vec<StyleSpan> = Vec::new();
    for (offset, c) in text.char_indices() {
        let style = styles[offset];
        if !styles[offset..offset + c.len_utf8()]
            .iter()
            .all(|s| *s == style)
        {
            return Err(Error::InvalidRange);
        }
        if let Some(kind) = kind(language, style) {
            if let Some(last) = output
                .last_mut()
                .filter(|s| s.kind == kind && s.range.end.0 == offset)
            {
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
fn kind(language: Language, style: u8) -> Option<StyleKind> {
    use StyleKind::*;
    Some(match language {
        Language::Rust => match style {
            1..=4 => Comment,
            5 => Number,
            6..=12 | 19 => Keyword,
            13..=15 | 21..=25 => String,
            16 => Operator,
            _ => return None,
        },
        Language::Python => match style {
            1 | 12 => Comment,
            2 => Number,
            3 | 4 | 6 | 7 | 13 | 16..=19 => String,
            5 | 14 => Keyword,
            10 => Operator,
            _ => return None,
        },
        Language::Json => match style {
            1 => Number,
            2..=5 | 9 | 10 => String,
            6 | 7 => Comment,
            8 => Operator,
            11 | 12 => Keyword,
            _ => return None,
        },
        Language::Html | Language::Xml => match style {
            1..=4 | 11..=14 => Keyword,
            5 => Number,
            6 | 7 | 19 | 24 | 25 => String,
            9 | 20 | 29 | 30 => Comment,
            _ => return None,
        },
        Language::Css => match style {
            1..=4 | 6 | 7 | 10..=12 | 15..=18 => Keyword,
            5 => Operator,
            9 => Comment,
            13 | 14 => String,
            _ => return None,
        },
        Language::Toml => match style {
            1 => Comment,
            3 | 5 | 6 => Keyword,
            4 | 14 => Number,
            8 => Operator,
            9..=13 | 15 => String,
            _ => return None,
        },
        Language::PlainText => return None,
        Language::Sql => match style {
            1..=3 | 13 | 15 | 17 | 18 => Comment,
            4 => Number,
            5 | 8 | 9 | 16 | 19..=22 => Keyword,
            6 | 7 | 23 => String,
            10 | 24 => Operator,
            _ => return None,
        },
        _ => match style {
            1..=3 | 15 | 17 | 18 | 23 | 24 => Comment,
            4 => Number,
            5 | 9 | 16 | 19 => Keyword,
            6..=8 | 12..=14 | 20..=22 => String,
            10 => Operator,
            _ => return None,
        },
    })
}
