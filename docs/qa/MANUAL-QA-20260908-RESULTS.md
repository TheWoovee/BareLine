# Manual QA — 2026-09-08

Status: **Available practical sweep complete; release acceptance remains blocked.** Testing only. No implementation fixes, builds, commits or merges were performed. Fixes await the user’s go-ahead.

The ledger records QA-001 through QA-066, including partial, failed and unexecuted cases. These are representative workflows, not proof that all 81 acceptance criteria or 387 commands pass. The issue register retains 32 observation IDs; ISSUE-026 is clarified by a passing closed-file workflow and is not a confirmed mutation defect. Native modal and startup findings retain explicit attribution caveats.

## Results and evidence

- [Consolidated issue register](MANUAL-QA-20260908-ISSUES.md) — severity, reproduction and evidence.
- [Acceptance coverage and remaining prerequisites](MANUAL-QA-20260908-COVERAGE.md) — all 27 packages and 81 acceptance criteria, with unexecuted variants identified.
- [Static command inventory](MANUAL-QA-20260908-COMMANDS.md) — 387 commands; inventory is not execution evidence.
- Native screenshots, accessibility snapshots, fixture manifest and PDF review: `tests/e2e/results/manual-qa-20260908/evidence/`.

## Tested scope

Commit: `51cd4c0d1651fce815c67ed268e42999a9c2bb33`. Executable SHA-256: `5CC58A14008BD4D4874703FB1511F25A1A1BEA4D200B8AEFEF2AC91E89A10AAE`. Byte-identical isolated portable copies on Windows build 26200, Ryzen 5 3600, 128 GiB RAM. Normal and maximized window layouts were exercised. This was an executable-only QA copy, not a signed release installation.

Completed practical workflows include Unicode editing/save, codecs and refusal without data loss, find/replace and capture undo, multiple carets/rectangles, split editing, 24 MiB paged edit/save with a complete byte oracle, comparisons and merge undo, language/outline/folding, settings, printing/PDF review, SHA-256, workspace create/rename/retained-delete/undo, and small-log append/pause/rotation/truncation. Recovery, large-line navigation, macro playback, search input routing, export and restart checks recorded failures or partial outcomes.

## Remaining acceptance work

Full acceptance cannot be declared from this sweep. Direct operator follow-up is needed for CLI arguments/IPC and ambiguous native modal/startup outcomes. Signed extension/catalog and release installation tests need suitable signed artifacts; accessibility/IME/DPI, remote-share permissions, release VM, fault-injection and performance matrices require their specified environments. Optional boundary variants and unexecuted command paths remain listed in the coverage document. Known failed paths were recorded and independent testing continued; none was fixed.

Important clarifications: QA057 successfully merged through the native menu despite QA056 palette routing failure. QA064 succeeded after closing files and explains the earlier open-file rename/delete guard. QA058 confirms a successful one-page PDF print, while smaller-window print controls still have a separate finding.

## Execution log and issues

QA-001 PASS — Isolated native startup opens editable Untitled 1 with zero-byte status. Evidence: 001-startup.

ISSUE-001 (Medium, visual): Dark application retains white stock menu bar and generic icon, unlike supplied main-editor reference. Fresh startup also shows an empty Output pane consuming about 200 px. Actual 1202×792 vs reference 1672×941; exact pixel comparison is not claimed. No fix made.

ISSUE-002 (High, recovery): Fresh isolated launch; type alpha TODO é 😀 مرحبا, press Enter. A Recovery unavailable banner reports Windows sharing violation code 32, with Retry Recovery or Save As. Only this QA process was launched in its new portable directory. Editing remains available. Evidence: 002-recovery-sharing-error. No fixes attempted; continuing independent tests.

ISSUE-003 (Medium, status rendering): After 44 bytes/two lines are entered, UIA reports 44 bytes and line 2 correctly, while the visible bottom bar remains 0 B · 1 line / Ln 1, Col 1 across multiple actions. Evidence: 003-selection-status.

ISSUE-004 (Medium, keyboard): Ctrl+Shift+Home at end of line 2 selects only beta TODO 123, leaving the first line unselected; expected selection through document start. Evidence: 003-selection-status.

