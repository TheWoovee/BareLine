// SPDX-License-Identifier: MPL-2.0
//! Spelling commands and menus (BIZ-31). Checking runs on the worker in
//! `bareline_app::spelling`; this module offers the misspelling under the
//! pointer or caret in the context menu and in Edit > Spelling.
use super::*;
use bareline_app::spelling::{SUGGESTION_WAIT, Target};
use bareline_commands::{CommandContext, CommandId, CommandRegistry, CommandSpec, CommandState};

/// One command per offered suggestion, the most likely first.
pub(super) const SUGGESTION_COMMANDS: [&str; 5] = [
    "spelling.suggestion.1",
    "spelling.suggestion.2",
    "spelling.suggestion.3",
    "spelling.suggestion.4",
    "spelling.suggestion.5",
];
const NO_TARGET: &str = "Place the caret on a misspelled word";

impl Shell {
    /// A service this system lacks (spell checking on Linux and macOS) is not
    /// news at every launch, nor worth the message bar over the last lines of
    /// text: its notice shows once per profile, as a notification that
    /// retires itself (LNX-EDIT-011).
    pub(super) fn retire_known_gap_notices(&mut self) {
        self.settings.record_known_gaps();
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        let Some(&notice) = crate::shell::native::KNOWN_GAP_NOTICES
            .iter()
            .find(|notice| workspace.message.as_deref() == Some(**notice))
        else {
            return;
        };
        workspace.message = None;
        if self.settings.note_known_gap(notice) {
            self.toasts.push_typed(
                format!("known-gap:{notice}"),
                toast::next_revision(),
                bareline_ui::theme::ToastLevel::Info,
                toast::NotificationKind::Outcome,
                notice,
                None,
                None,
                toast::NotificationLifetime::Transient,
                Instant::now(),
            );
        }
    }
}

pub(super) fn register(registry: &mut CommandRegistry) {
    for (id, title) in [
        ("spelling.toggle", "Spell Check"),
        ("spelling.suggestion.1", "Use Spelling Suggestion 1"),
        ("spelling.suggestion.2", "Use Spelling Suggestion 2"),
        ("spelling.suggestion.3", "Use Spelling Suggestion 3"),
        ("spelling.suggestion.4", "Use Spelling Suggestion 4"),
        ("spelling.suggestion.5", "Use Spelling Suggestion 5"),
        ("spelling.add", "Add to Dictionary"),
        ("spelling.ignore_all", "Ignore All"),
    ] {
        registry
            .register(CommandSpec {
                id: CommandId(id),
                title,
                category: "Edit",
                shortcut: "",
                action: Action::Contributed(CommandId(id)),
            })
            .expect("unique spelling command");
        let _ = registry.update_presentation(CommandId(id), |meta| {
            meta.keywords = vec!["spelling".into(), "spell check".into()];
        });
    }
}

