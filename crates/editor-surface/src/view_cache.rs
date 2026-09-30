// SPDX-License-Identifier: MPL-2.0
//! Per-line view caches carried across an edit (EDT-18, EDT-05).
//!
//! Shaped layouts, prepared long-line fragments and measured wrap rows are
//! keyed by logical line. An edit changes only the lines it reaches, so every
//! other line keeps its cache under its new number: typing reshapes one line
//! instead of the whole viewport, long lines elsewhere do not flash
//! "Preparing line…", and wrapped rows above the view stay measured, so the
//! view does not jump or blank while typing.
use crate::{
    EditorSurface,
    edit_walk::{EditWalk, shifted},
    row_map::WrapRows,
};
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_renderer::TextBackend;
use std::{
    collections::{BTreeMap, btree_map::Entry},
    ops::Range,
};

#[cfg(test)]
thread_local! {
    /// Line-number lookups `map_lines` made on this thread; a deterministic
    /// cost measure for complexity tests.
    pub(crate) static LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Where a cached line landed after an edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MappedLine {
    pub(crate) line: usize,
    /// Byte shift of the line's first byte.
    pub(crate) shift: i128,
    /// An edit reached the line's content or the byte after it (its line
    /// break, or the end of the text), so the cache no longer describes it.
    pub(crate) touched: bool,
}
/// The top of a wrapped view as a logical line and the rows into it, so an
/// edit that changes rows above the view keeps the same text at the top
/// (EDT-05).
#[derive(Clone, Debug)]
pub(crate) struct ScrollAnchor {
    /// The scroll position it describes; a later scroll replaces it.
    pub(crate) scroll_y: f64,
    pub(crate) line: usize,
    pub(crate) content: Range<usize>,
    pub(crate) rows: f64,
}
/// Maps cached lines, given as `(line, content bytes)` in increasing line
/// order before the edits `walk` holds, to their lines in `snapshot`, the text
/// after them. `None` when an edit removed the line's first byte. Lines with
/// no edit between them keep their distance, so only the first line after
/// each run of edits looks its number up: O(lines + edits) plus one lookup
/// per run.
pub(crate) fn map_lines(
    walk: &mut EditWalk,
    snapshot: &DocumentSnapshot,
    lines: &[(usize, Range<usize>)],
) -> Vec<Option<MappedLine>> {
    walk.restart();
    // (line before, line after, edits passed) of the last mapped line.
    let mut previous: Option<(usize, usize, usize)> = None;
    let mut mapped = Vec::with_capacity(lines.len());
    for (line, content) in lines {
        let (shift, next) = walk.advance(|edit| edit.range.end.0 < content.start);
        let (survives, touched) = next.map_or((true, false), |edit| {
            (edit.range.start.0 >= content.start, edit.range.start.0 <= content.end)
        });
        if !survives {
            mapped.push(None);
            continue;
        }
        let passed = walk.passed();
        let number = match previous {
            _ if passed == 0 => Some(*line),
            Some((before, after, at)) if at == passed => Some(after + (line - before)),
            _ => {
                #[cfg(test)]
                LOOKUPS.with(|lookups| lookups.set(lookups.get() + 1));
                snapshot.line_at(TextOffset(shifted(content.start, shift))).ok()
            }
        };
        let Some(number) = number else {
            mapped.push(None);
            continue;
        };
        previous = Some((*line, number, passed));
        mapped.push(Some(MappedLine {
            line: number,
            shift,
            touched,
        }));
    }
    mapped
}
impl EditorSurface {
    /// Carries the per-line caches of the snapshot they were built on through
    /// the one applied change that leads to the current snapshot. False when
    /// no such change links them; the caller then releases everything.
    pub(crate) fn carry_layouts(&mut self, backend: &mut impl TextBackend) -> bool {
        let Some(old) = self.layout_snapshot.take() else {
            return false;
        };
        let Some(change) = self
            .snapshot
            .applied_change()
            .filter(|change| {
                old.same_document(&self.snapshot) && change.matches_before(old.identity_token(), old.content_state)
            })
            .cloned()
        else {
            return false;
        };
        let edits = change.edits();
        if edits.windows(2).any(|pair| pair[1].before.start < pair[0].before.end) {
            return false;
        }
        let mut walk = EditWalk::from_edits(edits.iter().map(|edit| (edit.before.clone(), edit.inserted_len)));
        let anchor = self.scroll_anchor.take();
        // Every cached line with the content bytes it was measured on.
        let mut cached: BTreeMap<usize, Range<usize>> = BTreeMap::new();
        for (line, layout) in &self.layouts {
            cached.entry(*line).or_insert_with(|| layout.content.clone());
        }
        for (line, state) in &self.virtual_lines {
            cached.entry(*line).or_insert_with(|| state.range().clone());
        }
        for (line, measured) in self.rows.wrap_entries() {
            cached.entry(line).or_insert_with(|| measured.content.clone());
        }
        if let Some(anchor) = &anchor {
            cached.entry(anchor.line).or_insert_with(|| anchor.content.clone());
        }
        let cached: Vec<_> = cached.into_iter().collect();
        let mapped = map_lines(&mut walk, &self.snapshot, &cached);
        let find = |line: usize| {
            cached
                .binary_search_by_key(&line, |(line, _)| *line)
                .ok()
                .and_then(|index| mapped[index])
        };
        let moved = |range: &Range<usize>, shift: i128| shifted(range.start, shift)..shifted(range.end, shift);

        let mut layouts = BTreeMap::new();
        for (line, mut layout) in std::mem::take(&mut self.layouts) {
            let Some(to) = find(line).filter(|_| !layout.stale) else {
                backend.release_layout(layout.id);
                continue;
            };
            // An edited line keeps its old layout only until this draw reshapes
            // it, so revealing the caret still finds the caret's row in it.
            layout.stale = to.touched;
            layout.content = moved(&layout.content, to.shift);
            layout.start = shifted(layout.start, to.shift);
            layout.end = shifted(layout.end, to.shift);
            match layouts.entry(to.line) {
                Entry::Vacant(slot) => {
                    slot.insert(layout);
                }
                // Two old lines landed on one: keep a current layout over a stale one.
                Entry::Occupied(mut slot) => {
                    let loser = if slot.get().stale && !layout.stale {
                        slot.insert(layout)
                    } else {
                        layout
                    };
                    backend.release_layout(loser.id);
                }
            }
        }
        self.layouts = layouts;

        let state = self.snapshot.content_state;
        for (line, mut virtual_line) in std::mem::take(&mut self.virtual_lines) {
            if let Some(to) = find(line).filter(|to| !to.touched) {
                virtual_line.shift(state, to.shift);
                self.virtual_lines.insert(to.line, virtual_line);
            }
        }

        let mut wrap = BTreeMap::new();
        for (line, measured) in self.rows.take_wrap() {
            let Some(to) = find(line) else {
                continue;
            };
            // An edited line's rows are measured again when it is drawn. Until
            // then a line on screen, or the line at the top of the view, keeps
            // its old rows as an estimate rather than collapsing to one row,
            // which would shift every row below it for a frame.
            if to.touched
                && !self.layouts.contains_key(&to.line)
                && anchor.as_ref().is_none_or(|anchor| anchor.line != line)
            {
                continue;
            }
            wrap.insert(
                to.line,
                WrapRows {
                    rows: measured.rows,
                    content: moved(&measured.content, to.shift),
                },
            );
        }
        self.rows.replace_wrap(wrap);

        if let Some(mut widest) = self
            .horizontal_line
            .filter(|line| line.identity == old.identity_token())
        {
            walk.restart();
            let (shift, next) = walk.advance(|edit| edit.range.end.0 < widest.start);
            if !next.is_some_and(|edit| edit.range.start.0 <= widest.end) {
                widest.start = shifted(widest.start, shift);
                widest.end = shifted(widest.end, shift);
                widest.identity = self.snapshot.identity_token();
                self.horizontal_line = Some(widest);
            }
        }

        if let Some(anchor) = anchor
            && self.wrap
            && anchor.scroll_y.to_bits() == self.scroll_y.to_bits()
            && let Some(to) = find(anchor.line)
        {
            self.scroll_y = ((self.visual_line(to.line) as f64 + anchor.rows) * self.line_height() as f64).max(0.0);
        }
        true
    }
    /// Records which text is at the top of a wrapped view, for `carry_layouts`.
    pub(crate) fn capture_scroll_anchor(&mut self) {
        self.scroll_anchor = None;
        if !self.wrap {
            return;
        }
        let row = self.scroll_y / self.line_height() as f64;
        let line = self.logical_line(row.floor() as usize);
        let Some(layout) = self.layouts.get(&line).filter(|layout| !layout.stale) else {
            return;
        };
        self.scroll_anchor = Some(ScrollAnchor {
            scroll_y: self.scroll_y,
            line,
            content: layout.content.clone(),
            rows: row - self.visual_line(line) as f64,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document, Edit, EditTransaction};
    #[test]
    fn untouched_lines_map_with_one_lookup_per_run_of_edits() {
        let text = "0123456789\n".repeat(1_000);
        let mut document = Document::from_utf8(&text, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let before = document.snapshot();
        // Split line 100 in two and empty line 500.
        let edits = vec![
            Edit {
                range: TextOffset(100 * 11 + 3)..TextOffset(100 * 11 + 3),
                insert: "\n".into(),
            },
            Edit {
                range: TextOffset(500 * 11)..TextOffset(500 * 11 + 10),
                insert: String::new(),
            },
        ];
        document
            .apply(EditTransaction {
                base_revision: before.revision,
                edits,
            })
            .unwrap();
        let after = document.snapshot();
        let change = after.applied_change().unwrap().clone();
        let mut walk = EditWalk::from_edits(
            change
                .edits()
                .iter()
                .map(|edit| (edit.before.clone(), edit.inserted_len)),
        );
        let lines: Vec<_> = (0..1_000).map(|line| (line, line * 11..line * 11 + 10)).collect();
        LOOKUPS.with(|lookups| lookups.set(0));
        let mapped = map_lines(&mut walk, &after, &lines);
        // One lookup after each edit; every other line keeps its distance.
        assert_eq!(LOOKUPS.with(std::cell::Cell::get), 2);
        for (line, mapped) in mapped.iter().copied().enumerate() {
            let expected = match line {
                0..100 => (line, 0, false),
                100 => (100, 0, true),
                101..500 => (line + 1, 1, false),
                500 => (501, 1, true),
                _ => (line + 1, -9, false),
            };
            assert_eq!(
                mapped.map(|to| (to.line, to.shift, to.touched)),
                Some(expected),
                "line {line}"
            );
        }
        // An untouched line's text starts where it was mapped.
        for ((_, content), mapped) in lines.iter().zip(mapped.iter().copied()) {
            let to = mapped.unwrap();
            if !to.touched {
                let start = after.line_range(to.line).unwrap().start;
                assert_eq!(start.0, shifted(content.start, to.shift));
                assert_eq!(after.read(start..TextOffset(start.0 + 10), 10).unwrap(), "0123456789");
            }
        }
        // A deletion that removes a line's first byte drops the line.
        let mut walk = EditWalk::from_edits([(TextOffset(5)..TextOffset(12), 0)]);
        let mapped = map_lines(&mut walk, &before, &[(0, 0..10), (1, 11..21), (2, 22..32)]);
        assert_eq!(mapped[1], None);
        assert!(mapped[0].is_some_and(|to| to.touched));
        assert!(mapped[2].is_some_and(|to| !to.touched && to.shift == -7));
    }
}
