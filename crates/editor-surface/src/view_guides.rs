// SPDX-License-Identifier: MPL-2.0
//! View symbols painted over shaped lines: whitespace and line-ending marks,
//! indent guides, the edge line and the bracket pair at the caret (BIZ-07).
//! Each reads a bounded window of the snapshot; none edits text or history.
use crate::{EditorSurface, Input};
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_renderer::{Color, DrawOp, LayoutId, MAX_LAYOUT_BYTES, Point, Rect, TextBackend};
use bareline_ui::{rect, text};
use std::ops::Range;

/// Farthest a matching bracket is looked for, in bytes from the bracket at the
/// caret. The scan runs on the UI thread, so a partner farther away is not
/// highlighted and Go to Matching Brace reports that none was found.
pub const MAX_BRACE_DISTANCE: usize = 64 * 1024;
/// Whitespace runs marked per frame, so a screen of alternating spaces cannot
/// turn one paint into thousands of hit tests.
pub(crate) const MAX_WHITESPACE_RUNS: usize = 4096;

/// View > Show symbol and edge preferences for one surface; all off by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewGuides {
    pub end_of_line: bool,
    pub indent_guides: bool,
    /// Column of the vertical edge line; `None` hides it.
    pub edge_column: Option<usize>,
}

/// The last bracket scan and what it was keyed on: the snapshot's identity,
/// content state and length, the caret, and whether `<>` pair.
pub(crate) type BraceCache = (
    ((u64, u64), bareline_document::ContentStateId, usize, usize, bool),
    Option<(usize, usize)>,
);

/// `<` and `>` pair only in markup, where they delimit tags; elsewhere they are
/// comparison operators.
pub fn is_markup(language: bareline_syntax::Language) -> bool {
    matches!(
        language,
        bareline_syntax::Language::Html | bareline_syntax::Language::Xml
    )
}

/// The partner of a bracket byte and whether `byte` opens the pair.
fn bracket(byte: u8, markup: bool) -> Option<(u8, bool)> {
    Some(match byte {
        b'(' => (b')', true),
        b')' => (b'(', false),
        b'[' => (b']', true),
        b']' => (b'[', false),
        b'{' => (b'}', true),
        b'}' => (b'{', false),
        b'<' if markup => (b'>', true),
        b'>' if markup => (b'<', false),
        _ => return None,
    })
}

