// SPDX-License-Identifier: MPL-2.0
//! Built-in keymap presets (BIZ-08). A preset is the default keymap with a
//! table laid over it: every command the table names gets exactly the table's
//! chords (none for an empty list), and any other default binding that collides
//! with one of them is dropped. `Keymap::rebase` carries a person's own shortcut
//! changes from one preset to another.
use crate::{CommandId, CommandRegistry, KeyBinding, KeyChord, Keymap};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeymapPreset {
    /// The shortcuts each command registers.
    #[default]
    Bareline,
    /// Notepad++'s default shortcuts for the commands Bareline has.
    NotepadPlusPlus,
}
impl KeymapPreset {
    pub const ALL: [Self; 2] = [Self::Bareline, Self::NotepadPlusPlus];
    /// The value stored in settings and keymap files.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Bareline => "bareline",
            Self::NotepadPlusPlus => "notepad++",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.id() == id)
    }
    pub const fn title(self) -> &'static str {
        match self {
            Self::Bareline => "Bareline",
            Self::NotepadPlusPlus => "Notepad++",
        }
    }
    /// (command ID, chords) laid over the default keymap; empty for Bareline.
    pub const fn table(self) -> &'static [(&'static str, &'static [&'static str])] {
        match self {
            Self::Bareline => &[],
            Self::NotepadPlusPlus => NOTEPAD_PLUS_PLUS,
        }
    }
    /// The table as bindings, one per chord. Commands missing from `registry`
    /// (optional features) are skipped.
    pub fn bindings(self, registry: &CommandRegistry) -> Vec<KeyBinding> {
        self.table()
            .iter()
            .filter(|&&(id, _)| registry.spec(CommandId(id)).is_some())
            .flat_map(|&(id, chords)| {
                chords.iter().map(move |chord| KeyBinding {
                    command: CommandId(id),
                    sequence: vec![KeyChord::parse(chord).expect("valid preset shortcut")],
                })
            })
            .collect()
    }
}

/// Notepad++'s default shortcuts (Settings > Shortcut Mapper on a fresh
/// install) for the commands Bareline has. An empty list takes a Bareline
/// default away where the same keys mean something else in Notepad++.
/// Notepad++ commands that Bareline does not have are left unmapped.
const NOTEPAD_PLUS_PLUS: &[(&str, &[&str])] = &[
    // File
    ("file.new", &["Ctrl+N"]),
    ("file.open", &["Ctrl+O"]),
    ("file.save", &["Ctrl+S"]),
    ("file.save_as", &["Ctrl+Alt+S"]),
    ("file.save_all", &["Ctrl+Shift+S"]),
    ("file.close", &["Ctrl+W"]),
    ("view.tabs.closeAll", &["Ctrl+Shift+W"]),
    ("file.restore_closed", &["Ctrl+Shift+T"]),
    ("utilities.print", &["Ctrl+P"]),
    ("app.quit", &["Alt+F4"]),
    // Edit
    ("edit.undo", &["Ctrl+Z", "Alt+Backspace"]),
    ("edit.redo", &["Ctrl+Y", "Ctrl+Shift+Z"]),
    ("edit.cut", &["Ctrl+X", "Shift+Delete"]),
    ("edit.copy", &["Ctrl+C", "Ctrl+Insert"]),
    ("edit.paste", &["Ctrl+V", "Shift+Insert"]),
    ("edit.select_all", &["Ctrl+A"]),
    ("editor.indent", &["Tab"]),
    ("editor.unindent", &["Shift+Tab"]),
    ("editor.lines.duplicate", &["Ctrl+D"]),
    ("editor.lines.split", &["Ctrl+I"]),
    ("editor.lines.join", &["Ctrl+J"]),
    ("editor.lines.moveUp", &["Ctrl+Shift+Up"]),
    ("editor.lines.moveDown", &["Ctrl+Shift+Down"]),
    ("editor.case.upper", &["Ctrl+Shift+U"]),
    ("editor.case.lower", &["Ctrl+U"]),
    ("editor.case.title", &["Alt+U"]),
    ("editor.comment.toggleLine", &["Ctrl+Q"]),
    ("editor.comment.toggleBlock", &["Ctrl+Shift+Q"]),
    ("editor.completion.show", &["Ctrl+Space"]),
    ("editor.column.insert", &["Alt+C"]),
    // Ctrl+D duplicates and Ctrl+Shift+L deletes the line in Notepad++, whose
    // multi-select commands have no default shortcut.
    ("editor.selection.nextOccurrence", &[]),
    ("editor.selection.allOccurrences", &[]),
    // Search
    ("search.find", &["Ctrl+F"]),
    ("search.folder", &["Ctrl+Shift+F"]),
    ("search.open_documents", &[]),
    ("search.find_next", &["F3"]),
    ("search.find_previous", &["Shift+F3"]),
    ("search.replace", &["Ctrl+H"]),
    ("view.bottom_panel.search", &["F7"]),
    ("search.goto", &["Ctrl+G"]),
    ("editor.bookmark.toggle", &["Ctrl+F2"]),
    ("editor.bookmark.next", &["F2"]),
    ("editor.bookmark.previous", &["Shift+F2"]),
    // View
    ("view.tabs.next", &["Ctrl+PageDown"]),
    ("view.tabs.previous", &["Ctrl+PageUp"]),
    ("view.tabs.move_right", &["Ctrl+Shift+PageDown"]),
    ("view.tabs.move_left", &["Ctrl+Shift+PageUp"]),
    ("view.tabs.mru", &["Ctrl+Tab"]),
    ("view.focus_other", &["F8"]),
    ("view.fold.all", &["Alt+0"]),
    ("view.fold.unfoldAll", &["Alt+Shift+0"]),
    ("view.fold.toggleCurrent", &["Ctrl+Alt+F"]),
    ("view.fold.level1", &["Alt+1"]),
    ("view.fold.level2", &["Alt+2"]),
    ("view.fold.level3", &["Alt+3"]),
    ("view.fold.level4", &["Alt+4"]),
    ("view.fold.level5", &["Alt+5"]),
    ("view.fold.level6", &["Alt+6"]),
    ("view.fold.level7", &["Alt+7"]),
    ("view.fold.level8", &["Alt+8"]),
    // Macro, Run and Help
    ("macro.record", &["Ctrl+Shift+R"]),
    ("run.prompt", &["F5"]),
    ("help.about", &["F1"]),
];

