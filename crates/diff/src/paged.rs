// SPDX-License-Identifier: MPL-2.0
//! Poll-driven bounded paged comparison. The caller resolves page tickets on its I/O pool.
//!
//! One indexing pass hashes every normalized line of both sides into a bounded
//! anchor index that spills sorted runs to disk; lines unique on both sides form
//! a patience chain of split points. A second pass reads window pairs that start
//! and end on those aligned splits and diffs each with the resident algorithm, so
//! an inserted line shifts nothing after it. Gaps no window can hold stay local: a
//! one-sided gap is an exact insertion or removal (read only to find its kept
//! lines when blank lines are ignored), anything else a coarse block of its own
//! extent.
use crate::*;
use bareline_document::{
    Budget, Document,
    paged::{PagedSnapshot, TextWindow, WindowPoll, WindowRequest},
    source::PageTicket,
};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, VecDeque},
    fs::File,
    io::{self, BufReader, BufWriter, Read, Write},
    path::PathBuf,
    sync::atomic::AtomicU64,
};

/// Accounted bytes per anchor-index entry at its peak: the 72-byte bucket (key
/// plus both sides' first occurrence) at down to half load after the table
/// grows, then, while `anchors` collects, the table plus a doubling candidate
/// vector, and finally the candidate, predecessor, tail and chain arrays. A
/// table of `cap` entries is copied once into a sorted run, and after spilling
/// the candidate buffer, the chain window and the in-memory chain each hold at
/// most `cap` records, beside `FAN_IN` run buffers.
const INDEX_ENTRY_BYTES: usize = 320;
/// Resident-compare workspace per line of a window pair (line record, anchor
/// maps, Myers rows), on top of the window text itself.
const WINDOW_LINE_BYTES: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}
pub enum PagedComparePoll {
    /// One bounded input window consumed without retained output.
    Progress,
    Pending {
        side: Side,
        ticket: PageTicket,
    },
    Backpressure,
    Batch(Box<PagedBatch>),
    /// A hunk no bounded window pair holds: an exact one-sided insertion or
    /// removal, or (`coarse`) the changed extent of a gap or of the whole source
    /// after every byte was read under the window budget. Applying it requires
    /// caller-owned bounded materialization/staging.
    CoarseBlock(Box<DiffHunk>),
    Finished(CompareCompleteness),
}
struct Reader {
    snapshot: PagedSnapshot,
    cursor: usize,
    end: usize,
    limit: usize,
    retries: usize,
    request: Option<WindowRequest>,
    ready: Option<TextWindow>,
}
impl Reader {
    fn new(snapshot: PagedSnapshot) -> Self {
        Self {
            snapshot,
            cursor: 0,
            end: 0,
            limit: 0,
            retries: 0,
            request: None,
            ready: None,
        }
    }
    /// Read up to `cap` bytes from `cursor`, never past `limit`. Only a window
    /// that stops short of `limit` may give back a split trailing scalar.
    fn poll(&mut self, limit: usize, cap: usize, budget: &Budget) -> Result<Option<PageTicket>, CompareCompleteness> {
        if self.ready.is_some() {
            return Ok(None);
        }
        if self.request.is_none() {
            self.limit = limit.min(self.snapshot.len());
            self.end = self.limit.min(self.cursor.saturating_add(cap));
            self.retries = 0;
            self.request = Some(
                self.snapshot
                    .begin_read(TextOffset(self.cursor)..TextOffset(self.end), cap, budget)
                    .map_err(|_| CompareCompleteness::Failed)?,
            );
        }
        loop {
            match self.request.as_mut().expect("request").poll() {
                WindowPoll::Ready(window) => {
                    self.ready = Some(window);
                    self.request = None;
                    return Ok(None);
                }
                WindowPoll::Pending(ticket) => return Ok(Some(ticket)),
                WindowPoll::Unavailable(_) => return Err(CompareCompleteness::Unavailable),
                WindowPoll::Finished => return Err(CompareCompleteness::Failed),
                WindowPoll::InvalidUtf8 => {
                    // At most three trailing bytes can belong to a split UTF-8 scalar. Interior
                    // malformed UTF-8 fails after these bounded retries, never replacement text.
                    if self.retries == 3 || self.end <= self.cursor || self.end == self.limit {
                        return Err(CompareCompleteness::Failed);
                    }
                    self.end -= 1;
                    self.retries += 1;
                    self.request = Some(
                        self.snapshot
                            .begin_read(TextOffset(self.cursor)..TextOffset(self.end), cap, budget)
                            .map_err(|_| CompareCompleteness::Failed)?,
                    );
                }
            }
        }
    }
}
/// First occurrence and count of one normalized-line hash on one side.
#[derive(Clone, Copy, Default)]
struct Seen {
    count: u32,
    start: usize,
    end: usize,
    line: usize,
}
impl Seen {
    /// One side's counts of a hash from two runs: their sum, and the earlier
    /// first occurrence.
    fn merge(self, other: Self) -> Self {
        if self.count == 0 {
            return other;
        }
        if other.count == 0 {
            return self;
        }
        let first = if other.start < self.start { other } else { self };
        Self {
            count: self.count.saturating_add(other.count),
            ..first
        }
    }
}
/// Line hashes of both sides, exact and in bounded memory. Up to `cap` distinct
/// hashes stay in a table; a larger table is written to a sorted run in the
/// spill store and emptied, and the runs are merged by hash at the end of the
/// indexing pass (SRC-05). Without a spill store, or after a spill fails, the
/// table is sampled by content instead: the sampling rate halves whenever it
/// would exceed `cap`. Sampling depends only on the hash, so both sides keep
/// exactly the same lines, and every occurrence of a kept hash is counted.
struct AnchorIndex {
    lines: HashMap<u64, [Seen; 2]>,
    shift: u32,
    cap: usize,
    /// Sorted runs of earlier table contents, by hash.
    runs: Vec<Run>,
    spill: Option<SpillStore>,
    /// A spill write failed, or there is no spill store: the table is sampled.
    spill_failed: bool,
}
/// True when the top `shift` bits of the mixed hash are zero; each larger shift
/// keeps a subset of the lines the smaller one kept.
fn sampled(hash: u64, shift: u32) -> bool {
    shift == 0 || hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - shift) == 0
}
impl AnchorIndex {
    fn new(cap: usize, spill: Option<PathBuf>) -> Self {
        Self {
            lines: HashMap::new(),
            shift: 0,
            cap,
            runs: Vec::new(),
            spill: spill.map(SpillStore::new),
            spill_failed: false,
        }
    }
    fn record(&mut self, side: usize, hash: u64, seen: Seen) {
        if !sampled(hash, self.shift) {
            return;
        }
        let slot = &mut self.lines.entry(hash).or_default()[side];
        slot.count = slot.count.saturating_add(1);
        if slot.count == 1 {
            *slot = Seen { count: 1, ..seen };
        }
        if self.lines.len() > self.cap && self.shift == 0 && !self.spill_failed {
            match self.spill_lines() {
                Ok(()) => return,
                Err(_) => self.spill_failed = true,
            }
        }
        while self.lines.len() > self.cap && self.shift < 63 {
            self.shift += 1;
            let shift = self.shift;
            self.lines.retain(|hash, _| sampled(*hash, shift));
        }
    }
    /// Writes the table as one sorted run, then empties it. The table is kept
    /// when the write fails, so no count is lost.
    fn spill_lines(&mut self) -> io::Result<()> {
        let store = self
            .spill
            .as_mut()
            .ok_or_else(|| io::Error::other("no spill directory"))?;
        let mut entries: Vec<Anchor> = self
            .lines
            .iter()
            .map(|(&hash, &[left, right])| Anchor { hash, left, right })
            .collect();
        entries.sort_unstable_by_key(|entry| entry.hash);
        let mut out = store.create()?;
        for entry in &entries {
            out.push(entry)?;
        }
        self.runs.push(store.finish(out)?);
        self.lines.clear();
        if self.runs.len() >= FAN_IN
            && let Ok(run) = store.compact(&self.runs, by_hash, true)
        {
            store.remove(&std::mem::take(&mut self.runs));
            self.runs.push(run);
        }
        Ok(())
    }
    /// Lines unique on both sides, reduced to the longest chain ordered on both
    /// (patience), exactly as the resident anchor pass does. With spilled runs
    /// the runs are merged by hash, the candidates are sorted by left offset in
    /// runs of their own, and the chain is chosen in overlapping windows of
    /// `cap` candidates and kept in a run when it outgrows `cap`. A cancel stops
    /// the merge and the choice with an `Interrupted` error.
    fn anchors(&mut self, cancel: &CancelToken) -> io::Result<AnchorChain> {
        let unique = |entry: &Anchor| entry.left.count == 1 && entry.right.count == 1;
        let lines = std::mem::take(&mut self.lines);
        if self.runs.is_empty() {
            let mut candidates: Vec<Anchor> = lines
                .into_iter()
                .map(|(hash, [left, right])| Anchor { hash, left, right })
                .filter(unique)
                .collect();
            candidates.sort_unstable_by_key(|anchor| anchor.left.start);
            let chain = patience(&candidates, None)
                .into_iter()
                .map(|index| candidates[index])
                .collect();
            return Ok(AnchorChain::memory(chain));
        }
        let (shift, cap) = (self.shift, self.cap);
        let store = self
            .spill
            .as_mut()
            .ok_or_else(|| io::Error::other("no spill directory"))?;
        let mut last: Vec<Anchor> = lines
            .into_iter()
            .map(|(hash, [left, right])| Anchor { hash, left, right })
            .collect();
        last.sort_unstable_by_key(|entry| entry.hash);
        let mut entries = Merge::new(&self.runs, last, by_hash)?;
        let mut sorted: Vec<Run> = Vec::new();
        let mut buffer: Vec<Anchor> = Vec::new();
        let mut held: Option<Anchor> = None;
        loop {
            if cancel.is_cancelled() {
                return Err(interrupted());
            }
            let record = entries.pull()?;
            if let (Some(entry), Some(record)) = (held.as_mut(), record.as_ref())
                && entry.hash == record.hash
            {
                entry.absorb(record);
                continue;
            }
            if let Some(entry) = held.take()
                && sampled(entry.hash, shift)
                && unique(&entry)
            {
                buffer.push(entry);
                if buffer.len() >= cap {
                    buffer.sort_unstable_by_key(|anchor| anchor.left.start);
                    let mut out = store.create()?;
                    for anchor in &buffer {
                        out.push(anchor)?;
                    }
                    sorted.push(store.finish(out)?);
                    buffer.clear();
                    if sorted.len() >= FAN_IN {
                        let run = store.compact(&sorted, by_left, false)?;
                        store.remove(&std::mem::take(&mut sorted));
                        sorted.push(run);
                    }
                }
            }
            match record {
                Some(record) => held = Some(record),
                None => break,
            }
        }
        drop(entries);
        store.remove(&std::mem::take(&mut self.runs));
        buffer.sort_unstable_by_key(|anchor| anchor.left.start);
        let mut candidates = Merge::new(&sorted, buffer, by_left)?;
        let mut chain = ChainWriter {
            limit: cap,
            memory: Vec::new(),
            file: None,
        };
        choose(
            || candidates.pull(),
            cap,
            || cancel.is_cancelled(),
            |anchor| chain.push(store, anchor),
        )?;
        drop(candidates);
        store.remove(&sorted);
        let mut chain = chain.finish(store)?;
        if chain.source.is_some() {
            // The chain's run is read during the second pass; its store goes
            // with it, after the reader, so the directory is removed last.
            chain.store = self.spill.take();
        }
        Ok(chain)
    }
}
/// The error that ends a spilled merge or choice once the compare is cancelled.
fn interrupted() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "compare cancelled")
}
#[derive(Clone, Copy)]
struct Anchor {
    hash: u64,
    left: Seen,
    right: Seen,
}
/// Bytes of one spilled record: a line hash and both sides' `Seen`.
const RECORD_BYTES: usize = 72;
/// Runs one merge reads at once; more are merged into one run first.
const FAN_IN: usize = 64;
/// Buffer of each run reader and writer.
const RUN_BUFFER: usize = 16 * 1024;
fn by_hash(record: &Anchor) -> u64 {
    record.hash
}
fn by_left(record: &Anchor) -> u64 {
    record.left.start as u64
}
impl Anchor {
    /// Adds another run's counts of the same hash.
    fn absorb(&mut self, other: &Self) {
        self.left = self.left.merge(other.left);
        self.right = self.right.merge(other.right);
    }
    fn encode(&self) -> [u8; RECORD_BYTES] {
        let words = [
            self.hash,
            u64::from(self.left.count),
            self.left.start as u64,
            self.left.end as u64,
            self.left.line as u64,
            u64::from(self.right.count),
            self.right.start as u64,
            self.right.end as u64,
            self.right.line as u64,
        ];
        let mut bytes = [0; RECORD_BYTES];
        for (chunk, word) in bytes.as_chunks_mut::<8>().0.iter_mut().zip(words) {
            *chunk = word.to_le_bytes();
        }
        bytes
    }
    fn decode(bytes: &[u8; RECORD_BYTES]) -> Self {
        let mut words = [0u64; RECORD_BYTES / 8];
        for (word, chunk) in words.iter_mut().zip(bytes.as_chunks::<8>().0) {
            *word = u64::from_le_bytes(*chunk);
        }
        let seen = |at: usize| Seen {
            count: u32::try_from(words[at]).unwrap_or(u32::MAX),
            start: words[at + 1] as usize,
            end: words[at + 2] as usize,
            line: words[at + 3] as usize,
        };
        Self {
            hash: words[0],
            left: seen(1),
            right: seen(5),
        }
    }
}
/// Indices of the longest chain of `candidates` (in left order) increasing on
/// the right and starting past `floor` (patience), as the resident anchor pass
/// picks it.
fn patience(candidates: &[Anchor], floor: Option<usize>) -> Vec<usize> {
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; candidates.len()];
    for (idx, candidate) in candidates.iter().enumerate() {
        let j = candidate.right.start;
        if floor.is_some_and(|floor| j <= floor) {
            continue;
        }
        let p = tails.partition_point(|&t| candidates[t].right.start < j);
        if p > 0 {
            prev[idx] = tails[p - 1];
        }
        if p == tails.len() {
            tails.push(idx);
        } else {
            tails[p] = idx;
        }
    }
    let mut chain = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(idx) = at {
        chain.push(idx);
        at = (prev[idx] != usize::MAX).then_some(prev[idx]);
    }
    chain.reverse();
    chain
}
/// Chooses the chain from candidates in left order, `window` at a time:
/// patience over the window, keeping the part of its chain in the window's
/// first half, so a line moved by less than half a window is judged with what
/// follows it. When every candidate fits one window (the end of input is
/// probed before a full window is split), this is the exact chain of the
/// in-memory pass. `cancelled` is checked once per window.
fn choose(
    mut next: impl FnMut() -> io::Result<Option<Anchor>>,
    window: usize,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(Anchor) -> io::Result<()>,
) -> io::Result<()> {
    let window = window.max(2);
    let mut pending: Vec<Anchor> = Vec::new();
    // The candidate after a full window, read to learn whether input ends there.
    let mut ahead: Option<Anchor> = None;
    let mut floor = None;
    let mut done = false;
    loop {
        if cancelled() {
            return Err(interrupted());
        }
        pending.extend(ahead.take());
        while !done && pending.len() < window {
            match next()? {
                Some(anchor) => pending.push(anchor),
                None => done = true,
            }
        }
        if !done {
            match next()? {
                Some(anchor) => ahead = Some(anchor),
                None => done = true,
            }
        }
        let keep = if done { pending.len() } else { pending.len() / 2 };
        for index in patience(&pending, floor) {
            if index >= keep {
                break;
            }
            floor = Some(pending[index].right.start);
            emit(pending[index])?;
        }
        if done {
            return Ok(());
        }
        pending.drain(..keep);
    }
}
static SPILLS: AtomicU64 = AtomicU64::new(0);
/// An owned directory of sorted runs, made on first use and removed with its
/// owner. Runs hold line hashes, offsets and line numbers, never text.
struct SpillStore {
    parent: PathBuf,
    dir: Option<PathBuf>,
    files: u64,
    /// Records written, merged runs included.
    written: usize,
}
/// One sorted run on disk.
struct Run {
    path: PathBuf,
    records: usize,
}
impl SpillStore {
    fn new(parent: PathBuf) -> Self {
        Self {
            parent,
            dir: None,
            files: 0,
            written: 0,
        }
    }
    fn create(&mut self) -> io::Result<RunWriter> {
        let dir = match &self.dir {
            Some(dir) => dir.clone(),
            None => {
                let mut made = None;
                for _ in 0..16 {
                    let dir = self.parent.join(format!(
                        "bareline-compare-{}-{}",
                        std::process::id(),
                        SPILLS.fetch_add(1, Ordering::Relaxed)
                    ));
                    match std::fs::create_dir(&dir) {
                        Ok(()) => {
                            made = Some(dir);
                            break;
                        }
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                }
                let dir = made.ok_or_else(|| io::Error::other("no free spill directory name"))?;
                self.dir = Some(dir.clone());
                dir
            }
        };
        let path = dir.join(format!("{}.run", self.files));
        self.files += 1;
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        Ok(RunWriter {
            path,
            out: BufWriter::with_capacity(RUN_BUFFER, file),
            records: 0,
        })
    }
    /// Completes a run `create` started.
    fn finish(&mut self, mut out: RunWriter) -> io::Result<Run> {
        out.out.flush()?;
        self.written += out.records;
        Ok(Run {
            path: out.path,
            records: out.records,
        })
    }
    /// Merges `runs` into one, summing equal hashes when `sum` is set.
    fn compact(&mut self, runs: &[Run], key: fn(&Anchor) -> u64, sum: bool) -> io::Result<Run> {
        let mut merge = Merge::new(runs, Vec::new(), key)?;
        let mut out = self.create()?;
        let mut held: Option<Anchor> = None;
        while let Some(record) = merge.pull()? {
            if sum
                && let Some(entry) = held.as_mut()
                && entry.hash == record.hash
            {
                entry.absorb(&record);
                continue;
            }
            if let Some(entry) = held.replace(record) {
                out.push(&entry)?;
            }
        }
        if let Some(entry) = held {
            out.push(&entry)?;
        }
        drop(merge);
        self.finish(out)
    }
    fn remove(&self, runs: &[Run]) {
        for run in runs {
            let _ = std::fs::remove_file(&run.path);
        }
    }
}
impl Drop for SpillStore {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
struct RunWriter {
    path: PathBuf,
    out: BufWriter<File>,
    records: usize,
}
impl RunWriter {
    fn push(&mut self, record: &Anchor) -> io::Result<()> {
        self.out.write_all(&record.encode())?;
        self.records += 1;
        Ok(())
    }
}
struct RunReader {
    input: BufReader<File>,
    left: usize,
}
impl RunReader {
    fn open(run: &Run) -> io::Result<Self> {
        Ok(Self {
            input: BufReader::with_capacity(RUN_BUFFER, File::open(&run.path)?),
            left: run.records,
        })
    }
    fn pull(&mut self) -> io::Result<Option<Anchor>> {
        if self.left == 0 {
            return Ok(None);
        }
        let mut bytes = [0; RECORD_BYTES];
        self.input.read_exact(&mut bytes)?;
        self.left -= 1;
        Ok(Some(Anchor::decode(&bytes)))
    }
}
enum Source {
    Run(RunReader),
    Memory(std::vec::IntoIter<Anchor>),
}
/// The records of sorted runs and one sorted in-memory list, in key order;
/// equal keys come out in source order.
struct Merge {
    sources: Vec<Source>,
    heads: Vec<Option<Anchor>>,
    queue: BinaryHeap<Reverse<(u64, usize)>>,
    key: fn(&Anchor) -> u64,
}
impl Merge {
    fn new(runs: &[Run], memory: Vec<Anchor>, key: fn(&Anchor) -> u64) -> io::Result<Self> {
        let mut sources = Vec::with_capacity(runs.len() + 1);
        for run in runs {
            sources.push(Source::Run(RunReader::open(run)?));
        }
        sources.push(Source::Memory(memory.into_iter()));
        let mut merge = Self {
            heads: vec![None; sources.len()],
            queue: BinaryHeap::with_capacity(sources.len()),
            sources,
            key,
        };
        for index in 0..merge.sources.len() {
            merge.advance(index)?;
        }
        Ok(merge)
    }
    fn advance(&mut self, index: usize) -> io::Result<()> {
        let record = match &mut self.sources[index] {
            Source::Run(run) => run.pull()?,
            Source::Memory(records) => records.next(),
        };
        if let Some(record) = &record {
            self.queue.push(Reverse(((self.key)(record), index)));
        }
        self.heads[index] = record;
        Ok(())
    }
    fn pull(&mut self) -> io::Result<Option<Anchor>> {
        let Some(Reverse((_, index))) = self.queue.pop() else {
            return Ok(None);
        };
        let record = self.heads[index].take();
        self.advance(index)?;
        Ok(record)
    }
}
/// The chosen chain while it is written: in memory up to `limit` anchors,
/// then in a run.
struct ChainWriter {
    limit: usize,
    memory: Vec<Anchor>,
    file: Option<RunWriter>,
}
impl ChainWriter {
    fn push(&mut self, store: &mut SpillStore, anchor: Anchor) -> io::Result<()> {
        if let Some(file) = &mut self.file {
            return file.push(&anchor);
        }
        self.memory.push(anchor);
        if self.memory.len() > self.limit {
            let mut file = store.create()?;
            for anchor in &self.memory {
                file.push(anchor)?;
            }
            self.memory = Vec::new();
            self.file = Some(file);
        }
        Ok(())
    }
    fn finish(self, store: &mut SpillStore) -> io::Result<AnchorChain> {
        let Some(file) = self.file else {
            return Ok(AnchorChain::memory(self.memory));
        };
        let run = store.finish(file)?;
        Ok(AnchorChain {
            len: run.records,
            base: 0,
            loaded: VecDeque::new(),
            source: Some(RunReader::open(&run)?),
            limit: self.limit.max(2),
            store: None,
        })
    }
}
/// The anchor chain the second pass walks forward: all in memory, or read
/// from its run a bounded lookahead at a time.
struct AnchorChain {
    len: usize,
    /// Chain index of `loaded[0]`.
    base: usize,
    loaded: VecDeque<Anchor>,
    source: Option<RunReader>,
    /// Most anchors loaded from the run at once.
    limit: usize,
    /// The store holding the run; dropped after `source` closes it.
    store: Option<SpillStore>,
}
impl AnchorChain {
    fn memory(chain: Vec<Anchor>) -> Self {
        Self {
            len: chain.len(),
            base: 0,
            loaded: chain.into(),
            source: None,
            limit: usize::MAX,
            store: None,
        }
    }
    /// Drops the anchors before `from` and loads those a window from the
    /// current split can use: up to the first that ends past `reach` on either
    /// side, and at least two, within `limit`.
    fn fill(&mut self, from: usize, reach: (usize, usize)) -> io::Result<()> {
        while self.base < from && self.loaded.pop_front().is_some() {
            self.base += 1;
        }
        if let Some(source) = self.source.as_mut() {
            while self.base < from && source.pull()?.is_some() {
                self.base += 1;
            }
            while self.loaded.len() < self.limit
                && (self.loaded.len() < 2
                    || self
                        .loaded
                        .back()
                        .is_some_and(|last| last.left.end <= reach.0 && last.right.end <= reach.1))
            {
                match source.pull()? {
                    Some(anchor) => self.loaded.push_back(anchor),
                    None => break,
                }
            }
        }
        let _ = self.loaded.make_contiguous();
        Ok(())
    }
    /// The loaded anchors and the chain index of the first.
    fn loaded(&self) -> (&[Anchor], usize) {
        (self.loaded.as_slices().0, self.base)
    }
}
/// Splits one side into physical lines across page windows, as the resident
/// line pass does, and records each line's normalized hash in the index.
#[derive(Default)]
struct Lines {
    raw: String,
    /// The current line outgrew `MAX_LINE_BYTES`; it is counted, never an anchor.
    long: bool,
    pending_cr: bool,
    start: usize,
    line: usize,
    /// The resident hint for an insertion at end of input (`line_count() - 1`).
    eof_line: usize,
}
impl Lines {
    fn feed(&mut self, text: &str, mut offset: usize, side: usize, index: &mut AnchorIndex, o: &CompareOptions) {
        let mut rest = text;
        while !rest.is_empty() {
            if self.pending_cr {
                self.pending_cr = false;
                if let Some(tail) = rest.strip_prefix('\n') {
                    self.push("\n");
                    rest = tail;
                    offset += 1;
                }
                self.finish_line(offset, side, index, o);
                continue;
            }
            match rest.bytes().position(|byte| matches!(byte, b'\r' | b'\n')) {
                None => {
                    self.push(rest);
                    rest = "";
                }
                Some(k) => {
                    self.push(&rest[..=k]);
                    offset += k + 1;
                    let cr = rest.as_bytes()[k] == b'\r';
                    rest = &rest[k + 1..];
                    if cr {
                        self.pending_cr = true;
                    } else {
                        self.finish_line(offset, side, index, o);
                    }
                }
            }
        }
    }
    fn push(&mut self, text: &str) {
        if self.long {
            return;
        }
        if self.raw.len() + text.len() > MAX_LINE_BYTES {
            self.long = true;
            self.raw = String::new();
        } else {
            self.raw.push_str(text);
        }
    }
    fn finish_line(&mut self, end: usize, side: usize, index: &mut AnchorIndex, o: &CompareOptions) {
        if !self.long && !(o.ignore_blank_lines && self.raw.trim().is_empty()) {
            let seen = Seen {
                count: 1,
                start: self.start,
                end,
                line: self.line,
            };
            index.record(side, hash(&normalize(&self.raw, o, self.line == 0)), seen);
        }
        self.raw.clear();
        self.long = false;
        self.start = end;
        self.line += 1;
    }
    /// End of input: a pending CR or an unterminated final line completes here.
    fn finish(&mut self, end: usize, side: usize, index: &mut AnchorIndex, o: &CompareOptions) {
        let unterminated = !self.pending_cr && (self.long || !self.raw.is_empty());
        if self.pending_cr || unterminated {
            self.pending_cr = false;
            self.finish_line(end, side, index, o);
        }
        self.eof_line = self.line - usize::from(unterminated);
    }
}
/// A position aligned on both sides: byte offsets, the physical line starting
/// there, and the normalized hash of the anchor line just before it (0 if none).
#[derive(Clone, Copy, Default)]
struct Split {
    left: usize,
    right: usize,
    left_line: usize,
    right_line: usize,
    hash: u64,
}
enum Plan {
    Done,
    /// Diff the window pair up to `end`, then continue at anchor `next`.
    Window {
        end: Split,
        anchor: bool,
        next: usize,
    },
    /// No window reaches the next split; handle the gap up to `end` on its own.
    Gap {
        end: Split,
        next: usize,
        next_hash: u64,
    },
}
/// A two-sided gap scanned in byte windows for its changed extent, or a
/// one-sided gap under `ignore_blank_lines` scanned for its kept lines.
struct Gap {
    start: Split,
    end: Split,
    next: usize,
    next_hash: u64,
    extent: Option<(Range<TextOffset>, Range<TextOffset>)>,
    kept: Option<KeptLines>,
}
/// The first and last lines of one side that the resident line pass keeps
/// under `ignore_blank_lines` (lines with a non-whitespace character), found
/// across page windows with the resident CR/LF line rules.
struct KeptLines {
    line_start: usize,
    line: usize,
    nonblank: bool,
    pending_cr: bool,
    /// Start offset and physical line of the first kept line.
    first: Option<(usize, usize)>,
    /// End offset, after its terminator, of the last kept line.
    last_end: usize,
}
impl KeptLines {
    fn new(start: usize, line: usize) -> Self {
        Self {
            line_start: start,
            line,
            nonblank: false,
            pending_cr: false,
            first: None,
            last_end: start,
        }
    }
    fn feed(&mut self, text: &str, offset: usize) {
        for (k, ch) in text.char_indices() {
            let at = offset + k;
            if self.pending_cr {
                self.pending_cr = false;
                if ch == '\n' {
                    self.end_line(at + 1);
                    continue;
                }
                self.end_line(at);
            }
            match ch {
                '\r' => self.pending_cr = true,
                '\n' => self.end_line(at + 1),
                ch if !ch.is_whitespace() => self.nonblank = true,
                _ => {}
            }
        }
    }
    fn end_line(&mut self, end: usize) {
        if self.nonblank {
            self.first.get_or_insert((self.line_start, self.line));
            self.last_end = end;
        }
        self.nonblank = false;
        self.line_start = end;
        self.line += 1;
    }
    /// A gap ends at a line start or at end of input; a pending CR or an
    /// unterminated final line completes there.
    fn finish(&mut self, end: usize) {
        if self.pending_cr || self.line_start < end {
            self.pending_cr = false;
            self.end_line(end);
        }
    }
}
fn widen(
    extent: &mut Option<(Range<TextOffset>, Range<TextOffset>)>,
    left: Range<TextOffset>,
    right: Range<TextOffset>,
) {
    if let Some((l, r)) = extent {
        l.end = left.end;
        r.end = right.end;
    } else {
        *extent = Some((left, right));
    }
}
/// The final line of a window pair's text: the anchor both sides split after.
fn last_line(text: &str) -> &str {
    let body = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .or_else(|| text.strip_suffix('\r'))
        .unwrap_or(text);
    body.rfind(['\r', '\n']).map_or(text, |i| &text[i + 1..])
}
/// One outstanding batch is allowed. Drop it after consuming/applying to release
/// backpressure and its globally budgeted owned windows.
pub struct PagedBatch {
    pub hunks: Vec<DiffHunk>,
    pub completeness: CompareCompleteness,
    left: TextWindow,
    right: TextWindow,
    left_snapshot: PagedSnapshot,
    right_snapshot: PagedSnapshot,
    lease: Arc<AtomicBool>,
}
impl Drop for PagedBatch {
    fn drop(&mut self) {
        self.lease.store(false, Ordering::Release);
    }
}
impl PagedBatch {
    pub fn windows(&self) -> (&TextWindow, &TextWindow) {
        (&self.left, &self.right)
    }
    /// Validates both current snapshot states before producing a normal paged edit.
    /// Supply windows() to PagedDocument::apply_materialized on the destination.
    pub fn apply(
        &self,
        index: usize,
        direction: Direction,
        left_now: &PagedSnapshot,
        right_now: &PagedSnapshot,
        max_bytes: usize,
        policy: MergePolicy,
    ) -> Result<EditTransaction, ApplyError> {
        if left_now.revision != self.left_snapshot.revision
            || right_now.revision != self.right_snapshot.revision
            || left_now.content_state != self.left_snapshot.content_state
            || right_now.content_state != self.right_snapshot.content_state
        {
            return Err(ApplyError::Stale);
        }
        if !self.left.matches_snapshot(left_now) || !self.right.matches_snapshot(right_now) {
            return Err(ApplyError::Stale);
        }
        let h = self.hunks.get(index).ok_or(ApplyError::InvalidRange)?;
        if policy == MergePolicy::PreserveIgnoredDestination
            && ignores(&h.options)
            && self.completeness == CompareCompleteness::Coarse(CoarseReason::Windowed)
        {
            return Err(ApplyError::UnsupportedPreserve);
        }
        let lr = self.left.range();
        let rr = self.right.range();
        if h.left.start < lr.start
            || h.left.end > lr.end
            || h.right.start < rr.start
            || h.right.end > rr.end
            || h.left.start > h.left.end
            || h.right.start > h.right.end
        {
            return Err(ApplyError::InvalidRange);
        }
        if self.left.text().len().saturating_add(self.right.text().len()) > max_bytes {
            return Err(ApplyError::BudgetExceeded);
        }
        let budget = Budget::new(max_bytes.saturating_mul(8).saturating_add(4096));
        let ld = Document::from_utf8(self.left.text(), budget.clone(), Budget::new(0))
            .map_err(|_| ApplyError::BudgetExceeded)?;
        let rd =
            Document::from_utf8(self.right.text(), budget, Budget::new(0)).map_err(|_| ApplyError::BudgetExceeded)?;
        let ls = ld.snapshot();
        let rs = rd.snapshot();
        let mut local = make_hunk(
            &ls,
            &rs,
            TextOffset(h.left.start.0 - lr.start.0)..TextOffset(h.left.end.0 - lr.start.0),
            TextOffset(h.right.start.0 - rr.start.0)..TextOffset(h.right.end.0 - rr.start.0),
            (0, 0),
            h.stable_id.0,
        );
        local.options = h.options.clone();
        let mut transaction = apply_hunk_with_policy(policy, direction, &local, &ls, &rs, max_bytes)?;
        let (offset, revision) = match direction {
            Direction::LeftToRight => (rr.start.0, right_now.revision),
            Direction::RightToLeft => (lr.start.0, left_now.revision),
        };
        for edit in &mut transaction.edits {
            edit.range = TextOffset(edit.range.start.0 + offset)..TextOffset(edit.range.end.0 + offset);
        }
        transaction.base_revision = revision;
        Ok(transaction)
    }
}
pub struct PagedCompareJob {
    left: Reader,
    right: Reader,
    options: CompareOptions,
    cancel: CancelToken,
    budget: Budget,
    cap: usize,
    lease: Arc<AtomicBool>,
    terminal: Option<CompareCompleteness>,
    quality: CompareCompleteness,
    refinement_time: std::time::Duration,
    global_equal: bool,
    changed_extent: Option<(Range<TextOffset>, Range<TextOffset>)>,
    index: AnchorIndex,
    lines: [Lines; 2],
    /// The aligned split chain, set once the indexing pass completes; a
    /// spilled chain is read forward from its run a bounded window at a time.
    anchors: Option<AnchorChain>,
    next_anchor: usize,
    at: Split,
    /// The window pair being read: its end split, whether an anchor closes it,
    /// and the anchor to continue from.
    window: Option<(Split, bool, usize)>,
    gap: Option<Gap>,
    /// Bytes delivered by page windows across both passes.
    read_bytes: usize,
    sampled_gaps: usize,
}
impl PagedCompareJob {
    pub fn new(left: PagedSnapshot, right: PagedSnapshot, options: CompareOptions, cancel: CancelToken) -> Self {
        let budget = Budget::new(options.limits.max_memory_bytes / 8);
        let cap = (options.limits.max_memory_bytes / 64).clamp(4, 64 * 1024);
        let terminal = if options.limits.max_memory_bytes < 8192 {
            Some(CompareCompleteness::Failed)
        } else {
            None
        };
        let global_equal = left.len() == right.len();
        Self {
            left: Reader::new(left),
            right: Reader::new(right),
            index: AnchorIndex::new(
                (options.limits.max_memory_bytes / 4 / INDEX_ENTRY_BYTES).max(16),
                Some(std::env::temp_dir()),
            ),
            options,
            cancel,
            budget,
            cap,
            lease: Arc::new(AtomicBool::new(false)),
            terminal,
            quality: CompareCompleteness::Exact,
            refinement_time: std::time::Duration::ZERO,
            global_equal,
            changed_extent: None,
            lines: [Lines::default(), Lines::default()],
            anchors: None,
            next_anchor: 0,
            at: Split::default(),
            window: None,
            gap: None,
            read_bytes: 0,
            sampled_gaps: 0,
        }
    }
    /// Spill the anchor index's sorted runs into a directory made under `dir`
    /// (by default the system temporary directory) and removed with the job.
    /// `None` keeps the index in memory: above its share it is sampled by
    /// content, as after a failed spill.
    pub fn with_spill_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.index.spill = dir.map(SpillStore::new);
        self
    }
    /// Coarse blocks reported for gaps while the anchor index was sampled
    /// (more distinct lines than its memory share holds, and no spill store) or
    /// its spilled runs could not be read back. Such a block may cover a local
    /// change that a full index would have diffed exactly: nonzero means
    /// reduced precision.
    pub fn sampled_gaps(&self) -> usize {
        self.sampled_gaps
    }
    /// Index records written to spill runs so far, merged runs included.
    pub fn spilled_records(&self) -> usize {
        let store = |store: &Option<SpillStore>| store.as_ref().map_or(0, |store| store.written);
        store(&self.index.spill) + self.anchors.as_ref().map_or(0, |chain| store(&chain.store))
    }
    pub fn poll(&mut self) -> PagedComparePoll {
        if self.cancel.is_cancelled() {
            self.terminal = Some(CompareCompleteness::Cancelled);
        }
        if let Some(state) = self.terminal {
            return PagedComparePoll::Finished(state);
        }
        if self.lease.load(Ordering::Acquire) {
            return PagedComparePoll::Backpressure;
        }
        if self.anchors.is_none() {
            self.poll_index()
        } else if self.gap.is_some() {
            self.poll_gap()
        } else {
            self.poll_window()
        }
    }
    fn finish(&mut self, state: CompareCompleteness) -> PagedComparePoll {
        self.terminal = Some(state);
        PagedComparePoll::Finished(state)
    }
    fn degrade(&mut self, reason: CoarseReason) {
        if self.quality == CompareCompleteness::Exact {
            self.quality = CompareCompleteness::Coarse(reason);
        }
    }
    /// Whether one window pair of these sizes stays within the resident
    /// compare's exact byte bound and its share of the memory budget.
    fn fits(&self, bytes: (usize, usize), lines: (usize, usize)) -> bool {
        let limits = &self.options.limits;
        bytes.0 <= self.cap
            && bytes.1 <= self.cap
            && bytes.0 + bytes.1 <= limits.max_bytes_exact
            && (lines.0 + lines.1)
                .saturating_mul(WINDOW_LINE_BYTES)
                .saturating_add(bytes.0 + bytes.1)
                <= limits.max_memory_bytes / 4
    }
    fn block(
        &self,
        left: Range<TextOffset>,
        right: Range<TextOffset>,
        coarse: bool,
        hints: (Option<usize>, Option<usize>),
        id: u64,
    ) -> DiffHunk {
        let kind = if left.is_empty() {
            DiffKind::Added
        } else if right.is_empty() {
            DiffKind::Removed
        } else {
            DiffKind::Changed
        };
        DiffHunk {
            stable_id: HunkId(id),
            left_line_hint: hints.0,
            right_line_hint: hints.1,
            left,
            right,
            kind,
            coarse,
            intraline: Vec::new(),
            left_revision: self.left.snapshot.revision,
            right_revision: self.right.snapshot.revision,
            left_state: self.left.snapshot.content_state,
            right_state: self.right.snapshot.content_state,
            options: self.options.clone(),
        }
    }
    /// Pass one: read both sides in lockstep byte windows, hash every line into
    /// the anchor index and track byte equality for the no-anchor fallbacks.
    fn poll_index(&mut self) -> PagedComparePoll {
        let (llen, rlen) = (self.left.snapshot.len(), self.right.snapshot.len());
        if self.left.cursor == llen && self.right.cursor == rlen {
            self.lines[0].finish(llen, 0, &mut self.index, &self.options);
            self.lines[1].finish(rlen, 1, &mut self.index, &self.options);
            if self.global_equal {
                return self.finish(CompareCompleteness::Exact);
            }
            let anchors = match self.index.anchors(&self.cancel) {
                Ok(chain) => chain,
                Err(_) if self.cancel.is_cancelled() => return self.finish(CompareCompleteness::Cancelled),
                Err(_) => {
                    // Spilled runs that cannot be read back leave no anchors; the
                    // byte windows still bound the changed extent.
                    self.index.spill_failed = true;
                    AnchorChain::memory(Vec::new())
                }
            };
            let lines = (self.lines[0].line, self.lines[1].line);
            if anchors.len == 0 && !self.fits((llen, rlen), lines) {
                // Nothing aligns: the byte windows already bound the changed extent.
                let Some((left, right)) = self.changed_extent.take() else {
                    return self.finish(CompareCompleteness::Exact);
                };
                self.terminal = Some(CompareCompleteness::Coarse(CoarseReason::Bytes));
                let hints = ((left.start.0 == 0).then_some(0), (right.start.0 == 0).then_some(0));
                return PagedComparePoll::CoarseBlock(Box::new(self.block(left, right, true, hints, 0)));
            }
            self.anchors = Some(anchors);
            return PagedComparePoll::Progress;
        }
        for (side, reader) in [(Side::Left, &mut self.left), (Side::Right, &mut self.right)] {
            if self.cancel.is_cancelled() {
                self.terminal = Some(CompareCompleteness::Cancelled);
                return PagedComparePoll::Finished(CompareCompleteness::Cancelled);
            }
            if reader.cursor == reader.snapshot.len() {
                continue;
            }
            match reader.poll(reader.snapshot.len(), self.cap, &self.budget) {
                Ok(Some(ticket)) => return PagedComparePoll::Pending { side, ticket },
                Ok(None) => {}
                Err(state) => {
                    // Only disjoint fields while the readers are borrowed.
                    self.terminal = Some(state);
                    return PagedComparePoll::Finished(state);
                }
            }
        }
        if self.cancel.is_cancelled() {
            return self.finish(CompareCompleteness::Cancelled);
        }
        let left = self.left.ready.take();
        let right = self.right.ready.take();
        let lr = left
            .as_ref()
            .map_or(TextOffset(llen)..TextOffset(llen), TextWindow::range);
        let rr = right
            .as_ref()
            .map_or(TextOffset(rlen)..TextOffset(rlen), TextWindow::range);
        let lt = left.as_ref().map_or("", TextWindow::text);
        let rt = right.as_ref().map_or("", TextWindow::text);
        if lt != rt {
            self.global_equal = false;
            widen(&mut self.changed_extent, lr.clone(), rr.clone());
        }
        self.lines[0].feed(lt, lr.start.0, 0, &mut self.index, &self.options);
        self.lines[1].feed(rt, rr.start.0, 1, &mut self.index, &self.options);
        self.read_bytes += lt.len() + rt.len();
        self.left.cursor = lr.end.0;
        self.right.cursor = rr.end.0;
        // Return between bounded windows, including when cached pages are immediately
        // available, so the caller controls scheduling and cancellation latency.
        PagedComparePoll::Progress
    }
    /// The next step of pass two from the current aligned split.
    fn plan(&self) -> Plan {
        let (llen, rlen) = (self.left.snapshot.len(), self.right.snapshot.len());
        let at = self.at;
        if at.left == llen && at.right == rlen {
            return Plan::Done;
        }
        // The loaded part of the chain from `next_anchor` on; `fill` keeps it
        // past the farthest anchor a window from here can reach.
        let (loaded, base) = self.anchors.as_ref().map_or((&[][..], 0), AnchorChain::loaded);
        let total = self.anchors.as_ref().map_or(0, |chain| chain.len);
        let from = self.next_anchor.saturating_sub(base);
        let anchors = || {
            loaded
                .iter()
                .enumerate()
                .skip(from)
                .map(move |(k, anchor)| (base + k, anchor))
        };
        let span = |end: &Split| {
            (
                (end.left.saturating_sub(at.left), end.right.saturating_sub(at.right)),
                (
                    end.left_line.saturating_sub(at.left_line),
                    end.right_line.saturating_sub(at.right_line),
                ),
            )
        };
        let mut best = None;
        let mut all_fit = true;
        for (k, anchor) in anchors() {
            let end = Split {
                left: anchor.left.end,
                right: anchor.right.end,
                left_line: anchor.left.line + 1,
                right_line: anchor.right.line + 1,
                hash: anchor.hash,
            };
            let (bytes, lines) = span(&end);
            if !self.fits(bytes, lines) {
                all_fit = false;
                break;
            }
            best = Some(Plan::Window {
                end,
                anchor: true,
                next: k + 1,
            });
        }
        let eof = Split {
            left: llen,
            right: rlen,
            left_line: self.lines[0].eof_line,
            right_line: self.lines[1].eof_line,
            hash: 0,
        };
        if all_fit {
            let (bytes, lines) = span(&eof);
            if self.fits(bytes, lines) {
                best = Some(Plan::Window {
                    end: eof,
                    anchor: false,
                    next: total,
                });
            }
        }
        if let Some(plan) = best {
            return plan;
        }
        // When only the next anchor line breaks the fit, the two-sided region
        // before it is still diffed exactly instead of joining a coarse gap.
        if let Some(anchor) = loaded.get(from)
            && anchor.left.start > at.left
            && anchor.right.start > at.right
        {
            let end = Split {
                left: anchor.left.start,
                right: anchor.right.start,
                left_line: anchor.left.line,
                right_line: anchor.right.line,
                hash: at.hash,
            };
            let (bytes, lines) = span(&end);
            if self.fits(bytes, lines) {
                return Plan::Window {
                    end,
                    anchor: false,
                    next: self.next_anchor,
                };
            }
        }
        // The gap ends where the next anchor line starts. An anchor starting right
        // here that no window can hold joins the gap, so every gap makes progress.
        match anchors().find(|(_, anchor)| (anchor.left.start, anchor.right.start) != (at.left, at.right)) {
            Some((k, anchor)) => Plan::Gap {
                end: Split {
                    left: anchor.left.start,
                    right: anchor.right.start,
                    left_line: anchor.left.line,
                    right_line: anchor.right.line,
                    hash: at.hash,
                },
                next: k,
                next_hash: anchor.hash,
            },
            None => Plan::Gap {
                end: eof,
                next: total,
                next_hash: 0,
            },
        }
    }
    /// Pass two: diff one aligned window pair, or start handling a gap.
    fn poll_window(&mut self) -> PagedComparePoll {
        let planned = match self.window {
            Some(window) => Plan::Window {
                end: window.0,
                anchor: window.1,
                next: window.2,
            },
            None => {
                let reach = (
                    self.at.left.saturating_add(self.cap),
                    self.at.right.saturating_add(self.cap),
                );
                let from = self.next_anchor;
                // A chain run that cannot be read back ends the compare.
                let unreadable = self
                    .anchors
                    .as_mut()
                    .is_some_and(|chain| chain.fill(from, reach).is_err());
                if unreadable {
                    return self.finish(CompareCompleteness::Failed);
                }
                self.plan()
            }
        };
        let (end, anchor, next) = match planned {
            Plan::Done => return self.finish(self.quality),
            Plan::Window { end, anchor, next } => (end, anchor, next),
            Plan::Gap { end, next, next_hash } => {
                let start = self.at;
                let one_sided = start.left == end.left || start.right == end.right;
                if one_sided && !self.options.ignore_blank_lines {
                    // One side is empty: an exact insertion or removal, read from neither.
                    self.at = end;
                    self.next_anchor = next;
                    let hunk = self.block(
                        TextOffset(start.left)..TextOffset(end.left),
                        TextOffset(start.right)..TextOffset(end.right),
                        false,
                        (Some(start.left_line), Some(start.right_line)),
                        start.hash.rotate_left(17) ^ next_hash,
                    );
                    return PagedComparePoll::CoarseBlock(Box::new(hunk));
                }
                self.left.cursor = start.left;
                self.right.cursor = start.right;
                // Ignoring blank lines, the resident pass sees a one-sided gap only
                // through its kept lines, so that side is read to find them.
                let kept = one_sided.then(|| {
                    if start.left == end.left {
                        KeptLines::new(start.right, start.right_line)
                    } else {
                        KeptLines::new(start.left, start.left_line)
                    }
                });
                self.gap = Some(Gap {
                    start,
                    end,
                    next,
                    next_hash,
                    extent: None,
                    kept,
                });
                return self.poll_gap();
            }
        };
        self.window = Some((end, anchor, next));
        let start = self.at;
        for (side, reader, from, limit) in [
            (Side::Left, &mut self.left, start.left, end.left),
            (Side::Right, &mut self.right, start.right, end.right),
        ] {
            if self.cancel.is_cancelled() {
                self.terminal = Some(CompareCompleteness::Cancelled);
                return PagedComparePoll::Finished(CompareCompleteness::Cancelled);
            }
            if reader.request.is_none() && reader.ready.is_none() {
                reader.cursor = from;
            }
            match reader.poll(limit, self.cap, &self.budget) {
                Ok(Some(ticket)) => return PagedComparePoll::Pending { side, ticket },
                Ok(None) => {}
                Err(state) => {
                    self.terminal = Some(state);
                    return PagedComparePoll::Finished(state);
                }
            }
        }
        if self.cancel.is_cancelled() {
            return self.finish(CompareCompleteness::Cancelled);
        }
        let refinement_start = Instant::now();
        let left = self.left.ready.take().expect("ready left");
        let right = self.right.ready.take().expect("ready right");
        let lr = left.range();
        let rr = right.range();
        if lr != (TextOffset(start.left)..TextOffset(end.left))
            || rr != (TextOffset(start.right)..TextOffset(end.right))
        {
            return self.finish(CompareCompleteness::Failed);
        }
        self.read_bytes += left.text().len() + right.text().len();
        self.window = None;
        self.at = end;
        self.next_anchor = next;
        self.left.cursor = end.left;
        self.right.cursor = end.right;
        if anchor {
            // Hashes chose the split; normalized text confirms it. A collision still
            // yields a valid diff, only possibly not the minimal one.
            let (l, r) = (last_line(left.text()), last_line(right.text()));
            let first = |range: &Range<TextOffset>, line: &str| range.end.0 == line.len();
            if normalize(l, &self.options, first(&lr, l)) != normalize(r, &self.options, first(&rr, r)) {
                self.degrade(CoarseReason::Windowed);
            }
        }
        if left.text() == right.text() {
            return PagedComparePoll::Progress;
        }
        let budget = Budget::new(self.options.limits.max_memory_bytes / 4);
        let (Ok(ld), Ok(rd)) = (
            Document::from_utf8(left.text(), budget.clone(), Budget::new(0)),
            Document::from_utf8(right.text(), budget, Budget::new(0)),
        ) else {
            return self.finish(CompareCompleteness::Failed);
        };
        let mut local = self.options.clone();
        local.limits.max_memory_bytes /= 2;
        local.limits.time_budget_ms = self
            .options
            .limits
            .time_budget_ms
            .saturating_sub(self.refinement_time.as_millis().min(u128::from(u64::MAX)) as u64);
        let at_origin = lr.start.0 == 0 && rr.start.0 == 0;
        local.ignore_encoding_bom = self.options.ignore_encoding_bom && at_origin;
        let mut result = compare(&ld.snapshot(), &rd.snapshot(), &local, &self.cancel);
        self.refinement_time += refinement_start.elapsed();
        match result.completeness {
            CompareCompleteness::Exact => {}
            CompareCompleteness::Coarse(reason) => self.degrade(reason),
            state => return self.finish(state),
        }
        for h in &mut result.hunks {
            // Only a hunk before the window's first paired line lacked its context
            // line; the anchor that opened this window is that line.
            if h.left.start.0 == 0 && h.right.start.0 == 0 {
                h.stable_id = HunkId(h.stable_id.0 ^ start.hash.rotate_left(17));
            }
            h.left = TextOffset(h.left.start.0 + lr.start.0)..TextOffset(h.left.end.0 + lr.start.0);
            h.right = TextOffset(h.right.start.0 + rr.start.0)..TextOffset(h.right.end.0 + rr.start.0);
            h.left_revision = self.left.snapshot.revision;
            h.right_revision = self.right.snapshot.revision;
            h.left_state = self.left.snapshot.content_state;
            h.right_state = self.right.snapshot.content_state;
            h.options = self.options.clone();
            h.options.ignore_encoding_bom = self.options.ignore_encoding_bom && at_origin;
            h.left_line_hint = h.left_line_hint.map(|n| n + start.left_line);
            h.right_line_hint = h.right_line_hint.map(|n| n + start.right_line);
            for span in &mut h.intraline {
                span.left = TextOffset(span.left.start.0 + lr.start.0)..TextOffset(span.left.end.0 + lr.start.0);
                span.right = TextOffset(span.right.start.0 + rr.start.0)..TextOffset(span.right.end.0 + rr.start.0);
            }
        }
        if result.hunks.is_empty() {
            return PagedComparePoll::Progress;
        }
        self.lease.store(true, Ordering::Release);
        PagedComparePoll::Batch(Box::new(PagedBatch {
            hunks: result.hunks,
            completeness: result.completeness,
            left,
            right,
            left_snapshot: self.left.snapshot.clone(),
            right_snapshot: self.right.snapshot.clone(),
            lease: self.lease.clone(),
        }))
    }
    /// A two-sided gap no window holds: compare it in lockstep byte windows and
    /// report only its changed extent, as one coarse block.
    fn poll_gap(&mut self) -> PagedComparePoll {
        let (start, end) = match &self.gap {
            Some(gap) => (gap.start, gap.end),
            None => return PagedComparePoll::Progress,
        };
        for (side, reader, limit) in [
            (Side::Left, &mut self.left, end.left),
            (Side::Right, &mut self.right, end.right),
        ] {
            if self.cancel.is_cancelled() {
                self.terminal = Some(CompareCompleteness::Cancelled);
                return PagedComparePoll::Finished(CompareCompleteness::Cancelled);
            }
            if reader.cursor >= limit {
                continue;
            }
            match reader.poll(limit, self.cap, &self.budget) {
                Ok(Some(ticket)) => return PagedComparePoll::Pending { side, ticket },
                Ok(None) => {}
                Err(state) => {
                    self.terminal = Some(state);
                    return PagedComparePoll::Finished(state);
                }
            }
        }
        if self.cancel.is_cancelled() {
            return self.finish(CompareCompleteness::Cancelled);
        }
        let left = self.left.ready.take();
        let right = self.right.ready.take();
        if left.is_none() && right.is_none() {
            let Some(gap) = self.gap.take() else {
                return PagedComparePoll::Progress;
            };
            self.at = gap.end;
            self.next_anchor = gap.next;
            let id = start.hash.rotate_left(17) ^ gap.next_hash;
            if let Some(mut kept) = gap.kept {
                let added = start.left == end.left;
                kept.finish(if added { end.right } else { end.left });
                // Only blank lines: the resident result has no hunk here.
                let Some((first, line)) = kept.first else {
                    return PagedComparePoll::Progress;
                };
                let lines = TextOffset(first)..TextOffset(kept.last_end);
                let (l, r, hints) = if added {
                    (
                        TextOffset(end.left)..TextOffset(end.left),
                        lines,
                        (Some(start.left_line), Some(line)),
                    )
                } else {
                    (
                        lines,
                        TextOffset(end.right)..TextOffset(end.right),
                        (Some(line), Some(start.right_line)),
                    )
                };
                return PagedComparePoll::CoarseBlock(Box::new(self.block(l, r, false, hints, id)));
            }
            let Some((l, r)) = gap.extent else {
                return PagedComparePoll::Progress;
            };
            self.degrade(CoarseReason::Bytes);
            if self.index.shift > 0 || self.index.spill_failed {
                self.sampled_gaps += 1;
            }
            let hints = (
                (l.start.0 == start.left).then_some(start.left_line),
                (r.start.0 == start.right).then_some(start.right_line),
            );
            return PagedComparePoll::CoarseBlock(Box::new(self.block(l, r, true, hints, id)));
        }
        let lr = left
            .as_ref()
            .map_or(TextOffset(end.left)..TextOffset(end.left), TextWindow::range);
        let rr = right
            .as_ref()
            .map_or(TextOffset(end.right)..TextOffset(end.right), TextWindow::range);
        let lt = left.as_ref().map_or("", TextWindow::text);
        let rt = right.as_ref().map_or("", TextWindow::text);
        self.read_bytes += lt.len() + rt.len();
        if let Some(gap) = &mut self.gap {
            match &mut gap.kept {
                // The empty side never delivers text.
                Some(kept) => {
                    kept.feed(lt, lr.start.0);
                    kept.feed(rt, rr.start.0);
                }
                None if lt != rt => widen(&mut gap.extent, lr.clone(), rr.clone()),
                None => {}
            }
        }
        self.left.cursor = lr.end.0;
        self.right.cursor = rr.end.0;
        PagedComparePoll::Progress
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::source::{Generation, MemorySource, SourceKind};
    fn source(text: &str) -> PagedSnapshot {
        let (s, p) = MemorySource::new(
            text.len() as u64,
            Generation(1),
            SourceKind::Paged,
            4096,
            4096,
            Budget::new(8192),
        )
        .unwrap();
        p.publish(
            PageTicket {
                generation: Generation(1),
                page: 0,
            },
            text.as_bytes(),
            Generation(1),
        )
        .unwrap();
        PagedSnapshot::utf8(s, 0).unwrap()
    }
    #[test]
    fn pending_multi_gb_cancels_without_materialization() {
        let (s, _) = MemorySource::new(
            4 * 1024 * 1024 * 1024,
            Generation(1),
            SourceKind::Paged,
            4096,
            4096,
            Budget::new(4096),
        )
        .unwrap();
        let snapshot = PagedSnapshot::utf8(s, 0).unwrap();
        let cancel = CancelToken::default();
        let mut job = PagedCompareJob::new(snapshot.clone(), snapshot, CompareOptions::default(), cancel.clone());
        assert!(matches!(job.poll(), PagedComparePoll::Pending { side: Side::Left, .. }));
        assert!(job.budget.used() <= 64 * 1024);
        cancel.cancel();
        // Acknowledged by the very next poll: bounded work, not a wall-clock budget (QA-07).
        assert!(matches!(
            job.poll(),
            PagedComparePoll::Finished(CompareCompleteness::Cancelled)
        ));
    }
    #[test]
    fn unavailable_is_not_coarse() {
        let (s, p) = MemorySource::new(10, Generation(1), SourceKind::Paged, 4096, 4096, Budget::new(4096)).unwrap();
        p.mark_changed();
        let snapshot = PagedSnapshot::utf8(s, 0).unwrap();
        let mut job = PagedCompareJob::new(
            snapshot.clone(),
            snapshot,
            CompareOptions::default(),
            CancelToken::default(),
        );
        assert!(matches!(
            job.poll(),
            PagedComparePoll::Finished(CompareCompleteness::Unavailable)
        ));
    }
    #[test]
    fn owned_batch_backpressure_and_undoable_paged_copy() {
        let left = source("a\n");
        let right = source("b\n");
        let mut target =
            bareline_document::paged::PagedDocument::new(left.clone(), Budget::new(10000), Budget::new(10000));
        let mut job = PagedCompareJob::new(
            left.clone(),
            right.clone(),
            CompareOptions::default(),
            CancelToken::default(),
        );
        // The indexing pass reports progress before the aligned window's batch.
        let batch = loop {
            match job.poll() {
                PagedComparePoll::Progress => {}
                PagedComparePoll::Batch(batch) => break batch,
                _ => panic!("ready batch"),
            }
        };
        assert!(matches!(job.poll(), PagedComparePoll::Backpressure));
        let transaction = batch
            .apply(
                0,
                Direction::RightToLeft,
                &left,
                &right,
                1000,
                MergePolicy::PreserveIgnoredDestination,
            )
            .unwrap();
        target
            .apply_materialized(transaction, std::slice::from_ref(batch.windows().0))
            .unwrap();
        let mut read = target
            .snapshot()
            .begin_read(TextOffset(0)..TextOffset(2), 100, &Budget::new(100))
            .unwrap();
        match read.poll() {
            WindowPoll::Ready(window) => assert_eq!(window.text(), "b\n"),
            _ => panic!("edited ready"),
        };
        target.undo().unwrap();
        drop(batch);
        assert!(matches!(
            job.poll(),
            PagedComparePoll::Finished(CompareCompleteness::Exact)
        ));
    }
    #[test]
    fn utf8_window_boundary_and_coarse_generated_prefix() {
        let text = format!("{}{}{}", "a".repeat(127), "\u{1f642}", "b".repeat(128));
        let mut options = CompareOptions::default();
        options.limits.max_memory_bytes = 8192;
        let snapshot = source(&text);
        let mut job = PagedCompareJob::new(snapshot.clone(), snapshot, options, CancelToken::default());
        let mut windows = 0;
        loop {
            match job.poll() {
                PagedComparePoll::Progress => {
                    // Every indexing window ends on a scalar boundary.
                    assert!(text.is_char_boundary(job.left.cursor));
                    windows += 1;
                }
                PagedComparePoll::Finished(state) => {
                    assert_eq!(state, CompareCompleteness::Exact);
                    break;
                }
                _ => panic!("ready UTF-8 windows"),
            }
        }
        assert!(windows >= 3);
        assert_eq!(job.read_bytes, 2 * text.len());
        let budget = Budget::new(8192);
        let (left, lp) = MemorySource::new(
            4 * 1024 * 1024 * 1024,
            Generation(1),
            SourceKind::Paged,
            4096,
            4096,
            budget.clone(),
        )
        .unwrap();
        let (right, rp) = MemorySource::new(
            4 * 1024 * 1024 * 1024,
            Generation(2),
            SourceKind::Paged,
            4096,
            4096,
            budget,
        )
        .unwrap();
        let mut options = CompareOptions::default();
        options.limits.max_memory_bytes = 16384;
        options.limits.max_bytes_exact = 1;
        let cancel = CancelToken::default();
        let mut job = PagedCompareJob::new(
            PagedSnapshot::utf8(left, 0).unwrap(),
            PagedSnapshot::utf8(right, 0).unwrap(),
            options,
            cancel.clone(),
        );
        loop {
            match job.poll() {
                PagedComparePoll::Pending { side, ticket } => {
                    let (publisher, byte, generation) = match side {
                        Side::Left => (&lp, b'a', Generation(1)),
                        Side::Right => (&rp, b'b', Generation(2)),
                    };
                    publisher.publish(ticket, &vec![byte; 4096], generation).unwrap();
                }
                PagedComparePoll::Progress => {
                    assert!(job.left.cursor <= 256);
                    assert!(job.budget.used() <= 2048);
                    break;
                }
                _ => panic!("bounded coarse prefix"),
            }
        }
        cancel.cancel();
        assert!(matches!(
            job.poll(),
            PagedComparePoll::Finished(CompareCompleteness::Cancelled)
        ));
    }

    // Generated page contents exercise every byte without storing a giant fixture.
    fn divergent_scan(length: usize, equal: bool) {
        let source_budget = Budget::new(256 * 1024);
        let (left, lp) = MemorySource::new(
            length as u64,
            Generation(1),
            SourceKind::Paged,
            64 * 1024,
            64 * 1024,
            source_budget.clone(),
        )
        .unwrap();
        let (right, rp) = MemorySource::new(
            length as u64,
            Generation(2),
            SourceKind::Paged,
            64 * 1024,
            64 * 1024,
            source_budget.clone(),
        )
        .unwrap();
        let mut options = CompareOptions::default();
        options.limits.max_memory_bytes = 4 * 1024 * 1024;
        let mut job = PagedCompareJob::new(
            PagedSnapshot::utf8(left, 0).unwrap(),
            PagedSnapshot::utf8(right, 0).unwrap(),
            options,
            CancelToken::default(),
        );
        let a = vec![b'a'; 64 * 1024];
        let b = vec![if equal { b'a' } else { b'b' }; 64 * 1024];
        let (mut blocks, mut read_bytes) = (0, 0);
        let started = Instant::now();
        loop {
            match job.poll() {
                PagedComparePoll::Pending { side, ticket } => {
                    let (publisher, bytes, generation) = match side {
                        Side::Left => (&lp, &a, Generation(1)),
                        Side::Right => (&rp, &b, Generation(2)),
                    };
                    let count = (length - ticket.page as usize * bytes.len()).min(bytes.len());
                    publisher.publish(ticket, &bytes[..count], generation).unwrap();
                    read_bytes += count;
                }
                PagedComparePoll::Progress => {
                    assert!(job.budget.used() <= 512 * 1024);
                    assert!(source_budget.used() <= 256 * 1024);
                }
                PagedComparePoll::CoarseBlock(hunk) => {
                    assert_eq!(hunk.kind, DiffKind::Changed);
                    assert_eq!(hunk.left, TextOffset(0)..TextOffset(length));
                    assert_eq!(hunk.right, hunk.left);
                    blocks += 1;
                }
                PagedComparePoll::Finished(state) => {
                    assert_eq!(
                        state,
                        if equal {
                            CompareCompleteness::Exact
                        } else {
                            CompareCompleteness::Coarse(CoarseReason::Bytes)
                        }
                    );
                    break;
                }
                _ => panic!("global fallback must not emit local hunks"),
            }
        }
        assert_eq!(read_bytes, length * 2);
        assert_eq!(blocks, usize::from(!equal));
        eprintln!(
            "validated {read_bytes} bytes in {:?}; source budget 256KiB, window budget 512KiB",
            started.elapsed()
        );
    }

    #[test]
    fn divergent_200_mb_is_one_bounded_block() {
        divergent_scan(200 * 1024 * 1024, false);
        divergent_scan(2 * 1024 * 1024, true);
    }

    #[test]
    fn global_fallback_retains_verified_equal_prefix_and_suffix() {
        let mut options = CompareOptions::default();
        options.limits.max_memory_bytes = 8192;
        options.limits.max_bytes_exact = 1;
        let left = "a".repeat(384);
        let right = format!("{}{}{}", "a".repeat(128), "b".repeat(128), "a".repeat(128));
        let mut job = PagedCompareJob::new(source(&left), source(&right), options, CancelToken::default());
        let mut blocks = 0;
        loop {
            match job.poll() {
                PagedComparePoll::Progress => {}
                PagedComparePoll::CoarseBlock(hunk) => {
                    assert_eq!(hunk.left, TextOffset(128)..TextOffset(256));
                    assert_eq!(hunk.right, hunk.left);
                    assert_eq!(hunk.left_line_hint, None);
                    blocks += 1;
                }
                PagedComparePoll::Finished(CompareCompleteness::Coarse(CoarseReason::Bytes)) => {
                    break;
                }
                _ => panic!("bounded source ready"),
            }
        }
        assert_eq!(blocks, 1);
    }

    /// A multi-page source with every page already published.
    fn paged(text: &str) -> PagedSnapshot {
        const PAGE: usize = 4096;
        let (s, p) = MemorySource::new(
            text.len() as u64,
            Generation(1),
            SourceKind::Paged,
            PAGE,
            text.len().max(PAGE),
            Budget::new(text.len() + PAGE),
        )
        .unwrap();
        for (page, bytes) in text.as_bytes().chunks(PAGE).enumerate() {
            p.publish(
                PageTicket {
                    generation: Generation(1),
                    page: page as u64,
                },
                bytes,
                Generation(1),
            )
            .unwrap();
        }
        PagedSnapshot::utf8(s, 0).unwrap()
    }
    /// Every hunk the job reports, the number of batches, and its final state.
    fn run(job: &mut PagedCompareJob) -> (Vec<DiffHunk>, usize, CompareCompleteness) {
        let (mut hunks, mut batches) = (Vec::new(), 0);
        loop {
            match job.poll() {
                PagedComparePoll::Progress => {}
                PagedComparePoll::Batch(batch) => {
                    batches += 1;
                    hunks.extend(batch.hunks.iter().cloned());
                }
                PagedComparePoll::CoarseBlock(hunk) => hunks.push(*hunk),
                PagedComparePoll::Finished(state) => return (hunks, batches, state),
                _ => panic!("published sources never pend"),
            }
        }
    }
    /// The resident diff of the same texts: the semantics paged compare must match.
    fn resident(left: &str, right: &str, options: &CompareOptions) -> CompareResult {
        let document = |text: &str| Document::from_utf8(text, Budget::new(64 << 20), Budget::new(0)).unwrap();
        compare(
            &document(left).snapshot(),
            &document(right).snapshot(),
            options,
            &CancelToken::default(),
        )
    }
    fn assert_same(paged: &[DiffHunk], resident: &[DiffHunk]) {
        assert_eq!(paged.len(), resident.len());
        let spans = |h: &DiffHunk| {
            h.intraline
                .iter()
                .map(|span| (span.left.clone(), span.right.clone()))
                .collect::<Vec<_>>()
        };
        for (p, r) in paged.iter().zip(resident) {
            assert_eq!(
                (&p.left, &p.right, p.kind, p.coarse),
                (&r.left, &r.right, r.kind, r.coarse)
            );
            assert_eq!(
                (p.left_line_hint, p.right_line_hint),
                (r.left_line_hint, r.right_line_hint)
            );
            assert_eq!(spans(p), spans(r));
        }
    }
    fn realigns_like_resident(eol: &str, options: CompareOptions) {
        let lines: Vec<String> = (0..20_000).map(|i| format!("Line {i:05} payload{eol}")).collect();
        let left = lines.concat();
        let mut edited = lines.clone();
        edited[19_000] = format!("line 19000 EDITED{eol}");
        edited.remove(10_000);
        edited.insert(100, format!("inserted line{eol}"));
        let right = edited.concat();
        let mut job = PagedCompareJob::new(paged(&left), paged(&right), options.clone(), CancelToken::default());
        let (hunks, batches, state) = run(&mut job);
        assert_eq!(state, CompareCompleteness::Exact);
        let oracle = resident(&left, &right, &options);
        assert_eq!(oracle.completeness, CompareCompleteness::Exact);
        assert_eq!(
            hunks.iter().map(|h| h.kind).collect::<Vec<_>>(),
            [DiffKind::Added, DiffKind::Removed, DiffKind::Changed]
        );
        assert_same(&hunks, &oracle.hunks);
        // Several aligned windows; only the three that differ produce batches.
        assert!(left.len() > 4 * job.cap);
        assert_eq!(batches, 3);
        // One indexing pass plus one aligned pass, never more.
        assert!(job.read_bytes <= 2 * (left.len() + right.len()));
    }
    #[test]
    fn inserted_line_realigns_on_anchors_like_resident_compare() {
        // SRC-05: byte-offset windows used to mark everything after one inserted
        // line as changed; anchored windows report exactly the three edits.
        realigns_like_resident("\n", CompareOptions::default());
        let options = CompareOptions {
            ignore_case: true,
            whitespace: Whitespace::TrimEdges,
            ..CompareOptions::default()
        };
        // Normalizing options apply to anchors exactly as to resident lines.
        realigns_like_resident("\r\n", options);
    }
    #[test]
    fn line_index_joins_a_crlf_split_across_windows() {
        let options = CompareOptions::default();
        let text = "one\r\ntwo\r\nthree";
        let entries = |index: &AnchorIndex| {
            let mut entries: Vec<_> = index
                .lines
                .values()
                .map(|[seen, _]| (seen.start, seen.end, seen.line))
                .collect();
            entries.sort_unstable();
            entries
        };
        let index = || AnchorIndex::new(64, None);
        let (mut whole, mut split) = (index(), index());
        let mut lines = Lines::default();
        lines.feed(text, 0, 0, &mut whole, &options);
        lines.finish(text.len(), 0, &mut whole, &options);
        let mut windows = Lines::default();
        for (at, piece) in [(0, "one\r"), (4, "\ntwo\r\nthr"), (13, "ee")] {
            windows.feed(piece, at, 0, &mut split, &options);
        }
        windows.finish(text.len(), 0, &mut split, &options);
        assert_eq!(entries(&whole), [(0, 5, 0), (5, 10, 1), (10, 15, 2)]);
        assert_eq!(entries(&split), entries(&whole));
        assert_eq!(
            whole.lines.keys().collect::<BTreeSet<_>>(),
            split.lines.keys().collect::<BTreeSet<_>>()
        );
        // The resident end-of-input hint: an unterminated last line is not followed by another.
        assert_eq!((windows.line, windows.eof_line), (3, 2));
    }
    #[test]
    fn anchor_index_without_a_spill_store_stays_bounded_by_content_sampling() {
        // SRC-05: without a spill store the index never exceeds its entry cap;
        // sampling by hash keeps the same lines on both sides, so alignment
        // still matches resident.
        let lines: Vec<String> = (0..20_000).map(|i| format!("row {i:05}\n")).collect();
        let left = lines.concat();
        let mut edited = lines.clone();
        edited.insert(15_000, "added row\n".into());
        edited.remove(5_000);
        let right = edited.concat();
        let options = CompareOptions::default();
        let mut job = PagedCompareJob::new(paged(&left), paged(&right), options.clone(), CancelToken::default())
            .with_spill_dir(None);
        job.index.cap = 512;
        let (mut hunks, mut peak) = (Vec::new(), 0);
        let state = loop {
            let poll = job.poll();
            peak = peak.max(job.index.lines.len());
            match poll {
                PagedComparePoll::Progress => {}
                PagedComparePoll::Batch(batch) => hunks.extend(batch.hunks.iter().cloned()),
                PagedComparePoll::CoarseBlock(hunk) => hunks.push(*hunk),
                PagedComparePoll::Finished(state) => break state,
                _ => panic!("published sources never pend"),
            }
        };
        assert!(peak <= 512, "{peak}");
        assert!(job.index.shift >= 5, "{}", job.index.shift);
        assert_eq!(state, CompareCompleteness::Exact);
        assert_same(&hunks, &resident(&left, &right, &options).hunks);
        assert_eq!(hunks.len(), 2);
    }
    #[test]
    fn spilled_anchor_index_keeps_every_anchor_in_bounded_memory() {
        // SRC-05: an index far larger than its table spills sorted runs instead
        // of sampling. It keeps every anchor the in-memory index finds, and the
        // result is the resident diff's.
        let lines: Vec<String> = (0..20_000).map(|i| format!("row {i:05}\n")).collect();
        let left = lines.concat();
        let mut edited = lines.clone();
        edited.insert(15_000, "added row\n".into());
        edited.remove(5_000);
        edited[100] = "changed row\n".into();
        let right = edited.concat();
        let options = CompareOptions::default();
        let oracle = resident(&left, &right, &options);
        assert_eq!(oracle.completeness, CompareCompleteness::Exact);
        // The whole index in memory gives the reference chain.
        let mut whole = PagedCompareJob::new(paged(&left), paged(&right), options.clone(), CancelToken::default())
            .with_spill_dir(None);
        let (hunks, _, state) = run(&mut whole);
        assert_eq!(state, CompareCompleteness::Exact);
        assert_same(&hunks, &oracle.hunks);
        let dense = whole.anchors.as_ref().expect("anchors").len;
        assert_eq!(dense, 19_998);
        let parent = std::env::temp_dir().join(format!(
            "bareline-spill-test-{}-{}",
            std::process::id(),
            SPILLS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&parent).unwrap();
        let mut job = PagedCompareJob::new(paged(&left), paged(&right), options.clone(), CancelToken::default())
            .with_spill_dir(Some(parent.clone()));
        // 128 entries per table: about 155 runs, merged into one at every 64th.
        job.index.cap = 128;
        let (mut hunks, mut peak) = (Vec::new(), 0);
        let state = loop {
            let poll = job.poll();
            peak = peak.max(job.index.lines.len());
            match poll {
                PagedComparePoll::Progress => {}
                PagedComparePoll::Batch(batch) => hunks.extend(batch.hunks.iter().cloned()),
                PagedComparePoll::CoarseBlock(hunk) => hunks.push(*hunk),
                PagedComparePoll::Finished(state) => break state,
                _ => panic!("published sources never pend"),
            }
        };
        assert_eq!(state, CompareCompleteness::Exact);
        assert!(peak <= 128, "{peak}");
        assert_eq!(job.index.shift, 0, "the spilled index was sampled");
        assert!(!job.index.spill_failed);
        assert_same(&hunks, &oracle.hunks);
        // Every anchor of the in-memory chain, read back from the chain's run.
        let chain = job.anchors.as_ref().expect("anchors");
        assert_eq!(chain.len, dense);
        assert!(chain.store.is_some(), "the chain stayed in memory");
        // Table runs, candidate runs and the chain were all written.
        assert!(job.spilled_records() > 2 * dense, "{}", job.spilled_records());
        // Spilling adds no page reads: one indexing and one aligned pass.
        assert!(job.read_bytes <= 2 * (left.len() + right.len()));
        drop(job);
        assert_eq!(
            std::fs::read_dir(&parent).unwrap().count(),
            0,
            "spill files outlived the job"
        );
        std::fs::remove_dir(&parent).unwrap();
    }
    #[test]
    fn windowed_chain_matches_the_global_patience_chain() {
        // SRC-05: the spilled chain is chosen a window at a time; a line moved by
        // less than half a window is left out exactly as the global chain does.
        let anchor = |i: usize, right: usize| Anchor {
            hash: i as u64,
            left: Seen {
                count: 1,
                start: i * 10,
                end: i * 10 + 5,
                line: i,
            },
            right: Seen {
                count: 1,
                start: right * 10,
                end: right * 10 + 5,
                line: right,
            },
        };
        let rights = [0, 1, 2, 100, 3, 4, 5, 6, 7, 8];
        let candidates: Vec<Anchor> = rights.iter().enumerate().map(|(i, &r)| anchor(i, r)).collect();
        let global = patience(&candidates, None);
        assert_eq!(global, [0, 1, 2, 4, 5, 6, 7, 8, 9]);
        // A window the candidates fill exactly is chosen in one round.
        for window in [2, 4, rights.len(), 64] {
            let mut source = candidates.iter().copied();
            let mut chosen = Vec::new();
            choose(
                || Ok(source.next()),
                window,
                || false,
                |anchor| {
                    chosen.push(anchor.left.line);
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(chosen, global, "window {window}");
        }
    }
    #[test]
    fn a_cancel_stops_the_spilled_anchor_merge() {
        // SRC-05: the merge of spilled runs checks the cancel token instead of
        // finishing first; the index removes its spill files when it drops.
        let parent = std::env::temp_dir().join(format!(
            "bareline-spill-cancel-{}-{}",
            std::process::id(),
            SPILLS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&parent).unwrap();
        let mut index = AnchorIndex::new(4, Some(parent.clone()));
        for line in 0..64 {
            let seen = Seen {
                count: 1,
                start: line * 4,
                end: line * 4 + 4,
                line,
            };
            index.record(0, line as u64 * 7919, seen);
            index.record(1, line as u64 * 7919, seen);
        }
        assert!(!index.runs.is_empty(), "the index did not spill");
        let cancel = CancelToken::default();
        cancel.cancel();
        let error = index.anchors(&cancel).err().expect("the cancelled merge finished");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        drop(index);
        assert_eq!(
            std::fs::read_dir(&parent).unwrap().count(),
            0,
            "spill files outlived the index"
        );
        std::fs::remove_dir(&parent).unwrap();
    }
    #[test]
    fn gaps_larger_than_a_window_stay_local() {
        let lines: Vec<String> = (0..12_000).map(|i| format!("line {i:05} payload\n")).collect();
        let line = lines[0].len();
        let left = lines.concat();
        let options = CompareOptions::default();
        // An insertion no window can hold is still one exact hunk, read from neither side.
        let block: String = (0..5_000).map(|i| format!("new {i:05} block\n")).collect();
        let mut inserted = lines.clone();
        inserted.insert(6_000, block.clone());
        let right = inserted.concat();
        assert!(block.len() > 64 * 1024);
        let mut job = PagedCompareJob::new(paged(&left), paged(&right), options.clone(), CancelToken::default());
        let (hunks, _, state) = run(&mut job);
        assert_eq!(state, CompareCompleteness::Exact);
        assert_same(&hunks, &resident(&left, &right, &options).hunks);
        assert_eq!(hunks.len(), 1);
        assert!(!hunks[0].coarse && hunks[0].kind == DiffKind::Added);
        assert_eq!(job.read_bytes, 2 * (left.len() + right.len()) - block.len());
        // A two-sided change no window can hold is one coarse block bounded by its gap.
        let replaced: String = (0..5_000).map(|i| format!("alt {i:05} block\n")).collect();
        let mut changed = lines.clone();
        changed.drain(3_000..8_000);
        changed.insert(3_000, replaced.clone());
        let right = changed.concat();
        let mut job = PagedCompareJob::new(paged(&left), paged(&right), options.clone(), CancelToken::default());
        let (hunks, _, state) = run(&mut job);
        assert_eq!(state, CompareCompleteness::Coarse(CoarseReason::Bytes));
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].coarse && hunks[0].kind == DiffKind::Changed);
        assert_eq!(hunks[0].left, TextOffset(3_000 * line)..TextOffset(8_000 * line));
        assert_eq!(
            hunks[0].right,
            TextOffset(3_000 * line)..TextOffset(3_000 * line + replaced.len())
        );
        assert_eq!(job.sampled_gaps(), 0);
        // A sampled index reports its coarse gaps as reduced precision (SRC-05).
        let mut job =
            PagedCompareJob::new(paged(&left), paged(&right), options, CancelToken::default()).with_spill_dir(None);
        job.index.cap = 4_000;
        let (hunks, _, state) = run(&mut job);
        assert_eq!(state, CompareCompleteness::Coarse(CoarseReason::Bytes));
        assert!(job.index.shift > 0);
        assert!(job.sampled_gaps() >= 1);
        assert_eq!(job.sampled_gaps(), hunks.iter().filter(|hunk| hunk.coarse).count());
    }
    #[test]
    fn a_region_that_fits_without_its_anchor_line_stays_exact() {
        // SRC-05: a two-sided change that fits a window only without the anchor
        // line after it is diffed exactly, not reported as a coarse gap.
        let changed = |side: &str| -> String {
            (0..500)
                .map(|i| {
                    format!(
                        "{side} {i:04} {}
",
                        "x".repeat(93 - side.len())
                    )
                })
                .collect()
        };
        let (removed, added) = (changed("left"), changed("right"));
        let anchor = format!(
            "{}
",
            "B".repeat(20_000)
        );
        let tail: String = (0..100)
            .map(|i| {
                format!(
                    "tail {i:03}
"
                )
            })
            .collect();
        let left = format!("{removed}{anchor}{tail}");
        let right = format!("{added}{anchor}{tail}");
        let options = CompareOptions::default();
        let mut job = PagedCompareJob::new(paged(&left), paged(&right), options.clone(), CancelToken::default());
        assert!(removed.len() <= job.cap && removed.len() + anchor.len() > job.cap);
        let (hunks, _, state) = run(&mut job);
        assert_eq!(state, CompareCompleteness::Exact);
        assert_eq!(hunks.len(), 1);
        assert!(!hunks[0].coarse && hunks[0].kind == DiffKind::Changed);
        assert_eq!(hunks[0].left, TextOffset(0)..TextOffset(removed.len()));
        assert_eq!(hunks[0].right, TextOffset(0)..TextOffset(added.len()));
        let oracle = resident(&left, &right, &options);
        assert_eq!(oracle.completeness, CompareCompleteness::Exact);
        assert_eq!(
            oracle
                .hunks
                .iter()
                .map(|h| (&h.left, &h.right, h.kind, h.coarse))
                .collect::<Vec<_>>(),
            hunks
                .iter()
                .map(|h| (&h.left, &h.right, h.kind, h.coarse))
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn one_sided_gaps_ignore_blank_lines_like_resident_compare() {
        // An oversized one-sided gap is read for its kept lines when blank lines
        // are ignored: only blanks report nothing, and blank edges are trimmed.
        let lines: Vec<String> = (0..12_000).map(|i| format!("line {i:05} payload\n")).collect();
        let left = lines.concat();
        let options = CompareOptions {
            ignore_blank_lines: true,
            ..CompareOptions::default()
        };
        let blanks = " \t\n".repeat(25_000);
        assert!(blanks.len() > 64 * 1024);
        let mut inserted = lines.clone();
        inserted.insert(6_000, blanks.clone());
        let right = inserted.concat();
        let mut job = PagedCompareJob::new(paged(&left), paged(&right), options.clone(), CancelToken::default());
        let (hunks, _, state) = run(&mut job);
        assert_eq!(state, CompareCompleteness::Exact);
        assert!(hunks.is_empty());
        assert!(resident(&left, &right, &options).hunks.is_empty());
        assert_eq!(job.read_bytes, 2 * (left.len() + right.len()));
        // Kept lines between blank runs, on either side, with CRLF and a lone CR.
        let block = format!(
            "{blanks}new 00000 block\r\n \r\nnew 00001 block\rnew 00002 block\n{}",
            "\r\n".repeat(40_000)
        );
        let mut inserted = lines.clone();
        inserted.insert(6_000, block);
        let right = inserted.concat();
        for (left, right, kind) in [
            (left.as_str(), right.as_str(), DiffKind::Added),
            (right.as_str(), left.as_str(), DiffKind::Removed),
        ] {
            let mut job = PagedCompareJob::new(paged(left), paged(right), options.clone(), CancelToken::default());
            let (hunks, _, state) = run(&mut job);
            assert_eq!(state, CompareCompleteness::Exact);
            let oracle = resident(left, right, &options);
            assert_eq!(oracle.completeness, CompareCompleteness::Exact);
            assert_same(&hunks, &oracle.hunks);
            assert_eq!(hunks.len(), 1);
            assert!(!hunks[0].coarse && hunks[0].kind == kind);
        }
    }

    #[test]
    #[ignore = "explicit multi-GB throughput evidence; no giant allocation"]
    fn divergent_multi_gb_full_traversal() {
        divergent_scan(2 * 1024 * 1024 * 1024, false);
    }

    /// Cancels an active 4 GB traversal from another thread once it is under way. The
    /// very next poll acknowledges the request: bounded work, not a wall-clock budget
    /// that fails under load (QA-07). Returns the time from request to acknowledgement.
    fn cancel_multi_gb_traversal() -> std::time::Duration {
        let budget = Budget::new(256 * 1024);
        let (left, lp) = MemorySource::new(
            4 * 1024 * 1024 * 1024,
            Generation(1),
            SourceKind::Paged,
            65536,
            65536,
            budget.clone(),
        )
        .unwrap();
        let (right, rp) = MemorySource::new(
            4 * 1024 * 1024 * 1024,
            Generation(2),
            SourceKind::Paged,
            65536,
            65536,
            budget,
        )
        .unwrap();
        let cancel = CancelToken::default();
        let mut job = PagedCompareJob::new(
            PagedSnapshot::utf8(left, 0).unwrap(),
            PagedSnapshot::utf8(right, 0).unwrap(),
            CompareOptions::default(),
            cancel.clone(),
        );
        let (start, ready) = std::sync::mpsc::channel();
        let mut canceller = Some(std::thread::spawn(move || {
            ready.recv().unwrap();
            let requested = Instant::now();
            cancel.cancel();
            requested
        }));
        let a = vec![b'a'; 65536];
        let b = vec![b'b'; 65536];
        loop {
            match job.poll() {
                PagedComparePoll::Pending { side, ticket } => {
                    let (publisher, bytes, generation) = match side {
                        Side::Left => (&lp, &a, Generation(1)),
                        Side::Right => (&rp, &b, Generation(2)),
                    };
                    publisher.publish(ticket, bytes, generation).unwrap();
                }
                PagedComparePoll::Progress => {
                    if let Some(canceller) = canceller.take() {
                        start.send(()).unwrap();
                        let requested = canceller.join().unwrap();
                        assert!(matches!(
                            job.poll(),
                            PagedComparePoll::Finished(CompareCompleteness::Cancelled)
                        ));
                        return requested.elapsed();
                    }
                }
                _ => panic!("active traversal cannot complete before cancellation"),
            }
        }
    }
    #[test]
    fn cancellation_during_multi_gb_traversal_is_acknowledged() {
        cancel_multi_gb_traversal();
    }
    #[test]
    #[ignore = "timing budget (QA-07); run with `cargo test --release -- --ignored`"]
    fn cancellation_during_multi_gb_traversal_is_acknowledged_within_50_ms() {
        let elapsed = cancel_multi_gb_traversal();
        assert!(elapsed < std::time::Duration::from_millis(50), "{elapsed:?}");
        eprintln!("active multi-GB cancellation acknowledged in {elapsed:?}");
    }
}
