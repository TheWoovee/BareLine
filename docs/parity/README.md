# Maintained parity evidence index

## Current delivery checkpoint — 2026-09-16

The offline-authority consumer and bootstrap delivery gap is implemented with focused tests; parity-044 now awaits configured installed qualification. Current counts are **41 implemented awaiting qualification, 7 deferred and 4 intentional limits**, with zero fully qualified families. DEV-005/007/008/009 implementation is complete; native procedures/mappings and the full soak still have explicit remaining work. See [current status](../IMPLEMENTATION_STATUS.md).

Typed producers, schema-2 environment cells, retained producer replay and two-stage closure are documented in [TYPED-EVIDENCE.md](../../tests/e2e/TYPED-EVIDENCE.md). Use `runner.py resolve --stage prerequisites`, then independently reviewed `--stage final --closure ...`; missing cells or self-review do not pass. Historical checkpoints below retain their original counts and scope.


`index.json` preserves all 52 capability families from the blueprint matrix.
`blueprint_route` is historical intent and nullable `state` remains the final
parity verdict. No planned route is silently promoted. `current_ledger` is the
current delivery view and covers the same 52 IDs exactly once. Its states are
`implemented-and-tested`, `implemented-awaiting-qualification`, `open-defect`,
`intentional-limit` and `deferred`. Every ledger row names its code/command,
tests or retained evidence family, current limitation, actionable owner/next
step and source/binary evidence status. The current Notepad++ manual audit
remains pending; this index preserves the local authority rather than claiming
a new external audit.

Allowed final states are Implemented, Equivalent Different UX, Excluded With
Reason, v1.1 and Extension-provided (first-party). Exclusions and deferrals require
reason and a repository authority path. Other states require passing linked cases.
An empty evidence array means no independent acceptance evidence has been imported.

`cargo xtask qa resolve --inventory <runtime-inventory.json> --inventory-receipt
<inventory-receipt.json> --evidence <checked-bundle.json> --output
<new-report.json>`
enumerates every AC identifier in the acceptance authority plus success, disabled
and failure cases for every runtime command. Inventory schema is
`{"schema_version":2,"commit":"<40 lowercase hex>","binary_sha256":"<64 lowercase
hex>","commands":["file.open",...]}`. Export it from the actual Windows Shell
after first-frame registry composition, route validation and extension discovery:

```powershell
python .github/workflows/run_test_evidence.py --output <new-receipt.json> --timeout 180 -- cargo run -p bareline --features qa-inventory --locked -- --export-command-inventory <absolute-new-inventory.json> --inventory-commit <full-head-sha>
```

This mode includes dynamic contributions only when the launched runtime has a
valid trusted configuration and verified contributions. An ordinary preview
correctly exports no extension commands. Source regexes, fixtures and the
shell-only registry are not accepted inventories. The resolver verifies the
exact published inventory path and SHA-256 captured in T09 raw output, rehashes
the artifact, verifies the inventory binary hash, consumes the T09 receipt's
dirty-source manifest and rejects changed source or a commit mismatch. Missing
inventory, outcomes or identity and unresolved features produce nonzero status.
The report is evidence completeness, not release approval.

Create AC evidence bundles only from a retained native journey and the T09
receipt that captured its invocation:

```powershell
python tests/e2e/runner.py adapt --journey-result <result.json> --test-receipt <receipt.json> --evidence-id AC-006-01 --implementer <name> --reviewer <different-name> --fixture <actual-generated-fixture> --output target/qualification/<new-bundle.json>
```

The adapter accepts an AC or command outcome only when `journeys.json` has a
reviewed `qualification_mappings` entry for that exact journey/evidence ID and
all of its required steps have actual nonempty PASS observations. Journey
`cases` are provisional traceability hints and cannot qualify a whole AC. The
adapter hashes actual retained fixture files, verifies retained raw-output
hashes, requires the producer's exact result path/SHA-256 binding, rejects
changed source, and binds the journey commit, dirty-source manifest and
executable hash. It never creates PASS from a fixture declaration or Cargo
summary text. Command outcomes likewise require explicit reviewed mappings.

The same adapter can retain T09 `cargo xtask journey` evidence. For a
multi-journey result, add `--journey <exact-result-name> --os-build <exact-build>
--hardware <exact-description>`. Its own `identity` block must match the T09
receipt and the selected top-level result must have actually passed once. This
input contains top-level status only, so the adapter emits an observation bundle
with no AC or command evidence claims.

Each evidence record has `id` (AC identifier or `command:<id>:success|disabled|failure`),
full `commit`, `implementer`, distinct `reviewer`, `fixture_sha256`, exact `os_build`,
`hardware`, captured `mode`/`theme`/`dpi`, executed `command`/steps, `observed`, `status` (PASS/FAIL/NOT_RUN), repository
`result` path and `result_sha256`, checked `source_identity`, `binary_sha256`, and
the T09 receipt path/hash. The resolver reopens and rehashes the result, receipt,
fixtures and authoritative mapping; it revalidates the actual observations and
requires source identity, binary, command and recorded fields to agree. It rejects
duplicate/unknown IDs, altered artifacts, escaping paths and incomplete metadata. An exclusion instead
records `excluded:true`, `reason` and existing repository `authority`. Independent
review remains a human accountability assertion, not proof inferred from a file.

