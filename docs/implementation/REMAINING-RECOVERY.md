# Remaining recovery, save and session fixes

Scope: native retest 009/030/031 and paged-spill cleanup. Authority: PR-004 file lifecycle/recovery, source-generation integrity; preserve saved bytes and true external conflicts. Changes and static evidence recorded below; combined validation belongs to parent task.

## Changes frozen for combined validation

- 009: recovery preview now routes retained owned/foreign page tickets through the snapshot loader before falling back to its primary decoded source. The old direct FileSource call rejected valid owned generations as Changed. The UI calls a shared bounded 8 KiB preview helper; closing/changing selection discards the cancelled receiver so stale/cancelled results cannot become the next selection's preview. Existing resident/untitled recovery regression now checks this same helper before recovered-copy save.
- 030: see REMAINING-SESSION.md. Comparison setup runs before authoritative saved view installation, preserving the persisted active pane/tab and caret.
- Automated cleanup: see REMAINING-SPILL.md. Fixture teardown now drains independent prefetch actor ownership as well as the FIFO worker before deleting Windows files.
- 031: live paged clone refresh now adopts the actor's updated fingerprint after a peer save. Previously only following views adopted Completed.fingerprint, leaving normal clones on an obsolete disk identity. Extended the existing clone-lifetime regression with Save As, peer fingerprint/clean-state checks, and source_changed assertions. Watch reconciliation now requests a fresh check whenever acknowledged open-file identities change, without relying on a later OS event and without clearing a conflict before verification.

## Evidence and limits

Static call-path review only. No build, test, or UI run was performed by this owner. Parent owns combined validation. Existing local changes were preserved; no commits.

031 is **not confirmed closed**: the exact native banner is shared by watch.conflicts and the paged source_mismatch flag, so its text alone cannot attribute the original failure. The clone identity gap is concrete, but the recorded single-file reproduction still requires native save/recheck with an exact byte oracle and a true external-writer control. Saved-file recovery Compare also remains pending native acceptance. Session startup failure did not reproduce in the prior retest and is not newly claimed fixed.

## Native correction: restored original pages cancelled (009)

The preserved app2 profile relaunched successfully through a controlled process invocation; startup itself was not reproduced as an application error. Native preview passed. Compare then displayed a recovered pane hatched with `Source unavailable: Cancelled` while the current disk pane rendered correctly.

Concrete cause: paged_recovery::restore gave its durable primary FileSource the short-lived restore operation cancellation token. Workspace completion drops IoTicket, whose Drop cancels that token. Original recipe pages requested afterward were cancelled; owned pages and streaming save could still succeed, explaining the earlier byte oracle success. Normal transcode already separates this lifetime with a fresh cancellation token.

Fix: restored primary/foreign FileSources now own fresh tokens, while recipe validation and a final pre-publication cancellation check still use the operation token. Added deterministic regression to the existing opaque paged-recovery restart test: restore without caching original pages, cancel the operation, then read the viewport and assert exact decoded content. No builds/tests executed by this owner; parent owns the correction batch and native saved-file comparison retest.