QA-002 PASS — Ctrl+A selects complete multiline Unicode text; Ctrl+C, Ctrl+N, Ctrl+V reproduces it in a second tab. Ctrl+Z clears pasted content in one step and clean marker returns; Ctrl+Y restores content and modified marker. Recovery sharing banner recurs (ISSUE-002). Evidence: 004-clipboard-undo-redo.

QA-003 PASS — Native Save As to generated saved qa α.txt completed, tab renamed/clean, disk UTF-8 text exactly matched copied content. Evidence: 005-save-result. Native-dialog UIA cached element lookup failed; current screenshot filename/Save controls worked (tool limitation, not assigned to app).

QA-004 PASS — Literal Find TODO shows two matches, Enter selects first; Replace All TODO→DONE changes both matches, disables repeat replacement, and announces Replacement complete with current UIA text. Earlier stale replacement text issue was not reproduced. Evidence: 006-replace-complete.

ISSUE-005 (Medium, accessibility focus): Find and Replace receive typed text and visible focus, but UIA focused_element remains Editor; input values are absent from emitted editable nodes. Replacement controls are now present.

ISSUE-006 (Medium, focus/Escape): Escape with Replace All button focused leaves expanded Find/Replace open rather than dismissing its transient surface. Close Find button needed. Evidence: 006-replace-complete and observed Escape state.

QA-005 PASS — One undo after Replace All restores both TODO occurrences and the saved clean marker; initial selection is restored.

QA-006 PASS — Command palette filters Settings, keyboard Enter opens actual Settings with correct UIA focus. Font size 12→16 pt applies and portable settings.toml persists size=16.0. Color mode System→Light applies through Down/Enter with focus returned to combo. Evidence: 007-palette-settings, 008-settings-light.

QA-007 PASS — Settings Escape closes Settings and returns editor focus; font is visibly larger at persisted 16 pt. Maximize redraws to 1920×1032 without losing Unicode content/selection. Stale visible status persists after maximize (ISSUE-003).

QA-008 PASS — Clone to Other View creates two views of saved file. Clicking secondary line 2 and typing RIGHT updates both panes while primary TODO selection remains independent.

ISSUE-007 (Medium, split accessibility): With secondary caret at end of line 2 after typing, UIA still reports primary TODO selection and line 1 column 11, rather than active secondary caret. Evidence: 009-split-edit. Split layout also retains a global tab row above both pane tab rows (visual discrepancy to review).

QA-009 PASS — Undo in secondary clone restores shared bytes/clean state. Dragging central divider changes pane widths without losing content.

ISSUE-008 (High, native close/modal): Ctrl+W on dirty Untitled 1 leaves main menu disabled and the app effectively modal; repeated fresh window enumeration exposes only Bareline, with no close confirmation. Escape/Tab and target reactivation do not expose or dismiss it. Generated text has an exact saved copy. This may involve native dialog ownership/capture; attribution is unconfirmed. Close-protection acceptance blocked; continue other tests in separate isolated instance without fixes.

QA-010 PARTIAL — Forced termination of only verified QA PID 52000 (scratch data already saved) used to test crash recovery and exit the blocked close state. Restart retained Light theme/16 pt settings and opened Recovery Center with 10 checkpoints.

ISSUE-009 (High, recovery): Recovery Center previews for the latest saved-file entries report file not found (os error 2); Open recovered copy and Compare disabled. Recovery baseline after ordinary native edits is therefore not recoverable through the presented action. Evidence: 010-recovery-after-crash. Related to ISSUE-002; no fix or recovery data deletion performed.

QA-011 PASS (scoped) — UTF-16 LE BOM fixture opens decoded Unicode with CRLF identified. Ctrl+S leaves exact 52-byte fixture SHA256 b0d49e4fbd05df35b62547ee45298e8ce5fb9f3c6e9adeeb25f79d93230e1698 unchanged. Evidence: 011-utf16-open.

ISSUE-010 (Medium, file open): Native Open accepted three quoted existing filenames (utf16-le.txt, mixed-eol.txt, sample.rs) but opened only the first, with no error for the omitted files after completion. Multi-file dialog acceptance failed in this attempt.

