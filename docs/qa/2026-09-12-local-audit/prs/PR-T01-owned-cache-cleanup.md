# PR-T01 — Confine startup cache deletion to proven owned directories

Priority P1. Fixes TECH-001. Dependencies: none. Owner: Windows filesystem/launch implementor. Review contract: blueprint ADR-01/09 and file/recovery path-safety contracts. Start from the current local tree, not HEAD alone.

## Problem and evidence

`apps/bareline/src/windows_app/launch.rs:430–492`, especially `sweep_temp_caches`, scans every `Bareline-*` directory under TEMP and follows junctions before deleting children with a parseable PID. `../evidence/launch-probe.log` proves deletion outside the supplied temporary root. Process-query failure is also interpreted as proof of death. Both assumptions are unsafe for a destructive janitor.

## Implementation

1. Move cache ownership/cleanup policy into `crates/file-io`; put Windows reparse/handle operations in `crates/platform-windows`. Enumerate the exact cache roots created by owned spill, transcode and related writers. Replace the wildcard prefix policy with that explicit registry.
2. Have each cache writer create a small versioned ownership record atomically before publishing cache content. Include cache kind, random directory identity, PID plus process creation identity, format version and retained-recovery references. Keep legacy cleanup conservative: unproven legacy directories are retained and reported, not guessed safe.
3. Inspect root and candidate with no-follow metadata. Reject reparse points at every ancestor, root and child. Use platform directory guards already used by recovery/package storage so a checked directory cannot be swapped before enumeration/removal. Resolve and validate containment immediately before any recursive deletion. Do not assume string-prefix checks alone are sufficient.
4. Replace `bool alive` with `Alive | Dead | Unknown`. Access-denied/query failure and PID identity ambiguity mean Unknown, which must retain the cache. Delete only owned, unreferenced, definitely-dead candidates.
5. Run the bounded sweep after first frame on a worker. Cap entries/time, support cancellation and retain structured counts/reasons. Never scan unrelated user directories. Do not log file contents.

## Verification and acceptance

- Port `launch_probe`'s junction fixture into a Windows regression using an isolated root. The external sentinel must survive; the junction must be reported skipped.
- Cover an ordinary owned dead cache (removed), live cache, unknown liveness, PID reuse, unmarked directory, nested reparse point, recovery-referenced cache, and a root swapped after validation (all retained safely).
- Simulate deletion failure and cancellation: no false “removed” count; retry metadata survives. A bounded sweep cannot block input.
- Run focused file-io/Windows path tests and a single application integration compile; run full lifecycle suite after all shared file-safety PRs land.

No “fix” may disable recovery, delete all TEMP entries, follow links, weaken path guards, or infer ownership solely from a filename. Review the actual deletion targets in tests. Done means the original reproduction no longer deletes the sentinel and ordinary dead-cache reclamation still works.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
