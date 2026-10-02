// SPDX-License-Identifier: MPL-2.0
//! Spell checking of the visible text (BIZ-31). Tokenizing, the job bounds and
//! the worker are portable; the dictionary is a platform [`SpellChecker`] that
//! the factory builds on this module's worker thread, never on the UI thread.
//! Each drawn view submits at most one bounded window (the visible text plus a
//! margin); a newer window replaces a waiting one it makes stale.
//!
//! Only resident documents are checked; large-file (paged) views and the
//! secondary pane of a split view are not yet.
//!
//! Accessibility follow-up: misspellings are not yet exposed as UIA text
//! attributes. AccessKit carries `is_spelling_error` on text runs, but the
//! vendored AccessKit UIA text provider answers no annotation-type attribute
//! query, so exposing them needs that mapping first. Suggestions are native
//! menu items, which carry UIA names.
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_editor_surface::{EditorSurface, SpellScope};
use bareline_platform::spelling::{MAX_SUGGESTIONS, SpellChecker, SpellCheckerFactory};
use bareline_syntax::{StyleKind, StyleSpan};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Range,
    sync::{Arc, Condvar, Mutex, mpsc},
    time::Duration,
};
use unicode_segmentation::UnicodeSegmentation;

/// Bytes checked on each side of the visible text, so a short scroll already
/// shows its squiggles.
pub const MARGIN_BYTES: usize = 4 * 1024;
/// Largest window one job reads and checks.
pub const MAX_WINDOW_BYTES: usize = 64 * 1024;
/// Most words one job checks; the rest of a dense window waits for a scroll.
pub const MAX_WORDS_PER_JOB: usize = 8 * 1024;
/// Longer letter runs are not words (and would cost the checker the most).
pub const MAX_WORD_CHARS: usize = 64;
/// Windows waiting for the worker (one per drawn view is enough).
const MAX_PENDING_WINDOWS: usize = 4;
/// Verdicts the worker remembers before starting over.
const CACHE_WORDS: usize = 16 * 1024;
/// How often, in words, a job yields to a newer window or a menu request.
const YIELD_WORDS: usize = 32;
/// Longest the UI waits for suggestions while a context menu opens.
pub const SUGGESTION_WAIT: Duration = Duration::from_millis(250);

/// Stable language IDs that are prose, checked by default.
pub fn is_prose(stable_id: &str) -> bool {
    matches!(stable_id, "text" | "markdown")
}

/// What a view checks: nothing while spell checking is off (`enabled`) or the
/// language setting turns it off; otherwise every word of prose and only the
/// comments and strings of code. `language` is the per-language setting, which
/// defaults to on for prose and off for code.
pub fn scope(enabled: bool, language: Option<bool>, prose: bool) -> SpellScope {
    if !enabled || !language.unwrap_or(prose) {
        SpellScope::Off
    } else if prose {
        SpellScope::AllText
    } else {
        SpellScope::CommentsAndStrings
    }
}

/// Byte ranges of the words in `text` worth checking. Apostrophes inside a word
/// stay in it ("don't", "O’Brien"). Skipped: identifiers (camelCase and inner
/// capitals, snake_case, words with digits), all-capital acronyms, single
/// letters, dotted names ("file.txt", "e.g"), path segments, URLs and e-mail
/// addresses.
pub fn words(text: &str) -> Vec<Range<usize>> {
    let skipped = link_ranges(text);
    text.unicode_word_indices()
        .filter_map(|(start, word)| {
            let range = start..start + word.len();
            let count = word.chars().count();
            let path = [
                text[..range.start].chars().next_back(),
                text[range.end..].chars().next(),
            ]
            .into_iter()
            .flatten()
            .any(|c| matches!(c, '/' | '\\'));
            let keep = (2..=MAX_WORD_CHARS).contains(&count)
                && !path
                && !skipped
                    .iter()
                    .any(|link| link.start < range.end && range.start < link.end)
                && word.chars().all(|c| {
                    if c.is_ascii() {
                        c.is_ascii_alphabetic() || c == '\''
                    } else {
                        !c.is_numeric()
                    }
                })
                && !word.chars().skip(1).any(char::is_uppercase);
            keep.then_some(range)
        })
        .collect()
}

