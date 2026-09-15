# PR-T16: Paged persistent lifecycle boundary

- **Baseline:** `67ee505ba9fc5e9754b237c6b18ea5fd1916d90a` (reviewed local snapshot; includes T03, T04, T05, and T07)
- **Issue:** TECH-019
- **Scope:** Move paged path/fingerprint, saved-state, encoding/provenance, recovery retirement, and background file-operation ownership into a file-io session with immutable read handles, typed lifecycle commands, and identity/revision/generation-fenced terminal receipts. Keep viewport, folds, selection, peer presentation, layout, and input in editor-surface.
- **Preserved contracts:** Exact source bytes and BOM/encoding provenance; save-conflict displaced/proposed bytes and cleanup warnings; durable discard separate from physical purge; `CleanupHold` shared by every view/read handle; fresh recovery/status generations after resume; atomic cross-document move and linked undo.
- **Coordination:** No renderer or Windows types in file-io, no dependency cycle, and no wholesale rewrite. Cargo, build, native, and shared-target verification remain centrally owned. The accepted U13 nonblocking line-index receipt correction is carried as `3797786..876cb7e` (source commits `d65aae4`, `35334b6`).

## Evidence

- `file-io::paged_service::PagedSession` is the shared owner for the paged actor, path/fingerprint, opaque savepoint, encoding/conflict/cleanup state, recovery configuration/status/discard/cleanup hold, source generations, tail session, spill state, and operation generation. Its inner persistence locks are private; the surface receives a document-only edit guard plus named tail, recovery, retirement, spill and grouped-recovery transitions. `PagedEditorSurface` no longer stores duplicate path, fingerprint, saved-state, save-as, recovery, tail, retired-generation, or spill owners.
- `PagedReadHandle` moved to file-io. It is renderer-free, retains the session and cleanup hold transitively, resolves current and historical source pages, and exposes typed `Busy`/`Changed`/`SourceUnavailable` outcomes before the display adapter renders text.
- Save and Save Copy dispatch `PagedLifecycleCommand::Save`. Acceptance installs an RAII operation reservation before the document guard is released and keeps it through authoritative state and legacy actor-mirror publication, including unwind. A second save receives typed `Busy`; a later document edit makes the initiating view receipt stale without rewriting the committed disk result. A committed save always publishes its accepted path/fingerprint/savepoint and retains every cleanup warning. The accepted savepoint remains the older content state when an edit races the write, so the current document stays dirty. Displaced/proposed-byte artifacts remain owned by the unchanged T04 save primitive.
- Recovery retry and retirement and spill attachment also dispatch concrete typed commands under the same operation reservation and cancellation policy. Recovery retirement atomically verifies the full saved receipt and clean content state while the document actor is locked before detaching the matching journal; a newer edit therefore keeps its recovery source and restarts from that edit. A Retry receipt is checked again under the re-acquired document guard before its rebuild result changes view work. Page resolution returns typed `Busy`/`Changed`/`SourceUnavailable` results. Reload and encoding reinterpretation continue to build a replacement file-io session from a sealed read handle; there is no unused same-session reload/encoding command placeholder.
- Encoding diagnostics are a single document-owned latest terminal result. A later encoding failure replaces the earlier revision/range, while every accepted non-encoding terminal outcome, including a successful retry, clears the stale diagnostic. Conflict and cleanup receipts retain their independent durable queues.
- Recovery resume installs a fresh status owner and increments the operation generation. Every peer and captured read handle owns the same session, so the T05 `CleanupHold` remains live until the final view/read owner drops; durable discard and physical cleanup remain separate in `recovery_retirement`.
- Cross-document transfer and linked undo still acquire all document-only session guards before publication and use the unchanged grouped recovery commit/lease primitives.
- Added renderer-free file-io lifecycle contracts that Save Copy preserves UTF-8 BOM/CRLF bytes exactly and that a gated committed Save As survives a concurrent document edit, rejects a second save as busy, leaves the newer content dirty, retains post-commit cleanup authority, rejects stale recovery retirement, and restores the newer edit from the retained journal after restart. Ported the T09 release-gate behaviors to the file-io session: timeout, notify-before-wait, notify-during-wait, spurious notification predicate recheck, and cancellation wake.
- Lightweight local evidence: direct `rustfmt --edition 2024` on changed Rust files and `git diff --check` pass. Cargo and native checks remain central by policy.

## Proposed contained integration commands

Run centrally with the shared target, `--offline --locked`, stopping on the first failure:

1. `cargo test -p bareline-file-io paged_service::lifecycle_contract_tests::headless_client_gets_exact_copy_and_generation_fenced_terminal_receipt --offline --locked`
2. `cargo test -p bareline-file-io paged_service::lifecycle_contract_tests::committed_save_survives_concurrent_edit_and_retains_cleanup_authority --offline --locked`
3. `cargo test -p bareline-file-io paged_service::lifecycle_contract_tests::encoding_diagnostic_tracks_latest_save_attempt_and_clears_on_success --offline --locked`
4. `cargo test -p bareline-file-io paged_service::lifecycle_contract_tests::document_release_gate_preserves_predicate_across_notification_orderings --offline --locked`
5. `cargo test -p bareline-editor-surface paged_view::peer_tests::peer_owns_full_document_and_survives_other_view_close --offline --locked`
6. `cargo test -p bareline-editor-surface paged_spill::tests::pressure_spills_existing_paged_edit_history_and_refreshes_same_identity --offline --locked`
7. `cargo test -p bareline-editor-surface paged_transfer::tests::opaque_cross_document_move_restart_and_linked_undo_preserve_raw_bytes --offline --locked`
8. `cargo test -p bareline-app workspace::tests::paged_workspace_edits_undoes_navigates_and_saves --offline --locked`
9. `cargo test -p bareline-app workspace::tests::readonly_paged_reload_replaces_same_tab_and_preserves_policy --offline --locked`
10. `cargo test -p bareline-app workspace::tests::paged_recovery_restart_preserves_opaque_undo_and_recovers_stale_pointer --offline --locked`
11. `cargo test -p bareline-app workspace::tests::paged_recovery_failed_retirement_is_retryable --offline --locked`
12. `cargo test -p bareline-file-io recovery_retirement::tests::final_cleanup_hold_drop_cannot_miss_receipt_registration --offline --locked`

## Deferred qualification

- The contained Cargo cohort, neutral-layer compile, actual Windows app compile, and native lifecycle scenario remain owned by central integration policy.
