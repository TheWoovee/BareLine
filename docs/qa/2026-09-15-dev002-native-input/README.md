# DEV-002 — Focused native input verification

**Outcome:** fixed astral packet decoding, stale native UIA host focus and Settings packet-text handling. See the [implementation note](../../implementation/production-readiness/DEV-002-NATIVE-INPUT.md).

## Final results

| Scope | Evidence | Result |
|---|---|---|
| Packet decoding and context boundaries | [9 unit tests](unicode-unit-tests-qualified.json.stdout.log) | PASS |
| Native HWND focus and menu exclusions | [1 unit test](native-focus-unit.json.stdout.log) | PASS |
| Seven fields, two panes, distinct carets, F6 both ways, provider retirement | [13 checks](fields-and-split-complete.json), [capture receipt](field-probe-complete-receipt.json) | PASS |
| Single Exit, consent and discard/restart | [p0-3](p0-3-final.json), [journey result](journeys/p0-3-final.json) | PASS |
| Single Close and save consent | [p0-7](p0-7-final.json), [journey result](journeys/p0-7-final.json) | PASS |
| Exact emoji delivery and color-glyph check | [p4-4](p4-4-qualified.json), [journey result](journeys/p4-4-qualified.json) | PASS |

The final native records share binary SHA-256 `b52da9546ac1547a75a9679c8afa4d0c70c23fd4c23b57219112a58b54aff414`. The [final incremental debug build receipt](incremental-app-build-final.json) records the source identity; [verification-summary.json](verification-summary.json) records final source-file hashes and the subsequent formatting-only change to a cfg(test) block. No full suite, aggregate journey run or release build was performed.

The field probe reads exact `A🎉é文`, including the combining accent, after measured SendInput and verified focus. Split records retain both numeric runtime IDs and different caret offsets (0 and 6) while focus moves through Pane 2 → Pane 1 → Pane 2. It also verifies that closing the split retires those providers and restores Editor focus. Run and Go To text is inspected without submitting either command.

## Reproduction and diagnosis

- [Original empty emoji response](p4-4-before.json.stdout.log) reproduced the pre-fix failure. [First decoder iteration](p4-4-after.json.stdout.log) delivered only two emoji; unrelated wake messages were incorrectly clearing the pair. That iteration is superseded by the final packet/context handling.
- Focus failures were retained in the `fields-after*.json` and `p4-4-after-r2` records. Native focus diagnostics and the later successful Editor/field records distinguish the real app HWND from the UIA host's stale focus state.
- `fields-after-r5.json` records six passing input surfaces and empty Settings text before its route was repaired. The final complete probe passes all seven.
- Probe setup failures are retained, including incorrect field labels, a .NET selection-enum namespace, unavailable Get-FileHash in the captured environment, and an assumed Shift+F6 binding. The documented command is F6; the final probe exercises that same command in both directions. These setup failures are not classified as product defects. Cleanup was hardened after the report-writing failure, and its owned process was terminated.

## Reuse and boundaries

- [Probe source](field-probe.ps1.txt) is retained as an inert copy. The original was run explicitly from `target/qualification/dev002-20260915/` with a pinned executable and output path.
- [Retained-file manifest](retained-files.json) preserves byte hashes and original paths. Raw receipts keep their original target paths; sibling retained copies have matching bytes. Journey checkpoint JSON is under `journeys/`; screenshots and generated scratch profiles remain at their original target paths.
- All data and editor processes belong to generated scratch profiles. No AC evidence was imported and no parity family was promoted.
- Native busy-at-command timing was not asserted: deterministic busy Close/Exit stress remains a final qualification gate. Physical IME/screen-reader checks, the entire historical U08 interaction sequence, platform qualification and independent review also remain pending.
