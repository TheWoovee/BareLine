# Bareline 0.1.0 — Full review: architecture, security, leaks, and hands-on UX — 2026-09-08

Reviewed tree: `51cd4c0` with the dirty working tree present on 2026-09-08. Tested binary: `dist/windows/0.1.0-local-20260908/bareline-0.1.0-windows-x64-portable.zip` extracted to an isolated scratch folder (portable mode, own `data/` directory). The installer was **not** executed; it modifies the user's profile and per-user registry and the owner did not ask for that specifically. Screenshots referenced as `shots/NN_*.png` are in `docs/qa/evidence/review-20260908/`.

Companion documents:

- Implementation plan: [`docs/implementation/PLAN-20260908-REVIEW-FIXES.md`](../implementation/PLAN-20260908-REVIEW-FIXES.md)
- Earlier manual campaign (32 issues, partially overlapping): [`MANUAL-QA-20260908-ISSUES.md`](MANUAL-QA-20260908-ISSUES.md)

## 1. Verdict

The engine underneath is substantial (piece-tree document, paged sources, codecs, recovery journal, UIA bridge, isolated extension host, 444 commands) and the neutral crates are cleanly separated from Win32. The product on top of it is **not usable as a Notepad++ replacement today**. An ordinary user will hit, within the first five minutes:

1. Menus that are alphabetical dumps of internal command IDs (File starts with *Exit*, Edit needs scroll arrows, Search has ~20 single-item submenus, Window is empty, internal debug text appears as a menu item).
2. A recovery journal that writes a new directory tree for nearly every edit and never deletes it (99 directories / 1,570 files left on disk after one short session and a clean exit).
3. Large files that are effectively read-only with broken line numbers, no Find, no Go To Line, and a scrollbar that does not track position.
4. A crash-recovered document that stays "indexing…" forever, reports read-only while accepting typing, and cannot be saved with Ctrl+S.
5. A window that any external `ShowWindow(SW_SHOW)` call hides permanently (no taskbar button, no tray icon, process keeps running and keeps taking keystrokes).
6. Save / Close prompts that only offer *Discard* (Yes/No), never *Save*.

Status doc claims of "27/27 packages IMPLEMENTATION_COMPLETE_PENDING_ACCEPTANCE" are not supported by the running binary: the first-party extensions cannot run in any build from this source (trust policy hard-coded to `None`), paged Find/Split are gated off with "not yet connected" strings, Rust has no outline provider, and the Window menu is empty.

Severity scale used below: **Blocker** (data loss, crash, unrecoverable state), **High** (feature unusable or misleading), **Medium** (works but wrong/annoying), **Low** (polish).

## 2. Hands-on test log (impatient-user pass)

Method: launched the portable build, maximized, walked all 14 menus, typed Unicode text, exercised undo/redo, Find/Replace, palette, Settings, every dialog reachable from the menus, opened a 106 MB / 1.2 M-line log, toggled every workspace panel, killed the process to test recovery, and exited with unsaved changes. Input was driven with SendInput/SendKeys; screenshots were captured after each step.

### 2.1 Blockers and data-safety

