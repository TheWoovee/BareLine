# PR-U02 — Notification routing

## Scope and baseline

- Finding: UX-04.
- Baseline: `6e8a4e92776c94c650e250d2dac099c17602d08e` on `codex/audit-20260912-u02-fresh`.
- Contracts inspected: the shared modal owner, Save As conflict ownership, recovery ownership, and the bottom dock/status layout.
- Visual reference: `docs/blueprint/mockups/bareline-recovery-center.png`.

## Final behavior

Notifications enter the shell as typed events with a stable id, monotonic revision, explicit level and kind, complete text/details, optional document and generation owner, and scoped, transient, or persistent lifetime. Producers choose the level and lifetime; redraw never ingests a workspace string or infers severity from English text.

Active recovery preparation is stored and presented as typed scoped status for the active document identity. Switching documents projects only that document's current status. Completion resolves it. Failure replaces it with one persistent error carrying the complete error and recovery directory. Closing a document expires its UI-only progress while persistent recovery, cleanup, save-conflict, and process outcomes retain operation or global ownership until acknowledged or resolved.

Unresolved warnings and errors are retained independently of visual capacity. The stack paints only rows that fit the current height, up to the visual limit, and reserves an overflow control whenever notices are hidden and at least one row fits. Accessibility publishes only controls painted in that frame, so a zero-row viewport does not advertise fabricated bounds. The overflow details contain every hidden event, including rows hidden by a short viewport. Resolved transient history is bounded; active owned events are never discarded to satisfy that bound.

Every notification exposes its complete message through Details even when the producer supplied no separate diagnostic. Summary and detail wrapping use measured Unicode character boundaries and may discard whitespace used only at a visual wrap boundary; the Details value preserves the exact source. The detail surface uses the shared modal owner: pointer, keyboard, and UI Automation actions enter the same inert-background layer; Escape, Enter/Space on Close, Page Up/Down, the pointer Close button, and the UIA Close action share dismissal and focus restoration. Closing the modal also relinquishes toast keyboard focus before input returns to the editor. Toast focus clears on outside pointer input, expiry, acknowledgement, resolution, and retired ownership. Replacement preserves the stable control identity and active detail owner.

Save conflict and cleanup synchronization is typed and source-owned. The pump no longer clears `workspace.message` based on a coincident safety transaction, so a later unrelated pump result cannot be erased. Actual lifecycle, recovery, search, watch, session, and process failure producers enqueue explicit typed outcomes at their completion boundary.

## Controlled evidence

Code-level tests cover:

- forty unresolved persistent conflicts retained behind a height-aware overflow route without replay;
- exact revision dedupe, replacement, acknowledgement, and later reannouncement after resolution;
- document and generation scoped progress retirement while a closed-document failure survives;
- complete Unicode summary/details and leading multibyte whitespace wrapping;
- pointer detail/Close geometry and focus cleanup when an owner disappears;
- typed severity independent of message wording and bounded resolved history.

Local non-Cargo verification:

```text
rustfmt --edition 2024 --check apps/bareline/src/windows_app.rs apps/bareline/src/windows_app/*.rs
git diff --check
```

Root-owned contained commands requested:

```text
cargo test -p bareline toast::tests -- --nocapture
cargo test -p bareline modal::tests -- --nocapture
cargo test -p bareline recovery -- --nocapture
cargo test -p bareline notification_shell_tests -- --nocapture
cargo check -p bareline
```

## Remaining verification

The root-owned Cargo lane must compile and run the focused tests. Native review should verify a short viewport, pointer-open then Escape, pointer Close, UIA background suppression and Close, focus return to the editor or bottom dock invoker, complete long-path details, and no redraw or tab-switch replay.
