# PR implementation index — local audit

**31 planned work packages: 18 technical and 13 UI/UX.** Read [the implementor contract](IMPLEMENTOR_CONTRACT.md) before starting. Preserve the current uncommitted baseline. These are local plans, not published PRs.

Every brief identifies the current failure or gap, code ownership, ordered changes, state/error rules, dependencies and acceptance. Findings retain their evidence strength; a design or qualification gap is not automatically a reproduced runtime defect.

## Technical work packages

| PR | Implementation brief |
|---|---|
| PR-T01 | [PR-T01 — Confine startup cache deletion to proven owned directories](prs/PR-T01-owned-cache-cleanup.md) |
| PR-T02 | [PR-T02 — Make profile migration resumable and lossless](prs/PR-T02-resumable-data-migration.md) |
| PR-T03 | [PR-T03 — Carry approved overwrite identity through Save As and Save Copy](prs/PR-T03-save-destination-consent.md) |
| PR-T04 | [PR-T04 — Preserve displaced bytes across the save commit race](prs/PR-T04-preserve-concurrent-writes.md) |
| PR-T05 | [PR-T05 — Make discard cleanup asynchronous and truthful](prs/PR-T05-async-recovery-retirement.md) |
| PR-T06 | [PR-T06 — Give background tasks reliable cancellation and terminal outcomes](prs/PR-T06-task-terminal-states.md) |
| PR-T07 | [PR-T07 — Schedule queued work on available workers](prs/PR-T07-shared-worker-scheduling.md) |
| PR-T08 | [PR-T08 — Bound extension operations and handle spawn failure](prs/PR-T08-fallible-extension-workers.md) |
| PR-T09 | [PR-T09 — Fix flaky synchronization checks and preserve trustworthy run results](prs/PR-T09-reliable-test-evidence.md) |
| PR-T10 | [PR-T10 — Make real first-party component execution part of qualification](prs/PR-T10-first-party-runtime-qualification.md) |
| PR-T11 | [PR-T11 — Produce a configured, verifiable release feature set](prs/PR-T11-release-feature-configuration.md) |
| PR-T12 | [PR-T12 — Make Rust support metadata match tested toolchains](prs/PR-T12-toolchain-contract.md) |
| PR-T13 | [PR-T13 — Prevent new lint debt without hiding existing debt globally](prs/PR-T13-lint-debt-ratchet.md) |
| PR-T14 | [PR-T14 — Reconcile current capabilities, limits and acceptance evidence](prs/PR-T14-current-capability-ledger.md) |
| PR-T15 | [PR-T15 — Qualify the product's speed and resource claims](prs/PR-T15-performance-qualification.md) |
| PR-T16 | [PR-T16 — Separate paged presentation from persistent document lifecycle](prs/PR-T16-paged-lifecycle-boundary.md) |
| PR-T17 | [PR-T17 — Keep informational and isolated launch modes free of unrelated mutations](prs/PR-T17-side-effect-free-launch-parsing.md) |
| PR-T18 | [PR-T18 — Retire failed and canceled launch requests](prs/PR-T18-launch-request-lifecycle.md) |

## UI/UX work packages

Save As fixes are owned by **PR-T03**; native test investigations are owned by **PR-T09**. UI findings reference those to prevent duplicate implementations.

| PR | Priority | Implementation brief |
|---|---|---|
| PR-U01 | P1 | [PR-U01 — Establish modal focus, input ownership, and dismissal](ui-ux/PR-U01-overlay-and-modal-accessibility.md) |
| PR-U02 | P2 | [PR-U02 — Route status, progress, warnings, and errors through one presentation path](ui-ux/PR-U02-notification-routing.md) |
| PR-U03 | P2 | [PR-U03 — Apply the resolved theme to native popup menus](ui-ux/PR-U03-native-popup-theme.md) |
| PR-U04 | P2 | [PR-U04 — Make File and Search menus fit, reflect state, and expose mode selection](ui-ux/PR-U04-menu-information-architecture.md) |
| PR-U05 | P2 | [PR-U05 — Give Recovery Center truthful loading, empty, and live-refresh states](ui-ux/PR-U05-recovery-center-loading-refresh.md) |
| PR-U06 | P3 | [PR-U06 — Keep Macro Manager Close usable in every manager state](ui-ux/PR-U06-macro-manager-close-state.md) |
| PR-U07 | P1 | [PR-U07 — Complete Search and Recovery Center semantics](ui-ux/PR-U07-search-and-recovery-focus.md) |
| PR-U08 | P2 | [PR-U08 — Expose one stable editor provider per split pane](ui-ux/PR-U08-split-editor-providers.md) |
| PR-U09 | P1 | [PR-U09 — Bind Find results to the active document and revision](ui-ux/PR-U09-document-bound-find-state.md) |
| PR-U10 | P2 | [PR-U10 — Make Settings Revert an honest undo action](ui-ux/PR-U10-settings-revert-contract.md) |
| PR-U11 | P2 design gap | [PR-U11 — Put Search, Compare, and Output in one shared bottom dock](ui-ux/PR-U11-shared-bottom-dock.md) |
| PR-U12 | P3 design gap | [PR-U12 — Explain Find and Replace icon toggles](ui-ux/PR-U12-find-toggle-tooltips.md) |
| PR-U13 | P3 design gap | [PR-U13 — Distinguish estimated line numbers while large files index](ui-ux/PR-U13-estimated-gutter-cue.md) |
