# Manual QA coverage — 2026-09-08

Baseline: main checkout commit `51cd4c0`. State: **PRACTICAL SWEEP COMPLETE; ACCEPTANCE BLOCKED**. Final evidence mapped through QA-066 / ISSUE-032. Earlier progress sections remain historical and are superseded by the final reconciliation below. Root is the sole desktop controller. This document author made no desktop actions, builds, source fixes or benchmark runs. Report defects and preserve scratch evidence; **do not fix anything until the user says go**. Earlier automated gates and 2026-09-06 native observations are context, not current-binary passes.

## Execution and evidence rules

Use one verified current executable: record absolute path, SHA-256, profile, source commit, Windows build, renderer, actual client dimensions, scale, display/keyboard/IME and assistive-technology versions. Use isolated generated fixtures and private portable/settings/recovery roots. Record before/after bytes or hashes for mutations. Never overwrite prior expected fixtures or user scratch. Pause for user input, unrelated dialogs or lock; do not dismiss personal prompts. Follow the unlocked checklist order; its stale implementation/lock labels are historical, not the current implementation state.

For every case capture: ID, prerequisites, fixture/hash, steps, expected result, actual observation, screenshot/log paths, elapsed/timeout when relevant, reviewer, disposition and defect ID. Allowed dispositions: PLANNED, PASS with exact scope, FAIL, BLOCKED with observed reason, or UNAVAILABLE with required environment. A visible success message alone does not prove durable data. A screenshot does not prove bounded memory, cancellation, provenance or crash durability. No signing, publishing, system trust-store changes or installation into the personal environment is implied.

## Prioritized executable native workflows

Workflow inventory below describes planned scope; scoped execution through QA-017 is reconciled in the progress section. Unexecuted portions remain NOT RUN. Use menu/palette action names from the current runtime inventory; record missing/disabled actions rather than guessing shortcuts. Repeat critical mutation/undo/save flows in Resident and Paged documents and both panes where applicable.

| Order / workflow | Packages | Executable steps and expected result |
|---|---|---|
| N01 Baseline and overlays | 001,011,012,021,023 | Launch private portable instance; verify editable Untitled, menu/tab/gutter/status. Capture dark/light main screen, Find, palette, Settings and split composition against current mocks at actual dimensions. Resize/maximize/restore; no clipping or focus loss. Record differences, never resize screenshots to imply matching geometry. |
| N02 Input and saved state | 002,003,004,007 | Enter Latin/Arabic, emoji, combining marks and CRLF/LF fixtures; arrows/Home/End/mouse/Shift-select/scroll; Copy/Cut/Paste; undo/redo. Save As new scratch path, edit/save/edit/undo to saved state; verify exact bytes and dirty marker. Cancel Open/Save As/dirty close and retain text; close clean neighbor only. |
| N03 Search regression first | 005,024 | Ctrl+F query, Tab/Shift+Tab, F3/backward, close/reopen, replace one/all, one undo/redo. Verify document bytes, match counts, terminal notice, UIA field values and actual focused element after async results. Old 2026-09-06 stale UIA and status defects must be retested, not carried as current failures or passes. |
| N04 Power editing and split transfer | 006,010 | Multi-caret insertion and selection, measured rectangular edit through tabs/wide glyphs/short lines, line commands/bookmarks; split/clone, independent caret/folds/scroll, pin/reorder, synchronized scroll. Drag-copy/move same document and across documents with multiple ranges; verify exact bytes, selection, dirty state and linked undo/redo in both actors. Close one participant then undo/redo remaining view; cancel preparation and verify no mutation. |
| N05 Full-source search and reviewed replace | 005,026 | Search long-line cross-window literal/regex; exercise incomplete regex result and disabled replacement. Folder search generated tree, preview open/disk replacements, alter one source before Apply; stale file must skip/re-review. Verify per-file receipts, unique backups, cancel and linked undo of open documents. |
| N06 Language and navigation | 008,009,013 | Switch Rust/TOML while outline/completion runs; no stale document anchors. Fold large hidden span, navigate/edit/unfold/undo across seam, clone with independent folding; verify source bytes. Completion accept/dismiss, signatures, brackets/indent/comment commands. Explore scratch tree, create/rename/delete scratch only; Documents filtering, outline and map partial states. |
| N07 Commands and settings | 011,012,023 | Palette filtering, every menu/context/toolbar route, toolbar customization/persistence, shortcut remap/conflict, nested Escape/F6 focus. Change font size/theme/indent/resource limits; restart private instance, verify persisted units/values and source text/history retained after lowering budgets. Validate invalid values and rejected privileged workspace override. |
| N08 Macros and processes | 014 | Record stable actions/text, save/restart/replay, stop at a failing command and inspect location. Run reviewed harmless fixture process with quoted/metacharacter arguments; inspect output links, cancel bounded output producer, verify owned child exits and editor remains usable. |
| N09 Watch, compare and utilities | 015,017,025 | Append partial log line, pause/follow/unlock; truncate/rotate/rewrite generated file and inspect SourceChanged. Compare two fixtures, swap/ignore/recompare/cancel/navigate/merge one hunk/undo; stale merge disabled. Hash/encode/decode/statistics selected off-viewport bytes; export Unicode HTML/RTF and inspect output. Print only with available controlled printer/PDF destination. |
| N10 Extension journeys | 016,027 | With verified local signed fixture packages: install/enable/invoke/disable/remove in private root; inspect actual host drain. JSON large-number formatting, XML rejected external entities/unsupported XPath, Hex original versus edited preview. Rejected package/capability/oversized fixture must not mutate documents or access network. |
| N11 Lifecycle and Windows integration | 004,018,020 | Private CLI paths with spaces/non-BMP, multiple files, line/column/read-only and authenticated same-user handoff; private session restart with dirty recovery/conflict. Portable move, tray restore, diagnostics redaction. Controlled process termination only of owned QA process; flush-boundary/disk-full injection requires existing supported harness, not a new code change. Installer/update/rollback belongs in clean VM and signed-artifact lane. |
| N12 Scale, accessibility and physical input | 001,003,012,021,023,024 | Keyboard-only traversal and Narrator/NVDA where installed; actual field values/focus/live announcements/TextPattern bounded viewport. Physical IME commit/cancel, AltGr/dead keys, BiDi. Supported 100/125/150/200/250/300% and high contrast; candidate/popover placement and pointer hit alignment. Missing hardware/layout/tool is unavailable, not passed. |
| N13 Large fixtures and measurement | 002,007,009,019,024,025 | Bounded generated 20 MiB line, 100/500/5000 tabs and multi-GiB fixtures only after disk/RAM prerequisites verified. Exercise scroll/edit/find/cancel/save/tail/compare/UIA. Use existing measurement tools and real markers; distinguish first frame/editable/full load, process tree memory, runtime disk growth and acquisition. Paired pinned release Notepad++ trials are separate from functional debug testing. |

## Full package coverage

Each package maps to its three exact acceptance cases below plus these native workflows. Package completion requires its PR brief minimum scenarios and command-matrix rows as well; the 81 cases are not an exhaustive command list.

