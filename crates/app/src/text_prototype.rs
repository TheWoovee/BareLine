// SPDX-License-Identifier: MPL-2.0
//! PR-001 bounded input experiment, deliberately separate from the PR-003 editor.
use bareline_renderer::{DrawOp, LayoutError, LayoutId, Point, Rect, TextBackend};
use bareline_ui::{ACCENT, EDITOR, TEXT, rect};
use unicode_segmentation::UnicodeSegmentation;

const LIMIT: usize = 16 * 1024;
pub const ORIGIN: Point = Point { x: 64.0, y: 38.0 };
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composition {
    pub text: String,
    pub cursor: Option<(usize, usize)>,
}
#[derive(Default)]
pub struct TextPrototype {
    text: String,
    caret: usize,
    composition: Option<Composition>,
    shaped: Option<(String, u32, LayoutId)>,
}
impl TextPrototype {
    pub fn mixed_script() -> Self {
        let text = "Bareline — مرحباً بالعالم 👩🏽‍💻 café".to_string();
        Self {
            caret: text.len(),
            text,
            ..Self::default()
        }
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn composing(&self) -> bool {
        self.composition.is_some()
    }
    pub fn preedit(&mut self, text: String, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            self.composition = None;
            return;
        }
        if text.len() + self.text.len() > LIMIT || text.contains(['\r', '\n']) {
            return;
        }
        let cursor =
            cursor.filter(|&(start, end)| start <= end && text.is_char_boundary(start) && text.is_char_boundary(end));
        self.composition = Some(Composition { text, cursor });
    }
    pub fn cancel(&mut self) {
        self.composition = None;
    }
    pub fn insert(&mut self, text: &str) {
        if self.composing() || text.chars().any(char::is_control) || self.text.len() + text.len() > LIMIT {
            return;
        }
        self.text.insert_str(self.caret, text);
        self.caret += text.len();
    }
    pub fn commit(&mut self, text: &str) {
        self.cancel();
        self.insert(text);
    }
    pub fn left(&mut self) {
        if self.composing() {
            return;
        }
        self.caret = self.text[..self.caret]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i);
    }
    pub fn right(&mut self) {
        if self.composing() {
            return;
        }
        if let Some(g) = self.text[self.caret..].graphemes(true).next() {
            self.caret += g.len();
        }
    }
    pub fn home(&mut self) {
        if !self.composing() {
            self.caret = 0;
        }
    }
    pub fn end(&mut self) {
        if !self.composing() {
            self.caret = self.text.len();
        }
    }
    pub fn backspace(&mut self) {
        if self.composing() {
            return;
        }
        let end = self.caret;
        self.left();
        self.text.replace_range(self.caret..end, "");
    }
    pub fn delete(&mut self) {
        if self.composing() {
            return;
        }
        let start = self.caret;
        self.right();
        self.text.replace_range(start..self.caret, "");
        self.caret = start;
    }
    fn display(&self) -> String {
        let mut text = self.text.clone();
        if let Some(c) = &self.composition {
            text.insert_str(self.caret, &c.text);
        }
        text
    }
    pub fn click(&mut self, backend: &impl TextBackend, point: Point) -> Result<(), LayoutError> {
        if self.composing() {
            return Ok(());
        }
        if let Some((_, _, layout)) = self.shaped {
            let hit = backend.hit_test(
                layout,
                Point {
                    x: point.x - ORIGIN.x,
                    y: point.y - ORIGIN.y,
                },
            )?;
            // DirectWrite reports shaping clusters. Snap to a grapheme boundary for edits.
            self.caret = self
                .text
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .chain(Some(self.text.len()))
                .take_while(|i| *i <= hit.byte_offset)
                .last()
                .unwrap_or(0);
        }
        Ok(())
    }
    pub fn draw(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        operations: &mut Vec<DrawOp>,
    ) -> Result<Rect, LayoutError> {
        let display = self.display();
        let wrap_width = (width - ORIGIN.x - 18.0).max(1.0);
        if self
            .shaped
            .as_ref()
            .is_none_or(|(s, w, _)| s != &display || *w != wrap_width.to_bits())
        {
            if let Some((_, _, id)) = self.shaped.take() {
                backend.release_layout(id);
            }
            self.shaped = Some((
                display.clone(),
                wrap_width.to_bits(),
                backend.shape(&display, 16.0, wrap_width)?,
            ));
        }
        let id = self.shaped.as_ref().unwrap().2;
        let offset = self.caret
            + self
                .composition
                .as_ref()
                .map_or(0, |c| c.cursor.map_or(c.text.len(), |(_, end)| end));
        let mut caret = backend.caret(id, offset)?;
        caret.x += ORIGIN.x;
        caret.y += ORIGIN.y;
        operations.push(DrawOp::PushClip(rect(49.0, 34.0, width - 65.0, 30.0)));
        operations.push(DrawOp::Fill(rect(49.0, 34.0, width - 65.0, 30.0), EDITOR));
        operations.push(DrawOp::Layout {
            origin: ORIGIN,
            layout: id,
            color: TEXT,
        });
        if let Some(c) = &self.composition {
            for bounds in backend.range_rects(id, self.caret..self.caret + c.text.len())? {
                operations.push(DrawOp::Line {
                    from: Point {
                        x: ORIGIN.x + bounds.x,
                        y: ORIGIN.y + bounds.y + bounds.height,
                    },
                    to: Point {
                        x: ORIGIN.x + bounds.x + bounds.width,
                        y: ORIGIN.y + bounds.y + bounds.height,
                    },
                    color: ACCENT,
                    width: 1.0,
                });
            }
        }
        operations.push(DrawOp::Fill(caret, ACCENT));
        operations.push(DrawOp::PopClip);
        Ok(caret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composition_is_overlay_and_grapheme_edits_are_atomic() {
        let mut p = TextPrototype::default();
        p.insert("a👩🏽‍💻e\u{301}");
        p.backspace();
        assert_eq!(p.text(), "a👩🏽‍💻");
        p.preedit("مرحبا".into(), Some((0, 10)));
        assert_eq!(p.text(), "a👩🏽‍💻");
        p.cancel();
        assert_eq!(p.display(), p.text());
        p.preedit("مرحبا".into(), None);
        p.commit("ب");
        assert_eq!(p.text(), "a👩🏽‍💻ب");
        p.backspace();
        p.backspace();
        assert_eq!(p.text(), "a");
        p.insert(&"x".repeat(LIMIT));
        assert_eq!(p.text(), "a");
    }
}
