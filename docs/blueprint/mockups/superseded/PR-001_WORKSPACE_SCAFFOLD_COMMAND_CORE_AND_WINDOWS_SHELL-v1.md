# PR-001 — Workspace Scaffold, Command Core and Windows Shell

**Tracker row:** `PR-001` in `../03_PR_TRACKER.md`  
**Depends on:** None. This is a root/foundation PR.  
**Primary objective:** Create the Cargo workspace, winit application shell, Bareline UI/render contracts, Windows Direct2D/DirectWrite backend in hardware and software modes, `RecordingBackend` for headless tests, command registry, platform abstraction, native-chrome surfaces, diagnostics skeleton, `xtask perf` skeleton, repository hygiene files and a runnable Windows editor window that respects the startup budget.

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

- Cargo workspace and crate boundaries
- Winit main window with Bareline menu/tab/editor/status placeholder views
- Platform-neutral retained UI tree/layout primitives
- `RenderBackend`/`Painter` contract
- Windows Direct2D/DirectWrite renderer bootstrap and device-loss-safe frame loop, in hardware mode and in software render-target mode, selectable by setting `renderer.mode` (ADR-32)
- `RecordingBackend` headless renderer with deterministic monospace metrics for UI tests on every OS (ADR-07)
- CommandRegistry with immutable command IDs and default keybindings
- PlatformServices trait plus minimal Windows implementation, including native Win32 menu bar, context menu, message box and file/folder dialog surfaces; the menu bar is a projection of the command registry (ADR-04)
- Startup budget enforcement with a debug assertion that lists everything that ran before the first frame (ADR-33)
- `xtask` crate with `xtask perf` skeleton: cold launch to first frame and idle private bytes after 10 s, written as JSON to `tests/perf/results/` (ADR-10)
- Nightly GitHub Actions perf job that appends results and opens a tracking issue on a regression above 10 percent
- Structured logging with content-redaction rule
- CI for fmt/clippy/tests/`cargo deny` on Windows plus core-crate compile and RecordingBackend UI tests on Linux/macOS
- Repository hygiene: `LICENSE` (MPL-2.0), `LICENSE-SDK` note (MIT OR Apache-2.0 for SDK crates), `CONTRIBUTING.md` with DCO sign-off, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `.github` issue and PR templates, `deny.toml`, `rust-toolchain.toml`
- Release profile: `panic = "abort"`, `lto = "fat"`, `codegen-units = 1`, symbols separated into a PDB (ADR-08)

## Explicit non-goals

- Do not implement document editing beyond a temporary in-memory placeholder
- No UI controls beyond the retained-tree foundation; controls are PR-023
- No document editing beyond the placeholder
- Do not add extensions, search, syntax or installer yet

## Likely files / ownership

