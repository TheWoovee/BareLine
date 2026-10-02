# Coming from Notepad++

Bareline is a Windows text and code editor in the same family as Notepad++: tabs, two views, Scintilla's Lexilla lexers for syntax highlighting, PCRE2-style regular expressions, macros, Run (F5) and a Shortcut Mapper. This page maps what you know from Notepad++ to Bareline, lists what is intentionally different, and names the gaps. The full feature-by-feature status is in the [Notepad++ parity matrix](PARITY.md).

Notepad++ names below refer to Notepad++ 8.9.x, the version Bareline's comparison harness pins ([performance evidence](perf/README.md)).

## First steps

1. **Use Notepad++ shortcuts.** Set **Settings > Keyboard > Shortcut preset** to **Notepad++**, or choose **Settings > Import from Notepad++ > Use Notepad++ Shortcuts**. [The Notepad++ preset](NOTEPADPP_KEYMAP.md) lists every key it changes.
2. **Import a few preferences.** **Settings > Import from Notepad++ > Review Notepad++ Import…** reads one Notepad++ XML file of up to 1 MiB, such as `config.xml`, `stylers.xml`, `shortcuts.xml` or `session.xml` from `%APPDATA%\Notepad++`, and shows a report of what it would change. **Apply Reviewed Notepad++ Import** then applies it:
   - from `config.xml`: tab width and tabs-or-spaces, line numbers, current-line highlight and word wrap;
   - from `stylers.xml`: the default style's text and background colors, font and font size;
   - from `shortcuts.xml`: your customized shortcuts for New, Open, Close, Save, Find and Replace, laid over the Notepad++ preset;
   - from `session.xml` and the history list: local file paths, which **Open Imported Local Files** opens.

   Everything else is listed in the report as skipped. Plugin state, macros and Run commands are never imported, and no file the import names is followed or executed.
3. **Import user-defined languages and function lists.** **Language > User-Defined > Import User-defined Language…** reads a Notepad++ UDL file. Keywords, operators, comments, delimiters, folding markers and number rules are mapped; rules Bareline cannot express are reported as approximated or unsupported, and a name that collides with an existing language gets a suffix. **Language > Outline Definitions > Import Notepad++ Function List…** reads a Notepad++ function-list definition for the outline panel.

## Where things are

| In Notepad++ | In Bareline |
| --- | --- |
| Command access by menu only | Every command is also in the **Command Palette** (**Ctrl+Shift+P**), searchable by name. |
| **Settings > Preferences** | **Settings** (a page with categories, plus **Open settings.toml**). |
| **Settings > Shortcut Mapper** | **Settings > Shortcut Mapper**. |
| **Settings > Style Configurator** | Light, dark or system theme and theme color overrides in **Settings > Appearance**. There is no per-language style editor. |
| **File > Close More** (Close All, All But Current, To the Left/Right) | **File > Close Multiple**. |
| **File > Open Containing Folder**, **Copy Full Path** and similar | **File > Document**: **Open Containing Folder**, **Open Terminal Here**, **Copy Full Path**, **Copy File Name**, **Copy Directory Path**, **Rename…**. |
| **File > Load Session / Save Session** | **File > Recent Files > Load Session… / Save Session As…**. |
| **File > Open Folder as Workspace** | **File > Open Workspace Folder…**, with the workspace panel under **View > Panels**. |
| **Edit > Line Operations** | **Edit > Line Operations** and **Edit > Sort**: duplicate, move, join, split, remove empty or blank lines, remove duplicates or consecutive duplicates, sort ascending, descending, numeric and case-insensitive. |
| **Edit > Convert Case to** | **Edit > Case**: upper, lower, title and inverted case. |
| **Edit > Comment/Uncomment** | **Edit > Comment**: toggle line comment, toggle block comment. |
| **Edit > Blank Operations** | **Edit > Indent**: trim leading, trailing or both, tabs to spaces, spaces to tabs. |
| **Edit > Column Editor** (Alt+C), column mode (Alt+Shift+arrows) | The same keys, plus **Edit > Column > Column Selection Mode** for mouse and Shift+arrow rectangles. |
| Multi-editing | **Edit > Multi-caret**: add caret above or below, select next, skip, or all occurrences. |
| **Search > Find in Files** | **Search > Scope > Find in Folder…** (Ctrl+Shift+F in the Notepad++ preset), and [Replace in Files](user-guide/replace-in-files.md) with a preview and backups. |
| **Find All in All Opened Documents** | **Find in Open Documents** (Ctrl+Shift+F in the Bareline preset). |
| **Search > Mark** (five styles) | **Search > Mark**: five styles, each with its own clear command. |
| **Search > Bookmark** | **Edit > Bookmarks**: toggle, next, previous, clear, and select, copy, cut or delete the bookmarked lines. |
| **Search > Go to Matching Brace** (Ctrl+B) | **Search > Matching Brace**: Ctrl+B goes to it, Ctrl+Shift+B selects to it. |
| **View > Show Symbol**, **Word wrap**, **Zoom**, **Always on Top**, **Toggle Full Screen** (F11) | **View > Show**, **View > Word Wrap**, **View > Zoom**, **View > Always on Top**, **View > Full Screen** (F11). |
| **View > Fold All / Unfold All / Fold Level** | **View > Fold** (Alt+0, Alt+Shift+0 and Alt+1 to Alt+8 in the Notepad++ preset). |
| **View > Hide Lines** | **Edit > Line Operations > Hide Selected Lines** and **Show Hidden Lines**. |
| **View > Document Map / Function List / Document List** | **View > Panels**: document map, outline, document list. |
| **View > Move/Clone to Other View**, synchronized scrolling | **View > Split**. |
| **View > Monitoring (tail -f)** | **File > Document > Monitoring and Remote > Follow New Content**, or `--monitor`. |
| **Encoding > Encode in…** | **Encoding > Interpret As** (reread the original bytes). |
| **Encoding > Convert to…** | **Encoding > Convert To** (change the save encoding). See [Encodings](user-guide/encodings.md). |
| **Macro** menu | **Macro** menu: record, stop, play, play N times or to end of file, save, rename, shortcut. |
| **Run > Run…** (F5) with `$(FULL_CURRENT_PATH)` and other variables | **Run > Run…** (F5), with the same variables. See [Macros and external commands](user-guide/macros-and-external-commands.md). |
| **Tools > MD5 / SHA-1 / SHA-256 / SHA-512** | **Tools > Utilities**. |
| **Window > Windows…** | **Window** menu, **View > Panels > Document List**, and Ctrl+Tab. |
| **? > About** (F1) | **Help > About Bareline** (F1). |

