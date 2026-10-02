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
/// Every span covers at least one byte, so a full request window always fits.
/// Dense text (pretty-printed JSON, minified code) must never fail a window.
pub const MAX_SPANS: usize = MAX_REQUEST_BYTES;
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
/// object; callers can cancel between bounded windows and cannot seek it (only
/// the worker rewinds it to a carried checkpoint, see `restart`).
pub struct ForwardLexer {
    source: DocumentSnapshot,
    language: Language,
    next: TextOffset,
    checkpoint: Option<Checkpoint>,
    native: Option<bareline_lexilla_bridge::LexerSession>,
    options: LexOptions,
    /// A window failed part-way (cancelled), so `next` and `checkpoint` may
    /// not describe the session any more; only `restart` continues this pass.
    interrupted: bool,
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
            interrupted: false,
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
        )
        .inspect_err(|_| self.interrupted = true)?;
        self.next = end;
        self.checkpoint = result.checkpoint.clone();
        if result.fold_levels.is_none() {
            self.native = None;
        }
        Ok(result)
    }
    /// SRC-14: continue this pass on `source` from `checkpoint` instead of from
    /// byte 0. The primary lexer's session rewinds to its latest restart line at
    /// or before the checkpoint (`LexerSession::restart`) and re-lexes only the
    /// bytes from there to it, so later windows match a pass from zero. That
    /// holds while the text before the checkpoint is what this pass lexed: the
    /// checkpoint was emitted at or before this pass's revision and carried
    /// edit by edit to `source`, and `Checkpoint::rebase` keeps it only while
    /// every edit starts after it. Returns the bytes re-lexed, or `None` when
    /// the caller must start a pass from zero instead.
    fn restart(
        &mut self,
        source: &DocumentSnapshot,
        checkpoint: &Checkpoint,
        cancel: &Cancellation,
    ) -> Result<Option<usize>, Error> {
        let lexed = self.source.revision.0;
        let offset = checkpoint.offset.0;
        if !self.source.same_document(source)
            || !checkpoint.source.same_document(source)
            || checkpoint.source.revision != source.revision
            || checkpoint.language != self.language
            || checkpoint.definition.is_some()
            || self.options.definition.is_some()
            || self.options.preference != LexerPreference::Lexilla
            || checkpoint.born > lexed
            || lexed > source.revision.0
            || offset >= bareline_lexilla_bridge::SESSION_BYTES
        {
            return Ok(None);
        }
        let Some(session) = self.native.as_mut() else {
            return Ok(None);
        };
        // Until the rewind completes, `next` and `checkpoint` are stale.
        self.interrupted = true;
        let Ok(resumed) = session.restart(offset) else {
            return Ok(None);
        };
        let mut at = resumed;
        while at < offset {
            cancel.check()?;
            let mut end = offset.min(at.saturating_add(MAX_REQUEST_BYTES));
            if end < offset
                && let Ok(line) = source.line_at(TextOffset(end))
                && let Ok(range) = source.line_range(line)
                && range.start.0 > at
            {
                end = range.start.0;
            }
            while !source.is_boundary(TextOffset(end)) {
                end -= 1;
            }
            let text = source
                .read(TextOffset(at)..TextOffset(end), MAX_REQUEST_BYTES)
                .map_err(|_| Error::InvalidRange)?;
            match session.advance(&text, at, &|| cancel.is_cancelled()) {
                Ok(_) => at = end,
                Err(bareline_lexilla_bridge::Error::Cancelled) => return Err(Error::Cancelled),
                Err(_) => return Ok(None),
            }
        }
        self.source = source.clone();
        self.next = checkpoint.offset;
        self.checkpoint = Some(checkpoint.clone());
        self.interrupted = false;
        Ok(Some(offset - resumed))
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
    /// Revision of the pass that emitted this checkpoint. `rebase` keeps it, so
    /// the text before `offset` is the same in every revision from here to
    /// `source`'s (see `ForwardLexer::restart`).
    born: u64,
}
impl Checkpoint {
    pub fn offset(&self) -> TextOffset {
        self.offset
    }
    /// Carry this verified state to `next` when it directly follows this
    /// checkpoint's snapshot (PR-008). The state depends only on the bytes
    /// before `offset` plus one byte of CR/LF lookahead, so it survives exactly
    /// when the change starts after `offset`; anything at or after the edit is
    /// invalidated.
    pub fn rebase(&self, next: &DocumentSnapshot) -> Option<Checkpoint> {
        let first = first_change(&self.source, next)?;
        (self.offset.0 < first).then(|| Checkpoint {
            source: next.clone(),
            language: self.language,
            offset: self.offset,
            state: self.state,
            definition: self.definition.clone(),
            born: self.born,
        })
    }
}
/// First pre-edit byte the step from `previous` to `next` touches, or `None`
/// when `next` does not directly follow `previous` (another document, skipped
/// revisions, or no receipt), so callers must drop all state derived from it.
fn first_change(previous: &DocumentSnapshot, next: &DocumentSnapshot) -> Option<usize> {
    if !previous.same_document(next) {
        return None;
    }
    if previous.revision == next.revision {
        return Some(usize::MAX);
    }
    let change = next
        .applied_change()
        .filter(|change| change.matches_before(previous.identity_token(), previous.content_state))?;
    // Receipts list disjoint edits in ascending pre-edit order.
    Some(change.edits().first().map_or(usize::MAX, |edit| edit.before.start.0))
}
/// Maps ascending pre-edit offsets through disjoint ascending edits in one
/// pass. Text inserted at an offset, or replacing bytes around it, moves the
/// offset after the replacement, so a span that ends where text is typed grows
/// to cover it and a span wholly replaced collapses to nothing.
struct Carry<'a> {
    edits: &'a [bareline_document::change::CompactEdit],
    next: usize,
    delta: i128,
}
impl Carry<'_> {
    fn after(&mut self, offset: usize) -> usize {
        while let Some(edit) = self.edits.get(self.next).filter(|edit| edit.before.end.0 <= offset) {
            self.delta += edit.inserted_len as i128 - (edit.before.end.0 - edit.before.start.0) as i128;
            self.next += 1;
        }
        let mapped = match self.edits.get(self.next) {
            Some(edit) if edit.before.start.0 < offset => edit.before.start.0 as i128 + edit.inserted_len as i128,
            _ => offset as i128,
        };
        usize::try_from(mapped + self.delta).unwrap_or(0)
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
    /// never carry private Lexilla state; the worker's session keeps its own
    /// restart data at the same lines (see `ForwardLexer::restart`).
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
    /// A provisional stand-in for `next` while its fresh result is pending, so a
    /// view keeps the previous colors instead of flashing plain text. Spans are
    /// carried through the applied change (see `Carry`), only checkpoints
    /// before the change survive, and fold levels are dropped. `None` when `next`
    /// does not directly follow this result's snapshot.
    pub fn rebase(&self, next: &DocumentSnapshot) -> Option<SyntaxResult> {
        let first = first_change(&self.source, next)?;
        let edits: &[bareline_document::change::CompactEdit] = match next.applied_change() {
            Some(change) if first != usize::MAX => change.edits(),
            _ => &[],
        };
        let mut start = self.range.start.0 as i128;
        for edit in edits.iter().take_while(|edit| edit.before.start.0 < self.range.start.0) {
            // The range widens backward over a replacement that reaches into it.
            start = if edit.before.end.0 < self.range.start.0 {
                start + edit.inserted_len as i128 - (edit.before.end.0 - edit.before.start.0) as i128
            } else {
                start - (self.range.start.0 - edit.before.start.0) as i128
            };
        }
        let mut carry = Carry {
            edits,
            next: 0,
            delta: 0,
        };
        let mut spans = Vec::new();
        spans.try_reserve_exact(self.spans.len()).ok()?;
        for span in &self.spans {
            let range = TextOffset(carry.after(span.range.start.0))..TextOffset(carry.after(span.range.end.0));
            if range.start < range.end {
                spans.push(StyleSpan { range, kind: span.kind });
            }
        }
        let range = TextOffset(usize::try_from(start).ok()?)..TextOffset(carry.after(self.range.end.0));
        if range.start > range.end || range.end.0 > next.len() {
            return None;
        }
        Some(SyntaxResult {
            source: next.clone(),
            language: self.language,
            range,
            spans,
            status: Status::Provisional,
            checkpoint: self.checkpoint.as_ref().and_then(|c| c.rebase(next)),
            checkpoints: self.checkpoints.iter().filter_map(|c| c.rebase(next)).collect(),
            fold_levels: None,
            fold_pairs: self.fold_pairs.clone(),
            indent_folding: self.indent_folding,
        })
    }
    /// Lay this fresh result over a stand-in for the same snapshot: fresh spans
    /// replace the stand-in's inside this range, the stand-in keeps the rest, and
    /// the union stays provisional. Anything else returns this result unchanged.
    pub fn overlay(mut self, stand_in: &SyntaxResult) -> SyntaxResult {
        if !stand_in.is_current(&self.source) || stand_in.language != self.language {
            return self;
        }
        let (start, end) = (self.range.start, self.range.end);
        let before = stand_in
            .spans
            .iter()
            .filter(|span| span.range.start < start)
            .map(|span| StyleSpan {
                range: span.range.start..span.range.end.min(start),
                kind: span.kind,
            });
        let after = stand_in
            .spans
            .iter()
            .filter(|span| span.range.end > end)
            .map(|span| StyleSpan {
                range: span.range.start.max(end)..span.range.end,
                kind: span.kind,
            });
        self.spans = before.chain(std::mem::take(&mut self.spans)).chain(after).collect();
        self.range = start.min(stand_in.range.start)..end.max(stand_in.range.end);
        self.status = Status::Provisional;
        self.fold_levels = None;
        self
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
                    born: source.revision.0,
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
        born: source.revision.0,
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
                // A style-mapping error keeps the native spans; only Lexilla's
                // fold levels are dropped, which also retires the session.
                if let Ok(mut mapped) = lexilla::spans(&text, language, &styled.styles) {
                    for span in &mut mapped {
                        span.range.start.0 += range.start.0;
                        span.range.end.0 += range.start.0;
                    }
                    spans = mapped;
                    fold_levels = Some(styled.fold_levels);
                }
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
    fn insert(doc: &mut Document, at: usize, text: &str) -> DocumentSnapshot {
        doc.apply(EditTransaction {
            base_revision: doc.snapshot().revision,
            edits: vec![Edit {
                range: TextOffset(at)..TextOffset(at),
                insert: text.into(),
            }],
        })
        .unwrap();
        doc.snapshot()
    }
    /// Every checkpoint of a native forward pass in line-aligned windows.
    fn native_checkpoints(source: &DocumentSnapshot, stop: usize) -> Vec<Checkpoint> {
        let mut pass = ForwardLexer::configured(source.clone(), Language::Rust, LexerPreference::Native, None);
        let mut checkpoints = Vec::new();
        while pass.next.0 < stop.min(source.len()) {
            let mut end = source.len().min(pass.next.0 + MAX_REQUEST_BYTES);
            if end < source.len() {
                end = source
                    .line_range(source.line_at(TextOffset(end)).unwrap())
                    .unwrap()
                    .start
                    .0;
            }
            let result = pass.advance(TextOffset(end), &Cancellation::default()).unwrap();
            checkpoints.extend(result.checkpoints);
            checkpoints.extend(result.checkpoint);
        }
        checkpoints
    }
    #[test]
    fn rebased_checkpoints_survive_only_before_the_edit_and_resume_like_a_full_pass() {
        // SRC-14 (PR-008): an edit invalidates only checkpoints at or after it.
        let line = "let s = \"x\"; /* c */ 1\n";
        let text = line.repeat(2_000);
        let mut doc = document(&text);
        let before = doc.snapshot();
        let old = native_checkpoints(&before, usize::MAX);
        // An unclosed nested comment changes the state of everything after it.
        let edit_at = line.len() * 1_000;
        let after = insert(&mut doc, edit_at, "/* ");
        assert!(old.iter().any(|c| c.offset().0 > edit_at));
        let carried: Vec<_> = old.iter().filter_map(|c| c.rebase(&after)).collect();
        assert_eq!(carried.len(), old.iter().filter(|c| c.offset().0 < edit_at).count());
        assert!(carried.len() >= 3);
        let restart = carried.last().unwrap();
        let resumed = lex(
            after.clone(),
            Language::Rust,
            restart.offset()..TextOffset(after.len()),
            Some(restart),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(resumed.status, Status::Complete);
        let oracle = ForwardLexer::configured(after.clone(), Language::Rust, LexerPreference::Native, None)
            .advance(TextOffset(after.len()), &Cancellation::default())
            .unwrap();
        let tail: Vec<_> = oracle
            .spans
            .iter()
            .filter(|span| span.range.start >= restart.offset())
            .cloned()
            .collect();
        assert_eq!(resumed.spans, tail);
        assert_eq!(
            styled(&after, &resumed, StyleKind::Comment).last().unwrap().len(),
            after.len() - edit_at
        );
        // One byte of CR/LF lookahead: text inserted exactly at a checkpoint drops it.
        let mut crs = document(&"x\r".repeat(300));
        let checkpoint = native_checkpoints(&crs.snapshot(), usize::MAX).remove(0);
        let at = checkpoint.offset().0;
        assert!(checkpoint.rebase(&insert(&mut crs, at + 1, "y")).is_some());
        let joined = crs.snapshot();
        let checkpoint = native_checkpoints(&joined, usize::MAX).remove(0);
        assert!(checkpoint.rebase(&insert(&mut crs, at, "\n")).is_none());
        // Foreign documents and skipped revisions never inherit state.
        assert!(checkpoint.rebase(&document(&"x\r".repeat(300)).snapshot()).is_none());
        let once = insert(&mut doc, after.len(), "a");
        assert!(carried[0].rebase(&once).is_some());
        let twice = insert(&mut doc, once.len(), "b");
        assert!(carried[0].rebase(&twice).is_none());
    }
    #[test]
    fn rebased_result_keeps_colors_through_the_edit_until_a_fresh_window_lands() {
        // SRC-14: the previous styling stays, mapped through the edit, instead of
        // flashing plain text; typing inside a comment extends it.
        let text = "/* note */\nlet s = \"str\";\n";
        let mut doc = document(text);
        let before = doc.snapshot();
        let old = ForwardLexer::configured(before.clone(), Language::Rust, LexerPreference::Native, None)
            .advance(TextOffset(text.len()), &Cancellation::default())
            .unwrap();
        let after = insert(&mut doc, 8, "more ");
        let stand_in = old.rebase(&after).unwrap();
        assert!(stand_in.is_current(&after) && !old.is_current(&after));
        assert_eq!(stand_in.status, Status::Provisional);
        assert_eq!(stand_in.range, TextOffset(0)..TextOffset(after.len()));
        assert_eq!(styled(&after, &stand_in, StyleKind::Comment), ["/* note more */"]);
        assert_eq!(styled(&after, &stand_in, StyleKind::String), ["\"str\""]);
        assert_eq!(styled(&after, &stand_in, StyleKind::Keyword), ["let"]);
        assert!(stand_in.checkpoint.is_none() && stand_in.fold_levels.is_none());
        // A fresh first-line window replaces only what it covers.
        let first_line = "/* note more */\n".len();
        let fresh = ForwardLexer::configured(after.clone(), Language::Rust, LexerPreference::Native, None)
            .advance(TextOffset(first_line), &Cancellation::default())
            .unwrap();
        assert!(styled(&after, &fresh, StyleKind::String).is_empty());
        let merged = fresh.overlay(&stand_in);
        assert_eq!(merged.status, Status::Provisional);
        assert_eq!(merged.range, stand_in.range);
        assert_eq!(styled(&after, &merged, StyleKind::Comment), ["/* note more */"]);
        assert_eq!(styled(&after, &merged, StyleKind::String), ["\"str\""]);
        assert!(merged.spans.windows(2).all(|s| s[0].range.end <= s[1].range.start));
        // Deleting a whole token drops its span; the rest carries over again.
        let deleted = {
            let base = doc.snapshot();
            let at = text.find("\"str\"").unwrap() + 5;
            doc.apply(EditTransaction {
                base_revision: base.revision,
                edits: vec![Edit {
                    range: TextOffset(at)..TextOffset(at + 5),
                    insert: String::new(),
                }],
            })
            .unwrap();
            doc.snapshot()
        };
        let rebased = stand_in.rebase(&deleted).unwrap();
        assert!(styled(&deleted, &rebased, StyleKind::String).is_empty());
        assert_eq!(styled(&deleted, &rebased, StyleKind::Keyword), ["let"]);
        assert!(old.rebase(&deleted).is_none());
    }
    #[test]
    fn worker_resumes_at_a_rebased_checkpoint_after_an_edit() {
        // SRC-14: the edit re-lexes from the nearest verified checkpoint, not byte 0.
        let line = "let s = \"x\"; /* c */ 1\n";
        let mut doc = document(&line.repeat(30_000));
        let before = doc.snapshot();
        let old = native_checkpoints(&before, usize::MAX);
        let edit_at = line.len() * 29_000;
        let after = insert(&mut doc, edit_at, "// ");
        let restart = old.iter().filter_map(|c| c.rebase(&after)).last().unwrap();
        assert!(restart.offset().0 < edit_at && edit_at - restart.offset().0 <= 256 * line.len());
        let worker = SyntaxWorker::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = tx.send(());
        });
        let range = restart.offset()..TextOffset(after.len());
        let ticket = worker
            .submit_preferred(
                after.clone(),
                Language::Rust,
                range.clone(),
                Some(restart.clone()),
                notify,
                LexerPreference::Native,
            )
            .unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        let result = ticket.try_recv().unwrap().unwrap();
        assert!(result.is_current(&after));
        assert_eq!(result.status, Status::Complete);
        assert_eq!(result.range, range);
        assert_eq!(worker.lexed_bytes(), (range.end.0 - range.start.0) as u64);
        assert!(
            result
                .spans
                .iter()
                .any(|span| span.kind == StyleKind::Comment && span.range.start.0 == edit_at)
        );
    }
    #[test]
    fn primary_lexer_resumes_natively_only_past_its_session_bound() {
        // SRC-14: past the Lexilla session bound styling is native on every pass,
        // so a checkpoint there is a verified restart for the primary lexer too.
        let line = "0123456789012345\n";
        let text = line.repeat(bareline_lexilla_bridge::SESSION_BYTES / line.len() + 2_000);
        let source = Document::from_utf8(&text, Budget::new(64 << 20), Budget::new(4 << 20))
            .unwrap()
            .snapshot();
        let restart = native_checkpoints(&source, bareline_lexilla_bridge::SESSION_BYTES + MAX_REQUEST_BYTES)
            .into_iter()
            .find(|c| c.offset().0 >= bareline_lexilla_bridge::SESSION_BYTES)
            .unwrap();
        let worker = SyntaxWorker::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = tx.send(());
        });
        let range = restart.offset()..TextOffset(restart.offset().0 + 100 * line.len());
        let ticket = worker
            .submit(
                source.clone(),
                Language::Rust,
                range.clone(),
                Some(restart.clone()),
                notify,
            )
            .unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        let result = ticket.try_recv().unwrap().unwrap();
        assert_eq!(result.status, Status::Complete);
        assert_eq!(styled(&source, &result, StyleKind::Number).len(), 100);
        assert_eq!(worker.lexed_bytes(), (range.end.0 - range.start.0) as u64);
    }
    /// The window `range` of a primary-lexer pass from byte 0 over `source`,
    /// reached in line-aligned windows.
    fn primary_from_zero(source: &DocumentSnapshot, range: Range<TextOffset>) -> SyntaxResult {
        let mut pass = ForwardLexer::new(source.clone(), Language::Rust);
        while pass.next < range.start {
            let mut end = range.start.0.min(pass.next.0 + MAX_REQUEST_BYTES);
            if end < range.start.0 {
                end = source
                    .line_range(source.line_at(TextOffset(end)).unwrap())
                    .unwrap()
                    .start
                    .0;
            }
            pass.advance(TextOffset(end), &Cancellation::default()).unwrap();
        }
        pass.advance(range.end, &Cancellation::default()).unwrap()
    }
    #[test]
    fn primary_lexer_resumes_at_a_carried_checkpoint_inside_its_session() {
        // SRC-14: under the default (Lexilla) preference, an edit near the end of
        // a document below SESSION_BYTES re-lexes only from the nearest carried
        // checkpoint, and so does a scroll back up, with the colors and fold
        // levels of a pass from byte 0.
        let unit = concat!(
            "fn item() {\n",
            "    let text = \"a string value\"; /* block comment */ 1234;\n",
            "    // a line comment that keeps restart lines apart in this test\n",
            "}\n",
        );
        // Each 256-line checkpoint is at least RESTART_GAP after the previous
        // one, so the session keeps restart data at every one of them.
        assert!(64 * unit.len() >= bareline_lexilla_bridge::RESTART_GAP);
        let mut doc = document(&unit.repeat(4_000));
        let before = doc.snapshot();
        assert!(before.len() > 2 * MAX_REQUEST_BYTES && before.len() < bareline_lexilla_bridge::SESSION_BYTES);
        let worker = SyntaxWorker::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = tx.send(());
        });
        let request = |source: &DocumentSnapshot, range: Range<TextOffset>, checkpoint: Option<&Checkpoint>| {
            let ticket = worker
                .submit(
                    source.clone(),
                    Language::Rust,
                    range.clone(),
                    checkpoint.cloned(),
                    notify.clone(),
                )
                .unwrap();
            rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            let result = ticket.try_recv().unwrap().unwrap();
            assert_eq!(result.status, Status::Complete);
            assert_eq!(result.range, range);
            assert!(result.fold_levels.is_some(), "the primary lexer styled this window");
            result
        };
        // Opening the end of the file lexes everything before it once.
        let tail = before.line_range(before.line_count() - 400).unwrap().start;
        let first = request(&before, tail..TextOffset(before.len()), None);
        assert_eq!(worker.lexed_bytes(), before.len() as u64);
        // An unclosed comment near the end changes the state of everything after it.
        let edit_at = before.line_range(before.line_count() - 100).unwrap().start.0;
        let after = insert(&mut doc, edit_at, "/* ");
        let carried: Vec<_> = first.checkpoints.iter().filter_map(|c| c.rebase(&after)).collect();
        let restart = carried.last().unwrap();
        assert!(restart.offset().0 < edit_at && edit_at - restart.offset().0 <= 64 * unit.len());
        let lexed = worker.lexed_bytes();
        let range = restart.offset()..TextOffset(after.len());
        let resumed = request(&after, range.clone(), Some(restart));
        assert_eq!(worker.lexed_bytes() - lexed, (range.end.0 - range.start.0) as u64);
        let oracle = primary_from_zero(&after, range);
        assert_eq!(resumed.spans, oracle.spans);
        assert_eq!(resumed.fold_levels, oracle.fold_levels);
        assert!(
            resumed
                .spans
                .last()
                .is_some_and(|span| span.kind == StyleKind::Comment && span.range.end.0 == after.len())
        );
        // Scrolling back up resumes at the earlier checkpoint, not at byte 0.
        let early = &carried[0];
        let up = early.offset()..TextOffset(early.offset().0 + 100 * unit.len());
        let lexed = worker.lexed_bytes();
        let scrolled = request(&after, up.clone(), Some(early));
        assert_eq!(worker.lexed_bytes() - lexed, (up.end.0 - up.start.0) as u64);
        let oracle = primary_from_zero(&after, up);
        assert_eq!(scrolled.spans, oracle.spans);
        assert_eq!(scrolled.fold_levels, oracle.fold_levels);
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
        // The densest possible window (one span per byte) fits the span budget.
        let dense = document(&"1,".repeat(MAX_REQUEST_BYTES / 2)).snapshot();
        let result = lex(
            dense.clone(),
            Language::Rust,
            TextOffset(0)..TextOffset(dense.len()),
            None,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(result.status, Status::Complete);
        assert!(result.spans.len() > 32 * 1024);
    }
    fn pretty_json(min_len: usize) -> String {
        let mut text = String::from("[\n");
        let mut id = 0;
        while text.len() < min_len {
            text.push_str(&format!(
                "  {{\n    \"id\": {id},\n    \"name\": \"item-{id}\",\n    \"tags\": [\"a\", \"b\"],\n    \"active\": true\n  }},\n"
            ));
            id += 1;
        }
        text.push_str("  {\n    \"last\": \"tail\"\n  }\n]\n");
        text
    }
    #[test]
    fn dense_pretty_json_stays_highlighted_to_eof() {
        // SRC-13: about 60k spans per 256 KiB window used to fail the whole document.
        let text = pretty_json(400 * 1024);
        let source = document(&text).snapshot();
        for preference in [LexerPreference::Lexilla, LexerPreference::Native] {
            let mut pass = ForwardLexer::configured(source.clone(), Language::Json, preference, None);
            let mut windows = Vec::new();
            while pass.next.0 < source.len() {
                let mut end = source.len().min(pass.next.0 + MAX_REQUEST_BYTES);
                if end < source.len() {
                    end = source
                        .line_range(source.line_at(TextOffset(end)).unwrap())
                        .unwrap()
                        .start
                        .0;
                }
                let result = pass.advance(TextOffset(end), &Cancellation::default()).unwrap();
                assert_eq!(result.status, Status::Complete);
                assert!(!result.spans.is_empty());
                windows.push(result);
            }
            assert!(windows.len() >= 2);
            assert!(
                windows[0].spans.len() > 32 * 1024,
                "{preference:?} window was not dense"
            );
            let last = windows.last().unwrap();
            assert_eq!(last.range.end.0, source.len());
            assert!(
                styled(&source, last, StyleKind::String)
                    .iter()
                    .any(|s| s.contains("tail")),
                "{preference:?} lost highlighting before EOF"
            );
        }
    }
    #[test]
    fn lexilla_mapping_error_falls_back_to_native_spans() {
        // SRC-20: a mapping error must keep the native spans and drop only folds.
        let text = "{\n  \"key\": [1, 2],\n  \"flag\": true\n}\n";
        let source = document(text).snapshot();
        let range = TextOffset(0)..TextOffset(text.len());
        let native = ForwardLexer::configured(source.clone(), Language::Json, LexerPreference::Native, None)
            .advance(range.end, &Cancellation::default())
            .unwrap();
        let primary = lex(
            source.clone(),
            Language::Json,
            range.clone(),
            None,
            &Cancellation::default(),
        )
        .unwrap();
        assert!(primary.fold_levels.is_some(), "Lexilla must be active for this test");
        lexilla::FAIL_MAPPING.with(|fail| fail.set(true));
        let fallback = lex(
            source.clone(),
            Language::Json,
            range.clone(),
            None,
            &Cancellation::default(),
        );
        let mut pass = ForwardLexer::new(source.clone(), Language::Json);
        let forward = pass.advance(range.end, &Cancellation::default());
        lexilla::FAIL_MAPPING.with(|fail| fail.set(false));
        for result in [fallback.unwrap(), forward.unwrap()] {
            assert_eq!(result.status, Status::Complete);
            assert!(result.fold_levels.is_none());
            assert!(!result.spans.is_empty());
            assert_eq!(result.spans, native.spans);
            assert!(result.checkpoint.is_some());
        }
        assert!(pass.native.is_none());
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
