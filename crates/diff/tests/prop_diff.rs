// SPDX-License-Identifier: MPL-2.0
//! Seeded property tests (QA-05): applying an exact compare's hunks to one side yields
//! the other, hunks are ordered line-aligned ranges, and merges produce valid edits.
use bareline_diff::{
    ApplyError, CancelToken, CompareCompleteness, CompareOptions, DiffHunk, DiffKind, Direction, MergePolicy,
    ResourceLimits, Whitespace, apply_hunk, apply_hunk_with_policy, compare,
};
use bareline_document::{Budget, Document, DocumentSnapshot, EditTransaction, TextOffset};
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
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Few distinct lines, so unique anchors, repeated lines and Myers fallback all occur.
const LINES: &[&str] = &[
    "a\n",
    "b\n",
    "c\r\n",
    "d\r",
    "same\n",
    "same\n",
    "  x\ty \n",
    "X Y\n",
    "é\n",
    "e\u{301}\n",
    "\n",
    "\r\n",
    "last",
];

fn document(text: &str) -> Document {
    Document::from_utf8(text, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap()
}
fn slice(text: &str, range: &Range<TextOffset>) -> String {
    text[range.start.0..range.end.0].to_owned()
}
fn lines(rng: &mut Rng, max: usize) -> Vec<&'static str> {
    (0..rng.below(max + 1)).map(|_| *rng.pick(LINES)).collect()
}
/// Line-level insert/delete/replace plus occasional in-line edits of `base`.
fn mutate(rng: &mut Rng, base: &[&'static str]) -> String {
    let mut result: Vec<String> = base.iter().map(|line| (*line).to_owned()).collect();
    for _ in 0..rng.below(5) {
        let at = rng.below(result.len() + 1);
        match rng.below(4) {
            0 => result.insert(at, (*rng.pick(LINES)).to_owned()),
            1 if at < result.len() => {
                result.remove(at);
            }
            2 if at < result.len() => result[at] = (*rng.pick(LINES)).to_owned(),
            _ if at < result.len() => result[at].insert_str(0, rng.pick(&["Q", "é", " ", "\t"])),
            _ => {}
        }
    }
    result.concat()
}
fn line_boundaries(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut points = vec![0];
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i += 2;
            points.push(i);
        } else {
            if bytes[i] == b'\r' || bytes[i] == b'\n' {
                points.push(i + 1);
            }
            i += 1;
        }
    }
    points.push(text.len());
    points
}
fn exact_options() -> CompareOptions {
    CompareOptions {
        limits: ResourceLimits {
            // Wall-clock budgets would make the property nondeterministic.
            time_budget_ms: u64::MAX,
            ..ResourceLimits::default()
        },
        ..CompareOptions::default()
    }
}
/// Replace each hunk's `from` range with the other side's text, last hunk first.
fn apply_script(hunks: &[DiffHunk], from: &str, to: &str, left_to_right: bool) -> String {
    let mut output = from.to_owned();
    for hunk in hunks.iter().rev() {
        let (target, source) = if left_to_right {
            (&hunk.left, &hunk.right)
        } else {
            (&hunk.right, &hunk.left)
        };
        output.replace_range(target.start.0..target.end.0, &slice(to, source));
    }
    output
}
fn check_hunk_shape(hunks: &[DiffHunk], left: &str, right: &str, context: &str) {
    let (left_lines, right_lines) = (line_boundaries(left), line_boundaries(right));
    for (index, hunk) in hunks.iter().enumerate() {
        for (range, points, text) in [(&hunk.left, &left_lines, left), (&hunk.right, &right_lines, right)] {
            assert!(
                range.start <= range.end && range.end.0 <= text.len(),
                "{context}: hunk {index} {range:?}"
            );
            assert!(
                points.contains(&range.start.0) && points.contains(&range.end.0),
                "{context}: hunk {index} is not line aligned: {range:?}"
            );
        }
        let (left_empty, right_empty) = (hunk.left.is_empty(), hunk.right.is_empty());
        let consistent = match hunk.kind {
            DiffKind::Added => left_empty && !right_empty,
            DiffKind::Removed => right_empty && !left_empty,
            DiffKind::Changed => !left_empty && !right_empty,
            DiffKind::MovedAligned => left_empty != right_empty,
            DiffKind::Equal => false,
        };
        assert!(consistent, "{context}: hunk {index} kind {:?}", hunk.kind);
        for span in &hunk.intraline {
            for (inner, outer) in [(&span.left, &hunk.left), (&span.right, &hunk.right)] {
                let bounds = outer.start..=outer.end;
                assert!(
                    inner.start <= inner.end && bounds.contains(&inner.start) && bounds.contains(&inner.end),
                    "{context}: hunk {index} intraline {span:?} escapes its hunk"
                );
            }
        }
        if let Some(next) = hunks.get(index + 1) {
            assert!(
                hunk.left.end <= next.left.start && hunk.right.end <= next.right.start,
                "{context}: hunks {index} and {} overlap or are unordered",
                index + 1
            );
        }
    }
}
fn read(snapshot: &DocumentSnapshot) -> String {
    snapshot
        .read(TextOffset(0)..TextOffset(snapshot.len()), usize::MAX)
        .unwrap()
}