- `Cargo.toml`
- `rust-toolchain.toml`
- `deny.toml`
- `LICENSE*`
- `CONTRIBUTING.md`
- `SECURITY.md`
- `CODE_OF_CONDUCT.md`
- `.github/**`
- `xtask/**`
- `crates/app/**`
- `crates/platform/**`
- `crates/platform-windows/**`
- `crates/renderer/**`
- `crates/renderer-recording/**`
- `crates/ui/**`
- `crates/diagnostics/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Workspace and dependency direction

Create the dependency layers in this order and keep arrows one-way:

```text
platform <- platform-windows
renderer <- platform-windows
renderer <- renderer-recording
commands <- app
settings <- app
ui -> renderer + commands + settings
app -> ui + platform + commands + diagnostics
apps/bareline -> app + platform-windows
xtask -> (dev-only) process spawning and result files
```

`document`, `search`, `syntax`, `session`, `diff` and extension crates may exist as empty compile targets here, but PR-001 must not invent their later business behavior.

Dependencies are limited to the approved list in ADR-09. `deny.toml` bans the forbidden crates (`tokio`, `async-std`, `smol`, `egui`, `iced`, `gpui`, `druid`, `reqwest`, `openssl`, `resvg`, `image`) and enforces license and advisory checks.

### Window/event shell

- Use stable `winit 0.30.x`; run with `ControlFlow::Wait`, not a polling loop.
- One `ApplicationHandler` owns top-level windows and forwards `WindowEvent`/IME/keyboard/pointer/DPI events into platform-neutral `UiEvent` values.
- The main window starts with a native menu bar, empty tab strip, editor placeholder and one-line status bar. No start dashboard.
- Redraw only after invalidation, animation or OS exposure. Do not request continuous redraw while idle.
- Normalize physical/logical coordinates once at the event boundary; core UI layout uses logical pixels and renderer receives device scale explicitly.

### Startup budget

Before the first frame the process may: parse the CLI, read `settings.toml`, read the keymap, read the session manifest header, create the window and renderer. It may not: restore tabs, read documents, enumerate fonts beyond resolving the editor font family, rasterize icons for hidden toolbars, touch the network, start the extension host, or read recent-file metadata. Implement a `StartupLedger` that records every file open, thread spawn and network attempt before the first frame; in debug builds an assertion fails and prints the ledger if a forbidden action appears. The ledger is also written to the diagnostics log in release so `xtask perf` can attribute launch cost.

### Performance instrumentation

- `xtask perf launch` starts `bareline.exe` with an empty session N times, reads the first-frame timestamp from the diagnostics log or a named event, samples private bytes 10 s after first frame, and writes JSON with machine, OS, build hash, renderer mode and raw samples to `tests/perf/results/`.
- `tracing` spans exist for startup phases and each frame. In release builds instrumentation is compiled to a no-op unless `--features perf-spans` is set; the check for the feature must not cost a branch in the hot paint path.
- The nightly workflow runs `xtask perf launch` on the Windows runner in both renderer modes, appends results, compares against the previous seven runs and opens or updates a tracking issue when P50 regresses more than 10 percent. It never fails a PR build.

### Renderer contract

Define a minimal `RenderBackend` plus frame-scoped painter. Required primitives: fill/stroke rectangle, line, clip stack, simple image/icon, text shape/layout, text draw, opacity/layer, frame begin/end, resize and device-recovery signal. Keep backend-owned text handles opaque.

### Renderer modes

- **Hardware mode (default):** Direct2D device context on a DXGI swap chain. Create device resources lazily on first paint, not at window creation.
- **Software mode (mandatory):** Direct2D software render target (`D2D1_RENDER_TARGET_TYPE_SOFTWARE`) with DirectWrite text. Same `RenderBackend` implementation, different target creation. Selected by `renderer.mode = "software"`, and used automatically if hardware creation fails.
- **RecordingBackend (tests):** records every draw command into a `Vec<DrawOp>` and shapes text with deterministic monospace metrics (fixed advance, fixed line height). It compiles on every OS and is the backend for UI model, layout, hit-test and keybinding tests.

Windows implementation must:

- obtain the `HWND` from winit;
- create Direct2D/DirectWrite factories/device resources lazily;
- recreate only device-dependent resources after device loss;
- cache brushes/text formats by stable keys rather than recreating them per glyph;
- resolve only the configured editor font family at startup; do not enumerate the system font collection eagerly;
- never expose COM pointers outside `platform-windows`/renderer implementation.

### Native chrome

`PlatformServices` exposes `MenuHost` (build and update a native menu bar and context menus from a declarative `MenuModel` of command IDs), `Dialogs` (message box, open file, save file, pick folder) and `ShellOpen`. The Windows implementation uses HMENU and `IFileOpenDialog`/`IFileSaveDialog`. Menu item labels come from the localization layer; the menu model is data derived from the command registry, never the owner of commands. Everything else (tab strip, editor, status bar, panels, palette, settings) is custom-rendered through `RenderBackend`.

### Retained UI core

Implement only foundation primitives needed by later PRs: `ViewId`, `Rect`, layout node, focus chain, invalidation region, hit-test result, semantic/accessibility node, `UiEvent`, `CursorIcon`, and a small `View` trait/state model. Do not build a generic scene graph framework. Concrete controls are PR-023.

### Command core

Use an immutable string ID newtype such as `CommandId("file.open")`. `CommandSpec` contains title key, category, default shortcuts, enable predicate and handler reference. Register at startup and reject duplicate IDs in debug/tests. The placeholder shell must already execute `app.quit`, `file.new`, `help.about` and command-palette placeholder through the registry, proving menus are consumers rather than owners of commands. Other crates contribute commands through a `register_commands(&mut CommandRegistry)` function (ADR-15).

### Diagnostics

Use structured local logs with rotation and no document content. Capture version/build hash, backend init failures, renderer mode, the startup ledger and panic metadata. Disable telemetry/network calls entirely.

### Panic and crash policy

Release builds use `panic = "abort"`. Install a panic hook that writes version, build hash, panic location and renderer state to the diagnostics log and then aborts. Debug builds unwind. No document content ever enters the crash record.

### Repository hygiene and license headers

Every source file in core crates starts with `// SPDX-License-Identifier: MPL-2.0`. SDK/protocol crates use `// SPDX-License-Identifier: MIT OR Apache-2.0`. `CONTRIBUTING.md` documents DCO sign-off (`git commit -s`), the dependency policy, the startup budget and the tracker workflow. `SECURITY.md` documents private vulnerability reporting and the update signing key procedure. `.github/` contains bug, feature and performance-regression issue templates and a PR template with the dependency-justification field.

