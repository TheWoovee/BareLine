// SPDX-License-Identifier: MPL-2.0
//! Verified sequential syntax for paged sources. The caller owns source identity
//! and resolves pages; only bounded UTF-8 windows cross this API.
use crate::{
    Cancellation, Checkpoint, Error, Language, LexOptions, LexerPreference, MAX_REQUEST_BYTES,
    State, SyntaxResult,
};
use bareline_document::{Budget, Document, TextOffset};
use std::sync::Arc;

pub struct StreamLexer {
    language: Language,
    definition: Option<Arc<crate::udl::Definition>>,
    state: State,
    closed: bool,
    previous_cr: bool,
    next: usize,
    line: usize,
    native: Option<bareline_lexilla_bridge::LexerSession>,
}
pub struct StreamResult {
    pub syntax: SyntaxResult,
    pub origin: TextOffset,
    pub first_line: usize,
    pub eof: bool,
}
impl StreamLexer {
    pub fn new(
        language: Language,
        preference: LexerPreference,
        definition: Option<Arc<crate::udl::Definition>>,
    ) -> Self {
        use bareline_lexilla_bridge::CppMode;
        let mode = match language {
            Language::JavaScript | Language::TypeScript => CppMode::JavaScript,
            Language::Go => CppMode::Go,
            Language::Java => CppMode::Java,
            Language::CSharp => CppMode::CSharp,
            _ => CppMode::Default,
        };
        let native = if preference == LexerPreference::Lexilla
            && definition.is_none()
            && language != Language::PlainText
        {
            bareline_lexilla_bridge::LexerSession::new(
                language.metadata().lexilla,
                language.metadata().keywords,
                mode,
            )
            .ok()
        } else {
            None
        };
        Self {
            language,
            definition,
            state: State::Normal,
            closed: false,
            previous_cr: false,
            next: 0,
            line: 0,
            native,
        }
    }
    pub fn next(&self) -> TextOffset {
        TextOffset(self.next)
    }
    /// Nonterminal chunks must end after a full newline. No guessed mid-line
    /// token state or unverified preceding text is accepted.
    pub fn advance(
        &mut self,
        text: &str,
        origin: TextOffset,
        eof: bool,
        cancel: &Cancellation,
    ) -> Result<StreamResult, Error> {
        if self.closed
            || (self.previous_cr && text.starts_with('\n'))
            || origin.0 != self.next
            || text.len() > MAX_REQUEST_BYTES
            || (!eof && !text.ends_with('\n') && !text.ends_with('\r'))
        {
            return Err(Error::InvalidRange);
        }
        self.closed = true;
        cancel.check()?;
        let document = Document::from_utf8(
            text,
            Budget::new(MAX_REQUEST_BYTES * 4),
            Budget::new(MAX_REQUEST_BYTES),
        )
        .map_err(|_| Error::BudgetExceeded)?;
        let source = document.snapshot();
        let checkpoint = Checkpoint {
            source: source.clone(),
            language: self.language,
            offset: TextOffset(0),
            state: self.state,
            definition: self.definition.clone(),
        };
        let mut syntax = crate::lex_configured(
            source,
            self.language,
            TextOffset(0)..TextOffset(text.len()),
            Some(&checkpoint),
            cancel,
            None,
            LexOptions {
                line_origin: self.line,
                preference: LexerPreference::Native,
                definition: self.definition.clone(),
            },
        )?;
        self.state = syntax.checkpoint.as_ref().ok_or(Error::InvalidRange)?.state;
        if let Some(native) = &mut self.native {
            match native.advance(text, origin.0, &|| cancel.is_cancelled()) {
                Ok(output) => {
                    syntax.spans = crate::lexilla::spans(text, self.language, &output.styles)?;
                    syntax.fold_levels = Some(output.fold_levels);
                }
                Err(bareline_lexilla_bridge::Error::Cancelled) => return Err(Error::Cancelled),
                Err(_) => self.native = None,
            }
        }
        let first_line = self.line;
        self.line += text
            .as_bytes()
            .iter()
            .enumerate()
            .filter(|(i, b)| {
                **b == b'\n' || (**b == b'\r' && text.as_bytes().get(i + 1) != Some(&b'\n'))
            })
            .count();
        self.next += text.len();
        self.closed = eof;
        self.previous_cr = text.ends_with('\r');
        Ok(StreamResult {
            syntax,
            origin,
            first_line,
            eof,
        })
    }
}

