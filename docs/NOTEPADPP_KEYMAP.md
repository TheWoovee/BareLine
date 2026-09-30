# Notepad++ shortcut preset

Bareline ships two shortcut presets:

- **Bareline** (the default): the shortcuts listed under "Everyday shortcuts" in the [README](../README.md#everyday-shortcuts).
- **Notepad++**: Notepad++'s default shortcuts for the commands Bareline has, so keys such as Ctrl+D, Ctrl+Shift+F, Ctrl+Shift+Up/Down and Ctrl+Q do what a Notepad++ user expects.

## Choosing a preset

- On the first launch of a new profile, a notice points to the preset.
- **Settings > Keyboard > Shortcut preset** switches between the two (`[keyboard] preset = "bareline"` or `"notepad++"` in `settings.toml`; a user setting that workspace settings cannot change).
- **Settings > Import from Notepad++ > Use Notepad++ Shortcuts** and **Use Bareline Shortcuts** (also in the Command Palette) switch in one step. The active preset has a radio mark.
- **Apply Reviewed Notepad++ Import** switches to the Notepad++ preset and then lays the shortcuts customized in the imported file over it.

Shortcuts you change yourself, in the Shortcut Mapper, with a macro shortcut or by editing `keymap.toml`, stay on top of whichever preset is active, including shortcuts you removed. A preset key that you gave to another command stays with that command. `keymap.toml` records the preset it was laid out from (`preset = "notepad++"`; the Bareline preset writes no `preset` line), so the next switch can tell your changes from the preset's.

The preset table is `NOTEPAD_PLUS_PLUS` in `crates/commands/src/presets.rs`. Tests check that it names only registered commands, has no conflicting keys, reaches every command from a US keyboard, and that switching presets keeps your own shortcuts.

## What the Notepad++ preset changes

Commands that are not listed keep their Bareline shortcuts.

| Command | Bareline | Notepad++ preset |
| --- | --- | --- |
| Save As | Ctrl+Shift+S | Ctrl+Alt+S |
| Save All | none | Ctrl+Shift+S |
| Close All | none | Ctrl+Shift+W |
| Undo / Redo | Ctrl+Z / Ctrl+Y | also Alt+Backspace / Ctrl+Shift+Z |
| Cut / Copy / Paste | Ctrl+X / Ctrl+C / Ctrl+V | also Shift+Del / Ctrl+Ins / Shift+Ins |
| Duplicate Lines | Ctrl+Shift+D | Ctrl+D |
| Select Next Occurrence | Ctrl+D | none (Notepad++ has no default) |
| Select All Occurrences | Ctrl+Shift+L | none (Ctrl+Shift+L deletes the line in Notepad++) |
| Split Lines | none | Ctrl+I |
| Move Lines Up / Down | Alt+Up / Alt+Down | Ctrl+Shift+Up / Ctrl+Shift+Down |
| Uppercase / Lowercase / Title Case | none | Ctrl+Shift+U / Ctrl+U / Alt+U |
| Toggle Line Comment | Ctrl+/ | Ctrl+Q |
| Toggle Block Comment | Ctrl+Shift+/ | Ctrl+Shift+Q |
| Find in Folder (Find in Files) | none | Ctrl+Shift+F |
| Find in Open Documents | Ctrl+Shift+F | none |
| Show Search Panel (search results) | none | F7 |
| Focus Other View | F6 | F8 |
| Fold All / Unfold All | none | Alt+0 / Alt+Shift+0 |
| Fold Level 1–8 | none | Alt+1 to Alt+8 |
| Toggle Current Fold | none | Ctrl+Alt+F |
| Start Macro Recording | none | Ctrl+Shift+R |

These are the same in both presets: New, Open, Save, Close, Restore Last Closed Tab, Print, Exit, Select All, Indent/Unindent, Join Lines, Column Editor (Alt+C), Show Completion (Ctrl+Space), Find, Replace, Find Next/Previous, Go to Line, bookmarks (Ctrl+F2, F2, Shift+F2), tab navigation (Ctrl+PageUp/PageDown, Ctrl+Shift+PageUp/PageDown, Ctrl+Tab), Run (F5) and About (F1). Column selection with Alt+Shift+arrow keys works in both presets.

## Notepad++ defaults left unmapped

These Notepad++ default shortcuts have no Bareline command yet, or are left alone on purpose. Their keys do nothing in the Notepad++ preset unless you bind them yourself.

| Notepad++ command | Keys | Why |
| --- | --- | --- |
| Playback recorded macro | Ctrl+Shift+P | Kept for the Command Palette, Bareline's way to every command. Use Macro > Play Selected Macro. |
| Stop recording macro | Ctrl+Shift+R (second press) | One key cannot start and stop recording. Use Macro > Stop Macro Recording. |
| Hide lines | Alt+H | Alt+H opens Bareline's Help menu. Use Edit > Line Operations > Hide Selected Lines. |
| Line cut / line delete | Ctrl+L / Ctrl+Shift+L | No line cut or line delete command yet. |
| Transpose line | Ctrl+T | No command yet. |
| Single line comment / uncomment | Ctrl+K / Ctrl+Shift+K | Bareline only toggles comments (Ctrl+Q). |
| Go to matching brace / select to it | Ctrl+B / Ctrl+Alt+B | No command yet. |
| Select and find next / previous, volatile find | Ctrl+F3 / Ctrl+Shift+F3 / Ctrl+Alt+F3 / Ctrl+Alt+Shift+F3 | No command yet. |
| Incremental search | Ctrl+Alt+I | No command yet. |
| Next / previous search result | F4 / Shift+F4 | No command yet. |
| Unfold current level, unfold level 1–8 | Ctrl+Alt+Shift+F, Alt+Shift+1 to 8 | Bareline toggles folds; there are no unfold-level commands. |
| Word completion, path completion, function parameters hint | Ctrl+Enter / Ctrl+Alt+Space / Ctrl+Shift+Space | No separate commands. |
| Proper case (blend), sentence case | Alt+Shift+U / Ctrl+Alt+U / Ctrl+Alt+Shift+U | No such case commands. |
| Zoom in / out / restore | Ctrl+Numpad+ / Ctrl+Numpad- / Ctrl+Numpad/ | No zoom commands yet. |
| Full screen / post-it | F11 / F12 | No such view modes yet. |
