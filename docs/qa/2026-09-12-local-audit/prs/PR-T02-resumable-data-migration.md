# PR-T02 — Make profile migration resumable and lossless

Priority P1. Fixes TECH-002. Depends on PR-T01's shared path/ownership primitives if reused; otherwise separate launch-file ownership to avoid conflicts. Primary code: `apps/bareline/src/windows_app/launch.rs:413–428` and its migration callers; destination settings/session/recovery services in `crates/file-io` and `crates/settings`.

## Problem

`migrate_from_roaming` creates the local directory, attempts five renames, ignores errors, then refuses every later attempt because the local directory exists. A single sharing violation or cross-volume profile redirect can make all prior settings, sessions, recovery entries, macros and extensions appear missing. The source data still exists but is abandoned. The generated file-lock reproduction is in `../evidence/launch_probe.rs` and `launch-probe.log`.

## Implementation

1. Introduce a versioned migration journal with per-item states (`Pending`, `CopiedVerified`, `Published`, `SourceRetired`, `FailedRetryable`, `Conflict`). Destination existence alone is never completion. Persist journal transitions atomically under the new data root.
2. Preflight both roots using no-follow trusted directory guards. Enumerate only the five explicitly supported items; bound recursive traversal and reject links. Preserve originals until each destination is verified and published.
3. Try same-volume rename where safe. For cross-volume moves, copy into an exclusive staging sibling, flush, verify file hashes/tree manifest, then atomically publish. Never merge recovery journals by blind recursive overwrite. Migrate a whole owned generation with its manifest and provenance.
4. If a destination item already exists, compare identities/content. Identical data can finish migration; divergent settings/session/extension receipts require an explicit recoverable conflict, preserving both. Existing new settings must not be replaced silently.
5. Resume incomplete items after restart and after transient failures. Record a concise user-visible warning with a retry action and the retained source location. Move work off the first-frame/UI path. Keep readers on a coherent authoritative root while migration completes.

## Verification

Reproduce the locked-settings sequence: first attempt is retryable; release the handle; the second succeeds despite the destination directory already existing. Cover cross-volume-copy fallback using an injectable rename failure, interruption after each journal transition, pre-existing different destination data, partial recovery tree, link rejection and full success. Verify recovery can still restore the original bytes before and after migration. Tests must assert that no source is removed before a verified destination exists.

Acceptance: no silent reset to defaults, no permanent retry suppression, no partial journal advertised as complete, no network profile content read outside the chosen migration scope. Provide a migration-state table and actual failure/restart logs in the PR evidence note. Keep old compatibility behavior until the lossless path is verified.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
