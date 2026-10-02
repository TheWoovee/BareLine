// SPDX-License-Identifier: MPL-2.0
//! Streaming extended-grapheme navigation. Retained context is at most two
//! 4 KiB chunks, including clusters spanning megabytes of combining characters.
//! A move resolves in place within `SYNC_SCAN_BYTES`; only a longer scan goes
//! to the shared surface pool, never to a thread of its own (EDT-20).
use bareline_document::{DocumentSnapshot, TextOffset};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};
/// Text a move may scan on the UI thread. An ordinary cluster resolves within
/// one 4 KiB chunk and its context; only thousands of combining marks exceed it.
const SYNC_SCAN_BYTES: usize = 16 * 1024;
/// A scan's answer; `None` once its byte budget ran out first.
type Scan = Result<Option<usize>, String>;
pub(crate) struct Navigation {
    pub snapshot: DocumentSnapshot,
    pub origin: usize,
    pub extend: bool,
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Result<usize, String>>,
}
impl Drop for Navigation {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Navigation {
    pub fn start(
        snapshot: DocumentSnapshot,
        origin: usize,
        forward: bool,
        extend: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        Self::start_request(snapshot, origin, origin, Some(forward), extend, notify)
    }
    pub fn start_snap(
        snapshot: DocumentSnapshot,
        origin: usize,
        target: usize,
        extend: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        Self::start_request(snapshot, origin, target, None, extend, notify)
    }
    fn start_request(
        snapshot: DocumentSnapshot,
        origin: usize,
        target: usize,
        forward: Option<bool>,
        extend: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut budget = SYNC_SCAN_BYTES;
        let result = match resolve(&snapshot, target, forward, &cancel, &mut budget) {
            Ok(Some(offset)) => Ok(offset),
            Err(error) => Err(error),
            Ok(None) => {
                let (tx, receiver) = mpsc::sync_channel(1);
                let worker_cancel = cancel.clone();
                let captured = snapshot.clone();
                let worker_notify = notify.clone();
                let job = Box::new(move || {
                    let mut unlimited = usize::MAX;
                    let result = resolve(&captured, target, forward, &worker_cancel, &mut unlimited)
                        .and_then(|found| found.ok_or_else(|| "Grapheme scan stopped".to_owned()));
                    let _ = tx.send(result);
                    worker_notify();
                });
                if crate::surface_pool::submit(job).is_ok() {
                    return Ok(Self {
                        snapshot,
                        origin,
                        extend,
                        cancel,
                        receiver,
                    });
                }
                // Reported on the next pump like any failed scan; a full pool
                // never fails the caller's paint.
                Err("Grapheme worker busy".to_owned())
            }
        };
        // Answered here: the reply waits for the next pump, which `notify`
        // requests exactly as a worker's reply would.
        let (tx, receiver) = mpsc::sync_channel(1);
        let _ = tx.send(result);
        notify();
        Ok(Self {
            snapshot,
            origin,
            extend,
            cancel,
            receiver,
        })
    }
    pub fn poll(&self) -> Option<Result<usize, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("Grapheme worker stopped".into())),
        }
    }
}
fn resolve(
    snapshot: &DocumentSnapshot,
    target: usize,
    forward: Option<bool>,
    cancelled: &AtomicBool,
    budget: &mut usize,
) -> Scan {
    match forward {
        Some(forward) => boundary(snapshot, target, forward, cancelled, budget),
        None => floor_boundary(snapshot, target, cancelled, budget),
    }
}
fn floor_boundary(snapshot: &DocumentSnapshot, offset: usize, cancelled: &AtomicBool, budget: &mut usize) -> Scan {
    if !snapshot.is_boundary(TextOffset(offset)) {
        return Err("Invalid grapheme hit".into());
    }
    let mut cursor = GraphemeCursor::new(offset, snapshot.len(), true);
    let Some((start, text)) = charged(snapshot, offset, false, budget)? else {
        return Ok(None);
    };
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Grapheme hit cancelled".into());
        }
        match cursor.is_boundary(&text, start) {
            Ok(true) => return Ok(Some(offset)),
            Ok(false) => return boundary(snapshot, offset, false, cancelled, budget),
            Err(GraphemeIncomplete::PreContext(end)) => {
                let Some((from, context)) = charged(snapshot, end, true, budget)? else {
                    return Ok(None);
                };
                cursor.provide_context(&context, from);
            }
            Err(_) => return Err("Invalid grapheme hit context".into()),
        }
    }
}
/// `chunk`, charged against `budget`; `None` once the budget is spent.
fn charged(
    snapshot: &DocumentSnapshot,
    at: usize,
    backwards: bool,
    budget: &mut usize,
) -> Result<Option<(usize, String)>, String> {
    if *budget == 0 {
        return Ok(None);
    }
    let (start, text) = chunk(snapshot, at, backwards)?;
    *budget = budget.saturating_sub(text.len().max(1));
    Ok(Some((start, text)))
}
pub(crate) fn chunk(snapshot: &DocumentSnapshot, at: usize, backwards: bool) -> Result<(usize, String), String> {
    let (mut start, mut end) = if backwards {
        (at.saturating_sub(4096), at)
    } else {
        (at, at.saturating_add(4096).min(snapshot.len()))
    };
    while !snapshot.is_boundary(TextOffset(start)) {
        start += 1;
    }
    while !snapshot.is_boundary(TextOffset(end)) {
        end -= 1;
    }
    snapshot
        .read(TextOffset(start)..TextOffset(end), 4096)
        .map(|text| (start, text))
        .map_err(|e| format!("Grapheme source: {e:?}"))
}
fn boundary(
    snapshot: &DocumentSnapshot,
    origin: usize,
    forward: bool,
    cancelled: &AtomicBool,
    budget: &mut usize,
) -> Scan {
    if !snapshot.is_boundary(TextOffset(origin)) {
        return Err("Invalid grapheme origin".into());
    }
    let mut cursor = GraphemeCursor::new(origin, snapshot.len(), true);
    let Some((mut start, mut text)) = charged(snapshot, origin, !forward, budget)? else {
        return Ok(None);
    };
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Grapheme navigation cancelled".into());
        }
        let result = if forward {
            cursor.next_boundary(&text, start)
        } else {
            cursor.prev_boundary(&text, start)
        };
        match result {
            Ok(result) => return Ok(Some(result.unwrap_or(origin))),
            Err(GraphemeIncomplete::NextChunk) => {
                let Some(next) = charged(snapshot, start + text.len(), false, budget)? else {
                    return Ok(None);
                };
                (start, text) = next;
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                let Some(previous) = charged(snapshot, start, true, budget)? else {
                    return Ok(None);
                };
                (start, text) = previous;
            }
            Err(GraphemeIncomplete::PreContext(end)) => {
                let Some((from, context)) = charged(snapshot, end, true, budget)? else {
                    return Ok(None);
                };
                cursor.provide_context(&context, from);
            }
            Err(GraphemeIncomplete::InvalidOffset) => return Err("Invalid grapheme context".into()),
        }
    }
}
impl crate::EditorSurface {
    /// Install a started move. One answered in place applies at once, so the
    /// surface is not busy (holding queued input) until the next pump. That
    /// pump still reports the change: a move resolved during `draw` lands after
    /// the caret was drawn, and only a changed pump asks for the redraw.
    pub(crate) fn begin_grapheme_navigation(&mut self, job: Navigation) {
        self.grapheme_navigation = Some(job);
        self.navigation_applied |= self.pump_virtual_navigation();
    }
    pub(crate) fn pump_virtual_navigation(&mut self) -> bool {
        let Some(job) = &self.grapheme_navigation else {
            return false;
        };
        let Some(result) = job.poll() else {
            return false;
        };
        let valid = self.snapshot.same_document(&job.snapshot)
            && self.snapshot.content_state == job.snapshot.content_state
            && self.selection.caret == job.origin;
        let extend = job.extend;
        self.grapheme_navigation = None;
        if valid {
            match result {
                Ok(offset) => {
                    self.selection.caret = offset;
                    if !extend {
                        self.selection.anchor = offset;
                    }
                    self.selections = self.selection.into();
                    self.reveal_caret = true;
                }
                Err(error) => self.error = Some(error),
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document(text: &str) -> bareline_document::Document {
        bareline_document::Document::from_utf8(
            text,
            bareline_document::Budget::new(1 << 20),
            bareline_document::Budget::new(1 << 20),
        )
        .unwrap()
    }
    /// A wake that can be waited on and reports whether it ran on a worker of
    /// the shared surface pool.
    fn wake() -> (Arc<dyn Fn() + Send + Sync>, mpsc::Receiver<bool>) {
        let (sender, receiver) = mpsc::sync_channel(64);
        (
            Arc::new(move || {
                let _ = sender.try_send(crate::surface_pool::on_pool_worker());
            }),
            receiver,
        )
    }
    #[test]
    fn giant_cluster_uses_streamed_context_and_cancels() {
        let text = format!("a{}z", "\u{301}".repeat(100_000));
        let document = document(&text);
        let snapshot = document.snapshot();
        let live = AtomicBool::new(false);
        assert_eq!(
            boundary(&snapshot, 0, true, &live, &mut usize::MAX),
            Ok(Some(text.len() - 1))
        );
        assert_eq!(
            boundary(&snapshot, text.len() - 1, false, &live, &mut usize::MAX),
            Ok(Some(0))
        );
        // Hit positions from a bounded shape can lie deep inside a cluster whose
        // true beginning is outside retained layout context. Never accept them.
        assert_eq!(floor_boundary(&snapshot, 8193, &live, &mut usize::MAX), Ok(Some(0)));
        assert_eq!(
            floor_boundary(&snapshot, text.len() - 1, &live, &mut usize::MAX),
            Ok(Some(text.len() - 1))
        );
        assert!(boundary(&snapshot, 0, true, &AtomicBool::new(true), &mut usize::MAX).is_err());
        // The in-place attempt stops once its budget is read, without an answer.
        let mut budget = SYNC_SCAN_BYTES;
        assert_eq!(boundary(&snapshot, 0, true, &live, &mut budget), Ok(None));
        assert_eq!(budget, 0);
    }
    #[test]
    fn ordinary_moves_resolve_in_place_without_a_worker() {
        let text = "e\u{301}xa\u{308}\u{301} 👩🏽‍💻\r\nz".repeat(2000);
        let document = document(&text);
        let snapshot = document.snapshot();
        let (notify, notified) = wake();
        let submitted = crate::surface_pool::submitted_here();
        let unit = text.len() / 2000;
        for (index, start) in [0, unit * 700, text.len() - unit].into_iter().enumerate() {
            let right = Navigation::start(snapshot.clone(), start, true, false, notify.clone()).unwrap();
            // The reply is there at once: no thread, nothing to wait for.
            assert_eq!(right.poll(), Some(Ok(start + "e\u{301}".len())));
            let snap = Navigation::start_snap(snapshot.clone(), start, start + 1, true, notify.clone()).unwrap();
            assert_eq!(
                snap.poll(),
                Some(Ok(start)),
                "a hit inside a cluster floors to its start"
            );
            if index > 0 {
                let left = Navigation::start(snapshot.clone(), start, false, false, notify.clone()).unwrap();
                assert_eq!(left.poll(), Some(Ok(start - "z".len())));
            }
        }
        assert_eq!(crate::surface_pool::submitted_here(), submitted);
        // Each in-place answer still wakes the owner once, from this thread.
        let wakes: Vec<bool> = notified.try_iter().collect();
        assert_eq!(wakes, vec![false; 8]);
    }
    #[test]
    fn long_scans_share_the_bounded_pool_instead_of_spawning() {
        let text = format!("a{}z", "\u{301}".repeat(100_000));
        let document = document(&text);
        let snapshot = document.snapshot();
        let (notify, notified) = wake();
        let submitted = crate::surface_pool::submitted_here();
        for round in 1..=3 {
            let job = Navigation::start(snapshot.clone(), 0, true, false, notify.clone()).unwrap();
            assert_eq!(crate::surface_pool::submitted_here(), submitted + round);
            // The scan ran on the pool's fixed worker, not a thread of its own.
            assert!(
                notified
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .expect("pool completion wake"),
                "the scan ran off the shared surface pool"
            );
            assert_eq!(job.poll(), Some(Ok(text.len() - 1)));
        }
    }
}
