# Bareline Acceptance and Traceability

**Version:** 1.2 • 2026-09-05 • **Evidence state: NOT_STARTED**

This register adds 81 review-driven cases to the minimum scenarios already in each PR brief. It does not claim these 81 cases exhaust every product command. PR-021 must enumerate the implemented command registry and add one success, disabled-state and failure-path case for each command before acceptance. No code, compile, benchmark or user test has run in this blueprint package.

Each result records case ID, fixture/hash, implementation commit, Windows/build and hardware, test command or manual steps, observed output, reviewer and issue for failure. A case is not passed merely because its requirement is written. Performance results remain informational; correctness and data integrity still require evidence.

## Requirement ownership

| Requirement | Primary case families / PR owners |
|---|---|
| FR-001 | [PR-004](PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md) / AC-004-01–03; [PR-019](PRs/PR-019_HUGE-FILE_OPTIMIZATION_AND_PERFORMANCE_BENCHMARK_SUITE.md) / AC-019-01–03; [PR-021](PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md) / AC-021-01–03 |
| FR-002 | [PR-004](PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md) / AC-004-01–03 |
| FR-003 | [PR-004](PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md) / AC-004-01–03; [PR-010](PRs/PR-010_SPLIT_VIEWS_TAB_MANAGEMENT_AND_SYNCHRONIZED_SCROLLING.md) / AC-010-01–03 |
| FR-004 | [PR-010](PRs/PR-010_SPLIT_VIEWS_TAB_MANAGEMENT_AND_SYNCHRONIZED_SCROLLING.md) / AC-010-01–03 |
| FR-005 | [PR-002](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md) / AC-002-01–03; [PR-003](PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md) / AC-003-01–03; [PR-006](PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md) / AC-006-01–03 |
| FR-006 | [PR-003](PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md) / AC-003-01–03; [PR-006](PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md) / AC-006-01–03 |
| FR-007 | [PR-005](PRs/PR-005_SEARCH_REPLACE_AND_RESULTS_ENGINE.md) / AC-005-01–03; [PR-019](PRs/PR-019_HUGE-FILE_OPTIMIZATION_AND_PERFORMANCE_BENCHMARK_SUITE.md) / AC-019-01–03; [PR-026](PRs/PR-026_REPLACE_IN_FILES_AND_WORKSPACE_REPLACE.md) / AC-026-01–03 |
| FR-008 | [PR-006](PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md) / AC-006-01–03; [PR-013](PRs/PR-013_COMPLETION_SMART_TYPING_AND_LANGUAGE-AWARE_EDITING.md) / AC-013-01–03 |
| FR-009 | [PR-002](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md) / AC-002-01–03; [PR-007](PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md) / AC-007-01–03 |
| FR-010 | [PR-008](PRs/PR-008_LANGUAGE_CATALOG_SYNTAX_HIGHLIGHTING_FOLDING_AND_UDL.md) / AC-008-01–03 |
| FR-011 | [PR-008](PRs/PR-008_LANGUAGE_CATALOG_SYNTAX_HIGHLIGHTING_FOLDING_AND_UDL.md) / AC-008-01–03 |
| FR-012 | [PR-013](PRs/PR-013_COMPLETION_SMART_TYPING_AND_LANGUAGE-AWARE_EDITING.md) / AC-013-01–03 |
| FR-013 | [PR-003](PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md) / AC-003-01–03; [PR-012](PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md) / AC-012-01–03 |
| FR-014 | [PR-009](PRs/PR-009_WORKSPACE_EXPLORER_DOCUMENT_LIST_OUTLINE_AND_DOCUMENT_MAP.md) / AC-009-01–03 |
| FR-015 | [PR-009](PRs/PR-009_WORKSPACE_EXPLORER_DOCUMENT_LIST_OUTLINE_AND_DOCUMENT_MAP.md) / AC-009-01–03 |
| FR-016 | [PR-009](PRs/PR-009_WORKSPACE_EXPLORER_DOCUMENT_LIST_OUTLINE_AND_DOCUMENT_MAP.md) / AC-009-01–03 |
| FR-017 | [PR-004](PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md) / AC-004-01–03; [PR-020](PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md) / AC-020-01–03 |
| FR-018 | [PR-015](PRs/PR-015_FILE_WATCHING_MONITORING_TAIL_AND_SAFE_REMOTE_PATHS.md) / AC-015-01–03; [PR-019](PRs/PR-019_HUGE-FILE_OPTIMIZATION_AND_PERFORMANCE_BENCHMARK_SUITE.md) / AC-019-01–03 |
| FR-019 | [PR-014](PRs/PR-014_MACROS_EXTERNAL_COMMANDS_AND_OUTPUT_PANEL.md) / AC-014-01–03 |
| FR-020 | [PR-014](PRs/PR-014_MACROS_EXTERNAL_COMMANDS_AND_OUTPUT_PANEL.md) / AC-014-01–03 |
| FR-021 | [PR-017](PRs/PR-017_BUILT-IN_UTILITIES_COMPARE_FOUNDATION_EXPORT_AND_PRINT.md) / AC-017-01–03 |
| FR-021A | [PR-017](PRs/PR-017_BUILT-IN_UTILITIES_COMPARE_FOUNDATION_EXPORT_AND_PRINT.md) / AC-017-01–03; [PR-025](PRs/PR-025_DIFF_CORE_ENGINE.md) / AC-025-01–03 |
| FR-022 | [PR-016](PRs/PR-016_EXTENSION_PLATFORM_AND_ISOLATED_PLUGIN_MANAGER.md) / AC-016-01–03; [PR-020](PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md) / AC-020-01–03; [PR-027](PRs/PR-027_FIRST-PARTY_EXTENSIONS_JSON_XML_AND_HEX.md) / AC-027-01–03 |
| FR-023 | [PR-011](PRs/PR-011_MENUS_COMMAND_PALETTE_SHORTCUTS_TOOLBAR_AND_CONTEXT_ACTIONS.md) / AC-011-01–03; [PR-012](PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md) / AC-012-01–03; [PR-023](PRs/PR-023_UI_PRIMITIVES_AND_CONTROLS.md) / AC-023-01–03 |
| FR-024 | [PR-012](PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md) / AC-012-01–03 |
| FR-025 | [PR-018](PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md) / AC-018-01–03 |
| FR-026 | [PR-018](PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md) / AC-018-01–03 |
| FR-027 | [PR-018](PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md) / AC-018-01–03; [PR-020](PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md) / AC-020-01–03 |
| FR-028 | [PR-018](PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md) / AC-018-01–03 |
| FR-029 | [PR-020](PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md) / AC-020-01–03 |
| FR-030 | [PR-001](PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md) / AC-001-01–03; [PR-003](PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md) / AC-003-01–03; [PR-022](PRs/PR-022_CROSS-PLATFORM_READINESS_AND_LINUX_MACOS_ADAPTER_SKELETONS.md) / AC-022-01–03; [PR-023](PRs/PR-023_UI_PRIMITIVES_AND_CONTROLS.md) / AC-023-01–03; [PR-024](PRs/PR-024_ACCESSIBILITY_AND_UI_AUTOMATION.md) / AC-024-01–03 |
| FR-031 | [PR-001](PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md) / AC-001-01–03; [PR-018](PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md) / AC-018-01–03; [PR-020](PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md) / AC-020-01–03; [PR-021](PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md) / AC-021-01–03; [PR-022](PRs/PR-022_CROSS-PLATFORM_READINESS_AND_LINUX_MACOS_ADAPTER_SKELETONS.md) / AC-022-01–03 |

