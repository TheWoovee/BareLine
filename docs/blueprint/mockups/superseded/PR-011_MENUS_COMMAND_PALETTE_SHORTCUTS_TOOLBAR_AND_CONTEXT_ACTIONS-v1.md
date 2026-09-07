# PR-011 — Menus, Command Palette, Shortcuts, Toolbar and Context Actions

**Tracker row:** `PR-011` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-001`, `PR-003`, `PR-023`  
**Primary objective:** Turn the command registry into the complete discoverability layer: familiar menus plus searchable palette and configurable shortcuts without UI clutter.

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

- Full menu taxonomy
- Ctrl+Shift+P command palette with fuzzy search/synonyms
- Shortcut mapper with conflict detection
- Toolbar model and visibility/customization
- Editor/tab/tree context menus
- Command enabled/checked state model
- Import/export keymap
- Accessibility names for commands

## Explicit non-goals

- No new editor behaviors except command wiring
- No extension-contributed commands yet; reserve API

## Likely files / ownership

- `crates/commands/**`
- `crates/ui/src/command_palette/**`
- `crates/ui/src/menu/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Command registry as single source of truth

PR-011 owns `crates/commands` (ADR-15). Other PRs never edit this crate to add commands; each contributing crate exposes `pub fn register_commands(registry: &mut CommandRegistry)` and the app composition root calls them in a fixed order. Duplicate IDs fail in debug and tests.

Every built-in action is registered once. Menu, toolbar, context menu, palette, keymap and macro recorder reference `CommandId`; none stores executable closures independently. `CommandContext` exposes read-only app/document/view state plus dispatch handles.

### Enable/check state

Commands provide state selectors such as enabled/disabled, checked and optional dynamic label. Compute state from immutable app snapshot; do not perform filesystem/search work to decide whether a menu item is enabled.

### Keymap

Represent key chords in platform-neutral logical form plus physical-key option for users who need layout-independent bindings. Default Windows shortcuts mimic Notepad++ where sensible. Detect exact/prefix conflicts during edit/import. Dispatch rules must define text input vs shortcut precedence, especially AltGr and IME.

### Menus/context/toolbar

Build a declarative menu model from command IDs and separators/submenus. On Windows the menu bar and context menus are native HMENU projections of that model created through `PlatformServices` (ADR-04); enabled/checked state and shortcut labels are pushed into the native menu whenever the app snapshot changes. The palette, toolbar and shortcut mapper are custom-rendered with PR-023 controls. Persistence never depends on localized labels. Toolbar is optional/compact, hidden by default, and uses the same command IDs. Context menu is filtered by context but does not create duplicate commands.

### Command palette

Index title, category, menu path, keywords/synonyms and shortcut. Search is local and small enough to be immediate. Recent command weighting may be in-memory/local only. Palette shows disabled reason where useful and never exposes internal debug commands in release unless enabled.

### Focus routing

Commands target the active window/view/document determined at dispatch time. Modal popovers/settings intercept only commands they own; global close/escape semantics remain predictable.

## Minimum verification scenarios

- Assert every menu/toolbar/keybinding target resolves to exactly one registered command.
- Duplicate command IDs and key conflicts surface deterministic diagnostics.
- AltGr/IME text input is not swallowed as Ctrl+Alt shortcuts.
- Change UI language and saved shortcuts/macros still work because IDs are stable.
- Palette finds commands by title, synonym and menu path under 10 ms on normal catalog.
- Native menu bar and command palette are generated from the same menu model and show identical shortcuts.

## Implementation sequence

1. Mark `PR-011` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Every visible action resolves to stable command ID
- [ ] Palette executes actions and shows shortcuts
- [ ] Shortcut conflicts are explicit, not last-write-wins
- [ ] Menus remain usable at 100–300% scaling
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-011: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-023. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Registry projections; keyboard contract; discoverability. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-011-01** — Every menu/toolbar/palette action resolves to one stable ID; unavailable actions have a reason and no handler mutation.
- [ ] **AC-011-02** — Press Escape in nested popup, panel and editor states; only the topmost transient layer closes and focus returns predictably.
- [ ] **AC-011-03** — Test AltGr text, menu mnemonics and remapped shortcuts together; text entry is not consumed as a global shortcut.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
