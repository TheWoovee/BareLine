# PR-010 — Split Views, Tab Management and Synchronized Scrolling

**Tracker row:** `PR-010` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-003`, `PR-004`, `PR-023`  
**Primary objective:** Implement Notepad++-class dual-view behavior, the tab strip UI on top of the PR-004 `TabModel` (ADR-14), and persistent layouts.

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

- Horizontal/vertical two-pane split
- Move/clone document to other view
- Independent view state for same document
- Synchronized horizontal/vertical scroll options
- Tab strip UI: drag reorder, keyboard reorder, overflow/scroll, tab colors, pin indicator rendering, vertical-tab option
- Drag/drop tab between views
- Tab sort mode UI (model sort operations come from PR-004)
- MRU document switcher UI
- Persist/restore split and view states through the PR-004 session schema

## Explicit non-goals

- No arbitrary 3+ pane grid in v1
- No detached editor windows beyond new app instance

## Likely files / ownership

- `crates/app/src/views*`
- `crates/ui/**`
- `crates/ui/src/tabs/**`
- `crates/ui/src/split/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### View graph

Model the central editor area as a small split tree: leaf = `EditorViewId`, branch = orientation + ratio + children. Keep a maximum of two editor panes in Windows v1 unless requirements explicitly expand; do not evolve into arbitrary IDE docking.

### Move vs clone

- **Move to other view:** reassign tab/view placement; document remains same.
- **Clone to other view:** create new `EditorViewState` referencing same `DocumentId` with independent selections/scroll/folds.
- Saving/dirty/recovery state belongs to document and updates both views.

### Synchronized scrolling

Sync is an opt-in relation between two views. Vertical sync uses logical line/relative visible position, not raw scrollbar pixels; horizontal sync uses text x offset. Prevent feedback loops with origin tokens. If wrapping differs, map best-effort and never lock the UI in oscillation.

### Tabs

The `TabModel`, pin state, order, close policies, sort operations and persistence are owned by PR-004 and consumed here; do not redefine them (ADR-14). PR-010 owns the tab strip UI built from PR-023 controls: drag reorder, keyboard reorder, overflow/scroll, tab colors, pin indicator rendering, vertical tabs, the MRU switcher popover and drag between views. Every UI gesture maps to a `TabModel` operation or a command ID. Pinned region is stable at front unless a setting chooses otherwise. Closing a cloned view closes only that view unless it is last reference to dirty document, then normal dirty-close prompt applies.

### Session serialization

Persist split tree, ratios, tab/view membership and per-view state using stable IDs inside the PR-004 versioned JSON session schema (ADR-21); add a `layout` section rather than a second file. Invalid/corrupt layout falls back to one pane while preserving recoverable documents.

## Minimum verification scenarios

- Same document in two panes: edit one, both content updates; cursors/folds stay independent.
- Vertical/horizontal sync with different viewport sizes and wrap modes has no recursion/jitter.
- 500 tabs with pinned/colored/MRU order survive session restart/update migration.
- Drag/reorder cannot move a pinned tab into invalid state.
- Corrupt split layout does not prevent session recovery.

## Implementation sequence

1. Mark `PR-010` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Same document can be edited from both views without divergent storage
- [ ] Pinned/clone state survives restart
- [ ] Synchronized scroll cannot cause feedback loop
- [ ] Closing one clone does not close the other unless last view
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-010: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-06 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-003, FR-004. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Tab/view model; alignment map; synchronization. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-010-01** — Pin a tab and reorder across views; pinned-first default and per-view selection persist after restore.
- [ ] **AC-010-02** — Add a compare alignment spacer; it receives no document line number and synchronized scrolling maps logical lines correctly.
- [ ] **AC-010-03** — Clone a document in a second pane; edits share a document while caret, selection, folds and scroll remain independent.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
