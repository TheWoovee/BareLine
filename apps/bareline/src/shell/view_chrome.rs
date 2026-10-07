// SPDX-License-Identifier: MPL-2.0
//! Day-1 view chrome (BIZ-07): zoom commands, View toggles bound to the display
//! settings, brace matching, column selection mode, full screen and always on
//! top. The toggles write the user settings, so they persist like any other
//! preference; full screen and always on top are per-window session state.
use super::*;
use bareline_commands::{CommandId, CommandPresentation, CommandRegistry, CommandSpec, CommandState};

/// Registered view chrome commands: (id, title, category, default shortcut,
/// palette keywords). Ctrl+B follows Notepad++'s Go to Matching Brace; the
/// selecting variant takes Shift, as selection commands do elsewhere.
const COMMANDS: [(&str, &str, &str, &str, &[&str]); 14] = [
    ("view.zoom.in", "Zoom In", "View", "Ctrl+=", &["font size", "larger"]),
    ("view.zoom.out", "Zoom Out", "View", "Ctrl+-", &["font size", "smaller"]),
    (
        "view.zoom.reset",
        "Restore Default Zoom",
        "View",
        "Ctrl+0",
        &["font size", "zoom"],
    ),
    ("view.wordWrap", "Word Wrap", "View", "", &["wrap", "long lines"]),
    ("view.lineNumbers", "Line Numbers", "View", "", &["gutter", "margin"]),
    (
        "view.whitespace",
        "Show Whitespace",
        "View",
        "",
        &["spaces", "tabs", "symbols"],
    ),
    (
        "view.endOfLine",
        "Show End of Line",
        "View",
        "",
        &["eol", "crlf", "symbols"],
    ),
    (
        "view.indentGuides",
        "Show Indent Guides",
        "View",
        "",
        &["indentation", "symbols"],
    ),
    (
        "view.edgeLine",
        "Show Edge Line",
        "View",
        "",
        &["ruler", "column", "margin"],
    ),
    (
        "view.fullScreen",
        "Full Screen",
        "View",
        "F11",
        &["distraction", "maximize"],
    ),
    (
        "view.alwaysOnTop",
        "Always on Top",
        "View",
        "",
        &["pin window", "topmost"],
    ),
    (
        "editor.brace.goto",
        "Go to Matching Brace",
        "Search",
        "Ctrl+B",
        &["bracket", "parenthesis"],
    ),
    (
        "editor.brace.select",
        "Select to Matching Brace",
        "Search",
        "Ctrl+Shift+B",
        &["bracket", "parenthesis"],
    ),
    (
        "editor.column.selectMode",
        "Column Selection Mode",
        "Edit",
        "",
        &["rectangle", "block", "column mode"],
    ),
];
/// Keypad zoom keys. A command carries one default shortcut, so these are
/// internal aliases: they keep the menus showing one shortcut per row.
const KEYPAD_ALIASES: [(&str, &str, &str); 2] = [
    ("view.zoom.inKeypad", "Zoom In (Keypad)", "Ctrl+Physical:NumpadAdd"),
    (
        "view.zoom.outKeypad",
        "Zoom Out (Keypad)",
        "Ctrl+Physical:NumpadSubtract",
    ),
];
/// View toggles and the display setting each one flips.
const SETTING_TOGGLES: [(&str, &str); 6] = [
    ("view.wordWrap", "editor.wrap.mode"),
    ("view.lineNumbers", "editor.line_numbers"),
    ("view.whitespace", "editor.render.whitespace"),
    ("view.endOfLine", "editor.render.eol"),
    ("view.indentGuides", "editor.indent_guides"),
    ("view.edgeLine", "editor.edge.enabled"),
];
/// Font pixels one zoom command changes, the same as one Ctrl+wheel notch.
const ZOOM_STEP: f32 = 1.0;

/// Whether `id` is a view chrome command (see `command_route`).
pub(super) fn owns(id: &str) -> bool {
    COMMANDS.iter().any(|(owned, ..)| *owned == id) || KEYPAD_ALIASES.iter().any(|(owned, ..)| *owned == id)
}

