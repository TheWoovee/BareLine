# PR-U07 — Search and Recovery accessibility focus

## Baseline

- Base commit: `97fcf41ce355874358129453cf2a2f77d2ee0095`
- Worktree: `audit-implementation-worktrees/modal`

## Scope

Complete Search and Recovery Center semantics, text-provider ownership, focus movement, and modal/background isolation on top of PR-U01. Split-view providers remain assigned to PR-U08, and shared dock redesign remains assigned to PR-U11.

## Decisions

- Reuse PR-U01's modal descriptor for Recovery Center. Its semantic group is the active layer, so editor, tabs, and unrelated chrome become inert while recovery owns focus.
- Publish the visually focused Find, Replace, open-document, or folder field through the native text provider. Search provider identities use a namespace that editor selection rejects plus a non-recycling revision encoded with the field owner.
- Treat Find, Replace, and Files as one semantic mode group. Files remains the selected tab while the bottom search panel is visible; pointer and accessibility invocation transfer the current query into the requested panel.
- Keep asynchronous search/recovery publication data-only after close. Search row rebuilds clamp to the nearest surviving row; Recovery discovery may update the status but cannot reopen a dismissed Center or displace another modal.
- Folder and paged sources do not retain a full line index in their bounded result receipts. Their row names state `line unavailable`, retain the exact path/document and byte offset, and include the bounded excerpt rather than synthesizing a line number.

## Changed files

- `apps/bareline/src/windows_app/modal.rs`: Recovery modal descriptor and shared input/focus dispatch.
- `apps/bareline/src/windows_app/recovery.rs`: inert modal lifecycle, enabled-only focus traversal, late-open guard, nearest-row regression fixtures.
- `apps/bareline/src/windows_app/accessibility.rs`: search field provider ownership, stale-selection rejection, tab grouping/actions, Recovery active-layer checks.
- `apps/bareline/src/windows_app.rs`, `apps/bareline/src/windows_app/search.rs`, and `search/folder.rs`: tab routing and folder provider revision ownership.
- `crates/app/src/find.rs` and `crates/app/src/search_panel.rs`: focused-field ownership, mode semantics, stable result names, focus/selection state, and provider revisions.
- `crates/platform/src/accessibility.rs` and `crates/platform-windows/src/accessibility.rs`: neutral `TabList` role and native UIA Tab container mapping.

## Focused checks

- Source-only checks completed in the isolated worktree: `rustfmt --edition 2024` on the changed Rust files and `git diff --check`.
- Per integration policy, no worker Cargo or native desktop run was performed. Root can run the contained regressions after applying the commit:
  - `cargo test -p bareline-app --offline --locked search_panel::tests::accessibility_tabs_select_modes_and_provider_revisions_do_not_recycle`
  - `cargo test -p bareline-app --offline --locked search_panel::tests::grouped_panel_draws_only_visible_rows_and_activates_exact_source`
  - `cargo test -p bareline --offline --locked --features perf-spans,qa-inventory --bin bareline windows_app::accessibility::tests::search_provider_tracks_field_selection_modes_and_close_without_editing_document`
  - `cargo test -p bareline --offline --locked --features perf-spans,qa-inventory --bin bareline windows_app::accessibility::tests::recovery_modal_hides_background_text_and_restores_the_find_invoker`
  - `cargo test -p bareline --offline --locked --features perf-spans,qa-inventory --bin bareline windows_app::recovery::tests::removed_rows_select_the_nearest_survivor_and_closed_discovery_stays_closed`
  - `cargo test -p bareline-platform-windows --offline --locked accessibility::tests::provider_tree_maps_tab_list_and_selected_tab`
- Search semantic/provider changes require a reviewed native-semantic golden refresh in the integration checkout. Capture to a candidate with `BARELINE_CAPTURE_ACCESSIBILITY_GOLDEN`, inspect the Search-only delta, install it in `tests/a11y/native-semantic.json`, then rerun `cargo test -p bareline --offline --locked --features perf-spans,qa-inventory --bin bareline windows_app::accessibility::tests::complete_native_semantic_json_golden`.

## Unresolved qualifications

- Native desktop and integrated Cargo qualification are owned by the root integration pass.
- Folder and paged search receipts expose exact byte ranges but no retained full line index. The accessible name reports that the line is unavailable; adding bounded line metadata at worker publication belongs with the later search-result binding work rather than scanning source data on the UI thread.
