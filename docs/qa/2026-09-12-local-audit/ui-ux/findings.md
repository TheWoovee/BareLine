# UI/UX and user-journey audit — 2026-09-12

## Scope and evidence

This audit covers the current `Bareline-Editor` working tree at master `51cd4c0` plus current uncommitted changes. Manual native observations used an isolated copy of the first rebuilt binary, SHA-256 `C6B2318BCB92AAA47525320689971C690550C52358FB09CC7716409EB9F4C58F`. A final `xtask journey all` used the later rebuilt `target/debug/bareline.exe`, SHA-256 `E3F283C8CC61CCAE5D1549A1136B899D02ABF3CF95A27C9FC7BE8A42D7128702`. All files, profile/session state, and executed command output were under a generated scratch root.

Evidence labels mean: **native observed** was directly seen in the current Windows executable; **native journey** was asserted by `xtask`; **source confirmed** was traced in current source after Graft; **unverified** means no end-to-end result is claimed. Raw UIA trees/screenshots and logs are in [evidence](evidence/).

## Findings

### UX-01 — Save As/Save Copy cannot overwrite an existing destination after confirmation

**Severity: High. Native observed and source confirmed. Owner: root PR-T03.**

Choose an existing destination in Save As, confirm Windows' overwrite prompt, and the destination remains unchanged. The tab stays dirty and Bareline reports `File changed; destination was preserved`, leaving the intended payload in a `.bareline-9612-1.tmp` staged file. This makes a normal overwrite workflow unusable and requires users to recover from an unexpected temporary file. The exact observation is retained in [save-as-overwrite-transcript.txt](evidence/save-as-overwrite-transcript.txt); the transient dialog image was unavailable after the sanctioned controller reset.

`Workspace::save_internal` supplies an expected fingerprint only for the current same path (`crates/app/src/workspace.rs:2080-2088`), while `save_bytes` treats `expected == None` as destination-must-not-exist (`crates/file-io/src/lifecycle.rs:725-739`). Direct Save As also calls `save_file()` without a suggested name (`apps/bareline/src/windows_app.rs:2069-2088`), unlike dirty-close saving (`:1333-1357`), and the first dialog opened in `C:\Windows\System32` with a blank name. PR-T03 owns overwrite intent, initial folder, and suggested name.

### UX-02 — Modal input/focus ownership can silently edit the background document

**Severity: High. Native observed and source confirmed. Brief: PR-U01.**

Run and Go To draw active fields but publish no UIA nodes and report `Editor` focused. UIA text sent to either surface changes document bytes behind the prompt. Run also remained open after two Escapes. Compare Options exposed the same safety failure: it stayed open after two Escapes, its visible Done control was missing from UIA, and a background click plus Ctrl+Z changed the destination document while the modal still covered it. These failures can corrupt work for screen-reader, dictation, automation, and keyboard users.

Evidence: [run-prompt-hidden-uia.jpg](evidence/run-prompt-hidden-uia.jpg), [run-prompt-hidden-uia.txt](evidence/run-prompt-hidden-uia.txt), [goto-hidden-field-misroute-and-duplicate-status.jpg](evidence/goto-hidden-field-misroute-and-duplicate-status.jpg), [compare-options-background-edit.jpg](evidence/compare-options-background-edit.jpg), and [compare-options-background-edit.txt](evidence/compare-options-background-edit.txt).

The accessibility snapshot has no Run/Go To groups (`apps/bareline/src/windows_app/accessibility.rs:93-413`); those surfaces keep private fields/raw handlers (`run_prompt.rs:11-220`, `goto.rs:15-279`). Compare dispatch has its own `options_open` key handling (`windows_app/compare.rs:1491-1640`) rather than one enforced shell modal owner.

### UX-03 — Search, Recovery, and split-pane accessibility identity is incomplete

**Severity: High for assistive input; Medium for pane/recovery orientation. Native observed and source confirmed. Briefs: PR-U07 and PR-U08.**

Find/Replace fields appear in UIA, but the focused element remains Editor. Clicking a field through UIA and then using assistive `type_text` changed the selected document; raw key events changed the visually focused field. The visible Find/Replace/Files tabs are absent. Recovery Center leaves the background editor reported as focused/active. Two split panes expose pane-qualified tabs but only one generic Editor provider. Users cannot trust where assistive input will land or identify which pane owns caret/selection.

The snapshot defaults to editor id 2, only uses the active workspace editor as its text source, and appends surface semantics without a single ownership contract (`accessibility.rs:86-152,208`). PR-U07 owns search/recovery semantics after PR-U01; PR-U08 owns per-view editor providers and stable split identities.

### UX-04 — Messages are duplicated and actionable details are clipped

**Severity: Medium. Native observed and source confirmed. Brief: PR-U02.**

Recovery progress and Save As conflict text appeared simultaneously in the full-width message area and a lower-right toast. Long staged-file paths were clipped. This increases noise and can hide the only recovery location after a failed save. The duplicate progress is visible in [goto-hidden-field-misroute-and-duplicate-status.jpg](evidence/goto-hidden-field-misroute-and-duplicate-status.jpg).

`on_redraw` copies `workspace.message` into toasts while the original region remains rendered (`windows_app.rs:3056-3066`). Toast severity is inferred from English substrings and text is constrained to one 40-pixel row/380-pixel box (`toast.rs:36-100,153-204`). The fix must discard only stale UI progress when a tab closes; save conflicts, recovery cleanup failures, and process outcomes need operation/global ownership and must survive tab lifetime.

### UX-05 — Popup menus ignore the dark theme and core menus exceed the viewport

