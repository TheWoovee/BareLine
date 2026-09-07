# PR-014 — Macros, External Commands and Output Panel

**Tracker row:** `PR-014` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-006`, `PR-011`, `PR-023`  
**Primary objective:** Implement stable-command macros and safe external command execution with optional captured output.

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

- Macro recorder tied to command/edit transactions
- Play once/N times/until EOF
- Save/rename/import/export/shortcut macros
- Human-readable versioned macro format (TOML, ADR-21)
- Ghost/replay typing delay action
- External command definitions and placeholders
- Direct process spawn + optional shell mode via `std::process` on a dedicated thread (ADR-09)
- Cancellable process and stdout/stderr output panel built on the PR-023 virtual list

## Explicit non-goals

- No terminal emulator
- No build system

## Likely files / ownership

- `crates/commands/**`
- `crates/app/src/process*`
- `crates/ui/src/output/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Macro event model

Recorder listens to executed `CommandId` plus serializable arguments/edit text after command normalization, not raw mouse coordinates wherever avoidable. Macro file is versioned, human-readable TOML (`macros/*.toml`, ADR-21) with a `format_version` key. Non-deterministic commands (open dialog without recorded path, extension UI) are either parameterized explicitly or marked non-recordable.

### Playback

Playback dispatches through the same command registry. Support once, N times and until-EOF with cancellation and a maximum-iteration safeguard. One macro run may be grouped as one undo unit per document where practical, but long multi-document macros should preserve bounded undo memory.

### External command definition

Use `program` + `args[]` first; shell command is a separate explicit mode. Placeholders (`${file}`, `${dir}`, `${selection}`, `${line}`, `${column}`, `${workspace}`) are expanded as arguments without shell re-parsing in direct mode. Unsaved-document placeholders have defined empty/temp/refuse behavior.

### Process execution

Spawn through `std::process::Command` on a dedicated process-supervisor thread; do not introduce an async runtime (ADR-09). Capture stdout/stderr on two reader threads with a bounded ring buffer/spill-to-temp policy, support cancellation/kill tree where possible and show exit code/duration. Do not deadlock by filling pipe buffers.

### Output panel

Built on the PR-023 virtual list control. Virtualized lines, ANSI handling only if explicitly supported, link detection for `path:line:col` through safe parser, clear/copy/save actions. Output is not automatically executed or interpreted as HTML.

### Ghost/replay typing

Implement as a macro action that emits text at controlled intervals for demos/tests only. It must be cancelable and must not bypass normal edit transactions/recovery.

## Minimum verification scenarios

- Record/edit/search/navigate macro, restart app, replay deterministically.
- Until-EOF macro stops on no progress/EOF and can be canceled.
- External process emitting >1 GB output cannot grow editor memory without bound or deadlock.
- Arguments containing spaces/quotes/metacharacters are passed correctly in direct mode without shell injection.
- Cancel long-running process and ensure UI/output remain responsive.

## Implementation sequence

1. Mark `PR-014` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Recorded macro replays deterministic operations
- [ ] Dangerous shell interpolation requires explicit configuration/confirmation
- [ ] Hung child process can be terminated
- [ ] Macro references command IDs, not localized names
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-014: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-06, FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-019, FR-020. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Macro transactions; safe process spawn; output limits. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-014-01** — Record/replay only stable command IDs and explicit text; playback failure stops with a resumable location.
- [ ] **AC-014-02** — Run a path and argument containing spaces, quotes and shell metacharacters using direct spawn; no implicit shell interpretation occurs.
- [ ] **AC-014-03** — Cancel a process producing unlimited output; output storage stays bounded, process cleanup completes and the editor remains usable.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
