# Automated repeat test — 2026-09-08

One authorized full workspace batch ran against the working tree based on `51cd4c0d1651fce815c67ed268e42999a9c2bb33`, including the current uncommitted QA fixes. No source, fixture, or golden files were edited by this test runner. No separate build or retry was run.

Command (cmd.exe):

```text
cargo test --workspace --offline --locked --features bareline/perf-spans,bareline/qa-inventory --no-fail-fast > target\qa-repeat-20260908-tests.log 2>&1
```

Result: **exit 101; 533 passed, 1 failed, 7 ignored**. Counts include the outer test-harness results only; five nested Windows subprocess rerun summaries (one pass each, 52 filtered out) are excluded. All doc-test harnesses ran zero tests. Log: `target/qa-repeat-20260908-tests.log` (63,812 bytes; completed 2026-09-08 13:44:48 local).

## Failure

`bareline-editor-surface` — `paged_view::paged_spill::tests::pressure_spills_existing_paged_edit_history_and_refreshes_same_identity`

Panic at `crates/editor-surface/src/paged_spill.rs:284:34`: `Result::unwrap()` received `Os { code: 5, kind: PermissionDenied, message: "Access is denied." }`. The current source line is the fixture cleanup `fs::remove_dir_all(root).unwrap()`, after a worker-drain barrier. This batch failed during cleanup; no earlier assertion failure was reported. Log lines 360–370 preserve the actual failure. No retry or attribution beyond that evidence was attempted.

## Ignored tests

- `bareline-diff`: `paged::tests::divergent_multi_gb_full_traversal` — explicit multi-GB throughput evidence.
- `bareline-extension-host`, first_party integration target: `first_party_components_cross_authenticated_process_boundary` — requires WASI release artifacts.
- Same target: `generated_one_gib_json_validates_and_tree_reads_only_one_page` — coordinated release component gate.
- Same target: `hex_five_gib_goto_reads_only_visible_original_range` — coordinated release component gate.
- Same target: `xml_xpath_security_and_hex_original_generation_cross_the_host` — requires actual WASI artifacts.
- `bareline-platform-windows`: `process::tests::child_fixture` — controlled subprocess fixture, invoked by its parent test.
- Same target: `process::tests::descendant_fixture` — controlled descendant fixture, invoked by child_fixture.

Compilation completed with three existing dead-code warning groups in the Bareline test target: `search_draw`; six view helper methods; `watch_scroll_away`. These were warnings, not additional failures.

Native QA is owned by the root task and is not covered by this report. An automated batch with a cleanup failure is not a passing acceptance gate.
