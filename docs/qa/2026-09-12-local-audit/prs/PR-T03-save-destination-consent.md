# PR-T03 — Carry approved overwrite identity through Save As and Save Copy

Priority P1. Fixes TECH-003 and the corresponding native UX finding. Depends on PR-T01 only if sharing path guards. Land the destination contract before PR-T04; coordinate the shared `lifecycle.rs` changes. Own the save dialog improvements reported by Sol here to avoid a duplicate UX implementation.

## Current behavior

`Workspace::save_internal` (`crates/app/src/workspace.rs:2033–2122`) only captures an expected fingerprint for the document's existing same path. `save_bytes` (`crates/file-io/src/lifecycle.rs:725–739`) correctly interprets no fingerprint as “must be absent.” The Windows overwrite prompt returns only a path. Thus approved overwrite of another existing file always conflicts. Native and direct-production repro evidence is in `../ui-ux` and `../evidence/save-probe.log`.

## Interface and state changes

1. Add a typed prepared destination: canonical path, `MustBeAbsent` or `ReplaceCaptured(Fingerprint)`, consent state, originating document identity and operation (`Save`, `SaveAs`, `SaveCopy`). Do not overload `Option<Fingerprint>` with “permission to overwrite anything.”
2. In `crates/platform/src/lib.rs` and Windows `native.rs::dialog`, return the selected path separately from overwrite authority. Add asynchronous destination preflight in the shell. Capture trusted destination metadata/content fingerprint, then obtain the single overwrite confirmation against that capture. Suppress duplicate native/custom confirmations; do not treat a second fingerprint after consent as consent to a newly changed file.
3. Route File Save As, Untitled Save, Save All's untitled entries and Save Copy through the same preparation path (`windows_app.rs`, `lifecycle.rs`). Default filename should derive from the document; default directory should prefer the current document directory, then a valid last-used save location, then the user's Documents location. Do not inherit System32 as the user-facing default.
4. Thread the prepared target through resident `IoRequest`, encoded saves, `PagedSaveRequest`, and `PagedEditorSurface::save/save_copy`. Reject a target equal to the copy source using filesystem identity, not just spelling. Cancel/reject stale preflight if the document closes or changes operation.
5. Preserve semantics: Save As updates identity and recent files only after a successful receipt; Save Copy preserves the document path, dirty state, encoding, undo and recovery status. Failed/canceled writes preserve both source and target. Integrate TECH-004's stronger commit guarantee when PR-T04 lands.

## Tests and done criteria

Use UTF-8, UTF-16 and paged fixtures for: absent destination, existing destination accepted, overwrite declined, target changed after consent, source alias/hard-link rejection, read-only destination, close during preflight, Save All cancellation and Save Copy identity. Assert exact bytes, BOM, dirty state and no stray stage after success. Native test: confirm overwrite once and see the chosen destination updated, with a truthful success state. Test Save As defaults without admin privileges. Never “fix” this by removing conflict checks or always using the latest fingerprint.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
