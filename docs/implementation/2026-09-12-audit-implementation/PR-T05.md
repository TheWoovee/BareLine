# PR-T05: Asynchronous recovery retirement

- **Baseline:** `67fbae06c16e51dffa95ad3b08ef69f0ff91a725`
- **Scope:** Persist content-free discard intent off the UI thread, retain cleanup ownership across tab lifetime, and report/retry incomplete recovery cleanup truthfully.
- **Coordination:** Keep save transaction behavior and unrelated workspace close/lifecycle changes unchanged.

## Evidence

- `recovery_retirement` uses the process-wide bounded maintenance executor (2 running, 32 queued). Known recovery handles and paths remain in the state before fallible receiver draining begins. Terminal guards publish and wake on draining, tombstone, purge, and retained-cleanup unwind; the regressions prove failed work remains retryable after its UI ticket is dropped. Completed retained owners are pruned, cleanup paths are deduplicated, warning history is bounded, and a successful retry clears the prior cleanup failure.
- Resident discard transfers both generation handles, pending checkpoint/retirement receivers, known generation paths, and adopted-recovery paths to the worker. Paged discard shares one ticket across views and uses `try_lock` when taking a journal, so an actor-held recovery lock cannot block the UI.
- `Workspace::close` and the shell retain stable document-owner and consent-revision identity while discard is pending. Only the discard target is temporarily read-only. Tab reordering cannot redirect the close, and a changed target resumes recovery rather than disappearing. Application exit revalidates every current dirty or busy document after tombstone publication; a new or changed document cancels exit, restores the prior read-only state, and starts fresh recovery ownership.
- `retired.json` and a content-free cleanup proof are published before close is allowed. Cleanup captures the guarded root/candidate identities and a bounded no-follow proof read in a retained receipt, so a retry does not depend on `manifest.json` surviving a partial purge. Cleanup delegates to T01's proof-preserving Windows removal primitive with 4,096-entry and 250 ms limits. Unsupported or uncertain deletion fails closed and retains the exact receipt for retry; no recursive path fallback remains. A focused Windows boundary regression removes `manifest.json` before invoking the retained-proof cleanup.
- `IoRequest::RestorePagedRecovery` now performs recovery inspection and bounded full-text materialization on the existing I/O worker. Cancellation and inspection/materialization failures reach a failed terminal outcome; only the legitimate size-limit `Ok(None)` keeps the paged fallback. `Workspace::adopt_recovered_resident` only consumes prepared text.
- Undo Close Tab clears the completed discard ticket and creates a fresh resident slot, status, cancellation generation, and recovery root ownership. The restart regression gates the old purge, restores and edits the closed buffer, requires a different durable recovery directory, then proves the old cleanup cannot delete the new checkpoint. The shell regression exercises the deferred close entrypoint across a tab reorder and verifies the original read-only state is restored.
- Paged Undo Close Tab retains an explicit shared cleanup hold after durable tombstone publication. Physical removal waits while any view or the closed-editor owner can still read the recovered store; dropping the final view releases the hold and submits the exact retained cleanup receipt. This prevents neutral filesystems from unlinking the old source before a reopened editor establishes its next baseline.
- Held cleanup registration is linearized against final-owner release: the receipt enters the registry before the worker rechecks its weak owner. A final drop either takes that registered receipt or is observed by the post-registration requeue, without sleeps or submitting while the registry lock is held. The focused, hold-qualified barrier regression forces the former lost-wake ordering, verifies the tombstone is already durable and source bytes remain exact, then proves final release purges the journal. Tests that clear the process registry share one serialization guard; the hook uses bounded waits and cannot capture unrelated cleanup.
- Central correction evidence retained: the initial paged restart reported `Retain paged baseline .../source-1: Io OS3 NotFound`, proving that the reopened actor still depended on the retired generation. The initial shell fixtures also timed out before checkpoint completion because their gating filesystem did not delegate the required free-space query; the fixture now delegates that production primitive and reports recovery, actor, I/O and root state on any future timeout.
- Static evidence run in the private worktree: direct `rustfmt --edition 2024` on all changed Rust files and `git diff --check`. Cargo and native checks remain central by policy.

## Proposed contained integration commands

Run centrally with the shared target, `--offline --locked`, stopping on the first failure:

1. `cargo test -p bareline-file-io recovery_retirement::tests::delayed_drain_failed_tombstone_and_retained_cleanup_are_truthful --offline --locked`
2. `cargo test -p bareline-file-io resident_recovery::journal_tests::discard_removes_every_generation_even_mid_rotation --offline --locked`
3. `cargo test -p bareline-app workspace::tests::small_recovery_restores_an_editable_untitled_document --offline --locked`
4. `cargo test -p bareline-app workspace::tests::resident_and_untitled_automatic_recovery_restore_current_text --offline --locked`
5. `cargo test -p bareline-app workspace::tests::paged_recovery_restart_preserves_opaque_undo_and_recovers_stale_pointer --offline --locked`
6. `cargo test -p bareline deferred_close_tests --offline --locked`
7. `cargo test -p bareline-platform-windows owned_cache::tests::retained_cleanup_receipt_removes_candidate_after_manifest_is_gone --offline --locked`
8. `cargo test -p bareline-file-io recovery_retirement::tests::final_cleanup_hold_drop_cannot_miss_receipt_registration --offline --locked`

## Deferred qualification

- The proposed Cargo tests, native type/discard/continue-editing responsiveness scenario, and final integrated compile remain owned by integration.
