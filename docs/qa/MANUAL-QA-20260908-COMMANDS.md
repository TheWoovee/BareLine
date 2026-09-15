# Static command QA catalog — 2026-09-08

Main source baseline: `51cd4c0`. **STATIC SOURCE CATALOG — NOT A RUNTIME INVENTORY.** Default QA binary has no inventory export and no existing release command matrix artifact was found by the release coordinator. This catalog reads actual registration definitions; it does not prove that every registered command is reachable, enabled, correctly localized or implemented in the running binary. Dynamic providers and non-registry gestures require additional coverage below.

Root controls the desktop. No builds, source fixes or desktop actions were performed by this catalog author. Every success/disabled/failure slot is **NOT RUN**. Actual observations belong in [campaign results](MANUAL-QA-20260908-RESULTS.md); do not promote a whole AC case from a partial observation. Use [full coverage](MANUAL-QA-20260908-COVERAGE.md) for all 27 packages and 81 cases.

## How to execute each row

- Success: create the row's required scratch state; invoke the actual menu/palette/toolbar/context/shortcut route; verify intended bytes/state, target document, focus and terminal outcome. For mutations check undo/redo and saved bytes.
- Disabled: remove its prerequisite (selection, active document, writable target, complete search, configured provider or available history); verify disabled reason and zero mutation. If the command is always enabled, record a justified not-applicable outcome only after observation.
- Failure/cancel: use a controlled scratch cancellation, stale revision, invalid input, missing fixture or unavailable capability. Verify a clear terminal result, unchanged unrelated documents and focus recovery. No personal-file faults, system-disk exhaustion, trust-store mutation or fixes.
- “—” means empty/default shortcut in the inspected definition, not proof that no physical gesture or user keymap activates it. Commands may be internal or context-only. Match current command ID; do not guess that a displayed label maps uniquely.

## Navigation by command family

| Family | Native entry / fixture | Extra coverage |
|---|---|---|
| file, app, help | File menu, private generated files, About | Save/close/cancel/recovery and IPC targeting; diagnostics privacy |
| search | Find/Replace and search panel | Literal/Extended/regex, stale preparation, folder preview/apply receipts |
| editor, edit | Edit menu, scratch multi-selection and Paged source | 10k carets, rectangles, bookmarks, measured columns, one-step undo; drag transfer is also a gesture |
| view, workspace, documents, outline | View and workspace panes | Split/clone, independent state, pin/reorder, partial map/outline |
| language, encoding | Language/Encoding menus | Full-source folds, completion, UDL validation, exact codec/EOL bytes |
| settings | Settings, mapper, toolbar | Persistence, validation, conflicts, resource policies, mixed DPI |
| macro, run, output | Macro/Run and output panel | Recorded command acknowledgement, configured macro slots, direct argv and cancellation |
| utilities, compare | Tools and compare panel | Hash/transform/export/print, ignore/cancel/merge and linked undo |
| extensions | Extension manager | Verified fixture packages, provider state, host drain, JSON/XML/Hex |
| update, migration, tray, watch | Integration/monitoring controls | Private roots; signed/clean-VM or controlled-share prerequisites remain explicit |

## Registered literal definitions and expanded fixed slots

Rows extracted from the literal tuples inside the named registration functions, with explicit CODECS macro and fixed macro-slot expansion. Composition roots inspected: `crates/app/src/lib.rs:30` and `apps/bareline/src/windows_app.rs:284–311`. This bounded extraction is not a compiler or definitive runtime enumeration. Source locations support reconciliation; omitted generated/dynamic registrations stay unresolved rather than silently passed.

