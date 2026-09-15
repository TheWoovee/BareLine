# Contributing

Core changes use MPL-2.0; SDK/protocol changes use MIT OR Apache-2.0. Sign contributions with `git commit -s` (Developer Certificate of Origin).

Read the active PR brief, not the full blueprint. Follow AGENTS.md and keep focused evidence in docs/implementation. Preserve source headers and document any dependency addition against ADR-09. No browser or async runtime in the editor process. UI work must match the current mocks and remain event-driven.

Install the Rust 1.98.1 toolchain recorded in rust-toolchain.toml; it is the
compiler supported for the whole workspace. All members inherit this policy.
Run python .github/workflows/check_toolchain_contract.py after editing manifests
or workflow toolchain declarations. A lower Rust version is aspirational until
its locked dependency graph and supported targets pass a dedicated qualification.

Before first frame, only CLI, settings, keymap, session header, window and renderer initialization are permitted. Report measurements without treating performance targets as merge gates.

Use a targeted test or incremental compile appropriate to the change. Broaden verification when boundaries or failures justify it. Never present incomplete slices as feature completion.
