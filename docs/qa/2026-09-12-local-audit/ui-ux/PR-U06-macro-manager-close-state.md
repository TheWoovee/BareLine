# PR-U06 — Keep Macro Manager Close usable in every manager state

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

When Macro Manager opens with no recorded/imported macros, its visible Close button is disabled even though Escape exits. Pointer and accessibility users encounter an affordance that cannot be invoked.

## Scope

- `apps/bareline/src/windows_app/macros.rs`
- `crates/app/src/macros/manager.rs`
- Macro manager state/accessibility tests

## Ordered implementation

1. Replace the open-frame reliance on cached `self.command_context`: manager drawing reads it at `apps/bareline/src/windows_app/macros.rs:402-415`, while the only assignment is later in `macros_pump` at `:973`. Recompute or invalidate the cache on every manager open/close transition.
2. Decouple the Close button's enabled state from macro selection, storage readiness, recording, playback, and process state.
3. Keep `annotate_context`'s closed-manager rule (`apps/bareline/src/windows_app/macros.rs:382-387`), but guarantee the post-open context is installed before draw/semantic snapshot.
4. Keep Escape and Invoke on Close routed through the same close effect and focus-restoration path.

## State and error handling

Closing the manager should not cancel active recording/playback unless that is an existing explicit policy; it only dismisses the surface. A storage-load failure must still leave Close enabled.

## Tests and acceptance

- State-table test: zero macros, selected macro, loading, failed loading, recording, and playback all expose an enabled Close button whenever the manager is open.
- UIA Invoke on Close dismisses the manager and restores editor focus.
- Native journey opens an empty manager and closes it by mouse/Invoke, then repeats with Escape.

## Risks and dependencies

Small, isolated state fix. The principal risk is stale command context shared between the native menu and the already-open manager; test both contexts.

## Definition of done

The visible Close button is enabled and invokable in every open-manager state, and all dismissal paths restore focus consistently.

Status: Planned. Priority: P3. Finding: [UX-07](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
