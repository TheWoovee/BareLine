# Windows journey completion

Authority: `WINDOWS-COMPLETION-20260916.md`, the exact three-step contracts in `tests/e2e/journeys.json`, and AC definitions in `COVERAGE.md`.

Implement each procedure sequentially with generated scratch fixtures, exact expected bytes, bounded native observations and owned-process cleanup. Preserve desktop/foreground/Escape guards. Signed release and disposable-machine procedures require their actual inputs; their absence must remain NOT_RUN. Focused fixture/validator tests establish harness behavior, not native product acceptance. Native execution follows the final candidate build.

First slice: column selection across a tab, a wide glyph and a short line, one insertion, and one Undo restoring all original bytes. Observe all three UIA selections before applying the native Column Editor command.

UDL defect found during procedure development: the controller's installed definitions were memory-only. Add bounded worker-only loading and atomic installation under the existing profile's `languages/<validated-id>.json` machine-written catalog, publish import success only after durable commit, and resolve unambiguous extension associations after load. A malformed installed catalog is reported and preserved, and blocks silent replacement. User import/export remains the explicit authoring interface. Existing plain-language overrides take priority. Test native Windows filesystem persistence/restart, cancellation, corruption and ambiguous extensions without opening a UI.

## Native column defect

The actual DirectWrite run rejected a tiny CJK row as exceeding layout width. The resident consumer compared caret Y positions across fallback fonts and retained an older BiDi-unsafe advance calculation. It now uses the shared grapheme range measurement. Shared row validation accepts overlapping vertical bands from fallback fonts and still rejects distinct wrapped rows. A real offscreen Windows renderer regression covers CJK, combining marks, BiDi and emoji; the native journey must additionally prove exact insertion and one Undo. The input harness now supplies real scan codes and extended navigation flags, pins an already installed en-US layout only on its owned editor thread, and records the observed layout. Missing UIA ranges remain pending observations.

## Current checkpoint

Ten native procedures are implemented; crash/recovery, extension-isolation and install/update/rollback are still NOT_RUN and need lab-specific driver integration. Seven new fixture oracles pass synthetic checks. The full E2E tooling suite passes 113 tests. Plain text passed native execution in this pass; column discovery reached the product geometry defect and the final rerun stopped on foreground loss. No new full journey or acceptance promotion is implied. See tests/e2e/WINDOWS-LAB.md and docs/UNLOCK_CHECKLIST.md for the resume order.
