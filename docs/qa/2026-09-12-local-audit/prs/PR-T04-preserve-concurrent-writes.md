# PR-T04 — Preserve displaced bytes across the save commit race

Priority P1. Fixes TECH-004. Depends on PR-T03's typed destination. Own `crates/platform/src/lib.rs::LocalFileSystem`, `crates/platform-windows/src/files.rs::commit`, `crates/file-io/src/lifecycle.rs::save_bytes`, replacement fault fixtures and the paged save service. This is a data-safety PR; require an independent reviewer.

## Failure mechanism

The expected fingerprint is checked and its file handle dropped, followed by metadata validation and pathname-based `ReplaceFileW` without a backup. Another writer can change or replace the target in between. A post-save fingerprint validates the new editor bytes, not the displaced bytes. `../evidence/save_probe.rs` injects this interleaving through a platform adapter and delegates to the real Windows commit: save succeeds and the intervening payload is lost.

## Implementation contract

1. Replace the `commit(staged, target, existed) -> Result<()>` contract with a prepared commit transaction and a typed receipt containing target identity, preserved prior-version location/identity, commit state and cleanup responsibility. Keep Windows handles out of neutral crates.
2. For replacement, atomically preserve the displaced target through the Windows replacement backup facility in a unique, verified, same-directory location. The backup path must be exclusively owned and unavailable for substitution. Guard ancestors/stage and validate the actual displaced file against the approved fingerprint after the replacement. A second preflight check alone is not a solution.
3. If the displaced bytes differ, return an explicit conflict-after-commit state retaining **both** versions, their identities and recovery paths. Do not unconditionally roll back over another subsequent writer. Restore only when the current target can be proven to be this transaction's output under the platform transaction policy; otherwise retain both and ask the user through the existing conflict flow.
4. For create-new, preserve the no-replace behavior if a competing file appears. For normal replacement, retire the backup only after durable target verification and receipt publication. Crash recovery must distinguish precommit, committed-unacknowledged, conflict and cleanup-pending transactions.
5. Surface meaningful conflict actions in the shell: compare, save the editor version elsewhere, retain the other version. Never mark the document saved on an unresolved outcome. Preserve DACL/ADS/attributes and existing no-in-place fallback policy.

## Verification

Extend `fault_transitions` and Windows replacement tests with barriers immediately after expected verification, before replacement, after replacement and before receipt. At each barrier inject a write, rename, delete/recreate, cancel or simulated failure. The acceptance invariant is that the payload displaced by this transaction remains recoverable and the editor's proposed bytes are retained on conflict; no successful receipt claims that a conflicting target was unchanged. This does not promise recovery of versions that independent external writers have already overwritten among themselves. Cover resident/encoded/paged save, absent destination race, backup collision, sharing violation, low disk, restart with orphan transaction and another writer after commit.

Do not promise filesystem compare-and-swap from a pathname recheck. Document the platform guarantees, remaining unavoidable visibility window, and recovery policy explicitly. Done requires the original reproduction to retain the intervening bytes and report the correct typed outcome, with byte-level evidence.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
