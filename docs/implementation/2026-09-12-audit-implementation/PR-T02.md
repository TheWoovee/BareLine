# PR-T02 — Resumable lossless profile migration

- Issue: TECH-002
- Baseline source: `c3c13f19e377729809451739a4c5026302b3e16c`
- Audit reproduction binary: SHA-256 `e3f283c8cc61ccae5d1549a1136b899d02abf3cf95a27c9fc7be8a42d7128702`
- Dependencies: integrated PR-T17 launch scheduling/maintenance receipt and PR-T01 guarded ownership primitives.

## Scope

Replace the best-effort roaming-to-local renames with a versioned, per-item migration journal and a bounded no-follow migration engine. Preserve every source until a verified destination is published; retain divergent destinations as actionable conflicts; resume retryable and interrupted work after restart; and keep profile readers on one coherent authoritative root while migration is incomplete.

## Contract decisions

- Supported items are exactly `settings.toml`, `session.json`, `recovery`, `macros`, and `extensions`.
- Recovery generations are migrated as owned trees with manifests/provenance intact. No blind merge is permitted.
- FC-03 durability order and FC-09 lossless `PathBuf` handling remain authoritative.
- Local is the only live write root. Bootstrap reads local settings when present and otherwise reads the bounded legacy file; an uncertain legacy read fails instead of silently loading defaults.
- Session, recovery, macro, extension and keymap disk readers wait for the T17 maintenance receipt. A separate five-item authority receipt probes local first without following links, then legacy only after definite local absence. Unreadable paths remain unavailable; an empty/no-op migration report cannot redirect a valid local reader.
- Portable, diagnostic and performance launches receive no installed-profile fallback paths. Installed macro and extension runtimes keep distinct read roots while every mutation destination remains local.
- The settings reader revision is captured at bootstrap. Published settings are reconciled only while that revision is unchanged, so a user edit made before the maintenance receipt remains authoritative.
- A first verified pass retains every source. A later retirement pass may move an individual file only after rehashing it under an identity lease. Directory handles cannot exclude late child creation on Windows, so published source directories are durably marked retained by policy and are not repeatedly rehashed or presented as retryable.
- Directory enumeration charges the shared budget before retaining each entry and rejects depth beyond 64. Tree hashes use lossless length-prefixed names, explicit type/end records, file lengths and per-file content hashes.
- `profile.migration.retry` is an actual command owned by the retained T17 maintenance request. It regates profile readers, schedules the retry, installs the new authority receipt, then wakes the readers.
- R2 runtime reconciliation keeps fallback extension packages read-only until an exact local inventory restore completes, including repeated retries to the same incomplete root; preserves running extension and macro operations during retry; switches future macro reads without replacing live edits; and applies settings only when the receipt proves a migrated destination is present and no value draft is active. Dismissal, stop and cancellation commands remain available while retry maintenance is unsettled.
- Windows requires every descendant handle to close before a nonempty staged directory can be renamed, including handles opened with delete sharing. The migration root identity remains pinned across that release and atomic rename; the journal retains legacy reader authority until a complete destination fingerprint matches. Failed verification quarantines only a reacquired matching identity when possible and otherwise leaves the unverified local tree explicitly non-authoritative for retry.

## State and authority table

| Journal state | Durable meaning | Reader authority | Source handling |
| --- | --- | --- | --- |
| `Pending` | No verified publication | Separate authority receipt | Retained |
| `CopiedVerified` | Staging matches a checked source generation | Separate authority receipt | Retained |
| `Published` | Destination was atomically published and reverified | Local | Files may be eligible for a fresh retirement attempt; directories are retained by policy |
| `SourceRetired` | Verified source was moved to the retained area | Local | Retained under local migration provenance |
| `FailedRetryable` | No safe completion was established | Separate authority receipt | Retained with retry detail |
| `Conflict` | Source and destination generations differ | Existing local destination | Both retained; user warning emitted |

## Focused checks

- Worker Cargo runs are prohibited. Root will execute the contained migration and launch commands after review/integration.
- `cargo test -p bareline-file-io profile_migration::tests --offline --locked`
- `cargo test -p bareline-platform-windows migration_ --offline --locked`
- `cargo test -p bareline --bin bareline migration_receipt_does_not_overwrite_a_newer_live_settings_revision --offline --locked`
- Deterministic fixtures cover a locked/read-denied first attempt followed by restart, injected atomic publication failure followed by staged resume, local-only, empty and damaged-journal authority, a divergent destination, retained recovery bytes, fresh file-generation conflict, durable directory-retention disposition, flat/deep traversal limits, and equal-size/equal-count tree framing. Runtime fixtures cover blocked fallback inventory removal/permission writes, successful loaded-fallback handoff to local extension and macro roots, and preservation of an open Settings tab.
- Native Windows adapter fixtures cover handle publication for files and nested directories, a real sharing violation followed by retry, engine restart from a verified stage, child creation while the production directory lease is held followed by the engine's retained-by-policy decision, and real junction rejection with an external sentinel.
- `python docs/implementation/2026-09-12-audit-implementation/evidence/probe_directory_rename_with_child_lease.py` records the native handle boundary in `evidence/T02-directory-rename-child-lease-probe.log`: parent rename fails with Win32 error 5 for descendant handles sharing read alone and read plus delete.
- The post-publication fixture injects a changed child and a failed exact-identity quarantine, proves source absence leaves the corrupted local tree unavailable, then restores the retained source, removes the injected corruption, and proves a later full verification succeeds.
- Failure/restart log assertions: first locked item is `FailedRetryable` with source present, local item absent and journal present; restart reaches `Published` with byte-identical local data and source still present. Injected publication failure follows the same retained-source contract and resumes from the verified stage.

## Unresolved qualification

- Root owns contained Cargo compilation/tests and redirected cross-volume qualification. Results remain pending.
