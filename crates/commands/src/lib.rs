// SPDX-License-Identifier: MPL-2.0
use std::collections::BTreeMap;
mod contributions;
pub use contributions::*;
mod discovery;
mod keymap;
pub use discovery::*;
pub use keymap::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandId(pub &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A contributing crate owns behavior and resolves this stable ID at the composition root.
    Contributed(CommandId),
    New,
    Quit,
    About,
    Palette,
    Undo,
    Redo,
    SelectAll,
    Open,
    Save,
    SaveAs,
    Copy,
    Cut,
    Paste,
    CancelFileOperations,
    Close,
    Find,
    FindNext,
    FindPrevious,
    FindClose,
    FindMatchCase,
    FindWholeWord,
    Replace,
    ReplaceOne,
    ReplaceAll,
    FindMode,
    FindCancel,
}

#[derive(Clone, Debug)]
pub struct CommandSpec {
    pub id: CommandId,
    pub title: &'static str,
    pub category: &'static str,
    pub shortcut: &'static str,
    pub action: Action,
}

#[derive(Default)]
pub struct CommandRegistry {
    entries: BTreeMap<CommandId, CommandSpec>,
    presentations: BTreeMap<CommandId, CommandPresentation>,
    pub contributions: DynamicContributions,
}

impl CommandRegistry {
    pub fn register(&mut self, spec: CommandSpec) -> Result<(), CommandId> {
        if self.entries.contains_key(&spec.id) {
            return Err(spec.id);
        }
        self.entries.insert(spec.id, spec);
        Ok(())
    }
    pub fn dispatch(&self, id: CommandId) -> Option<Action> {
        self.entries.get(&id).map(|s| s.action)
    }
    pub fn entries(&self) -> impl Iterator<Item = &CommandSpec> {
        self.entries.values()
    }
    pub fn shortcut(&self, key: &str) -> Option<Action> {
        if key.is_empty() {
            return None;
        }
        self.entries
            .values()
            .find(|s| s.shortcut == key)
            .map(|s| s.action)
    }
}

pub fn shell_commands() -> CommandRegistry {
    let mut registry = CommandRegistry::default();
    for (id, title, category, shortcut, action) in [
        (
            "search.cancel",
            "Cancel Search / Replacement",
            "Search",
            "",
            Action::FindCancel,
        ),
        ("file.new", "New", "File", "Ctrl+N", Action::New),
        (
            "search.whole_word",
            "Whole Word",
            "Search",
            "",
            Action::FindWholeWord,
        ),
        ("file.close", "Close", "File", "Ctrl+W", Action::Close),
        ("search.find", "Find…", "Search", "Ctrl+F", Action::Find),
        (
            "search.replace",
            "Replace…",
            "Search",
            "Ctrl+H",
            Action::Replace,
        ),
        (
            "search.replace_one",
            "Replace Selected Match",
            "Search",
            "",
            Action::ReplaceOne,
        ),
        (
            "search.replace_all",
            "Replace All in Current Document",
            "Search",
            "",
            Action::ReplaceAll,
        ),
        (
            "search.mode",
            "Toggle Literal / Extended",
            "Search",
            "",
            Action::FindMode,
        ),
        (
            "search.find_next",
            "Find Next",
            "Search",
            "F3",
            Action::FindNext,
        ),
        (
            "search.find_previous",
            "Find Previous",
            "Search",
            "Shift+F3",
            Action::FindPrevious,
        ),
        (
            "search.close_find",
            "Close Find",
            "Search",
            "",
            Action::FindClose,
        ),
        (
            "search.match_case",
            "Match Case",
            "Search",
            "",
            Action::FindMatchCase,
        ),
        ("edit.copy", "Copy", "Edit", "Ctrl+C", Action::Copy),
        ("edit.cut", "Cut", "Edit", "Ctrl+X", Action::Cut),
        ("edit.paste", "Paste", "Edit", "Ctrl+V", Action::Paste),
        ("file.open", "Open…", "File", "Ctrl+O", Action::Open),
        ("file.save", "Save", "File", "Ctrl+S", Action::Save),
        (
            "file.cancel_operations",
            "Cancel File Operations",
            "File",
            "",
            Action::CancelFileOperations,
        ),
        (
            "file.save_as",
            "Save As…",
            "File",
            "Ctrl+Shift+S",
            Action::SaveAs,
        ),
        ("app.quit", "Exit", "File", "Alt+F4", Action::Quit),
        (
            "view.command_palette",
            "Command Palette",
            "View",
            "Ctrl+Shift+P",
            Action::Palette,
        ),
        ("help.about", "About Bareline", "Help", "F1", Action::About),
        ("edit.undo", "Undo", "Edit", "Ctrl+Z", Action::Undo),
        ("edit.redo", "Redo", "Edit", "Ctrl+Y", Action::Redo),
        (
            "edit.select_all",
            "Select All",
            "Edit",
            "Ctrl+A",
            Action::SelectAll,
        ),
    ] {
        registry
            .register(CommandSpec {
                id: CommandId(id),
                title,
                category,
                shortcut,
                action,
            })
            .expect("unique built-in command");
        let keywords = match action {
            Action::Palette => vec!["commands".into(), "actions".into()],
            Action::Find => vec!["search".into(), "locate".into()],
            Action::Quit => vec!["quit".into(), "exit".into()],
            Action::SaveAs => vec!["export".into(), "write copy".into()],
            Action::Undo => vec!["revert edit".into()],
            _ => Vec::new(),
        };
        registry
            .set_presentation(
                CommandId(id),
                CommandPresentation {
                    menu_path: category.into(),
                    keywords,
                    ..CommandPresentation::default()
                },
            )
            .expect("registered command");
    }
    registry
        .register(CommandSpec {
            id: CommandId("internal.dynamic.invoke"),
            title: "Invoke contributed command",
            category: "Tools",
            shortcut: "",
            action: Action::Contributed(CommandId("internal.dynamic.invoke")),
        })
        .expect("unique dynamic envelope");
    registry
        .set_presentation(
            CommandId("internal.dynamic.invoke"),
            CommandPresentation {
                internal: true,
                ..Default::default()
            },
        )
        .expect("registered dynamic envelope");
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_registration_preserves_original_dispatch() {
        let mut r = shell_commands();
        let mut duplicate = r
            .entries()
            .find(|s| s.id == CommandId("file.new"))
            .unwrap()
            .clone();
        duplicate.action = Action::Quit;
        assert_eq!(r.register(duplicate), Err(CommandId("file.new")));
        assert_eq!(r.shortcut("Ctrl+N"), Some(Action::New));
        assert_eq!(r.dispatch(CommandId("unknown")), None);
    }
}
