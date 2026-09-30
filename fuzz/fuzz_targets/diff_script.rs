// SPDX-License-Identifier: MPL-2.0
//! Compare two documents (input split at the first NUL). Exact and coarse hunks are an
//! edit script from left to right; with the ignore options selected by the first byte,
//! hunks stay ordered and every merge transaction applies to its target.
#![no_main]
use bareline_diff::{
    ApplyError, CancelToken, CompareCompleteness, CompareOptions, Direction, MergePolicy, ResourceLimits, Whitespace,
    apply_hunk_with_policy, compare,
};
use bareline_document::{Budget, Document, EditTransaction};
use libfuzzer_sys::fuzz_target;

fn document(text: &str) -> Document {
    Document::from_utf8(text, Budget::new(1 << 24), Budget::new(1 << 24)).unwrap()
}

fuzz_target!(|data: &[u8]| {
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    let split = rest.iter().position(|byte| *byte == 0).unwrap_or(rest.len());
    let left = String::from_utf8_lossy(&rest[..split]).into_owned();
    let right = String::from_utf8_lossy(rest.get(split + 1..).unwrap_or_default()).into_owned();
    let (l, r) = (document(&left).snapshot(), document(&right).snapshot());
    let limits = ResourceLimits {
        // Wall-clock budgets would make findings irreproducible.
        time_budget_ms: u64::MAX,
        ..ResourceLimits::default()
    };
    let exact = CompareOptions {
        limits: limits.clone(),
        ..CompareOptions::default()
    };
    let result = compare(&l, &r, &exact, &CancelToken::default());
    assert!(matches!(
        result.completeness,
        CompareCompleteness::Exact | CompareCompleteness::Coarse(_)
    ));
    let mut script = left.clone();
    for hunk in result.hunks.iter().rev() {
        script.replace_range(
            hunk.left.start.0..hunk.left.end.0,
            &right[hunk.right.start.0..hunk.right.end.0],
        );
    }
    assert_eq!(script, right, "hunks are not an edit script");

    let options = CompareOptions {
        whitespace: [Whitespace::Significant, Whitespace::TrimEdges, Whitespace::IgnoreAll][usize::from(flags % 3)],
        ignore_blank_lines: flags & 0x04 != 0,
        ignore_case: flags & 0x08 != 0,
        ignore_eol_style: flags & 0x10 != 0,
        ignore_encoding_bom: flags & 0x20 != 0,
        normalize_tabs: flags & 0x40 != 0,
        limits,
    };
    let result = compare(&l, &r, &options, &CancelToken::default());
    for pair in result.hunks.windows(2) {
        assert!(pair[0].left.end <= pair[1].left.start && pair[0].right.end <= pair[1].right.start);
    }
    let policy = if flags & 0x80 != 0 {
        MergePolicy::CopySelectedRange
    } else {
        MergePolicy::PreserveIgnoredDestination
    };
    for hunk in result.hunks.iter().take(8) {
        for (direction, target) in [(Direction::LeftToRight, &r), (Direction::RightToLeft, &l)] {
            match apply_hunk_with_policy(policy, direction, hunk, &l, &r, 1 << 20) {
                Ok(transaction) => {
                    let mut merged = Document::fork_from_snapshot(target, Budget::new(1 << 24), Budget::new(1 << 24))
                        .expect("complete snapshot");
                    let base_revision = merged.snapshot().revision;
                    merged
                        .apply(EditTransaction {
                            base_revision,
                            edits: transaction.edits,
                        })
                        .expect("merge transaction must apply to its target");
                }
                Err(ApplyError::UnsupportedPreserve | ApplyError::BudgetExceeded) => {}
                Err(error) => panic!("{error:?}"),
            }
        }
    }
});
