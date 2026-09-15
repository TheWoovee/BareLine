# PR-T08 — Fallible extension workers

Baseline: `003ccfd2928e1902939492505f48013b6e05d12e`

## Scope

- Move extension invocation, manager/catalog/package, and inventory export jobs onto bounded shared admission.
- Keep dedicated extension transport and host watchdog threads separate, named, and fallibly spawned.
- Make admission, spawn, disconnection, cancellation, and shutdown terminal without resuming an uncontained child or retaining partial ownership.
- Preserve package verification, sandbox, token, job, pipe identity, environment, and grant boundaries.

## Decisions

- `extensions::extension_worker` is one process-global `BoundedExecutor` with four running slots and one total cap of 32 pending jobs across windows. Invocation is `General`; manager, package, catalog, and inventory work is `Bulk`. Admission remains nonblocking and reports Busy or Closed without setting a pending receiver.
- Invocation, manager, and inventory jobs own a one-shot result/wake guard. Unwind or rejected admission publishes a sanitized terminal error before waking exactly once. Invocation ownership also calls `InvocationBroker::finish` so grants and staged proposals are revoked.
- Queued cancellation is checked before work begins. Manager/package code retains its existing cancellation checks at durable publication boundaries; a cancellation after successful publication does not erase the completed receipt. Inventory owns a shell-lifetime cancellation token, checks it before any filesystem work and immediately before the staging-file rename, and uses a process-wide operation sequence in stage names so concurrent windows cannot claim the same path.
- Temporary cleanup ownership begins only after create_new has successfully opened the stage. An existing stage therefore remains untouched when exclusive creation fails.
- The extension-host watchdog is a named fallible thread created before component parsing/instantiation. The transport watchdog is a named fallible thread created before child launch. `SandboxedProcessLauncher` continues to create suspended, job-contained children and resumes only after containment. Failed guard transfer terminates the job, drops its kill-on-close handle, boundedly reaps the child, then joins the watchdog.
- Transport and host watchdogs remain dedicated because they must supervise and interrupt the operation they accompany; queueing them behind extension work would defeat timeout containment. Test-only pipe peers and unrelated actor/service threads remain outside this scoped migration.

## Changed files

- `docs/implementation/2026-09-12-audit-implementation/PR-T08.md`
- `apps/bareline/src/windows_app/extensions.rs`
- `apps/bareline/src/windows_app/inventory.rs`
- `crates/platform-windows/src/extension_transport.rs`
- `apps/extension-host/src/lib.rs`

## Evidence

- `rustfmt --edition 2024` on the four changed Rust files: PASS.
- `git diff --check`: PASS (line-ending conversion notices only).
- Production census over the four named source files: no ordinary `std::thread::spawn` remains; the two dedicated watchdogs use named fallible `Builder::spawn`. The two remaining `thread::spawn` hits in `extension_transport.rs` are existing test-only pipe peers.
- Review corrections add a preexisting-stage sentinel regression for exclusive-create failure and a gated queued-inventory cancellation regression that observes no target or stage creation.
- Requested central checks:
  - `cargo test --offline --locked -p bareline windows_app::extensions::manager_tests -- --test-threads=1`
  - `cargo test --offline --locked -p bareline windows_app::inventory::worker_tests -- --test-threads=1`
  - `cargo test --offline --locked -p bareline-platform-windows extension_transport::tests::watchdog_spawn_failure_is_terminal_before_launch -- --exact`
  - `cargo test --offline --locked -p bareline-platform-windows extension_transport::tests::failed_watchdog_transfer_terminates_and_reaps_contained_child -- --exact`
  - `cargo test --offline --locked -p bareline-extension-host tests::watchdog_spawn_failure_precedes_component_execution -- --exact`
  - `cargo test --offline --locked -p bareline-extension-host tests::safe_component_runs_and_cpu_loop_terminates -- --exact`
  - `cargo test --release --offline --locked -p bareline-extension-host --test first_party`

## Unresolved qualification

- Worker policy prohibits Cargo/native/full gates. All listed central checks, including the real release-profile WASI cases, remain pending.
