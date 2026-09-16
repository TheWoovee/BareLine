# Twenty sequential readiness tasks — 2026-09-15

All twenty selected implementation tasks are complete with focused verification. The [implementation ledger](../../implementation/production-readiness/BATCH-20260915-TWENTY-TASKS.md) records each task in order.

| Area | Completed in this batch |
|---|---|
| DEV-007 — 5 tasks | Required offline-root config, independent key validation, prepared/artifact binding, verified build handoff, explicit editor/helper runtime policy |
| DEV-005 — 7 tasks | Full environment identity, required-cell matrix, repeated cases across cells, exact source/binary matching, FAIL/NOT_RUN retention, complete cell aggregation, reviewed cell exclusions/CLI |
| DEV-009 — 8 tasks | Safe collection plans, bounded/deduplicated copies, dependency closure, immutable publication, relocated verification, negative integrity checks, readiness reporting, complete collection CLI |

## Verification

**79 distinct Python tests, one Windows Rust regression and the release-configuration contract script pass.** One incremental editor/helper debug build passed in **15.529 seconds**. Full suites, release builds, native product journeys, signing and independent acceptance were not run.

The [verification summary](verification-summary.json) retains commands, source identities and candidate hashes. All 38 capture receipts have unchanged before/after source identities and matching raw logs. Initial failures are retained: a Windows path-prefix test-fixture error, a false Windows ctime comparison (fixed), and a legacy diagnostic-text expectation (compatibility restored). `32-current-coverage-check` intentionally exits 1 because real qualification remains incomplete.

## Portable retention

The new collector retained **143 original file identities / 114 unique objects / 680,724 input bytes**. Its pinned manifest SHA-256 is `6fb8aae4d4fb57200ce9a4ec63051c254e5b4836737647d7a39588e662e588ae`.

- [Independent bundle verification](bundle-verification.json): exact hashes and rederived dependency closure pass without consulting original paths.
- [Retained path/hash index](retained-files.json): maps readable original names to the stored objects.
- [Readiness report](readiness-report.json): 47 open work items and missing real acceptance/inventory/environment evidence; report exit 1 is expected.
- [Hash-addressed manifest](bundle/manifest-6fb8aae4d4fb57200ce9a4ec63051c254e5b4836737647d7a39588e662e588ae.json): immutable source/tooling snapshot with original paths and byte identities.

The relocation/cache-removal drill uses isolated synthetic data. Candidate executable hashes are recorded; executable bytes are not included in this source/tooling bundle. This is not a complete product release archive. Later documentation edits do not rewrite captured source fingerprints or earlier native candidate identities.

## Remaining work

**0 of this batch remain; 7 development packages and 47 of 49 top-level readiness items remain open.** Ten native procedures remain unimplemented under DEV-003. DEV-007 still needs catalog/runtime authority integration, signed bootstrap delivery and configured rotation qualification. DEV-005 still needs typed non-native producers and final closure semantics. DEV-009 still needs those semantic verifiers and a complete actual release bundle.

No parent package, capability or acceptance criterion was closed by synthetic tooling tests. Publication and production signing remain separate work.