| ID | Sev | Screen / action | What happened | What should happen | Evidence |
|---|---|---|---|---|---|
| UX-01 | Blocker | Any editing | Every checkpoint of a *resident* Untitled document creates a brand-new `data/recovery/paged-<pid>-<ns>-<n>/` tree (16 files, ~5 KB) plus a `bareline-transcode-*` dir. 20 keystrokes produced 7 new trees. Nothing is ever deleted: 41 trees after launch+typing, 48 after 20 more keys, **99 trees / 1,570 files after a clean exit with "Discard"**. On a laptop this is an unbounded inode/disk leak, and Recovery Center only shows one of them. | One journal directory per document, rewritten in place (or a bounded ring of N checkpoints), pruned on clean close/discard. | Directory counts above; `crates/file-io/src/resident_recovery.rs:304` creates a new `PagedRecovery` per checkpoint; retirement in `resident_recovery.rs:150-175` only runs when `status.complete && status.durable.is_some()`, which the untitled path never reaches. |
| UX-02 | Blocker | Window | Calling `ShowWindow(hwnd, SW_SHOW)` from another process (what launchers, AutoHotkey, UIA tools, and "activate existing instance" helpers do) **hides** the main window. Reproduced 4/4 times on fresh launches; `SW_SHOWNA`, `SW_RESTORE`, `SetForegroundWindow` are fine, `SW_SHOWNORMAL` brings it back. While hidden there is no taskbar button and no tray icon (the tray icon is only created by *Minimize to Tray*), yet the process stays alive and keeps accepting keystrokes. Launching a second instance hands off and exits without restoring the window. | External show/activate must never hide the window. The instance handoff must call `set_visible(true)` before `focus_window()`. | `shots/25_restored.png`, `shots/27_second_instance.png`; `apps/bareline/src/windows_app/instance.rs:99` only does `set_minimized(false)`. Root cause is in the winit visibility-flag round trip; see plan P1-2. |
| UX-03 | Blocker | Crash recovery → relaunch | After a forced kill, relaunch opens a tab called "Recovered document" (original name "Untitled 1" lost). Status bar shows `170 B loaded · indexing…` forever and `RO`, yet typing is accepted and inserted at offset 0. Ctrl+S shows a banner "Preparing selected text…" that never completes and no Save dialog. The tab never shows a dirty marker. The document can only be closed by discarding. | Recovered document should be a normal resident document named after the original, editable, saveable, with dirty state. | `shots/41_save_dialog.png`, `shots/42_after_wait.png` |
| UX-04 | Blocker | Open 106 MB log (1.2 M lines) | File opens progressively (good) but then: status stuck at `65536 B loaded · indexing…`; gutter numbers restart at 1 on every page (line 50 shows as "1"); Ctrl+End shows a 2-line view "ms / (empty)" numbered 1–2; scrollbar thumb stays at the top and dragging it 500 px moves ~50 lines; Ctrl+F, Ctrl+H, Ctrl+G do **nothing** (silently disabled); document is `RO`. Banner text: "Resident data moved to private paged storage." | Real line numbers, proportional scrollbar, Find/Go-to-line/edit on paged files (the code exists per ARCH-02), plain-language status. | `shots/49_open_big_5s.png`, `shots/51_ctrl_end.png`, `shots/57_find_big.png`. Overlaps ISSUE-004, ISSUE-013. |
| UX-05 | Blocker | Close tab / Alt+F4 with unsaved changes | Prompt is a stock `MessageBox` "Discard unsaved changes to Recovered document • and close this tab?" with **Yes / No** only. No *Save*, no *Cancel* distinct from *No*, dirty-dot glyph leaked into the message, file list not shown on app exit. | Save / Don't Save / Cancel task dialog listing the dirty documents. | `shots/45_close_tab.png`, `shots/109_altf4.png` |
| UX-06 | High | Memory / handles | Baseline 58 MB / 538 handles. After opening the 106 MB file: 254 MB / 1,493 handles. After closing it: 169 MB / 1,460 handles, still 1,491 handles two minutes later. Document Map afterwards renders the *closed* log's minimap for a 10-line document. Roughly 900 handles and 110 MB are retained after close. | Closing a document releases its pages, spill files, layouts, and map cache. | `AppProcs` samples in test log; `shots/106_file_from_tree.png` (stale map). |
| UX-07 | High | Windows Error Reporting | The Application event log contains an `AppHangB1` for `bareline.exe` at 16:13 today ("stopped interacting with Windows and was closed"), from the owner's earlier run. The binary reports version **0.0.0.0** to WER because it has no VERSIONINFO resource. | No UI-thread hangs; embed a version resource so crash buckets are attributable. | Event IDs 1001/1002, 2026-09-08 16:13. |

### 2.2 Menus and command surface