QA-012 PASS — Opening sample.rs detected Rust and visibly colored keywords, strings and macro invocation. Status accessibility reported Rust; the stale painted status remains ISSUE-003.

QA-013 PASS — Choose Language palette action opened a native picker; Enter on Plain text removed Rust colors without modifying document bytes or marking the tab dirty. ISSUE-011 MEDIUM — In Light mode the language picker remains dark and UIA focus stays Editor, despite keyboard navigation being captured by the picker. Evidence: 012-language-picker-light.

QA-014 PASS — Mixed CRLF/LF/CR fixture opened as four lines, accessibility identified Mixed, and unedited Ctrl+S preserved SHA256 10d2d05d0aea88c4825551c17f4dea08441d5284e1d86bf9e17a176c26edc9c1.

QA-015 PASS — Find LF returned two matches; Whole Word reduced this to one; Find Next selected line 2 LF. Closing Find preserved selection. F3 with the single match produced no visible change, so repeat-wrap behavior is inconclusive.

ISSUE-012 HIGH — Search > Find in Open Documents shows an empty focused-looking query box, but typing LF edits the document. Repeated after explicitly clicking the visible query box: line 2 became LFLF two while search stayed empty and disabled. Open-doc search execution is blocked by input routing. Recovery code 32 also repeated. Evidence: 013-open-doc-search-wrong-input.

ISSUE-004 additional reproduction — Ctrl+End in the loaded 24 MiB document moves from line 3 column 27 to line 3 column 65, not document end; editor was explicitly clicked first. This matches Ctrl+Shift+Home behaving as line-only navigation.

QA-016 PASS — Workspace folder picker loaded the isolated fixture tree; expanding root listed fixtures and clicking paged-24MiB.log opened it. Initial 64 KiB read-only/indexing state transitioned to 25,165,824 bytes and 387,073 lines. Wheel scrolling moved viewport from lines 1–19 to 28–46. ISSUE-013 MEDIUM — Dragging the visible right scrollbar thumb to the bottom did not scroll; caret instead moved to line 1 column 65. Evidence: 014-paged-scroll.

QA-017 PASS — Inserting QA at line 30 of the loaded 24 MiB document visibly changed that line and size by two bytes; Ctrl+Z restored line, size, caret and clean marker. No dirty save performed in this case.

QA-018 PARTIAL — Manage Extensions opens and correctly exposes disabled editor and empty Installed state, with Runtime not installed. Actual signed runtime/catalog availability is being checked separately. ISSUE-014 MEDIUM — Clicking Discover through UIA and then directly by visible coordinates leaves Installed selected; no tab/content change. Extension panel also stays dark in Light mode (extends ISSUE-011). Evidence: 015-extensions-empty-controls.

QA-019 PARTIAL / ISSUE-015 HIGH — 20 MiB single line initially displayed, but End produced Preparing line… and Caret column indexing, still present after the extension-manager test; selection reports BudgetExceeded. App remained responsive to menus/panels. End-of-line rendering did not complete during this interval. Evidence: 016-long-line-end-pending. QA-018 extension execution BLOCKED: static read-only support confirmed compiled_trust() returns None in this build; supplied blex archives unsigned, signed fixtures expired/test-only. No trust bypass/install or rebuild attempted. Escape did not dismiss manager; visible Close did.

QA-020 PARTIAL — 1 GiB log immediately showed its first viewport with explicit loading/read-only state. Follow New Content correctly disabled with reason Save edits and wait for current work before following while load pending. Further tail checks deferred until indexing completes.

ISSUE-016 MEDIUM — With 239px Workspace sidebar visible, clicking painted mixed-eol.txt tab around x357,y68 selected utf16-le.txt instead. Global tab hit-testing appears offset relative to painted tabs; exact implementation cause unconfirmed. Evidence: 017-tab-hit-test-workspace. Pending small compare-left.txt open has not completed while 1 GiB index remains pending; continuing loaded-document cases.

QA-021 PASS — Add Caret Below at start of UTF-16 fixture created two insertion points; typing QA inserted at both line starts; one Ctrl+Z restored both and clean state. Evidence: 018-multi-caret.

