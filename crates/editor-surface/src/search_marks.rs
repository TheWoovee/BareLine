// SPDX-License-Identifier: MPL-2.0
//! Five independent bounded view decorations; no document text or history mutation.
use crate::edit_walk::EditWalk;
use bareline_document::{EditTransaction, TextOffset, change::AppliedChange};
use std::ops::Range;
const MAX_MARKS: usize = 65536;
#[derive(Clone, Default)]
pub struct SearchMarks {
    /// Sorted by `(start, end)` within each style.
    styles: [Vec<Range<TextOffset>>; 5],
    /// Longest range per style, so a paint query can binary-search its window.
    longest: [usize; 5],
}
impl SearchMarks {
    pub fn set(&mut self, style: u8, mut ranges: Vec<Range<TextOffset>>) -> Result<(), String> {
        let index = style
            .checked_sub(1)
            .filter(|value| *value < 5)
            .ok_or("Mark style must be 1 through 5")? as usize;
        if ranges.iter().any(|range| range.start > range.end) {
            return Err("Invalid mark range".into());
        }
        if ranges.len()
            + self
                .styles
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != index)
                .map(|(_, marks)| marks.len())
                .sum::<usize>()
            > MAX_MARKS
        {
            return Err("Mark decoration limit reached".into());
        }
        ranges.sort_by_key(|range| (range.start, range.end));
        ranges.dedup();
        self.longest[index] = ranges
            .iter()
            .map(|range| range.end.0 - range.start.0)
            .max()
            .unwrap_or(0);
        self.styles[index] = ranges;
        Ok(())
    }
    pub fn clear(&mut self, style: Option<u8>) {
        match style {
            None => *self = Self::default(),
            Some(style) => {
                if let Some(index) = style.checked_sub(1).filter(|value| *value < 5) {
                    self.styles[index as usize].clear();
                    self.longest[index as usize] = 0;
                }
            }
        }
    }
    pub fn mapped(&self, transaction: &EditTransaction) -> Self {
        self.mapped_walk(EditWalk::new(transaction))
    }
    /// Map through a committed receipt, whatever produced it (typing, a
    /// prepared source transaction, undo or redo).
    pub fn mapped_change(&self, change: &AppliedChange) -> Self {
        self.mapped_walk(EditWalk::from_edits(
            change
                .edits()
                .iter()
                .map(|edit| (edit.before.clone(), edit.inserted_len)),
        ))
    }
    /// Unchanged marks shift by every edit before them; marks an edit touches are
    /// dropped. One merge walk per style over its sorted ranges and the sorted edits.
    fn mapped_walk(&self, mut walk: EditWalk) -> Self {
        let mut mapped = Self::default();
        for (style, ranges) in self.styles.iter().enumerate() {
            walk.restart();
            for range in ranges {
                let (shift, next) = walk.advance(|edit| edit.range.end <= range.start);
                if next.is_some_and(|edit| edit.range.start < range.end) {
                    continue;
                }
                let start = range.start.0 as i128 + shift;
                let end = range.end.0 as i128 + shift;
                if start >= 0 && end <= usize::MAX as i128 {
                    mapped.styles[style].push(TextOffset(start as usize)..TextOffset(end as usize));
                }
            }
            mapped.longest[style] = self.longest[style];
        }
        mapped
    }
    /// Marks that may intersect `start..end`, found by binary search rather than
    /// a scan of every mark.
    pub fn overlapping(&self, start: usize, end: usize) -> impl Iterator<Item = (u8, Range<TextOffset>)> + '_ {
        self.styles.iter().enumerate().flat_map(move |(index, ranges)| {
            let longest = self.longest[index];
            let first = ranges.partition_point(|range| range.start.0.saturating_add(longest) < start);
            let last = ranges.partition_point(|range| range.start.0 < end);
            ranges[first..last.max(first)]
                .iter()
                .filter(move |range| range.end.0 >= start)
                .cloned()
                .map(move |range| (index as u8 + 1, range))
        })
    }
    pub fn iter(&self) -> impl Iterator<Item = (u8, Range<TextOffset>)> + '_ {
        self.styles
            .iter()
            .enumerate()
            .flat_map(|(index, ranges)| ranges.iter().cloned().map(move |range| (index as u8 + 1, range)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Edit, Revision};
    #[test]
    fn independent_styles_map_only_unchanged_matches_and_clear_separately() {
        let mut marks = SearchMarks::default();
        for style in 1..=5 {
            marks.set(style, vec![TextOffset(10)..TextOffset(14)]).unwrap();
        }
        let transaction = EditTransaction {
            base_revision: Revision(0),
            edits: vec![Edit {
                range: TextOffset(0)..TextOffset(2),
                insert: "long".into(),
            }],
        };
        let mut moved = marks.mapped(&transaction);
        assert!(moved.iter().all(|(_, range)| range == (TextOffset(12)..TextOffset(16))));
        moved.clear(Some(3));
        assert_eq!(moved.iter().count(), 4);
        assert_eq!(marks.iter().count(), 5);
        moved.clear(None);
        assert_eq!(moved.iter().count(), 0);
    }
}
