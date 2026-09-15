// SPDX-License-Identifier: MPL-2.0
//! Progressive long-line preparation. Only one small fragment is shaped per
//! paint; exact measured advances become bounded sparse navigation checkpoints.
use bareline_document::{DocumentSnapshot, TextOffset};
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};
const CHUNK: usize = 4096;
const CONTEXT: usize = 2048;
struct Prepared {
    start: usize,
    text: String,
}
#[derive(Clone, Copy)]
struct Checkpoint {
    byte: usize,
    x: f64,
    row: usize,
}
struct Pending {
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Result<Prepared, String>>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
pub(crate) struct VirtualLine {
    state: bareline_document::ContentStateId,
    range: Range<usize>,
    cursor: Checkpoint,
    checkpoints: Vec<Checkpoint>,
    pending: Option<Pending>,
    pub text: Option<String>,
    pub end: usize,
    pub context_start: usize,
    measured: Option<(f64, usize)>,
    request_x: f64,
    request_row: usize,
    request_caret: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragment_keeps_complex_script_context_and_backward_seek_discards_stale_result() {
        let text = format!("{}مرحبا 👩🏽‍💻 தமிழ் a\u{301}{}", "x".repeat(4088), "y".repeat(9000));
        let doc = bareline_document::Document::from_utf8(
            &text,
            bareline_document::Budget::new(1 << 20),
            bareline_document::Budget::new(1 << 20),
        )
        .unwrap();
        let snapshot = doc.snapshot();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let mut line = VirtualLine::new(&snapshot, 0..text.len());
        let ready = |line: &mut VirtualLine| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !line.prepare(&snapshot, notify.clone()).unwrap() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        };
        ready(&mut line);
        assert!(line.text.as_ref().unwrap().contains("مرحبا 👩🏽‍💻 தமிழ் a\u{301}"));
        line.measured(4096.0, 20.0, 20.0, false);
        line.seek(5000.0, 0, None, false);
        ready(&mut line);
        assert!(line.context_start < line.origin().0);
        assert!(line.text.as_ref().unwrap().contains("مرحبا 👩🏽‍💻 தமிழ் a\u{301}"));
        line.measured(4096.0, 20.0, 20.0, false);
        line.seek(10000.0, 0, None, false);
        let _ = line.prepare(&snapshot, notify.clone());
        line.seek(0.0, 0, None, false);
        ready(&mut line);
        assert_eq!(line.origin().0, 0);
        assert_eq!(line.context_start, 0);
        assert!(line.text.as_ref().unwrap().len() <= CHUNK + 2 * CONTEXT);
    }
}
impl VirtualLine {
    pub fn new(snapshot: &DocumentSnapshot, range: Range<usize>) -> Self {
        let cursor = Checkpoint {
            byte: range.start,
            x: 0.0,
            row: 0,
        };
        Self {
            state: snapshot.content_state,
            range,
            cursor,
            checkpoints: vec![cursor],
            pending: None,
            text: None,
            end: cursor.byte,
            context_start: cursor.byte,
            measured: None,
            request_x: 0.0,
            request_row: 0,
            request_caret: None,
        }
    }
    pub fn matches(&self, snapshot: &DocumentSnapshot, range: &Range<usize>) -> bool {
        self.state == snapshot.content_state && self.range == *range
    }
    pub fn origin(&self) -> (usize, f64, usize) {
        (self.cursor.byte, self.cursor.x, self.cursor.row)
    }
    pub fn seek(&mut self, x: f64, row: usize, caret: Option<usize>, wrap: bool) {
        self.request_x = x;
        self.request_row = row;
        self.request_caret = caret;
        let before = caret.map_or(if wrap { row < self.cursor.row } else { x < self.cursor.x }, |caret| {
            caret < self.cursor.byte
        });
        if before {
            let checkpoint = self
                .checkpoints
                .iter()
                .rev()
                .find(|p| caret.map_or(if wrap { p.row <= row } else { p.x <= x }, |caret| p.byte <= caret))
                .copied()
                .unwrap_or(self.checkpoints[0]);
            self.cursor = checkpoint;
            self.pending = None;
            self.text = None;
            self.measured = None;
            self.end = checkpoint.byte;
        }
        if let Some((width, rows)) = self.measured {
            let after = caret.map_or(
                if wrap {
                    row >= self.cursor.row.saturating_add(rows)
                } else {
                    x >= self.cursor.x + width
                },
                |caret| caret >= self.end,
            );
            if after && self.end < self.range.end {
                let checkpoint = Checkpoint {
                    byte: self.end,
                    x: if wrap { 0.0 } else { self.cursor.x + width },
                    row: self.cursor.row.saturating_add(rows),
                };
                if self.checkpoints.last().is_none_or(|last| last.byte < checkpoint.byte) {
                    if self.checkpoints.len() >= 512 {
                        self.checkpoints = self.checkpoints.iter().step_by(2).copied().collect();
                    }
                    self.checkpoints.push(checkpoint);
                }
                self.cursor = checkpoint;
                self.text = None;
                self.measured = None;
            }
        }
    }
    /// Unwrapped caret jumps need only a nearby shaped fragment, not every prefix advance.
    pub fn pan_before_origin(&mut self, snapshot: &DocumentSnapshot) -> Option<usize> {
        let old = self.cursor.byte;
        if old <= self.range.start {
            return None;
        }
        let mut before = old - 1;
        while before > self.range.start && !snapshot.is_boundary(TextOffset(before)) {
            before -= 1;
        }
        self.anchor_caret(snapshot, before).then_some(old)
    }
    pub fn anchor_caret(&mut self, snapshot: &DocumentSnapshot, caret: usize) -> bool {
        if caret < self.range.start
            || caret > self.range.end
            || (caret >= self.cursor.byte
                && caret
                    <= if self.text.is_some() {
                        self.end
                    } else {
                        self.cursor.byte.saturating_add(CHUNK)
                    })
        {
            return false;
        }
        let mut start = caret.saturating_sub(CONTEXT).max(self.range.start);
        while start < caret && !snapshot.is_boundary(TextOffset(start)) {
            start += 1;
        }
        self.cursor = Checkpoint {
            byte: start,
            x: 0.0,
            row: 0,
        };
        self.checkpoints = vec![self.cursor];
        self.pending = None;
        self.text = None;
        self.measured = None;
        self.end = start;
        true
    }
    pub fn prepare(
        &mut self,
        snapshot: &DocumentSnapshot,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<bool, String> {
        if let Some(pending) = &self.pending {
            match pending.receiver.try_recv() {
                Ok(result) => {
                    let prepared = result?;
                    self.context_start = prepared.start;
                    self.text = Some(prepared.text);
                    self.pending = None;
                    return Ok(true);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    return Err("Long-line preparation stopped".into());
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(false),
            }
        }
        if self.text.is_some() {
            return Ok(true);
        }
        let snapshot = snapshot.clone();
        let start = self.cursor.byte;
        let mut end = start.saturating_add(CHUNK).min(self.range.end);
        while !snapshot.is_boundary(TextOffset(end)) {
            end -= 1;
        }
        self.end = end;
        let mut context_start = start.saturating_sub(CONTEXT).max(self.range.start);
        let mut context_end = end.saturating_add(CONTEXT).min(self.range.end);
        while !snapshot.is_boundary(TextOffset(context_start)) {
            context_start += 1;
        }
        while !snapshot.is_boundary(TextOffset(context_end)) {
            context_end -= 1;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("bareline-layout-fragment".into())
            .spawn(move || {
                if worker_cancel.load(Ordering::Relaxed) {
                    return;
                }
                let result = snapshot
                    .read(TextOffset(context_start)..TextOffset(context_end), CHUNK + 2 * CONTEXT)
                    .map(|text| Prepared {
                        start: context_start,
                        text,
                    })
                    .map_err(|e| format!("Long-line source: {e:?}"));
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    notify();
                }
            })
            .map_err(|e| e.to_string())?;
        self.pending = Some(Pending { cancel, receiver });
        Ok(false)
    }
    pub fn measured(&mut self, width: f32, height: f32, line_height: f32, wrap: bool) -> bool {
        let rows = if wrap {
            (height / line_height).ceil().max(1.0) as usize
        } else {
            0
        };
        self.measured = Some((f64::from(width), rows));
        self.end < self.range.end
            && self.request_caret.map_or(
                if wrap {
                    self.request_row >= self.cursor.row.saturating_add(rows)
                } else {
                    self.request_x >= self.cursor.x + f64::from(width)
                },
                |caret| caret >= self.end,
            )
    }
}