pub(super) fn register(registry: &mut CommandRegistry) {
    for (id, title, category, shortcut, keywords) in COMMANDS {
        let id = CommandId(id);
        let registered = registry.register(CommandSpec {
            id,
            title,
            category,
            shortcut,
            action: Action::Contributed(id),
        });
        debug_assert!(registered.is_ok(), "duplicate command ID {id:?}");
        let _ = registry.set_presentation(
            id,
            CommandPresentation {
                menu_path: category.into(),
                keywords: keywords.iter().map(|keyword| (*keyword).into()).collect(),
                accessible_name: Some(title.into()),
                ..Default::default()
            },
        );
    }
    for (id, title, shortcut) in KEYPAD_ALIASES {
        let id = CommandId(id);
        let registered = registry.register(CommandSpec {
            id,
            title,
            category: "View",
            shortcut,
            action: Action::Contributed(id),
        });
        debug_assert!(registered.is_ok(), "duplicate command ID {id:?}");
        let _ = registry.set_presentation(
            id,
            CommandPresentation {
                internal: true,
                ..Default::default()
            },
        );
    }
}

/// The painted guides the display settings ask for.
pub(super) fn view_guides(settings: &bareline_settings::EffectiveSettings) -> bareline_editor_surface::ViewGuides {
    bareline_editor_surface::ViewGuides {
        end_of_line: settings.show_eol,
        indent_guides: settings.indent_guides,
        edge_column: settings.edge_enabled.then_some(settings.edge_column),
    }
}

/// Whether the display setting behind a View toggle is on. Show Whitespace is
/// checked only while every whitespace character is marked.
fn toggle_checked(settings: &bareline_settings::EffectiveSettings, key: &str) -> bool {
    match key {
        "editor.wrap.mode" => settings.word_wrap,
        "editor.line_numbers" => settings.line_numbers,
        "editor.render.whitespace" => settings.whitespace == "all",
        "editor.render.eol" => settings.show_eol,
        "editor.indent_guides" => settings.indent_guides,
        "editor.edge.enabled" => settings.edge_enabled,
        _ => false,
    }
}

/// The value a View toggle writes. Turning Show Whitespace off returns to the
/// default, which marks whitespace inside selections only.
fn toggled_value(settings: &bareline_settings::EffectiveSettings, key: &str) -> bareline_settings::SettingValue {
    let on = !toggle_checked(settings, key);
    match key {
        "editor.render.whitespace" => {
            bareline_settings::SettingValue::Text(if on { "all" } else { "selection" }.into())
        }
        _ => bareline_settings::SettingValue::Bool(on),
    }
}

/// Why a View toggle did not change: a workspace or session value outranks the
/// user setting it wrote. Names the setting by its Settings title, not its key.
fn overridden_message(key: &str) -> String {
    let title = bareline_settings::DEFINITIONS
        .iter()
        .find(|definition| definition.key == key)
        .map_or("This view setting", |definition| definition.title);
    format!("{title} is overridden by the workspace or session settings; change it there.")
}

/// Per-window view state that is not a saved preference.
#[derive(Default)]
pub(super) struct ViewChromeRuntime {
    always_on_top: bool,
    /// Plain drags and Shift+arrow keys select rectangles, as Alt does.
    pub(super) column_mode: bool,
    /// The pointer shape last set on the window, so moves that keep it make
    /// no call (LNX-UI-015).
    cursor: Option<winit::window::CursorIcon>,
}

