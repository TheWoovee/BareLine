// SPDX-License-Identifier: MPL-2.0
//! Bounded native fallback highlighting. Call `lex` off the UI thread.
//! Checkpoints are opaque and tied to document identity, revision and language.
pub mod catalog;
pub mod folding;
mod lexilla;
pub mod outline;
pub mod service;
pub mod udl;
use bareline_document::{DocumentSnapshot, TextOffset};
pub use service::{SubmitError, SyntaxTicket, SyntaxWorker};
use std::{
    ops::Range,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
pub const MAX_SPANS: usize = 32 * 1024;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LexerPreference {
    #[default]
    Lexilla,
    Native,
}
#[derive(Clone, Default)]
struct LexOptions {
    line_origin: usize,
    preference: LexerPreference,
    definition: Option<Arc<udl::Definition>>,
}

/// A worker-local verified forward pass. Opaque Lexilla state stays in this
/// object; callers can cancel between bounded windows and cannot seek it.
pub struct ForwardLexer {
    source: DocumentSnapshot,
    language: Language,
    next: TextOffset,
    checkpoint: Option<Checkpoint>,
    native: Option<bareline_lexilla_bridge::LexerSession>,
    options: LexOptions,
}
impl ForwardLexer {
    pub fn new(source: DocumentSnapshot, language: Language) -> Self {
        Self::configured(source, language, LexerPreference::Lexilla, None)
    }
    pub fn configured(
        source: DocumentSnapshot,
        language: Language,
        preference: LexerPreference,
        definition: Option<Arc<udl::Definition>>,
    ) -> Self {
        use bareline_lexilla_bridge::CppMode;
        let mode = match language {
            Language::JavaScript | Language::TypeScript => CppMode::JavaScript,
            Language::Go => CppMode::Go,
            Language::Java => CppMode::Java,
            Language::CSharp => CppMode::CSharp,
            _ => CppMode::Default,
        };
        Self {
            source,
            language,
            next: TextOffset(0),
            checkpoint: None,
            native: if preference == LexerPreference::Lexilla && definition.is_none() {
                bareline_lexilla_bridge::LexerSession::new(
                    language.metadata().lexilla,
                    language.metadata().keywords,
                    mode,
                )
                .ok()
            } else {
                None
            },
            options: LexOptions {
                line_origin: 0,
                preference,
                definition,
            },
        }
    }
    pub fn advance(&mut self, end: TextOffset, cancel: &Cancellation) -> Result<SyntaxResult, Error> {
        let result = lex_configured(
            self.source.clone(),
            self.language,
            self.next..end,
            self.checkpoint.as_ref(),
            cancel,
            self.native.as_mut(),
            self.options.clone(),
        )?;
        self.next = end;
        self.checkpoint = result.checkpoint.clone();
        if result.fold_levels.is_none() {
            self.native = None;
        }
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    PlainText,
    Rust,
    Json,
    C,
    Cpp,
    CSharp,
    Java,
    JavaScript,
    TypeScript,
    Python,
    Go,
    Html,
    Css,
    Xml,
    Sql,
    Toml,
}
impl Language {
    /// Filename detection only. Explicit/workspace overrides are caller-owned.
    pub fn detect(path: &Path) -> Self {
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        catalog::CATALOG
            .iter()
            .find(|entry| entry.extensions.iter().any(|ext| ext.eq_ignore_ascii_case(extension)))
            .map_or(Self::PlainText, |entry| entry.language)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleKind {
    Keyword,
    String,
    Number,
    Comment,
    Operator,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleSpan {
    pub range: Range<TextOffset>,
    pub kind: StyleKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Complete,
    Provisional,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Cancelled,
    InvalidRange,
    BudgetExceeded,
    StaleCheckpoint,
}

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    fn check(&self) -> Result<(), Error> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Copy)]
enum State {
    Normal,
    Block(usize),
    Quote { escaped: bool, delimiter: char },
    Raw(usize),
}
#[derive(Clone)]
pub struct Checkpoint {
    source: DocumentSnapshot,
    language: Language,
    offset: TextOffset,
    state: State,
    definition: Option<Arc<udl::Definition>>,
}
impl Checkpoint {
    pub fn offset(&self) -> TextOffset {
        self.offset
    }
}
#[derive(Clone)]
pub struct SyntaxResult {
    source: DocumentSnapshot,
    pub language: Language,
    pub range: Range<TextOffset>,
    pub spans: Vec<StyleSpan>,
    pub status: Status,
    pub checkpoint: Option<Checkpoint>,
    /// Sparse native fallback restart points, every 256 logical lines. These
    /// never serialize or claim to restore private Lexilla state.
    pub checkpoints: Vec<Checkpoint>,
    // Present only when actual Lexilla produced this verified window.
    pub(crate) fold_levels: Option<Vec<i32>>,
    pub(crate) fold_pairs: Vec<(char, char)>,
    pub(crate) indent_folding: bool,
}
impl SyntaxResult {
    pub fn is_current(&self, snapshot: &DocumentSnapshot) -> bool {
        self.source.same_document(snapshot) && self.source.revision == snapshot.revision
    }
}

/// A request may start at zero or exactly at an opaque checkpoint. Without a
/// checkpoint, nonzero requests return conservative plain text, never guessed
/// string/comment state. Checkpoints are emitted only at newline/EOF boundaries.
pub fn lex(
    source: DocumentSnapshot,
    language: Language,
    range: Range<TextOffset>,
    checkpoint: Option<&Checkpoint>,
    cancel: &Cancellation,
) -> Result<SyntaxResult, Error> {
    lex_with_session(source, language, range, checkpoint, cancel, None)
}
pub fn lex_udl(
    source: DocumentSnapshot,
    definition: Arc<udl::Definition>,
    range: Range<TextOffset>,
    checkpoint: Option<&Checkpoint>,
    cancel: &Cancellation,
) -> Result<SyntaxResult, Error> {
    definition.validate()?;
    lex_configured(
        source,
        Language::PlainText,
        range,
        checkpoint,
        cancel,
        None,
        LexOptions {
            line_origin: 0,
            preference: LexerPreference::Native,
            definition: Some(definition),
        },
    )
}
fn lex_with_session(
    source: DocumentSnapshot,
    language: Language,
    range: Range<TextOffset>,
    checkpoint: Option<&Checkpoint>,
    cancel: &Cancellation,
    session: Option<&mut bareline_lexilla_bridge::LexerSession>,
) -> Result<SyntaxResult, Error> {
    lex_configured(
        source,
        language,
        range,
        checkpoint,
        cancel,
        session,
        LexOptions::default(),
    )
}
fn lex_configured(
    source: DocumentSnapshot,
    language: Language,
    range: Range<TextOffset>,
    checkpoint: Option<&Checkpoint>,
    cancel: &Cancellation,
    session: Option<&mut bareline_lexilla_bridge::LexerSession>,
    options: LexOptions,
) -> Result<SyntaxResult, Error> {
    let definition = options.definition;
    if let Some(definition) = &definition {
        definition.validate()?;
    }
    cancel.check()?;
    if range.start > range.end
        || range.end.0 > source.len()
        || !source.is_boundary(range.start)
        || !source.is_boundary(range.end)
    {
        return Err(Error::InvalidRange);
    }
    if range.end.0 - range.start.0 > MAX_REQUEST_BYTES {
        return Err(Error::BudgetExceeded);
    }
    let mut state = if let Some(cp) = checkpoint {
        if !cp.source.same_document(&source)
            || cp.source.revision != source.revision
            || cp.language != language
            || cp.offset != range.start
            || match (&cp.definition, &definition) {
                (None, None) => false,
                (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
                _ => true,
            }
        {
            return Err(Error::StaleCheckpoint);
        }
        cp.state
    } else {
        State::Normal
    };
    if range.start.0 != 0
        && checkpoint.is_none()
        && session.is_none()
        && (language != Language::PlainText || definition.is_some())
    {
        return Ok(SyntaxResult {
            source,
            language,
            range,
            spans: vec![],
            status: Status::Provisional,
            checkpoint: None,
            checkpoints: Vec::new(),
            fold_levels: None,
            fold_pairs: Vec::new(),
            indent_folding: false,
        });
    }
    let text = source
        .read(range.clone(), MAX_REQUEST_BYTES)
        .map_err(|_| Error::InvalidRange)?;
    let mut spans = Vec::new();
    let mut checkpoints = Vec::new();
    let mut line = options.line_origin + source.line_at(range.start).map_err(|_| Error::InvalidRange)?;
    let mut i = 0;
    let custom = definition.as_deref();
    let custom_keywords: std::collections::BTreeSet<&str> = custom
        .into_iter()
        .flat_map(|d| d.keywords.iter().map(String::as_str))
        .collect();
    let line_comment = custom.map_or(language.metadata().line_comment, |d| d.line_comment.as_deref());
    let block_comment = custom.map_or(language.metadata().block_comment, |d| {
        d.block_comment.as_ref().map(|(a, b)| (a.as_str(), b.as_str()))
    });
    while i < text.len() && (language != Language::PlainText || custom.is_some()) {
        cancel.check()?;
        let start = i;
        let rest = &text[i..];
        let kind = match state {
            State::Block(depth) => {
                let (open, close) = block_comment.unwrap_or(("/*", "*/"));
                if language == Language::Rust && rest.starts_with(open) {
                    state = State::Block(depth + 1);
                    i += open.len();
                } else if rest.starts_with(close) {
                    state = if depth == 1 {
                        State::Normal
                    } else {
                        State::Block(depth - 1)
                    };
                    i += close.len();
                } else {
                    i += rest.chars().next().unwrap().len_utf8();
                }
                Some(StyleKind::Comment)
            }
            State::Quote { escaped, delimiter } => {
                let c = rest.chars().next().unwrap();
                i += c.len_utf8();
                state = if escaped {
                    State::Quote {
                        escaped: false,
                        delimiter,
                    }
                } else if c == delimiter {
                    State::Normal
                } else {
                    State::Quote {
                        escaped: c == '\\',
                        delimiter,
                    }
                };
                Some(StyleKind::String)
            }
            State::Raw(hashes) => {
                if rest.starts_with('"')
                    && rest
                        .as_bytes()
                        .get(1..1 + hashes)
                        .is_some_and(|s| s.iter().all(|b| *b == b'#'))
                {
                    i += 1 + hashes;
                    state = State::Normal;
                } else {
                    i += rest.chars().next().unwrap().len_utf8();
                }
                Some(StyleKind::String)
            }
            State::Normal => {
                if let Some(token) = line_comment.filter(|token| rest.starts_with(token)) {
                    i += token.len();
                    while i < text.len() && !matches!(text.as_bytes()[i], b'\r' | b'\n') {
                        cancel.check()?;
                        i += text[i..].chars().next().unwrap().len_utf8();
                    }
                    Some(StyleKind::Comment)
                } else if let Some((open, _)) = block_comment.filter(|(open, _)| rest.starts_with(open)) {
                    i += open.len();
                    state = State::Block(1);
                    Some(StyleKind::Comment)
                } else if language == Language::Rust && raw_start(rest).is_some() {
                    let (length, hashes) = raw_start(rest).unwrap();
                    i += length;
                    state = State::Raw(hashes);
                    Some(StyleKind::String)
                } else if custom.is_some_and(|d| d.strings.contains(&rest.chars().next().unwrap()))
                    || (custom.is_none()
                        && (rest.starts_with('"')
                            || (language != Language::Rust && language != Language::Json && rest.starts_with('\''))
                            || (matches!(language, Language::JavaScript | Language::TypeScript | Language::Go)
                                && rest.starts_with('`'))))
                {
                    let delimiter = rest.chars().next().unwrap();
                    i += delimiter.len_utf8();
                    state = State::Quote {
                        escaped: false,
                        delimiter,
                    };
                    Some(StyleKind::String)
                } else if language == Language::Rust && char_literal_length(rest).is_some() {
                    i += char_literal_length(rest).unwrap();
                    Some(StyleKind::String)
                } else {
                    let c = rest.chars().next().unwrap();
                    i += c.len_utf8();
                    if c.is_ascii_digit()
                        || (language == Language::Json
                            && c == '-'
                            && text.as_bytes().get(i).is_some_and(u8::is_ascii_digit))
                    {
                        // Keep signs only after exponents, and stop before Rust's range operator.
                        while i < text.len() {
                            cancel.check()?;
                            let b = text.as_bytes()[i];
                            let accepted = b.is_ascii_alphanumeric()
                                || b == b'_'
                                || (b == b'.'
                                    && !text[i..].starts_with("..")
                                    && text.as_bytes().get(i + 1).is_some_and(u8::is_ascii_digit))
                                || (matches!(b, b'+' | b'-') && matches!(text.as_bytes()[i - 1], b'e' | b'E'));
                            if !accepted {
                                break;
                            }
                            i += 1;
                        }
                        Some(StyleKind::Number)
                    } else if c == '_' || c.is_alphabetic() {
                        while i < text.len() {
                            cancel.check()?;
                            let next = text[i..].chars().next().unwrap();
                            if next != '_' && !next.is_alphanumeric() {
                                break;
                            }
                            i += next.len_utf8();
                        }
                        let word = &text[start..i];
                        let keyword = custom.map_or_else(
                            || language.metadata().keywords.split_ascii_whitespace().any(|k| k == word),
                            |_| custom_keywords.contains(word),
                        );
                        keyword.then_some(StyleKind::Keyword)
                    } else {
                        custom
                            .map_or("{}[]():;,.+-*/%=!<>|&^?~", |d| d.operators.as_str())
                            .contains(c)
                            .then_some(StyleKind::Operator)
                    }
                }
            }
        };
        if text
            .as_bytes()
            .get(i.wrapping_sub(1))
            .is_some_and(|b| *b == b'\n' || (*b == b'\r' && text.as_bytes().get(i) != Some(&b'\n')))
        {
            line += 1;
            if line.is_multiple_of(256) {
                checkpoints.push(Checkpoint {
                    source: source.clone(),
                    language,
                    offset: TextOffset(range.start.0 + i),
                    state,
                    definition: definition.clone(),
                });
            }
        }
        if let Some(kind) = kind {
            let absolute = TextOffset(range.start.0 + start)..TextOffset(range.start.0 + i);
            if let Some(last) = spans
                .last_mut()
                .filter(|last: &&mut StyleSpan| last.kind == kind && last.range.end == absolute.start)
            {
                last.range.end = absolute.end;
            } else {
                if spans.len() == MAX_SPANS {
                    return Err(Error::BudgetExceeded);
                }
                spans.push(StyleSpan { range: absolute, kind });
            }
        }
    }
    cancel.check()?;
    let at_boundary = range.end.0 == source.len() || text.ends_with(['\r', '\n']);
    let fallback_verified =
        range.start.0 == 0 || checkpoint.is_some() || (language == Language::PlainText && definition.is_none());
    let checkpoint = (at_boundary && fallback_verified).then(|| Checkpoint {
        source: source.clone(),
        language,
        offset: range.end,
        state,
        definition: definition.clone(),
    });
    if !fallback_verified {
        checkpoints.clear();
    }
    // Lexilla is primary at a verified document origin. Native checkpoints retain
    // their own state and are never misrepresented as Lexilla continuation state.
    let mut fold_levels = None;
    if options.preference == LexerPreference::Lexilla
        && (range.start.0 == 0 || session.is_some())
        && language != Language::PlainText
    {
        let lexer = language.metadata().lexilla;
        let mode = match language {
            Language::JavaScript | Language::TypeScript => bareline_lexilla_bridge::CppMode::JavaScript,
            Language::Go => bareline_lexilla_bridge::CppMode::Go,
            Language::Java => bareline_lexilla_bridge::CppMode::Java,
            Language::CSharp => bareline_lexilla_bridge::CppMode::CSharp,
            _ => bareline_lexilla_bridge::CppMode::Default,
        };
        let styled = if let Some(session) = session {
            session.advance(&text, range.start.0, &|| cancel.is_cancelled())
        } else {
            bareline_lexilla_bridge::lex_with_mode(
                &text,
                lexer,
                language.metadata().keywords,
                0,
                0,
                &|| cancel.is_cancelled(),
                mode,
            )
        };
        match styled {
            Ok(styled) => {
                spans = lexilla::spans(&text, language, &styled.styles)?;
                for span in &mut spans {
                    span.range.start.0 += range.start.0;
                    span.range.end.0 += range.start.0;
                }
                fold_levels = Some(styled.fold_levels);
            }
            Err(bareline_lexilla_bridge::Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => {} // The bounded native result remains usable if Lexilla fails.
        }
    }
    let status = if fallback_verified || fold_levels.is_some() {
        Status::Complete
    } else {
        spans.clear();
        Status::Provisional
    };
    Ok(SyntaxResult {
        source,
        language,
        range,
        spans,
        status,
        checkpoint,
        checkpoints,
        fold_levels,
        fold_pairs: definition.as_ref().map_or_else(
            || vec![('{', '}'), ('[', ']')],
            |definition| definition.fold_pairs.clone(),
        ),
        indent_folding: language == Language::Python && definition.is_none(),
    })
}

fn raw_start(text: &str) -> Option<(usize, usize)> {
    let prefix = if text.starts_with("br") || text.starts_with("cr") {
        2
    } else if text.starts_with('r') {
        1
    } else {
        return None;
    };
    let hashes = text.as_bytes()[prefix..]
        .iter()
        .take(256)
        .take_while(|b| **b == b'#')
        .count();
    if hashes > 255 || text.as_bytes().get(prefix + hashes) != Some(&b'"') {
        return None;
    }
    Some((prefix + hashes + 1, hashes))
}
fn char_literal_length(text: &str) -> Option<usize> {
    if !text.starts_with('\'') {
        return None;
    }
    let mut chars = text[1..].char_indices();
    let (_, first) = chars.next()?;
    if matches!(first, '\r' | '\n' | '\'') {
        return None;
    }
    if first != '\\' {
        let (at, c) = chars.next()?;
        return (c == '\'').then_some(at + 2);
    }
    // Rust escapes are bounded: one escaped scalar, xNN, or u{1..6 hex digits}.
    let (at, c) = chars.next()?;
    if c == 'u' && text.as_bytes().get(at + 2) == Some(&b'{') {
        let tail = &text[at + 3..];
        let n = tail.bytes().take_while(u8::is_ascii_hexdigit).take(7).count();
        return (n > 0 && n <= 6 && tail[n..].starts_with("}'")).then_some(at + 3 + n + 2);
    }
    let end = if c == 'x' { at + 4 } else { at + 1 + c.len_utf8() };
    (text.as_bytes().get(end) == Some(&b'\'')).then_some(end + 1)
}
const RUST_KEYWORDS: &str = "as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while abstract become box do final macro override priv typeof unsized virtual yield try union gen";

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document, Edit, EditTransaction};
    #[test]
    fn configured_udl_carries_comments_and_custom_folds_and_rejects_replacement_checkpoint() {
        let definition = Arc::new(udl::Definition {
            version: 1,
            id: "angle".into(),
            name: "Angle".into(),
            extensions: vec!["angle".into()],
            keywords: vec!["begin".into()],
            operators: "<>".into(),
            line_comment: Some("#".into()),
            block_comment: Some(("/*".into(), "*/".into())),
            strings: vec!['"'],
            fold_pairs: vec![('<', '>')],
        });
        let first = "begin <\n/* hidden\n";
        let text = format!("{first}> */\nvalue\n>\n");
        let source = document(&text).snapshot();
        let mut pass = ForwardLexer::configured(
            source.clone(),
            Language::PlainText,
            LexerPreference::Native,
            Some(definition.clone()),
        );
        let one = pass.advance(TextOffset(first.len()), &Cancellation::default()).unwrap();
        let two = pass.advance(TextOffset(text.len()), &Cancellation::default()).unwrap();
        let mut folds = folding::FoldAccumulator::default();
        folds.advance(&source, &one, 10).unwrap();
        folds.advance(&source, &two, 10).unwrap();
        assert_eq!(
            folds.known(),
            &[folding::Fold {
                header: 0,
                end: 4,
                level: 1
            }]
        );
        assert!(
            two.spans
                .iter()
                .any(|span| span.kind == StyleKind::Comment && span.range.start == TextOffset(first.len()))
        );
        assert!(matches!(
            lex_udl(
                source,
                Arc::new((*definition).clone()),
                TextOffset(first.len())..TextOffset(text.len()),
                one.checkpoint.as_ref(),
                &Cancellation::default()
            ),
            Err(Error::StaleCheckpoint)
        ));
    }
    #[test]
    fn all_fifteen_native_definitions_validate_and_native_python_folds() {
        for entry in catalog::CATALOG {
            entry.native_definition().validate().unwrap();
        }
        let text = "def f():\n    value = 1\n    return value\nother = 2\n";
        let source = document(text).snapshot();
        let mut pass = ForwardLexer::configured(source.clone(), Language::Python, LexerPreference::Native, None);
        let result = pass.advance(TextOffset(text.len()), &Cancellation::default()).unwrap();
        assert!(result.fold_levels.is_none());
        assert_eq!(
            folding::folds(&source, &result, 10).unwrap(),
            vec![folding::Fold {
                header: 0,
                end: 2,
                level: 1
            }]
        );
    }
    fn document(text: &str) -> Document {
        Document::from_utf8(text, Budget::new(4 << 20), Budget::new(4 << 20)).unwrap()
    }
    fn styled(source: &DocumentSnapshot, result: &SyntaxResult, kind: StyleKind) -> Vec<String> {
        result
            .spans
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| source.read(s.range.clone(), MAX_REQUEST_BYTES).unwrap())
            .collect()
    }
    fn curly_quote_definition() -> Arc<udl::Definition> {
        Arc::new(udl::Definition {
            version: 1,
            id: "curly".into(),
            name: "Curly".into(),
            extensions: vec!["curly".into()],
            keywords: vec!["begin".into()],
            operators: "<>".into(),
            line_comment: Some("#".into()),
            block_comment: None,
            strings: vec!['\u{201c}', '\u{201d}', '\u{ab}', '\u{bb}'],
            fold_pairs: Vec::new(),
        })
    }
    #[test]
    fn udl_multibyte_string_delimiters_do_not_split_a_scalar() {
        let text = "begin \u{201c}hello \u{1f642}\u{201d} tail\n\u{ab}guillemet\u{bb}\n";
        let source = document(text).snapshot();
        let result = lex_udl(
            source.clone(),
            curly_quote_definition(),
            TextOffset(0)..TextOffset(text.len()),
            None,
            &Cancellation::default(),
        )
        .unwrap();
        let strings = styled(&source, &result, StyleKind::String);
        assert!(
            strings.iter().any(|s| s.contains("hello")),
            "curly-quoted run was not highlighted: {strings:?}"
        );
        assert!(strings.iter().any(|s| s.contains("guillemet")), "{strings:?}");
    }
    #[cfg(debug_assertions)]
    #[test]
    fn udl_tokenizer_survives_random_utf8_inputs() {
        // Cheap deterministic PRNG; 200 random UTF-8 strings through the
        // tokenizer must never panic on a mid-scalar slice.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabet = [
            'a',
            'b',
            ' ',
            '\n',
            '#',
            '<',
            '>',
            '\\',
            '"',
            '\'',
            '\u{201c}',
            '\u{201d}',
            '\u{ab}',
            '\u{bb}',
            '\u{1f642}',
            '\u{301}',
            '\u{4e2d}',
            '0',
            '.',
            '9',
        ];
        let definition = curly_quote_definition();
        for _ in 0..200 {
            let length = (next() % 96) as usize;
            let text: String = (0..length)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect();
            let source = document(&text).snapshot();
            let _ = lex_udl(
                source,
                definition.clone(),
                TextOffset(0)..TextOffset(text.len()),
                None,
                &Cancellation::default(),
            );
        }
    }
    #[test]
    fn rust_utf8_raw_nested_comments_and_lifetimes() {
        let text = "pub fn f<'a>(s: &'a str) { let crab = '🦀'; let n = 0..10; let raw = r##\"/*fake*/\"##; /* outer /* inner */ end */ }";
        let source = document(text).snapshot();
        let result = lex(
            source.clone(),
            Language::Rust,
            TextOffset(0)..TextOffset(text.len()),
            None,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(
            styled(&source, &result, StyleKind::String),
            ["'🦀'", "r##\"/*fake*/\"##"]
        );
        assert_eq!(
            styled(&source, &result, StyleKind::Comment),
            ["/* outer /* inner */ end */"]
        );
        assert_eq!(styled(&source, &result, StyleKind::Number), ["0", "10"]);
        assert!(
            result
                .spans
                .iter()
                .all(|s| source.is_boundary(s.range.start) && source.is_boundary(s.range.end))
        );
        assert!(result.spans.windows(2).all(|s| s[0].range.end <= s[1].range.start));
    }
    #[test]
    fn verified_multiline_checkpoint_and_stale_rejection() {
        let text = "let s = r#\"hello\nworld\"#;\n/* first\nsecond */ let n = 1;\n";
        let mut doc = document(text);
        let source = doc.snapshot();
        let split = text.find('\n').unwrap() + 1;
        let first = lex(
            source.clone(),
            Language::Rust,
            TextOffset(0)..TextOffset(split),
            None,
            &Cancellation::default(),
        )
        .unwrap();
        let provisional = lex(
            source.clone(),
            Language::Rust,
            TextOffset(split)..TextOffset(text.len()),
            None,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(provisional.status, Status::Provisional);
        assert!(provisional.spans.is_empty() && provisional.checkpoint.is_none());
        let second = lex(
            source.clone(),
            Language::Rust,
            provisional.range.clone(),
            first.checkpoint.as_ref(),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(second.status, Status::Complete);
        assert_eq!(styled(&source, &second, StyleKind::String), ["world\"#"]);
        assert_eq!(styled(&source, &second, StyleKind::Comment), ["/* first\nsecond */"]);
        let foreign = document(text).snapshot();
        assert_eq!(
            lex(
                foreign,
                Language::Rust,
                provisional.range.clone(),
                first.checkpoint.as_ref(),
                &Cancellation::default()
            )
            .err(),
            Some(Error::StaleCheckpoint)
        );
        doc.apply(EditTransaction {
            base_revision: source.revision,
            edits: vec![Edit {
                range: TextOffset(0)..TextOffset(0),
                insert: " ".into(),
            }],
        })
        .unwrap();
        assert!(!second.is_current(&doc.snapshot()));
        assert_eq!(
            lex(
                doc.snapshot(),
                Language::Rust,
                provisional.range,
                first.checkpoint.as_ref(),
                &Cancellation::default()
            )
            .err(),
            Some(Error::StaleCheckpoint)
        );
    }
    #[test]
    fn json_and_resource_boundaries() {
        let text = "{\"é\": [true, null, -12.5e+3], \"escaped\": \"a\\\"b\"}";
        let source = document(text).snapshot();
        let result = lex(
            source.clone(),
            Language::Json,
            TextOffset(0)..TextOffset(text.len()),
            None,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(styled(&source, &result, StyleKind::Number), ["-12.5e+3"]);
        assert_eq!(styled(&source, &result, StyleKind::Keyword), ["true", "null"]);
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert_eq!(
            lex(source.clone(), Language::Json, result.range.clone(), None, &cancelled).err(),
            Some(Error::Cancelled)
        );
        assert_eq!(
            lex(
                source,
                Language::Json,
                TextOffset(3)..TextOffset(4),
                None,
                &Cancellation::default()
            )
            .err(),
            Some(Error::InvalidRange)
        );
        let large = document(&" ".repeat(MAX_REQUEST_BYTES + 1)).snapshot();
        assert_eq!(
            lex(
                large.clone(),
                Language::Rust,
                TextOffset(0)..TextOffset(large.len()),
                None,
                &Cancellation::default()
            )
            .err(),
            Some(Error::BudgetExceeded)
        );
        let dense = document(&"1 ".repeat(MAX_SPANS + 1)).snapshot();
        assert_eq!(
            lex(
                dense.clone(),
                Language::Rust,
                TextOffset(0)..TextOffset(dense.len()),
                None,
                &Cancellation::default()
            )
            .err(),
            Some(Error::BudgetExceeded)
        );
    }
    #[test]
    fn worker_delivers_current_request_after_superseding() {
        let worker = SyntaxWorker::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = tx.send(());
        });
        let large = document(&"let x = 1;\n".repeat(20_000)).snapshot();
        let old = worker
            .submit(
                large.clone(),
                Language::Rust,
                TextOffset(0)..TextOffset(large.len()),
                None,
                notify.clone(),
            )
            .unwrap();
        old.cancel();
        let source = document("{\"ok\": true}").snapshot();
        let current = worker
            .submit(
                source.clone(),
                Language::Json,
                TextOffset(0)..TextOffset(source.len()),
                None,
                notify,
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Ok(result) = current.try_recv() {
                let result = result.unwrap();
                assert!(result.is_current(&source));
                assert_eq!(result.status, Status::Complete);
                break;
            }
            rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .unwrap();
        }
    }
}

pub mod stream;
