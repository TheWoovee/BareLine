# PR-U05 — Recovery Center loading and live refresh

## Scope

Finding UX-06 on accepted U02 baseline `81d98190e8ba995dba656d5ce4247260bd537e6a`, plus isolated U02 compile correction `3a2fc582b11ec300306c5f35d1779c04dccc227b`. This package changes the Recovery Center discovery, rows, preview, actions, and accessibility status in `apps/bareline/src/windows_app/recovery.rs`. Its small `crates/app/src/workspace.rs` interface assigns tracked recovery restores a request ID and publishes a bounded typed terminal result only after the recovered editor is published or the owned worker fails. The shared modal and typed notification contracts accepted in U02 remain authoritative.

The current blueprint reference is `docs/blueprint/mockups/bareline-recovery-center.png`: a clear Recovery Center heading, document rows as the primary content, a bounded selected-row preview, consequential actions at the bottom, and status/error information inside the modal.

## Intended behavior

Recovery content has explicit `Discovering`, `Ready(rows)`, and `Failed(error)` states. Every worker completion carries the discovery generation and root identity that launched it; stale completions cannot replace current content. Empty text appears only in terminal `Ready`.

Opening and closing the center attaches or detaches only the preview worker. Global discovery continues independently. A ready result replaces rows while the center is open, preserves selection by recovery identity, and restarts preview only when the selected representative changes. A failed result exposes Retry and Close with the full error in UI Automation. Open and Close remain available while discovery is loading or failed; checkpoint actions are mutually gated while restore, compare, export, or delete owns the checkpoint. The initial keyboard command and UI Automation focus identify the same enabled visible action, falling back to Close for an incomplete selection or busy checkpoint, and both the named activation keys and the character-space event route through it.

When a discovery with rows completes while the center is closed, U02 publishes one transient completion notice for that generation. Export and delete terminal outcomes request a fresh fenced discovery. Restore records its request ID, checkpoint directory, root, and discovery generation, then waits for the matching Workspace terminal result. Paged success follows editor publication; resident success follows the exact tracked recovered-text insertion acknowledgement, preserving its normal dirty and undo behavior. A matching failure retains the checkpoint and publishes the full error; an old root or generation cannot refresh or surface stale failure. Pending resident acknowledgements share the same bounded result capacity and results are selectively consumed. A removed selection moves to the nearest survivor and clears its preview.

## Controlled evidence

Deterministic controller tests publish tagged completions without timing sleeps and cover loading-to-ready live rows, terminal empty, stale root/generation rejection, failed retry semantics with full UIA error, identity-preserving selection, nearest-survivor refresh, closed one-time notification, preview/discovery lifetime separation, loading/failed opening, initial command focus, character-space activation, mutual checkpoint-action gating, incomplete-selection fallback focus, and restore submission-to-terminal refresh/failure fencing. Existing resident/untitled and paged Workspace recovery fixtures now submit tracked restores and assert that the published terminal document identity matches the inserted editor; a missing-directory worker fixture asserts a full failure is consumable and does not remain pending. A bounded selective-retention test verifies another request's result is not removed. `rustfmt --check` parses the touched Rust modules and `git diff --check` verifies the patch.

Root-owned contained commands requested:

```text
cargo test -p bareline recovery::tests -- --nocapture
cargo test -p bareline-app tracked_recovery_restore -- --nocapture
cargo test -p bareline-app paged_recovery_restart_preserves_opaque_undo_and_recovers_stale_pointer -- --nocapture
cargo test -p bareline-app resident_and_untitled_automatic_recovery_restore_current_text -- --nocapture
cargo test -p bareline accessibility::tests::complete_native_semantic_json_golden -- --nocapture
cargo check -p bareline
```

The native acceptance journey remains root-owned: open during delayed discovery, verify that final empty never appears in flight, publish an Untitled checkpoint, observe the live row without reopening, exercise failure Retry through keyboard and UIA, and confirm refresh after the actual terminal restore, export, and delete outcomes. The semantic fixture will need regeneration/review because Recovery Center now includes its explicit status node and failure action set.
