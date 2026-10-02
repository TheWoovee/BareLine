// SPDX-License-Identifier: MPL-2.0
//! Bounded, source-identified projection; omitted fold bodies never become bytes.
use super::{JobCompletion, PagedReadHandle, worker};
use crate::paged_navigation::SharedLineIndex;
use bareline_document::{
    Budget, BudgetClaim, DocumentBuilder, DocumentSnapshot, TextOffset,
    line_lookup::{LineLookupPoll, LineTarget},
    paged::{LineCheckpoint, PagedSnapshot, WindowPoll},
};
use bareline_file_io::cancellation::Cancellation;
use bareline_platform::executor::WorkKind;
use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};
const BYTES: usize = 64 * 1024;
const PIECES: usize = 128;
/// Source bytes on either side of the visible window whose folds a mapping
/// resolves; hidden lines and carried folds further away are not looked up
/// (PED-07).
const MARGIN: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceAffinity {
    Before,
    After,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewportSegment {
    pub local: Range<TextOffset>,
    pub source: Range<TextOffset>,
    pub first_global_line: Option<u64>,
    pub source_line_start: Option<TextOffset>,
}
pub struct MappedViewport {
    pub source: PagedSnapshot,
    pub generation: u64,
    pub projection: DocumentSnapshot,
    pub segments: Vec<ViewportSegment>,
    pub anchors: Vec<FoldAnchor>,
    /// Carried folds too far from the window to resolve, unchanged; the view
    /// keeps them for a mapping that reaches them (PED-07).
    pub deferred: Vec<FoldAnchor>,
    _claim: BudgetClaim,
}
#[derive(Clone)]
pub struct FoldAnchor {
    pub header: TextOffset,
    /// Start of the first body line; the collapsed body is `body..end`.
    pub body: TextOffset,
    pub end: TextOffset,
    pub fold: bareline_syntax::folding::Fold,
    pub collapsed: bool,
    /// Whether `fold` holds this anchor's lines in the text it describes. A
    /// carried fold whose line shift could not be counted keeps only its bytes
    /// until a mapping looks it up (PED-07).
    pub lines_known: bool,
}
impl MappedViewport {
    pub fn source_offset(&self, local: TextOffset, affinity: SourceAffinity) -> Option<TextOffset> {
        let segment = match affinity {
            SourceAffinity::Before => self
                .segments
                .iter()
                .find(|s| s.local.start < local && local <= s.local.end)
                .or_else(|| self.segments.first().filter(|s| s.local.start == local)),
            SourceAffinity::After => self
                .segments
                .iter()
                .find(|s| s.local.start <= local && local < s.local.end)
                .or_else(|| self.segments.last().filter(|s| s.local.end == local)),
        }?;
        Some(TextOffset(segment.source.start.0 + local.0 - segment.local.start.0))
    }
    pub fn local_offset(&self, source: TextOffset) -> Option<TextOffset> {
        let segment = self
            .segments
            .iter()
            .find(|s| s.source.start <= source && source <= s.source.end)?;
        Some(TextOffset(segment.local.start.0 + source.0 - segment.source.start.0))
    }
}
/// The text a mapping job projects: the generation its result carries and the
/// document's shared line index, which fold lookups resume from instead of
/// scanning from byte zero (PED-07).
pub struct MappedText {
    pub generation: u64,
    pub line_index: SharedLineIndex,
    /// Collapsed folds, by header and end line, that the view knows without
    /// byte anchors. Their anchors are found wherever they are: without one
    /// the view could neither carry the fold through an edit nor reveal it.
    pub unanchored: Vec<(usize, usize)>,
}
pub struct MappingJob {
    pub result: Receiver<Result<MappedViewport, String>>,
    cancel: Cancellation,
}
impl Drop for MappingJob {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
pub fn request(
    handle: PagedReadHandle,
    start: TextOffset,
    folds: Vec<bareline_syntax::folding::Fold>,
    manual: Vec<Range<usize>>,
    rebased: Vec<FoldAnchor>,
    text: MappedText,
    budget: Budget,
    notify: Arc<dyn Fn() + Send + Sync>,
) -> Result<MappingJob, String> {
    let cancel = Cancellation::default();
    let cancellation = cancel.clone();
    let (sender, result) = mpsc::sync_channel(1);
    worker()
        .submit(
            WorkKind::Interactive,
            Box::new(move || {
                let completion = JobCompletion::new(sender, notify);
                let result = build(&handle, start, folds, manual, rebased, &text, &budget, &cancellation);
                completion.complete(result);
            }),
        )
        .map_err(|_| "Viewport worker queue is full".to_owned())?;
    Ok(MappingJob { result, cancel })
}
/// Fold lookups through the document's shared line index. Each resumes from
/// the previous lookup's verified position when that is closer, so none scans
/// from byte zero once the index covers the text (PED-07). Answers are kept for
/// the job: the start of a line found once also gives that offset's line.
struct Lookups<'a> {
    handle: &'a PagedReadHandle,
    index: &'a SharedLineIndex,
    budget: &'a Budget,
    cancel: &'a Cancellation,
    hint: Option<LineCheckpoint>,
    lines: HashMap<usize, usize>,
    starts: HashMap<usize, TextOffset>,
}
impl Lookups<'_> {
    fn lookup(&mut self, target: LineTarget) -> Result<LineLookupPoll, String> {
        let cancel = self.cancel;
        let result = self
            .index
            .lookup(self.handle, target, self.budget, &mut self.hint, &mut || {
                cancel.check().map_err(|e| format!("Fold lookup: {e}"))
            })?;
        match result {
            LineLookupPoll::Line(_) | LineLookupPoll::Range(_) => Ok(result),
            _ => Err(format!("Folded view unavailable: {}.", result.failure_message())),
        }
    }
    fn line(&mut self, offset: TextOffset) -> Result<usize, String> {
        if let Some(line) = self.lines.get(&offset.0) {
            return Ok(*line);
        }
        match self.lookup(LineTarget::Byte(offset))? {
            LineLookupPoll::Line(line) => {
                self.lines.insert(offset.0, line);
                Ok(line)
            }
            _ => Err("Fold line unavailable".into()),
        }
    }
    fn line_start(&mut self, line: usize) -> Result<TextOffset, String> {
        if let Some(start) = self.starts.get(&line) {
            return Ok(*start);
        }
        match self.lookup(LineTarget::Line(line))? {
            LineLookupPoll::Range(range) => {
                self.starts.insert(line, range.start);
                self.lines.insert(range.start.0, line);
                Ok(range.start)
            }
            _ => Err("Fold line unavailable".into()),
        }
    }
}
fn build(
    handle: &PagedReadHandle,
    start: TextOffset,
    mut folds: Vec<bareline_syntax::folding::Fold>,
    manual: Vec<Range<usize>>,
    rebased: Vec<FoldAnchor>,
    text: &MappedText,
    budget: &Budget,
    cancel: &Cancellation,
) -> Result<MappedViewport, String> {
    let claim = budget
        .claim(PIECES * std::mem::size_of::<ViewportSegment>() + (folds.len() + manual.len() + rebased.len()) * 128)
        .map_err(|e| format!("Viewport map: {e}"))?;
    let len = handle.snapshot().len();
    let mut index = Lookups {
        handle,
        index: &text.line_index,
        budget,
        cancel,
        hint: None,
        lines: HashMap::new(),
        starts: HashMap::new(),
    };
    // Only folds near the window are looked up, so a mapping reads about the
    // window and its margin, not every fold of the document (PED-07). Hidden
    // lines that end before the window's first line never reach the walk.
    let first_line = index.line(start)?;
    let near = start.0.saturating_sub(MARGIN);
    let mut reach = start.0.saturating_add(BYTES + MARGIN).min(len);
    let mut resolved = vec![false; rebased.len()];
    let mut anchors = Vec::new();
    // Carried collapsed folds resolved here, with their new lines. Each returns
    // as an anchor even when its body lies past the hidden ranges looked up.
    let mut carried = Vec::new();
    let (gaps, mut collapsed) = loop {
        for (anchor, done) in rebased.iter().zip(resolved.iter_mut()) {
            if *done || anchor.end.0 < near || anchor.header.0 > reach {
                continue;
            }
            *done = true;
            let header = index.line(anchor.header)?;
            let after = index.line(anchor.end)?;
            let end = if anchor.end.0 == len {
                after
            } else {
                after.saturating_sub(1)
            };
            if header < end {
                let fold = bareline_syntax::folding::Fold {
                    header,
                    end,
                    level: anchor.fold.level,
                };
                let located = FoldAnchor {
                    header: anchor.header,
                    body: anchor.body,
                    end: anchor.end,
                    fold: fold.clone(),
                    collapsed: anchor.collapsed,
                    lines_known: true,
                };
                if anchor.collapsed {
                    folds.push(fold);
                    carried.push(located);
                } else {
                    anchors.push(located);
                }
            }
        }
        folds.sort_by_key(|fold| (fold.header, fold.end));
        folds.dedup_by_key(|fold| (fold.header, fold.end));
        let mut hidden: Vec<_> = folds
            .iter()
            .map(|fold| fold.header + 1..fold.end + 1)
            .chain(manual.iter().cloned())
            .filter(|range| range.end > first_line)
            .collect();
        hidden.sort_by_key(|range| (range.start, range.end));
        // A line after the one holding `reach` starts past it, so ranges are
        // compared by line and those past it are never looked up.
        let reach_line = index.line(TextOffset(reach))?;
        let mut gaps: Vec<Range<TextOffset>> = Vec::new();
        let mut collapsed = Vec::new();
        for range in hidden {
            cancel.check().map_err(|e| format!("Fold mapping: {e}"))?;
            // In line order, every later range starts past the reach too.
            if range.start > reach_line {
                break;
            }
            let first = index.line_start(range.start)?;
            let last = match index.line_start(range.end) {
                Ok(offset) => offset,
                Err(error) => {
                    // The line after the last one starts at the end of the text. The
                    // failed lookup ended there, so this one reads nothing.
                    let end = TextOffset(len);
                    if index.line(end)?.checked_add(1) == Some(range.end) {
                        end
                    } else {
                        return Err(error);
                    }
                }
            };
            if let Some(fold) = folds
                .iter()
                .find(|fold| fold.header + 1 == range.start && fold.end + 1 == range.end)
            {
                collapsed.push(FoldAnchor {
                    header: index.line_start(fold.header)?,
                    body: first,
                    end: last,
                    fold: fold.clone(),
                    collapsed: true,
                    lines_known: true,
                });
            }
            if first < last {
                if let Some(previous) = gaps.last_mut()
                    && first <= previous.end
                {
                    previous.end = previous.end.max(last);
                } else {
                    gaps.push(first..last);
                }
            }
        }
        // Folds not looked up start past the reach; once the projection ends
        // within it, none of them can hide any of its text.
        let end = walk_end(start.0, &gaps, len);
        if end <= reach {
            break (gaps, collapsed);
        }
        // Hidden text pushed the projection past the reach: widen it, at least
        // doubling, so a run of large folds takes few passes.
        reach = end
            .saturating_add(MARGIN)
            .max(reach.saturating_add(reach.saturating_sub(start.0)))
            .min(len);
    };
    // A carried collapsed fold whose header shares the line holding the reach
    // starts its hidden range past that line, so the walk did not look it up;
    // it keeps its carried bytes rather than leave the view's state.
    let mut found: std::collections::HashSet<_> = collapsed
        .iter()
        .map(|anchor| (anchor.fold.header, anchor.fold.end))
        .collect();
    for anchor in carried {
        if found.insert((anchor.fold.header, anchor.fold.end)) {
            collapsed.push(anchor);
        }
    }
    // Collapsed folds the view knows only by line get anchors wherever they
    // are, once: the view keeps the anchors this mapping returns.
    let unanchored: std::collections::HashSet<_> = text.unanchored.iter().copied().collect();
    if !unanchored.is_empty() {
        for fold in &folds {
            if !unanchored.contains(&(fold.header, fold.end)) || found.contains(&(fold.header, fold.end)) {
                continue;
            }
            cancel.check().map_err(|e| format!("Fold mapping: {e}"))?;
            let body = index.line_start(fold.header + 1)?;
            let end = match index.line_start(fold.end + 1) {
                Ok(offset) => offset,
                Err(error) => {
                    let end = TextOffset(len);
                    if index.line(end)? == fold.end {
                        end
                    } else {
                        return Err(error);
                    }
                }
            };
            anchors.push(FoldAnchor {
                header: index.line_start(fold.header)?,
                body,
                end,
                fold: fold.clone(),
                collapsed: true,
                lines_known: true,
            });
        }
    }
    anchors.extend(collapsed);
    let deferred = rebased
        .into_iter()
        .zip(resolved)
        .filter_map(|(anchor, done)| (!done).then_some(anchor))
        .collect();
    let mut builder =
        DocumentBuilder::new(budget.clone(), Budget::new(0)).map_err(|e| format!("Fold projection: {e}"))?;
    let mut segments = Vec::new();
    let mut cursor = start;
    let mut bytes = 0;
    while cursor.0 < handle.snapshot().len() && bytes < BYTES && segments.len() < PIECES {
        if let Some(gap) = gaps.iter().find(|gap| gap.start <= cursor && cursor < gap.end) {
            cursor = gap.end;
            continue;
        }
        let end = gaps
            .iter()
            .find(|gap| gap.start > cursor)
            .map_or(handle.snapshot().len(), |gap| gap.start.0)
            .min(cursor.0.saturating_add(BYTES - bytes));
        let mut request = handle
            .snapshot()
            .begin_line_viewport(cursor, end - cursor.0, budget)
            .map_err(|e| format!("Fold window: {e}"))?;
        let window = loop {
            cancel.check().map_err(|e| format!("Fold window: {e}"))?;
            match request.poll() {
                WindowPoll::Ready(window) => break window,
                WindowPoll::Pending(ticket) => {
                    if !handle
                        .resolve_captured_page(ticket)
                        .map_err(|error| error.to_string())?
                    {
                        std::thread::yield_now();
                    }
                }
                _ => return Err("Fold projection source unavailable".into()),
            }
        };
        if window.text().is_empty() {
            break;
        }
        let range = window.range();
        let line = index.line(range.start)?;
        let source_line_start = index.line_start(line)?;
        builder
            .append(window.text())
            .map_err(|e| format!("Fold projection: {e}"))?;
        segments.push(ViewportSegment {
            local: TextOffset(bytes)..TextOffset(bytes + window.text().len()),
            source: range.clone(),
            first_global_line: Some(line as u64),
            source_line_start: Some(source_line_start),
        });
        bytes += window.text().len();
        cursor = range.end;
    }
    Ok(MappedViewport {
        source: handle.snapshot().clone(),
        generation: text.generation,
        projection: builder.prefix(),
        segments,
        anchors,
        deferred,
        _claim: claim,
    })
}
/// Where a walk from `start` that skips `gaps` (in order, merged) has passed
/// `BYTES` visible bytes, or the end of the text. Window reads only trim their
/// edges, so the projection never reaches past it.
fn walk_end(start: usize, gaps: &[Range<TextOffset>], len: usize) -> usize {
    let (mut cursor, mut left) = (start, BYTES);
    for gap in gaps {
        if gap.end.0 <= cursor {
            continue;
        }
        if gap.start.0 > cursor {
            let visible = gap.start.0 - cursor;
            if visible >= left {
                return cursor + left;
            }
            left -= visible;
        }
        cursor = gap.end.0;
    }
    cursor.saturating_add(left).min(len)
}
