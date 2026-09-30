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
/// Reads the bounded 4 KiB chunk before (`backwards`) or after `at`, snapped
/// inward to scalar boundaries and charged against the command budget.
fn bounded_chunk(
    snapshot: &DocumentSnapshot,
    at: usize,
    backwards: bool,
    total: &mut usize,
    limits: Limits,
) -> Result<(usize, String), Error> {
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
    charge(total, end - start, limits)?;
    snapshot
        .read(TextOffset(start)..TextOffset(end), limits.max_bytes)
        .map(|text| (start, text))
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
    let (mut start, mut text) = bounded_chunk(snapshot, boundary, false, &mut total, limits)?;
    loop {
        match cursor.is_boundary(&text, start) {
            Ok(true) => return Ok(boundary),
            Ok(false) => break,
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (from, context) = bounded_chunk(snapshot, end, true, &mut total, limits)?;
                cursor.provide_context(&context, from);
            }
            Err(_) => return Err(Error::InvalidBoundary),
        }
    }
    loop {
        match cursor.prev_boundary(&text, start) {
            Ok(result) => return Ok(result.unwrap_or(0)),
            Err(GraphemeIncomplete::PrevChunk) => {
                (start, text) = bounded_chunk(snapshot, start, true, &mut total, limits)?;
            }
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (from, context) = bounded_chunk(snapshot, end, true, &mut total, limits)?;
                cursor.provide_context(&context, from);
            }
            Err(_) => return Err(Error::InvalidBoundary),
        }
    }
}
/// The neighboring extended-grapheme boundary of the boundary `origin`, streamed
/// through bounded chunks so a huge logical line is never materialized. Line
/// terminators are clusters of their own, so this crosses lines as Backspace and
/// Delete expect. Returns `origin` at the document edges.
fn grapheme_step(snapshot: &DocumentSnapshot, origin: usize, forward: bool, limits: Limits) -> Result<usize, Error> {
    use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};
    let mut cursor = GraphemeCursor::new(origin, snapshot.len(), true);
    let mut total = 0;
    let (mut start, mut text) = bounded_chunk(snapshot, origin, !forward, &mut total, limits)?;
    loop {
        let result = if forward {
            cursor.next_boundary(&text, start)
        } else {
            cursor.prev_boundary(&text, start)
        };
        match result {
            Ok(result) => return Ok(result.unwrap_or(origin)),
            Err(GraphemeIncomplete::NextChunk) => {
                (start, text) = bounded_chunk(snapshot, start + text.len(), false, &mut total, limits)?;
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                (start, text) = bounded_chunk(snapshot, start, true, &mut total, limits)?;
            }
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (from, context) = bounded_chunk(snapshot, end, true, &mut total, limits)?;
                cursor.provide_context(&context, from);
            }
            Err(GraphemeIncomplete::InvalidOffset) => return Err(Error::InvalidBoundary),
        }
    }
}
/// Visits the extended graphemes of `start..end` in order through bounded chunks,
/// carrying the last (possibly incomplete) cluster into the next chunk so a huge
/// logical line is never materialized. `visit` returns false to stop early.
fn walk_graphemes(
    snapshot: &DocumentSnapshot,
    start: usize,
    end: usize,
    limits: Limits,
    mut visit: impl FnMut(usize, &str) -> bool,
) -> Result<(), Error> {
    let mut total = 0;
    let mut carry = String::new();
    let mut carry_start = start;
    let mut at = start;
    while at < end {
        let mut next = at.saturating_add(4096).min(end);
        while !snapshot.is_boundary(TextOffset(next)) {
            next -= 1;
        }
        charge(&mut total, next - at, limits)?;
        carry.push_str(&snapshot.read(TextOffset(at)..TextOffset(next), limits.max_bytes)?);
        at = next;
        let mut consumed = 0;
        let mut graphemes = carry.grapheme_indices(true).peekable();
        while let Some((index, grapheme)) = graphemes.next() {
            // The final cluster of a chunk may continue in the next one.
            if at < end && graphemes.peek().is_none() {
                break;
            }
            if !visit(carry_start + index, grapheme) {
                return Ok(());
            }
            consumed = index + grapheme.len();
        }
        carry.drain(..consumed);
        carry_start += consumed;
    }
    Ok(())
}
fn is_line_break(grapheme: &str) -> bool {
    grapheme.starts_with(['\r', '\n'])
}
/// Normalizes editing ranges; overlapping selections and duplicate carets mutate once.
pub fn normalize(snapshot: &DocumentSnapshot, set: &SelectionSet, limits: Limits) -> Result<SelectionSet, Error> {
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
    let primary = snap(snapshot, set.primary().caret, limits)?;
    let mut ranges = Vec::with_capacity(set.selections.len());
    for s in &set.selections {
        let a = snap(snapshot, s.anchor, limits)?;
        let b = snap(snapshot, s.caret, limits)?;
        ranges.push(a.min(b)..a.max(b));
    }
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        if let Some(last) = merged.last_mut()
            && (r.start < last.end || r.start == last.start)
        {
            last.end = last.end.max(r.end);
            continue;
        }
        merged.push(r);
    }
    let primary = merged
        .iter()
        .position(|r| r.contains(&primary) || r.end == primary)
        .unwrap_or(0);
    Ok(SelectionSet {
        selections: merged
            .into_iter()
            .map(|r| Selection {
                anchor: r.start,
                caret: r.end,
            })
            .collect(),
        primary,
    })
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
    for s in set.selections {
        charge(&mut total, text.len(), limits)?;
        edits.push(Edit {
            range: TextOffset(s.anchor)..TextOffset(s.caret),
            insert: text.into(),
        });
    }
    finish(snapshot, edits, limits)
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
        if backward {
            s.anchor = grapheme_step(snapshot, s.caret, false, limits)?;
        } else {
            s.caret = grapheme_step(snapshot, s.caret, true, limits)?;
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
        Self::with_metrics(text, tab_width, fallback_width)
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
fn fallback_width(g: &str) -> usize {
    if g.chars().any(|c|matches!(c as u32,0x1100..=0x115f|0x2e80..=0xa4cf|0xac00..=0xd7a3|0xf900..=0xfaff|0xfe10..=0xfe6f|0xff01..=0xff60|0x1f000..=0x1faff|0x20000..=0x3ffff)){2}else{1}
}
/// Fallback display width of one cluster at `column`, counted as `DisplayColumnMap::new` does.
fn cluster_width(g: &str, column: usize, tab_width: usize) -> usize {
    if g == "\t" {
        tab_width.max(1) - column % tab_width.max(1)
    } else {
        fallback_width(g)
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
    let set = normalize(snapshot, set, limits)?;
    let linewise = !matches!(
        action,
        Transform::Uppercase
            | Transform::Lowercase
            | Transform::Titlecase
            | Transform::InvertCase
            | Transform::DuplicateSelections
    );
    let mut ranges = Vec::<Range<usize>>::new();
    for s in &set.selections {
        let mut r = s.range();
        if linewise {
            let first = snapshot.line_at(TextOffset(r.start))?;
            let last = snapshot
                .line_at(TextOffset(if r.end > r.start { r.end - 1 } else { r.end }))
                .or_else(|_| snapshot.line_at(TextOffset(snap(snapshot, r.end.saturating_sub(1), limits)?)))?;
            r = snapshot.line_range(first)?.start.0..snapshot.line_range(last)?.end.0;
        }
        if let Some(prev) = ranges.last_mut()
            && r.start <= prev.end
        {
            prev.end = prev.end.max(r.end);
            continue;
        }
        ranges.push(r);
    }
    let mut edits = Vec::new();
    let mut total = 0;
    for mut r in ranges {
        let mut source = snapshot.read(TextOffset(r.start)..TextOffset(r.end), limits.max_bytes)?;
        charge(&mut total, source.len(), limits)?;
        if matches!(action, Transform::MoveUp | Transform::MoveDown) {
            let first = snapshot.line_at(TextOffset(r.start))?;
            if matches!(action, Transform::MoveUp) {
                if first == 0 {
                    continue;
                }
                let (start, previous) = line(snapshot, first - 1, limits)?;
                r.start = start;
                source = move_rows(&source, &previous, false);
            } else {
                let next = snapshot.line_at(TextOffset(r.end))?;
                if r.end == snapshot.len() {
                    continue;
                }
                let (start, following) = line(snapshot, next, limits)?;
                r.end = start + following.len();
                source = move_rows(&source, &following, true);
            }
            edits.push(Edit {
                range: TextOffset(r.start)..TextOffset(r.end),
                insert: source,
            });
            continue;
        }
        let result = match &action {
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
                    .map_or("\n", |(_, e)| e);
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
                    .map_or("\n", |(_, e)| *e)
                    .to_string();
                let trailing = rows.last().is_some_and(|(_, e)| !e.is_empty());
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
                    let body = match &action {
                        Transform::TrimStart => body.trim_start().to_string(),
                        Transform::TrimEnd => body.trim_end().to_string(),
                        Transform::Trim => body.trim().to_string(),
                        Transform::Indent => format!("{}{}", " ".repeat(limits.tab_width.min(limits.max_bytes)), body),
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
                out
            }
        };
        charge(&mut total, result.len(), limits)?;
        edits.push(Edit {
            range: TextOffset(r.start)..TextOffset(r.end),
            insert: result,
        });
    }
    finish(snapshot, edits, limits)
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
fn move_rows(selected: &str, neighbor: &str, down: bool) -> String {
    let eol = split_rows(selected)
        .into_iter()
        .chain(split_rows(neighbor))
        .find(|(_, e)| !e.is_empty())
        .map_or("\n", |(_, e)| e);
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
    format!(
        "{}{}{}{}",
        strip_one_eol(a),
        eol,
        strip_one_eol(b),
        if trailing { eol } else { "" }
    )
}
/// Line prefix, in bytes, up to which an added caret keeps the display column.
const ADD_CARET_WINDOW: usize = 64 << 10;
pub fn add_caret(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    below: bool,
    limits: Limits,
) -> Result<SelectionSet, Error> {
    let mut out = normalize(snapshot, set, limits)?;
    let p = out.primary();
    let n = snapshot.line_at(TextOffset(p.caret))?;
    let target = if below { n.checked_add(1) } else { n.checked_sub(1) }.ok_or(Error::OutOfBounds)?;
    let prefix = p.caret - snapshot.line_range(n)?.start.0;
    let range = snapshot.line_range(target)?;
    let caret = if prefix > ADD_CARET_WINDOW {
        // Past the window a display column would cost O(column) reads on the UI
        // thread. Keep the byte offset into the line instead (the same position
        // for tab-free ASCII), clamped to the target's content; `normalize` below
        // snaps it to a grapheme boundary.
        let mut tail = range.end.0.saturating_sub(2).max(range.start.0);
        while !snapshot.is_boundary(TextOffset(tail)) {
            tail += 1;
        }
        let tail = snapshot.read(TextOffset(tail)..range.end, 2)?;
        let content_end = range.end.0 - (tail.len() - content(&tail).len());
        (range.start.0 + prefix).min(content_end)
    } else {
        // Stream only the caret's prefix and the target line up to that column,
        // never either whole line.
        let mut column = 0;
        walk_graphemes(snapshot, p.caret - prefix, p.caret, limits, |_, g| {
            if is_line_break(g) {
                return false;
            }
            column += cluster_width(g, column, limits.tab_width);
            true
        })?;
        let mut caret = range.start.0;
        let mut reached = 0;
        walk_graphemes(snapshot, range.start.0, range.end.0, limits, |at, g| {
            if is_line_break(g) {
                return false;
            }
            reached += cluster_width(g, reached, limits.tab_width);
            if reached > column {
                return false;
            }
            caret = at + g.len();
            true
        })?;
        caret
    };
    out.selections.push(Selection { anchor: caret, caret });
    out.primary = out.selections.len() - 1;
    normalize(snapshot, &out, limits)
}
pub fn select_occurrences(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    all: bool,
    limits: Limits,
) -> Result<SelectionSet, Error> {
    let mut out = normalize(snapshot, set, limits)?;
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
    let text = snapshot.read(TextOffset(0)..TextOffset(snapshot.len()), limits.max_bytes)?;
    let matches = text.match_indices(&needle).map(|(i, _)| i..i + needle.len());
    for r in matches
        .clone()
        .filter(|r| all || r.start >= primary_range.end)
        .chain(matches.filter(|r| !all && r.start < primary_range.end))
    {
        if out.selections.iter().any(|s| s.range() == r) {
            continue;
        }
        if snap(snapshot, r.start, limits)? != r.start || snap(snapshot, r.end, limits)? != r.end {
            continue;
        }
        out.selections.push(Selection {
            anchor: r.start,
            caret: r.end,
        });
        out.primary = out.selections.len() - 1;
        if out.selections.len() > limits.max_selections {
            return Err(Error::BudgetExceeded);
        }
        if !all {
            break;
        }
    }
    normalize(snapshot, &out, limits)
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
    #[test]
    fn backspace_delete_and_enter_edit_a_twenty_megabyte_single_line() {
        // The line exceeds the 16 MiB command budget, which whole-line reads hit.
        let text = format!("\t{}{{", "x".repeat(20 << 20));
        let mut d = doc(&text);
        let middle = 18 << 20;
        let caret: SelectionSet = Selection {
            anchor: middle,
            caret: middle,
        }
        .into();
        let edit = delete(&d.snapshot(), &caret, true, Limits::default()).unwrap();
        assert_eq!(
            edit.transaction.edits[0].range,
            TextOffset(middle - 1)..TextOffset(middle)
        );
        d.apply(edit.transaction).unwrap();
        let caret: SelectionSet = Selection {
            anchor: middle - 1,
            caret: middle - 1,
        }
        .into();
        let edit = delete(&d.snapshot(), &caret, false, Limits::default()).unwrap();
        assert_eq!(
            edit.transaction.edits[0].range,
            TextOffset(middle - 1)..TextOffset(middle)
        );
        d.apply(edit.transaction).unwrap();
        let end = d.snapshot().len();
        assert_eq!(end, text.len() - 2);
        let edit = crate::completion::smart_newline(
            &d.snapshot(),
            &Selection {
                anchor: end,
                caret: end,
            }
            .into(),
            bareline_syntax::Language::Rust,
            Limits::default(),
        )
        .unwrap();
        // Only the leading tab and the brace before the caret were consulted.
        assert_eq!(edit.transaction.edits[0].insert, "\n\t    ");
        d.apply(edit.transaction).unwrap();
        assert_eq!(d.snapshot().line_count(), 2);
    }
    #[test]
    fn added_caret_keeps_display_column_without_reading_whole_lines() {
        let d = doc("\tab\n界x\nshort");
        let set: SelectionSet = Selection { anchor: 2, caret: 2 }.into();
        // Column 5 (tab to 4, then "a") lands after "界x" (width 3) at the line end.
        let below = add_caret(&d.snapshot(), &set, true, Limits::default()).unwrap();
        assert_eq!(below.primary(), Selection { anchor: 8, caret: 8 });
        // From after "界" (column 2), line 0 snaps back before the tab (columns 0-4).
        let set: SelectionSet = Selection { anchor: 7, caret: 7 }.into();
        let above = add_caret(&d.snapshot(), &set, false, Limits::default()).unwrap();
        assert_eq!(above.primary(), Selection { anchor: 0, caret: 0 });
    }
    #[test]
    fn added_caret_past_the_window_of_a_twenty_megabyte_line_keeps_the_byte_offset() {
        // Column tracking would stream 17 MiB, past the 16 MiB command budget.
        let text = format!("ab\r\n{}\ncd", "x".repeat(20 << 20));
        let d = doc(&text);
        let caret = 4 + (17 << 20);
        let set: SelectionSet = Selection { anchor: caret, caret }.into();
        // Short neighbors clamp to their content end, before any terminator.
        let above = add_caret(&d.snapshot(), &set, false, Limits::default()).unwrap();
        assert_eq!(above.primary(), Selection { anchor: 2, caret: 2 });
        let below = add_caret(&d.snapshot(), &set, true, Limits::default()).unwrap();
        assert_eq!(below.primary().caret, text.len());
        // A long neighbor keeps the same byte offset into its line.
        let long = "x".repeat(ADD_CARET_WINDOW + 16);
        let d = doc(&format!("{long}\n{long}"));
        let caret = ADD_CARET_WINDOW + 8;
        let set: SelectionSet = Selection { anchor: caret, caret }.into();
        let below = add_caret(&d.snapshot(), &set, true, Limits::default()).unwrap();
        let expected = long.len() + 1 + caret;
        assert_eq!(
            below.primary(),
            Selection {
                anchor: expected,
                caret: expected
            }
        );
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
/// Ephemeral line-start text anchors, rebased through the committed transaction
/// and normalized back to line starts against the resulting snapshot.
#[derive(Clone, Debug, Default)]
pub struct Bookmarks {
    pub anchors: std::collections::BTreeSet<usize>,
}
impl Bookmarks {
    /// Toggles by line: any anchor on the caret's line counts as its bookmark.
    pub fn toggle(&mut self, snapshot: &DocumentSnapshot, offset: usize) -> Result<(), Error> {
        let line = snapshot.line_at(TextOffset(offset))?;
        let range = snapshot.line_range(line)?;
        // The last line also owns the document end.
        let end = if line + 1 == snapshot.line_count() {
            range.end.0 + 1
        } else {
            range.end.0
        };
        let on_line: Vec<usize> = self.anchors.range(range.start.0..end).copied().collect();
        if on_line.is_empty() {
            self.anchors.insert(range.start.0);
        } else {
            for anchor in on_line {
                self.anchors.remove(&anchor);
            }
        }
        Ok(())
    }
    /// Moves every anchor to the start of its line in `snapshot`; anchors that
    /// land on the same line merge.
    pub fn normalize(&mut self, snapshot: &DocumentSnapshot) {
        self.anchors = self
            .anchors
            .iter()
            .filter_map(|&anchor| {
                let mut anchor = anchor.min(snapshot.len());
                while !snapshot.is_boundary(TextOffset(anchor)) {
                    anchor -= 1;
                }
                let line = snapshot.line_at(TextOffset(anchor)).ok()?;
                Some(snapshot.line_range(line).ok()?.start.0)
            })
            .collect();
    }
    /// Whether an anchor lies on the line whose bytes are `start..=end`.
    pub fn on_line(&self, start: usize, end: usize) -> bool {
        self.anchors.range(start..=end).next().is_some()
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
    /// One merge walk over the sorted anchors and edits. An anchor inside a
    /// replaced range collapses to the edit start; callers normalize to line
    /// starts once the resulting snapshot exists.
    pub fn map_edits(&mut self, transaction: &EditTransaction) {
        let mut walk = crate::edit_walk::EditWalk::new(transaction);
        self.anchors = self
            .anchors
            .iter()
            .map(|&anchor| {
                let (shift, next) = walk.advance(|edit| edit.range.end.0 <= anchor);
                match next {
                    Some(edit) if edit.range.start.0 <= anchor => crate::edit_walk::shifted(edit.range.start.0, shift),
                    _ => crate::edit_walk::shifted(anchor, shift),
                }
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
        normalize(snapshot, &SelectionSet { selections, primary: 0 }, limits)
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
    let mut out = normalize(snapshot, set, limits)?;
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
    normalize(snapshot, &out, limits)
}
pub fn expand_lines(snapshot: &DocumentSnapshot, set: &SelectionSet, limits: Limits) -> Result<SelectionSet, Error> {
    let mut out = normalize(snapshot, set, limits)?;
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
    normalize(snapshot, &out, limits)
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
#[derive(Default)]
pub struct OccurrenceHistory {
    previous: Vec<SelectionSet>,
    count: usize,
}
impl OccurrenceHistory {
    pub fn remember(&mut self, set: &SelectionSet, limits: Limits) -> Result<(), Error> {
        if self
            .count
            .checked_add(set.selections.len())
            .is_none_or(|n| n > limits.max_selections)
        {
            return Err(Error::BudgetExceeded);
        }
        self.count += set.selections.len();
        self.previous.push(set.clone());
        Ok(())
    }
    pub fn undo(&mut self) -> Option<SelectionSet> {
        let set = self.previous.pop()?;
        self.count -= set.selections.len();
        Some(set)
    }
    pub fn clear(&mut self) {
        self.previous.clear();
        self.count = 0;
    }
}
pub fn skip_occurrence(snapshot: &DocumentSnapshot, set: &SelectionSet, limits: Limits) -> Result<SelectionSet, Error> {
    let normalized = normalize(snapshot, set, limits)?;
    let old = normalized.primary();
    let mut next = select_occurrences(snapshot, &normalized, false, limits)?;
    if next.selections.len() > normalized.selections.len() {
        next.selections.retain(|s| *s != old);
        next.primary = next.primary.min(next.selections.len() - 1);
    }
    normalize(snapshot, &next, limits)
}

/// Clipboard rows retain a final empty row, unlike line-transform terminator parsing.
fn clipboard_rows(text: &str) -> Vec<&str> {
    let mut rows: Vec<_> = split_rows(text).into_iter().map(|(body, _)| body).collect();
    if text.is_empty() || text.ends_with(['\r', '\n']) {
        rows.push("");
    }
    rows
}
