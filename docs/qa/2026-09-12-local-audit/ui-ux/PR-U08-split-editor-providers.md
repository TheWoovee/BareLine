# PR-U08 — Expose one stable editor provider per split pane

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

Two visible split panes expose pane-qualified tabs but only one generic `Editor` text provider. A screen-reader user cannot identify which pane owns the text, caret, selection, or focus.

## Scope

- `apps/bareline/src/windows_app/accessibility.rs`
- `apps/bareline/src/windows_app/views.rs`
- `crates/platform-windows/src/accessibility.rs`
- `crates/platform-windows/src/accessibility/text_provider.rs`
- View/session accessibility tests and native split journeys

## Ordered implementation

1. Assign a stable provider identity to each live view tab/pane, separate from shared document identity.
2. Build one editor semantic/text provider per visible pane with pane number, document title/path disambiguation, viewport bounds, caret, selection, and scroll state.
3. Bind text operations to the view's shared document plus its independent selection/viewport state.
4. Move accessibility focus with pointer focus, keyboard pane switching, tab activation, session restore, and compare close.
5. Retire providers on pane/tab close and emit structure/focus changes without rebuilding unaffected providers.
6. Preserve document identity sharing so edits appear in both panes without collapsing both views into one provider.

## State, error, and race handling

- If a pane disappears mid-request, return unavailable and focus the surviving pane.
- Tab moves retain view identity; cloning creates a new view/provider identity.
- Async paged viewport changes update only the owning provider and cannot redirect the other pane's selection.

## Tests and acceptance

- Horizontal and vertical split snapshots contain two uniquely named editor providers and exactly one focused provider.
- Editing shared content updates both providers while carets/selections remain independent.
- Move/clone/close/session-restore tests retain stable identities and correct focus.
- Repeat with resident and paged documents and invoke TextPattern range/selection operations in each pane.

## Dependencies and risks

Depends on PR-U01. Provider lifetime must avoid stale UIA handles and excess structure events during caret movement.

## Definition of done

Every visible pane is independently identifiable and operable through accessibility APIs, with stable view identity and correct shared-document behavior.

Status: Planned. Priority: P2. Finding: [UX-03](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