| Package | Workflows | Current manual state |
|---|---|---|
| PR-001 | N01,N12 | PLANNED / NOT RUN |
| PR-002 | N02,N07,N13 | PLANNED / NOT RUN |
| PR-003 | N02,N12,N13 | PLANNED / NOT RUN |
| PR-004 | N02,N11 | PLANNED / NOT RUN |
| PR-005 | N03,N05 | PLANNED / NOT RUN |
| PR-006 | N04 | PLANNED / NOT RUN |
| PR-007 | N02,N13 | PLANNED / NOT RUN |
| PR-008 | N06 | PLANNED / NOT RUN |
| PR-009 | N06,N13 | PLANNED / NOT RUN |
| PR-010 | N04 | PLANNED / NOT RUN |
| PR-011 | N01,N07 | PLANNED / NOT RUN |
| PR-012 | N01,N07,N12 | PLANNED / NOT RUN |
| PR-013 | N06 | PLANNED / NOT RUN |
| PR-014 | N08 | PLANNED / NOT RUN |
| PR-015 | N09 | PLANNED / NOT RUN |
| PR-016 | N10 | PLANNED / NOT RUN |
| PR-017 | N09 | PLANNED / NOT RUN |
| PR-018 | N11 | PLANNED / NOT RUN |
| PR-019 | N13 | PLANNED / NOT RUN |
| PR-020 | N11 | PLANNED / NOT RUN |
| PR-021 | N01,N12 | PLANNED / NOT RUN |
| PR-022 | External foreign-host lane | PLANNED / NOT RUN |
| PR-023 | N01,N07,N12 | PLANNED / NOT RUN |
| PR-024 | N03,N12,N13 | PLANNED / NOT RUN |
| PR-025 | N09,N13 | PLANNED / NOT RUN |
| PR-026 | N05 | PLANNED / NOT RUN |
| PR-027 | N10 | PLANNED / NOT RUN |

## All 81 acceptance cases

Exact expected results below are transcribed from the acceptance register. “Harness/evidence” means the named assertion cannot be established by UI observation alone; reuse existing evidence/harness under the coordinator, without new builds or fixes. Publishing language in the register is an eventual release requirement, not authorization to publish during this campaign.

