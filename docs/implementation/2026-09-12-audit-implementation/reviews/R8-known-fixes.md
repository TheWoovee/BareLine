# R8 review of the remaining native findings

Root reviewed these exact local deltas. Source acceptance does not imply native acceptance. The owner requested a finite closing pass, reuse of existing Sol medium agents, status checks no more than ten minutes apart, and contained tests followed by one final integrated gate run.

| PR | Accepted change | Review and acceptance evidence |
|---|---|---|
| T05 | Initialize standard Windows common controls before platform dialogs. | Missing initialization is confirmed; whether it resolves the observed hidden TaskDialog remains a native hypothesis. First contained compilation rejected an unavailable `Error::from_win32` API; the failure is retained as `final-T05-common-controls-r8.json` and returned to the implementor. |
| T16 | Publish the exact paged-save terminal before subsequent recovery maintenance, with document/generation ownership. | Returned the first patch to eliminate unconsumed legacy receipts and protect a concurrent resident save banner. Accepted `fdbdbbf`. Eight terminal tests and the existing failed-retirement retry test pass in `final-T16-terminal-r8.json` and `final-T16-retirement-r8.json`. Worker busy state continues protecting maintenance. |
| U08 | Refuse comparison close while either view has pending work and show actionable feedback. | Accepted delta `a4dbae0..e821f78`; the regression queues a secondary edit, refuses close, then verifies exact surviving Undo/Redo after settlement. This closes the final technical review's specific comparison-retirement finding. |
| U04 | Label the folder summary's count as files searched. | Accepted `d13b1e7`. The count already includes searched files without matches; displayed result groups can truncate, so changing the count to group length would be incorrect. Copy-only change requires visual confirmation, not another standalone test suite. |
| U05 | Remove a restored checkpoint row on its matching typed terminal, exclude its directory while the restored document lives, and fence older discovery results. | Accepted `7e56d85`. Claimed paths remain sweep references; closing their owner releases suppression. Failures and other checkpoints remain discoverable. The existing terminal discovery publication resolves the stale notification, including zero results. |

Implementation context and acceptance steps are in `PR-T05-R8.md`, `PR-T16-R8.md`, `PR-U04-R8.md`, `PR-U05-R8.md`, and the U08 correction below. Original PR plans and prior review history remain applicable.

## U08 implementor context

`close_compare` formerly cleared comparison state and collapsed the secondary view even when history transfer refused a busy publisher. Mirror the existing split/tab retirement guard before changing any comparison or alignment state. On refusal, preserve both views and show “Wait for pending edits before closing the comparison.” After the secondary edit settles, a later Close comparison may collapse it. `close_compare_waits_for_secondary_history_to_settle` verifies refusal, feedback, subsequent close, and exact Undo/Redo in the surviving document. This is a contained history-safety correction; it does not qualify the separate raw UIA focus observation.

## Finite native confirmation

Use one freshly hashed combined build. Confirm visible dirty-close consent, paged save-status completion, recovery restore/list/notification refresh, comparison close history, and corrected folder wording. Preserve previously passed unaffected workflows at their original binary boundaries. The BMP journey now supplies exact input; the astral SendInput journey still fails before renderer assertions. A normal-user emoji check can qualify that input route only; do not relabel the failed automated journey.

Recovery “Export Edits” intentionally exports transaction segments and an incomplete gap report, as implemented by `file-io::recovery::export_edits`. The observed zero-byte edit segment is not evidence of full-document export failure. No export behavior change was accepted.

## R8 executed confirmation

The independent Sol technical reviewer found no residual source defect in T16/U08/U05/U04. Their contained tests pass (T16 8+1+1, U08 16, U05 13). The native Sol tester confirmed all four known fixes on app `9eb824f6...`, plus normal UI emoji input. Exact receipts and binary/source boundaries are in `FINAL_STATUS.md`.

T05's first compilation failed on an unavailable API, then its manifestless platform-library test failed 0/1 on ICC_STANDARD_CLASSES. The real manifest-bearing app successfully initialized, but the single-close native check still produced an invisible TaskDialog. Thus initialization is not the visibility fix. Keep both failed tests and the failed native attempt. A scoped native callback correction remains pending; full final gates have not been rerun prematurely.

## Accepted terminal result

The scoped TDN_CREATED visibility callback (`23b121d8..e8611ba`) passed independent native p0-7, visible Cancel preservation, exact Save, About, manual Exit/discard/restart, and Save Copy overwrite checks. The independent technical reviewer accepted the callback and rehashed the evidence. Final full-suite and build/lint receipts are summarized in `FINAL_ACCEPTANCE.md`.

Root reviewed final U08 lint-only delta `f5cd5766..b6d98a18`: iteration order and history/idle fences are unchanged. Exactly three obsolete baseline records were removed; no new debt was admitted. The affected 16 view tests, complete lint census/ratchet, formatting and final build pass. Automated p0-3 initial focus and synthetic astral input remain explicitly failed/unqualified routes; no aggregate or native busy-resumption pass is claimed.
