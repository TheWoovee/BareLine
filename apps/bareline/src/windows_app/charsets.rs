// SPDX-License-Identifier: MPL-2.0
//! Searchable Character Sets picker (P1-7): lists every supported codec by
//! writing-system family, filters as you type, and applies the selected set
//! through the existing interpret command so busy checks and the dirty-document
//! confirmation stay owned by `encoding_dispatch`.
use super::*;
use bareline_app::encoding as model;
use bareline_commands::{Action, CommandId};
use bareline_renderer::{DrawOp, LayoutError, Rect};
use bareline_ui::{rect, text, text_field::TextField};

const ROW_HEIGHT: f32 = 22.0;
const HEADER_HEIGHT: f32 = 20.0;
const LIST_TOP: f32 = 70.0;
const HINT_HEIGHT: f32 = 26.0;

#[derive(Default)]
pub(super) struct CharsetRuntime {
    pub open: bool,
    pub field: TextField,
    /// Index into the selectable (codec) rows of the filtered list.
    pub selected: usize,
    /// First visible row offset in pixels from the top of the filtered list.
    pub scroll: f32,
    pub bounds: Rect,
    pub status: String,
}

enum Row {
    Header(model::CodecFamily),
    Codec(&'static model::CodecChoice),
}

fn filtered_rows(query: &str) -> Vec<Row> {
    let matches = model::charset_matches(query);
    let mut rows = Vec::new();
    for (family, members) in model::character_sets() {
        let present: Vec<_> = members
            .into_iter()
            .filter(|codec| matches.iter().any(|item| item.interpret == codec.interpret))
            .collect();
        if present.is_empty() {
            continue;
        }
        rows.push(Row::Header(family));
        rows.extend(present.into_iter().map(Row::Codec));
    }
    rows
}

fn codec_count(rows: &[Row]) -> usize {
    rows.iter().filter(|row| matches!(row, Row::Codec(_))).count()
}

fn selected_choice(rows: &[Row], selected: usize) -> Option<&'static model::CodecChoice> {
    rows.iter()
        .filter_map(|row| match row {
            Row::Codec(choice) => Some(*choice),
            Row::Header(_) => None,
        })
        .nth(selected)
}

/// Pixel extent of every row so the list and selection math share one source.
fn row_extents(rows: &[Row]) -> Vec<(f32, f32)> {
    let mut top = 0.0f32;
    rows.iter()
        .map(|row| {
            let height = match row {
                Row::Header(_) => HEADER_HEIGHT,
                Row::Codec(_) => ROW_HEIGHT,
            };
            let extent = (top, height);
            top += height;
            extent
        })
        .collect()
}

