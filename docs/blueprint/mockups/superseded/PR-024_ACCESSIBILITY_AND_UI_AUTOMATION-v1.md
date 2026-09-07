# PR-024 — Accessibility and UI Automation

**Tracker row:** `PR-024` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-003`, `PR-011`, `PR-023`  
**Primary objective:** Make every Bareline surface operable by keyboard and readable by screen readers through AccessKit (ADR-06): a semantic tree for chrome, bounded text-pattern access for the editor, visible focus, high-contrast verification and accessible names for all commands.

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

- `accesskit`, `accesskit_winit` and `accesskit_windows` integration in `platform-windows`
- Neutral semantic tree built from PR-023 `SemanticNode`s and the editor view
- Roles, names, states and actions for tabs, tree, list, settings rows, search controls, palette and status bar
- Editor text pattern: caret, selection, current line and visible text through bounded ranges
- Keyboard-only operability of all chrome with visible focus indicators
- High-contrast verification of every surface
- Accessible names for all commands from PR-011 title keys
- Screen-reader smoke test scripts for Narrator and NVDA

## Explicit non-goals

- No braille display special handling
- No speech synthesis or self-voicing
- No accessibility for native menus beyond what Win32 already provides (ADR-04)

## Likely files / ownership

- `crates/a11y/**`
- `crates/ui/src/semantic/**`
- `crates/editor-surface/src/a11y*`
- `crates/platform-windows/src/a11y*`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Semantic tree

`crates/a11y` owns a neutral `SemanticTree` built incrementally from the retained UI tree. Each PR-023 control already emits a `SemanticNode`; this PR adds container nodes (window, tab list, panel, toolbar, status bar), ordering, and live-region announcements for banners and search progress. Updates are diffs, not full rebuilds, and are produced on the UI thread after layout, never during paint.

### AccessKit adapter

`platform-windows` hosts the `accesskit_windows` adapter and feeds it tree updates from `accesskit_winit` events. The adapter is created lazily on the first UI Automation client request so idle startup pays nothing. No Win32 UIA type appears outside `platform-windows`.

### Editor text pattern

The editor exposes `TextRange`s over the visible viewport plus overscan only. Caret and selection map from byte offsets to grapheme-based text units through the existing derived views. Requests beyond the visible range are served in bounded steps (one screen at a time) and never trigger a full-document read or line-index scan. Current line, word at caret, and selection text are provided for announcements. Composition (IME preedit) is exposed as an active composition range.

### Keyboard operability

Every interactive control is reachable through the PR-023 focus chain. Panels expose F6-style cycling between editor, sidebar, bottom panel and status bar. Escape always returns focus to the editor from any transient surface. Focus is restored to the previous element when a popover, palette or dialog closes.

### Focus visibility

A focus ring is drawn from the `focus` token with a minimum contrast ratio of 3:1 against both light and dark surfaces. High-contrast mode uses system colors through the theme layer, not special-case drawing.

### Command names

Every `CommandSpec` title key resolves to a localized accessible name. Toolbar buttons, palette items and context actions reuse the same name so screen readers announce identical text everywhere.

### Test hooks

A test-only accessor dumps the semantic tree as JSON for golden tests in the RecordingBackend. Narrator and NVDA smoke scripts are documented and run manually before release; their expected announcements are checked into `tests/a11y/`.

## Minimum verification scenarios

- Narrator reads tab names with dirty and pinned state, menu items, palette results, settings rows with their keys, and the caret line and column.
- NVDA announces search result count changes and banner text through live regions.
- Keyboard-only smoke pass reaches every FR-030 surface: menus, tabs, sidebar tree, bottom panel, settings, palette, find bar, status bar.
- Text pattern on a 1 GB document returns only bounded ranges and never triggers a full line-index scan.
- Focus returns to the anchor after closing a popover, palette or dialog in every case.
- Semantic tree JSON golden test for the default layout and for a layout with all panels open.
- High-contrast theme renders every surface with readable text and a visible focus ring.

## Implementation sequence

1. Mark `PR-024` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Every FR-030 item has a test or a documented manual script with expected output
- [ ] No accessibility call reads more than the visible range plus overscan
- [ ] Keyboard-only pass completes with no unreachable control
- [ ] AccessKit adapter is created lazily and costs nothing when no client is attached
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-024: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-030. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: AccessKit tree; UI Automation; assistive workflow. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-024-01** — Use Narrator through open, edit, find, conflict and recovery; names, roles, values and focused state are announced.
- [ ] **AC-024-02** — Change selection/scroll in a 5 GiB file; semantic text-range requests are bounded and do not materialize the entire document.
- [ ] **AC-024-03** — Enable high contrast and test focus/selection/error distinction without color; real DirectWrite results supplement headless tests.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