## Early uncertainty reduction

PR-001 prototypes real DirectWrite shaping, IME candidate placement, mixed BiDi, device loss and the initial AccessKit path. PR-002 proves opaque-span offsets and recovery ownership before UI polish. PR-005 proves cross-window PCRE2 semantics before destructive replacement. PR-018 rehearses offline signing and trust recovery before publishing. Record prototype failures as contract changes in the ADR log; do not defer these uncertainties to release QA.

## PR-001 acceptance cases

Owner: [implementation brief](PRs/PR-001_WORKSPACE_SCAFFOLD_COMMAND_CORE_AND_WINDOWS_SHELL.md). Contracts: FC-06, FC-07, FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-001-01 | Render mixed Arabic/Latin with emoji and IME composition; candidate window follows caret at 100%, 150% and 200% DPI. |
| AC-001-02 | Open 5,000 lazy tabs; worker count remains bounded and no document reads occur before the first frame. |
| AC-001-03 | Verify codec, filesystem-capability and VerifiedPackageSource interfaces compile with fake providers; online provider fails closed. |

## PR-002 acceptance cases

Owner: [implementation brief](PRs/PR-002_PAGED_DOCUMENT_ENGINE_AND_CORE_UNDO.md). Contracts: FC-01, FC-02, FC-03, FC-06. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-002-01 | Generate random CR/LF/CRLF edits, including separators split across pieces; line counts match the reference model. |
| AC-002-02 | Delete a range, evict its base pages and externally replace the source; undo restores owned inverse bytes exactly. |
| AC-002-03 | Race a Resident background fill with truncation; missing old-generation bytes return Unavailable and never new contents. |

