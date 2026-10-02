# Notepad++ parity and competitor matrix

This page compares Bareline's current preview with Notepad++ feature by feature, then places Bareline among other editors people use for the same jobs. It describes what exists, not how well it performs; performance comparisons are published only from a qualified benchmark ([performance evidence](perf/README.md)). Whether a Bareline feature has been verified is shown in [STATUS.md](STATUS.md).

## Notepad++ parity matrix

Compared with Notepad++ 8.9.x and the plugins it commonly ships with. **Have**: Bareline has an equivalent. **Partial**: Bareline has part of it, as described. **Missing**: no equivalent yet. **Different**: deliberately done another way.

### Files, tabs and sessions

| Notepad++ feature | Bareline | Notes |
| --- | --- | --- |
| Tabs, two views, move or clone to the other view | Have | Horizontal or vertical split, synchronized scrolling. |
| Close All / All But Current / To the Left / To the Right | Have | **File > Close Multiple**. |
| Copy full path, file name, directory; Open Containing Folder; open a terminal | Have | **File > Document**. |
| Rename | Partial | Not available for documents in large-file mode. |
| Recent files | Have | With pinning; a separate Recent Folders list. |
| Save and load sessions | Have | Named session files. |
| Session snapshot and periodic backup | Different | Continuous recovery journals and a Recovery Center ([Recovery](user-guide/recovery.md)). |
| File change detection and reload | Have | Keep or reload; optional automatic reload of clean local files. |
| Monitoring (tail -f) | Have | **Follow New Content** or `--monitor`. |
| Files on network shares | Partial | Read through **Open Remote File with Permission**; no in-place save yet (Save Copy). |
| Drag and drop of files and folders | Have | A dropped folder opens as the workspace. |
| Folder as Workspace | Have | |
| Print, Print Now | Have | With header, footer, line numbers and syntax colors. |
| Portable mode | Different | An empty `bareline.portable` marker instead of `doLocalConf.xml`. |
| Command-line switches | Partial | `-n`, `-c`, `-ro`, `-multiInst`, `-nosession`, `-noPlugin` accepted, `-notabbar` ignored; `-l`, `-x`, `-y`, `-openSession` and other switches refused. At most 16 files per launch. |

### Editing

| Notepad++ feature | Bareline | Notes |
| --- | --- | --- |
| Undo and redo | Have | Undo history is bounded by memory settings. |
| Column mode and Column Editor | Have | Alt+Shift+arrows, Alt+C, and Column Selection Mode. |
| Multi-editing | Have | Add caret above or below, select next, skip or all occurrences. |
| Duplicate, move, join, split lines | Have | Split is at 80 columns. |
| Remove empty, blank, duplicate and consecutive duplicate lines | Have | |
| Sort lines | Partial | Ascending, descending, numeric and case-insensitive; no sort by length, random order or reverse line order. |
| Line delete, line cut, transpose line | Missing | |
| Convert case | Partial | Upper, lower, title and inverted case; no sentence or random case. |
| Comment and uncomment | Partial | Toggle line and toggle block comment; no separate comment and uncomment commands. |
| Blank operations | Partial | Trim leading, trailing or both, tabs to spaces and spaces to tabs; no end-of-line-to-space commands. |
| Auto-completion and call tips | Partial | Local word and function completion and static signatures; no language server. |
| Brace matching | Have | Highlight, go to (Ctrl+B) and select to (Ctrl+Shift+B). |
| Insert and overwrite modes | Have | |
| Clipboard history | Have | Off by default, kept in memory only. |
| Hide lines | Have | |

### Search and replace

| Notepad++ feature | Bareline | Notes |
| --- | --- | --- |
| Normal, Extended and Regular expression modes | Have | PCRE2 with line-based `^` and `$` like Notepad++ ([Search](user-guide/search-and-regex.md)). |
| `. matches newline` | Have | |
| Case-changing replacement escapes (`\U`, `\L`, `\E`) | Missing | Refused rather than inserted literally. |
| Find in Files, Replace in Files | Have | Replace in Files adds a preview, backups, a receipt and rollback ([Replace in Files](user-guide/replace-in-files.md)). |
| Find All in All Opened Documents | Have | |
| Mark with five styles | Have | |
| Bookmarks and bookmarked-line operations | Partial | Toggle, next, previous, clear, select, copy, cut and delete bookmarked lines; no inverse bookmark or remove unmarked lines. |
| Incremental search | Missing | |
| Select and Find Next (Ctrl+F3), volatile find | Missing | |
| Next and previous search result (F4) | Missing | |
| Find characters in range | Missing | |
| Go to line | Have | |

### View

| Notepad++ feature | Bareline | Notes |
| --- | --- | --- |
| Word wrap, show whitespace and end of line, indent guides, edge line | Have | |
| Zoom | Have | Ctrl+wheel and zoom commands. |
| Full screen, always on top | Have | |
| Post-it and distraction-free modes | Missing | |
| Document map, function list, document list | Have | Function list as the outline panel. |
| Fold all, unfold all, fold level 1 to 8 | Have | |
| Unfold a single level | Missing | |
| Dark mode | Have | Light, dark or system theme. |
| Vertical tabs and tab colors | Have | |

### Encodings and languages

