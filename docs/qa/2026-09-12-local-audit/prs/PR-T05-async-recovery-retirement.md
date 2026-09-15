# PR-T05 — Make discard cleanup asynchronous and truthful

Priority P1/P2. Fixes TECH-005/006. Depends on PR-T01's owned deletion policy. Main files: `crates/file-io/src/resident_recovery.rs:352–400`, `paged_recovery.rs`, `crates/editor-surface/src/lib.rs:856–860`, `crates/app/src/workspace.rs::close`, and the shell close/exit flow. Reuse existing recovery status/receipt patterns.

## Problem

`discard_now()` blocks for checkpoint and retirement receivers (up to two seconds each), then sleeps/retries deletion. It removes all owned paths from state and resets status even if deletion never succeeds. It is reached from editor close/discard and Drop. The successful cleanup test does not cover locked files, slow workers, or failed purge. This creates UI stalls and a false impression that discarded recovery text has gone.

## Implementation

1. Model retirement explicitly: `Active -> DiscardRequested -> WorkersDrained -> Purging -> Complete` or `CleanupPending(reason, owned_paths)`. Generation handles and pending paths remain owned until successful deletion. Return typed outcomes instead of `()`.
2. Cancel and drain workers off the UI thread. Avoid long waits/sleeps in close, Drop, drawing, and event handling. Keep a retained cleanup service with bounded concurrency and retry backoff, independent of the closed tab's lifetime. Drop must hand off owned work safely, not perform seconds of synchronous I/O.
3. Persist a minimal discard tombstone before allowing tab retirement so a crash cannot resurrect intentionally discarded text in Recovery Center. Write it in the worker and keep the close request logically pending while the rest of the UI remains interactive; do not block the UI to obtain durability. On tombstone failure retain the recovery ownership and offer retry/cancel, rather than claiming durable discard. The tombstone must not contain document content. The janitor can resume deletion after restart without presenting these journals as recoverable drafts.
4. Distinguish “document discarded” from “recovery cleanup pending.” Report a nonblocking, actionable cleanup warning on persistent failure, with retry. Do not falsely set public cleanup status to complete. Keep failed paths for retry and privacy accountability.
5. Preserve save and ordinary crash-recovery rules: closing without discard must retain legitimate recoverable text; saving/undo-to-clean retires only obsolete generations. Reuse exact identity/generation checks and no-follow deletion guards.

## Acceptance tests

Inject a checkpoint worker that holds a handle, a delayed retirement worker, purge access denial, cancellation and exit during each transition. The UI-facing request must return promptly without waiting for storage; the test should use a synchronization gate, not a narrow elapsed-time assertion. Release the handle and prove every generation is eventually removed. Persistent failure must retain a cleanup record and truthful status; restart must not resurrect a discarded draft. Retain the existing successful-discard and recovery-restart tests. Native: type, discard, continue editing another document while cleanup is delayed; observe no frozen window or duplicate warnings.

Also review `Workspace::adopt_recovered_resident`'s synchronous inspection/restore-text call. Keep any large disk materialization in the existing restore worker before returning to the UI; record this review as part of the lifecycle responsiveness evidence, not a separate speculative defect.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
