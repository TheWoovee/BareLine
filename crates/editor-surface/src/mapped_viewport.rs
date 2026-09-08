// SPDX-License-Identifier: MPL-2.0
//! Bounded, source-identified projection; omitted fold bodies never become bytes.
use super::{PagedReadHandle, worker};
use bareline_document::{Budget, BudgetClaim, DocumentBuilder, DocumentSnapshot, TextOffset, line_lookup::{LineLookupPoll, LineTarget}, paged::{PagedSnapshot, SparseLineIndex, WindowPoll}};
use bareline_file_io::cancellation::Cancellation;
use std::{ops::Range, sync::{Arc, mpsc::{self, Receiver}}};
const BYTES: usize = 64 * 1024;
const PIECES: usize = 128;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceAffinity { Before, After }
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
    _claim: BudgetClaim,
}
#[derive(Clone)]
pub struct FoldAnchor { pub header: TextOffset, pub end: TextOffset, pub fold: bareline_syntax::folding::Fold, pub collapsed: bool }
impl MappedViewport {
    pub fn source_offset(&self, local: TextOffset, affinity: SourceAffinity) -> Option<TextOffset> {
        let segment = match affinity {
            SourceAffinity::Before => self.segments.iter().find(|s| s.local.start < local && local <= s.local.end).or_else(|| self.segments.first().filter(|s| s.local.start == local)),
            SourceAffinity::After => self.segments.iter().find(|s| s.local.start <= local && local < s.local.end).or_else(|| self.segments.last().filter(|s| s.local.end == local)),
        }?;
        Some(TextOffset(segment.source.start.0 + local.0 - segment.local.start.0))
    }
    pub fn local_offset(&self, source: TextOffset) -> Option<TextOffset> {
        let segment = self.segments.iter().find(|s| s.source.start <= source && source <= s.source.end)?;
        Some(TextOffset(segment.local.start.0 + source.0 - segment.source.start.0))
    }
}
pub struct MappingJob { pub result: Receiver<Result<MappedViewport, String>>, cancel: Cancellation }
impl Drop for MappingJob { fn drop(&mut self) { self.cancel.cancel(); } }
pub fn request(handle: PagedReadHandle, start: TextOffset, folds: Vec<bareline_syntax::folding::Fold>, manual: Vec<Range<usize>>, rebased: Vec<FoldAnchor>, generation: u64, budget: Budget, notify: Arc<dyn Fn() + Send + Sync>) -> Result<MappingJob, String> {
    let cancel = Cancellation::default(); let cancellation = cancel.clone();
    let (sender, result) = mpsc::sync_channel(1);
    worker().try_send(Box::new(move || { let result = build(&handle, start, folds, manual, rebased, generation, &budget, &cancellation); let _ = sender.try_send(result); notify(); })).map_err(|_| "Viewport worker queue is full".to_owned())?;
    Ok(MappingJob { result, cancel })
}
fn lookup(handle: &PagedReadHandle, index: &mut SparseLineIndex, target: LineTarget, budget: &Budget, cancel: &Cancellation) -> Result<LineLookupPoll, String> {
    let mut request = index.lookup(target, budget.clone()).map_err(|e| format!("Fold lookup: {e:?}"))?;
    loop {
        cancel.check().map_err(|e| format!("Fold lookup: {e:?}"))?;
        let result = request.poll(); index.retain_lookup_progress(&request).map_err(|e| format!("Fold checkpoint: {e:?}"))?;
        match result {
            LineLookupPoll::Line(_) | LineLookupPoll::Range(_) => return Ok(result),
            LineLookupPoll::Progress(_) => {},
            LineLookupPoll::Pending(ticket) => { if !handle.resolve_captured_page(ticket)? { std::thread::yield_now(); } },
            _ => return Err(format!("Fold source unavailable: {result:?}")),
        }
    }
}
fn line_start(handle: &PagedReadHandle, index: &mut SparseLineIndex, line: usize, budget: &Budget, cancel: &Cancellation) -> Result<TextOffset, String> {
    match lookup(handle, index, LineTarget::Line(line), budget, cancel)? { LineLookupPoll::Range(range) => Ok(range.start), _ => Err("Fold line unavailable".into()) }
}
fn build(handle: &PagedReadHandle, start: TextOffset, mut folds: Vec<bareline_syntax::folding::Fold>, manual: Vec<Range<usize>>, rebased: Vec<FoldAnchor>, generation: u64, budget: &Budget, cancel: &Cancellation) -> Result<MappedViewport, String> {
    let claim = budget.claim(PIECES * std::mem::size_of::<ViewportSegment>() + (folds.len() + manual.len() + rebased.len()) * 128).map_err(|e| format!("Viewport map: {e:?}"))?;
    let mut index = SparseLineIndex::new(handle.snapshot().clone(), 256, BYTES, budget).map_err(|e| format!("Fold index: {e:?}"))?;
    let mut gaps: Vec<Range<TextOffset>> = Vec::new();
    let mut anchors = Vec::new();
    for anchor in rebased {
        let header = match lookup(handle, &mut index, LineTarget::Byte(anchor.header), budget, cancel)? { LineLookupPoll::Line(line) => line, _ => return Err("Fold header unavailable".into()) };
        let after = match lookup(handle, &mut index, LineTarget::Byte(anchor.end), budget, cancel)? { LineLookupPoll::Line(line) => line, _ => return Err("Fold end unavailable".into()) };
        let end = if anchor.end.0 == handle.snapshot().len() { after } else { after.saturating_sub(1) };
        if header < end {
            let fold = bareline_syntax::folding::Fold { header, end, level: anchor.fold.level };
            if anchor.collapsed { folds.push(fold); } else { anchors.push(FoldAnchor { header: anchor.header, end: anchor.end, fold, collapsed: false }); }
        }
    }
    folds.sort_by_key(|fold| (fold.header, fold.end)); folds.dedup_by_key(|fold| (fold.header, fold.end));
    let mut hidden: Vec<_> = folds.iter().map(|fold| fold.header + 1..fold.end + 1).chain(manual).collect(); hidden.sort_by_key(|range| (range.start, range.end));
    for range in hidden {
        cancel.check().map_err(|e| format!("Fold mapping: {e:?}"))?;
        let first = line_start(handle, &mut index, range.start, budget, cancel)?;
        let last = match line_start(handle, &mut index, range.end, budget, cancel) { Ok(offset) => offset, Err(error) => {
            if matches!(index.line_count(), bareline_document::paged::LineCount::Known(count) if range.end == count) { TextOffset(handle.snapshot().len()) } else { return Err(error); }
        } };
        if let Some(fold) = folds.iter().find(|fold| fold.header + 1 == range.start && fold.end + 1 == range.end) {
            anchors.push(FoldAnchor { header: line_start(handle, &mut index, fold.header, budget, cancel)?, end: last, fold: fold.clone(), collapsed: true });
        }
        if first < last { if let Some(previous) = gaps.last_mut() && first <= previous.end { previous.end = previous.end.max(last); } else { gaps.push(first..last); } }
    }
    let mut builder = DocumentBuilder::new(budget.clone(), Budget::new(0)).map_err(|e| format!("Fold projection: {e:?}"))?;
    let mut segments = Vec::new(); let mut cursor = start; let mut bytes = 0;
    while cursor.0 < handle.snapshot().len() && bytes < BYTES && segments.len() < PIECES {
        if let Some(gap) = gaps.iter().find(|gap| gap.start <= cursor && cursor < gap.end) { cursor = gap.end; continue; }
        let end = gaps.iter().find(|gap| gap.start > cursor).map_or(handle.snapshot().len(), |gap| gap.start.0).min(cursor.0.saturating_add(BYTES - bytes));
        let mut request = handle.snapshot().begin_viewport(cursor, end - cursor.0, budget).map_err(|e| format!("Fold window: {e:?}"))?;
        let window = loop { cancel.check().map_err(|e| format!("Fold window: {e:?}"))?; match request.poll() {
            WindowPoll::Ready(window) => break window,
            WindowPoll::Pending(ticket) => { if !handle.resolve_captured_page(ticket)? { std::thread::yield_now(); } },
            _ => return Err("Fold projection source unavailable".into()),
        } };
        if window.text().is_empty() { break; }
        let range = window.range();
        let line = match lookup(handle, &mut index, LineTarget::Byte(range.start), budget, cancel)? { LineLookupPoll::Line(line) => line, _ => return Err("Fold source line unavailable".into()) };
        let source_line_start = line_start(handle, &mut index, line, budget, cancel)?;
        builder.append(window.text()).map_err(|e| format!("Fold projection: {e:?}"))?;
        segments.push(ViewportSegment { local: TextOffset(bytes)..TextOffset(bytes + window.text().len()), source: range.clone(), first_global_line: Some(line as u64), source_line_start: Some(source_line_start) });
        bytes += window.text().len(); cursor = range.end;
    }
    Ok(MappedViewport { source: handle.snapshot().clone(), generation, projection: builder.prefix(), segments, anchors, _claim: claim })
}
