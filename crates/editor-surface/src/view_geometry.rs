// SPDX-License-Identifier: MPL-2.0
//! Per-view caret timing and logical display controls; no timer thread.
use crate::EditorSurface;
use std::time::{Duration, Instant};
pub(crate) struct CaretBlink { focused: bool, visible: bool, deadline: Option<Instant> }
impl Default for CaretBlink {
    fn default() -> Self { Self { focused: true, visible: true, deadline: Some(Instant::now()+Duration::from_millis(530)) } }
}
impl EditorSurface {
    pub(crate) fn resolve_visual_navigation(&mut self, backend: &impl bareline_renderer::TextBackend) -> Result<(), bareline_renderer::LayoutError> {
        use crate::Input;
        use bareline_document::TextOffset;
        use bareline_renderer::Point;
        use unicode_segmentation::UnicodeSegmentation;
        let Some(input) = self.visual_navigation.pop_front() else { return Ok(()); };
        let (down, extend) = match input { Input::Up(extend) => (false, extend), Input::Down(extend) => (true, extend), _ => return Ok(()) };
        let line = self.snapshot.line_at(TextOffset(self.selection.caret)).map_err(|_| bareline_renderer::LayoutError::InvalidOffset)?;
        let Some(layout) = self.layouts.get(&line) else { self.visual_navigation.push_front(input); return Ok(()); };
        let caret = backend.caret(layout.id, self.selection.caret.saturating_sub(layout.start).min(layout.end-layout.start))?;
        let x = *self.preferred_x.get_or_insert(caret.x);
        let mut y = caret.y + if down { self.line_height()*1.5 } else { -self.line_height()*0.5 };
        let height = backend.layout_size(layout.id)?.1;
        let target_line = if y < 0.0 {
            self.logical_line(self.visual_line(line).saturating_sub(1))
        } else if y >= height {
            self.logical_line(self.visual_line(line).saturating_add(self.wrap_rows.get(&line).copied().unwrap_or(1)))
        } else { line };
        let Some(target) = self.layouts.get(&target_line) else { return Ok(()); };
        if target_line != line { y = if down { 0.0 } else { (backend.layout_size(target.id)?.1-self.line_height()*0.5).max(0.0) }; }
        let hit = backend.hit_test(target.id, Point { x, y })?;
        let text = self.snapshot.read(TextOffset(target.start)..TextOffset(target.end), bareline_renderer::MAX_LAYOUT_BYTES).map_err(|_| bareline_renderer::LayoutError::InvalidOffset)?;
        let local = text.grapheme_indices(true).map(|(i,_)|i).chain(Some(text.len())).take_while(|i|*i <= hit.byte_offset).last().unwrap_or(0);
        self.selection.caret = target.start+local;
        if !extend { self.selection.anchor = self.selection.caret; }
        self.selections = self.selection.into();
        self.reveal_caret = true;
        (self.notify)();
        Ok(())
    }
    pub fn set_focused(&mut self, focused: bool) {
        if self.blink.focused == focused { return; }
        self.blink.focused = focused;
        self.reset_caret_blink();
    }
    pub fn reset_caret_blink(&mut self) {
        self.blink.visible = true;
        self.blink.deadline = self.blink.focused.then(|| Instant::now()+Duration::from_millis(530));
    }
    pub fn blink_deadline(&self) -> Option<Instant> { self.blink.deadline }
    pub fn tick_caret_blink(&mut self, now: Instant) -> bool {
        if self.blink.deadline.is_some_and(|deadline| now >= deadline) {
            self.blink.visible = !self.blink.visible;
            self.blink.deadline = Some(now+Duration::from_millis(530));
            true
        } else { false }
    }
    pub(crate) fn caret_visible(&self) -> bool { self.blink.focused && (self.blink.visible || self.composition.is_some()) }
    pub fn zoom_by(&mut self, steps: f32) -> bool {
        if !steps.is_finite() { return false; }
        let size = (self.font_pixels+steps).clamp(8.0, 96.0);
        if size == self.font_pixels { return false; }
        self.font_pixels = size;
        self.layout_revision = None;
        self.wrap_rows.clear();
        self.reveal_caret = true;
        true
    }
    pub fn scroll_horizontal(&mut self, delta: f64) {
        if delta.is_finite() && !self.wrap {
            self.scroll_x = (self.scroll_x+delta).max(0.0);
            self.reveal_caret = false;
        }
    }
    pub fn set_wrap(&mut self, wrap: bool) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.wrap_rows.clear();
            self.layout_revision = None;
            self.scroll_x = 0.0;
            self.reveal_caret = true;
        }
    }
}
