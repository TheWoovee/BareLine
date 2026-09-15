# PR-T06 — Give background tasks reliable cancellation and terminal outcomes

Priority P2. Fixes TECH-007/008. Dependencies: none; land before expanding pool adoption in PR-T07/T08. Files: `crates/app/src/task.rs`, `apps/bareline/src/windows_app/launch.rs`, `crates/app/src/accessibility.rs`; corresponding task tests. Keep the no-async-runtime architecture.

## Evidence

`../evidence/task_probe.rs` includes the production task module unchanged. Canceling after notification still returns `Some(42)`; Drop leaves the cancellation flag false. A controlled panic disconnects the result but `poll()` returns the same `None` as pending forever. The dead worker remains in `worker_count` and `running`. Debug/unwind behavior was executed; the release profile deliberately aborts on panic and must be described separately.

## Implementation sequence

1. Replace `Option<T>` polling with `TaskPoll<T> { Pending, Complete(T), Cancelled, Failed(TaskFailure) }` or an equivalent explicit terminal state. Channel disconnection without a delivered outcome is a failure, not Pending. Make consumption one-shot and define the post-consumption response.
2. Make cancel linearization explicit. Check cancellation before executing queued work, before publishing, and before returning a buffered result. Once the caller cancels, it must never observe success from that task. Drop signals cancellation; abandoned work checks the flag at existing bounded checkpoints. Terminal cancellation must still wake the owning controller so it clears busy state.
3. Use a completion guard so running metrics decrement on every unwind/return path. Prune dead worker channels and report actual live capacity. If the build unwinds and the architecture allows containment, catch task panics at the worker boundary and report a sanitized failure; never pass panic payload contents into user diagnostics. Do not change global release `panic=abort` casually or promise catch-unwind handles abort/OOM.
4. Update launch navigation to clear `navigation`, stop retrying a closed/failed task, and report a useful failure/cancellation. Update accessible-text jobs to clear `pending` on all terminal paths and invalidate stale document/revision results. Fire-and-forget jobs also need terminal cleanup semantics.
5. Preserve typed `Wake::One(Source)` routing and bounded queues. Do not deliver results to a new tab reusing an old index.

## Tests and acceptance

Use channels/barriers, not sleeps, to cover cancel before dequeue, cancel while work runs, cancel after send-before-poll, dropped owner, send failure, work error, notification error/panic where supported and worker loss. Assert one terminal event and no stale publication. Retain the pool bound test. Port each original probe into focused regressions and keep the standalone evidence. A launch canceled after its result is queued must not move a caret; failed accessible reads cannot remain permanently Pending. Run focused app task/accessibility tests and one native compile. Do not broaden adoption until these contracts hold.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
