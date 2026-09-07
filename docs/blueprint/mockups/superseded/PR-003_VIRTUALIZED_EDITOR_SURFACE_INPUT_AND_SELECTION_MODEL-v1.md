# PR-003 — Virtualized Editor Surface, Input and Selection Model

**Tracker row:** `PR-003` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-001`, `PR-002`, `PR-023`  
**Primary objective:** Replace the placeholder with the real editor viewport: visible-line virtualization, text layout, caret, selection, scrolling, IME-aware input routing and one unified stream/multi/rectangle selection model, driven by `DocumentService` snapshots and transactions.

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

- EditorSurface view-model contract consuming `DocumentService` snapshots and submitting `EditTransaction`s (ADR-11)
- Visible line/run request API over the internal UTF-8 sequence (ADR-02)
- Caret and selection painting
- Mouse hit testing and keyboard navigation
- Scroll/zoom plumbing; scrollbar and splitter controls come from PR-023
- IME/text composition bridge
- SelectionSet with primary/multiple ranges
- Rectangle projection into per-line ranges
- Hidden-line ranges in `EditorViewState` so PR-006 Hide Lines is view metadata
- Native context menu hook through `PlatformServices` (ADR-04)
- Basic typing/delete/backspace/undo/redo integration

## Explicit non-goals

- No syntax coloring yet
- No find/replace UI
- No split views

## Likely files / ownership

- `crates/editor-surface/**`
- `crates/ui/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Editor state

Keep editor-view state separate from `Document`: `EditorViewState { document_id, selections, scroll, wrap, viewport, preferred_column, ime, blink, hidden_ranges, layout_cache_key }`. Cloned views of the same document therefore have independent cursors/scroll while sharing document revisions. `hidden_ranges` is a sorted set of logical line ranges that the visible-line pipeline skips; it never changes document bytes.

### Document access

The surface holds a `DocumentSnapshot` for paint and hit testing and sends every mutation as an `EditTransaction` to `DocumentService::apply`. It subscribes to revision notifications to refresh its snapshot. Because the logical sequence is always UTF-8 (ADR-02), boundary snapping uses UTF-8 sequence, grapheme and opaque-span atomicity rules; there is no per-encoding branch in the surface.

### Visible-line pipeline

Each paint cycle should:

1. convert scroll position to a bounded logical-line/byte request, skipping `hidden_ranges`;
2. ask the snapshot for visible lines plus small overscan; for `Paged` sources a page not yet available renders as a placeholder row and triggers `prefetch`;
3. request syntax spans already available for those ranges without blocking;
4. shape/layout only visible text through `RenderBackend`;
5. cache `VisualLine` layouts and invalidate selectively after revision/font/wrap changes;
6. paint background/current-line, selections, glyphs, whitespace, carets and overlays in deterministic layers.

Never build a UI node per file line. Long logical lines must be chunked/virtualized so a single multi-megabyte line cannot allocate an unbounded text-layout object.

### Positions and hit testing

Canonical selection endpoints remain byte offsets. A `VisualPosition` may contain line, wrapped-fragment and x coordinate, but it is derived. Hit testing uses shaped cluster metrics and maps to a legal grapheme/text boundary; selections may still preserve arbitrary byte ranges only for explicit byte-oriented operations.

### Keyboard/input

Normalize key handling to command IDs first; printable/IME text becomes insert transactions only after shortcuts decline it. Support dead keys and IME preedit separately from committed text. Preedit is an overlay and must never enter undo/recovery until commit. Right-click and the menu key request a context menu through `PlatformServices::MenuHost` with a `MenuModel` built from the command registry.

### Selection model

Implement one `SelectionSet` with primary selection and sorted normalized ranges. Stream selection is native. Rectangle mode stores rectangle anchors/display columns temporarily and projects to per-line selections when an edit command executes. This prevents a second editing engine.

### Scrolling

Use 64-bit/`f64` logical scroll coordinates independent of scrollbar pixel range. The scrollbar control is the PR-023 `Scrollbar`; the surface supplies a proportional mapping that supports unknown total line count while indexing. Mouse wheel/trackpad events accumulate smoothly but no animation loop runs when settled.

### Wrapping

Wrap is a view concern. Cache wrap breaks per visible line/width/font/revision; do not rewrite document line indexes. Offer no-wrap, viewport wrap and optional column wrap. Very long lines must expose progressive layout so wrap cannot freeze the UI.

### Caret and IME

Caret blink is the only normal periodic UI activity and should suspend when unfocused. IME candidate positioning derives from current shaped caret rectangle. Multi-cursor text commit duplicates committed text to each normalized caret in one transaction; IME preedit is shown at primary caret only unless platform behavior dictates otherwise.

## Minimum verification scenarios

- Type, delete, navigate and select mixed ASCII, Tamil, Arabic, CJK, combining marks and emoji without splitting encoded characters.
- 10,000 carets remain correct and edit as one undoable transaction; UI may cap visible decorations only with explicit UX indication.
- Scroll a 1 GB `Paged` fixture before total line indexing completes; thumb and viewport remain usable and unavailable pages render as placeholders.
- A 20 MB single line can be opened and horizontally navigated without a multi-second main-thread allocation.
- IME preedit cancel leaves document revision unchanged; commit increments exactly once.
- Clone two views of one document and verify view state independence, including independent `hidden_ranges`.
- All surface tests run against `RecordingBackend` on Linux and macOS CI.

## Implementation sequence

1. Mark `PR-003` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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
- Bound background work and make it cancellable when user action can supersede it.
- Avoid new always-running timers/threads when event-driven behavior is possible.
- Treat file paths, encodings, session data and extension data as untrusted input.
- Never mutate a user file before a complete replacement is ready unless the operation is explicitly an in-place user request and has a safe failure design.
- Only crates from the approved dependency list (ADR-09) may be added; justify each new dependency in the PR description.

## Acceptance checklist

- [ ] Typing submits `EditTransaction`s to the PR-002 `DocumentService`; no UI crate owns document bytes
- [ ] At most visible/nearby lines are formatted for paint
- [ ] 1000 added carets remain responsive on a moderate file
- [ ] Unicode tests cover combining marks, emoji and CJK input without invalid byte ranges
- [ ] Hidden-line ranges are view state and never alter bytes
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-003: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-01, FC-02, FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-005, FR-006, FR-013, FR-030. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Virtual surface; input; long-line and opaque-span boundaries. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-003-01** — Select across emoji clusters, combining accents and opaque byte markers; text edits never split their atomic boundaries.
- [ ] **AC-003-02** — Scroll a 20 MiB single line; layout is bounded and cancellation leaves input responsive.
- [ ] **AC-003-03** — Commit and cancel IME preedit; only committed text enters undo and recovery; verify real Windows shaping and BiDi hit testing.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
