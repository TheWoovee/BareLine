// SPDX-License-Identifier: MPL-2.0
//! Settings drawing and layout. Drawing records the hit rects the event and
//! semantics code reads back; it never performs I/O.
use super::*;
/// What the font picker draws in each face beside the face's name.
const FONT_SAMPLE: &str = "AaBb 0O1l";
impl SettingsController {
    pub(super) fn reset_dialog_bounds(&self) -> Rect {
        let sidebar = 164.0_f32.min(self.bounds.width * 0.26);
        rect(
            self.bounds.x + sidebar + 38.0,
            self.bounds.y + 120.0,
            (self.bounds.width - sidebar - 76.0).max(100.0),
            112.0,
        )
    }
    pub fn draw(
        &mut self,
        bounds: Rect,
        backend: &mut impl TextBackend,
        ops: &mut Vec<DrawOp>,
    ) -> Result<(), LayoutError> {
        for mut field in self.retired_fields.drain(..) {
            field.release(backend);
        }
        // Per-face preview layouts shaped last frame have now been painted, so
        // release them before shaping this frame's set.
        for layout in std::mem::take(&mut self.retired_layouts) {
            backend.release_layout(layout);
        }
        self.query
            .set_placeholder(&self.label("settings.search", "Search settings"));
        self.bounds = bounds;
        let effective = self.effective();
        let theme = config::Theme::resolve(effective.theme, self.system, &effective.theme_overrides)
            .unwrap_or_else(|_| config::Theme::builtin(self.system.dark));
        let color = |key: &str| Color(theme.color(key).unwrap().rgb);
        let bg = color("surface.editor");
        let chrome = color("surface.chrome");
        let foreground = color("text");
        let muted = color("text.muted");
        let border = color("border");
        let focus = color("focus.ring");
        // Controls and selected rows take tokens composited over the editor, as
        // the rest of the UI does. `color` drops alpha, so it would paint the
        // translucent `selection.row` as a solid band (A11Y-01).
        let ui = bareline_ui::theme::UiTheme::from_tokens(|key| theme.color(key).map(|c| (c.rgb, c.alpha))).unwrap();
        ops.push(DrawOp::PushClip(bounds));
        ops.push(DrawOp::Fill(bounds, bg));
        // Persistent header: title, a way to open the file, a close button and the
        // Escape hint, so the page can be closed with the mouse alone (UX-54b).
        const HEADER: f32 = 44.0;
        ops.push(DrawOp::Fill(rect(bounds.x, bounds.y, bounds.width, HEADER), chrome));
        text(
            ops,
            bounds.x + 18.0,
            bounds.y + 12.0,
            self.label("settings.title", "Settings"),
            16.0,
            foreground,
        );
        text(
            ops,
            bounds.x + 100.0,
            bounds.y + 15.0,
            self.label("settings.close_hint", "Esc to close"),
            11.0,
            muted,
        );
        self.close_button = rect(bounds.x + bounds.width - 44.0, bounds.y + 8.0, 30.0, 28.0);
        self.open_toml = rect(bounds.x + bounds.width - 228.0, bounds.y + 8.0, 172.0, 28.0);
        ops.push(DrawOp::StrokeRounded(
            self.open_toml,
            color("border.interactive"),
            4.0,
            1.0,
        ));
        text(
            ops,
            self.open_toml.x + 12.0,
            self.open_toml.y + 6.0,
            self.label("settings.open_toml", "Open settings.toml"),
            12.0,
            foreground,
        );
        ops.push(DrawOp::StrokeRounded(
            self.close_button,
            color("border.interactive"),
            4.0,
            1.0,
        ));
        text(
            ops,
            self.close_button.x + 10.0,
            self.close_button.y + 5.0,
            "\u{00d7}",
            16.0,
            foreground,
        );
        ops.push(DrawOp::Line {
            from: Point {
                x: bounds.x,
                y: bounds.y + HEADER,
            },
            to: Point {
                x: bounds.x + bounds.width,
                y: bounds.y + HEADER,
            },
            color: border,
            width: 1.0,
        });
        let bounds = rect(
            bounds.x,
            bounds.y + HEADER,
            bounds.width,
            (bounds.height - HEADER).max(1.0),
        );
        self.bounds = bounds;
        let sidebar = 164.0_f32.min(bounds.width * 0.26);
        ops.push(DrawOp::Fill(rect(bounds.x, bounds.y, sidebar, bounds.height), chrome));
        for (index, category) in CATEGORIES.iter().enumerate() {
            let y = bounds.y + index as f32 * 38.0;
            if self.category == *category {
                ops.push(DrawOp::Fill(
                    rect(bounds.x, y, sidebar, 38.0),
                    color("surface.elevated"),
                ));
                ops.push(DrawOp::Fill(rect(bounds.x, y, 3.0, 38.0), focus));
            }
            text(
                ops,
                bounds.x + 18.0,
                y + 10.0,
                self.label(&format!("settings.category.{category}"), category),
                13.0,
                foreground,
            );
        }
        let x = bounds.x + sidebar + 18.0;
        let width = (bounds.width - sidebar - 36.0).max(1.0);
        self.search_bounds = rect(x, bounds.y + 12.0, width, 32.0);
        self.query
            .draw_with_theme(backend, self.search_bounds, self.query_focused, ui, ops)?;
        self.scope_user = rect(x, bounds.y + 52.0, 78.0, 28.0);
        self.scope_workspace = rect(x + 86.0, bounds.y + 52.0, 130.0, 28.0);
        self.opt_in = rect(x + 230.0, bounds.y + 52.0, (width - 230.0).max(0.0), 28.0);
        for (target, label, scope) in [
            (self.scope_user, "User", Scope::User),
            (self.scope_workspace, "Workspace", Scope::Workspace),
        ] {
            text(
                ops,
                target.x,
                target.y + 4.0,
                if self.scope == scope { "◉" } else { "○" },
                15.0,
                if self.scope == scope { focus } else { muted },
            );
            text(
                ops,
                target.x + 24.0,
                target.y + 4.0,
                self.label(
                    if scope == Scope::User {
                        "settings.scope.user"
                    } else {
                        "settings.scope.workspace"
                    },
                    label,
                ),
                13.0,
                foreground,
            );
        }
        if self.scope == Scope::Workspace {
            text(
                ops,
                self.opt_in.x,
                self.opt_in.y + 4.0,
                if self.workspace_opted_in {
                    "Workspace preferences enabled"
                } else {
                    "Enable workspace preferences"
                },
                12.0,
                focus,
            );
        }
        let header = bounds.y + 88.0;
        text(
            ops,
            x,
            header,
            self.label("settings.effective", "Effective values"),
            13.0,
            muted,
        );
        ops.push(DrawOp::Line {
            from: Point {
                x: bounds.x + sidebar,
                y: header + 24.0,
            },
            to: Point {
                x: bounds.x + bounds.width,
                y: header + 24.0,
            },
            color: border,
            width: 1.0,
        });
        let row_height = if width < 620.0 { 96.0 } else { 58.0 };
        // Appearance shows the theme cards where the color-mode picker row used
        // to be; the settings rows start below the card band.
        let cards_visible = self.theme_cards_visible();
        let card_band = if cards_visible { 108.0 } else { 0.0 };
        if cards_visible {
            let mode = self.theme_mode_value();
            let card_y = header + 30.0;
            let gap = 12.0;
            let card_width = ((width - gap * 2.0) / 3.0).max(1.0);
            let mut cards = [Rect::default(); 3];
            for (index, (title, value)) in THEME_CARDS.iter().enumerate() {
                let card = rect(x + index as f32 * (card_width + gap), card_y, card_width, 92.0);
                cards[index] = card;
                // Resolve the card's own theme so it previews that mode's colors.
                let card_theme = config::Theme::resolve(
                    match *value {
                        "light" => config::ThemeMode::Light,
                        "dark" => config::ThemeMode::Dark,
                        _ => config::ThemeMode::System,
                    },
                    self.system,
                    &effective.theme_overrides,
                )
                .unwrap_or_else(|_| config::Theme::builtin(*value == "dark"));
                let pick = |key: &str, fallback: Color| card_theme.color(key).map(|c| Color(c.rgb)).unwrap_or(fallback);
                let card_bg = pick("surface.editor", bg);
                let card_fg = pick("text", foreground);
                let accent = pick("accent", focus);
                let selected = mode == *value;
                ops.push(DrawOp::FillRounded(card, card_bg, 6.0));
                ops.push(DrawOp::StrokeRounded(
                    card,
                    if selected { focus } else { border },
                    6.0,
                    if selected { 2.0 } else { 1.0 },
                ));
                // Accent swatch, so each card advertises its accent color.
                ops.push(DrawOp::FillRounded(
                    rect(card.x + 12.0, card.y + 12.0, 40.0, 20.0),
                    accent,
                    4.0,
                ));
                text(
                    ops,
                    card.x + 12.0,
                    card.y + 48.0,
                    self.label(&format!("settings.theme.{value}"), title),
                    14.0,
                    card_fg,
                );
                if selected {
                    text(
                        ops,
                        card.x + 12.0,
                        card.y + 68.0,
                        self.label("settings.theme.current", "Selected"),
                        11.0,
                        accent,
                    );
                }
            }
            self.theme_cards = cards;
        } else {
            self.theme_cards = [Rect::default(); 3];
        }
        let top = header + 30.0 + card_band;
        let footer_height = if self.external_change.is_some() { 98.0 } else { 60.0 };
        let count = ((bounds.y + bounds.height - footer_height - top) / row_height).max(0.0) as usize;
        let definitions = self.definitions();
        self.first = self.first.min(definitions.len().saturating_sub(1));
        let old = std::mem::take(&mut self.rows);
        for (index, definition) in definitions.iter().skip(self.first).take(count).enumerate() {
            let y = top + index as f32 * row_height;
            let compact = width < 620.0;
            let control = if compact {
                rect(x, y + 52.0, width, 30.0)
            } else {
                rect(x + width * 0.70, y + 5.0, width * 0.30, 34.0)
            };
            // The visible TOML key itself is the copy target; its accessible
            // name and command expose the action without extra row clutter.
            let copy = rect(x + 10.0, y + 25.0, (width * 0.29 - 10.0).max(0.0), 22.0);
            let disabled = self.scope == Scope::Workspace && !definition.workspace_allowed;
            let state = old
                .iter()
                .find(|r| r.definition.key == definition.key)
                .map(|r| r.value.state)
                .unwrap_or_default();
            let copy_state = old
                .iter()
                .find(|r| r.definition.key == definition.key)
                .map(|r| r.copy.state)
                .unwrap_or_default();
            let value = self.value_text(definition.key);
            text(
                ops,
                x + 10.0,
                y + 4.0,
                self.label(&format!("setting.{}.title", definition.key), definition.title),
                13.0,
                foreground,
            );
            text(ops, x + 10.0, y + 27.0, definition.key, 11.0, muted);
            if !compact {
                ops.push(DrawOp::PushClip(rect(x + width * 0.29, y, width * 0.39, row_height)));
                text(
                    ops,
                    x + width * 0.29,
                    y + 16.0,
                    self.label(definition.description_id, definition.description),
                    12.0,
                    muted,
                );
                ops.push(DrawOp::PopClip);
            }
            let selected_row = self.selected == self.first + index && !self.query_focused;
            let outline = if selected_row {
                focus
            } else {
                color("border.interactive")
            };
            if matches!(definition.kind, SettingKind::Boolean) {
                // Booleans are switches, not two-item dropdowns (UX-54k).
                let on = value == on_off(true);
                let track = rect(control.x + 8.0, control.y + 7.0, 40.0, 20.0);
                ops.push(DrawOp::FillRounded(
                    track,
                    if on && !disabled {
                        focus
                    } else {
                        color("surface.chrome")
                    },
                    10.0,
                ));
                ops.push(DrawOp::StrokeRounded(track, outline, 10.0, 1.0));
                ops.push(DrawOp::FillRounded(
                    rect(
                        if on { track.x + 21.0 } else { track.x + 2.0 },
                        track.y + 2.0,
                        16.0,
                        16.0,
                    ),
                    color("surface.editor"),
                    8.0,
                ));
                text(
                    ops,
                    track.x + 52.0,
                    control.y + 8.0,
                    value,
                    13.0,
                    if disabled { muted } else { foreground },
                );
            } else {
                let has_list = self.choice_list(definition.key).is_some();
                ops.push(DrawOp::StrokeRounded(control, outline, 3.0, 1.0));
                ops.push(DrawOp::PushClip(control));
                text(
                    ops,
                    control.x + 12.0,
                    control.y + 8.0,
                    value,
                    13.0,
                    if disabled { muted } else { foreground },
                );
                // A color map previews its overrides as swatches beside the count.
                if config::is_color_map(definition.key) {
                    let mut swatch_x = control.x + 90.0;
                    for hex in effective.theme_overrides.values().take(6) {
                        if let Ok(parsed) = config::ThemeColor::parse(hex) {
                            let swatch = rect(swatch_x, control.y + 9.0, 16.0, 16.0);
                            ops.push(DrawOp::FillRounded(swatch, Color(parsed.rgb), 3.0));
                            ops.push(DrawOp::StrokeRounded(swatch, border, 3.0, 1.0));
                            swatch_x += 20.0;
                        }
                    }
                }
                if has_list {
                    // Only rows that actually open a list draw a chevron (UX-54a).
                    text(
                        ops,
                        control.x + control.width - 20.0,
                        control.y + 8.0,
                        "\u{2304}",
                        13.0,
                        muted,
                    );
                } else if matches!(definition.kind, SettingKind::Strings | SettingKind::Map) {
                    text(
                        ops,
                        control.x + control.width - 26.0,
                        control.y + 8.0,
                        self.label("settings.edit", "Edit\u{2026}"),
                        11.0,
                        muted,
                    );
                }
                ops.push(DrawOp::PopClip);
            }
            if definition.key == "editor.font.family"
                && let Some(missing) = self.missing_font()
            {
                text(
                    ops,
                    x + 10.0,
                    y + 44.0,
                    format!("\u{26a0} \u{201c}{missing}\u{201d} is not installed on this computer."),
                    11.0,
                    color("danger"),
                );
            }
            if definition.restart_required {
                text(
                    ops,
                    x + 10.0,
                    y + 44.0,
                    self.label("settings.restart", "Restart required"),
                    10.0,
                    muted,
                );
            }
            ops.push(DrawOp::Line {
                from: Point {
                    x: bounds.x + sidebar,
                    y: y + row_height,
                },
                to: Point {
                    x: bounds.x + bounds.width,
                    y: y + row_height,
                },
                color: border,
                width: 1.0,
            });
            self.rows.push(Row {
                definition,
                value: Button {
                    id: ViewId(
                        2000 + config::DEFINITIONS
                            .iter()
                            .position(|d| d.key == definition.key)
                            .unwrap() as u64
                            * 2,
                    ),
                    label: definition.title.into(),
                    bounds: control,
                    toggle: false,
                    state: ControlState { disabled, ..state },
                },
                copy: Button {
                    id: ViewId(
                        2001 + config::DEFINITIONS
                            .iter()
                            .position(|d| d.key == definition.key)
                            .unwrap() as u64
                            * 2,
                    ),
                    label: format!("Copy {}", definition.key),
                    bounds: copy,
                    toggle: false,
                    state: copy_state,
                },
            });
        }
        if definitions.is_empty() {
            text(
                ops,
                x,
                top + 12.0,
                if self.query.value().is_empty() {
                    "Edit this category in settings.toml"
                } else {
                    "No matching settings"
                },
                13.0,
                muted,
            );
        }
        let bottom = bounds.y + bounds.height - 44.0;
        self.reset = rect(x + width - 132.0, bottom, 132.0, 30.0);
        self.retry = rect(x + width - 204.0, bottom, 64.0, 30.0);
        self.revert = rect(x + width - 278.0, bottom, 68.0, 30.0);
        self.external_reload = Rect::default();
        self.external_keep = Rect::default();
        if let Some(change) = &self.external_change {
            let conflict_y = bottom - 38.0;
            self.external_reload = rect(x + width - 212.0, conflict_y, 96.0, 30.0);
            self.external_keep = rect(x + width - 108.0, conflict_y, 108.0, 30.0);
            let scope = if change.scope == Scope::Workspace {
                "Workspace"
            } else {
                "User"
            };
            text(
                ops,
                x,
                conflict_y + 8.0,
                &format!("{scope} settings changed on disk"),
                13.0,
                color("danger"),
            );
            for (bounds, label) in [(self.external_reload, "Reload disk"), (self.external_keep, "Keep mine")] {
                ops.push(DrawOp::StrokeRounded(bounds, color("border.interactive"), 3.0, 1.0));
                text(ops, bounds.x + 10.0, bounds.y + 8.0, label, 12.0, foreground);
            }
        }
        let status = self.status_description();
        ops.push(DrawOp::PushClip(rect(x, bottom, (width - 290.0).max(0.0), 36.0)));
        text(
            ops,
            x,
            bottom + 8.0,
            status,
            13.0,
            if self.error.is_some() {
                color("danger")
            } else {
                foreground
            },
        );
        ops.push(DrawOp::PopClip);
        text(
            ops,
            self.revert.x,
            self.revert.y + 8.0,
            self.label("settings.revert", "Revert"),
            12.0,
            if self.can_revert() { foreground } else { muted },
        );
        if matches!(self.current().status, SaveStatus::Failed(_)) {
            text(
                ops,
                self.retry.x,
                self.retry.y + 8.0,
                self.label("settings.retry", "Retry"),
                12.0,
                focus,
            );
        }
        ops.push(DrawOp::StrokeRounded(self.reset, color("border.interactive"), 3.0, 1.0));
        text(
            ops,
            self.reset.x + 12.0,
            self.reset.y + 8.0,
            self.label("settings.reset_section", "Reset section"),
            12.0,
            foreground,
        );
        if self.reset_pending {
            let dialog = self.reset_dialog_bounds();
            ops.push(DrawOp::Fill(dialog, color("surface.elevated")));
            ops.push(DrawOp::Stroke(dialog, focus, 2.0));
            text(
                ops,
                dialog.x + 12.0,
                dialog.y + 18.0,
                self.localizer
                    .format(
                        "settings.reset",
                        &[
                            (
                                "section",
                                &self.label(&format!("settings.category.{}", self.category), &self.category),
                            ),
                            (
                                "scope",
                                &self.label(
                                    if self.scope == Scope::User {
                                        "settings.scope.user"
                                    } else {
                                        "settings.scope.workspace"
                                    },
                                    if self.scope == Scope::User { "User" } else { "Workspace" },
                                ),
                            ),
                        ],
                    )
                    .unwrap_or_else(|_| {
                        format!(
                            "Reset {} in {} settings?",
                            self.category,
                            if self.scope == Scope::User { "User" } else { "Workspace" }
                        )
                    }),
                14.0,
                foreground,
            );
            text(
                ops,
                dialog.x + 12.0,
                dialog.y + 54.0,
                self.label("settings.reset_hint", "Enter: reset section   Escape: cancel"),
                13.0,
                muted,
            );
            for (id, label, x) in [(8011, "Reset", dialog.x + 12.0), (8012, "Cancel", dialog.x + 92.0)] {
                let button = rect(x, dialog.y + 76.0, 72.0, 28.0);
                ops.push(DrawOp::StrokeRounded(
                    button,
                    if self.focus.focused() == Some(ViewId(id)) {
                        focus
                    } else {
                        color("border.interactive")
                    },
                    4.0,
                    1.0,
                ));
                text(
                    ops,
                    x + 8.0,
                    button.y + 6.0,
                    self.label(
                        if id == 8011 {
                            "settings.reset_button"
                        } else {
                            "settings.cancel"
                        },
                        label,
                    ),
                    13.0,
                    foreground,
                );
            }
        }
        let mut shaped_previews: Vec<LayoutId> = Vec::new();
        if let Some(popup) = &self.popup {
            let source = Labels(&popup.labels);
            ops.push(DrawOp::FillRounded(popup.list.bounds, color("surface.elevated"), 4.0));
            ops.push(DrawOp::StrokeRounded(
                popup.list.bounds,
                color("border.interactive"),
                4.0,
                1.0,
            ));
            if popup.font_preview {
                // Each entry names its family in the UI font, so a symbol face
                // (Wingdings, D050000L) stays readable, and previews the face in a
                // short sample at the row's end; a family that will not shape shows
                // no sample. Selected rows use the row selection pair and bar (A11Y-01).
                let row_theme = ui.widgets();
                let padding = Metrics::COMPACT.padding;
                let size = Metrics::COMPACT.font_size;
                ops.push(DrawOp::PushClip(popup.list.bounds));
                for index in popup.list.visible(&source) {
                    let row = popup.list.row_bounds(index);
                    let selected = popup.list.selected == Some(index);
                    if selected {
                        bareline_ui::widgets::paint_selected_row(row, row_theme, ops);
                    }
                    let row_text = if selected { row_theme.selection_text } else { foreground };
                    let label = &popup.labels[index];
                    let family = match popup.values.get(index) {
                        Some(SettingValue::Text(name)) if !name.is_empty() => Some(name.as_str()),
                        _ => None,
                    };
                    let origin = Point {
                        x: row.x + padding,
                        y: row.y + 6.0,
                    };
                    let width = (row.width - padding * 2.0).max(1.0);
                    text(ops, origin.x, origin.y, label.clone(), size, row_text);
                    let sample =
                        family.and_then(|family| backend.shape_with_font_family(FONT_SAMPLE, size, width, family).ok());
                    if let Some(layout) = sample {
                        // The sample sits at the row's end, and only where it
                        // clears the name.
                        let sample_width = backend.layout_size(layout).map_or(f32::INFINITY, |(width, _)| width);
                        let label_end = origin.x + backend.measure_text(label, size).map_or(width, |(width, _)| width);
                        let x = row.x + row.width - padding - sample_width;
                        if x >= label_end + padding {
                            ops.push(DrawOp::Layout {
                                origin: Point { x, y: origin.y },
                                layout,
                                color: row_text,
                            });
                        }
                        shaped_previews.push(layout);
                    }
                }
                ops.push(DrawOp::PopClip);
                if popup.list.state.focused {
                    ops.push(DrawOp::Stroke(popup.list.bounds, focus, 2.0));
                }
            } else {
                popup.list.paint(&source, ui.widgets(), ops);
            }
        }
        // Retire this frame's preview layouts once the frame has been painted.
        self.retired_layouts.append(&mut shaped_previews);
        if let Some(edit) = &mut self.value_edit {
            if let Some(row) = self.rows.iter().find(|row| row.definition.key == edit.key) {
                edit.bounds = row.value.bounds;
            }
            let panel = rect(
                edit.bounds.x - 4.0,
                edit.bounds.y - 4.0,
                edit.bounds.width + 8.0,
                edit.bounds.height + 64.0,
            );
            ops.push(DrawOp::FillRounded(panel, color("surface.elevated"), 4.0));
            ops.push(DrawOp::StrokeRounded(panel, color("border.interactive"), 4.0, 1.0));
            edit.field.draw_with_theme(
                backend,
                edit.bounds,
                self.focus.focused() == Some(ViewId(8007)),
                ui,
                ops,
            )?;
            if let Some(reason) = edit.field.validation() {
                ops.push(DrawOp::PushClip(rect(
                    edit.bounds.x,
                    edit.bounds.y + edit.bounds.height,
                    edit.bounds.width,
                    22.0,
                )));
                text(
                    ops,
                    edit.bounds.x,
                    edit.bounds.y + edit.bounds.height + 2.0,
                    reason,
                    11.0,
                    color("danger"),
                );
                ops.push(DrawOp::PopClip);
            }
            for (id, label, x) in [(8009, "Apply", edit.bounds.x), (8010, "Cancel", edit.bounds.x + 80.0)] {
                let bounds = rect(x, edit.bounds.y + edit.bounds.height + 24.0, 72.0, 28.0);
                ops.push(DrawOp::StrokeRounded(
                    bounds,
                    if self.focus.focused() == Some(ViewId(id)) {
                        focus
                    } else {
                        color("border.interactive")
                    },
                    4.0,
                    1.0,
                ));
                text(
                    ops,
                    x + 8.0,
                    bounds.y + 6.0,
                    self.localizer
                        .format(
                            if id == 8009 {
                                "settings.apply"
                            } else {
                                "settings.cancel"
                            },
                            &[],
                        )
                        .unwrap_or_else(|_| label.into()),
                    13.0,
                    foreground,
                );
            }
        }
        if let Some(edit) = self.collection_edit.as_ref() {
            let panel = rect(
                self.bounds.x + sidebar + 30.0,
                self.bounds.y + 60.0,
                (self.bounds.width - sidebar - 60.0).max(240.0),
                (self.bounds.height - 120.0).max(180.0),
            );
            ops.push(DrawOp::FillRounded(panel, color("surface.elevated"), 6.0));
            ops.push(DrawOp::StrokeRounded(panel, focus, 6.0, 2.0));
            let (title, map, colors, picker_mode, entries, catalog, filter) = {
                (
                    config::DEFINITIONS
                        .iter()
                        .find(|d| d.key == edit.key)
                        .map_or(edit.key, |d| d.title)
                        .to_owned(),
                    edit.map,
                    edit.colors,
                    edit.command_picker(),
                    edit.entries.clone(),
                    edit.catalog.clone(),
                    edit.field.value().to_lowercase(),
                )
            };
            text(ops, panel.x + 14.0, panel.y + 12.0, title, 15.0, foreground);
            let mut remove = Vec::new();
            let rows_top = panel.y + 44.0;
            // A command picker keeps the chip list short so the picker below has
            // room; other editors fill the panel with chips.
            let visible = if picker_mode {
                3
            } else {
                (((panel.height - 150.0) / 28.0).max(0.0)) as usize
            };
            for (index, (key, value)) in entries.iter().take(visible).enumerate() {
                let row_y = rows_top + index as f32 * 28.0;
                let chip = rect(panel.x + 14.0, row_y, panel.width - 60.0, 24.0);
                ops.push(DrawOp::FillRounded(chip, color("surface.chrome"), 12.0));
                ops.push(DrawOp::PushClip(chip));
                if colors {
                    // Color rows carry a swatch of the value they set.
                    if let Ok(parsed) = config::ThemeColor::parse(value) {
                        let swatch = rect(chip.x + 8.0, chip.y + 5.0, 14.0, 14.0);
                        ops.push(DrawOp::FillRounded(swatch, Color(parsed.rgb), 3.0));
                        ops.push(DrawOp::StrokeRounded(swatch, border, 3.0, 1.0));
                    }
                    text(
                        ops,
                        chip.x + 30.0,
                        chip.y + 4.0,
                        format!("{key}  {value}"),
                        12.0,
                        foreground,
                    );
                } else {
                    let label = if picker_mode {
                        // Chips read as command titles, not raw IDs.
                        if value.is_empty() { key.clone() } else { value.clone() }
                    } else if map {
                        format!("{key} = {value}")
                    } else {
                        key.clone()
                    };
                    text(ops, chip.x + 10.0, chip.y + 4.0, label, 12.0, foreground);
                }
                ops.push(DrawOp::PopClip);
                let button = rect(panel.x + panel.width - 40.0, row_y, 24.0, 24.0);
                ops.push(DrawOp::StrokeRounded(button, color("border.interactive"), 4.0, 1.0));
                text(ops, button.x + 8.0, button.y + 3.0, "\u{00d7}", 13.0, muted);
                remove.push(button);
            }
            if entries.len() > visible {
                text(
                    ops,
                    panel.x + 14.0,
                    rows_top + visible as f32 * 28.0 + 4.0,
                    format!("{} more not shown", entries.len() - visible),
                    11.0,
                    muted,
                );
            }
            // The field sits below the chips in picker mode (so the picker list
            // can fill the middle), and near the bottom otherwise.
            let field_y = if picker_mode {
                rows_top + visible as f32 * 28.0 + 20.0
            } else {
                panel.y + panel.height - 76.0
            };
            let field_bounds = rect(panel.x + 14.0, field_y, panel.width - 110.0, 30.0);
            let add = rect(panel.x + panel.width - 88.0, field_y, 74.0, 30.0);
            let apply = rect(panel.x + 14.0, panel.y + panel.height - 38.0, 78.0, 28.0);
            let cancel = rect(panel.x + 100.0, panel.y + panel.height - 38.0, 78.0, 28.0);
            // The command picker: catalog commands matching the filter and not
            // already added, each a clickable row that adds its ID.
            let mut picker = Vec::new();
            if picker_mode {
                let picker_top = field_y + 40.0;
                let picker_bottom = panel.y + panel.height - 46.0;
                let mut row_y = picker_top;
                for (index, (id, cmd_title)) in catalog.iter().enumerate() {
                    if row_y + 24.0 > picker_bottom {
                        break;
                    }
                    if entries.iter().any(|(existing, _)| existing == id) {
                        continue;
                    }
                    if !filter.is_empty()
                        && !cmd_title.to_lowercase().contains(&filter)
                        && !id.to_lowercase().contains(&filter)
                    {
                        continue;
                    }
                    let row = rect(panel.x + 14.0, row_y, panel.width - 28.0, 24.0);
                    ops.push(DrawOp::PushClip(row));
                    text(ops, row.x + 8.0, row.y + 4.0, cmd_title.clone(), 12.0, foreground);
                    ops.push(DrawOp::PopClip);
                    picker.push((row, index));
                    row_y += 26.0;
                }
                if picker.is_empty() {
                    text(
                        ops,
                        panel.x + 14.0,
                        picker_top + 4.0,
                        "No matching commands",
                        12.0,
                        muted,
                    );
                }
            }
            if let Some(edit) = self.collection_edit.as_mut() {
                edit.bounds = panel;
                edit.remove = remove;
                edit.add = add;
                edit.apply = apply;
                edit.cancel = cancel;
                edit.picker = picker;
                edit.field.draw_with_theme(backend, field_bounds, true, ui, ops)?;
            }
            for (bounds, label) in [(add, "Add"), (apply, "Apply"), (cancel, "Cancel")] {
                ops.push(DrawOp::StrokeRounded(bounds, color("border.interactive"), 4.0, 1.0));
                text(ops, bounds.x + 12.0, bounds.y + 6.0, label, 12.0, foreground);
            }
            if let Some(reason) = self.collection_edit.as_ref().and_then(|edit| edit.error.clone()) {
                text(
                    ops,
                    panel.x + 190.0,
                    panel.y + panel.height - 32.0,
                    reason,
                    11.0,
                    color("danger"),
                );
            }
        }
        if self.restart_pending {
            // Restart-required settings say so and offer to relaunch right away
            // instead of silently doing nothing.
            let toast = rect(x, bounds.y + bounds.height - 90.0, width.min(520.0), 38.0);
            ops.push(DrawOp::FillRounded(toast, color("surface.elevated"), 6.0));
            ops.push(DrawOp::StrokeRounded(toast, focus, 6.0, 1.0));
            text(
                ops,
                toast.x + 12.0,
                toast.y + 11.0,
                self.label("settings.restart_toast", "Restart Bareline to apply this change."),
                12.0,
                foreground,
            );
            self.restart_now = rect(toast.x + toast.width - 184.0, toast.y + 5.0, 96.0, 28.0);
            ops.push(DrawOp::StrokeRounded(self.restart_now, focus, 4.0, 1.0));
            text(
                ops,
                self.restart_now.x + 12.0,
                self.restart_now.y + 6.0,
                self.label("settings.restart_now", "Restart now"),
                12.0,
                focus,
            );
            self.restart_dismiss = rect(toast.x + toast.width - 80.0, toast.y + 5.0, 70.0, 28.0);
            ops.push(DrawOp::StrokeRounded(
                self.restart_dismiss,
                color("border.interactive"),
                4.0,
                1.0,
            ));
            text(
                ops,
                self.restart_dismiss.x + 14.0,
                self.restart_dismiss.y + 6.0,
                self.label("settings.restart_later", "Later"),
                12.0,
                foreground,
            );
        }
        if self.value_edit.is_none() && self.popup.is_none() && !self.reset_pending {
            if let Some(id) = self.focused_id().filter(|id| *id != ViewId(8000)) {
                if let Some(node) = self.semantics().into_iter().find(|node| node.id == id) {
                    ops.push(DrawOp::Stroke(node.bounds, focus, 2.0));
                }
            }
        }
        ops.push(DrawOp::PopClip);
        self.refresh_focus();
        Ok(())
    }
}

