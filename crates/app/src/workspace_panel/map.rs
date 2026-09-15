// SPDX-License-Identifier: MPL-2.0
//! Fixed-size byte-density map. No per-line objects or exact-line-total claims.
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_renderer::{DrawOp, Point, Rect};
use bareline_ui::widgets::Theme;
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};
const BUCKETS: usize = 256;
pub struct DocumentMap {
    pub open: bool,
    source: Option<DocumentSnapshot>,
    density: [Option<f32>; BUCKETS],
    pending: Option<Receiver<(usize, Option<f32>)>>,
    cancel: Arc<AtomicBool>,
    bounds: Rect,
    viewport: Range<TextOffset>,
}
#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document, DocumentBuilder};
    #[test]
    fn map_cache_is_keyed_by_document_and_revision_and_cleared_on_close() {
        let mut map = DocumentMap::default();
        map.open = true;
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let document = Document::from_utf8(
            "abc
def",
            Budget::new(4096),
            Budget::new(4096),
        )
        .unwrap();
        let source = document.snapshot();
        map.refresh(&source, TextOffset(0)..TextOffset(4), notify.clone());
        map.density[0] = Some(1.0);
        // Same document and revision: the cached density survives.
        map.refresh(&source, TextOffset(0)..TextOffset(4), notify.clone());
        assert_eq!(map.density[0], Some(1.0));
        // A different document invalidates it.
        let other = Document::from_utf8("zzzzzz", Budget::new(4096), Budget::new(4096))
            .unwrap()
            .snapshot();
        map.refresh(&other, TextOffset(0)..TextOffset(4), notify.clone());
        assert_eq!(map.density[0], None);
        map.density[0] = Some(0.5);
        // Closing an unrelated document leaves the cache alone.
        map.forget(source.identity_token());
        assert!(map.source.is_some());
        map.forget(other.identity_token());
        assert!(map.source.is_none());
        assert_eq!(map.density[0], None);
    }
    #[test]
    fn partial_map_navigation_uses_bytes_and_rejects_other_documents() {
        let mut builder = DocumentBuilder::new(Budget::new(4096), Budget::new(4096)).unwrap();
        builder.append("aλb\ntext").unwrap();
        let source = builder.prefix();
        let mut map = DocumentMap::default();
        map.open = true;
        map.source = Some(source.clone());
        map.viewport = TextOffset(0)..TextOffset(4);
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: 64.0,
            height: 100.0,
        };
        let mut ops = Vec::new();
        map.draw(bounds, &mut ops);
        assert!(
            ops.iter()
                .any(|op| matches!(op, DrawOp::Text { text, .. } if text == "Partial"))
        );
        let target = map.pointer(Point { x: 1.0, y: 25.0 }, &source).unwrap();
        assert!(source.is_boundary(target));
        assert_eq!(map.density.len(), 256);
        let other = Document::from_utf8("aλb\ntext", Budget::new(4096), Budget::new(4096))
            .unwrap()
            .snapshot();
        assert_eq!(map.pointer(Point { x: 1.0, y: 25.0 }, &other), None);
        map.clear();
        assert!(map.cancel.load(Ordering::Relaxed));
        assert!(map.source.is_none());
    }
}
impl Default for DocumentMap {
    fn default() -> Self {
        Self {
            open: false,
            source: None,
            density: [None; BUCKETS],
            pending: None,
            cancel: Arc::new(AtomicBool::new(false)),
            bounds: Rect::default(),
            viewport: TextOffset(0)..TextOffset(0),
        }
    }
}
impl Drop for DocumentMap {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl DocumentMap {
    pub fn clear(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.pending = None;
        self.source = None;
        self.density = [None; BUCKETS];
    }
    /// Drop the cached density when the document it was built from is closed, so
    /// a later document of the same size never shows the previous document's map.
    pub fn forget(&mut self, identity: (u64, u64)) {
        if self
            .source
            .as_ref()
            .is_some_and(|source| source.identity_token() == identity)
        {
            self.clear();
        }
    }
    pub fn refresh(
        &mut self,
        source: &DocumentSnapshot,
        viewport: Range<TextOffset>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.viewport = viewport;
        if !self.open {
            self.cancel.store(true, Ordering::Relaxed);
            self.pending = None;
            self.source = None;
            return;
        }
        if self.source.as_ref().is_some_and(|old| {
            old.same_document(source)
                && old.revision == source.revision
                && old.len() == source.len()
                && old.is_complete() == source.is_complete()
        }) {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.pending = None;
        self.source = Some(source.clone());
        self.density = [None; BUCKETS];
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let source = source.clone();
        let (tx, rx) = mpsc::sync_channel(16);
        if std::thread::Builder::new()
            .name("document-map".into())
            .spawn(move || {
                for bucket in 0..BUCKETS {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let mut start = ((source.len() as u128 * bucket as u128) / BUCKETS as u128) as usize;
                    for _ in 0..4 {
                        if source.is_boundary(TextOffset(start)) {
                            break;
                        }
                        start = start.saturating_sub(1);
                    }
                    let mut end = source.len().min(start.saturating_add(128));
                    for _ in 0..4 {
                        if source.is_boundary(TextOffset(end)) {
                            break;
                        }
                        end = end.saturating_sub(1);
                    }
                    let density = source.read(TextOffset(start)..TextOffset(end), 128).ok().map(|s| {
                        if s.is_empty() {
                            0.0
                        } else {
                            s.bytes().filter(|b| !b.is_ascii_whitespace()).count() as f32 / s.len() as f32
                        }
                    });
                    if tx.send((bucket, density)).is_err() {
                        return;
                    }
                    notify();
                }
            })
            .is_ok()
        {
            self.pending = Some(rx);
        }
    }
    pub fn pump(&mut self) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        let mut changed = false;
        for _ in 0..16 {
            match rx.try_recv() {
                Ok((i, v)) => {
                    self.density[i] = v;
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(_) => {
                    self.pending = None;
                    changed = true;
                    break;
                }
            }
        }
        changed
    }
    pub fn pointer(&self, p: Point, current: &DocumentSnapshot) -> Option<TextOffset> {
        if !self.open || !self.bounds.contains(p) || self.bounds.height <= 0.0 {
            return None;
        }
        let old = self.source.as_ref()?;
        if !old.same_document(current) || old.revision != current.revision {
            return None;
        }
        let ratio = ((p.y - self.bounds.y) / self.bounds.height).clamp(0.0, 1.0) as f64;
        let mut offset = (ratio * current.len() as f64) as usize;
        for _ in 0..4 {
            if current.is_boundary(TextOffset(offset)) {
                return Some(TextOffset(offset));
            }
            offset = offset.saturating_sub(1);
        }
        None
    }
    pub fn draw(&mut self, bounds: Rect, ops: &mut Vec<DrawOp>) {
        self.draw_with_theme(bounds, Theme::default(), ops);
    }
    pub fn draw_with_theme(&mut self, bounds: Rect, theme: Theme, ops: &mut Vec<DrawOp>) {
        if !self.open {
            return;
        }
        self.bounds = bounds;

        ops.push(DrawOp::Fill(bounds, theme.surface));
        ops.push(DrawOp::PushClip(bounds));
        for (index, density) in self.density.iter().enumerate() {
            let y = bounds.y + index as f32 / BUCKETS as f32 * bounds.height;
            let height = (bounds.height / BUCKETS as f32).max(1.0);
            if let Some(d) = density {
                ops.push(DrawOp::Fill(
                    Rect {
                        x: bounds.x + 3.0,
                        y,
                        width: (bounds.width - 6.0).max(0.0) * d,
                        height,
                    },
                    theme.muted,
                ));
            }
        }
        if let Some(source) = &self.source {
            let len = source.len().max(1) as f32;
            let y = bounds.y + self.viewport.start.0 as f32 / len * bounds.height;
            let height =
                ((self.viewport.end.0.saturating_sub(self.viewport.start.0)) as f32 / len * bounds.height).max(3.0);
            ops.push(DrawOp::Stroke(
                Rect {
                    x: bounds.x,
                    y,
                    width: bounds.width,
                    height,
                },
                theme.focus,
                1.0,
            ));
            if !source.is_complete() || self.density.iter().any(Option::is_none) {
                ops.push(DrawOp::Text {
                    origin: Point {
                        x: bounds.x + 2.0,
                        y: bounds.y + 2.0,
                    },
                    text: if self.pending.is_some() { "Sampling" } else { "Partial" }.into(),
                    size: 10.0,
                    color: theme.text,
                });
            }
        }
        ops.push(DrawOp::PopClip);
    }
}
