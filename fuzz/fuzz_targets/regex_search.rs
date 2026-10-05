// SPDX-License-Identifier: MPL-2.0
//! PCRE2 compile and match through the public search API. Input: [flags] pattern NUL
//! haystack NUL replacement. Matches are ordered, UTF-8 aligned and in bounds, and a
//! complete Replace All transaction applies to the searched document.
#![no_main]
use bareline_document::{Budget, Document, TextOffset};
use bareline_search::{Case, Completeness, SearchJob, SearchMode, SearchQuery, decode_replacement, scan};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    let mut parts = rest.splitn(3, |byte| *byte == 0);
    let pattern = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
    let haystack = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
    let replacement = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
    let mut document = Document::from_utf8(&haystack, Budget::new(1 << 24), Budget::new(1 << 24)).unwrap();
    let snapshot = document.snapshot();
    let mut query = SearchQuery::literal(pattern);
    query.mode = [
        SearchMode::Regex,
        SearchMode::Regex,
        SearchMode::Extended,
        SearchMode::Literal,
    ][usize::from(flags & 3)];
    query.case = if flags & 0x04 != 0 {
        Case::Folded
    } else {
        Case::Sensitive
    };
    query.whole_word = flags & 0x08 != 0;
    query.dot_matches_newline = flags & 0x10 != 0;
    query.results_ram_bytes = 1 << 20;
    if flags & 0x20 != 0 {
        // Bounded selection on scalar boundaries.
        let mut end = haystack.len() / 2;
        while !haystack.is_char_boundary(end) {
            end -= 1;
        }
        query.selection = Some(TextOffset(0)..TextOffset(end));
    }
    let results = scan(&snapshot, &query, &SearchJob::default(), |_| {});
    let limit = query
        .selection
        .as_ref()
        .map_or(haystack.len(), |selection| selection.end.0);
    let mut previous: Option<&std::ops::Range<TextOffset>> = None;
    for found in results.matches() {
        let range = &found.range;
        assert!(range.start <= range.end && range.end.0 <= limit, "match out of bounds");
        assert!(haystack.is_char_boundary(range.start.0) && haystack.is_char_boundary(range.end.0));
        if let Some(previous) = previous {
            assert!(
                previous.start < range.start && previous.end <= range.start,
                "unordered matches"
            );
        }
        previous = Some(range);
    }
    if results.completeness() != Completeness::Complete || results.is_empty() {
        return;
    }
    let Ok(replacement) = decode_replacement(&replacement, query.mode) else {
        return;
    };
    if let Ok(transaction) = results.prepare_replace(&snapshot, &replacement, 1 << 22) {
        document
            .apply(transaction)
            .expect("Replace All transaction must apply to the searched snapshot");
    }
});
