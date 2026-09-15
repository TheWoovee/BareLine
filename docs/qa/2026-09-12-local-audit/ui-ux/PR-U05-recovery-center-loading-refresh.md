# PR-U05 — Give Recovery Center truthful loading, empty, and live-refresh states

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

Opening Recovery Center during background discovery shows the final-sounding message `No recovery checkpoints found.` A checkpoint can arrive moments later; closing the center then announces one exists, and reopening reveals it.

## Scope

- `apps/bareline/src/windows_app/recovery.rs`
- Recovery semantic/status nodes in `windows_app/accessibility.rs`
- Native recovery journey coverage

## Ordered implementation

1. Model center content explicitly as `Discovering`, `Ready(Vec<rows>)`, or `Failed(error)`. Do not infer empty from `rows.is_empty()` while discovery is pending.
2. On `recovery.open`, show a progress state if discovery has not completed. Keep destructive/recovery actions disabled with a meaningful reason.
3. When discovery completes, update rows in the open center without forcing close/reopen, preserve selection by recovery identity, and announce the result count once.
4. If discovery fails, show retry and close actions with the full error available to UIA.
5. If rows arrive while the center is closed, use a single notification routed through PR-U02; opening should immediately show the same ready state.
6. Refresh or remove a row after restore/export/delete and select the nearest surviving row.

## State, error, and race handling

- Tag discoveries with a generation/root identity; ignore late results from obsolete roots or relaunch state.
- If the selected recovery disappears during refresh, clear preview and select the nearest row.
- Closing the center cancels preview work but need not cancel global discovery; reopening attaches to the current generation.

## Tests and acceptance

- Deterministic delayed-discovery test: open center, assert “Searching for recovery checkpoints…”, publish one row, assert it appears without reopening.
- Empty completion says no checkpoints only after the worker finishes.
- Failure offers Retry and does not reuse stale rows.
- Native journey creates an Untitled checkpoint, opens during discovery, and never displays a false final empty state.
- UIA status announces loading then the final count exactly once.

## Risks and dependencies

Coordinate user-visible notices with PR-U02. Do not couple center lifetime to worker lifetime; recovery discovery also supports startup notification.

## Definition of done

Recovery Center never calls an in-flight search empty, live results appear without reopening, and every loading/error/ready transition is keyboard- and screen-reader-readable.

Status: Planned. Priority: P2. Finding: [UX-06](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