| Case | Exact fixture/action and expected result | Workflow / prerequisite lane | Campaign state |
|---|---|---|---|
| AC-001-01 | Shape one mixed Arabic/Latin/emoji line with DirectWrite and BiDi caret tracking; place IME preedit/candidate windows at 100%, 150% and 200% DPI and recover from device loss. | N01,N12 | PARTIAL — QA-002 Unicode display; physical IME/DPI/device loss NOT RUN; ISSUE-003 status |
| AC-001-02 | Open 5,000 lazy tabs; worker count remains bounded and no document reads occur before the first frame. | N01,N12 | PLANNED / NOT RUN |
| AC-001-03 | Verify codec, filesystem-capability and VerifiedPackageSource interfaces compile with fake providers; online provider fails closed. | N01,N12 | PLANNED / NOT RUN |
| AC-002-01 | Generate random CR/LF/CRLF edits, including separators split across pieces; line counts match the reference model. | N02,N07,N13; harness/evidence | PLANNED / NOT RUN |
| AC-002-02 | Delete a range, evict its base pages and externally replace the source; undo restores owned inverse bytes exactly. | N02,N07,N13; harness/evidence | PLANNED / NOT RUN |
| AC-002-03 | Race a Resident background fill with truncation; missing old-generation bytes return Unavailable and never new contents. | N02,N07,N13; harness/evidence | PLANNED / NOT RUN |
| AC-003-01 | Select across emoji clusters, combining accents and opaque byte markers; text edits never split their atomic boundaries. | N02,N12,N13 | PARTIAL / FAILURE OBSERVED — QA-021/022 multicaret/occurrence edit and undo; ISSUE-004 word navigation and ISSUE-017 pointer word/drag selection; atomic-boundary cases incomplete |
| AC-003-02 | Scroll a 20 MiB single line; layout is bounded and cancellation leaves input responsive. | N02,N12,N13 | PARTIAL / FAILURE OBSERVED — QA-019 20MiB line displayed, menus responsive; ISSUE-015 End/indexing pending with BudgetExceeded; bounded completed layout not established |
| AC-003-03 | Commit and cancel IME preedit; only committed text enters undo and recovery; verify real Windows shaping and BiDi hit testing. | N02,N12,N13 | PLANNED / NOT RUN |
| AC-004-01 | Edit, save, edit again and undo to saved ContentStateId; the dirty marker clears without comparing monotonic revisions. | N02,N11 | PASS (scoped native) — QA-003 save, QA-004 replacement, QA-005 undo to saved clean state |
| AC-004-02 | Kill the process after each segment/journal/checkpoint flush boundary; recovery stops at the last valid durable record. | N02,N11 | PARTIAL / FAILURE OBSERVED — QA-010 one owned process termination; ISSUE-002/009 recovery unavailable; flush-boundary campaign NOT RUN |
| AC-004-03 | Start a save then type; successful save clears only its captured state. Missing source regions disable Save/Save As and offer export with gaps. | N02,N11 | PLANNED / NOT RUN |
| AC-005-01 | Match across a 16 MiB window in a 20 MiB line; case-fold results map to correct original TextOffsets. | N03,N05 | PLANNED / NOT RUN |
| AC-005-02 | Exercise lookbehind, anchors, empty matches and partial regex contexts; unsupported cases report incomplete and disable replacement. | N03,N05 | PARTIAL — QA-044 invalid regex refusal, QA-061 captures replace/atomic undo pass; lookbehind/anchors/empty/partial-context cases not exercised |
| AC-005-03 | Replace in two dirty open documents while one revision changes; prepare aborts before either mutates; successful linked undo reverses both. | N03,N05 | PLANNED / NOT RUN |
| AC-006-01 | Column insertion across tabs, wide glyphs and short lines follows display columns and undoes in one transaction. | N04 | PARTIAL — QA-042/043 two-row keyboard rectangle and repeated-text edit each undo once; tabs/wide glyphs/short-line display-column fixture NOT RUN |
| AC-006-02 | Enable clipboard history then exceed 20 entries and 16 MiB total; bounded eviction occurs and history never persists to disk. | N04 | PLANNED / NOT RUN |
| AC-006-03 | Invoke comment commands before PR-013; show unavailable with a reason and do not mutate text. | N04 | PLANNED / NOT RUN |
| AC-007-01 | Round-trip invalid UTF-8, genuine U+FFFD/private-use characters and noncanonical legacy sequences without provenance collisions. | N02,N13 | PARTIAL — QA-029 Windows-1252 fallback exact-byte save; not forced invalid UTF-8/opaque-marker or all provenance cases |
| AC-007-02 | Open/save UTF-16LE/BE, UTF-32LE/BE and true Latin-1 fixtures with BOM and mixed EOL; exact unedited bytes survive. | N02,N13 | PARTIAL — QA-011 UTF16LE, QA-014 mixed EOL and QA-054 UTF8 BOM exact-byte saves; QA-055 lossy conversion refused with original hash intact; remaining specified codecs NOT RUN |
| AC-007-03 | Transcode a generated 5 GiB UTF-16 file under the RAM budget; disk quota exhaustion is visible and original data remains intact. | N02,N13 | PLANNED / NOT RUN |
| AC-008-01 | Validate paired Lexilla/native definitions for all 15 languages listed in FC-09; publish coverage and known grammar gaps. | N06 | PARTIAL / FAILURE OBSERVED — QA-012 Rust colors/QA-013 Plain switch; ISSUE-022 override lost after switch; full15-language paired coverage NOT RUN |
| AC-008-02 | Open in the middle of a multiline string; provisional highlighting is labelled until a valid lexer checkpoint resolves context. | N06 | PARTIAL — QA-049 small Rust gutter fold/unfold preserves bytes; middle-of-multiline-string provisional checkpoint case NOT RUN |
| AC-008-03 | Import malformed/oversized UDL input; reject unsafe structure and preserve the previous definition. | N06 | PLANNED / NOT RUN |
| AC-009-01 | Select main.rs then Cargo.toml; outline changes language and old-revision results never appear in the new document. | N06,N13 | PARTIAL — QA-031 Document List activates Rust; QA-032 outline fn main; rapid Rust/TOML stale-result case NOT RUN |
| AC-009-02 | Open a folder with symlink cycles and inaccessible descendants; traversal is bounded and shows partial results. | N06,N13 | PARTIAL — QA-036 create folder/file and tree activation; ISSUE-026 monitored-file rename/delete incomplete; cycle/inaccessible traversal NOT RUN |
| AC-009-03 | Scroll before line indexing completes; minimap and outline use estimates explicitly and do not allocate per-line objects. | N06,N13 | PARTIAL — QA-051 small-file map Sampling/completion/click; partially indexed large-file estimates/scroll NOT RUN |
| AC-010-01 | Pin a tab and reorder across views; pinned-first default and per-view selection persist after restore. | N04 | PLANNED / NOT RUN |
| AC-010-02 | Add a compare alignment spacer; it receives no document line number and synchronized scrolling maps logical lines correctly. | N04 | PLANNED / NOT RUN |
| AC-010-03 | Clone a document in a second pane; edits share a document while caret, selection, folds and scroll remain independent. | N04 | PARTIAL — QA-008/009 shared clone edits/undo, independent selection, divider; independent folds/scroll NOT RUN; ISSUE-007 active-pane UIA |
| AC-011-01 | Every menu/toolbar/palette action resolves to one stable ID; unavailable actions have a reason and no handler mutation. | N01,N07 | PARTIAL / FAILURE OBSERVED — QA-026 toolbar Find, QA-030 mapper F3 restore; ISSUE-018 hidden tabs and ISSUE-020 invalid key accepted; full matrix NOT RUN |
| AC-011-02 | Press Escape in nested popup, panel and editor states; only the topmost transient layer closes and focus returns predictably. | N01,N07 | PARTIAL / FAILURE OBSERVED — Settings/Mapper/Macro Escape works QA-007/025/030; ISSUE-006 Find/utility/extensions Escape fails; nested layers incomplete |
| AC-011-03 | Test AltGr text, menu mnemonics and remapped shortcuts together; text entry is not consumed as a global shortcut. | N01,N07 | PLANNED / NOT RUN |
| AC-012-01 | Change font size to 12 pt at mixed DPI; preview, persisted value and restored editor use the same point unit. | N01,N07,N12 | PARTIAL — QA-006/007 font16pt applied/persisted, QA-010 restored; specified12pt mixed-DPI equivalence NOT RUN |
| AC-012-02 | Attempt workspace override of execution, network or extension grants; reject it while accepting allowed indentation preferences. | N01,N07,N12 | PLANNED / NOT RUN |
| AC-012-03 | Check Light/Dark/System tokens and override persistence; normal text meets 4.5:1 and focus indicators 3:1 on actual surfaces. | N01,N07,N12 | PARTIAL / FAILURE OBSERVED — QA-006/010 Light persistence; ISSUE-011 dark language/extensions/document/outline panels; contrast/System/Dark NOT RUN |
| AC-013-01 | Register PR-006 CommentProvider with a language; line/block toggles handle empty selections and undo correctly. | N06 | PARTIAL — QA-033 Rust line comment toggle twice; undo/empty selection/block variants NOT RUN |
| AC-013-02 | Type while completion computes; old-generation/revision suggestions cannot replace newer text. | N06 | PARTIAL / FAILURE OBSERVED — completion discovered message; ISSUE-023 Tab indents instead of accepting; stale revision test NOT RUN |
| AC-013-03 | Complete within strings and comments using bounded local indexes; suppress semantically unsupported suggestions. | N06 | PLANNED / NOT RUN |
| AC-014-01 | Record/replay only stable command IDs and explicit text; playback failure stops with a resumable location. | N08 | PARTIAL / FAILURE OBSERVED — QA-040 repeats pending replay with no large load; ISSUE-027 no completed insertion; cancellation QA-025 |
| AC-014-02 | Run a path and argument containing spaces, quotes and shell metacharacters using direct spawn; no implicit shell interpretation occurs. | N08 | BLOCKED — QA-047 exact command confirmation observed without terminal; fresh-profile QA-048 ISSUE-029 modal/launch attribution unresolved; argv execution not verified |
| AC-014-03 | Cancel a process producing unlimited output; output storage stays bounded, process cleanup completes and the editor remains usable. | N08 | NOT RUN — process execution blocked QA-048; macro/file cancellation does not establish child-process/output cleanup |
| AC-015-01 | Append to a log with a partial final page; verify continuity before accepting the new generation, without falsely entering SourceChanged. | N09 | PARTIAL — QA-037 small-log external append displayed; specific partial-final-page continuity/generation case incomplete |
| AC-015-02 | Rotate, truncate and rewrite same-size content; uncertain continuity becomes SourceChanged and preserves owned edits. | N09 | PARTIAL — QA-065 rotation and truncation preserve captured content and explicit reopen works; same-size rewrite/owned dirty edit variant not exercised |
| AC-015-03 | Pause tail scrolling while appends continue; unlock stops following and captures a fixed generation after confirmation. | N09 | PARTIAL / FAILURE OBSERVED — QA-037 pause/resume and read-only follow; ISSUE-025 misleading unlock discard/close wording; fixed generation and paused scroll anchoring not proved |
| AC-016-01 | Install a signed offline runtime and extension using the PR-001 provider contract; no PR-018 online code is required. | N10; verified package/artifact prerequisites | BLOCKED — QA-018 manager opens; compiled trust absent and supplied packages unsigned/expired/test-only per QA-019; no verified installation |
| AC-016-02 | Disable all extensions; host exits and installed files remain. Explicit Remove runtime deletes only its verified owned files. | N10; verified package/artifact prerequisites | PLANNED / NOT RUN |
| AC-016-03 | Send oversized frames, unauthorized capabilities and a CPU loop; reject/terminate the offending extension within per-extension limits. | N10; verified package/artifact prerequisites | PLANNED / NOT RUN |
| AC-017-01 | Copy a hunk left-to-right then undo; source labels, dirty state and selection match the actual target revision. | N09 | PARTIAL — QA-057 native-menu right-to-left first-hunk merge and single undo restore clean target; disk unchanged. Source-label ISSUE-019/selection full scope unresolved; QA-056 palette failure separate |
| AC-017-02 | Choose ignore options and recompare; cancellation is distinct from a completed coarse result and stale merge buttons disable. | N09 | PARTIAL — QA-057 ignore-whitespace indicator applied; actual whitespace suppression/cancel/coarse/stale merge not exercised |
| AC-017-03 | Print/export Unicode with syntax/theme options; unsupported printer/filesystem errors preserve the original and show a clear retry path. | N09 | PARTIAL / FAILURE OBSERVED — QA-058 PDF full one-page layout/source review passes; QA-063 HTML export OS32/no output ISSUE-032; full export/options/retry acceptance incomplete |
| AC-018-01 | Reject an expired, rollback, wrong-channel, wrong-publisher or hash-mismatched update before staging application files. | N11; verified package/artifact prerequisites | PLANNED / NOT RUN |
| AC-018-02 | Verify signatures on inner executables after extracting final ZIP/installer; compare reproducibility only before signing. | N11; verified package/artifact prerequisites | PLANNED / NOT RUN |
| AC-018-03 | Launch CLI with non-BMP paths and arguments via authenticated same-user IPC; portable mode writes only to its configured data root. | N11; verified package/artifact prerequisites | NOT RUN — QA-066 launch API lacks argument support; requires direct operator CLI/non-BMP/multiple-path/flags/IPC matrix |
| AC-019-01 | Run paired baseline trials with pinned executable hashes, alternating app order and separate first-frame/editable/full-load timings. | N13; harness/evidence | PLANNED / NOT RUN |
| AC-019-02 | Measure 100 and 500 tabs plus extension process memory; report peak total private bytes, disk growth and downloads separately. | N13; harness/evidence | PLANNED / NOT RUN |
| AC-019-03 | Publish all raw runs, sample counts, P50/P95 and timeouts; hosted CI noise never becomes a comparative marketing claim. | N13; harness/evidence | PLANNED / NOT RUN |
| AC-020-01 | Inject disk-full and process death at every recovery/replace receipt transition; reconcile originals, backups and committed targets by fingerprint. | N11; harness/evidence | PARTIAL / FAILURE OBSERVED — QA-010 controlled QA process death; ISSUE-009 unavailable recovery; disk-full/receipt-boundary campaign NOT RUN |
| AC-020-02 | Test archive traversal, reparse escapes, IPC spoofing, stale signed metadata and compromised release-key recovery. | N11; harness/evidence | PLANNED / NOT RUN |
| AC-020-03 | Verify diagnostics contain no document text or sensitive path data and no telemetry upload occurs. | N11; harness/evidence | PLANNED / NOT RUN |
| AC-021-01 | Resolve every atomic case and every registered command to an implementation commit and independent result; exclusions retain rationale. | N01,N12 | PLANNED / NOT RUN |
| AC-021-02 | Run the key workflows with keyboard and screen reader on both Windows floor builds and mixed DPI. | N01,N12 | PLANNED / NOT RUN |
| AC-021-03 | Publish release readiness with unresolved correctness items, performance measurements and limited parity claims; mockups never count as runtime evidence. | N01,N12 | PLANNED / NOT RUN |
| AC-022-01 | Compile neutral crates without Windows imports on Linux/macOS; platform gaps return explicit Unsupported. | External foreign-host lane; harness/evidence | PLANNED / NOT RUN |
| AC-022-02 | Round-trip unpaired Windows UTF-16 and Unix non-UTF-8 path encodings through versioned state without lossy identity conversion. | External foreign-host lane; harness/evidence | PLANNED / NOT RUN |
| AC-022-03 | Run RecordingBackend tests on each OS and record separate real-platform readiness rather than claiming complete ports. | External foreign-host lane; harness/evidence | PLANNED / NOT RUN |
| AC-023-01 | Navigate every control with Tab/Shift+Tab/arrows; focus is visible and disabled controls do not dispatch edits. | N01,N07,N12 | PARTIAL / FAILURE OBSERVED — QA-042/043 keyboard rectangle/column dialog, QA-050 invalid font rejected; ISSUE-012/028 folder panel input trapping, ISSUE-024 clipped print controls |
| AC-023-02 | Use IME, clipboard and validation in text fields; cancellation returns focus without leaking preedit into undo. | N01,N07,N12 | PARTIAL — QA-050 validation/cancel preserves12pt; physical IME/preedit/clipboard control matrix NOT RUN |
| AC-023-03 | At 100/125/150/200/250% DPI, hit targets align with paint and popovers stay on-screen. | N01,N07,N12 | PLANNED / NOT RUN |
| AC-024-01 | Use Narrator through open, edit, find, conflict and recovery; names, roles, values and focused state are announced. | N03,N12,N13 | PARTIAL / FAILURE OBSERVED — current UIA observations ISSUE-005/007/011 and recovery ISSUE-009; Narrator NOT RUN |
| AC-024-02 | Change selection/scroll in a 5 GiB file; semantic text-range requests are bounded and do not materialize the entire document. | N03,N12,N13 | PLANNED / NOT RUN |
| AC-024-03 | Enable high contrast and test focus/selection/error distinction without color; real DirectWrite results supplement headless tests. | N03,N12,N13 | PLANNED / NOT RUN |
| AC-025-01 | With all ignore options off, applying all hunks reproduces the target bytes; with ignores on, only explicit nonignored ranges change. | N09,N13; harness/evidence | PLANNED / NOT RUN |
| AC-025-02 | Force normalized-hash collisions and whitespace/case equality; compare normalized bytes, retaining original byte ranges for edits. | N09,N13; harness/evidence | PLANNED / NOT RUN |
| AC-025-03 | Compare divergent multi-GB files under anchor/output caps; distinguish CompletedCoarse, Cancelled and Unavailable without claiming incomplete output is exact. | N09,N13; harness/evidence | PLANNED / NOT RUN |
| AC-026-01 | Preview then externally modify a file; Apply skips it for re-review instead of re-matching and changing unseen contents. | N05 | PLANNED / NOT RUN |
| AC-026-02 | Replace in a mix of dirty open files and disk files; revisions/fingerprints are checked, backups unique and per-file receipts durable. | N05 | PLANNED / NOT RUN |
| AC-026-03 | Kill between replacement and receipt commit; restart reconciles by target/backup fingerprints and never reapplies blindly. | N05 | PLANNED / NOT RUN |
| AC-027-01 | Format JSON containing large integers and exponent lexemes; preserve numeric values and documented lexical policy. | N10; verified package/artifact prerequisites | PLANNED / NOT RUN |
| AC-027-02 | Reject XML DTD/external entities/XInclude and unsupported XPath syntax; network stays inaccessible and depth/time limits hold. | N10; verified package/artifact prerequisites | PLANNED / NOT RUN |
| AC-027-03 | Open Hex original-byte mode on UTF-16 with unsaved text edits; display original generation explicitly and distinguish an encoded edited preview. | N10; verified package/artifact prerequisites | PLANNED / NOT RUN |
## Existing journey and command matrix reconciliation