ISSUE-017 MEDIUM — Native word selection: double-clicking hello only placed caret; dragging from first-word start to end also left a caret at start without selection. Ctrl+D with no selection inside a word produced no selection; selection-based command tested separately below.

QA-022 PASS — With h selected, Ctrl+D added the second h occurrence; typing H changed both to Hello Hello. ISSUE-004 extends to Ctrl+Shift+Right: it selected one character rather than the next word.

QA-023 PASS — Base64 encode transformed selected Hello Hello into SGVsbG8gSGVsbG8=, and one Undo restored original text and selection. Utility Close worked. ISSUE-006 extends to utility result and Extensions: Escape did not dismiss those panels.

QA-024 PARTIAL — Start recording, type MACRO over selection, Stop recorded Macro 1. Play Selected Macro entered a running state (Cancel enabled, Play disabled on subsequent menu) but document did not change and status stayed Recorded Macro 1 during subsequent manager navigation. Concurrent 1 GiB open remains loading. No completed playback claim; retained for pending-worker assessment. Evidence: 019-macro-record-pending-play.

QA-025 PASS — Macro Manager Escape returned to editor. Cancel Macro Playback reported Macro cancelled. Cancel File Operations reported cancellation and removed the pending 1 GiB tab; UI remained usable. Large-file completion and macro completion not accepted.

ISSUE-018 MEDIUM — View > Show Toolbar displays the five toolbar buttons but the entire painted document-tab row disappears (blank row below toolbar), while accessibility still lists all document tabs. Persists after refresh. Evidence: 020-toolbar-hides-tabs.

QA-026 PASS — Toolbar Find opened the native find bar. Extended \x4D matched M in MACRO after Whole Word was cleared; Find Previous selected M, with correct 1-of-1 result.

QA-027 PASS — Compare with disk file paired compare-left/right, showed old/new changed line plus added line, counted two differences, and Next navigated to 2/2. Options exposed whitespace/case/EOL and color controls. Evidence: 021-compare-second-hunk. ISSUE-019 MEDIUM — Compare pane tab labels show mixed-eol.txt and paged-24MiB.log above actual compare-left content; compare options top/title and lower controls are clipped by layout at default window size.

ISSUE-019 clarification — Options Done and lower controls are visible at bottom on refreshed capture; clipping claim is limited to top/title.

QA-028 PASS — OS read-only fixture permits buffer edits but Ctrl+S explicitly fails File operation failed: file is read-only, keeping edited buffer. Original disk SHA d89450dcd687f4e1bbe6aed6e2952441346ff55442e478dd4b8344d60f915cf8 preserved. Evidence: 022-readonly-save-failure.

QA-029 PASS — Invalid UTF-8 fixture was detected as Windows-1252, rendered its decodable bytes, and unedited save preserved original SHA528bbdf683c527675f48dad9d45e1ed9924286278e615725b9c591aacd96fb20. This is legacy fallback, not a forced UTF-8 error test.

ISSUE-020 MEDIUM — Shortcut Mapper accepted Ctrl+DefinitelyNotAKey for Find Next and displayed CTRL+DEFINITELYNOTAKEY as assigned without validation error. Expected unsupported key rejection. Testing only in isolated settings; original F3 will be restored through mapper. Evidence: 023-invalid-shortcut-accepted.

QA-030 PASS — Shortcut Mapper filtered Find Next, exposed correct focused shortcut field, accepted restoring F3, and Escape returned to editor. Invalid-key validation failure remains ISSUE-020; original F3 restored.

ISSUE-021 MEDIUM — Clicking already-open sample.rs in Workspace reports File is already open in tab 2 but leaves invalid-utf8.bin active; it does not navigate to the existing document. Toolbar currently hides its tab (ISSUE-018), compounding access.

QA-031 PASS — Document List displayed ten loaded documents including dirty marker and activated sample.rs on click. ISSUE-022 MEDIUM — sample.rs is Rust again after switching away/back, although QA-013 explicitly set Plain text without closing it; per-document language override did not remain effective over this sequence.

