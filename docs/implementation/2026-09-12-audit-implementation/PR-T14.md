# PR-T14 — Current capability ledger and acceptance evidence

Issue: TECH-017. Baseline: `e7648af1715ecf5a0214377db2f802f7e330a9e4` on
`codex/audit-20260912-t14-fresh`. This note describes the local PR-T14 worktree;
it is not a release-readiness claim.

## Dependencies and evidence boundary

- PR-T09 owns evidence capture and source/binary identity. PR-T14 consumes its
  retained observations through a checked adapter.
- The capability states describe current implementation and known limits. A row
  is not `implemented-and-tested` until matching current evidence is imported.
- Final integrated qualification and native receipts are still pending. Later
  receipts must be imported and resolved against the exact composed command
  inventory and matching source/binary identity.
- Historical blueprint and implementation receipts remain unchanged. The audit
  status registry and central check log are maintained by the integrator.

## Implemented local changes

- `docs/parity/index.json` now has a one-to-one 52-family `current_ledger`.
  Every row records one of the five current states plus code/command, tests,
  limitation, owner, next step and source/binary identity status. Seven v1.1
  deferrals and four product exclusions also resolve the old nullable parity
  verdict where repository authority is already sufficient. No implemented
  row is promoted to tested before final matching evidence exists.
- The actual Windows Shell inventory export retains full application command
  registration, route checking and synchronized verified extension
  contributions. Schema 2 additionally hashes the running executable on its
  bounded worker before atomically publishing the new file. Unconfigured
  preview runs continue to omit unavailable extension commands.
- `tests/e2e/runner.py adapt` consumes actual PR021 native step observations or
  T09 `cargo xtask journey` status/identity records plus a successful T09
  wrapper receipt. PR021 claims require an explicit reviewed journey/evidence
  mapping, all named PASS observations and hashes of retained generated
  fixtures. The producer's exact result path/SHA-256 must be present in the T09
  output. Status-only xtask results remain observations and claim no AC. The
  adapter verifies the executable, result/raw/receipt hashes, full commit,
  dirty-source manifest, stable source across the run and independent reviewer.
  It cannot derive PASS from provisional case associations, fixture declarations
  or nested Cargo summary lines. `resolve` revalidates linked results, mappings,
  fixture hashes, receipt source identity and binary, and requires an exact
  published inventory path/SHA-256 from its T09 receipt.
- The sole current complete mapping, `column_multi_cursor` to `AC-006-01`,
  explicitly requires observed insertion across tabs, wide glyphs and a short
  line by display column, followed by restoration with one Undo invocation.
  This is a planned qualification contract; no native PASS is claimed here.
- The root README now states the Windows x64 floor, 256 MiB resident threshold,
  1 MiB page, 64 MiB per-file/256 MiB aggregate cache, 128 MiB undo RAM,
  100,000-change and 20 GiB transcode defaults, configurable maxima, supported
  encodings, exact-byte save/recovery policy, and fail-closed preview trust.
  The active codec catalog's obsolete paged-transcode sentence was reconciled.
- `docs/IMPLEMENTATION_STATUS.md` points current readers to the ledger while
  preserving historical package receipts. `docs/UNLOCK_CHECKLIST.md` remains the
  native resume list; the live status and central check log supersede its
  ordinary implementation labels as corrections integrate.

## Integrated pre-qualification checkpoint

- On integrated source, the T14 Python adapter/resolver cohort passes 13 checks
  and the composed command-inventory cohort passes all 10 checks after the U04
  search-mode routing correction. The actual inventory export remains pending.
- PR-U02 passes 20 focused notification checks. PR-T16 passes 12 contained
  lifecycle checks. These remain source-level receipts, not final AC/native
  qualification.
- PR-T11's corrected source is integrated. Its first connected run built three
  pre-correction release binaries, then failed before installed delivery. The
  corrected connected fixture, actual install, installed Fast run, signing and external
  acceptance remain pending.
- All 31 audit packages are integrated. PR-U05's focused controller,
  result-retention/failure and actual resident/paged insertion checks pass, so
  Recovery Center moves to `implemented-awaiting-qualification`. The ledger now
  has 41 such rows, no open defect, seven deferred rows and four intentional
  limits. It still has no `implemented-and-tested` row or imported AC evidence;
  root-owned final gates and a fresh independent audit are next.

## Focused verification

- `python -m unittest discover -s tests/e2e -p test_runner.py` — 13 passed,
  including unrelated-case, altered-result, failed-result, inventory-mutation
  and binary-mismatch rejection contracts.
- `python -m py_compile tests/e2e/runner.py tests/e2e/test_runner.py
  .github/workflows/run_test_evidence.py` — passed.
- `python .github/workflows/run_test_evidence.py --self-test` — passed; the
  retained-failure/nested-summary/no-overwrite contract remains intact.
- PowerShell `ConvertFrom-Json` parsed the ledger; feature and current-ledger
  counts are both 52. The initial T14 snapshot had 37 awaiting qualification,
  four open defects, seven deferred and four intentional limits; the integrated
  pre-qualification checkpoint above supersedes those delivery-state counts.
- `rustfmt --edition 2024 --check apps/bareline/src/windows_app/inventory.rs
  xtask/src/journey.rs` and `git diff --check` — passed
  after focused formatting.

Cargo tests, the actual inventory export, evidence resolution and final native/
workspace qualification are intentionally root-owned. Use a new ignored output
path so publishing a receipt does not change the frozen source fingerprint. The
minimal final sequence is documented in `docs/parity/README.md`; it must retain:

1. Schema-2 composed inventory plus its successful T09 receipt.
2. Final native result bundles adapted from actual step observations, with the
   same commit, dirty-source manifest and binary hash as the inventory.
3. Explicit command success/disabled/failure and AC outcomes; missing results
   remain unresolved. Do not count nested fixture summaries as top-level tests.
4. PR-T15 schema-2 performance report with `claims_eligible:false` until matched
   owner comparator/hardware cohorts exist.

Current focused receipts (including U11 and U04 on later integration snapshots
and corrected T10 Fast) are useful history but do not qualify this T14 baseline
or the eventual final binary. Final qualification must import fresh matching
receipts rather than relabeling those runs.