| ID | Sev | Menu | Problem | Evidence |
|---|---|---|---|---|
| UX-10 | High | All | Items are emitted in registration/alphabetical order with **no separators and no grouping**. File: *Exit* is first, *New/Open* are items 16–17 of 36, Save is item 27. Edit: Copy, Cut, Paste, Redo, Select All, Undo, then 45 more. | `shots/04_menu_file.png`, `05_menu_edit.png` |
| UX-11 | High | Edit, Search | Edit menu is taller than a 1080 p screen and shows Win32 scroll arrows. Search menu has 32 items. | `05_menu_edit.png`, `06_menu_search.png` |
| UX-12 | High | Search, View | ~20 entries render as **submenus containing a single item with the same label** ("Find in Folder… ▸ Find in Folder…", "Mark All — Style 1 ▸", "Next Tab ▸"). Two clicks for every one of them. | `06b_submenu_findfolder.png`, `07_menu_view.png` |
| UX-13 | High | Window | The **Window menu is empty** — clicking it drops nothing. | `14_menu_window.png` |
| UX-14 | High | Encoding | An internal debug string is a menu item: `UTF-8 → UTF-8; Utf8Sample; BOM off; 0 invalid spans / 0 original bytes`. "Encoding…" opens the same list as a popup anchored to the bottom-right of the screen instead of a chooser; there is no list of code pages at all (no ANSI/Windows-125x/UTF-16 choice visible). Duplicates: "Line Endings…" and "Line Endings ▸", "Convert Save Encoding To…" and "Convert To ▸". | `08_enc.png`, `91_encoding.png` |
| UX-15 | High | File | Contextual commands pollute the top level: 5 tail-follow items, 5 recovery items, 2 conversion items, 3 tray items, 3 "remote file with permission" items — 21 of 36 entries are disabled most of the time. Missing: **Recent Files**, Close All, Close Others, Rename, Reload from Disk, Print, Open Folder. | `04_menu_file.png` |
| UX-16 | High | View | No Zoom In/Out/Reset, Word Wrap, Show Whitespace/EOL, Line Numbers, Fold All/Unfold All, Full Screen, Distraction-Free. Folding lives under *Language*. Toolbar is off by default and "Show Toolbar" is the last item. | `07_menu_view.png` |
| UX-17 | High | Search | No **Go To Line** (Ctrl+G does nothing anywhere). Menu starts with four *Cancel/Close* items that are meaningless when nothing is open. "Match Case", "Whole Word", "Toggle Literal / Extended" are toggles without check marks. | `06_menu_search.png`, `53_goto.png` |
| UX-18 | Medium | Language | No language list in the menu (only "Choose Language…"); the picker itself lists ~10 languages with no search and no scrollbar. | `09_lang.png`, `92_language.png` |
| UX-19 | Medium | Settings, Macro, Run, Extensions | Internal actions leak as menu items: "Change Setting", "Copy Setting TOML Key", "Close Settings", "Close Macro Manager" (enabled when it is not open), "Cancel External Command" (enabled when nothing runs), "Select Next Extension Command", "Run Declared Background Command (120 seconds)". Run has no *Run…* (F5) prompt — you must load a command definition file. | `10_settings.png`, `11_macro.png`, `12_run.png`, `17_ext.png` |
| UX-20 | Medium | Utilities | Sentence case ("Base64 decode") while every other menu is Title Case. "Print font family / footer / header / margins / line numbers" are *settings* exposed as menu actions. Print is not under File and has no Ctrl+P. The **Utilities menu disappears** from the menu bar when the last tab is closed. | `18_util.png`, `58_close_big.png` |
| UX-21 | Medium | Shortcut labels | "Ctrl+Alt+UP", "Alt+DOWN", "TAB", "ESCAPE", "Ctrl+SPACE" — should be "Up", "Down", "Tab", "Esc", "Space". | `05_menu_edit.png`, `09_lang.png` |
| UX-22 | Medium | Menu theme | Win32 menus are white on a dark app; the mockup shows themed menus. | all menu shots; overlaps ISSUE-001 |
| UX-23 | Medium | Command palette | Typing `sett` ranks *SHA-1 (legacy integrity hash)*, *SHA-256*, *SHA-512*, *Save As…* above *Settings*, which is not in the visible list at all. Matching is substring-in-any-order rather than prefix/fuzzy with scoring. | `68_palette_typed.png` |
| UX-24 | Medium | Shortcut Mapper | Lists raw IDs (`diff.added.overview`, `diff.changed`) mixed with titles, *Exit* first, list clipped with no scrollbar, monospace title. | `75_shortcut_mapper.png`; overlaps ISSUE-020 |

### 2.3 Editor and documents

| ID | Sev | Action | Problem | Evidence |
|---|---|---|---|---|
| UX-30 | High | Type in Untitled | Tab title shows **two** dirty dots: "Untitled 1 • •". | `62_find_typed.png` |
| UX-31 | High | Close last tab | Editor goes blank, status bar shows only dashes, the "Preparing selected text…" banner from the closed document stays on screen, no new Untitled is created and no empty-state hint. Notepad++ always keeps one document. | `46_after_discard.png` |
| UX-32 | High | Ctrl+S on Untitled | Native Save As appears with an **empty "Save as type"** filter and no default file name; the editor content behind every native dialog is blanked (document appears empty while the dialog is open). | `41_save_dialog.png`, `43_saveas.png`, `47_open_dialog.png` |
| UX-33 | Medium | Type `}` on an auto-indented line | No auto-dedent: closing brace stays at the inner indent level (line 10 in `95_rust_highlight.png`). After `}` at column 0, Enter still re-indents the next line. | `95_rust_highlight.png`, `20_typed.png` |
| UX-34 | Medium | Type emoji | A color emoji (U+1F389) typed through the IME/SendInput path did not appear; the status byte count did not include it. Needs a keyboard-level recheck, but the DirectWrite fallback chain should include Segoe UI Emoji. | `20_typed.png` |
| UX-35 | Medium | Find bar | Works, but: no placeholder, toggles are cryptic ("Aa", "ab", "Lit"), no regex toggle visible, no whole-document match highlighting, F3 from the field did not advance ("1 of 3" after Enter+F3), input spans the full 1,400 px width. | `62_find_typed.png`, `63_find_next.png` |
| UX-36 | Medium | Replace bar | Third row has an unlabeled mode button ("Literal") and "Scope: Current document" as plain text; buttons have four different widths. | `65_replace.png` |
| UX-37 | Medium | Context menu | Only Undo/Redo/Cut/Copy/Paste/Select All/Find. Cut/Copy enabled with no selection. Tab strip has no context menu. | `107_context_menu.png` |
| UX-38 | Medium | Window position | Each launch cascades the window 26 px further (52→130→156→182); size/maximized state is not restored. | `Wins` output in log |
| UX-39 | Low | Scrollbar | Full-height scrollbar track and thumb drawn for an empty one-line document; tab-strip scroll arrows always visible with one tab. | `03_maxclick.png` |
| UX-40 | Low | Status bar | No "Ctrl+Shift+P for commands" hint, no indentation mode (Spaces: 4), no selection length, no zoom. Clicking status items opens nothing except Encoding. | `03_maxclick.png` |

