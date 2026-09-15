# PR-U11 — Put Search, Compare, and Output in one shared bottom dock

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

Output reserves a bottom band and search results use a different full-width layout, while Compare has its required main two-editor composition plus separate controls/status. Switching tasks lacks a consistent panel history, focus, resize, or close model. This is a retained product/design gap recorded by the implementation status, not a reproduced runtime failure.

## Scope

- `apps/bareline/src/windows_app.rs`
- `windows_app/search*`, `windows_app/compare.rs`, `windows_app/macros.rs`
- `windows_app/workspace_panels.rs`
- dock size/session settings and semantics

## Ordered implementation

1. Before composition, inspect `docs/blueprint/mockups/bareline-compare-dark.png`, `bareline-compare-merge.png`, `bareline-compare-options.png`, `bareline-search-panel.png`, `bareline-main-editor-dark.png`, and `docs/blueprint/11_UI_INTERACTION_SPEC.md`, especially the compare requirement that each current hunk exposes both copy directions.
2. Add a bottom-dock controller with stable tabs for Search, Compare, and Output, one active tab, collapsed state, and persisted bounded height.
3. Place Search results, Compare results/controls/status, and Output in that dock as the approved mockup specifies. Preserve Compare's two editors in the main editor area; do not squeeze them into the bottom panel.
4. Share one splitter, focus traversal, close/collapse commands, unread/progress badges, and keyboard route.
5. Preserve editor viewport when opening/switching tabs and stack safely at narrow/high-DPI sizes.
6. Persist active tab/height in session state and restore only panels whose underlying operation/state remains valid.

## Tests and acceptance

- Open all three surfaces in sequence; one bottom region appears, tabs switch without duplicate content, and editor geometry changes only with dock height.
- Resize, collapse/reopen, session restore, async completion, and document close retain valid focus and bounded layout.
- UIA exposes a tab list and one selected panel with complete child semantics.

## Dependencies and risks

Land after PR-U01/U07/U08 so the dock uses stable focus and provider ownership. Preserve Compare's main two-editor view and Output's persistent process results.

## Definition of done

Search, Compare, and Output share one predictable, resizable, accessible bottom-dock model without losing existing operations or state.

Status: Planned. Priority: P2 design gap. Finding: [UX-11](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
