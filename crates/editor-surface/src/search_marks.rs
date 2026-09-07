// SPDX-License-Identifier: MPL-2.0
//! Five independent bounded view decorations; no document text or history mutation.
use bareline_document::{EditTransaction, TextOffset};
use std::ops::Range;
const MAX_MARKS: usize = 65536;
#[derive(Clone, Default)]
pub struct SearchMarks { styles: [Vec<Range<TextOffset>>; 5] }
impl SearchMarks {
    pub fn set(&mut self, style: u8, mut ranges: Vec<Range<TextOffset>>) -> Result<(), String> {
        let index = style.checked_sub(1).filter(|value| *value < 5).ok_or("Mark style must be 1 through 5")? as usize;
        if ranges.iter().any(|range| range.start > range.end) { return Err("Invalid mark range".into()); }
        if ranges.len() + self.styles.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, marks)| marks.len()).sum::<usize>() > MAX_MARKS {
            return Err("Mark decoration limit reached".into());
        }
        ranges.sort_by_key(|range| (range.start, range.end));
        ranges.dedup();
        self.styles[index] = ranges;
        Ok(())
    }
    pub fn clear(&mut self, style: Option<u8>) {
        match style {
            None => self.styles.iter_mut().for_each(Vec::clear),
            Some(style) => if let Some(index) = style.checked_sub(1).filter(|value| *value < 5) { self.styles[index as usize].clear(); },
        }
    }
    pub fn mapped(&self, transaction: &EditTransaction) -> Self {
        let mut mapped = Self::default();
        for (style, ranges) in self.styles.iter().enumerate() {
            for range in ranges {
                let mut shift: i128 = 0;
                let mut valid = true;
                for edit in &transaction.edits {
                    if edit.range.end <= range.start {
                        shift += edit.insert.len() as i128 - (edit.range.end.0 - edit.range.start.0) as i128;
                    } else if edit.range.start < range.end {
                        valid = false;
                        break;
                    }
                }
                let start = range.start.0 as i128 + shift;
                let end = range.end.0 as i128 + shift;
                if valid && start >= 0 && end <= usize::MAX as i128 {
                    mapped.styles[style].push(TextOffset(start as usize)..TextOffset(end as usize));
                }
            }
        }
        mapped
    }
    pub fn iter(&self) -> impl Iterator<Item = (u8, Range<TextOffset>)> + '_ {
        self.styles.iter().enumerate().flat_map(|(index, ranges)| ranges.iter().cloned().map(move |range| (index as u8 + 1, range)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Edit, Revision};
    #[test]
    fn independent_styles_map_only_unchanged_matches_and_clear_separately() {
        let mut marks = SearchMarks::default();
        for style in 1..=5 { marks.set(style, vec![TextOffset(10)..TextOffset(14)]).unwrap(); }
        let transaction = EditTransaction { base_revision: Revision(0), edits: vec![Edit { range: TextOffset(0)..TextOffset(2), insert: "long".into() }] };
        let mut moved = marks.mapped(&transaction);
        assert!(moved.iter().all(|(_, range)| range == (TextOffset(12)..TextOffset(16))));
        moved.clear(Some(3));
        assert_eq!(moved.iter().count(), 4);
        assert_eq!(marks.iter().count(), 5);
        moved.clear(None);
        assert_eq!(moved.iter().count(), 0);
    }
}
