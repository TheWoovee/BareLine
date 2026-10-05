// SPDX-License-Identifier: MPL-2.0
//! Streaming inverse validation over an interval map (REC-08). The map describes the
//! current document as spans of the sealed baseline and of validated segments'
//! inserted bytes. A transaction is checked by comparing its inverse bytes with the
//! spans it removes, then the map is spliced; no document copy is materialized. Work
//! is O(journal payload) reads and O(edits) memory instead of O(records x size)
//! scratch writes, so a full disk can neither fail nor slow inspection.
//!
//! Locating an offset walks the block headers (one length each) from the start, so a
//! journal of E edits costs O(E x E / BLOCK) header visits in the worst case, besides
//! the payload reads. Typical journals hold few blocks; one at the record and edit
//! limits pays that quadratic term. A prefix index would not remove it on its own,
//! because every splice shifts the starts of all later blocks; a balanced tree over
//! the blocks is the follow-up if that bound is ever measured to matter.
use super::*;

/// Spans per block before a block splits; bounds the element moves of one splice.
const BLOCK: usize = 256;
/// Most spans the map holds (about 128 MiB). Reaching it is a resource failure, which
/// keeps every record and reports the source unavailable instead of corruption.
const MAX_SPANS: usize = 1 << 22;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    Baseline,
    /// Index of the validated record whose segment holds the bytes.
    Segment(usize),
}
#[derive(Clone, Copy, Debug)]
struct Span {
    origin: Origin,
    offset: u64,
    len: u64,
}
struct Block {
    len: u64,
    spans: Vec<Span>,
}
fn out_of_memory() -> io::Error {
    io::Error::from(io::ErrorKind::OutOfMemory)
}
#[cfg(test)]
thread_local! {
    /// Models an unreadable volume or exhausted resources during validation.
    pub(super) static FAIL_READS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(test)]
fn injected_read_failure() -> io::Result<()> {
    if FAIL_READS.with(|fail| fail.get()) {
        Err(io::Error::other("injected validation read failure"))
    } else {
        Ok(())
    }
}
#[cfg(not(test))]
fn injected_read_failure() -> io::Result<()> {
    Ok(())
}

