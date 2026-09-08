# Batch 06 encoding closure

PR007 is implementation complete pending acceptance. Fourteen packages now have that state, thirteen remain in progress and none is fully accepted. PR010 remains open because independent paged selections outside the loaded viewport are not yet represented.

The isolated source cohort was checkpointed at fd1b8ad and integrated with completed original-tree commits at 1550d70. The affected offline locked gate selected file-io, app, editor-surface and bareline with `bareline/perf-spans,bareline/qa-inventory`. File-io48, app82 and editor29 passed. Four native compile defects were repaired together. The next native run exposed a resident-only selection API used on a paged adapter and a golden comparison sensitive to checkout CRLF. A bounded paged selection facade now preserves the viewport while validating contained endpoints; the golden compares complete parsed JSON values, retaining every semantic field and array order. No baseline semantics changed.

The native rerun passed 23/23, including delayed document identity and fold-result routing tests, followed by two focused paged regressions. Six newly passing tests across this cohort bring cumulative distinct Rust coverage to **395**; repeated suites are not added again. No full workspace suite was repeated. Logs are in the original checkout's ignored `target/integration-batch-06-*.log`.

The final default `cargo build -p bareline --offline --locked` passed in 28.72 seconds. Native SHA-256: `2472E2BE468B07366C5775D9AF2A73A9D62D435FAD533D7F73F619BAB31F1ED1`. Twelve existing warning groups remain visible. This is the tested isolated source artifact, not a signed release.

PR007 now exposes precise current-text save failure ranges, rejects stale reveal requests and presents complete paged EOL counts with explicit Pending/Unavailable states. Manual large-file, native input/visual and detection-quality acceptance remains pending. PR010's per-pane styling, byte anchors and fold routing are verified partial work; off-viewport global selection is still required before promotion. No manual QA, benchmark adapter, remote or publication operation occurred.
