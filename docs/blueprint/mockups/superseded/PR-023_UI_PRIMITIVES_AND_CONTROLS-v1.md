# PR-023 — UI Primitives and Controls

**Tracker row:** `PR-023` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-001`  
**Primary objective:** Deliver the reusable custom-rendered control set that every later UI PR consumes (ADR-05): buttons, toggles, text field, combo, slider, virtual list and tree, scrollbar, splitter, tab strip primitive, tooltip, popover, banner, focus and keyboard navigation model, token-based theming and semantic node emission.

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

- Control state model and event routing on top of the PR-001 retained tree
- Button, toggle, checkbox, radio group
- Combo/dropdown with keyboard search
- Single-line text field with IME, selection, clipboard and undo
- Slider and numeric stepper
- Virtual list and virtual tree
- Scrollbar with proportional thumb and unknown-total support
- Splitter
- Tab strip primitive (rendering and hit testing only, no tab model)
- Tooltip, popover/anchored panel, banner/inline notification
- Focus ring, focus chain and keyboard navigation model
- Semantic token theming and DPI-aware metrics
- Semantic node emission per control for PR-024

## Explicit non-goals

- No document editor behavior; the editor surface is PR-003
- No menus; menu bar and context menus are native (ADR-04)
- No settings pages, search panel or palette; those PRs compose these controls
- No animation framework beyond simple, interruptible transitions

## Likely files / ownership

- `crates/ui/src/controls/**`
- `crates/ui/src/theme/**`
- `crates/ui/src/focus/**`
- `crates/ui/tests/**` (RecordingBackend golden tests)

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Control state model

Each control is a `View` with explicit `ControlState { enabled, focused, hovered, pressed, checked, invalid }` and an immutable props struct. State transitions happen only through `UiEvent` handlers and return `Invalidate(region)`; controls never call the renderer directly outside `paint`. Controls hold no application data; they emit typed messages (`Activated`, `Changed(value)`, `Dismissed`) to the owning view.

### Event routing

PR-001 delivers normalized `UiEvent`. Routing order: capture by an open popover, then focused view, then hit-test target, then ancestors. Popovers dismiss on outside click, Escape and focus loss, with an explicit `sticky` option for panels. Keyboard: Tab/Shift+Tab walk the focus chain, arrows move within groups (radio, list, tree, tab strip), Space/Enter activate. Every rule is a table-driven test.

### Text field

The single-line text field is a tiny editor: grapheme-aware caret (`unicode-segmentation`), selection by mouse and keyboard, cut/copy/paste through `PlatformServices` clipboard, bounded undo stack, placeholder, validation state, and IME preedit as an overlay that never enters undo until commit. Dead keys and AltGr must produce text, not shortcuts. Provide a `password` mode and a `numeric` filter. Long values scroll horizontally without allocating a layout for the whole string per frame.

### Virtualization contract

Virtual list and tree share one contract: `ItemSource { len() -> Option<usize>, item_height(index) -> Px, range(start, count) -> Iterator<Item> }`. Only visible items plus overscan are laid out. `len() == None` means unknown total; the scrollbar then uses estimated extent and refines as items are discovered. Tree nodes expand lazily through `children(node) -> Option<Vec<NodeId>>` so a 1M-entry folder never enumerates beyond what is visible.

### Scrollbar

Scrollbar maps an `f64` logical range to thumb pixels, supports unknown totals, minimum thumb size, page/step clicks, drag with deferred commit when the owner asks, and no animation loop when settled. It is the same component the editor surface (PR-003) and every panel use.

### Popover placement

Anchored to a rect with preferred side and automatic flip when space is insufficient; clamped to the window; arrow optional. Dismissal returns focus to the anchor.

### Theming

Controls read semantic tokens only: `surface`, `surface.elevated`, `text`, `text.muted`, `accent`, `selection`, `border`, `focus`, `danger`, `warning`, `success`. Token values come from the theme layer and the brand tokens in `../00_BRAND_AND_LOGO.md`. No control contains an RGB literal. High-contrast mode swaps the token set, not the control code.

### DPI

All metrics are logical pixels; the renderer receives scale. Control heights, paddings and hit targets are defined in one metrics table with a compact and a comfortable density.

### Semantic nodes

Every control emits a `SemanticNode { role, name, value, states, actions, bounds }` from its current props and state. PR-024 turns these into UI Automation. Names come from localized keys, never from painted text alone.

## Minimum verification scenarios

- RecordingBackend golden tests for every control in light, dark and high-contrast at 100, 150, 200 and 300 percent scale.
- Text field IME preedit then commit produces exactly one value change; preedit cancel produces none.
- Text field handles dead keys and AltGr on a German and a US-International layout without triggering shortcuts.
- Virtual list over a 1M-item source keeps constant memory and completes a 60 Hz scroll workload in the recording backend without laying out invisible items.
- Virtual tree over a lazily enumerated 1M-node fixture enumerates only expanded levels.
- Scrollbar with unknown total refines its thumb as the source reports more items, with no jump larger than one thumb height per update.
- Keyboard traversal order test across a form of every control type matches the declared focus chain.
- Popover flips side when anchored near the window edge and returns focus to the anchor on dismiss.

## Implementation sequence

1. Mark `PR-023` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Every control has a RecordingBackend golden test and emits a semantic node
- [ ] No control performs file, network or process I/O
- [ ] Text field handles IME, dead keys and AltGr correctly
- [ ] Virtual list and tree lay out only visible items plus overscan
- [ ] No RGB literal exists in control code; tokens only
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-023: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-023, FR-030. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Control primitives; keyboard semantics; DPI focus. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-023-01** — Navigate every control with Tab/Shift+Tab/arrows; focus is visible and disabled controls do not dispatch edits.
- [ ] **AC-023-02** — Use IME, clipboard and validation in text fields; cancellation returns focus without leaking preedit into undo.
- [ ] **AC-023-03** — At 100/125/150/200/250% DPI, hit targets align with paint and popovers stay on-screen.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
