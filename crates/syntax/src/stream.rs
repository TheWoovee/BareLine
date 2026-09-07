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
        if origin.0 != self.next
            || text.len() > MAX_REQUEST_BYTES
            || (!eof && !text.ends_with('\n') && !text.ends_with('\r'))
        {
            return Err(Error::InvalidRange);
        }
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
        Ok(StreamResult {
            syntax,
            origin,
            first_line,
            eof,
        })
    }
}
