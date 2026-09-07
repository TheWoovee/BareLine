# Integrated development checkpoint — 2026-09-08

This checkpoint follows 362075e. It integrates the current owning PR batches; it is not whole-product completion or acceptance. Manual desktop QA remains paused by the owner until development is complete. No remote operation occurred.

## Automated evidence

- Selected 20-package test gate: 263 passed initially, two search failures repaired and the complete search library rerun passed 30/30, yielding 265 passing cases across the selected targets. The unchanged 20 MiB regex fixture and cancellation/deadline cases remain enabled.
- Windows platform library: 35 passed, zero failed, two subprocess entry fixtures ignored in the parent and invoked by their supervising tests. The final gate covers atomic replacement/rollback, exact staged bytes, running-image refusal, no-replace collision preservation, accessibility source identity and renderer recovery.
- Native debug build: `cargo build -p bareline --offline --locked` passed. SHA-256: `3F80343BBE0B55635D3006E1B62E8C6E6CF4818F74C5548545AF7C31FB7373DE`.
- Renderer, recording renderer and diagnostics all-target scoped Clippy passed with warnings denied.
- Portability architecture guard and its negative fixtures passed. Four pure Python performance-contract tests passed; the Windows adapter scripts passed syntax parsing and scoped source review only. No benchmark or desktop adapter was launched.
- Raw local logs remain in ignored `target/integration-batch-02*.log`; they are not portable acceptance artifacts.

## Consequential repairs

The shared Windows rename boundary now resolves the retained no-follow parent handle to its canonical path, keeps both handles alive, and performs one `FileRenameInfo` operation with explicit replacement policy. Both callers use the same aligned variable-length buffer with a structure-size minimum and zero UTF-16 tail. The previous single-character buffer was too short; the relative-root extended operation also failed on this host. Existing collision and rollback assertions were preserved.

Other reviewed repairs include atomic macro-library loading, captured process output working directories, search replay query/document identity checks, transactional rename budgets, accessibility queued-action source identity and fail-closed snapshot/source pairing, Recovery Center lifecycle-event routing, and SaveAll cancellation state reset.

## Remaining development and acceptance

PR011 and PR025 retain implementation-complete/pending-acceptance status. No additional PR is promoted solely because this integration gate passes. PR001 still needs its final consumer/evidence audit; PR022 foreign-host compile/test and native readiness acceptance remain unexecuted. PR004 Center compare orchestration and keyboard actions, PR014 workspace interpolation and acknowledged search/power command consumers, PR016 provider/manager completion, and the other owning PR scopes remain open.

The native build has six warning groups: test-only SessionTab import, portable shell setting consumer, macro accessibility/command receipts, power accessibility nodes/actions, and the unused view accessor. They describe remaining integration work and are not suppressed.

The two disconnected shell-integration source files are excluded from this commit and retained locally: `apps/bareline/src/windows_app/shell_integration.rs` and `crates/platform-windows/src/shell_integration.rs`. Workflow/benchmark sources are checkpointed as source only; the native build does not establish their runtime acceptance. No signed packaging, foreign CI, physical input, assistive-technology or visual acceptance is claimed.