## PR-003 acceptance cases

Owner: [implementation brief](PRs/PR-003_VIRTUALIZED_EDITOR_SURFACE_INPUT_AND_SELECTION_MODEL.md). Contracts: FC-01, FC-02, FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-003-01 | Select across emoji clusters, combining accents and opaque byte markers; text edits never split their atomic boundaries. |
| AC-003-02 | Scroll a 20 MiB single line; layout is bounded and cancellation leaves input responsive. |
| AC-003-03 | Commit and cancel IME preedit; only committed text enters undo and recovery; verify real Windows shaping and BiDi hit testing. |

## PR-004 acceptance cases

Owner: [implementation brief](PRs/PR-004_FILE_LIFECYCLE_TABS_SESSIONS_AND_CRASH_RECOVERY.md). Contracts: FC-03, FC-04. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-004-01 | Edit, save, edit again and undo to saved ContentStateId; the dirty marker clears without comparing monotonic revisions. |
| AC-004-02 | Kill the process after each segment/journal/checkpoint flush boundary; recovery stops at the last valid durable record. |
| AC-004-03 | Start a save then type; successful save clears only its captured state. Missing source regions disable Save/Save As and offer export with gaps. |

## PR-005 acceptance cases

Owner: [implementation brief](PRs/PR-005_SEARCH_REPLACE_AND_RESULTS_ENGINE.md). Contracts: FC-05, FC-06. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-005-01 | Match across a 16 MiB window in a 20 MiB line; case-fold results map to correct original TextOffsets. |
| AC-005-02 | Exercise lookbehind, anchors, empty matches and partial regex contexts; unsupported cases report incomplete and disable replacement. |
| AC-005-03 | Replace in two dirty open documents while one revision changes; prepare aborts before either mutates; successful linked undo reverses both. |

## PR-006 acceptance cases

Owner: [implementation brief](PRs/PR-006_POWER_EDITING_MULTI-CURSOR_COLUMN_LINES_AND_BOOKMARKS.md). Contracts: FC-01, FC-02, FC-06. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-006-01 | Column insertion across tabs, wide glyphs and short lines follows display columns and undoes in one transaction. |
| AC-006-02 | Enable clipboard history then exceed 20 entries and 16 MiB total; bounded eviction occurs and history never persists to disk. |
| AC-006-03 | Invoke comment commands before PR-013; show unavailable with a reason and do not mutate text. |

## PR-007 acceptance cases

Owner: [implementation brief](PRs/PR-007_ENCODING_EOL_AND_UNICODE_FIDELITY.md). Contracts: FC-01, FC-02, FC-04. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-007-01 | Round-trip invalid UTF-8, genuine U+FFFD/private-use characters and noncanonical legacy sequences without provenance collisions. |
| AC-007-02 | Open/save UTF-16LE/BE, UTF-32LE/BE and true Latin-1 fixtures with BOM and mixed EOL; exact unedited bytes survive. |
| AC-007-03 | Transcode a generated 5 GiB UTF-16 file under the RAM budget; disk quota exhaustion is visible and original data remains intact. |

## PR-008 acceptance cases

