// SPDX-License-Identifier: MPL-2.0
//! Bounded single-line field shared by find, palette and settings controls.
use crate::rect;
use bareline_renderer::{DrawOp, LayoutError, LayoutId, Point, Rect, TextBackend};
use unicode_segmentation::UnicodeSegmentation;
const LIMIT: usize = 16 * 1024;
const HISTORY_LIMIT: usize = 256 * 1024;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextMode {
    #[default]
    Text,
    Password,
    Decimal,
}
struct FieldState {
    value: String,
    anchor: usize,
    caret: usize,
}
#[derive(Default)]
pub struct TextField {
    mode: TextMode,
    placeholder: String,
    validation: Option<String>,
    value: String,
    anchor: usize,
    caret: usize,
    composition: Option<(String, usize)>,
    layout: Option<(String, LayoutId)>,
    scroll: f32,
    origin: Point,
    undo: std::collections::VecDeque<FieldState>,
    redo: std::collections::VecDeque<FieldState>,
    history_bytes: usize,
}
impl TextField {
    pub fn mode(&self) -> TextMode {
        self.mode
    }
    pub fn set_mode(&mut self, mode: TextMode) -> bool {
        if mode == TextMode::Decimal && !decimal_input(&self.value) {
            return false;
        }
        self.cancel();
        self.mode = mode;
        // Old history may not satisfy a newly selected filter or privacy mode.
        self.undo.clear();
        self.redo.clear();
        self.history_bytes = 0;
        true
    }
    pub fn set_placeholder(&mut self, value: &str) {
        self.placeholder = value.chars().take(256).collect();
    }
    pub fn set_validation(&mut self, reason: Option<String>) {
        self.validation = reason;
    }
    pub fn validation(&self) -> Option<&str> {
        self.validation.as_deref()
    }
    pub fn semantic_value(&self) -> String {
        self.display_value(&self.value)
    }
    fn display_value(&self, value: &str) -> String {
        if self.mode == TextMode::Password {
            "•".repeat(value.graphemes(true).count())
        } else {
            value.into()
        }
    }
    fn display_offset(&self, value: &str, offset: usize) -> usize {
        if self.mode == TextMode::Password {
            value.grapheme_indices(true).take_while(|(i, _)| *i < offset).count() * "•".len()
        } else {
            offset
        }
    }
    pub fn value(&self) -> &str {
        &self.value
    }
    pub fn composing(&self) -> bool {
        self.composition.is_some()
    }
    /// Committed byte offsets used by the native text provider. Preedit stays
    /// separate until IME commit, matching the field's editing contract.
    pub fn selection(&self) -> (usize, usize) {
        (self.anchor, self.caret)
    }
    pub fn composition_text(&self) -> Option<&str> {
        self.composition.as_ref().map(|(text, _)| text.as_str())
    }
    pub fn set_selection(&mut self, anchor: usize, caret: usize) -> bool {
        if anchor > self.value.len()
            || caret > self.value.len()
            || !self.value.is_char_boundary(anchor)
            || !self.value.is_char_boundary(caret)
        {
            return false;
        }
        self.cancel();
        self.anchor = anchor;
        self.caret = caret;
        true
    }
    fn range(&self) -> std::ops::Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }
    pub fn selected(&self) -> &str {
        if self.mode == TextMode::Password {
            ""
        } else {
            &self.value[self.range()]
        }
    }
    pub fn select_all(&mut self) {
        self.cancel();
        self.anchor = 0;
        self.caret = self.value.len();
    }
    pub fn insert(&mut self, value: &str) -> bool {
        if self.composing() || value.chars().any(char::is_control) {
            return false;
        }
        let range = self.range();
        if self.value.len() - range.len() + value.len() > LIMIT {
            return false;
        }
        if self.mode == TextMode::Decimal {
            let mut candidate = self.value.clone();
            candidate.replace_range(range.clone(), value);
            if !decimal_input(&candidate) {
                return false;
            }
        }
        if range.is_empty() && value.is_empty() {
            return false;
        }
        for state in self.redo.drain(..) {
            self.history_bytes -= state.value.len();
        }
        let before = self.capture();
        self.history_bytes += before.value.len();
        self.undo.push_back(before);
        self.trim_history();
        self.value.replace_range(range.clone(), value);
        self.caret = range.start + value.len();
        self.anchor = self.caret;
        true
    }
    fn capture(&self) -> FieldState {
        FieldState {
            value: self.value.clone(),
            anchor: self.anchor,
            caret: self.caret,
        }
    }
    fn restore(&mut self, state: FieldState) {
        self.value = state.value;
        self.anchor = state.anchor;
        self.caret = state.caret;
    }
    fn trim_history(&mut self) {
        while self.undo.len() + self.redo.len() > 32 || self.history_bytes > HISTORY_LIMIT {
            let oldest = self.undo.pop_front().or_else(|| self.redo.pop_front());
            if let Some(state) = oldest {
                self.history_bytes -= state.value.len();
            } else {
                break;
            }
        }
    }
    pub fn undo(&mut self, redo: bool) {
        if self.composing() {
            return;
        }
        let state = if redo {
            self.redo.pop_back()
        } else {
            self.undo.pop_back()
        };
        if let Some(state) = state {
            self.history_bytes -= state.value.len();
            let current = self.capture();
            self.history_bytes += current.value.len();
            if redo {
                self.undo.push_back(current);
            } else {
                self.redo.push_back(current);
            }
            self.restore(state);
            self.trim_history();
        }
    }
    pub fn cancel(&mut self) {
        self.composition = None;
    }
    pub fn preedit(&mut self, value: String, cursor: Option<(usize, usize)>) {
        if value.is_empty() {
            self.cancel();
            return;
        }
        if value.chars().any(char::is_control) || self.value.len() - self.range().len() + value.len() > LIMIT {
            return;
        }
        let caret = cursor
            .filter(|(a, b)| a <= b && value.is_char_boundary(*a) && value.is_char_boundary(*b))
            .map_or(value.len(), |(_, b)| b);
        self.composition = Some((value, caret));
    }
    pub fn commit(&mut self, value: &str) -> bool {
        self.cancel();
        self.insert(value)
    }
    pub fn horizontal(&mut self, right: bool, extend: bool) {
        if self.composing() {
            return;
        }
        if !extend && !self.range().is_empty() {
            self.caret = if right { self.range().end } else { self.range().start };
        } else if right {
            self.caret += self.value[self.caret..].graphemes(true).next().map_or(0, str::len);
        } else {
            self.caret = self.value[..self.caret]
                .grapheme_indices(true)
                .next_back()
                .map_or(0, |(i, _)| i);
        }
        if !extend {
            self.anchor = self.caret;
        }
    }
    pub fn edge(&mut self, end: bool, extend: bool) {
        if self.composing() {
            return;
        }
        self.caret = if end { self.value.len() } else { 0 };
        if !extend {
            self.anchor = self.caret;
        }
    }
    pub fn delete(&mut self, forward: bool) -> bool {
        if self.composing() {
            return false;
        }
        if self.range().is_empty() {
            self.horizontal(forward, true);
        }
        self.insert("")
    }
    pub fn release(&mut self, backend: &mut impl TextBackend) {
        if let Some((_, id)) = self.layout.take() {
            backend.release_layout(id);
        }
    }
    pub fn click(&mut self, backend: &impl TextBackend, point: Point, extend: bool) -> Result<(), LayoutError> {
        if self.composing() {
            return Ok(());
        }
        if let Some((value, id)) = &self.layout {
            if value != &self.display_value(&self.value) {
                return Ok(());
            }
            let hit = backend.hit_test(
                *id,
                Point {
                    x: point.x - self.origin.x,
                    y: point.y - self.origin.y,
                },
            )?;
            let hit_offset = if self.mode == TextMode::Password {
                self.value
                    .grapheme_indices(true)
                    .nth(hit.byte_offset / "•".len())
                    .map_or(self.value.len(), |(i, _)| i)
            } else {
                hit.byte_offset
            };
            self.caret = self
                .value
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .chain(Some(self.value.len()))
                .take_while(|i| *i <= hit_offset)
                .last()
                .unwrap_or(0);
            if !extend {
                self.anchor = self.caret;
            }
        }
        Ok(())
    }
    pub fn draw(
        &mut self,
        backend: &mut impl TextBackend,
        bounds: Rect,
        focused: bool,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Rect, LayoutError> {
        self.draw_with_theme(backend, bounds, focused, crate::theme::UiTheme::default(), ops)
    }
    pub fn draw_with_theme(
        &mut self,
        backend: &mut impl TextBackend,
        bounds: Rect,
        focused: bool,
        theme: crate::theme::UiTheme,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Rect, LayoutError> {
        let range = self.range();
        let mut display = self.value.clone();
        let caret_offset = if let Some((preedit, cursor)) = &self.composition {
            display.replace_range(range.clone(), preedit);
            range.start + cursor
        } else {
            self.caret
        };
        let caret_offset = self.display_offset(&display, caret_offset);
        let selected_range = self.display_offset(&self.value, range.start)..self.display_offset(&self.value, range.end);
        let preedit_range = self.composition.as_ref().map(|(preedit, _)| {
            self.display_offset(&display, range.start)..self.display_offset(&display, range.start + preedit.len())
        });
        let display = self.display_value(&display);
        if self.layout.as_ref().is_none_or(|(value, _)| value != &display) {
            self.release(backend);
            self.layout = Some((display.clone(), backend.shape(&display, 13.0, 1_000_000.0)?));
        }
        let id = self.layout.as_ref().unwrap().1;
        let mut caret = backend.caret(id, caret_offset)?;
        let available = (bounds.width - 16.0).max(1.0);
        if caret.x < self.scroll {
            self.scroll = caret.x;
        }
        if caret.x + caret.width > self.scroll + available {
            self.scroll = caret.x + caret.width - available;
        }
        self.scroll = self.scroll.max(0.0);
        self.origin = Point {
            x: bounds.x + 8.0 - self.scroll,
            y: bounds.y + (bounds.height - caret.height) / 2.0,
        };
        let selected = if self.composition.is_none() && !range.is_empty() {
            backend.range_rects(id, selected_range)?
        } else {
            Vec::new()
        };
        let underlines = if let Some(preedit_range) = preedit_range {
            backend.range_rects(id, preedit_range)?
        } else {
            Vec::new()
        };
        ops.push(DrawOp::FillRounded(bounds, theme.editor, 4.0));
        ops.push(DrawOp::StrokeRounded(
            bounds,
            if focused { theme.focus } else { theme.interactive },
            4.0,
            if focused { 2.0 } else { 1.0 },
        ));
        ops.push(DrawOp::PushClip(rect(
            bounds.x + 3.0,
            bounds.y + 2.0,
            bounds.width - 6.0,
            bounds.height - 4.0,
        )));
        for selected in selected {
            ops.push(DrawOp::Fill(
                rect(
                    self.origin.x + selected.x,
                    self.origin.y + selected.y,
                    selected.width,
                    selected.height,
                ),
                theme.selection,
            ));
        }
        ops.push(DrawOp::Layout {
            origin: self.origin,
            layout: id,
            color: theme.text,
        });
        if self.value.is_empty() && self.composition.is_none() && !self.placeholder.is_empty() {
            crate::text(ops, self.origin.x, self.origin.y, &self.placeholder, 13.0, theme.muted);
        }
        for line in underlines {
            ops.push(DrawOp::Line {
                from: Point {
                    x: self.origin.x + line.x,
                    y: self.origin.y + line.y + line.height,
                },
                to: Point {
                    x: self.origin.x + line.x + line.width,
                    y: self.origin.y + line.y + line.height,
                },
                color: theme.focus,
                width: 1.0,
            });
        }
        caret.x += self.origin.x;
        caret.y += self.origin.y;
        if focused {
            ops.push(DrawOp::Fill(caret, theme.caret));
        }
        ops.push(DrawOp::PopClip);
        Ok(caret)
    }
}

fn decimal_input(value: &str) -> bool {
    let unsigned = value.strip_prefix(['-', '+']).unwrap_or(value);
    let mut dot = false;
    unsigned.chars().all(|c| {
        if c == '.' && !dot {
            dot = true;
            true
        } else {
            c.is_ascii_digit()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_composition_and_horizontal_viewport_preserve_graphemes() {
        let mut field = TextField::default();
        field.insert("a👩🏽‍💻e\u{301}");
        field.delete(false);
        assert_eq!(field.value(), "a👩🏽‍💻");
        field.select_all();
        field.preedit("候補".into(), Some((0, 3)));
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        let mut ops = Vec::new();
        let caret = field
            .draw(&mut backend, rect(10.0, 20.0, 40.0, 28.0), true, &mut ops)
            .unwrap();
        assert!(caret.x >= 10.0 && caret.x <= 50.0);
        assert!(bareline_renderer::balanced_clips(&ops));
        field.cancel();
        assert_eq!(field.selected(), "a👩🏽‍💻");
        field.commit("文");
        assert_eq!(field.value(), "文");
        field.undo(false);
        assert_eq!(field.selected(), "a👩🏽‍💻");
        field.undo(true);
        assert_eq!(field.value(), "文");
        assert!(!field.insert("\n"));
        assert!(!field.insert(&"x".repeat(LIMIT)));
        field.release(&mut backend);
    }
}
