# Remaining issue closure — 2026-09-08

Status: confirmed reproduced defects addressed; combined automated and focused native verification complete. Direct end-to-end UIA properties and the unexecuted coverage listed below remain qualified.

Scope: remaining failures and partial findings from RETEST-NATIVE-20260908.md, plus the paged-spill cleanup failure in RETEST-AUTOMATED-20260908.md. Existing local fixes and user ignore files are preserved. All implementation agents use Astra at low effort.

## Combined validation plan

Run one full workspace automated batch and one native executable build after the implementation group freezes. Repeat only checks justified by an actual failure or subsequent change.

Native checks use a fresh isolated portable profile and scratch fixtures:

| Issue | Required evidence |
| --- | --- |
| 005/011 | Focused custom query/panel fields expose correct UIA focus and value; editor content remains unchanged. |
| 008 | Dirty Close shows an owned confirmation; No preserves document and permits further input; Yes closes only scratch document. |
| 009 | Saved-file recovery checkpoint preview and comparison work; recovered bytes match the protected edits. |
| 012 | Palette typing, Paste and Select All stay in palette in both ordinary split and compare. |
| 015 | Giant-line End reaches exact column and collapsed UIA selection is available. |
| 019 | Compare tab labels match pane documents; options cover underlying scrollbar. |
| 028 | Folder search cancellation, close command, Escape and application close work without document edits. |
| 030 | Clean exit persists session; relaunch restores selected document and caret. |
| 031 | Own paged save preserves exact bytes without false warning; real external writes are still detected. |
| Additional rendering | Six-digit gutter remains separate from text; Macro/Run does not leave duplicate footer or blank reserved region. |

## Results

The full workspace batch produced 536 unique passing outcomes after two reviewed fixture corrections and exact reruns; seven tests remain ignored. A subsequent correction batch passed five focused regressions and rebuilt successfully. No second full workspace suite was run. See [automated evidence](REMAINING-AUTOMATED-20260908.md) for commands, original failures, corrections, logs and executable hashes.

Native checks passed for split/compare palette isolation (012), dirty tab/application close with No and Yes (008), recovery preview/comparison/recovered-copy bytes (009), asynchronous giant-line column publication (015), compare labels/options (019), folder configuration cancellation/close routes (028), simple noncompare session caret restoration (030), and own-save versus genuine external-write detection (031). Six-digit gutter and Macro/Output footer rendering passed in the stated scope. Recovery saved-copy bytes matched the independent 46-byte oracle and the original disk file remained unchanged. See [native evidence](REMAINING-NATIVE-20260908.md) for both build-specific passes.

Final native executable SHA-256: `f0674363759ab07d4323edba427d5386aa78765c4e03d466e2d88105a508923c`.

005/011: production snapshot and active accessibility bridge both publish Find focus ID 6000 with the correct value length, enabled and focusable state. Sky still reports aggregate Editor focus. This is not a demonstrated remaining application defect, but direct end-to-end UIA GetFocus/HasKeyboardFocus/Value verification remains unconfirmed. No speculative provider change was made. Diagnostics are metadata-only and gated by the QA feature.

Other explicit coverage limits: direct collapsed UIA GetSelection, cancellation of an actively running folder-search worker, comparison-session restoration, and all sidebar/Output/window-size combinations were not independently established by this focused native pass. These are not reported as passing. Prior ignored release-artifact/performance tests remain ignored.

All implementation agents used Astra at low effort. Changes are local; no commit, remote push or publication was performed. Existing user ignore-file changes were preserved. Final whitespace diff check found no errors.
