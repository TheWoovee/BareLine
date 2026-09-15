// SPDX-License-Identifier: MPL-2.0
//! Authoritative global power commands. Preparation only reads captured sources;
//! publication is performed by the paged actor's journaled source transaction.
use crate::{
    Selection,
    power::{self, Rectangle, SelectionSet, consumer::Arguments},
};
use bareline_document::{
    Budget, Document, Edit, TextOffset,
    history::{EditMetadata, EditOrigin},
    line_lookup::{LineLookupPoll, LineTarget},
    paged::{
        OwnedTextRange, PagedSnapshot, PreparedSourceTransaction, SourceEdit, SourceTransactionPoll, SparseLineIndex,
    },
};
use bareline_file_io::paged_service::PagedReadHandle;
use power::captured::{CapturedRangeReader, StagingOptions};
use std::{io::Read, ops::Range, sync::Arc};

#[derive(Clone, Default)]
pub struct PowerViewState {
    pub bookmarks: Vec<usize>,
    pub hidden: Vec<Range<usize>>,
    pub rectangle: Option<Rectangle>,
    pub occurrence_history: Vec<SelectionSet>,
}
impl PowerViewState {
    pub fn map_edits(&mut self, edits: &[bareline_document::change::CompactEdit]) {
        let map = |offset: usize| -> Option<usize> {
            let mut delta = 0isize;
            for edit in edits {
                if offset < edit.before.start.0 {
                    break;
                }
                if offset < edit.before.end.0 {
                    return None;
                }
                delta = delta
                    .checked_add(edit.inserted_len as isize - (edit.before.end.0 - edit.before.start.0) as isize)?;
            }
            offset.checked_add_signed(delta)
        };
        self.bookmarks = self.bookmarks.iter().filter_map(|offset| map(*offset)).collect();
        self.hidden = self
            .hidden
            .iter()
            .filter(|range| {
                !edits
                    .iter()
                    .any(|edit| edit.before.start.0 < range.end && edit.before.end.0 > range.start)
            })
            .filter_map(|range| Some(map(range.start)?..map(range.end)?))
            .collect();
        self.rectangle = None;
        self.occurrence_history.clear();
    }
}
#[derive(Clone, Default)]
pub struct PowerStateHistory(Vec<(bareline_document::ContentStateId, PowerViewState)>);
impl PowerStateHistory {
    pub fn transition(
        &mut self,
        current: bareline_document::ContentStateId,
        next: bareline_document::ContentStateId,
        state: &mut PowerViewState,
        change: &bareline_document::change::AppliedChange,
    ) {
        let mut saved = state.clone();
        saved.occurrence_history.clear();
        self.0.retain(|(id, _)| *id != current);
        self.0.push((current, saved));
        while self.0.len() > 16
            || self
                .0
                .iter()
                .map(|(_, state)| state.bookmarks.len() * 8 + state.hidden.len() * 16)
                .sum::<usize>()
                > 4 << 20
        {
            self.0.remove(0);
        }
        if !matches!(change.direction, bareline_document::change::ChangeDirection::Edit) {
            if let Some((_, saved)) = self.0.iter().find(|(id, _)| *id == next) {
                *state = saved.clone();
                return;
            }
        }
        state.map_edits(change.edits());
    }
}
#[derive(Clone)]
pub struct Capture {
    pub source: PagedReadHandle,
    pub selections: SelectionSet,
    pub state: PowerViewState,
    pub language: bareline_syntax::Language,
    pub definition: Option<Arc<bareline_syntax::udl::Definition>>,
    pub tab_width: usize,
    pub typing: crate::paged_typing::TypingConfig,
    pub literal_contexts: Vec<Option<bool>>,
    pub column_maps: Option<std::collections::BTreeMap<usize, power::DisplayColumnMap>>,
}
pub struct PreparedPower {
    pub source: PagedSnapshot,
    pub selections: SelectionSet,
    pub state: PowerViewState,
    pub hidden_lines: Vec<Range<u64>>,
    pub transaction: Option<PreparedSourceTransaction>,
    pub clipboard: Option<String>,
    pub clipboard_rectangle: Option<Rectangle>,
    pub arguments: Arguments,
}
pub fn supports_command(id: &str) -> bool {
    if matches!(
        id,
        "editor.rectangle.gesture" | "editor.rectangle.extend" | "editor.clipboard.rectangle"
    ) {
        return true;
    }
    if matches!(
        id,
        "editor.bookmark.selectLines" | "editor.column.context" | "editor.rectangle.cut" | "editor.lines.refreshHidden"
    ) {
        return true;
    }
    power::transform_for_command(id).is_some()
        || matches!(
            id,
            "editor.column.insert"
                | "editor.rectangle.select"
                | "editor.rectangle.copy"
                | "editor.rectangle.paste"
                | "editor.rectangle.delete"
                | "editor.paste.plainText"
                | "editor.paste.fromHistory"
                | "editor.caret.above"
                | "editor.caret.below"
                | "editor.caret.toggle"
                | "editor.selection.nextOccurrence"
                | "editor.selection.allOccurrences"
                | "editor.selection.skipOccurrence"
                | "editor.selection.undoOccurrence"
                | "editor.selection.rotatePrimary"
                | "editor.selection.escape"
                | "editor.selection.expandLines"
                | "editor.bookmark.toggle"
                | "editor.bookmark.clear"
                | "editor.bookmark.next"
                | "editor.bookmark.previous"
                | "editor.bookmark.copyLines"
                | "editor.bookmark.cutLines"
                | "editor.bookmark.deleteLines"
                | "editor.lines.hide"
                | "editor.lines.showAll"
                | "editor.comment.toggleLine"
                | "editor.comment.toggleBlock"
        )
}
fn parameter<T: std::str::FromStr>(args: &Arguments, key: &str) -> Result<T, String> {
    args.get(key)
        .ok_or_else(|| format!("Missing {key}"))?
        .parse()
        .map_err(|_| format!("Invalid {key}"))
}
fn rectangle(args: &Arguments) -> Result<Rectangle, String> {
    Ok(Rectangle {
        first_line: parameter(args, "first_line")?,
        last_line: parameter(args, "last_line")?,
        start_column: parameter(args, "start_column")?,
        end_column: parameter(args, "end_column")?,
    })
}
pub fn validate_arguments(id: &str, args: &Arguments) -> Result<(), String> {
    if !supports_command(id) {
        return Err("Unsupported paged power command".into());
    }
    if args.len() > 16
        || args
            .iter()
            .try_fold(0usize, |n, (key, value)| {
                n.checked_add(key.len())?.checked_add(value.len())
            })
            .is_none_or(|n| n > 16 << 20)
    {
        return Err("Power arguments exceed budget".into());
    }
    let rectangle_keys = ["first_line", "last_line", "start_column", "end_column"];
    for key in args.keys() {
        let allowed = match id {
            "editor.rectangle.gesture" => matches!(key.as_str(), "anchor_offset" | "caret_offset"),
            "editor.rectangle.extend" => matches!(key.as_str(), "dx" | "dy"),
            "editor.clipboard.rectangle" => matches!(key.as_str(), "rows" | "text"),
            "editor.column.insert" => {
                rectangle_keys.contains(&key.as_str())
                    || ["mode", "text", "start", "step", "width", "base", "repeat"].contains(&key.as_str())
            }
            "editor.rectangle.select"
            | "editor.rectangle.copy"
            | "editor.rectangle.cut"
            | "editor.rectangle.delete" => rectangle_keys.contains(&key.as_str()),
            "editor.rectangle.paste" | "editor.paste.plainText" | "editor.paste.fromHistory" => {
                key == "text" || rectangle_keys.contains(&key.as_str())
            }
            "editor.caret.toggle" => key == "offset",
            "editor.indent" | "editor.unindent" => rectangle_keys.contains(&key.as_str()),
            "editor.comment.toggleLine" | "editor.comment.toggleBlock" => {
                ["line_prefix", "block_start", "block_end"].contains(&key.as_str())
            }
            _ => false,
        };
        if !allowed {
            return Err(format!("Unknown power argument: {key}"));
        }
    }
    if (id.starts_with("editor.rectangle.") && !matches!(id, "editor.rectangle.gesture" | "editor.rectangle.extend"))
        || id == "editor.column.insert"
        || rectangle_keys.iter().any(|key| args.contains_key(*key))
    {
        let rectangle = rectangle(args)?;
        if rectangle.first_line > rectangle.last_line || rectangle.start_column > rectangle.end_column {
            return Err("Invalid rectangle".into());
        }
    }
    if matches!(
        id,
        "editor.paste.plainText" | "editor.paste.fromHistory" | "editor.rectangle.paste"
    ) && !args.contains_key("text")
    {
        return Err("Missing captured paste text".into());
    }
    if id == "editor.column.insert" {
        match args.get("mode").map(String::as_str) {
            Some("text") if args.contains_key("text") => {}
            Some("numbers") => {
                let _: i64 = parameter(args, "start")?;
                let _: i64 = parameter(args, "step")?;
                let _: usize = parameter(args, "width")?;
                let base: u32 = parameter(args, "base")?;
                let _: usize = parameter(args, "repeat")?;
                if !matches!(base, 2 | 8 | 10 | 16) {
                    return Err("Invalid number base".into());
                }
            }
            _ => return Err("Invalid column mode".into()),
        }
    }
    if id == "editor.caret.toggle" {
        let _: usize = parameter(args, "offset")?;
    }
    if id == "editor.rectangle.extend" {
        for key in ["dx", "dy"] {
            if !(-1..=1).contains(&parameter::<isize>(args, key)?) {
                return Err("Invalid rectangle movement".into());
            }
        }
    }
    if id == "editor.clipboard.rectangle" {
        let rows = parameter::<usize>(args, "rows")?;
        if rows == 0 || rows > power::Limits::default().max_selections || !args.contains_key("text") {
            return Err("Invalid rectangle clipboard".into());
        }
    }
    if args.contains_key("block_start") != args.contains_key("block_end") {
        return Err("Incomplete captured block comment policy".into());
    }
    Ok(())
}
fn lookup(capture: &Capture, target: LineTarget, options: &StagingOptions) -> Result<LineLookupPoll, String> {
    let index = SparseLineIndex::new(capture.source.snapshot().clone(), 16, 64 * 1024, &options.budget)
        .map_err(|e| format!("{e:?}"))?;
    let mut request = index
        .lookup(target, options.budget.clone())
        .map_err(|e| format!("{e:?}"))?;
    loop {
        options.cancellation.check().map_err(|e| format!("{e:?}"))?;
        match request.poll() {
            value @ (LineLookupPoll::Line(_) | LineLookupPoll::Range(_)) => return Ok(value),
            LineLookupPoll::Progress(_) => {}
            LineLookupPoll::Pending(ticket) => {
                if !capture
                    .source
                    .resolve_captured_page(ticket)
                    .map_err(|error| error.to_string())?
                {
                    std::thread::yield_now();
                }
            }
            value => return Err(format!("Power line lookup: {value:?}")),
        }
    }
}
fn line_at(capture: &Capture, offset: usize, options: &StagingOptions) -> Result<usize, String> {
    match lookup(capture, LineTarget::Byte(TextOffset(offset)), options)? {
        LineLookupPoll::Line(line) => Ok(line),
        _ => Err("No logical line".into()),
    }
}
fn line_range(capture: &Capture, line: usize, options: &StagingOptions) -> Result<Range<usize>, String> {
    match lookup(capture, LineTarget::Line(line), options)? {
        LineLookupPoll::Range(range) => Ok(range.start.0..range.end.0),
        _ => Err("No logical range".into()),
    }
}
fn read(capture: &Capture, range: Range<usize>, options: &StagingOptions, limit: usize) -> Result<String, String> {
    if range.end.saturating_sub(range.start) > limit {
        return Err("Selected command data exceeds its memory quota".into());
    }
    let mut text = String::new();
    CapturedRangeReader::new(
        capture.source.clone(),
        TextOffset(range.start)..TextOffset(range.end),
        options.budget.clone(),
        options.cancellation.clone(),
    )
    .map_err(|e| e.to_string())?
    .read_to_string(&mut text)
    .map_err(|e| e.to_string())?;
    Ok(text)
}
/// Bounded source rows for the UI renderer. The returned map is keyed by global
/// logical line, independent of folds, viewport fragments, and selection proxies.
pub fn measurement_rows(
    capture: &Capture,
    id: &str,
    args: &Arguments,
    options: &StagingOptions,
) -> Result<Option<Vec<(usize, String)>>, String> {
    if capture.column_maps.is_some() {
        return Ok(None);
    }
    if id == "editor.clipboard.rectangle" {
        let first = line_at(capture, capture.selections.primary().caret, options)?;
        let last = first
            .checked_add(parameter::<usize>(args, "rows")?.saturating_sub(1))
            .ok_or("Rectangle row overflow")?;
        let mut args = Arguments::new();
        for (key, value) in [
            ("first_line", first),
            ("last_line", last),
            ("start_column", 0),
            ("end_column", 0),
        ] {
            args.insert(key.into(), value.to_string());
        }
        return measurement_rows(capture, "editor.rectangle.select", &args, options);
    }
    let rectangle = if id == "editor.rectangle.extend" {
        let current = capture.state.rectangle.unwrap_or_else(|| Rectangle {
            first_line: 0,
            last_line: 0,
            start_column: 0,
            end_column: 0,
        });
        let first = if capture.state.rectangle.is_some() {
            current.first_line
        } else {
            line_at(capture, capture.selections.primary().caret, options)?
        };
        let last = if capture.state.rectangle.is_some() {
            current.last_line
        } else {
            first
        };
        Some(Rectangle {
            first_line: first.min(last.saturating_add_signed(parameter(args, "dy")?)),
            last_line: last
                .saturating_add_signed(parameter(args, "dy")?)
                .min(line_at(capture, capture.source.snapshot().len(), options)?)
                .max(first),
            ..current
        })
    } else if args.contains_key("first_line") {
        Some(rectangle(args)?)
    } else {
        capture.state.rectangle
    };
    let lines = if id == "editor.rectangle.gesture" {
        let a = line_at(capture, parameter(args, "anchor_offset")?, options)?;
        let b = line_at(capture, parameter(args, "caret_offset")?, options)?;
        Some(a.min(b)..=a.max(b))
    } else if let Some(rectangle) = rectangle {
        Some(rectangle.first_line..=rectangle.last_line)
    } else if id == "editor.column.context" {
        let line = line_at(capture, capture.selections.primary().caret, options)?;
        Some(line..=line)
    } else {
        None
    };
    let Some(lines) = lines else {
        return Ok(None);
    };
    if lines.end().saturating_sub(*lines.start()) >= power::Limits::default().max_selections {
        return Err("Column row quota exceeded".into());
    }
    let first = *lines.start();
    let start = line_range(capture, first, options)?.start;
    let end = line_range(capture, *lines.end(), options)?.end;
    let text = read(capture, start..end, options, options.memory / 4)?;
    let document = Document::from_utf8(&text, Budget::new(options.memory), Budget::new(options.memory))
        .map_err(|e| format!("Column rows: {e:?}"))?;
    let snapshot = document.snapshot();
    let mut rows = Vec::new();
    // Index the captured block once. Repeated whole-source line refinement for
    // every row would make a large rectangle quadratic in its line count.
    for number in lines {
        let range = snapshot
            .line_range(number - first)
            .map_err(|e| format!("Column range: {e:?}"))?;
        rows.push((
            number,
            snapshot
                .read(range, options.memory / 4)
                .map_err(|e| format!("Column row: {e:?}"))?,
        ));
    }
    Ok(Some(rows))
}
pub fn prepare(
    mut capture: Capture,
    id: &str,
    args: &Arguments,
    options: &StagingOptions,
) -> Result<PreparedPower, String> {
    validate_arguments(id, args)?;
    if id == "editor.clipboard.rectangle" {
        let caret = capture.selections.primary().caret;
        let first = line_at(&capture, caret, options)?;
        let range = line_range(&capture, first, options)?;
        let map = capture
            .column_maps
            .as_ref()
            .and_then(|maps| maps.get(&first))
            .ok_or("Clipboard rectangle needs measured rows")?;
        let column = map.column(caret - range.start);
        let last = first
            .checked_add(parameter::<usize>(args, "rows")?.saturating_sub(1))
            .ok_or("Rectangle row overflow")?;
        let mut values = Arguments::new();
        for (key, value) in [
            ("first_line", first),
            ("last_line", last),
            ("start_column", column),
            ("end_column", column),
        ] {
            values.insert(key.into(), value.to_string());
        }
        values.insert("text".into(), args.get("text").ok_or("Missing clipboard text")?.clone());
        return prepare(capture, "editor.rectangle.paste", &values, options);
    }
    if id == "editor.rectangle.extend" {
        let mut value = if let Some(rectangle) = capture.state.rectangle {
            rectangle
        } else {
            let caret = capture.selections.primary().caret;
            let line = line_at(&capture, caret, options)?;
            let range = line_range(&capture, line, options)?;
            let map = capture
                .column_maps
                .as_ref()
                .and_then(|maps| maps.get(&line))
                .ok_or("Rectangle keyboard selection needs measured rows")?;
            let column = map.column(caret - range.start);
            Rectangle {
                first_line: line,
                last_line: line,
                start_column: column,
                end_column: column,
            }
        };
        value.last_line = value
            .last_line
            .saturating_add_signed(parameter(args, "dy")?)
            .min(line_at(&capture, capture.source.snapshot().len(), options)?);
        value.first_line = value.first_line.min(value.last_line);
        value.end_column = value.end_column.saturating_add_signed(parameter(args, "dx")?);
        value.start_column = value.start_column.min(value.end_column);
        let args = [
            ("first_line", value.first_line),
            ("last_line", value.last_line),
            ("start_column", value.start_column),
            ("end_column", value.end_column),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.to_string()))
        .collect();
        return prepare(capture, "editor.rectangle.select", &args, options);
    }
    if id == "editor.rectangle.gesture" {
        let position = |key| -> Result<(usize, usize), String> {
            let offset = parameter::<usize>(args, key)?;
            let line = line_at(&capture, offset, options)?;
            let range = line_range(&capture, line, options)?;
            let map = capture
                .column_maps
                .as_ref()
                .and_then(|maps| maps.get(&line))
                .ok_or("Rectangle gesture needs measured rows")?;
            Ok((
                line,
                map.column(offset.checked_sub(range.start).ok_or("Invalid rectangle endpoint")?),
            ))
        };
        let (a, x) = position("anchor_offset")?;
        let (b, y) = position("caret_offset")?;
        let args = [
            ("first_line", a.min(b)),
            ("last_line", a.max(b)),
            ("start_column", x.min(y)),
            ("end_column", x.max(y)),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.to_string()))
        .collect();
        return prepare(capture, "editor.rectangle.select", &args, options);
    }
    let _claim = options
        .budget
        .claim(options.memory)
        .map_err(|e| format!("Power memory quota: {e:?}"))?;
    let limit = options.memory / 2;
    let limits = power::Limits {
        max_bytes: limit,
        tab_width: capture.tab_width,
        ..Default::default()
    };
    if capture.selections.selections.is_empty()
        || capture.selections.selections.len() > limits.max_selections
        || capture.selections.primary >= capture.selections.selections.len()
    {
        return Err("Invalid global selection set".into());
    }
    let mut selections = capture.selections.clone();
    let mut edits = Vec::new();
    let mut clipboard = None;
    let mut recorded = args.clone();
    let err = |e| format!("Power command: {e:?}");
    match id {
        "editor.comment.toggleBlock" => {
            use power::CommentProvider;
            let tokens = if args.contains_key("block_start") || args.contains_key("line_prefix") {
                power::CommentTokens {
                    line: args.get("line_prefix").cloned(),
                    block: args
                        .get("block_start")
                        .zip(args.get("block_end"))
                        .map(|(start, end)| (start.clone(), end.clone())),
                }
            } else {
                let document = Document::from_utf8("", Budget::new(1 << 20), Budget::new(1 << 20)).map_err(err)?;
                if let Some(definition) = capture.definition.as_deref() {
                    crate::completion::DefinitionComments(definition).tokens_for(&document.snapshot())
                } else {
                    crate::completion::LanguageComments(capture.language).tokens_for(&document.snapshot())
                }
                .ok_or("No comment definition for this language")?
            };
            if let Some(prefix) = &tokens.line {
                recorded.insert("line_prefix".into(), prefix.clone());
            }
            if let Some((start, end)) = &tokens.block {
                recorded.insert("block_start".into(), start.clone());
                recorded.insert("block_end".into(), end.clone());
            }
            let mut ordered = capture
                .selections
                .selections
                .iter()
                .copied()
                .enumerate()
                .collect::<Vec<_>>();
            ordered.sort_by_key(|(_, selection)| selection.range().start);
            let mut after = capture.selections.selections.clone();
            let mut delta = 0isize;
            for (index, selection) in ordered {
                let plan = crate::paged_typing::prepare(
                    capture.source.snapshot(),
                    selection,
                    crate::paged_typing::TypingRequest::CommentWithTokens {
                        block: true,
                        tokens: tokens.clone(),
                    },
                    &capture.typing,
                    &options.cancellation,
                    |start, length| window(&capture, start, length, options),
                )?
                .ok_or("Block comment plan unavailable")?;
                after[index] = Selection {
                    anchor: plan
                        .selection
                        .anchor
                        .checked_add_signed(delta)
                        .ok_or("Comment selection overflow")?,
                    caret: plan
                        .selection
                        .caret
                        .checked_add_signed(delta)
                        .ok_or("Comment selection overflow")?,
                };
                for edit in &plan.transaction.edits {
                    delta = delta
                        .checked_add(edit.insert.len() as isize - (edit.range.end.0 - edit.range.start.0) as isize)
                        .ok_or("Comment delta overflow")?;
                }
                edits.extend(plan.transaction.edits);
            }
            selections = SelectionSet {
                selections: after,
                primary: capture.selections.primary,
            };
        }
        "editor.caret.toggle" => {
            let offset = parameter::<usize>(args, "offset")?;
            if offset > capture.source.snapshot().len() {
                return Err("Caret exceeds source".into());
            }
            let (origin, text) = window(&capture, TextOffset(offset.saturating_sub(65536)), 131072, options)?;
            let local = offset.checked_sub(origin.0).ok_or("Caret context unavailable")?;
            use unicode_segmentation::UnicodeSegmentation;
            if local != text.len() && !text.grapheme_indices(true).any(|(start, _)| start == local) {
                return Err("Caret is not a verified grapheme boundary".into());
            }
            if let Some(index) = selections
                .selections
                .iter()
                .position(|selection| selection.anchor == offset && selection.caret == offset)
            {
                if selections.selections.len() > 1 {
                    selections.selections.remove(index);
                    selections.primary = selections.primary.min(selections.selections.len() - 1);
                }
            } else {
                if selections.selections.len() >= limits.max_selections {
                    return Err("Caret quota exceeded".into());
                }
                selections.selections.push(Selection {
                    anchor: offset,
                    caret: offset,
                });
                selections
                    .selections
                    .sort_by_key(|selection| selection.anchor.min(selection.caret));
                selections.primary = selections
                    .selections
                    .iter()
                    .position(|selection| selection.anchor == offset && selection.caret == offset)
                    .unwrap();
            }
        }
        "editor.lines.refreshHidden" => {}
        "editor.column.context" => {
            let selected = selections.primary();
            let range = selected.range();
            let first = line_at(&capture, range.start, options)?;
            let last = line_at(
                &capture,
                range.end.saturating_sub(usize::from(!range.is_empty())),
                options,
            )?;
            let caret_line = line_range(&capture, line_at(&capture, selected.caret, options)?, options)?;
            let text = read(&capture, caret_line.clone(), options, limit)?;
            let number = line_at(&capture, selected.caret, options)?;
            let fallback = power::DisplayColumnMap::new(text.trim_end_matches(['\r', '\n']), capture.tab_width);
            let map = if let Some(maps) = capture.column_maps.as_ref() {
                maps.get(&number).ok_or("Missing measured caret row")?
            } else {
                &fallback
            };
            let column = map.column(selected.caret - caret_line.start);
            capture.state.rectangle = Some(Rectangle {
                first_line: first,
                last_line: last,
                start_column: column,
                end_column: column,
            });
        }
        "editor.bookmark.selectLines" => {
            let mut rows = Vec::new();
            for anchor in &capture.state.bookmarks {
                let range = line_range(&capture, line_at(&capture, *anchor, options)?, options)?;
                rows.push(Selection {
                    anchor: range.start,
                    caret: range.end,
                });
            }
            if !rows.is_empty() {
                selections = SelectionSet {
                    selections: rows,
                    primary: 0,
                };
            }
        }
        "editor.selection.rotatePrimary" => selections.rotate_primary(),
        "editor.selection.escape" => {
            selections.escape();
            capture.state.rectangle = None;
        }
        "editor.selection.undoOccurrence" => {
            if let Some(previous) = capture.state.occurrence_history.pop() {
                selections = previous;
            }
        }
        "editor.bookmark.clear" => capture.state.bookmarks.clear(),
        "editor.bookmark.toggle" => {
            let anchor = line_range(
                &capture,
                line_at(&capture, selections.primary().caret, options)?,
                options,
            )?
            .start;
            match capture.state.bookmarks.binary_search(&anchor) {
                Ok(index) => {
                    capture.state.bookmarks.remove(index);
                }
                Err(index) => {
                    if capture.state.bookmarks.len() >= limits.max_selections {
                        return Err("Bookmark quota exceeded".into());
                    }
                    capture.state.bookmarks.insert(index, anchor);
                }
            }
        }
        "editor.bookmark.next" | "editor.bookmark.previous" => {
            let at = selections.primary().caret;
            let next = if id.ends_with("previous") {
                capture
                    .state
                    .bookmarks
                    .iter()
                    .rev()
                    .find(|offset| **offset < at)
                    .or_else(|| capture.state.bookmarks.last())
            } else {
                capture
                    .state
                    .bookmarks
                    .iter()
                    .find(|offset| **offset > at)
                    .or_else(|| capture.state.bookmarks.first())
            };
            if let Some(caret) = next {
                selections = Selection {
                    anchor: *caret,
                    caret: *caret,
                }
                .into();
            }
        }
        "editor.lines.showAll" => capture.state.hidden.clear(),
        "editor.lines.hide" => {
            for selection in &selections.selections {
                let range = selection.range();
                let first = line_at(&capture, range.start, options)?;
                let last = line_at(
                    &capture,
                    range.end.saturating_sub(usize::from(!range.is_empty())),
                    options,
                )?;
                let first = first.max(1);
                if first <= last {
                    capture
                        .state
                        .hidden
                        .push(line_range(&capture, first, options)?.start..line_range(&capture, last, options)?.end);
                }
            }
            if capture.state.hidden.len() > limits.max_selections {
                return Err("Hidden range quota exceeded".into());
            }
        }
        "editor.bookmark.copyLines" | "editor.bookmark.cutLines" | "editor.bookmark.deleteLines" => {
            let mut text = String::new();
            for anchor in &capture.state.bookmarks {
                let range = line_range(&capture, line_at(&capture, *anchor, options)?, options)?;
                let body = read(
                    &capture,
                    range.clone(),
                    options,
                    (4usize << 20).saturating_sub(text.len()),
                )?;
                text.push_str(&body);
                if id != "editor.bookmark.copyLines" {
                    edits.push(Edit {
                        range: TextOffset(range.start)..TextOffset(range.end),
                        insert: String::new(),
                    });
                }
            }
            if id != "editor.bookmark.deleteLines" {
                clipboard = Some(text);
            }
            if !edits.is_empty() {
                selections = Selection {
                    anchor: edits[0].range.start.0,
                    caret: edits[0].range.start.0,
                }
                .into();
                capture.state.bookmarks.clear();
            }
        }
        "editor.selection.nextOccurrence" | "editor.selection.allOccurrences" | "editor.selection.skipOccurrence" => {
            if capture.state.occurrence_history.len() == 64 {
                capture.state.occurrence_history.remove(0);
            }
            capture.state.occurrence_history.push(selections.clone());
            selections = occurrences(&capture, id, options, limits)?;
        }
        _ => {
            let configured = if args.contains_key("first_line") {
                Some(rectangle(args)?)
            } else {
                capture.state.rectangle
            };
            let first = if let Some(rectangle) = configured {
                rectangle.first_line
            } else {
                line_at(
                    &capture,
                    selections
                        .selections
                        .iter()
                        .map(|selection| selection.anchor.min(selection.caret))
                        .min()
                        .unwrap(),
                    options,
                )?
                .saturating_sub(1)
            };
            let last = if let Some(rectangle) = configured {
                rectangle.last_line
            } else {
                let end = selections
                    .selections
                    .iter()
                    .map(|selection| selection.anchor.max(selection.caret))
                    .max()
                    .unwrap();
                line_at(&capture, end, options)?
            };
            let start = line_range(&capture, first, options)?.start;
            let last_range = line_range(&capture, last, options)?;
            let end = if configured.is_none() && last_range.end < capture.source.snapshot().len() {
                line_range(&capture, last + 1, options)?.end
            } else {
                last_range.end
            };
            let text = read(&capture, start..end, options, limit)?;
            let document =
                Document::from_utf8(&text, Budget::new(options.memory), Budget::new(options.memory)).map_err(err)?;
            let snapshot = document.snapshot();
            let local = if configured.is_some() {
                Selection { anchor: 0, caret: 0 }.into()
            } else {
                SelectionSet {
                    selections: selections
                        .selections
                        .iter()
                        .map(|selection| {
                            Ok(Selection {
                                anchor: selection
                                    .anchor
                                    .checked_sub(start)
                                    .ok_or("Selection precedes command range")?,
                                caret: selection
                                    .caret
                                    .checked_sub(start)
                                    .ok_or("Selection precedes command range")?,
                            })
                        })
                        .collect::<Result<_, String>>()?,
                    primary: selections.primary,
                }
            };
            let local_rectangle = configured.map(|rectangle| Rectangle {
                first_line: rectangle.first_line - first,
                last_line: rectangle.last_line - first,
                ..rectangle
            });
            let maps = capture.column_maps.as_ref().map(|maps| {
                maps.iter()
                    .filter_map(|(line, map)| line.checked_sub(first).map(|line| (line, map.clone())))
                    .collect::<std::collections::BTreeMap<_, _>>()
            });
            let mut mutation = None;
            let mut next = None;
            match id {
                "editor.indent" | "editor.unindent" => {
                    mutation = Some(
                        power::consumer::rectangle_indent_mapped(
                            &snapshot,
                            local_rectangle.ok_or("Missing captured rectangle")?,
                            id == "editor.unindent",
                            limits,
                            maps.as_ref(),
                        )
                        .map_err(err)?,
                    )
                }
                "editor.caret.above" | "editor.caret.below" => {
                    next = Some(power::add_caret(&snapshot, &local, id.ends_with("below"), limits).map_err(err)?)
                }
                "editor.caret.toggle" => {
                    next = Some(
                        power::toggle_caret(
                            &snapshot,
                            &local,
                            parameter::<usize>(args, "offset")?
                                .checked_sub(start)
                                .ok_or("Caret precedes command range")?,
                            limits,
                        )
                        .map_err(err)?,
                    )
                }
                "editor.selection.expandLines" => {
                    next = Some(power::expand_lines(&snapshot, &local, limits).map_err(err)?)
                }
                "editor.rectangle.select" => {
                    let rectangle = local_rectangle.ok_or("Missing rectangle")?;
                    let mut rows = Vec::new();
                    for number in rectangle.first_line..=rectangle.last_line {
                        let range = snapshot.line_range(number).map_err(err)?;
                        let body = snapshot.read(range.clone(), limit).map_err(err)?;
                        let fallback =
                            power::DisplayColumnMap::new(body.trim_end_matches(['\r', '\n']), capture.tab_width);
                        let map = if let Some(maps) = maps.as_ref() {
                            maps.get(&number).ok_or("Missing measured column row")?
                        } else {
                            &fallback
                        };
                        rows.push(Selection {
                            anchor: range.start.0 + map.at(rectangle.start_column).0,
                            caret: range.start.0 + map.at(rectangle.end_column).0,
                        });
                    }
                    next = Some(SelectionSet {
                        selections: rows,
                        primary: 0,
                    });
                    capture.state.rectangle = configured;
                }
                "editor.rectangle.copy" | "editor.rectangle.cut" => {
                    let rectangle = local_rectangle.ok_or("Missing rectangle")?;
                    clipboard =
                        Some(power::rectangle_copy_mapped(&snapshot, rectangle, limits, maps.as_ref()).map_err(err)?);
                    if id.ends_with("cut") {
                        mutation = Some(
                            power::rectangle_paste_mapped(&snapshot, rectangle, "", limits, maps.as_ref())
                                .map_err(err)?,
                        );
                    }
                }
                "editor.column.insert" => {
                    let value = if args.get("mode").map(String::as_str) == Some("text") {
                        power::ColumnInsert::Text(args["text"].clone())
                    } else {
                        power::ColumnInsert::Numbers {
                            start: parameter(args, "start")?,
                            step: parameter(args, "step")?,
                            width: parameter(args, "width")?,
                            base: parameter(args, "base")?,
                            repeat: parameter(args, "repeat")?,
                        }
                    };
                    mutation = Some(
                        power::column_insert_mapped(
                            &snapshot,
                            local_rectangle.ok_or("Missing rectangle")?,
                            value,
                            limits,
                            maps.as_ref(),
                        )
                        .map_err(err)?,
                    );
                }
                "editor.rectangle.paste"
                | "editor.rectangle.delete"
                | "editor.paste.plainText"
                | "editor.paste.fromHistory" => {
                    let text = if id.ends_with("delete") {
                        ""
                    } else {
                        args.get("text").ok_or("Missing captured paste")?
                    };
                    mutation = Some(
                        if let Some(rectangle) = local_rectangle {
                            power::rectangle_paste_mapped(&snapshot, rectangle, text, limits, maps.as_ref())
                        } else {
                            power::replace(&snapshot, &local, text, limits)
                        }
                        .map_err(err)?,
                    );
                    if let Some(rectangle) = configured {
                        for (key, value) in [
                            ("first_line", rectangle.first_line),
                            ("last_line", rectangle.last_line),
                            ("start_column", rectangle.start_column),
                            ("end_column", rectangle.end_column),
                        ] {
                            recorded.insert(key.into(), value.to_string());
                        }
                    }
                }
                "editor.comment.toggleLine" | "editor.comment.toggleBlock" => {
                    let tokens = if args.contains_key("line_prefix") || args.contains_key("block_start") {
                        power::CommentTokens {
                            line: args.get("line_prefix").cloned(),
                            block: args
                                .get("block_start")
                                .zip(args.get("block_end"))
                                .map(|(a, b)| (a.clone(), b.clone())),
                        }
                    } else {
                        use power::CommentProvider;
                        if let Some(definition) = capture.definition.as_deref() {
                            crate::completion::DefinitionComments(definition).tokens_for(&snapshot)
                        } else {
                            crate::completion::LanguageComments(capture.language).tokens_for(&snapshot)
                        }
                        .ok_or("No comment definition for this language")?
                    };
                    if let Some(prefix) = &tokens.line {
                        recorded.insert("line_prefix".into(), prefix.clone());
                    }
                    if let Some((open, close)) = &tokens.block {
                        recorded.insert("block_start".into(), open.clone());
                        recorded.insert("block_end".into(), close.clone());
                    }
                    struct Policy(power::CommentTokens);
                    impl power::CommentProvider for Policy {
                        fn tokens_for(&self, _: &bareline_document::DocumentSnapshot) -> Option<power::CommentTokens> {
                            Some(self.0.clone())
                        }
                    }
                    mutation = Some(
                        crate::completion::toggle_comment_with_provider(
                            &snapshot,
                            &local,
                            &Policy(tokens),
                            id.ends_with("toggleBlock"),
                            limits,
                        )
                        .map_err(err)?,
                    );
                }
                _ => return Err("Command has no paged preparation path".into()),
            }
            if let Some(prepared) = mutation {
                next = Some(prepared.selections);
                edits = prepared
                    .transaction
                    .edits
                    .into_iter()
                    .map(|edit| Edit {
                        range: TextOffset(start + edit.range.start.0)..TextOffset(start + edit.range.end.0),
                        insert: edit.insert,
                    })
                    .collect();
            }
            if let Some(next) = next {
                selections = SelectionSet {
                    selections: next
                        .selections
                        .into_iter()
                        .map(|selection| Selection {
                            anchor: start + selection.anchor,
                            caret: start + selection.caret,
                        })
                        .collect(),
                    primary: next.primary,
                };
            }
        }
    }
    let mut hidden_lines = Vec::new();
    for range in &capture.state.hidden {
        hidden_lines.push(
            line_at(&capture, range.start, options)? as u64
                ..line_at(&capture, range.end.saturating_sub(1), options)?.saturating_add(1) as u64,
        );
    }
    let clipboard_rectangle = clipboard
        .as_ref()
        .and(rectangle(&recorded).ok().or(capture.state.rectangle));
    let transaction = if edits.is_empty() {
        None
    } else {
        let compact = edits
            .iter()
            .map(|edit| bareline_document::change::CompactEdit {
                before: edit.range.clone(),
                inserted_len: edit.insert.len(),
            })
            .collect::<Vec<_>>();
        let transaction = stage(&capture, &selections, edits, options)?;
        capture.state.map_edits(&compact);
        hidden_lines.clear();
        Some(transaction)
    };
    Ok(PreparedPower {
        source: capture.source.snapshot().clone(),
        selections,
        state: capture.state,
        hidden_lines,
        transaction,
        clipboard,
        clipboard_rectangle,
        arguments: recorded,
    })
}
fn window(
    capture: &Capture,
    start: TextOffset,
    length: usize,
    options: &StagingOptions,
) -> Result<(TextOffset, String), String> {
    let mut request = capture
        .source
        .snapshot()
        .begin_viewport(start, length, &options.budget)
        .map_err(|e| format!("{e:?}"))?;
    loop {
        options.cancellation.check().map_err(|e| format!("{e:?}"))?;
        match request.poll() {
            bareline_document::paged::WindowPoll::Ready(window) => {
                return Ok((window.range().start, window.text().into()));
            }
            bareline_document::paged::WindowPoll::Pending(ticket) => {
                if !capture
                    .source
                    .resolve_captured_page(ticket)
                    .map_err(|error| error.to_string())?
                {
                    std::thread::yield_now();
                }
            }
            _ => return Err("Typing context unavailable".into()),
        }
    }
}
pub fn prepare_input(
    mut capture: Capture,
    input: crate::Input,
    options: &StagingOptions,
) -> Result<PreparedPower, String> {
    if let Some(rectangle) = capture.state.rectangle {
        let mut args = Arguments::new();
        for (key, value) in [
            ("first_line", rectangle.first_line),
            ("last_line", rectangle.last_line),
            ("start_column", rectangle.start_column),
            ("end_column", rectangle.end_column),
        ] {
            args.insert(key.into(), value.to_string());
        }
        let id = match input {
            crate::Input::Insert(text) => {
                args.insert("text".into(), text);
                "editor.rectangle.paste"
            }
            crate::Input::Backspace | crate::Input::Delete => "editor.rectangle.delete",
            _ => return Err("Unsupported rectangle input".into()),
        };
        return prepare(capture, id, &args, options);
    }
    let _claim = options
        .budget
        .claim(options.memory)
        .map_err(|e| format!("Input memory quota: {e:?}"))?;
    let mut edits = Vec::new();
    let mut after = capture.selections.selections.clone();
    let mut delta = 0isize;
    let mut ordered = capture.selections.selections.iter().enumerate().collect::<Vec<_>>();
    ordered.sort_by_key(|(_, selection)| selection.range().start);
    for (index, selection) in ordered {
        let mut config = capture.typing.clone();
        config.literal_context = capture.literal_contexts.get(index).copied().flatten();
        let plan = crate::paged_typing::prepare(
            capture.source.snapshot(),
            *selection,
            crate::paged_typing::TypingRequest::Input(input.clone()),
            &config,
            &options.cancellation,
            |start, length| window(&capture, start, length, options),
        )?;
        let (mut planned, next) = if let Some(plan) = plan {
            (plan.transaction.edits, plan.selection)
        } else {
            let range = selection.range();
            match &input {
                crate::Input::Insert(text) => (
                    vec![Edit {
                        range: TextOffset(range.start)..TextOffset(range.end),
                        insert: text.clone(),
                    }],
                    Selection {
                        anchor: range.start + text.len(),
                        caret: range.start + text.len(),
                    },
                ),
                crate::Input::Backspace | crate::Input::Delete if !range.is_empty() => (
                    vec![Edit {
                        range: TextOffset(range.start)..TextOffset(range.end),
                        insert: String::new(),
                    }],
                    Selection {
                        anchor: range.start,
                        caret: range.start,
                    },
                ),
                crate::Input::Backspace | crate::Input::Delete => {
                    let (origin, text) =
                        window(&capture, TextOffset(range.start.saturating_sub(65536)), 131072, options)?;
                    let document = Document::from_utf8(&text, Budget::new(1 << 20), Budget::new(1 << 20))
                        .map_err(|e| format!("{e:?}"))?;
                    let local = Selection {
                        anchor: range.start - origin.0,
                        caret: range.start - origin.0,
                    };
                    let prepared = power::delete(
                        &document.snapshot(),
                        &local.into(),
                        matches!(input, crate::Input::Backspace),
                        power::Limits::default(),
                    )
                    .map_err(|e| format!("{e:?}"))?;
                    let next = prepared.selections.primary();
                    (
                        prepared
                            .transaction
                            .edits
                            .into_iter()
                            .map(|edit| Edit {
                                range: TextOffset(edit.range.start.0 + origin.0)
                                    ..TextOffset(edit.range.end.0 + origin.0),
                                insert: edit.insert,
                            })
                            .collect(),
                        Selection {
                            anchor: next.anchor + origin.0,
                            caret: next.caret + origin.0,
                        },
                    )
                }
                _ => return Err("Unsupported paged power input".into()),
            }
        };
        after[index] = Selection {
            anchor: next
                .anchor
                .checked_add_signed(delta)
                .ok_or("Input selection overflow")?,
            caret: next.caret.checked_add_signed(delta).ok_or("Input selection overflow")?,
        };
        for edit in &planned {
            delta = delta
                .checked_add(edit.insert.len() as isize - (edit.range.end.0 - edit.range.start.0) as isize)
                .ok_or("Input edit overflow")?;
        }
        edits.append(&mut planned);
    }
    let selections = SelectionSet {
        selections: after,
        primary: capture.selections.primary,
    };
    let transaction = if edits.is_empty() {
        None
    } else {
        let compact = edits
            .iter()
            .map(|edit| bareline_document::change::CompactEdit {
                before: edit.range.clone(),
                inserted_len: edit.insert.len(),
            })
            .collect::<Vec<_>>();
        let transaction = stage(&capture, &selections, edits, options)?;
        capture.state.map_edits(&compact);
        Some(transaction)
    };
    Ok(PreparedPower {
        source: capture.source.snapshot().clone(),
        selections,
        state: capture.state,
        hidden_lines: Vec::new(),
        transaction,
        clipboard: None,
        clipboard_rectangle: None,
        arguments: Arguments::new(),
    })
}
fn occurrences(
    capture: &Capture,
    id: &str,
    options: &StagingOptions,
    limits: power::Limits,
) -> Result<SelectionSet, String> {
    use unicode_segmentation::UnicodeSegmentation;
    let selected = capture.selections.primary().range();
    let needle = read(capture, selected.clone(), options, limits.max_bytes)?;
    if needle.is_empty() {
        let caret = capture.selections.primary().caret;
        let length = capture.source.snapshot().len();
        let mut request = capture
            .source
            .snapshot()
            .begin_viewport(
                TextOffset(caret.saturating_sub(16 * 1024)),
                32 * 1024 + 8,
                &options.budget,
            )
            .map_err(|error| format!("{error:?}"))?;
        let window = loop {
            options.cancellation.check().map_err(|_| "Word selection cancelled")?;
            match request.poll() {
                bareline_document::paged::WindowPoll::Ready(window) => break window,
                bareline_document::paged::WindowPoll::Pending(_) => std::thread::yield_now(),
                _ => return Err("Word selection source is unavailable".into()),
            }
        };
        let start = window.range().start.0;
        let end = window.range().end.0;
        let text = window.text();
        let mut selections = capture.selections.clone();
        if let Some((at, word)) = text
            .unicode_word_indices()
            .find(|(at, word)| start + at <= caret && caret < start + at + word.len())
        {
            let from = start + at;
            let to = from + word.len();
            if (from > start || start == 0) && (to < end || end == length) {
                selections.selections[selections.primary] = Selection {
                    anchor: from,
                    caret: to,
                };
            }
        }
        return Ok(selections);
    }
    if needle.len() > options.memory / 16 {
        return Err("Occurrence pattern exceeds its memory quota".into());
    }
    let pattern = needle.as_bytes();
    let mut failure = vec![0usize; pattern.len()];
    let mut matched = 0;
    for index in 1..pattern.len() {
        while matched > 0 && pattern[index] != pattern[matched] {
            matched = failure[matched - 1];
        }
        if pattern[index] == pattern[matched] {
            matched += 1;
        }
        failure[index] = matched;
    }
    matched = 0;
    let mut position = 0usize;
    let mut matches = Vec::new();
    let mut boundaries = std::collections::VecDeque::from([0usize]);
    let mut retained = String::new();
    let mut bytes = [0u8; 16 * 1024];
    let mut utf8 = Vec::new();
    let mut reader = CapturedRangeReader::new(
        capture.source.clone(),
        TextOffset(0)..TextOffset(capture.source.snapshot().len()),
        options.budget.clone(),
        options.cancellation.clone(),
    )
    .map_err(|e| e.to_string())?;
    loop {
        options.cancellation.check().map_err(|e| format!("{e:?}"))?;
        let count = reader.read(&mut bytes).map_err(|e| e.to_string())?;
        utf8.extend_from_slice(&bytes[..count]);
        let valid = match std::str::from_utf8(&utf8) {
            Ok(_) => utf8.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return Err("Invalid occurrence source UTF-8".into()),
        };
        retained.push_str(std::str::from_utf8(&utf8[..valid]).map_err(|e| e.to_string())?);
        utf8.drain(..valid);
        if retained.len() > limits.max_bytes {
            return Err("Grapheme context exceeds occurrence quota".into());
        }
        let cut = if count == 0 {
            retained.len()
        } else {
            retained.grapheme_indices(true).last().map_or(0, |(index, _)| index)
        };
        for grapheme in retained[..cut].graphemes(true) {
            boundaries.push_back(position);
            while boundaries
                .front()
                .is_some_and(|start| position.saturating_sub(*start) > pattern.len())
            {
                boundaries.pop_front();
            }
            for (index, byte) in grapheme.bytes().enumerate() {
                while matched > 0 && byte != pattern[matched] {
                    matched = failure[matched - 1];
                }
                if byte == pattern[matched] {
                    matched += 1;
                }
                position += 1;
                if matched == pattern.len() {
                    let start = position - pattern.len();
                    if index + 1 == grapheme.len() && boundaries.contains(&start) {
                        if matches.len() == limits.max_selections {
                            return Err("Occurrence selection quota exceeded".into());
                        }
                        matches.push(start..position);
                    }
                    matched = failure[matched - 1];
                }
            }
        }
        retained.drain(..cut);
        if count == 0 {
            if !utf8.is_empty() {
                return Err("Truncated occurrence source".into());
            }
            break;
        }
    }
    let mut out = capture.selections.clone();
    let all = id.ends_with("allOccurrences");
    let candidates = matches
        .iter()
        .filter(|range| all || range.start >= selected.end)
        .chain(matches.iter().filter(|range| !all && range.start < selected.end));
    let mut chosen = None;
    for range in candidates {
        if out.selections.iter().any(|selection| selection.range() == *range) {
            continue;
        }
        chosen = Some(Selection {
            anchor: range.start,
            caret: range.end,
        });
        if all {
            out.selections.push(chosen.unwrap());
        } else {
            break;
        }
    }
    if !all {
        if let Some(selection) = chosen {
            if id.ends_with("skipOccurrence") {
                out.selections[out.primary] = selection;
            } else {
                out.selections.push(selection);
                out.primary = out.selections.len() - 1;
            }
        }
    }
    let primary = out.primary();
    out.selections.sort_by_key(|selection| {
        (
            selection.anchor.min(selection.caret),
            selection.anchor.max(selection.caret),
        )
    });
    out.selections.dedup();
    out.primary = out
        .selections
        .iter()
        .position(|selection| *selection == primary)
        .unwrap_or(0);
    Ok(out)
}
fn stage(
    capture: &Capture,
    after: &SelectionSet,
    edits: Vec<Edit>,
    options: &StagingOptions,
) -> Result<PreparedSourceTransaction, String> {
    use bareline_file_io::owned_store::StreamingStoreBuilder;
    let builder = || {
        StreamingStoreBuilder::new(
            &options.cache,
            options.quota / 2,
            options.platform.clone(),
            options.source_options,
            options.budget.clone(),
            options.cancellation.clone(),
        )
        .map_err(|e| e.to_string())
    };
    let mut inverse = builder()?;
    let mut inserted = builder()?;
    let mut ranges = Vec::new();
    for edit in edits {
        let start = inverse.len();
        let mut reader = CapturedRangeReader::new(
            capture.source.clone(),
            edit.range.clone(),
            options.budget.clone(),
            options.cancellation.clone(),
        )
        .map_err(|e| e.to_string())?;
        std::io::copy(&mut reader, &mut inverse).map_err(|e| e.to_string())?;
        let removed = start..inverse.len();
        let added = inserted.append_utf8(&edit.insert).map_err(|e| e.to_string())?;
        ranges.push((edit.range, removed, added));
    }
    let inverse = inverse.finish().map_err(|e| e.to_string())?;
    let inserted = inserted.finish().map_err(|e| e.to_string())?;
    let edits = ranges
        .into_iter()
        .map(|(range, removed, added)| SourceEdit {
            range,
            inverse: OwnedTextRange {
                source: inverse.clone(),
                range: removed,
            },
            inserted: OwnedTextRange {
                source: inserted.clone(),
                range: added,
            },
        })
        .collect();
    let convert = |set: &SelectionSet| {
        let primary = set.primary();
        std::iter::once(primary)
            .chain(
                set.selections
                    .iter()
                    .copied()
                    .enumerate()
                    .filter(|(index, _)| *index != set.primary)
                    .map(|(_, selection)| selection),
            )
            .map(|selection| bareline_document::history::Selection {
                anchor: TextOffset(selection.anchor),
                caret: TextOffset(selection.caret),
            })
            .collect()
    };
    let metadata = EditMetadata {
        before: convert(&capture.selections),
        after: convert(after),
        origin: EditOrigin::Command,
        boundary: power::consumer::next_receipt_sequence(),
        ..Default::default()
    };
    let mut request = capture
        .source
        .snapshot()
        .prepare_source_transaction(edits, metadata, options.budget.clone())
        .map_err(|e| format!("{e:?}"))?;
    loop {
        options.cancellation.check().map_err(|e| format!("{e:?}"))?;
        match request.poll() {
            SourceTransactionPoll::Ready(prepared) => return Ok(prepared),
            SourceTransactionPoll::Progress => {}
            SourceTransactionPoll::Pending(ticket) => {
                if !request.resolve_owned(ticket).map_err(|e| format!("{e:?}"))?
                    && !capture
                        .source
                        .resolve_captured_page(ticket)
                        .map_err(|error| error.to_string())?
                {
                    std::thread::yield_now();
                }
            }
            _ => return Err("Paged power transaction validation failed".into()),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_comment_and_rectangle_arguments_are_explicit() {
        let args = [("line_prefix".into(), "--".into())].into_iter().collect();
        assert!(validate_arguments("editor.comment.toggleLine", &args).is_ok());
        let missing = [("block_start".into(), "/*".into())].into_iter().collect();
        assert!(validate_arguments("editor.comment.toggleBlock", &missing).is_err());
        let rectangle = [
            ("text".into(), "literal".into()),
            ("first_line".into(), "40000".into()),
            ("last_line".into(), "40002".into()),
            ("start_column".into(), "2".into()),
            ("end_column".into(), "4".into()),
        ]
        .into_iter()
        .collect();
        assert!(validate_arguments("editor.paste.fromHistory", &rectangle).is_ok());
        assert!(validate_arguments("editor.paste.fromHistory", &Arguments::new()).is_err());
        assert!(supports_command("editor.bookmark.selectLines"));
    }
    #[test]
    fn view_anchors_follow_actual_delta_and_undo_restores_removed_bookmarks() {
        let mut document = Document::from_utf8("a\nb\nc\n", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let before = document.snapshot();
        let mut state = PowerViewState {
            bookmarks: vec![2, 4],
            hidden: vec![4..6],
            ..Default::default()
        };
        let mut history = PowerStateHistory::default();
        document
            .apply(bareline_document::EditTransaction {
                base_revision: before.revision,
                edits: vec![Edit {
                    range: TextOffset(2)..TextOffset(4),
                    insert: String::new(),
                }],
            })
            .unwrap();
        let after = document.snapshot();
        history.transition(
            before.content_state,
            after.content_state,
            &mut state,
            after.applied_change().unwrap(),
        );
        assert_eq!(state.bookmarks, vec![2]);
        assert_eq!(state.hidden, vec![2..4]);
        document.undo().unwrap();
        let undone = document.snapshot();
        history.transition(
            after.content_state,
            undone.content_state,
            &mut state,
            undone.applied_change().unwrap(),
        );
        assert_eq!(state.bookmarks, vec![2, 4]);
        assert_eq!(state.hidden, vec![4..6]);
    }
}