#[cfg(test)]
mod visual_contract_tests {
    use super::*;
    use bareline_renderer_recording::RecordingBackend;
    #[test]
    fn failed_save_exposes_retry_even_without_an_unrelated_controller_error() {
        let mut controller =
            SettingsController::new(SettingsDocument::empty(Scope::User), None, SystemAppearance::default());
        controller.show();
        controller.current_mut().status = SaveStatus::Failed("Disk full".into());
        assert!(controller.error.is_none());
        assert!(
            controller
                .semantics()
                .iter()
                .any(|node| node.id == ViewId(8005) && node.actions.contains(&SemanticAction::Invoke))
        );
        assert!(controller.status_description().contains("Disk full"));
    }
    #[test]
    fn malformed_persisted_value_reports_its_key_without_resetting_other_values() {
        let document = SettingsDocument::parse(
            b"schema_version=1\n[editor.font]\nsize=999\nfamily='Consolas'\n",
            Scope::User,
        )
        .unwrap();
        let controller = SettingsController::new(document, None, SystemAppearance::default());
        assert!(controller.status_description().contains("editor.font.size"));
        assert_eq!(controller.effective().editor_font_size_pt, 12.0);
        assert_eq!(controller.effective().editor_font_family, "Consolas");
    }
    #[test]
    fn invalid_value_keeps_draft_and_cancel_restores_row_focus() {
        let mut controller = SettingsController::new(
            SettingsDocument::empty(Scope::User),
            None,
            SystemAppearance {
                dark: false,
                high_contrast: false,
                highlight: None,
            },
        );
        controller.show();
        controller
            .draw(
                rect(0.0, 34.0, 1200.0, 660.0),
                &mut RecordingBackend::default(),
                &mut Vec::new(),
            )
            .unwrap();
        let invoker = controller.rows[1].value.id;
        // Row 1 (editor.font.size) now opens a size picker; the "Custom…" entry
        // hands the row to the free-text value editor, where an out-of-range
        // draft must be kept rather than silently applied.
        controller.choose(1);
        let custom = controller
            .popup
            .as_ref()
            .unwrap()
            .labels
            .iter()
            .position(|label| label.as_str() == CUSTOM_ENTRY)
            .unwrap();
        controller.commit_choice("editor.font.size", custom);
        assert_eq!(controller.focused_id(), Some(ViewId(8007)));
        assert!(controller.accessibility_set_value(8007, "999"));
        controller.accessibility_action(8009, true);
        assert!(controller.editing_value());
        assert_eq!(controller.effective().editor_font_size_pt, 12.0);
        assert_eq!(controller.text_field_mut().unwrap().value(), "999");
        controller.accessibility_action(8010, true);
        assert!(!controller.editing_value());
        assert_eq!(controller.focused_id(), Some(invoker));
        controller.request_reset();
        controller.dismiss();
        controller.show();
        assert!(!controller.reset_pending);
        assert_eq!(controller.focused_id(), Some(ViewId(8000)));
    }
    #[test]
    fn migration_reconciliation_retains_an_uncommitted_value_draft() {
        let mut controller =
            SettingsController::new(SettingsDocument::empty(Scope::User), None, SystemAppearance::default());
        controller.show();
        controller
            .draw(
                rect(0.0, 34.0, 1200.0, 660.0),
                &mut RecordingBackend::default(),
                &mut Vec::new(),
            )
            .unwrap();
        controller.choose(1);
        let custom = controller
            .popup
            .as_ref()
            .unwrap()
            .labels
            .iter()
            .position(|label| label.as_str() == CUSTOM_ENTRY)
            .unwrap();
        controller.commit_choice("editor.font.size", custom);
        assert!(controller.accessibility_set_value(8007, "19"));
        let revision = controller.revision;

        assert!(!controller.reconcile_user_document(SettingsDocument::empty(Scope::User), revision));
        assert_eq!(controller.revision, revision);
        assert!(controller.editing_value());
        assert_eq!(controller.text_field_mut().unwrap().value(), "19");
    }
    #[test]
    fn switches_are_check_boxes_and_header_actions_are_buttons() {
        let mut controller =
            SettingsController::new(SettingsDocument::empty(Scope::User), None, SystemAppearance::default());
        controller.show();
        let mut backend = RecordingBackend::default();
        let mut ops = Vec::new();
        controller
            .draw(rect(0.0, 34.0, 1200.0, 660.0), &mut backend, &mut ops)
            .unwrap();
        let row = controller
            .rows
            .iter()
            .find(|row| matches!(row.definition.kind, SettingKind::Boolean))
            .expect("the first category has a switch");
        let (id, key) = (row.value.id, row.definition.key);
        let on = |controller: &SettingsController| {
            matches!(
                controller.effective().setting_value(key),
                Some(SettingValue::Bool(true))
            )
        };
        let before = on(&controller);
        let node = controller.semantics().into_iter().find(|node| node.id == id).unwrap();
        assert_eq!(node.role, SemanticRole::Checkbox);
        assert_eq!(node.selected, before);
        assert!(node.actions.contains(&SemanticAction::Invoke));
        controller.accessibility_action(id.0, true);
        assert_eq!(on(&controller), !before);
        let node = controller.semantics().into_iter().find(|node| node.id == id).unwrap();
        assert_eq!(node.selected, !before);
        for id in [8024, 8025] {
            let node = controller.semantics().into_iter().find(|node| node.id.0 == id).unwrap();
            assert_eq!(node.role, SemanticRole::Button);
            assert!(node.actions.contains(&SemanticAction::Invoke) && node.bounds.width > 0.0);
        }
        assert_eq!(
            controller.accessibility_action(8025, true),
            Some(SettingsEffect::OpenToml(Scope::User))
        );
        assert_eq!(controller.accessibility_action(8024, true), Some(SettingsEffect::Close));
        assert!(!controller.open);
    }
    #[test]
    fn settings_reference_order_copy_target_and_opaque_popup() {
        let mut controller = SettingsController::new(
            SettingsDocument::empty(Scope::User),
            None,
            SystemAppearance {
                dark: true,
                high_contrast: false,
                highlight: None,
            },
        );
        controller.show();
        let mut backend = RecordingBackend::default();
        let mut ops = Vec::new();
        controller
            .draw(rect(0.0, 34.0, 1200.0, 660.0), &mut backend, &mut ops)
            .unwrap();
        assert_eq!(
            controller.rows.iter().map(|row| row.definition.key).collect::<Vec<_>>(),
            [
                "editor.font.family",
                "editor.font.size",
                "editor.line_numbers",
                "editor.wrap.mode",
                "editor.render.whitespace",
                "editor.tab.width",
                "editor.insert_spaces"
            ]
        );
        let copy = controller.rows[0].copy.bounds;
        assert_eq!(
            controller.event(UiEvent::PointerDown(Point {
                x: copy.x + 1.0,
                y: copy.y + 1.0
            })),
            None
        );
        assert_eq!(
            controller.event(UiEvent::PointerUp(Point {
                x: copy.x + 1.0,
                y: copy.y + 1.0
            })),
            Some(SettingsEffect::CopyKey("editor.font.family".into()))
        );
        // Row 3 (editor.wrap.mode) is a choice picker; rows 2/6 are now booleans
        // that toggle in place without raising a popup.
        controller.choose(3);
        let popup_bounds = controller.popup.as_ref().unwrap().list.bounds;
        ops.clear();
        controller
            .draw(rect(0.0, 34.0, 1200.0, 660.0), &mut backend, &mut ops)
            .unwrap();
        assert!(
            ops.iter()
                .any(|op| matches!(op, DrawOp::FillRounded(bounds, _, _) if *bounds == popup_bounds))
        );
        assert!(bareline_renderer::balanced_clips(&ops));
    }
    #[test]
    fn choice_popups_paint_the_selected_row_as_the_editor_composite() {
        let system = SystemAppearance {
            dark: true,
            high_contrast: false,
            highlight: None,
        };
        let mut controller = SettingsController::new(SettingsDocument::empty(Scope::User), None, system);
        controller.show();
        let mut backend = RecordingBackend::default();
        let mut ops = Vec::new();
        let bounds = rect(0.0, 34.0, 1200.0, 660.0);
        controller.draw(bounds, &mut backend, &mut ops).unwrap();
        let effective = controller.effective();
        let theme = config::Theme::resolve(effective.theme, system, &effective.theme_overrides).unwrap();
        let editor = theme.color("surface.editor").unwrap();
        let raw = theme.color("selection.row").unwrap();
        // The default dark row band is translucent; its bare rgb is the accent,
        // which the row text and the focus bar cannot be read against.
        assert!(raw.alpha < 255);
        let band = raw.composite(editor);
        assert_ne!(band.rgb, raw.rgb);
        let row_text = theme.color("selection.row.text").unwrap().composite(editor);
        // Row 0 (editor.font.family) previews faces; row 3 (editor.wrap.mode)
        // is a plain choice list.
        for index in [0, 3] {
            controller.popup = None;
            controller.choose(index);
            let popup = controller.popup.as_ref().unwrap();
            assert_eq!(popup.font_preview, index == 0);
            let row = popup.list.row_bounds(popup.list.selected.unwrap());
            ops.clear();
            controller.draw(bounds, &mut backend, &mut ops).unwrap();
            let fill = ops
                .iter()
                .position(|op| matches!(op, DrawOp::Fill(area, _) if *area == row))
                .expect("the selected row is filled");
            assert_eq!(ops[fill], DrawOp::Fill(row, Color(band.rgb)), "popup {index}");
            let DrawOp::Fill(_, bar) = &ops[fill + 1] else {
                panic!("the selected row has a focus bar");
            };
            assert!(config::ThemeColor::opaque(bar.0).contrast(band) >= 3.0, "popup {index}");
            let label = ops[fill..]
                .iter()
                .find_map(|op| match op {
                    DrawOp::Text { color, .. } | DrawOp::Layout { color, .. } => Some(*color),
                    _ => None,
                })
                .expect("the selected row has a label");
            assert_eq!(label, Color(row_text.rgb), "popup {index}");
            assert!(
                config::ThemeColor::opaque(label.0).contrast(band) >= 4.5,
                "popup {index}"
            );
        }
    }
    /// Robustness LNX-UI-013: notifications covered the page's status line and
    /// its Revert, Retry and Reset buttons; the shell keeps them above this.
    #[test]
    fn the_open_page_reports_where_its_footer_starts() {
        let mut controller =
            SettingsController::new(SettingsDocument::empty(Scope::User), None, SystemAppearance::default());
        assert_eq!(controller.footer_top(), None);
        controller.show();
        let bounds = rect(0.0, 34.0, 1200.0, 660.0);
        controller
            .draw(bounds, &mut RecordingBackend::default(), &mut Vec::new())
            .unwrap();
        let top = controller.footer_top().expect("an open, drawn page has a footer");
        assert_eq!(top, controller.revert.y);
        assert!(top > bounds.y && top + 30.0 <= bounds.y + bounds.height);
        controller.open = false;
        assert_eq!(controller.footer_top(), None);
    }
    #[test]
    fn font_picker_names_every_face_in_the_ui_font_and_previews_it_in_a_sample() {
        let mut controller =
            SettingsController::new(SettingsDocument::empty(Scope::User), None, SystemAppearance::default());
        controller.set_font_families(vec![("D050000L".into(), false), ("DejaVu Sans Mono".into(), true)]);
        controller.show();
        let mut backend = RecordingBackend::default();
        let mut ops = Vec::new();
        let bounds = rect(0.0, 34.0, 1200.0, 660.0);
        // Rows exist once drawn; row 0 is editor.font.family.
        controller.draw(bounds, &mut backend, &mut ops).unwrap();
        controller.choose(0);
        assert!(controller.popup.as_ref().unwrap().font_preview);
        ops.clear();
        controller.draw(bounds, &mut backend, &mut ops).unwrap();
        // A symbol face cannot draw its own name: the name is UI-font text.
        for name in ["D050000L", "DejaVu Sans Mono  ·  monospaced"] {
            assert!(
                ops.iter()
                    .any(|op| matches!(op, DrawOp::Text { text, .. } if text.as_str() == name)),
                "{name} is drawn in the UI font"
            );
        }
        // A face still shows itself, in a sample beside a name it clears.
        assert!(ops.iter().any(|op| matches!(op, DrawOp::Layout { .. })));
    }
}