/// Index of the bracket that closes `text[0]`, counting nesting. Brackets are
/// ASCII, so scanning bytes never lands inside a multi-byte character.
pub fn scan_forward(text: &[u8], open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (index, &byte) in text.iter().enumerate() {
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

/// Index of the bracket that opens the last byte of `text`, counting nesting.
pub fn scan_backward(text: &[u8], open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (index, &byte) in text.iter().enumerate().rev() {
        if byte == close {
            depth += 1;
        } else if byte == open {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

/// The bracket beside `caret` and its partner, as document offsets
/// `(bracket, partner)`. As in Notepad++, the character before the caret is
/// tried first. Strings and comments are not special. The partner must lie
/// within [`MAX_BRACE_DISTANCE`] bytes; otherwise there is no match.
pub fn matching_brace(snapshot: &DocumentSnapshot, caret: usize, markup: bool) -> Option<(usize, usize)> {
    let len = snapshot.len();
    let byte_at = |offset: usize| -> Option<u8> {
        let end = offset.checked_add(1).filter(|end| *end <= len)?;
        if !snapshot.is_boundary(TextOffset(offset)) || !snapshot.is_boundary(TextOffset(end)) {
            return None;
        }
        snapshot
            .read(TextOffset(offset)..TextOffset(end), 1)
            .ok()?
            .bytes()
            .next()
    };
    for at in caret.checked_sub(1).into_iter().chain(std::iter::once(caret)) {
        let Some(byte) = byte_at(at) else {
            continue;
        };
        let Some((partner, opens)) = bracket(byte, markup) else {
            continue;
        };
        let found = if opens {
            let mut end = len.min(at.saturating_add(MAX_BRACE_DISTANCE + 1));
            while end > at + 1 && !snapshot.is_boundary(TextOffset(end)) {
                end -= 1;
            }
            let window = snapshot
                .read(TextOffset(at)..TextOffset(end), MAX_BRACE_DISTANCE + 1)
                .ok()?;
            scan_forward(window.as_bytes(), byte, partner).map(|index| at + index)
        } else {
            let mut start = (at + 1).saturating_sub(MAX_BRACE_DISTANCE + 1);
            while start < at && !snapshot.is_boundary(TextOffset(start)) {
                start += 1;
            }
            let window = snapshot
                .read(TextOffset(start)..TextOffset(at + 1), MAX_BRACE_DISTANCE + 1)
                .ok()?;
            scan_backward(window.as_bytes(), partner, byte).map(|index| start + index)
        };
        return found.map(|partner| (at, partner));
    }
    None
}

/// Byte offsets in `line` where an indent guide is drawn: every indentation
/// stop (a multiple of `tab_width`) that more indentation follows, as Scintilla
/// draws them. A stop inside a tab has no character of its own and is skipped.
pub fn indent_guide_offsets(line: &str, tab_width: usize) -> Vec<usize> {
    let tab_width = tab_width.max(1);
    let mut column = 0usize;
    let mut offsets = Vec::new();
    for (index, byte) in line.bytes().enumerate() {
        let width = match byte {
            b' ' => 1,
            b'\t' => tab_width - column % tab_width,
            _ => break,
        };
        if column > 0 && column.is_multiple_of(tab_width) {
            offsets.push(index);
        }
        column += width;
    }
    offsets
}

/// Runs of spaces, and single tabs (`true`), as byte ranges in `line`.
pub fn whitespace_runs(line: &str) -> Vec<(Range<usize>, bool)> {
    let bytes = line.as_bytes();
    let mut runs = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b' ' => {
                let start = index;
                while index < bytes.len() && bytes[index] == b' ' {
                    index += 1;
                }
                runs.push((start..index, false));
            }
            b'\t' => {
                runs.push((index..index + 1, true));
                index += 1;
            }
            _ => index += 1,
        }
    }
    runs
}

/// The mark shown after a line for its terminator.
pub fn eol_label(terminator: &str) -> Option<&'static str> {
    match terminator {
        "\r\n" => Some("CRLF"),
        "\n" => Some("LF"),
        "\r" => Some("CR"),
        _ => None,
    }
}

/// Where the edge line at `column` is painted, or `None` while it is scrolled
/// out of the text area. `advance` is the width of one space.
pub fn edge_line_x(text_left: f32, scroll_x: f64, column: usize, advance: f32, width: f32) -> Option<f32> {
    if column == 0 || !advance.is_finite() || advance <= 0.0 {
        return None;
    }
    let x = text_left + (column as f64 * f64::from(advance) - scroll_x) as f32;
    (x.is_finite() && x >= text_left && x < width).then_some(x)
}

/// One shaped line as painted: `x`/`y` is the layout origin on screen.
pub(crate) struct ShapedLine {
    pub layout: LayoutId,
    /// Document offsets of the layout's first byte and end.
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub x: f32,
    pub y: f32,
}

impl EditorSurface {
    pub fn set_view_guides(&mut self, guides: ViewGuides) {
        self.guides = guides;
    }
    pub fn view_guides(&self) -> ViewGuides {
        self.guides
    }
    /// The bracket beside the caret and its partner; see [`matching_brace`].
    pub fn matching_brace(&self) -> Option<(usize, usize)> {
        matching_brace(&self.snapshot, self.selection.caret, is_markup(self.language))
    }
    /// [`Self::matching_brace`] for painting: a repaint with the same text,
    /// caret and language, such as a caret blink, reuses the last scan.
    pub(crate) fn cached_matching_brace(&mut self) -> Option<(usize, usize)> {
        let key = (
            self.snapshot.identity_token(),
            self.snapshot.content_state,
            self.snapshot.len(),
            self.selection.caret,
            is_markup(self.language),
        );
        if let Some((cached, found)) = self.brace_cache
            && cached == key
        {
            return found;
        }
        let found = self.matching_brace();
        self.brace_cache = Some((key, found));
        found
    }
    /// Caret inputs for Go to Matching Brace (the caret lands before the
    /// partner) or Select to Matching Brace (both brackets included), as in
    /// Notepad++. `None` when the caret is not beside a matched bracket.
    pub fn matching_brace_inputs(&self, select: bool) -> Option<Vec<Input>> {
        let (at, partner) = self.matching_brace()?;
        Some(if select {
            vec![
                Input::SetCaret(at.min(partner), false),
                Input::SetCaret(at.max(partner) + 1, true),
            ]
        } else {
            vec![Input::SetCaret(partner, false)]
        })
    }
    fn symbol_color(&self) -> Color {
        crate::estimated_gutter(self.theme.gutter, self.theme.ui.editor)
    }
    /// Outline the matched brackets that fall inside this shaped line.
    pub(crate) fn draw_brace_marks(
        &self,
        backend: &impl TextBackend,
        ops: &mut Vec<DrawOp>,
        shaped: &ShapedLine,
        braces: (usize, usize),
    ) {
        for at in [braces.0, braces.1] {
            if !(shaped.start..shaped.end).contains(&at) {
                continue;
            }
            let Ok(rects) = backend.range_rects(shaped.layout, at - shaped.start..at - shaped.start + 1) else {
                continue;
            };
            for r in rects {
                ops.push(DrawOp::Stroke(
                    rect(shaped.x + r.x, shaped.y + r.y, r.width, r.height),
                    self.theme.ui.focus,
                    1.0,
                ));
            }
        }
    }
    /// Whitespace, line-ending and indent-guide marks for a line whose layout
    /// holds all of its text. `budget` bounds the whitespace runs per frame.
    pub(crate) fn draw_line_symbols(
        &self,
        backend: &impl TextBackend,
        ops: &mut Vec<DrawOp>,
        shaped: &ShapedLine,
        budget: &mut usize,
    ) {
        let whitespace_all = self.whitespace == "all";
        let whitespace_selected = self.whitespace == "selection";
        if !whitespace_all && !whitespace_selected && !self.guides.end_of_line && !self.guides.indent_guides {
            return;
        }
        let Ok(line) = self
            .snapshot
            .read(TextOffset(shaped.start)..TextOffset(shaped.end), MAX_LAYOUT_BYTES)
        else {
            return;
        };
        let color = self.symbol_color();
        let line_height = self.line_height();
        if self.guides.indent_guides {
            for offset in indent_guide_offsets(&line, self.tab_width) {
                if let Ok(caret) = backend.caret(shaped.layout, offset) {
                    let x = shaped.x + caret.x;
                    let top = shaped.y + caret.y;
                    ops.push(DrawOp::Line {
                        from: Point { x, y: top },
                        to: Point {
                            x,
                            y: top + line_height,
                        },
                        color,
                        width: 1.0,
                    });
                }
            }
        }
        let shown: Vec<Range<usize>> = if whitespace_all {
            vec![Range {
                start: 0,
                end: line.len(),
            }]
        } else if whitespace_selected {
            self.selection_set()
                .selections
                .iter()
                .filter_map(|selection| {
                    let a = selection.anchor.min(selection.caret).max(shaped.start);
                    let b = selection.anchor.max(selection.caret).min(shaped.end);
                    (a < b).then(|| a - shaped.start..b - shaped.start)
                })
                .collect()
        } else {
            Vec::new()
        };
        if !shown.is_empty() {
            'runs: for (run, tab) in whitespace_runs(&line) {
                for part in &shown {
                    let bytes = run.start.max(part.start)..run.end.min(part.end);
                    if bytes.is_empty() {
                        continue;
                    }
                    if *budget == 0 {
                        break 'runs;
                    }
                    *budget -= 1;
                    self.mark_whitespace(backend, ops, shaped, bytes, tab, color);
                }
            }
        }
        if self.guides.end_of_line
            && let Ok(range) = self.snapshot.line_range(shaped.line)
            && range.end.0 > shaped.end
            && let Ok(terminator) = self.snapshot.read(TextOffset(shaped.end)..range.end, 2)
            && let Some(label) = eol_label(&terminator)
            && let Ok(caret) = backend.caret(shaped.layout, shaped.end - shaped.start)
        {
            let size = (self.font_pixels * 0.6).max(8.0);
            text(
                ops,
                shaped.x + caret.x + self.font_pixels * 0.25,
                shaped.y + caret.y + (line_height - size * 1.2).max(0.0) / 2.0,
                label,
                size,
                color,
            );
        }
    }
    /// A centred dot per space, or an arrow across a tab.
    fn mark_whitespace(
        &self,
        backend: &impl TextBackend,
        ops: &mut Vec<DrawOp>,
        shaped: &ShapedLine,
        bytes: Range<usize>,
        tab: bool,
        color: Color,
    ) {
        let count = bytes.len();
        let Ok(rects) = backend.range_rects(shaped.layout, bytes) else {
            return;
        };
        let total: f32 = rects.iter().map(|r| r.width).sum();
        if count == 0 || !total.is_finite() || total <= 0.0 {
            return;
        }
        let advance = total / count as f32;
        let dot = (self.font_pixels / 8.0).clamp(1.5, 3.0);
        for r in rects {
            let middle = shaped.y + r.y + r.height / 2.0;
            if tab {
                if r.width <= 6.0 {
                    continue;
                }
                let left = shaped.x + r.x + 2.0;
                let right = shaped.x + r.x + r.width - 2.0;
                for (from, to) in [
                    ((left, middle), (right, middle)),
                    ((right, middle), (right - 3.0, middle - 3.0)),
                    ((right, middle), (right - 3.0, middle + 3.0)),
                ] {
                    ops.push(DrawOp::Line {
                        from: Point { x: from.0, y: from.1 },
                        to: Point { x: to.0, y: to.1 },
                        color,
                        width: 1.0,
                    });
                }
            } else {
                let dots = ((r.width / advance).round() as usize).min(count);
                for index in 0..dots {
                    let centre = shaped.x + r.x + advance * (index as f32 + 0.5);
                    ops.push(DrawOp::Fill(
                        rect(centre - dot / 2.0, middle - dot / 2.0, dot, dot),
                        color,
                    ));
                }
            }
        }
    }
    /// The vertical edge line across the text body, when it is in view.
    pub(crate) fn draw_edge_line(&self, backend: &mut impl TextBackend, ops: &mut Vec<DrawOp>, body: Rect) {
        let Some(column) = self.guides.edge_column else {
            return;
        };
        let Ok(space) = backend.shape_with_font_family(" ", self.font_pixels, 1_000_000.0, &self.font_family) else {
            return;
        };
        let advance = backend.layout_size(space).map(|size| size.0);
        backend.release_layout(space);
        let Ok(advance) = advance else {
            return;
        };
        if let Some(x) = edge_line_x(self.text_left(), self.scroll_x, column, advance, body.x + body.width) {
            ops.push(DrawOp::Line {
                from: Point { x, y: body.y },
                to: Point {
                    x,
                    y: body.y + body.height,
                },
                color: self.symbol_color(),
                width: 1.0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    use bareline_renderer_recording::RecordingBackend;
    use std::sync::Arc;

    fn snapshot(text: &str) -> DocumentSnapshot {
        Document::from_utf8(text, Budget::new(1 << 20), Budget::new(1 << 20))
            .unwrap()
            .snapshot()
    }

    #[test]
    fn nested_brackets_match_and_the_character_before_the_caret_wins() {
        let text = snapshot("f(a[b]{c(d)}e)");
        assert_eq!(matching_brace(&text, 1, false), Some((1, 13)));
        assert_eq!(matching_brace(&text, 14, false), Some((13, 1)));
        assert_eq!(matching_brace(&text, 4, false), Some((3, 5)));
        assert_eq!(matching_brace(&text, 9, false), Some((8, 10)));
        assert_eq!(matching_brace(&text, 11, false), Some((10, 8)));
        // Between `]` and `{`: the bracket before the caret is used.
        assert_eq!(matching_brace(&text, 6, false), Some((5, 3)));
        assert_eq!(matching_brace(&text, 0, false), None);
        assert_eq!(matching_brace(&text, 3, false), Some((3, 5)));
    }

    #[test]
    fn unbalanced_brackets_and_non_ascii_neighbours() {
        assert_eq!(matching_brace(&snapshot("((a)"), 0, false), None);
        assert_eq!(matching_brace(&snapshot("(a))"), 4, false), None);
        // The character before the caret is `é` (two bytes), so the `(` after
        // the caret is used.
        assert_eq!(matching_brace(&snapshot("é(x)"), 2, false), Some((2, 4)));
        assert_eq!(matching_brace(&snapshot("x"), 5, false), None);
        assert_eq!(matching_brace(&snapshot(""), 0, false), None);
    }

    #[test]
    fn strings_are_not_special_and_angle_brackets_pair_only_in_markup() {
        // No lexer is consulted: a bracket inside a string still counts.
        assert_eq!(matching_brace(&snapshot("(\")\")"), 0, false), Some((0, 2)));
        let tag = snapshot("<a>x</a>");
        assert_eq!(matching_brace(&tag, 0, true), Some((0, 2)));
        assert_eq!(matching_brace(&tag, 0, false), None);
        assert!(is_markup(bareline_syntax::Language::Html));
        assert!(!is_markup(bareline_syntax::Language::Rust));
    }

    #[test]
    fn brace_scan_is_bounded_by_distance() {
        let inside = format!("({})", "x".repeat(MAX_BRACE_DISTANCE - 1));
        let text = snapshot(&inside);
        assert_eq!(matching_brace(&text, 0, false), Some((0, MAX_BRACE_DISTANCE)));
        assert_eq!(
            matching_brace(&text, MAX_BRACE_DISTANCE + 1, false),
            Some((MAX_BRACE_DISTANCE, 0))
        );
        let beyond = format!("({})", "x".repeat(MAX_BRACE_DISTANCE));
        let text = snapshot(&beyond);
        assert_eq!(matching_brace(&text, 0, false), None);
        assert_eq!(matching_brace(&text, MAX_BRACE_DISTANCE + 2, false), None);
    }

    #[test]
    fn scans_count_nesting_from_either_end() {
        assert_eq!(scan_forward(b"(()())", b'(', b')'), Some(5));
        assert_eq!(scan_forward(b"(()", b'(', b')'), None);
        assert_eq!(scan_backward(b"(()())", b'(', b')'), Some(0));
        assert_eq!(scan_backward(b"())", b'(', b')'), None);
        // Misuse (a closer first) is a miss, not an underflow.
        assert_eq!(scan_forward(b")(", b'(', b')'), None);
    }

    #[test]
    fn select_includes_both_brackets_and_goto_lands_before_the_partner() {
        let document = Document::from_utf8("a(bc)", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let mut view = EditorSurface::loading(document.snapshot(), Arc::new(|| {}));
        view.selection.caret = 1;
        view.selection.anchor = 1;
        assert!(matches!(
            view.matching_brace_inputs(false).as_deref(),
            Some([Input::SetCaret(4, false)])
        ));
        assert!(matches!(
            view.matching_brace_inputs(true).as_deref(),
            Some([Input::SetCaret(1, false), Input::SetCaret(5, true)])
        ));
        view.selection.caret = 3;
        view.selection.anchor = 3;
        assert!(view.matching_brace_inputs(false).is_none());
    }

    #[test]
    fn repaints_reuse_the_brace_scan_until_the_caret_or_text_moves() {
        let document = Document::from_utf8("a(bc)", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let mut view = EditorSurface::loading(document.snapshot(), Arc::new(|| {}));
        view.selection.caret = 1;
        assert_eq!(view.cached_matching_brace(), Some((1, 4)));
        // A marked cache entry proves the next repaint does not scan again.
        let (key, _) = view.brace_cache.unwrap();
        view.brace_cache = Some((key, Some((0, 0))));
        assert_eq!(view.cached_matching_brace(), Some((0, 0)));
        view.selection.caret = 3;
        assert_eq!(view.cached_matching_brace(), None);
        view.selection.caret = 1;
        assert_eq!(view.cached_matching_brace(), Some((1, 4)));
        let other = Document::from_utf8("a(bc)", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let (key, _) = view.brace_cache.unwrap();
        view.brace_cache = Some((key, Some((0, 0))));
        view.snapshot = other.snapshot();
        assert_eq!(view.cached_matching_brace(), Some((1, 4)), "another document rescans");
    }

    #[test]
    fn indent_guides_sit_on_indentation_stops_inside_the_indentation() {
        assert_eq!(indent_guide_offsets("        x", 4), vec![4]);
        assert_eq!(indent_guide_offsets("            ", 4), vec![4, 8]);
        assert_eq!(indent_guide_offsets("\t\tx", 4), vec![1]);
        assert_eq!(indent_guide_offsets("\t\t\tx", 8), vec![1, 2]);
        // A stop inside a tab has no character to anchor to.
        assert_eq!(indent_guide_offsets("  \tx", 4), Vec::<usize>::new());
        assert_eq!(indent_guide_offsets("  \t  x", 4), vec![3]);
        assert_eq!(indent_guide_offsets("x    y", 4), Vec::<usize>::new());
        assert_eq!(indent_guide_offsets("    x", 0), vec![1, 2, 3]);
    }

    #[test]
    fn whitespace_runs_and_line_ending_labels() {
        assert_eq!(
            whitespace_runs("a  b\t c"),
            vec![(1..3, false), (4..5, true), (5..6, false)]
        );
        assert_eq!(whitespace_runs("\t\t"), vec![(0..1, true), (1..2, true)]);
        assert!(whitespace_runs("abc").is_empty());
        assert_eq!(eol_label("\r\n"), Some("CRLF"));
        assert_eq!(eol_label("\n"), Some("LF"));
        assert_eq!(eol_label("\r"), Some("CR"));
        assert_eq!(eol_label(""), None);
    }

    #[test]
    fn edge_line_follows_the_column_and_horizontal_scroll() {
        assert_eq!(edge_line_x(64.0, 0.0, 80, 9.5, 2000.0), Some(64.0 + 760.0));
        assert_eq!(edge_line_x(64.0, 100.0, 80, 9.5, 2000.0), Some(64.0 + 660.0));
        // Scrolled past the edge, beyond the right side, or not configured.
        assert_eq!(edge_line_x(64.0, 800.0, 80, 9.5, 2000.0), None);
        assert_eq!(edge_line_x(64.0, 0.0, 80, 9.5, 500.0), None);
        assert_eq!(edge_line_x(64.0, 0.0, 0, 9.5, 2000.0), None);
        assert_eq!(edge_line_x(64.0, 0.0, 80, f32::NAN, 2000.0), None);
    }

    fn vertical_lines_at(ops: &[DrawOp], x: f32) -> usize {
        ops.iter()
            .filter(|op| {
                matches!(op, DrawOp::Line { from, to, .. }
                    if (from.x - x).abs() < 0.01 && (to.x - x).abs() < 0.01 && to.y > from.y)
            })
            .count()
    }

    #[test]
    fn painted_guides_and_edge_line_use_the_shaped_column_geometry() {
        let document = Document::from_utf8("        x\nabc\n", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let mut view = EditorSurface::loading(document.snapshot(), Arc::new(|| {}));
        let mut backend = RecordingBackend::default();
        // The recording backend advances 0.6 em per character at 16 px.
        let advance = 16.0_f32 * 0.6;
        let guide = view.text_left() + 4.0 * advance;
        let edge = view.text_left() + 10.0 * advance;
        let mut ops = Vec::new();
        view.draw(&mut backend, 800.0, 600.0, &mut ops).unwrap();
        assert_eq!(vertical_lines_at(&ops, guide), 0, "guides are off by default");
        assert_eq!(vertical_lines_at(&ops, edge), 0, "the edge is off by default");
        view.set_view_guides(ViewGuides {
            end_of_line: true,
            indent_guides: true,
            edge_column: Some(10),
        });
        ops.clear();
        view.draw(&mut backend, 800.0, 600.0, &mut ops).unwrap();
        assert_eq!(vertical_lines_at(&ops, guide), 1);
        assert_eq!(vertical_lines_at(&ops, edge), 1);
        // The status bar also names the document's line ending; count only the
        // marks painted in the text area above it.
        let endings = ops
            .iter()
            .filter(|op| {
                matches!(op, DrawOp::Text { text, origin, .. }
                    if text == "LF" && origin.y < 600.0 - bareline_ui::STATUS_HEIGHT)
            })
            .count();
        assert_eq!(endings, 2, "one LF mark per terminated line");
        assert!(bareline_renderer::balanced_clips(&ops));
    }
}