Use `tests/e2e/journeys.json` as the existing journey step inventory, without treating its manifest or runner as native execution evidence. Every step must record actual observation and screenshot/byte oracle as appropriate. Journey mapping:

| Existing journey | Linked acceptance cases | State |
|---|---|---|
| plain_text: Quick plain-text edit | AC-003-01, AC-004-01 | PLANNED / NOT RUN |
| code_config: Code/config edit | AC-008-01, AC-013-01 | PLANNED / NOT RUN |
| regex_transform: Regex transform | AC-005-01, AC-005-02 | PLANNED / NOT RUN |
| column_multi_cursor: Column/multi-cursor transform | AC-006-01 | PLANNED / NOT RUN |
| huge_log_tail: Huge log search/tail | AC-015-01, AC-019-01 | PLANNED / NOT RUN |
| workspace: Workspace navigation | AC-009-01 | PLANNED / NOT RUN |
| udl: UDL import | AC-008-03 | PLANNED / NOT RUN |
| macro_external: Macro/external command | AC-014-01, AC-014-02, AC-014-03 | PLANNED / NOT RUN |
| split_clone_sync: Split/clone/sync | AC-010-01, AC-010-02 | PLANNED / NOT RUN |
| crash_recovery: Session crash/recovery | AC-004-02, AC-020-01 | PLANNED / NOT RUN |
| extension_isolation: Extension install/crash | AC-016-01, AC-027-01 | PLANNED / NOT RUN |
| portable: Portable usage | AC-018-01 | PLANNED / NOT RUN |
| install_update_rollback: Install/update/rollback | AC-018-03, AC-020-03 | PLANNED / NOT RUN |
The existing release command matrix/runtime inventory is a required additional input. Its exact artifact path has been requested from the desktop coordinator; no source-regex inventory or invented command count substitutes for it. Once located, record its path/hash/commit here and append results against its existing stable IDs. For **every registered command**, preserve three independent slots: success, disabled-state reason/no mutation, and failure or cancellation/no unintended mutation. Run each exposed menu/palette/toolbar/context/shortcut projection, plus keyboard focus return. Commands absent from the current inventory or without executable prerequisites remain unresolved. Resolver output alone is not a native pass.