| Stable command ID | Defined title | Default shortcut | Definition | Success | Disabled | Failure/cancel |
|---|---|---|---|---|---|---|
| app.quit | Exit | Alt+F4 | crates/commands/src/lib.rs:181 | NOT RUN | NOT RUN | NOT RUN |
| compare.applyColor | Apply compare color | — | apps/bareline/src/windows_app/compare.rs:55 | NOT RUN | NOT RUN | NOT RUN |
| compare.colorblind | Color-blind compare palette | — | apps/bareline/src/windows_app/compare.rs:54 | NOT RUN | NOT RUN | NOT RUN |
| compare.colorsTab | Compare colors | — | apps/bareline/src/windows_app/compare.rs:57 | NOT RUN | NOT RUN | NOT RUN |
| compare.defaults | Use compare theme defaults | — | apps/bareline/src/windows_app/compare.rs:53 | NOT RUN | NOT RUN | NOT RUN |
| compare.disk | Compare with disk file… | — | apps/bareline/src/windows_app/compare.rs:43 | NOT RUN | NOT RUN | NOT RUN |
| compare.external | Compare with current disk version | — | apps/bareline/src/windows_app/compare.rs:45 | NOT RUN | NOT RUN | NOT RUN |
| compare.generalTab | General compare options | — | apps/bareline/src/windows_app/compare.rs:56 | NOT RUN | NOT RUN | NOT RUN |
| compare.ignoreBlank | Ignore blank lines | — | apps/bareline/src/windows_app/compare.rs:38 | NOT RUN | NOT RUN | NOT RUN |
| compare.ignoreBom | Ignore encoding BOM | — | apps/bareline/src/windows_app/compare.rs:40 | NOT RUN | NOT RUN | NOT RUN |
| compare.ignoreCase | Ignore case | — | apps/bareline/src/windows_app/compare.rs:37 | NOT RUN | NOT RUN | NOT RUN |
| compare.ignoreEol | Ignore EOL style | — | apps/bareline/src/windows_app/compare.rs:39 | NOT RUN | NOT RUN | NOT RUN |
| compare.ignoreWhitespace | Ignore all whitespace | — | apps/bareline/src/windows_app/compare.rs:59 | NOT RUN | NOT RUN | NOT RUN |
| compare.lastSaved | Compare with last saved version | — | apps/bareline/src/windows_app/compare.rs:44 | NOT RUN | NOT RUN | NOT RUN |
| compare.leftSource | Change left source | — | apps/bareline/src/windows_app/compare.rs:34 | NOT RUN | NOT RUN | NOT RUN |
| compare.normalizeTabs | Normalize tabs | — | apps/bareline/src/windows_app/compare.rs:41 | NOT RUN | NOT RUN | NOT RUN |
| compare.resetAdded | Reset added colors | — | apps/bareline/src/windows_app/compare.rs:60 | NOT RUN | NOT RUN | NOT RUN |
| compare.resetChanged | Reset changed colors | — | apps/bareline/src/windows_app/compare.rs:62 | NOT RUN | NOT RUN | NOT RUN |
| compare.resetCurrent | Reset current difference colors | — | apps/bareline/src/windows_app/compare.rs:64 | NOT RUN | NOT RUN | NOT RUN |
| compare.resetMoved | Reset moved colors | — | apps/bareline/src/windows_app/compare.rs:63 | NOT RUN | NOT RUN | NOT RUN |
| compare.resetRemoved | Reset removed colors | — | apps/bareline/src/windows_app/compare.rs:61 | NOT RUN | NOT RUN | NOT RUN |
| compare.rightSource | Change right source | — | apps/bareline/src/windows_app/compare.rs:35 | NOT RUN | NOT RUN | NOT RUN |
| compare.syncHorizontal | Synchronize horizontal scrolling | — | apps/bareline/src/windows_app/compare.rs:42 | NOT RUN | NOT RUN | NOT RUN |
| compare.themeDark | Dark compare theme | — | apps/bareline/src/windows_app/compare.rs:51 | NOT RUN | NOT RUN | NOT RUN |
| compare.themeLight | Light compare theme | — | apps/bareline/src/windows_app/compare.rs:50 | NOT RUN | NOT RUN | NOT RUN |
| compare.themeSystem | System compare theme | — | apps/bareline/src/windows_app/compare.rs:52 | NOT RUN | NOT RUN | NOT RUN |
| compare.trimEdges | Ignore leading/trailing whitespace | — | apps/bareline/src/windows_app/compare.rs:58 | NOT RUN | NOT RUN | NOT RUN |
| compare.whitespace | Compare whitespace mode | — | apps/bareline/src/windows_app/compare.rs:36 | NOT RUN | NOT RUN | NOT RUN |
| edit.backspace | Delete Previous Character | — | crates/app/src/macros.rs:172 | NOT RUN | NOT RUN | NOT RUN |
| edit.copy | Copy | Ctrl+C | crates/commands/src/lib.rs:162 | NOT RUN | NOT RUN | NOT RUN |
| edit.cut | Cut | Ctrl+X | crates/commands/src/lib.rs:163 | NOT RUN | NOT RUN | NOT RUN |
| edit.delete | Delete Next Character | — | crates/app/src/macros.rs:173 | NOT RUN | NOT RUN | NOT RUN |
| edit.insert_text | Insert Recorded Text | — | crates/app/src/macros.rs:171 | NOT RUN | NOT RUN | NOT RUN |
| edit.move_down | Move Caret Down | — | crates/app/src/macros.rs:177 | NOT RUN | NOT RUN | NOT RUN |
| edit.move_end | Move Caret to Line End | — | crates/app/src/macros.rs:179 | NOT RUN | NOT RUN | NOT RUN |
| edit.move_home | Move Caret to Line Start | — | crates/app/src/macros.rs:178 | NOT RUN | NOT RUN | NOT RUN |
| edit.move_left | Move Caret Left | — | crates/app/src/macros.rs:174 | NOT RUN | NOT RUN | NOT RUN |
| edit.move_right | Move Caret Right | — | crates/app/src/macros.rs:175 | NOT RUN | NOT RUN | NOT RUN |
| edit.move_up | Move Caret Up | — | crates/app/src/macros.rs:176 | NOT RUN | NOT RUN | NOT RUN |
| edit.paste | Paste | Ctrl+V | crates/commands/src/lib.rs:164 | NOT RUN | NOT RUN | NOT RUN |
| edit.redo | Redo | Ctrl+Y | crates/commands/src/lib.rs:191 | NOT RUN | NOT RUN | NOT RUN |
| edit.select_all | Select All | Ctrl+A | crates/commands/src/lib.rs:192 | NOT RUN | NOT RUN | NOT RUN |
| edit.undo | Undo | Ctrl+Z | crates/commands/src/lib.rs:190 | NOT RUN | NOT RUN | NOT RUN |
| editor.bookmark.clear | Clear Bookmarks | — | crates/editor-surface/src/power.rs:1551 | NOT RUN | NOT RUN | NOT RUN |
| editor.bookmark.copyLines | Copy Bookmarked Lines | — | apps/bareline/src/windows_app/power.rs:71 | NOT RUN | NOT RUN | NOT RUN |
| editor.bookmark.cutLines | Cut Bookmarked Lines | — | apps/bareline/src/windows_app/power.rs:71 | NOT RUN | NOT RUN | NOT RUN |
| editor.bookmark.deleteLines | Delete Bookmarked Lines | — | apps/bareline/src/windows_app/power.rs:71 | NOT RUN | NOT RUN | NOT RUN |
| editor.bookmark.next | Next Bookmark | F2 | crates/editor-surface/src/power.rs:1549 | NOT RUN | NOT RUN | NOT RUN |
| editor.bookmark.previous | Previous Bookmark | Shift+F2 | crates/editor-surface/src/power.rs:1550 | NOT RUN | NOT RUN | NOT RUN |
| editor.bookmark.selectLines | Select Bookmarked Lines | — | crates/editor-surface/src/power.rs:1552 | NOT RUN | NOT RUN | NOT RUN |
| editor.bookmark.toggle | Toggle Bookmark | Ctrl+F2 | crates/editor-surface/src/power.rs:1548 | NOT RUN | NOT RUN | NOT RUN |
| editor.caret.above | Add Caret Above | Ctrl+Alt+Up | crates/editor-surface/src/power.rs:1503 | NOT RUN | NOT RUN | NOT RUN |
| editor.caret.below | Add Caret Below | Ctrl+Alt+Down | crates/editor-surface/src/power.rs:1504 | NOT RUN | NOT RUN | NOT RUN |
| editor.case.invert | Invert Case | — | crates/editor-surface/src/power.rs:1556 | NOT RUN | NOT RUN | NOT RUN |
| editor.case.lower | Lowercase | — | crates/editor-surface/src/power.rs:1554 | NOT RUN | NOT RUN | NOT RUN |
| editor.case.title | Title Case | — | crates/editor-surface/src/power.rs:1555 | NOT RUN | NOT RUN | NOT RUN |
| editor.case.upper | Uppercase | — | crates/editor-surface/src/power.rs:1553 | NOT RUN | NOT RUN | NOT RUN |
| editor.clipboard.toggleHistory | Toggle Clipboard History | — | crates/editor-surface/src/power.rs:1537 | NOT RUN | NOT RUN | NOT RUN |
| editor.column.insert | Column Editor… | Alt+C | crates/editor-surface/src/power.rs:1532 | NOT RUN | NOT RUN | NOT RUN |
| editor.comment.toggleBlock | Toggle Block Comment | Ctrl+Shift+/ | crates/editor-surface/src/power.rs:1543 | NOT RUN | NOT RUN | NOT RUN |
| editor.comment.toggleLine | Toggle Line Comment | Ctrl+/ | crates/editor-surface/src/power.rs:1542 | NOT RUN | NOT RUN | NOT RUN |
| editor.completion.show | Show Completion | Ctrl+Space | crates/app/src/language.rs:845 | NOT RUN | NOT RUN | NOT RUN |
| editor.indent | Indent | Tab | crates/editor-surface/src/power.rs:1560 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.duplicate | Duplicate Lines | Ctrl+Shift+D | crates/editor-surface/src/power.rs:1564 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.hide | Hide Selected Lines | — | crates/editor-surface/src/power.rs:1533 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.join | Join Lines | Ctrl+J | crates/editor-surface/src/power.rs:1568 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.moveDown | Move Lines Down | Alt+Down | crates/editor-surface/src/power.rs:1567 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.moveUp | Move Lines Up | Alt+Up | crates/editor-surface/src/power.rs:1566 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.removeBlank | Remove Blank Lines | — | crates/editor-surface/src/power.rs:1589 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.removeConsecutiveDuplicates | Remove Consecutive Duplicate Lines | — | crates/editor-surface/src/power.rs:1583 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.removeDuplicates | Remove Duplicate Lines | — | crates/editor-surface/src/power.rs:1578 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.removeEmpty | Remove Empty Lines | — | crates/editor-surface/src/power.rs:1588 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.showAll | Show Hidden Lines | — | crates/editor-surface/src/power.rs:1534 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.sortAscending | Sort Lines Ascending | — | crates/editor-surface/src/power.rs:1570 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.sortDescending | Sort Lines Descending | — | crates/editor-surface/src/power.rs:1571 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.sortIgnoreCase | Sort Lines Ignoring Case | — | crates/editor-surface/src/power.rs:1573 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.sortNumeric | Sort Lines Numerically | — | crates/editor-surface/src/power.rs:1572 | NOT RUN | NOT RUN | NOT RUN |
| editor.lines.split | Split Lines at 80 Columns | — | crates/editor-surface/src/power.rs:1569 | NOT RUN | NOT RUN | NOT RUN |
| editor.paste.fromHistory | Paste from History… | — | crates/editor-surface/src/power.rs:1536 | NOT RUN | NOT RUN | NOT RUN |
| editor.paste.plainText | Paste Plain Text | Ctrl+Shift+V | crates/editor-surface/src/power.rs:1535 | NOT RUN | NOT RUN | NOT RUN |
| editor.rectangle.delete | Delete Rectangle | — | apps/bareline/src/windows_app/power.rs:71 | NOT RUN | NOT RUN | NOT RUN |
| editor.rectangle.paste | Paste into Rectangle | — | apps/bareline/src/windows_app/power.rs:71 | NOT RUN | NOT RUN | NOT RUN |
| editor.selection.allOccurrences | Select All Occurrences | Ctrl+Shift+L | crates/editor-surface/src/power.rs:1510 | NOT RUN | NOT RUN | NOT RUN |
| editor.selection.duplicate | Duplicate Selections | — | crates/editor-surface/src/power.rs:1565 | NOT RUN | NOT RUN | NOT RUN |
| editor.selection.escape | Keep Primary Selection | Escape | crates/editor-surface/src/power.rs:1527 | NOT RUN | NOT RUN | NOT RUN |
| editor.selection.expandLines | Expand to Lines | — | crates/editor-surface/src/power.rs:1526 | NOT RUN | NOT RUN | NOT RUN |
| editor.selection.nextOccurrence | Select Next Occurrence | Ctrl+D | crates/editor-surface/src/power.rs:1505 | NOT RUN | NOT RUN | NOT RUN |
| editor.selection.rotatePrimary | Rotate Primary Selection | — | crates/editor-surface/src/power.rs:1521 | NOT RUN | NOT RUN | NOT RUN |
| editor.selection.skipOccurrence | Skip Occurrence | — | crates/editor-surface/src/power.rs:1515 | NOT RUN | NOT RUN | NOT RUN |
| editor.selection.undoOccurrence | Undo Added Occurrence | — | crates/editor-surface/src/power.rs:1516 | NOT RUN | NOT RUN | NOT RUN |
| editor.spaces.toTabs | Convert Spaces to Tabs | — | crates/editor-surface/src/power.rs:1563 | NOT RUN | NOT RUN | NOT RUN |
| editor.tabs.toSpaces | Convert Tabs to Spaces | — | crates/editor-surface/src/power.rs:1562 | NOT RUN | NOT RUN | NOT RUN |
| editor.unindent | Unindent | Shift+Tab | crates/editor-surface/src/power.rs:1561 | NOT RUN | NOT RUN | NOT RUN |
| editor.whitespace.trim | Trim Whitespace | — | crates/editor-surface/src/power.rs:1559 | NOT RUN | NOT RUN | NOT RUN |
| editor.whitespace.trimEnd | Trim Trailing Whitespace | — | crates/editor-surface/src/power.rs:1558 | NOT RUN | NOT RUN | NOT RUN |
| editor.whitespace.trimStart | Trim Leading Whitespace | — | crates/editor-surface/src/power.rs:1557 | NOT RUN | NOT RUN | NOT RUN |
| encoding.binary | Binary Warning Options… | — | crates/app/src/encoding.rs:49 | NOT RUN | NOT RUN | NOT RUN |
| encoding.binary.edit | Allow Text Editing | — | crates/app/src/encoding.rs:51 | NOT RUN | NOT RUN | NOT RUN |
| encoding.binary.info | Binary-like bytes detected; choose how to open text | — | crates/app/src/encoding.rs:50 | NOT RUN | NOT RUN | NOT RUN |
| encoding.binary.readonly | Keep Read-only Text | — | crates/app/src/encoding.rs:51 | NOT RUN | NOT RUN | NOT RUN |
| encoding.bom_off | Omit Byte Order Mark | — | crates/app/src/encoding.rs:43 | NOT RUN | NOT RUN | NOT RUN |
| encoding.bom_on | Write Byte Order Mark | — | crates/app/src/encoding.rs:43 | NOT RUN | NOT RUN | NOT RUN |
| encoding.choose | Encoding… | — | crates/app/src/encoding.rs:39 | NOT RUN | NOT RUN | NOT RUN |
| encoding.choose_convert | Convert Save Encoding To… | — | crates/app/src/encoding.rs:42 | NOT RUN | NOT RUN | NOT RUN |
| encoding.choose_interpret | Interpret Original Bytes As… | — | crates/app/src/encoding.rs:41 | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.big5 | Big5 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.eucjp | EUC-JP | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.euckr | EUC-KR | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.gbk | GBK | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.latin1 | ISO-8859-1 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.shiftjis | Shift-JIS | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.utf16be | UTF-16 BE | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.utf16le | UTF-16 LE | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.utf32be | UTF-32 BE | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.utf32le | UTF-32 LE | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.utf8 | UTF-8 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.windows1250 | Windows-1250 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.windows1251 | Windows-1251 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.windows1252 | Windows-1252 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.windows1253 | Windows-1253 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.windows1254 | Windows-1254 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.windows1255 | Windows-1255 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.windows1256 | Windows-1256 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.windows1257 | Windows-1257 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.convert.windows1258 | Windows-1258 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.eol | Line Endings… | — | crates/app/src/encoding.rs:44 | NOT RUN | NOT RUN | NOT RUN |
| encoding.eol.cr | Convert Document to CR | — | crates/app/src/encoding.rs:45 | NOT RUN | NOT RUN | NOT RUN |
| encoding.eol.crlf | Convert Document to CRLF | — | crates/app/src/encoding.rs:45 | NOT RUN | NOT RUN | NOT RUN |
| encoding.eol.lf | Convert Document to LF | — | crates/app/src/encoding.rs:44 | NOT RUN | NOT RUN | NOT RUN |
| encoding.eol.selection_cr | Convert Selection to CR | — | crates/app/src/encoding.rs:48 | NOT RUN | NOT RUN | NOT RUN |
| encoding.eol.selection_crlf | Convert Selection to CRLF | — | crates/app/src/encoding.rs:47 | NOT RUN | NOT RUN | NOT RUN |
| encoding.eol.selection_lf | Convert Selection to LF | — | crates/app/src/encoding.rs:46 | NOT RUN | NOT RUN | NOT RUN |
| encoding.failure | Show Encoding Save Failure | — | crates/app/src/encoding.rs:40 | NOT RUN | NOT RUN | NOT RUN |
| encoding.info | Encoding Details | — | crates/app/src/encoding.rs:39 | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.big5 | Big5 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.eucjp | EUC-JP | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.euckr | EUC-KR | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.gbk | GBK | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.latin1 | ISO-8859-1 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.shiftjis | Shift-JIS | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.utf16be | UTF-16 BE | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.utf16le | UTF-16 LE | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.utf32be | UTF-32 BE | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.utf32le | UTF-32 LE | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.utf8 | UTF-8 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.windows1250 | Windows-1250 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.windows1251 | Windows-1251 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.windows1252 | Windows-1252 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.windows1253 | Windows-1253 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.windows1254 | Windows-1254 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.windows1255 | Windows-1255 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.windows1256 | Windows-1256 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.windows1257 | Windows-1257 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| encoding.interpret.windows1258 | Windows-1258 | — | crates/app/src/encoding.rs:21 (CODECS macro expansion) | NOT RUN | NOT RUN | NOT RUN |
| ext.hex.open | Hex: Original Bytes | — | apps/bareline/src/windows_app/extensions.rs:313 | NOT RUN | NOT RUN | NOT RUN |
| ext.json.format | JSON: Format | — | apps/bareline/src/windows_app/extensions.rs:307 | NOT RUN | NOT RUN | NOT RUN |
| ext.json.minify | JSON: Minify | — | apps/bareline/src/windows_app/extensions.rs:308 | NOT RUN | NOT RUN | NOT RUN |
| ext.json.tree | JSON: Tree | — | apps/bareline/src/windows_app/extensions.rs:309 | NOT RUN | NOT RUN | NOT RUN |
| ext.json.validate | JSON: Validate | — | apps/bareline/src/windows_app/extensions.rs:306 | NOT RUN | NOT RUN | NOT RUN |
| ext.xml.format | XML: Format | — | apps/bareline/src/windows_app/extensions.rs:311 | NOT RUN | NOT RUN | NOT RUN |
| ext.xml.validate | XML: Validate | — | apps/bareline/src/windows_app/extensions.rs:310 | NOT RUN | NOT RUN | NOT RUN |
| ext.xml.xpath | XML: XPath | — | apps/bareline/src/windows_app/extensions.rs:312 | NOT RUN | NOT RUN | NOT RUN |
| extensions.approve | Approve Reviewed Extension Permissions | — | apps/bareline/src/windows_app/extensions.rs:299 | NOT RUN | NOT RUN | NOT RUN |
| extensions.cancel | Cancel Extension | — | apps/bareline/src/windows_app/extensions.rs:278 | NOT RUN | NOT RUN | NOT RUN |
| extensions.catalog | Open Signed Offline Extension Catalog | — | apps/bareline/src/windows_app/extensions.rs:280 | NOT RUN | NOT RUN | NOT RUN |
| extensions.close | Close Extensions | — | apps/bareline/src/windows_app/extensions.rs:279 | NOT RUN | NOT RUN | NOT RUN |
| extensions.disable | Disable Selected Extension | — | apps/bareline/src/windows_app/extensions.rs:303 | NOT RUN | NOT RUN | NOT RUN |
| extensions.install | Install Selected Package | — | apps/bareline/src/windows_app/extensions.rs:288 | NOT RUN | NOT RUN | NOT RUN |
| extensions.manage | Manage Extensions | — | apps/bareline/src/windows_app/extensions.rs:277 | NOT RUN | NOT RUN | NOT RUN |
| extensions.next_command | Select Next Extension Command | — | apps/bareline/src/windows_app/extensions.rs:294 | NOT RUN | NOT RUN | NOT RUN |
| extensions.permissions | Review Selected Extension Permissions | — | apps/bareline/src/windows_app/extensions.rs:295 | NOT RUN | NOT RUN | NOT RUN |
| extensions.remove | Uninstall Selected Extension | — | apps/bareline/src/windows_app/extensions.rs:304 | NOT RUN | NOT RUN | NOT RUN |
| extensions.remove_runtime | Remove Runtime | — | apps/bareline/src/windows_app/extensions.rs:305 | NOT RUN | NOT RUN | NOT RUN |
| extensions.run_background | Run Declared Background Command (120 seconds) | — | apps/bareline/src/windows_app/extensions.rs:290 | NOT RUN | NOT RUN | NOT RUN |
| extensions.run_selected | Run Selected Extension Command | — | apps/bareline/src/windows_app/extensions.rs:289 | NOT RUN | NOT RUN | NOT RUN |
| extensions.runtime_catalog | Install Signed Offline Runtime | — | apps/bareline/src/windows_app/extensions.rs:284 | NOT RUN | NOT RUN | NOT RUN |
| file.cancel_operations | Cancel File Operations | — | crates/commands/src/lib.rs:167 | NOT RUN | NOT RUN | NOT RUN |
| file.cancel_save_all | Cancel Save All | — | apps/bareline/src/windows_app/lifecycle.rs:45 | NOT RUN | NOT RUN | NOT RUN |
| file.close | Close | Ctrl+W | crates/commands/src/lib.rs:104 | NOT RUN | NOT RUN | NOT RUN |
| file.external.auto_reload | Automatically Reload Clean Local Files (This Session) | — | apps/bareline/src/windows_app/watch.rs:60 | NOT RUN | NOT RUN | NOT RUN |
| file.external.check | Check for External Changes | — | apps/bareline/src/windows_app/watch.rs:61 | NOT RUN | NOT RUN | NOT RUN |
| file.external.keep | Keep Current Buffer | — | apps/bareline/src/windows_app/watch.rs:62 | NOT RUN | NOT RUN | NOT RUN |
| file.external.reload | Reload External Changes | — | apps/bareline/src/windows_app/watch.rs:63 | NOT RUN | NOT RUN | NOT RUN |
| file.monitor.pause | Pause Following Scroll | — | apps/bareline/src/windows_app/watch.rs:65 | NOT RUN | NOT RUN | NOT RUN |
| file.monitor.reopen | Reopen and Follow | — | apps/bareline/src/windows_app/watch.rs:67 | NOT RUN | NOT RUN | NOT RUN |
| file.monitor.resume | Resume Following | — | apps/bareline/src/windows_app/watch.rs:66 | NOT RUN | NOT RUN | NOT RUN |
| file.monitor.start | Follow New Content | — | apps/bareline/src/windows_app/watch.rs:64 | NOT RUN | NOT RUN | NOT RUN |
| file.monitor.unlock | Unlock to Edit (Stop Monitoring) | — | apps/bareline/src/windows_app/watch.rs:68 | NOT RUN | NOT RUN | NOT RUN |
| file.new | New | Ctrl+N | crates/commands/src/lib.rs:96 | NOT RUN | NOT RUN | NOT RUN |
| file.open | Open… | Ctrl+O | crates/commands/src/lib.rs:165 | NOT RUN | NOT RUN | NOT RUN |
| file.read_only | Set Read-Only | — | apps/bareline/src/windows_app/lifecycle.rs:44 | NOT RUN | NOT RUN | NOT RUN |
| file.remote.follow | Follow Remote File with Permission… | — | apps/bareline/src/windows_app/watch.rs:59 | NOT RUN | NOT RUN | NOT RUN |
| file.remote.open | Open Remote File with Permission… | — | apps/bareline/src/windows_app/watch.rs:57 | NOT RUN | NOT RUN | NOT RUN |
| file.remote.reload | Reload Remote File with Permission… | — | apps/bareline/src/windows_app/watch.rs:58 | NOT RUN | NOT RUN | NOT RUN |
| file.restore_closed | Restore Last Closed Tab | Ctrl+Shift+T | apps/bareline/src/windows_app/lifecycle.rs:39 | NOT RUN | NOT RUN | NOT RUN |
| file.reveal | Open Containing Folder | — | apps/bareline/src/windows_app/shell_integration.rs:4 | NOT RUN | NOT RUN | NOT RUN |
| file.save | Save | Ctrl+S | crates/commands/src/lib.rs:166 | NOT RUN | NOT RUN | NOT RUN |
| file.save_all | Save All | — | apps/bareline/src/windows_app/lifecycle.rs:38 | NOT RUN | NOT RUN | NOT RUN |
| file.save_as | Save As… | Ctrl+Shift+S | crates/commands/src/lib.rs:174 | NOT RUN | NOT RUN | NOT RUN |
| file.save_copy | Save Copy… | — | apps/bareline/src/windows_app/lifecycle.rs:37 | NOT RUN | NOT RUN | NOT RUN |
| file.terminal | Open Terminal Here | — | apps/bareline/src/windows_app/shell_integration.rs:4 | NOT RUN | NOT RUN | NOT RUN |
| help.about | About Bareline | F1 | crates/commands/src/lib.rs:189 | NOT RUN | NOT RUN | NOT RUN |
| language.choose | Choose Language… | — | crates/app/src/language.rs:840 | NOT RUN | NOT RUN | NOT RUN |
| language.signatures.import | Import Static Signatures… | — | crates/app/src/language.rs:846 | NOT RUN | NOT RUN | NOT RUN |
| language.udl.edit | Edit Imported Language Definition | — | crates/app/src/language.rs:842 | NOT RUN | NOT RUN | NOT RUN |
| language.udl.export | Export User-defined Language… | — | crates/app/src/language.rs:843 | NOT RUN | NOT RUN | NOT RUN |
| language.udl.import | Import User-defined Language… | — | crates/app/src/language.rs:841 | NOT RUN | NOT RUN | NOT RUN |
| language.udl.preview | Preview User-defined Language | — | crates/app/src/language.rs:844 | NOT RUN | NOT RUN | NOT RUN |
| macro.cancel | Cancel Macro Playback | Macro | crates/app/src/macros.rs:205 | NOT RUN | NOT RUN | NOT RUN |
| macro.export | Export Selected Macro… | Macro | crates/app/src/macros.rs:207 | NOT RUN | NOT RUN | NOT RUN |
| macro.ghost | Set Macro Typing Delay | Macro | crates/app/src/macros.rs:214 | NOT RUN | NOT RUN | NOT RUN |
| macro.import | Import Macro… | Macro | crates/app/src/macros.rs:206 | NOT RUN | NOT RUN | NOT RUN |
| macro.manager | Manage Macros… | Macro | crates/app/src/macros.rs:209 | NOT RUN | NOT RUN | NOT RUN |
| macro.manager_close | Close Macro Manager | Macro | crates/app/src/macros.rs:217 | NOT RUN | NOT RUN | NOT RUN |
| macro.play | Play Selected Macro | Macro | crates/app/src/macros.rs:204 | NOT RUN | NOT RUN | NOT RUN |
| macro.play_eof | Play Macro Until End of File | Macro | crates/app/src/macros.rs:208 | NOT RUN | NOT RUN | NOT RUN |
| macro.play_n | Play Macro N Times | Macro | crates/app/src/macros.rs:211 | NOT RUN | NOT RUN | NOT RUN |
| macro.record | Start Macro Recording | Macro | crates/app/src/macros.rs:202 | NOT RUN | NOT RUN | NOT RUN |
| macro.reload | Retry Loading Macros | Macro | crates/app/src/macros.rs:216 | NOT RUN | NOT RUN | NOT RUN |
| macro.rename | Rename Selected Macro | Macro | crates/app/src/macros.rs:210 | NOT RUN | NOT RUN | NOT RUN |
| macro.resume | Resume Failed Macro | Macro | crates/app/src/macros.rs:212 | NOT RUN | NOT RUN | NOT RUN |
| macro.save | Save Macros | Macro | crates/app/src/macros.rs:215 | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.01 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.02 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.03 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.04 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.05 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.06 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.07 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.08 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.09 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.10 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.11 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.12 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.13 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.14 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.15 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.16 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.17 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.18 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.19 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.20 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.21 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.22 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.23 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.24 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.25 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.26 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.27 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.28 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.29 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.30 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.31 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.saved.32 | Saved Macro (internal slot; configured name dynamic) | — | crates/app/src/macros.rs:38 / register_commands | NOT RUN | NOT RUN | NOT RUN |
| macro.shortcut | Assign Macro Shortcut | Macro | crates/app/src/macros.rs:213 | NOT RUN | NOT RUN | NOT RUN |
| macro.stop | Stop Macro Recording | Macro | crates/app/src/macros.rs:203 | NOT RUN | NOT RUN | NOT RUN |
| migration.apply | Apply Reviewed Notepad++ Import | — | apps/bareline/src/windows_app/migration.rs:7 | NOT RUN | NOT RUN | NOT RUN |
| migration.cancel | Cancel Import Work | — | apps/bareline/src/windows_app/migration.rs:7 | NOT RUN | NOT RUN | NOT RUN |
| migration.open_paths | Open Imported Local Files | — | apps/bareline/src/windows_app/migration.rs:7 | NOT RUN | NOT RUN | NOT RUN |
| migration.review | Review Notepad++ Import… | — | apps/bareline/src/windows_app/migration.rs:7 | NOT RUN | NOT RUN | NOT RUN |
| output.clear | Clear Output | Tools | crates/app/src/macros.rs:221 | NOT RUN | NOT RUN | NOT RUN |
| output.close | Close Output | Tools | crates/app/src/macros.rs:224 | NOT RUN | NOT RUN | NOT RUN |
| output.copy | Copy Output | Tools | crates/app/src/macros.rs:222 | NOT RUN | NOT RUN | NOT RUN |
| output.open_link | Open Selected Output Location | Tools | crates/app/src/macros.rs:225 | NOT RUN | NOT RUN | NOT RUN |
| output.save | Save Output… | Tools | crates/app/src/macros.rs:223 | NOT RUN | NOT RUN | NOT RUN |
| run.cancel | Cancel External Command | Run | crates/app/src/macros.rs:220 | NOT RUN | NOT RUN | NOT RUN |
| run.execute | Run Loaded External Command | Run | crates/app/src/macros.rs:219 | NOT RUN | NOT RUN | NOT RUN |
| run.load | Load External Command Definition… | Run | crates/app/src/macros.rs:218 | NOT RUN | NOT RUN | NOT RUN |
| search.cancel | Cancel Search / Replacement | — | crates/commands/src/lib.rs:89 | NOT RUN | NOT RUN | NOT RUN |
| search.cancel_panel | Cancel Open Document Search | — | crates/app/src/search_panel.rs:34 | NOT RUN | NOT RUN | NOT RUN |
| search.close_find | Close Find | — | crates/commands/src/lib.rs:148 | NOT RUN | NOT RUN | NOT RUN |
| search.close_panel | Close Search Results | — | crates/app/src/search_panel.rs:35 | NOT RUN | NOT RUN | NOT RUN |
| search.find | Find… | Ctrl+F | crates/commands/src/lib.rs:105 | NOT RUN | NOT RUN | NOT RUN |
| search.find_next | Find Next | F3 | crates/commands/src/lib.rs:134 | NOT RUN | NOT RUN | NOT RUN |
| search.find_previous | Find Previous | Shift+F3 | crates/commands/src/lib.rs:141 | NOT RUN | NOT RUN | NOT RUN |
| search.folder | Find in Folder… | — | apps/bareline/src/windows_app/search.rs:21 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.clearAll | Clear All Marks | — | apps/bareline/src/windows_app/search.rs:34 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.clearStyle1 | Clear Mark Style 1 | — | apps/bareline/src/windows_app/search.rs:29 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.clearStyle2 | Clear Mark Style 2 | — | apps/bareline/src/windows_app/search.rs:30 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.clearStyle3 | Clear Mark Style 3 | — | apps/bareline/src/windows_app/search.rs:31 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.clearStyle4 | Clear Mark Style 4 | — | apps/bareline/src/windows_app/search.rs:32 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.clearStyle5 | Clear Mark Style 5 | — | apps/bareline/src/windows_app/search.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.style1 | Mark All — Style 1 | — | apps/bareline/src/windows_app/search.rs:24 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.style2 | Mark All — Style 2 | — | apps/bareline/src/windows_app/search.rs:25 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.style3 | Mark All — Style 3 | — | apps/bareline/src/windows_app/search.rs:26 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.style4 | Mark All — Style 4 | — | apps/bareline/src/windows_app/search.rs:27 | NOT RUN | NOT RUN | NOT RUN |
| search.mark.style5 | Mark All — Style 5 | — | apps/bareline/src/windows_app/search.rs:28 | NOT RUN | NOT RUN | NOT RUN |
| search.match_case | Match Case | — | crates/commands/src/lib.rs:155 | NOT RUN | NOT RUN | NOT RUN |
| search.mode | Toggle Literal / Extended | — | crates/commands/src/lib.rs:127 | NOT RUN | NOT RUN | NOT RUN |
| search.open_documents | Find in Open Documents | Ctrl+Shift+F | crates/app/src/search_panel.rs:29 | NOT RUN | NOT RUN | NOT RUN |
| search.replace | Replace… | Ctrl+H | crates/commands/src/lib.rs:106 | NOT RUN | NOT RUN | NOT RUN |
| search.replace_all | Replace All in Current Document | — | crates/commands/src/lib.rs:120 | NOT RUN | NOT RUN | NOT RUN |
| search.replace_one | Replace Selected Match | — | crates/commands/src/lib.rs:113 | NOT RUN | NOT RUN | NOT RUN |
| search.replaceInFiles | Replace in Files… | — | apps/bareline/src/windows_app/search/replace.rs:299 | NOT RUN | NOT RUN | NOT RUN |
| search.replaceInWorkspace | Replace in Workspace… | — | apps/bareline/src/windows_app/search/replace.rs:300 | NOT RUN | NOT RUN | NOT RUN |
| search.replacePreview.apply | Apply Reviewed Replacements | — | apps/bareline/src/windows_app/search/replace.rs:301 | NOT RUN | NOT RUN | NOT RUN |
| search.replacePreview.backups | Toggle Backups for This Replacement Job | — | apps/bareline/src/windows_app/search/replace.rs:291 | NOT RUN | NOT RUN | NOT RUN |
| search.replacePreview.cancel | Cancel Workspace Replacement | — | apps/bareline/src/windows_app/search/replace.rs:306 | NOT RUN | NOT RUN | NOT RUN |
| search.replacePreview.close | Close Replacement Preview | — | apps/bareline/src/windows_app/search/replace.rs:310 | NOT RUN | NOT RUN | NOT RUN |
| search.replacePreview.includeBinary | Toggle Binary Replacement Inclusion | — | apps/bareline/src/windows_app/search/replace.rs:287 | NOT RUN | NOT RUN | NOT RUN |
| search.replacePreview.preserveCase | Toggle Preserve Replacement Case | — | apps/bareline/src/windows_app/search/replace.rs:283 | NOT RUN | NOT RUN | NOT RUN |
| search.replacePreview.refresh | Refresh Replacement Preview | — | apps/bareline/src/windows_app/search/replace.rs:295 | NOT RUN | NOT RUN | NOT RUN |
| search.replacePreview.rollback | Restore Last Replacement Backups | — | apps/bareline/src/windows_app/search/replace.rs:311 | NOT RUN | NOT RUN | NOT RUN |
| search.replacePreview.toggleAll | Toggle All Preview Changes | — | apps/bareline/src/windows_app/search/replace.rs:302 | NOT RUN | NOT RUN | NOT RUN |
| search.scope.current | Find in Current Document | — | apps/bareline/src/windows_app/search.rs:23 | NOT RUN | NOT RUN | NOT RUN |
| search.scope.selection | Find in Selection | — | apps/bareline/src/windows_app/search.rs:22 | NOT RUN | NOT RUN | NOT RUN |
| search.whole_word | Whole Word | — | crates/commands/src/lib.rs:97 | NOT RUN | NOT RUN | NOT RUN |
| settings.change | Change Setting | — | crates/settings/src/lib.rs:98 | NOT RUN | NOT RUN | NOT RUN |
| settings.close | Close Settings | — | crates/settings/src/lib.rs:93 | NOT RUN | NOT RUN | NOT RUN |
| settings.copy_key | Copy Setting TOML Key | — | crates/settings/src/lib.rs:97 | NOT RUN | NOT RUN | NOT RUN |
| settings.keymap_export | Export Keymap | — | apps/bareline/src/windows_app/shortcuts.rs:23 | NOT RUN | NOT RUN | NOT RUN |
| settings.keymap_import | Import Keymap | — | apps/bareline/src/windows_app/shortcuts.rs:22 | NOT RUN | NOT RUN | NOT RUN |
| settings.keymap_open | Open Keymap File | — | apps/bareline/src/windows_app/shortcuts.rs:24 | NOT RUN | NOT RUN | NOT RUN |
| settings.open | Settings | — | crates/settings/src/lib.rs:92 | NOT RUN | NOT RUN | NOT RUN |
| settings.reset_section | Reset Settings Section | — | crates/settings/src/lib.rs:96 | NOT RUN | NOT RUN | NOT RUN |
| settings.retry | Retry Settings Save | — | crates/settings/src/lib.rs:94 | NOT RUN | NOT RUN | NOT RUN |
| settings.revert | Revert Settings Changes | — | crates/settings/src/lib.rs:95 | NOT RUN | NOT RUN | NOT RUN |
| settings.shortcut_apply | Apply Shortcut | — | apps/bareline/src/windows_app/shortcuts.rs:37 | NOT RUN | NOT RUN | NOT RUN |
| settings.shortcut_close | Close Shortcut Mapper | — | apps/bareline/src/windows_app/shortcuts.rs:38 | NOT RUN | NOT RUN | NOT RUN |
| settings.shortcuts | Shortcut Mapper | — | apps/bareline/src/windows_app/shortcuts.rs:21 | NOT RUN | NOT RUN | NOT RUN |
| tray.hide | Minimize to Tray | — | apps/bareline/src/windows_app/shell_integration.rs:4 | NOT RUN | NOT RUN | NOT RUN |
| tray.restore | Restore Window | — | apps/bareline/src/windows_app/shell_integration.rs:4 | NOT RUN | NOT RUN | NOT RUN |
| tray.toggle | Keep Running in Tray | — | apps/bareline/src/windows_app/shell_integration.rs:4 | NOT RUN | NOT RUN | NOT RUN |
| update.apply_on_exit | Apply Update on Exit | — | apps/bareline/src/windows_app/update.rs:242 | NOT RUN | NOT RUN | NOT RUN |
| update.cancel | Cancel Update | — | apps/bareline/src/windows_app/update.rs:243 | NOT RUN | NOT RUN | NOT RUN |
| update.check | Check for Updates | — | apps/bareline/src/windows_app/update.rs:241 | NOT RUN | NOT RUN | NOT RUN |
| update.discard | Discard Pending Update | — | apps/bareline/src/windows_app/update.rs:244 | NOT RUN | NOT RUN | NOT RUN |
| utilities.base64Decode | Base64 decode | — | crates/app/src/utilities.rs:663 | NOT RUN | NOT RUN | NOT RUN |
| utilities.base64Encode | Base64 encode | — | crates/app/src/utilities.rs:662 | NOT RUN | NOT RUN | NOT RUN |
| utilities.cancel | Cancel utility operation | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.copyResult | Copy utility result | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.dismiss | Close utility dialog | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.exportHtml | Export syntax-colored HTML | — | crates/app/src/utilities.rs:667 | NOT RUN | NOT RUN | NOT RUN |
| utilities.exportRtf | Export syntax-colored RTF | — | crates/app/src/utilities.rs:668 | NOT RUN | NOT RUN | NOT RUN |
| utilities.md5 | MD5 (legacy integrity hash) | — | crates/app/src/utilities.rs:658 | NOT RUN | NOT RUN | NOT RUN |
| utilities.print | Print document… | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.printFont | Print font family | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.printFooter | Print footer | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.printHeader | Print header | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.printMargins | Print margins | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.printNow | Choose printer and print… | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.printNumbers | Print line numbers | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.printRange | Print selection only | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.printSelection | Print selection… | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.printSize | Print font size | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.printSyntax | Print syntax colors | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.retry | Retry utility | — | apps/bareline/src/windows_app/utilities.rs:33 | NOT RUN | NOT RUN | NOT RUN |
| utilities.sha1 | SHA-1 (legacy integrity hash) | — | crates/app/src/utilities.rs:659 | NOT RUN | NOT RUN | NOT RUN |
| utilities.sha256 | SHA-256 | — | crates/app/src/utilities.rs:660 | NOT RUN | NOT RUN | NOT RUN |
| utilities.sha512 | SHA-512 | — | crates/app/src/utilities.rs:661 | NOT RUN | NOT RUN | NOT RUN |
| utilities.statistics | Document statistics | — | crates/app/src/utilities.rs:666 | NOT RUN | NOT RUN | NOT RUN |
| utilities.urlDecode | URL decode | — | crates/app/src/utilities.rs:665 | NOT RUN | NOT RUN | NOT RUN |
| utilities.urlEncode | URL encode | — | crates/app/src/utilities.rs:664 | NOT RUN | NOT RUN | NOT RUN |
| view.clone_other | Clone to Other View | — | apps/bareline/src/windows_app/views.rs:664 | NOT RUN | NOT RUN | NOT RUN |
| view.close_split | Close Split View | — | apps/bareline/src/windows_app/views.rs:666 | NOT RUN | NOT RUN | NOT RUN |
| view.command_palette | Command Palette | Ctrl+Shift+P | crates/commands/src/lib.rs:182 | NOT RUN | NOT RUN | NOT RUN |
| view.focus_other | Focus Other View | F6 | apps/bareline/src/windows_app/views.rs:667 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.all | Fold All Known Regions | — | crates/app/src/language.rs:851 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.level1 | Fold Level 1 | — | crates/app/src/language.rs:854 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.level2 | Fold Level 2 | — | crates/app/src/language.rs:855 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.level3 | Fold Level 3 | — | crates/app/src/language.rs:856 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.level4 | Fold Level 4 | — | crates/app/src/language.rs:857 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.level5 | Fold Level 5 | — | crates/app/src/language.rs:858 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.level6 | Fold Level 6 | — | crates/app/src/language.rs:859 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.level7 | Fold Level 7 | — | crates/app/src/language.rs:860 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.level8 | Fold Level 8 | — | crates/app/src/language.rs:861 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.toggleCurrent | Toggle Current Fold | — | crates/app/src/language.rs:853 | NOT RUN | NOT RUN | NOT RUN |
| view.fold.unfoldAll | Unfold All | — | crates/app/src/language.rs:852 | NOT RUN | NOT RUN | NOT RUN |
| view.move_other | Move to Other View | — | apps/bareline/src/windows_app/views.rs:665 | NOT RUN | NOT RUN | NOT RUN |
| view.split_horizontal | Split Horizontally | — | apps/bareline/src/windows_app/views.rs:663 | NOT RUN | NOT RUN | NOT RUN |
| view.split_vertical | Split Vertically | — | apps/bareline/src/windows_app/views.rs:662 | NOT RUN | NOT RUN | NOT RUN |
| view.sync_horizontal | Synchronize Horizontal Scrolling | — | apps/bareline/src/windows_app/views.rs:669 | NOT RUN | NOT RUN | NOT RUN |
| view.sync_vertical | Synchronize Vertical Scrolling | — | apps/bareline/src/windows_app/views.rs:668 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.color | Cycle Tab Color | — | apps/bareline/src/windows_app/views.rs:676 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.move_left | Move Tab Left | Ctrl+Shift+PageUp | apps/bareline/src/windows_app/views.rs:680 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.move_right | Move Tab Right | Ctrl+Shift+PageDown | apps/bareline/src/windows_app/views.rs:681 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.mru | Recent Document Switcher | Ctrl+Tab | apps/bareline/src/windows_app/views.rs:688 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.next | Next Tab | Ctrl+PageDown | apps/bareline/src/windows_app/views.rs:687 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.pin | Pin or Unpin Tab | — | apps/bareline/src/windows_app/views.rs:675 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.previous | Previous Tab | Ctrl+PageUp | apps/bareline/src/windows_app/views.rs:686 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.sort_descending | Sort Tabs Descending | — | apps/bareline/src/windows_app/views.rs:679 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.sort_name | Sort Tabs by Name | — | apps/bareline/src/windows_app/views.rs:677 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.sort_path | Sort Tabs by Path | — | apps/bareline/src/windows_app/views.rs:678 | NOT RUN | NOT RUN | NOT RUN |
| view.tabs.vertical | Vertical Tabs | — | apps/bareline/src/windows_app/views.rs:674 | NOT RUN | NOT RUN | NOT RUN |
| view.toolbar_customize | Customize Toolbar… | — | apps/bareline/src/windows_app/toolbar.rs:15 | NOT RUN | NOT RUN | NOT RUN |
| view.toolbar_focus | Focus Toolbar | — | apps/bareline/src/windows_app/toolbar.rs:16 | NOT RUN | NOT RUN | NOT RUN |
| view.toolbar_toggle | Show Toolbar | — | apps/bareline/src/windows_app/toolbar.rs:14 | NOT RUN | NOT RUN | NOT RUN |