/// URLs (from their scheme or "www." to the next space) and whitespace-delimited
/// chunks holding an '@' (e-mail addresses, handles, decorators).
fn link_ranges(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    for (index, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if !c.is_whitespace() {
            start.get_or_insert(index);
            continue;
        }
        let Some(chunk_start) = start.take() else {
            continue;
        };
        let chunk = &text[chunk_start..index];
        let link = if chunk.contains('@') {
            Some(0)
        } else if let Some(scheme_end) = chunk.find("://") {
            let scheme = chunk[..scheme_end]
                .char_indices()
                .rev()
                .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                .last()
                .map_or(scheme_end, |(at, _)| at);
            Some(scheme)
        } else {
            chunk
                .match_indices("www.")
                .map(|(at, _)| at)
                .find(|at| !chunk[..*at].chars().next_back().is_some_and(char::is_alphanumeric))
        };
        if let Some(link) = link {
            out.push(chunk_start + link..index);
        }
    }
    out
}

/// The window one job checks: the visible text plus [`MARGIN_BYTES`] each side,
/// at most [`MAX_WINDOW_BYTES`] and inside a document of `len` bytes. Offsets
/// may still split a character; the worker snaps them.
pub fn window(visible: Range<usize>, len: usize) -> Range<usize> {
    let start = visible.start.min(len).saturating_sub(MARGIN_BYTES);
    let end = visible.end.saturating_add(MARGIN_BYTES).min(len).max(start);
    start..end.min(start + MAX_WINDOW_BYTES)
}