### 2.4 Panels, dialogs, and screens

| ID | Sev | Screen | Problem | Evidence |
|---|---|---|---|---|
| UX-50 | High | Workspace panel | Opening it **duplicates the active tab** in the tab strip ("Untitled 1 •" over the panel and "Untitled 1 • •" over the editor). No close button, no "Open Folder" button (only grey text at the bottom), no file icons, thick teal focus rectangle around the whole tree. | `96_workspace_panel.png`, `100_tree_click.png`; overlaps ISSUE-016 |
| UX-51 | High | Document List / Outline / Map | Toggling Document List **replaces** the Workspace tree instead of stacking. Outline for a Rust file says "No outline provider for this language". Document Map renders stale content from a closed file. Panels have no headers with close buttons and no splitter affordance. | `106_file_from_tree.png` |
| UX-52 | High | Extensions | A grey box overlays the editor leaving a 40 px sliver of text at the top. Monospace headings, a sidebar with one item, "Previous extension / Next extension" buttons, "Arguments… / Cancel / Results" buttons with nothing to act on, "Owner trust policy unavailable; installed packages cannot be verified". Nothing can be installed. | `81_extensions.png`; ARCH-01; ISSUE-014 |
| UX-53 | High | Recovery Center | Full-screen takeover with the tab strip removed. The only row is a raw path plus `| Complete | protected 1788875350405` (nanosecond timestamp). Preview uses a proportional font for code. Five full-width buttons. Only one of the 99 checkpoints is listed. | `79_recovery_center.png` |
| UX-54 | Medium | Settings | Full-page replacement with no visible Close/Back (Escape works, discovered by trial). Only 6 editor settings and 5 appearance settings. "Theme color overrides" and "Toolbar commands" require typing raw JSON into a dropdown-styled field. Dropdown items are lowercase ("system/light/dark") while the value shows "System". Mockup's theme cards and accent colors are absent. | `70_settings.png`, `72_theme_dropdown.png` |
| UX-54a | High | Settings › Editor | **Font family and font size are typed, not picked.** `editor.font.family` is `SettingKind::Text` and `editor.font.size` is `SettingKind::Number`, yet every row draws a dropdown chevron, so the user clicks expecting a list and gets a text box. Nothing in the app enumerates installed fonts; a typo silently falls back to another font. Notepad++ shows a font list with preview. | `70_settings.png`; `crates/settings/src/model.rs:153-169` |
| UX-54b | High | Settings | **No visible way to close the page.** There is no × or Back control, no tab, and the menu item is buried as "Close Settings" under Settings. Escape works but nothing tells the user. | `70_settings.png` |
| UX-55 | Medium | Light theme | Applies live (good). Recovery banner stays dark on light; active tab barely distinguishable; language picker/extensions/other custom panels reported dark in the earlier campaign. | `74_settings_esc.png`, `80_rc_esc.png`; ISSUE-011 |
| UX-56 | Medium | Column Editor | "Mode: text / numbers" and "Base: 10/16/8/2" are free-text fields you must type words into. | `85_column_editor.png` |
| UX-57 | Medium | Document statistics | Generic "Utility result" title, result duplicated in a banner, a meaningless "Retry" button. | `87_docstats.png` |
| UX-58 | Medium | Print | No preview; Font/Size/Margins/Range are buttons that cycle values with no indication they are clickable. Not reachable from File or Ctrl+P. | `89_print.png`; ISSUE-024 |
| UX-59 | Medium | Macro manager | Empty state with every button enabled; no Record/Delete/Export inside the manager; Ctrl+Shift+R style shortcuts absent from the menu. | `83_macro_mgr.png`; ISSUE-027 |
| UX-60 | Medium | Banners | Status banners ("Resident data moved to private paged storage.", "Preparing selected text…", "3 recovery checkpoint(s) available…") are developer language, persist across documents, cover the last text line, and cannot be dismissed. | `49_open_big_5s.png`, `58_close_big.png` |
| UX-61 | Low | About | Fine functionally; no logo, no link to release notes. | `77_about.png` |

