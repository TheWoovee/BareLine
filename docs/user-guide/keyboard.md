# Keyboard presets

Bareline has two built-in shortcut presets:

- **Bareline** (the default): the shortcuts listed under [Everyday shortcuts](../../README.md#everyday-shortcuts).
- **Notepad++**: Notepad++'s default shortcuts for the commands Bareline has, so that keys such as Ctrl+D (duplicate line), Ctrl+Shift+F (find in files), Ctrl+Shift+Up/Down (move lines) and Ctrl+Q (comment) do what a Notepad++ user expects.

[The Notepad++ preset](../NOTEPADPP_KEYMAP.md) lists every key it changes and the Notepad++ shortcuts that have no Bareline command yet.

## Choosing a preset

- On the first launch of a new profile, a notice points to the preset.
- **Settings > Keyboard > Shortcut preset** switches between the two. In `settings.toml` this is `[keyboard] preset = "bareline"` or `"notepad++"`; a workspace cannot change it.
- **Settings > Import from Notepad++** has **Use Notepad++ Shortcuts** and **Use Bareline Shortcuts**. The active preset has a radio mark.
- **Apply Reviewed Notepad++ Import** switches to the Notepad++ preset and then applies the shortcuts customized in the imported Notepad++ configuration that map to a Bareline command.

## Your own shortcuts

- **Settings > Shortcut Mapper** lists every command with its shortcut and lets you change or remove it.
- **Import Keymap**, **Export Keymap** and **Open Keymap File** read, write and open `keymap.toml` in your profile.
- **Assign Macro Shortcut** gives a saved macro a shortcut ([Macros](macros-and-external-commands.md#macros)).

Shortcuts you change yourself stay on top of whichever preset is active, including shortcuts you removed. A preset key that you gave to another command stays with that command when you switch presets.

## Keyboard layouts

Shortcuts are resolved from the key and modifiers you press. On layouts with an AltGr key, AltGr arrives as Ctrl+Alt: keys that produce a character with AltGr type that character, and other Ctrl+Alt shortcuts, such as **Add Caret Above** (Ctrl+Alt+Up), still work. Testing with physical non-US keyboards and input method editors is still pending ([STATUS.md](../STATUS.md)).
