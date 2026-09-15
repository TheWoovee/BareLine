# QA fix validation — 2026-09-08

Validation of the source batch through ISSUE-013's resident-scrollbar correction. Menu/long-line completion remains a subsequent source batch. Historical QA results are preserved.

## Automated evidence

- Workspace library batch: target/qa-fixes-batch-libraries.log; remaining targets: target/qa-fixes-batch-remaining.log. Passing targets were retained rather than repeatedly rerun.
- Native, editor and Windows corrections: target/qa-fixes-final-targets.log — 36 native, 56 editor and 50 Windows tests passed; two Windows tests ignored. Nested child reruns are not additional coverage.
- After resident scrollbar wiring: target/qa-fixes-scroll-final-tests.log — native/editor tests passed. Native build passed in target/qa-fixes-scroll-final-build.log, with three warning groups.
- Initial compile failures were corrected. The accessibility golden received only 32 primary-equivalent selection arrays and six corrected compare Editor bounds. The paged spill test failed at fixture deletion after behavior assertions passed; a worker completion barrier makes handle release precede deletion.
- ISSUE-032's focused native regression first reproduced OS32 at commit. The corrected directory guard passed new and replacement export with exact content checks. Path checks and sharing restrictions remain intact.

Validated executable SHA-256: 7e3d8e9041c572f5ec6bbbc38515494ab206a6169b12eb847287ad3ea365af32

## Focused native checks

| Issue | Actual result |
| --- | --- |
| 001/003 | Branded icon, no empty startup Output, live status values observed; white native menu pending subsequent fix. |
| 002 | Unicode typing in fresh portable profile produced no original recovery sharing-error banner. |
| 006 | Utility result Escape passed; Find Escape initially failed, then passed in corrected build. |
| 013 | Original drag and track-click failure reproduced. Corrected build thumb drag moved viewport from line 1 to approximately line 318,258 while caret stayed line 1, column 1. Fixture is resident under the 256 MiB default threshold despite its paged filename. |
| 017 | Empty-selection Ctrl+D selected alpha from inside the word. |
| 022 | Explicit Plain text applied with Enter survived switching away and back in sample.rs. No dedicated correction claimed. |
| 030 | Clean profile exited and restarted successfully, restoring the open file and distant scroll position. Original saved-profile failure is not independently reproduced by this standard session check. |
| 032 | Original HTML export failure reproduced in GUI; phase-specific native regression established commit cause and verifies correction. |
| 005 | Sky still reported Editor focus/no Find value despite visible Find typing. Sky also misreported native Save As filename focus as Search Box. Static AccessKit focus/value route shows no proven cause; attribution remains unresolved. |
| 008 | Dirty scratch Ctrl+W disabled owner menus; Sky did not expose a confirmation through owner capture, window listing, or owner activation. Escape did not visibly recover. Native dialog versus automation attribution remains unresolved. |

All native input used isolated generated scratch profiles and fixtures. No personal application data was modified.

## Completion batch

ISSUE-001/015/029 source batch passed combined tests in target/qa-fixes-completion-tests-rerun.log: 36 native, 87 application, 57 editor and 50 Windows tests; two Windows tests ignored. Build passed in target/qa-fixes-completion-build.log with three warning groups. Four owner-draw API type errors from the first compile were corrected together before this run. No full workspace rerun was performed for each fix.

Final executable SHA-256: 7a72b4c9e16103e02b0888a3928ae37ff5c43957e6fbced6b6192cef4721d8f4

Native completion checks: themed dark menu bar retained all accessible menu names; File menu opened and its Open command launched the native file picker. The 20 MiB ASCII-line fixture loaded, End rendered the distant text fragment, and its footer resolved to exact column 20,971,521. The worker regression also passed chunk-spanning combining/emoji, checkpoint reuse, revision invalidation and cancellation checks. Run confirmation now has the editor owner HWND; this corrects ownership without claiming direct reproduction of the original automation symptom.

Disposition: 27 issue IDs have source corrections (including related/shared causes); ISSUE-022 did not reproduce after explicit application, and ISSUE-026 is expected guarded behavior. ISSUE-005 and ISSUE-008 still require independent native attribution; ISSUE-030's original saved-profile failure remains unconfirmed although clean session restart passed. These three are not claimed fixed.
