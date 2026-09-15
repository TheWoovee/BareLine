# PR-T07 — Shared worker scheduling

Baseline: `1708befe0a119506cab01d854467cc8fc4130335`

## Scope

- Replace private per-worker queues with neutral shared bounded scheduling.
- Adopt the executor in the app task pool and paged editor worker without reversing crate dependencies.
- Preserve fixed worker limits, nonblocking admission, cancellation semantics, and document actor serialization.
- Add deterministic scheduling, capacity, cancellation, shutdown, and paged-service regressions.

## Decisions

- `bareline-platform::executor::BoundedExecutor` owns one FIFO `VecDeque` with a documented total pending cap. All fixed workers consume that queue after releasing its mutex.
- FIFO provides finite-position fairness without a priority scheduler. `WorkKind` classifies aggregate general, interactive, bulk, and maintenance counts without recording paths or document content.
- Admission is nonblocking and returns typed `SubmitError::{Busy, Closed}`. Failed thread creation reduces reported live capacity.
- Executor drop closes admission and detaches immediately. Accepted work drains against worker-owned shared state; UI/runtime destruction never joins a gated job.
- The app task pool delegates scheduling and metrics to the neutral executor while retaining T06 cancellation, one-shot terminal outcomes, and sanitized panic behavior.
- The paged worker preserves four threads and the former aggregate capacity of 64 pending jobs. Existing actor mutexes, `power_actor_busy`, cancellation, and identity/revision validation remain the mutation-order authority.
- Paged result and linked-transfer adapters publish a sanitized failure before waking on unwind. Fire-and-forget spill cleanup clears its pending owner on unwind. Release `panic=abort` remains unchanged.
- The T03 prepared save destination, document identity validation, and write condition remain the save boundary after rebasing onto the integrated T03 snapshot.

## Changed files

- `crates/platform/src/executor.rs`
- `crates/platform/src/lib.rs`
- `crates/app/src/task.rs`
- `crates/editor-surface/src/paged_view.rs`
- `crates/editor-surface/src/mapped_viewport.rs`
- `crates/editor-surface/src/paged_transfer.rs`
- `crates/editor-surface/src/paged_spill.rs`
- `docs/implementation/2026-09-12-audit-implementation/PR-T07.md`

## Evidence

- `rustc --edition 2024 --test crates/platform/src/executor.rs -o .tmp-pr-t07-executor-tests.exe` followed by `.\\.tmp-pr-t07-executor-tests.exe --test-threads=1`: PASS, 5 passed, 0 failed. Covers idle-worker dispatch, total queue cap and typed closure, failed spawn/reduced capacity, FIFO fairness, and nonblocking drop followed by eventual worker exit. The temporary executable was removed.
- `git diff --check`: PASS (line-ending conversion notices only).
- Central app-task compilation initially failed with E0382 because the main paged completion guard moved `notify` before recovery setup borrowed it. The guard now takes an `Arc` clone; central compilation and paged checks remain pending.
- Requested root-run checks after integration:
  - `cargo test --offline --locked -p bareline-platform executor::tests -- --test-threads=1`
  - `cargo test --offline --locked -p bareline-app task::tests -- --test-threads=1`
  - `cargo test --offline --locked -p bareline-editor-surface paged_view::peer_tests::paged_adapter_reports_terminal_failure_and_wakes_after_unwind -- --exact`
  - `cargo test --offline --locked -p bareline-editor-surface paged_view::peer_tests::paged_save_does_not_strand_another_documents_edit_and_undo -- --exact`

## Unresolved qualification

- Worker policy prohibits Cargo. Root owns the listed contained checks and integrated native qualification.