### 2.5 Settings page audit (all 31 declared settings)

Source: `crates/settings/src/model.rs` (definitions), `crates/app/src/settings.rs:728-800` (editing), `apps/bareline/src/windows_app/settings.rs:458` (empty categories).

| ID | Sev | Finding |
|---|---|---|
| UX-54c | High | **Dead dropdown code.** `crates/app/src/settings.rs:732-739` returns `begin_value_edit` (free text) for every `Text`, `Integer`, `Number`, `Strings` and `Map` setting *before* reaching the choice lists written for exactly those kinds at `:748-781` (font families "Cascadia Mono / Consolas / monospace", font sizes 8–72 pt, tab width 1–16, power-of-two byte quotas). Those lists are unreachable. This is why font, size, tab width and every quota are typed instead of picked. |
| UX-54d | High | **Basic editor settings hidden under Advanced.** "Insert spaces" (`editor.insert_spaces`) and "Line numbers" (`editor.line_numbers`) are categorized as *Advanced*, so the Editor page shows six items and a user looking for "show line numbers" will not find it. |
| UX-54e | High | **Empty categories.** *Keyboard* and *Extensions* have zero settings and render "Edit this category in settings.toml". Keyboard should host the Shortcut Mapper (or link to it); Extensions should host the runtime/trust state or be removed. |
| UX-54f | High | **Byte quotas exposed as raw byte counts.** Eight settings (`document.resident_max_bytes` 4096…1,099,511,627,776, `page_size_bytes`, `page_cache_bytes`, `aggregate_cache_bytes`, `undo.aggregate_ram_bytes`, `transcode.temp_quota_bytes`, `clipboard.history.max_total_bytes`, `max_entry_bytes`) must be typed as integers with no unit, no KB/MB/GB formatting and no "auto" option. Nobody knows what 268435456 means. |
| UX-54g | Medium | **Developer wording in labels/descriptions.** "Resident document limit", "Source page bytes", "Aggregate document memory bytes", "Shared cap across tabs. Lowering retains live data and pauses new allocations above the cap", "Older complete changes are retired on document workers", "rebuild native menu labels". Descriptions also contain a mojibake dash (`�`) in the extracted strings, i.e. a non-ASCII separator that does not survive the build's encoding. |
| UX-54h | Medium | **Map/list settings are raw TOML/JSON text.** `theme.overrides`, `language.policies`, `language.associations` (Map) and `toolbar.commands`, `search.excludes` (Strings) show `[...]`/`{}` in a field and open the TOML file on click. Needs a key/value row editor, a pattern list editor with add/remove, and a command picker for the toolbar. |
| UX-54i | Medium | **Display language is a free-text field with one valid value.** `language.locale` accepts any string; only "en" exists. Either hide it until a second locale ships or make it a choice. |
| UX-54j | Medium | **Files category has one setting** (`session.restore`). Missing the things a Notepad++ user expects under Files: default encoding for new files, default line ending, auto-save/backup on interval, reload-on-external-change policy (there are menu commands for it but no setting), open-last-folder, confirm before closing dirty tabs. Search has one setting (`search.excludes`); missing case/whole-word/regex defaults, wrap-around, max results. |
| UX-54k | Medium | **Choice labels are raw values and inconsistent.** Dropdown entries show `system / light / dark`, `off / …`, `none / selection / …` in lowercase while the field shows "System"/"Off". Booleans render as an On/Off dropdown instead of a toggle switch. |
| UX-54l | Low | Restart-required settings (`renderer.mode`) give no restart prompt after change; "Workspace" scope radio at the top silently rejects most settings with "This setting is controlled by User scope" only after you try to edit. |

### 2.6 What worked

Typing, selection, undo/redo, Find with live match count, Replace UI, Settings live theme switch, Escape closing Find/Replace/Settings/Recovery/Column Editor/About, syntax highlighting after choosing Rust, folder tree browsing, crash recovery producing a document, progressive open of a 106 MB file (first page visible in ~1.5 s), memory returning partially on close.

## 3. Architecture and code quality review

Full table with evidence quotes: `docs/qa/evidence/review-20260908/review_architecture.md`. Summary of the 21 findings:

