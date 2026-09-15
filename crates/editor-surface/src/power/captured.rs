// SPDX-License-Identifier: MPL-2.0
//! Worker-only, bounded reads of the actual captured document selection.
use bareline_document::{
    Budget, TextOffset,
    paged::{TextWindow, WindowPoll},
};
use bareline_file_io::cancellation::Cancellation;
use bareline_file_io::paged_service::PagedReadHandle;
use std::{
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
};

#[derive(Clone)]
pub struct StagingOptions {
    pub cache: std::path::PathBuf,
    pub quota: u64,
    pub platform: std::sync::Arc<dyn bareline_platform::LocalFileSystem>,
    pub source_options: bareline_file_io::source::SourceOptions,
    pub budget: Budget,
    pub memory: usize,
    pub cancellation: Cancellation,
}

/// Produces a fully validated token on a worker; only the owning paged actor may
/// lease it, journal it durably, and publish it. Selection ranges are expanded
/// against the captured global line index, never a viewport proxy.
pub fn prepare_transform(
    captured: PagedReadHandle,
    ranges: &[Range<TextOffset>],
    action: super::Transform,
    tab_width: usize,
    mut metadata: bareline_document::history::EditMetadata,
    options: &StagingOptions,
) -> io::Result<bareline_document::paged::PreparedSourceTransaction> {
    use bareline_document::paged::{OwnedTextRange, SourceEdit, SourceTransactionPoll};
    use bareline_file_io::owned_store::StreamingStoreBuilder;
    if ranges.is_empty() || ranges.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid transform range count",
        ));
    }
    let plans = plan_ranges(&captured, ranges, &action, options)?;
    let _memory = options
        .budget
        .claim(options.memory)
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    // Inverse/output stores and temporary sort runs each have a disjoint third.
    let quota = options.quota / 3;
    let builder = || {
        StreamingStoreBuilder::new(
            &options.cache,
            quota,
            options.platform.clone(),
            options.source_options,
            options.budget.clone(),
            options.cancellation.clone(),
        )
    };
    let mut inverse = builder()?;
    let mut inserted = builder()?;
    let mut staged = Vec::new();
    for (range, pivot) in &plans {
        let start = inverse.len();
        let mut reader = CapturedRangeReader::new(
            captured.clone(),
            range.clone(),
            options.budget.clone(),
            options.cancellation.clone(),
        )?;
        io::copy(&mut reader, &mut inverse)?;
        let removed = start..inverse.len();
        reader.seek(SeekFrom::Start(0))?;
        let start = inserted.len();
        if let Some(pivot) = pivot {
            super::streaming::move_lines(
                reader,
                &mut inserted,
                *pivot,
                matches!(action, super::Transform::MoveDown),
                quota,
                || options.cancellation.check().is_err(),
            )?;
        } else {
            super::streaming::transform_lines(
                reader,
                &mut inserted,
                &options.cache,
                options.memory,
                quota,
                action.clone(),
                tab_width,
                || options.cancellation.check().is_err(),
            )?;
        }
        staged.push((range.clone(), removed, start..inserted.len()));
    }
    let inverse = inverse.finish()?;
    let inserted = inserted.finish()?;
    metadata.after.clear();
    let mut delta = 0i128;
    for (range, _, added) in &staged {
        let end = range.start.0 as i128 + delta + (added.end - added.start) as i128;
        let caret = usize::try_from(end)
            .map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "Transform selection overflow"))?;
        metadata.after.push(bareline_document::history::Selection {
            anchor: TextOffset(caret),
            caret: TextOffset(caret),
        });
        delta += (added.end - added.start) as i128 - (range.end.0 - range.start.0) as i128;
    }
    let edits = staged
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
    let mut request = captured
        .snapshot()
        .prepare_source_transaction(edits, metadata, options.budget.clone())
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    loop {
        options
            .cancellation
            .check()
            .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "Transform cancelled"))?;
        match request.poll() {
            SourceTransactionPoll::Ready(prepared) => return Ok(prepared),
            SourceTransactionPoll::Progress => {}
            SourceTransactionPoll::Pending(ticket) => {
                if !request
                    .resolve_owned(ticket)
                    .map_err(|e| io::Error::other(format!("{e:?}")))?
                    && !captured
                        .resolve_captured_page(ticket)
                        .map_err(|error| io::Error::other(error.to_string()))?
                {
                    std::thread::yield_now();
                }
            }
            SourceTransactionPoll::Unavailable(reason) => {
                return Err(io::Error::other(format!("Transform source unavailable: {reason:?}")));
            }
            SourceTransactionPoll::Failed(error) => {
                return Err(io::Error::other(format!("Transform validation failed: {error:?}")));
            }
            SourceTransactionPoll::Cancelled => {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "Transform cancelled"));
            }
            SourceTransactionPoll::Finished => return Err(io::Error::other("Transform validation already finished")),
        }
    }
}

