# PR-U10 — Make Settings Revert an honest undo action

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

Changing Font size from 12 pt to 9 pt immediately autosaved and showed `All changes saved`. The still-visible Revert action then did nothing. `acknowledge_saved` advances the saved baseline to the new value, and `revert()` merely copies that baseline, so the label promises a capability that disappears almost immediately.

## Scope

- `crates/settings/src/persistence.rs`
- `crates/app/src/settings.rs`
- `apps/bareline/src/windows_app/settings.rs`
- Settings semantics/render/state tests

## Ordered implementation

1. Implement and document this contract: Revert restores the state from when Settings opened, including values already autosaved during that Settings session. Show that scope in its help text; do not silently reduce Revert to a brief unsaved-write window.
2. Capture a per-scope baseline on Settings open and retain it across successful autosaves until close or explicit baseline acceptance.
3. Enable Revert only when current effective settings differ from that baseline; invoking it applies the baseline, queues one save, refreshes runtime consumers, and remains retryable on write failure.
4. Keep Reset Section separate: it removes overrides after confirmation and is undoable through the same open-session baseline.
5. Expose the enabled reason/status through UIA and show success/failure once.

## State, error, and race handling

- Revert during a pending save supersedes that generation; late acknowledgement cannot restore the superseded value.
- User/workspace scopes retain independent baselines.
- External file changes require a conflict/reload decision rather than silently redefining the open-session baseline.

## Tests and acceptance

- Change an autosaved value, wait for `Saved`, invoke Revert, and verify UI, runtime, and disk return to the opening value.
- Reset Section then Revert restores all section overrides from the opening baseline.
- Delayed/failing saves, scope switches, close/reopen, and external edits retain truthful enabled state and error recovery.

## Dependencies and risks

No dependency. Changing baseline semantics affects user expectations; update the brief help text and docs with the open-session contract. Coordinate persistence generations with the existing settings writer; do not add a second independent writer.

## Definition of done

Revert restores the opening settings after autosave, updates disk and live consumers safely, and is disabled only when there is no difference from that baseline or an explicitly explained operation prevents it.

Status: Planned. Priority: P2. Finding: [UX-10](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
