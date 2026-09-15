# PR-U08 — Stable editor providers for split panes

## Baseline

- Base commit: `7a17953c07961d2d8ea03f9933ae0845b2657581`
- Worktree: `audit-implementation-worktrees/modal`
- Dependencies: integrated PR-U01 modal ownership and PR-U07 Search/Recovery ownership.

## Scope

Expose one stable, independently focused accessibility editor provider for each visible split view while retaining shared resident or paged document bytes. Cover provider retirement and view identity through move, clone, close, session restore, and both split orientations. Dock composition remains assigned to PR-U11.

## Decisions

- Use each persisted view-tab id for the stable editor node/provider id. Moving a tab therefore retains the provider; cloning receives the controller's new tab id; session restore reconstructs the same ids. The view model rejects tab ids outside the checked 44-bit provider-block contract, so provider and bounded-run ids cannot alias.
- Publish pane text as bounded `AccessibilityTextView` records. Each record carries its own selection, visible geometry, composition and provider-qualified source identity while its reader delegates to the shared resident or paged document.
- Qualify each stable provider with a shell-wide generation derived from the full document source identity. Viewport-only movement retains the generation; document edits, replacements and session restoration retire stale readers and actions.
- Key the Windows TextPattern override and every retained COM range by editor owner id. Removing one owner retires only that provider; source/context mismatches return element unavailable. Legacy virtual editor context remains available when its bounded viewport run is empty.
- Keep the existing single-text snapshot fields for unsplit editors and U01/U07 fields. Split records use a separate `0x4...` owner namespace, leaving modal and Search namespaces unchanged.

## Changed files

- `apps/bareline/src/windows_app.rs`
- `apps/bareline/src/windows_app/accessibility.rs`
- `apps/bareline/src/windows_app/views.rs`
- `crates/app/src/accessibility.rs`
- `crates/platform/src/accessibility.rs`
- `crates/platform-windows/src/accessibility.rs`
- `crates/platform-windows/src/accessibility/text_provider.rs`
- This implementation note.

## Focused checks

- `cargo test -p bareline --bin bareline split_panes_publish_independent_text_owners_and_retire_closed_provider`
- `cargo test -p bareline folded_large_gap_keeps_independent_pane_source_layout_and_click`
- `cargo test -p bareline pane_source_generation_tracks_full_document_replacement_and_revision`
- `cargo test -p bareline closed_view_retires_its_source_generation_without_recycling_the_survivor`
- `cargo test -p bareline-platform-windows pane_sources_keep_independent_selection_and_retire_by_owner`
- `cargo test -p bareline-platform-windows legacy_virtual_source_remains_available_without_a_bounded_text_run`
- `cargo test -p bareline-app restored_tab_ids_must_fit_the_native_provider_block_contract`
- Existing view-model coverage: `cargo test -p bareline-app views::tests::clones_share_document_identity_with_independent_state_and_last_reference_close_guard`
- Cargo and native desktop qualification are owned by root integration; no Cargo or desktop run was made in this isolated worktree.

## Unresolved qualifications

- Native UIA journeys remain for root after integration: horizontal/vertical split, pointer and keyboard pane focus, provider retirement, restored session, and resident/paged TextPattern operations.
- Dock composition remains assigned to PR-U11.
