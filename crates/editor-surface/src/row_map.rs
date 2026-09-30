// SPDX-License-Identifier: MPL-2.0
//! Visual rows of logical lines: hidden lines, view spacers and measured wrap
//! rows. Each keeps prefix sums, so the first row of a line costs three binary
//! searches instead of a scan of every hidden range per spacer and per wrapped
//! line; with Fold All and word wrap that scan ran billions of steps per frame
//! (EDT-19).
use std::{
    collections::BTreeMap,
    ops::{Range, RangeInclusive},
};

#[cfg(test)]
thread_local! {
    /// Prefix-sum entries compared on this thread; a deterministic cost measure
    /// for complexity tests.
    pub(crate) static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn step() {
    STEPS.with(|steps| steps.set(steps.get() + 1));
}

/// Measured rows of one wrapped line and the content bytes they were measured
/// on, which an edit that leaves those bytes alone carries to the line's new
/// number (EDT-05).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WrapRows {
    pub(crate) rows: usize,
    pub(crate) content: Range<usize>,
}
#[derive(Clone, Default)]
pub(crate) struct RowMap {
    /// Sorted, disjoint and non-adjacent hidden line ranges.
    hidden: Vec<RangeInclusive<usize>>,
    /// Lines hidden by `hidden[..=i]`.
    hidden_through: Vec<usize>,
    /// Sorted, unique `(line, rows)`: view-only rows shown before `line`.
    spacers: Vec<(usize, usize)>,
    /// `(line, rows)` of the spacers before visible lines, rows accumulated
    /// through each entry.
    spacer_rows: Vec<(usize, usize)>,
    wrap: BTreeMap<usize, WrapRows>,
    /// `(line, rows)` of the visible wrapped lines taller than one row, extra
    /// rows accumulated through each entry. Bounded like `wrap`, which drawing
    /// trims to about `MAX_LAYOUTS` lines, so one measurement updates it in
    /// place instead of rebuilding it.
    wrap_rows: Vec<(usize, usize)>,
}
/// The accumulated value of the last entry `before` accepts, or zero.
fn prefix(sums: &[(usize, usize)], before: impl Fn(usize) -> bool) -> usize {
    let count = sums.partition_point(|&(line, _)| {
        #[cfg(test)]
        step();
        before(line)
    });
    count.checked_sub(1).map_or(0, |last| sums[last].1)
}
impl RowMap {
    pub(crate) fn hidden(&self) -> &[RangeInclusive<usize>] {
        &self.hidden
    }
    /// Installs sorted, merged hidden ranges, as `refresh_hidden_lines` builds them.
    pub(crate) fn set_hidden(&mut self, hidden: Vec<RangeInclusive<usize>>) {
        let mut total = 0usize;
        self.hidden_through = hidden
            .iter()
            .map(|range| {
                total = total.saturating_add(range.end().saturating_sub(*range.start()).saturating_add(1));
                total
            })
            .collect();
        self.hidden = hidden;
        self.rebuild_spacers();
        self.rebuild_wrap();
    }
    fn hidden_index(&self, line: usize) -> Option<usize> {
        self.hidden
            .partition_point(|range| {
                #[cfg(test)]
                step();
                *range.start() <= line
            })
            .checked_sub(1)
    }
    pub(crate) fn is_hidden(&self, line: usize) -> bool {
        self.hidden_index(line)
            .is_some_and(|last| line <= *self.hidden[last].end())
    }
    /// Hidden lines at or before `line`.
    fn hidden_through(&self, line: usize) -> usize {
        let Some(last) = self.hidden_index(line) else {
            return 0;
        };
        let range = &self.hidden[last];
        let before = last.checked_sub(1).map_or(0, |index| self.hidden_through[index]);
        before.saturating_add(line.min(*range.end()) - range.start() + 1)
    }
    pub(crate) fn spacers(&self) -> &[(usize, usize)] {
        &self.spacers
    }
    /// Installs sorted, unique spacers, as `set_view_spacers` validates them.
    pub(crate) fn set_spacers(&mut self, spacers: Vec<(usize, usize)>) {
        self.spacers = spacers;
        self.rebuild_spacers();
    }
    fn rebuild_spacers(&mut self) {
        let mut total = 0usize;
        self.spacer_rows = self
            .spacers
            .iter()
            .filter(|(line, _)| !self.is_hidden(*line))
            .map(|&(line, rows)| {
                total = total.saturating_add(rows);
                (line, total)
            })
            .collect();
    }
    pub(crate) fn wrap_rows(&self, line: usize) -> Option<usize> {
        self.wrap.get(&line).map(|measured| measured.rows)
    }
    pub(crate) fn wrap_len(&self) -> usize {
        self.wrap.len()
    }
    pub(crate) fn wrap_entries(&self) -> impl Iterator<Item = (usize, &WrapRows)> {
        self.wrap.iter().map(|(line, measured)| (*line, measured))
    }
    /// Records the rows `line` measured over `content`; true when its row count changed.
    pub(crate) fn set_wrap_rows(&mut self, line: usize, rows: usize, content: Range<usize>) -> bool {
        let previous = self
            .wrap
            .insert(line, WrapRows { rows, content })
            .map(|measured| measured.rows);
        if previous == Some(rows) {
            return false;
        }
        if !self.is_hidden(line) {
            self.shift_wrap(line, rows.saturating_sub(1));
        }
        true
    }
    /// Sets the extra rows of visible `line` in the accumulated sums.
    fn shift_wrap(&mut self, line: usize, extra: usize) {
        let index = self.wrap_rows.partition_point(|&(wrapped, _)| wrapped < line);
        let before = index.checked_sub(1).map_or(0, |last| self.wrap_rows[last].1);
        let old = match self.wrap_rows.get(index) {
            Some(&(wrapped, total)) if wrapped == line => total - before,
            _ if extra == 0 => return,
            _ => {
                self.wrap_rows.insert(index, (line, before));
                0
            }
        };
        for entry in &mut self.wrap_rows[index..] {
            entry.1 = (entry.1 - old).saturating_add(extra);
        }
        if extra == 0 {
            self.wrap_rows.remove(index);
        }
    }
    pub(crate) fn clear_wrap(&mut self) {
        self.wrap.clear();
        self.wrap_rows.clear();
    }
    pub(crate) fn retain_wrap(&mut self, keep: impl Fn(usize) -> bool) {
        self.wrap.retain(|line, _| keep(*line));
        self.rebuild_wrap();
    }
    /// Removes every measurement, for a caller that maps them through an edit
    /// and installs the result with `replace_wrap`.
    pub(crate) fn take_wrap(&mut self) -> BTreeMap<usize, WrapRows> {
        self.wrap_rows.clear();
        std::mem::take(&mut self.wrap)
    }
    pub(crate) fn replace_wrap(&mut self, wrap: BTreeMap<usize, WrapRows>) {
        self.wrap = wrap;
        self.rebuild_wrap();
    }
    fn rebuild_wrap(&mut self) {
        let mut total = 0usize;
        self.wrap_rows = self
            .wrap
            .iter()
            .filter(|(line, measured)| measured.rows > 1 && !self.is_hidden(**line))
            .map(|(line, measured)| {
                total = total.saturating_add(measured.rows - 1);
                (*line, total)
            })
            .collect();
    }
    /// First visual row of `line`: lines before it that are not hidden, spacer
    /// rows up to it and, with word wrap, the extra rows of wrapped lines before it.
    pub(crate) fn visual_line(&self, line: usize, wrap: bool) -> usize {
        let spacers = prefix(&self.spacer_rows, |before| before <= line);
        let wrapped = if wrap {
            prefix(&self.wrap_rows, |wrapped| wrapped < line)
        } else {
            0
        };
        line.saturating_sub(self.hidden_through(line))
            .saturating_add(spacers)
            .saturating_add(wrapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The per-call scans `visual_line` replaced.
    fn scanned(rows: &RowMap, line: usize) -> usize {
        let hidden: usize = rows
            .hidden
            .iter()
            .map(|r| {
                if line < *r.start() {
                    0
                } else {
                    line.min(*r.end()) - r.start() + 1
                }
            })
            .sum();
        let spacers: usize = rows
            .spacers
            .iter()
            .filter(|(before, _)| *before <= line && !rows.hidden.iter().any(|range| range.contains(before)))
            .map(|(_, count)| count)
            .sum();
        let wrapped: usize = rows
            .wrap
            .range(..line)
            .filter(|(line, _)| !rows.hidden.iter().any(|range| range.contains(line)))
            .map(|(_, measured)| measured.rows.saturating_sub(1))
            .sum();
        line - hidden + spacers + wrapped
    }
    #[test]
    fn prefix_sums_match_the_scans_through_measurements_and_folds() {
        let mut rows = RowMap::default();
        rows.set_spacers(vec![(0, 2), (5, 1), (12, 3), (40, 1)]);
        for line in (0..60).step_by(3) {
            rows.set_wrap_rows(line, 1 + line % 4, 0..0);
        }
        rows.set_hidden(vec![4..=6, 12..=20, 33..=33]);
        let check = |rows: &RowMap| {
            for line in 0..70 {
                assert_eq!(rows.visual_line(line, true), scanned(rows, line), "line {line}");
                assert_eq!(
                    rows.is_hidden(line),
                    rows.hidden.iter().any(|range| range.contains(&line))
                );
            }
        };
        check(&rows);
        // Measurements update the sums in place, including a hidden line's and
        // a line shrinking back to one row.
        for (line, measured) in [(9, 5), (13, 4), (3, 1), (61, 2), (0, 7), (9, 1)] {
            rows.set_wrap_rows(line, measured, 0..0);
            check(&rows);
        }
        rows.retain_wrap(|line| line % 2 == 0);
        check(&rows);
        rows.set_hidden(Vec::new());
        check(&rows);
        let wrap = rows.take_wrap();
        assert_eq!(rows.visual_line(50, true), 50 + 7);
        rows.replace_wrap(wrap);
        check(&rows);
    }
    #[test]
    fn fold_all_with_wrap_costs_logarithmic_steps_per_row() {
        // Fold All over 8,192 regions plus the most wrap measurements drawing
        // keeps: the scans cost 8,192 × (512 + spacers) steps per call.
        let mut rows = RowMap::default();
        rows.set_hidden((0..8_192).map(|fold| fold * 10 + 1..=fold * 10 + 8).collect());
        rows.set_spacers((0..1_000).map(|spacer| (spacer * 80 + 9, 1)).collect());
        for line in 0..bareline_renderer::MAX_LAYOUTS {
            rows.set_wrap_rows(line * 160, 3, 0..0);
        }
        STEPS.with(|steps| steps.set(0));
        let mut total = 0;
        for line in (0..81_920).step_by(97) {
            total += rows.visual_line(line, true);
        }
        let calls = 81_920usize.div_ceil(97);
        let steps = STEPS.with(std::cell::Cell::get);
        // Three binary searches: log2(8,192) + log2(1,000) + log2(512) < 40.
        assert!(steps <= calls * 40, "{steps} steps for {calls} calls");
        assert!(total > 0);
    }
}
