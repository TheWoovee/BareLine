# PR-022 — Cross-Platform Readiness and Linux/macOS Adapter Skeletons

**Tracker row:** `PR-022` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-001`  
**Primary objective:** Continuously enforce portability while Windows ships first: core compile/tests on all desktop OSes plus minimal platform adapter skeletons that prove boundaries are real.

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

- Cross-platform core CI
- No-Windows-import lint/check policy for neutral crates
- Linux/macOS PlatformServices adapter skeleton crates
- Linux/macOS CI runs of the shared UI tests on the PR-001 `RecordingBackend` (ADR-07)
- Optional real software painter for Linux/macOS previews, only if cheap
- Path/shortcut/menu abstraction tests
- Portable language/settings/session tests on all OSes
- Documented porting checklist for native menus/dialogs (ADR-04), packaging, shell, printing, tray and update

## Explicit non-goals

- Do not ship Linux/macOS GUI release in Windows v1
- Do not force platform behavior to look identical when native conventions differ

## Likely files / ownership

- `crates/platform-linux/**`
- `crates/platform-macos/**`
- `.github/workflows/**`
- `crates/platform/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### What this PR proves

This PR is not a Linux/macOS product port. It is an architectural test that Windows-first work is not contaminating reusable cores.

### CI matrix

Build/test neutral crates on Windows MSVC, current macOS and a mainstream Linux runner from the beginning. Add a dependency/lint rule or architectural test that prevents `windows`/Win32 imports from neutral crates. Platform-specific crates may be cfg-gated.

### Platform interfaces

Create compile-complete skeletons for `platform-linux` and `platform-macos` implementing trait surfaces with `Unsupported` only for capabilities not needed by core tests. The headless renderer already exists: PR-001 ships `RecordingBackend` in `crates/renderer-recording` (ADR-07), which records draw commands and uses deterministic monospace metrics. This PR wires it into the Linux and macOS CI jobs so the shared UI, layout, hit-test and keybinding tests run on all three OSes, and may add an optional real software painter for visual previews only if it costs little. Do not fake fully working printing/tray/update. Native menu bar, context menus and file dialogs are `PlatformServices` capabilities (ADR-04); the skeletons return `Unsupported` for them until a real port.

### Path/keyboard conventions

Core uses `PathBuf`/`OsString`; serialized display forms do not become canonical paths. Shortcut model has semantic modifiers (Primary/Alt/Shift/Super) and platform mapping. Menus/tray/file dialogs/printing/updater remain `PlatformServices` capabilities.

### Shared UI

The retained Bareline view/layout/editor code must compile and its tests must pass against `RecordingBackend` independent of the Windows renderer. Winit event normalization stays portable. Windows-specific hit-test/titlebar behavior is an adapter. Any new core PR after this should keep CI green; PR-022 is a continuing guardrail, not a one-time branch.

### Porting checklist

Document concrete remaining work for Linux (Wayland/X11 renderer, native or GTK menus and dialogs through `PlatformServices` per ADR-04, file watch, shell, packaging, updater) and macOS (CoreGraphics/Metal renderer choice, native NSMenu and NSOpenPanel through `PlatformServices`, sandbox/signing/notarization, file watch, packaging/updater). Choose actual renderer backends only after a prototype benchmarks text quality/RSS/startup against requirements.

## Minimum verification scenarios

- Neutral crate graph builds/tests on all three OSes.
- Public APIs contain no HWND/HANDLE/DirectWrite/UTF-16-Windows-path assumptions.
- The shared UI test suite runs on Linux and macOS CI against `RecordingBackend`; a trivial Linux/macOS winit window can drive the shared `UiEvent` and render contract in an example target.
- Windows-specific feature addition intentionally fails architecture check if imported into neutral crate.
- Porting checklist names every `PlatformServices` function still unsupported on each OS.

## Implementation sequence

1. Mark `PR-022` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Core document/search/session/syntax crates test on Windows/Linux/macOS
- [ ] No Win32 type leaks into public core APIs
- [ ] Shared UI tests pass on Linux/macOS CI against the PR-001 `RecordingBackend`, and a minimal winit shell using it compiles there when CI runners support it
- [ ] Porting checklist identifies remaining platform-only work
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-022: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.3 delivery slices and acceptance

**Normative contracts:** FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-030, FR-031. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

| Slice name | What compiles and is testable at slice end | AC cases or minimum scenarios that prove it |
|---|---|---|
| Portable contracts | Neutral crate compile targets expose explicit Unsupported adapter gaps. | AC-022-01; compile on Linux/macOS without Windows imports. |
| Lossless paths | Tagged platform path serialization round-trips native byte/code-unit identities. | AC-022-02; preserve non-Unicode paths across state reload. |
| Platform skeletons | RecordingBackend integrates with OS adapter skeletons and separate readiness reports. | AC-022-03; distinguish headless coverage from real-platform readiness. |

- [ ] **AC-022-01**: Compile neutral crates without Windows imports on Linux/macOS; platform gaps return explicit Unsupported.
- [ ] **AC-022-02**: Round-trip unpaired Windows UTF-16 and Unix non-UTF-8 path encodings through versioned state without lossy identity conversion.
- [ ] **AC-022-03**: Run RecordingBackend tests on each OS and record separate real-platform readiness rather than claiming complete ports.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
