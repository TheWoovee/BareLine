# Windows completion — 2026-09-16

## Authorized scope

The owner requested completion of all locally executable remaining work, sequentially, and delegated routine product decisions. Windows is the current release target. Real Linux/macOS host qualification is deferred to a later update; mocks cannot establish that qualification. No additional test computer is currently available from the owner.

Continue preserving existing changes and evidence. Use generated owned scratch data. Run focused checks during implementation and one consolidated qualification pass after code converges. Do not treat implementation, synthetic tooling checks, native product checks, signed release verification and independent review as interchangeable.

## Execution order

1. Finish offline authority consumers and signed metadata delivery (DEV-007).
2. Complete configured release assembly and resumable signing handoff (DEV-008).
3. Finish typed evidence producers and two-stage closure (DEV-005/009).
4. Implement remaining product journey procedures and acceptance/command definitions (DEV-003/004).
5. Implement bounded reliability soak and assess a local shakedown (DEV-006).
6. Resolve documented product limits, complete Windows-facing documentation and prepare available environment/qualification coverage.
7. Run consolidated Windows checks, repair observed defects, retain evidence and issue an honest readiness record.

## Boundaries

Real signing identities, public hosting endpoints and protected signing access cannot be fabricated. Clean alternate Windows versions, physical assistive/input/hardware coverage and independent review remain unverified where unavailable. The 72-hour requirement requires actual elapsed uninterrupted execution. These limits do not block independent implementation work.

## Result

Four remaining integration/tooling packages are implemented with focused verification (DEV-005/007/008/009); INV-001 is dispositioned. The live queue is 7 completed/dispositioned, 41 active and 1 owner-deferred. DEV-003/004/006 retain explicit implementation/qualification gaps; no work was silently marked complete because a prerequisite was unavailable.

The native adapter now implements ten procedures. Three older journeys retain focused native passes. The seven new procedures need native execution; the latest column rerun stopped on foreground loss. Its discovered DirectWrite bug and the discovered UDL persistence defect have focused real Windows regressions. Crash/recovery, extension-isolation and installer/update automation still require lab integration; the concrete runbook is `tests/e2e/WINDOWS-LAB.md`.

Consolidated verification, source identities, failures and retained artifacts are recorded in `docs/qa/2026-09-16-windows-completion/README.md`. Actual release signing, clean Windows-floor VMs, physical input/AT/hardware coverage, 72-hour elapsed testing and independent final review remain external execution gates. Linux/macOS qualification is owner-deferred to the next update.

Final integration also repaired the first-party PowerShell runner: validated artifact paths are passed explicitly, absent environment variables are removed on restore, and T09 capture control JSON is excluded from command receipts. Fast 3/3 passes. Notice generation revealed 15 omitted upstream license texts; exact commit-bound supplements now allow full notices and SDK license aggregation. The Inno compiler installation-command preparation was blocked by automatic review ("blocked by policy"); no installer compilation or product installation occurred.