Catalog row count: 387. This is a static extraction count, not a runtime command total.

## Dynamic and nonliteral reconciliation (all NOT RUN)

| Provider / route | Required reconciliation |
|---|---|
| Extension contributions | Enumerate installed enabled verified packages and each owner-qualified command; retain provider identity/generation and capability. Disabled/uninstalled/rejected providers must not dispatch stale actions. Include JSON/XML/Hex command sets from actual manager state. |
| Saved macros | Fixed 32 slots are listed where defined; names, recorded actions, arguments and availability depend on private macro configuration. Test configured, empty, deleted and failed playback slots. |
| Language/UDL and signatures | Catalog chooser actions plus actual installed language/definition rows. Catalog entries, imported names and completion suggestions are dynamic UI items, not invented stable commands. |
| Keymaps/toolbars/context projections | Record effective remaps, platform gestures and context menu paths for each existing ID. Localization and user customization can change displayed labels and exposure. |
| Recent files, tabs, workspace nodes and results | Generated item actions depend on target identities, not merely labels. Exercise stale/closed/reordered targets and keyboard versus pointer activation. |
| Direct gestures/internal actions | Typing, IME, caret/selection, scroll/zoom, splitter, tab drag and text drag may bypass registry. Retain their workflows in the coverage document; absence from this table is not absence from QA. |
| Nonliteral CommandSpec / helper registrations | If runtime reveals any ID missing here, append its source/provider and three NOT RUN slots before recording results. This table does not certify completeness of arbitrary Rust helper/macro expansions. |

The authoritative release reconciliation still requires a definitive runtime inventory from an appropriately supported artifact or a separately reviewed exhaustive source enumeration. This catalog is immediately usable for navigation and command-family testing, without claiming that prerequisite has been met.