### Build profiles

Create Windows x64 release packaging profile with symbols separated from distributable artifacts. Use `panic = "abort"`, `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`. Do not use size flags that materially damage hot editor loops without benchmark evidence.

## Minimum verification scenarios

- Launch and close 100 times without orphan processes or increasing persisted state.
- Leave idle for 60 seconds and verify no continuous redraw/timer wake loop.
- Resize/DPI-change the window repeatedly; UI remains crisp and no renderer resource leak is visible.
- Force Direct2D device recreation path in a test/mock and confirm the window survives.
- Software render mode launches when hardware creation is blocked in a test.
- Duplicate command ID test fails deterministically.
- Neutral crates compile on Windows, Linux and macOS CI without importing Win32 types.
- `xtask perf launch` runs twice and produces comparable JSON with the same schema.
- RecordingBackend UI tests run on Linux and macOS CI.
- `cargo deny check` passes.
- Startup-budget assertion fails deterministically if a document read is injected before first frame.

## Implementation sequence

1. Mark `PR-001` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Application starts to an empty editor window
- [ ] Ctrl+N/Ctrl+O/Ctrl+S command IDs dispatch to stub handlers without panics
- [ ] No core crate imports windows-rs
- [ ] CI demonstrates platform-neutral crates compile on three desktop OS families
- [ ] `xtask perf launch` produces JSON results and the nightly perf workflow exists
- [ ] `RecordingBackend` runs UI tests headlessly on all three OS families
- [ ] Hardware and software renderer modes both launch and are selectable by setting
- [ ] Startup ledger shows no forbidden work before the first frame
- [ ] `cargo deny check` passes
- [ ] LICENSE, LICENSE-SDK, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, deny.toml and .github templates are present
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-001: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-06, FC-07, FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-030, FR-031. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Foundation contracts; Windows text/input prototype; command and render scaffold. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-001-01** — Render mixed Arabic/Latin with emoji and IME composition; candidate window follows caret at 100%, 150% and 200% DPI.
- [ ] **AC-001-02** — Open 5,000 lazy tabs; worker count remains bounded and no document reads occur before the first frame.
- [ ] **AC-001-03** — Verify codec, filesystem-capability and VerifiedPackageSource interfaces compile with fake providers; online provider fails closed.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
