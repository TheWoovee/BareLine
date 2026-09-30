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
    ops::Range,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};
const BYTES: usize = 64 * 1024;
const PIECES: usize = 128;
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
    generation: u64,
    line_index: SharedLineIndex,
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
                let result = build(
                    &handle,
                    &line_index,
                    start,
                    folds,
                    manual,
                    rebased,
                    generation,
                    &budget,
                    &cancellation,
                );
                completion.complete(result);
            }),
        )
        .map_err(|_| "Viewport worker queue is full".to_owned())?;
    Ok(MappingJob { result, cancel })
}
/// Fold lookups through the document's shared line index. Each resumes from
/// the previous lookup's verified position when that is closer, so none scans
/// from byte zero once the index covers the text (PED-07).
struct Lookups<'a> {
    handle: &'a PagedReadHandle,
    index: &'a SharedLineIndex,
    budget: &'a Budget,
    cancel: &'a Cancellation,
    hint: Option<LineCheckpoint>,
}
impl Lookups<'_> {
    fn lookup(&mut self, target: LineTarget) -> Result<LineLookupPoll, String> {
        let cancel = self.cancel;
        let result = self
            .index
            .lookup(self.handle, target, self.budget, &mut self.hint, &mut || {
                cancel.check().map_err(|e| format!("Fold lookup: {e:?}"))
            })?;
        match result {
            LineLookupPoll::Line(_) | LineLookupPoll::Range(_) => Ok(result),
            _ => Err(format!("Fold source unavailable: {result:?}")),
        }
    }
    fn line(&mut self, offset: TextOffset) -> Result<usize, String> {
        match self.lookup(LineTarget::Byte(offset))? {
            LineLookupPoll::Line(line) => Ok(line),
            _ => Err("Fold line unavailable".into()),
        }
    }
    fn line_start(&mut self, line: usize) -> Result<TextOffset, String> {
        match self.lookup(LineTarget::Line(line))? {
            LineLookupPoll::Range(range) => Ok(range.start),
            _ => Err("Fold line unavailable".into()),
        }
    }
}
fn build(
    handle: &PagedReadHandle,
    line_index: &SharedLineIndex,
    start: TextOffset,
    mut folds: Vec<bareline_syntax::folding::Fold>,
    manual: Vec<Range<usize>>,
    rebased: Vec<FoldAnchor>,
    generation: u64,
    budget: &Budget,
    cancel: &Cancellation,
) -> Result<MappedViewport, String> {
    let claim = budget
        .claim(PIECES * std::mem::size_of::<ViewportSegment>() + (folds.len() + manual.len() + rebased.len()) * 128)
        .map_err(|e| format!("Viewport map: {e:?}"))?;
    let mut index = Lookups {
        handle,
        index: line_index,
        budget,
        cancel,
        hint: None,
    };
    let mut gaps: Vec<Range<TextOffset>> = Vec::new();
    let mut anchors = Vec::new();
    for anchor in rebased {
        let header = index.line(anchor.header)?;
        let after = index.line(anchor.end)?;
        let end = if anchor.end.0 == handle.snapshot().len() {
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
            if anchor.collapsed {
                folds.push(fold);
            } else {
                anchors.push(FoldAnchor {
                    header: anchor.header,
                    body: anchor.body,
                    end: anchor.end,
                    fold,
                    collapsed: false,
                });
            }
        }
    }
    folds.sort_by_key(|fold| (fold.header, fold.end));
    folds.dedup_by_key(|fold| (fold.header, fold.end));
    let mut hidden: Vec<_> = folds
        .iter()
        .map(|fold| fold.header + 1..fold.end + 1)
        .chain(manual)
        .collect();
    hidden.sort_by_key(|range| (range.start, range.end));
    for range in hidden {
        cancel.check().map_err(|e| format!("Fold mapping: {e:?}"))?;
        let first = index.line_start(range.start)?;
        let last = match index.line_start(range.end) {
            Ok(offset) => offset,
            Err(error) => {
                // The line after the last one starts at the end of the text. The
                // failed lookup ended there, so this one reads nothing.
                let end = TextOffset(handle.snapshot().len());
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
            anchors.push(FoldAnchor {
                header: index.line_start(fold.header)?,
                body: first,
                end: last,
                fold: fold.clone(),
                collapsed: true,
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
    let mut builder =
        DocumentBuilder::new(budget.clone(), Budget::new(0)).map_err(|e| format!("Fold projection: {e:?}"))?;
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
            .map_err(|e| format!("Fold window: {e:?}"))?;
        let window = loop {
            cancel.check().map_err(|e| format!("Fold window: {e:?}"))?;
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
            .map_err(|e| format!("Fold projection: {e:?}"))?;
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
        generation,
        projection: builder.prefix(),
        segments,
        anchors,
        _claim: claim,
    })
}
