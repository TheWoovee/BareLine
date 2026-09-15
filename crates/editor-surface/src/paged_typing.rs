// SPDX-License-Identifier: MPL-2.0
//! Bounded plans for the real paged actor. This module never edits an adapter.
use crate::{
    Input, Selection, completion,
    power::{CommentProvider, Limits, SelectionSet},
};
use bareline_document::{Budget, Document, Edit, EditTransaction, TextOffset, paged::PagedSnapshot};
use bareline_file_io::cancellation::Cancellation;
use bareline_syntax::Language;
use std::sync::Arc;
const CONTEXT: usize = 256 * 1024;
#[derive(Clone)]
pub struct TypingConfig {
    pub language: Language,
    pub definition: Option<Arc<bareline_syntax::udl::Definition>>,
    pub smart_pairs: bool,
    pub smart_indent: bool,
    pub tab_width: usize,
    /// Only a current, source-mapped lexer may provide this value.
    pub literal_context: Option<bool>,
}
#[derive(Clone)]
pub enum TypingRequest {
    Input(Input),
    Comment {
        block: bool,
    },
    CommentWithTokens {
        block: bool,
        tokens: crate::power::CommentTokens,
    },
}
pub struct TypingPlan {
    pub transaction: EditTransaction,
    pub selection: Selection,
}
fn temporary(text: &str, source: &PagedSnapshot) -> Result<Document, String> {
    let mut document = Document::from_utf8(text, Budget::new(CONTEXT * 4), Budget::new(CONTEXT))
        .map_err(|error| format!("{error:?}"))?;
    document
        .initialize_metadata(source.metadata().clone())
        .map_err(|error| format!("{error:?}"))?;
    Ok(document)
}
fn limits(config: &TypingConfig) -> Limits {
    Limits {
        max_bytes: CONTEXT,
        max_selections: 8192,
        tab_width: config.tab_width.clamp(1, 16),
    }
}
fn suffix(source: &PagedSnapshot, config: &TypingConfig, typed: char) -> Result<Option<String>, String> {
    let document = temporary("", source)?;
    let set = Selection::default().into();
    let edit = completion::smart_pair_configured(
        &document.snapshot(),
        &set,
        typed,
        config.language,
        None,
        limits(config),
        config.definition.as_deref(),
    )
    .map_err(|error| format!("{error:?}"))?;
    Ok(edit
        .transaction
        .edits
        .first()
        .and_then(|edit| edit.insert.strip_prefix(typed))
        .filter(|tail| !tail.is_empty())
        .map(str::to_owned))
}
pub fn handles(input: &Input, config: &TypingConfig) -> bool {
    match input {
        Input::Backspace => config.smart_pairs,
        Input::Insert(value) => {
            (config.smart_indent && matches!(value.as_str(), "\n" | "\r\n" | "\r"))
                || (config.smart_pairs && value.chars().count() == 1)
        }
        _ => false,
    }
}
/// Run on the existing paged worker. `read` resolves the actor's current source
/// without recursively acquiring its lock; it returns the actual trimmed origin.
/// Empty edits mean an overtype caret move, not a history transaction.
pub fn prepare(
    source: &PagedSnapshot,
    selection: Selection,
    request: TypingRequest,
    config: &TypingConfig,
    cancel: &Cancellation,
    mut read: impl FnMut(TextOffset, usize) -> Result<(TextOffset, String), String>,
) -> Result<Option<TypingPlan>, String> {
    cancel.check().map_err(|_| "Typing cancelled")?;
    if selection.anchor > source.len() || selection.caret > source.len() {
        return Err("Typing source changed".into());
    }
    if let Some(definition) = &config.definition {
        definition.validate().map_err(|error| format!("{error:?}"))?;
    }
    let range = selection.range();
    let make = |edits: Vec<Edit>, caret: usize| TypingPlan {
        transaction: EditTransaction {
            base_revision: source.revision,
            edits,
        },
        selection: Selection { anchor: caret, caret },
    };
    let mut bounded_read = |start: usize, count: usize| -> Result<(usize, String), String> {
        cancel.check().map_err(|_| "Typing cancelled")?;
        if count > CONTEXT {
            return Err("Typing context exceeds 256 KiB".into());
        }
        let (origin, text) = read(TextOffset(start), count)?;
        if text.len() > count || origin.0 > source.len() || origin.0.saturating_add(text.len()) > source.len() {
            return Err("Invalid typing source window".into());
        }
        cancel.check().map_err(|_| "Typing cancelled")?;
        Ok((origin.0, text))
    };
    let exact = |origin: usize, text: &str, start: usize, end: usize| -> Result<String, String> {
        let a = start.checked_sub(origin).ok_or("Typing boundary unavailable")?;
        let b = end.checked_sub(origin).ok_or("Typing boundary unavailable")?;
        text.get(a..b)
            .map(str::to_owned)
            .ok_or_else(|| "Typing boundary unavailable".into())
    };
    let captured_tokens = if let TypingRequest::CommentWithTokens { tokens, .. } = &request {
        Some(tokens.clone())
    } else {
        None
    };
    match request {
        TypingRequest::Input(Input::Insert(value))
            if config.smart_indent && matches!(value.as_str(), "\n" | "\r\n" | "\r") =>
        {
            let (origin, text) = bounded_read(range.start.saturating_sub(64 * 1024), 64 * 1024 + 8)?;
            let prefix = exact(origin, &text, origin, range.start)?;
            let line = prefix.rfind(['\n', '\r']).map_or(0, |at| at + 1);
            if origin != 0 && line == 0 {
                return Ok(None);
            }
            let eol = text
                .find(['\r', '\n'])
                .map(|at| {
                    if text[at..].starts_with("\r\n") {
                        "\r\n"
                    } else if text.as_bytes()[at] == b'\r' {
                        "\r"
                    } else {
                        "\n"
                    }
                })
                .unwrap_or(value.as_str());
            let prefix = &prefix[line..];
            let document = temporary(prefix, source)?;
            let snapshot = document.snapshot();
            let set = Selection {
                anchor: prefix.len(),
                caret: prefix.len(),
            }
            .into();
            let language = if config.literal_context == Some(true) {
                Language::PlainText
            } else {
                config.language
            };
            let edit = completion::smart_newline(&snapshot, &set, language, limits(config))
                .map_err(|error| format!("{error:?}"))?;
            let mut insert = edit.transaction.edits.first().ok_or("No newline edit")?.insert.clone();
            if eol != "\n" && insert.starts_with('\n') {
                insert.replace_range(..1, eol);
            }
            if config.literal_context != Some(true)
                && config.definition.as_ref().is_some_and(|definition| {
                    prefix.trim_end().chars().next_back().is_some_and(|last| {
                        !language.metadata().indent_after.contains(last)
                            && definition.fold_pairs.iter().any(|(open, _)| *open == last)
                    })
                })
            {
                insert.push_str(&" ".repeat(config.tab_width.clamp(1, 16)));
            }
            let caret = range.start + insert.len();
            Ok(Some(make(
                vec![Edit {
                    range: TextOffset(range.start)..TextOffset(range.end),
                    insert,
                }],
                caret,
            )))
        }
        TypingRequest::Input(Input::Insert(value)) if config.smart_pairs && value.chars().count() == 1 => {
            let typed = value.chars().next().unwrap();
            if range.is_empty()
                && (matches!(typed, ')' | ']' | '}' | '\"' | '\'')
                    || config.definition.as_ref().is_some_and(|definition| {
                        definition.strings.contains(&typed)
                            || definition.fold_pairs.iter().any(|(_, close)| *close == typed)
                    }))
                && range.start < source.len()
            {
                let (origin, text) = bounded_read(range.start, 4)?;
                if exact(origin, &text, range.start, range.start + typed.len_utf8()).is_ok_and(|next| next == value) {
                    return Ok(Some(make(Vec::new(), range.start + typed.len_utf8())));
                }
            }
            if config.literal_context == Some(true) {
                return Ok(None);
            }
            let Some(close) = suffix(source, config, typed)? else {
                return Ok(None);
            };
            if range.is_empty() {
                return Ok(Some(make(
                    vec![Edit {
                        range: TextOffset(range.start)..TextOffset(range.start),
                        insert: format!("{value}{close}"),
                    }],
                    range.start + value.len(),
                )));
            }
            // Two edge inserts wrap even a multi-GB selection without copying it.
            let end = range
                .end
                .checked_add(value.len() + close.len())
                .ok_or("Typing offset overflow")?;
            Ok(Some(make(
                vec![
                    Edit {
                        range: TextOffset(range.start)..TextOffset(range.start),
                        insert: value,
                    },
                    Edit {
                        range: TextOffset(range.end)..TextOffset(range.end),
                        insert: close,
                    },
                ],
                end,
            )))
        }
        TypingRequest::Input(Input::Backspace)
            if config.smart_pairs && range.is_empty() && range.start > 0 && range.start < source.len() =>
        {
            let (origin, text) = bounded_read(range.start.saturating_sub(4), 12)?;
            let at = range.start.checked_sub(origin).ok_or("Pair context unavailable")?;
            let Some(before) = text.get(..at).and_then(|prefix| prefix.chars().next_back()) else {
                return Ok(None);
            };
            let Some(after) = text.get(at..).and_then(|suffix| suffix.chars().next()) else {
                return Ok(None);
            };
            if suffix(source, config, before)?.is_some_and(|expected| expected == after.to_string()) {
                let caret = range.start - before.len_utf8();
                return Ok(Some(make(
                    vec![Edit {
                        range: TextOffset(caret)..TextOffset(range.start + after.len_utf8()),
                        insert: String::new(),
                    }],
                    caret,
                )));
            }
            Ok(None)
        }
        TypingRequest::Comment { block } | TypingRequest::CommentWithTokens { block, .. } => {
            let document = temporary("", source)?;
            let tokens = if let Some(tokens) = captured_tokens {
                Some(tokens)
            } else if let Some(definition) = &config.definition {
                completion::DefinitionComments(definition).tokens_for(&document.snapshot())
            } else {
                completion::LanguageComments(config.language).tokens_for(&document.snapshot())
            }
            .ok_or("Comments unavailable for this language")?;
            if block {
                let (open, close) = tokens.block.ok_or("Block comments unavailable for this language")?;
                let unwrap = if range.len() >= open.len() + close.len() {
                    let (a, prefix) = bounded_read(range.start, open.len() + 4)?;
                    let (b, suffix) = bounded_read(range.end.saturating_sub(close.len() + 4), close.len() + 8)?;
                    exact(a, &prefix, range.start, range.start + open.len()).is_ok_and(|text| text == open)
                        && exact(b, &suffix, range.end - close.len(), range.end).is_ok_and(|text| text == close)
                } else {
                    false
                };
                let (edits, caret) = if unwrap {
                    (
                        vec![
                            Edit {
                                range: TextOffset(range.start)..TextOffset(range.start + open.len()),
                                insert: String::new(),
                            },
                            Edit {
                                range: TextOffset(range.end - close.len())..TextOffset(range.end),
                                insert: String::new(),
                            },
                        ],
                        range.end - open.len() - close.len(),
                    )
                } else if range.is_empty() {
                    (
                        vec![Edit {
                            range: TextOffset(range.start)..TextOffset(range.start),
                            insert: format!("{open}{close}"),
                        }],
                        range.start + open.len() + close.len(),
                    )
                } else {
                    (
                        vec![
                            Edit {
                                range: TextOffset(range.start)..TextOffset(range.start),
                                insert: open.clone(),
                            },
                            Edit {
                                range: TextOffset(range.end)..TextOffset(range.end),
                                insert: close.clone(),
                            },
                        ],
                        range.end + open.len() + close.len(),
                    )
                };
                return Ok(Some(make(edits, caret)));
            }
            if range.len() > CONTEXT - 128 * 1024 {
                return Err("Line comment selection exceeds bounded context; use smaller regions".into());
            }
            let (origin, text) = bounded_read(range.start.saturating_sub(64 * 1024), CONTEXT)?;
            let from = range.start.checked_sub(origin).ok_or("Comment context unavailable")?;
            let to = range
                .end
                .checked_sub(origin)
                .filter(|end| *end <= text.len())
                .ok_or("Comment context unavailable")?;
            if origin > 0 && !text.get(..from).is_some_and(|prefix| prefix.contains(['\n', '\r'])) {
                return Err("Comment line exceeds bounded context".into());
            }
            let last = if to > from { to - 1 } else { to };
            if origin + text.len() < source.len()
                && !text.as_bytes()[last.min(text.len())..]
                    .iter()
                    .any(|byte| matches!(*byte, b'\n' | b'\r'))
            {
                return Err("Comment line end unavailable".into());
            }
            let document = temporary(&text, source)?;
            let snapshot = document.snapshot();
            let set: SelectionSet = Selection {
                anchor: from,
                caret: to,
            }
            .into();
            let edit = if let Some(definition) = &config.definition {
                completion::toggle_comment_with_provider(
                    &snapshot,
                    &set,
                    &completion::DefinitionComments(definition),
                    false,
                    limits(config),
                )
            } else {
                completion::toggle_comment(&snapshot, &set, config.language, false, limits(config))
            }
            .map_err(|error| format!("{error:?}"))?;
            let mut edits = edit.transaction.edits;
            for edit in &mut edits {
                edit.range.start.0 += origin;
                edit.range.end.0 += origin;
            }
            Ok(Some(make(edits, origin + edit.selections.primary().caret)))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::source::{Generation, MemorySource, SourceKind};
    fn source(len: usize) -> PagedSnapshot {
        let (source, _publisher) = MemorySource::new(
            len as u64,
            Generation(1),
            SourceKind::Paged,
            4096,
            4096,
            Budget::new(8192),
        )
        .unwrap();
        PagedSnapshot::utf8(source, 0).unwrap()
    }
    fn config() -> TypingConfig {
        TypingConfig {
            language: Language::Rust,
            definition: None,
            smart_pairs: true,
            smart_indent: true,
            tab_width: 4,
            literal_context: Some(false),
        }
    }
    #[test]
    fn huge_block_comment_uses_captured_tokens_and_only_edge_reads() {
        let source = source(1 << 30);
        let mut requested = 0usize;
        let plan = prepare(
            &source,
            Selection {
                anchor: 7,
                caret: source.len() - 7,
            },
            TypingRequest::CommentWithTokens {
                block: true,
                tokens: crate::power::CommentTokens {
                    line: None,
                    block: Some(("<*".into(), "*>".into())),
                },
            },
            &config(),
            &Cancellation::default(),
            |start, count| {
                requested += count;
                Ok((start, "x".repeat(count.min(source.len() - start.0))))
            },
        )
        .unwrap()
        .unwrap();
        assert!(requested < 64);
        assert_eq!(plan.transaction.edits.len(), 2);
        assert_eq!(plan.transaction.edits[0].insert, "<*");
        assert_eq!(plan.transaction.edits[1].insert, "*>");
        assert_eq!(
            plan.transaction.edits[1].range,
            TextOffset(source.len() - 7)..TextOffset(source.len() - 7)
        );
    }
    #[test]
    fn huge_wrap_has_two_edge_inserts_without_reading_selection() {
        let source = source(1 << 30);
        let plan = prepare(
            &source,
            Selection {
                anchor: 7,
                caret: source.len() - 7,
            },
            TypingRequest::Input(Input::Insert("(".into())),
            &config(),
            &Cancellation::default(),
            |_, _| panic!("wrapping must not read selected bytes"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(plan.transaction.edits.len(), 2);
        assert_eq!(plan.transaction.edits[0].range, TextOffset(7)..TextOffset(7));
        assert_eq!(
            plan.transaction.edits[1].range,
            TextOffset(source.len() - 7)..TextOffset(source.len() - 7)
        );
        assert_eq!(
            plan.transaction
                .edits
                .iter()
                .map(|edit| edit.insert.len())
                .sum::<usize>(),
            2
        );
        assert_eq!(plan.selection.caret, source.len() - 5);
    }
    #[test]
    fn overtype_moves_global_caret_without_history_edit() {
        let source = source(1000);
        let plan = prepare(
            &source,
            Selection {
                anchor: 700,
                caret: 700,
            },
            TypingRequest::Input(Input::Insert(")".into())),
            &config(),
            &Cancellation::default(),
            |start, _| Ok((start, ")".into())),
        )
        .unwrap()
        .unwrap();
        assert!(plan.transaction.edits.is_empty());
        assert_eq!(plan.selection.caret, 701);
    }
    #[test]
    fn cancelled_and_foreign_context_never_produce_edits() {
        let source = source(1000);
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(
            prepare(
                &source,
                Selection::default(),
                TypingRequest::Input(Input::Insert("(".into())),
                &config(),
                &cancel,
                |_, _| unreachable!()
            )
            .is_err()
        );
        assert!(
            prepare(
                &source,
                Selection {
                    anchor: 700,
                    caret: 700
                },
                TypingRequest::Input(Input::Insert(")".into())),
                &config(),
                &Cancellation::default(),
                |_, _| Ok((TextOffset(1001), ")".into()))
            )
            .is_err()
        );
    }
}
