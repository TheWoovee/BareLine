# PR-T14 — Reconcile current capabilities, limits and acceptance evidence

Priority P2 qualification. Fixes TECH-017. Depends on the issue fixes it claims complete and PR-T09's evidence identity work. Documentation/evidence integration; do not rewrite the preserved blueprint or declare unfinished functions implemented.

## Evidence gap

Root application README still says open is limited to 1 MiB and recovery/large-file work is unfinished. Current code/tests implement substantially more. `docs/IMPLEMENTATION_STATUS.md` combines historical and later contradictory completion records. `docs/parity/index.json` has 52/52 null states and zero imported evidence. Native journey success is separate and is not automatically consumed by the parity resolver. Another implementor cannot reliably distinguish current capability, defect, missing qualification and historical scope.

## Implementation

1. Add a current capability ledger with states: implemented-and-tested, implemented-awaiting-qualification, open-defect, intentional-limit and deferred. Link every row to the active code/command, current test or native evidence, limitation, owning PR and source/binary identity. Preserve historical records rather than silently altering their results.
2. Update README with actual supported platform, launch command, storage thresholds versus hard limits, file encodings, save/recovery policy, preview trust configuration and where to find current acceptance. Derive numerical limits from current settings/source and tests, not old review prose.
3. Reconcile the 52 parity families against current runtime command inventory and acceptance IDs. Export actual composed commands; include dynamic contributions when a fixture-trusted runtime is available. Do not fabricate inventories by regex or mark all extension commands as implemented in an unconfigured preview build.
4. Import valid evidence into the existing resolver format. Record dirty-source digest and binary hash in addition to commit. Keep missing success/disabled/failure outcomes explicit. Link the native journey harness's real observations to acceptance records through a checked adapter, not synthetic PASS outputs.
5. Retire duplicate implementation ownership: this audit's PR-T03 owns overwrite/default save behavior; UI briefs reference it. Mark September 8 defects already fixed as historical and retain their regression tests. Update the unlock checklist to distinguish environment-only checks from ordinary code gaps.

## Acceptance

A new agent can answer “what works, what is broken, what must I test, which PR fixes it” from this ledger without reading all prior conversations. The resolver reports truthfully; unresolved rows may remain, but every one has an actionable owner/next step. Every completed claim links to actual matching evidence. No numerical pass count includes subprocess fixture executions twice. Local documentation changes do not authorize release or remote operations.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
