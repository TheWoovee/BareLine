# PR-U03 — Native popup menu theme

## Scope

- Audit finding: UX-05
- Baseline snapshot: `8d8a17afa739b235a0bc634e8aa461a8212f87e8`
- Branch: `codex/audit-20260912-u03`
- Dependencies: integrated U01/U07/U08 modal and routing fixes; existing native `MenuBar` owner-draw implementation

## Intended change

Extend the existing Win32 owner-draw menu implementation recursively through popup submenus. Preserve command identifiers, shortcuts, native keyboard navigation, and accessibility metadata. Map enabled, disabled, selected, checked/radio, separator, shortcut, and submenu-arrow states to the resolved theme; defer to system rendering in high contrast. Refresh DPI/theme-owned GDI resources safely and restore every modified menu item before teardown.

## Evidence

Implemented recursive popup enumeration with per-item original type/data ownership, per-menu original background ownership, stable boxed `MSAAMENUINFO`, themed popup layout/state drawing, high-contrast system colors, DPI/font refresh, and transactional subtree fallback. Menu rebuilds now tear down old item data before destroying the old `HMENU`, then attach fresh metadata while preserving the resolved theme. Dynamic command/submenu labels and radio state update both Win32 and the retained accessibility text.

Local contained checks:

- `rustfmt --edition 2024 crates/platform-windows/src/menu_bar.rs crates/platform-windows/src/native.rs` — passed.
- `git diff --check` — passed (Git reports the repository's expected LF-to-CRLF checkout warning).
- Added a Windows unit test that constructs and restores a synthetic three-level `HMENU`, plus label/shortcut column coverage. Per the audit coordinator's serialization rule, these tests were not run here.
- Review correction coverage exercises nested popup mnemonics (unique execute, duplicate selection/cycling, disabled exclusion, and escaped `&&`), native-fallback type projection, and restoration after dynamic label/radio updates. Brush replacement now commits only after every live menu accepts the new brush; partial failure rolls back while retaining any brush that a live menu may still reference.
- Root titles without explicit `&` markers retain their established Alt+first-letter access keys; failed partial item rollback retains only records still referenced by a live menu.

Root-owned proposed checks after review/integration:

- `cargo test -p bareline-platform-windows menu_bar::tests`
- `cargo clippy -p bareline-platform-windows --all-targets -- -D warnings`
- `cargo build -p bareline`
- Native menu journey: File/Search nested popup interiors and enabled/disabled/selected/checked/radio/separator/shortcut/submenu states in light, dark, system, and Windows high contrast at 100%, 150%, and 200% DPI; verify arrows, access keys, accelerators, mouse hover, submenu expansion, UIA/MSAA names, structural rebuild, and GDI handle stability.

Pending: all Cargo/build/Clippy and native desktop/DPI/accessibility checks above.

Root-owned compile evidence: the first contained platform-module compile stopped before tests because the pinned Windows bindings require the subclass APIs from `UI::Shell`, and Rust inferred signed literals in the muted-color blend. Both source issues were corrected. The rerun passed all three actual-HMENU module/fallback tests and the contained shell `cargo check`; test-only `EnableMenuItem` return-value warnings were then acknowledged without changing behavior. Evidence: `evidence/U03-native-menu-contract-central.log` in the original checkout.