impl Keymap {
    /// The default keymap with `preset`'s table laid over it.
    pub fn preset(registry: &CommandRegistry, preset: KeymapPreset) -> Self {
        let table = preset.bindings(registry);
        let named = |command: CommandId| preset.table().iter().any(|&(id, _)| id == command.0);
        let mut bindings: Vec<KeyBinding> = Self::defaults(registry)
            .bindings
            .into_iter()
            .filter(|default| !named(default.command) && !table.iter().any(|binding| overlaps(binding, default)))
            .collect();
        bindings.extend(table);
        Self { bindings }
    }
    /// This keymap moved from base `from` onto base `onto`. A command whose
    /// bindings differ from `from` was changed by the person: it keeps exactly
    /// its bindings here (none when they were removed) and wins over any binding
    /// of `onto` it collides with. A command that only lost bindings to such a
    /// change, and every unchanged command, takes its bindings from `onto`.
    pub fn rebase(&self, from: &Keymap, onto: &Keymap, registry: &CommandRegistry) -> Result<Keymap, String> {
        let mut commands: Vec<CommandId> = self
            .bindings
            .iter()
            .chain(&from.bindings)
            .map(|binding| binding.command)
            .collect();
        commands.sort();
        commands.dedup();
        let changed: Vec<CommandId> = commands
            .iter()
            .copied()
            .filter(|&command| {
                gained(self, from, command).next().is_some()
                    || from.bindings.iter().any(|lost| {
                        // A lost binding that no other command took over was
                        // removed on purpose.
                        lost.command == command
                            && !self.bindings.contains(lost)
                            && !commands.iter().any(|&other| {
                                other != command && gained(self, from, other).any(|binding| overlaps(binding, lost))
                            })
                    })
            })
            .collect();
        let kept: Vec<KeyBinding> = self
            .bindings
            .iter()
            .filter(|binding| changed.contains(&binding.command))
            .cloned()
            .collect();
        let mut bindings: Vec<KeyBinding> = onto
            .bindings
            .iter()
            .filter(|binding| !changed.contains(&binding.command) && !kept.iter().any(|keep| overlaps(keep, binding)))
            .cloned()
            .collect();
        bindings.extend(kept);
        let mut keymap = Keymap::default();
        keymap.replace(bindings, registry)?;
        Ok(keymap)
    }
}
/// Bindings of `command` in `keymap` that `from` does not have.
fn gained<'a>(keymap: &'a Keymap, from: &'a Keymap, command: CommandId) -> impl Iterator<Item = &'a KeyBinding> {
    keymap
        .bindings
        .iter()
        .filter(move |binding| binding.command == command && !from.bindings.contains(binding))
}
/// The two bindings cannot both be in one keymap (see `Keymap::conflicts`).
fn overlaps(first: &KeyBinding, second: &KeyBinding) -> bool {
    first.sequence.starts_with(&second.sequence) || second.sequence.starts_with(&first.sequence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_commands;

    fn sorted(keymap: &Keymap) -> Vec<(CommandId, String)> {
        let mut bindings: Vec<_> = keymap
            .bindings()
            .iter()
            .map(|binding| {
                (
                    binding.command,
                    binding
                        .sequence
                        .iter()
                        .map(KeyChord::label)
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            })
            .collect();
        bindings.sort();
        bindings
    }
    /// `keymap` with `command` bound to exactly `chord` (unbound for `None`).
    fn bind(keymap: &Keymap, command: &'static str, chord: Option<&str>, registry: &CommandRegistry) -> Keymap {
        let mut bindings: Vec<KeyBinding> = keymap
            .bindings()
            .iter()
            .filter(|binding| binding.command.0 != command)
            .cloned()
            .collect();
        bindings.extend(chord.map(|chord| KeyBinding {
            command: CommandId(command),
            sequence: vec![KeyChord::parse(chord).unwrap()],
        }));
        let mut next = Keymap::default();
        next.replace(bindings, registry).unwrap();
        next
    }

    #[test]
    fn notepad_plus_plus_table_parses_and_has_no_internal_conflicts() {
        let mut commands: Vec<&str> = NOTEPAD_PLUS_PLUS.iter().map(|&(id, _)| id).collect();
        let bindings: Vec<KeyBinding> = NOTEPAD_PLUS_PLUS
            .iter()
            .flat_map(|&(id, chords)| {
                chords.iter().map(move |chord| KeyBinding {
                    command: CommandId(id),
                    sequence: vec![KeyChord::parse(chord).unwrap_or_else(|error| panic!("{id} {chord}: {error}"))],
                })
            })
            .collect();
        let conflicts = Keymap::conflicts(&bindings);
        assert!(
            conflicts.is_empty(),
            "the Notepad++ table binds one chord twice: {conflicts:?}"
        );
        let listed = commands.len();
        commands.sort_unstable();
        commands.dedup();
        assert_eq!(commands.len(), listed, "a command is listed twice");
        assert!(KeymapPreset::Bareline.table().is_empty());
        for preset in KeymapPreset::ALL {
            assert_eq!(KeymapPreset::from_id(preset.id()), Some(preset));
        }
        assert_eq!(KeymapPreset::from_id("emacs"), None);
    }

    #[test]
    fn a_preset_replaces_its_commands_and_drops_colliding_defaults() {
        let registry = shell_commands();
        assert_eq!(
            sorted(&Keymap::preset(&registry, KeymapPreset::Bareline)),
            sorted(&Keymap::defaults(&registry))
        );
        let notepad = Keymap::preset(&registry, KeymapPreset::NotepadPlusPlus);
        assert!(Keymap::conflicts(notepad.bindings()).is_empty());
        assert_eq!(notepad.shortcut_label(CommandId("file.save_as")), "Ctrl+Alt+S");
        assert_eq!(notepad.shortcut_label(CommandId("edit.redo")), "Ctrl+Y, Ctrl+Shift+Z");
        // Commands the table does not name keep their defaults.
        assert_eq!(
            notepad.shortcut_label(CommandId("view.command_palette")),
            "Ctrl+Shift+P"
        );
    }

    #[test]
    fn switching_presets_preserves_user_overrides() {
        let registry = shell_commands();
        let bareline = Keymap::preset(&registry, KeymapPreset::Bareline);
        let notepad = Keymap::preset(&registry, KeymapPreset::NotepadPlusPlus);
        // The person moved New onto the chord Notepad++ uses for Save As,
        // removed About's shortcut and gave Find a second chord.
        let user = bind(&bareline, "file.new", Some("Ctrl+Alt+S"), &registry);
        let user = bind(&user, "help.about", None, &registry);
        let mut bindings = user.bindings().to_vec();
        bindings.push(KeyBinding {
            command: CommandId("search.find"),
            sequence: vec![KeyChord::parse("Ctrl+K").unwrap(), KeyChord::parse("Ctrl+F").unwrap()],
        });
        let mut user = user;
        user.replace(bindings, &registry).unwrap();

        let moved = user.rebase(&bareline, &notepad, &registry).unwrap();
        assert_eq!(moved.shortcut_label(CommandId("file.new")), "Ctrl+Alt+S");
        assert_eq!(moved.shortcut_label(CommandId("help.about")), "");
        assert_eq!(moved.shortcut_label(CommandId("search.find")), "Ctrl+F, Ctrl+K Ctrl+F");
        // Save As loses the chord the person took; the rest follow Notepad++.
        assert_eq!(moved.shortcut_label(CommandId("file.save_as")), "");
        assert_eq!(moved.shortcut_label(CommandId("edit.redo")), "Ctrl+Y, Ctrl+Shift+Z");
        assert!(Keymap::conflicts(moved.bindings()).is_empty());

        // Switching back restores Bareline's bindings with the overrides on
        // top, and Save As is not mistaken for a removal the person made.
        let back = moved.rebase(&notepad, &bareline, &registry).unwrap();
        assert_eq!(sorted(&back), sorted(&user));
        assert_eq!(back.shortcut_label(CommandId("file.save_as")), "Ctrl+Shift+S");

        // Without overrides a switch lands exactly on the preset.
        assert_eq!(
            sorted(&bareline.rebase(&bareline, &notepad, &registry).unwrap()),
            sorted(&notepad)
        );
    }
}
