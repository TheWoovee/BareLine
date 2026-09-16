# Ten sequential defect repairs — 2026-09-15

All ten selected defects are fixed with focused regressions. See the [implementation ledger](../../implementation/production-readiness/BATCH-20260915-TEN-DEFECTS.md) for each finding and fix.

## Evidence

- `01-repro`/`01-green`: Find mode accessible names across Literal, Extended and Regex, for both controls. `01-red` is an initial test-fixture setup error, preserved separately.
- `02` through `10` red/green pairs: each reproduced evidence-validation defect and its passing regression. Additional `06-control` and `07-complete` receipts cover valid fixture binding and missing/overridden environment fields.
- `11` through `14`: 60 distinct Python checks pass (11 new regressions, 14 runner, 13 adapter-exit, 22 native-adapter). These are synthetic tooling checks, not native product acceptance. The 13 adapter-exit tests pass again in `16` after bounding actual response reads.
- One focused Rust regression passes. One incremental debug app build passes in 8.281 seconds (`15`); no full suite or release build ran.

[Verification summary](verification-summary.json) records commands, historical source identities, counts and the new app hash. [Retention manifest](retained-files.json) maps every copied receipt/log/source snapshot to its original path, exact bytes and SHA-256. All raw capture hashes and source-before/after equality were verified. Earlier receipts retain their original identity; later source snapshots do not relabel earlier runs.

## Qualification boundary and remaining work

No native or physical screen-reader recapture ran on the rebuilt executable. No independent reviewer or AC import is claimed. The full suite and release qualification remain deferred. These repairs cover existing evidence validation; typed producers and aggregation across required environments remain open under DEV-005.

**Remaining: 0 of this batch; 47 of 49 top-level readiness items.** DEV-003 contains ten unimplemented native procedures, starting with column_multi_cursor. Those are a separate scope from this completed ten-defect batch. Historical native captures and prior QA folders are unchanged.