Owner: [implementation brief](PRs/PR-008_LANGUAGE_CATALOG_SYNTAX_HIGHLIGHTING_FOLDING_AND_UDL.md). Contracts: FC-01, FC-02, FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-008-01 | Validate paired Lexilla/native definitions for all 15 languages listed in FC-09; publish coverage and known grammar gaps. |
| AC-008-02 | Open in the middle of a multiline string; provisional highlighting is labelled until a valid lexer checkpoint resolves context. |
| AC-008-03 | Import malformed/oversized UDL input; reject unsafe structure and preserve the previous definition. |

## PR-009 acceptance cases

Owner: [implementation brief](PRs/PR-009_WORKSPACE_EXPLORER_DOCUMENT_LIST_OUTLINE_AND_DOCUMENT_MAP.md). Contracts: FC-02, FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-009-01 | Select main.rs then Cargo.toml; outline changes language and old-revision results never appear in the new document. |
| AC-009-02 | Open a folder with symlink cycles and inaccessible descendants; traversal is bounded and shows partial results. |
| AC-009-03 | Scroll before line indexing completes; minimap and outline use estimates explicitly and do not allocate per-line objects. |

## PR-010 acceptance cases

Owner: [implementation brief](PRs/PR-010_SPLIT_VIEWS_TAB_MANAGEMENT_AND_SYNCHRONIZED_SCROLLING.md). Contracts: FC-06. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-010-01 | Pin a tab and reorder across views; pinned-first default and per-view selection persist after restore. |
| AC-010-02 | Add a compare alignment spacer; it receives no document line number and synchronized scrolling maps logical lines correctly. |
| AC-010-03 | Clone a document in a second pane; edits share a document while caret, selection, folds and scroll remain independent. |

## PR-011 acceptance cases

Owner: [implementation brief](PRs/PR-011_MENUS_COMMAND_PALETTE_SHORTCUTS_TOOLBAR_AND_CONTEXT_ACTIONS.md). Contracts: FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-011-01 | Every menu/toolbar/palette action resolves to one stable ID; unavailable actions have a reason and no handler mutation. |
| AC-011-02 | Press Escape in nested popup, panel and editor states; only the topmost transient layer closes and focus returns predictably. |
| AC-011-03 | Test AltGr text, menu mnemonics and remapped shortcuts together; text entry is not consumed as a global shortcut. |

## PR-012 acceptance cases

Owner: [implementation brief](PRs/PR-012_SETTINGS_THEMES_LOCALIZATION_DPI_AND_ACCESSIBILITY.md). Contracts: FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-012-01 | Change font size to 12 pt at mixed DPI; preview, persisted value and restored editor use the same point unit. |
| AC-012-02 | Attempt workspace override of execution, network or extension grants; reject it while accepting allowed indentation preferences. |
| AC-012-03 | Check Light/Dark/System tokens and override persistence; normal text meets 4.5:1 and focus indicators 3:1 on actual surfaces. |

## PR-013 acceptance cases

Owner: [implementation brief](PRs/PR-013_COMPLETION_SMART_TYPING_AND_LANGUAGE-AWARE_EDITING.md). Contracts: FC-01, FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-013-01 | Register PR-006 CommentProvider with a language; line/block toggles handle empty selections and undo correctly. |
| AC-013-02 | Type while completion computes; old-generation/revision suggestions cannot replace newer text. |
| AC-013-03 | Complete within strings and comments using bounded local indexes; suppress semantically unsupported suggestions. |

## PR-014 acceptance cases

Owner: [implementation brief](PRs/PR-014_MACROS_EXTERNAL_COMMANDS_AND_OUTPUT_PANEL.md). Contracts: FC-06, FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-014-01 | Record/replay only stable command IDs and explicit text; playback failure stops with a resumable location. |
| AC-014-02 | Run a path and argument containing spaces, quotes and shell metacharacters using direct spawn; no implicit shell interpretation occurs. |
| AC-014-03 | Cancel a process producing unlimited output; output storage stays bounded, process cleanup completes and the editor remains usable. |

## PR-015 acceptance cases