| ID | Sev | Finding | Location |
|---|---|---|---|
| ARCH-01 | High | First-party JSON/XML/Hex extension commands can never run: owner trust policy is `fn compiled_trust() -> Option<OwnerTrust> { None }`; also breaks the inventory export unless `--no-extensions`. | `apps/bareline/src/windows_app/extensions.rs:984` |
| ARCH-02 | High | Stale gates disable Find/Replace/split/open-document search for paged files with "not yet connected to paged storage", although `find.rs`, `search_panel.rs`, `views.rs:1209` implement them. Keyboard path swallows the disabled reason. | `apps/bareline/src/windows_app.rs:797-868` |
| ARCH-03 | High | One global `paged-view-io` worker (queue 16) for all paged docs holds the actor mutex across full-file Save; readers `try_lock` then spin with `yield_now()`. | `crates/editor-surface/src/paged_view.rs:50-62, 1279-1290, 1450-1478` |
| ARCH-04 | High | Every frame re-resolves settings/theme ~13× (`ui_theme()`/`effective()` clone the settings document, re-validate, rebuild token maps, run contrast validation). | `windows_app.rs:2446-2753`, `crates/settings/src/model.rs:975-1003` |
| ARCH-05 | Medium | Two keystroke resolvers; legacy one rebuilds `Keymap::defaults` from 444 commands and a full `command_context()` per key press. | `windows_app.rs:2172-2226` |
| ARCH-06 | Medium | 14-deep boolean `*_dispatch` chain; unrouted IDs fall into `execute_power` with a misleading message; keyboard path drops `DispatchError::Disabled`. | `windows_app.rs:1186-1236`, `settings.rs:723-731` |
| ARCH-07 | Medium | `Shell` god object: 53 fields mutated from 26 child modules; `window_event` is 1,274 lines, `dispatch` 539, `run` 397. | `windows_app.rs:67, 1007, 1634` |
| ARCH-08 | Medium | `WorkspaceEditor: Deref<Target=EditorSurface>` silently routes paged documents to the viewport surface; 223 explicit `Paged(` matches. | `crates/app/src/workspace.rs:16-27` |
| ARCH-09 | Medium | 366 `Result<_, String>` signatures; control flow matches on error text; two mutex-poisoning policies. | `paged_view.rs:905,930,955,280,1283` |
| ARCH-10 | Medium | 62 files fail `cargo fmt --check`; 465 lines > 240 chars; one 1,166-char line. Newest paged/power modules are unreviewable one-liners. | `crates/editor-surface/src/paged_power.rs:151` |
| ARCH-11 | Medium | `editor-surface` owns paged file lifecycle (save, recovery, fingerprints) — duplicate of `file-io` responsibilities. | `paged_view.rs:1038-1056` |
| ARCH-12 | Medium | 64 ad-hoc thread spawns + 51 `sync_channel(1)` copies of the same one-shot job pattern; unbounded thread creation. | `macros.rs:480`, `settings.rs:228`, `workspace_panels.rs:460` |
| ARCH-13 | Medium | `views.draw()` pumps workers and changes the selected tab — rendering mutates state. | `windows_app/views.rs:2160-2190` |
| ARCH-14 | Low | Dead code reported by the compiler (`watch_scroll_away`, `search_draw`, six `ViewsRuntime` accessors). | `watch.rs:409`, `views.rs:1470`, `search.rs:74` |
| ARCH-15 | Low | Three policies for duplicate command IDs (`let _ =`, `.expect`, `is_none()`). | `crates/app/src/language.rs:867` etc. |
| ARCH-16 | Low | `sync_contributions()` clones every extension command record on every command and wake. | `windows_app.rs:2954`, `extensions.rs:1221` |
| ARCH-17 | Low | Palette path clones the whole `Keymap` per key event. | `windows_app.rs:1929, 2033` |
| ARCH-18 | Low | Three overlapping `Theme` structs. | `crates/ui/src/widgets.rs:9`, `ui/src/theme.rs:73`, `settings/src/theme.rs:120` |
| ARCH-19 | Low | Raw `QueryPerformanceCounter` FFI in the app crate bypasses `platform-windows`. | `windows_app/performance.rs:61` |
| ARCH-20 | Low | "Fold All Known Regions" silently folds level 1 only. | `windows_app/language.rs:221`, `crates/app/src/language.rs:853` |
| ARCH-21 | Low | Every worker wake runs all ~25 `*_pump` methods. | `windows_app.rs:345-410` |

Positive: Win32 types are confined to `crates/platform-windows` (one FFI exception); no `todo!`/`unimplemented!`/`#[allow(dead_code)]`; all traced `std::fs` calls run on worker threads; command inventory validation enforces unique IDs at startup.

