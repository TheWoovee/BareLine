# Integration batch 10 — 2026-09-08

Status: coordinated automated gate and default native build passed. Package completion requires the corresponding full-scope review; manual and external acceptance remain pending. No GUI acceptance, controlled native benchmark, signing, publication, or remote mutation was performed.

Source: `closure-final`, prior cohort checkpoint `5bfc4c1`, original passing repairs merged at `91c936aee148494e737e7eda9c7ef8746fac1075`, and final transfer/language checkpoint `28ddb8442865cfdbc623fb4e2c7e353b38859f3f`, followed by the exact repairs recorded with this note.

## Verification

Offline locked Cargo tested the affected native/application/document/file-I/O/editor/search/syntax/settings/platform/macros/UI/renderer/commands/protocol targets with `bareline/perf-spans,bareline/qa-inventory`. The complete run is `target/integration-batch-10-rerun4.log` in the original checkout. Only the three failed targets were rerun after repairs, in `target/integration-batch-10-targeted.log`.

| Target | Passing tests |
| --- | ---: |
| Native application | 35 |
| Application model | 87 |
| Commands | 8 |
| Document | 26 |
| Editor surface | 56 |
| Extension protocol | 10 |
| File I/O | 60 |
| Recovery process-death integration | 3 |
| Macros | 9 |
| Neutral platform | 10 |
| Windows backend | 50 |
| Recording renderer | 1 |
| Search | 40 |
| Settings | 17 |
| Syntax | 21 |
| UI unit / advanced / themed | 11 / 9 / 1 |

Windows backend retains two existing ignored tests. Five nested child reruns are excluded from coverage totals. The renderer target has no unit tests. Comparing successful function names against `8bb3249` identifies 45 new passing functions, bringing cumulative distinct recorded Rust coverage from 447 to 492. All six initially observed runtime failures are resolved.

Default native build passed in 35.48 seconds (`target/integration-batch-10-build.log`). SHA-256 of `target/debug/bareline.exe`: `5CC58A14008BD4D4874703FB1511F25A1A1BEA4D200B8AEFEF2AC91E89A10AAE`. Three warning groups remain; this is not a warning-free claim.

## Repaired findings

Compiler fixes used the actual type/field paths, tuple selection endpoints, Result dispatch shape and TextField selection API. Drag promotion iterates copied identity tokens so its pending request can be retained without a borrow conflict.

The mapped-fold fixture now invokes the real fold command after supplying metadata; restored user fold state is intentionally preserved by metadata refresh. Its large-gap geometry, footer reader and click assertions remain intact. The accessibility baseline received a reviewed exact identifier delta; hierarchy validation and full-field comparison remain strict.

Applied-change receipts now support the existing 10,000-caret edit and undo contract within the same 64 MiB test budgets. A new regression verifies complete receipts. Budget fixtures account for exact live receipt charges; the original exhausted-payload refusal remains intact. Scheduler accounting includes the returned receipt allocation.

Linked transfer undo remains publicly busy until each participant has installed its viewport and selection, while internal worker readiness permits the required peer read. The opaque UTF-16 cross-document move, restart, raw-byte preservation, linked undo/redo and full selection assertions pass.

## Scope of evidence

The gate exercises mapped fold/source geometry, per-pane completion identity, language session persistence, configurable resource/history policies, existing-paged spill, captured remote-read grants, folder controls, 100-document replacement, partial regex and referenced capture expansion beyond 64 MiB, global power/typing, durable paged transfers and recovery failure branches. Passing these source tests does not establish native interaction quality, physical accessibility, controlled performance results, or release acceptance.