Owner: [implementation brief](PRs/PR-015_FILE_WATCHING_MONITORING_TAIL_AND_SAFE_REMOTE_PATHS.md). Contracts: FC-02, FC-04. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-015-01 | Append to a log with a partial final page; verify continuity before accepting the new generation, without falsely entering SourceChanged. |
| AC-015-02 | Rotate, truncate and rewrite same-size content; uncertain continuity becomes SourceChanged and preserves owned edits. |
| AC-015-03 | Pause tail scrolling while appends continue; unlock stops following and captures a fixed generation after confirmation. |

## PR-016 acceptance cases

Owner: [implementation brief](PRs/PR-016_EXTENSION_PLATFORM_AND_ISOLATED_PLUGIN_MANAGER.md). Contracts: FC-07, FC-08. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-016-01 | Install a signed offline runtime and extension using the PR-001 provider contract; no PR-018 online code is required. |
| AC-016-02 | Disable all extensions; host exits and installed files remain. Explicit Remove runtime deletes only its verified owned files. |
| AC-016-03 | Send oversized frames, unauthorized capabilities and a CPU loop; reject/terminate the offending extension within per-extension limits. |

## PR-017 acceptance cases

Owner: [implementation brief](PRs/PR-017_BUILT-IN_UTILITIES_COMPARE_FOUNDATION_EXPORT_AND_PRINT.md). Contracts: FC-04, FC-06. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-017-01 | Copy a hunk left-to-right then undo; source labels, dirty state and selection match the actual target revision. |
| AC-017-02 | Choose ignore options and recompare; cancellation is distinct from a completed coarse result and stale merge buttons disable. |
| AC-017-03 | Print/export Unicode with syntax/theme options; unsupported printer/filesystem errors preserve the original and show a clear retry path. |

## PR-018 acceptance cases

Owner: [implementation brief](PRs/PR-018_WINDOWS_INTEGRATION_CLI_INSTALLER_PORTABLE_MODE_TRAY_AND_UPDATER.md). Contracts: FC-04, FC-07, FC-08, FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-018-01 | Reject an expired, rollback, wrong-channel, wrong-publisher or hash-mismatched update before staging application files. |
| AC-018-02 | Verify signatures on inner executables after extracting final ZIP/installer; compare reproducibility only before signing. |
| AC-018-03 | Launch CLI with non-BMP paths and arguments via authenticated same-user IPC; portable mode writes only to its configured data root. |

## PR-019 acceptance cases

Owner: [implementation brief](PRs/PR-019_HUGE-FILE_OPTIMIZATION_AND_PERFORMANCE_BENCHMARK_SUITE.md). Contracts: FC-02, FC-06, FC-10. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-019-01 | Run paired baseline trials with pinned executable hashes, alternating app order and separate first-frame/editable/full-load timings. |
| AC-019-02 | Measure 100 and 500 tabs plus extension process memory; report peak total private bytes, disk growth and downloads separately. |
| AC-019-03 | Publish all raw runs, sample counts, P50/P95 and timeouts; hosted CI noise never becomes a comparative marketing claim. |

## PR-020 acceptance cases

Owner: [implementation brief](PRs/PR-020_SECURITY_RECOVERY_AND_UPDATE_HARDENING.md). Contracts: FC-03, FC-04, FC-07, FC-08. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-020-01 | Inject disk-full and process death at every recovery/replace receipt transition; reconcile originals, backups and committed targets by fingerprint. |
| AC-020-02 | Test archive traversal, reparse escapes, IPC spoofing, stale signed metadata and compromised release-key recovery. |
| AC-020-03 | Verify diagnostics contain no document text or sensitive path data and no telemetry upload occurs. |

## PR-021 acceptance cases

Owner: [implementation brief](PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md). Contracts: FC-01 through FC-10. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-021-01 | Resolve every atomic case and every registered command to an implementation commit and independent result; exclusions retain rationale. |
| AC-021-02 | Run the key workflows with keyboard and screen reader on both Windows floor builds and mixed DPI. |
| AC-021-03 | Publish release readiness with unresolved correctness items, performance measurements and limited parity claims; mockups never count as runtime evidence. |

## PR-022 acceptance cases

