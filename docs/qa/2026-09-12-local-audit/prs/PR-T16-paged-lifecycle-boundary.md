# PR-T16 — Separate paged presentation from persistent document lifecycle

Priority P3 architecture. Fixes TECH-019. Depends on PR-T03/T04/T05/T07 so this refactor does not duplicate or erase their behavioral fixes. Scope is a maintainability boundary improvement, not a newly reproduced data-loss defect. Primary files: `crates/editor-surface/src/paged_view.rs`, its paged transfer/spill/view modules, `crates/file-io/src/paged_service.rs`, and `crates/app/src/workspace.rs`.

## Current boundary

`PagedEditorSurface` directly holds `Arc<Mutex<Box<PagedOpened>>>`, save identity, path/fingerprint, recovery actors and stringly typed pending results (`paged_view.rs:424–449`). The recent `paged_service` extraction moved serialization policy, but the presentation type still coordinates persistent lifecycle. This makes changes to UI, saving, peer views and recovery share a large mutation surface. Do not reopen the old removed `WorkspaceEditor::Deref` defect; it is not present in the current API.

## Implementation sequence

1. Define a file-io-owned paged session/service with a stable document identity, immutable snapshot/read handle, typed lifecycle commands and terminal receipts. It owns path/fingerprint, save state, encoding/provenance, recovery retirement and background file-operation ownership. No renderer or Windows types may enter this service.
2. Keep viewport, selection, folds, layout caches, hover, scroll, peer presentation and input projection in editor-surface. The surface references the lifecycle handle and applies receipts after checking identity/revision/generation; it must not mutate persistence internals through a public actor lock.
3. Migrate one lifecycle at a time: save and Save Copy, then reload/encoding, then recovery and spill retirement. Provide temporary thin adapters so each step compiles and retains behavior. Keep source transactions and linked cross-document undo atomic across the boundary.
4. Replace internal `Result<_,String>` conversions at this boundary with typed error/receipt variants for Busy, Changed, Cancelled, Encoding, Conflict, SourceUnavailable and CleanupPending. Convert to user text at the shell; never branch on error strings.
5. Hide public `surface`, path/fingerprint mutation where possible behind narrowly named APIs. Record the permitted owners of saved state and generation retirement so secondary views cannot diverge from their document.

## Verification

Run the existing paged edit/undo/restart, cross-document move, save-copy identity, reload policy, source-generation, opaque-byte and failed-retirement tests at each migration step. Add a contract test using a headless lifecycle client without a renderer, and a view test with a controlled lifecycle service. Do not add tests that merely assert struct shape. Compile neutral layers on supported CI hosts and the actual Windows app.

Acceptance: one persistence owner, typed terminal outcomes, no platform leakage, unchanged exact-byte/recovery/undo guarantees, and peer views converge on the same document state. Avoid a wholesale rewrite or a dependency cycle. Any behavior change discovered during refactoring needs its own explicit reproduction and review note.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