## Environment-dependent acceptance

These are PLANNED with availability UNVERIFIED, not asserted unavailable without observation. Root records actual available environment and blocker per case.

| Lane | Required environment / evidence | Boundary |
|---|---|---|
| Physical display/input | Actual DPI settings/displays, keyboard layouts, IME, hardware/software renderer and recoverable device-loss facility | Record supported scales; do not change personal settings without coordinated restoration. Screenshots at one scale cannot close mixed-DPI/IME acceptance. |
| Assistive technology | Narrator/NVDA/Accessibility Insights versions and supported Windows floor-build hosts | UIA tree checks do not replace spoken announcements or physical navigation. |
| Printer | Controlled printer or PDF output to QA root | Do not consume personal print jobs; record unavailable device separately from export. |
| Network/remote paths | Controlled share, offline/disconnect/permission fixtures and reviewed package endpoint | Do not use personal shares or arbitrary public servers. No network grant or trust-store mutation is implied. |
| Distribution/security | Clean VM/account, actual signed final artifacts, pinned tools, approved fixture trust and update metadata | Readiness pending if artifacts/VM absent. Do not install/update/uninstall personal app or publish/sign during this campaign. |
| Foreign platform | Linux/macOS runtime hosts and specified native dependencies | Windows evidence does not close AC-022 foreign execution. |
| Performance/large storage | Adequate verified scratch space, pinned release binaries/configuration, dedicated same-machine baseline conditions | Debug UI timings and hosted CI cannot establish comparative claims. Record raw failures/timeouts as well as successful runs. |
| Controlled faults | Existing supported owned-process/disk-full/fault-boundary harness | No new fault injection source or deliberate exhaustion of personal/system disks. If unavailable, retain exact case pending. |

## References and historical regression carry-forward

- `docs/UNLOCK_CHECKLIST.md`: original ordered visual/input/dialog resume checks. Retest every applicable row; historical lock and implementation labels are not current outcomes.
- `docs/implementation/PR-021-NATIVE-20260906.md`: prior hashed debug checkpoints, stale Find UIA focus/value publication, status clipping, modal visibility/tool limitations, preserved user scratch. Never overwrite those fixtures.
- `docs/blueprint/10_ACCEPTANCE_AND_TRACEABILITY.md`: authoritative 81-case register.
- `docs/IMPLEMENTATION_STATUS.md` and owning `docs/implementation/PR-001.md` through `PR-027` notes: implementation/gate evidence remains separate from this campaign.
- `tests/e2e/journeys.json`, `docs/implementation/PR-021.md`, `docs/implementation/PR-021-HEADLESS-PLAN.md`: existing journey, inventory, release and environment contracts.

This document author did not execute native tests. Root observations are linked and scoped in the progress section; failed observations remain reviewable issues, with fixes held for the user's go-ahead.

## Historical evidence reconciliation through QA-017 / ISSUE-013

Source: [root execution log](MANUAL-QA-20260908-RESULTS.md). Tested binary commit51cd4c0, SHA256 `5CC58A14008BD4D4874703FB1511F25A1A1BEA4D200B8AEFEF2AC91E89A10AAE`. Windows26200. This snapshot is not a live mirror of later log additions.

Only AC-004-01 is promoted to a scoped native PASS: save captured text, replace it, then undo restores saved content and clean marker (QA-003/004/005). It does not close save races, crash recovery or full lifecycle acceptance. At that earlier snapshot, fourteen other AC rows had partial evidence; the newer reconciliation below supersedes those counts. Package rows remain unaccepted; the partial progress here does not grant package completion.

| Workflow | Actual scope observed | Remaining / disposition |
|---|---|---|
| N01 | QA-001 startup; QA-006/007 palette/Settings/maximize | PARTIAL: ISSUE-001 chrome/Output pane and ISSUE-003 painted status; exact dimensions, other screens/themes pending |
| N02 | QA-002/003 clipboard/undo/Save As; QA-011/014 exact UTF-16LE/mixed-EOL save | PARTIAL: ISSUE-004 document navigation, ISSUE-008 dirty close modal, ISSUE-010 multi-file Open; cut/IME/remaining encoding coverage pending |
| N03 | QA-004/005 literal Replace All/current UIA text/undo; QA-015 Whole Word | PARTIAL: ISSUE-005 UIA focus/values, ISSUE-006 Escape; backward/wrap and regex/Extended pending |
| N04 | QA-008/009 clone edits/shared undo/independent selection/divider | PARTIAL: ISSUE-007 active-secondary UIA; multicaret/rectangle/transfer/history/folds/scroll still pending |
| N05 | Open-document search attempted | BLOCKED subflow: ISSUE-012 query typing mutates editor; do not repeat identical attempt. Folder/regex/reviewed replacement remain separate pending workflows |
| N06 | QA-012/013 Rust highlighting/language switch; QA-016 basic workspace tree | PARTIAL: ISSUE-011 language-picker theme/focus; completion/folds/UDL/outline/map pending |
| N07 | QA-006/007 settings font/theme/palette/focus; QA-010 persistence | PARTIAL: toolbar/keymap/context/resource policy coverage pending |
| N11 | QA-010 restart after owned process termination | FAILURE OBSERVED: ISSUE-002/009 recovery; close protection ISSUE-008. Do not claim crash recovery or safe dirty close |
| N13 | QA-016 24MiB load/index/wheel; QA-017 edit/undo/clean state | PARTIAL: ISSUE-013 scrollbar and ISSUE-004 Ctrl+End; no dirty save, huge/20MiB-line/memory/comparative acceptance |