## 4. Security, resource leaks, and crash safety

Full table with evidence and exploit scenarios: `docs/qa/evidence/review-20260908/review_security.md` (32 findings). Release profile is `panic = "abort"`, so every panic below means the process dies and unsaved work is lost; there is no emergency-save path in the panic hook.

### 4.1 Crashes reachable from ordinary input

| ID | Sev | Finding | Location |
|---|---|---|---|
| CRASH-01 | **Critical, reproduced** | "Sort Lines Numerically" comparator is not a total order (`total_cmp` for numeric pairs, lexical `cmp` otherwise). With ~32+ mixed lines (`10`, `12 apples`, `2`, `1st`) Rust's `sort_by` panics → abort. Reproduced with a standalone repro on the project toolchain. | `crates/editor-surface/src/power.rs:627-646` |
| CRASH-02 | **Critical** | Same comparator in the streaming/external sort used for large selections and paged documents; worker panics → abort. | `crates/editor-surface/src/power/streaming.rs:97-102` |
| CRASH-03 | High | UDL tokenizer advances one *byte* past a string delimiter that may be multibyte (curly quotes, `«»`), then slices `&text[i..]` mid-scalar → highlight worker panics → abort on any file containing that character. | `crates/syntax/src/lib.rs:445-451` |
| CRASH-04 | Medium | Paged `Action::Edit` byte-slices the read window with unchecked subtraction/indexing (sibling `Prepared` path uses `checked_sub`/`get`). | `crates/editor-surface/src/paged_view.rs:1404-1420` |
| CRASH-05 | Medium | `views.bounds[pane].unwrap()` on a stored drag state after split collapse → `None.unwrap()`. | `apps/bareline/src/windows_app/power.rs:257` |
| CRASH-06..14 | Low | `std::env::args()` + byte slicing in the extension host; UDL import bypasses path trust (device path blocks worker forever); `expect()` on thread spawn in four worker inits; release `assert!` in `HistoryStack::push`; NaN geometry in `clamp`; unvalidated `SearchMarks` range; `.lock().unwrap()` in search service. | see full table |

### 4.2 Resource leaks

| ID | Sev | Finding | Location |
|---|---|---|---|
| LEAK-01 | High | Paged **reload** and **interpret-as-encoding** drop the old editor instead of retiring it; DirectWrite layouts are never released. After ~4–10 reloads the 512-layout cap is hit, shaping fails for every editor, and the window stops painting for the session. | `crates/app/src/workspace.rs:533, 549` (correct pattern at `:606-607`) |
| LEAK-02 | Medium | Secondary pane overwritten during power-edit promotion without retiring the old view; same mechanism. | `apps/bareline/src/windows_app/power_stream.rs:174` |
| LEAK-03 | Low | Recovery journals (full plaintext including unsaved Untitled documents) live in **Roaming** `%APPDATA%`, so domain roaming profiles sync unsaved text to profile servers. | `apps/bareline/src/windows_app/launch.rs:267,279` |
| LEAK-04 | Low | `%TEMP%\Bareline-transcode-*` / `Bareline-owned-spill` caches are removed only by `Drop`; after abort or power loss they persist forever; no startup sweep. Naming inconsistency `Bareline-transcode` vs `Bareline-transcodes`. | `crates/app/src/workspace.rs:168-780` |
| LEAK-05 | Low | Renderer `brushes` map grows per distinct colour; `formats` map never evicts and hard-fails at 512 entries. | `crates/platform-windows/src/renderer.rs:44-46, 172-210` |
| UX-01 | Blocker | Recovery checkpoint directories are never retired (see §2.1). Observed 99 directories / 1,570 files after one session. | `crates/file-io/src/resident_recovery.rs:150-175, 304` |
| UX-06 | High | ~900 handles and ~110 MB retained after closing a 106 MB document; Document Map shows the closed file. | observed |

### 4.3 Security

| ID | Sev | Finding | Location |
|---|---|---|---|
| SEC-01 | Medium | "Open Containing Folder" launches an **unqualified** `explorer.exe` via `ShellExecuteW`; CWD is never pinned, so a folder containing `notes.txt` + `explorer.exe` gets code execution when the user reveals the file. | `crates/platform-windows/src/shell_integration.rs:12-13` |
| SEC-02 | Medium | Shell-mode external commands splice `${file}`, `${dir}`, `${selection}` into `cmd.exe /c <template>` with no metacharacter escaping; a filename `a&calc.exe.txt` or a selection containing `& | ^ %VAR%` is executed. Mitigated by the per-run confirmation and user-authored definitions. | `crates/macros/src/process.rs:172-186, 345-393, 620-629` |
| SEC-03 | Medium | Extension host runs with the user's full token, environment and inherited CWD; containment is only a kill-on-close job. Wasmtime is the sole barrier. | `crates/platform-windows/src/process.rs:49-69` |
| SEC-04 | Low | Shell-mode child CWD is the document directory, so unqualified tool names resolve from the document folder first. | `crates/macros/src/process.rs:626-629` |
| SEC-05..11 | Low | Non-secret host "nonce" on the command line; single pipe instance fails on a same-user squatter instead of retrying; host forwards unchecked trailing bytes; production host accepts WebAssembly text format; zip entry count not cross-checked (duplicate names keep last); unbounded panel output line length; worker parks in `child.wait()` for up to 120 s. | see full table |

