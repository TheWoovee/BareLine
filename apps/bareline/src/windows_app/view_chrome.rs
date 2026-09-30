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

/// Per-window view state that is not a saved preference.
#[derive(Default)]
pub(super) struct ViewChromeRuntime {
    always_on_top: bool,
    /// Plain drags and Shift+arrow keys select rectangles, as Alt does.
    pub(super) column_mode: bool,
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
            Ok(()) if toggle_checked(&self.settings.effective(), key) == was => {
                Some(format!("The workspace settings set {key}; change it there."))
            }
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
}
