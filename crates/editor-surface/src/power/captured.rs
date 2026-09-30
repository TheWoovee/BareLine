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
    // Selections follow their text (EDT-04). The rows before each selection end
    // are counted while its range is copied, so indentation can place it exactly.
    let before = if metadata.before.len() == ranges.len() {
        metadata
            .before
            .iter()
            .map(|selection| (selection.anchor.0, selection.caret.0))
            .collect::<Vec<_>>()
    } else {
        ranges.iter().map(|range| (range.start.0, range.end.0)).collect()
    };
    let mut ends = before
        .iter()
        .flat_map(|&(anchor, caret)| [anchor, caret])
        .collect::<Vec<_>>();
    ends.sort_unstable();
    ends.dedup();
    let mut staged = Vec::new();
    for (range, pivot) in &plans {
        let start = inverse.len();
        let mut reader = CapturedRangeReader::new(
            captured.clone(),
            range.clone(),
            options.budget.clone(),
            options.cancellation.clone(),
        )?;
        let targets =
            &ends[ends.partition_point(|&end| end < range.start.0)..ends.partition_point(|&end| end <= range.end.0)];
        let mut counter = RowCounter::new(&mut inverse, range.start.0, targets);
        io::copy(&mut reader, &mut counter)?;
        let rows = counter.finish();
        let removed = start..inverse.len();
        reader.seek(SeekFrom::Start(0))?;
        let start = inserted.len();
        let mut moved = None;
        if let Some(pivot) = pivot {
            let down = matches!(action, super::Transform::MoveDown);
            let block = super::streaming::move_lines(reader, &mut inserted, *pivot, down, quota, || {
                options.cancellation.check().is_err()
            })?;
            // The selected block starts after the preceding line when moving up.
            let origin = range.start.0 + if down { 0 } else { *pivot as usize };
            moved = Some((origin, block));
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
        staged.push(Staged {
            range: range.clone(),
            removed,
            added: start..inserted.len(),
            moved,
            rows,
        });
    }
    let inverse = inverse.finish()?;
    let inserted = inserted.finish()?;
    metadata.after.clear();
    for (anchor, caret) in before {
        let collapsed = anchor == caret;
        let place = |offset, start| place_after(&staged, &action, tab_width, offset, start, collapsed);
        let selection = bareline_document::history::Selection {
            anchor: TextOffset(place(anchor, anchor <= caret)?),
            caret: TextOffset(place(caret, caret < anchor)?),
        };
        if metadata.after.last() != Some(&selection) {
            metadata.after.push(selection);
        }
    }
    let edits = staged
        .into_iter()
        .map(|staged| SourceEdit {
            range: staged.range,
            inverse: OwnedTextRange {
                source: inverse.clone(),
                range: staged.removed,
            },
            inserted: OwnedTextRange {
                source: inserted.clone(),
                range: staged.added,
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

/// One staged range: its source range, its inverse and output bytes, where a
/// moved block landed, and the rows of the selection ends inside it.
struct Staged {
    range: Range<TextOffset>,
    removed: Range<u64>,
    added: Range<u64>,
    moved: Option<(usize, super::MovedBlock)>,
    rows: Rows,
}
/// A selection end inside a staged range: the row breaks before it and whether
/// it starts a row.
#[derive(Clone, Copy)]
struct RowEnd {
    offset: usize,
    row: usize,
    row_start: bool,
}
struct Rows {
    ends: Vec<RowEnd>,
    /// The range holds at most one row (and its line break).
    single: bool,
}
/// Passes a staged range's source through to `inner`, counting rows for the
/// sorted absolute selection ends in `targets`.
struct RowCounter<'a, W> {
    inner: W,
    origin: usize,
    targets: &'a [usize],
    position: usize,
    /// Row breaks before `position`, not yet counting a final '\r'.
    rows: usize,
    last: Option<u8>,
    ends: Vec<RowEnd>,
}
impl<'a, W: io::Write> RowCounter<'a, W> {
    fn new(inner: W, origin: usize, targets: &'a [usize]) -> Self {
        Self {
            inner,
            origin,
            targets,
            position: origin,
            rows: 0,
            last: None,
            ends: Vec::with_capacity(targets.len()),
        }
    }
    fn mark(&mut self) {
        let mut targets = self.targets;
        while let Some((&offset, rest)) = targets.split_first() {
            if offset > self.position {
                break;
            }
            // A selection end is never inside "\r\n", so a '\r' before it ended a row.
            self.ends.push(RowEnd {
                offset,
                row: self.rows + usize::from(self.last == Some(b'\r')),
                row_start: offset == self.origin || matches!(self.last, Some(b'\n' | b'\r')),
            });
            targets = rest;
        }
        self.targets = targets;
    }
    fn finish(mut self) -> Rows {
        self.mark();
        let breaks = self.rows + usize::from(self.last == Some(b'\r'));
        Rows {
            ends: self.ends,
            single: breaks <= usize::from(matches!(self.last, Some(b'\n' | b'\r'))),
        }
    }
}
impl<W: io::Write> io::Write for RowCounter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        for &byte in &bytes[..written] {
            self.mark();
            if (self.last == Some(b'\r') && byte != b'\n') || byte == b'\n' {
                self.rows += 1;
            }
            self.last = Some(byte);
            self.position += 1;
        }
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
/// Offset after the staged edits of a selection end at `offset` before them.
/// Inside a rewritten range only exact mappings keep a position: a moved block,
/// a duplicate's original, Indent (every row gains `tab_width` spaces), and a
/// caret in a single row that Unindent or Trim Leading shortened at its start.
/// Any other end goes to the range start (a selection's first end, or a caret in
/// a single row) or its end; both are line boundaries of the output.
fn place_after(
    staged: &[Staged],
    action: &super::Transform,
    tab_width: usize,
    offset: usize,
    start: bool,
    collapsed: bool,
) -> io::Result<usize> {
    use super::Transform;
    let overflow = || io::Error::new(io::ErrorKind::OutOfMemory, "Transform selection overflow");
    let mut delta = 0i128;
    for staged in staged {
        let range = &staged.range;
        let result = i128::from(staged.added.end - staged.added.start);
        let length = (range.end.0 - range.start.0) as i128;
        if offset < range.start.0 {
            break;
        }
        if offset <= range.end.0 {
            let local = (offset - range.start.0) as i128;
            let row = staged
                .rows
                .ends
                .binary_search_by_key(&offset, |end| end.offset)
                .ok()
                .map(|index| staged.rows.ends[index]);
            let placed = if let Some((origin, block)) = staged.moved {
                block.map(offset.saturating_sub(origin)) as i128
            } else if matches!(action, Transform::Duplicate | Transform::DuplicateSelections) {
                local
            } else if matches!(action, Transform::Indent)
                && let Some(end) = row
            {
                // A selection from a row start keeps whole rows selected.
                let rows = if end.row_start && !collapsed {
                    end.row
                } else {
                    end.row + 1
                };
                local + (rows as i128) * (tab_width as i128)
            } else if collapsed && staged.rows.single && matches!(action, Transform::Unindent | Transform::TrimStart) {
                (local + result - length).max(0)
            } else if start || (collapsed && staged.rows.single) {
                0
            } else {
                result
            };
            let absolute = range.start.0 as i128 + delta + placed.clamp(0, result);
            return usize::try_from(absolute).map_err(|_| overflow());
        }
        delta += result - length;
    }
    usize::try_from(offset as i128 + delta).map_err(|_| overflow())
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
                previous_boundary(captured, selected.end.0, options)?
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

/// Start of the character that ends at the UTF-8 boundary `offset`. Line
/// lookups for the last selected character need it: `offset - 1` falls inside
/// any multi-byte character.
pub(crate) fn previous_boundary(
    captured: &PagedReadHandle,
    offset: usize,
    options: &StagingOptions,
) -> io::Result<usize> {
    if offset == 0 {
        return Ok(0);
    }
    // At most one scalar (4 bytes); the window start snaps forward to a boundary.
    let mut request = captured
        .snapshot()
        .begin_viewport(TextOffset(offset.saturating_sub(4)), offset.min(4), &options.budget)
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    loop {
        options
            .cancellation
            .check()
            .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "Captured read cancelled"))?;
        match request.poll() {
            WindowPoll::Ready(window) => {
                return window
                    .text()
                    .chars()
                    .next_back()
                    .filter(|_| window.range().end.0 == offset)
                    .map(|last| offset - last.len_utf8())
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "Captured selection splits a UTF-8 scalar")
                    });
            }
            WindowPoll::Pending(ticket) => {
                if !captured
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
            _ => return Err(io::Error::other("Captured source is unavailable")),
        }
    }
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
