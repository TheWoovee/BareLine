# PR-T18 — Retire failed and canceled launch requests

Priority P2. Fixes TECH-021/022. Depends on PR-T06's terminal task outcomes and coordinate with PR-T17's launch refactor. Files: `apps/bareline/src/windows_app/launch.rs:9–156`, `instance.rs:57–100`, `crates/app/src/workspace.rs` open/close completion interfaces. Keep user paths and persistent document identities distinct.

## Current failure paths

`LaunchRuntime::queue` caps `pending` at 256. `launch_pump` retains any path for which it cannot find an editor, even when opening has already failed. Repeated nonexistent/denied paths can permanently consume capacity; the instance handoff refuses new requests when the queue is full. The pending navigation task is also only polled after finding its live editor, so closing that tab can strand the single `navigation` slot and block later CLI line/column navigation. These are traced source paths, not a claimed native saturation measurement.

## Implementation

1. Assign every accepted path request a stable request ID and explicit state: Queued, Opening, Opened(document identity), Navigating(task), Monitoring, Complete, Failed or Cancelled. A pathname alone is not sufficient to distinguish a failed open from an open still pending or a newly reopened document.
2. Expose terminal open receipts from Workspace (success identity or error) to the launch consumer. Retire failed requests immediately and surface their error once. Use `path_loading` only as a transitional compatibility check, not as a substitute for request identity.
3. Poll/retire navigation tasks independently of whether a tab remains visible. On close, cancel the associated task and release the slot. On success, apply selection only if request, document identity and captured revision/generation still match. Reopening the same path cannot receive an old task's caret position.
4. Keep the 256 limit as an outstanding-work bound, not a lifetime error counter. If admission fails, return a clear bounded handoff rejection so the sender can report/fallback according to the existing single-instance contract. Do not accept an IPC request and then silently lose it.
5. Give monitor startup retry a terminal/retryable distinction and bounded retry policy; it must not reinsert permanent errors forever. Cancellation and shutdown clear all owned tasks without blocking the UI.

## Verification

Use a fake open service/barriers to submit more than 256 **sequential failed** opens; each failure must release capacity and a later valid path must open/navigate. Test a genuinely full concurrent queue rejects predictably. Start paged navigation, close its tab before completion, then open another requested line: the second task must run, and the closed/reopened path must not receive stale selection. Cover same-path duplicate requests, canceled IPC sender, readonly/monitor flags, failed task, shutdown and document mutation during navigation.

Native acceptance: two generated missing files followed by a valid file using ordinary CLI handoff show truthful errors and successful navigation; a gated large-file close/reopen test verifies no stale caret jump. Keep these tests in the existing harness and retain source/binary identity. Do not clear all pending requests indiscriminately or weaken the admission bound.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
