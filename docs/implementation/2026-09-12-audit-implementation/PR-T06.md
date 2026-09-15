# PR-T06 — Task terminal states

Baseline: `3c7f4a1681d676b38b49192b594578b914222212`

## Scope

- Give background tasks explicit pending, completed, cancelled, and failed outcomes.
- Make cancellation and owner drop prevent successful publication.
- Keep worker metrics correct across normal return and unwind, and prune lost workers.
- Update launch navigation and accessibility consumers to clear terminal work.
- Preserve the bounded, no-async-runtime architecture and release `panic=abort`.

## Decisions

- `Task::poll` returns `TaskPoll::{Pending, Complete, Cancelled, Failed, Consumed}`. Terminal delivery is one-shot and later observation is explicitly `Consumed`.
- `Task` remains a single-consumer type through its unsynchronized `Receiver`; cancellation clones share a gate that linearizes cancellation against successful result publication.
- `Task::cancel` publishes cancellation through the shared cancellation flag and fires the owning wake exactly once, without waiting for queued or gated work. `Drop` signals cancellation without waking an owner that no longer exists.
- Work panics are converted to sanitized `TaskFailure::Panicked` outcomes when unwinding is available. The worker boundary separately contains task/notification panics and uses guards for running/live metrics. The repository's release `panic = "abort"` remains unchanged.
- Accessibility reads use the shared bounded fire-and-forget pool, but schedule terminal cleanup outside the protected read. Every failure clears `pending`; cached results include and revalidate document identity and revision.
- Launch polling changes only the navigation task consumer and clears it on every terminal outcome.

## Changed files

- `crates/app/src/task.rs`
- `crates/app/src/accessibility.rs`
- `apps/bareline/src/windows_app/launch.rs`
- `docs/implementation/2026-09-12-audit-implementation/PR-T06.md`

## Evidence

- `rustc --edition 2024 --test crates/app/src/task.rs ... --test-threads=1`: PASS, 9/9 before the final two pool/disconnection regressions were added; the Cargo run below covers the final suite.
- `cargo test --offline --locked -p bareline-app task::tests -- --test-threads=1`: PASS, 11 passed, 0 failed, 100 filtered out.
- `cargo test --offline --locked -p bareline-app accessibility::tests::paged_read_terminal_cleanup_rejects_stale_results -- --exact`: PASS, 1 passed, 0 failed, 110 filtered out. This schedules both a controlled unwind/failure and a stale successful result through the actual shared worker cleanup path.
- `cargo check --offline --locked -p bareline`: BLOCKED before native consumer compilation by five unrelated save-interface errors in baseline `crates/app/src/workspace.rs`: its `IoRequest` initializers use `target`/`expected` while the dependency artifact visible through the shared target exposes `destination`. No out-of-scope correction or repeat was made.
- `git diff --check`: PASS (line-ending conversion notices only).

## Unresolved qualification

- Native compilation must be repeated after the concurrent save-interface work is reconciled/integrated. Wider integration and native behavior qualification remain owned by the root coordinator.
