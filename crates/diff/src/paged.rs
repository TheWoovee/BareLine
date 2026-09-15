// SPDX-License-Identifier: MPL-2.0
//! Poll-driven bounded paged comparison. The caller resolves page tickets on its I/O pool.
use crate::*;
use bareline_document::{
    Budget, Document,
    paged::{PagedSnapshot, TextWindow, WindowPoll, WindowRequest},
    source::PageTicket,
};

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
    /// Whole-source fallback after every byte was read under the window budget.
    /// Applying this range requires caller-owned bounded materialization/staging.
    CoarseBlock(Box<DiffHunk>),
    Finished(CompareCompleteness),
}
struct Reader {
    snapshot: PagedSnapshot,
    cursor: usize,
    end: usize,
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
            retries: 0,
            request: None,
            ready: None,
        }
    }
    fn poll(&mut self, cap: usize, budget: &Budget) -> Result<Option<PageTicket>, CompareCompleteness> {
        if self.ready.is_some() {
            return Ok(None);
        }
        if self.request.is_none() {
            self.end = (self.cursor + cap).min(self.snapshot.len());
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
                    if self.retries == 3 || self.end <= self.cursor || self.end == self.snapshot.len() {
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
    windows: usize,
    refinement_time: std::time::Duration,
    global_coarse: bool,
    global_equal: bool,
    changed_extent: Option<(Range<TextOffset>, Range<TextOffset>)>,
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
        let global_coarse = left.len().saturating_add(right.len()) > options.limits.max_bytes_exact;
        let global_equal = left.len() == right.len();
        Self {
            left: Reader::new(left),
            right: Reader::new(right),
            options,
            cancel,
            budget,
            cap,
            lease: Arc::new(AtomicBool::new(false)),
            terminal,
            quality: CompareCompleteness::Exact,
            windows: 0,
            refinement_time: std::time::Duration::ZERO,
            global_coarse,
            global_equal,
            changed_extent: None,
        }
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
        if self.left.cursor == self.left.snapshot.len() && self.right.cursor == self.right.snapshot.len() {
            if self.global_coarse {
                self.global_coarse = false;
                if self.global_equal {
                    self.terminal = Some(CompareCompleteness::Exact);
                    return PagedComparePoll::Finished(CompareCompleteness::Exact);
                }
                self.terminal = Some(CompareCompleteness::Coarse(CoarseReason::Bytes));
                // If no window pair actually differed there is nothing to report.
                let Some((left, right)) = self.changed_extent.take() else {
                    self.terminal = Some(CompareCompleteness::Exact);
                    return PagedComparePoll::Finished(CompareCompleteness::Exact);
                };
                let kind = if left.is_empty() {
                    DiffKind::Added
                } else if right.is_empty() {
                    DiffKind::Removed
                } else {
                    DiffKind::Changed
                };
                return PagedComparePoll::CoarseBlock(Box::new(DiffHunk {
                    stable_id: HunkId(0),
                    left_line_hint: (left.start.0 == 0).then_some(0),
                    right_line_hint: (right.start.0 == 0).then_some(0),
                    left,
                    right,
                    kind,
                    intraline: Vec::new(),
                    left_revision: self.left.snapshot.revision,
                    right_revision: self.right.snapshot.revision,
                    left_state: self.left.snapshot.content_state,
                    right_state: self.right.snapshot.content_state,
                    options: self.options.clone(),
                }));
            }
            let state = if self.windows > 1 && self.quality == CompareCompleteness::Exact {
                CompareCompleteness::Coarse(CoarseReason::Windowed)
            } else {
                self.quality
            };
            self.terminal = Some(state);
            return PagedComparePoll::Finished(state);
        }
        for (side, reader) in [(Side::Left, &mut self.left), (Side::Right, &mut self.right)] {
            if self.cancel.is_cancelled() {
                self.terminal = Some(CompareCompleteness::Cancelled);
                return PagedComparePoll::Finished(CompareCompleteness::Cancelled);
            }
            match reader.poll(self.cap, &self.budget) {
                Ok(Some(ticket)) => return PagedComparePoll::Pending { side, ticket },
                Ok(None) => {}
                Err(state) => {
                    self.terminal = Some(state);
                    return PagedComparePoll::Finished(state);
                }
            }
        }
        if self.cancel.is_cancelled() {
            self.terminal = Some(CompareCompleteness::Cancelled);
            return PagedComparePoll::Finished(CompareCompleteness::Cancelled);
        }
        let refinement_start = Instant::now();
        let left = self.left.ready.take().expect("ready left");
        let right = self.right.ready.take().expect("ready right");
        if self.global_coarse {
            self.global_equal &= left.text() == right.text();
            if left.text() != right.text() {
                if let Some((l, r)) = &mut self.changed_extent {
                    l.end = left.range().end;
                    r.end = right.range().end;
                } else {
                    self.changed_extent = Some((left.range(), right.range()));
                }
            }
            self.left.cursor = left.range().end.0;
            self.right.cursor = right.range().end.0;
            self.windows += 1;
            // Return between bounded windows, including when cached pages are immediately
            // available, so the caller controls scheduling and cancellation latency.
            return PagedComparePoll::Progress;
        }
        let budget = Budget::new(self.options.limits.max_memory_bytes / 4);
        let (Ok(ld), Ok(rd)) = (
            Document::from_utf8(left.text(), budget.clone(), Budget::new(0)),
            Document::from_utf8(right.text(), budget, Budget::new(0)),
        ) else {
            self.terminal = Some(CompareCompleteness::Failed);
            return PagedComparePoll::Finished(CompareCompleteness::Failed);
        };
        let mut local = self.options.clone();
        local.limits.max_memory_bytes /= 2;
        local.limits.time_budget_ms = self
            .options
            .limits
            .time_budget_ms
            .saturating_sub(self.refinement_time.as_millis().min(u128::from(u64::MAX)) as u64);
        local.ignore_encoding_bom = self.options.ignore_encoding_bom && self.windows == 0;
        let mut result = compare(&ld.snapshot(), &rd.snapshot(), &local, &self.cancel);
        self.refinement_time += refinement_start.elapsed();
        if !matches!(
            result.completeness,
            CompareCompleteness::Exact | CompareCompleteness::Coarse(_)
        ) {
            self.terminal = Some(result.completeness);
            return PagedComparePoll::Finished(result.completeness);
        }
        if result.completeness != CompareCompleteness::Exact {
            self.quality = result.completeness;
        }
        let lr = left.range();
        let rr = right.range();
        for h in &mut result.hunks {
            h.left = TextOffset(h.left.start.0 + lr.start.0)..TextOffset(h.left.end.0 + lr.start.0);
            h.right = TextOffset(h.right.start.0 + rr.start.0)..TextOffset(h.right.end.0 + rr.start.0);
            h.left_revision = self.left.snapshot.revision;
            h.right_revision = self.right.snapshot.revision;
            h.left_state = self.left.snapshot.content_state;
            h.right_state = self.right.snapshot.content_state;
            h.options = self.options.clone();
            h.options.ignore_encoding_bom = self.options.ignore_encoding_bom && self.windows == 0;
            h.left_line_hint = None;
            h.right_line_hint = None;
            for span in &mut h.intraline {
                span.left = TextOffset(span.left.start.0 + lr.start.0)..TextOffset(span.left.end.0 + lr.start.0);
                span.right = TextOffset(span.right.start.0 + rr.start.0)..TextOffset(span.right.end.0 + rr.start.0);
            }
        }
        self.left.cursor = lr.end.0;
        self.right.cursor = rr.end.0;
        self.windows += 1;
        // Arbitrary page-window edges can split a logical line or CRLF; quality is coarse.
        let quality = if lr.start.0 > 0
            || rr.start.0 > 0
            || lr.end.0 < self.left.snapshot.len()
            || rr.end.0 < self.right.snapshot.len()
        {
            CompareCompleteness::Coarse(CoarseReason::Windowed)
        } else {
            result.completeness
        };
        self.lease.store(true, Ordering::Release);
        PagedComparePoll::Batch(Box::new(PagedBatch {
            hunks: result.hunks,
            completeness: quality,
            left,
            right,
            left_snapshot: self.left.snapshot.clone(),
            right_snapshot: self.right.snapshot.clone(),
            lease: self.lease.clone(),
        }))
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
        let start = Instant::now();
        cancel.cancel();
        assert!(matches!(
            job.poll(),
            PagedComparePoll::Finished(CompareCompleteness::Cancelled)
        ));
        assert!(start.elapsed().as_millis() < 50);
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
        let batch = match job.poll() {
            PagedComparePoll::Batch(batch) => batch,
            _ => panic!("ready batch"),
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
                PagedComparePoll::Batch(batch) => {
                    assert_eq!(batch.windows().0.text(), batch.windows().1.text());
                    windows += 1;
                    drop(batch)
                }
                PagedComparePoll::Finished(state) => {
                    assert!(matches!(state, CompareCompleteness::Coarse(_)));
                    break;
                }
                _ => panic!("ready UTF-8 windows"),
            }
        }
        assert!(windows >= 3);
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

    #[test]
    #[ignore = "explicit multi-GB throughput evidence; no giant allocation"]
    fn divergent_multi_gb_full_traversal() {
        divergent_scan(2 * 1024 * 1024 * 1024, false);
    }

    #[test]
    fn cancellation_during_multi_gb_traversal_is_acknowledged() {
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
        let worker = std::thread::spawn(move || {
            ready.recv().unwrap();
            let requested = Instant::now();
            cancel.cancel();
            requested
        });
        let mut signal = Some(start);
        let a = vec![b'a'; 65536];
        let b = vec![b'b'; 65536];
        let acknowledged = loop {
            match job.poll() {
                PagedComparePoll::Pending { side, ticket } => {
                    let (publisher, bytes, generation) = match side {
                        Side::Left => (&lp, &a, Generation(1)),
                        Side::Right => (&rp, &b, Generation(2)),
                    };
                    publisher.publish(ticket, bytes, generation).unwrap();
                }
                PagedComparePoll::Progress => {
                    if let Some(sender) = signal.take() {
                        sender.send(()).unwrap();
                    }
                }
                PagedComparePoll::Finished(CompareCompleteness::Cancelled) => break Instant::now(),
                _ => panic!("active traversal cannot complete before cancellation"),
            }
        };
        let elapsed = acknowledged.duration_since(worker.join().unwrap());
        assert!(elapsed < std::time::Duration::from_millis(50), "{elapsed:?}");
        eprintln!("active multi-GB cancellation acknowledged in {elapsed:?}");
    }
}