### Prioritized next native work without duplicate attempts

1. Power editing: multicaret and measured rectangle insertion/undo on a small generated fixture; then same-document/cross-document drag-copy/move and linked undo/redo. Existing clone basics need not repeat.
2. Language workflow: completion accept/cancel/stale suggestions, comment/bracket/indent; fold/unfold and source-preserving edits across a large hidden span. Rust color detection/language switching already observed.
3. Unvisited command UI: toolbar show/customize/focus, shortcut mapper conflict/remap/reset, workspace Documents/Outline/Map; use static [command catalog](MANUAL-QA-20260908-COMMANDS.md). Do not repeat the broken open-doc query route.
4. Compare/utilities: two small fixtures, hunk navigation/ignore/merge/undo; selected hash/transform/export. Print is prerequisite-dependent.
5. Macros/processes and watch/tail: harmless local fixture commands, record/replay/cancel; generated append/pause/unlock/rotation. No arbitrary processes or personal files.
6. Search variants in the working Find surface: Extended/regex/backward navigation and explicit incomplete state; independently assess folder search before attempting reviewed replace. Record new routing failure once and move on.
7. Verified extension manager/JSON/XML/Hex only if packages are available; otherwise record prerequisites. CLI/IPC private-root read-only/line-column/multiple paths can be assessed independently of the failed file dialog.
8. Physical IME/AT/high contrast/scales and controlled performance/fault/foreign-host acceptance remain separate prerequisite lanes. Preserve existing recovery/modal failures for the issue list; no fixes or repeated destructive recovery attempts are implied.

## Historical reconciliation through QA-034 / ISSUE-024

Read-only mapping of the root log; no new native execution by this author. AC-004-01 remains the only complete scoped native case. No new full acceptance PASS is earned by QA-018–034. Unchanged/unexecuted AC rows retain NOT RUN. Earlier prioritized list is historical; use the remaining list below.

| Added evidence | Coverage and remaining scope |
|---|---|
| QA-018/019 | Extension manager empty state observed; Discover failed ISSUE-014. Actual extension execution BLOCKED by absent compiled trust and unsigned/expired fixture artifacts. No install/trust bypass/rebuild. |
| QA-019/020/025 | Long line and1GiB first view partial; long-line End/index failed ISSUE-015. Cancel File Operations removed pending1GiB tab, UI usable. No full1GiB load/large-save/tail/benchmark completion. |
| QA-021/022 | Two carets, selected occurrence editing and single undo observed. Rectangle,10k carets and drag transfers remain untested; ISSUE-017 blocks pointer selection setup. |
| QA-023 | Selected Base64 encode and one undo restored text/selection. Hash/decode/export/print remain distinct tests. Utility Escape failure remains ISSUE-006. |
| QA-024/025 | Recording/stop and playback cancellation observed. Playback completion not proven; concurrent large-file work makes attribution uncertain. |
| QA-026 | Toolbar Find, Extended hex escape and backward match observed. Toolbar hides painted tabs ISSUE-018. Full regex contexts/capture expansion remain untested. |
| QA-027 | Disk compare, two differences, Next and options exposure observed. Mislabeled pane tabs/top clipping ISSUE-019. Merge, ignores/recompare/cancel not yet established. |
| QA-028/029 | Read-only Save failure preserved original hash and buffer; Windows1252 fallback exact bytes preserved. Not dirty-save success or forced invalid-UTF8 interpretation. |
| QA-030–034 | Mapper restores F3/focus; document list activation; Rust outline/comment toggles; fixture reset/save exact hash. Invalid shortcut accepted ISSUE-020, already-open workspace activation ISSUE-021, language override loss ISSUE-022, completion Tab mutation ISSUE-023. |
| ISSUE-024 | Print button no dialog/progress and Shift+Tab reaches underlying editor. Printing native route blocked before printer availability can be assessed; do not classify this only as unavailable hardware. |

### Remaining executable native workflows — priority order

1. **Compare merge/undo and options:** reuse loaded small comparison, explicit target labels/bytes, one hunk copy then undo; ignore/recompare/cancel. Do not repeat basic opening/Next already observed.
2. **Macro replay without competing load:** small scratch record/replay after no file work is pending; establish actual terminal/byte result. Existing cancellation need not repeat.
3. **Small saved-log tail/watch:** append/pause/unlock/rotate controlled fixture; this avoids unfinished1GiB indexing. Record SourceChanged and owned edit preservation.
4. **Power gaps:** rectangle setup by an available keyboard route, multi-range transform, bookmark navigation/commands. Pointer drag transfer remains dependent on working selection/hit input; if blocked, record once without duplicating ISSUE-017.
5. **Search gaps:** regex/captures/empty matches in the working Find surface; folder query input only if independently reachable, then reviewed replace/stale-file handling. Open-document query typing already failed ISSUE-012; no repeat required.
6. **Language/navigation gaps:** fold/unfold/large seam, completion Enter versus advertised Tab, bracket/smart-indent behavior; outline navigation/map and per-pane state. Preserve existing Tab failure rather than retrying it unchanged.
7. **Utilities and configuration gaps:** hash/decode/export to scratch; toolbar customization/focus; invalid Settings values/resource limits with saved fixtures. Print routing already blocked ISSUE-024.
8. **CLI/IPC and session navigation:** generated multiple/non-BMP paths, explicit line/column/read-only and private-root handoff. Recovery/modal-close failures already recorded; no repeat forced exit needed just to reconfirm.

### Separate prerequisites / unavailable scope

- **Known blocked now:** signed extension/runtime execution (QA-019 trust/artifacts); printing route (ISSUE-024), open-document search route (ISSUE-012), reliable close/recovery flow (ISSUE-008/009). These are distinct from unknown environment availability.
- **Availability still unverified:** physical IME/AltGr/dead keys, additional DPI/displays, Narrator/NVDA/high contrast, controlled network share and clean Windows floor-build VM. If absent, state the exact missing environment; do not infer a pass.
- **Dedicated later acceptance:** pinned release comparative benchmarks, adequate multi-GiB storage/measurement, exact durable fault-boundary harness, Linux/macOS host execution, signed installer/update/rollback artifacts. No builds, publication, trust changes or fixes are authorized by this list.

## Historical reconciliation through QA-053 / ISSUE-030

This section supersedes the earlier next-work lists and blocked-print description. Source remains the root [results log](MANUAL-QA-20260908-RESULTS.md), through QA-053 only. AC-004-01 remains the single fully satisfied scoped native case; no additional full case is promoted. No results file/source/build/UI action by this author.