impl Shell {
    /// The pointer's shape at window point `point`, for an editor area of
    /// `editor`: a resize arrow over the side panel's splitter, the bottom
    /// panel's sash and the split divider (and while one is dragged), an
    /// I-beam over a pane's text, and the arrow elsewhere, as in Notepad++
    /// (LNX-UI-015).
    pub(super) fn pointer_cursor_in(&self, point: Point, editor: bareline_renderer::Rect) -> winit::window::CursorIcon {
        use winit::window::CursorIcon;
        if self.modal.is_some()
            || self.palette.open
            || self.settings.controller.open
            || self.extensions.open
            || self.power.open
            || self.toasts.contains(point)
        {
            return CursorIcon::Default;
        }
        if self.panels.left_splitter_at(point) {
            return CursorIcon::ColResize;
        }
        let local = Point {
            x: point.x - editor.x,
            y: point.y - editor.y,
        };
        let dock = self.dock.current_layout();
        if self.dock.is_dragging() || dock.is_some_and(|layout| layout.splitter.contains(local)) {
            return CursorIcon::RowResize;
        }
        if let Some(icon) = self.views.splitter_cursor(local) {
            return icon;
        }
        if !editor.contains(point) || dock.is_some_and(|layout| layout.outer.contains(local)) {
            return CursorIcon::Default;
        }
        let size = (editor.width, editor.height);
        if self
            .workspace
            .as_ref()
            .is_some_and(|workspace| self.views.over_text(workspace, self.app.active, local, size))
        {
            CursorIcon::Text
        } else {
            CursorIcon::Default
        }
    }
    /// The pointer's share of every window event: its shape (LNX-UI-015) and
    /// the hovered tab's tooltip, which waits the Find bar's hover delay and
    /// hides on a press or when the pointer leaves (LNX-UI-014).
    pub(super) fn pointer_chrome_event(&mut self, event: &WindowEvent) {
        let mut redraw = false;
        if let WindowEvent::CursorMoved { position, .. } = event
            && let Some(window) = &self.window
        {
            let logical = position.to_logical::<f32>(window.scale_factor());
            let point = Point {
                x: logical.x,
                y: logical.y,
            };
            self.update_cursor(point);
            let editor = self.editor_bounds();
            let local = Point {
                x: point.x - editor.x,
                y: point.y - editor.y,
            };
            redraw |= self.views.hover_tabs(local, super::tooltip_clock_ms());
        }
        if matches!(
            event,
            WindowEvent::CursorLeft { .. } | WindowEvent::Focused(false) | WindowEvent::MouseInput { .. }
        ) {
            redraw |= self.views.dismiss_tab_tooltip();
        }
        if redraw && let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// Give the window the pointer shape for window point `point`; only a
    /// change reaches the window.
    pub(super) fn update_cursor(&mut self, point: Point) {
        let icon = self.pointer_cursor_in(point, self.editor_bounds());
        if self.view_chrome.cursor != Some(icon)
            && let Some(window) = &self.window
        {
            window.set_cursor(icon);
            self.view_chrome.cursor = Some(icon);
        }
    }
}

impl Shell {
    /// Whether a pointer or Shift+arrow gesture selects a rectangle: Alt, or
    /// column selection mode without Ctrl (Ctrl keeps adding carets).
    pub(super) fn rectangle_modifier(&self) -> bool {
        self.modifiers.alt_key() || (self.view_chrome.column_mode && !self.modifiers.control_key())
    }
    /// [`Self::rectangle_modifier`] for Shift+arrow keys: column selection mode
    /// claims them only while the editor, not a field or panel, has the keyboard.
    pub(super) fn rectangle_keys(&self) -> bool {
        self.modifiers.alt_key()
            || (self.rectangle_modifier()
                && !self.context_field_active()
                && self.panels_accessibility_focus().is_none()
                && self.views_accessibility_focus().is_none())
    }
    /// The focus handoff of a plain editor click, for a press that column
    /// selection mode claims before the click handler runs: a pressed split
    /// pane becomes active, and Find, search and the dock give up the keyboard.
    pub(super) fn column_press_handoff(&mut self, pane: usize) {
        if let Some(workspace) = self.workspace.as_mut() {
            if self.views.secondary.is_some() {
                self.views.activate(workspace, &mut self.app, pane as u32);
            }
            workspace.search_focus = false;
            workspace.find.blur();
        }
        self.blur_dock_ownership();
    }
    pub(super) fn view_chrome_annotate(&self, context: &mut bareline_commands::CommandContext) {
        let settings = self.settings.effective();
        for (id, key) in SETTING_TOGGLES {
            context.states.entry(CommandId(id)).or_default().checked = toggle_checked(&settings, key);
        }
        context.states.entry(CommandId("view.fullScreen")).or_default().checked =
            self.window.as_ref().is_some_and(|window| window.fullscreen().is_some());
        context.states.entry(CommandId("view.alwaysOnTop")).or_default().checked = self.view_chrome.always_on_top;
        context
            .states
            .entry(CommandId("editor.column.selectMode"))
            .or_default()
            .checked = self.view_chrome.column_mode;
        let editor = self
            .workspace
            .as_ref()
            .and_then(|workspace| self.views.active_workspace_editor(workspace, self.app.active));
        if editor.is_none() {
            for id in [
                "view.zoom.in",
                "view.zoom.out",
                "view.zoom.reset",
                "view.zoom.inKeypad",
                "view.zoom.outKeypad",
                "editor.brace.goto",
                "editor.brace.select",
            ] {
                context
                    .states
                    .insert(CommandId(id), CommandState::disabled("Open a document first"));
            }
        }
    }
    pub(super) fn view_chrome_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if let Some((_, key)) = SETTING_TOGGLES.iter().find(|(toggle, _)| *toggle == id) {
            self.toggle_view_setting(key);
        } else {
            match id {
                "view.zoom.in" | "view.zoom.inKeypad" => self.zoom_active_view(Some(ZOOM_STEP)),
                "view.zoom.out" | "view.zoom.outKeypad" => self.zoom_active_view(Some(-ZOOM_STEP)),
                "view.zoom.reset" => self.zoom_active_view(None),
                "editor.brace.goto" | "editor.brace.select" => self.jump_to_matching_brace(id == "editor.brace.select"),
                "editor.column.selectMode" => {
                    self.view_chrome.column_mode = !self.view_chrome.column_mode;
                    if let Some(workspace) = &mut self.workspace {
                        workspace.message = Some(if self.view_chrome.column_mode {
                            "Column selection mode is on: drag or press Shift+arrow keys to select a rectangle.".into()
                        } else {
                            "Column selection mode is off.".into()
                        });
                    }
                }
                "view.fullScreen" => {
                    if let Some(window) = &self.window {
                        let full = window.fullscreen().is_some();
                        window.set_fullscreen((!full).then_some(winit::window::Fullscreen::Borderless(None)));
                    }
                }
                "view.alwaysOnTop" => {
                    if let Some(window) = &self.window {
                        self.view_chrome.always_on_top = !self.view_chrome.always_on_top;
                        window.set_window_level(if self.view_chrome.always_on_top {
                            winit::window::WindowLevel::AlwaysOnTop
                        } else {
                            winit::window::WindowLevel::Normal
                        });
                    }
                }
                _ => return false,
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    /// Flip a display setting in the user settings, which saves it and applies
    /// it to every editor on the next frame.
    fn toggle_view_setting(&mut self, key: &str) {
        let effective = self.settings.effective();
        let was = toggle_checked(&effective, key);
        let scope = self.settings.controller.scope;
        self.settings.controller.scope = bareline_settings::Scope::User;
        let result = self.settings.controller.edit(key, toggled_value(&effective, key));
        self.settings.controller.scope = scope;
        let message = match result {
            Err(error) => Some(format!("Could not change the view setting: {error}")),
            // A workspace or session value still wins over the user setting.
            Ok(()) if toggle_checked(&self.settings.effective(), key) == was => Some(overridden_message(key)),
            Ok(()) => None,
        };
        if let Some(message) = message
            && let Some(workspace) = &mut self.workspace
        {
            workspace.message = Some(message);
        }
    }
    /// Zoom the focused view by `step` font pixels, or back to the configured
    /// size, sharing the Ctrl+wheel zoom state.
    fn zoom_active_view(&mut self, step: Option<f32>) {
        let Some(workspace) = self.workspace.as_mut() else {
            return;
        };
        let Some(editor) = self.views.active_workspace_editor_mut(workspace, self.app.active) else {
            return;
        };
        match step {
            Some(step) => {
                editor.zoom_by(step);
            }
            None => {
                editor.reset_zoom();
            }
        }
    }
    fn jump_to_matching_brace(&mut self, select: bool) {
        let Some(workspace) = self.workspace.as_mut() else {
            return;
        };
        let Some(editor) = self.views.active_workspace_editor_mut(workspace, self.app.active) else {
            return;
        };
        if !editor.jump_to_matching_brace(select) {
            workspace.message = Some(format!(
                "No matching brace next to the caret within {} KiB.",
                bareline_editor_surface::view_guides::MAX_BRACE_DISTANCE / 1024
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> CommandRegistry {
        let mut registry = CommandRegistry::default();
        register(&mut registry);
        registry
    }

    /// Every view chrome command is registered with a human title, routes to
    /// this handler, and has a home in the curated menus unless it is a keypad
    /// alias (UI-04 follow-up).
    #[test]
    fn view_chrome_commands_register_route_and_have_menu_homes() {
        let registry = registry();
        let tree = bareline_app::menus::tree_command_ids();
        for (id, ..) in COMMANDS {
            let spec = registry
                .spec(CommandId(id))
                .unwrap_or_else(|| panic!("{id} registered"));
            assert!(!bareline_commands::title_looks_like_identifier(spec.title), "{id}");
            assert_eq!(
                super::super::command_route(id),
                Some(super::super::Route::ViewChrome),
                "{id}"
            );
            assert!(tree.contains(&id), "{id} has no place in the menu tree");
            assert!(
                !registry.presentation(CommandId(id)).is_some_and(|meta| meta.internal),
                "{id}"
            );
        }
        for (id, ..) in KEYPAD_ALIASES {
            assert_eq!(
                super::super::command_route(id),
                Some(super::super::Route::ViewChrome),
                "{id}"
            );
            assert!(
                registry.presentation(CommandId(id)).is_some_and(|meta| meta.internal),
                "{id}"
            );
            assert!(!tree.contains(&id), "{id} is an alias, not a menu row");
        }
        for (toggle, key) in SETTING_TOGGLES {
            assert!(owns(toggle), "{toggle}");
            assert!(
                bareline_settings::DEFINITIONS
                    .iter()
                    .any(|definition| definition.key == key),
                "{toggle} flips unknown setting {key}"
            );
        }
        assert!(!owns("view.tabs.closeAll"));
    }

    /// The defaults bind the zoom, brace and full-screen keys, including both
    /// keypad keys, to these commands.
    #[test]
    fn default_shortcuts_reach_view_chrome_commands() {
        use bareline_commands::{InputContext, KeyChord, KeyResolution, Keymap};
        let keymap = Keymap::defaults(&registry());
        for (chord, id) in [
            ("Ctrl+=", "view.zoom.in"),
            ("Ctrl+-", "view.zoom.out"),
            ("Ctrl+0", "view.zoom.reset"),
            ("Ctrl+Physical:NumpadAdd", "view.zoom.inKeypad"),
            ("Ctrl+Physical:NumpadSubtract", "view.zoom.outKeypad"),
            ("F11", "view.fullScreen"),
            ("Ctrl+B", "editor.brace.goto"),
            ("Ctrl+Shift+B", "editor.brace.select"),
        ] {
            assert_eq!(
                keymap.resolve(&[KeyChord::parse(chord).unwrap()], InputContext::default()),
                KeyResolution::Command(CommandId(id)),
                "{chord}"
            );
        }
        // The keypad aliases stay out of the Zoom rows' shortcut labels.
        assert_eq!(keymap.shortcut_label(CommandId("view.zoom.in")), "Ctrl+=");
    }

    /// Each toggle is checked from, and flips, its own setting; Show Whitespace
    /// alternates between marking everything and the selection-only default.
    #[test]
    fn toggles_read_and_flip_their_settings() {
        use bareline_settings::{EffectiveSettings, SettingValue};
        let mut settings = EffectiveSettings::default();
        for (_, key) in SETTING_TOGGLES {
            let before = toggle_checked(&settings, key);
            let value = toggled_value(&settings, key);
            let mut document = bareline_settings::SettingsDocument::empty(bareline_settings::Scope::User);
            document.set(key, value).unwrap();
            let resolved = bareline_settings::resolve(&document, None, false, None).values;
            assert_eq!(toggle_checked(&resolved, key), !before, "{key}");
        }
        assert!(!toggle_checked(&settings, "editor.render.whitespace"));
        assert_eq!(
            toggled_value(&settings, "editor.render.whitespace"),
            SettingValue::Text("all".into())
        );
        settings.whitespace = "all".into();
        assert!(toggle_checked(&settings, "editor.render.whitespace"));
        assert_eq!(
            toggled_value(&settings, "editor.render.whitespace"),
            SettingValue::Text("selection".into())
        );
        // Word wrap is stored as a mode; the toggle's boolean is accepted.
        assert_eq!(toggled_value(&settings, "editor.wrap.mode"), SettingValue::Bool(true));
    }

    /// An overridden toggle names its setting as Settings shows it, never by key.
    #[test]
    fn overridden_toggle_message_uses_the_setting_title() {
        for (_, key) in SETTING_TOGGLES {
            let message = overridden_message(key);
            assert!(!message.contains(key), "{message}");
            assert!(message.contains("workspace or session"), "{message}");
        }
        assert!(overridden_message("editor.render.eol").starts_with("Show line endings "));
    }

    #[test]
    fn view_guides_follow_the_display_settings() {
        let mut settings = bareline_settings::EffectiveSettings::default();
        assert_eq!(view_guides(&settings), bareline_editor_surface::ViewGuides::default());
        settings.show_eol = true;
        settings.indent_guides = true;
        settings.edge_column = 100;
        assert_eq!(
            view_guides(&settings).edge_column,
            None,
            "a disabled edge keeps its column"
        );
        settings.edge_enabled = true;
        assert_eq!(
            view_guides(&settings),
            bareline_editor_surface::ViewGuides {
                end_of_line: true,
                indent_guides: true,
                edge_column: Some(100),
            }
        );
    }

    /// LNX-UI-015: the pointer was the arrow everywhere.
    #[test]
    fn pointer_shapes_follow_text_dividers_and_chrome() {
        use winit::window::CursorIcon;
        let mut shell = super::super::accessibility::tests::headless_shell();
        let mut workspace = bareline_app::workspace::Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(crate::shell::native::FileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        shell.workspace = Some(workspace);
        let editor = bareline_ui::rect(0.0, 0.0, 1000.0, 700.0);
        let at = |shell: &Shell, x: f32, y: f32| shell.pointer_cursor_in(Point { x, y }, editor);
        assert_eq!(at(&shell, 300.0, 200.0), CursorIcon::Text, "text");
        assert_eq!(at(&shell, 10.0, 200.0), CursorIcon::Default, "line numbers");
        assert_eq!(at(&shell, 300.0, 10.0), CursorIcon::Default, "tab strip");
        assert_eq!(at(&shell, 995.0, 200.0), CursorIcon::Default, "scroll bar");
        assert_eq!(at(&shell, 300.0, 690.0), CursorIcon::Default, "status strip");
        // A notification over the text keeps the arrow.
        shell.toasts.enqueue(
            super::toast::Notification::new(
                "cursor",
                1,
                bareline_ui::theme::ToastLevel::Error,
                super::toast::NotificationKind::Outcome,
                "Over the text",
                None,
                None,
                super::toast::NotificationLifetime::Persistent,
            ),
            std::time::Instant::now(),
        );
        shell.toasts.draw(
            &mut bareline_renderer_recording::RecordingBackend::default(),
            1000.0,
            700.0,
            Default::default(),
            &mut Vec::new(),
        );
        let toast = shell.toasts.accessibility()[0].bounds;
        assert_eq!(
            at(&shell, toast.x + toast.width / 2.0, toast.y + toast.height / 2.0),
            CursorIcon::Default
        );
        // A split's divider resizes its panes.
        shell.views.test_set_splitter(bareline_ui::rect(497.0, 0.0, 6.0, 676.0));
        assert_eq!(at(&shell, 499.0, 300.0), CursorIcon::ColResize);
        shell
            .views
            .test_set_splitter(bareline_ui::rect(0.0, 330.0, 1000.0, 6.0));
        assert_eq!(at(&shell, 300.0, 332.0), CursorIcon::RowResize);
        // A modal surface owns the pointer.
        shell.activate_modal(super::modal::ModalSurface::Goto);
        assert_eq!(at(&shell, 300.0, 200.0), CursorIcon::Default);
    }
}