fn plan_ranges(
    captured: &PagedReadHandle,
    ranges: &[Range<TextOffset>],
    action: &super::Transform,
    options: &StagingOptions,
) -> io::Result<Vec<(Range<TextOffset>, Option<u64>)>> {
    use bareline_document::{
        line_lookup::{LineLookupPoll, LineTarget},
        paged::SparseLineIndex,
    };
    let index = SparseLineIndex::new(captured.snapshot().clone(), 16, 64 * 1024, &options.budget)
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    let lookup = |target| -> io::Result<LineLookupPoll> {
        let mut request = index
            .lookup(target, options.budget.clone())
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        loop {
            options
                .cancellation
                .check()
                .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "Transform planning cancelled"))?;
            match request.poll() {
                result @ (LineLookupPoll::Line(_) | LineLookupPoll::Range(_)) => return Ok(result),
                LineLookupPoll::Progress(_) => {}
                LineLookupPoll::Pending(ticket) => {
                    if !captured
                        .resolve_captured_page(ticket)
                        .map_err(|error| io::Error::other(error.to_string()))?
                    {
                        std::thread::yield_now();
                    }
                }
                result => return Err(io::Error::other(format!("Transform line lookup: {result:?}"))),
            }
        }
    };
    let line_at = |offset| -> io::Result<usize> {
        match lookup(LineTarget::Byte(TextOffset(offset)))? {
            LineLookupPoll::Line(line) => Ok(line),
            _ => Err(io::Error::other("Line lookup returned no line")),
        }
    };
    let line_range = |line| -> io::Result<Range<TextOffset>> {
        match lookup(LineTarget::Line(line))? {
            LineLookupPoll::Range(range) => Ok(range),
            _ => Err(io::Error::other("Line lookup returned no range")),
        }
    };
    let linewise = !matches!(
        action,
        super::Transform::Uppercase
            | super::Transform::Lowercase
            | super::Transform::Titlecase
            | super::Transform::InvertCase
            | super::Transform::DuplicateSelections
    );
    let mut merged = Vec::<Range<TextOffset>>::new();
    for (position, selected) in ranges.iter().enumerate() {
        if selected.start > selected.end
            || selected.end.0 > captured.snapshot().len()
            || position > 0 && ranges[position - 1].start > selected.start
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid transform selections",
            ));
        }
        let range = if linewise {
            line_range(line_at(selected.start.0)?)?.start..line_range(line_at(if selected.end > selected.start {
                selected.end.0 - 1
            } else {
                selected.end.0
            })?)?
            .end
        } else {
            selected.clone()
        };
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    let mut planned = Vec::<(Range<TextOffset>, Option<u64>)>::new();
    for range in merged {
        let (range, pivot) = match action {
            super::Transform::MoveUp => {
                let line = line_at(range.start.0)?;
                if line == 0 {
                    continue;
                }
                let preceding = line_range(line - 1)?;
                let pivot = (range.start.0 - preceding.start.0) as u64;
                (preceding.start..range.end, Some(pivot))
            }
            super::Transform::MoveDown => {
                if range.end.0 == captured.snapshot().len() {
                    continue;
                }
                let next = line_range(line_at(range.end.0)?)?;
                let pivot = (range.end.0 - range.start.0) as u64;
                (range.start..next.end, Some(pivot))
            }
            _ => (range, None),
        };
        if planned.last().is_some_and(|(previous, _)| previous.end > range.start) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Adjacent move ranges overlap",
            ));
        }
        planned.push((range, pivot));
    }
    if planned.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "No movable lines"));
    }
    Ok(planned)
}

