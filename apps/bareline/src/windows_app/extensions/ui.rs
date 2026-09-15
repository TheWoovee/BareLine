// SPDX-License-Identifier: MPL-2.0
//! Native manager controls share the bounded TextField model and expose the same
//! hit targets through accessibility. IDs are local UI identities, not commands.
use super::*;
use bareline_platform::accessibility::{AccessibilityAction, AccessibilityNode, AccessibilityRole};
use bareline_renderer::{DrawOp, Rect};
use bareline_ui::{rect, text_field::TextField};
const ROOT: u64 = 60000;
const FIELD: u64 = 61000;
// Catalogs contain at most 4096 entries. Keep both dynamic ranges separate from
// the manager's fixed controls and other panels (Language starts at 70000).
const PACKAGE_ROW_BASE: u64 = 8_000_000;
const PACKAGE_ACTION_BASE: u64 = 8_010_000;
// The enable/disable switch on each installed card needs its own stable identity,
// kept clear of the row-select and per-row action ranges above (catalogs hold at
// most 4096 entries, so each base has room for the full index span).
const PACKAGE_TOGGLE_BASE: u64 = 8_020_000;
#[derive(Clone)]
struct Control {
    id: u64,
    label: String,
    action: String,
    bounds: Rect,
    disabled: bool,
    selected: bool,
    role: AccessibilityRole,
}
/// Guest panel output is untrusted and may be a single megabyte-long line. Rendering
/// and the accessibility tree both take a bounded excerpt (SEC-10).
const PANEL_LINE_LIMIT: usize = 4096;
const PANEL_OUTPUT_EXCERPT: usize = 64 * 1024;
fn panel_excerpt(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
/// Turn a package identifier (e.g. `bareline.json-tools`) into a plain-language
/// name for the card heading. The manifest carries no separate display name, so
/// the identifier's words are capitalised and joined with spaces.
fn friendly_name(id: &str) -> String {
    let mut name = String::with_capacity(id.len());
    for word in id.split(['.', '_', '-', ' ']) {
        if word.is_empty() {
            continue;
        }
        if !name.is_empty() {
            name.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            name.extend(first.to_uppercase());
            name.push_str(chars.as_str());
        }
    }
    if name.is_empty() { id.to_string() } else { name }
}

pub(super) struct ManagerUi {
    pub(super) theme: bareline_ui::theme::UiTheme,
    fields: Vec<TextField>,
    controls: Vec<Control>,
    focus: u64,
    pressed: Option<u64>,
    arguments_open: bool,
    field_page: usize,
    results_open: bool,
    result_page: usize,
    caret: Option<Rect>,
}
impl Default for ManagerUi {
    fn default() -> Self {
        Self {
            theme: Default::default(),
            fields: (0..65).map(|_| TextField::default()).collect(),
            controls: vec![],
            focus: ROOT,
            pressed: None,
            arguments_open: false,
            field_page: 0,
            results_open: false,
            result_page: 0,
            caret: None,
        }
    }
}
impl ExtensionsRuntime {
    pub fn ime_caret(&self) -> Option<Rect> {
        self.open.then_some(self.ui.caret).flatten()
    }
    pub(super) fn argument_text(&self) -> Result<String, String> {
        let value = self
            .ui
            .fields
            .iter()
            .map(TextField::value)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if value.len() > 4096 {
            Err("Command arguments exceed 4096 bytes".into())
        } else {
            Ok(value)
        }
    }
    pub(super) fn visible_rows(&self) -> Vec<(usize, String)> {
        if matches!(self.tab, 1 | 2) {
            self.catalog
                .as_ref()
                .map(|catalog| {
                    catalog
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| {
                            self.tab != 2
                                || self
                                    .installed
                                    .iter()
                                    .any(|row| row.package.id == entry.id && row.package.version != entry.version)
                        })
                        .map(|(index, entry)| (index, format!("{} {}", entry.id, entry.version)))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            self.installed
                .iter()
                .enumerate()
                .filter(|(_, row)| self.tab != 3 || !row.state.enabled)
                .map(|(index, row)| (index, format!("{} {}", row.package.id, row.package.version)))
                .collect()
        }
    }
    fn control(
        &mut self,
        ops: &mut Vec<DrawOp>,
        id: u64,
        label: impl Into<String>,
        action: impl Into<String>,
        bounds: Rect,
        disabled: bool,
        selected: bool,
        role: AccessibilityRole,
    ) {
        let label = label.into();
        ops.push(DrawOp::FillRounded(
            bounds,
            if selected {
                self.ui.theme.elevated
            } else {
                self.ui.theme.chrome
            },
            4.0,
        ));
        ops.push(DrawOp::StrokeRounded(
            bounds,
            if self.ui.focus == id || selected {
                self.ui.theme.focus
            } else {
                self.ui.theme.border
            },
            1.0,
            4.0,
        ));
        ops.push(DrawOp::PushClip(bounds));
        ops.push(text(
            bounds.x + 8.0,
            bounds.y + 9.0,
            &label,
            13.0,
            if disabled {
                self.ui.theme.muted
            } else {
                self.ui.theme.text
            },
        ));
        ops.push(DrawOp::PopClip);
        self.ui.controls.push(Control {
            id,
            label,
            action: action.into(),
            bounds,
            disabled,
            selected,
            role,
        });
    }
    pub(super) fn draw_manager(
        &mut self,
        renderer: &mut impl bareline_renderer::TextBackend,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) {
        if !self.open {
            return;
        }
        self.bounds = rect(0.0, 82.0, width, (height - 112.0).max(0.0));
        self.ui.controls.clear();
        self.ui.caret = None;
        // Full-panel background so no editor text shows through behind the
        // Extensions manager (UX-52: "no editor sliver behind it").
        ops.push(DrawOp::Fill(rect(0.0, 0.0, width, height), self.ui.theme.chrome));
        ops.push(DrawOp::Fill(self.bounds, self.ui.theme.chrome));
        let sidebar = (width * 0.186).clamp(140.0, 296.0);
        let x = sidebar + 24.0;
        let w = (width - x - 24.0).max(120.0);
        ops.push(text(20.0, 106.0, "Extensions", 16.0, self.ui.theme.focus));
        ops.push(DrawOp::FillRounded(
            rect(10.0, 136.0, sidebar - 20.0, 48.0),
            self.ui.theme.elevated,
            6.0,
        ));
        ops.push(text(30.0, 152.0, "Extensions", 16.0, self.ui.theme.text));
        ops.push(text(x, 102.0, "Extensions", 22.0, self.ui.theme.text));
        for (index, label) in ["Installed", "Discover", "Updates", "Disabled"].iter().enumerate() {
            self.control(
                ops,
                ROOT + 1 + index as u64,
                *label,
                format!("tab:{index}"),
                rect(x + index as f32 * 110.0, 144.0, 104.0, 36.0),
                false,
                self.tab == index,
                AccessibilityRole::Tab,
            );
        }
        self.control(
            ops,
            60009,
            "Close",
            "extensions.close",
            rect(width - 90.0, 96.0, 70.0, 32.0),
            false,
            false,
            AccessibilityRole::Button,
        );
        // Honest status when this build carries no owner trust pin
        // (UX-52/ARCH-01). Drawn inline (not an early return) so the panel's
        // controls and accessibility tree stay intact.
        if !super::trust_available() {
            ops.push(text(
                x,
                166.0,
                "Extensions require a signed runtime; not available in this build.",
                13.0,
                self.ui.theme.muted,
            ));
        }
        ops.push(DrawOp::StrokeRounded(
            rect(x, 192.0, w, 54.0),
            self.ui.theme.border,
            1.0,
            8.0,
        ));
        ops.push(text(
            x + 20.0,
            210.0,
            "Extensions run isolated in a separate process.",
            16.0,
            self.ui.theme.text,
        ));
        ops.push(text(x, 266.0, "Runtime", 16.0, self.ui.theme.text));
        ops.push(DrawOp::FillRounded(
            rect(x, 292.0, w, 76.0),
            self.ui.theme.elevated,
            8.0,
        ));
        ops.push(text(
            x + 20.0,
            310.0,
            if self.running() {
                "Runtime installed; host running"
            } else if self.runtime_package.is_some() {
                "Runtime installed; host stopped"
            } else {
                "Runtime not installed"
            },
            16.0,
            self.ui.theme.text,
        ));
        ops.push(text(
            x + 20.0,
            337.0,
            "Bareline project · verified offline runtime",
            13.0,
            self.ui.theme.muted,
        ));
        let busy = self.manager_pending.is_some();
        self.control(
            ops,
            60010,
            if self.runtime_package.is_some() {
                "Remove runtime"
            } else {
                "Install runtime"
            },
            if self.runtime_package.is_some() {
                "extensions.remove_runtime"
            } else {
                "extensions.runtime_catalog"
            },
            rect(x + w - 160.0, 308.0, 148.0, 36.0),
            busy || !self.enabled,
            false,
            AccessibilityRole::Button,
        );
        self.control(
            ops,
            60011,
            "Open signed catalog",
            "extensions.catalog",
            rect(x, 382.0, 170.0, 32.0),
            busy || !self.enabled,
            false,
            AccessibilityRole::Button,
        );
        self.control(
            ops,
            60012,
            "Arguments…",
            "arguments",
            rect(x + 180.0, 382.0, 120.0, 32.0),
            false,
            self.ui.arguments_open,
            AccessibilityRole::Button,
        );
        self.control(
            ops,
            60013,
            "Cancel",
            "extensions.cancel",
            rect(x + 310.0, 382.0, 90.0, 32.0),
            !self.running() && !busy,
            false,
            AccessibilityRole::Button,
        );
        self.control(
            ops,
            60018,
            "Results",
            "results",
            rect(x + 410.0, 382.0, 90.0, 32.0),
            self.panel_output.is_empty(),
            self.ui.results_open,
            AccessibilityRole::Button,
        );
        let body_bottom = (height - 130.0).max(520.0);
        if self.ui.results_open {
            let lines = ((body_bottom - 450.0) / 20.0).floor().max(1.0) as usize;
            let start = self.ui.result_page;
            ops.push(DrawOp::PushClip(rect(x, 430.0, w, (body_bottom - 438.0).max(0.0))));
            for (index, line) in self.panel_output.lines().skip(start).take(lines).enumerate() {
                ops.push(text(
                    x,
                    434.0 + index as f32 * 20.0,
                    panel_excerpt(line, PANEL_LINE_LIMIT),
                    13.0,
                    self.ui.theme.text,
                ));
            }
            ops.push(DrawOp::PopClip);
            self.control(
                ops,
                60025,
                "Previous results",
                "results_prev",
                rect(x, body_bottom, 140.0, 32.0),
                start == 0,
                false,
                AccessibilityRole::Button,
            );
            self.control(
                ops,
                60026,
                "Next results",
                "results_next",
                rect(x + 150.0, body_bottom, 130.0, 32.0),
                self.panel_output.lines().skip(start + lines).next().is_none(),
                false,
                AccessibilityRole::Button,
            );
            self.control(
                ops,
                60027,
                "Copy results",
                "results_copy",
                rect(x + 290.0, body_bottom, 130.0, 32.0),
                false,
                false,
                AccessibilityRole::Button,
            );
        } else if self.ui.arguments_open {
            ops.push(text(
                x,
                432.0,
                "Arguments: XPath query then prefix=URI; JSON/Hex key=value per line.",
                13.0,
                self.ui.theme.muted,
            ));
            let count = ((body_bottom - 484.0) / 38.0).floor().clamp(1.0, 8.0) as usize;
            let start = self.ui.field_page.min(64);
            for index in start..(start + count).min(65) {
                let bounds = rect(x + 70.0, 458.0 + (index - start) as f32 * 38.0, w - 70.0, 32.0);
                ops.push(text(
                    x,
                    bounds.y + 8.0,
                    format!("Line {}", index + 1),
                    13.0,
                    self.ui.theme.muted,
                ));
                let focused = self.ui.focus == FIELD + index as u64;
                match self.ui.fields[index].draw_with_theme(renderer, bounds, focused, self.ui.theme, ops) {
                    Ok(caret) if focused => self.ui.caret = Some(caret),
                    Ok(_) => {}
                    Err(error) => self.message = Some(format!("Argument layout: {error:?}")),
                }
                self.ui.controls.push(Control {
                    id: FIELD + index as u64,
                    label: format!("Command argument line {}", index + 1),
                    action: String::new(),
                    bounds,
                    disabled: false,
                    selected: false,
                    role: AccessibilityRole::TextField,
                });
            }
            self.control(
                ops,
                60014,
                "Previous lines",
                "args_prev",
                rect(x, body_bottom, 130.0, 32.0),
                start == 0,
                false,
                AccessibilityRole::Button,
            );
            self.control(
                ops,
                60015,
                "Next lines",
                "args_next",
                rect(x + 140.0, body_bottom, 130.0, 32.0),
                start + count >= 65,
                false,
                AccessibilityRole::Button,
            );
        } else {
            let rows = self.visible_rows();
            let selected = rows.iter().position(|(index, _)| *index == self.selected).unwrap_or(0);
            let page = selected / 3 * 3;
            let cw = (w - 24.0) / 3.0;
            let catalog = matches!(self.tab, 1 | 2);
            ops.push(text(
                x,
                410.0,
                if catalog {
                    "Available extensions"
                } else {
                    "Installed extensions"
                },
                14.0,
                self.ui.theme.muted,
            ));
            // A selected installed extension keeps its command controls docked at
            // the foot of the panel; leave room for them so cards never overlap.
            let has_command_row = !catalog && self.installed.get(self.selected).is_some();
            let card_top = 432.0;
            let card_bottom = if has_command_row {
                (body_bottom - 64.0).max(card_top + 150.0)
            } else {
                (body_bottom - 8.0).max(card_top + 150.0)
            };
            let card_h = card_bottom - card_top;
            for (position, (index, label)) in rows.iter().skip(page).take(3).enumerate() {
                let cx = x + position as f32 * (cw + 12.0);
                let card = rect(cx, card_top, cw, card_h);
                let is_selected = *index == self.selected;
                ops.push(DrawOp::FillRounded(card, self.ui.theme.elevated, 8.0));
                ops.push(DrawOp::StrokeRounded(
                    card,
                    if is_selected {
                        self.ui.theme.focus
                    } else {
                        self.ui.theme.border
                    },
                    1.0,
                    8.0,
                ));
                // Owned copies so the immutable borrow of `installed` ends before
                // the `&mut self` control calls below.
                let (name, subtitle, enabled, caps) = if catalog {
                    let (id, version) = label.rsplit_once(' ').unwrap_or((label.as_str(), ""));
                    (friendly_name(id), format!("Version {version}"), None, Vec::new())
                } else if let Some(row) = self.installed.get(*index) {
                    (
                        friendly_name(&row.package.id),
                        format!("{} · Version {}", row.package.manifest.publisher, row.package.version),
                        Some(row.state.enabled),
                        row.package
                            .manifest
                            .capabilities
                            .iter()
                            .map(|cap| cap.name().to_string())
                            .collect::<Vec<_>>(),
                    )
                } else {
                    (label.clone(), String::new(), None, Vec::new())
                };
                let badge: String = name
                    .split_whitespace()
                    .filter_map(|word| word.chars().next())
                    .take(2)
                    .collect::<String>()
                    .to_uppercase();
                ops.push(DrawOp::PushClip(card));
                let icon = rect(cx + 16.0, card_top + 14.0, 42.0, 42.0);
                ops.push(DrawOp::FillRounded(icon, self.ui.theme.chrome, 8.0));
                ops.push(DrawOp::StrokeRounded(icon, self.ui.theme.border, 1.0, 8.0));
                ops.push(text(icon.x + 11.0, icon.y + 14.0, badge, 14.0, self.ui.theme.focus));
                ops.push(text(
                    cx + 70.0,
                    card_top + 20.0,
                    name.as_str(),
                    15.0,
                    self.ui.theme.text,
                ));
                ops.push(text(
                    cx + 70.0,
                    card_top + 40.0,
                    subtitle.as_str(),
                    12.0,
                    self.ui.theme.muted,
                ));
                // Whole header selects the card (no button chrome over the name).
                self.ui.controls.push(Control {
                    id: PACKAGE_ROW_BASE + *index as u64,
                    label: if subtitle.is_empty() {
                        name.clone()
                    } else {
                        format!("{name}, {subtitle}")
                    },
                    action: format!("select:{index}"),
                    bounds: rect(cx + 8.0, card_top + 8.0, cw - 16.0, 56.0),
                    disabled: false,
                    selected: is_selected,
                    role: AccessibilityRole::ListItem,
                });
                if catalog {
                    ops.push(DrawOp::PopClip);
                    self.control(
                        ops,
                        PACKAGE_ACTION_BASE + *index as u64,
                        "Install or update",
                        format!("install:{index}"),
                        rect(cx + 16.0, card_top + 74.0, cw - 32.0, 34.0),
                        busy || self.running() || !self.enabled,
                        false,
                        AccessibilityRole::Button,
                    );
                } else if let Some(enabled) = enabled {
                    let toggle_y = card_top + 70.0;
                    ops.push(text(
                        cx + 16.0,
                        toggle_y + 5.0,
                        if enabled { "Enabled" } else { "Disabled" },
                        13.0,
                        self.ui.theme.text,
                    ));
                    let toggle = rect(cx + 96.0, toggle_y, 44.0, 22.0);
                    ops.push(DrawOp::FillRounded(
                        toggle,
                        if enabled {
                            self.ui.theme.focus
                        } else {
                            self.ui.theme.border
                        },
                        11.0,
                    ));
                    let knob = rect(toggle.x + if enabled { 25.0 } else { 3.0 }, toggle.y + 3.0, 16.0, 16.0);
                    ops.push(DrawOp::FillRounded(knob, self.ui.theme.text, 8.0));
                    self.ui.controls.push(Control {
                        id: PACKAGE_TOGGLE_BASE + *index as u64,
                        label: if enabled {
                            "Disable extension".into()
                        } else {
                            "Enable extension".into()
                        },
                        action: format!("permission:{index}"),
                        bounds: toggle,
                        disabled: busy || !self.enabled,
                        selected: enabled,
                        role: AccessibilityRole::Checkbox,
                    });
                    // Capabilities as labelled chips; the first names the format.
                    ops.push(text(
                        cx + 16.0,
                        card_top + 122.0,
                        "Capabilities",
                        12.0,
                        self.ui.theme.muted,
                    ));
                    let mut chip_x = cx + 16.0;
                    let mut chip_y = card_top + 140.0;
                    for (ci, cap) in caps.iter().enumerate() {
                        let chip_w = cap.chars().count() as f32 * 7.0 + 18.0;
                        if chip_x + chip_w > cx + cw - 12.0 && chip_x > cx + 16.0 {
                            chip_x = cx + 16.0;
                            chip_y += 26.0;
                        }
                        let chip = rect(chip_x, chip_y, chip_w, 22.0);
                        ops.push(DrawOp::FillRounded(
                            chip,
                            if ci == 0 {
                                self.ui.theme.selection
                            } else {
                                self.ui.theme.chrome
                            },
                            4.0,
                        ));
                        ops.push(text(
                            chip_x + 9.0,
                            chip_y + 5.0,
                            cap.as_str(),
                            12.0,
                            if ci == 0 {
                                self.ui.theme.focus
                            } else {
                                self.ui.theme.text
                            },
                        ));
                        chip_x += chip_w + 8.0;
                    }
                    ops.push(DrawOp::PopClip);
                    self.control(
                        ops,
                        PACKAGE_ACTION_BASE + *index as u64,
                        "Permissions",
                        format!("review:{index}"),
                        rect(cx + cw - 126.0, toggle_y - 5.0, 110.0, 32.0),
                        busy || !self.enabled,
                        false,
                        AccessibilityRole::Button,
                    );
                } else {
                    ops.push(DrawOp::PopClip);
                }
            }
            if rows.is_empty() {
                ops.push(text(
                    x,
                    444.0,
                    if catalog {
                        "Open a signed offline catalog to view available packages."
                    } else {
                        "No extensions in this view."
                    },
                    15.0,
                    self.ui.theme.muted,
                ));
            }
            if has_command_row {
                let command = self.installed[self.selected]
                    .package
                    .manifest
                    .commands
                    .get(self.command_selection)
                    .cloned()
                    .unwrap_or_default();
                ops.push(text(
                    x,
                    body_bottom - 52.0,
                    format!("Command: {command}"),
                    13.0,
                    self.ui.theme.text,
                ));
                for (i, (label, action)) in [
                    ("Next command", "extensions.next_command"),
                    ("Run", "extensions.run_selected"),
                    ("Background (120s)", "extensions.run_background"),
                    ("Remove", "extensions.remove"),
                    ("Approve reviewed", "extensions.approve"),
                ]
                .iter()
                .enumerate()
                {
                    self.control(
                        ops,
                        60020 + i as u64,
                        *label,
                        *action,
                        rect(x + i as f32 * (w / 5.0), body_bottom - 34.0, w / 5.0 - 6.0, 30.0),
                        busy || !self.enabled
                            || (*action == "extensions.approve" && self.permission_review != Some(self.selected)),
                        false,
                        AccessibilityRole::Button,
                    );
                }
            }
        }
        if let Some(message) = &self.message {
            ops.push(text(x, height - 85.0, message, 13.0, self.ui.theme.text));
        }
        ops.push(text(
            x,
            height - 55.0,
            "Disabling extensions stops the host. Removing the runtime frees disk space.",
            13.0,
            self.ui.theme.muted,
        ));
    }
}
impl super::super::Shell {
    fn extension_control(&mut self, el: &super::super::ActiveEventLoop, id: u64) -> bool {
        let Some(control) = self
            .extensions
            .ui
            .controls
            .iter()
            .find(|control| control.id == id && !control.disabled)
            .cloned()
        else {
            return false;
        };
        self.extensions.ui.focus = id;
        let action = control.action.as_str();
        if let Some(tab) = action
            .strip_prefix("tab:")
            .and_then(|value| value.parse::<usize>().ok())
        {
            self.extensions.tab = tab;
            self.extensions.selected = 0;
            self.extensions.permission_review = None;
        } else if let Some(index) = action
            .strip_prefix("select:")
            .and_then(|value| value.parse::<usize>().ok())
        {
            self.extensions.selected = index;
            self.extensions.command_selection = 0;
            self.extensions.permission_review = None;
        } else if let Some(index) = action
            .strip_prefix("install:")
            .and_then(|value| value.parse::<usize>().ok())
        {
            self.extensions.selected = index;
            self.extensions_dispatch(el, "extensions.install");
        } else if let Some(index) = action
            .strip_prefix("permission:")
            .and_then(|value| value.parse::<usize>().ok())
        {
            self.extensions.selected = index;
            let enabled = self
                .extensions
                .installed
                .get(index)
                .is_some_and(|row| row.state.enabled);
            self.extensions_dispatch(
                el,
                if enabled {
                    "extensions.disable"
                } else {
                    "extensions.permissions"
                },
            );
        } else if let Some(index) = action
            .strip_prefix("review:")
            .and_then(|value| value.parse::<usize>().ok())
        {
            self.extensions.selected = index;
            self.extensions_dispatch(el, "extensions.permissions");
        } else {
            match action {
                "arguments" => {
                    self.extensions.ui.arguments_open = !self.extensions.ui.arguments_open;
                    self.extensions.ui.results_open = false;
                }
                "results" => {
                    self.extensions.ui.results_open = !self.extensions.ui.results_open;
                    self.extensions.ui.arguments_open = false;
                }
                "results_prev" => self.extensions.ui.result_page = self.extensions.ui.result_page.saturating_sub(4),
                "results_next" => self.extensions.ui.result_page = self.extensions.ui.result_page.saturating_add(4),
                "results_copy" => {
                    if let Some(platform) = &self.platform
                        && let Err(error) = platform.set_clipboard_text(&self.extensions.panel_output)
                    {
                        self.extensions.message = Some(error.to_string());
                    }
                }
                "args_prev" => self.extensions.ui.field_page = self.extensions.ui.field_page.saturating_sub(4),
                "args_next" => self.extensions.ui.field_page = (self.extensions.ui.field_page + 4).min(64),
                "previous" => self.extensions.move_selection(-1),
                "next" => self.extensions.move_selection(1),
                "" => {}
                action => {
                    self.extensions_dispatch(el, action);
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn extensions_ui_event(
        &mut self,
        el: &super::super::ActiveEventLoop,
        event: &super::super::WindowEvent,
    ) -> bool {
        use super::super::{ElementState, Key, MouseButton, NamedKey, WindowEvent};
        use winit::event::Ime;
        if !self.extensions.open || self.palette.open {
            return false;
        }
        let field_index = self
            .extensions
            .ui
            .focus
            .checked_sub(FIELD)
            .filter(|index| *index < 65)
            .map(|index| index as usize);
        match event {
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let hit = self
                    .extensions
                    .ui
                    .controls
                    .iter()
                    .find(|control| !control.disabled && control.bounds.contains(self.pointer))
                    .map(|control| control.id);
                if *state == ElementState::Pressed {
                    if let Some(index) = field_index {
                        self.extensions.ui.fields[index].cancel();
                    }
                    self.extensions.ui.pressed = hit;
                    if let Some(id) = hit {
                        self.extensions.ui.focus = id;
                    }
                    if let Some(index) = hit.and_then(|id| id.checked_sub(FIELD)).filter(|index| *index < 65)
                        && let Some(renderer) = &self.renderer
                    {
                        let _ = self.extensions.ui.fields[index as usize].click(
                            renderer,
                            self.pointer,
                            self.modifiers.shift_key(),
                        );
                    }
                } else if let Some(id) = self.extensions.ui.pressed.take().filter(|id| Some(*id) == hit) {
                    self.extension_control(el, id);
                }
            }
            WindowEvent::Ime(Ime::Preedit(value, cursor)) => {
                if let Some(index) = field_index {
                    self.extensions.ui.fields[index].preedit(value.clone(), *cursor);
                }
            }
            WindowEvent::Ime(Ime::Commit(value)) => {
                if let Some(index) = field_index {
                    self.extensions.ui.fields[index].commit(value);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let ctrl = self.modifiers.control_key();
                let shift = self.modifiers.shift_key();
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        if let Some(index) = field_index
                            && self.extensions.ui.fields[index].composing()
                        {
                            self.extensions.ui.fields[index].cancel();
                        } else {
                            self.extensions.open = false;
                        }
                    }
                    Key::Named(NamedKey::Tab) => {
                        if let Some(index) = field_index {
                            self.extensions.ui.fields[index].cancel();
                        }
                        let controls: Vec<u64> = self
                            .extensions
                            .ui
                            .controls
                            .iter()
                            .filter(|control| !control.disabled)
                            .map(|control| control.id)
                            .collect();
                        if !controls.is_empty() {
                            let old = controls
                                .iter()
                                .position(|id| *id == self.extensions.ui.focus)
                                .unwrap_or(0);
                            self.extensions.ui.focus = controls[(old as isize + if shift { -1 } else { 1 })
                                .rem_euclid(controls.len() as isize)
                                as usize];
                        }
                    }
                    _ if field_index.is_some() => {
                        let field = &mut self.extensions.ui.fields[field_index.unwrap()];
                        if !field.composing() {
                            match &event.logical_key {
                                Key::Named(NamedKey::Backspace) => {
                                    field.delete(false);
                                }
                                Key::Named(NamedKey::Delete) => {
                                    field.delete(true);
                                }
                                Key::Named(NamedKey::ArrowLeft) => field.horizontal(false, shift),
                                Key::Named(NamedKey::ArrowRight) => field.horizontal(true, shift),
                                Key::Named(NamedKey::Home) => field.edge(false, shift),
                                Key::Named(NamedKey::End) => field.edge(true, shift),
                                Key::Character(key) if ctrl && !self.modifiers.alt_key() => {
                                    match key.to_ascii_lowercase().as_str() {
                                        "a" => field.select_all(),
                                        "c" | "x" => {
                                            if let Some(platform) = &self.platform
                                                && platform.set_clipboard_text(field.selected()).is_ok()
                                                && key.eq_ignore_ascii_case("x")
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
                                        "z" => field.undo(shift),
                                        "y" => field.undo(true),
                                        _ => {}
                                    }
                                }
                                Key::Character(value) if !ctrl || self.modifiers.alt_key() => {
                                    field.insert(value);
                                }
                                _ => {}
                            }
                        }
                    }
                    Key::Named(NamedKey::Enter | NamedKey::Space) => {
                        self.extension_control(el, self.extensions.ui.focus);
                    }
                    Key::Named(NamedKey::ArrowDown) => self.extensions.move_selection(1),
                    Key::Named(NamedKey::ArrowUp) => self.extensions.move_selection(-1),
                    _ => {}
                }
            }
            // Lifecycle/redraw events must reach the shell regardless of pointer position.
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(crate) fn extensions_accessibility_nodes(&self) -> Vec<AccessibilityNode> {
        self.extensions.accessibility_nodes()
    }
    pub(crate) fn extensions_accessibility_focus(&self) -> Option<u64> {
        self.extensions.accessibility_focus()
    }
    pub(crate) fn extensions_accessibility(
        &mut self,
        el: &super::super::ActiveEventLoop,
        action: &AccessibilityAction,
    ) -> bool {
        if !self.extensions.open {
            return false;
        }
        if let Some(handled) = self.extensions.accessibility_edit(action) {
            if handled && let Some(window) = &self.window {
                window.request_redraw();
            }
            return handled;
        }
        match action {
            AccessibilityAction::Invoke(id) => self.extension_control(el, *id),
            _ => false,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_panel_output_is_capped_on_char_boundaries() {
        let long = "é".repeat(4096);
        let capped = panel_excerpt(&long, PANEL_LINE_LIMIT);
        assert!(capped.len() <= PANEL_LINE_LIMIT);
        assert!(long.starts_with(capped));
        assert_eq!(panel_excerpt("short", PANEL_LINE_LIMIT), "short");
        let huge = "x".repeat(1024 * 1024);
        assert_eq!(panel_excerpt(&huge, PANEL_OUTPUT_EXCERPT).len(), PANEL_OUTPUT_EXCERPT);
    }
    #[test]
    fn structured_arguments_are_separate_bounded_committed_fields() {
        let mut runtime = ExtensionsRuntime::default();
        runtime.ui.fields[0].insert("//p:item");
        runtime.ui.fields[1].insert("p=urn:fixture");
        runtime.ui.fields[2].preedit("pending".into(), Some((0, 7)));
        assert_eq!(runtime.argument_text().unwrap(), "//p:item\np=urn:fixture");
        runtime.ui.fields[3].insert(&"x".repeat(4096));
        assert!(runtime.argument_text().is_err());
    }
}

impl ExtensionsRuntime {
    fn accessibility_nodes(&self) -> Vec<AccessibilityNode> {
        if !self.open {
            return vec![];
        }
        let bounds = self.bounds;
        let mut nodes = vec![AccessibilityNode {
            id: ROOT,
            parent: 1,
            role: AccessibilityRole::Group,
            name: "Extensions".into(),
            value: None,
            bounds: [
                bounds.x as f64,
                bounds.y as f64,
                bounds.width as f64,
                bounds.height as f64,
            ],
            disabled: false,
            selected: false,
            expanded: None,
            focusable: true,
            invokable: false,
        }];
        nodes.extend(self.ui.controls.iter().map(|control| {
            AccessibilityNode {
                id: control.id,
                parent: ROOT,
                role: control.role,
                name: control.label.clone(),
                value: control
                    .id
                    .checked_sub(FIELD)
                    .filter(|index| *index < 65)
                    .map(|index| self.ui.fields[index as usize].value().to_owned()),
                bounds: [
                    control.bounds.x as f64,
                    control.bounds.y as f64,
                    control.bounds.width as f64,
                    control.bounds.height as f64,
                ],
                disabled: control.disabled,
                selected: control.selected,
                expanded: None,
                focusable: !control.disabled,
                invokable: !control.disabled && control.role != AccessibilityRole::TextField,
            }
        }));
        if let Some(message) = &self.message {
            nodes.push(AccessibilityNode {
                id: 62001,
                parent: ROOT,
                role: AccessibilityRole::Status,
                name: message.clone(),
                value: None,
                bounds: [bounds.x as f64, bounds.y as f64, 0.0, 0.0],
                disabled: false,
                selected: false,
                expanded: None,
                focusable: false,
                invokable: false,
            });
        }
        if !self.panel_output.is_empty() {
            nodes.push(AccessibilityNode {
                id: 62000,
                parent: ROOT,
                role: AccessibilityRole::Group,
                name: "Extension result".into(),
                value: Some(panel_excerpt(&self.panel_output, PANEL_OUTPUT_EXCERPT).to_owned()),
                bounds: [bounds.x as f64, bounds.y as f64, bounds.width as f64, 0.0],
                disabled: false,
                selected: false,
                expanded: None,
                focusable: false,
                invokable: false,
            });
        }
        nodes
    }
    fn accessibility_focus(&self) -> Option<u64> {
        self.open.then_some(
            if self
                .ui
                .controls
                .iter()
                .any(|control| control.id == self.ui.focus && !control.disabled)
            {
                self.ui.focus
            } else {
                ROOT
            },
        )
    }
}

impl ExtensionsRuntime {
    fn accessibility_edit(&mut self, action: &AccessibilityAction) -> Option<bool> {
        if !self.open {
            return Some(false);
        }
        match action {
            AccessibilityAction::Focus(id) => Some(
                if *id == ROOT
                    || self
                        .ui
                        .controls
                        .iter()
                        .any(|control| control.id == *id && !control.disabled)
                {
                    self.ui.focus = *id;
                    true
                } else {
                    false
                },
            ),
            AccessibilityAction::SetValue { id, value } => Some(
                if let Some(index) = id.checked_sub(FIELD).filter(|index| *index < 65)
                    && self.ui.controls.iter().any(|control| control.id == *id)
                    && value.len() <= 4096
                    && !value.chars().any(char::is_control)
                {
                    let field = &mut self.ui.fields[index as usize];
                    field.select_all();
                    field.commit(value);
                    true
                } else {
                    false
                },
            ),
            _ => None,
        }
    }
}
/// Real manager layout/projection fixtures. The package is signed with the same
/// deterministic TEST-ONLY seed used by protocol fixtures and is never executed.
#[cfg(test)]
pub(super) fn accessibility_test_cases() -> Vec<(&'static str, Vec<AccessibilityNode>, Option<u64>)> {
    use bareline_extensions_protocol::{
        Catalog, CatalogPolicy, OfflinePackageSource, PackageRequest, VerifiedPackageSource,
    };
    use bareline_renderer::RenderBackend;
    // TextFields retain backend-owned LayoutIds across frames, as in production.
    let mut backend = bareline_renderer_recording::RecordingBackend::default();
    backend.resize(1000, 800, 1.0).unwrap();
    let mut capture = |name, runtime: &mut ExtensionsRuntime| {
        let mut ops = vec![];
        runtime.draw_manager(&mut backend, 1000.0, 800.0, &mut ops);
        backend.render(&ops).unwrap();
        (name, runtime.accessibility_nodes(), runtime.accessibility_focus())
    };
    let mut runtime = ExtensionsRuntime::default();
    let mut cases = vec![capture("extensions-closed", &mut runtime)];
    runtime.open = true;
    cases.push(capture("extensions-open-runtime-absent", &mut runtime));
    let root = std::env::temp_dir().join(format!(
        "bareline-semantic-package-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let metadata = include_bytes!("fixtures/catalog.json");
    let catalog: Catalog = serde_json::from_slice(metadata).unwrap();
    let digest = catalog.entries[0].sha256.clone();
    let archive = root.join(format!("{digest}.blex"));
    std::fs::write(&archive, include_bytes!("fixtures/manager.blex")).unwrap();
    let policy = CatalogPolicy {
        public_key: include_str!("fixtures/public-key.txt"),
        publisher: "fixture",
        channel: "stable",
        platform: "windows-x64",
        artifact_type: "extension",
        highest_metadata_version: 0,
        now_unix: 100,
    };
    let source = OfflinePackageSource::open(
        root.clone(),
        metadata,
        include_str!("fixtures/catalog.minisig"),
        &policy,
    )
    .unwrap();
    let verified = source
        .fetch(&PackageRequest {
            id: "fixture.tools".into(),
            version: "1".into(),
        })
        .unwrap();
    let package = verified.install(&root, &AtomicBool::new(false)).unwrap();
    runtime.installed.push(InstalledRow {
        state: InstalledState {
            id: package.id.clone(),
            digest,
            version: package.version.clone(),
            approved: package.manifest.capabilities.clone(),
            enabled: true,
            generation: 1,
            command_count: package.manifest.commands.len(),
        },
        package,
    });
    cases.push(capture("extensions-installed-package-runtime-absent", &mut runtime));
    runtime.installed[0].state.enabled = false;
    runtime.tab = 3;
    cases.push(capture("extensions-disabled-package", &mut runtime));
    runtime.tab = 0;
    runtime.ui.arguments_open = true;
    cases.push(capture("extensions-arguments", &mut runtime));
    assert_eq!(
        runtime.accessibility_edit(&AccessibilityAction::Focus(FIELD)),
        Some(true)
    );
    cases.push(capture("extensions-argument-focus", &mut runtime));
    assert_eq!(
        runtime.accessibility_edit(&AccessibilityAction::SetValue {
            id: FIELD,
            value: "//p:item".into()
        }),
        Some(true)
    );
    assert_eq!(
        runtime.accessibility_edit(&AccessibilityAction::SetValue {
            id: FIELD + 1,
            value: "p=urn:fixture".into()
        }),
        Some(true)
    );
    assert_eq!(
        runtime.accessibility_edit(&AccessibilityAction::SetValue {
            id: FIELD,
            value: "forbidden\nline".into()
        }),
        Some(false)
    );
    cases.push(capture("extensions-argument-value", &mut runtime));
    runtime.ui.results_open = true;
    runtime.ui.arguments_open = false;
    runtime.panel_output = "fixture.result\nValidated fixture".into();
    cases.push(capture("extensions-result", &mut runtime));
    runtime.installed.pop().unwrap().package.remove_cached().unwrap();
    std::fs::remove_dir(root).unwrap();
    cases
}