impl Shell {
    /// The misspelling spelling commands act on: the one a context menu was
    /// opened on, or else the one at the caret of the active document.
    fn spelling_target(&self) -> Option<Target> {
        let workspace = self.workspace.as_ref()?;
        if let Some(target) = &workspace.spelling.target {
            return Some(target.clone());
        }
        let editor = workspace.editors.get(self.app.active)?.resident()?;
        Target::at(editor, editor.selection.caret)
    }
    pub(super) fn spelling_annotate_context(&self, context: &mut CommandContext) {
        context.states.insert(
            CommandId("spelling.toggle"),
            CommandState {
                checked: self.settings.effective().spell_check,
                ..Default::default()
            },
        );
        let target = self.spelling_target();
        let suggestions = target
            .as_ref()
            .zip(self.workspace.as_ref())
            .and_then(|(target, workspace)| workspace.spelling.cached_suggestions(&target.word));
        for (index, id) in SUGGESTION_COMMANDS.into_iter().enumerate() {
            let state = match (suggestions.and_then(|list| list.get(index)), &target) {
                (Some(word), _) => CommandState {
                    label: Some(word.clone()),
                    ..Default::default()
                },
                (None, Some(_)) if index == 0 && suggestions.is_some() => CommandState {
                    label: Some("No Suggestions".into()),
                    ..CommandState::disabled("The dictionary has no suggestions for this word")
                },
                (None, Some(_)) if index == 0 => CommandState {
                    label: Some("Looking Up Suggestions…".into()),
                    ..CommandState::disabled("Suggestions are still loading")
                },
                (None, Some(_)) => CommandState::disabled("No more suggestions"),
                (None, None) => CommandState::disabled(NO_TARGET),
            };
            context.states.insert(CommandId(id), state);
        }
        for id in ["spelling.add", "spelling.ignore_all"] {
            if target.is_none() {
                context.states.insert(CommandId(id), CommandState::disabled(NO_TARGET));
            }
        }
    }
    /// Before a context menu opens: remember the misspelling at `offset` of the
    /// active document and return its menu rows (suggestions, Add to
    /// Dictionary, Ignore All and a separator), or nothing off a misspelling.
    fn spelling_menu_rows(&mut self, offset: Option<usize>) -> Vec<&'static str> {
        let active = self.app.active;
        let Some(workspace) = &mut self.workspace else {
            return Vec::new();
        };
        workspace.spelling.target = None;
        let Some(editor) = workspace.editors.get(active).and_then(|editor| editor.resident()) else {
            return Vec::new();
        };
        let Some(target) = Target::at(editor, offset.unwrap_or(editor.selection.caret)) else {
            return Vec::new();
        };
        // A short wait, off the UI thread's checking path, so the menu opens
        // with its suggestions; a late answer is kept for the next menu.
        let count = workspace.spelling.suggestions(&target.word, SUGGESTION_WAIT).len();
        workspace.spelling.target = Some(target);
        let mut rows = SUGGESTION_COMMANDS[..count.clamp(1, SUGGESTION_COMMANDS.len())].to_vec();
        rows.extend(["spelling.add", "spelling.ignore_all", "-"]);
        rows
    }
    /// Rows for the keyboard context menu: the misspelling at the caret.
    pub(super) fn spelling_caret_menu_rows(&mut self) -> Vec<&'static str> {
        self.spelling_menu_rows(None)
    }
    /// Rows for a right-click: the misspelling under the pointer, if the
    /// pointer is over the text of the single editor view.
    pub(super) fn spelling_pointer_menu_rows(&mut self) -> Vec<&'static str> {
        if self.views.open() || self.app.palette {
            return Vec::new();
        }
        let pointer = self.editor_pointer();
        let Some(bounds) = self.views.bounds[0].filter(|bounds| bounds.contains(pointer)) else {
            return Vec::new();
        };
        let local = Point {
            x: pointer.x - bounds.x,
            y: pointer.y - bounds.y,
        };
        let offset = self
            .workspace
            .as_ref()
            .zip(self.renderer.as_ref())
            .and_then(|(workspace, renderer)| {
                let editor = workspace.editors.get(self.app.active)?.resident()?;
                editor.offset_at(renderer, local).ok().flatten()
            });
        match offset {
            Some(offset) => self.spelling_menu_rows(Some(offset)),
            None => Vec::new(),
        }
    }
    /// After a context menu closes (and its command ran).
    pub(super) fn spelling_menu_closed(&mut self) {
        if let Some(workspace) = &mut self.workspace {
            workspace.spelling.target = None;
        }
    }
    pub(super) fn spelling_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if !id.starts_with("spelling.") {
            return false;
        }
        if id == "spelling.toggle" {
            let enabled = !self.settings.effective().spell_check;
            let scope = self.settings.controller.scope;
            self.settings.controller.scope = bareline_settings::Scope::User;
            if let Err(error) = self
                .settings
                .controller
                .edit("editor.spell_check", bareline_settings::SettingValue::Bool(enabled))
            {
                self.settings.controller.error = Some(error);
            }
            self.settings.controller.scope = scope;
        } else if let Some(target) = self.spelling_target()
            && let Some(workspace) = &mut self.workspace
        {
            match id {
                "spelling.add" => workspace
                    .spelling
                    .add_to_dictionary(&target.word, &mut workspace.editors),
                "spelling.ignore_all" => workspace.spelling.ignore_all(&target.word, &mut workspace.editors),
                _ => {
                    let replacement = SUGGESTION_COMMANDS
                        .iter()
                        .position(|command| *command == id)
                        .and_then(|index| workspace.spelling.cached_suggestions(&target.word)?.get(index).cloned());
                    if let Some(replacement) = replacement {
                        spelling_replace(workspace, self.app.active, &target, replacement);
                    }
                }
            }
            workspace.spelling.target = None;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

/// Replace the misspelled word with `replacement` as one undoable edit, unless
/// the text changed since the word was found.
fn spelling_replace(workspace: &mut Workspace, active: usize, target: &Target, replacement: String) {
    let Some(editor) = workspace
        .editors
        .get_mut(active)
        .and_then(|editor| editor.resident_mut())
    else {
        return;
    };
    if editor.snapshot().identity_token() != target.identity || editor.busy() {
        workspace.message = Some("The text changed before the correction. Try the suggestion again.".into());
        return;
    }
    let selection = bareline_editor_surface::Selection {
        anchor: target.range.start,
        caret: target.range.end,
    };
    match editor.set_selections(selection.into()) {
        Ok(()) => editor.enqueue(Input::Insert(replacement)),
        Err(error) => workspace.message = Some(error),
    }
}

#[cfg(test)]
mod tests {
    /// LNX-EDIT-011: a missing spelling service was announced in the message
    /// bar over the text at every launch.
    #[test]
    fn known_gap_notices_show_once_per_profile() {
        let root = std::env::temp_dir().join(format!(
            "bareline-known-gaps-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let profile = |name: &str| {
            super::super::settings::SettingsRuntime::new(
                bareline_settings::SettingsDocument::empty(bareline_settings::Scope::User),
                Some(root.join(name).join("settings.toml")),
                std::sync::Arc::new(|| {}),
            )
        };
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        let mut first = profile("a");
        assert!(first.note_known_gap("gap"));
        assert!(!first.note_known_gap("gap"));
        let mut next_launch = profile("a");
        assert!(!next_launch.note_known_gap("gap"));
        assert!(next_launch.note_known_gap("another gap"));
        // A first launch creates its profile folder later: the record waits.
        let mut first_launch = profile("c");
        assert!(first_launch.note_known_gap("gap"));
        assert!(!root.join("c").join("known-gaps.txt").exists());
        std::fs::create_dir_all(root.join("c")).unwrap();
        first_launch.record_known_gaps();
        assert!(!profile("c").note_known_gap("gap"));
        // Through the shell: the notice leaves the message bar and becomes a
        // notification the first time only.
        for (launch, shown) in [(1, true), (2, false)] {
            let Some(&notice) = crate::shell::native::KNOWN_GAP_NOTICES.first() else {
                break;
            };
            let mut shell = super::super::accessibility::tests::headless_shell();
            shell.settings = profile("b");
            let mut workspace = bareline_app::workspace::Workspace::new(
                std::sync::Arc::new(|| {}),
                std::sync::Arc::new(crate::shell::native::FileSystem),
            )
            .unwrap();
            workspace.message = Some(notice.to_owned());
            shell.workspace = Some(workspace);
            shell.retire_known_gap_notices();
            assert_eq!(shell.workspace.as_ref().unwrap().message, None, "launch {launch}");
            shell.toasts.draw(
                &mut bareline_renderer_recording::RecordingBackend::default(),
                1200.0,
                760.0,
                Default::default(),
                &mut Vec::new(),
            );
            let notified = shell.toasts.accessibility().iter().any(|toast| toast.text == notice);
            assert_eq!(notified, shown, "launch {launch}");
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
