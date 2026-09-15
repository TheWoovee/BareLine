# PR-U09 — Bind Find results to the active document and revision

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

After a paged search found 3,097 `needle` matches, switching to a resident document left the old query/count visible. Changing the query to `alpha` did not replace the stale `3,097 matches`, and Replace/Replace All remained unavailable. Users can navigate or reason from results that belong to another document.

## Scope

- `crates/app/src/find.rs`
- `crates/app/src/workspace.rs` search start/pump/navigation
- `apps/bareline/src/windows_app/views.rs::select_tab` and every active-tab transition
- `apps/bareline/src/windows_app/search.rs` / `search/replace.rs`

## Ordered implementation

1. Define a find session key from document identity, content state/revision, query, options, and scope.
2. Store that key with pending and completed resident/paged results; expose results/status/actions only when it matches the active editor.
3. On every tab/pane activation, either restore that view/document's cached matching session or immediately mark the shared controller pending/idle and start the current query against the new editor.
4. Cancel or detach the prior document's worker without allowing its late completion to overwrite the active status.
5. Gate navigation and replacement on the same key, with a visible `Results changed; search again` state for revision drift.
6. Preserve the query text across tabs, but never preserve another document's count, selection, or enabled actions.

## State, error, and race handling

- `views::select_tab` currently installs the new view and changes `app.active` (`apps/bareline/src/windows_app/views.rs:1475-1498`) without synchronizing the workspace-owned `FindController`.
- `FindController` holds one shared pending/result/status set (`crates/app/src/find.rs:28-50`); make publication compare the complete key before mutating visible state.
- Closing, cloning, or moving views must key cache lifetime by document identity and bound retained sessions.

## Tests and acceptance

- Search document A, switch to B with zero matches, and assert A's count/actions disappear before any B worker result.
- Rapid A→B→A with delayed completions never publishes a result under the wrong tab.
- Repeat resident→resident, paged→resident, resident→paged, split panes, and after an edit changes revision.
- Replace All can enable only from a complete result matching active identity/revision/query/options.

## Dependencies and risks

Use PR-U07 semantics for truthful status announcements. Preserve bounded worker/cache ownership and avoid restarting on focus-only changes within the same document.

## Definition of done

Every displayed count, match, navigation target, and replacement action is provably tied to the active document state.

Status: Planned. Priority: P1. Finding: [UX-09](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
