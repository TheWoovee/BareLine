// SPDX-License-Identifier: MPL-2.0
//! Revision-bound power edits. Offsets address UTF-8 text, never original bytes.
pub mod captured;
pub mod consumer;
pub mod streaming;
use crate::Selection;
use bareline_document::{DocumentSnapshot, Edit, EditTransaction, Error, TextOffset};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionSet {
    pub selections: Vec<Selection>,
    pub primary: usize,
}
impl From<Selection> for SelectionSet {
    fn from(value: Selection) -> Self {
        Self {
            selections: vec![value],
            primary: 0,
        }
    }
}
impl SelectionSet {
    pub fn primary(&self) -> Selection {
        self.selections.get(self.primary).copied().unwrap_or_default()
    }
    pub fn escape(&mut self) {
        *self = self.primary().into();
    }
    pub fn rotate_primary(&mut self) {
        if !self.selections.is_empty() {
            self.primary = (self.primary + 1) % self.selections.len();
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_selections: usize,
    pub tab_width: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 16 << 20,
            max_selections: 100_000,
            tab_width: 4,
        }
    }
}
pub struct PowerEdit {
    pub transaction: EditTransaction,
    pub selections: SelectionSet,
}
fn charge(total: &mut usize, n: usize, limits: Limits) -> Result<(), Error> {
    *total = total
        .checked_add(n)
        .filter(|v| *v <= limits.max_bytes)
        .ok_or(Error::BudgetExceeded)?;
    Ok(())
}
fn line(snapshot: &DocumentSnapshot, number: usize, limits: Limits) -> Result<(usize, String), Error> {
    let r = snapshot.line_range(number)?;
    let s = snapshot.read(r.clone(), limits.max_bytes)?;
    Ok((r.start.0, s))
}
fn content(s: &str) -> &str {
    s.trim_end_matches(['\r', '\n'])
}
fn snap(snapshot: &DocumentSnapshot, offset: usize, limits: Limits) -> Result<usize, Error> {
    if offset > snapshot.len() {
        return Err(Error::OutOfBounds);
    }
    let mut boundary = offset;
    while !snapshot.is_boundary(TextOffset(boundary)) {
        boundary -= 1;
    }
    // Endpoints need no source read. Interior carets need only bounded grapheme
    // context, never a materialized logical line (which may exceed the budget).
    if boundary == 0 || boundary == snapshot.len() {
        return Ok(boundary);
    }
    use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};
    let mut cursor = GraphemeCursor::new(boundary, snapshot.len(), true);
    let mut total = 0;
    let mut read_chunk = |at: usize, backwards: bool| -> Result<(usize, String), Error> {
        let (mut start, mut end) = if backwards {
            (at.saturating_sub(4096), at)
        } else {
            (at, at.saturating_add(4096).min(snapshot.len()))
        };
        while !snapshot.is_boundary(TextOffset(start)) {
            start += 1;
        }
        while !snapshot.is_boundary(TextOffset(end)) {
            end -= 1;
        }
        charge(&mut total, end - start, limits)?;
        snapshot
            .read(TextOffset(start)..TextOffset(end), limits.max_bytes)
            .map(|text| (start, text))
    };
    let (mut start, mut text) = read_chunk(boundary, false)?;
    loop {
        match cursor.is_boundary(&text, start) {
            Ok(true) => return Ok(boundary),
            Ok(false) => break,
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (from, context) = read_chunk(end, true)?;
                cursor.provide_context(&context, from);
            }
            Err(_) => return Err(Error::InvalidBoundary),
        }
    }
    loop {
        match cursor.prev_boundary(&text, start) {
            Ok(result) => return Ok(result.unwrap_or(0)),
            Err(GraphemeIncomplete::PrevChunk) => {
                (start, text) = read_chunk(start, true)?;
            }
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (from, context) = read_chunk(end, true)?;
                cursor.provide_context(&context, from);
            }
            Err(_) => return Err(Error::InvalidBoundary),
        }
    }
}
/// Normalizes editing ranges; overlapping selections and duplicate carets mutate once.
/// Every returned selection runs forward (`anchor <= caret`), as edit preparation expects.
pub fn normalize(snapshot: &DocumentSnapshot, set: &SelectionSet, limits: Limits) -> Result<SelectionSet, Error> {
    let mut set = normalize_directed(snapshot, set, limits)?;
    for s in &mut set.selections {
        let range = s.range();
        *s = Selection {
            anchor: range.start,
            caret: range.end,
        };
    }
    Ok(set)
}
/// Like [`normalize`], but each selection keeps its direction, so a view can store the
/// result and keep extending from the caret end. The primary selection is tracked by
/// index; a merged selection takes the primary's direction when it absorbed the
/// primary, and its first member's otherwise.
pub fn normalize_directed(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    limits: Limits,
) -> Result<SelectionSet, Error> {
    if limits.tab_width > limits.max_bytes {
        return Err(Error::BudgetExceeded);
    }
    if !snapshot.is_complete() {
        return Err(Error::IncompleteSource);
    }
    if set.selections.is_empty() || set.primary >= set.selections.len() {
        return Err(Error::OutOfBounds);
    }
    if set.selections.len() > limits.max_selections {
        return Err(Error::BudgetExceeded);
    }
    let mut snapped = Vec::with_capacity(set.selections.len());
    for s in &set.selections {
        snapped.push(Selection {
            anchor: snap(snapshot, s.anchor, limits)?,
            caret: snap(snapshot, s.caret, limits)?,
        });
    }
    Ok(merge_directed(&snapped, set.primary))
}
/// The merge step of [`normalize_directed`], for selections already on grapheme
/// boundaries: a large occurrence set skips a source read per selection end.
fn merge_directed(selections: &[Selection], primary_index: usize) -> SelectionSet {
    let mut ranges = selections
        .iter()
        .enumerate()
        .map(|(index, s)| (s.range(), s.anchor > s.caret, index))
        .collect::<Vec<_>>();
    ranges.sort_by_key(|(r, _, _)| (r.start, r.end));
    let mut merged: Vec<(Range<usize>, bool)> = Vec::new();
    let mut primary = 0;
    for (r, backward, index) in ranges {
        if let Some((last, last_backward)) = merged.last_mut()
            && (r.start < last.end || r.start == last.start)
        {
            last.end = last.end.max(r.end);
            if index == primary_index {
                *last_backward = backward;
                primary = merged.len() - 1;
            }
            continue;
        }
        if index == primary_index {
            primary = merged.len();
        }
        merged.push((r, backward));
    }
    SelectionSet {
        selections: merged
            .into_iter()
            .map(|(r, backward)| {
                if backward {
                    Selection {
                        anchor: r.end,
                        caret: r.start,
                    }
                } else {
                    Selection {
                        anchor: r.start,
                        caret: r.end,
                    }
                }
            })
            .collect(),
        primary,
    }
}
/// `finish` yields one caret per edit, in edit order. When a producer left the
/// primary at the first caret, move it to the caret of the edit that consumed the
/// previous primary caret, so a multi-caret edit does not jump the view (EDT-12).
pub(crate) fn keep_primary(prepared: &mut PowerEdit, before: &SelectionSet) {
    let edits = &prepared.transaction.edits;
    if prepared.selections.primary != 0 || edits.len() < 2 || edits.len() != prepared.selections.selections.len() {
        return;
    }
    let caret = before.primary().caret;
    if let Some(index) = edits
        .iter()
        .position(|edit| edit.range.start.0 <= caret && caret <= edit.range.end.0)
    {
        prepared.selections.primary = index;
    }
}
pub(crate) fn finish(snapshot: &DocumentSnapshot, mut edits: Vec<Edit>, limits: Limits) -> Result<PowerEdit, Error> {
    if limits.tab_width > limits.max_bytes {
        return Err(Error::BudgetExceeded);
    }
    if !snapshot.is_complete() {
        return Err(Error::IncompleteSource);
    }
    if edits.len() > limits.max_selections {
        return Err(Error::BudgetExceeded);
    }
    edits.sort_by_key(|e| e.range.start);
    let mut total = 0;
    let mut previous = None;
    let mut delta = 0isize;
    let mut selections = Vec::new();
    for e in &edits {
        if e.range.start > e.range.end || !snapshot.is_boundary(e.range.start) || !snapshot.is_boundary(e.range.end) {
            return Err(Error::InvalidBoundary);
        }
        if previous.is_some_and(|end| e.range.start.0 < end) {
            return Err(Error::OverlappingEdits);
        }
        previous = Some(e.range.end.0);
        charge(&mut total, e.range.end.0 - e.range.start.0, limits)?;
        charge(&mut total, e.insert.len(), limits)?;
        let p = e
            .range
            .start
            .0
            .checked_add_signed(delta)
            .and_then(|v| v.checked_add(e.insert.len()))
            .ok_or(Error::BudgetExceeded)?;
        selections.push(Selection { anchor: p, caret: p });
        delta += e.insert.len() as isize - (e.range.end.0 - e.range.start.0) as isize;
    }
    if selections.is_empty() {
        selections.push(Selection::default());
    }
    Ok(PowerEdit {
        transaction: EditTransaction {
            base_revision: snapshot.revision,
            edits,
        },
        selections: SelectionSet { selections, primary: 0 },
    })
}
pub fn replace(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    text: &str,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    let set = normalize(snapshot, set, limits)?;
    let mut total = 0;
    let mut edits = Vec::new();
    for s in &set.selections {
        charge(&mut total, text.len(), limits)?;
        edits.push(Edit {
            range: TextOffset(s.anchor)..TextOffset(s.caret),
            insert: text.into(),
        });
    }
    // One edit per normalized selection, already in order: the primary keeps its index.
    let mut prepared = finish(snapshot, edits, limits)?;
    prepared.selections.primary = set.primary;
    Ok(prepared)
}
pub fn delete(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    backward: bool,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    let mut set = normalize(snapshot, set, limits)?;
    for s in &mut set.selections {
        if s.anchor != s.caret {
            continue;
        }
        let n = snapshot.line_at(TextOffset(s.caret))?;
        let (start, text) = line(snapshot, n, limits)?;
        let local = s.caret - start;
        if backward {
            if local == 0 && start > 0 {
                let (a, t) = line(snapshot, n - 1, limits)?;
                s.anchor = a + t.grapheme_indices(true).next_back().map_or(0, |(i, _)| i);
            } else {
                s.anchor = start
                    + text
                        .grapheme_indices(true)
                        .map(|(i, _)| i)
                        .take_while(|i| *i < local)
                        .last()
                        .unwrap_or(0);
            }
        } else {
            s.caret = start
                + text
                    .grapheme_indices(true)
                    .map(|(i, _)| i)
                    .chain(Some(text.len()))
                    .find(|i| *i > local)
                    .unwrap_or(local);
        }
    }
    replace(snapshot, &set, "", limits)
}
/// Metrics may be supplied by the renderer; fallback width handles tabs and common wide clusters.
#[derive(Clone, Debug)]
pub struct DisplayColumnMap {
    pub stops: Vec<(usize, usize)>,
}
impl DisplayColumnMap {
    pub fn with_metrics(text: &str, tab_width: usize, mut width: impl FnMut(&str) -> usize) -> Self {
        let mut col = 0;
        let mut stops = vec![(0, 0)];
        for (i, g) in text.grapheme_indices(true) {
            col += if g == "\t" {
                tab_width.max(1) - col % tab_width.max(1)
            } else {
                width(g)
            };
            stops.push((i + g.len(), col));
        }
        Self { stops }
    }
    pub fn new(text: &str, tab_width: usize) -> Self {
        Self::with_metrics(text, tab_width, |g| {
            if g.chars().any(|c|matches!(c as u32,0x1100..=0x115f|0x2e80..=0xa4cf|0xac00..=0xd7a3|0xf900..=0xfaff|0xfe10..=0xfe6f|0xff01..=0xff60|0x1f000..=0x1faff|0x20000..=0x3ffff)){2}else{1}
        })
    }
    pub fn at(&self, column: usize) -> (usize, usize) {
        let &(byte, col) = self.stops.iter().rev().find(|(_, c)| *c <= column).unwrap_or(&(0, 0));
        (byte, column.saturating_sub(col))
    }
    pub fn column(&self, byte: usize) -> usize {
        self.stops
            .iter()
            .take_while(|(b, _)| *b <= byte)
            .last()
            .map_or(0, |(_, c)| *c)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Rectangle {
    pub first_line: usize,
    pub last_line: usize,
    pub start_column: usize,
    pub end_column: usize,
}
#[derive(Clone, Debug)]
pub enum ColumnInsert {
    Text(String),
    Numbers {
        start: i64,
        step: i64,
        width: usize,
        base: u8,
        repeat: usize,
    },
}
fn number(value: i64, base: u8, width: usize) -> Result<String, Error> {
    if ![2, 8, 10, 16].contains(&base) {
        return Err(Error::OutOfBounds);
    }
    let n = value.unsigned_abs();
    let digits = match base {
        2 => format!("{n:b}"),
        8 => format!("{n:o}"),
        16 => format!("{n:x}"),
        _ => n.to_string(),
    };
    Ok(format!(
        "{}{:0>width$}",
        if value < 0 { "-" } else { "" },
        digits,
        width = width
    ))
}
pub fn column_insert(
    snapshot: &DocumentSnapshot,
    rectangle: Rectangle,
    insert: ColumnInsert,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    column_insert_mapped(snapshot, rectangle, insert, limits, None)
}
pub fn column_insert_mapped(
    snapshot: &DocumentSnapshot,
    rectangle: Rectangle,
    insert: ColumnInsert,
    limits: Limits,
    maps: Option<&std::collections::BTreeMap<usize, DisplayColumnMap>>,
) -> Result<PowerEdit, Error> {
    if rectangle.first_line > rectangle.last_line || rectangle.last_line >= snapshot.line_count() {
        return Err(Error::OutOfBounds);
    }
    if rectangle.last_line - rectangle.first_line >= limits.max_selections {
        return Err(Error::BudgetExceeded);
    }
    let mut edits = Vec::new();
    let mut total = 0;
    for (row, n) in (rectangle.first_line..=rectangle.last_line).enumerate() {
        let (start, text) = line(snapshot, n, limits)?;
        let body = content(&text);
        let fallback;
        let map = if let Some(maps) = maps {
            maps.get(&n).ok_or(Error::OutOfBounds)?
        } else {
            fallback = DisplayColumnMap::new(body, limits.tab_width);
            &fallback
        };
        let left = rectangle.start_column.min(rectangle.end_column);
        let right = rectangle.start_column.max(rectangle.end_column);
        let (a, mut pad) = map.at(left);
        if a < body.len() && !body[a..].starts_with('\t') {
            pad = 0;
        }
        let (mut b, _) = map.at(right);
        if right > left && map.column(b) < right && b < body.len() {
            b = map.stops.iter().find(|(p, _)| *p > b).map_or(b, |(p, _)| *p);
        }
        let value = match &insert {
            ColumnInsert::Text(s) => {
                if s.len() > limits.max_bytes {
                    return Err(Error::BudgetExceeded);
                }
                s.clone()
            }
            ColumnInsert::Numbers {
                start,
                step,
                width,
                base,
                repeat,
            } => {
                if *width > limits.max_bytes || *repeat > limits.max_bytes {
                    return Err(Error::BudgetExceeded);
                }
                let value = start
                    .checked_add(
                        step.checked_mul(i64::try_from(row).map_err(|_| Error::BudgetExceeded)?)
                            .ok_or(Error::BudgetExceeded)?,
                    )
                    .ok_or(Error::BudgetExceeded)?;
                let s = number(value, *base, *width)?;
                if s.len().checked_mul(*repeat).is_none_or(|v| v > limits.max_bytes) {
                    return Err(Error::BudgetExceeded);
                }
                s.repeat(*repeat)
            }
        };
        // An interior tab is expanded to spaces; wide clusters are snapped as a unit.
        let mut replacement = String::new();
        charge(&mut total, pad, limits)?;
        charge(&mut total, value.len(), limits)?;
        replacement.extend(std::iter::repeat_n(' ', pad));
        replacement.push_str(&value);
        let (right_byte, inside) = map.at(right);
        if inside > 0 && right_byte < body.len() && body[right_byte..].starts_with('\t') {
            let next = map.stops.iter().find(|(p, _)| *p > right_byte).copied().unwrap();
            b = next.0;
            let suffix = next.1 - right;
            charge(&mut total, suffix, limits)?;
            replacement.extend(std::iter::repeat_n(' ', suffix));
        }
        edits.push(Edit {
            range: TextOffset(start + a)..TextOffset(start + b),
            insert: replacement,
        });
    }
    finish(snapshot, edits, limits)
}
/// Removes a rectangle's text. Unlike an insertion, deletion never pads a row
/// that ends before the rectangle: such a row is left unchanged. A zero-width
/// rectangle removes nothing unless `backward` names a direction: then each
/// row loses the grapheme before (`Some(true)`, Backspace) or after
/// (`Some(false)`, Delete) the column, never a line break. Every row keeps a caret.
pub fn rectangle_delete_mapped(
    snapshot: &DocumentSnapshot,
    rectangle: Rectangle,
    backward: Option<bool>,
    limits: Limits,
    maps: Option<&std::collections::BTreeMap<usize, DisplayColumnMap>>,
) -> Result<PowerEdit, Error> {
    if rectangle.first_line > rectangle.last_line || rectangle.last_line >= snapshot.line_count() {
        return Err(Error::OutOfBounds);
    }
    if rectangle.last_line - rectangle.first_line >= limits.max_selections {
        return Err(Error::BudgetExceeded);
    }
    let left = rectangle.start_column.min(rectangle.end_column);
    let right = rectangle.start_column.max(rectangle.end_column);
    let mut edits = Vec::new();
    let mut carets = Vec::new();
    let mut total = 0;
    let mut delta = 0isize;
    for n in rectangle.first_line..=rectangle.last_line {
        let (start, text) = line(snapshot, n, limits)?;
        let body = content(&text);
        let fallback;
        let map = if let Some(maps) = maps {
            maps.get(&n).ok_or(Error::OutOfBounds)?
        } else {
            fallback = DisplayColumnMap::new(body, limits.tab_width);
            &fallback
        };
        let next = |byte: usize| map.stops.iter().find(|(p, _)| *p > byte).map_or(byte, |(p, _)| *p);
        let (a, inside) = map.at(left);
        // (removed range, spaces kept before and after it) in row-local bytes.
        let (from, to, pad, suffix) = if a >= body.len() {
            // The row ends at or before the column. Only Backspace at its exact end removes text.
            if backward == Some(true) && inside == 0 && a > 0 && right == left {
                let previous = map.stops.iter().rev().find(|(p, _)| *p < a).map_or(0, |(p, _)| *p);
                (previous, a, 0, 0)
            } else {
                (body.len(), body.len(), 0, 0)
            }
        } else if right > left {
            // As in column_insert: a split tab keeps its outside columns as spaces;
            // a wide cluster is removed as a unit.
            let pad = if body[a..].starts_with('\t') { inside } else { 0 };
            let (mut b, beyond) = map.at(right);
            let mut suffix = 0;
            if beyond > 0 && b < body.len() && body[b..].starts_with('\t') {
                let (end, column) = map.stops.iter().find(|(p, _)| *p > b).copied().unwrap_or((b, right));
                suffix = column.saturating_sub(right);
                b = end;
            } else if map.column(b) < right && b < body.len() {
                b = next(b);
            }
            (a, b, pad, suffix)
        } else {
            match backward {
                // Inside a tab or wide cluster, either key removes that cluster.
                Some(_) if inside > 0 => (a, next(a), 0, 0),
                Some(true) => {
                    let previous = map.stops.iter().rev().find(|(p, _)| *p < a).map_or(a, |(p, _)| *p);
                    (previous, a, 0, 0)
                }
                Some(false) => (a, next(a), 0, 0),
                None => (a, a, 0, 0),
            }
        };
        let caret = (start + from)
            .checked_add_signed(delta)
            .and_then(|v| v.checked_add(pad))
            .ok_or(Error::BudgetExceeded)?;
        carets.push(Selection { anchor: caret, caret });
        if from < to || pad + suffix > 0 {
            charge(&mut total, pad + suffix, limits)?;
            let insert = " ".repeat(pad + suffix);
            delta += insert.len() as isize - (to - from) as isize;
            edits.push(Edit {
                range: TextOffset(start + from)..TextOffset(start + to),
                insert,
            });
        }
    }
    let mut prepared = finish(snapshot, edits, limits)?;
    prepared.selections = SelectionSet {
        selections: carets,
        primary: 0,
    };
    Ok(prepared)
}
#[derive(Clone, Debug)]
pub enum Transform {
    Uppercase,
    Lowercase,
    Titlecase,
    InvertCase,
    TrimStart,
    TrimEnd,
    Trim,
    Indent,
    Unindent,
    TabsToSpaces,
    SpacesToTabs,
    Duplicate,
    DuplicateSelections,
    MoveUp,
    MoveDown,
    Join,
    Split {
        column: usize,
    },
    Sort {
        descending: bool,
        case_sensitive: bool,
        numeric: bool,
    },
    RemoveDuplicates,
    RemoveConsecutiveDuplicates,
    RemoveEmpty,
    RemoveBlank,
}
pub fn transform(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    action: Transform,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    let set = normalize_directed(snapshot, set, limits)?;
    let case = matches!(
        action,
        Transform::Uppercase | Transform::Lowercase | Transform::Titlecase | Transform::InvertCase
    );
    let linewise = !case && !matches!(action, Transform::DuplicateSelections);
    // Each merged range keeps the indices of the selections it covers, so the
    // output can place them on the transformed text instead of after it (EDT-04).
    let mut ranges = Vec::<(Range<usize>, Range<usize>)>::new();
    for (index, s) in set.selections.iter().enumerate() {
        let mut r = s.range();
        if linewise {
            let first = snapshot.line_at(TextOffset(r.start))?;
            let last = snapshot
                .line_at(TextOffset(if r.end > r.start { r.end - 1 } else { r.end }))
                .or_else(|_| snapshot.line_at(TextOffset(snap(snapshot, r.end.saturating_sub(1), limits)?)))?;
            r = snapshot.line_range(first)?.start.0..snapshot.line_range(last)?.end.0;
        }
        if let Some((prev, members)) = ranges.last_mut()
            && r.start <= prev.end
        {
            prev.end = prev.end.max(r.end);
            members.end = index + 1;
            continue;
        }
        ranges.push((r, index..index + 1));
    }
    // A line break the selected text does not supply follows the document (EDT-24).
    let fallback_eol = snapshot.insertion_eol();
    let row_local = matches!(
        action,
        Transform::TrimStart
            | Transform::TrimEnd
            | Transform::Trim
            | Transform::Indent
            | Transform::Unindent
            | Transform::TabsToSpaces
            | Transform::SpacesToTabs
    );
    let mut edits = Vec::new();
    let mut after = Vec::with_capacity(set.selections.len());
    let mut delta = 0isize;
    let mut total = 0;
    for (mut r, members) in ranges {
        let mut source = snapshot.read(TextOffset(r.start)..TextOffset(r.end), limits.max_bytes)?;
        charge(&mut total, source.len(), limits)?;
        let mut placement = if matches!(action, Transform::Duplicate | Transform::DuplicateSelections) {
            // The original stays first, so its selections keep their offsets.
            Placement::Kept
        } else {
            Placement::Whole { same_offsets: false }
        };
        let result = match &action {
            Transform::MoveUp | Transform::MoveDown => {
                let first = snapshot.line_at(TextOffset(r.start))?;
                let origin = r.start;
                if matches!(action, Transform::MoveUp) && first > 0 {
                    let (start, previous) = line(snapshot, first - 1, limits)?;
                    r.start = start;
                    let (moved, block) = move_rows(&source, &previous, false, fallback_eol);
                    placement = Placement::Moved {
                        origin: origin - start,
                        block,
                    };
                    source = format!("{previous}{source}");
                    moved
                } else if matches!(action, Transform::MoveDown) && r.end < snapshot.len() {
                    let next = snapshot.line_at(TextOffset(r.end))?;
                    let (start, following) = line(snapshot, next, limits)?;
                    r.end = start + following.len();
                    let (moved, block) = move_rows(&source, &following, true, fallback_eol);
                    placement = Placement::Moved { origin: 0, block };
                    source.push_str(&following);
                    moved
                } else {
                    // Nothing to move past: the text and its selections stay.
                    placement = Placement::Kept;
                    source.clone()
                }
            }
            Transform::DuplicateSelections => {
                if source.len() > limits.max_bytes / 2 {
                    return Err(Error::BudgetExceeded);
                }
                source.repeat(2)
            }
            Transform::Duplicate => {
                let eol = split_rows(&source)
                    .into_iter()
                    .find(|(_, e)| !e.is_empty())
                    .map_or(fallback_eol, |(_, e)| e);
                let separator = if source.ends_with(['\r', '\n']) { "" } else { eol };
                if source
                    .len()
                    .checked_mul(2)
                    .and_then(|n| n.checked_add(separator.len()))
                    .is_none_or(|n| n > limits.max_bytes)
                {
                    return Err(Error::BudgetExceeded);
                }
                format!("{source}{separator}{source}")
            }
            Transform::Uppercase => source.to_uppercase(),
            Transform::Lowercase => source.to_lowercase(),
            Transform::Titlecase => source
                .split_word_bounds()
                .map(|w| {
                    let mut c = w.chars();
                    c.next().map_or_else(String::new, |f| {
                        format!("{}{}", f.to_uppercase(), c.as_str().to_lowercase())
                    })
                })
                .collect(),
            Transform::InvertCase => source
                .chars()
                .map(|c| {
                    if c.is_uppercase() {
                        c.to_lowercase().collect::<String>()
                    } else {
                        c.to_uppercase().collect()
                    }
                })
                .collect(),
            _ => {
                let mut rows = split_rows(&source);
                let eol = rows
                    .iter()
                    .find(|(_, e)| !e.is_empty())
                    .map_or(fallback_eol, |(_, e)| *e)
                    .to_string();
                let trailing = rows.last().is_some_and(|(_, e)| !e.is_empty());
                let mut placed = Vec::new();
                let mut source_row = 0;
                match &action {
                    Transform::Sort {
                        descending,
                        case_sensitive,
                        numeric,
                    } => {
                        rows.sort_by_cached_key(|row| sort_key(row.0, *case_sensitive, *numeric));
                        if *descending {
                            rows.reverse();
                        }
                    }
                    Transform::RemoveDuplicates => {
                        let mut seen = std::collections::BTreeSet::new();
                        rows.retain(|(s, _)| seen.insert(*s));
                    }
                    Transform::RemoveConsecutiveDuplicates => rows.dedup_by(|a, b| a.0 == b.0),
                    Transform::RemoveEmpty => rows.retain(|(s, _)| !s.is_empty()),
                    Transform::RemoveBlank => rows.retain(|(s, _)| !s.trim().is_empty()),
                    _ => {}
                }
                let mut out = String::new();
                for (i, (body, ending)) in rows.iter().enumerate() {
                    let row: &str = body;
                    let body = match &action {
                        Transform::TrimStart => body.trim_start().to_string(),
                        Transform::TrimEnd => body.trim_end().to_string(),
                        Transform::Trim => body.trim().to_string(),
                        Transform::Indent => {
                            format!("{}{}", " ".repeat(limits.tab_width.min(limits.max_bytes)), body)
                        }
                        Transform::Unindent => {
                            if let Some(s) = body.strip_prefix('\t') {
                                s.to_string()
                            } else {
                                body.chars()
                                    .skip(body.chars().take(limits.tab_width).take_while(|c| *c == ' ').count())
                                    .collect()
                            }
                        }
                        Transform::TabsToSpaces => {
                            let mut out = String::new();
                            let mut col = 0;
                            for g in body.graphemes(true) {
                                if g == "\t" {
                                    let n = limits.tab_width.max(1) - col % limits.tab_width.max(1);
                                    if out.len().checked_add(n).is_none_or(|v| v > limits.max_bytes) {
                                        return Err(Error::BudgetExceeded);
                                    }
                                    out.extend(std::iter::repeat_n(' ', n));
                                    col += n;
                                } else {
                                    out.push_str(g);
                                    col += DisplayColumnMap::new(g, limits.tab_width).column(g.len());
                                }
                            }
                            out
                        }
                        Transform::SpacesToTabs => {
                            let width = limits.tab_width.max(1);
                            let mut out = String::new();
                            let mut col = 0;
                            let mut spaces = 0;
                            for g in body.graphemes(true).chain(Some("")) {
                                if g == " " {
                                    spaces += 1;
                                    continue;
                                }
                                while spaces > 0 {
                                    let n = width - col % width;
                                    if spaces >= n && n > 1 {
                                        out.push('\t');
                                        spaces -= n;
                                        col += n;
                                    } else {
                                        out.push(' ');
                                        spaces -= 1;
                                        col += 1;
                                    }
                                }
                                out.push_str(g);
                                col += if g == "\t" {
                                    width - col % width
                                } else {
                                    DisplayColumnMap::new(g, width).column(g.len())
                                };
                            }
                            out
                        }
                        Transform::Split { column } => {
                            let mut out = String::new();
                            for (n, g) in body.graphemes(true).enumerate() {
                                if n > 0 && n % column.max(&1) == 0 {
                                    out.push_str(&eol);
                                }
                                out.push_str(g);
                            }
                            out
                        }
                        _ => body.to_string(),
                    };
                    if row_local {
                        let (prefix, suffix) = common_affixes(row, &body);
                        placed.push(RowPlacement {
                            source: source_row,
                            body: row.len(),
                            output: out.len(),
                            output_body: body.len(),
                            prefix,
                            suffix,
                        });
                        source_row += row.len() + ending.len();
                    }
                    out.push_str(&body);
                    if matches!(action, Transform::Duplicate) {
                        out.push_str(if ending.is_empty() { &eol } else { ending });
                        out.push_str(&body);
                        out.push_str(ending);
                    } else if matches!(action, Transform::Join) {
                        if i + 1 < rows.len() {
                            out.push(' ');
                        } else {
                            out.push_str(ending);
                        }
                    } else if matches!(
                        action,
                        Transform::Sort { .. }
                            | Transform::RemoveDuplicates
                            | Transform::RemoveConsecutiveDuplicates
                            | Transform::RemoveEmpty
                            | Transform::RemoveBlank
                    ) {
                        if i + 1 < rows.len() || trailing {
                            out.push_str(&eol);
                        }
                    } else {
                        out.push_str(ending);
                    }
                    if out.len() > limits.max_bytes {
                        return Err(Error::BudgetExceeded);
                    }
                }
                if row_local {
                    placement = Placement::Rows(placed);
                }
                out
            }
        };
        charge(&mut total, result.len(), limits)?;
        if result == source && !matches!(placement, Placement::Moved { .. }) {
            // Unchanged text keeps its selections exactly.
            placement = Placement::Kept;
        } else if let Placement::Whole { same_offsets } = &mut placement {
            // Case mapping rewrites characters in place unless it changes their length.
            *same_offsets = case && result.len() == source.len();
        }
        let output = r.start.checked_add_signed(delta).ok_or(Error::BudgetExceeded)?;
        for s in &set.selections[members] {
            let collapsed = s.anchor == s.caret;
            let place =
                |offset: usize, start: bool| output + placement.place(offset - r.start, &result, start, collapsed);
            after.push(Selection {
                anchor: place(s.anchor, s.anchor <= s.caret),
                caret: place(s.caret, s.caret < s.anchor),
            });
        }
        // An unchanged range is no edit: no dirty flag and no empty undo step (EDT-23).
        if result != source {
            delta += result.len() as isize - source.len() as isize;
            edits.push(Edit {
                range: TextOffset(r.start)..TextOffset(r.end),
                insert: result,
            });
        }
    }
    let mut prepared = finish(snapshot, edits, limits)?;
    // Selections of one range can land on the same output (a sorted block); keep one.
    let mut primary = set.primary;
    let mut selections = Vec::with_capacity(after.len());
    for (index, selection) in after.into_iter().enumerate() {
        if selections.last() == Some(&selection) {
            if index <= primary {
                primary -= 1;
            }
            continue;
        }
        selections.push(selection);
    }
    prepared.selections = SelectionSet { selections, primary };
    Ok(prepared)
}
/// Where the selections of one transformed range land, relative to its replacement.
enum Placement {
    /// Offsets keep their distance from the range start.
    Kept,
    /// Rows were reordered, joined, split or rewritten: a selection covers the whole
    /// result, and a caret keeps its offset only when `same_offsets` says the text
    /// kept its byte positions; otherwise it goes to the end of the result.
    Whole { same_offsets: bool },
    /// The selected block, starting `origin` bytes into the range, moved.
    Moved { origin: usize, block: MovedBlock },
    /// Rows keep their count; offsets follow their row.
    Rows(Vec<RowPlacement>),
}
struct RowPlacement {
    source: usize,
    body: usize,
    output: usize,
    output_body: usize,
    /// Bytes the source and output bodies share at their start and, after that,
    /// at their end (see [`common_affixes`]). Offsets in either part map exactly;
    /// an offset in the rewritten middle goes to the end of the output's middle.
    prefix: usize,
    suffix: usize,
}
impl Placement {
    /// Output offset of `local` (relative to the replaced range) inside `result`,
    /// always on one of its character boundaries. `start` marks the first end of a
    /// non-empty selection.
    fn place(&self, local: usize, result: &str, start: bool, collapsed: bool) -> usize {
        let at = match self {
            Placement::Kept => local,
            Placement::Whole { same_offsets } if collapsed => {
                if *same_offsets {
                    local
                } else {
                    result.len()
                }
            }
            Placement::Whole { .. } => {
                if start {
                    0
                } else {
                    result.len()
                }
            }
            Placement::Moved { origin, block } => block.map(local.saturating_sub(*origin)),
            Placement::Rows(rows) => match rows[..rows.partition_point(|row| row.source <= local)].last() {
                None => local,
                Some(row) => {
                    let column = local - row.source;
                    row.output
                        + if column == 0 && !collapsed {
                            // A selection from a line start keeps whole lines selected.
                            0
                        } else if column >= row.body {
                            row.output_body + (column - row.body)
                        } else if column >= row.body - row.suffix {
                            row.output_body - (row.body - column)
                        } else if column <= row.prefix {
                            column
                        } else {
                            row.output_body - row.suffix
                        }
                }
            },
        };
        floor_char(result, at)
    }
}
/// The largest character boundary of `text` at or before `at`.
fn floor_char(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}
/// Byte lengths of the longest common prefix of `a` and `b` and of the longest
/// common suffix of what follows it, both on character boundaries of each text.
/// Row commands edit a row's start (indentation), end (trailing blanks) or middle
/// (tab conversion); text outside that edit keeps its bytes.
fn common_affixes(a: &str, b: &str) -> (usize, usize) {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    let mut prefix = x.iter().zip(y).take_while(|(l, r)| l == r).count();
    while !(a.is_char_boundary(prefix) && b.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let limit = x.len().min(y.len()) - prefix;
    let mut suffix = x
        .iter()
        .rev()
        .zip(y.iter().rev())
        .take(limit)
        .take_while(|(l, r)| l == r)
        .count();
    while !(a.is_char_boundary(x.len() - suffix) && b.is_char_boundary(y.len() - suffix)) {
        suffix -= 1;
    }
    (prefix, suffix)
}
/// Total-order sort key for line sorting, shared by the in-memory and the
/// streaming sort so both order identically.
///
/// With `numeric` set, lines that parse as a number sort by value first (using
/// `f64::total_cmp` semantics, so `NaN` has a defined place); every line that
/// does not parse sorts lexically **after** all numeric lines. Without
/// `numeric` the key is purely lexical. The key is a plain tuple, so the order
/// is total by construction and `sort_by_cached_key` cannot panic on it.
pub(crate) fn sort_key(text: &str, case_sensitive: bool, numeric: bool) -> (bool, i64, String) {
    let folded = if case_sensitive {
        text.to_string()
    } else {
        text.to_lowercase()
    };
    if numeric {
        if let Ok(value) = folded.trim().parse::<f64>() {
            return (false, total_order_bits(value), folded);
        }
    }
    (true, 0, folded)
}

/// Maps a `f64` onto an `i64` whose natural order matches `f64::total_cmp`.
fn total_order_bits(value: f64) -> i64 {
    let bits = value.to_bits() as i64;
    bits ^ (((bits >> 63) as u64 >> 1) as i64)
}

fn split_rows(text: &str) -> Vec<(&str, &str)> {
    let mut rows = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' || bytes[i] == b'\n' {
            let end = i;
            i += 1;
            if bytes[end] == b'\r' && bytes.get(i) == Some(&b'\n') {
                i += 1;
            }
            rows.push((&text[start..end], &text[end..i]));
            start = i;
        } else {
            i += 1;
        }
    }
    if start < text.len() {
        rows.push((&text[start..], ""));
    }
    rows
}
fn strip_one_eol(s: &str) -> &str {
    s.strip_suffix("\r\n")
        .or_else(|| s.strip_suffix(['\r', '\n']))
        .unwrap_or(s)
}
/// Where a moved line block lands in the rotated text: its output start, its
/// length without the final line break, and the length of the break written
/// after it (0 when the block ends the text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MovedBlock {
    pub start: usize,
    pub body: usize,
    pub eol: usize,
}
impl MovedBlock {
    /// Output offset of `local`, an offset into the block's source text.
    pub fn map(self, local: usize) -> usize {
        self.start
            + if local <= self.body {
                local
            } else {
                self.body + self.eol
            }
    }
}
fn move_rows(selected: &str, neighbor: &str, down: bool, fallback_eol: &str) -> (String, MovedBlock) {
    let eol = split_rows(selected)
        .into_iter()
        .chain(split_rows(neighbor))
        .find(|(_, e)| !e.is_empty())
        .map_or(fallback_eol, |(_, e)| e);
    let trailing = if down {
        neighbor.ends_with(['\r', '\n'])
    } else {
        selected.ends_with(['\r', '\n'])
    };
    let (a, b) = if down {
        (neighbor, selected)
    } else {
        (selected, neighbor)
    };
    let text = format!(
        "{}{}{}{}",
        strip_one_eol(a),
        eol,
        strip_one_eol(b),
        if trailing { eol } else { "" }
    );
    let body = strip_one_eol(selected).len();
    let block = if down {
        MovedBlock {
            start: strip_one_eol(neighbor).len() + eol.len(),
            body,
            eol: if trailing { eol.len() } else { 0 },
        }
    } else {
        MovedBlock {
            start: 0,
            body,
            eol: eol.len(),
        }
    };
    (text, block)
}
pub fn add_caret(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    below: bool,
    limits: Limits,
) -> Result<SelectionSet, Error> {
    let mut out = normalize_directed(snapshot, set, limits)?;
    let p = out.primary();
    let n = snapshot.line_at(TextOffset(p.caret))?;
    let target = if below { n.checked_add(1) } else { n.checked_sub(1) }.ok_or(Error::OutOfBounds)?;
    let (start, text) = line(snapshot, n, limits)?;
    let column = DisplayColumnMap::new(content(&text), limits.tab_width).column(p.caret - start);
    let (start, text) = line(snapshot, target, limits)?;
    let p = start + DisplayColumnMap::new(content(&text), limits.tab_width).at(column).0;
    out.selections.push(Selection { anchor: p, caret: p });
    out.primary = out.selections.len() - 1;
    normalize_directed(snapshot, &out, limits)
}
pub fn select_occurrences(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    all: bool,
    limits: Limits,
) -> Result<SelectionSet, Error> {
    let mut out = normalize_directed(snapshot, set, limits)?;
    let p = out.primary();
    let primary_range = p.range();
    let needle = snapshot.read(
        TextOffset(primary_range.start)..TextOffset(primary_range.end),
        limits.max_bytes,
    )?;
    if needle.is_empty() {
        let mut start = p.caret.saturating_sub(16 * 1024);
        let mut end = p.caret.saturating_add(16 * 1024).min(snapshot.len());
        while start < p.caret && !snapshot.is_boundary(TextOffset(start)) {
            start += 1;
        }
        while end > p.caret && !snapshot.is_boundary(TextOffset(end)) {
            end -= 1;
        }
        let text = snapshot.read(TextOffset(start)..TextOffset(end), limits.max_bytes)?;
        if let Some((at, word)) = text
            .unicode_word_indices()
            .find(|(at, word)| start + at <= p.caret && p.caret < start + at + word.len())
        {
            let from = start + at;
            let to = from + word.len();
            if (from > start || start == 0) && (to < end || end == snapshot.len()) {
                out.selections[out.primary] = Selection {
                    anchor: from,
                    caret: to,
                };
            }
        }
        return Ok(out);
    }
    // Existing selections, as a sorted set: a large Select All stays n log n (EDT-14).
    let mut taken: std::collections::BTreeSet<(usize, usize)> = out
        .selections
        .iter()
        .map(|s| {
            let r = s.range();
            (r.start, r.end)
        })
        .collect();
    if all {
        let mut added = false;
        occurrences_in(snapshot, &needle, 0, snapshot.len(), limits, |r| {
            if taken.insert((r.start, r.end)) {
                out.selections.push(Selection {
                    anchor: r.start,
                    caret: r.end,
                });
                if out.selections.len() > limits.max_selections {
                    return Err(Error::BudgetExceeded);
                }
                added = true;
            }
            Ok(false)
        })?;
        if added {
            out.primary = out.selections.len() - 1;
        }
    } else {
        // Search forward from the primary, then wrap; neither reads the whole document.
        let mut found = None;
        let mut visit = |r: Range<usize>| -> Result<bool, Error> {
            if taken.contains(&(r.start, r.end)) {
                return Ok(false);
            }
            found = Some(r);
            Ok(true)
        };
        if !occurrences_in(snapshot, &needle, primary_range.end, snapshot.len(), limits, &mut visit)? {
            occurrences_in(snapshot, &needle, 0, primary_range.end, limits, &mut visit)?;
        }
        if let Some(r) = found {
            out.selections.push(Selection {
                anchor: r.start,
                caret: r.end,
            });
            out.primary = out.selections.len() - 1;
            if out.selections.len() > limits.max_selections {
                return Err(Error::BudgetExceeded);
            }
        }
    }
    // Existing selections are normalized and occurrences are grapheme-aligned, so
    // only the merge remains: no source read per selection end (P1-A7).
    Ok(merge_directed(&out.selections, out.primary))
}
/// Visits the non-overlapping occurrences of `needle` that start in `from..to`
/// and begin and end on grapheme boundaries, reading the source in bounded
/// windows. `visit` returns true to stop; the result says whether it stopped.
fn occurrences_in(
    snapshot: &DocumentSnapshot,
    needle: &str,
    from: usize,
    to: usize,
    limits: Limits,
    mut visit: impl FnMut(Range<usize>) -> Result<bool, Error>,
) -> Result<bool, Error> {
    const WINDOW: usize = 1 << 20;
    let mut start = from;
    let mut next = from;
    while start < to {
        // Windows split on grapheme boundaries, so each one segments correctly.
        let mut split = start.saturating_add(WINDOW).min(to);
        if split < to {
            while !snapshot.is_boundary(TextOffset(split)) {
                split -= 1;
            }
            split = snap(snapshot, split, limits)?;
            if split <= start {
                split = to;
            }
        }
        // Read past the split so a match starting before it is complete.
        let mut end = split.saturating_add(needle.len()).min(snapshot.len());
        while !snapshot.is_boundary(TextOffset(end)) {
            end -= 1;
        }
        let text = snapshot.read(TextOffset(start)..TextOffset(end), limits.max_bytes)?;
        let mut boundaries = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(Some(text.len()))
            .peekable();
        for (local, _) in text.match_indices(needle) {
            let at = start + local;
            if at >= split {
                break;
            }
            if at < next {
                continue;
            }
            let finish = local + needle.len();
            let mut aligned = true;
            for target in [local, finish] {
                while boundaries.next_if(|b| *b < target).is_some() {}
                aligned &= boundaries.peek() == Some(&target);
            }
            // The window end may cut a cluster; a match touching it asks the source.
            if aligned && finish == text.len() && end < snapshot.len() {
                aligned = snap(snapshot, end, limits)? == end;
            }
            if !aligned {
                continue;
            }
            next = at + needle.len();
            if visit(at..next)? {
                return Ok(true);
            }
        }
        start = split;
    }
    Ok(false)
}
/// A same-document move is one delete+insert transaction. Dropping inside the source is a no-op.
pub fn drag_text(
    snapshot: &DocumentSnapshot,
    selection: Selection,
    target: usize,
    copy: bool,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    let set = normalize(snapshot, &selection.into(), limits)?;
    let range = set.primary().range();
    let target = snap(snapshot, target, limits)?;
    if !copy && (range.start..=range.end).contains(&target) {
        return finish(snapshot, Vec::new(), limits);
    }
    let text = snapshot.read(TextOffset(range.start)..TextOffset(range.end), limits.max_bytes)?;
    let mut edits = vec![Edit {
        range: TextOffset(target)..TextOffset(target),
        insert: text,
    }];
    if !copy {
        edits.push(Edit {
            range: TextOffset(range.start)..TextOffset(range.end),
            insert: String::new(),
        });
    }
    finish(snapshot, edits, limits)
}
#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    /// 64 rows mixing plain integers, floats, signed values, `NaN`, and lines
    /// that only start with digits — the shape that made the old two-branch
    /// comparator intransitive.
    pub(super) fn mixed_sort_rows() -> Vec<String> {
        let seeds = [
            "10",
            "2",
            "12 apples",
            "1st",
            "-3.5",
            "NaN",
            "0",
            "007",
            "3",
            "banana",
            "1e3",
            "-0",
            "inf",
            "-inf",
            "42",
            "12",
            "9 lives",
            "100",
            "5.5",
            "5.50",
            "2nd place",
            "apple",
            "Zebra",
            "  7  ",
            "0.1",
            "-1",
            "1000000",
            "3.14159",
            "NaN too",
            "",
            " ",
            "0x10",
        ];
        let mut rows = Vec::with_capacity(64);
        for seed in seeds {
            rows.push(seed.to_string());
        }
        for seed in seeds {
            rows.push(seed.to_string());
        }
        rows
    }

    #[test]
    fn numeric_sort_of_sixty_four_mixed_rows_is_total_and_deterministic() {
        let rows = mixed_sort_rows();
        assert_eq!(rows.len(), 64);
        let input = rows.join("\n");
        let action = Transform::Sort {
            descending: false,
            case_sensitive: true,
            numeric: true,
        };
        let sort_once = |text: &str| {
            let document = doc(text);
            let snapshot = document.snapshot();
            let selection = Selection {
                anchor: 0,
                caret: text.len(),
            };
            let edit = transform(&snapshot, &selection.into(), action.clone(), Limits::default()).unwrap();
            let mut document = document;
            document.apply(edit.transaction).unwrap();
            self::text(&document)
        };
        let sorted = sort_once(&input);
        // Deterministic: sorting the sorted text is a fixed point.
        assert_eq!(sort_once(&sorted), sorted);
        let mut expected = rows.clone();
        expected.sort_by_cached_key(|row| sort_key(row, true, true));
        assert_eq!(sorted.split('\n').collect::<Vec<_>>(), expected);
        // Numeric lines come first and are ordered by value, non-numeric after.
        let numeric: Vec<f64> = sorted
            .split('\n')
            .map_while(|row| row.trim().parse::<f64>().ok())
            .filter(|value| !value.is_nan())
            .collect();
        assert!(numeric.windows(2).all(|pair| pair[0] <= pair[1]));
        let first_text = sorted
            .split('\n')
            .position(|row| row.trim().parse::<f64>().is_err())
            .unwrap();
        assert!(
            sorted
                .split('\n')
                .skip(first_text)
                .all(|row| row.trim().parse::<f64>().is_err())
        );
    }
    #[test]
    fn collapsed_caret_on_giant_line_uses_bounded_grapheme_context() {
        let text = format!("{}e\u{301}\u{1f642}z", "x".repeat(20 << 20));
        let document = doc(&text);
        let snapshot = document.snapshot();
        let limits = Limits {
            max_bytes: 16 * 1024,
            ..Limits::default()
        };
        for (requested, expected) in [
            (text.len(), text.len()),
            (10 << 20, 10 << 20),
            ((20 << 20) + 1, 20 << 20),
            ((20 << 20) + 4, (20 << 20) + 3),
        ] {
            let selection = Selection {
                anchor: requested,
                caret: requested,
            };
            let actual = normalize(&snapshot, &selection.into(), limits).unwrap().primary();
            assert_eq!(
                actual,
                Selection {
                    anchor: expected,
                    caret: expected
                }
            );
        }
    }
    fn doc(s: &str) -> Document {
        Document::from_utf8(s, Budget::new(64 << 20), Budget::new(64 << 20)).unwrap()
    }
    fn text(d: &Document) -> String {
        let s = d.snapshot();
        s.read(TextOffset(0)..TextOffset(s.len()), 64 << 20).unwrap()
    }
    #[test]
    fn ten_thousand_carets_one_undo_and_grapheme_delete() {
        let original = "🦀\n".repeat(10_000);
        let mut d = doc(&original);
        let selections = (0..10_000)
            .map(|n| Selection {
                anchor: n * 5 + 4,
                caret: n * 5 + 4,
            })
            .collect();
        let set = SelectionSet { selections, primary: 0 };
        let edit = replace(&d.snapshot(), &set, "•", Limits::default()).unwrap();
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), "🦀•\n".repeat(10_000));
        let edit = delete(&d.snapshot(), &edit.selections, true, Limits::default()).unwrap();
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), original);
        d.undo().unwrap();
        assert_eq!(text(&d), "🦀•\n".repeat(10_000));
        d.undo().unwrap();
        assert_eq!(text(&d), original);
        assert_eq!(d.undo(), Err(Error::EmptyHistory));
    }
    #[test]
    fn rectangle_unicode_tabs_short_lines_and_all_bases() {
        for base in [2, 8, 10, 16] {
            let mut d = doc("\t界🦀\nx\n•");
            let original = text(&d);
            let edit = column_insert(
                &d.snapshot(),
                Rectangle {
                    first_line: 0,
                    last_line: 2,
                    start_column: 2,
                    end_column: 2,
                },
                ColumnInsert::Numbers {
                    start: 9,
                    step: 1,
                    width: 4,
                    base,
                    repeat: 1,
                },
                Limits::default(),
            )
            .unwrap();
            d.apply(edit.transaction).unwrap();
            let expected = format!(
                "  {}  界🦀\nx {}\n• {}",
                number(9, base, 4).unwrap(),
                number(10, base, 4).unwrap(),
                number(11, base, 4).unwrap()
            );
            assert_eq!(text(&d), expected);
            d.undo().unwrap();
            assert_eq!(text(&d), original);
        }
    }
    #[test]
    fn overlap_incomplete_budget_and_mixed_eol() {
        let mut d = doc("b\r\na\na\r");
        let set = Selection {
            anchor: 0,
            caret: d.snapshot().len(),
        }
        .into();
        let edit = transform(&d.snapshot(), &set, Transform::RemoveDuplicates, Limits::default()).unwrap();
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), "b\r\na\r\n");
        d.undo().unwrap();
        let set = SelectionSet {
            selections: vec![Selection { anchor: 0, caret: 3 }, Selection { anchor: 1, caret: 4 }],
            primary: 1,
        };
        let edit = replace(&d.snapshot(), &set, "x", Limits::default()).unwrap();
        assert_eq!(edit.transaction.edits.len(), 1);
        let mut builder = bareline_document::DocumentBuilder::new(Budget::new(1024), Budget::new(1024)).unwrap();
        builder.append("a").unwrap();
        assert!(matches!(
            replace(&builder.prefix(), &Selection::default().into(), "x", Limits::default()),
            Err(Error::IncompleteSource)
        ));
        assert!(matches!(
            replace(
                &d.snapshot(),
                &set,
                "more",
                Limits {
                    max_bytes: 1,
                    ..Limits::default()
                }
            ),
            Err(Error::BudgetExceeded)
        ));
    }
    #[test]
    fn move_preserves_empty_rows() {
        let mut d = doc("a\n\nb\n");
        let edit = transform(
            &d.snapshot(),
            &Selection { anchor: 0, caret: 3 }.into(),
            Transform::MoveDown,
            Limits::default(),
        )
        .unwrap();
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), "b\na\n\n");
    }
    #[test]
    fn selected_block_duplicate_and_rectangle_round_trip() {
        let mut d = doc("a\r\nb\r\n");
        let edit = transform(
            &d.snapshot(),
            &Selection { anchor: 0, caret: 6 }.into(),
            Transform::Duplicate,
            Limits::default(),
        )
        .unwrap();
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), "a\r\nb\r\na\r\nb\r\n");
        let mut d = doc("\tX\n界🦀\nx");
        let rectangle = Rectangle {
            first_line: 0,
            last_line: 2,
            start_column: 2,
            end_column: 4,
        };
        let copied = rectangle_copy(&d.snapshot(), rectangle, Limits::default()).unwrap();
        assert_eq!(copied, "  \n🦀\n  ");
        let edit = rectangle_paste(&d.snapshot(), rectangle, "•\n界\n🦀", Limits::default()).unwrap();
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), "  •X\n界界\nx 🦀");
    }
    fn run(d: &mut Document, set: &SelectionSet, action: Transform) -> SelectionSet {
        let edit = transform(&d.snapshot(), set, action, Limits::default()).unwrap();
        d.apply(edit.transaction).unwrap();
        edit.selections
    }
    fn selected(d: &Document, selection: Selection) -> String {
        let range = selection.range();
        d.snapshot()
            .read(TextOffset(range.start)..TextOffset(range.end), 1 << 20)
            .unwrap()
    }
    #[test]
    fn repeated_move_carries_the_block_and_never_its_neighbours_eol() {
        for eol in ["\n", "\r\n"] {
            let lines = |order: &str| order.chars().map(|l| format!("{l}{eol}")).collect::<String>();
            let mut d = doc(&lines("abcd"));
            let c = lines("ab").len();
            // A whole-line selection of "c", including its line break.
            let mut set: SelectionSet = Selection {
                anchor: c,
                caret: c + 1 + eol.len(),
            }
            .into();
            for expected in ["acbd", "cabd"] {
                set = run(&mut d, &set, Transform::MoveUp);
                assert_eq!(text(&d), lines(expected), "{eol:?}");
                assert_eq!(selected(&d, set.primary()), format!("c{eol}"));
            }
            // At the top nothing moves and nothing is edited.
            let edit = transform(&d.snapshot(), &set, Transform::MoveUp, Limits::default()).unwrap();
            assert!(edit.transaction.edits.is_empty());
            assert_eq!(edit.selections, set);
            for expected in ["acbd", "abcd", "abdc"] {
                set = run(&mut d, &set, Transform::MoveDown);
                assert_eq!(text(&d), lines(expected), "{eol:?}");
                assert_eq!(selected(&d, set.primary()), format!("c{eol}"));
            }
            assert!(
                transform(&d.snapshot(), &set, Transform::MoveDown, Limits::default())
                    .unwrap()
                    .transaction
                    .edits
                    .is_empty()
            );
        }
        // A caret on an unterminated last line moves with it; the file stays CRLF.
        let mut d = doc("a\r\nb");
        let set = run(&mut d, &Selection { anchor: 4, caret: 4 }.into(), Transform::MoveUp);
        assert_eq!(text(&d), "b\r\na");
        assert_eq!(set.primary(), Selection { anchor: 1, caret: 1 });
        let set = run(&mut d, &set, Transform::MoveDown);
        assert_eq!(text(&d), "a\r\nb");
        assert_eq!(set.primary(), Selection { anchor: 4, caret: 4 });
    }
    #[test]
    fn repeated_duplicate_keeps_the_original_and_the_document_eol() {
        let mut d = doc("a\r\nb");
        let mut set: SelectionSet = Selection { anchor: 4, caret: 4 }.into();
        for expected in ["a\r\nb\r\nb", "a\r\nb\r\nb\r\nb"] {
            set = run(&mut d, &set, Transform::Duplicate);
            assert_eq!(text(&d), expected);
            assert_eq!(d.snapshot().eol_label(), "CRLF");
            assert_eq!(set.primary(), Selection { anchor: 4, caret: 4 });
        }
        let mut d = doc("x\ny\n");
        let mut set: SelectionSet = Selection { anchor: 0, caret: 2 }.into();
        for expected in ["x\nx\ny\n", "x\nx\nx\ny\n"] {
            set = run(&mut d, &set, Transform::Duplicate);
            assert_eq!(text(&d), expected);
            assert_eq!(selected(&d, set.primary()), "x\n");
        }
        // Splitting an unterminated row uses the document's CRLF, not LF.
        let mut d = doc("a\r\nbcdef");
        run(
            &mut d,
            &Selection { anchor: 4, caret: 4 }.into(),
            Transform::Split { column: 2 },
        );
        assert_eq!(text(&d), "a\r\nbc\r\nde\r\nf");
    }
    #[test]
    fn repeated_indent_keeps_the_selection_extent_and_direction() {
        let mut d = doc("one\r\ntwo\r\nthree\r\n");
        let mut set: SelectionSet = Selection { anchor: 0, caret: 10 }.into();
        for expected in [
            "    one\r\n    two\r\nthree\r\n",
            "        one\r\n        two\r\nthree\r\n",
        ] {
            set = run(&mut d, &set, Transform::Indent);
            assert_eq!(text(&d), expected);
            let (one, _) = expected.split_once("three").unwrap();
            assert_eq!(selected(&d, set.primary()), one);
        }
        set = run(&mut d, &set, Transform::Unindent);
        assert_eq!(selected(&d, set.primary()), "    one\r\n    two\r\n");
        // A partial, backward selection keeps its characters and its direction.
        let mut d = doc("one\ntwo\nthree\n");
        let set = run(&mut d, &Selection { anchor: 6, caret: 1 }.into(), Transform::Indent);
        assert_eq!(set.primary(), Selection { anchor: 14, caret: 5 });
        assert_eq!(selected(&d, set.primary()), "ne\n    tw");
        // A caret keeps its place in the text.
        let mut d = doc("one\n");
        let set = run(&mut d, &Selection { anchor: 2, caret: 2 }.into(), Transform::Indent);
        assert_eq!(set.primary(), Selection { anchor: 6, caret: 6 });
    }
    #[test]
    fn no_op_transforms_prepare_no_edit() {
        for (text, selection, action) in [
            ("abc\n", Selection { anchor: 1, caret: 1 }, Transform::Uppercase),
            ("a\nb\n", Selection { anchor: 0, caret: 4 }, Transform::TrimEnd),
            (
                "a\nb\n",
                Selection { anchor: 0, caret: 4 },
                Transform::Sort {
                    descending: false,
                    case_sensitive: true,
                    numeric: false,
                },
            ),
        ] {
            let d = doc(text);
            let edit = transform(&d.snapshot(), &selection.into(), action, Limits::default()).unwrap();
            assert!(edit.transaction.edits.is_empty(), "{text:?}");
            assert_eq!(edit.selections.primary(), selection);
        }
    }
    #[test]
    fn multi_caret_edits_keep_the_primary_index() {
        let d = doc("abc");
        let set = SelectionSet {
            selections: (1..=3).map(|n| Selection { anchor: n, caret: n }).collect(),
            primary: 2,
        };
        let edit = replace(&d.snapshot(), &set, "x", Limits::default()).unwrap();
        assert_eq!(edit.selections.primary, 2);
        let edit = delete(&d.snapshot(), &set, true, Limits::default()).unwrap();
        assert_eq!(edit.selections.primary, 2);
        let edit = transform(&d.snapshot(), &set, Transform::Uppercase, Limits::default()).unwrap();
        assert_eq!(edit.selections.primary(), Selection { anchor: 3, caret: 3 });
        // Producers without selection tracking recover it from the consumed caret.
        let mut prepared = finish(
            &d.snapshot(),
            (0..3)
                .map(|n| Edit {
                    range: TextOffset(n)..TextOffset(n + 1),
                    insert: String::new(),
                })
                .collect(),
            Limits::default(),
        )
        .unwrap();
        keep_primary(&mut prepared, &set);
        assert_eq!(prepared.selections.primary, 2);
    }
    #[test]
    fn normalize_directed_keeps_direction_and_tracks_the_primary() {
        let d = doc("hello world");
        let set = SelectionSet {
            selections: vec![Selection { anchor: 11, caret: 6 }, Selection { anchor: 0, caret: 5 }],
            primary: 0,
        };
        let directed = normalize_directed(&d.snapshot(), &set, Limits::default()).unwrap();
        assert_eq!(
            directed.selections,
            vec![Selection { anchor: 0, caret: 5 }, Selection { anchor: 11, caret: 6 }]
        );
        assert_eq!(directed.primary(), Selection { anchor: 11, caret: 6 });
        let forward = normalize(&d.snapshot(), &set, Limits::default()).unwrap();
        assert_eq!(forward.primary(), Selection { anchor: 6, caret: 11 });
        // A merged selection takes the direction of the primary it absorbed.
        for (primary, expected) in [
            (0, Selection { anchor: 8, caret: 1 }),
            (1, Selection { anchor: 1, caret: 8 }),
        ] {
            let set = SelectionSet {
                selections: vec![Selection { anchor: 3, caret: 1 }, Selection { anchor: 2, caret: 8 }],
                primary,
            };
            let merged = normalize_directed(&d.snapshot(), &set, Limits::default()).unwrap();
            assert_eq!(merged.selections, vec![expected]);
            assert_eq!(merged.primary, 0);
        }
    }
    #[test]
    fn rectangle_delete_never_pads_ragged_rows() {
        let rectangle = |first_line, last_line, start_column, end_column| Rectangle {
            first_line,
            last_line,
            start_column,
            end_column,
        };
        // Cut: an empty paste removes the block and leaves short rows untouched.
        let mut d = doc("abcdef\nab\n\nabcdef");
        let edit = rectangle_paste(&d.snapshot(), rectangle(0, 3, 3, 5), "", Limits::default()).unwrap();
        assert_eq!(edit.selections.selections.len(), 4);
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), "abcf\nab\n\nabcf");
        // Zero-width Backspace removes the grapheme before the column on every row
        // that reaches it, including a row ending exactly at the column.
        let mut d = doc("abcdef\nab\nx");
        let edit = rectangle_delete_mapped(
            &d.snapshot(),
            rectangle(0, 2, 2, 2),
            Some(true),
            Limits::default(),
            None,
        )
        .unwrap();
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), "acdef\na\nx");
        assert_eq!(
            edit.selections.selections,
            [1, 7, 9].map(|caret| Selection { anchor: caret, caret })
        );
        // Zero-width Delete removes the grapheme after it and never joins lines.
        let mut d = doc("a🦀c\n\nxy");
        let edit = rectangle_delete_mapped(
            &d.snapshot(),
            rectangle(0, 2, 1, 1),
            Some(false),
            Limits::default(),
            None,
        )
        .unwrap();
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), "ac\n\nx");
        // A column inside a tab removes the tab.
        let mut d = doc("\tX");
        let edit = rectangle_delete_mapped(
            &d.snapshot(),
            rectangle(0, 0, 2, 2),
            Some(true),
            Limits::default(),
            None,
        )
        .unwrap();
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), "X");
        // Without a direction, a zero-width block has nothing to remove.
        let d = doc("abc\nabc");
        let edit = rectangle_paste(&d.snapshot(), rectangle(0, 1, 1, 1), "", Limits::default()).unwrap();
        assert!(edit.transaction.edits.is_empty());
    }
    #[test]
    fn select_next_occurrence_reads_bounded_windows_past_sixteen_mebibytes() {
        let text = format!("needle{}needle", "x".repeat(17 << 20));
        let d = doc(&text);
        let first: SelectionSet = Selection { anchor: 0, caret: 6 }.into();
        let next = select_occurrences(&d.snapshot(), &first, false, Limits::default()).unwrap();
        let last = Selection {
            anchor: text.len() - 6,
            caret: text.len(),
        };
        assert_eq!(next.selections, vec![Selection { anchor: 0, caret: 6 }, last]);
        assert_eq!(next.primary(), last);
        // From the last occurrence the search wraps to the first.
        let wrapped = select_occurrences(&d.snapshot(), &last.into(), false, Limits::default()).unwrap();
        assert_eq!(wrapped.selections.len(), 2);
        let all = select_occurrences(&d.snapshot(), &first, true, Limits::default()).unwrap();
        assert_eq!(all.selections.len(), 2);
        // Matches inside a grapheme cluster are not occurrences.
        let d = doc("e e\u{301} e");
        let all = select_occurrences(
            &d.snapshot(),
            &Selection { anchor: 0, caret: 1 }.into(),
            true,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            all.selections,
            vec![Selection { anchor: 0, caret: 1 }, Selection { anchor: 6, caret: 7 }]
        );
    }
    #[test]
    fn skip_occurrence_makes_the_new_occurrence_primary() {
        let d = doc("ab ab ab");
        let set = SelectionSet {
            selections: vec![Selection { anchor: 0, caret: 2 }, Selection { anchor: 6, caret: 8 }],
            primary: 0,
        };
        let next = skip_occurrence(&d.snapshot(), &set, Limits::default()).unwrap();
        assert_eq!(
            next.selections,
            vec![Selection { anchor: 3, caret: 5 }, Selection { anchor: 6, caret: 8 }]
        );
        assert_eq!(next.primary(), Selection { anchor: 3, caret: 5 });
    }
    /// Both ends of every output selection are character boundaries of the text.
    fn assert_boundaries(d: &Document, set: &SelectionSet) {
        let text = text(d);
        for s in &set.selections {
            assert!(
                text.is_char_boundary(s.anchor) && text.is_char_boundary(s.caret),
                "{s:?} in {text:?}"
            );
        }
    }
    fn carets(offsets: &[usize], primary: usize) -> SelectionSet {
        SelectionSet {
            selections: offsets.iter().map(|&n| Selection { anchor: n, caret: n }).collect(),
            primary,
        }
    }
    #[test]
    fn row_commands_keep_carets_on_character_boundaries() {
        // An internal tab widens, then narrows, the text before the caret (between 日 and 本).
        let mut d = doc("x\t日本");
        let set = run(&mut d, &carets(&[5], 0), Transform::TabsToSpaces);
        assert_eq!(text(&d), "x   日本");
        assert_boundaries(&d, &set);
        assert_eq!(set, carets(&[7], 0));
        let set = run(&mut d, &set, Transform::SpacesToTabs);
        assert_eq!(text(&d), "x\t日本");
        assert_boundaries(&d, &set);
        assert_eq!(set, carets(&[5], 0));
        // Trim strips Unicode blanks (NBSP, U+3000) that indentation does not count.
        let mut d = doc("\u{a0}日本\n\u{3000}x\u{3000}");
        let set = run(&mut d, &carets(&[5, 13], 1), Transform::Trim);
        assert_eq!(text(&d), "日本\nx");
        assert_boundaries(&d, &set);
        assert_eq!(set, carets(&[3, 8], 1));
        // Carets on adjacent rows each keep their own text through one indent.
        let mut d = doc("日本\n語x\n");
        let set = run(&mut d, &carets(&[3, 10], 0), Transform::Indent);
        assert_eq!(text(&d), "    日本\n    語x\n");
        assert_boundaries(&d, &set);
        assert_eq!(set, carets(&[7, 18], 0));
    }
    #[test]
    fn reordering_commands_put_carets_after_the_rewritten_rows() {
        let sort = Transform::Sort {
            descending: false,
            case_sensitive: true,
            numeric: false,
        };
        // Offsets kept from the source would fall inside a multi-byte character.
        for (text_before, before, action, text_after, expected) in [
            ("b日\na\n", &[4, 5][..], sort, "a\nb日\n", 7),
            ("日本語", &[6][..], Transform::Split { column: 1 }, "日\n本\n語", 11),
            ("\n日本", &[0, 4][..], Transform::RemoveEmpty, "日本", 6),
            ("日\r\n本", &[0, 5][..], Transform::Join, "日 本", 7),
        ] {
            let mut d = doc(text_before);
            let set = run(&mut d, &carets(before, 0), action);
            assert_eq!(text(&d), text_after);
            assert_boundaries(&d, &set);
            assert_eq!(set, carets(&[expected], 0), "{text_before:?}");
        }
        // Case mapping of the same length keeps a caret merged into the range in place.
        let mut d = doc("abcd");
        let set = SelectionSet {
            selections: vec![Selection { anchor: 0, caret: 3 }, Selection { anchor: 3, caret: 3 }],
            primary: 1,
        };
        let set = run(&mut d, &set, Transform::Uppercase);
        assert_eq!(text(&d), "ABCd");
        assert_eq!(set.primary(), Selection { anchor: 3, caret: 3 });
    }
    fn select_all_of(count: usize) -> SelectionSet {
        let d = doc(&"ab ".repeat(count));
        select_occurrences(
            &d.snapshot(),
            &Selection { anchor: 0, caret: 2 }.into(),
            true,
            Limits::default(),
        )
        .unwrap()
    }
    #[test]
    fn select_all_occurrences_takes_a_hundred_thousand_matches() {
        let all = select_all_of(100_000);
        assert_eq!(all.selections.len(), 100_000);
        assert_eq!(
            all.primary(),
            Selection {
                anchor: 299_997,
                caret: 299_999
            }
        );
    }
    #[test]
    #[ignore = "timing budget (P1-A7); run with `cargo test --release -- --ignored`"]
    fn select_all_occurrences_of_a_hundred_thousand_matches_takes_under_a_second() {
        let started = std::time::Instant::now();
        assert_eq!(select_all_of(100_000).selections.len(), 100_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}

/// Opt-in process memory only; no serialization or filesystem interface.
#[derive(Default)]
pub struct ClipboardHistory {
    enabled: bool,
    entries: std::collections::VecDeque<String>,
    bytes: usize,
}
impl ClipboardHistory {
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.entries = std::collections::VecDeque::new();
            self.bytes = 0;
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn entries(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn admit(&mut self, text: &str) -> Result<(), Error> {
        self.admit_with_limits(text, 20, 16 << 20, 4 << 20)
    }
    pub fn admit_with_limits(&mut self, text: &str, count: usize, total: usize, entry: usize) -> Result<(), Error> {
        if !self.enabled {
            return Ok(());
        }
        let count = count.min(20);
        if count == 0 || text.len() > entry.min(4 << 20) || text.len() > total.min(16 << 20) {
            return Err(Error::BudgetExceeded);
        }
        while self.entries.len() >= count || self.bytes + text.len() > total.min(16 << 20) {
            if let Some(old) = self.entries.pop_back() {
                self.bytes -= old.len();
            } else {
                break;
            }
        }
        self.bytes += text.len();
        self.entries.push_front(text.to_owned());
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentTokens {
    pub line: Option<String>,
    pub block: Option<(String, String)>,
}
pub trait CommentProvider {
    fn tokens_for(&self, document: &DocumentSnapshot) -> Option<CommentTokens>;
}
pub struct NoComments;
impl CommentProvider for NoComments {
    fn tokens_for(&self, _: &DocumentSnapshot) -> Option<CommentTokens> {
        None
    }
}
pub fn comment_tokens(
    provider: &impl CommentProvider,
    document: &DocumentSnapshot,
) -> Result<CommentTokens, &'static str> {
    provider
        .tokens_for(document)
        .ok_or("no comment definition for this language")
}
/// Ephemeral line-start text anchors, rebased through the committed transaction.
#[derive(Clone, Debug, Default)]
pub struct Bookmarks {
    pub anchors: std::collections::BTreeSet<usize>,
}
impl Bookmarks {
    pub fn toggle(&mut self, snapshot: &DocumentSnapshot, offset: usize) -> Result<(), Error> {
        let start = snapshot.line_range(snapshot.line_at(TextOffset(offset))?)?.start.0;
        if !self.anchors.remove(&start) {
            self.anchors.insert(start);
        }
        Ok(())
    }
    pub fn clear(&mut self) {
        self.anchors.clear();
    }
    pub fn next(&self, offset: usize, backward: bool) -> Option<usize> {
        if backward {
            self.anchors
                .range(..offset)
                .next_back()
                .or_else(|| self.anchors.last())
                .copied()
        } else {
            self.anchors
                .range(offset.saturating_add(1)..)
                .next()
                .or_else(|| self.anchors.first())
                .copied()
        }
    }
    pub fn map_edits(&mut self, transaction: &EditTransaction) {
        self.anchors = self
            .anchors
            .iter()
            .map(|&anchor| {
                let mut delta = 0isize;
                for e in &transaction.edits {
                    if e.range.end.0 <= anchor {
                        delta += e.insert.len() as isize - (e.range.end.0 - e.range.start.0) as isize;
                    } else if e.range.start.0 <= anchor {
                        return e.range.start.0.saturating_add_signed(delta);
                    }
                }
                anchor.saturating_add_signed(delta)
            })
            .collect();
    }
    pub fn selections(&self, snapshot: &DocumentSnapshot, limits: Limits) -> Result<SelectionSet, Error> {
        if self.anchors.len() > limits.max_selections {
            return Err(Error::BudgetExceeded);
        }
        let mut selections = Vec::new();
        for &anchor in &self.anchors {
            let r = snapshot.line_range(snapshot.line_at(TextOffset(anchor))?)?;
            selections.push(Selection {
                anchor: r.start.0,
                caret: r.end.0,
            });
        }
        if selections.is_empty() {
            return Err(Error::OutOfBounds);
        }
        normalize_directed(snapshot, &SelectionSet { selections, primary: 0 }, limits)
    }
}
#[cfg(test)]
mod metadata_tests {
    use super::*;
    #[test]
    fn clipboard_opt_in_caps_and_forgets() {
        let mut h = ClipboardHistory::default();
        h.admit("ignored").unwrap();
        assert_eq!(h.entries().count(), 0);
        h.set_enabled(true);
        for n in 0..22 {
            h.admit(&n.to_string()).unwrap();
        }
        assert_eq!(h.entries().count(), 20);
        assert!(h.admit(&"x".repeat((4 << 20) + 1)).is_err());
        for _ in 0..5 {
            h.admit(&"x".repeat(4 << 20)).unwrap();
        }
        assert_eq!(h.bytes(), 16 << 20);
        h.set_enabled(false);
        assert_eq!(h.bytes(), 0);
        assert_eq!(h.entries().count(), 0);
    }
    #[test]
    fn occurrence_history_keeps_the_latest_sixty_four_steps() {
        let mut history = OccurrenceHistory::default();
        for n in 0..100 {
            history
                .remember(&Selection { anchor: n, caret: n }.into(), Limits::default())
                .unwrap();
        }
        assert_eq!(history.len(), OccurrenceHistory::MAX_ENTRIES);
        assert_eq!(history.undo().unwrap().primary(), Selection { anchor: 99, caret: 99 });
        // The selection budget evicts old steps instead of refusing new ones.
        let limits = Limits {
            max_selections: 2,
            ..Limits::default()
        };
        let pair = SelectionSet {
            selections: vec![Selection { anchor: 0, caret: 0 }, Selection { anchor: 1, caret: 1 }],
            primary: 0,
        };
        let mut history = OccurrenceHistory::default();
        history.remember(&pair, limits).unwrap();
        history.remember(&pair, limits).unwrap();
        assert_eq!(history.len(), 1);
        history.clear();
        assert!(history.is_empty());
    }
    #[test]
    fn partial_tab_and_excessive_width() {
        let d = bareline_document::Document::from_utf8(
            "\tX",
            bareline_document::Budget::new(1024),
            bareline_document::Budget::new(1024),
        )
        .unwrap();
        let e = column_insert(
            &d.snapshot(),
            Rectangle {
                first_line: 0,
                last_line: 0,
                start_column: 1,
                end_column: 2,
            },
            ColumnInsert::Text("Z".into()),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(e.transaction.edits[0].insert, " Z  ");
        assert!(matches!(
            transform(
                &d.snapshot(),
                &Selection::default().into(),
                Transform::TabsToSpaces,
                Limits {
                    max_bytes: 16,
                    tab_width: 1 << 30,
                    ..Limits::default()
                }
            ),
            Err(Error::BudgetExceeded)
        ));
        assert!(comment_tokens(&NoComments, &d.snapshot()).is_err());
    }
}
/// Plain-text rows are sufficient for deterministic rectangle paste; a single row repeats.
pub fn rectangle_paste(
    snapshot: &DocumentSnapshot,
    rectangle: Rectangle,
    text: &str,
    limits: Limits,
) -> Result<PowerEdit, Error> {
    rectangle_paste_mapped(snapshot, rectangle, text, limits, None)
}
pub fn rectangle_paste_mapped(
    snapshot: &DocumentSnapshot,
    rectangle: Rectangle,
    text: &str,
    limits: Limits,
    maps: Option<&std::collections::BTreeMap<usize, DisplayColumnMap>>,
) -> Result<PowerEdit, Error> {
    if text.len() > limits.max_bytes {
        return Err(Error::BudgetExceeded);
    }
    if text.is_empty() {
        // Cut and an empty paste remove the block; they must not pad short rows.
        return rectangle_delete_mapped(snapshot, rectangle, None, limits, maps);
    }
    let rows = clipboard_rows(text);
    let count = rectangle
        .last_line
        .checked_sub(rectangle.first_line)
        .and_then(|n| n.checked_add(1))
        .ok_or(Error::OutOfBounds)?;
    if count > limits.max_selections {
        return Err(Error::BudgetExceeded);
    }
    if rows.len() > 1 && rows.len() != count {
        return Err(Error::OutOfBounds);
    }
    let mut edits = Vec::new();
    let mut total = 0;
    for row in 0..count {
        let value = rows.get(if rows.len() > 1 { row } else { 0 }).map_or("", |s| *s);
        let projected = column_insert_mapped(
            snapshot,
            Rectangle {
                first_line: rectangle.first_line + row,
                last_line: rectangle.first_line + row,
                ..rectangle
            },
            ColumnInsert::Text(value.into()),
            limits,
            maps,
        )?;
        for edit in &projected.transaction.edits {
            charge(&mut total, edit.insert.len(), limits)?;
            charge(&mut total, edit.range.end.0 - edit.range.start.0, limits)?;
        }
        edits.extend(projected.transaction.edits);
    }
    finish(snapshot, edits, limits)
}
pub fn rectangle_copy(snapshot: &DocumentSnapshot, rectangle: Rectangle, limits: Limits) -> Result<String, Error> {
    rectangle_copy_mapped(snapshot, rectangle, limits, None)
}
pub fn rectangle_copy_mapped(
    snapshot: &DocumentSnapshot,
    rectangle: Rectangle,
    limits: Limits,
    maps: Option<&std::collections::BTreeMap<usize, DisplayColumnMap>>,
) -> Result<String, Error> {
    if !snapshot.is_complete() {
        return Err(Error::IncompleteSource);
    }
    if rectangle.first_line > rectangle.last_line || rectangle.last_line - rectangle.first_line >= limits.max_selections
    {
        return Err(Error::BudgetExceeded);
    }
    let left = rectangle.start_column.min(rectangle.end_column);
    let right = rectangle.start_column.max(rectangle.end_column);
    let mut output = String::new();
    let mut bytes = 0;
    for n in rectangle.first_line..=rectangle.last_line {
        let (_, text) = line(snapshot, n, limits)?;
        let body = content(&text);
        let fallback;
        let map = if let Some(maps) = maps {
            maps.get(&n).ok_or(Error::OutOfBounds)?
        } else {
            fallback = DisplayColumnMap::new(body, limits.tab_width);
            &fallback
        };
        if n > rectangle.first_line {
            charge(&mut bytes, 1, limits)?;
            output.push('\n');
        }
        for pair in map.stops.windows(2) {
            let (a, ca) = pair[0];
            let (b, cb) = pair[1];
            if ca >= right || cb <= left {
                continue;
            }
            if &body[a..b] == "\t" {
                let spaces = cb.min(right) - ca.max(left);
                charge(&mut bytes, spaces, limits)?;
                output.extend(std::iter::repeat_n(' ', spaces));
            } else {
                charge(&mut bytes, b - a, limits)?;
                output.push_str(&body[a..b]);
            }
        }
        let last = map.stops.last().map_or(0, |(_, c)| *c);
        let pad = right.saturating_sub(last.max(left));
        charge(&mut bytes, pad, limits)?;
        output.extend(std::iter::repeat_n(' ', pad));
    }
    Ok(output)
}
pub fn toggle_caret(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    offset: usize,
    limits: Limits,
) -> Result<SelectionSet, Error> {
    let mut out = normalize_directed(snapshot, set, limits)?;
    let p = snap(snapshot, offset, limits)?;
    if let Some(i) = out.selections.iter().position(|s| s.anchor == p && s.caret == p) {
        if out.selections.len() > 1 {
            out.selections.remove(i);
            out.primary = out.primary.min(out.selections.len() - 1);
        }
    } else {
        out.selections.push(Selection { anchor: p, caret: p });
        out.primary = out.selections.len() - 1;
    }
    normalize_directed(snapshot, &out, limits)
}
pub fn expand_lines(snapshot: &DocumentSnapshot, set: &SelectionSet, limits: Limits) -> Result<SelectionSet, Error> {
    let mut out = normalize_directed(snapshot, set, limits)?;
    for s in &mut out.selections {
        let range = s.range();
        let backward = s.anchor > s.caret;
        let start = snapshot.line_at(TextOffset(range.start))?;
        let mut last = if range.end > range.start {
            range.end - 1
        } else {
            range.end
        };
        while !snapshot.is_boundary(TextOffset(last)) {
            last -= 1;
        }
        let end = snapshot.line_at(TextOffset(last))?;
        let first = snapshot.line_range(start)?.start.0;
        let last = snapshot.line_range(end)?.end.0;
        s.anchor = if backward { last } else { first };
        s.caret = if backward { first } else { last };
    }
    normalize_directed(snapshot, &out, limits)
}
pub fn transform_for_command(id: &str) -> Option<Transform> {
    Some(match id {
        "editor.case.upper" => Transform::Uppercase,
        "editor.case.lower" => Transform::Lowercase,
        "editor.case.title" => Transform::Titlecase,
        "editor.case.invert" => Transform::InvertCase,
        "editor.whitespace.trimStart" => Transform::TrimStart,
        "editor.whitespace.trimEnd" => Transform::TrimEnd,
        "editor.whitespace.trim" => Transform::Trim,
        "editor.indent" => Transform::Indent,
        "editor.unindent" => Transform::Unindent,
        "editor.tabs.toSpaces" => Transform::TabsToSpaces,
        "editor.spaces.toTabs" => Transform::SpacesToTabs,
        "editor.lines.duplicate" => Transform::Duplicate,
        "editor.selection.duplicate" => Transform::DuplicateSelections,
        "editor.lines.moveUp" => Transform::MoveUp,
        "editor.lines.moveDown" => Transform::MoveDown,
        "editor.lines.join" => Transform::Join,
        "editor.lines.split" => Transform::Split { column: 80 },
        "editor.lines.sortAscending" => Transform::Sort {
            descending: false,
            case_sensitive: true,
            numeric: false,
        },
        "editor.lines.sortDescending" => Transform::Sort {
            descending: true,
            case_sensitive: true,
            numeric: false,
        },
        "editor.lines.sortNumeric" => Transform::Sort {
            descending: false,
            case_sensitive: true,
            numeric: true,
        },
        "editor.lines.sortIgnoreCase" => Transform::Sort {
            descending: false,
            case_sensitive: false,
            numeric: false,
        },
        "editor.lines.removeDuplicates" => Transform::RemoveDuplicates,
        "editor.lines.removeConsecutiveDuplicates" => Transform::RemoveConsecutiveDuplicates,
        "editor.lines.removeEmpty" => Transform::RemoveEmpty,
        "editor.lines.removeBlank" => Transform::RemoveBlank,
        _ => return None,
    })
}
pub fn register_commands(registry: &mut bareline_commands::CommandRegistry) {
    use bareline_commands::{Action, CommandId, CommandSpec};
    for (id, title, shortcut) in [
        ("editor.caret.above", "Add Caret Above", "Ctrl+Alt+Up"),
        ("editor.caret.below", "Add Caret Below", "Ctrl+Alt+Down"),
        ("editor.selection.nextOccurrence", "Select Next Occurrence", "Ctrl+D"),
        (
            "editor.selection.allOccurrences",
            "Select All Occurrences",
            "Ctrl+Shift+L",
        ),
        ("editor.selection.skipOccurrence", "Skip Occurrence", ""),
        ("editor.selection.undoOccurrence", "Undo Added Occurrence", ""),
        ("editor.selection.rotatePrimary", "Rotate Primary Selection", ""),
        ("editor.selection.expandLines", "Expand to Lines", ""),
        ("editor.selection.escape", "Keep Primary Selection", "Escape"),
        ("editor.column.insert", "Column Editor…", "Alt+C"),
        ("editor.lines.hide", "Hide Selected Lines", ""),
        ("editor.lines.showAll", "Show Hidden Lines", ""),
        ("editor.paste.plainText", "Paste Plain Text", "Ctrl+Shift+V"),
        ("editor.paste.fromHistory", "Paste from History…", ""),
        ("editor.clipboard.toggleHistory", "Toggle Clipboard History", ""),
        ("editor.comment.toggleLine", "Toggle Line Comment", "Ctrl+/"),
        ("editor.comment.toggleBlock", "Toggle Block Comment", "Ctrl+Shift+/"),
        ("editor.bookmark.toggle", "Toggle Bookmark", "Ctrl+F2"),
        ("editor.bookmark.next", "Next Bookmark", "F2"),
        ("editor.bookmark.previous", "Previous Bookmark", "Shift+F2"),
        ("editor.bookmark.clear", "Clear Bookmarks", ""),
        ("editor.bookmark.selectLines", "Select Bookmarked Lines", ""),
        ("editor.case.upper", "Uppercase", ""),
        ("editor.case.lower", "Lowercase", ""),
        ("editor.case.title", "Title Case", ""),
        ("editor.case.invert", "Invert Case", ""),
        ("editor.whitespace.trimStart", "Trim Leading Whitespace", ""),
        ("editor.whitespace.trimEnd", "Trim Trailing Whitespace", ""),
        ("editor.whitespace.trim", "Trim Whitespace", ""),
        ("editor.indent", "Indent", "Tab"),
        ("editor.unindent", "Unindent", "Shift+Tab"),
        ("editor.tabs.toSpaces", "Convert Tabs to Spaces", ""),
        ("editor.spaces.toTabs", "Convert Spaces to Tabs", ""),
        ("editor.lines.duplicate", "Duplicate Lines", "Ctrl+Shift+D"),
        ("editor.selection.duplicate", "Duplicate Selections", ""),
        ("editor.lines.moveUp", "Move Lines Up", "Alt+Up"),
        ("editor.lines.moveDown", "Move Lines Down", "Alt+Down"),
        ("editor.lines.join", "Join Lines", "Ctrl+J"),
        ("editor.lines.split", "Split Lines at 80 Columns", ""),
        ("editor.lines.sortAscending", "Sort Lines Ascending", ""),
        ("editor.lines.sortDescending", "Sort Lines Descending", ""),
        ("editor.lines.sortNumeric", "Sort Lines Numerically", ""),
        ("editor.lines.sortIgnoreCase", "Sort Lines Ignoring Case", ""),
        ("editor.lines.removeDuplicates", "Remove Duplicate Lines", ""),
        (
            "editor.lines.removeConsecutiveDuplicates",
            "Remove Consecutive Duplicate Lines",
            "",
        ),
        ("editor.lines.removeEmpty", "Remove Empty Lines", ""),
        ("editor.lines.removeBlank", "Remove Blank Lines", ""),
    ] {
        let id = CommandId(id);
        if registry.dispatch(id).is_none() {
            let _ = registry.register(CommandSpec {
                id,
                title,
                category: "Edit",
                shortcut,
                action: Action::Contributed(id),
            });
        }
    }
}
/// Prepares the two halves only. A linked DocumentService coordinator must validate
/// both document identities/revisions and commit or undo these atomically.
pub struct CrossDocumentDrag {
    pub source_snapshot: DocumentSnapshot,
    pub target_snapshot: DocumentSnapshot,
    pub source: PowerEdit,
    pub target: PowerEdit,
}
pub fn cross_document_drag(
    source: &DocumentSnapshot,
    selection: Selection,
    target: &DocumentSnapshot,
    offset: usize,
    limits: Limits,
) -> Result<CrossDocumentDrag, Error> {
    if source.same_document(target) {
        return Err(Error::WrongDocument);
    }
    let selections = normalize(source, &selection.into(), limits)?;
    let r = selections.primary().range();
    let text = source.read(TextOffset(r.start)..TextOffset(r.end), limits.max_bytes)?;
    let source_edit = replace(source, &selections, "", limits)?;
    let target_edit = replace(
        target,
        &Selection {
            anchor: offset,
            caret: offset,
        }
        .into(),
        &text,
        limits,
    )?;
    Ok(CrossDocumentDrag {
        source_snapshot: source.clone(),
        target_snapshot: target.clone(),
        source: source_edit,
        target: target_edit,
    })
}
/// Retains exact previous selection sets for occurrence undo, independent of byte undo.
/// Only the latest [`OccurrenceHistory::MAX_ENTRIES`] steps are kept, and the owner
/// clears it on every edit and on Escape (EDT-13): the oldest entries are dropped
/// instead of refusing further Select Next.
#[derive(Default)]
pub struct OccurrenceHistory {
    previous: std::collections::VecDeque<SelectionSet>,
    count: usize,
}
impl OccurrenceHistory {
    pub const MAX_ENTRIES: usize = 64;
    pub fn remember(&mut self, set: &SelectionSet, limits: Limits) -> Result<(), Error> {
        if set.selections.len() > limits.max_selections {
            return Err(Error::BudgetExceeded);
        }
        while self.previous.len() >= Self::MAX_ENTRIES || self.count + set.selections.len() > limits.max_selections {
            let Some(oldest) = self.previous.pop_front() else {
                break;
            };
            self.count -= oldest.selections.len();
        }
        self.count += set.selections.len();
        self.previous.push_back(set.clone());
        Ok(())
    }
    pub fn undo(&mut self) -> Option<SelectionSet> {
        let set = self.previous.pop_back()?;
        self.count -= set.selections.len();
        Some(set)
    }
    pub fn len(&self) -> usize {
        self.previous.len()
    }
    pub fn is_empty(&self) -> bool {
        self.previous.is_empty()
    }
    pub fn clear(&mut self) {
        self.previous.clear();
        self.count = 0;
    }
}
pub fn skip_occurrence(snapshot: &DocumentSnapshot, set: &SelectionSet, limits: Limits) -> Result<SelectionSet, Error> {
    let normalized = normalize_directed(snapshot, set, limits)?;
    let old = normalized.primary();
    let mut next = select_occurrences(snapshot, &normalized, false, limits)?;
    if next.selections.len() > normalized.selections.len() {
        let added = next.primary();
        next.selections.retain(|s| *s != old);
        next.primary = next.selections.iter().position(|s| *s == added).unwrap_or(0);
    }
    normalize_directed(snapshot, &next, limits)
}

/// Clipboard rows retain a final empty row, unlike line-transform terminator parsing.
fn clipboard_rows(text: &str) -> Vec<&str> {
    let mut rows: Vec<_> = split_rows(text).into_iter().map(|(body, _)| body).collect();
    if text.is_empty() || text.ends_with(['\r', '\n']) {
        rows.push("");
    }
    rows
}
