# DEV-003 — Regex transform native procedure

**State:** implemented and focused native-verified. Three procedures are implemented; ten remain unimplemented. Broader DEV-003 and release qualification remain open.

## Scope and authority

Follow the unchanged three-step regex_transform contract in [journeys.json](../../../tests/e2e/journeys.json), [PR-021](../../blueprint/PRs/PR-021_WINDOWS_PRODUCT_POLISH_PARITY_MATRIX_AND_RELEASE_QA.md), [PR-005](../../blueprint/PRs/PR-005_SEARCH_REPLACE_AND_RESULTS_ENGINE.md), FC-05/06 and UI-FN/UI-RP. Open generated multiline UTF-8 data, enable regex, observe actual expanded captures and count before applying, check exact transformed bytes and one-step Undo restoring original bytes.

Use the existing Replacement Preview with a dedicated scratch folder containing only the generated open document. Its visible rows and status currently have no accessibility publication; expose those same bounded read-only details without changing layout or replacement semantics. Use native commands and owned UIA field focus plus keyboard input. Preserve Job containment, exact binary/source/artifact binding, desktop/foreground/Escape guards and checked Exit.

Verify a multiline pattern with named/numbered capture expansion, multiple matches and unchanged Unicode/nonmatching text. Keep independently specified expected data; do not derive the replacement oracle with the product regex engine. Test malformed/missing preview, incorrect count/captures, byte changes and partial failure handling with focused source checks. Retain actual native observations and generated files. No full suite, aggregate native run, release build or AC mapping/import. Only an incremental debug build if product wiring changes.

## Result

- Native s1/s2/s3 pass with checked editor Exit 0 on debug binary `7fecdbf149a4dc073db6fa791944b92296aa204c6b179fc063ea73870605a8d7`. The captured cell is Windows 10.0.26200, dark private profile, software renderer, 96 DPI and keyboard/native-command automation. The final run took 18.865 seconds.
- The PCRE2 multiline query `(?m)^item=(?<name>[a-z]+)\nqty=(\d+)$` and replacement `${name}:$2` produce two exact preview rows at UTF-8 offsets 17 and 53. Native mode-menu checked state, complete match/preview counts, actual expanded rows and the owned client capture are retained. Preview changes neither text nor disk.
- Apply reports one open document, zero disk files, two matches and no skips/failures. Save produces independently expected UTF-8/LF bytes. One Ctrl+Z restores the complete original text; disk stays transformed until Save restores all 79 original bytes. Combining text, emoji, CJK and nonmatching lines remain unchanged.
- The app now publishes the same six visible preview/outcome rows and bounded status it renders. This closes the observed read-only accessibility gap; it does not implement or qualify all preview action/focus, screen-reader or physical keyboard behavior. The existing generic accessible mode-control label remains a broader QUAL-024 follow-up; this procedure observes the unambiguous checked native Regex menu instead.
- Focused checks: 9 new regex oracle tests, 22 shared adapter regressions, 10 code/config regressions and one new Rust preview-viewport test pass. One incremental debug app build passed in 13.790 seconds. An initial test-target compile caught an import-path error; its failed receipt is preserved. Only the named Rust test ran (159 others filtered out).
- Seven unsuccessful native captures remain retained: one foreground stop before typing, field TextPattern readback, replacement-control setup / static mode-label assumptions, one initial provider publication timeout, and the folder field/confirmation discoveries. The final harness uses ValuePattern for Find fields, explicit native Replace/Regex commands, a five-second bounded initial provider wait, and the observed Folder edit plus one Select Folder button activation. Observation polling never repeats input.
- [Retained evidence](../../qa/2026-09-15-regex-transform/README.md) binds the source, executable, fixture, generated files and logs. No full suite, aggregate native run, release build, AC mapping/import, commit or publishing ran.

**47 of 49 top-level backlog items remain open. Ten DEV-003 procedures remain unimplemented.** Next: `column_multi_cursor`. Other environments, large/boundary regex, cancellation/stale sources, open-document groups, physical accessibility and final integrated qualification remain pending.
