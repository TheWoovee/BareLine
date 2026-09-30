// SPDX-License-Identifier: MPL-2.0
//! Fixed-size byte-density map. No per-line objects or exact-line-total claims.
use crate::task::{Task, TaskPoll};
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_renderer::{DrawOp, Point, Rect};
use bareline_ui::widgets::Theme;
use std::{ops::Range, sync::Arc};
const BUCKETS: usize = 256;
type Density = [Option<f32>; BUCKETS];
pub struct DocumentMap {
    pub open: bool,
    source: Option<DocumentSnapshot>,
    density: Density,
    /// At most one sampling job runs on the shared task pool. Edits that land
    /// while it runs coalesce into one follow-up sample of the latest revision.
    pending: Option<(Task<Density>, DocumentSnapshot)>,
    /// The density was sampled from exactly `source`.
    sampled: bool,
    /// The source is only the loaded window of a paged document.
    window: bool,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
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
        map.refresh(&source, TextOffset(0)..TextOffset(4), false, notify.clone());
        map.density[0] = Some(1.0);
        // Same document and revision: the cached density survives.
        map.refresh(&source, TextOffset(0)..TextOffset(4), false, notify.clone());
        assert_eq!(map.density[0], Some(1.0));
        // A different document invalidates it.
        let other = Document::from_utf8("zzzzzz", Budget::new(4096), Budget::new(4096))
            .unwrap()
            .snapshot();
        map.refresh(&other, TextOffset(0)..TextOffset(4), false, notify.clone());
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
        assert!(map.pending.is_none());
        assert!(map.source.is_none());
    }
    #[test]
    fn edits_coalesce_into_one_pooled_sample_and_paged_windows_are_labelled() {
        let mut map = DocumentMap::default();
        map.open = true;
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let mut document = Document::from_utf8("abc def", Budget::new(4096), Budget::new(4096)).unwrap();
        let first = document.snapshot();
        map.refresh(&first, TextOffset(0)..TextOffset(7), false, notify.clone());
        let job = map.pending.as_ref().map(|(_, sampling)| sampling.identity_token());
        assert_eq!(job, Some(first.identity_token()));
        // Rapid edits while sampling do not start more jobs.
        for text in ["x", "y", "z"] {
            document
                .apply(bareline_document::EditTransaction {
                    base_revision: document.snapshot().revision,
                    edits: vec![bareline_document::Edit {
                        range: TextOffset(0)..TextOffset(0),
                        insert: text.into(),
                    }],
                })
                .unwrap();
            map.refresh(
                &document.snapshot(),
                TextOffset(0)..TextOffset(7),
                false,
                notify.clone(),
            );
            assert_eq!(map.pending.as_ref().map(|(_, sampling)| sampling.identity_token()), job);
        }
        let latest = document.snapshot();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !(map.sampled && map.pending.is_none()) {
            map.pump();
            assert!(std::time::Instant::now() < deadline, "map sampling never settled");
            std::thread::yield_now();
        }
        // The follow-up sample covers the latest revision, not every edit.
        assert!(
            map.source
                .as_ref()
                .is_some_and(|source| source.revision == latest.revision)
        );
        assert!(map.density.iter().all(Option::is_some));
        map.refresh(&latest, TextOffset(0)..TextOffset(4), true, notify);
        let mut ops = Vec::new();
        map.draw(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 64.0,
                height: 100.0,
            },
            &mut ops,
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, DrawOp::Text { text, .. } if text == "Window"))
        );
    }
}
impl Default for DocumentMap {
    fn default() -> Self {
        Self {
            open: false,
            source: None,
            density: [None; BUCKETS],
            pending: None,
            sampled: false,
            window: false,
            notify: None,
            bounds: Rect::default(),
            viewport: TextOffset(0)..TextOffset(0),
        }
    }
}
impl DocumentMap {
    pub fn clear(&mut self) {
        // Dropping the task cancels it.
        self.pending = None;
        self.source = None;
        self.sampled = false;
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
    /// `window` marks a paged document, whose snapshot is only its loaded window.
    pub fn refresh(
        &mut self,
        source: &DocumentSnapshot,
        viewport: Range<TextOffset>,
        window: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.viewport = viewport;
        self.window = window;
        if !self.open {
            self.pending = None;
            self.source = None;
            self.sampled = false;
            return;
        }
        self.notify = Some(notify);
        let current = self.source.as_ref().is_some_and(|old| same_state(old, source));
        if !current {
            // Another document's density is meaningless; the same document keeps
            // its previous map on screen until the new sample lands.
            if !self.source.as_ref().is_some_and(|old| old.same_document(source)) {
                self.pending = None;
                self.density = [None; BUCKETS];
            }
            self.source = Some(source.clone());
            self.sampled = false;
        }
        if !self.sampled && self.pending.is_none() {
            self.start();
        }
    }
    /// Sample the current source on the shared pool. A full queue leaves the map
    /// unsampled; the next refresh retries.
    fn start(&mut self) {
        let (Some(source), Some(notify)) = (self.source.clone(), self.notify.clone()) else {
            return;
        };
        let sampling = source.clone();
        if let Ok(task) = crate::task::spawn(
            move || notify(),
            move |cancel| sample(&source, || cancel.is_cancelled()),
        ) {
            self.pending = Some((task, sampling));
        }
    }
    pub fn pump(&mut self) -> bool {
        let Some((task, _)) = &self.pending else {
            return false;
        };
        match task.poll() {
            TaskPoll::Pending => false,
            TaskPoll::Complete(density) => {
                let Some((_, sampling)) = self.pending.take() else {
                    return false;
                };
                let Some(source) = &self.source else {
                    return true;
                };
                // An older revision of the same document still beats an empty map.
                if source.same_document(&sampling) {
                    self.density = density;
                }
                self.sampled = same_state(source, &sampling);
                if !self.sampled {
                    self.start();
                }
                true
            }
            TaskPoll::Cancelled | TaskPoll::Failed(_) | TaskPoll::Consumed => {
                self.pending = None;
                true
            }
        }
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
            // A paged document maps only its loaded window, never the whole file.
            let label = if self.window {
                Some("Window")
            } else if self.pending.is_some() {
                Some("Sampling")
            } else if !source.is_complete() || self.density.iter().any(Option::is_none) {
                Some("Partial")
            } else {
                None
            };
            if let Some(label) = label {
                ops.push(DrawOp::Text {
                    origin: Point {
                        x: bounds.x + 2.0,
                        y: bounds.y + 2.0,
                    },
                    text: label.into(),
                    size: 10.0,
                    color: theme.text,
                });
            }
        }
        ops.push(DrawOp::PopClip);
    }
}
fn same_state(old: &DocumentSnapshot, new: &DocumentSnapshot) -> bool {
    old.same_document(new)
        && old.revision == new.revision
        && old.len() == new.len()
        && old.is_complete() == new.is_complete()
}
/// Density of non-whitespace bytes in a small sample at each bucket start.
fn sample(source: &DocumentSnapshot, cancelled: impl Fn() -> bool) -> Density {
    let mut density = [None; BUCKETS];
    for (bucket, value) in density.iter_mut().enumerate() {
        if cancelled() {
            break;
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
        *value = source.read(TextOffset(start)..TextOffset(end), 128).ok().map(|s| {
            if s.is_empty() {
                0.0
            } else {
                s.bytes().filter(|b| !b.is_ascii_whitespace()).count() as f32 / s.len() as f32
            }
        });
    }
    density
}
