# PR-T04 — Preserve displaced bytes across save commit races

Original baseline: `97fcf41ce355874358129453cf2a2f77d2ee0095`; final rebased baseline: `ec0d5f488bce0f523850889969b51a9f07e2dd79`.

## Scope

Replace the pathname-only save commit with a prepared platform transaction and typed receipt. Replacement must atomically retain the file displaced by this transaction, validate those retained bytes against the destination approved by PR-T03, and keep both versions when a race is detected. Create-new remains no-replace.

## Decisions

- PR-T03's captured `DestinationCondition` remains the authority for whether a destination may be created or replaced.
- Every prepared commit owns an independent proposed-byte copy, an immutable UTF-16 manifest, and append-only state records in a unique transaction directory. The copy uses bounded chunks with cancellation between reads and removes partial preparation artifacts on failure. Replacement also reserves a displaced-version name there.
- A replacement receipt describes the committed editor version and the independently retained displaced version. Windows opens no-follow verification/delete handles before publishing the receipt; cleanup consumes those handles and deletes the exact retained file objects.
- A conflict after commit remains unresolved and must not advance document saved state. Recovery artifacts remain owned until a terminal cleanup decision.
- Windows performs the atomic displacement through its native replacement backup facility; neutral crates carry paths, fingerprints, states and cleanup ownership only.
- Transaction state records `precommit`, `committed-unacknowledged`, `conflict`, or `cleanup-pending`. Opening a file discovers bounded orphan transaction directories through guarded no-follow reads; malformed or partial records remain visible as `unverified` recovery entries.
- Recovery discovery and cleanup retry run on the bounded file worker. Parent scans are deduplicated, cancellation-aware, and publish terminal results through the normal wake path.
- Conflict actions retain an explicit selected transaction. Compare opens and starts the correct pair after load; Save Elsewhere opens the preserved editor generation and invokes the native destination picker plus PR-T03 preflight; cleanup retry consumes only the selected receipt owner.
- Conflict opens use T18 tracked outcomes in a reserved monotonic request range and consume only their own receipts. They deliberately load fresh recovery snapshots even when a dirty tab already owns the target path; completion must match the action receipt's exact document identity and path. A failed peer open drains every request owned by that action.
- Verified `cleanup-pending` restart records reacquire guarded native cleanup handles and expose a real retry owner. Unverified and cleanup-only records never enable content-opening actions.
- Cleanup publishes an immutable authority manifest through the live receipt and retains its exact no-write/no-delete-sharing handle before the `cleanup-pending` state becomes durable. Restart retry revalidates that authority under a reacquired transaction-directory lease, including the window after artifact/state retirement and before primary-manifest deletion.
- Once target, proposed and displaced bytes are durably verified, the document is saved. A later cleanup failure returns a cleanup warning with the live receipt and exact handles retained for retry; it does not leave Save All or close waiting on a write that already committed.

Windows NTFS replacement atomically publishes the staged pathname and captures the target displaced by that operation. The platform cannot prevent an independent writer from changing the published target afterward. Verification therefore reports that later change as a conflict while the receipt-held proposed copy and displaced copy remain immutable. No pathname fingerprint check is described as compare-and-swap.

## Changed files

- `crates/platform/src/lib.rs`: prepared commit, typed receipt/state/cleanup contract and state readback.
- `crates/platform-windows/src/files.rs`, `replacement_faults.rs`: unique guarded transaction directories, `ReplaceFileW` displaced backup, exact-handle cleanup, native race/sharing/crash fixtures.
- `crates/file-io/src/lifecycle.rs`, `fault_transitions.rs`, `paged_service.rs`: receipt validation, typed postcommit outcomes, orphan discovery and deterministic lifecycle barriers.
- `crates/editor-surface/src/paged_view.rs`, `crates/app/src/workspace.rs`: preserve typed paged/resident conflicts and expose recovery actions without advancing saved state.
- `apps/bareline/src/windows_app.rs`, `windows_app/lifecycle.rs`, `windows_app/compare.rs`: deferred commands to start comparison, save the editor version elsewhere, retain recovery, and retry verified cleanup.

## Evidence

- Added byte-level fixtures for a writer immediately before native replacement, another writer after commit, an absent-destination create race, native sharing violation, cancellation after replacement, every staged/flush/replace/receipt storage-failure boundary, process death around both generic and actual Windows replacement, a gated asynchronous recovery scan, production partial-cleanup failure plus restart retry, a substituted-directory sentinel, and tracked Shell compare/Save Elsewhere flows.
- Crash fixtures read back transaction state and verify exact proposed/displaced bytes.
- `rustfmt --edition 2024` on changed Rust files and `git diff --check` are the worker-side checks. Root owns the contained Cargo cohort in the integration checkout.

## Qualifications

Root runs all Cargo checks in the integration checkout after source review. Native dialog qualification remains deferred; native filesystem tests still require execution on NTFS. The compatibility `commit(staged,target,existed)` primitive remains for non-document adapters, but document saving calls only `prepare_commit` plus `commit_transaction`.
