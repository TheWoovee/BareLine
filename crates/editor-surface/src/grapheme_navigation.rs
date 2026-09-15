// SPDX-License-Identifier: MPL-2.0
//! Streaming extended-grapheme navigation. Retained context is at most two
//! 4 KiB chunks, including clusters spanning megabytes of combining characters.
use bareline_document::{DocumentSnapshot, TextOffset};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};
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
        let worker_cancel = cancel.clone();
        let captured = snapshot.clone();
        let (tx, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("bareline-grapheme".into())
            .spawn(move || {
                let result = match forward {
                    Some(forward) => boundary(&captured, target, forward, &worker_cancel),
                    None => floor_boundary(&captured, target, &worker_cancel),
                };
                let _ = tx.send(result);
                notify();
            })
            .map_err(|e| e.to_string())?;
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
fn floor_boundary(snapshot: &DocumentSnapshot, offset: usize, cancelled: &AtomicBool) -> Result<usize, String> {
    if !snapshot.is_boundary(TextOffset(offset)) {
        return Err("Invalid grapheme hit".into());
    }
    let mut cursor = GraphemeCursor::new(offset, snapshot.len(), true);
    let (start, text) = chunk(snapshot, offset, false)?;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Grapheme hit cancelled".into());
        }
        match cursor.is_boundary(&text, start) {
            Ok(true) => return Ok(offset),
            Ok(false) => return boundary(snapshot, offset, false, cancelled),
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (from, context) = chunk(snapshot, end, true)?;
                cursor.provide_context(&context, from);
            }
            Err(_) => return Err("Invalid grapheme hit context".into()),
        }
    }
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
pub(crate) fn boundary(
    snapshot: &DocumentSnapshot,
    origin: usize,
    forward: bool,
    cancelled: &AtomicBool,
) -> Result<usize, String> {
    if !snapshot.is_boundary(TextOffset(origin)) {
        return Err("Invalid grapheme origin".into());
    }
    let mut cursor = GraphemeCursor::new(origin, snapshot.len(), true);
    let (mut start, mut text) = chunk(snapshot, origin, !forward)?;
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
            Ok(result) => return Ok(result.unwrap_or(origin)),
            Err(GraphemeIncomplete::NextChunk) => {
                (start, text) = chunk(snapshot, start + text.len(), false)?;
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                (start, text) = chunk(snapshot, start, true)?;
            }
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (from, context) = chunk(snapshot, end, true)?;
                cursor.provide_context(&context, from);
            }
            Err(GraphemeIncomplete::InvalidOffset) => return Err("Invalid grapheme context".into()),
        }
    }
}
impl crate::EditorSurface {
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
    #[test]
    fn giant_cluster_uses_streamed_context_and_cancels() {
        let text = format!("a{}z", "\u{301}".repeat(100_000));
        let document = bareline_document::Document::from_utf8(
            &text,
            bareline_document::Budget::new(1 << 20),
            bareline_document::Budget::new(1 << 20),
        )
        .unwrap();
        let snapshot = document.snapshot();
        assert_eq!(
            boundary(&snapshot, 0, true, &AtomicBool::new(false)).unwrap(),
            text.len() - 1
        );
        assert_eq!(
            boundary(&snapshot, text.len() - 1, false, &AtomicBool::new(false)).unwrap(),
            0
        );
        // Hit positions from a bounded shape can lie deep inside a cluster whose
        // true beginning is outside retained layout context. Never accept them.
        assert_eq!(floor_boundary(&snapshot, 8193, &AtomicBool::new(false)).unwrap(), 0);
        assert_eq!(
            floor_boundary(&snapshot, text.len() - 1, &AtomicBool::new(false)).unwrap(),
            text.len() - 1
        );
        assert!(boundary(&snapshot, 0, true, &AtomicBool::new(true)).is_err());
    }
}
