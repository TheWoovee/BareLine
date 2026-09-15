// SPDX-License-Identifier: MPL-2.0
//! Per-view caret timing and logical display controls; no timer thread.
use crate::EditorSurface;
use std::time::{Duration, Instant};
/// A shaped anchor for an overlapping paged window, never an estimated byte/pixel ratio.
#[derive(Clone, Copy, Debug)]
pub struct HorizontalAnchor {
    pub offset: bareline_document::TextOffset,
    /// Pixel x relative to the editor text area, not the window chrome.
    pub screen_x: f32,
    pub direction: i8,
}
pub(crate) struct CaretBlink {
    focused: bool,
    visible: bool,
    deadline: Option<Instant>,
}
impl Default for CaretBlink {
    fn default() -> Self {
        Self {
            focused: true,
            visible: true,
            deadline: Some(Instant::now() + Duration::from_millis(530)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapped_fold_gap_keeps_adjacent_shaped_rows_and_verified_gutter_labels() {
        use crate::paged_view::ViewportSegment;
        use bareline_document::{Budget, Document, TextOffset};
        use bareline_renderer::{DrawOp, TextBackend};
        let header = "தமிழ் {\n";
        let hidden = "hidden\n".repeat(50_000);
        assert!(hidden.len() > 256 * 1024);
        let suffix = "مرحبا 👩🏽‍💻 e\u{301}";
        let source = format!("{header}{hidden}{suffix}");
        let suffix_start = header.len() + hidden.len();
        let projected = format!("{}{}", &source[..header.len()], &source[suffix_start..]);
        let document = Document::from_utf8(&projected, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let mut view = EditorSurface::loading(document.snapshot(), std::sync::Arc::new(|| {}));
        view.set_source_segments(&[
            ViewportSegment {
                local: TextOffset(0)..TextOffset(header.len()),
                source: TextOffset(0)..TextOffset(header.len()),
                first_global_line: Some(0),
                source_line_start: Some(TextOffset(0)),
            },
            ViewportSegment {
                local: TextOffset(header.len())..TextOffset(projected.len()),
                source: TextOffset(suffix_start)..TextOffset(source.len()),
                first_global_line: Some(50_001),
                source_line_start: Some(TextOffset(suffix_start)),
            },
        ]);
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        let mut ops = Vec::new();
        view.draw(&mut backend, 900.0, 400.0, &mut ops).unwrap();
        let first = &view.layouts[&0];
        let second = &view.layouts[&1];
        assert!(first.end <= header.len());
        assert_eq!(second.start, header.len());
        assert_eq!(view.visual_line(1), 1);
        assert_eq!(view.source_line_at(TextOffset(header.len() - 1)), Some(0));
        assert_eq!(view.source_line_at(TextOffset(header.len())), Some(50_001));
        for label in ["1", "50002"] {
            assert!(
                ops.iter()
                    .any(|op| matches!(op, DrawOp::Text { origin, text, .. } if origin.x == 14.0 && text == label))
            );
        }
        let caret = backend.caret(second.id, 0).unwrap();
        let hit = backend
            .hit_test(second.id, bareline_renderer::Point { x: caret.x, y: caret.y })
            .unwrap();
        assert_eq!(second.start + hit.byte_offset, header.len());
        assert!(
            view.accessibility_geometry(&backend, 900.0, 400.0)
                .iter()
                .any(|(range, _)| range.start == header.len())
        );
    }
    #[test]
    fn paged_gutter_shows_window_start_line_plus_local_index() {
        use crate::paged_view::ViewportSegment;
        use bareline_document::{Budget, Document, TextOffset};
        use bareline_renderer::DrawOp;
        let text = "aaa\nbbb\nccc\n";
        let document = Document::from_utf8(text, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let mut view = EditorSurface::loading(document.snapshot(), std::sync::Arc::new(|| {}));
        // A resident window whose first line is global line 599,999 (shown as 600000).
        let base = 599_999u64;
        view.set_source_segments(&[ViewportSegment {
            local: TextOffset(0)..TextOffset(text.len()),
            source: TextOffset(0)..TextOffset(text.len()),
            first_global_line: Some(base),
            source_line_start: Some(TextOffset(0)),
        }]);
        // Gutter line = window start line + local index, exactly.
        assert_eq!(view.source_line_at(TextOffset(0)), Some(base));
        assert_eq!(view.source_line_at(TextOffset(4)), Some(base + 1));
        assert_eq!(view.source_line_at(TextOffset(8)), Some(base + 2));
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        let mut ops = Vec::new();
        view.draw(&mut backend, 900.0, 400.0, &mut ops).unwrap();
        // Painted labels are 1-based, so the first row reads 600000.
        for label in ["600000", "600001", "600002"] {
            assert!(
                ops.iter()
                    .any(|op| matches!(op, DrawOp::Text { origin, text, .. } if origin.x == 14.0 && text == label)),
                "missing gutter label {label}"
            );
        }
    }
    #[test]
    fn overlapping_window_retains_shaped_anchor_and_busy_wheel_delta() {
        use bareline_renderer::TextBackend;
        let make = |text: &str| {
            let doc = bareline_document::Document::from_utf8(
                text,
                bareline_document::Budget::new(1 << 20),
                bareline_document::Budget::new(1 << 20),
            )
            .unwrap();
            EditorSurface::loading(doc.snapshot(), std::sync::Arc::new(|| {}))
        };
        let text = format!("{}தமிழ் مرحبا 👩🏽‍💻", "a".repeat(100));
        let mut first = make(&text);
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        first.draw(&mut backend, 400.0, 300.0, &mut Vec::new()).unwrap();
        first.scroll_horizontal(800.0);
        let anchor = first.horizontal_window_anchor(&backend, 400.0).unwrap();
        assert!(anchor.offset.0 >= 50);
        let mut next = make(&text[50..]);
        next.restore_horizontal_anchor(bareline_document::TextOffset(anchor.offset.0 - 50), anchor.screen_x)
            .unwrap();
        next.scroll_horizontal(25.0);
        assert!(next.horizontal_anchor_pending());
        next.draw(&mut backend, 400.0, 300.0, &mut Vec::new()).unwrap();
        assert!(!next.horizontal_anchor_pending());
        let layout = next.layouts.get(&0).unwrap();
        let caret = backend.caret(layout.id, anchor.offset.0 - 50 - layout.start).unwrap();
        let screen = (layout.x_origin - next.scroll_x) as f32 + caret.x;
        assert!((screen - (anchor.screen_x - 25.0)).abs() < 0.1);
    }
    #[test]
    fn zoom_survives_settings_reapplication_and_clone_changes_are_independent() {
        let document = bareline_document::Document::from_utf8(
            "தமிழ் 👩🏽‍💻 مرحبا",
            bareline_document::Budget::new(1 << 20),
            bareline_document::Budget::new(1 << 20),
        )
        .unwrap();
        let mut view = EditorSurface::loading(document.snapshot(), std::sync::Arc::new(|| {}));
        view.apply_visual_preferences(12.0, 4, true, true, "none");
        assert!(view.zoom_by(3.0));
        let zoomed = view.font_pixels;
        view.apply_visual_preferences(12.0, 4, true, true, "none");
        assert_eq!(view.font_pixels, zoomed);
        let mut clone = view.clone_view();
        clone.zoom_by(-2.0);
        assert_eq!(view.font_pixels, zoomed);
        assert_ne!(clone.font_pixels, zoomed);
        view.set_focused(false);
        assert_eq!(view.blink_deadline(), None);
        view.set_focused(true);
        let deadline = view.blink_deadline();
        view.set_focused(true);
        assert_eq!(deadline, view.blink_deadline());
    }
}
impl EditorSurface {
    /// Installs the paged owner's verified map alongside its local projection.
    /// Geometry stays in projection coordinates; omitted source bytes add no rows.
    pub fn set_source_segments(&mut self, segments: &[crate::paged_view::ViewportSegment]) {
        self.source_rows = (!segments.is_empty()).then(|| (self.snapshot.content_state, segments.to_vec()));
    }

    /// Shared gutter geometry for painting, pointer hits and accessibility.
    pub fn text_left(&self) -> f32 {
        if !self.line_numbers {
            return crate::LEFT;
        }
        // Source anchors reserve enough digits even for a small paged viewport.
        let lines = self.snapshot.line_count() as u64;
        let largest = self
            .source_rows
            .as_ref()
            .filter(|(state, _)| *state == self.snapshot.content_state)
            .and_then(|(_, segments)| segments.iter().filter_map(|piece| piece.first_global_line).max())
            .unwrap_or(0)
            .saturating_add(lines)
            .max(1);
        let digits = largest.ilog10() + 1;
        // One em per numeral conservatively allows supported fonts, with a
        // separate 26-pixel fold target after the number column.
        crate::LEFT.max(14.0 + digits as f32 * self.font_pixels + 26.0)
    }

    /// Zero-based source line at a local shaped position. At a fold seam the
    /// following piece owns the caret, matching After affinity for hit testing.
    pub fn source_line_at(&self, local: bareline_document::TextOffset) -> Option<u64> {
        let local_line = self.snapshot.line_at(local).ok()?;
        let Some((state, segments)) = &self.source_rows else {
            return Some(local_line as u64);
        };
        if *state != self.snapshot.content_state {
            return None;
        }
        let segment = segments
            .iter()
            .find(|piece| piece.local.start <= local && local < piece.local.end)
            .or_else(|| segments.last().filter(|piece| piece.local.end == local))?;
        let first_local = self.snapshot.line_at(segment.local.start).ok()?;
        segment
            .first_global_line?
            .checked_add(local_line.checked_sub(first_local)? as u64)
    }

    pub fn horizontal_window_anchor(
        &self,
        backend: &impl bareline_renderer::TextBackend,
        width: f32,
    ) -> Option<HorizontalAnchor> {
        if self.wrap || self.horizontal_intent == 0 || self.pending_horizontal_anchor.is_some() {
            return None;
        }
        let viewport = (width - self.text_left() - 16.0).max(1.0);
        let row = (self.scroll_y / self.line_height() as f64).floor() as usize;
        let line = self.logical_line(row);
        let layout = self.layouts.get(&line)?;
        let extent = backend.layout_size(layout.id).ok()?.0;
        let direction = if self.horizontal_intent < 0 && layout.start == 0 && self.scroll_x <= 0.0 {
            -1
        } else if self.horizontal_intent > 0
            && layout.end == self.snapshot.len()
            && self.scroll_x + f64::from(viewport) >= layout.x_origin + f64::from(extent) - f64::from(viewport) * 0.25
        {
            1
        } else {
            return None;
        };
        let hit = backend
            .hit_test(
                layout.id,
                bareline_renderer::Point {
                    x: (self.scroll_x - layout.x_origin) as f32 + viewport * 0.5,
                    y: layout.context_y + self.line_height() * 0.5,
                },
            )
            .ok()?;
        let offset = (layout.start + hit.byte_offset).min(layout.end);
        let caret = backend.caret(layout.id, offset - layout.start).ok()?;
        Some(HorizontalAnchor {
            offset: bareline_document::TextOffset(offset),
            screen_x: (layout.x_origin - self.scroll_x) as f32 + caret.x,
            direction,
        })
    }
    pub fn restore_horizontal_anchor(
        &mut self,
        offset: bareline_document::TextOffset,
        screen_x: f32,
    ) -> Result<(), String> {
        if !screen_x.is_finite() || !self.snapshot.is_boundary(offset) {
            return Err("Invalid horizontal viewport anchor".into());
        }
        self.pending_horizontal_anchor = Some((offset.0, screen_x, 0.0));
        self.horizontal_intent = 0;
        self.reveal_caret = false;
        (self.notify)();
        Ok(())
    }
    pub fn horizontal_anchor_pending(&self) -> bool {
        self.pending_horizontal_anchor.is_some()
    }
    pub fn virtual_navigation_pending(&self) -> bool {
        self.grapheme_navigation.is_some() || !self.visual_navigation.is_empty()
    }
    pub(crate) fn resolve_visual_navigation(
        &mut self,
        backend: &impl bareline_renderer::TextBackend,
    ) -> Result<(), bareline_renderer::LayoutError> {
        use crate::Input;
        use bareline_document::TextOffset;
        use bareline_renderer::Point;
        use unicode_segmentation::UnicodeSegmentation;
        if self.grapheme_navigation.is_some() {
            return Ok(());
        }
        let Some(input) = self.visual_navigation.pop_front() else {
            return Ok(());
        };
        if let Input::Left(extend) | Input::Right(extend) = input {
            self.preferred_x = None;
            let right = matches!(input, Input::Right(_));
            let line = self
                .snapshot
                .line_at(TextOffset(self.selection.caret))
                .map_err(|_| bareline_renderer::LayoutError::InvalidOffset)?;
            let Some(layout) = self.layouts.get(&line) else {
                return Ok(());
            };
            let current = backend.caret(
                layout.id,
                self.selection
                    .caret
                    .saturating_sub(layout.start)
                    .min(layout.end - layout.start),
            )?;
            let text = self
                .snapshot
                .read(
                    TextOffset(layout.start)..TextOffset(layout.end),
                    bareline_renderer::MAX_LAYOUT_BYTES,
                )
                .map_err(|_| bareline_renderer::LayoutError::InvalidOffset)?;
            let mut best: Option<(f32, usize)> = None;
            for local in text.grapheme_indices(true).map(|(i, _)| i).chain(Some(text.len())) {
                if local == text.len() && self.content_range(line).is_some_and(|range| layout.end < range.end) {
                    continue;
                }
                let caret = backend.caret(layout.id, local)?;
                let dx = caret.x - current.x;
                if (caret.y - current.y).abs() < self.line_height() * 0.5 && if right { dx > 0.01 } else { dx < -0.01 }
                {
                    if best.is_none_or(|(distance, _)| dx.abs() < distance) {
                        best = Some((dx.abs(), layout.start + local));
                    }
                }
            }
            let target = best.map(|(_, offset)| offset).or_else(|| {
                if right {
                    self.next_grapheme(self.selection.caret)
                } else {
                    self.previous_grapheme(self.selection.caret)
                }
            });
            if let Some(target) = target {
                if self.grapheme_navigation.is_none() {
                    self.grapheme_navigation = Some(
                        crate::grapheme_navigation::Navigation::start_snap(
                            self.snapshot.clone(),
                            self.selection.caret,
                            target,
                            extend,
                            self.notify.clone(),
                        )
                        .map_err(|_| bareline_renderer::LayoutError::BackendFailure)?,
                    );
                }
            } else if self.grapheme_navigation.is_none() {
                self.grapheme_navigation = Some(
                    crate::grapheme_navigation::Navigation::start(
                        self.snapshot.clone(),
                        self.selection.caret,
                        right,
                        extend,
                        self.notify.clone(),
                    )
                    .map_err(|_| bareline_renderer::LayoutError::BackendFailure)?,
                );
            }
            return Ok(());
        }
        let (down, extend) = match input {
            Input::Up(extend) => (false, extend),
            Input::Down(extend) => (true, extend),
            _ => return Ok(()),
        };
        let line = self
            .snapshot
            .line_at(TextOffset(self.selection.caret))
            .map_err(|_| bareline_renderer::LayoutError::InvalidOffset)?;
        let Some(layout) = self.layouts.get(&line) else {
            self.visual_navigation.push_front(input);
            return Ok(());
        };
        let caret = backend.caret(
            layout.id,
            self.selection
                .caret
                .saturating_sub(layout.start)
                .min(layout.end - layout.start),
        )?;
        let x = *self.preferred_x.get_or_insert(caret.x + layout.x_origin as f32);
        let mut y = caret.y
            + if down {
                self.line_height() * 1.5
            } else {
                -self.line_height() * 0.5
            };
        let height = backend.layout_size(layout.id)?.1;
        let target_line = if y < 0.0 {
            self.logical_line(self.visual_line(line).saturating_sub(1))
        } else if y >= height {
            self.logical_line(
                self.visual_line(line)
                    .saturating_add(self.wrap_rows.get(&line).copied().unwrap_or(1)),
            )
        } else {
            line
        };
        let Some(target) = self.layouts.get(&target_line) else {
            return Ok(());
        };
        if target_line != line {
            y = if down {
                0.0
            } else {
                (backend.layout_size(target.id)?.1 - self.line_height() * 0.5).max(0.0)
            };
        }
        let hit = backend.hit_test(
            target.id,
            Point {
                x: x - target.x_origin as f32,
                y,
            },
        )?;
        let text = self
            .snapshot
            .read(
                TextOffset(target.start)..TextOffset(target.end),
                bareline_renderer::MAX_LAYOUT_BYTES,
            )
            .map_err(|_| bareline_renderer::LayoutError::InvalidOffset)?;
        let local = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(Some(text.len()))
            .take_while(|i| *i <= hit.byte_offset)
            .last()
            .unwrap_or(0);
        self.grapheme_navigation = Some(
            crate::grapheme_navigation::Navigation::start_snap(
                self.snapshot.clone(),
                self.selection.caret,
                target.start + local,
                extend,
                self.notify.clone(),
            )
            .map_err(|_| bareline_renderer::LayoutError::BackendFailure)?,
        );
        Ok(())
    }
    pub fn set_focused(&mut self, focused: bool) {
        if self.blink.focused == focused {
            return;
        }
        self.blink.focused = focused;
        self.reset_caret_blink();
    }
    pub fn reset_caret_blink(&mut self) {
        self.blink.visible = true;
        self.blink.deadline = self.blink.focused.then(|| Instant::now() + Duration::from_millis(530));
    }
    pub fn blink_deadline(&self) -> Option<Instant> {
        self.blink.deadline
    }
    pub fn tick_caret_blink(&mut self, now: Instant) -> bool {
        if self.blink.deadline.is_some_and(|deadline| now >= deadline) {
            self.blink.visible = !self.blink.visible;
            self.blink.deadline = Some(now + Duration::from_millis(530));
            true
        } else {
            false
        }
    }
    pub(crate) fn caret_visible(&self) -> bool {
        self.blink.focused && (self.blink.visible || self.composition.is_some())
    }
    pub fn zoom_by(&mut self, steps: f32) -> bool {
        if !steps.is_finite() {
            return false;
        }
        let size = (self.font_pixels + steps).clamp(8.0, 96.0);
        if size == self.font_pixels {
            return false;
        }
        self.font_pixels = size;
        self.zoom_offset = size - self.base_font_pixels;
        self.clear_column_metrics();
        self.layout_revision = None;
        self.wrap_rows.clear();
        self.reveal_caret = true;
        true
    }
    pub fn scroll_horizontal(&mut self, delta: f64) {
        if delta.is_finite() && !self.wrap {
            self.horizontal_intent = if delta < 0.0 {
                -1
            } else if delta > 0.0 {
                1
            } else {
                0
            };
            if let Some((_, _, pending_delta)) = &mut self.pending_horizontal_anchor {
                *pending_delta += delta;
                return;
            }
            if self.scroll_x + delta < 0.0
                && let Ok(line) = self
                    .snapshot
                    .line_at(bareline_document::TextOffset(self.selection.caret))
                && let Some(state) = self.virtual_lines.get_mut(&line)
                && let Some(offset) = state.pan_before_origin(&self.snapshot)
            {
                self.pending_horizontal_anchor = Some((offset, 0.0, delta));
                self.scroll_x = 0.0;
                self.reveal_caret = false;
                (self.notify)();
                return;
            }
            self.scroll_x = (self.scroll_x + delta).max(0.0);
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
