# PR-T17 — Side-effect-free launch parsing

- Issue: TECH-020
- Baseline source: `3c7f4a1681d676b38b49192b594578b914222212`
- Audit `cli-side-effects` reproduction binary: SHA-256 `e3f283c8cc61ccae5d1549a1136b899d02abf3cf95a27c9fc7be8a42d7128702`
- Dependencies: none; this change precedes PR-T01 and PR-T02.

## Scope

Separate argument validation and launch-mode selection from profile discovery and persistent initialization. Informational and invalid invocations must return before profile/cache work. Portable and performance/diagnostic launches must select their authoritative roots before any installed-profile work. Preserve original-launch-CWD path resolution, supported CLI options, instance handoff, and process hardening.

## Decisions

- `parse` now performs only lossless argument interpretation and validation, returning a typed requested mode plus raw paths/options. It does not inspect the executable directory, profiles, TEMP, or performance fixture paths.
- `prepare` runs only after Help/Version have returned. It validates an explicitly requested performance fixture, captures the original launch CWD, selects Performance/Portable/Diagnostic/Installed ownership, resolves relative document paths, then applies process search hardening.
- A portable marker can select Portable for either an ordinary or diagnostic launch; it never causes installed-profile maintenance. Performance always owns its declared marked root. A non-portable diagnostic requires `--diagnostic-root` with an empty regular `.bareline-diagnostic` marker, and settings/session/recovery/extensions/diagnostics all stay under that root.
- Performance workloads reject every legacy diagnostic execution flag and handle-diagnostic request before root preparation. Portable handle diagnostics retain diagnostic intent and bypass normal single-instance forwarding even though Portable owns their data root.
- Help output documents the diagnostic-root option, its sentinel requirement and the portable-executable alternative.
- Only Installed mode receives a one-shot maintenance payload. The first successfully presented frame schedules a background worker through a retained runtime. Scheduling failures and disconnected workers become retained errors; successful completion retains the local profile root, exact migrated entry names and cleanup count. PR-T02 can take that receipt to reconcile newly migrated settings/session against live revisions without blindly overwriting changes made since first frame. PR-T01/PR-T02 will replace the existing helper internals with their safe/resumable implementations.
- Contradictory renderer and legacy diagnostic modes, Help+Version, missing/unknown diagnostics, and existing malformed performance arguments fail during side-effect-free validation.

## Changed files

- `apps/bareline/src/windows_app/launch.rs`
- `apps/bareline/src/windows_app/performance.rs`
- `apps/bareline/src/windows_app.rs`
- `apps/bareline/src/windows_app/instance.rs`
- `apps/bareline/src/windows_app/accessibility.rs`
- `xtask/src/journey.rs`
- `docs/implementation/2026-09-12-audit-implementation/PR-T17.md`

## Checks

- `rustfmt --edition 2024` on the five modified Rust files: PASS.
- `git diff --check`: PASS (Git reports only the repository's CRLF conversion warning).
- Root-owned after patch integration: `cargo test -p bareline --bin bareline windows_app::launch::tests --no-default-features --offline --locked`. Cargo is intentionally not run from this isolated worktree because mixed worktree interfaces can contaminate the shared incremental artifacts. The crate's default feature set is empty and all T17 paths are unconditional in the Windows binary module, so the focused command will cover the asserted parser/mode logic on the Windows host; it does not replace native subprocess or first-frame verification.
- Pending final native subprocess checks for help/version/invalid, portable, diagnostic/performance, and installed first-frame scheduling.

## Unresolved qualification

- PR-T01/PR-T02 own safe/resumable migration and cleanup internals; PR-T17 will only determine when those installed-profile jobs may run.
- Final native desktop and integrated qualification remain root-owned.
