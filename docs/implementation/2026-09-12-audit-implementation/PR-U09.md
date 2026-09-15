# PR-U09 — Document-bound Find state

## Baseline

- Base commit: `ba572832716547fc0df7493003e6c7b9ab9f7e58`
- Dependency: corrected PR-U08 split-pane accessibility provider ownership and integrated PR-U07 Search/Recovery ownership.
- Worktree: `audit-implementation-worktrees/modal`.

## Scope

Bind Find query/options, results, pending navigation and replacement to the exact document source, revision and request generation for resident and paged editors. Stale results cannot affect a different document or a newer revision. Shared dock composition remains assigned to PR-U11.

## Decisions

- Define each request session from the resident or paged full document identity, content state/revision, complete query/options/scope, and a non-recycling controller generation.
- Clear visible results and disable replacement synchronously when an active document, revision, query, option, or scope no longer matches. Late worker results compare their captured session and generation before changing status or results.
- Bind Find on ordinary tab changes, split-pane activation, clone/move/close/collapse, session restore, search-result activation, drawing after revision changes, navigation, and replacement. Switching source also detaches pending navigation/replacement tickets so completion cannot target another active document.
- Preserve query text across document changes and start the matching resident or paged request from the next active-view preparation. Shared dock layout remains assigned to PR-U11.
- Treat selection scope as source-owned byte offsets: clear it on a document change, retain it across revisions of the same document, and capture it from the actual active split view. Visiting another document also retires a prior cancellation so returning starts a fresh request with truthful status.
- Route source clearing through the workspace so navigation and replacement tickets retire together, and qualify disconnected-worker status by the captured session and generation.

## Changed files

- `crates/app/src/find.rs`
- `crates/app/src/workspace.rs`
- `apps/bareline/src/windows_app/accessibility.rs`
- `apps/bareline/src/windows_app/search.rs`
- `apps/bareline/src/windows_app/views.rs`
- `apps/bareline/src/windows_app/watch.rs`
- This implementation note.

## Focused checks

- `cargo test -p bareline-app find_results_and_replace_actions_follow_the_bound_document_generation`
- `cargo test -p bareline split_pane_activation_retires_the_previous_documents_find_results`
- `cargo test -p bareline folded_large_gap_keeps_independent_pane_source_layout_and_click`
- `cargo test -p bareline-app document_switch_clears_byte_scope_and_restarts_a_cancelled_query`
- `cargo test -p bareline-app stale_worker_disconnect_cannot_replace_the_current_document_status`
- `cargo test -p bareline find_selection_scope_uses_the_active_split_view`
- Existing navigation/replace regressions: `cargo test -p bareline-app find_is_worker_backed_and_navigation_preserves_document_revision` and `cargo test -p bareline-app replace_all_is_one_undo_and_cancelled_preparation_cannot_mutate`
- Cargo and native desktop qualification are owned by root integration; no Cargo or desktop run was made in this isolated worktree.

## Unresolved qualifications

- Native resident↔paged and rapid pane-switch journeys remain for final integrated qualification.
- Shared Find/Search dock composition remains assigned to PR-U11.