- **Printing is partially successful, not wholly blocked:** QA-035 produced and viewed a39,469-byte Microsoft Print to PDF artifact with Rust text, line number and header. ISSUE-024 is clarified: maximization made the print action reachable; initial separate-dialog visibility was automation ownership. Default-size clipping/input-routing observations remain; full page layout/export/theme/printer-failure scope remains untested.
- **Workspace/tail:** QA-036 create folder/file and open/save worked. QA-037 small-log follow/append/pause/resume and rejected typing worked. ISSUE-025 unlock wording misleading; QA-038/039 monitored-file rename/delete did not complete (ISSUE-026). Do not repeat these exact monitored-file attempts.
- **Power/folding/settings/map:** QA-041 bookmark toggle/navigation; QA-042/043 two-row rectangle/repeated text with single undo; QA-049 small Rust fold/unfold; QA-050 invalid font rejected/cancelled; QA-051 small map sampling/click. These do not establish wide/tab/short-line columns, large-fold mapping, all settings or large-map behavior.
- **Search/macro/run:** QA-044 numeric regex plus malformed-query refusal worked. Folder input route QA-045 fails as ISSUE-012; ISSUE-028 panel cannot dismiss and routes Tab to document. Macro playback still pending without loading QA-040/ISSUE-027. Run result unobserved QA-047; fresh-profile modal/input freeze QA-048/ISSUE-029 requires direct human reproduction. No process completion or process-output cancellation claim.
- **About/session:** QA-052 About worked; notice file absent because executable-only QA copy lacks distribution assets. QA-053 clean exit wrote caret/fold session state, but two launches yielded no surviving process/window: ISSUE-030 saved-session reopening BLOCKED, launcher versus startup cause unresolved. Dark restart persistence is not passed.

### Compact remaining executable manual sweep

All items below are NOT RUN or incomplete as explicitly stated. First obtain a stable targetable QA window; ISSUE-030 means ordinary UI work cannot proceed in that instance until launch availability is established. This is an execution prerequisite, not permission to fix/build. Avoid another broad screen sweep.

| Priority | Exact remaining small-fixture case | Expected observation / stop condition |
|---|---|---|
| 1 | Existing two-file compare: copy one explicit hunk into destination, undo once; toggle one ignore option and recompare, then cancel a separate comparison | Exact target bytes/dirty state and undo; completed versus cancelled distinct. Stop if labels make target ambiguous (ISSUE-019); do not repeat opening/Next. |
| 2 | Rectangle fixture containing tab, wide character and short line; type one character then undo | Measured columns and padding correct across all rows, one undo. Basic two-row rectangle already passed, so use these missing boundaries only. |
| 3 | Keyboard-selected scratch range: attempt same-document drag-copy/move and cross-document transfer only if pointer drag can start | Exact source/destination bytes and linked undo/redo including one closed peer. If ISSUE-017 prevents drag setup, mark blocked once; no repeated pointer workaround sweep. |
| 4 | Loaded Rust scratch: completion accept with Enter, smart bracket/indent; large hidden fold seam only if readily prepared | Distinguish Enter from known failing Tab; matching bytes/undo. Basic gutter toggle/comment commands already covered. |
| 5 | Working Find bar: regex capture replacement, empty-match progress and case/backward behavior on bounded fixture | Correct replacement/one undo, no infinite empty match, explicit invalid/incomplete result. Folder/open-document replacement remains blocked by ISSUE-012/028, not a new attempt target. |
| 6 | Utilities: selected SHA256 oracle, Base64 decode inverse, HTML or RTF export to new scratch path; inspect existing PDF full page | Correct selected data/output without modifying unrelated bytes. Encode/undo and PDF production already observed; no duplicate printer invocation needed. |
| 7 | Saved scratch small log: truncate/rotate/same-size rewrite after independent follow setup | SourceChanged/reload warning and owned edits retained. Append/pause/resume already observed; monitored rename/delete already failed. |
| 8 | Scratch settings: resource limit lowering with saved text/history; toolbar customization/focus; invalid setting cancellation where not already observed | No text loss; bounded policy visible/persisted; correct control focus. Font invalid-value case and mapper restore already covered. |
| 9 | Private CLI/IPC launch: spaced/non-BMP path, two paths, explicit read-only/line/column | Correct target tabs/location/flags; no personal instance interaction. Subject to targetable launch prerequisite; separate from failed multi-select Open dialog. |
| 10 | Clean, unmonitored disposable workspace file rename then retain-for-undo delete/undo | Separates normal workspace lifecycle from failed previously monitored case. Use new fixture only; preserve prior watch evidence. |

Do not repeat completed small startup/clipboard/Save As/UTF16LE/mixed-EOL save/Find ReplaceAll/settings persistence/clone edit/basic toolbar Find/Document List/outline/comment/map display checks. Macro replay and Run need direct-human confirmation of already-recorded failures, not another automated attempt. Dirty close/recovery and saved-session reopening remain unresolved issues.

### Environment-required or blocked acceptance beyond the sweep

| Scope | Concrete prerequisite / current boundary |
|---|---|
| JSON/XML/Hex and extension lifecycle | Build with legitimate compiled trust plus nonexpired verified signed runtime/extensions. Current build/artifacts cannot execute this lane; no bypass or rebuild. |
| Narrator/NVDA and physical input | Available screen reader, actual IME/keyboard layouts, operator able to hear announcements/observe candidate windows. UIA focus evidence alone is insufficient. |
| Mixed DPI/device loss | Available displays/scales or approved temporary scale setup, restoration plan and supported device-loss route. Existing maximize capture is not mixed-DPI acceptance. |
| Release/license/install/update | Complete distribution assets (About notices absent from executable-only copy), pinned verified signed package, clean VM/account and reviewed update fixtures. No personal install, signing or publication. |
| Huge files/performance | Stable isolated app, adequate scratch storage, existing generator/measurement tools and pinned comparable release baseline.1GiB first viewport/loading cancellation and24MiB edit do not close full-load/5GiB/performance cases. |
| Fault/recovery | Existing controlled durable-boundary harness and owned process/storage fixtures. Current recovery/close issues002/008/009/030 remain failed/blocked observations; random repeated process death does not cover boundaries. |
| Remote/foreign systems | Controlled disposable share for disconnect/permissions and Linux/macOS hosts for actual platform contracts. Neither can be inferred from Windows local-file tests. |
| Folder/open-doc replacement | Working query input and dismiss route; currently ISSUE-012/028 prevents execution. Record dependent replacement scenarios blocked, not passed or silently omitted. |

## Historical reconciliation and sweep boundary through QA-056

Root results add QA-054 UTF8 BOM exact-byte save and QA-055 unrepresentable Windows1252 conversion refusal with unchanged original hash. These extend encoding coverage without closing every codec case. QA-056 reproduces ISSUE-012 in Compare: palette query typing edits the left document; coordinator confirms Undo restored clean and no disk save. Merge workflow is **blocked**, not an unattempted task to retry through the same palette. The PDF artifact reviewer, as reported by root, confirmed one-page print layout/source preservation; no new full AC-017-03 pass because export/options/failure recovery remain incomplete.

A new empty portable profile launches while the saved-session profile twice fails (root restart differential). Ordinary independent checks may use that already-working isolated profile; saved-profile restart acceptance remains blocked ISSUE-030.

### Minimum practical finish list for this manual sweep

“Minimum” defines useful remaining independent smoke coverage, not full release acceptance. Each item needs one controlled execution or an explicit observed blocker; no 387-command combination sweep is implied. Stop a route once it repeats the known input-routing fault, undo incidental scratch mutation when safe, and record the dependency blocked.