/// Comment and string spans inside `window`, merged where they touch, which a
/// code language limits its checks to.
pub fn checkable_spans(spans: &[StyleSpan], window: &Range<usize>) -> Vec<Range<usize>> {
    let first = spans.partition_point(|span| span.range.end.0 <= window.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for span in spans[first..].iter().take_while(|span| span.range.start.0 < window.end) {
        if !matches!(span.kind, StyleKind::Comment | StyleKind::String) {
            continue;
        }
        let range = span.range.start.0.max(window.start)..span.range.end.0.min(window.end);
        match out.last_mut() {
            Some(last) if last.end >= range.start => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
    out
}

/// Words the user accepted this session and the checker's remembered verdicts.
#[derive(Default)]
struct Session {
    verdicts: HashMap<String, bool>,
    /// Ignore All, and Add to Dictionary even if the dictionary refused the word.
    accepted: HashSet<String>,
}
impl Session {
    fn is_correct(&mut self, checker: &mut dyn SpellChecker, word: &str) -> bool {
        if self.accepted.contains(word) {
            return true;
        }
        if let Some(verdict) = self.verdicts.get(word) {
            return *verdict;
        }
        // A checker that fails on a word flags nothing rather than everything.
        let verdict = checker.is_correct(word).unwrap_or(true);
        if self.verdicts.len() >= CACHE_WORDS {
            self.verdicts.clear();
        }
        self.verdicts.insert(word.to_owned(), verdict);
        verdict
    }
    fn run(&mut self, checker: &mut dyn SpellChecker, command: Command) {
        match command {
            Command::Suggest(word, reply) => {
                let _ = reply.send(checker.suggestions(&word, MAX_SUGGESTIONS).unwrap_or_default());
            }
            Command::Add(word) => {
                let _ = checker.add_to_dictionary(&word);
                self.accepted.insert(word);
            }
            Command::Ignore(word) => {
                self.accepted.insert(word);
            }
        }
    }
}

struct CheckRequest {
    snapshot: DocumentSnapshot,
    window: Range<usize>,
    /// Absolute comment and string ranges; `None` checks every word.
    allowed: Option<Vec<Range<usize>>>,
}
impl CheckRequest {
    /// Whether this request makes `other` pointless: an older snapshot of the
    /// same document, or the same window again.
    fn supersedes(&self, other: &CheckRequest) -> bool {
        let (document, revision) = self.snapshot.identity_token();
        let (other_document, other_revision) = other.snapshot.identity_token();
        document == other_document && (other_revision < revision || other.window == self.window)
    }
}
/// One finished window: the misspelled words of the snapshot with `identity`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckResult {
    pub identity: (u64, u64),
    pub misspelled: Vec<Range<TextOffset>>,
}

/// Check one window, or `None` when `interrupted` asks it to yield. At most
/// [`MAX_WORDS_PER_JOB`] words are checked; a word cut by the window edge is
/// left for a window that holds all of it.
fn check_window(
    request: &CheckRequest,
    checker: &mut dyn SpellChecker,
    session: &mut Session,
    mut interrupted: impl FnMut() -> bool,
) -> Option<Vec<Range<TextOffset>>> {
    let snapshot = &request.snapshot;
    let len = snapshot.len();
    let mut start = request.window.start.min(len);
    let mut end = request.window.end.clamp(start, len).min(start + MAX_WINDOW_BYTES);
    while start < end && !snapshot.is_boundary(TextOffset(start)) {
        start += 1;
    }
    while end > start && !snapshot.is_boundary(TextOffset(end)) {
        end -= 1;
    }
    let Ok(text) = snapshot.read(TextOffset(start)..TextOffset(end), MAX_WINDOW_BYTES) else {
        return Some(Vec::new());
    };
    let mut misspelled = Vec::new();
    for (index, word) in words(&text).into_iter().take(MAX_WORDS_PER_JOB).enumerate() {
        if index % YIELD_WORDS == YIELD_WORDS - 1 && interrupted() {
            return None;
        }
        if (word.start == 0 && start > 0) || (word.end == text.len() && end < len) {
            continue;
        }
        let absolute = start + word.start..start + word.end;
        if let Some(allowed) = &request.allowed {
            let at = allowed.partition_point(|range| range.end < absolute.end);
            if !allowed.get(at).is_some_and(|range| range.start <= absolute.start) {
                continue;
            }
        }
        if !session.is_correct(checker, &text[word]) {
            misspelled.push(TextOffset(absolute.start)..TextOffset(absolute.end));
        }
    }
    Some(misspelled)
}

enum Command {
    Suggest(String, mpsc::Sender<Vec<String>>),
    Add(String),
    Ignore(String),
}
enum Outcome {
    Checked(CheckResult),
    Unavailable(String),
}
#[derive(Default)]
struct State {
    /// Waiting windows, oldest first.
    checks: VecDeque<CheckRequest>,
    commands: VecDeque<Command>,
    stop: bool,
}
impl State {
    /// Queue `request` in place of the waiting windows it supersedes, keeping
    /// at most [`MAX_PENDING_WINDOWS`] (the oldest go first).
    fn submit(&mut self, request: CheckRequest) {
        self.checks.retain(|queued| !request.supersedes(queued));
        self.checks.push_back(request);
        while self.checks.len() > MAX_PENDING_WINDOWS {
            self.checks.pop_front();
        }
    }
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
impl Shared {
    fn push(&self, update: impl FnOnce(&mut State)) {
        if let Ok(mut state) = self.state.lock() {
            update(&mut state);
            self.wake.notify_one();
        }
    }
}

/// The spelling thread. Commands (suggestions, Add, Ignore) run before the next
/// window; a running window yields to either or to a window that supersedes it,
/// and is resumed unless superseded.
struct Worker {
    shared: Arc<Shared>,
    outcomes: mpsc::Receiver<Outcome>,
}
impl Worker {
    fn spawn(factory: SpellCheckerFactory, notify: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let (sender, outcomes) = mpsc::channel();
        let worker = shared.clone();
        std::thread::Builder::new().name("spelling".into()).spawn(move || {
            let mut checker = match factory() {
                Ok(checker) => checker,
                Err(error) => {
                    let _ = sender.send(Outcome::Unavailable(error));
                    notify();
                    return;
                }
            };
            let mut session = Session::default();
            loop {
                let (command, request) = {
                    let Ok(mut state) = worker.state.lock() else {
                        return;
                    };
                    while state.checks.is_empty() && state.commands.is_empty() && !state.stop {
                        let Ok(next) = worker.wake.wait(state) else {
                            return;
                        };
                        state = next;
                    }
                    if state.stop {
                        return;
                    }
                    match state.commands.pop_front() {
                        Some(command) => (Some(command), None),
                        None => (None, state.checks.pop_front()),
                    }
                };
                if let Some(command) = command {
                    session.run(checker.as_mut(), command);
                    continue;
                }
                let Some(request) = request else {
                    continue;
                };
                let interrupted = || match worker.state.lock() {
                    Ok(state) => {
                        state.stop
                            || !state.commands.is_empty()
                            || state.checks.iter().any(|queued| queued.supersedes(&request))
                    }
                    Err(_) => true,
                };
                match check_window(&request, checker.as_mut(), &mut session, interrupted) {
                    Some(misspelled) => {
                        let identity = request.snapshot.identity_token();
                        if sender
                            .send(Outcome::Checked(CheckResult { identity, misspelled }))
                            .is_err()
                        {
                            return;
                        }
                        notify();
                    }
                    None => {
                        if let Ok(mut state) = worker.state.lock()
                            && !state.checks.iter().any(|queued| queued.supersedes(&request))
                        {
                            state.checks.push_front(request);
                        }
                    }
                }
            }
        })?;
        Ok(Self { shared, outcomes })
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.push(|state| state.stop = true);
    }
}

/// What a request asked for; an identical one is not sent again.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RequestKey {
    identity: (u64, u64),
    window: Range<usize>,
    allowed: Option<Vec<Range<usize>>>,
    generation: u64,
}

/// A misspelled word that a spelling command acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub identity: (u64, u64),
    pub range: Range<usize>,
    pub word: String,
}
impl Target {
    /// The misspelling at `offset` in `editor`, read from the current snapshot.
    pub fn at(editor: &EditorSurface, offset: usize) -> Option<Self> {
        let range = editor.spelling_mark_at(offset)?;
        let word = editor
            .snapshot()
            .read(TextOffset(range.start)..TextOffset(range.end), MAX_WORD_CHARS * 4)
            .ok()?;
        Some(Self {
            identity: editor.snapshot().identity_token(),
            range,
            word,
        })
    }
}

/// Workspace spelling state: one worker, started the first time a view needs a
/// check, with a few windows waiting at most.
#[derive(Default)]
pub struct Spelling {
    factory: Option<SpellCheckerFactory>,
    worker: Option<Worker>,
    /// The most recent requests, newest last; one of these is not sent again.
    requested: VecDeque<RequestKey>,
    /// Bumped by Add to Dictionary and Ignore All so the visible text is checked again.
    generation: u64,
    /// Why spell checking is unavailable (no dictionary, API missing); shown once.
    pub unavailable: Option<String>,
    unavailable_shown: bool,
    suggestions: Option<(String, Vec<String>)>,
    pending_suggestions: Option<(String, mpsc::Receiver<Vec<String>>)>,
    /// The misspelling a context menu was opened on, for the commands it lists.
    pub target: Option<Target>,
}

impl Spelling {
    /// Install the platform checker factory; without one nothing is checked.
    pub fn set_factory(&mut self, factory: SpellCheckerFactory) {
        self.factory = Some(factory);
        self.worker = None;
        self.requested.clear();
        self.unavailable = None;
    }
    /// Apply finished windows to the resident editors showing their documents.
    pub fn pump(&mut self, editors: &mut [crate::workspace::WorkspaceEditor]) -> bool {
        let mut changed = false;
        if let Some((word, receiver)) = &self.pending_suggestions
            && let Ok(list) = receiver.try_recv()
        {
            self.suggestions = Some((word.clone(), list));
            self.pending_suggestions = None;
            changed = true;
        }
        while let Some(outcome) = self.worker.as_ref().and_then(|worker| worker.outcomes.try_recv().ok()) {
            changed = true;
            match outcome {
                Outcome::Checked(result) => {
                    for editor in editors.iter_mut() {
                        if let Some(surface) = editor.resident_mut()
                            && surface.snapshot().identity_token().0 == result.identity.0
                        {
                            surface.set_spelling_marks(result.identity, result.misspelled.clone());
                        }
                    }
                }
                Outcome::Unavailable(reason) => {
                    self.unavailable = Some(reason);
                    self.worker = None;
                }
            }
        }
        changed
    }
    /// The unavailability reason, the first time it is asked for.
    pub fn take_unavailable_notice(&mut self) -> Option<String> {
        if self.unavailable_shown {
            return None;
        }
        let notice = self.unavailable.clone()?;
        self.unavailable_shown = true;
        Some(notice)
    }
    /// After a draw: ask for the drawn view's window when it changed, and fetch
    /// suggestions for a misspelling under the caret for the Edit menu. `syntax`
    /// supplies comment and string spans for a code language; without current
    /// spans a code view is not checked.
    pub fn refresh(
        &mut self,
        editor: &mut EditorSurface,
        syntax: Option<&bareline_syntax::SyntaxResult>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        let scope = editor.spell_scope;
        if scope == SpellScope::Off || self.unavailable.is_some() || self.factory.is_none() {
            // Turned back on, the view is checked again.
            let document = editor.snapshot().identity_token().0;
            self.requested.retain(|key| key.identity.0 != document);
            editor.clear_spelling_marks();
            return;
        }
        let snapshot = editor.snapshot();
        if !snapshot.is_complete() {
            return;
        }
        let window = window(editor.visible_text.start.0..editor.visible_text.end.0, snapshot.len());
        let allowed = match scope {
            SpellScope::CommentsAndStrings => {
                let Some(syntax) = syntax.filter(|result| result.is_current(snapshot)) else {
                    return;
                };
                Some(checkable_spans(&syntax.spans, &window))
            }
            _ => None,
        };
        let key = RequestKey {
            identity: snapshot.identity_token(),
            window: window.clone(),
            allowed: allowed.clone(),
            generation: self.generation,
        };
        if !self.requested.contains(&key) {
            if self.worker.is_none()
                && let Some(factory) = self.factory.clone()
            {
                match Worker::spawn(factory, notify) {
                    Ok(worker) => self.worker = Some(worker),
                    Err(error) => {
                        self.unavailable = Some(format!("Spell checking could not start: {error}"));
                        return;
                    }
                }
            }
            let request = CheckRequest {
                snapshot: snapshot.clone(),
                window,
                allowed,
            };
            if let Some(worker) = &self.worker {
                worker.shared.push(|state| state.submit(request));
            }
            self.requested.push_back(key);
            while self.requested.len() > MAX_PENDING_WINDOWS {
                self.requested.pop_front();
            }
        }
        if let Some(target) = Target::at(editor, editor.selection.caret) {
            let _ = self.suggestions(&target.word, Duration::ZERO);
        }
    }
    /// Suggestions for `word`: cached, or asked of the worker with a wait of at
    /// most `wait`. A late answer is kept for the next menu.
    pub fn suggestions(&mut self, word: &str, wait: Duration) -> Vec<String> {
        if let Some(list) = self.cached_suggestions(word) {
            return list.to_vec();
        }
        let Some(worker) = &self.worker else {
            return Vec::new();
        };
        if self
            .pending_suggestions
            .as_ref()
            .is_none_or(|(pending, _)| pending != word)
        {
            let (reply, receiver) = mpsc::channel();
            worker
                .shared
                .push(|state| state.commands.push_back(Command::Suggest(word.to_owned(), reply)));
            self.pending_suggestions = Some((word.to_owned(), receiver));
        }
        let Some((_, receiver)) = &self.pending_suggestions else {
            return Vec::new();
        };
        match receiver.recv_timeout(wait) {
            Ok(list) => {
                self.pending_suggestions = None;
                self.suggestions = Some((word.to_owned(), list.clone()));
                list
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Vec::new(),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.pending_suggestions = None;
                Vec::new()
            }
        }
    }
    /// Cached suggestions for `word` only; never waits.
    pub fn cached_suggestions(&self, word: &str) -> Option<&[String]> {
        self.suggestions
            .as_ref()
            .filter(|(cached, _)| cached == word)
            .map(|(_, list)| list.as_slice())
    }
    /// Add `word` to the user's dictionary and check the visible text again.
    pub fn add_to_dictionary(&mut self, word: &str, editors: &mut [crate::workspace::WorkspaceEditor]) {
        self.accept(Command::Add(word.to_owned()), editors);
    }
    /// Accept `word` everywhere for the rest of this session.
    pub fn ignore_all(&mut self, word: &str, editors: &mut [crate::workspace::WorkspaceEditor]) {
        self.accept(Command::Ignore(word.to_owned()), editors);
    }
    fn accept(&mut self, command: Command, editors: &mut [crate::workspace::WorkspaceEditor]) {
        if let Some(worker) = &self.worker {
            worker.shared.push(|state| state.commands.push_back(command));
        }
        // The next results replace the marks instead of adding to marks that
        // still flag the accepted word.
        for editor in editors.iter_mut() {
            if let Some(surface) = editor.resident_mut() {
                surface.clear_spelling_marks();
            }
        }
        self.generation += 1;
        self.requested.clear();
        self.suggestions = None;
        self.target = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn found(text: &str) -> Vec<&str> {
        words(text).into_iter().map(|range| &text[range]).collect()
    }

    #[test]
    fn tokenizer_finds_words_and_keeps_inner_apostrophes() {
        assert_eq!(
            found("Hello, wrold! It's James' café. Don’t"),
            ["Hello", "wrold", "It's", "James", "café", "Don’t"]
        );
        assert_eq!(found("well-known  line\r\nbreak"), ["well", "known", "line", "break"]);
    }

    #[test]
    fn tokenizer_skips_identifiers_acronyms_and_short_or_numeric_words() {
        assert_eq!(
            found("camelCase PascalCase snake_case SCREAMING_CASE HTTP abc123 x 42 iPhone plain"),
            ["plain"]
        );
        assert!(found(&"a".repeat(MAX_WORD_CHARS + 1)).is_empty());
        assert_eq!(found(&"a".repeat(MAX_WORD_CHARS)).len(), 1);
    }

    #[test]
    fn tokenizer_skips_urls_addresses_dotted_names_and_paths() {
        assert_eq!(
            found("see https://exampel.com/pathh?q=wrod and (http://x.org) now"),
            ["see", "and", "now"]
        );
        assert_eq!(found("visit www.exampel.com today"), ["visit", "today"]);
        assert_eq!(found("mail someone@exampel.com or @handle"), ["mail", "or"]);
        assert_eq!(
            found("open file.txt e.g. src/libb C:\\Userz\\dir done"),
            ["open", "done"]
        );
    }

    #[test]
    fn scope_is_on_for_prose_and_off_for_code_by_default() {
        assert_eq!(scope(true, None, true), SpellScope::AllText);
        assert_eq!(scope(true, None, false), SpellScope::Off);
        assert_eq!(scope(true, Some(true), false), SpellScope::CommentsAndStrings);
        assert_eq!(scope(true, Some(false), true), SpellScope::Off);
        assert_eq!(
            scope(false, Some(true), true),
            SpellScope::Off,
            "the global toggle wins"
        );
        assert!(is_prose("text") && is_prose("markdown") && !is_prose("rust"));
    }

    #[test]
    fn window_adds_a_margin_and_stays_bounded() {
        assert_eq!(
            window(10_000..12_000, 100_000),
            10_000 - MARGIN_BYTES..12_000 + MARGIN_BYTES
        );
        assert_eq!(window(100..200, 300), 0..300);
        let wide = window(50_000..500_000, 1_000_000);
        assert_eq!(wide.start, 50_000 - MARGIN_BYTES);
        assert_eq!(wide.end - wide.start, MAX_WINDOW_BYTES);
        assert_eq!(
            window(900..950, 10),
            0..10,
            "a stale visible range still stays inside the document"
        );
    }

    #[test]
    fn code_languages_check_only_comments_and_strings() {
        let span = |range: Range<usize>, kind| StyleSpan {
            range: TextOffset(range.start)..TextOffset(range.end),
            kind,
        };
        let spans = [
            span(0..5, StyleKind::Keyword),
            span(6..20, StyleKind::Comment),
            span(20..30, StyleKind::Comment),
            span(40..50, StyleKind::String),
            span(60..70, StyleKind::Comment),
        ];
        assert_eq!(checkable_spans(&spans, &(8..65)), vec![8..30, 40..50, 60..65]);
    }

    /// Accepts the words in its dictionary and counts the words it was asked about.
    struct Fake {
        known: HashSet<&'static str>,
        asked: Arc<AtomicUsize>,
    }
    impl SpellChecker for Fake {
        fn is_correct(&mut self, word: &str) -> Result<bool, String> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            Ok(self.known.contains(word))
        }
        fn suggestions(&mut self, word: &str, limit: usize) -> Result<Vec<String>, String> {
            Ok((1..=10).map(|n| format!("{word}{n}")).take(limit).collect())
        }
        fn add_to_dictionary(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }
    fn fake(known: &[&'static str]) -> (Fake, Arc<AtomicUsize>) {
        let asked = Arc::new(AtomicUsize::new(0));
        (
            Fake {
                known: known.iter().copied().collect(),
                asked: asked.clone(),
            },
            asked,
        )
    }
    fn document(text: &str) -> Document {
        Document::from_utf8(text, Budget::new(1 << 24), Budget::new(1 << 24)).unwrap()
    }
    fn request(text: &str, window: Range<usize>, allowed: Option<Vec<Range<usize>>>) -> CheckRequest {
        CheckRequest {
            snapshot: document(text).snapshot(),
            window,
            allowed,
        }
    }
    fn spans(text: &str, result: &[Range<TextOffset>]) -> Vec<String> {
        result
            .iter()
            .map(|range| text[range.start.0..range.end.0].to_owned())
            .collect()
    }

    #[test]
    fn a_job_flags_unknown_words_and_asks_the_checker_once_per_word() {
        let text = "the cat sat on teh mat teh end";
        let (mut checker, asked) = fake(&["the", "cat", "sat", "on", "mat", "end"]);
        let mut session = Session::default();
        let result = check_window(&request(text, 0..text.len(), None), &mut checker, &mut session, || {
            false
        })
        .unwrap();
        assert_eq!(spans(text, &result), ["teh", "teh"]);
        assert_eq!(asked.load(Ordering::SeqCst), 7, "repeated words hit the cache");
        session.run(&mut checker, Command::Ignore("teh".into()));
        let again = check_window(&request(text, 0..text.len(), None), &mut checker, &mut session, || {
            false
        })
        .unwrap();
        assert!(again.is_empty(), "Ignore All accepts the word for the session");
    }

    #[test]
    fn a_job_is_bounded_and_skips_words_cut_by_its_window() {
        let text = "wrod ".repeat(MAX_WORDS_PER_JOB + 100);
        let (mut checker, _) = fake(&[]);
        let mut session = Session::default();
        let all = check_window(&request(&text, 0..text.len(), None), &mut checker, &mut session, || {
            false
        })
        .unwrap();
        assert_eq!(
            all.len(),
            MAX_WORDS_PER_JOB,
            "a dense window checks a bounded number of words"
        );
        let cut = check_window(
            &request("abcd wrod efgh", 2..12, None),
            &mut checker,
            &mut session,
            || false,
        )
        .unwrap();
        assert_eq!(
            cut,
            vec![TextOffset(5)..TextOffset(9)],
            "only whole words inside the window"
        );
        // Offsets inside a character are snapped instead of failing the read.
        let snapped = check_window(&request("é wrod", 1..7, None), &mut checker, &mut session, || false).unwrap();
        assert_eq!(snapped, vec![TextOffset(3)..TextOffset(7)]);
    }

    #[test]
    fn a_code_job_checks_only_inside_allowed_spans() {
        let text = "fn wrod() { /* a speling note */ let s = \"misteak\"; }";
        let comment = text.find("/*").unwrap()..text.find("*/").unwrap() + 2;
        let string = text.find('"').unwrap()..text.rfind('"').unwrap() + 1;
        let (mut checker, _) = fake(&["note"]);
        let mut session = Session::default();
        let result = check_window(
            &request(text, 0..text.len(), Some(vec![comment, string])),
            &mut checker,
            &mut session,
            || false,
        )
        .unwrap();
        assert_eq!(spans(text, &result), ["speling", "misteak"]);
    }

    #[test]
    fn a_job_yields_when_newer_work_arrives() {
        let text = "wrod ".repeat(YIELD_WORDS * 3);
        let (mut checker, asked) = fake(&[]);
        let mut session = Session::default();
        let result = check_window(&request(&text, 0..text.len(), None), &mut checker, &mut session, || {
            true
        });
        assert!(result.is_none());
        assert!(
            asked.load(Ordering::SeqCst) <= 1,
            "the cache answers repeats before the yield point"
        );
    }

    #[test]
    fn waiting_windows_stay_bounded_and_newer_snapshots_replace_older_ones() {
        let first = document("one two");
        let second = document("three four");
        let mut state = State::default();
        let at = |document: &Document, window: Range<usize>| CheckRequest {
            snapshot: document.snapshot(),
            window,
            allowed: None,
        };
        state.submit(at(&first, 0..3));
        state.submit(at(&first, 0..3));
        assert_eq!(state.checks.len(), 1, "the same window again replaces the waiting one");
        state.submit(at(&first, 4..7));
        state.submit(at(&second, 0..5));
        assert_eq!(state.checks.len(), 3, "other windows and documents wait side by side");
        for start in 0..10 {
            state.submit(at(&second, start..start + 1));
        }
        assert_eq!(state.checks.len(), MAX_PENDING_WINDOWS);
        assert_eq!(state.checks.back().unwrap().window, 9..10, "the newest window is kept");
        let mut edited = document("one two");
        let old = at(&edited, 0..7);
        edited
            .apply(bareline_document::EditTransaction {
                base_revision: old.snapshot.revision,
                edits: vec![bareline_document::Edit {
                    range: TextOffset(0)..TextOffset(0),
                    insert: "x".into(),
                }],
            })
            .unwrap();
        let new = at(&edited, 0..8);
        assert!(new.supersedes(&old) && !old.supersedes(&new));
    }

    #[test]
    fn the_worker_checks_off_the_calling_thread_and_answers_suggestions() {
        let caller = std::thread::current().id();
        let factory: SpellCheckerFactory = Arc::new(move || {
            assert_ne!(
                std::thread::current().id(),
                caller,
                "the checker is built on the worker"
            );
            let (checker, _) = fake(&["good"]);
            Ok(Box::new(checker) as Box<dyn SpellChecker>)
        });
        let worker = Worker::spawn(factory, Arc::new(|| {})).unwrap();
        worker
            .shared
            .push(|state| state.submit(request("good baad", 0..9, None)));
        match worker.outcomes.recv_timeout(Duration::from_secs(30)).unwrap() {
            Outcome::Checked(result) => assert_eq!(result.misspelled, vec![TextOffset(5)..TextOffset(9)]),
            Outcome::Unavailable(reason) => panic!("{reason}"),
        }
        let (reply, receiver) = mpsc::channel();
        worker
            .shared
            .push(|state| state.commands.push_back(Command::Suggest("baad".into(), reply)));
        let list = receiver.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(list.len(), MAX_SUGGESTIONS, "top five suggestions");
    }

    #[test]
    fn a_view_is_checked_once_per_window_and_marked_by_the_pump() {
        let factory: SpellCheckerFactory = Arc::new(|| {
            let (checker, _) = fake(&["good"]);
            Ok(Box::new(checker) as Box<dyn SpellChecker>)
        });
        let mut spelling = Spelling::default();
        spelling.set_factory(factory);
        let text = "good baad";
        let mut editor = EditorSurface::loading(document(text).snapshot(), Arc::new(|| {}));
        editor.visible_text = TextOffset(0)..TextOffset(text.len());
        // Off (the default for a view) sends nothing and starts no worker.
        spelling.refresh(&mut editor, None, Arc::new(|| {}));
        assert!(spelling.worker.is_none());
        editor.spell_scope = SpellScope::CommentsAndStrings;
        spelling.refresh(&mut editor, None, Arc::new(|| {}));
        assert!(spelling.worker.is_none(), "code without lexer styles is not checked");
        editor.spell_scope = SpellScope::AllText;
        spelling.refresh(&mut editor, None, Arc::new(|| {}));
        spelling.refresh(&mut editor, None, Arc::new(|| {}));
        assert_eq!(spelling.requested.len(), 1, "an unchanged view is not checked again");
        let outcome = spelling
            .worker
            .as_ref()
            .unwrap()
            .outcomes
            .recv_timeout(Duration::from_secs(30))
            .unwrap();
        let Outcome::Checked(result) = outcome else {
            panic!("the fake checker is available");
        };
        let mut editors = [crate::workspace::WorkspaceEditor::Resident(editor)];
        let (sender, outcomes) = mpsc::channel();
        sender.send(Outcome::Checked(result)).unwrap();
        spelling.worker.as_mut().unwrap().outcomes = outcomes;
        assert!(spelling.pump(&mut editors));
        let surface = editors[0].resident().unwrap();
        assert_eq!(surface.spelling_mark_at(6), Some(5..9));
        assert_eq!(Target::at(surface, 6).unwrap().word, "baad");
        spelling.ignore_all("baad", &mut editors);
        assert_eq!(editors[0].resident().unwrap().spelling_mark_at(6), None);
        assert!(spelling.requested.is_empty(), "the visible text is checked again");
    }

    #[test]
    fn an_unavailable_checker_is_reported_once_and_stops_the_worker() {
        let factory: SpellCheckerFactory = Arc::new(|| Err("no dictionary".into()));
        let worker = Worker::spawn(factory.clone(), Arc::new(|| {})).unwrap();
        let outcome = worker.outcomes.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(matches!(&outcome, Outcome::Unavailable(reason) if reason == "no dictionary"));
        let (sender, outcomes) = mpsc::channel();
        sender.send(outcome).unwrap();
        let mut spelling = Spelling::default();
        spelling.set_factory(factory);
        spelling.worker = Some(Worker {
            shared: Arc::new(Shared::default()),
            outcomes,
        });
        assert!(spelling.pump(&mut []));
        assert!(
            spelling.worker.is_none(),
            "no more windows go to a checker that cannot start"
        );
        assert_eq!(spelling.take_unavailable_notice().as_deref(), Some("no dictionary"));
        assert_eq!(spelling.take_unavailable_notice(), None);
    }
}
