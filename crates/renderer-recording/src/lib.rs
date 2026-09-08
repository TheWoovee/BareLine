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
    wrapped: BTreeMap<LayoutId, usize>,
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
    fn shape_wrapped(&mut self, text: &str, size: f32, width: f32, family: &str) -> Result<LayoutId, LayoutError> {
        let id = self.shape_with_font_family(text, size, width, family)?;
        self.wrapped.insert(id, (width/(size*0.6)).floor().max(1.0) as usize);
        Ok(id)
    }
    fn layout_size(&self, id: LayoutId) -> Result<(f32, f32), LayoutError> {
        let (offsets, size) = self.layouts.get(&id).ok_or(LayoutError::InvalidHandle)?;
        let count = offsets.len()-1;
        let columns = self.wrapped.get(&id).copied().unwrap_or(count.max(1));
        Ok((count.min(columns) as f32*size*0.6, count.max(1).div_ceil(columns) as f32*size*if self.wrapped.contains_key(&id) {1.2} else {1.5}))
    }
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
        let mut column = (p.x / advance)
            .round()
            .clamp(0.0, (offsets.len() - 1) as f32) as usize;
        if let Some(columns) = self.wrapped.get(&layout) {
            column = ((p.y/(size*1.2)).floor().max(0.0) as usize).saturating_mul(*columns).saturating_add(column.min(*columns)).min(offsets.len()-1);
        }
        Ok(TextHit {
            byte_offset: offsets[column],
            inside: p.x >= 0.0
                && p.x <= (offsets.len() - 1) as f32 * advance
                && p.y >= 0.0
                && p.y < self.layout_size(layout)?.1,
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
        let columns = self.wrapped.get(&layout).copied();
        Ok(Rect {
            x: columns.map_or(index, |n| index%n) as f32 * size * 0.6,
            y: columns.map_or(0, |n| index/n) as f32 * size * 1.2,
            width: 1.5,
            height: size * if columns.is_some() {1.2} else {1.5},
        })
    }
    fn release_layout(&mut self, layout: LayoutId) {
        self.wrapped.remove(&layout);
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
        if let Some(columns) = self.wrapped.get(&layout) {
            let (offsets, size) = self.layouts.get(&layout).ok_or(LayoutError::InvalidHandle)?;
            let a = offsets.binary_search(&bytes.start).map_err(|_| LayoutError::InvalidOffset)?;
            let b = offsets.binary_search(&bytes.end).map_err(|_| LayoutError::InvalidOffset)?;
            let mut result = Vec::new();
            let mut cursor = a;
            while cursor < b {
                let next = ((cursor/columns+1)*columns).min(b);
                result.push(Rect { x: (cursor%columns) as f32*size*0.6, y: (cursor/columns) as f32*size*1.2, width: (next-cursor) as f32*size*0.6, height: size*1.2 });
                cursor = next;
            }
            return Ok(result);
        }
        Ok(vec![Rect {
            width: end.x - start.x,
            ..start
        }])
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use bareline_renderer::Image;
    #[test]
    fn frames_cancel_recover_and_reject_crossed_stacks() {
        let mut backend = RecordingBackend::default();
        let image = Image::rgba(1, 1, vec![255, 0, 0, 128]).unwrap();
        let ops = [DrawOp::PushLayer { bounds: Rect::default(), opacity: 0.5 },
            DrawOp::Image { image, destination: Rect::default(), opacity: 1.0 }, DrawOp::PopLayer];
        { let mut frame = backend.begin_frame(); frame.extend(&ops); }
        assert!(backend.operations.is_empty());
        backend.simulate_device_loss();
        let mut frame = backend.begin_frame(); frame.extend(&ops);
        assert_eq!(frame.finish(), Ok(FrameStatus::Recreate));
        let mut frame = backend.begin_frame(); frame.extend(&ops);
        assert_eq!(frame.finish(), Ok(FrameStatus::Presented));
        assert_eq!(backend.operations, ops);
        assert!(backend.render(&[DrawOp::PushClip(Rect::default()), ops[0].clone(), DrawOp::PopClip, DrawOp::PopLayer]).is_err());
        assert_eq!(backend.operations, ops);
        assert!(Image::rgba(u32::MAX, u32::MAX, vec![]).is_err());
    }
}
