// SPDX-License-Identifier: MPL-2.0
//! Local, bounded completion and language-aware transaction preparation.
use crate::power::{self, CommentProvider, CommentTokens, Limits, PowerEdit, SelectionSet};
use bareline_document::{DocumentSnapshot, Edit, Error, TextOffset};
use bareline_syntax::{Language, StyleKind, SyntaxResult};
use std::{collections::BTreeMap, ops::Range};
#[derive(Clone, Copy, Debug)]
pub struct CompletionLimits {
    pub max_scan_bytes: usize,
    pub max_words: usize,
    pub max_bytes: usize,
    pub max_items: usize,
}
impl Default for CompletionLimits {
    fn default() -> Self {
        Self {
            max_scan_bytes: 256 << 10,
            max_words: 16_384,
            max_bytes: 2 << 20,
            max_items: 128,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionKind {
    Keyword,
    DocumentWord,
    Function,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub text: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
}
pub struct CompletionResult {
    source: DocumentSnapshot,
    pub replacement: Range<TextOffset>,
    pub items: Vec<CompletionItem>,
    pub partial: bool,
}
impl CompletionResult {
    pub fn is_current(&self, snapshot: &DocumentSnapshot) -> bool {
        self.source.same_document(snapshot) && self.source.revision == snapshot.revision
    }
}
/// Index replacement is atomic on budget failure. Updates scan explicit regions only.
#[derive(Default)]
pub struct WordIndex {
    source: Option<DocumentSnapshot>,
    words: BTreeMap<String, usize>,
    bytes: usize,
    pub partial: bool,
}
impl WordIndex {
    pub fn len(&self) -> usize {
        self.words.len()
    }
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn update(
        &mut self,
        snapshot: &DocumentSnapshot,
        range: Range<TextOffset>,
        limits: CompletionLimits,
        cancel: &bareline_syntax::Cancellation,
    ) -> Result<(), Error> {
        let text = snapshot.read(range.clone(), limits.max_scan_bytes)?;
        let mut words = BTreeMap::<String, usize>::new();
        let mut bytes = 0;
        for word in text
            .split(|c: char| c != '_' && !c.is_alphanumeric())
            .filter(|w| {
                w.chars()
                    .next()
                    .is_some_and(|c| c == '_' || c.is_alphabetic())
            })
        {
            if cancel.is_cancelled() {
                return Err(Error::IncompleteSource);
            }
            if word.len() > 128 {
                continue;
            }
            if let Some(count) = words.get_mut(word) {
                *count += 1;
            } else {
                if words.len() >= limits.max_words || bytes + word.len() > limits.max_bytes {
                    return Err(Error::BudgetExceeded);
                }
                bytes += word.len();
                words.insert(word.to_owned(), 1);
            }
        }
        self.words = words;
        self.bytes = bytes;
        self.partial =
            range.start.0 != 0 || range.end.0 < snapshot.len() || !snapshot.is_complete();
        self.source = Some(snapshot.clone());
        Ok(())
    }
    fn current(&self, snapshot: &DocumentSnapshot) -> bool {
        self.source
            .as_ref()
            .is_some_and(|s| s.same_document(snapshot) && s.revision == snapshot.revision)
    }
}
pub trait CompletionProvider {
    fn complete(
        &self,
        snapshot: &DocumentSnapshot,
        caret: TextOffset,
        language: Language,
        syntax: Option<&SyntaxResult>,
        limits: CompletionLimits,
    ) -> Result<CompletionResult, Error>;
}
impl CompletionProvider for WordIndex {
    fn complete(
        &self,
        snapshot: &DocumentSnapshot,
        caret: TextOffset,
        language: Language,
        syntax: Option<&SyntaxResult>,
        limits: CompletionLimits,
    ) -> Result<CompletionResult, Error> {
        if !snapshot.is_boundary(caret) {
            return Err(Error::InvalidBoundary);
        }
        let range = snapshot.line_range(snapshot.line_at(caret)?)?;
        let prefix = snapshot.read(range.start..caret, limits.max_scan_bytes)?;
        let prefix = prefix
            .rsplit(|c: char| c != '_' && !c.is_alphanumeric())
            .next()
            .unwrap_or("");
        let replacement = TextOffset(caret.0 - prefix.len())..caret;
        let supported = !syntax.filter(|s| s.is_current(snapshot)).is_some_and(|s| {
            s.spans.iter().any(|span| {
                span.range.start <= caret
                    && caret < span.range.end
                    && matches!(span.kind, StyleKind::String | StyleKind::Comment)
            })
        });
        let mut items = BTreeMap::<String, CompletionItem>::new();
        let mut bytes = 0;
        if supported {
            for word in language.metadata().keywords.split_ascii_whitespace() {
                if word.starts_with(prefix) && word != prefix {
                    if items.len() >= limits.max_items || bytes + word.len() > limits.max_bytes {
                        break;
                    }
                    bytes += word.len();
                    items.insert(
                        word.into(),
                        CompletionItem {
                            text: word.into(),
                            kind: CompletionKind::Keyword,
                            detail: None,
                        },
                    );
                }
            }
        }
        if self.current(snapshot) {
            for word in self
                .words
                .keys()
                .filter(|w| w.starts_with(prefix) && w.as_str() != prefix)
            {
                if items.len() >= limits.max_items || bytes + word.len() > limits.max_bytes {
                    break;
                }
                if !items.contains_key(word) {
                    bytes += word.len();
                    items.insert(
                        word.clone(),
                        CompletionItem {
                            text: word.clone(),
                            kind: CompletionKind::DocumentWord,
                            detail: None,
                        },
                    );
                }
            }
        }
        Ok(CompletionResult {
            source: snapshot.clone(),
            replacement,
            items: items.into_values().collect(),
            partial: self.partial || !self.current(snapshot),
        })
    }
}
pub fn accept(
    snapshot: &DocumentSnapshot,
    result: &CompletionResult,
    index: usize,
    selections: &SelectionSet,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    if !result.is_current(snapshot) {
        return Err(Error::StaleRevision);
    }
    let item = result.items.get(index).ok_or(Error::OutOfBounds)?;
    let needle = snapshot.read(result.replacement.clone(), limits.max_bytes)?;
    let mut set = power::normalize(snapshot, selections, limits)?;
    for s in &mut set.selections {
        if s.anchor != s.caret || s.caret < needle.len() {
            return Err(Error::InvalidBoundary);
        }
        let start = s.caret - needle.len();
        if snapshot.read(TextOffset(start)..TextOffset(s.caret), limits.max_bytes)? != needle {
            return Err(Error::InvalidBoundary);
        }
        s.anchor = start;
    }
    power::replace(snapshot, &set, &item.text, limits)
}
#[derive(Clone, Copy)]
pub struct LanguageComments(pub Language);
impl CommentProvider for LanguageComments {
    fn tokens_for(&self, _: &DocumentSnapshot) -> Option<CommentTokens> {
        let m = self.0.metadata();
        if m.line_comment.is_none() && m.block_comment.is_none() {
            None
        } else {
            Some(CommentTokens {
                line: m.line_comment.map(str::to_owned),
                block: m.block_comment.map(|(a, b)| (a.into(), b.into())),
            })
        }
    }
}
pub fn toggle_comment(
    snapshot: &DocumentSnapshot,
    selections: &SelectionSet,
    language: Language,
    block: bool,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    let tokens = LanguageComments(language)
        .tokens_for(snapshot)
        .ok_or(Error::OutOfBounds)?;
    let set = power::normalize(snapshot, selections, limits)?;
    let mut edits = Vec::new();
    let mut bytes = 0;
    if block {
        let (open, close) = tokens.block.ok_or(Error::OutOfBounds)?;
        for s in set.selections {
            let range = TextOffset(s.anchor)..TextOffset(s.caret);
            let text = snapshot.read(range.clone(), limits.max_bytes)?;
            let insert = if let Some(inner) = text
                .strip_prefix(&open)
                .and_then(|s| s.strip_suffix(&close))
            {
                inner.into()
            } else {
                format!("{open}{text}{close}")
            };
            bytes += text.len() + insert.len();
            if bytes > limits.max_bytes {
                return Err(Error::BudgetExceeded);
            }
            edits.push(Edit { range, insert });
        }
    } else {
        let token = tokens.line.ok_or(Error::OutOfBounds)?;
        let mut lines = std::collections::BTreeSet::new();
        for s in set.selections {
            let first = snapshot.line_at(TextOffset(s.anchor))?;
            let mut end = s.caret;
            if end > s.anchor {
                end -= 1;
                while !snapshot.is_boundary(TextOffset(end)) {
                    end -= 1;
                }
            }
            let last = snapshot.line_at(TextOffset(end))?;
            for line in first..=last {
                if lines.len() >= limits.max_selections {
                    return Err(Error::BudgetExceeded);
                }
                lines.insert(line);
            }
        }
        let mut rows = Vec::new();
        let mut all_commented = true;
        let mut indent = usize::MAX;
        for n in lines {
            let range = snapshot.line_range(n)?;
            let text = snapshot.read(range.clone(), limits.max_bytes)?;
            bytes += text.len();
            if bytes > limits.max_bytes {
                return Err(Error::BudgetExceeded);
            }
            let leading = text.len() - text.trim_start_matches([' ', '\t']).len();
            if !text.trim().is_empty() {
                indent = indent.min(leading);
                all_commented &= text[leading..].starts_with(&token);
            }
            rows.push((range, text, leading));
        }
        let indent = indent.min(limits.max_bytes);
        for (range, text, leading) in rows {
            if text.trim().is_empty() {
                continue;
            }
            let offset = if all_commented { leading } else { indent };
            let mut end = offset;
            if all_commented {
                end += token.len();
                if text[end..].starts_with(' ') {
                    end += 1;
                }
            }
            edits.push(Edit {
                range: TextOffset(range.start.0 + offset)..TextOffset(range.start.0 + end),
                insert: if all_commented {
                    String::new()
                } else {
                    format!("{token} ")
                },
            });
        }
    }
    power::finish(snapshot, edits, limits)
}
pub fn smart_pair(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    typed: char,
    language: Language,
    syntax: Option<&SyntaxResult>,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    let set = power::normalize(snapshot, set, limits)?;
    let close = match typed {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' => '"',
        '\'' => '\'',
        _ => return power::replace(snapshot, &set, &typed.to_string(), limits),
    };
    if language == Language::PlainText {
        return power::replace(snapshot, &set, &typed.to_string(), limits);
    }
    let mut edits = Vec::new();
    let mut paired = Vec::new();
    for s in &set.selections {
        let suppressed = syntax
            .filter(|s| s.is_current(snapshot))
            .is_some_and(|syntax| {
                syntax.spans.iter().any(|span| {
                    span.range.start.0 <= s.caret
                        && s.caret < span.range.end.0
                        && matches!(span.kind, StyleKind::String | StyleKind::Comment)
                })
            });
        let range = TextOffset(s.anchor)..TextOffset(s.caret);
        let original = snapshot.read(range.clone(), limits.max_bytes)?;
        edits.push(Edit {
            range,
            insert: if suppressed {
                typed.to_string()
            } else {
                format!("{typed}{original}{close}")
            },
        });
        paired.push(!suppressed);
    }
    let mut edit = power::finish(snapshot, edits, limits)?;
    for ((s, before), paired) in edit
        .selections
        .selections
        .iter_mut()
        .zip(set.selections)
        .zip(paired)
    {
        if paired && s.caret > 0 && before.anchor == before.caret {
            s.caret -= close.len_utf8();
            s.anchor = s.caret;
        }
    }
    Ok(edit)
}
#[derive(Clone, Debug)]
pub struct Signature {
    pub name: String,
    pub display: String,
}
/// Data-only tab-separated name/signature loader; callers own bounded file reads.
pub fn load_signatures(
    text: &str,
    max_bytes: usize,
    max_items: usize,
) -> Result<Vec<Signature>, Error> {
    if text.len() > max_bytes {
        return Err(Error::BudgetExceeded);
    }
    let mut out = Vec::new();
    for line in text.lines().filter(|s| !s.trim().is_empty()) {
        if out.len() >= max_items {
            return Err(Error::BudgetExceeded);
        }
        let (name, display) = line.split_once('\t').ok_or(Error::InvalidBoundary)?;
        if name.is_empty()
            || name.len() > 128
            || display.len() > 4096
            || !name.chars().all(|c| c == '_' || c.is_alphanumeric())
        {
            return Err(Error::InvalidBoundary);
        }
        out.push(Signature {
            name: name.into(),
            display: display.into(),
        });
    }
    Ok(out)
}
/// Popup selection is explicit: Enter/Tab should be consumed only when this returns an index.
#[derive(Default)]
pub struct CompletionPopup {
    pub result: Option<CompletionResult>,
    pub selected: Option<usize>,
}
impl CompletionPopup {
    pub fn open(&mut self, result: CompletionResult, select_first: bool) {
        self.selected = (select_first && !result.items.is_empty()).then_some(0);
        self.result = Some(result);
    }
    pub fn navigate(&mut self, down: bool) {
        let count = self.result.as_ref().map_or(0, |r| r.items.len());
        if count == 0 {
            self.selected = None;
            return;
        }
        self.selected = Some(match self.selected {
            None => {
                if down {
                    0
                } else {
                    count - 1
                }
            }
            Some(n) => {
                if down {
                    (n + 1) % count
                } else {
                    (n + count - 1) % count
                }
            }
        });
    }
    pub fn acceptance(&self, snapshot: &DocumentSnapshot) -> Option<usize> {
        self.result
            .as_ref()
            .filter(|r| r.is_current(snapshot))
            .and(self.selected)
    }
    pub fn close(&mut self) {
        self.result = None;
        self.selected = None;
    }
}
pub fn smart_newline(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    language: Language,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    let set = power::normalize(snapshot, set, limits)?;
    let mut edits = Vec::new();
    let mut bytes = 0;
    let eol = match snapshot.eol_label() {
        "CRLF" => "\r\n",
        "CR" => "\r",
        _ => "\n",
    };
    for s in set.selections {
        let line = snapshot.line_range(snapshot.line_at(TextOffset(s.anchor))?)?;
        let prefix = snapshot.read(line.start..TextOffset(s.anchor), limits.max_bytes)?;
        let indent: String = prefix
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let extra = prefix
            .trim_end()
            .chars()
            .next_back()
            .is_some_and(|c| language.metadata().indent_after.contains(c));
        let added = if extra { limits.tab_width } else { 0 };
        bytes += eol.len() + indent.len() + added;
        if bytes > limits.max_bytes {
            return Err(Error::BudgetExceeded);
        }
        let insert = format!("{eol}{indent}{}", " ".repeat(added));
        edits.push(Edit {
            range: TextOffset(s.anchor)..TextOffset(s.caret),
            insert,
        });
    }
    power::finish(snapshot, edits, limits)
}
pub fn pair_backspace(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    let mut normalized = power::normalize(snapshot, set, limits)?;
    for s in &mut normalized.selections {
        if s.anchor != s.caret || s.caret == 0 || s.caret >= snapshot.len() {
            continue;
        }
        if !snapshot.is_boundary(TextOffset(s.caret - 1))
            || !snapshot.is_boundary(TextOffset(s.caret + 1))
        {
            continue;
        }
        let around = snapshot.read(TextOffset(s.caret - 1)..TextOffset(s.caret + 1), 2)?;
        if matches!(around.as_str(), "()" | "[]" | "{}" | "\"\"" | "''") {
            s.anchor -= 1;
            s.caret += 1;
        }
    }
    power::delete(snapshot, &normalized, true, limits)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Selection;
    use bareline_document::{Budget, Document};
    fn doc(s: &str) -> Document {
        Document::from_utf8(s, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap()
    }
    fn text(d: &Document) -> String {
        let s = d.snapshot();
        s.read(TextOffset(0)..TextOffset(s.len()), 1 << 20).unwrap()
    }
    #[test]
    fn stale_completion_and_failed_index_update_are_safe() {
        let mut d = doc("fn foobar() {}\nfo");
        let snapshot = d.snapshot();
        let mut index = WordIndex::default();
        index
            .update(
                &snapshot,
                TextOffset(0)..TextOffset(snapshot.len()),
                CompletionLimits::default(),
                &Default::default(),
            )
            .unwrap();
        let result = index
            .complete(
                &snapshot,
                TextOffset(snapshot.len()),
                Language::Rust,
                None,
                CompletionLimits::default(),
            )
            .unwrap();
        assert!(result.items.iter().any(|i| i.text == "foobar"));
        let original = index.len();
        assert!(
            index
                .update(
                    &snapshot,
                    TextOffset(0)..TextOffset(snapshot.len()),
                    CompletionLimits {
                        max_words: 0,
                        ..Default::default()
                    },
                    &Default::default()
                )
                .is_err()
        );
        assert_eq!(index.len(), original);
        d.apply(
            power::replace(
                &snapshot,
                &Selection::default().into(),
                "x",
                Limits::default(),
            )
            .unwrap()
            .transaction,
        )
        .unwrap();
        assert!(matches!(
            accept(
                &d.snapshot(),
                &result,
                0,
                &Selection {
                    anchor: snapshot.len(),
                    caret: snapshot.len()
                }
                .into(),
                Limits::default()
            ),
            Err(Error::StaleRevision)
        ));
    }
    #[test]
    fn mixed_comments_pairs_and_one_undo() {
        let mut d = doc("  let a=1;\n  // let b=2;\n");
        let set = Selection {
            anchor: 0,
            caret: d.snapshot().len(),
        }
        .into();
        let e = toggle_comment(
            &d.snapshot(),
            &set,
            Language::Rust,
            false,
            Limits::default(),
        )
        .unwrap();
        d.apply(e.transaction).unwrap();
        assert_eq!(text(&d), "  // let a=1;\n  // // let b=2;\n");
        d.undo().unwrap();
        assert_eq!(text(&d), "  let a=1;\n  // let b=2;\n");
        let mut d = doc("a\nb");
        let set = SelectionSet {
            selections: vec![
                Selection {
                    anchor: 1,
                    caret: 1,
                },
                Selection {
                    anchor: 3,
                    caret: 3,
                },
            ],
            primary: 0,
        };
        let e = smart_pair(
            &d.snapshot(),
            &set,
            '(',
            Language::Rust,
            None,
            Limits::default(),
        )
        .unwrap();
        d.apply(e.transaction).unwrap();
        assert_eq!(text(&d), "a()\nb()");
        let e = pair_backspace(&d.snapshot(), &e.selections, Limits::default()).unwrap();
        d.apply(e.transaction).unwrap();
        assert_eq!(text(&d), "a\nb");
        d.undo().unwrap();
        d.undo().unwrap();
        assert_eq!(text(&d), "a\nb");
        assert_eq!(d.undo(), Err(Error::EmptyHistory));
    }
    #[test]
    fn popup_does_not_accept_without_active_selection_and_loader_bounds() {
        let d = doc("ret");
        let index = WordIndex::default();
        let r = index
            .complete(
                &d.snapshot(),
                TextOffset(3),
                Language::Rust,
                None,
                Default::default(),
            )
            .unwrap();
        let mut popup = CompletionPopup::default();
        popup.open(r, false);
        assert_eq!(popup.acceptance(&d.snapshot()), None);
        popup.navigate(true);
        assert_eq!(popup.acceptance(&d.snapshot()), Some(0));
        assert!(load_signatures("bad", 100, 2).is_err());
        assert_eq!(load_signatures("f\tf(value)", 100, 2).unwrap()[0].name, "f");
    }
}
/// Nearby signature hints are suppressed unless a current lexer excludes strings/comments.
pub fn parameter_hint<'a>(
    snapshot: &DocumentSnapshot,
    caret: TextOffset,
    syntax: Option<&SyntaxResult>,
    signatures: &'a [Signature],
) -> Result<Option<(&'a Signature, usize)>, Error> {
    let Some(syntax) =
        syntax.filter(|s| s.is_current(snapshot) && s.status == bareline_syntax::Status::Complete)
    else {
        return Ok(None);
    };
    let line = snapshot.line_range(snapshot.line_at(caret)?)?;
    if line.start < syntax.range.start || caret > syntax.range.end {
        return Ok(None);
    }
    let text = snapshot.read(line.start..caret, 4096)?;
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for (i, c) in text.char_indices() {
        let offset = line.start.0 + i;
        if syntax.spans.iter().any(|s| {
            s.range.start.0 <= offset
                && offset < s.range.end.0
                && matches!(s.kind, StyleKind::String | StyleKind::Comment)
        }) {
            continue;
        }
        match c {
            '(' => {
                if stack.len() >= 128 {
                    return Ok(None);
                }
                stack.push((i, 0));
            }
            ')' => {
                stack.pop();
            }
            ',' => {
                if let Some((_, argument)) = stack.last_mut() {
                    *argument += 1;
                }
            }
            _ => {}
        }
    }
    let Some((open, argument)) = stack.last() else {
        return Ok(None);
    };
    let prefix = text[..*open].trim_end();
    let name = prefix
        .rsplit(|c: char| c != '_' && !c.is_alphanumeric())
        .next()
        .unwrap_or("");
    Ok(signatures
        .iter()
        .find(|s| s.name == name)
        .map(|s| (s, *argument)))
}
pub fn overtype_closer(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    typed: char,
    limits: Limits,
) -> Result<Option<SelectionSet>, Error> {
    if !matches!(typed, ')' | ']' | '}' | '"' | '\'') {
        return Ok(None);
    }
    let mut set = power::normalize(snapshot, set, limits)?;
    for s in &mut set.selections {
        if s.anchor != s.caret || s.caret + typed.len_utf8() > snapshot.len() {
            return Ok(None);
        }
        if snapshot.read(
            TextOffset(s.caret)..TextOffset(s.caret + typed.len_utf8()),
            typed.len_utf8(),
        )? != typed.to_string()
        {
            return Ok(None);
        }
        s.caret += typed.len_utf8();
        s.anchor = s.caret;
    }
    Ok(Some(set))
}
