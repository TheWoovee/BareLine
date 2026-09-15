# PR-T08 — Bound extension operations and handle spawn failure

Priority P2. Fixes TECH-010. Depends on PR-T06/T07 for jobs moved to the common executor. Do not migrate every actor/service in the repository in this PR.

## Concrete scope

Production infallible spawn sites remain at `apps/bareline/src/windows_app/extensions.rs::start` and `manager_work`, `inventory.rs::inventory_pump`, `crates/platform-windows/src/extension_transport.rs::run_host_process`, and `apps/extension-host/src/lib.rs::execute` watchdog. The census is `../evidence/graft-thread-spawn.txt`. Ordinary `std::thread::spawn` can panic on resource exhaustion; the editor release profile aborts. The audit did not exhaust the user's machine to force this failure.

## Implementation

1. Move manager/catalog/package jobs and inventory export onto the tested bounded executor. Keep a global cap for invocation/manager work across windows, with explicit Busy responses. A rejected submit must not leave `pending` set or start a timeout without a child.
2. Use fallible named `Builder::spawn` only for dedicated watchdog/transport threads that must not queue behind the operation they supervise. If a watchdog cannot start, fail before resuming the untrusted child; terminate and reap any suspended child and release pipe/job handles.
3. Give each operation one owner for cancellation, result, watchdog, child and installed/temporary artifact cleanup. Poll Disconnected as terminal failure. A timeout or cancellation cannot publish a partial package receipt or leave grants active. Notify the exact shell runtime once.
4. Preserve current signed package verification, restricted token, process memory/active-process limits, cleaned environment and authenticated pipe identity. Never use a successful spawn as evidence of a valid extension result.
5. Document legitimate dedicated actors left outside the pool; do not replace them indiscriminately. Make the bounded worker census reflect the exact migrated sites and their queue/cancellation limits.

## Tests

Add injectable spawn/submit failure before each ownership transfer. Verify no process resumes without containment/watchdog, no busy state persists after failure, and all handles/staging files are retired. Test overlapping extension manager actions, cancel while queued, extension-host timeout, failed result send, and closing the editor during invocation. Keep the current sandbox/isolation tests. Run the small real WASI host integration cases from PR-T10 after wiring. Acceptance requires a controlled error and responsive editor under injected resource failure, not an abort or silent loss of work.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