fn truncated(label: &str, width: f32) -> String {
    let max = (width / 7.0).floor().max(4.0) as usize;
    if label.chars().count() <= max {
        return label.to_string();
    }
    let mut out: String = label.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

impl CharsetRuntime {
    pub(super) fn open(&mut self) {
        self.open = true;
        self.status.clear();
        self.selected = 0;
        self.scroll = 0.0;
        self.field.select_all();
        self.field.insert("");
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw(
        &mut self,
        renderer: &mut impl bareline_renderer::TextBackend,
        width: f32,
        height: f32,
        theme: bareline_ui::theme::UiTheme,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        if !self.open {
            self.field.release(renderer);
            return Ok(None);
        }
        let w = width.clamp(320.0, 520.0);
        let h = (height - 120.0).clamp(220.0, 420.0);
        let y = 60.0f32.min((height - h - 20.0).max(0.0));
        self.bounds = rect((width - w) / 2.0, y, w, h);
        let b = self.bounds;
        ops.push(DrawOp::Fill(b, theme.elevated));
        ops.push(DrawOp::Stroke(b, theme.border, 1.0));
        text(ops, b.x + 16.0, b.y + 10.0, "Character sets", 14.0, theme.text);

        let field = rect(b.x + 16.0, b.y + 32.0, b.width - 32.0, 26.0);
        let caret = self.field.draw_with_theme(renderer, field, true, theme, ops)?;

        let rows = filtered_rows(self.field.value());
        let list = rect(
            b.x + 12.0,
            b.y + LIST_TOP,
            b.width - 24.0,
            (b.height - LIST_TOP - HINT_HEIGHT).max(0.0),
        );
        let extents = row_extents(&rows);
        // Keep the selected codec row inside the visible band before painting.
        let mut selected_top = 0.0f32;
        let mut seen = 0usize;
        for (index, row) in rows.iter().enumerate() {
            if matches!(row, Row::Codec(_)) {
                if seen == self.selected {
                    selected_top = extents[index].0;
                    break;
                }
                seen += 1;
            }
        }
        let visible = list.height.max(ROW_HEIGHT);
        if selected_top < self.scroll {
            self.scroll = selected_top;
        } else if selected_top + ROW_HEIGHT > self.scroll + visible {
            self.scroll = selected_top + ROW_HEIGHT - visible;
        }
        self.scroll = self.scroll.max(0.0);

        ops.push(DrawOp::PushClip(list));
        for (index, row) in rows.iter().enumerate() {
            let top = extents[index].0;
            let row_height = extents[index].1;
            if top + row_height <= self.scroll || top >= self.scroll + list.height {
                continue;
            }
            let y = list.y + top - self.scroll;
            match row {
                Row::Header(family) => {
                    text(
                        ops,
                        list.x + 8.0,
                        y + 4.0,
                        &truncated(family.label(), list.width),
                        11.0,
                        theme.muted,
                    );
                }
                Row::Codec(choice) => {
                    let codec_index = rows[..index]
                        .iter()
                        .filter(|item| matches!(item, Row::Codec(_)))
                        .count();
                    let is_selected = codec_index == self.selected;
                    if is_selected {
                        ops.push(DrawOp::Fill(rect(list.x, y, list.width, ROW_HEIGHT), theme.selection));
                    }
                    text(
                        ops,
                        list.x + 16.0,
                        y + 4.0,
                        &truncated(choice.label, list.width - 24.0),
                        13.0,
                        theme.text,
                    );
                }
            }
        }
        ops.push(DrawOp::PopClip);
        let hint = if self.status.is_empty() {
            "Type to filter; Enter applies the selected set"
        } else {
            self.status.as_str()
        };
        text(ops, b.x + 16.0, b.y + b.height - 20.0, hint, 12.0, theme.muted);
        Ok(Some(caret))
    }
}

impl Shell {
    fn charsets_submit(&mut self, el: &ActiveEventLoop) {
        let rows = filtered_rows(self.charsets.field.value());
        let Some(choice) = selected_choice(&rows, self.charsets.selected) else {
            self.charsets.status = "No character set matches".into();
            return;
        };
        let id = choice.interpret;
        self.charsets.open = false;
        self.charsets.field.cancel();
        self.charsets.status.clear();
        self.dispatch(el, Action::Contributed(CommandId(id)));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    fn charsets_click_row(&mut self, el: &ActiveEventLoop) {
        let list_y = self.charsets.bounds.y + LIST_TOP;
        let local = self.pointer.y - list_y + self.charsets.scroll;
        if local < 0.0 {
            return;
        }
        let rows = filtered_rows(self.charsets.field.value());
        let extents = row_extents(&rows);
        for (index, row) in rows.iter().enumerate() {
            let (top, row_height) = extents[index];
            if local >= top && local < top + row_height {
                if matches!(row, Row::Codec(_)) {
                    self.charsets.selected = rows[..index]
                        .iter()
                        .filter(|item| matches!(item, Row::Codec(_)))
                        .count();
                    self.charsets_submit(el);
                }
                return;
            }
        }
    }
    pub(super) fn charsets_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if !self.charsets.open || self.palette.open {
            return false;
        }
        match event {
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        let composing = self.charsets.field.composing();
                        self.charsets.field.cancel();
                        if !composing {
                            self.charsets.open = false;
                        }
                    }
                    Key::Named(NamedKey::Enter) => self.charsets_submit(el),
                    Key::Named(NamedKey::ArrowUp) => {
                        self.charsets.selected = self.charsets.selected.saturating_sub(1);
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        let rows = filtered_rows(self.charsets.field.value());
                        let count = codec_count(&rows);
                        if self.charsets.selected + 1 < count {
                            self.charsets.selected += 1;
                        }
                    }
                    Key::Named(NamedKey::PageUp) => {
                        self.charsets.selected = self.charsets.selected.saturating_sub(8);
                    }
                    Key::Named(NamedKey::PageDown) => {
                        let rows = filtered_rows(self.charsets.field.value());
                        let count = codec_count(&rows);
                        self.charsets.selected = (self.charsets.selected + 8).min(count.saturating_sub(1));
                    }
                    key => {
                        let field = &mut self.charsets.field;
                        match key {
                            Key::Named(NamedKey::Backspace) => {
                                field.delete(false);
                            }
                            Key::Named(NamedKey::Delete) => {
                                field.delete(true);
                            }
                            Key::Named(NamedKey::ArrowLeft) => field.horizontal(false, self.modifiers.shift_key()),
                            Key::Named(NamedKey::ArrowRight) => field.horizontal(true, self.modifiers.shift_key()),
                            Key::Named(NamedKey::Home) => field.edge(false, self.modifiers.shift_key()),
                            Key::Named(NamedKey::End) => field.edge(true, self.modifiers.shift_key()),
                            Key::Character(v) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                                match v.to_lowercase().as_str() {
                                    "a" => field.select_all(),
                                    "c" | "x" => {
                                        if let Some(platform) = &self.platform
                                            && platform.set_clipboard_text(field.selected()).is_ok()
                                            && v.eq_ignore_ascii_case("x")
                                        {
                                            field.insert("");
                                        }
                                    }
                                    "v" => {
                                        if let Some(platform) = &self.platform
                                            && let Ok(value) = platform.clipboard_text()
                                        {
                                            field.commit(&value);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            _ if !self.modifiers.control_key() || self.modifiers.alt_key() => {
                                if let Some(value) = &event.text {
                                    field.insert(value);
                                }
                            }
                            _ => {}
                        }
                        self.charsets.selected = 0;
                        self.charsets.scroll = 0.0;
                        self.charsets.status.clear();
                    }
                }
            }
            WindowEvent::Ime(ime) => {
                let field = &mut self.charsets.field;
                match ime {
                    Ime::Preedit(v, c) => field.preedit(v.clone(), *c),
                    Ime::Commit(v) => {
                        field.commit(v);
                    }
                    Ime::Disabled => field.cancel(),
                    Ime::Enabled => {}
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !self.charsets.bounds.contains(self.pointer) {
                    self.charsets.open = false;
                } else {
                    let field = rect(
                        self.charsets.bounds.x + 16.0,
                        self.charsets.bounds.y + 32.0,
                        self.charsets.bounds.width - 32.0,
                        26.0,
                    );
                    if field.contains(self.pointer) {
                        if let Some(renderer) = &self.renderer {
                            let _ = self
                                .charsets
                                .field
                                .click(renderer, self.pointer, self.modifiers.shift_key());
                        }
                    } else {
                        self.charsets_click_row(el);
                    }
                }
            }
            WindowEvent::Focused(false) => {
                self.charsets.field.cancel();
                return false;
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
                return false;
            }
            WindowEvent::CursorMoved { .. }
            | WindowEvent::RedrawRequested
            | WindowEvent::CloseRequested
            | WindowEvent::Resized(_)
            | WindowEvent::ScaleFactorChanged { .. } => return false,
            _ => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_finds_ansi_by_fragment_and_family() {
        let rows = filtered_rows("");
        assert_eq!(codec_count(&rows), model::CODECS.len());
        let western = filtered_rows("windows-1252");
        assert_eq!(codec_count(&western), 1);
        let choice = selected_choice(&western, 0).expect("windows-1252 present");
        assert_eq!(choice.interpret, "encoding.interpret.windows1252");

        let ansi = filtered_rows("1252");
        assert_eq!(codec_count(&ansi), 1);
        let by_family = filtered_rows("cyrillic");
        assert!(codec_count(&by_family) >= 1);
        assert!(selected_choice(&by_family, 0).is_some());
        assert!(selected_choice(&filtered_rows("no-such-set"), 0).is_none());
    }
}
