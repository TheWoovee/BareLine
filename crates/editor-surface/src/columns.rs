// SPDX-License-Identifier: MPL-2.0
//! Cancellable exact columns over bounded chunks and bounded checkpoints. Short
//! counts run in place; long ones go to the shared surface pool.
use bareline_document::DocumentSnapshot;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};
/// Bytes counted on the UI thread instead of on the pool (EDT-20). Matches the
/// checkpoint spacing, so once a long line has been counted, every later label
/// on it is resolved in place.
const SYNC_COUNT_BYTES: usize = 65536;
struct Pending {
    target: usize,
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Result<(Vec<(usize, usize)>, usize), String>>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
#[derive(Default)]
pub(crate) struct Columns {
    identity: Option<(u64, u64)>,
    line_start: usize,
    checkpoints: Vec<(usize, usize)>,
    pending: Option<Pending>,
    failure: Option<String>,
    resolved: Option<(usize, usize)>,
}
impl Columns {
    /// Consume worker completion independently of painting so the event-loop
    /// pump can request the frame that publishes the resolved status to UIA.
    pub fn poll(&mut self) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("Column worker stopped".into()),
        };
        let target = pending.target;
        self.pending = None;
        self.absorb(target, result);
        true
    }
    fn absorb(&mut self, target: usize, result: Result<(Vec<(usize, usize)>, usize), String>) {
        match result {
            Ok((points, column)) => {
                self.resolved = Some((target, column));
                self.checkpoints.extend(points);
                self.checkpoints.sort_unstable();
                self.checkpoints.dedup_by_key(|point| point.0);
                while self.checkpoints.len() > 512 {
                    self.checkpoints = self.checkpoints.iter().step_by(2).copied().collect();
                }
            }
            Err(error) => self.failure = Some(error),
        }
    }
    pub fn label(
        &mut self,
        snapshot: &DocumentSnapshot,
        line_start: usize,
        target: usize,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> String {
        if self.identity != Some(snapshot.identity_token()) || self.line_start != line_start {
            self.pending = None;
            self.identity = Some(snapshot.identity_token());
            self.line_start = line_start;
            self.checkpoints = vec![(line_start, 1)];
            self.failure = None;
            self.resolved = None;
        }
        self.poll();
        if let Some((offset, column)) = self.resolved
            && offset == target
        {
            return column.to_string();
        }
        if let Some((_, column)) = self.checkpoints.iter().find(|point| point.0 == target) {
            return column.to_string();
        }
        if self.pending.as_ref().is_some_and(|pending| pending.target != target) {
            self.pending = None;
            self.failure = None;
        }
        if self.pending.is_none() && self.failure.is_none() {
            let seed = self
                .checkpoints
                .iter()
                .rev()
                .find(|point| point.0 <= target)
                .copied()
                .unwrap_or((line_start, 1));
            if target.saturating_sub(seed.0) <= SYNC_COUNT_BYTES {
                self.absorb(target, count(snapshot, seed, target, &AtomicBool::new(false)));
                if let Some((offset, column)) = self.resolved
                    && offset == target
                {
                    return column.to_string();
                }
                return "unavailable (source error)".into();
            }
            let captured = snapshot.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let cancelled = cancel.clone();
            let (tx, receiver) = mpsc::sync_channel(1);
            let retry = notify.clone();
            if crate::surface_pool::submit(Box::new(move || {
                let result = count(&captured, seed, target, &cancelled);
                if !cancelled.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    notify();
                }
            }))
            .is_ok()
            {
                self.pending = Some(Pending {
                    target,
                    cancel,
                    receiver,
                });
            } else {
                retry();
            }
        }
        if self.failure.is_some() {
            "unavailable (source error)".into()
        } else {
            "counting…".into()
        }
    }
}
fn count(
    snapshot: &DocumentSnapshot,
    (start, column): (usize, usize),
    target: usize,
    cancel: &AtomicBool,
) -> Result<(Vec<(usize, usize)>, usize), String> {
    let mut cursor = GraphemeCursor::new(start, snapshot.len(), true);
    let (mut chunk_start, mut text) = super::grapheme_navigation::chunk(snapshot, start, false)?;
    let mut column = column;
    let mut points = Vec::new();
    let mut checkpoint = start;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Column count cancelled".into());
        }
        match cursor.next_boundary(&text, chunk_start) {
            Ok(Some(boundary)) => {
                column += 1;
                if boundary >= target {
                    if boundary == target {
                        points.push((target, column));
                    }
                    return Ok((points, column));
                }
                if boundary.saturating_sub(checkpoint) >= 65536 {
                    points.push((boundary, column));
                    checkpoint = boundary;
                    if points.len() >= 512 {
                        points = points.into_iter().step_by(2).collect();
                    }
                }
            }
            Ok(None) => {
                return Ok((points, column));
            }
            Err(GraphemeIncomplete::NextChunk) => {
                (chunk_start, text) = super::grapheme_navigation::chunk(snapshot, chunk_start + text.len(), false)?;
            }
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (from, context) = super::grapheme_navigation::chunk(snapshot, end, true)?;
                cursor.provide_context(&context, from);
            }
            Err(_) => return Err("Invalid column context".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;
    #[test]
    fn column_worker_completion_requests_frame_without_another_input() {
        let text = "x".repeat(70000);
        let document = bareline_document::Document::from_utf8(
            &text,
            bareline_document::Budget::new(1 << 20),
            bareline_document::Budget::new(1 << 20),
        )
        .unwrap();
        let (notify, notified) = mpsc::sync_channel(1);
        let mut editor = crate::EditorSurface::loading(
            document.snapshot(),
            Arc::new(move || {
                let _ = notify.try_send(());
            }),
        );
        editor.selection = crate::Selection {
            anchor: text.len(),
            caret: text.len(),
        };
        assert_eq!(editor.column_label(), "counting…");
        notified
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("column worker notification");
        assert!(
            editor.pump(),
            "completion must request a redraw without input or painting"
        );
        assert_eq!(editor.column_label(), "70001");
        assert!(!editor.pump(), "resolved status must not request repeated frames");
    }
    #[test]
    fn short_counts_resolve_in_place_and_long_ones_use_the_pool_once() {
        let text = format!("{}{}", "ab\u{301}c".repeat(1000), "x".repeat(200_000));
        let document = bareline_document::Document::from_utf8(
            &text,
            bareline_document::Budget::new(1 << 20),
            bareline_document::Budget::new(1 << 20),
        )
        .unwrap();
        let snapshot = document.snapshot();
        let (notify, notified) = mpsc::sync_channel(1);
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = notify.try_send(crate::surface_pool::on_pool_worker());
        });
        let submitted = crate::surface_pool::submitted_here();
        let mut cache = Columns::default();
        // 5,000 bytes of three-cluster units: counted in place, no job, no wait.
        assert_eq!(cache.label(&snapshot, 0, 5000, notify.clone()), "3001");
        assert!(cache.pending.is_none());
        assert_eq!(crate::surface_pool::submitted_here(), submitted);
        // Past the in-place budget: exactly one pool job.
        let far = 5000 + 150_000;
        assert_eq!(cache.label(&snapshot, 0, far, notify.clone()), "counting…");
        assert_eq!(crate::surface_pool::submitted_here(), submitted + 1);
        // The count ran on the pool's fixed worker, not a thread of its own.
        assert!(
            notified
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("column pool wake"),
            "the count ran off the shared surface pool"
        );
        assert!(cache.poll());
        assert_eq!(
            cache.label(&snapshot, 0, far, notify.clone()),
            (3000 + 150_000 + 1).to_string()
        );
        // The counted path left 64 KiB checkpoints: carets near it count in place.
        assert_eq!(
            cache.label(&snapshot, 0, far - 1000, notify.clone()),
            (3000 + 149_000 + 1).to_string()
        );
        assert_eq!(
            cache.label(&snapshot, 0, far + 40_000, notify.clone()),
            (3000 + 190_000 + 1).to_string()
        );
        assert_eq!(crate::surface_pool::submitted_here(), submitted + 1);
    }
    #[test]
    fn streamed_columns_preserve_cross_chunk_clusters_and_checkpoint_counts() {
        let text = format!(
            "{}a{}🇺🇸👩🏽‍💻{}",
            "x".repeat(70000),
            "\u{301}".repeat(7000),
            "y".repeat(70000)
        );
        let mut document = bareline_document::Document::from_utf8(
            &text,
            bareline_document::Budget::new(1 << 20),
            bareline_document::Budget::new(1 << 20),
        )
        .unwrap();
        let snapshot = document.snapshot();
        let cancel = AtomicBool::new(false);
        let (points, column) = count(&snapshot, (0, 1), text.len(), &cancel).unwrap();
        assert_eq!(column, text.graphemes(true).count() + 1);
        let seed = points
            .iter()
            .copied()
            .find(|(offset, _)| *offset > 65536 && *offset < text.len())
            .unwrap();
        assert_eq!(count(&snapshot, seed, text.len(), &cancel).unwrap().1, column);
        let mut cache = Columns {
            identity: Some(snapshot.identity_token()),
            line_start: 0,
            checkpoints: points,
            resolved: Some((text.len(), column)),
            ..Default::default()
        };
        assert_eq!(
            cache.label(&snapshot, 0, text.len(), Arc::new(|| {})),
            column.to_string()
        );
        document
            .apply(bareline_document::EditTransaction {
                base_revision: snapshot.revision,
                edits: vec![bareline_document::Edit {
                    range: bareline_document::TextOffset(0)..bareline_document::TextOffset(0),
                    insert: "z".into(),
                }],
            })
            .unwrap();
        let updated = document.snapshot();
        assert_ne!(
            cache.label(&updated, 0, text.len(), Arc::new(|| {})),
            column.to_string()
        );
        assert_eq!(cache.identity, Some(updated.identity_token()));
        assert_eq!(cache.checkpoints, vec![(0, 1)]);
        drop(cache);
        cancel.store(true, Ordering::Relaxed);
        assert!(count(&snapshot, (0, 1), text.len(), &cancel).is_err());
    }
}
