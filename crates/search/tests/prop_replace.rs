// SPDX-License-Identifier: MPL-2.0
//! Seeded property tests (QA-16): literal search results and the Replace All /
//! Replace One transactions prepared from them, applied to a `Document`, match a
//! `String` oracle; stale, cancelled and empty results never produce edits.
use bareline_document::{Budget, Document, DocumentSnapshot, TextOffset};
use bareline_search::{Case, Completeness, ReplaceError, ReplaceScope, SearchJob, SearchQuery, scan};
use std::ops::Range;

/// SplitMix64. Report the seed and case of a failure to reproduce it exactly.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }
    fn text(&mut self, max: usize) -> String {
        (0..self.below(max + 1))
            .map(|_| PIECES[self.below(PIECES.len())])
            .collect()
    }
}

/// Only ASCII letters change under case folding here, so `to_ascii_lowercase` is an
/// exact, offset-preserving oracle for `Case::Folded`.
const PIECES: &[&str] = &["a", "b", "A", "B", "ab", "aa", " ", "\n", "\r\n", "é", "中", "👩🏽‍💻"];

fn read_all(snapshot: &DocumentSnapshot) -> String {
    snapshot
        .read(TextOffset(0)..TextOffset(snapshot.len()), usize::MAX)
        .unwrap()
}
fn boundaries(text: &str) -> Vec<usize> {
    text.char_indices().map(|(i, _)| i).chain(Some(text.len())).collect()
}
/// Leftmost, non-overlapping matches inside `selection`, as `str::replace` finds them.
fn oracle_matches(text: &str, pattern: &str, folded: bool, selection: Range<usize>) -> Vec<Range<usize>> {
    let (haystack, needle) = if folded {
        (text.to_ascii_lowercase(), pattern.to_ascii_lowercase())
    } else {
        (text.to_owned(), pattern.to_owned())
    };
    haystack[selection.clone()]
        .match_indices(&needle)
        .map(|(start, found)| selection.start + start..selection.start + start + found.len())
        .collect()
}
fn replaced(text: &str, ranges: &[Range<usize>], replacement: &str) -> String {
    let mut result = String::new();
    let mut cursor = 0;
    for range in ranges {
        result.push_str(&text[cursor..range.start]);
        result.push_str(replacement);
        cursor = range.end;
    }
    result.push_str(&text[cursor..]);
    result
}

#[test]
fn literal_replace_matches_string_oracle() {
    for seed in [0x5e_a2c4_0001u64, 0x5e_a2c4_0002, 0x5e_a2c4_0003] {
        let mut rng = Rng(seed);
        for case in 0..60 {
            let context = format!("seed {seed:#x} case {case}");
            let text = rng.text(40);
            let points = boundaries(&text);
            // Half the patterns are cut from the text so most cases have matches.
            let pattern = if rng.chance(50) && points.len() > 1 {
                let start = rng.below(points.len() - 1);
                let end = start + 1 + rng.below((points.len() - start - 1).min(3));
                text[points[start]..points[end]].to_owned()
            } else {
                let mut pattern = rng.text(2);
                if pattern.is_empty() {
                    pattern.push('a');
                }
                pattern
            };
            let replacement = rng.text(3);
            let folded = rng.chance(30);
            let selection = if rng.chance(30) {
                let a = points[rng.below(points.len())];
                let b = points[rng.below(points.len())];
                a.min(b)..a.max(b)
            } else {
                0..text.len()
            };
            let mut query = SearchQuery::literal(pattern.clone());
            if folded {
                query.case = Case::Folded;
            }
            if selection != (0..text.len()) {
                query.selection = Some(TextOffset(selection.start)..TextOffset(selection.end));
            }

            let mut document = Document::from_utf8(&text, Budget::new(1 << 24), Budget::new(1 << 24)).unwrap();
            let snapshot = document.snapshot();
            let results = scan(&snapshot, &query, &SearchJob::default(), |_| {});
            let expected = oracle_matches(&text, &pattern, folded, selection);
            assert_eq!(results.completeness(), Completeness::Complete, "{context}");
            let found: Vec<Range<usize>> = results
                .matches()
                .iter()
                .map(|m| m.range.start.0..m.range.end.0)
                .collect();
            assert_eq!(found, expected, "{context}: {pattern:?} in {text:?}");
            assert_eq!(results.count(), expected.len(), "{context}");
            if expected.is_empty() {
                assert!(
                    matches!(
                        results.prepare_replace(&snapshot, &replacement, usize::MAX),
                        Err(ReplaceError::NoMatch)
                    ),
                    "{context}: empty results prepared edits"
                );
                continue;
            }

            let cancelled = SearchJob::default();
            cancelled.cancel();
            assert!(
                matches!(
                    results.prepare_replace_scoped(&snapshot, &replacement, usize::MAX, ReplaceScope::All, &cancelled),
                    Err(ReplaceError::Cancelled)
                ),
                "{context}: cancelled job prepared edits"
            );
            let (scope, selected) = if rng.chance(50) {
                (ReplaceScope::All, expected.clone())
            } else {
                let one = expected[rng.below(expected.len())].clone();
                (ReplaceScope::One(TextOffset(one.start)..TextOffset(one.end)), vec![one])
            };
            let transaction = results
                .prepare_replace_scoped(&snapshot, &replacement, usize::MAX, scope, &SearchJob::default())
                .unwrap();
            assert_eq!(transaction.base_revision, snapshot.revision, "{context}");
            assert_eq!(transaction.edits.len(), selected.len(), "{context}");
            document.apply(transaction).unwrap();
            let after = document.snapshot();
            assert_eq!(
                read_all(&after),
                replaced(&text, &selected, &replacement),
                "{context}: replace {pattern:?} with {replacement:?} in {text:?}"
            );
            // The results belong to the old revision and must never apply again.
            assert!(!results.is_current(&after), "{context}");
            assert!(
                matches!(
                    results.prepare_replace(&after, &replacement, usize::MAX),
                    Err(ReplaceError::Stale)
                ),
                "{context}: stale results prepared edits"
            );
        }
    }
}