pub struct CapturedRangeReader {
    captured: PagedReadHandle,
    range: Range<TextOffset>,
    position: usize,
    window: Option<TextWindow>,
    budget: Budget,
    cancellation: Cancellation,
}
impl CapturedRangeReader {
    pub fn new(
        captured: PagedReadHandle,
        range: Range<TextOffset>,
        budget: Budget,
        cancellation: Cancellation,
    ) -> io::Result<Self> {
        if range.start > range.end || range.end.0 > captured.snapshot().len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Captured selection is out of bounds",
            ));
        }
        Ok(Self {
            captured,
            range,
            position: 0,
            window: None,
            budget,
            cancellation,
        })
    }
    fn refill(&mut self, absolute: usize) -> io::Result<()> {
        self.window = None;
        let start = absolute.saturating_sub(3);
        let mut request = self
            .captured
            .snapshot()
            .begin_viewport(TextOffset(start), 64 * 1024, &self.budget)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        loop {
            self.cancellation
                .check()
                .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "Captured read cancelled"))?;
            match request.poll() {
                WindowPoll::Ready(window) => {
                    if window.range().start.0 > absolute || window.range().end.0 <= absolute {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Captured selection has an invalid boundary",
                        ));
                    }
                    if absolute == self.range.start.0
                        && !window.text().is_char_boundary(absolute - window.range().start.0)
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Captured selection splits a UTF-8 scalar",
                        ));
                    }
                    if self.range.end.0 <= window.range().end.0
                        && !window
                            .text()
                            .is_char_boundary(self.range.end.0 - window.range().start.0)
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Captured selection splits a UTF-8 scalar",
                        ));
                    }
                    self.window = Some(window);
                    return Ok(());
                }
                WindowPoll::Pending(ticket) => {
                    if !self
                        .captured
                        .resolve_captured_page(ticket)
                        .map_err(|error| io::Error::other(error.to_string()))?
                    {
                        std::thread::yield_now();
                    }
                }
                WindowPoll::Unavailable(reason) => {
                    return Err(io::Error::other(format!("Captured source unavailable: {reason:?}")));
                }
                WindowPoll::InvalidUtf8 => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Captured source is not UTF-8",
                    ));
                }
                WindowPoll::Finished => return Err(io::Error::other("Captured read already finished")),
            }
        }
    }
}
impl Read for CapturedRangeReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.cancellation
            .check()
            .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "Captured read cancelled"))?;
        let absolute = self.range.start.0 + self.position;
        if output.is_empty() || absolute == self.range.end.0 {
            return Ok(0);
        }
        if self
            .window
            .as_ref()
            .is_none_or(|window| absolute < window.range().start.0 || absolute >= window.range().end.0)
        {
            self.refill(absolute)?;
        }
        let window = self.window.as_ref().unwrap();
        let count = output.len().min(window.range().end.0.min(self.range.end.0) - absolute);
        let local = absolute - window.range().start.0;
        output[..count].copy_from_slice(&window.text().as_bytes()[local..local + count]);
        self.position += count;
        Ok(count)
    }
}
impl Seek for CapturedRangeReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let length = self.range.end.0 - self.range.start.0;
        let position = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(n) => length as i128 + i128::from(n),
            SeekFrom::Current(n) => self.position as i128 + i128::from(n),
        };
        if position < 0 || position > length as i128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Captured seek outside selection",
            ));
        }
        self.position = position as usize;
        Ok(self.position as u64)
    }
}

/// Read only selections which fit the caller's clipboard quota. The caller must
/// validate this captured identity again before a cut is submitted.
pub fn clipboard_text(
    captured: PagedReadHandle,
    ranges: &[Range<TextOffset>],
    limit: usize,
    budget: Budget,
    cancellation: Cancellation,
) -> io::Result<String> {
    if ranges.len() > 10_000 {
        return Err(io::Error::new(
            io::ErrorKind::OutOfMemory,
            "Too many clipboard selections",
        ));
    }
    let mut length = ranges.len().saturating_sub(1);
    for (index, range) in ranges.iter().enumerate() {
        if range.start > range.end
            || range.end.0 > captured.snapshot().len()
            || index > 0 && ranges[index - 1].end > range.start
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid clipboard selection ranges",
            ));
        }
        length = length
            .checked_add(range.end.0 - range.start.0)
            .filter(|n| *n <= limit)
            .ok_or_else(|| io::Error::new(io::ErrorKind::OutOfMemory, "Selection exceeds clipboard limit"))?;
    }
    if length > limit {
        return Err(io::Error::new(
            io::ErrorKind::OutOfMemory,
            "Selection exceeds clipboard limit",
        ));
    }
    let _claim = budget.claim(length).map_err(|e| io::Error::other(format!("{e:?}")))?;
    let mut output = String::new();
    output.try_reserve_exact(length).map_err(io::Error::other)?;
    for (index, range) in ranges.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        CapturedRangeReader::new(captured.clone(), range.clone(), budget.clone(), cancellation.clone())?
            .read_to_string(&mut output)?;
    }
    Ok(output)
}
