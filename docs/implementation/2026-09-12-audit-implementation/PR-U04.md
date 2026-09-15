# PR-U04 — Menu information architecture and live state

## Scope

- Audit finding: UX-05
- Baseline: PR-U03 tip `dae79ee41020ec762240c0b809a047aea9430495`, rooted at reviewed snapshot `8d8a17afa739b235a0bc634e8aa461a8212f87e8`
- Branch: `codex/audit-20260912-u03`
- Dependencies: PR-U03 checked/radio native projection; integrated search result/document-state fixes; existing command registry and palette

## Intended change

Shorten the first-level File and Search menus with stable, meaningful submenus while preserving command IDs, shortcuts, and exhaustive command-palette reachability. Recompute real command applicability and search mode/scope/toggle check state in the shared command context immediately before native popup synchronization. Keep cancellation enabled through the cancelling state and require a current reviewed replacement set before enabling apply. Add structural viewport-fit and state-table coverage.

## Evidence

- `rustfmt --edition 2024` completed for the six changed Rust files.
- `git diff --check` passed.
- Added structural coverage for File/Search first-level item counts and conservative 600/768/800 logical-pixel height estimates.
- Added state-table coverage for replacement idle/running/cancelling/complete/stale states and Save All idle/ready/running states. These tests are authored but intentionally not run in this worktree.
- R1 replaces inferred scope state with an explicit current owner that changes on Current Document, Selection, Open Documents, Folder, and panel-tab transitions; cancellation and retained results do not change that intent.
- R1 shares the reviewed-apply predicate between menus, replacement-panel paint/hit state, keyboard/pointer dispatch, and preview consumption. Only selected resident/paged sources participate in the freshness check, and a rejected direct Apply retains the preview unchanged.
- R1 moves the viewport assertion to the fully registered Shell command model so automatically appended commands are included.
- R2 keeps Folder/Open Documents as explicit owners, while Current/Selection projection follows the bound Find query so switching documents immediately retires a document-specific selection scope. Its regression uses the Selection command path and a real `bind_find_to` source switch.
- R3 places the shell-registered profile migration retry inside File ▸ Document and Disk ▸ Recovery. This removes the auto-appended fifteenth File row while preserving its command ID, palette category, and recovery-oriented placement.
- The final inventory correction maps the three explicit search-mode IDs to the Search dispatch route. The route remains exact, so unknown `search.mode.*` IDs still fail inventory validation.

The shell now computes mode/scope/toggle, cancellation, replacement-review, and Save All states from the current runtime snapshot whenever it refreshes native menus, and dispatch rechecks the same shared context. Async search/save transitions already request redraws, so acknowledged state changes trigger another native projection.

Root-owned verification remains pending:

- `cargo test -p bareline-app menus::tests`
- `cargo test -p bareline menu_state_tests`
- `cargo test -p bareline menu_projection_tests`
- `cargo test -p bareline save_all_menu_state_covers_idle_ready_and_running`
- `cargo check -p bareline`
- `cargo clippy -p bareline-app -p bareline --all-targets -- -D warnings`
- Native File/Search popup journey at 600, 768, and 800 logical-pixel heights, plus keyboard checked/radio updates and stale-preview disabled-reason inspection.

Disk replacement applies retain their existing commit-time fingerprint revalidation. The menu can pre-detect stale resident and paged document snapshots; an external disk change that happens after preview remains a commit-time rejection because synchronously rereading every disk target while opening a menu would block the UI.
