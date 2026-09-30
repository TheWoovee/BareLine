// SPDX-License-Identifier: MPL-2.0
//! Merge-walk remapping of view anchors through one committed transaction.
//!
//! Anchors are visited in nondecreasing offset order and each edit is passed at
//! most once, so remapping `n` anchors through `m` edits costs O(n + m) after
//! sorting instead of O(n × m).
use bareline_document::{EditTransaction, TextOffset};
use std::ops::Range;

#[cfg(test)]
thread_local! {
    /// Anchors queried plus edits passed on this thread; a deterministic cost
    /// measure for complexity tests.
    pub(crate) static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One edit in before-coordinates: the replaced range and the inserted length.
/// Built from an [`EditTransaction`] or from a committed `AppliedChange` receipt.
pub(crate) struct WalkEdit {
    pub(crate) range: Range<TextOffset>,
    pub(crate) inserted: usize,
}
pub(crate) struct EditWalk {
    edits: Vec<WalkEdit>,
    next: usize,
    shift: i128,
}
impl EditWalk {
    pub(crate) fn new(transaction: &EditTransaction) -> Self {
        Self::from_edits(
            transaction
                .edits
                .iter()
                .map(|edit| (edit.range.clone(), edit.insert.len())),
        )
    }
    /// Walks edits given as `(replaced range, inserted length)` pairs in
    /// before-coordinates, such as the compact edits of an `AppliedChange`.
    pub(crate) fn from_edits(edits: impl IntoIterator<Item = (Range<TextOffset>, usize)>) -> Self {
        // Committed edits are sorted and non-overlapping, so edit ends are
        // nondecreasing too; sorting an already sorted list is linear.
        let mut edits: Vec<_> = edits
            .into_iter()
            .map(|(range, inserted)| WalkEdit { range, inserted })
            .collect();
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        Self {
            edits,
            next: 0,
            shift: 0,
        }
    }
    /// Rewinds to the first edit so another sorted anchor list can be walked
    /// without collecting and sorting the edits again.
    pub(crate) fn restart(&mut self) {
        self.next = 0;
        self.shift = 0;
    }
    /// Passes every remaining edit for which `before` holds and returns the
    /// accumulated length delta with the first edit not passed. Callers query
    /// nondecreasing offsets with a predicate that is monotone in the offset.
    pub(crate) fn advance(&mut self, before: impl Fn(&WalkEdit) -> bool) -> (i128, Option<&WalkEdit>) {
        #[cfg(test)]
        STEPS.with(|steps| steps.set(steps.get() + 1));
        while let Some(edit) = self.edits.get(self.next) {
            if !before(edit) {
                break;
            }
            self.shift += edit.inserted as i128 - (edit.range.end.0 - edit.range.start.0) as i128;
            self.next += 1;
            #[cfg(test)]
            STEPS.with(|steps| steps.set(steps.get() + 1));
        }
        (self.shift, self.edits.get(self.next))
    }
    /// Edits passed since the last restart.
    pub(crate) fn passed(&self) -> usize {
        self.next
    }
}
/// Applies a signed delta, saturating at zero like the previous per-item remaps.
pub(crate) fn shifted(offset: usize, shift: i128) -> usize {
    usize::try_from((offset as i128 + shift).max(0)).unwrap_or(usize::MAX)
}
/// Maps offsets with fold-anchor semantics: `None` when an edit strictly contains
/// the offset, and an insertion exactly at the offset moves it only when `right`
/// is set. Offsets may arrive in any order.
pub(crate) fn map_offsets(transaction: &EditTransaction, offsets: &[usize], right: bool) -> Vec<Option<usize>> {
    let mut order: Vec<usize> = (0..offsets.len()).collect();
    order.sort_by_key(|&index| offsets[index]);
    let mut walk = EditWalk::new(transaction);
    let mut mapped = vec![None; offsets.len()];
    for index in order {
        let offset = offsets[index];
        let (shift, next) = walk.advance(|edit| {
            edit.range.end.0 < offset || (edit.range.end.0 == offset && (!edit.range.is_empty() || right))
        });
        if next.is_some_and(|edit| edit.range.start.0 < offset && offset < edit.range.end.0) {
            continue;
        }
        mapped[index] = usize::try_from(offset as i128 + shift).ok();
    }
    mapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Edit, Revision};
    #[test]
    fn fold_semantics_match_per_item_mapping() {
        let transaction = EditTransaction {
            base_revision: Revision(0),
            edits: vec![
                Edit {
                    range: TextOffset(10)..TextOffset(10),
                    insert: "abc".into(),
                },
                Edit {
                    range: TextOffset(2)..TextOffset(5),
                    insert: String::new(),
                },
            ],
        };
        // Unsorted input; 3 is strictly inside the deletion; 10 is the insertion point.
        let offsets = [10, 3, 0, 5, 20];
        assert_eq!(
            map_offsets(&transaction, &offsets, true),
            vec![Some(10), None, Some(0), Some(2), Some(20)]
        );
        assert_eq!(
            map_offsets(&transaction, &offsets, false),
            vec![Some(7), None, Some(0), Some(2), Some(20)]
        );
    }
    #[test]
    fn marks_bookmarks_and_folds_remap_in_one_pass_over_many_edits() {
        let (items, edits) = (20_000usize, 5_000usize);
        // Two-byte insertions at 40j + 5 never touch an item at 10i..10i + 2.
        let transaction = EditTransaction {
            base_revision: Revision(0),
            edits: (0..edits)
                .map(|j| Edit {
                    range: TextOffset(40 * j + 5)..TextOffset(40 * j + 5),
                    insert: "ab".into(),
                })
                .collect(),
        };
        let steps = |run: &mut dyn FnMut()| {
            STEPS.with(|steps| steps.set(0));
            run();
            STEPS.with(|steps| steps.get())
        };
        // A per-item scan of every edit would take items × edits = 10^8 steps.
        let bound = items + edits;

        let mut marks = crate::search_marks::SearchMarks::default();
        marks
            .set(
                1,
                (0..items).map(|i| TextOffset(10 * i)..TextOffset(10 * i + 2)).collect(),
            )
            .unwrap();
        let mut mapped = None;
        assert!(steps(&mut || mapped = Some(marks.mapped(&transaction))) <= bound);
        let mapped = mapped.unwrap();
        assert_eq!(mapped.iter().count(), items);
        assert!(
            mapped
                .iter()
                .any(|(_, range)| range == (TextOffset(42)..TextOffset(44)))
        );

        let mut bookmarks = crate::power::Bookmarks {
            anchors: (0..items).map(|i| 10 * i).collect(),
        };
        assert!(steps(&mut || bookmarks.map_edits(&transaction)) <= bound);
        assert_eq!(bookmarks.anchors.len(), items);
        assert!(bookmarks.anchors.contains(&42));

        let starts: Vec<usize> = (0..items).map(|i| 10 * i).collect();
        let mut folded = Vec::new();
        assert!(steps(&mut || folded = map_offsets(&transaction, &starts, true)) <= bound);
        assert_eq!(folded[4], Some(42));
        assert!(folded.iter().all(Option::is_some));
    }
}