#[test]
fn exact_hunks_are_an_edit_script_in_both_directions() {
    let mut rng = Rng(0xd1ff_0001);
    let options = exact_options();
    for case in 0..400 {
        let base = lines(&mut rng, 24);
        let left = base.concat();
        let right = if rng.chance(15) {
            lines(&mut rng, 24).concat()
        } else {
            mutate(&mut rng, &base)
        };
        let context = format!("case {case} left={left:?} right={right:?}");
        let (mut left_document, mut right_document) = (document(&left), document(&right));
        let (l, r) = (left_document.snapshot(), right_document.snapshot());
        let result = compare(&l, &r, &options, &CancelToken::default());
        assert_eq!(result.completeness, CompareCompleteness::Exact, "{context}");
        assert_eq!(
            result.hunks.is_empty(),
            left == right,
            "{context}: empty diff iff equal"
        );
        check_hunk_shape(&result.hunks, &left, &right, &context);
        assert_eq!(
            apply_script(&result.hunks, &left, &right, true),
            right,
            "{context}: A -> B"
        );
        assert_eq!(
            apply_script(&result.hunks, &right, &left, false),
            left,
            "{context}: B -> A"
        );
        // apply_hunk produces exactly the script's edits against the captured states.
        let mut to_right = Vec::new();
        let mut to_left = Vec::new();
        for hunk in &result.hunks {
            assert!(hunk.matches_states(l.revision, l.content_state, r.revision, r.content_state));
            let mut forward = apply_hunk(Direction::LeftToRight, hunk, &l, &r, usize::MAX).unwrap();
            let mut backward = apply_hunk(Direction::RightToLeft, hunk, &l, &r, usize::MAX).unwrap();
            assert_eq!(
                (forward.base_revision, backward.base_revision),
                (r.revision, l.revision)
            );
            assert_eq!((forward.edits.len(), backward.edits.len()), (1, 1), "{context}");
            let (forward, backward) = (forward.edits.remove(0), backward.edits.remove(0));
            assert_eq!(
                (&forward.range, &forward.insert),
                (&hunk.right, &slice(&left, &hunk.left)),
                "{context}"
            );
            assert_eq!(
                (&backward.range, &backward.insert),
                (&hunk.left, &slice(&right, &hunk.right)),
                "{context}"
            );
            to_right.push(forward);
            to_left.push(backward);
            let budget = hunk.left.end.0 - hunk.left.start.0;
            if budget > 0 {
                assert_eq!(
                    apply_hunk(Direction::LeftToRight, hunk, &l, &r, budget - 1).map(|_| ()),
                    Err(ApplyError::BudgetExceeded),
                    "{context}"
                );
            }
        }
        // All hunks commit as one multi-edit transaction on each document.
        if !to_left.is_empty() {
            left_document
                .apply(EditTransaction {
                    base_revision: l.revision,
                    edits: to_left,
                })
                .unwrap_or_else(|error| panic!("{context}: merge into left failed: {error:?}"));
            right_document
                .apply(EditTransaction {
                    base_revision: r.revision,
                    edits: to_right,
                })
                .unwrap_or_else(|error| panic!("{context}: merge into right failed: {error:?}"));
        }
        assert_eq!(read(&left_document.snapshot()), right, "{context}");
        assert_eq!(read(&right_document.snapshot()), left, "{context}");
        // Hunks are bound to the captured content states.
        if let Some(hunk) = result.hunks.first() {
            assert_eq!(
                apply_hunk(Direction::LeftToRight, hunk, &left_document.snapshot(), &r, usize::MAX).map(|_| ()),
                Err(ApplyError::Stale),
                "{context}"
            );
        }
        let identical = compare(&l, &l, &options, &CancelToken::default());
        assert!(identical.hunks.is_empty(), "{context}: self compare");
    }
}

#[test]
fn ignore_options_keep_hunks_ordered_and_merges_applicable() {
    let mut rng = Rng(0xd1ff_0002);
    for case in 0..300 {
        let base = lines(&mut rng, 16);
        let left = base.concat();
        let right = mutate(&mut rng, &base);
        let mut options = exact_options();
        options.whitespace = *rng.pick(&[Whitespace::Significant, Whitespace::TrimEdges, Whitespace::IgnoreAll]);
        options.ignore_blank_lines = rng.chance(50);
        options.ignore_case = rng.chance(50);
        options.ignore_eol_style = rng.chance(50);
        options.ignore_encoding_bom = rng.chance(50);
        options.normalize_tabs = rng.chance(50);
        let context = format!("case {case} left={left:?} right={right:?}");
        let (l, r) = (document(&left).snapshot(), document(&right).snapshot());
        let result = compare(&l, &r, &options, &CancelToken::default());
        assert_eq!(result.completeness, CompareCompleteness::Exact, "{context}");
        check_hunk_shape(&result.hunks, &left, &right, &context);
        for hunk in &result.hunks {
            for (direction, target, target_text) in [
                (Direction::LeftToRight, &r, &right),
                (Direction::RightToLeft, &l, &left),
            ] {
                for policy in [MergePolicy::PreserveIgnoredDestination, MergePolicy::CopySelectedRange] {
                    match apply_hunk_with_policy(policy, direction, hunk, &l, &r, 1 << 20) {
                        Ok(transaction) => {
                            let mut merged =
                                Document::fork_from_snapshot(target, Budget::new(1 << 20), Budget::new(1 << 20))
                                    .unwrap();
                            assert_eq!(read(&merged.snapshot()), *target_text);
                            let transaction = EditTransaction {
                                base_revision: merged.snapshot().revision,
                                edits: transaction.edits,
                            };
                            merged
                                .apply(transaction)
                                .unwrap_or_else(|error| panic!("{context}: {policy:?} merge rejected: {error:?}"));
                        }
                        Err(ApplyError::UnsupportedPreserve | ApplyError::BudgetExceeded) => {}
                        Err(error) => panic!("{context}: {policy:?} {direction:?} failed: {error:?}"),
                    }
                }
            }
        }
    }
}
