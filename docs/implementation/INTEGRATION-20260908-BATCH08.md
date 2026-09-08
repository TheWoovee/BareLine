# Batch 08 views closure

PR010 is implementation complete pending acceptance after its remaining off-viewport selection requirement was implemented and independently reviewed. Eighteen packages now have that state, nine remain in progress and none is fully accepted.

Paged views retain canonical global anchor/caret independently of their loaded projection. Bounded asynchronous validation rejects invalid UTF-8 endpoints and stale results; pending restore waits for the exact acknowledgement. Clone/session capture and real search/accessibility callers use global endpoints. Unsupported proxy editing and clipboard operations are explicitly refused, and an offscreen selection does not paint a false edge caret. This completes the gap left open in batch06 alongside the already verified per-pane styling, byte anchors, global folds and scroll synchronization.

Focused offline locked verification passed: native23, paged2, workspace13 and accessibility6. Existing regressions were extended to cover a far-off-viewport selection, independent clones, session roundtrip and invalid UTF-8 boundaries; no new test names are added to the cumulative **411** distinct passing Rust count. The complete reviewed semantic golden still passes. No file-io or full workspace suite was repeated.

The final default build passed in25.00 seconds. Native SHA-256: `39E030A5788E2AED5E6D3D042C4A069728BDA31DFB5039C8727358648BE1D468`. Twelve existing warning groups remain visible. Logs: original checkout `target/integration-batch-08-{views,paged,workspace,access,default-build}.log`.

Independent review found no blocker in canonical selection authority, bounded stale validation, pending restore acknowledgement, capture/clone and caller handling. Manual native keyboard/pointer/visual and controlled large-file acceptance remain pending. No remote, publication or manual QA operation occurred. Concurrent original-tree development is not included in this tested artifact.
