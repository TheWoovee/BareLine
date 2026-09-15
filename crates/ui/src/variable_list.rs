// SPDX-License-Identifier: MPL-2.0
//! Variable-height virtualization uses the owner's indexed geometry to seek.
use crate::widgets::Theme;
use crate::{rect, text};
use bareline_renderer::{DrawOp, Point, Rect};

pub trait VariableItemSource {
    fn len(&self) -> Option<usize>;
    fn discovered(&self) -> usize;
    /// Find the item intersecting offset using a resident prefix-sum index.
    fn index_at(&self, offset: f64) -> usize;
    fn offset_of(&self, index: usize) -> f64;
    fn item_height(&self, index: usize) -> f32;
    fn label(&self, index: usize) -> &str;
    fn is_empty(&self) -> bool {
        self.discovered() == 0
    }
}
pub struct VisibleItem {
    pub index: usize,
    pub bounds: Rect,
}
pub struct VariableList {
    pub bounds: Rect,
    pub offset: f64,
    pub selected: Option<usize>,
}
impl VariableList {
    /// At most 4096 rows can be emitted per viewport, even for corrupt/tiny metrics.
    pub fn visible(&self, source: &impl VariableItemSource, overscan: usize) -> Vec<VisibleItem> {
        if !self.offset.is_finite() || !self.bounds.height.is_finite() || self.bounds.height <= 0.0 {
            return Vec::new();
        }
        let count = source.len().unwrap_or(source.discovered()).min(source.discovered());
        let start = source
            .index_at(self.offset.max(0.0))
            .min(count)
            .saturating_sub(overscan.min(64));
        let mut rows = Vec::new();
        let mut after = 0;
        for index in start..count.min(start.saturating_add(4096)) {
            let top = source.offset_of(index);
            let height = source.item_height(index);
            if !top.is_finite() || !height.is_finite() || height <= 0.0 {
                break;
            }
            if top >= self.offset + self.bounds.height as f64 {
                if after >= overscan.min(64) {
                    break;
                }
                after += 1;
            }
            rows.push(VisibleItem {
                index,
                bounds: rect(
                    self.bounds.x,
                    self.bounds.y + (top - self.offset) as f32,
                    self.bounds.width,
                    height,
                ),
            });
        }
        rows
    }
    pub fn hit_test(&self, source: &impl VariableItemSource, point: Point) -> Option<usize> {
        if !self.bounds.contains(point) {
            return None;
        }
        let offset = self.offset + (point.y - self.bounds.y) as f64;
        let index = source.index_at(offset);
        if index >= source.discovered() || source.len().is_some_and(|len| index >= len) {
            return None;
        }
        let top = source.offset_of(index);
        (offset >= top && offset < top + source.item_height(index) as f64).then_some(index)
    }
    pub fn reveal(&mut self, source: &impl VariableItemSource, index: usize) -> bool {
        if index >= source.discovered() || source.len().is_some_and(|len| index >= len) {
            return false;
        }
        let top = source.offset_of(index);
        let height = source.item_height(index) as f64;
        if !top.is_finite() || !height.is_finite() || height <= 0.0 {
            return false;
        }
        if top < self.offset || height > self.bounds.height as f64 {
            self.offset = top.max(0.0);
        } else if top + height > self.offset + self.bounds.height as f64 {
            self.offset = (top + height - self.bounds.height as f64).max(0.0);
        }
        self.selected = Some(index);
        true
    }
    pub fn paint(&self, source: &impl VariableItemSource, theme: Theme, ops: &mut Vec<DrawOp>) {
        ops.push(DrawOp::PushClip(self.bounds));
        for row in self.visible(source, 1) {
            if self.selected == Some(row.index) {
                ops.push(DrawOp::Fill(row.bounds, theme.selection));
            }
            text(
                ops,
                row.bounds.x + 10.0,
                row.bounds.y + 6.0,
                source.label(row.index),
                13.0,
                theme.text,
            );
        }
        ops.push(DrawOp::PopClip);
    }
}
