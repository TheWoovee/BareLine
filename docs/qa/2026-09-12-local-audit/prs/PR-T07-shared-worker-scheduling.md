# PR-T07 — Schedule queued work on available workers

Priority P2. Fixes TECH-009. Depends on PR-T06. Files: `crates/app/src/task.rs::Pool`, `crates/editor-surface/src/paged_view.rs:125–153`, paged submission call sites and worker tests. Coordinate with recovery work; do not create an app-crate dependency from editor-surface.

## Problem

Both pool designs assign jobs to private worker queues. If worker 0 is blocked in a file operation, a later short job assigned to it waits even when other workers are idle. `task_probe` proves this with two workers and three jobs. Paged workers also return one chosen sender without checking whether another worker has capacity. This affects responsiveness under mixed file operations, accessible reads and navigation; no claim of a measured universal UI latency is made.

## Implementation

1. Extract a small neutral bounded executor, or share an interface through an existing neutral layer without reversing crate dependencies. Use one bounded pending queue consumed by all workers. Release the queue lock before running a job. A Condvar-backed queue is sufficient; no Tokio or extra async runtime.
2. Preserve the current fixed worker cap and introduce a documented total queue cap. Admission is nonblocking; return a typed Busy/Closed error. Use explicit priority classes only where justified (interactive viewport/accessibility versus bulk file work) and prevent starvation through fair aging/round-robin between classes.
3. Keep mutation order at the document actor level, not through accidental queue affinity. Maintain `power_actor_busy`, identity/revision validation and one mutation per actor. Jobs for independent actors may run concurrently. Snapshot readers remain usable while long saves operate on immutable captured data.
4. If a worker cannot spawn, expose reduced capacity; no debug assertion may turn an expected resource failure into a panic. Apply PR-T06's cancellation/terminal rules when queues drain or workers disconnect.
5. Instrument submitted/running/queued/rejected counts and task kind in privacy-safe diagnostic spans. This is for qualification, not an always-running polling timer.

## Verification

Create a gated long job, run a second job to free another worker, then enqueue a short job. The short job must execute before the gate is released, without increasing thread count. Repeat against the paged service with a long save and a short operation on another document. Test full queue, canceled queued jobs, failed worker creation, shutdown, actor serialization and fairness under continuous bulk work. Assert byte-level save/edit/undo invariants remain intact. Use synchronization to prove scheduling; performance samples are informational and must not become narrow timing merge gates.

Done: no runnable job is stranded solely by assignment to a blocked worker while capacity is idle; cancellation is terminal; no regression to unbounded spawning or concurrent mutations of one actor.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