1. **One successful dirty Paged save/reopen:** the24MiB file has edit/undo coverage but no dirty save. Use a disposable copy, make a small edit, save, verify expected bytes/hash and reopen if the current profile can do so. If recovery/source or launch failure prevents this, report blocked rather than repeat crash tests.
2. **One regex replacement with captures:** working Find surface only; verify exact replaced text and one undo. Invalid query and numeric matching already covered. Stop if field typing reaches editor.
3. **One utility export plus independent hash:** export a small Unicode fixture to HTML or RTF and inspect output; run selected SHA256 against a known oracle. Base64 encode/undo and PDF layout are already covered.
4. **One small-log source replacement event:** rotate or truncate a disposable followed log; check SourceChanged and owned text preservation. Append/pause/resume already covered, so no throughput run.
5. **One CLI smoke:** two generated paths including spaces/non-BMP, with explicit read-only/line-column options as supported; verify intended tabs/positions and private root. Saved-session profile must not be treated as working solely because fresh profile starts.
6. **One ordinary unmonitored workspace rename/delete/undo:** new disposable file; distinguish basic lifecycle from failed formerly monitored watch.log. Stop after a clear failure.

### Optional variants after the minimum, not reasons to prolong a failing sweep

- Rectangle tabs/wide glyphs/short-line boundaries,10k carets, numeric Column Editor and other bookmark/line commands.
- Drag-copy/move and linked undo if pointer selection can start; presently dependent on ISSUE-017. Compare merge is blocked by QA-056; do not keep probing equivalent palette routes.
- Completion Enter, smart brackets/indent, large-fold seam, large-map click and toolbar customization. Basic discovery/folds/map/toolbar already have observations; known Tab/theme/layout defects remain.
- Empty regex matches, additional codecs, Base64 decode, secondary export format and repeated theme/shortcut variants.
- Resource-cap lowering/retention and clipboard-history limits require saved isolated fixtures and clear expected state; no stress-memory exhaustion is needed to finish this sweep.

### Acceptance that remains outside this practical boundary

Known broken routes (recovery/dirty close/saved-profile restart, folder/open-doc search and replacement, compare palette, macro completion, Run modal) remain failures or blocked/direct-human-reproduction items. Do not infer success from inability to execute. Signed extensions require legitimate trust/runtime artifacts; notices/install/update require complete distribution/clean VM; AT/IME/DPI require actual tools/layouts/displays; network requires disposable controlled share; foreign platforms require real hosts; comparative performance and durable fault boundaries require their dedicated existing harness/environment. No fixes, builds, trust bypass, publishing or additional environment changes are authorized by this document.

## FINAL — practical sweep complete through QA-066 / ISSUE-032

**Practical native sweep complete; product/release acceptance remains blocked.** This is the finite tested workflow sweep, not full coverage of all81 acceptance cases or all387 static command rows. Unexecuted optional variants and environment-dependent cases remain unexecuted. No fixes, source edits, builds or desktop actions by this documentation author. Root RESULTS remains authoritative and unchanged.

| Final follow-up | Final scoped outcome |
|---|---|
| QA-057 compare merge | Native Tools menu right-to-left first-hunk merge and single undo passed; clean original left buffer and both unchanged disk fixtures. This supersedes “merge blocked” in earlier sections. Only palette route remains failed QA-056/ISSUE-012. Ignore indicator applied; whitespace-only suppression/cancellation remain untested. |
| QA-058 PDF | Independent artifact review passed: one Letter612×792 page, all61 source characters, complete syntax text/line number/header/Page1 footer, no clipping/overlap or out-of-page characters. Evidence rust-print-review.png/.md. No need for another basic print/layout check. |
| QA-059/060 Paged dirty save | Full25,165,833-byte oracle exactly matched original plus9-byte marker, SHA256 dea694e7d68ec701bfb5308d71b7ee4e4ae5a59ee160e6e961b87d3d2831cff5; close/reopen showed marker. ISSUE-031 unexpected own-save Source changed warning remains; repeated recovery OS32 ISSUE-002 remains. Bytes passing does not clear these notices/recovery failures. |
| QA-061 regex replacement | item([0-9]+)→value$1 replaced both matches with Unicode suffix preserved; one undo restored original. Missing regex variants remain explicitly partial in AC-005-02. |
| QA-062 hash |20UTF8-byte Unicode utility SHA256 matched independent Node oracle exactly. |
| QA-063 export | HTML export failed OS32 at new destination; no file produced, source preserved. ISSUE-032 HIGH remains; practical export attempt is complete as FAIL, not a task to retry until a fix is authorized. |
| QA-064 workspace lifecycle | Explicit close-first guard observed; after close, rename succeeded, retain-for-undo delete removed path, undo restored exact content. Earlier ISSUE-026 open-file no-ops are explained by this guard and are not confirmed corruption/mutation failures. |
| QA-065 watch | Rename/replacement and truncation detected; old captured content retained, explicit Reopen and follow loaded new bytes. No silent switch/loss observed. |
| QA-066 CLI | NOT RUN: approved launch tool accepts executable path only, no arguments. Direct human/operator launch with documented two-path/non-BMP/line-column/read-only/IPC matrix required. No shell workaround/build is implied. |

### Closure boundary

The six minimum finish-list items in the previous section now have a disposition: Paged save PASS scoped; capture replace PASS; hash PASS/export FAIL; rotation/truncation PASS; ordinary closed-file lifecycle PASS; CLI NOT RUN with concrete operator prerequisite. There is no remaining minimum automated/native-tool task in that list to repeat.

AC-004-01 saved-state undo remains the only acceptance row promoted to scoped complete native evidence. QA-057 merge/undo itself passes, but AC-017-01 remains partial because its full source-label/selection requirements are not closed and ISSUE-019 remains. No additional full case is promoted. Current-script extraction counts, source completion, prior unit tests and screenshots must not be used to promote unexecuted AC rows or static command slots.

Acceptance remains blocked by documented recovery/close/startup, input-routing/focus, macro/Run, HTML export and other unresolved issues in RESULTS. Exact severity and attribution follow that log; modal/launcher concerns retain their direct-human-reproduction qualification. The merge and workspace clarifications above must accompany any summary so superseded blockage/no-op interpretations are not reported as current confirmed failures.

### Explicitly unexecuted optional or prerequisite follow-ups

- Optional variants: tabs/wide/short-line rectangle boundary matrix;10k carets; drag-copy/move and linked cross-document transfer; completion Enter/smart typing variants; large-fold/map interaction; toolbar customization; resource/clipboard limits; remaining regex contexts/empty matches; additional codecs; decode/RTF/extra export options. These remain NOT RUN, not waived acceptance requirements.
- Direct operator: CLI/IPC argument matrix; confirm modal/Run and saved-session startup behavior where automation attribution is uncertain.
- Legitimate environment/artifacts: trusted signed extension/runtime packages; full distribution notice assets and signed installer/update clean-VM fixtures; real AT/IME/layout/DPI hardware; controlled network shares; Linux/macOS hosts; dedicated comparative performance and durable fault-boundary setups.
- Rechecks after separately authorized fixes: specific failed routes and their affected workflows. Current authorization remains testing/documentation only; this document does not authorize repair or imply product readiness.