| Notepad++ feature | Bareline | Notes |
| --- | --- | --- |
| Encode in / Convert to | Have | **Interpret As** and **Convert To** ([Encodings](user-guide/encodings.md)). |
| Character sets | Partial | Unicode, ISO-8859, Windows, KOI8, Mac, OEM and the main CJK encodings; no stateful encodings such as ISO-2022-JP. |
| Byte order mark and line-ending conversion | Have | |
| Built-in languages | Partial | 66 languages with Lexilla lexers; Notepad++ ships more. |
| User Defined Languages | Partial | Import of Notepad++ UDL files with a report of unsupported rules; no UDL editor dialog like Notepad++'s. |
| Style Configurator | Partial | Theme and color overrides; no per-language style editor. |
| Function list definitions | Partial | Import of Notepad++ function-list files. |

### Macros, Run and tools

| Notepad++ feature | Bareline | Notes |
| --- | --- | --- |
| Record, play, save, run N times or to end of file | Have | A playback is undone in one step. |
| Macros that include Replace | Missing | Macros replay Find Next and Find Previous, not Replace. |
| Macro shortcuts | Have | |
| Run (F5) with `$(FULL_CURRENT_PATH)` and other variables | Different | Same variables; Bareline confirms before running and refuses values it cannot pass safely ([Run](user-guide/macros-and-external-commands.md#run-f5)). |
| Saved Run commands in the Run menu | Partial | One loaded external command definition at a time. |
| MD5, SHA-1, SHA-256, SHA-512 | Have | |
| Compare plugin | Have | Built in ([Compare](user-guide/compare.md)); no folder compare. |
| JSON and XML plugins | Partial | Format, minify, validate, XPath built in; no JSON tree view. |
| Hex editor plugin | Partial | Read-only Hex View of the first 1 MiB. |
| Spell check plugin | Have | Windows spelling dictionaries; normal editor only ([Spell check](user-guide/spell-check.md)). |
| Export to HTML and RTF | Have | |
| MIME tools (Base64, URL) | Have | |

### Settings, platform and ecosystem

| Notepad++ feature | Bareline | Notes |
| --- | --- | --- |
| Preferences | Partial | Settings page and `settings.toml`; some rows have no effect yet ([Known issues](../README.md#known-issues-in-this-preview)). |
| Shortcut Mapper | Have | Plus a Notepad++ shortcut preset ([keymap](NOTEPADPP_KEYMAP.md)). |
| Import of Notepad++ configuration | Partial | Selected preferences, colors, fonts, a few shortcuts and session paths. |
| Plugins and Plugins Admin | Missing | By design for 1.0: no third-party code runs in the editor process. |
| User-interface translations | Missing | English only; a resource table is ready for language packs ([localization](localization.md)). |
| Automatic updates | Missing | Preview updates are manual. |
| 32-bit and ARM64 builds | Missing | Windows x64 only; ARM64 is built in CI but not released. |

## Competitor matrix

Statements about other products are limited to what their publishers document publicly, as of October 2026, and are written to be neutral. "Not assessed" means the Bareline project has not verified the point, not that the product lacks it. Product names are trademarks of their owners. Recheck this table against the publishers' current documentation before quoting it.

| Product | Platforms | License and price | Documents | Large files | Built-in two-file compare | Extensions | Restores unsaved work after a restart |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **Bareline** (preview) | Windows x64 | MPL-2.0 core; free | Tabs, two views | Paged editing above 256 MiB | Yes | None in 1.0; extension host disabled | Yes, from recovery journals |
| **Notepad++** | Windows (x86, x64, ARM64) | GPL-3.0; free | Tabs, two views | Loads the whole file into memory | Through a plugin (ComparePlus) | Native plugins, Plugins Admin | Session snapshot and periodic backup |
| **Notepad3** | Windows | Open source; free | One document per window | Not assessed | No | No | Not assessed |
| **Notepad4** | Windows | Open source; free | One document per window | Not assessed | No | No | Not assessed |
| **EmEditor** | Windows | Commercial; a free edition exists | Tabs | Publisher documents support for very large files | Yes | Plugins and macros | Not assessed |
| **klogg** | Windows, macOS, Linux | GPL-3.0; free | Log viewer; does not edit | Built for searching large log files | No | No | Not applicable |
| **Sublime Text** | Windows, macOS, Linux | Proprietary; paid license | Tabs, split layouts | Not assessed | Not assessed | Python packages | Yes ("hot exit") |
| **Visual Studio Code** | Windows, macOS, Linux | MIT-licensed source (Code - OSS); Microsoft builds are free under a Microsoft license | Tabs, editor groups | Turns some features off for large files (`editor.largeFileOptimizations`) | Yes (diff editor) | Extension marketplace | Yes ("hot exit") |
| **Zed** | Not assessed for Windows; macOS and Linux | GPL-3.0 editor source; free | Tabs, panes | Not assessed | Not assessed | WebAssembly extensions | Not assessed |
| **Windows 11 Notepad** | Windows 11 | Included with Windows; closed source | Tabs | Not assessed | No | No | Yes, restores open tabs and unsaved content |

### Where Bareline is placed

The design choices that set Bareline apart are the ones it can show in its own code and tests: continuous recovery journals, paged editing of large files, Compare and reviewable Replace in Files built in, JSON, XML and Hex tools without plugins, no telemetry, and a core written in Rust. It does not aim to match Notepad++ on plugins, the number of languages or user-interface translations in 1.0, and as a new preview it has none of the field history of the editors above. Speed and memory comparisons are made only from the qualified benchmark in [docs/perf](perf/README.md).
