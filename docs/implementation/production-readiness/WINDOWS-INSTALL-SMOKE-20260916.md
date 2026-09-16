# Windows local installer and quick smoke — 2026-09-16

## Request and scope

The owner explicitly requested a Windows installer, installation on this PC and a quick feature test. This authorizes the local product installation and owned native input for this run. Work remains sequential, with one necessary incremental optimized build and a small smoke subset instead of a full suite.

Contracts: [Windows packaging](../../../packaging/windows/README.md), [installer definition](../../../packaging/windows/bareline.iss), [native adapter](../../../tests/e2e/README.md), [current native resume point](../../UNLOCK_CHECKLIST.md), and the product authority in [decision log](../../blueprint/07_DECISION_LOG.md).

## Procedure

1. Build the current preview editor and update helper with locked dependencies, without release trust or QA fault features.
2. Generate current dependency notices and CycloneDX metadata, assemble the payload and compile with the pinned Inno Setup 6.4.3 compiler.
3. Install per user, preserving existing profiles and default associations; verify installed payload hashes and Start Menu registration.
4. Run the column procedure first as requested by the native resume checklist, followed by a bounded core editing smoke subset against the installed executable. Use generated documents and isolated profiles only. Stop native input on foreground loss, physical Escape or unavailable desktop; preserve failures.
5. Retain build/package/install/native observations and report the actual scope. This unsigned preview installation does not qualify signed releases, Windows clean-VM upgrade/rollback, other platform floors or the full feature matrix.

## Result

Completed: unsigned installer and portable ZIP built; per-user installation verified; four installed-app journeys passed all 12 steps with clean exits. One optimized build (4m 48s), about 66 seconds of native checks, no full suite. [Execution report](../../qa/2026-09-16-windows-install-smoke/README.md) preserves the initial post-install verifier error and its read-only correction. Evidence root: `target/qualification/windows-install-smoke-20260916`.