QA-032 PASS — Outline opened for sample.rs and listed fn main. Both document/outline panels remain dark in Light mode (ISSUE-011).

ISSUE-023 HIGH — In Rust, Ctrl+Space at prefix mess showed message. Down visibly selected it, but Tab indented the line by four spaces and dismissed completion instead of accepting message, despite hint Enter/Tab. Evidence: 024-completion-tab-indents. Completion discovery itself worked.

QA-033 PASS — Ctrl+/ added // to the Rust scratch line with comment coloring; a second Ctrl+/ removed it.

QA-034 PASS — Restored generated sample.rs original text via editor and Ctrl+S cleared dirty marker and recovery banner; disk SHA verified against manifest. No project source edited.

ISSUE-024 HIGH — Print options opens, but Choose printer and print (below editor area at y607) produces no dialog or progress through pointer and UIA click. Focus stays Print header. Printing output blocked. Evidence: 025-print-button-no-response.

ISSUE-024 additional — Shift+Tab intended to traverse Print options instead marked the underlying disabled Rust document dirty; focus stayed Print header. Same class of input-routing failure as ISSUE-012.

QA-035 — Print to Microsoft Print to PDF completed and produced evidence/rust-print.pdf (39,469 bytes); native viewer displayed Rust text, line number and file header. Full page layout not yet inspected. ISSUE-024 clarification: maximizing puts Print button inside editor area and opens native Print; initial separate-dialog obscuring was automation ownership, not an app defect. Windows print filename rejected forward slashes, accepted backslashes (OS behavior).

QA-036 — Workspace Create Folder and Create File PASS: native commands created fixtures/workspace-created and its empty watch.log; expanding tree exposed file, clicking opened it, seeded two lines and saved. File stat independently confirmed initial zero bytes.

QA-037 — Small live log PASS scoped: Follow New Content displayed external third line; Pause changed to Resume, subsequent external fourth line appeared after Resume; typing while monitored left content unchanged. Unlock to edit tested next. This does not validate 1 GiB throughput, rotation, or long paused scroll anchoring.

ISSUE-025 — MEDIUM: Unlock to edit on clean monitored watch.log asks “Discard unsaved changes to Stop monitoring and capture a fixed generation for editing? and close this tab?” despite rejected editing/clean tab; Yes leaves tab open and shows external-change controls. Misleading destructive/close wording. Evidence 026-watch-unlock-warning (post-confirmation); prompt observed natively.

QA-038 / ISSUE-026 — MEDIUM: Rename Selected Entry on watch.log after monitoring unlock accepted renamed-watch.log in Save As, but original watch.log remains sole disk entry and tree/tab unchanged. External-change banner obscures any transient outcome; no completed rename observed. Evidence 027. Follow snapshot may retain handles; cause not diagnosed or fixed.

QA-039 — Retain-for-Undo delete of the same previously monitored log likewise left watch.log on disk/tree; undo success cannot be claimed for this blocked open-file case. Grouped with ISSUE-026.

QA-040 / ISSUE-027 — HIGH: Play Selected Macro (recorded MACRO insertion) on new empty Untitled 2, with 1 GiB operation already cancelled, still inserts nothing after subsequent menu/manager inspection. Cancel Playback is enabled while Play/Record disabled; manager status only Saved (50 ms delay). Same incomplete behavior as QA024, now independent of pending 1 GiB open. Evidence 028.

QA-041 — Bookmark shortcut/navigation PASS scoped: Ctrl+F2 at line 1, pointer moved to line 3, F2 returned caret to line 1. No visible bookmark glyph in left gutter at 16 pt/sidebar layout (visual discoverability limitation).

QA-042 — Keyboard rectangular replacement PASS: Alt+Shift+Down then Alt+Shift+Right at first column selected first character across first two lines; typing X replaced both a characters, single Undo restored both. UIA selected_text exposed only primary row a, extending known multi-selection accessibility limitation.

QA-043 — Column Editor repeated-text PASS scoped: Alt+C retained rectangle, Repeated text Z and Enter replaced both selected characters; single Undo restored both. Dialog dark in Light theme and lower action/help area clipped by editor/output boundary at 1202×792 (same layout family as ISSUE024). Keyboard Enter works.

