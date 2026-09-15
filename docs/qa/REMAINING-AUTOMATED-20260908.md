# Remaining issues — automated verification, 2026-09-08

One serial workspace gate ran:

`cargo test --workspace --offline --locked --features bareline/perf-spans,bareline/qa-inventory --no-fail-fast`

Initial result: **534 passed, 2 failed, 7 ignored**, exit 101. Counts use the final test summary in each Cargo Running/Doc-tests block, excluding nested subprocess summaries; machine-readable breakdown: `target/remaining-qa/test-counts.json`. Complete log: `target/remaining-qa/tests.log`.

Both failures were reviewed fixture drift from the intended rendering corrections:

- `paged_view::peer_tests::global_scroll_crosses_windows_and_keeps_midline_coordinates_exact`: fixed pixel clicks landed in the newly widened line-number gutter. Rendering owner changed them to offsets from `text_left()` while preserving scrolling/selection assertions. Focused test passed: **1 passed, 0 failed**, log `target/remaining-qa/paged-scroll-retest.log`.
- `windows_app::accessibility::tests::complete_native_semantic_json_golden`: Compare option geometry moved upward to fit above the footer. Accessibility owner reviewed and updated only the corresponding y coordinates. Exact test passed: **1 passed, 0 failed**, log `target/remaining-qa/semantic-retest.log`.

Final unique test dispositions after these focused retests: **536 passed, 0 outstanding failures, 7 ignored**. The workspace suite was not repeated. The prior paged-spill cleanup failure passed in this run.

Then ran serially:

`cargo build -p bareline --offline --locked --features perf-spans,qa-inventory`

Build passed, exit 0; three dead-code warnings, no errors. Log: `target/remaining-qa/build.log`.

Executable: `D:\Notepad_REq\Bareline_Product_Blueprint\Bareline-Editor\target\debug\bareline.exe`

SHA-256: `8627c9f5a2081969972e9a94c7816eca30eced7931c884957c328c8559ef6fba`

No native UI was launched by this verification task. Interactive acceptance remains separate; automated results alone do not close dirty-dialog visibility, palette input, rendered compare/Output, recovery/session, source-watch or native accessibility observations.

## Correction batch after first native sweep

This batch includes deferred native close prompts, asynchronous column-status redraw propagation, recovery preview input correction, and QA-only Find/accessibility publication diagnostics. The full workspace suite was not repeated.

Initial shell compilation found private `ViewsRuntime.controller` access in the deferred-close helper. Replaced it with the existing `pane_token(pane())` accessor; retained initial failure log `target/remaining-qa/correction-shell.log`.

Serial focused results, all exit 0:

- `cargo test -p bareline --offline --locked --features perf-spans,qa-inventory --bin bareline -- deferred_close_rejects_changed_tab_document_or_index find_focus_and_values_survive_native_snapshot_publication complete_native_semantic_json_golden`: 3 passed, 0 failed. Log `correction-shell-retry.log`.
- `cargo test -p bareline-editor-surface --offline --locked column_worker_completion_requests_frame_without_another_input`: 1 passed, 0 failed. Log `correction-column.log`.
- `cargo test -p bareline-app --offline --locked paged_recovery_restart_preserves_opaque_undo_and_recovers_stale_pointer`: 1 passed, 0 failed. Log `correction-recovery.log`.

**Correction batch: 5 focused tests passed, 0 failed, 0 ignored.** These results overlap earlier tests and are not added to the previous whole-workspace total.

Then `cargo build -p bareline --offline --locked --features perf-spans,qa-inventory` passed, exit 0, with the same three dead-code warnings. Log `target/remaining-qa/correction-build.log`.

Updated executable path remains `D:\Notepad_REq\Bareline_Product_Blueprint\Bareline-Editor\target\debug\bareline.exe`.

Updated SHA-256: `f0674363759ab07d4323edba427d5386aa78765c4e03d466e2d88105a508923c`.

This task did not launch native UI; the correction executable still requires the parent's interactive acceptance sweep.