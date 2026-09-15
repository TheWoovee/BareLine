// SPDX-License-Identifier: MPL-2.0
//! Cancellable exact columns over bounded chunks; one shared worker and bounded checkpoints.
use bareline_document::DocumentSnapshot;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};
type Job = Box<dyn FnOnce() + Send>;
fn worker() -> &'static SyncSender<Job> {
    static WORKER: OnceLock<SyncSender<Job>> = OnceLock::new();
    // A failed spawn drops the receiver with the closure, so `try_send` then fails
    // and the caller reports "Column worker stopped" instead of aborting.
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(8);
        let _spawned = std::thread::Builder::new()
            .name("bareline-columns".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job();
                }
            });
        debug_assert!(_spawned.is_ok(), "column worker");
        tx
    })
}
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
        match result {
            Ok((points, column)) => {
                self.resolved = Some((pending.target, column));
                self.checkpoints.extend(points);
                self.checkpoints.sort_unstable();
                self.checkpoints.dedup_by_key(|point| point.0);
                while self.checkpoints.len() > 512 {
                    self.checkpoints = self.checkpoints.iter().step_by(2).copied().collect();
                }
            }
            Err(error) => self.failure = Some(error),
        }
        self.pending = None;
        true
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
            let captured = snapshot.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let cancelled = cancel.clone();
            let (tx, receiver) = mpsc::sync_channel(1);
            let retry = notify.clone();
            if worker()
                .try_send(Box::new(move || {
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
