// SPDX-License-Identifier: MPL-2.0
use super::*;
use bareline_commands::{CommandId, CommandRegistry, CommandSpec, KeyBinding, KeyChord};
use bareline_renderer::{DrawOp, LayoutError, Rect};
use bareline_ui::{rect, text, text_field::TextField};

#[derive(Default)]
pub(super) struct ShortcutsRuntime {
    pub open: bool,
    pub query: TextField,
    pub binding: TextField,
    pub binding_focus: bool,
    pub selected: usize,
    pub rows: Vec<CommandId>,
    pub bounds: Rect,
    pub status: String,
    pending_open: Option<std::sync::mpsc::Receiver<Result<PathBuf, String>>>,
}
pub(super) fn register(registry: &mut CommandRegistry) {
    for (id, title) in [
        ("settings.shortcuts", "Shortcut Mapper"),
        ("settings.keymap_import", "Import Keymap"),
        ("settings.keymap_export", "Export Keymap"),
        ("settings.keymap_open", "Open Keymap File"),
    ] {
        registry
            .register(CommandSpec {
                id: CommandId(id),
                title,
                category: "Settings",
                shortcut: "",
                action: Action::Contributed(CommandId(id)),
            })
            .expect("unique shortcut command");
    }
    for (id, title) in [
        ("settings.shortcut_apply", "Apply Shortcut"),
        ("settings.shortcut_close", "Close Shortcut Mapper"),
    ] {
        registry
            .register(CommandSpec {
                id: CommandId(id),
                title,
                category: "Settings",
                shortcut: "",
                action: Action::Contributed(CommandId(id)),
            })
            .expect("unique mapper command");
        registry
            .set_presentation(
                CommandId(id),
                bareline_commands::CommandPresentation {
                    internal: true,
                    ..Default::default()
                },
            )
            .expect("registered mapper command");
    }
}
/// Capitalize a single lowercase token, e.g. "added" -> "Added".
fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
/// A raw dotted token like `diff.added` reads as an internal ID in the list.
/// Turn it into words, dropping a leading namespace segment; leave real titles
/// (anything already containing a space, or without a dot) untouched.
fn prettify_title(title: &str, id: &str) -> String {
    if title.contains(' ') || !title.contains('.') {
        return title.to_string();
    }
    let mut parts: Vec<&str> = title.split('.').filter(|part| !part.is_empty()).collect();
    if parts.len() > 1
        && matches!(
            parts[0],
            "diff" | "compare" | "editor" | "view" | "app" | "file" | "search"
        )
    {
        parts.remove(0);
    }
    let label = parts.iter().map(|part| title_case(part)).collect::<Vec<_>>().join(" ");
    if label.is_empty() {
        title_case(id.rsplit('.').next().unwrap_or(id))
    } else {
        label
    }
}
/// Top-level menu a command belongs to, from its presentation path or category.
fn menu_of(registry: &CommandRegistry, id: CommandId) -> String {
    let category = registry
        .entries()
        .find(|entry| entry.id == id)
        .map_or("", |entry| entry.category);
    registry
        .presentation(id)
        .map(|presentation| presentation.menu_path.as_str())
        .filter(|path| !path.is_empty())
        .and_then(|path| path.split(['>', '›']).next())
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map_or_else(|| category.to_string(), str::to_string)
}
/// User-facing row label: the command's menu category and a readable title,
/// never a raw command ID.
fn display_label(registry: &CommandRegistry, id: CommandId) -> String {
    let spec = registry.entries().find(|entry| entry.id == id);
    let category = spec.map_or("", |entry| entry.category);
    let pretty = prettify_title(spec.map_or(id.0, |entry| entry.title), id.0);
    if category.is_empty() {
        pretty
    } else {
        format!("{category}: {pretty}")
    }
}
impl ShortcutsRuntime {
    /// A non-empty binding that fails to parse (unknown key or too many chords)
    /// yields an explanatory message; a valid or empty binding yields `None`.
    fn binding_error(&self) -> Option<String> {
        let value = self.binding.value();
        if value.trim().is_empty() {
            return None;
        }
        let chords: Vec<&str> = value.split_whitespace().collect();
        if chords.len() > 4 {
            return Some("Use one to four chords".into());
        }
        chords.iter().find_map(|chord| KeyChord::parse(chord).err())
    }
    fn refresh(&mut self, registry: &CommandRegistry) {
        let query = self.query.value().to_lowercase();
        let mut rows: Vec<CommandId> = registry
            .entries()
            .filter(|e| !registry.presentation(e.id).is_some_and(|p| p.internal))
            .filter(|e| {
                query.is_empty()
                    || display_label(registry, e.id).to_lowercase().contains(&query)
                    || menu_of(registry, e.id).to_lowercase().contains(&query)
                    || e.id.0.to_lowercase().contains(&query)
            })
            .map(|e| e.id)
            .collect();
        // Group by menu, then by readable label within each menu.
        rows.sort_by(|a, b| {
            menu_of(registry, *a)
                .cmp(&menu_of(registry, *b))
                .then_with(|| display_label(registry, *a).cmp(&display_label(registry, *b)))
        });
        self.rows = rows;
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }
    fn select(&mut self, keymap: &bareline_commands::Keymap) {
        self.binding.select_all();
        let label = self
            .rows
            .get(self.selected)
            .map_or(String::new(), |id| keymap.shortcut_label(*id));
        self.binding.insert(&label);
    }
    pub(super) fn accessibility_nodes(
        &self,
        registry: &CommandRegistry,
    ) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        if !self.open {
            return Vec::new();
        }
        let b = self.bounds;
        let y = b.y + b.height - 114.0;
        let node = |id, role, name: String, value, b: Rect, selected| AccessibilityNode {
            id,
            parent: 1,
            role,
            name,
            value,
            bounds: [b.x as f64, b.y as f64, b.width as f64, b.height as f64],
            disabled: false,
            selected,
            expanded: None,
            focusable: true,
            invokable: role != AccessibilityRole::TextField,
        };
        let mut nodes = vec![
            node(
                19000,
                AccessibilityRole::TextField,
                "Find command".into(),
                Some(self.query.value().into()),
                rect(b.x + 20.0, b.y + 50.0, b.width - 40.0, 32.0),
                false,
            ),
            node(
                19001,
                AccessibilityRole::TextField,
                "Shortcut sequence".into(),
                Some(self.binding.value().into()),
                rect(b.x + 20.0, y, b.width - 210.0, 32.0),
                false,
            ),
            node(
                19002,
                AccessibilityRole::Button,
                "Apply shortcut".into(),
                None,
                rect(b.x + b.width - 178.0, y, 74.0, 32.0),
                false,
            ),
            node(
                19003,
                AccessibilityRole::Button,
                "Close shortcut mapper".into(),
                None,
                rect(b.x + b.width - 94.0, y, 74.0, 32.0),
                false,
            ),
        ];
        let count = ((b.height - 220.0) / 28.0).max(1.0) as usize;
        let start = self.selected.saturating_sub(count - 1);
        for (index, id) in self.rows.iter().enumerate().skip(start).take(count) {
            nodes.push(node(
                25000 + index as u64,
                AccessibilityRole::ListItem,
                display_label(registry, *id),
                None,
                rect(
                    b.x + 16.0,
                    b.y + 92.0 + (index - start) as f32 * 28.0,
                    b.width - 32.0,
                    27.0,
                ),
                index == self.selected,
            ));
        }
        nodes
    }
    #[allow(clippy::too_many_arguments)] // Explicit borrowed render/context inputs avoid retaining stale command state.
    pub fn draw(
        &mut self,
        renderer: &mut impl bareline_renderer::TextBackend,
        width: f32,
        height: f32,
        theme: bareline_ui::theme::UiTheme,
        registry: &CommandRegistry,
        keymap: &bareline_commands::Keymap,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        if !self.open {
            self.query.release(renderer);
            self.binding.release(renderer);
            return Ok(None);
        }
        self.refresh(registry);
        let w = width.clamp(280.0, 760.0);
        let h = (height - 70.0).clamp(280.0, 560.0);
        self.bounds = rect((width - w) / 2.0, 40.0, w, h);
        let b = self.bounds;
        ops.push(DrawOp::Fill(b, theme.elevated));
        ops.push(DrawOp::Stroke(b, theme.border, 1.0));
        text(ops, b.x + 20.0, b.y + 16.0, "Shortcut Mapper", 20.0, theme.text);
        let query = rect(b.x + 20.0, b.y + 50.0, b.width - 40.0, 32.0);
        let mut caret = self
            .query
            .draw_with_theme(renderer, query, !self.binding_focus, theme, ops)?;
        let count = ((h - 220.0) / 28.0).max(1.0) as usize;
        let start = self.selected.saturating_sub(count - 1);
        for (row, id) in self.rows.iter().enumerate().skip(start).take(count) {
            let y = b.y + 92.0 + (row - start) as f32 * 28.0;
            if row == self.selected {
                ops.push(DrawOp::Fill(rect(b.x + 16.0, y, b.width - 32.0, 27.0), theme.selection));
            }
            // A divider at each menu boundary groups the list without stealing a row.
            if row > 0 && menu_of(registry, self.rows[row - 1]) != menu_of(registry, *id) {
                ops.push(DrawOp::Fill(rect(b.x + 16.0, y, b.width - 32.0, 1.0), theme.border));
            }
            text(
                ops,
                b.x + 24.0,
                y + 4.0,
                &display_label(registry, *id),
                13.0,
                theme.text,
            );
            text(
                ops,
                b.x + b.width * 0.62,
                y + 4.0,
                keymap.shortcut_label(*id),
                13.0,
                theme.muted,
            );
        }
        // Scrollbar: track plus a thumb sized and placed by the scroll offset.
        let total = self.rows.len();
        if total > count {
            let track_x = b.x + b.width - 10.0;
            let track_top = b.y + 92.0;
            let track_h = count as f32 * 28.0;
            ops.push(DrawOp::Fill(rect(track_x, track_top, 4.0, track_h), theme.border));
            let thumb_h = (track_h * count as f32 / total as f32).clamp(16.0, track_h);
            let max_start = (total - count) as f32;
            let frac = if max_start == 0.0 {
                0.0
            } else {
                start as f32 / max_start
            };
            ops.push(DrawOp::Fill(
                rect(track_x, track_top + (track_h - thumb_h) * frac, 4.0, thumb_h),
                theme.muted,
            ));
        }
        let y = b.y + b.height - 114.0;
        text(
            ops,
            b.x + 20.0,
            y - 21.0,
            "One to four chords, separated by spaces. Empty removes binding.",
            12.0,
            theme.muted,
        );
        let field = rect(b.x + 20.0, y, b.width - 210.0, 32.0);
        let c = self
            .binding
            .draw_with_theme(renderer, field, self.binding_focus, theme, ops)?;
        if self.binding_focus {
            caret = c;
        }
        for (x, label) in [(b.x + b.width - 178.0, "Apply"), (b.x + b.width - 94.0, "Done")] {
            let r = rect(x, y, 74.0, 32.0);
            ops.push(DrawOp::Fill(r, theme.elevated));
            ops.push(DrawOp::Stroke(r, theme.border, 1.0));
            text(ops, x + 12.0, y + 8.0, label, 13.0, theme.text);
        }
        text(ops, b.x + 20.0, y + 43.0, &self.status, 12.0, theme.muted);
        text(
            ops,
            b.x + 20.0,
            y + 65.0,
            "Tab: edit binding | Enter: apply | Escape: close",
            12.0,
            theme.muted,
        );
        Ok(Some(caret))
    }
}
fn changed_map(
    current: &bareline_settings::KeymapDocument,
    id: CommandId,
    value: &str,
    registry: &CommandRegistry,
) -> Result<bareline_settings::KeymapDocument, String> {
    let mut next = current.clone();
    if value.trim().is_empty() {
        let bindings = current
            .keymap
            .bindings()
            .iter()
            .filter(|b| b.command != id)
            .cloned()
            .collect();
        let mut map = current.keymap.clone();
        map.replace(bindings, registry)?;
        return bareline_settings::KeymapDocument::parse(&map.export_toml(), registry);
    }
    let sequence = value
        .split_whitespace()
        .map(KeyChord::parse)
        .collect::<Result<Vec<_>, _>>()?;
    next.set_binding(KeyBinding { command: id, sequence }, registry)?;
    Ok(next)
}
impl Shell {
    pub(super) fn shortcuts_action(&mut self, action: Action) -> bool {
        if !self.shortcuts.open
            || self.palette.open
            || !matches!(
                action,
                Action::Copy | Action::Cut | Action::Paste | Action::SelectAll | Action::Undo | Action::Redo
            )
        {
            return false;
        }
        let field = if self.shortcuts.binding_focus {
            &mut self.shortcuts.binding
        } else {
            &mut self.shortcuts.query
        };
        match action {
            Action::SelectAll => field.select_all(),
            Action::Undo => field.undo(false),
            Action::Redo => field.undo(true),
            Action::Paste => {
                if let Some(platform) = &self.platform
                    && let Ok(value) = platform.clipboard_text()
                {
                    field.commit(&value);
                }
            }
            Action::Copy | Action::Cut => {
                if let Some(platform) = &self.platform
                    && platform.set_clipboard_text(field.selected()).is_ok()
                    && action == Action::Cut
                {
                    field.insert("");
                }
            }
            _ => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn shortcuts_accessibility(
        &mut self,
        action: &bareline_platform::accessibility::AccessibilityAction,
    ) -> bool {
        use bareline_platform::accessibility::AccessibilityAction;
        if !self.shortcuts.open || self.palette.open {
            return false;
        }
        match action {
            AccessibilityAction::Focus(id) => {
                if *id == 19000 || *id == 19001 {
                    self.shortcuts.binding_focus = *id == 19001;
                } else if *id >= 25000 && (*id - 25000) < self.shortcuts.rows.len() as u64 {
                    self.shortcuts.selected = (*id - 25000) as usize;
                    self.shortcuts.select(&self.settings.keymap.keymap);
                }
            }
            AccessibilityAction::SetValue { id, value } if *id == 19000 || *id == 19001 => {
                self.shortcuts.binding_focus = *id == 19001;
                let field = if *id == 19000 {
                    &mut self.shortcuts.query
                } else {
                    &mut self.shortcuts.binding
                };
                field.select_all();
                field.insert(value);
                if *id == 19000 {
                    self.shortcuts.refresh(&self.app.commands);
                    self.shortcuts.select(&self.settings.keymap.keymap);
                }
            }
            AccessibilityAction::Invoke(19002) => self.shortcut_apply(),
            AccessibilityAction::Invoke(19003) => self.shortcuts.open = false,
            AccessibilityAction::Invoke(id) if *id >= 25000 && (*id - 25000) < self.shortcuts.rows.len() as u64 => {
                self.shortcuts.selected = (*id - 25000) as usize;
                self.shortcuts.select(&self.settings.keymap.keymap);
                self.shortcuts.binding_focus = true;
            }
            _ => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn shortcuts_pump(&mut self, el: &ActiveEventLoop) {
        let result = self.shortcuts.pending_open.as_ref().and_then(|r| match r.try_recv() {
            Ok(v) => Some(v),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("Keymap worker stopped".into())),
            Err(_) => None,
        });
        if let Some(result) = result {
            self.shortcuts.pending_open = None;
            match result {
                Ok(path) => {
                    if self.ensure_workspace(el) {
                        self.workspace.as_mut().unwrap().open(path);
                        self.shortcuts.open = false;
                        self.settings.controller.dismiss();
                    }
                }
                Err(error) => {
                    self.shortcuts.status = error.clone();
                    self.settings.controller.error = Some(error);
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
    pub(super) fn shortcuts_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        match id {
            "settings.shortcut_apply" => self.shortcut_apply(),
            "settings.shortcut_close" => self.shortcuts.open = false,
            "settings.shortcuts" => {
                self.shortcuts.open = true;
                self.shortcuts.binding_focus = false;
                self.shortcuts.refresh(&self.app.commands);
                self.shortcuts.select(&self.settings.keymap.keymap);
                self.palette.dismiss();
                self.app.palette = false;
            }
            "settings.keymap_export" => {
                if let Some(platform) = &self.platform {
                    match platform.save_file() {
                        Ok(Some(path)) => self.settings.save_keymap(self.settings.keymap.clone(), Some(path)),
                        Ok(None) => {}
                        Err(error) => self.shortcuts.status = error,
                    }
                }
            }
            "settings.keymap_open" => {
                if self.shortcuts.pending_open.is_none() {
                    if let Some(path) = self.settings.keymap_path() {
                        let document = self.settings.keymap.clone();
                        let notify = self.notify.clone();
                        let (tx, rx) = std::sync::mpsc::sync_channel(1);
                        self.shortcuts.pending_open = Some(rx);
                        if let Err(error) =
                            std::thread::Builder::new()
                                .name("bareline-keymap-open".into())
                                .spawn(move || {
                                    let result = (|| {
                                        if let Some(parent) = path.parent() {
                                            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                                        }
                                        bareline_settings::atomic_create_config(
                                            &path,
                                            document.to_toml().as_bytes(),
                                            &bareline_platform_windows::WindowsFileSystem,
                                        )
                                        .map_err(|e| e.to_string())?;
                                        Ok(path)
                                    })();
                                    let _ = tx.send(result);
                                    notify();
                                })
                        {
                            self.shortcuts.pending_open = None;
                            self.shortcuts.status = error.to_string();
                        }
                    } else {
                        self.shortcuts.status = "Keymap storage is unavailable".into();
                    }
                }
            }
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn shortcut_apply(&mut self) {
        let Some(id) = self.shortcuts.rows.get(self.shortcuts.selected).copied() else {
            return;
        };
        match changed_map(
            &self.settings.keymap,
            id,
            self.shortcuts.binding.value(),
            &self.app.commands,
        ) {
            Ok(next) => {
                self.settings.save_keymap(next, None);
                self.shortcuts.status = if self.settings.keymap_busy() {
                    "Saving shortcut changes...".into()
                } else {
                    self.settings.controller.error.clone().unwrap_or_default()
                };
            }
            Err(error) => self.shortcuts.status = error,
        }
    }
    pub(super) fn shortcuts_event(&mut self, _el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if !self.shortcuts.open || self.palette.open {
            return false;
        }
        match event {
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if self.modifiers.control_key()
                    && self.modifiers.shift_key()
                    && matches!(&event.logical_key,Key::Character(v) if v.eq_ignore_ascii_case("p"))
                {
                    return false;
                }
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        let composing = self.shortcuts.query.composing() || self.shortcuts.binding.composing();
                        self.shortcuts.query.cancel();
                        self.shortcuts.binding.cancel();
                        if !composing {
                            self.shortcuts.open = false;
                        }
                    }
                    Key::Named(NamedKey::Tab) => {
                        self.shortcuts.binding_focus = !self.shortcuts.binding_focus;
                    }
                    Key::Named(NamedKey::Enter) => {
                        if self.shortcuts.binding_focus {
                            self.shortcut_apply();
                        } else {
                            self.shortcuts.binding_focus = true;
                        }
                    }
                    Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) if !self.shortcuts.binding_focus => {
                        if event.logical_key == Key::Named(NamedKey::ArrowUp) {
                            self.shortcuts.selected = self.shortcuts.selected.saturating_sub(1);
                        } else {
                            self.shortcuts.selected =
                                (self.shortcuts.selected + 1).min(self.shortcuts.rows.len().saturating_sub(1));
                        }
                        self.shortcuts.select(&self.settings.keymap.keymap);
                    }
                    key => {
                        let field = if self.shortcuts.binding_focus {
                            &mut self.shortcuts.binding
                        } else {
                            &mut self.shortcuts.query
                        };
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
                                    "z" => field.undo(self.modifiers.shift_key()),
                                    "y" => field.undo(true),
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
                        if !self.shortcuts.binding_focus {
                            self.shortcuts.refresh(&self.app.commands);
                            self.shortcuts.select(&self.settings.keymap.keymap);
                        } else {
                            // Validate the chord as it is typed so an unknown key
                            // is rejected on entry rather than silently accepted.
                            self.shortcuts.status = self.shortcuts.binding_error().unwrap_or_default();
                        }
                    }
                }
            }
            WindowEvent::Ime(ime) => {
                let field = if self.shortcuts.binding_focus {
                    &mut self.shortcuts.binding
                } else {
                    &mut self.shortcuts.query
                };
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
                let b = self.shortcuts.bounds;
                let p = self.pointer;
                let y = b.y + b.height - 114.0;
                if p.y >= y && p.y < y + 32.0 && p.x >= b.x + b.width - 178.0 {
                    if p.x < b.x + b.width - 100.0 {
                        self.shortcut_apply();
                    } else {
                        self.shortcuts.open = false;
                    }
                } else if p.y >= y && p.y < y + 32.0 {
                    self.shortcuts.binding_focus = true;
                    if let Some(renderer) = &self.renderer {
                        let _ = self.shortcuts.binding.click(renderer, p, self.modifiers.shift_key());
                    }
                } else if p.y >= b.y + 50.0 && p.y < b.y + 82.0 {
                    self.shortcuts.binding_focus = false;
                    if let Some(renderer) = &self.renderer {
                        let _ = self.shortcuts.query.click(renderer, p, self.modifiers.shift_key());
                    }
                } else if p.y >= b.y + 92.0 && p.y < y - 25.0 {
                    let count = ((b.height - 220.0) / 28.0).max(1.0) as usize;
                    let start = self.shortcuts.selected.saturating_sub(count - 1);
                    self.shortcuts.selected =
                        (start + ((p.y - b.y - 92.0) / 28.0) as usize).min(self.shortcuts.rows.len().saturating_sub(1));
                    self.shortcuts.select(&self.settings.keymap.keymap);
                }
            }
            WindowEvent::Focused(false) => {
                self.shortcuts.query.cancel();
                self.shortcuts.binding.cancel();
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
    fn mapper_preserves_map_on_conflict_and_supports_remove_and_multichord() {
        let registry = bareline_commands::shell_commands();
        let original = bareline_settings::KeymapDocument::defaults(&registry);
        let id = CommandId("file.save");
        assert!(changed_map(&original, id, "Ctrl+N", &registry).is_err());
        assert_eq!(original.keymap.shortcut_label(id), "Ctrl+S");
        let next = changed_map(&original, id, "Ctrl+K Ctrl+S", &registry).unwrap();
        assert_eq!(next.keymap.shortcut_label(id), "Ctrl+K Ctrl+S");
        let removed = changed_map(&next, id, "", &registry).unwrap();
        assert!(removed.keymap.shortcut_label(id).is_empty());
    }
    #[test]
    fn mapper_semantics_have_unique_ids_and_live_edit_values() {
        let mut registry = bareline_commands::shell_commands();
        register(&mut registry);
        let mut mapper = ShortcutsRuntime {
            open: true,
            bounds: rect(20.0, 40.0, 760.0, 560.0),
            ..Default::default()
        };
        mapper.query.insert("save");
        mapper.refresh(&registry);
        mapper.binding.insert("Ctrl+K Ctrl+S");
        let nodes = mapper.accessibility_nodes(&registry);
        let mut snapshot = bareline_app::accessibility::snapshot("Bareline", 900.0, 700.0, None, nodes, 19001);
        assert!(snapshot.validate().is_ok());
        assert_eq!(
            snapshot.nodes.iter().find(|n| n.id == 19001).unwrap().value.as_deref(),
            Some("Ctrl+K Ctrl+S")
        );
        snapshot.focus = 19000;
        assert!(snapshot.validate().is_ok());
    }
    #[test]
    fn invalid_chord_is_rejected_on_entry_and_on_apply() {
        let registry = bareline_commands::shell_commands();
        let original = bareline_settings::KeymapDocument::defaults(&registry);
        // Apply path: an unknown key name never reaches the keymap.
        assert!(changed_map(&original, CommandId("file.save"), "Ctrl+DefinitelyNotAKey", &registry).is_err());
        // Entry path: live validation flags the same input and clears for a valid chord.
        let mut mapper = ShortcutsRuntime::default();
        mapper.binding.insert("Ctrl+DefinitelyNotAKey");
        assert!(mapper.binding_error().is_some());
        mapper.binding.select_all();
        mapper.binding.insert("Ctrl+K Ctrl+S");
        assert!(mapper.binding_error().is_none());
    }
    #[test]
    fn dotted_titles_render_as_readable_labels_never_raw_ids() {
        let mut registry = bareline_commands::shell_commands();
        // A command whose title is a raw theme token, like the compare colors.
        registry
            .register(CommandSpec {
                id: CommandId("compare.colorAdded"),
                title: "diff.added",
                category: "Compare",
                shortcut: "",
                action: Action::Contributed(CommandId("compare.colorAdded")),
            })
            .unwrap();
        let label = display_label(&registry, CommandId("compare.colorAdded"));
        assert_eq!(label, "Compare: Added");
        assert!(!label.contains("diff."));
    }
}

#[cfg(test)]
pub(super) fn accessibility_test_setup(shell: &mut Shell, scenario: &str) {
    shell.shortcuts = ShortcutsRuntime::default();
    shell.shortcuts.open = scenario != "closed";
    shell.shortcuts.binding_focus = scenario == "focus_binding";
    if scenario == "filtered" {
        shell.shortcuts.query.insert("Save");
    }
    if scenario == "error" {
        shell.shortcuts.status = "Shortcut conflicts with an existing binding".into();
    }
    shell.shortcuts.refresh(&shell.app.commands);
    shell.shortcuts.select(&shell.settings.keymap.keymap);
    let mut renderer = bareline_renderer_recording::RecordingBackend::default();
    let mut operations = Vec::new();
    shell
        .shortcuts
        .draw(
            &mut renderer,
            1000.0,
            800.0,
            shell.settings.ui_theme(),
            &shell.app.commands,
            &shell.settings.keymap.keymap,
            &mut operations,
        )
        .unwrap();
}
