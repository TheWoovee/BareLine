// SPDX-License-Identifier: MPL-2.0
//! Revision-bound power edits. Offsets address UTF-8 text, never original bytes.
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
        self.selections
            .get(self.primary)
            .copied()
            .unwrap_or_default()
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
fn line(
    snapshot: &DocumentSnapshot,
    number: usize,
    limits: Limits,
) -> Result<(usize, String), Error> {
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
    let (start, s) = line(snapshot, snapshot.line_at(TextOffset(boundary))?, limits)?;
    Ok(start
        + s.grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(Some(s.len()))
            .take_while(|i| *i <= offset - start)
            .last()
            .unwrap_or(0))
}
/// Normalizes editing ranges; overlapping selections and duplicate carets mutate once.
pub fn normalize(
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
pub(crate) fn finish(
    snapshot: &DocumentSnapshot,
    mut edits: Vec<Edit>,
    limits: Limits,
) -> Result<PowerEdit, Error> {
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
        if e.range.start > e.range.end
            || !snapshot.is_boundary(e.range.start)
            || !snapshot.is_boundary(e.range.end)
        {
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
        selections.push(Selection {
            anchor: p,
            caret: p,
        });
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
        selections: SelectionSet {
            selections,
            primary: 0,
        },
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
    pub fn with_metrics(
        text: &str,
        tab_width: usize,
        mut width: impl FnMut(&str) -> usize,
    ) -> Self {
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
        let &(byte, col) = self
            .stops
            .iter()
            .rev()
            .find(|(_, c)| *c <= column)
            .unwrap_or(&(0, 0));
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
    column_insert_mapped(snapshot,rectangle,insert,limits,None)
}
pub fn column_insert_mapped(
    snapshot:&DocumentSnapshot,rectangle:Rectangle,insert:ColumnInsert,limits:Limits,
    maps:Option<&std::collections::BTreeMap<usize,DisplayColumnMap>>,
)->Result<PowerEdit,Error> {
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
        let map = if let Some(maps)=maps {maps.get(&n).ok_or(Error::OutOfBounds)?}else{fallback=DisplayColumnMap::new(body,limits.tab_width);&fallback};
        let left = rectangle.start_column.min(rectangle.end_column);
        let right = rectangle.start_column.max(rectangle.end_column);
        let (a, mut pad) = map.at(left);
        if a < body.len() && !body[a..].starts_with('\t') {
            pad = 0;
        }
        let (mut b, _) = map.at(right);
        if right > left && map.column(b) < right && b < body.len() {
            b = map
                .stops
                .iter()
                .find(|(p, _)| *p > b)
                .map_or(b, |(p, _)| *p);
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
                if s.len()
                    .checked_mul(*repeat)
                    .is_none_or(|v| v > limits.max_bytes)
                {
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
            let next = map
                .stops
                .iter()
                .find(|(p, _)| *p > right_byte)
                .copied()
                .unwrap();
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
                .or_else(|_| {
                    snapshot.line_at(TextOffset(snap(snapshot, r.end.saturating_sub(1), limits)?))
                })?;
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
                let separator = if source.ends_with(['\r', '\n']) {
                    ""
                } else {
                    eol
                };
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
                        rows.sort_by(|a, b| {
                            let x = if *case_sensitive {
                                a.0.to_string()
                            } else {
                                a.0.to_lowercase()
                            };
                            let y = if *case_sensitive {
                                b.0.to_string()
                            } else {
                                b.0.to_lowercase()
                            };
                            if *numeric {
                                match (x.trim().parse::<f64>(), y.trim().parse::<f64>()) {
                                    (Ok(x), Ok(y)) => x.total_cmp(&y),
                                    _ => x.cmp(&y),
                                }
                            } else {
                                x.cmp(&y)
                            }
                        });
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
                        Transform::Indent => format!(
                            "{}{}",
                            " ".repeat(limits.tab_width.min(limits.max_bytes)),
                            body
                        ),
                        Transform::Unindent => {
                            if let Some(s) = body.strip_prefix('\t') {
                                s.to_string()
                            } else {
                                body.chars()
                                    .skip(
                                        body.chars()
                                            .take(limits.tab_width)
                                            .take_while(|c| *c == ' ')
                                            .count(),
                                    )
                                    .collect()
                            }
                        }
                        Transform::TabsToSpaces => {
                            let mut out = String::new();
                            let mut col = 0;
                            for g in body.graphemes(true) {
                                if g == "\t" {
                                    let n = limits.tab_width.max(1) - col % limits.tab_width.max(1);
                                    if out
                                        .len()
                                        .checked_add(n)
                                        .is_none_or(|v| v > limits.max_bytes)
                                    {
                                        return Err(Error::BudgetExceeded);
                                    }
                                    out.extend(std::iter::repeat_n(' ', n));
                                    col += n;
                                } else {
                                    out.push_str(g);
                                    col +=
                                        DisplayColumnMap::new(g, limits.tab_width).column(g.len());
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
pub fn add_caret(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    below: bool,
    limits: Limits,
) -> Result<SelectionSet, Error> {
    let mut out = normalize(snapshot, set, limits)?;
    let p = out.primary();
    let n = snapshot.line_at(TextOffset(p.caret))?;
    let target = if below {
        n.checked_add(1)
    } else {
        n.checked_sub(1)
    }
    .ok_or(Error::OutOfBounds)?;
    let (start, text) = line(snapshot, n, limits)?;
    let column = DisplayColumnMap::new(content(&text), limits.tab_width).column(p.caret - start);
    let (start, text) = line(snapshot, target, limits)?;
    let p = start
        + DisplayColumnMap::new(content(&text), limits.tab_width)
            .at(column)
            .0;
    out.selections.push(Selection {
        anchor: p,
        caret: p,
    });
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
    let primary_range=p.range();
    let needle = snapshot.read(TextOffset(primary_range.start)..TextOffset(primary_range.end), limits.max_bytes)?;
    if needle.is_empty() {
        return Ok(out);
    }
    let text = snapshot.read(TextOffset(0)..TextOffset(snapshot.len()), limits.max_bytes)?;
    let matches = text
        .match_indices(&needle)
        .map(|(i, _)| i..i + needle.len());
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
    let text = snapshot.read(
        TextOffset(range.start)..TextOffset(range.end),
        limits.max_bytes,
    )?;
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
    fn doc(s: &str) -> Document {
        Document::from_utf8(s, Budget::new(64 << 20), Budget::new(64 << 20)).unwrap()
    }
    fn text(d: &Document) -> String {
        let s = d.snapshot();
        s.read(TextOffset(0)..TextOffset(s.len()), 64 << 20)
            .unwrap()
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
        let set = SelectionSet {
            selections,
            primary: 0,
        };
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
        let edit = transform(
            &d.snapshot(),
            &set,
            Transform::RemoveDuplicates,
            Limits::default(),
        )
        .unwrap();
        d.apply(edit.transaction).unwrap();
        assert_eq!(text(&d), "b\r\na\r\n");
        d.undo().unwrap();
        let set = SelectionSet {
            selections: vec![
                Selection {
                    anchor: 0,
                    caret: 3,
                },
                Selection {
                    anchor: 1,
                    caret: 4,
                },
            ],
            primary: 1,
        };
        let edit = replace(&d.snapshot(), &set, "x", Limits::default()).unwrap();
        assert_eq!(edit.transaction.edits.len(), 1);
        let mut builder =
            bareline_document::DocumentBuilder::new(Budget::new(1024), Budget::new(1024)).unwrap();
        builder.append("a").unwrap();
        assert!(matches!(
            replace(
                &builder.prefix(),
                &Selection::default().into(),
                "x",
                Limits::default()
            ),
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
            &Selection {
                anchor: 0,
                caret: 3,
            }
            .into(),
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
            &Selection {
                anchor: 0,
                caret: 6,
            }
            .into(),
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
        let edit =
            rectangle_paste(&d.snapshot(), rectangle, "•\n界\n🦀", Limits::default()).unwrap();
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
    pub fn admit_with_limits(
        &mut self,
        text: &str,
        count: usize,
        total: usize,
        entry: usize,
    ) -> Result<(), Error> {
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
        let start = snapshot
            .line_range(snapshot.line_at(TextOffset(offset))?)?
            .start
            .0;
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
                        delta +=
                            e.insert.len() as isize - (e.range.end.0 - e.range.start.0) as isize;
                    } else if e.range.start.0 <= anchor {
                        return e.range.start.0.saturating_add_signed(delta);
                    }
                }
                anchor.saturating_add_signed(delta)
            })
            .collect();
    }
    pub fn selections(
        &self,
        snapshot: &DocumentSnapshot,
        limits: Limits,
    ) -> Result<SelectionSet, Error> {
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
        normalize(
            snapshot,
            &SelectionSet {
                selections,
                primary: 0,
            },
            limits,
        )
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
    rectangle_paste_mapped(snapshot,rectangle,text,limits,None)
}
pub fn rectangle_paste_mapped(snapshot:&DocumentSnapshot,rectangle:Rectangle,text:&str,limits:Limits,maps:Option<&std::collections::BTreeMap<usize,DisplayColumnMap>>)->Result<PowerEdit,Error> {
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
        let value = rows
            .get(if rows.len() > 1 { row } else { 0 })
            .map_or("", |s| *s);
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
pub fn rectangle_copy(
    snapshot: &DocumentSnapshot,
    rectangle: Rectangle,
    limits: Limits,
) -> Result<String, Error> {
    rectangle_copy_mapped(snapshot,rectangle,limits,None)
}
pub fn rectangle_copy_mapped(snapshot:&DocumentSnapshot,rectangle:Rectangle,limits:Limits,maps:Option<&std::collections::BTreeMap<usize,DisplayColumnMap>>)->Result<String,Error> {
    if !snapshot.is_complete() {
        return Err(Error::IncompleteSource);
    }
    if rectangle.first_line > rectangle.last_line
        || rectangle.last_line - rectangle.first_line >= limits.max_selections
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
        let map=if let Some(maps)=maps{maps.get(&n).ok_or(Error::OutOfBounds)?}else{fallback=DisplayColumnMap::new(body,limits.tab_width);&fallback};
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
    if let Some(i) = out
        .selections
        .iter()
        .position(|s| s.anchor == p && s.caret == p)
    {
        if out.selections.len() > 1 {
            out.selections.remove(i);
            out.primary = out.primary.min(out.selections.len() - 1);
        }
    } else {
        out.selections.push(Selection {
            anchor: p,
            caret: p,
        });
        out.primary = out.selections.len() - 1;
    }
    normalize(snapshot, &out, limits)
}
pub fn expand_lines(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    limits: Limits,
) -> Result<SelectionSet, Error> {
    let mut out = normalize(snapshot, set, limits)?;
    for s in &mut out.selections {
        let range=s.range();let backward=s.anchor>s.caret;
        let start = snapshot.line_at(TextOffset(range.start))?;
        let mut last=if range.end>range.start{range.end-1}else{range.end};
        while !snapshot.is_boundary(TextOffset(last)){last-=1;}
        let end = snapshot.line_at(TextOffset(last))?;
        let first=snapshot.line_range(start)?.start.0;let last=snapshot.line_range(end)?.end.0;
        s.anchor=if backward{last}else{first};s.caret=if backward{first}else{last};
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
        (
            "editor.selection.nextOccurrence",
            "Select Next Occurrence",
            "Ctrl+D",
        ),
        (
            "editor.selection.allOccurrences",
            "Select All Occurrences",
            "Ctrl+Shift+L",
        ),
        ("editor.selection.skipOccurrence", "Skip Occurrence", ""),
        (
            "editor.selection.undoOccurrence",
            "Undo Added Occurrence",
            "",
        ),
        (
            "editor.selection.rotatePrimary",
            "Rotate Primary Selection",
            "",
        ),
        ("editor.selection.expandLines", "Expand to Lines", ""),
        (
            "editor.selection.escape",
            "Keep Primary Selection",
            "Escape",
        ),
        ("editor.column.insert", "Column Editor…", "Alt+C"),
        ("editor.lines.hide", "Hide Selected Lines", ""),
        ("editor.lines.showAll", "Show Hidden Lines", ""),
        ("editor.paste.plainText", "Paste Plain Text", "Ctrl+Shift+V"),
        ("editor.paste.fromHistory", "Paste from History…", ""),
        (
            "editor.clipboard.toggleHistory",
            "Toggle Clipboard History",
            "",
        ),
        ("editor.comment.toggleLine", "Toggle Line Comment", "Ctrl+/"),
        (
            "editor.comment.toggleBlock",
            "Toggle Block Comment",
            "Ctrl+Shift+/",
        ),
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
        (
            "editor.lines.sortIgnoreCase",
            "Sort Lines Ignoring Case",
            "",
        ),
        (
            "editor.lines.removeDuplicates",
            "Remove Duplicate Lines",
            "",
        ),
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
pub fn skip_occurrence(
    snapshot: &DocumentSnapshot,
    set: &SelectionSet,
    limits: Limits,
) -> Result<SelectionSet, Error> {
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
fn clipboard_rows(text:&str)->Vec<&str>{
    let mut rows:Vec<_>=split_rows(text).into_iter().map(|(body,_)|body).collect();
    if text.is_empty()||text.ends_with(['\r','\n']){rows.push("");}
    rows
}
