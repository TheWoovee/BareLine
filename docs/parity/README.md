# Maintained parity evidence index

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
`hardware`, executed `command`/steps, `observed`, `status` (PASS/FAIL/NOT_RUN), repository
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
inventory cohort passes all 10 checks. The actual schema-2 export and matching
native/AC evidence are still pending. T11's configured source and the three
pre-correction binaries from its first connected run likewise await the corrected
connected run, actual installation, installed Fast execution, signing and
external acceptance. All 31 audit packages are now integrated; the ledger has
no current `open-defect` row. Root-owned final gates, matching native/AC receipts
and a fresh independent audit remain next.
