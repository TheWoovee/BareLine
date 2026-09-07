# PR-025 — Diff Core Engine

**Tracker row:** `PR-025` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-002`  
**Primary objective:** Deliver the OS-neutral `crates/diff` engine (ADR-12): bounded line diff with anchoring and Myers refinement, intraline refinement, coarse fallback, cancellation and revision-tagged results over `DocumentSnapshot` chunks. Consumed by PR-004 conflict and recovery previews, PR-015 external-change preview and the PR-017 Compare Workspace.

## Agent contract

This brief includes local scope and must be read with its assigned v1.2 foundation sections and acceptance cases below. Implement the scope below without scanning the whole documentation directory. Read a dependency PR only when you need the exact merged interface. If a requirement here conflicts with code already merged, preserve the public product intent and make the smallest compatible contract adjustment rather than inventing a parallel architecture. If an architecture question arises, read `../07_DECISION_LOG.md`; it overrides this file.

## Architecture snapshot (do not redesign casually)

- Rust 2024 Cargo workspace. MPL-2.0 core; MIT OR Apache-2.0 extension SDK and first-party extensions.
- `winit 0.30.x` owns portable window/input events; Windows rendering uses Direct2D/DirectWrite (hardware or software mode) behind a neutral `RenderBackend`; a `RecordingBackend` exists for headless tests. The document is **never** stored in a toolkit text widget.
- `bareline-document` owns the resident-or-paged `ByteSource` (no mmap in v1), piece tree, snapshots, revisions, the `DocumentService` actor and edit transactions. The valid UTF-8 text view and original byte domain are distinct; undecodable spans retain original bytes through provenance metadata, and large transcodes use disk-backed storage (FC-01/02).
- Canonical document positions are byte offsets into the internal UTF-8 sequence; line/column/grapheme are derived views.
- UI thread never performs unbounded file I/O, regex, syntax indexing or extension RPC. No async runtime in the editor process.
- Third-party extensions never load inside `bareline.exe`; the extension host is an optional signed download.
- Windows-specific code belongs in `platform-windows`; cross-platform core crates must remain OS-neutral. Native Win32 menus/dialogs are reached only through `PlatformServices`.
- Every user-visible command has a stable command ID.
- Persistent formats are versioned and use atomic writes/migrations: TOML for human-edited files, JSON for machine-written state, binary CRC32C journals.
- Performance is designed in: the startup budget, the dependency policy and continuous `xtask perf` measurement apply to every PR; performance numbers are targets, never merge gates. See `../07_DECISION_LOG.md`.

## Scope to implement

- `CompareOptions`, `DiffKind`, `DiffHunk`, `CompareResult`, `CompareCompleteness` types
- Line-oriented diff with patience/histogram anchoring and bounded Myers refinement
- Intraline word/token/grapheme refinement on changed line pairs
- Coarse block/anchor fallback under resource limits
- Cancellation token checked at bounded intervals
- Chunked streaming over `DocumentSnapshot` for both sides
- Revision tagging and stale-result rejection helpers
- Hunk application helper that produces `EditTransaction`s for accept left/right
- `xtask perf` scenario for diff throughput and memory

## Explicit non-goals

- No UI; the Compare Workspace is PR-017
- No folder compare (v1.1, ADR-26)
- No binary diff
- No three-way merge

## Likely files / ownership

- `crates/diff/**`
- `tests/diff/**`
- `xtask/src/perf/diff*`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Types

```rust
CompareOptions {
  whitespace: Significant | TrimEdges | IgnoreAll,
  ignore_blank_lines: bool,
  ignore_case: bool,
  ignore_eol_style: bool,
  ignore_encoding_bom: bool,
  normalize_tabs: bool,
  limits: ResourceLimits { max_lines_exact, max_bytes_exact, time_budget_ms, max_memory_bytes },
}

DiffKind = Equal | Added | Removed | Changed | MovedAligned
DiffHunk { stable_id: HunkId, left: TextByteRange, right: TextByteRange, left_line_hint, right_line_hint, kind: DiffKind, intraline: Vec<IntralineSpan> }
CompareTerminal = CompletedExact | CompletedCoarse { reason } | Cancelled | Unavailable | Failed
CompareResult { left_revision, right_revision, hunk_batches: BoundedBatches, terminal_state, stats: DiffStats }
```

`HunkId` is stable across recompares when the surrounding anchor lines are unchanged, so navigation and the current-difference marker survive edits. Hunks retain original text byte ranges plus optional logical line hints; ignore normalization must never lose merge coordinates.

### Line hashing and normalization

Each side is read as line chunks from its snapshot. A normalized hash per line applies the options (trim, collapse whitespace, case fold, strip EOL, expand tabs) without materializing a normalized copy of the whole file; only a bounded window of normalized lines is held for refinement. Hash collisions are resolved by comparing normalized bytes under the same options.

### Algorithm

1. Find unique-line anchors (patience) or low-frequency anchors (histogram) within capped windows or spill-backed indexes; both anchors and emitted hunk batches count toward the global memory budget.
2. Between anchors, run Myers on regions whose size is within `limits`; regions above the limit are recursively anchored, and if no anchor exists they become a single `Changed` block.
3. Classify blocks as Added, Removed or Changed; detect `MovedAligned` when a removed block's hash sequence appears as an added block elsewhere within a bounded search window.
4. For `Changed` pairs, run bounded intraline refinement on word tokens first, then graphemes for short spans, producing `IntralineSpan`s.

### Coarse fallback

If `max_lines_exact`, `max_bytes_exact`, `time_budget_ms` or `max_memory_bytes` is exceeded, the engine finishes with anchors and block classification only and reports `Coarse { reason }`. Unavailable source and cancellation are separate terminal states; neither is a completed coarse result. All buffers and output batches fit the resource budget with backpressure.

### Cancellation and staleness

A `CancelToken` is checked at least every 64 KiB of input or 1 ms of work. Results carry both snapshot revisions; the helper `is_stale(result, left_now, right_now)` lets consumers discard results whose sources moved. Consumers never paint stale hunks.

### Applying hunks

`apply_hunk(direction, hunk, source_snapshot, target_snapshot) -> EditTransaction` produces a normal transaction against the target's revision so merge and copy actions inherit undo, dirty state, recovery and save safety from `DocumentService`.

### Threading

The engine is a pure function over snapshots and options. Callers run it on the I/O or search pool; the crate spawns no threads and has no global state.

## Minimum verification scenarios

- Golden fixtures for insert, delete, change, move-like alignment, Unicode (combining marks, emoji, CJK), blank-line handling and every ignore option, each with expected hunks and intraline spans.
- Property test: with ignore options off, applying all right hunks yields right bytes; with ignores on, only selected nonignored ranges change. Both are undoable.
- Stale-result test: edit one side during a background compare and prove the result is rejected by `is_stale`.
- Multi-GB generated fixture pair diffs within bounded memory, cancels within 50 ms of the token being set, and reports `Coarse` with a reason.
- Highly divergent 200 MB pair (no anchors) completes as a single Changed block without quadratic time.
- Stable hunk IDs survive an unrelated edit elsewhere in the document.
- `xtask perf` records diff throughput and peak memory for the 10 MB and 1 GB pairs.

## Implementation sequence

1. Mark `PR-025` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
2. Add/confirm the public types and interfaces required by this PR before wiring UI details.
3. Implement core behavior with unit/property tests at the owning crate level.
4. Wire command IDs and UI state. UI event handlers must dispatch to commands/services instead of containing document/search business logic.
5. Add failure/cancellation handling for any file/background/process operation.
6. Run the focused tests for changed crates, then the workspace compile/clippy set needed to catch interface breakage.
7. Update the tracker with implementation commit and set **Implemented = DONE**. Verification/testing/acceptance may be performed by separate agents according to project workflow.

## Required test categories

- Happy-path unit tests for every new public behavior.
- At least one error-path test for each filesystem/process/FFI boundary introduced by this PR.
- Regression tests for any bug discovered while implementing.
- Cross-platform compile tests for neutral crates touched by this PR.
- No giant binary fixtures: generate large data during test/benchmark setup.

## Performance and safety constraints

- Do not materialize an entire file before its first viewport is interactive, and never fully materialize a file above the resident threshold in RAM (ADR-01).
- Only crates from the approved dependency list (ADR-09) may be added; justify each new dependency in the PR description.
- Bound background work and make it cancellable when user action can supersede it.
- Avoid new always-running timers/threads when event-driven behavior is possible.
- Treat file paths, encodings, session data and extension data as untrusted input.
- Never mutate a user file before a complete replacement is ready unless the operation is explicitly an in-place user request and has a safe failure design.


## Acceptance checklist

- [ ] Exact and coarse modes are both covered by tests with expected completeness
- [ ] No full-file string materialization on either side
- [ ] Cancellation is acknowledged within 50 ms in the multi-GB scenario
- [ ] Hunk application produces undoable `EditTransaction`s
- [ ] Crate has no OS imports and no threads of its own
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-025: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-02, FC-06 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-021A. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Bounded diff; normalized equality; original byte ranges. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-025-01** — With all ignore options off, applying all hunks reproduces the target bytes; with ignores on, only explicit nonignored ranges change.
- [ ] **AC-025-02** — Force normalized-hash collisions and whitespace/case equality; compare normalized bytes, retaining original byte ranges for edits.
- [ ] **AC-025-03** — Compare divergent multi-GB files under anchor/output caps; distinguish CompletedCoarse, Cancelled and Unavailable without claiming incomplete output is exact.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