/// A bounded viewport projection. Each accepted byte is compared with the
/// viewport snapshot before verified source styling is copied into it.
pub struct ViewportProjection {
    source: bareline_document::DocumentSnapshot,
    origin: usize,
    covered: usize,
    spans: Vec<crate::StyleSpan>,
    language: Language,
    first_line: Option<usize>,
}
impl ViewportProjection {
    pub fn new(
        source: bareline_document::DocumentSnapshot,
        origin: TextOffset,
        language: Language,
    ) -> Result<Self, Error> {
        if source.len() > MAX_REQUEST_BYTES {
            return Err(Error::BudgetExceeded);
        }
        Ok(Self {
            source,
            origin: origin.0,
            covered: 0,
            spans: Vec::new(),
            language,
            first_line: None,
        })
    }
    /// The caller retains the one authoritative visible-piece map; this builder
    /// only validates/copies each supplied segment and stores no second map.
    pub fn for_projection(
        source: bareline_document::DocumentSnapshot,
        language: Language,
    ) -> Result<Self, Error> {
        Self::new(source, TextOffset(0), language)
    }
    pub fn accept(&mut self, window: &StreamResult) -> Result<(), Error> {
        self.accept_segment(
            window,
            TextOffset(self.origin)..TextOffset(self.origin + self.source.len()),
            TextOffset(0)..TextOffset(self.source.len()),
        )
    }
    pub fn accept_segment(
        &mut self,
        window: &StreamResult,
        source: std::ops::Range<TextOffset>,
        local: std::ops::Range<TextOffset>,
    ) -> Result<(), Error> {
        if source.start > source.end
            || local.start > local.end
            || source.end.0 - source.start.0 != local.end.0 - local.start.0
            || local.end.0 > self.source.len()
        {
            return Err(Error::InvalidRange);
        }
        let start = source.start.0.max(window.origin.0);
        let end = source
            .end
            .0
            .min(window.origin.0 + window.syntax.range.end.0);
        if start >= end {
            return Ok(());
        }
        let projected = local.start.0 + start - source.start.0;
        if projected != self.covered {
            return Err(Error::StaleCheckpoint);
        }
        let in_window = start - window.origin.0;
        let expected = self
            .source
            .read(
                TextOffset(projected)..TextOffset(projected + end - start),
                MAX_REQUEST_BYTES,
            )
            .map_err(|_| Error::InvalidRange)?;
        let actual = window
            .syntax
            .source
            .read(
                TextOffset(in_window)..TextOffset(end - window.origin.0),
                MAX_REQUEST_BYTES,
            )
            .map_err(|_| Error::InvalidRange)?;
        if expected != actual {
            return Err(Error::StaleCheckpoint);
        }
        if self.first_line.is_none() {
            self.first_line = Some(
                window.first_line
                    + window
                        .syntax
                        .source
                        .line_at(TextOffset(in_window))
                        .map_err(|_| Error::InvalidRange)?,
            );
        }
        for span in &window.syntax.spans {
            let a = (window.origin.0 + span.range.start.0).max(start);
            let b = (window.origin.0 + span.range.end.0).min(end);
            if a < b {
                if self.spans.len() >= crate::MAX_SPANS {
                    return Err(Error::BudgetExceeded);
                }
                self.spans.push(crate::StyleSpan {
                    range: TextOffset(local.start.0 + a - source.start.0)
                        ..TextOffset(local.start.0 + b - source.start.0),
                    kind: span.kind,
                });
            }
        }
        self.covered = projected + end - start;
        Ok(())
    }
    pub fn finish(self) -> Result<(SyntaxResult, usize), Error> {
        if self.covered != self.source.len() {
            return Err(Error::InvalidRange);
        }
        let end = self.source.len();
        Ok((
            SyntaxResult {
                source: self.source,
                language: self.language,
                range: TextOffset(0)..TextOffset(end),
                spans: self.spans,
                status: crate::Status::Complete,
                checkpoint: None,
                checkpoints: Vec::new(),
                fold_levels: None,
                fold_pairs: Vec::new(),
                indent_folding: false,
            },
            self.first_line.unwrap_or(0),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(text: &str) -> bareline_document::DocumentSnapshot {
        Document::from_utf8(text, Budget::new(1 << 20), Budget::new(1 << 20))
            .unwrap()
            .snapshot()
    }
    #[test]
    fn streaming_comments_and_fold_headers_keep_absolute_context() {
        let mut lexer = StreamLexer::new(Language::Rust, LexerPreference::Native, None);
        let first = "fn main() { /* comment\n";
        let second = "still comment */\n}\n";
        let one = lexer
            .advance(first, TextOffset(0), false, &Cancellation::default())
            .unwrap();
        let two = lexer
            .advance(
                second,
                TextOffset(first.len()),
                true,
                &Cancellation::default(),
            )
            .unwrap();
        assert_eq!(two.first_line, 1);
        assert!(
            two.syntax
                .spans
                .iter()
                .any(|span| span.range.start.0 == 0 && span.kind == crate::StyleKind::Comment)
        );
        let mut folds = crate::folding::FoldAccumulator::default();
        folds.advance_stream(&one, 32).unwrap();
        folds.advance_stream(&two, 32).unwrap();
        let anchor = folds
            .anchored()
            .iter()
            .find(|anchor| anchor.fold.header == 0)
            .unwrap();
        assert_eq!(anchor.header, TextOffset(0));
        assert_eq!(
            anchor.body,
            TextOffset(first.len())..TextOffset(first.len() + second.len())
        );
        assert!(
            folds
                .known()
                .iter()
                .any(|fold| fold.header == 0 && fold.end == 2)
        );
        assert!(
            lexer
                .advance(
                    "",
                    TextOffset(first.len() + second.len()),
                    true,
                    &Cancellation::default()
                )
                .is_err()
        );
    }
    #[test]
    fn sparse_checkpoints_follow_global_lines_across_windows() {
        let mut lexer = StreamLexer::new(Language::Rust, LexerPreference::Native, None);
        let first = "x\n".repeat(200);
        lexer
            .advance(&first, TextOffset(0), false, &Cancellation::default())
            .unwrap();
        let second = lexer
            .advance(
                &"x\n".repeat(100),
                TextOffset(first.len()),
                true,
                &Cancellation::default(),
            )
            .unwrap();
        assert!(
            second
                .syntax
                .checkpoints
                .iter()
                .any(|checkpoint| checkpoint.offset() == TextOffset(112))
        );
    }
    #[test]
    fn projection_compares_actual_bytes_and_rejects_mismatched_view() {
        let mut lexer = StreamLexer::new(Language::Rust, LexerPreference::Native, None);
        let one = lexer
            .advance("/* note\n", TextOffset(0), false, &Cancellation::default())
            .unwrap();
        let two = lexer
            .advance("text */\n", TextOffset(8), true, &Cancellation::default())
            .unwrap();
        let mut projection =
            ViewportProjection::new(source("note\ntext"), TextOffset(3), Language::Rust).unwrap();
        projection.accept(&one).unwrap();
        projection.accept(&two).unwrap();
        let (result, line) = projection.finish().unwrap();
        assert_eq!(line, 0);
        assert!(
            result
                .spans
                .iter()
                .all(|span| span.kind == crate::StyleKind::Comment)
        );
        let mut wrong =
            ViewportProjection::new(source("wrong"), TextOffset(3), Language::Rust).unwrap();
        assert!(wrong.accept(&one).is_err());
    }
    #[test]
    fn cancellation_poison_and_split_crlf_are_unavailable() {
        let mut lexer = StreamLexer::new(Language::Python, LexerPreference::Native, None);
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(lexer.advance("x\n", TextOffset(0), false, &cancel).is_err());
        assert!(
            lexer
                .advance("x\n", TextOffset(0), false, &Cancellation::default())
                .is_err()
        );
        let mut lexer = StreamLexer::new(Language::Python, LexerPreference::Native, None);
        lexer
            .advance("x\r", TextOffset(0), false, &Cancellation::default())
            .unwrap();
        assert!(
            lexer
                .advance("\ny", TextOffset(2), true, &Cancellation::default())
                .is_err()
        );
    }
    #[test]
    fn projection_styles_two_visible_pieces_without_lexing_the_join() {
        let text = "/* hidden */let x = 1;";
        let mut lexer = StreamLexer::new(Language::Rust, LexerPreference::Native, None);
        let window = lexer
            .advance(text, TextOffset(0), true, &Cancellation::default())
            .unwrap();
        let mut projection =
            ViewportProjection::for_projection(source("/*let"), Language::Rust).unwrap();
        projection
            .accept_segment(
                &window,
                TextOffset(0)..TextOffset(2),
                TextOffset(0)..TextOffset(2),
            )
            .unwrap();
        projection
            .accept_segment(
                &window,
                TextOffset(12)..TextOffset(15),
                TextOffset(2)..TextOffset(5),
            )
            .unwrap();
        let (result, _) = projection.finish().unwrap();
        assert!(
            result
                .spans
                .iter()
                .any(|span| span.kind == crate::StyleKind::Comment
                    && span.range.end <= TextOffset(2))
        );
        assert!(result.spans.iter().any(
            |span| span.kind == crate::StyleKind::Keyword && span.range.start >= TextOffset(2)
        ));
    }
}
