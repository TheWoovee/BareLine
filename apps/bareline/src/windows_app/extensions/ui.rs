// SPDX-License-Identifier: MPL-2.0
//! Native manager controls share the bounded TextField model and expose the same
//! hit targets through accessibility. IDs are local UI identities, not commands.
use super::*;
use bareline_platform::accessibility::{AccessibilityAction, AccessibilityNode, AccessibilityRole};
use bareline_renderer::{DrawOp, Rect};
use bareline_ui::{ACCENT, BORDER, CHROME, ELEVATED, MUTED, TEXT, rect, text_field::TextField};
const ROOT: u64 = 60000;
const FIELD: u64 = 61000;
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
pub(super) struct ManagerUi {
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
                                || self.installed.iter().any(|row| {
                                    row.package.id == entry.id
                                        && row.package.version != entry.version
                                })
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
            if selected { ELEVATED } else { CHROME },
            4.0,
        ));
        ops.push(DrawOp::StrokeRounded(
            bounds,
            if self.ui.focus == id || selected {
                ACCENT
            } else {
                BORDER
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
            if disabled { MUTED } else { TEXT },
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
        ops.push(DrawOp::Fill(self.bounds, CHROME));
        let sidebar = (width * 0.186).clamp(140.0, 296.0);
        let x = sidebar + 24.0;
        let w = (width - x - 24.0).max(120.0);
        ops.push(text(20.0, 106.0, "Extensions", 16.0, ACCENT));
        ops.push(DrawOp::FillRounded(
            rect(10.0, 136.0, sidebar - 20.0, 48.0),
            ELEVATED,
            6.0,
        ));
        ops.push(text(30.0, 152.0, "Extensions", 16.0, TEXT));
        ops.push(text(x, 102.0, "Extensions", 22.0, TEXT));
        for (index, label) in ["Installed", "Discover", "Updates", "Disabled"]
            .iter()
            .enumerate()
        {
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
        ops.push(DrawOp::StrokeRounded(
            rect(x, 192.0, w, 54.0),
            BORDER,
            1.0,
            8.0,
        ));
        ops.push(text(
            x + 20.0,
            210.0,
            "Extensions run isolated in a separate process.",
            16.0,
            TEXT,
        ));
        ops.push(text(x, 266.0, "Runtime", 16.0, TEXT));
        ops.push(DrawOp::FillRounded(rect(x, 292.0, w, 76.0), ELEVATED, 8.0));
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
            TEXT,
        ));
        ops.push(text(
            x + 20.0,
            337.0,
            "Bareline project · verified offline runtime",
            13.0,
            MUTED,
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
            ops.push(DrawOp::PushClip(rect(
                x,
                430.0,
                w,
                (body_bottom - 438.0).max(0.0),
            )));
            for (index, line) in self
                .panel_output
                .lines()
                .skip(start)
                .take(lines)
                .enumerate()
            {
                ops.push(text(x, 434.0 + index as f32 * 20.0, line, 13.0, TEXT));
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
                self.panel_output
                    .lines()
                    .skip(start + lines)
                    .next()
                    .is_none(),
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
                MUTED,
            ));
            let count = ((body_bottom - 484.0) / 38.0).floor().clamp(1.0, 8.0) as usize;
            let start = self.ui.field_page.min(64);
            for index in start..(start + count).min(65) {
                let bounds = rect(
                    x + 70.0,
                    458.0 + (index - start) as f32 * 38.0,
                    w - 70.0,
                    32.0,
                );
                ops.push(text(
                    x,
                    bounds.y + 8.0,
                    format!("Line {}", index + 1),
                    13.0,
                    MUTED,
                ));
                let focused = self.ui.focus == FIELD + index as u64;
                match self.ui.fields[index].draw(renderer, bounds, focused, ops) {
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
            let selected = rows
                .iter()
                .position(|(index, _)| *index == self.selected)
                .unwrap_or(0);
            let page = selected / 3 * 3;
            let cw = (w - 24.0) / 3.0;
            for (position, (index, label)) in rows.iter().skip(page).take(3).enumerate() {
                let cx = x + position as f32 * (cw + 12.0);
                ops.push(DrawOp::FillRounded(
                    rect(cx, 432.0, cw, (body_bottom - 440.0).max(160.0)),
                    ELEVATED,
                    8.0,
                ));
                self.control(
                    ops,
                    70000 + *index as u64,
                    label,
                    format!("select:{index}"),
                    rect(cx + 8.0, 442.0, cw - 16.0, 36.0),
                    false,
                    *index == self.selected,
                    AccessibilityRole::ListItem,
                );
                let catalog = matches!(self.tab, 1 | 2);
                if catalog {
                    self.control(
                        ops,
                        75000 + *index as u64,
                        "Install / update",
                        format!("install:{index}"),
                        rect(cx + 8.0, 488.0, cw - 16.0, 34.0),
                        busy || self.running() || !self.enabled,
                        false,
                        AccessibilityRole::Button,
                    );
                } else if let Some(row) = self.installed.get(*index) {
                    let enabled = row.state.enabled;
                    let caps = row
                        .package
                        .manifest
                        .capabilities
                        .iter()
                        .map(|cap| cap.name())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    ops.push(text(cx + 12.0, 530.0, caps, 12.0, MUTED));
                    self.control(
                        ops,
                        75000 + *index as u64,
                        if enabled {
                            "Enabled · Disable"
                        } else {
                            "Review permissions"
                        },
                        format!("permission:{index}"),
                        rect(cx + 8.0, 488.0, cw - 16.0, 34.0),
                        busy || !self.enabled,
                        false,
                        AccessibilityRole::Button,
                    );
                }
            }
            if rows.is_empty() {
                ops.push(text(
                    x,
                    444.0,
                    if matches!(self.tab, 1 | 2) {
                        "Open a signed offline catalog to view available packages."
                    } else {
                        "No extensions in this view."
                    },
                    15.0,
                    MUTED,
                ));
            }
            if !matches!(self.tab, 1 | 2) && self.installed.get(self.selected).is_some() {
                let command = self.installed[self.selected]
                    .package
                    .manifest
                    .commands
                    .get(self.command_selection)
                    .cloned()
                    .unwrap_or_default();
                ops.push(text(
                    x,
                    body_bottom - 72.0,
                    format!("Command: {command}"),
                    13.0,
                    TEXT,
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
                        rect(
                            x + i as f32 * (w / 5.0),
                            body_bottom - 44.0,
                            w / 5.0 - 6.0,
                            34.0,
                        ),
                        busy || !self.enabled
                            || (*action == "extensions.approve"
                                && self.permission_review != Some(self.selected)),
                        false,
                        AccessibilityRole::Button,
                    );
                }
            }
            self.control(
                ops,
                60016,
                "Previous extension",
                "previous",
                rect(x, body_bottom, 160.0, 32.0),
                rows.len() < 2,
                false,
                AccessibilityRole::Button,
            );
            self.control(
                ops,
                60017,
                "Next extension",
                "next",
                rect(x + 170.0, body_bottom, 150.0, 32.0),
                rows.len() < 2,
                false,
                AccessibilityRole::Button,
            );
        }
        if let Some(message) = &self.message {
            ops.push(text(x, height - 85.0, message, 13.0, TEXT));
        }
        ops.push(text(
            x,
            height - 55.0,
            "Disabling extensions stops the host. Removing the runtime frees disk space.",
            13.0,
            MUTED,
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
                "results_prev" => {
                    self.extensions.ui.result_page =
                        self.extensions.ui.result_page.saturating_sub(4)
                }
                "results_next" => {
                    self.extensions.ui.result_page =
                        self.extensions.ui.result_page.saturating_add(4)
                }
                "results_copy" => {
                    if let Some(platform) = &self.platform
                        && let Err(error) =
                            platform.set_clipboard_text(&self.extensions.panel_output)
                    {
                        self.extensions.message = Some(error.to_string());
                    }
                }
                "args_prev" => {
                    self.extensions.ui.field_page = self.extensions.ui.field_page.saturating_sub(4)
                }
                "args_next" => {
                    self.extensions.ui.field_page = (self.extensions.ui.field_page + 4).min(64)
                }
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
                    if let Some(index) = hit
                        .and_then(|id| id.checked_sub(FIELD))
                        .filter(|index| *index < 65)
                        && let Some(renderer) = &self.renderer
                    {
                        let _ = self.extensions.ui.fields[index as usize].click(
                            renderer,
                            self.pointer,
                            self.modifiers.shift_key(),
                        );
                    }
                } else if let Some(id) = self
                    .extensions
                    .ui
                    .pressed
                    .take()
                    .filter(|id| Some(*id) == hit)
                {
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
                            self.extensions.ui.focus = controls[(old as isize
                                + if shift { -1 } else { 1 })
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
                                                && platform
                                                    .set_clipboard_text(field.selected())
                                                    .is_ok()
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
            _ => return self.extensions.bounds.contains(self.pointer),
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
                value: Some(self.panel_output.clone()),
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
pub(super) fn accessibility_test_cases() -> Vec<(&'static str, Vec<AccessibilityNode>, Option<u64>)>
{
    use bareline_extensions_protocol::{
        Catalog, CatalogPolicy, OfflinePackageSource, PackageRequest, VerifiedPackageSource,
    };
    use bareline_renderer::RenderBackend;
    let capture = |name, runtime: &mut ExtensionsRuntime| {
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        backend.resize(1000, 800, 1.0).unwrap();
        let mut ops = vec![];
        runtime.draw_manager(&mut backend, 1000.0, 800.0, &mut ops);
        backend.render(&ops).unwrap();
        (
            name,
            runtime.accessibility_nodes(),
            runtime.accessibility_focus(),
        )
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
    cases.push(capture(
        "extensions-installed-package-runtime-absent",
        &mut runtime,
    ));
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
    runtime
        .installed
        .pop()
        .unwrap()
        .package
        .remove_cached()
        .unwrap();
    std::fs::remove_dir(root).unwrap();
    cases
}