/// The validated journal prefix as an interval map over its immutable inputs.
pub(super) struct Replay {
    directory: PathBuf,
    blocks: Vec<Block>,
    len: u64,
    baseline: File,
    /// Most recently read earlier segment; edits are local, so one handle suffices.
    cached: Option<(usize, File)>,
    /// Spans across all blocks, bounded by `MAX_SPANS`.
    spans: usize,
    /// Comparison buffers, allocated once instead of per removing edit.
    scratch: Vec<u8>,
    /// Bytes read from the baseline and segments; pins the complexity in tests.
    pub(super) read_bytes: u64,
}
impl Replay {
    /// Validate `records` in order against the sealed baseline. Returns how many form
    /// a valid prefix; data mismatches end the prefix. Any other failure (resources,
    /// reads, cancellation) is returned: it proves nothing about the journal.
    pub(super) fn build(
        directory: &Path,
        records: &[Record],
        baseline_len: u64,
        cancel: &Cancellation,
    ) -> io::Result<(usize, Self)> {
        let baseline = File::open(directory.join("baseline.bin"))?;
        if baseline.metadata()?.len() != baseline_len {
            return Err(invalid("recovery baseline length changed"));
        }
        let mut blocks = Vec::new();
        let spans = usize::from(baseline_len != 0);
        if baseline_len != 0 {
            blocks.push(Block {
                len: baseline_len,
                spans: vec![Span {
                    origin: Origin::Baseline,
                    offset: 0,
                    len: baseline_len,
                }],
            });
        }
        let mut replay = Self {
            directory: directory.into(),
            blocks,
            len: baseline_len,
            baseline,
            cached: None,
            spans,
            scratch: Vec::new(),
            read_bytes: 0,
        };
        for index in 0..records.len() {
            cancelled(cancel)?;
            match replay.apply(records, index, cancel) {
                Ok(()) => {}
                Err(error) if matches!(error.kind(), io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof) => {
                    return Ok((index, replay));
                }
                Err(error) => return Err(error),
            }
        }
        Ok((records.len(), replay))
    }
    /// Every inverse is compared before the map changes, so a rejected record leaves
    /// the map at the end of the valid prefix.
    fn apply(&mut self, records: &[Record], index: usize, cancel: &Cancellation) -> io::Result<()> {
        let record = &records[index];
        let mut segment = File::open(self.directory.join(&record.segment.name))?;
        let mut placed = Vec::new();
        placed
            .try_reserve_exact(record.edits.len())
            .map_err(|_| out_of_memory())?;
        let mut position = 0u64;
        let mut previous_end = 0u64;
        for edit in &record.edits {
            let end = edit
                .offset
                .checked_add(edit.removed)
                .filter(|end| *end <= self.len)
                .ok_or_else(|| invalid("recovery edit outside source"))?;
            if edit.offset < previous_end {
                return Err(invalid("overlapping recovery edits"));
            }
            previous_end = end;
            // Check inverse bytes as well as segment hashes: applying to the wrong baseline fails closed.
            self.compare(records, edit.offset, edit.removed, &mut segment, position, cancel)?;
            let inserted_at = position
                .checked_add(edit.removed)
                .ok_or_else(|| invalid("recovery segment overflow"))?;
            placed.push((edit.offset, edit.removed, inserted_at, edit.inserted));
            position = inserted_at
                .checked_add(edit.inserted)
                .ok_or_else(|| invalid("recovery segment overflow"))?;
        }
        if position != record.segment.len {
            return Err(invalid("recovery segment length mismatch"));
        }
        // Pre-transaction offsets stay valid when the rightmost edit is applied first.
        for &(offset, removed, inserted_at, inserted) in placed.iter().rev() {
            cancelled(cancel)?;
            let span = (inserted != 0).then_some(Span {
                origin: Origin::Segment(index),
                offset: inserted_at,
                len: inserted,
            });
            self.splice(offset, removed, span)?;
        }
        Ok(())
    }
    fn compare(
        &mut self,
        records: &[Record],
        offset: u64,
        len: u64,
        segment: &mut File,
        position: u64,
        cancel: &Cancellation,
    ) -> io::Result<()> {
        if len == 0 {
            return Ok(());
        }
        segment.seek(SeekFrom::Start(position))?;
        let mut scratch = std::mem::take(&mut self.scratch);
        if scratch.len() < 2 * CHUNK {
            scratch
                .try_reserve_exact(2 * CHUNK - scratch.len())
                .map_err(|_| out_of_memory())?;
            scratch.resize(2 * CHUNK, 0);
        }
        let result = (|| -> io::Result<()> {
            let (actual, expected) = scratch.split_at_mut(CHUNK);
            for (origin, start, count) in self.pieces(offset, len)? {
                let mut done = 0u64;
                while done < count {
                    cancelled(cancel)?;
                    let n = (count - done).min(CHUNK as u64) as usize;
                    self.read_origin(records, origin, start + done, &mut actual[..n])?;
                    segment.read_exact(&mut expected[..n])?;
                    self.read_bytes += 2 * n as u64;
                    if actual[..n] != expected[..n] {
                        return Err(invalid("recovery inverse does not match baseline"));
                    }
                    done += n as u64;
                }
            }
            Ok(())
        })();
        self.scratch = scratch;
        result
    }
    fn read_origin(&mut self, records: &[Record], origin: Origin, offset: u64, output: &mut [u8]) -> io::Result<()> {
        injected_read_failure()?;
        let file = match origin {
            Origin::Baseline => &mut self.baseline,
            Origin::Segment(index) => {
                if self.cached.as_ref().is_none_or(|(cached, _)| *cached != index) {
                    let name = &records
                        .get(index)
                        .ok_or_else(|| invalid("recovery span outside prefix"))?
                        .segment
                        .name;
                    self.cached = Some((index, File::open(self.directory.join(name))?));
                }
                match self.cached.as_mut() {
                    Some((_, file)) => file,
                    None => return Err(invalid("recovery segment handle")),
                }
            }
        };
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(output)
    }
    /// Source pieces of the current byte range `offset..offset + len`.
    fn pieces(&self, offset: u64, len: u64) -> io::Result<Vec<(Origin, u64, u64)>> {
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= self.len)
            .ok_or_else(|| invalid("recovery edit outside source"))?;
        let mut pieces = Vec::new();
        let mut total = 0u64;
        let mut block_start = 0u64;
        for block in &self.blocks {
            let block_end = block_start + block.len;
            if block_end > offset && block_start < end {
                let mut at = block_start;
                for span in &block.spans {
                    let span_end = at + span.len;
                    if span_end > offset && at < end {
                        let from = offset.max(at);
                        let to = end.min(span_end);
                        pieces.try_reserve(1).map_err(|_| out_of_memory())?;
                        pieces.push((span.origin, span.offset + (from - at), to - from));
                        total += to - from;
                    }
                    at = span_end;
                }
            }
            if block_end >= end {
                break;
            }
            block_start = block_end;
        }
        if total != len {
            return Err(invalid("recovery map length mismatch"));
        }
        Ok(pieces)
    }
    /// Split so a span boundary falls at `offset`; returns the position of the first
    /// span starting there, or `(blocks.len(), 0)` at the end of the document.
    fn split(&mut self, offset: u64) -> io::Result<(usize, usize)> {
        let mut block_start = 0u64;
        for (index, block) in self.blocks.iter_mut().enumerate() {
            let block_end = block_start + block.len;
            if offset < block_end {
                let mut at = block_start;
                let mut inside = None;
                for (position, span) in block.spans.iter().enumerate() {
                    if offset == at {
                        return Ok((index, position));
                    }
                    if offset < at + span.len {
                        inside = Some((position, *span, offset - at));
                        break;
                    }
                    at += span.len;
                }
                let (position, span, head) = inside.ok_or_else(|| invalid("recovery map length mismatch"))?;
                if self.spans >= MAX_SPANS {
                    return Err(out_of_memory());
                }
                block.spans.try_reserve(1).map_err(|_| out_of_memory())?;
                block.spans[position].len = head;
                block.spans.insert(
                    position + 1,
                    Span {
                        origin: span.origin,
                        offset: span.offset + head,
                        len: span.len - head,
                    },
                );
                self.spans += 1;
                return Ok((index, position + 1));
            }
            block_start = block_end;
        }
        if offset == block_start {
            Ok((self.blocks.len(), 0))
        } else {
            Err(invalid("recovery edit outside source"))
        }
    }
    fn splice(&mut self, offset: u64, removed: u64, insert: Option<Span>) -> io::Result<()> {
        let end = offset
            .checked_add(removed)
            .filter(|end| *end <= self.len)
            .ok_or_else(|| invalid("recovery edit outside source"))?;
        let (first_block, first_span) = self.split(offset)?;
        // A second split lands at or after the first and never shifts it.
        let (last_block, last_span) = self.split(end)?;
        let mut dropped = 0;
        if first_block == last_block {
            if let Some(block) = self.blocks.get_mut(first_block) {
                dropped += block.spans.drain(first_span..last_span).len();
            }
        } else {
            let first = &mut self.blocks[first_block].spans;
            dropped += first.len() - first_span;
            first.truncate(first_span);
            for block in &mut self.blocks[first_block + 1..last_block] {
                dropped += block.spans.len();
                block.spans.clear();
            }
            if let Some(block) = self.blocks.get_mut(last_block) {
                dropped += block.spans.drain(..last_span).len();
            }
        }
        self.spans -= dropped;
        let mut target = first_block;
        if let Some(span) = insert {
            let mut position = first_span;
            if target == self.blocks.len() {
                // Text appended at the end joins the last block.
                if let Some(last) = self.blocks.last() {
                    target -= 1;
                    position = last.spans.len();
                } else {
                    self.blocks.try_reserve(1).map_err(|_| out_of_memory())?;
                    self.blocks.push(Block {
                        len: 0,
                        spans: Vec::new(),
                    });
                }
            }
            if self.spans >= MAX_SPANS {
                return Err(out_of_memory());
            }
            let spans = &mut self.blocks[target].spans;
            spans.try_reserve(1).map_err(|_| out_of_memory())?;
            spans.insert(position, span);
            self.spans += 1;
        }
        let touched = target..last_block.saturating_add(1).min(self.blocks.len());
        for block in &mut self.blocks[touched] {
            block.len = block.spans.iter().map(|span| span.len).sum::<u64>();
        }
        if self
            .blocks
            .get(target)
            .is_some_and(|block| block.spans.len() > 2 * BLOCK)
        {
            let tail = self.blocks[target].spans.split_off(BLOCK);
            let tail_len: u64 = tail.iter().map(|span| span.len).sum();
            self.blocks[target].len -= tail_len;
            self.blocks.try_reserve(1).map_err(|_| out_of_memory())?;
            self.blocks.insert(
                target + 1,
                Block {
                    len: tail_len,
                    spans: tail,
                },
            );
        }
        self.blocks.retain(|block| !block.spans.is_empty());
        self.len = self.len - removed + insert.map_or(0, |span| span.len);
        Ok(())
    }
    /// Stream the reconstructed document; reads each output byte once.
    pub(super) fn write_to(
        &mut self,
        records: &[Record],
        output: &mut impl Write,
        cancel: &Cancellation,
    ) -> io::Result<()> {
        let blocks = std::mem::take(&mut self.blocks);
        let mut buffer = vec![0u8; CHUNK];
        let result = (|| -> io::Result<()> {
            for span in blocks.iter().flat_map(|block| &block.spans) {
                let mut done = 0u64;
                while done < span.len {
                    cancelled(cancel)?;
                    let n = (span.len - done).min(CHUNK as u64) as usize;
                    self.read_origin(records, span.origin, span.offset + done, &mut buffer[..n])?;
                    output.write_all(&buffer[..n])?;
                    done += n as u64;
                }
            }
            Ok(())
        })();
        self.blocks = blocks;
        result
    }
}