QA-044 — Regex search PASS scoped: [0-9]+ reported 3 matches, Next selected 123 (1 of 3); malformed [ displayed Invalid query while preserving document.

QA-045 — Folder search BLOCKED by ISSUE-012 input routing: selected small workspace-created folder, visible focused query retained [. Ctrl+A selected underlying entire Untitled 2 document, typing line replaced document while query stayed [. UIA reports focused Find in folder despite editing underlying disabled document. Evidence 029. Folder search/result navigation and downstream replacement cannot be accepted through this route.

QA-046 / ISSUE-028 — HIGH: Folder search remains open after Escape, Cancel folder search UIA activation, Cancel Search menu, Close Search Results palette and Alt+F4. Lower fields/buttons clipped at normal and maximized size; Tab indents underlying document. Native window inventory shows no separate close prompt. Evidence 030. Current process retained for evidence; fresh isolated profile will be used to complete remaining unrelated cases. QA-047 — Run marker definition loads and native confirmation shows exact executable/arguments. Confirmed in separately targeted dialog; Output stays Starting / 0 bytes discarded across subsequent close checks, no marker or exit result observed. Completion blocked in this stuck-panel session; requires fresh-profile check before calling an independent Run defect.

Harness isolation note: fresh app-session copy was launched; after capture, only original QA-owned stuck PID76696 (verified app/bareline.exe path) was terminated. No user app/process or source affected. New profile PID58884 remains for final checks; no build performed.

QA-048 / ISSUE-029 — HIGH, needs direct human reproduction: fresh-profile Run marker loads, Run menu dispatch leaves empty editor visible but typing/palette/Close no longer act. No separate confirmation appears in native window inventory. Could involve native modal ownership/automation; record as blocked, not successful execution. Evidence 031. No command output completion observed.

QA-049 — Rust folding PASS scoped: saved multiline session-fold.rs, Unfold All exposed all 7 lines, clicking nested fold gutter hid lines 4–5, clicking again restored. Saving from Untitled to .rs initially collapsed outer block automatically before a fold command; unexpected initial fold state noted for review. Document bytes retained.

QA-050 — Settings validation PASS: Font size -1 rejected with Value is outside the permitted type or range; Cancel restored displayed 12 pt and left effective size unchanged.

QA-051 — Document Map PARTIAL: toggle shows Sampling then completed density rendering; map click moves caret (line 3 col15 on 82-byte Rust fixture). Small file fits viewport, so large-file map scrolling/viewport highlighting not proved. Explicit Dark theme applied for restart persistence.

QA-052 — About dialog opens with Version0.1.0, Build unknown, x86_64, Hardware renderer, Portable mode, local diagnostics enabled (evidence032). Third-party notices returns file-not-found because this QA profile copied executable without distribution notice assets; packaging prerequisite, not attributed as a source defect.

QA-053 / ISSUE-030 — HIGH, launch outcome unconfirmed: clean Alt+F4 with saved session-fold.rs exited and wrote session.json including caret45 / unfolded state. Two subsequent Sky launches returned no targetable window; no bareline process remained, diagnostics had no new first-frame event and crash log empty. No matching recent Windows application crash event. Saved-session reopen therefore BLOCKED, not passed; direct launch reproduction needed to distinguish startup failure from launcher behavior.

Restart differential: identical executable in brand-new app-final portable profile launches successfully via the same Sky API after saved app-session profile twice failed. Strengthens profile/state-specific startup concern, without proving exact cause.

QA-054 — UTF-8 BOM open/save PASS: BOM α renders without BOM glyph, menu reports BOM on; save retains original SHA d41050151dc4fc5a15092ee2f079df40415a9516cb1b460b730af60c83d2db90. QA-055 — Lossy conversion prevention PASS: Convert to Windows-1252 then Save reports Unrepresentable at text bytes4..6, retains dirty buffer and same original disk SHA. No silent replacement/loss.

**QA056 — FAIL / ISSUE-012 extension:** While comparing compare-left.txt against compare-right.txt (2 hunks), opening the command palette reported Search commands focus, but typing merge inserted merge into the left document and left the palette query unchanged. Merge/undo command workflow is blocked by the reproduced input routing defect. Disk fixture was not saved. Evidence: 033-compare-palette-input-routing.

**QA057 — PASS (scoped):** Compare options Ignore all whitespace applied and indicator changed. Native Tools > Compare > Copy right to left successfully replaced first hunk old with new; single Ctrl+Z restored original clean left buffer. This bypasses QA056 palette blockage; merge itself is not blocked. Both disk fixtures unchanged (no save). Whitespace-only suppression and comparison cancellation not exercised.

**QA058 — PASS (artifact scope):** Independent PDF reviewer confirmed rust-print.pdf has one Letter 612×792 page, preserves all 61 source characters, and visually shows complete syntax-colored text, line number, header and Page 1 footer without clipping or overlap. No PDF characters fall outside page bounds. Evidence: evidence/rust-print-review.png and rust-print-review.md. Git status audit found no implementation-source changes.

**QA059 — PASS save bytes / ISSUE-031 MED:** Editing the 24 MiB isolated paged copy by inserting QA_PAGED plus a space and saving produced exactly 25,165,833 bytes, matching original bytes prefixed with the 9-byte marker (SHA256 dea694e7d68ec701bfb5308d71b7ee4e4ae5a59ee160e6e961b87d3d2831cff5). After save, app unexpectedly displayed Source changed; current bytes preserved despite no external write to this fixture. Recovery sharing error also recurred (ISSUE-002). Evidence: 034-paged-save-self-change.

**QA060 — PASS (scoped):** Closed the clean paged-save tab and reopened the file through Open. The first viewport shows the persisted QA_PAGED marker and original line content; independent full byte equality was already verified in QA059.

**QA061 — PASS:** Current-document regex item([0-9]+) found two matches in item12 item34 α😀. Replace All with value$1 yielded value12 value34 α😀; closing Find and one Ctrl+Z restored both original matches and Unicode suffix.

**QA062 — PASS:** SHA-256 utility over item12 item34 α😀 reported 20 UTF-8 bytes and 6fcb6027bb8b3584661eb3375014194bdfac649dcf4581fc61c737888a47be16, exactly matching independent Node crypto oracle.

**QA063 — FAIL / ISSUE-032 HIGH:** Utilities > Export syntax-colored HTML from unsaved item12 item34 α😀 to a new unicode-export.html produced Utility stopped: file being used by another process (OS error 32), Source preserved; retry the command. No output file was created. This is a fresh export destination; no external writer was used. Evidence: 035-html-export-sharing-error.

**QA064 — PASS / clarification to ISSUE-026:** Unmonitored one.txt rename while open explicitly requested Close the document before renaming or deleting its file. Closing tab then renaming via Workspace succeeded (one.txt absent, renamed.txt exact content). Selecting and closing renamed.txt then Retain for Undo delete removed original path and created retained sibling. Undo Workspace Delete restored renamed.txt with exact workspace QA plus LF. Prior QA038–039 remained open-file cases; do not classify the no-op as confirmed mutation failure without considering this guard.

**QA065 — PASS:** Follow New Content on small rotate.log detected rename-and-replacement, preserved captured old content, and offered Reopen and follow. Choosing it displayed after rotation/new record. In-place truncation to short plus LF again preserved captured content and offered explicit reopen; reopening displayed short. No silent switch or fixture loss observed.

**QA066 — NOT RUN (tool prerequisite):** CLI two-path/non-BMP path, line/column/read-only flags and multi-instance IPC acknowledgement were not manually executed. Approved native launch API only accepts an app path, without arguments; terminal/Run-box workaround was not used. Static CLI review is supplemental evidence only. Requires direct operator launch with the documented argument matrix; no build or implementation change is required for that follow-up.

## Final repository verification

`git status --short` after testing showed only the pre-existing `.gitignore` modification, pre-existing untracked `.ignore`, and QA documentation under `docs/qa/`. No implementation-source changes or new builds were performed. Generated fixtures and 75 evidence files remain under the isolated QA results directory.
