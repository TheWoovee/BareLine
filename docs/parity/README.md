# Maintained parity evidence index

`index.json` preserves all 52 capability families from the blueprint matrix.
`blueprint_route` is historical intent; nullable `state` is deliberately unresolved
until justified by current delivery and independent evidence. No planned route is
silently promoted. Add newly discovered manual capabilities before implementation.
The current Notepad++ manual audit remains pending; this index preserves the local
authority rather than claiming a new external audit.

Allowed final states are Implemented, Equivalent Different UX, Excluded With
Reason, v1.1 and Extension-provided (first-party). Exclusions and deferrals require
reason and a repository authority path. Other states require passing linked cases.
An empty evidence array means no independent acceptance evidence has been imported.

`cargo xtask qa resolve --inventory <runtime-inventory.json> --output <new-report.json>`
enumerates every AC identifier in the acceptance authority plus success, disabled
and failure cases for every runtime command. Inventory schema is
`{"schema_version":1,"commit":"<40 lowercase hex>","commands":["file.open",...]}`.
Export from the composed runtime registry including dynamic contributions; source
regexes or the shell-only registry are not exhaustive substitutes. Missing inventory,
missing outcomes, mismatched commits and unresolved features produce nonzero status.
The report is evidence completeness, not release approval. The current runtime
inventory export is a host integration handoff, not fabricated by this resolver.

Each evidence record has `id` (AC identifier or `command:<id>:success|disabled|failure`),
full `commit`, `implementer`, distinct `reviewer`, `fixture_sha256`, exact `os_build`,
`hardware`, executed `command`/steps, `observed`, `status` (PASS/FAIL/NOT_RUN), repository
`result` path and `result_sha256`. The resolver verifies artifact hashes and rejects
duplicate/unknown IDs, escaping paths and incomplete metadata. An exclusion instead
records `excluded:true`, `reason` and existing repository `authority`. Independent
review remains a human accountability assertion, not proof inferred from a file.

Cases provisionally associated with each capability derive from its PR owners;
reviewers must narrow/add atomic and command cases as they assess actual parity.
No feature row may be deleted silently. Commit/result records are updated when code
changes; a stale result does not establish the new runtime inventory's behavior.
Performance measurements remain informational and reuse PR019 raw data/table.
Reliability, release artifacts and both-floor assistive native QA remain separate
required evidence under PR021 and the existing packaging verifier.
