# Complete acceptance and minimum-scenario register

Generated from the preserved v1.3 authorities for the [implementation backlog](BACKLOG.md). All statuses mean **evidence reconciliation/qualification pending for the selected candidate**, not that every scenario has never run. Timing targets remain informational under ADR-10. Product tests may require headless, native, physical, VM or foreign-host evidence as specified.

## Coverage totals

- 81 atomic acceptance criteria, each assigned exactly once to its QUAL task.
- 199 minimum verification scenarios, preserved without silently dropping requirements.
- 52 capability families, each assigned to qualification or an existing scope disposition.
- Runtime commands: capture the actual configured inventory and track success/disabled/failure separately; no command outcome is inferred from a count.

## QUAL-001 — Foundation, startup and rendering

[Original brief](../../../docs/blueprint/PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-001-01:** Shape one mixed Arabic/Latin/emoji line with DirectWrite and BiDi caret tracking; place IME preedit/candidate windows at 100%, 150% and 200% DPI and recover from device loss.
- [ ] **AC-001-02:** Open 5,000 lazy tabs; worker count remains bounded and no document reads occur before the first frame.
- [ ] **AC-001-03:** Verify codec, filesystem-capability and VerifiedPackageSource interfaces compile with fake providers; online provider fails closed.

### Minimum scenarios

- [ ] **MV-001-01:** Launch and close 100 times without orphan processes or increasing persisted state.
- [ ] **MV-001-02:** Leave idle for 60 seconds and verify no continuous redraw/timer wake loop.
- [ ] **MV-001-03:** Resize/DPI-change the window repeatedly; UI remains crisp and no renderer resource leak is visible.
- [ ] **MV-001-04:** Force Direct2D device recreation path in a test/mock and confirm the window survives.
- [ ] **MV-001-05:** Software render mode launches when hardware creation is blocked in a test.
- [ ] **MV-001-06:** Duplicate command ID test fails deterministically.
- [ ] **MV-001-07:** Neutral crates compile on Windows, Linux and macOS CI without importing Win32 types.
- [ ] **MV-001-08:** `xtask perf launch` runs twice and produces comparable JSON with the same schema.
- [ ] **MV-001-09:** RecordingBackend UI tests run on Linux and macOS CI.
- [ ] **MV-001-10:** `cargo deny check` passes.
- [ ] **MV-001-11:** Startup-budget assertion fails deterministically if a document read is injected before first frame.

## QUAL-002 — Document storage, transactions and undo

[Original brief](../../../docs/blueprint/PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-002-01:** Generate random CR/LF/CRLF edits, including separators split across pieces; line counts match the reference model.
- [ ] **AC-002-02:** Delete a range, evict its base pages and externally replace the source; undo restores owned inverse bytes exactly.
- [ ] **AC-002-03:** Race a Resident background fill with truncation; missing old-generation bytes return Unavailable and never new contents.

### Minimum scenarios

- [ ] **MV-002-01:** Property test 100k random insert/delete/replace operations against a simple `Vec<u8>` oracle.
- [ ] **MV-002-02:** Same random sequence with undo-all then redo-all produces byte-identical results and correct selections.
- [ ] **MV-002-03:** 100 MB fixture opens `Resident` with the first viewport published before the full read completes.
- [ ] **MV-002-04:** 1 GB fixture opens `Paged` with a bounded cache; open generated 1 GB/5 GB streamed fixture and assert resident memory does not scale linearly with base size.
- [ ] **MV-002-05:** `Paged` source truncated externally yields `SourceChanged` and `Unavailable` reads, never a crash.
- [ ] **MV-002-06:** Two threads submit transactions through `DocumentService` and ordering is serialized with monotonic revisions.
- [ ] **MV-002-07:** Newline mapping correctness across CRLF/LF/CR byte sequences over UTF-8 input, including opaque spans retaining invalid original bytes.
- [ ] **MV-002-08:** Snapshot reader continues to see its original revision while mutations proceed.
- [ ] **MV-002-09:** Corrupt/out-of-range transaction is rejected without partial mutation.

## QUAL-003 — Editor input, selection and large-line layout

[Original brief](../../../docs/blueprint/PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-003-01:** Select across emoji clusters, combining accents and opaque byte markers; text edits never split their atomic boundaries.
- [ ] **AC-003-02:** Scroll a 20 MiB single line; layout is bounded and cancellation leaves input responsive.
- [ ] **AC-003-03:** Commit and cancel IME preedit; only committed text enters undo and recovery; verify real Windows shaping and BiDi hit testing.

### Minimum scenarios

- [ ] **MV-003-01:** Type, delete, navigate and select mixed ASCII, Tamil, Arabic, CJK, combining marks and emoji without splitting encoded characters.
- [ ] **MV-003-02:** 10,000 carets remain correct and edit as one undoable transaction; UI may cap visible decorations only with explicit UX indication.
- [ ] **MV-003-03:** Scroll a 1 GB `Paged` fixture before total line indexing completes; thumb and viewport remain usable and unavailable pages render as placeholders.
- [ ] **MV-003-04:** A 20 MB single line can be opened and horizontally navigated without a multi-second main-thread allocation.
- [ ] **MV-003-05:** IME preedit cancel leaves document revision unchanged; commit increments exactly once.
- [ ] **MV-003-06:** Clone two views of one document and verify view state independence, including independent `hidden_ranges`.
- [ ] **MV-003-07:** All surface tests run against `RecordingBackend` on Linux and macOS CI.

## QUAL-004 — Save, sessions and crash recovery

[Original brief](../../../docs/blueprint/PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-004-01:** Edit, save, edit again and undo to saved ContentStateId; the dirty marker clears without comparing monotonic revisions.
- [ ] **AC-004-02:** Kill the process after each segment/journal/checkpoint flush boundary; recovery stops at the last valid durable record.
- [ ] **AC-004-03:** Start a save then type; successful save clears only its captured state. Missing source regions disable Save/Save As and offer export with gaps.

### Minimum scenarios

- [ ] **MV-004-01:** Kill process after every journal write boundary and recover to last valid record.
- [ ] **MV-004-02:** Simulate disk-full/permission-denied/AV lock during save; original file remains byte-identical.
- [ ] **MV-004-03:** External writer changes a file between edit and save; conflict is detected and no overwrite occurs without user action.
- [ ] **MV-004-04:** Conflict dialog shows a diff preview produced by `crates/diff`.
- [ ] **MV-004-05:** Session with 500 tabs restores incrementally without blocking first interactive window; session restore shows the first frame before any document read completes.
- [ ] **MV-004-06:** Pinned tabs/splits/caret/scroll survive versioned session migration.
- [ ] **MV-004-07:** Restore Last Closed Tab restores document, caret and pin state.
- [ ] **MV-004-08:** Malicious session entries with `..`, device paths or UNC do not trigger access before trust evaluation.
- [ ] **MV-004-09:** `Paged` document truncated externally shows the SourceChanged banner and Reload reopens cleanly.

## QUAL-005 — Search, regex, marks and replacement

[Original brief](../../../docs/blueprint/PRs/PR-005_SEARCH_REPLACE_AND_RESULTS_ENGINE.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-005-01:** Match across a 16 MiB window in a 20 MiB line; case-fold results map to correct original TextOffsets.
- [ ] **AC-005-02:** Exercise lookbehind, anchors, empty matches and partial regex contexts; unsupported cases report incomplete and disable replacement.
- [ ] **AC-005-03:** Replace in two dirty open documents while one revision changes; prepare aborts before either mutates; successful linked undo reverses both.

### Minimum scenarios

- [ ] **MV-005-01:** Matches adjacent to 16 MiB chunk boundaries for literal and single-line regex modes.
- [ ] **MV-005-02:** A multi-line regex match that crosses a chunk boundary is found via partial-match streaming.
- [ ] **MV-005-03:** Regex captures/backreferences/lookarounds and common Notepad++ replacement patterns fixtures.
- [ ] **MV-005-04:** Cancel a search over generated 20 GB corpus and observe stop acknowledgement within target budget.
- [ ] **MV-005-05:** Search result navigation in 1 GB file stays bounded and does not trigger full-file parse.
- [ ] **MV-005-06:** Replace-all in 100 open docs is one undo transaction per document and reports partial failures.
- [ ] **MV-005-07:** Mark styles 1 to 5 apply and clear independently and survive edits above the marked lines.

## QUAL-006 — Multi-cursor, columns and power editing

[Original brief](../../../docs/blueprint/PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-006-01:** Column insertion across tabs, wide glyphs and short lines follows display columns and undoes in one transaction.
- [ ] **AC-006-02:** Enable clipboard history then exceed 20 entries and 16 MiB total; bounded eviction occurs and history never persists to disk.
- [ ] **AC-006-03:** Invoke comment commands before PR-013; show unavailable with a reason and do not mutate text.

### Minimum scenarios

- [ ] **MV-006-01:** Reproduce Notepad++ issue-class UTF-8 rectangle pastes with multibyte bullets/emoji and prove no byte corruption.
- [ ] **MV-006-02:** Rectangle copy/paste across tabs, CJK wide glyphs and short lines.
- [ ] **MV-006-03:** 10k cursor insertion/delete/undo produces exactly one undo entry.
- [ ] **MV-006-04:** Line sort/remove-duplicate on mixed EOL selection preserves configured EOL policy.
- [ ] **MV-006-05:** Column Editor inserts padded sequences across a rectangle with short lines, in all four bases, as one undo entry.
- [ ] **MV-006-06:** Hidden lines are view-only and do not change bytes or save output.
- [ ] **MV-006-07:** Clipboard history is empty after restart and absent when disabled.
- [ ] **MV-006-08:** Comment commands report unavailable through the hook before PR-013 and dispatch to a stub provider in tests.

## QUAL-007 — Encoding, provenance and EOL fidelity

[Original brief](../../../docs/blueprint/PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-007-01:** Round-trip invalid UTF-8, genuine U+FFFD/private-use characters and noncanonical legacy sequences without provenance collisions.
- [ ] **AC-007-02:** Open/save UTF-16LE/BE, UTF-32LE/BE and true Latin-1 fixtures with BOM and mixed EOL; exact unedited bytes survive.
- [ ] **AC-007-03:** Transcode a generated 5 GiB UTF-16 file under the RAM budget; disk quota exhaustion is visible and original data remains intact.

### Minimum scenarios

- [ ] **MV-007-01:** Round-trip representative UTF-8/BOM, UTF-16LE/BE, UTF-32, Windows-1252, Shift-JIS, GBK, Big5, EUC-JP and other chosen supported pages byte-identically when untouched.
- [ ] **MV-007-02:** Legacy file containing undecodable bytes round-trips byte-identically through tagged original-byte provenance.
- [ ] **MV-007-03:** UTF-16 2 GB generated fixture transcodes in a streaming pass with bounded memory and viewport-first display.
- [ ] **MV-007-04:** Interpret-as switches from Windows-1252 to Shift-JIS without touching disk.
- [ ] **MV-007-05:** Edit near transcode chunk boundaries in a multibyte legacy encoding and save/reopen correctly.
- [ ] **MV-007-06:** Attempt conversion containing unrepresentable characters; save is refused without corrupting original.
- [ ] **MV-007-07:** Mixed CRLF/LF/CR stays mixed after unrelated edit.
- [ ] **MV-007-08:** Huge file encoding detection reads only bounded samples before first viewport.

## QUAL-008 — Language catalog, syntax, folding and UDL

[Original brief](../../../docs/blueprint/PRs/PR-008_LANGUAGE_CATALOG_SYNTAX_HIGHLIGHTING_FOLDING_AND_UDL.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-008-01:** Validate paired Lexilla/native definitions for all 15 languages listed in FC-09; publish coverage and known grammar gaps.
- [ ] **AC-008-02:** Open in the middle of a multiline string; provisional highlighting is labelled until a valid lexer checkpoint resolves context.
- [ ] **AC-008-03:** Import malformed/oversized UDL input; reject unsafe structure and preserve the previous definition.

### Minimum scenarios

- [ ] **MV-008-01:** Catalog parity audit against current Notepad++ built-in language families.
- [ ] **MV-008-02:** Lexing fixture snapshots for representative languages and multiline-state edits.
- [ ] **MV-008-03:** Edit near top of 1 GB file invalidates bounded regions and viewport re-highlights without full relaunch.
- [ ] **MV-008-04:** Collapse/expand fold on large file cannot freeze UI.
- [ ] **MV-008-05:** Import several real Notepad++ UDL XML fixtures and emit deterministic compatibility report.
- [ ] **MV-008-06:** Fuzz malformed lexer/UDL inputs across FFI boundary.
- [ ] **MV-008-07:** Native UDL fallback produces styled viewport within 100 ms for each top-15 language on 1 GB fixture.
- [ ] **MV-008-08:** Fold Level N commands act on folds known so far and continue as indexing completes.

## QUAL-009 — Workspace, document list, outline and map

[Original brief](../../../docs/blueprint/PRs/PR-009_WORKSPACE_EXPLORER_DOCUMENT_LIST_OUTLINE_AND_DOCUMENT_MAP.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-009-01:** Select main.rs then Cargo.toml; outline changes language and old-revision results never appear in the new document.
- [ ] **AC-009-02:** Open a folder with symlink cycles and inaccessible descendants; traversal is bounded and shows partial results.
- [ ] **AC-009-03:** Scroll before line indexing completes; minimap and outline use estimates explicitly and do not allocate per-line objects.

### Minimum scenarios

- [ ] **MV-009-01:** Open directory containing 1M generated entries across nested folders; first UI interaction only enumerates required levels.
- [ ] **MV-009-02:** Rename/delete/create race produces an error/update, never a stale phantom operation.
- [ ] **MV-009-03:** Document list handles 5k tabs with virtualized rendering.
- [ ] **MV-009-04:** Outline progressively fills on large source file and stale click cannot jump into unrelated text.
- [ ] **MV-009-05:** Document map navigates 5 GB log while total line indexing is incomplete.
- [ ] **MV-009-06:** Import three real Notepad++ functionList definitions and emit a deterministic mapping report.

## QUAL-010 — Tabs, split/clone and synchronized views

[Original brief](../../../docs/blueprint/PRs/PR-010_SPLIT_VIEWS_TAB_MANAGEMENT_AND_SYNCHRONIZED_SCROLLING.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-010-01:** Pin a tab and reorder across views; pinned-first default and per-view selection persist after restore.
- [ ] **AC-010-02:** Add a compare alignment spacer; it receives no document line number and synchronized scrolling maps logical lines correctly.
- [ ] **AC-010-03:** Clone a document in a second pane; edits share a document while caret, selection, folds and scroll remain independent.

### Minimum scenarios

- [ ] **MV-010-01:** Same document in two panes: edit one, both content updates; cursors/folds stay independent.
- [ ] **MV-010-02:** Vertical/horizontal sync with different viewport sizes and wrap modes has no recursion/jitter.
- [ ] **MV-010-03:** 500 tabs with pinned/colored/MRU order survive session restart/update migration.
- [ ] **MV-010-04:** Drag/reorder cannot move a pinned tab into invalid state.
- [ ] **MV-010-05:** Corrupt split layout does not prevent session recovery.

## QUAL-011 — Menus, palette, shortcuts and command routing

[Original brief](../../../docs/blueprint/PRs/PR-011_MENUS_COMMAND_PALETTE_SHORTCUTS_TOOLBAR_AND_CONTEXT_ACTIONS.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-011-01:** Every menu/toolbar/palette action resolves to one stable ID; unavailable actions have a reason and no handler mutation.
- [ ] **AC-011-02:** Press Escape in nested popup, panel and editor states; only the topmost transient layer closes and focus returns predictably.
- [ ] **AC-011-03:** Test AltGr text, menu mnemonics and remapped shortcuts together; text entry is not consumed as a global shortcut.

### Minimum scenarios

- [ ] **MV-011-01:** Assert every menu/toolbar/keybinding target resolves to exactly one registered command.
- [ ] **MV-011-02:** Duplicate command IDs and key conflicts surface deterministic diagnostics.
- [ ] **MV-011-03:** AltGr/IME text input is not swallowed as Ctrl+Alt shortcuts.
- [ ] **MV-011-04:** Change UI language and saved shortcuts/macros still work because IDs are stable.
- [ ] **MV-011-05:** Palette finds commands by title, synonym and menu path under 10 ms on normal catalog.
- [ ] **MV-011-06:** Native menu bar and command palette are generated from the same menu model and show identical shortcuts.

## QUAL-012 — Settings, themes, DPI and localization infrastructure

[Original brief](../../../docs/blueprint/PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-012-01:** Change font size to 12 pt at mixed DPI; preview, persisted value and restored editor use the same point unit.
- [ ] **AC-012-02:** Attempt workspace override of execution, network or extension grants; reject it while accepting allowed indentation preferences.
- [ ] **AC-012-03:** Check Light/Dark/System tokens and override persistence; normal text meets 4.5:1 and focus indicators 3:1 on actual surfaces.

### Minimum scenarios

- [ ] **MV-012-01:** Settings schema migrations preserve unrelated/unknown keys and survive malformed single entries.
- [ ] **MV-012-02:** Runtime light/dark/high-contrast switch remains readable.
- [ ] **MV-012-03:** 100/125/150/200/300% DPI resize produces stable hit targets/no blurry text.
- [ ] **MV-012-04:** Every setting row displays its TOML key and a copy action.
- [ ] **MV-012-05:** Locale with long strings/RTL does not clip critical actions.

## QUAL-013 — Completion, comments and smart typing

[Original brief](../../../docs/blueprint/PRs/PR-013_COMPLETION_SMART_TYPING_AND_LANGUAGE-AWARE_EDITING.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-013-01:** Register PR-006 CommentProvider with a language; line/block toggles handle empty selections and undo correctly.
- [ ] **AC-013-02:** Type while completion computes; old-generation/revision suggestions cannot replace newer text.
- [ ] **AC-013-03:** Complete within strings and comments using bounded local indexes; suppress semantically unsupported suggestions.

### Minimum scenarios

- [ ] **MV-013-01:** Completion remains responsive in 1 GB file and bounded memory is asserted.
- [ ] **MV-013-02:** Multi-cursor pair insertion/indent is one undo step.
- [ ] **MV-013-03:** Typing quote/brace in comments/strings follows language rule without waiting for background lexing.
- [ ] **MV-013-04:** Completion accept preserves correct byte ranges after a concurrent/stale provider result by rejecting/remapping revision.
- [ ] **MV-013-05:** No network requests or AI dependency exists.

## QUAL-014 — Macros, external processes and output

[Original brief](../../../docs/blueprint/PRs/PR-014_MACROS_EXTERNAL_COMMANDS_AND_OUTPUT_PANEL.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-014-01:** Record/replay only stable command IDs and explicit text; playback failure stops with a resumable location.
- [ ] **AC-014-02:** Run a path and argument containing spaces, quotes and shell metacharacters using direct spawn; no implicit shell interpretation occurs.
- [ ] **AC-014-03:** Cancel a process producing unlimited output; output storage stays bounded, process cleanup completes and the editor remains usable.

### Minimum scenarios

- [ ] **MV-014-01:** Record/edit/search/navigate macro, restart app, replay deterministically.
- [ ] **MV-014-02:** Until-EOF macro stops on no progress/EOF and can be canceled.
- [ ] **MV-014-03:** External process emitting >1 GB output cannot grow editor memory without bound or deadlock.
- [ ] **MV-014-04:** Arguments containing spaces/quotes/metacharacters are passed correctly in direct mode without shell injection.
- [ ] **MV-014-05:** Cancel long-running process and ensure UI/output remain responsive.

## QUAL-015 — File watching, tail and path trust

[Original brief](../../../docs/blueprint/PRs/PR-015_FILE_WATCHING_MONITORING_TAIL_AND_SAFE_REMOTE_PATHS.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-015-01:** Append to a log with a partial final page; verify continuity before accepting the new generation, without falsely entering SourceChanged.
- [ ] **AC-015-02:** Rotate, truncate and rewrite same-size content; uncertain continuity becomes SourceChanged and preserves owned edits.
- [ ] **AC-015-03:** Pause tail scrolling while appends continue; unlock stops following and captures a fixed generation after confirmation.

### Minimum scenarios

- [ ] **MV-015-01:** External save/atomic-replace/rename/delete patterns from common editors are detected correctly.
- [ ] **MV-015-02:** Dirty buffer is never silently replaced by external change.
- [ ] **MV-015-03:** Tail a rapidly growing multi-GB log with bounded memory and rotation/truncation handling.
- [ ] **MV-015-04:** Rotate or truncate the log during tail: document enters `SourceChanged`, "Reopen and follow" resumes on the new file, and the process never crashes or reads stale bytes as new.
- [ ] **MV-015-05:** Malicious session referencing UNC does not cause SMB/credential access at startup.
- [ ] **MV-015-06:** Watcher overflow recovers state without freezing UI.
- [ ] **MV-015-07:** Watcher thread survives directory deletion and re-creation without leaking handles.

## QUAL-016 — Extension manager, runtime and isolation

[Original brief](../../../docs/blueprint/PRs/PR-016_EXTENSION_PLATFORM_AND_ISOLATED_PLUGIN_MANAGER.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-016-01:** Install a signed offline runtime and extension using the PR-001 provider contract; no PR-018 online code is required.
- [ ] **AC-016-02:** Disable all extensions; host exits and installed files remain. Explicit Remove runtime deletes only its verified owned files.
- [ ] **AC-016-03:** Send oversized frames, unauthorized capabilities and a CPU loop; reject/terminate the offending extension within per-extension limits.

### Minimum scenarios

- [ ] **MV-016-01:** Kill/hang/fuzz extension host while editing; editor and documents remain intact.
- [ ] **MV-016-02:** Oversized/malformed frames rejected before large allocation.
- [ ] **MV-016-03:** Extension requests workspace write/network/process without grant and receives deterministic denial.
- [ ] **MV-016-04:** Stale revision edit cannot apply.
- [ ] **MV-016-05:** Extension update adding capability requires explicit re-approval.
- [ ] **MV-016-06:** Zero enabled extensions => host stopped, no WASM runtime in the editor process. Previously installed runtime files remain until explicit Remove runtime; a fresh core install has none.
- [ ] **MV-016-07:** Tampered `bareline-exthost-x64` pack signature or hash is rejected before extraction; the editor stays usable.
- [ ] **MV-016-08:** Catalog index with an invalid minisign signature is rejected before parsing.

## QUAL-017 — Compare, utilities, export and print

[Original brief](../../../docs/blueprint/PRs/PR-017_BUILT-IN_UTILITIES_COMPARE_FOUNDATION_EXPORT_AND_PRINT.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-017-01:** Copy a hunk left-to-right then undo; source labels, dirty state and selection match the actual target revision.
- [ ] **AC-017-02:** Choose ignore options and recompare; cancellation is distinct from a completed coarse result and stale merge buttons disable.
- [ ] **AC-017-03:** Print/export Unicode with syntax/theme options; unsupported printer/filesystem errors preserve the original and show a clear retry path.

### Minimum scenarios

- [ ] **MV-017-01:** Hash 20 GB generated file with bounded memory/cancel.
- [ ] **MV-017-02:** Base64/URL invalid input does not partially edit selection.
- [ ] **MV-017-03:** Diff ordinary source files with line and intraline highlighting, navigation and aligned synchronized scrolling.
- [ ] **MV-017-04:** Configure distinct compare colors in Light, Dark and System themes; switch Windows theme live and preserve overrides.
- [ ] **MV-017-05:** Execute left→right/right→left hunk merge; verify one-step undo and no implicit save.
- [ ] **MV-017-06:** Exercise ignore whitespace/case/blank/EOL options with deterministic fixtures.
- [ ] **MV-017-07:** Edit while compare is running and verify stale revision results never paint.
- [ ] **MV-017-08:** Compare Workspace stays responsive and shows Coarse completeness when `crates/diff` reports it for a generated multi-GB/highly-different fixture; cancel from the toolbar stops the job.
- [ ] **MV-017-09:** Compare Workspace restores both sources, layout, options and sync-scroll choice from a session round trip.
- [ ] **MV-017-10:** HTML export escapes `<>&` and does not allow source-controlled script injection.
- [ ] **MV-017-11:** Print preview/print adapter handles DPI/font/line wrapping and cancellation.

## QUAL-018 — CLI, portable, installer and updater

[Original brief](../../../docs/blueprint/PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-018-01:** Reject an expired, rollback, wrong-channel, wrong-publisher or hash-mismatched update before staging application files.
- [ ] **AC-018-02:** Verify signatures on inner executables after extracting final ZIP/installer; compare reproducibility only before signing.
- [ ] **AC-018-03:** Launch CLI with non-BMP paths and arguments via authenticated same-user IPC; portable mode writes only to its configured data root.

### Minimum scenarios

- [ ] **MV-018-01:** CLI paths with spaces/Unicode/device-like strings are parsed and transferred correctly.
- [ ] **MV-018-02:** Concurrent launches do not corrupt session or lose open requests.
- [ ] **MV-018-03:** Installer upgrade preserves settings/recovery and pinned tabs.
- [ ] **MV-018-04:** Portable mode writes nothing to expected installed config/registry locations.
- [ ] **MV-018-05:** Tampered update manifest/package/signature is rejected before apply; rollback restores prior executable.
- [ ] **MV-018-06:** Imported Notepad++ session cannot trigger untrusted UNC access without PR-015 policy.
- [ ] **MV-018-07:** `winget validate` passes on the generated manifest and the manifest hash matches the released installer.
- [ ] **MV-018-08:** Installer per-user mode completes on a standard user account with no UAC prompt and no writes under `Program Files` or `HKLM`.
- [ ] **MV-018-09:** Extension host pack download through the updater verifies minisign and hash before extraction; a tampered pack is rejected.
- [ ] **MV-018-10:** Launch with an existing instance that is hung: the pipe connect times out and a new instance appears within the startup budget.

## QUAL-019 — Huge-file and performance evidence

[Original brief](../../../docs/blueprint/PRs/PR-019_HUGE-FILE_OPTIMIZATION_AND_PERFORMANCE_BENCHMARK_SUITE.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-019-01:** Run paired baseline trials with pinned executable hashes, alternating app order and separate first-frame/editable/full-load timings.
- [ ] **AC-019-02:** Measure 100 and 500 tabs plus extension process memory; report peak total private bytes, disk growth and downloads separately.
- [ ] **AC-019-03:** Publish all raw runs, sample counts, P50/P95 and timeouts; hosted CI noise never becomes a comparative marketing claim.

### Minimum scenarios

- [ ] **MV-019-01:** Benchmark runner can execute twice and produce comparable schema/data.
- [ ] **MV-019-02:** 5 GB open does not allocate 5 GB nor perform full syntax/line scan before first viewport.
- [ ] **MV-019-03:** Search and indexing cancellation meet bounded acknowledgement target.
- [ ] **MV-019-04:** No >100 ms UI-thread stalls during defined 1 GB scroll/search workload unless OS-level external event is documented.
- [ ] **MV-019-05:** Publish side-by-side raw Bareline/Notepad++ baseline table with methodology, not marketing-only percentages.

## QUAL-020 — Security, recovery and update hardening

[Original brief](../../../docs/blueprint/PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-020-01:** Inject disk-full and process death at every recovery/replace receipt transition; reconcile originals, backups and committed targets by fingerprint.
- [ ] **AC-020-02:** Test archive traversal, reparse escapes, IPC spoofing, stale signed metadata and compromised release-key recovery.
- [ ] **AC-020-03:** Verify diagnostics contain no document text or sensitive path data and no telemetry upload occurs.

### Minimum scenarios

- [ ] **MV-020-01:** Corpus fuzz session/UDL/extension package/RPC parsers without crash/OOM.
- [ ] **MV-020-02:** Zip-slip, symlink/reparse, absolute/device-path tests cannot escape staging/install roots.
- [ ] **MV-020-03:** UNC-in-session startup produces zero remote access until explicitly trusted.
- [ ] **MV-020-04:** Simulated package swap between verify/apply is detected.
- [ ] **MV-020-05:** Compromised extension cannot directly mutate editor memory/file without broker permission.
- [ ] **MV-020-06:** Security test matrix maps each known Notepad++ issue class researched in baseline to a Bareline control/regression test.
- [ ] **MV-020-07:** Two clean CI builds of the same commit produce identical artifact hashes, or every difference is traced to a documented nondeterminism source.
- [ ] **MV-020-08:** Minisign verification rejects altered payloads, wrong keys and revoked keys; a key rotation dry run succeeds end to end.
- [ ] **MV-020-09:** Sentinel strings placed in open documents never appear in crash files or security logs.
- [ ] **MV-020-10:** Truncating a Paged source externally during an edit yields `SourceChanged`, no crash and no foreign bytes in the buffer.

## QUAL-021 — Final product workflows and readiness

[Original brief](../../../docs/blueprint/PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-021-01:** Resolve every atomic case and every registered command to an implementation commit and independent result; exclusions retain rationale.
- [ ] **AC-021-02:** Run the key workflows with keyboard and screen reader on both Windows floor builds and mixed DPI.
- [ ] **AC-021-03:** Publish release readiness with unresolved correctness items, performance measurements and limited parity claims; mockups never count as runtime evidence.

### Minimum scenarios

- [ ] **MV-021-01:** Full keyboard-only smoke pass and screen-reader/high-contrast/DPI pass.
- [ ] **MV-021-02:** 72-hour soak with representative open tabs/tail/search/extensions disabled/enabled does not show unbounded leak.
- [ ] **MV-021-03:** Abrupt termination during edit/save/update cases recovers or preserves original as designed.
- [ ] **MV-021-04:** Clean VM install/uninstall leaves no unwanted associations/services/tasks.
- [ ] **MV-021-05:** Parity matrix has no unexplained missing core row.
- [ ] **MV-021-06:** Every artifact in the GitHub Release has a `SHA-256SUMS` entry and the minisign signature verifies against the embedded public key.
- [ ] **MV-021-07:** Fresh clone plus `cargo build --locked --release` on the pinned toolchain reproduces the released binaries or the SBOM documents every deviation.
- [ ] **MV-021-08:** Controller marks `Accepted=DONE` only after independent verification/test evidence is attached.

## QUAL-022 — Portable contracts and foreign-host probes

[Original brief](../../../docs/blueprint/PRs/PR-022_CROSS-PLATFORM_READINESS_AND_LINUX_MACOS_ADAPTER_SKELETONS.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-022-01:** Compile neutral crates without Windows imports on Linux/macOS; platform gaps return explicit Unsupported.
- [ ] **AC-022-02:** Round-trip unpaired Windows UTF-16 and Unix non-UTF-8 path encodings through versioned state without lossy identity conversion.
- [ ] **AC-022-03:** Run RecordingBackend tests on each OS and record separate real-platform readiness rather than claiming complete ports.

### Minimum scenarios

- [ ] **MV-022-01:** Neutral crate graph builds/tests on all three OSes.
- [ ] **MV-022-02:** Public APIs contain no HWND/HANDLE/DirectWrite/UTF-16-Windows-path assumptions.
- [ ] **MV-022-03:** The shared UI test suite runs on Linux and macOS CI against `RecordingBackend`; a trivial Linux/macOS winit window can drive the shared `UiEvent` and render contract in an example target.
- [ ] **MV-022-04:** Windows-specific feature addition intentionally fails architecture check if imported into neutral crate.
- [ ] **MV-022-05:** Porting checklist names every `PlatformServices` function still unsupported on each OS.

## QUAL-023 — Controls, focus and UI primitives

[Original brief](../../../docs/blueprint/PRs/PR-023_UI_PRIMITIVES_AND_CONTROLS.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-023-01:** Navigate every control with Tab/Shift+Tab/arrows; focus is visible and disabled controls do not dispatch edits.
- [ ] **AC-023-02:** Use IME, clipboard and validation in text fields; cancellation returns focus without leaking preedit into undo.
- [ ] **AC-023-03:** At 100/125/150/200/250% DPI, hit targets align with paint and popovers stay on-screen.

### Minimum scenarios

- [ ] **MV-023-01:** RecordingBackend golden tests for every control in light, dark and high-contrast at 100, 150, 200 and 300 percent scale.
- [ ] **MV-023-02:** Text field IME preedit then commit produces exactly one value change; preedit cancel produces none.
- [ ] **MV-023-03:** Text field handles dead keys and AltGr on a German and a US-International layout without triggering shortcuts.
- [ ] **MV-023-04:** Virtual list over a 1M-item source keeps constant memory and completes a 60 Hz scroll workload in the recording backend without laying out invisible items.
- [ ] **MV-023-05:** Virtual tree over a lazily enumerated 1M-node fixture enumerates only expanded levels.
- [ ] **MV-023-06:** Scrollbar with unknown total refines its thumb as the source reports more items, with no jump larger than one thumb height per update.
- [ ] **MV-023-07:** Keyboard traversal order test across a form of every control type matches the declared focus chain.
- [ ] **MV-023-08:** Popover flips side when anchored near the window edge and returns focus to the anchor on dismiss.

## QUAL-024 — Accessibility, UIA and screen readers

[Original brief](../../../docs/blueprint/PRs/PR-024_ACCESSIBILITY_AND_UI_AUTOMATION.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-024-01:** Use Narrator through open, edit, find, conflict and recovery; names, roles, values and focused state are announced.
- [ ] **AC-024-02:** Change selection/scroll in a 5 GiB file; semantic text-range requests are bounded and do not materialize the entire document.
- [ ] **AC-024-03:** Enable high contrast and test focus/selection/error distinction without color; real DirectWrite results supplement headless tests.

### Minimum scenarios

- [ ] **MV-024-01:** Narrator reads tab names with dirty and pinned state, menu items, palette results, settings rows with their keys, and the caret line and column.
- [ ] **MV-024-02:** NVDA announces search result count changes and banner text through live regions.
- [ ] **MV-024-03:** Keyboard-only smoke pass reaches every FR-030 surface: menus, tabs, sidebar tree, bottom panel, settings, palette, find bar, status bar.
- [ ] **MV-024-04:** Text pattern on a 1 GB document returns only bounded ranges and never triggers a full line-index scan.
- [ ] **MV-024-05:** Focus returns to the anchor after closing a popover, palette or dialog in every case.
- [ ] **MV-024-06:** Semantic tree JSON golden test for the default layout and for a layout with all panels open.
- [ ] **MV-024-07:** High-contrast theme renders every surface with readable text and a visible focus ring.

## QUAL-025 — Diff engine correctness and bounds

[Original brief](../../../docs/blueprint/PRs/PR-025_DIFF_CORE_ENGINE.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-025-01:** With all ignore options off, applying all hunks reproduces the target bytes; with ignores on, only explicit nonignored ranges change.
- [ ] **AC-025-02:** Force normalized-hash collisions and whitespace/case equality; compare normalized bytes, retaining original byte ranges for edits.
- [ ] **AC-025-03:** Compare divergent multi-GB files under anchor/output caps; distinguish CompletedCoarse, Cancelled and Unavailable without claiming incomplete output is exact.

### Minimum scenarios

- [ ] **MV-025-01:** Golden fixtures for insert, delete, change, move-like alignment, Unicode (combining marks, emoji, CJK), blank-line handling and every ignore option, each with expected hunks and intraline spans.
- [ ] **MV-025-02:** Property test: with ignore options off, applying all right hunks yields right bytes; with ignores on, only selected nonignored ranges change. Both are undoable.
- [ ] **MV-025-03:** Stale-result test: edit one side during a background compare and prove the result is rejected by `is_stale`.
- [ ] **MV-025-04:** Multi-GB generated fixture pair diffs within bounded memory, cancels within 50 ms of the token being set, and reports `Coarse` with a reason.
- [ ] **MV-025-05:** Highly divergent 200 MB pair (no anchors) completes as a single Changed block without quadratic time.
- [ ] **MV-025-06:** Stable hunk IDs survive an unrelated edit elsewhere in the document.
- [ ] **MV-025-07:** `xtask perf` records diff throughput and peak memory for the 10 MB and 1 GB pairs.

## QUAL-026 — Workspace replacement and durable rollback

[Original brief](../../../docs/blueprint/PRs/PR-026_REPLACE_IN_FILES_AND_WORKSPACE_REPLACE.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-026-01:** Preview then externally modify a file; Apply skips it for re-review instead of re-matching and changing unseen contents.
- [ ] **AC-026-02:** Replace in a mix of dirty open files and disk files; revisions/fingerprints are checked, backups unique and per-file receipts durable.
- [ ] **AC-026-03:** Kill between replacement and receipt commit; restart reconciles by target/backup fingerprints and never reapplies blindly.

### Minimum scenarios

- [ ] **MV-026-01:** Interrupt a workspace replace over 10,000 generated files midway; already-replaced files are valid, untouched files are byte-identical, and the summary is exact.
- [ ] **MV-026-02:** Replace across 100 open documents produces exactly one undo record per document and reports partial failures when two documents are made read-only mid-run.
- [ ] **MV-026-03:** Windows-1252 and UTF-16 LE files with mixed EOL are replaced and round-trip encoding, BOM and EOL byte-identically outside the replaced spans.
- [ ] **MV-026-04:** Preview shows every change before any write; unchecking a file or a match excludes it from Phase 2.
- [ ] **MV-026-05:** Regex replacement with capture groups and case-preserving option produces expected output on fixtures.
- [ ] **MV-026-06:** Binary file in scope is skipped by default and included only with the explicit option.
- [ ] **MV-026-07:** Disk-full injected during one file leaves that file untouched and continues with the rest, reporting the failure.

## QUAL-027 — First-party JSON, XML and Hex extensions

[Original brief](../../../docs/blueprint/PRs/PR-027_FIRST-PARTY_EXTENSIONS_JSON_XML_AND_HEX.md). **Status:** candidate evidence pending.

### Atomic acceptance

- [ ] **AC-027-01:** Format JSON containing large integers and exponent lexemes; preserve numeric values and documented lexical policy.
- [ ] **AC-027-02:** Reject XML DTD/external entities/XInclude and unsupported XPath syntax; network stays inaccessible and depth/time limits hold.
- [ ] **AC-027-03:** Open Hex original-byte mode on UTF-16 with unsaved text edits; display original generation explicitly and distinguish an encoded edited preview.

### Minimum scenarios

- [ ] **MV-027-01:** JSON and XML format, minify and validate golden tests on fixture corpora including deeply nested, Unicode and malformed inputs with expected error positions.
- [ ] **MV-027-02:** 1 GB generated JSON is validated with bounded memory through snapshot ranges; the tree panel expands the root without materializing the whole document.
- [ ] **MV-027-03:** Hex view over the 5 GB fixture reads only the visible range and goto offset is immediate.
- [ ] **MV-027-04:** Killing the extension host while a formatter is running leaves the editor and document intact; the pending `ApplyEdits` is rejected as stale or never arrives.
- [ ] **MV-027-05:** Catalog install path end to end: fetch signed index, verify, download `.blex`, verify hash, install, enable, run a command.
- [ ] **MV-027-06:** Sideload of a `.blex` with a modified hash is refused with a clear message.
- [ ] **MV-027-07:** A new author follows the walkthrough in a clean environment and produces a working extension; record the elapsed time in the PR evidence.

## Capability family coverage

| ID | Capability | Work/disposition |
|---|---|---|
| parity-001 | New/Open/Save/Save As/Save All/Save Copy | QUAL-004; qualify or repair |
| parity-002 | Reload from disk, Set read-only | QUAL-004; qualify or repair |
| parity-003 | Recent files / tabs / pin / sort / document switcher | QUAL-004, QUAL-010; qualify or repair |
| parity-004 | Restore last closed tab | QUAL-004; qualify or repair |
| parity-005 | Dual view / clone / synchronized scroll | QUAL-010; qualify or repair |
| parity-006 | Move to new instance | QUAL-018; qualify or repair |
| parity-007 | Undo/redo/cut/copy/paste/line editing | QUAL-003, QUAL-006; qualify or repair |
| parity-008 | Paste Special, drag-and-drop text | QUAL-006; qualify or repair |
| parity-009 | Hide Lines | QUAL-006; qualify or repair |
| parity-010 | Clipboard History panel | QUAL-006; qualify or repair |
| parity-011 | Column mode / multi-editing | QUAL-003, QUAL-006; qualify or repair |
| parity-012 | Column Editor (number sequences) | QUAL-006; qualify or repair |
| parity-013 | Find/replace/count/mark/find in files | QUAL-005; qualify or repair |
| parity-014 | Replace in files / projects | QUAL-026; qualify or repair |
| parity-015 | Mark with style tokens 1–5 | QUAL-005; qualify or repair |
| parity-016 | Go to / bookmarks / brace match | QUAL-006, QUAL-013; qualify or repair |
| parity-017 | Encoding/character sets/EOL conversion | QUAL-007; qualify or repair |
| parity-018 | Syntax highlighting / folding | QUAL-008; qualify or repair |
| parity-019 | Fold All / Unfold All / Fold Level N | QUAL-008; qualify or repair |
| parity-020 | User Defined Language | QUAL-008; qualify or repair |
| parity-021 | Auto-completion/function hints | QUAL-013; qualify or repair |
| parity-022 | Word wrap/symbols/guides/zoom/fullscreen/always-on-top | QUAL-003, QUAL-012; qualify or repair |
| parity-023 | Document map | QUAL-009; qualify or repair |
| parity-024 | Function List | QUAL-009; qualify or repair |
| parity-025 | Folder as Workspace / projects | QUAL-009; qualify or repair |
| parity-026 | Sessions / workspace restore / backups | QUAL-004; qualify or repair |
| parity-027 | Monitoring (tail -f) | QUAL-015; qualify or repair |
| parity-028 | Macros | QUAL-014; qualify or repair |
| parity-029 | Ghost/replay typing | QUAL-014; qualify or repair |
| parity-030 | Run external commands | QUAL-014; qualify or repair |
| parity-031 | Hash tools | QUAL-017; qualify or repair |
| parity-032 | Print / export | QUAL-017; qualify or repair |
| parity-033 | Compare (N++ plugin) | QUAL-017, QUAL-025; qualify or repair |
| parity-034 | Compare inline view, folder compare | REL-009; retain scope decision |
| parity-035 | Plugins / Plugins Admin | QUAL-016; qualify or repair |
| parity-036 | Seed XML/JSON/Hex capabilities (not ecosystem breadth) | QUAL-027; qualify or repair |
| parity-037 | Spell check, CSV tools, LSP client | REL-009; retain scope decision |
| parity-038 | Preferences / Style Configurator / Shortcut Mapper | QUAL-011, QUAL-012; qualify or repair |
| parity-039 | Localization (N++ ships ~90 languages) | REL-009; retain scope decision |
| parity-040 | Command line args / multi-instance | QUAL-018; qualify or repair |
| parity-041 | Explorer “Edit with…” / file associations | QUAL-018; qualify or repair |
| parity-042 | Replace Windows Notepad | REL-009; retain scope decision |
| parity-043 | System tray | QUAL-018; qualify or repair |
| parity-044 | Update / debug info | QUAL-018, QUAL-020; qualify or repair |
| parity-045 | Portable package | QUAL-018; qualify or repair |
| parity-046 | Character Panel | REL-009; retain scope decision |
| parity-047 | Post-It mode (frameless) | REL-009; retain scope decision |
| parity-048 | Document Peeker (tab hover preview) | REL-009; retain scope decision |
| parity-049 | Local history (not in N++) | REL-009; retain scope decision |
| parity-050 | Integrated terminal (not in N++) | REL-009; retain scope decision |
| parity-051 | Source control panel (not in N++) | REL-009; retain scope decision |
| parity-052 | Telemetry (not in N++) | REL-009; retain scope decision |

Scope authorities and exact current limitations remain in [the capability ledger](../../parity/index.json). The seven deferred and four excluded rows are preserved, not new development tasks.

## Functional requirement ownership

All 31 numbered FR families plus FR-021A are assigned from the acceptance authority.

| Requirement | Qualification owners |
|---|---|
| FR-001 | QUAL-004, QUAL-019, QUAL-021 |
| FR-002 | QUAL-004 |
| FR-003 | QUAL-004, QUAL-010 |
| FR-004 | QUAL-010 |
| FR-005 | QUAL-002, QUAL-003, QUAL-006 |
| FR-006 | QUAL-003, QUAL-006 |
| FR-007 | QUAL-005, QUAL-019, QUAL-026 |
| FR-008 | QUAL-006, QUAL-013 |
| FR-009 | QUAL-002, QUAL-007 |
| FR-010 | QUAL-008 |
| FR-011 | QUAL-008 |
| FR-012 | QUAL-013 |
| FR-013 | QUAL-003, QUAL-012 |
| FR-014 | QUAL-009 |
| FR-015 | QUAL-009 |
| FR-016 | QUAL-009 |
| FR-017 | QUAL-004, QUAL-020 |
| FR-018 | QUAL-015, QUAL-019 |
| FR-019 | QUAL-014 |
| FR-020 | QUAL-014 |
| FR-021 | QUAL-017 |
| FR-021A | QUAL-017, QUAL-025 |
| FR-022 | QUAL-016, QUAL-020, QUAL-027 |
| FR-023 | QUAL-011, QUAL-012, QUAL-023 |
| FR-024 | QUAL-012 |
| FR-025 | QUAL-018 |
| FR-026 | QUAL-018 |
| FR-027 | QUAL-018, QUAL-020 |
| FR-028 | QUAL-018 |
| FR-029 | QUAL-020 |
| FR-030 | QUAL-001, QUAL-003, QUAL-022, QUAL-023, QUAL-024 |
| FR-031 | QUAL-001, QUAL-018, QUAL-020, QUAL-021, QUAL-022 |

## Runtime command outcome inventory

[COMMAND_COVERAGE.json](COMMAND_COVERAGE.json) enumerates all 499 historical command IDs and all 1,497 success/disabled/failure outcomes. These are discovery entries, not final release evidence. DEV-004 must refresh from the actual configured runtime and assign complete procedures, including verified dynamic extension contributions.
