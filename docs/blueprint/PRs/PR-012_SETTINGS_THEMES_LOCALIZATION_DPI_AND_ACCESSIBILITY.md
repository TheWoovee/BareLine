# PR-012 — Settings, Themes, Localization and DPI

**Tracker row:** `PR-012` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-001`, `PR-003`, `PR-011`, `PR-023`  
**Primary objective:** Provide the polished, searchable configuration and visual system that removes legacy preference clutter while preserving power. Accessibility and UI Automation are PR-024 (ADR-06).

## Agent contract

This brief includes local scope and must be read with its assigned v1.3 foundation sections and acceptance cases below. Implement the scope below without scanning the whole documentation directory. Read a dependency PR only when you need the exact merged interface. If a requirement here conflicts with code already merged, preserve the public product intent and make the smallest compatible contract adjustment rather than inventing a parallel architecture. If an architecture question arises, read `../07_DECISION_LOG.md`; it overrides this file.

## Architecture snapshot (do not redesign casually)

- Rust 2024 Cargo workspace. MPL-2.0 core; MIT OR Apache-2.0 extension SDK and first-party extensions.
- `winit 0.30.x` owns portable window/input events; Windows rendering uses Direct2D/DirectWrite (hardware or software mode) behind a neutral `RenderBackend`; a `RecordingBackend` exists for headless tests. The document is **never** stored in a toolkit text widget.
- `bareline-document` owns the resident-or-paged `ByteSource` (no mmap in v1), piece tree, snapshots, revisions, the `DocumentService` actor and edit transactions. The valid UTF-8 text view and original byte domain are distinct; undecodable spans retain original bytes through provenance metadata, and large transcodes use disk-backed storage (FC-01/02).
- Canonical editor positions are TextOffset values into the valid UTF-8 text view; RawOffset addresses original bytes and is never implicitly interchangeable (FC-01). Line, column and grapheme are derived views.
- UI thread never performs unbounded file I/O, regex, syntax indexing or extension RPC. No async runtime in the editor process.
- Third-party extensions never load inside `bareline.exe`; the extension host is an optional signed download.
- Windows-specific code belongs in `platform-windows`; cross-platform core crates must remain OS-neutral. Native Win32 menus/dialogs are reached only through `PlatformServices`.
- Every user-visible command has a stable command ID.
- Persistent formats are versioned and use atomic writes/migrations: TOML for human-edited files, JSON for machine-written state, binary CRC32C journals.
- Performance is designed in: the startup budget, the dependency policy and continuous `xtask perf` measurement apply to every PR; performance numbers are targets, never merge gates. See `../07_DECISION_LOG.md`.

## Scope to implement

- Versioned settings schema/TOML persistence
- Searchable settings UI
- System/Light/Dark themes
- Editor color theme schema
- High-contrast behavior
- Toolbar/tab/editor visual preferences
- Localization resource system; English ships in v1, community packs accepted (ADR-28)
- Runtime/restart language switch policy
- DPI scaling tests
- Settings rows show their TOML key with a copy action (FR 5.4)

## Explicit non-goals

- No online theme marketplace
- No cloud sync

## Likely files / ownership

- `crates/settings/**`
- `ui-assets/themes/**`
- `crates/ui/src/settings/**`
- `locales/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Settings model

Use typed settings structs with defaults and schema version. Layer resolution: compiled defaults < global file < optional workspace < session-only override. A setting definition includes stable key, type/range, category, restart requirement and description key.

Settings, themes and keymaps are TOML edited through `toml_edit` so user comments survive; session and other machine-written state are JSON (ADR-21). Write TOML atomically. Preserve unknown keys during migrations where safe so downgrade/extension data is not destroyed. Invalid values fall back only for that key and surface diagnostics; one bad setting must not reset the entire profile.

### Settings UI

Built from PR-023 controls. Search by title/description/key. Every setting row displays its TOML key and offers a copy action so power users can edit `settings.toml` directly (FR 5.4). Categories are shallow. Advanced settings are collapsed, not hidden in separate XML. Changes with safe live effect apply immediately with revert support; expensive/restart-required options state that clearly.

### Theme

Separate chrome tokens from syntax tokens. Themes define semantic colors (`surface`, `surface.chrome`, `text`, `text.muted`, `accent`, `selection`, `caret`, `border`, plus `diff.added`, `diff.removed`, `diff.changed`, `diff.moved`, `diff.current` and their gutter/overview variants), not widget-specific RGB scattered in code. The built-in Light and Dark themes use the brand tokens defined in `../00_BRAND_AND_LOGO.md` (ADR-29). Validate contrast for essential states and integrate Windows system light/dark/high-contrast preference.

### Localization

Use stable message IDs and parameterized strings. Never concatenate fragments whose word order matters. Support fallback to English. Locale files are data-only and cannot execute formatting code. English is the only shipped locale in v1; the resource system, fallback and switching are complete so community packs can be contributed through the open-source repository without code changes (ADR-28).

### DPI

All layout metrics are logical pixels; renderer gets scale. Text follows user/system scaling within practical editor controls. Per-monitor DPI changes re-layout without recreating documents or views. The semantic/accessibility tree is PR-024.

### Font/settings performance

Changing editor font/theme invalidates visible layout caches and schedules progressive repaint; it must not trigger synchronous full-file re-layout.

## Minimum verification scenarios

- Settings schema migrations preserve unrelated/unknown keys and survive malformed single entries.
- Runtime light/dark/high-contrast switch remains readable.
- 100/125/150/200/300% DPI resize produces stable hit targets/no blurry text.
- Every setting row displays its TOML key and a copy action.
- Locale with long strings/RTL does not clip critical actions.

## Implementation sequence

1. Mark `PR-012` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] No setting requires XML editing
- [ ] Unknown future settings keys are preserved where feasible
- [ ] Main window remains readable at 300% scaling
- [ ] Dark theme covers dialogs/panels consistently
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-012: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.3 delivery slices and acceptance

**Normative contracts:** FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-013, FR-023, FR-024. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

| Slice name | What compiles and is testable at slice end | AC cases or minimum scenarios that prove it |
|---|---|---|
| Settings scopes | User and allowlisted workspace settings resolve effective values with atomic persistence. | AC-012-02; reject workspace execution and extension grants. |
| Theme tokens | Light/Dark/System selection and functional contrast overrides load through one schema. | AC-012-03; restore token overrides after restart. |
| Units and persistence | Point-based editor font preview converts consistently across mixed-DPI windows. | AC-012-01; persist 12 pt and move between scale factors. |

- [ ] **AC-012-01**: Change font size to 12 pt at mixed DPI; preview, persisted value and restored editor use the same point unit.
- [ ] **AC-012-02**: Attempt workspace override of execution, network or extension grants; reject it while accepting allowed indentation preferences.
- [ ] **AC-012-03**: Check Light/Dark/System tokens and override persistence; normal text meets 4.5:1 and focus indicators 3:1 on actual surfaces.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