**Severity: Medium. Native observed and source confirmed. Briefs: PR-U03 and PR-U04.**

Dark editor/title surfaces open light-gray native popups. File reaches the bottom edge and scrolls at 1200×800; Search exposes roughly thirty actions, including irrelevant state-dependent commands, without consistently visible toggle state. This harms discoverability and forces routine file/search work through dense menus.

The static tree is at `crates/app/src/menus.rs:208-240`; its tests cover reachability rather than fit. `MenuBar::attach` owner-draws only top-level positions and applies colors at the root (`crates/platform-windows/src/menu_bar.rs:32-72,121-142`).

### UX-06 — Recovery Center presents an in-flight discovery as a final empty state

**Severity: Medium. Native observed and source confirmed. Brief: PR-U05.**

Opening Recovery Center soon after editing showed `No recovery checkpoints found`; closing immediately announced one checkpoint, and reopening displayed it. A user can abandon recoverable work because the UI calls pending discovery empty. `recovery.open` has no loading state (`windows_app/recovery.rs:200-207`), rows publish later (`:593-609`), and the draw path infers empty from the current row list.

### UX-07 — Macro Manager's visible Close button is disabled when empty

**Severity: Low. Native observed and source confirmed. Brief: PR-U06.**

The empty manager owns focus correctly, yet its visible Close button cannot be invoked; only Escape works. The manager draws from cached `self.command_context` (`windows_app/macros.rs:402-415`), whose only assignment occurs later in `macros_pump` (`:973`), while annotation marks close not applicable while the manager is closed (`:382-387`). Recompute/invalidate context on manager open/close.

### UX-08 — Two current native journey assertions are intermittent or environment-sensitive

**Severity: Test/release confidence; product severity unassigned. Native journey evidence. Owner: root T09.**

The first suite was 12/13: `p0-7` failed to observe the dirty-close prompt, then passed three isolated reruns; manual Save/Don't Save/Cancel behavior also passed. The final E3-hash suite was 11/13: `p0-7` failed again with `no save prompt appeared...`, and `p4-4` reported `no colourful emoji glyph detected (saturation before 165, after 165)`. The other eleven journeys passed. Earlier native Unicode/emoji rendering and the earlier `p4-4` pass conflict with the saturation result. Preserve both assertions as timing/capture versus product investigations until diagnostic evidence identifies the failing layer. Logs: [journey-current-binary.log](evidence/journey-current-binary.log), [journey-all.txt](evidence/journey-all.txt), [journey-p0-7-reruns.txt](evidence/journey-p0-7-reruns.txt).

### UX-09 — Find results survive a document switch and misrepresent the active file

**Severity: High. Native observed and source confirmed. Brief: PR-U09.**

After a paged file produced 3,097 `needle` matches, switching to a resident file retained `needle` and `3,097 matches`. Changing the query to `alpha` left the stale count visible and Replace actions unavailable. Users may navigate, replace, or abandon work based on another document's results. The exact transcript is in [stale-find-document-switch.txt](evidence/stale-find-document-switch.txt).

`Views::select_tab` changes the view and `app.active` without synchronizing Find (`windows_app/views.rs:1475-1498`). The workspace owns one `FindController` with shared pending/results/status (`crates/app/src/find.rs:28-50`). Results and actions need a document identity/revision/query/options key.

### UX-10 — Settings Revert becomes a visible no-op after immediate autosave

**Severity: Medium. Native observed and source confirmed. Brief: PR-U10.**

Changing Font size from 12 pt to 9 pt saved immediately; pressing the still-visible Revert control left 9 pt in UI and `settings.toml`. Confirmed Reset Section restored 12 pt. Users cannot undo an accidental preference change despite an active-looking Revert affordance. `acknowledge_saved` advances the saved baseline and `revert()` copies that same baseline (`crates/settings/src/persistence.rs:105-136`).

### UX-11 — Three deferred presentation contracts remain current

**Severity: Medium/Low. Native/source review; briefs: PR-U11, PR-U12, PR-U13.**

Search, Compare, and Output still use three incompatible workspace layouts instead of the specified shared bottom dock, causing large layout/focus changes. Find's compact toggles still lack tooltips, leaving `Aa`, `ab`, and Extended semantics hard to learn. Large-file estimated gutter numbers still rely on faded color rather than italic/tooltip/non-color explanation. Current implementation status explicitly records each deferral; the audit retains separate implementor-ready briefs rather than merging them into unrelated fixes.

## Positive current behavior

- New resident save succeeded, subsequent edit/Ctrl+S updated exact disk bytes, and reopen content was verified.
- Save All cancellation retained the dirty Untitled and reported `0 saved; 1 cancelled or closed; 0 failed or changed during save`.
- Dirty-close Escape canceled closing; Don't Save closed the scratch tab and preserved its last saved bytes.
- Resident Replace All changed two matches and Undo restored the original. Cancel reported cancellation.
- A 3,162,037-byte UTF-8/CRLF fixture opened, rendered Unicode/emoji, found 3,097 matches, split horizontally, accepted a paged edit, saved, and matched exact disk prefix/size.
- Macro record/playback reproduced a recorded edit and reported completion.
- Explicit `C:\Windows\System32\cmd.exe /d /c echo BARELINE_AUDIT` ran after native confirmation, exited 0, and displayed output; evidence is [external-command-output.jpg](evidence/external-command-output.jpg).
- A session-enabled private relaunch restored two clean tabs in order and restored the active document.
- Compare found one difference, copied right to left as a normal dirty edit, and Undo restored the destination.
- Settings search/focus, command palette filtering/disabled reasons, and confirmed section reset worked.
