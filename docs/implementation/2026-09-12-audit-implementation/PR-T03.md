# PR-T03 — Save destination consent

Baseline: `3c7f4a1681d676b38b49192b594578b914222212`

## Scope

Carry a typed, captured destination condition through Save, Save As, Save Copy, Save All, resident/encoded I/O, and paged saves. Separate path selection from overwrite authority, reject copy aliases by filesystem identity, and give save dialogs document-derived defaults without an unsafe working-directory fallback.

## Decisions

- An approved target carries either `MustBeAbsent` or `ReplaceCaptured(Fingerprint)`; writers consume that exact condition and never refresh it after consent.
- Destination preparation is asynchronous in the application shell. The shell owns the single overwrite confirmation and stale-document/operation validation.
- Save As changes document identity only on a successful save receipt. Save Copy does not change document state.
- PR-T04 may replace the low-level commit mechanism while retaining this prepared destination contract.

## Changed files

- `crates/file-io/src/lifecycle.rs`: typed destination/preflight contract, exact condition consumption, cancellation tests.
- `crates/file-io/src/fault_transitions.rs`: existing save fault seam now supplies the typed captured-replacement condition.
- `crates/file-io/src/paged_service.rs`: typed paged destination condition.
- `crates/editor-surface/src/paged_view.rs`: prepared paged Save/Copy and actor-side full-document identity validation.
- `crates/app/src/workspace.rs`: full-document identity API, consent validation, prepared resident/encoded/paged request wiring and regressions.
- `crates/platform/src/lib.rs`: document-save picker with name/directory defaults, separate from overwrite authority.
- `crates/platform-windows/src/native.rs`: prepared picker suppresses only its native overwrite prompt; known Documents fallback and one captured-overwrite task dialog.
- `apps/bareline/src/windows_app.rs`, `apps/bareline/src/windows_app/lifecycle.rs`: asynchronous bounded/cancellable preflight, stale operation/revision rejection, Save/Save As/Copy/All routing and close/exit coordination.

## Evidence

- `rustfmt --edition 2024` on every changed Rust file: passed.
- `git diff --check`: passed (Git reports only the repository's expected LF/CRLF conversion warnings).
- A preliminary isolated `cargo check -p bareline --offline --locked --target-dir D:/Notepad_REq/Bareline_Product_Blueprint/Bareline-Editor/target` passed before the final review corrections. Shared artifacts were later found to mix worktree interfaces, so this is recorded as preliminary rather than final evidence.
- Added contained regressions for accepted/declined overwrite capture, a target changed after consent, cancellation during a gated fingerprint, UTF-16 prepared overwrite, resident/paged prepared copy semantics, paged full-document versus viewport identity, stale content revision rejection, and resident/paged accepted overwrite.
- Added shell regressions for forced-paged close/preflight identity after viewport movement and synchronous Save All rejection advancing to the next entry.

## Qualifications

Per root coordination, the final contained Cargo test/check cohort runs only after integration; it was not run from this isolated worktree. Native verification remains pending for the coordinated final qualification: one overwrite prompt, accepted/declined paths, Save All cancellation, close during preflight, dialog defaults outside System32, read-only destination, and UTF-8/UTF-16/paged byte/state checks.
