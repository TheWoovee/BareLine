// SPDX-License-Identifier: MPL-2.0
//! Worker-side search over stable snapshots. No file I/O or UI dependencies.
mod disk_source;
mod extended;
mod fold;
pub mod folders;
pub mod paged;
mod regex;
pub mod replace_disk;
pub mod replace_files;
pub mod service;
pub mod sources;
mod word;
use bareline_document::{DocumentSnapshot, Edit, EditTransaction, Revision, TextOffset};
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
};

pub const MAX_PATTERN_BYTES: usize = 64 * 1024;
pub const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;
const BATCH_SIZE: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchJobId(u64);
#[derive(Clone)]
pub struct SearchJob {
    pub id: SearchJobId,
    cancelled: Arc<AtomicBool>,
    terminal: Arc<AtomicU8>,
    io_cancel: bareline_file_io::cancellation::Cancellation,
}
impl Default for SearchJob {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: SearchJobId(NEXT.fetch_add(1, Ordering::Relaxed)),
            cancelled: Arc::default(),
            terminal: Arc::default(),
            io_cancel: Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchTermination {
    Finished,
    Cancelled,
}
impl SearchJob {
    /// Worker acknowledgment, independent from request cancellation and result completeness.
    pub fn termination(&self) -> Option<SearchTermination> {
        match self.terminal.load(Ordering::Acquire) {
            1 => Some(SearchTermination::Finished),
            2 => Some(SearchTermination::Cancelled),
            _ => None,
        }
    }
    pub(crate) fn acknowledge_terminal(&self) {
        self.terminal
            .store(if self.is_cancelled() { 2 } else { 1 }, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.io_cancel.cancel();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMode {
    Literal,
    Extended,
    Regex,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    Sensitive,
    Folded,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    pub pattern: String,
    pub mode: SearchMode,
    pub case: Case,
    pub whole_word: bool,
    /// Text-domain bounds; None searches the complete snapshot.
    pub selection: Option<Range<TextOffset>>,
    pub results_ram_bytes: usize,
    /// Continue counting after retained matches reach the result budget.
    pub count_beyond_limit: bool,
}
impl SearchQuery {
    pub fn literal(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            mode: SearchMode::Literal,
            case: Case::Sensitive,
            whole_word: false,
            selection: None,
            results_ram_bytes: MAX_RESULT_BYTES,
            count_beyond_limit: false,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    Complete,
    Cancelled,
    ResultLimit,
    Unsupported,
    UnsupportedStreaming,
    RegexLimit,
    InvalidQuery,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub range: Range<TextOffset>,
}
/// Every entry is in the source snapshot's UTF-8 text domain. Line hints are omitted.
pub struct SearchBatch<'a> {
    pub job: SearchJobId,
    pub revision: Revision,
    pub source: &'a DocumentSnapshot,
    pub matches: &'a [SearchMatch],
}
pub struct SearchResults {
    pub job: SearchJobId,
    source: DocumentSnapshot,
    matches: Vec<SearchMatch>,
    completeness: Completeness,
    captures: Option<Vec<Vec<Option<Range<TextOffset>>>>>,
    capture_names: Vec<(String, usize)>,
    total_count: usize,
    count_complete: bool,
}
impl SearchResults {
    /// Stable source identity and revision for grouped results/navigation.
    pub fn source(&self) -> &DocumentSnapshot {
        &self.source
    }
    /// Owned result storage, excluding shared document snapshot storage.
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.matches.capacity() * std::mem::size_of::<SearchMatch>()
            + self.capture_names.capacity() * std::mem::size_of::<(String, usize)>()
            + self
                .capture_names
                .iter()
                .map(|(name, _)| name.capacity())
                .sum::<usize>()
            + self.captures.as_ref().map_or(0, |groups| {
                groups.capacity() * std::mem::size_of::<Vec<Option<Range<TextOffset>>>>()
                    + groups
                        .iter()
                        .map(|group| group.capacity() * std::mem::size_of::<Option<Range<TextOffset>>>())
                        .sum::<usize>()
            })
    }
    pub fn matches(&self) -> &[SearchMatch] {
        &self.matches
    }
    pub fn completeness(&self) -> Completeness {
        self.completeness
    }
    /// Count is exact only for Complete results, otherwise it is a lower bound.
    pub fn count(&self) -> usize {
        self.total_count
    }
    pub fn count_complete(&self) -> bool {
        self.count_complete
    }
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }
    pub fn is_current(&self, current: &DocumentSnapshot) -> bool {
        self.source.same_document(current) && self.source.revision == current.revision
    }
    pub fn next(&self, at: TextOffset, backwards: bool, wrap: bool) -> Option<&SearchMatch> {
        if backwards {
            let index = self.matches.partition_point(|m| m.range.start < at);
            index
                .checked_sub(1)
                .and_then(|i| self.matches.get(i))
                .or_else(|| wrap.then(|| self.matches.last()).flatten())
        } else {
            let index = self.matches.partition_point(|m| m.range.start < at);
            self.matches
                .get(index)
                .or_else(|| wrap.then(|| self.matches.first()).flatten())
        }
    }
    /// Prepares owned edits only after complete results and identity/revision validation.
    /// DocumentService still validates the revision atomically at actual commit.
    pub fn prepare_replace(
        &self,
        current: &DocumentSnapshot,
        replacement: &str,
        staging_limit: usize,
    ) -> Result<EditTransaction, ReplaceError> {
        self.prepare_replace_scoped(
            current,
            replacement,
            staging_limit,
            ReplaceScope::All,
            &SearchJob::default(),
        )
    }
    /// Run on a worker: staging is bounded and cancellation never returns partial edits.
    pub fn prepare_replace_scoped(
        &self,
        current: &DocumentSnapshot,
        replacement: &str,
        staging_limit: usize,
        scope: ReplaceScope,
        job: &SearchJob,
    ) -> Result<EditTransaction, ReplaceError> {
        self.prepare_replace_ranges(current, replacement, staging_limit, scope, job, true)
    }
    fn prepare_replace_ranges(
        &self,
        current: &DocumentSnapshot,
        replacement: &str,
        staging_limit: usize,
        scope: ReplaceScope,
        job: &SearchJob,
        include_inverse: bool,
    ) -> Result<EditTransaction, ReplaceError> {
        if self.completeness != Completeness::Complete {
            return Err(ReplaceError::Incomplete);
        }
        if !self.is_current(current) {
            return Err(ReplaceError::Stale);
        }
        if job.cancelled.load(Ordering::Acquire) {
            return Err(ReplaceError::Cancelled);
        }
        let matches = match scope {
            ReplaceScope::All => self.matches.as_slice(),
            ReplaceScope::One(range) => {
                let index = self.matches.partition_point(|m| m.range.start < range.start);
                let found = self
                    .matches
                    .get(index)
                    .filter(|m| m.range == range)
                    .ok_or(ReplaceError::NoMatch)?;
                std::slice::from_ref(found)
            }
        };
        if matches.is_empty() {
            return Err(ReplaceError::NoMatch);
        }
        let limit = staging_limit.min(MAX_RESULT_BYTES);
        let mut used = 0usize;
        let mut edits = Vec::new();
        for m in matches {
            if job.is_cancelled() {
                return Err(ReplaceError::Cancelled);
            }
            used = used
                .checked_add(if include_inverse {
                    m.range.end.0 - m.range.start.0
                } else {
                    0
                })
                .and_then(|n| n.checked_add(std::mem::size_of::<Edit>()))
                .ok_or(ReplaceError::StagingLimit)?;
            if used > limit {
                return Err(ReplaceError::StagingLimit);
            }
            let insert = if let Some(captures) = &self.captures {
                let index = self
                    .matches
                    .partition_point(|candidate| candidate.range.start < m.range.start);
                regex::expand(
                    replacement,
                    &captures[index],
                    &self.capture_names,
                    current,
                    limit - used,
                )?
            } else {
                if replacement.len() > limit - used {
                    return Err(ReplaceError::StagingLimit);
                }
                replacement.to_owned()
            };
            used += insert.len();
            edits.push(Edit {
                range: m.range.clone(),
                insert,
            });
        }
        if job.cancelled.load(Ordering::Acquire) {
            return Err(ReplaceError::Cancelled);
        }
        Ok(EditTransaction {
            base_revision: current.revision,
            edits,
        })
    }
}
pub enum ReplaceScope {
    All,
    One(Range<TextOffset>),
}
#[derive(Debug, PartialEq, Eq)]
pub enum ReplaceError {
    Incomplete,
    Stale,
    StagingLimit,
    Cancelled,
    NoMatch,
    InvalidReplacement,
}
pub fn decode_replacement(value: &str, mode: SearchMode) -> Result<String, ReplaceError> {
    match mode {
        SearchMode::Literal => Ok(value.into()),
        SearchMode::Extended => extended::decode(value).ok_or(ReplaceError::InvalidReplacement),
        SearchMode::Regex => Ok(value.into()),
    }
}

/// Linear-time, non-overlapping literal scan. Prefix state crosses every chunk boundary;
/// no whole-line/window copy is needed. Cancellation checkpoints are at most 4096 bytes apart.
/// The callback borrows batches; retaining copies is the consumer's budget responsibility.
pub fn scan(
    snapshot: &DocumentSnapshot,
    query: &SearchQuery,
    job: &SearchJob,
    mut emit: impl FnMut(SearchBatch<'_>),
) -> SearchResults {
    let mut result = SearchResults {
        job: job.id,
        source: snapshot.clone(),
        matches: Vec::new(),
        completeness: Completeness::Complete,
        captures: None,
        capture_names: Vec::new(),
        total_count: 0,
        count_complete: false,
    };
    if job.cancelled.load(Ordering::Acquire) {
        result.completeness = Completeness::Cancelled;
        return result;
    }
    if !snapshot.is_complete() {
        result.completeness = Completeness::Unsupported;
        return result;
    }
    if query.mode == SearchMode::Regex {
        return regex::scan(snapshot, query, job, emit);
    }
    if query.pattern.is_empty() || query.pattern.len() > MAX_PATTERN_BYTES {
        result.completeness = Completeness::InvalidQuery;
        return result;
    }
    let expanded;
    let query_pattern = if query.mode == SearchMode::Extended {
        let Some(value) = extended::decode(&query.pattern) else {
            result.completeness = Completeness::InvalidQuery;
            return result;
        };
        expanded = value;
        expanded.as_str()
    } else {
        query.pattern.as_str()
    };
    let mut folded_pattern = String::new();
    if query.case == Case::Folded {
        let mut buffer = [0; 12];
        for c in query_pattern.chars() {
            let mapped = fold::character(c, &mut buffer);
            if folded_pattern.len() + mapped.len() > MAX_PATTERN_BYTES {
                result.completeness = Completeness::InvalidQuery;
                return result;
            }
            folded_pattern.push_str(mapped);
        }
    }
    let pattern = if query.case == Case::Folded {
        folded_pattern.as_bytes()
    } else {
        query_pattern.as_bytes()
    };
    let range = query
        .selection
        .clone()
        .unwrap_or(TextOffset(0)..TextOffset(snapshot.len()));
    let Ok(chunks) = snapshot.chunks(range.clone()) else {
        result.completeness = Completeness::InvalidQuery;
        return result;
    };
    let mut prefix = vec![0usize; pattern.len()];
    let mut matched = 0;
    for i in 1..pattern.len() {
        while matched > 0 && pattern[i] != pattern[matched] {
            matched = prefix[matched - 1];
        }
        if pattern[i] == pattern[matched] {
            matched += 1;
        }
        prefix[i] = matched;
    }
    matched = 0;
    let capacity = query.results_ram_bytes.min(MAX_RESULT_BYTES) / std::mem::size_of::<SearchMatch>();
    result.matches = Vec::with_capacity(capacity.min(BATCH_SIZE));
    let mut emitted = 0;
    let mut offset = range.start.0;
    let mut seen = 0usize;
    let mut mapping = if query.case == Case::Folded {
        vec![(0usize, false); pattern.len()]
    } else {
        Vec::new()
    };
    'scan: for chunk in chunks {
        for unit in fold::MappedBytes::new(chunk, offset, query.case == Case::Folded) {
            if seen.is_multiple_of(4096) && job.cancelled.load(Ordering::Acquire) {
                result.completeness = Completeness::Cancelled;
                break 'scan;
            }
            if !mapping.is_empty() {
                mapping[seen % pattern.len()] = (unit.start, unit.first);
            }
            seen += 1;
            while matched > 0 && unit.byte != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if unit.byte == pattern[matched] {
                matched += 1;
            }
            if matched == pattern.len() {
                let (start, first) = if mapping.is_empty() {
                    (unit.end - matched, true)
                } else {
                    mapping[(seen - matched) % pattern.len()]
                };
                if !first || !unit.last {
                    matched = prefix[matched - 1];
                    continue;
                }
                if query.whole_word {
                    match word::boundaries(snapshot, start, unit.end) {
                        Some(true) => {}
                        Some(false) => {
                            matched = prefix[matched - 1];
                            continue;
                        }
                        None => {
                            result.completeness = Completeness::InvalidQuery;
                            break 'scan;
                        }
                    }
                }
                result.total_count += 1;
                if result.matches.len() == capacity {
                    result.completeness = Completeness::ResultLimit;
                    if query.count_beyond_limit {
                        matched = 0;
                        continue;
                    }
                    result.total_count -= 1;
                    break 'scan;
                }
                if result.matches.len() == result.matches.capacity() {
                    result
                        .matches
                        .reserve_exact((capacity - result.matches.len()).min(BATCH_SIZE));
                }
                result.matches.push(SearchMatch {
                    range: TextOffset(start)..TextOffset(unit.end),
                });
                matched = 0;
                if result.matches.len() - emitted == BATCH_SIZE {
                    emit(SearchBatch {
                        job: job.id,
                        revision: snapshot.revision,
                        source: snapshot,
                        matches: &result.matches[emitted..],
                    });
                    emitted = result.matches.len();
                }
            }
        }
        offset += chunk.len();
    }
    if job.cancelled.load(Ordering::Acquire) {
        result.completeness = Completeness::Cancelled;
    }
    if emitted < result.matches.len() {
        emit(SearchBatch {
            job: job.id,
            revision: snapshot.revision,
            source: snapshot,
            matches: &result.matches[emitted..],
        });
    }
    if job.cancelled.load(Ordering::Acquire) {
        result.completeness = Completeness::Cancelled;
    }
    result.count_complete = result.completeness == Completeness::Complete
        || (query.count_beyond_limit && result.completeness == Completeness::ResultLimit);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    #[test]
    fn extended_mode_finds_multiline_unicode_and_rejects_malformed_escapes() {
        let snapshot = document("a\r\n\t🦀\0z").snapshot();
        let mut query = SearchQuery::literal(r"\r\n\t\U0001F980\0");
        query.mode = SearchMode::Extended;
        let result = scan(&snapshot, &query, &SearchJob::default(), |_| {});
        assert_eq!(result.completeness(), Completeness::Complete);
        assert_eq!(result.matches()[0].range, TextOffset(1)..TextOffset(9));
        for pattern in [r"\", r"\q", r"\x0", r"\uD800", r"\U00110000"] {
            query.pattern = pattern.into();
            assert_eq!(
                scan(&snapshot, &query, &SearchJob::default(), |_| {}).completeness(),
                Completeness::InvalidQuery
            );
        }
    }
    #[test]
    fn count_can_finish_after_retained_results_fill_without_enabling_replace() {
        let snapshot = document("a a a a").snapshot();
        let mut query = SearchQuery::literal("a");
        query.results_ram_bytes = std::mem::size_of::<SearchMatch>();
        query.count_beyond_limit = true;
        let result = scan(&snapshot, &query, &SearchJob::default(), |_| {});
        assert_eq!(result.matches().len(), 1);
        assert_eq!(result.count(), 4);
        assert!(result.count_complete());
        assert_eq!(result.completeness(), Completeness::ResultLimit);
        assert_eq!(
            result.prepare_replace(&snapshot, "b", 4096).err(),
            Some(ReplaceError::Incomplete)
        );
    }
    fn document(text: &str) -> Document {
        Document::from_utf8(text, Budget::new(64 << 20), Budget::new(32 << 20)).unwrap()
    }
    #[test]
    fn literal_crosses_sixteen_mib_in_twenty_mib_line_and_replaces_atomically() {
        let boundary = 16 << 20;
        let mut text = "x".repeat(boundary - 2);
        text.push_str("🦀needle");
        text.push_str(&"x".repeat((20 << 20) - text.len()));
        let mut doc = document(&text);
        let snapshot = doc.snapshot();
        let mut streamed = Vec::new();
        let result = scan(
            &snapshot,
            &SearchQuery::literal("🦀needle"),
            &SearchJob::default(),
            |batch| {
                assert_eq!(batch.revision, snapshot.revision);
                assert!(batch.source.same_document(&snapshot));
                streamed.extend_from_slice(batch.matches);
            },
        );
        assert_eq!(result.completeness(), Completeness::Complete);
        assert_eq!(result.matches(), streamed);
        assert_eq!(
            result.matches()[0].range,
            TextOffset(boundary - 2)..TextOffset(boundary + 8)
        );
        doc.apply(result.prepare_replace(&snapshot, "found", 1024).unwrap())
            .unwrap();
        assert_eq!(
            doc.snapshot()
                .read(TextOffset(boundary - 2)..TextOffset(boundary + 3), 5)
                .unwrap(),
            "found"
        );
        assert_eq!(
            result.prepare_replace(&doc.snapshot(), "bad", 1024).err(),
            Some(ReplaceError::Stale)
        );
        doc.undo().unwrap();
        assert_eq!(
            doc.snapshot()
                .read(TextOffset(boundary - 2)..TextOffset(boundary + 8), 10)
                .unwrap(),
            "🦀needle"
        );
    }
    #[test]
    fn replacement_scope_cancellation_and_atomic_undo() {
        let mut doc = document("cat cat cat");
        let snapshot = doc.snapshot();
        let results = scan(&snapshot, &SearchQuery::literal("cat"), &SearchJob::default(), |_| {});
        let job = SearchJob::default();
        assert_eq!(
            results
                .prepare_replace_scoped(
                    &snapshot,
                    "dog",
                    1024,
                    ReplaceScope::One(TextOffset(1)..TextOffset(3)),
                    &job
                )
                .err(),
            Some(ReplaceError::NoMatch)
        );
        let one = results
            .prepare_replace_scoped(
                &snapshot,
                "dog",
                1024,
                ReplaceScope::One(TextOffset(4)..TextOffset(7)),
                &job,
            )
            .unwrap();
        assert_eq!(one.edits.len(), 1);
        doc.apply(one).unwrap();
        assert_eq!(
            doc.snapshot().read(TextOffset(0)..TextOffset(11), 11).unwrap(),
            "cat dog cat"
        );
        doc.undo().unwrap();
        let snapshot = doc.snapshot();
        let results = scan(&snapshot, &SearchQuery::literal("cat"), &SearchJob::default(), |_| {});
        job.cancel();
        assert_eq!(
            results
                .prepare_replace_scoped(&snapshot, "dog", 1024, ReplaceScope::All, &job)
                .err(),
            Some(ReplaceError::Cancelled)
        );
        assert_eq!(doc.snapshot().revision, snapshot.revision);
        doc.apply(results.prepare_replace(&snapshot, "x", 1024).unwrap())
            .unwrap();
        assert_eq!(doc.snapshot().read(TextOffset(0)..TextOffset(5), 5).unwrap(), "x x x");
        doc.undo().unwrap();
        assert_eq!(
            doc.snapshot().read(TextOffset(0)..TextOffset(11), 11).unwrap(),
            "cat cat cat"
        );
    }
    #[test]
    fn bounded_results_cancellation_and_unsupported_queries_cannot_replace() {
        let snapshot = document(&"ab".repeat(100_000)).snapshot();
        let mut query = SearchQuery::literal("ab");
        query.results_ram_bytes = 2 * std::mem::size_of::<SearchMatch>();
        let capped = scan(&snapshot, &query, &SearchJob::default(), |_| {});
        assert_eq!(capped.count(), 2);
        assert_eq!(capped.completeness(), Completeness::ResultLimit);
        assert_eq!(
            capped.prepare_replace(&snapshot, "", 1024).err(),
            Some(ReplaceError::Incomplete)
        );
        query.results_ram_bytes = MAX_RESULT_BYTES;
        let job = SearchJob::default();
        let cancelled = scan(&snapshot, &query, &job, |_| job.cancel());
        assert_eq!(cancelled.completeness(), Completeness::Cancelled);
        assert!(
            cancelled.count() <= 2048,
            "stop within one checkpoint after callback cancellation"
        );
        query.mode = SearchMode::Regex;
        query.pattern = "[".into();
        assert_eq!(
            scan(&snapshot, &query, &SearchJob::default(), |_| {}).completeness(),
            Completeness::InvalidQuery
        );
    }
    #[test]
    fn selection_nonoverlap_navigation_and_document_identity() {
        let snapshot = document("éaaaa éaa").snapshot();
        let mut query = SearchQuery::literal("aa");
        query.selection = Some(TextOffset(2)..TextOffset(6));
        let result = scan(&snapshot, &query, &SearchJob::default(), |_| {});
        assert_eq!(result.count(), 2);
        assert_eq!(
            result.next(TextOffset(4), false, false).unwrap().range.start,
            TextOffset(4)
        );
        assert_eq!(
            result.next(TextOffset(2), true, true).unwrap().range.start,
            TextOffset(4)
        );
        assert_eq!(
            result
                .prepare_replace(&document("éaaaa éaa").snapshot(), "x", 1024)
                .err(),
            Some(ReplaceError::Stale)
        );
        assert_eq!(
            result.prepare_replace(&snapshot, "x", 0).err(),
            Some(ReplaceError::StagingLimit)
        );
        query.selection = Some(TextOffset(1)..TextOffset(6));
        assert_eq!(
            scan(&snapshot, &query, &SearchJob::default(), |_| {}).completeness(),
            Completeness::InvalidQuery
        );
    }
    #[test]
    fn full_unicode_folding_preserves_text_ranges_and_rejects_partial_expansions() {
        let snapshot = document("Straße STRASSE ςΣσ İ ﬃ").snapshot();
        for (pattern, expected) in [
            ("strasse", vec![(0, 7), (8, 15)]),
            ("σσσ", vec![(16, 22)]),
            ("i\u{307}", vec![(23, 25)]),
            ("ffi", vec![(26, 29)]),
            ("ff", vec![]),
        ] {
            let mut query = SearchQuery::literal(pattern);
            query.case = Case::Folded;
            let result = scan(&snapshot, &query, &SearchJob::default(), |_| {});
            assert_eq!(result.completeness(), Completeness::Complete);
            assert_eq!(
                result
                    .matches()
                    .iter()
                    .map(|m| (m.range.start.0, m.range.end.0))
                    .collect::<Vec<_>>(),
                expected,
                "{pattern}"
            );
        }
        let mut text = "x".repeat((16 << 20) - 4);
        text.push_str("Straße");
        text.push_str(&"x".repeat(4 << 20));
        let mut query = SearchQuery::literal("STRASSE");
        query.case = Case::Folded;
        let result = scan(&document(&text).snapshot(), &query, &SearchJob::default(), |_| {});
        assert_eq!(
            result.matches()[0].range,
            TextOffset((16 << 20) - 4)..TextOffset((16 << 20) + 3)
        );
    }
    #[test]
    fn whole_word_checks_combining_marks_connectors_and_selection_edges() {
        let text = "foo food _foo foo\u{301} foo-foo";
        let snapshot = document(text).snapshot();
        let mut query = SearchQuery::literal("foo");
        query.whole_word = true;
        let result = scan(&snapshot, &query, &SearchJob::default(), |_| {});
        assert_eq!(
            result.matches().iter().map(|m| m.range.start.0).collect::<Vec<_>>(),
            vec![0, 20, 24]
        );
        query.selection = Some(TextOffset(4)..TextOffset(7));
        assert_eq!(scan(&snapshot, &query, &SearchJob::default(), |_| {}).count(), 0);
    }
}

/// Explicit per-job replacement choices; matching semantics remain unchanged.
#[derive(Clone, Copy, Default)]
pub struct ReplacementOptions {
    pub preserve_case: bool,
    pub include_binary: bool,
}
/// Preserve uniform upper/lower case or initial capitalization; mixed case is literal.
pub fn preserve_replacement_case(original: &str, replacement: &str) -> String {
    let mut letters = original.chars().filter(|c| c.is_lowercase() || c.is_uppercase());
    let Some(first_letter) = letters.next() else {
        return replacement.into();
    };
    let mut all_upper = first_letter.is_uppercase();
    let mut all_lower = first_letter.is_lowercase();
    let mut rest_lower = true;
    for letter in letters {
        all_upper &= letter.is_uppercase();
        all_lower &= letter.is_lowercase();
        rest_lower &= letter.is_lowercase();
    }
    if all_upper {
        return replacement.to_uppercase();
    }
    if all_lower {
        return replacement.to_lowercase();
    }
    if first_letter.is_uppercase() && rest_lower {
        let mut first = true;
        return replacement
            .chars()
            .flat_map(|c| {
                if first && (c.is_uppercase() || c.is_lowercase()) {
                    first = false;
                    c.to_uppercase().collect::<Vec<_>>()
                } else {
                    c.to_lowercase().collect::<Vec<_>>()
                }
            })
            .collect();
    }
    replacement.into()
}

#[cfg(test)]
mod replacement_case_tests {
    use super::preserve_replacement_case;
    #[test]
    fn uniform_and_initial_case_preserve_unicode_without_changing_mixed_case() {
        assert_eq!(preserve_replacement_case("ABC", "straße"), "STRASSE");
        assert_eq!(preserve_replacement_case("abc", "DOG"), "dog");
        assert_eq!(preserve_replacement_case("Abc", "dOG"), "Dog");
        assert_eq!(preserve_replacement_case("aBc", "Dog"), "Dog");
        assert_eq!(preserve_replacement_case("123", "Dog"), "Dog");
    }
}