Cases provisionally associated with each capability derive from its historical PR owners;
reviewers must add explicit complete mappings as they assess actual parity.
No feature row may be deleted silently. Commit/result records are updated when code
changes; a stale result does not establish the new runtime inventory's behavior.
Performance measurements remain informational and reuse PR019 raw data/table.
PR-T15 reports use schema 2 `performance_qualification_report` artifacts with
stable row/coverage IDs, application hashes, T09 source-before/source-after
identity, `claims_eligible` and explicit ineligibility reasons. No report has
been generated yet; owner-provided comparator/hardware runs remain pending.
Reliability, release artifacts and both-floor assistive native QA remain separate
required evidence under PR021 and the existing packaging verifier. The current
ledger therefore uses no `implemented-and-tested` rows until final integrated
qualification imports matching receipts. At the pre-qualification checkpoint,
the T14 Python adapter/resolver cohort passes 13 checks and the composed command
inventory cohort passes all 10 checks. An actual schema-2 export with 499 commands
exists on its historical binary. Matching final candidate inventory and native/AC
evidence remain pending. T11 completed nonshipping catalog/runtime/component
installation and installed Fast 3/3 and Large 2/2 checks; the corrected reporter
validated the retained artifacts. Production configuration, signing and shipping
acceptance remain pending. All 31 original audit packages are integrated. The
expanded September 15 review reopens `parity-044` for the offline-root policy
configuration gap and separately records a reproduced QA adapter-exit false PASS;
see the [expanded findings](../qa/2026-09-15-readiness-backlog/README.md).
Final local correctness/build/lint gates passed
within the boundaries in [FINAL_ACCEPTANCE.md](../implementation/2026-09-12-audit-implementation/FINAL_ACCEPTANCE.md).
The [September 15 audit](../qa/2026-09-15-status-audit/README.md) rechecked their
retained hashes and the current code boundary. Follow the
[production-readiness plan](../implementation/PLAN-20260915-PRODUCTION-READINESS.md)
for remaining driver/mapping work, candidate qualification and release evidence.

DEV-003 supplies a native adapter and one product procedure. Its historical CRLF
failure identified NEW-FILE-DEFAULTS under QUAL-012. The [focused repair](../qa/2026-09-15-new-file-defaults/README.md)
now passes six Rust tests, 22 adapter tests and the native UTF-8/CRLF round trip.
Parity-038 returns to awaiting qualification: **40 families await qualification,
one open defect remains (parity-044), seven are deferred and four have intentional
limits**. No family or AC is fully qualified. Additional native encoding cells
remain pending after foreground interruption, alongside broader platform coverage.
The subsequent [code_config procedure](../qa/2026-09-15-code-config/README.md) passes Rust token-color, completion/indentation and save/reopen observations and repairs missing UIA text geometry. Eleven DEV-003 procedures remained unimplemented at that checkpoint; the family counts and pending full qualification are unchanged.

The subsequent [regex_transform procedure](../qa/2026-09-15-regex-transform/README.md) passes all three native steps and clean Exit for multiline capture preview, exact replacement count/bytes and one Undo. It also exposes bounded read-only preview rows/status in accessibility. **Ten DEV-003 procedures remain unimplemented; 47 top-level backlog items remain open.** Family counts remain 40 implemented-awaiting-qualification, one open defect, seven deferred and four intentional limits. No AC/command evidence was imported or family promoted.

## Validation repairs — 2026-09-15

The [ten-defect batch](../implementation/production-readiness/BATCH-20260915-TEN-DEFECTS.md) enforces unique JSON fields, integer schemas/accounting, the exact captured canonical journey, captured fixture/artifact hashes and scratch containment, unchanged executable bytes, all five captured environment fields, normalized distinct reviewer names and a single unambiguous capture binding. At most 128 distinct captured artifacts are accepted. Reads are bounded to 16 MiB per evidence JSON/output stream and 256 KiB per native response; log hashes bind the bytes actually parsed.

Older bundles missing environment fields must be reissued from complete captured results; missing execution identity requires recapture. Preserve historical artifacts and their original limitations. These repairs do not implement typed producers, environment aggregation, independent identity verification or final AC qualification.

## Required environments and portable retention — twenty-task checkpoint

Resolution now requires a reviewed environment matrix for completeness. `runner.py resolve --matrix <matrix.json>` aggregates case/cell results with exact captured environment/source/binary identities. FAIL/NOT_RUN and missing cells remain visible. Checked unexecuted captures may have no fixtures; PASS still requires bound generated artifacts. Exclusions require an exact reviewed case/cell entry and unchanged authority hash. Command cells remain bound to the captured runtime inventory.

The [retention workflow](../../tests/e2e/EVIDENCE_RETENTION.md) provides `evidence_bundle.py collect|verify|report`. It retains byte-addressed objects, original paths and dependency hashes, preserves failures, and verifies after relocation/cache deletion. Reports list retained cases/cells, open work and limitations. They do not attest signing, reproducibility or semantic acceptance; typed producers and final closure remain further DEV-005/009 work.