Verified clean by the reviewer: single-instance pipe (SID-hashed name, user DACL, peer PID/session/SID checks); update chain (minisign + SHA-256 + Authenticode on one held handle, handle-based rename, no TOCTOU); path trust (UNC/device/ADS/reparse/hard-link refusal); PCRE2 limits (match/depth/heap + 2 s callout deadline, no JIT); extension framing (all length prefixes bounded, WASI closed); zip extraction (no zip-slip, size bombs, absolute paths); atomic save via `ReplaceFileW`; no first-party memory mapping; `cargo deny check advisories` clean.

## 5. Design improvement suggestions

These go beyond bug fixes; they are what would make Bareline *feel* better than Notepad++ rather than merely match it.

1. **Curated menus, not generated ones.** Hand-author each top-level menu as an ordered tree with separators and submenus (File → Recent Files ▸, Encoding → Character Sets ▸, Search → Bookmarks ▸, View → Zoom ▸, Show Symbol ▸). Keep the auto-generated inventory for the palette only. Contextual commands (tail-follow, conversion, tray, remote) move into a contextual "Document" group that is hidden, not disabled, when not applicable.
2. **Owner-drawn or themed menus and dialogs.** The mockups show dark menus and dark dialogs. Use `SetPreferredAppMode(AllowDark)` + `FlushMenuThemes` (uxtheme ordinals 135/136) for native menus, and render the custom overlays with the same token set as the editor. Native file dialogs are acceptable, but they must be parented and centered, and the editor must keep painting behind them.
3. **Three-button save prompt as a task dialog** (`TaskDialogIndirect`) listing dirty documents with checkboxes, like VS Code's "Save All / Don't Save / Cancel".
4. **Persistent single-line status strip** with clickable segments (language, EOL, encoding, indent, Ln/Col, zoom) and a right-aligned "Ctrl+Shift+P" hint, replacing the ad-hoc banners. Banners become toast notifications with a close button and a 6 s auto-dismiss for informational ones.
5. **Panel system**: left dock (Workspace, Document List, Outline stacked with headers and ×), right dock (Map), bottom dock (Output, Search results, Compare). Each panel remembers size; panels never overlap the tab strip.
6. **Large-file mode UX**: a yellow "Large file mode — read-only until indexing completes (37 %)" strip with a progress bar, real global line numbers, proportional scrollbar based on byte offset, Find/Go-to-line always available (byte-offset based), and a one-click "Edit anyway (loads N MB)" action.
7. **Recovery Center as a list of documents**, not directories: name, original path, last checkpoint time, size, and Restore / Compare / Delete per row, with "Delete all older than…" housekeeping.
8. **Palette scoring**: prefix and word-boundary matches first, then fuzzy; show category and shortcut; remember last used.
9. **Settings**: keep the searchable page but add the theme cards (Light/Dark/System with accent colors) from the mockup, typed editors for list/map values, and a "Open settings.toml" escape hatch instead of JSON-in-a-combobox.
10. **First-run**: create Untitled 1, show a dismissible welcome hint (Ctrl+O, Ctrl+Shift+P, Ctrl+, ), remember window bounds and maximized state.
11. **Language menu** with the full lexer list grouped A–Z, and an Outline provider for at least Rust/C/C++/Python/JS/TS via the existing regex outline definitions.
12. **Brand**: use the actual logo (`docs/blueprint/mockups/logo-brand`) for the window icon, taskbar, tray, About, and installer; the generic `IDI_APPLICATION` tray icon and the "b" glyph do not read as a product.

## 6. Cross-reference to the earlier campaign

Overlaps: UX-04 ↔ ISSUE-004/013/015; UX-50 ↔ ISSUE-016; UX-22 ↔ ISSUE-001; UX-52 ↔ ISSUE-014; UX-55 ↔ ISSUE-011; UX-58 ↔ ISSUE-024; UX-59 ↔ ISSUE-027; UX-24 ↔ ISSUE-020. Everything else in section 2 is new.
