# PR-U06 — Keep Macro Manager Close usable in every manager state

Baseline: `96f3566661cb1f397a89106f7aef00927793c0c0`

## Scope

Make the visible Macro Manager Close control enabled whenever the manager is open, install fresh command state before its first draw and semantic snapshot, and route pointer/UIA Invoke and Escape through one dismissal effect that restores editor focus. Recording, playback, process, and storage ownership remain unchanged.

## Decisions

- Manager open/close transitions rebuild command context immediately instead of waiting for the later macro pump.
- Close applicability depends only on whether the manager is open.
- All close inputs use the existing manager close path; dismissing the surface does not cancel recording or playback.

## Evidence

Focused state-table coverage includes empty, selected, loading, failed loading, recording, and playback states. A manager regression injects the stale closed-manager command state, verifies the rendered/semantic Close remains enabled with UIA Invoke, and proves UIA and Escape return the same native-owner dismissal effect.

Changed files:

- `apps/bareline/src/windows_app/macros.rs`: immediate open/close command-context refresh and one dismissal/focus-restoration path.
- `crates/app/src/macros/manager.rs`: unconditional open-manager Close state and shared dismissal effect for pointer, UIA, and Escape.

Worker-side checks are `rustfmt --edition 2024` on both Rust files and `git diff --check`. Root owns contained Cargo and native UIA/mouse/Escape qualification in the integration checkout.

Final release-warning cleanup gates `MacrosRuntime::height` to tests; exhaustive reference inspection found its sole caller in the test-only accessibility snapshot helper. Production dock layout continues to use the existing explicit bounds path.