### What replaces common plugins

Bareline 1.0 runs no plugins. These built-in features cover what some of the plugins bundled with or commonly added to Notepad++ do; the tools are not the plugins and do not match them feature for feature.

| Plugin | Built in to Bareline |
| --- | --- |
| Compare / ComparePlus | **Tools > Compare** ([Compare](user-guide/compare.md)). |
| JSON Viewer, JSTool | **Tools > JSON, XML and Hex**: format, minify and validate JSON. There is no JSON tree view. |
| XML Tools | Format and validate XML, and XPath queries. DTDs and external entities are never loaded. |
| HEX-Editor | **Hex View**: a read-only view of the first 1 MiB of a file's bytes. There is no hex editing. |
| DSpellCheck | [Spell check](user-guide/spell-check.md) with the Windows spelling dictionaries. |
| NppExport | **Tools > Export**: syntax-colored HTML and RTF. |
| MIME Tools | **Tools > Utilities**: Base64 and URL encode and decode. |

## Command line

Bareline accepts the Notepad++ spellings `-n<line>`, `-c<column>`, `-ro`, `-multiInst`, `-nosession` and `-noPlugin`, ignores `-notabbar`, and refuses other Notepad++ switches such as `-l<language>`, `-x`/`-y` or `-openSession` as unknown options. A launch opens at most 16 files and names the rest in a notice. See [Command line](../README.md#command-line).

## What is intentionally different

- **No plugins.** Plugins run inside the editor process with its full rights. Bareline 1.0 ships the most-used tools built in instead; a separate, sandboxed extension host exists in the repository but is disabled in preview builds.
- **Recovery instead of periodic backup.** Notepad++ can save session snapshots and periodic backups. Bareline journals unsaved edits continuously in the background and offers them in a Recovery Center after a crash ([Recovery](user-guide/recovery.md)).
- **Replace in Files is reviewed.** Bareline shows a preview, keeps backups and a receipt, and can roll a job back; Notepad++ replaces in the files directly.
- **Large files are paged.** Files above 256 MiB are not loaded whole. Line numbers in such files are estimates until a background count finishes ([Large files](user-guide/large-files.md)).
- **Run asks first and quotes values.** Run (F5) shows the resolved program and asks for confirmation before anything starts, never looks in the current folder for the program, and refuses a variable value it cannot pass to the target program as plain data.
- **Some keys mean something else** in the default Bareline preset: Ctrl+D selects the next occurrence, Ctrl+Shift+F finds in open documents, lines move with Alt+Up/Down and comments toggle with Ctrl+/. The Notepad++ preset gives Ctrl+D, Ctrl+Shift+F, Ctrl+Shift+Up/Down and Ctrl+Q their Notepad++ meanings. Ctrl+Shift+P opens the Command Palette in both presets.
- **Profile location.** Bareline keeps its profile in `%LOCALAPPDATA%\Bareline`, or in a `data` folder beside a portable copy marked by an empty `bareline.portable` file. Settings are TOML files, not XML.
- **Software drawing by default.** Bareline draws with the processor unless you choose hardware drawing.

## Known gaps

These Notepad++ features have no Bareline equivalent yet. [PARITY.md](PARITY.md) tracks them, and the [Notepad++ preset page](NOTEPADPP_KEYMAP.md#notepad-defaults-left-unmapped) lists the shortcuts left unmapped.

- Plugins and Plugins Admin.
- User-interface languages other than English.
- About 66 built-in languages against the larger Notepad++ set; no user-defined language editor dialog that matches Notepad++'s.
- Line delete and line cut (Ctrl+L, Ctrl+Shift+L), transpose line (Ctrl+T), separate comment and uncomment commands, sentence case and random case.
- Incremental search, select-and-find-next (Ctrl+F3), next and previous search result (F4), find characters in range.
- Folder comparison and an inline diff view.
- Hex editing, a JSON tree view, distraction-free mode and post-it mode.
- Network shares can be opened only through **Open Remote File with Permission** and cannot be saved in place.
- 32-bit and ARM64 builds.
- Macros recorded in Notepad++ are not imported. Bareline macros replay Find Next and Find Previous but not Replace.
