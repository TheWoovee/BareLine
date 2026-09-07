// SPDX-License-Identifier: MPL-2.0
use bareline_renderer::{
    DrawOp, FrameStatus, LayoutError, LayoutId, MAX_LAYOUT_BYTES, MAX_LAYOUTS, Point, Rect,
    RenderBackend, TextBackend, TextHit, balanced_clips,
};
use std::collections::BTreeMap;
#[derive(Default)]
pub struct RecordingBackend {
    pub operations: Vec<DrawOp>,
    pub size: (u32, u32),
    pub scale: f32,
    lost: bool,
    layouts: BTreeMap<LayoutId, (Vec<usize>, f32)>,
    pub styles: BTreeMap<LayoutId, Vec<bareline_renderer::TextStyle>>,
}
impl RecordingBackend {
    pub fn simulate_device_loss(&mut self) {
        self.lost = true;
    }
    pub fn measure(&self, text: &str, font_size: f32) -> (f32, f32) {
        (
            text.chars().count() as f32 * font_size * 0.6,
            font_size * 1.5,
        )
    }
}
impl RenderBackend for RecordingBackend {
    type Error = LayoutError;
    fn resize(&mut self, width: u32, height: u32, scale: f32) -> Result<(), Self::Error> {
        self.size = (width, height);
        self.scale = scale;
        Ok(())
    }
    fn render(&mut self, operations: &[DrawOp]) -> Result<FrameStatus, Self::Error> {
        if !balanced_clips(operations) {
            return Err(LayoutError::BackendFailure);
        }
        for op in operations {
            if let DrawOp::Layout { layout, .. } = op
                && !self.layouts.contains_key(layout)
            {
                return Err(LayoutError::InvalidHandle);
            }
        }
        if std::mem::take(&mut self.lost) {
            return Ok(FrameStatus::Recreate);
        }
        self.operations.clear();
        self.operations.extend_from_slice(operations);
        Ok(FrameStatus::Presented)
    }
}
impl TextBackend for RecordingBackend {
    fn set_styles(
        &mut self,
        id: LayoutId,
        styles: &[bareline_renderer::TextStyle],
    ) -> Result<(), LayoutError> {
        let (offsets, _) = self.layouts.get(&id).ok_or(LayoutError::InvalidHandle)?;
        let mut previous = 0;
        for style in styles {
            if style.bytes.start < previous
                || style.bytes.start >= style.bytes.end
                || offsets.binary_search(&style.bytes.start).is_err()
                || offsets.binary_search(&style.bytes.end).is_err()
            {
                return Err(LayoutError::InvalidOffset);
            }
            previous = style.bytes.end;
        }
        self.styles.insert(id, styles.to_vec());
        Ok(())
    }
    fn shape(&mut self, text: &str, size: f32, width: f32) -> Result<LayoutId, LayoutError> {
        if text.len() > MAX_LAYOUT_BYTES
            || self.layouts.len() >= MAX_LAYOUTS
            || !size.is_finite()
            || size <= 0.0
            || !width.is_finite()
            || width <= 0.0
        {
            return Err(LayoutError::ResourceLimit);
        }
        let id = LayoutId::allocate();
        let offsets = text
            .char_indices()
            .map(|(i, _)| i)
            .chain(Some(text.len()))
            .collect();
        self.layouts.insert(id, (offsets, size));
        Ok(id)
    }
    fn hit_test(&self, layout: LayoutId, p: Point) -> Result<TextHit, LayoutError> {
        let (offsets, size) = self
            .layouts
            .get(&layout)
            .ok_or(LayoutError::InvalidHandle)?;
        let advance = size * 0.6;
        let column = (p.x / advance)
            .round()
            .clamp(0.0, (offsets.len() - 1) as f32) as usize;
        Ok(TextHit {
            byte_offset: offsets[column],
            inside: p.x >= 0.0
                && p.x <= (offsets.len() - 1) as f32 * advance
                && p.y >= 0.0
                && p.y < size * 1.5,
            trailing: (p.x / advance).fract() >= 0.5,
        })
    }
    fn caret(&self, layout: LayoutId, offset: usize) -> Result<Rect, LayoutError> {
        let (offsets, size) = self
            .layouts
            .get(&layout)
            .ok_or(LayoutError::InvalidHandle)?;
        let index = offsets
            .binary_search(&offset)
            .map_err(|_| LayoutError::InvalidOffset)?;
        Ok(Rect {
            x: index as f32 * size * 0.6,
            y: 0.0,
            width: 1.5,
            height: size * 1.5,
        })
    }
    fn release_layout(&mut self, layout: LayoutId) {
        self.styles.remove(&layout);
        self.layouts.remove(&layout);
    }
    fn range_rects(
        &self,
        layout: LayoutId,
        bytes: std::ops::Range<usize>,
    ) -> Result<Vec<Rect>, LayoutError> {
        if bytes.start > bytes.end {
            return Err(LayoutError::InvalidOffset);
        }
        let start = self.caret(layout, bytes.start)?;
        let end = self.caret(layout, bytes.end)?;
        Ok(vec![Rect {
            width: end.x - start.x,
            ..start
        }])
    }
}
