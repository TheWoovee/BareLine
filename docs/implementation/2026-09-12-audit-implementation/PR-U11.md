# PR-U11 — Shared bottom dock ownership

## Baseline

- Base commit: `4f46208bbafffd5a3195a4a4f48eb6f2813f2ee1`.
- Dependency: PR-U12 Find option tooltips, PR-U09 document-bound Find sessions, PR-U08 split providers, and PR-U07 Search accessibility ownership.
- Worktree: `audit-implementation-worktrees/modal`.

## Scope

Unify Search, Compare controls, and Output under one bottom dock with one selected surface, height, splitter, focus owner, and session representation. Compare editor panes remain in the main editor area and both copy directions remain available.

## Decisions

- `DockRuntime` is the sole owner of the selected Search, Compare, or Output surface, collapsed state, bounded height, splitter drag, tab focus, and busy/unread badges.
- Search keeps its Find/Replace/Files semantics and document-bound state but paints inside the dock body. Compare retains both editor panes in the main view and puts source, navigation, status, options, and both hunk copy directions in the dock. Output retains its process rows while using the same body.
- All three tabs have stable semantic identities. Unavailable tabs remain discoverable and disabled; only the selected surface contributes child semantics or text ownership.
- Session layout stores the active panel, collapsed state, and finite bounded logical height. Restore asks the dock to reconcile that preference with the surfaces whose underlying state actually survived.
- Dock header focus, selected-surface control focus, and editor focus are distinct owners. Existing Search and Output user commands request and reveal their retained surface, and hidden completion is marked unread.
- Search owns and clips its exact supplied body, Output translates window input once into its painted editor-local body, and Compare uses a compact three-row control layout when height or width is constrained.
- Search translates text hit testing with the same nonzero dock origin used for paint and semantics. Every raw Search, Compare, and Output body click transfers focus from the dock header to the clicked child; Shift+Tab from the header returns to the editor.
- Output exposes a stable focusable body when it has no selected visible row. A retained selected row can own accessibility focus only while Output owns keyboard input, so editor and dock-header focus remain authoritative.

## Changed files

- `apps/bareline/src/windows_app.rs`
- `apps/bareline/src/windows_app/{accessibility,compare,dock,macros,session,views}.rs`
- `crates/app/src/{macros,search_panel,workspace}.rs`
- `crates/file-io/src/session/mod.rs`
- This implementation note.

## Focused checks

- `cargo test -p bareline windows_app::dock::tests -- --nocapture`
- `cargo test -p bareline-app search_panel::tests::external_dock_bounds_own_search_hit_and_semantic_geometry -- --exact`
- `cargo test -p bareline-file-io session::tests::bottom_panel_layout_round_trips_and_rejects_unbounded_state -- --exact`
- `cargo test -p bareline windows_app::accessibility::tests::bottom_dock_exposes_one_selected_surface_and_retires_background_search_owner -- --exact`
- `cargo test -p bareline windows_app::accessibility::tests::compare_dock_exposes_both_current_hunk_copy_directions -- --exact`
- `cargo test -p bareline windows_app::accessibility::tests::shared_body_focus_transfer_retires_header_and_shift_tab_returns_to_editor -- --exact`
- `cargo test -p bareline windows_app::compare::tests::compact_dock_keeps_merge_directions_and_close_inside_body -- --exact`
- `cargo test -p bareline windows_app::macros::tests::dock_output_keeps_window_hit_bounds_and_local_controller_coordinates -- --exact`

Commands are recorded for the root integration checkout. This worker did not run Cargo or native desktop control.

## Unresolved qualifications

- Root-owned contained compilation and the combined U07/U08/U11 semantic golden review remain pending.
- Native high-DPI splitter, focus traversal, process-output completion, and compare-copy qualification remain pending on the integrated build.