Owner: [implementation brief](PRs/PR-022_CROSS-PLATFORM_READINESS_AND_LINUX_MACOS_ADAPTER_SKELETONS.md). Contracts: FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-022-01 | Compile neutral crates without Windows imports on Linux/macOS; platform gaps return explicit Unsupported. |
| AC-022-02 | Round-trip unpaired Windows UTF-16 and Unix non-UTF-8 path encodings through versioned state without lossy identity conversion. |
| AC-022-03 | Run RecordingBackend tests on each OS and record separate real-platform readiness rather than claiming complete ports. |

## PR-023 acceptance cases

Owner: [implementation brief](PRs/PR-023_UI_PRIMITIVES_AND_CONTROLS.md). Contracts: FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-023-01 | Navigate every control with Tab/Shift+Tab/arrows; focus is visible and disabled controls do not dispatch edits. |
| AC-023-02 | Use IME, clipboard and validation in text fields; cancellation returns focus without leaking preedit into undo. |
| AC-023-03 | At 100/125/150/200/250% DPI, hit targets align with paint and popovers stay on-screen. |

## PR-024 acceptance cases

Owner: [implementation brief](PRs/PR-024_ACCESSIBILITY_AND_UI_AUTOMATION.md). Contracts: FC-09. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-024-01 | Use Narrator through open, edit, find, conflict and recovery; names, roles, values and focused state are announced. |
| AC-024-02 | Change selection/scroll in a 5 GiB file; semantic text-range requests are bounded and do not materialize the entire document. |
| AC-024-03 | Enable high contrast and test focus/selection/error distinction without color; real DirectWrite results supplement headless tests. |

## PR-025 acceptance cases

Owner: [implementation brief](PRs/PR-025_DIFF_CORE_ENGINE.md). Contracts: FC-02, FC-06. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-025-01 | With all ignore options off, applying all hunks reproduces the target bytes; with ignores on, only explicit nonignored ranges change. |
| AC-025-02 | Force normalized-hash collisions and whitespace/case equality; compare normalized bytes, retaining original byte ranges for edits. |
| AC-025-03 | Compare divergent multi-GB files under anchor/output caps; distinguish CompletedCoarse, Cancelled and Unavailable without claiming incomplete output is exact. |

## PR-026 acceptance cases

Owner: [implementation brief](PRs/PR-026_REPLACE_IN_FILES_AND_WORKSPACE_REPLACE.md). Contracts: FC-04, FC-05. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-026-01 | Preview then externally modify a file; Apply skips it for re-review instead of re-matching and changing unseen contents. |
| AC-026-02 | Replace in a mix of dirty open files and disk files; revisions/fingerprints are checked, backups unique and per-file receipts durable. |
| AC-026-03 | Kill between replacement and receipt commit; restart reconciles by target/backup fingerprints and never reapplies blindly. |

## PR-027 acceptance cases

Owner: [implementation brief](PRs/PR-027_FIRST-PARTY_EXTENSIONS_JSON_XML_AND_HEX.md). Contracts: FC-01, FC-07. Evidence: **NOT_STARTED**.

| Case | Fixture/action and expected result |
|---|---|
| AC-027-01 | Format JSON containing large integers and exponent lexemes; preserve numeric values and documented lexical policy. |
| AC-027-02 | Reject XML DTD/external entities/XInclude and unsupported XPath syntax; network stays inaccessible and depth/time limits hold. |
| AC-027-03 | Open Hex original-byte mode on UTF-16 with unsaved text edits; display original generation explicitly and distinguish an encoded edited preview. |

## Benchmark evidence protocol

Follow FC-10: pin baseline Notepad++ 8.9.8 by executable hash and record current stable separately. Record exact CPU, RAM, storage, OS build, power mode, renderer, extensions, cache state and build profile. Use 30 warm, 10 cold, 30 open and five long-running samples, alternating applications. Preserve all raw measurements, timeouts and exclusions. Report median/P95 with sample count and distinguish first frame, editable viewport, full resident load, styled viewport, first durable edit and sealed baseline. Compare total process memory and 100/500 populated tabs, plus disk/download cost. A hardware reference machine is not yet selected; report that as pending, never fabricate numbers.
