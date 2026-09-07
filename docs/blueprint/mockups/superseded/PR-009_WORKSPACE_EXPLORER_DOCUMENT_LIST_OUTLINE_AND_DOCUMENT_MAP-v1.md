# PR-009 — Workspace Explorer, Document List, Outline and Document Map

**Tracker row:** `PR-009` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-003`, `PR-008`, `PR-023`  
**Primary objective:** Add optional navigation panels while keeping the editor dominant: folder-as-workspace, document list, outline/function list and progressive document map.

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

- Workspace root model and lazy tree enumeration
- Create/rename/delete file/folder actions
- Document list with sort/filter/activate/save/close
- Outline extraction contracts and language definitions
- Notepad++ `functionList` parser definition importer with imported/approximated/unsupported report (ADR-22)
- Follow-caret and outline filter
- Progressive document map/minimap
- Panel docking/toggle state
- Workspace-specific settings opt-in path

## Explicit non-goals

- No Git/status decorations
- No project build system

## Likely files / ownership

- `crates/workspace/**`
- `crates/ui/src/sidebar/**`
- `crates/ui/src/outline/**`
- `crates/ui/src/document_map/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Workspace model

A workspace is a list of root folders plus optional workspace settings. Opening a folder does not create `.bareline` files. Tree nodes are lazy: enumerate a directory only when expanded or needed by search/indexing.

### File tree

Use the PR-023 virtual tree control. The tree model is keyed by stable path/node IDs. File operations call `PlatformServices`/file service and handle races (file disappeared, permission denied, case-only rename). Watcher events update nodes incrementally instead of full-tree rescans.

### Document list

PR-023 virtual list of open tabs/documents with filter/sort and actions. It reads tab/session metadata, not filesystem repeatedly. It must stay responsive with thousands of open documents.

### Outline

Consume revisioned outline symbols from syntax service. Symbols contain stable kind/name/range/nesting. Results may be partial; UI indicates indexing. Filtering happens over symbol metadata off the paint path. Clicking a stale symbol revalidates/remaps range.

Outline definitions are TOML in the language catalog. Provide an importer for Notepad++ `functionList` XML parser definitions: regex-based function and class rules are mapped where the regex is PCRE2-compatible and the structure fits the Bareline outline schema. Emit a deterministic report listing imported, approximated and unsupported rules, exactly like the PR-008 UDL importer. Malformed input cannot crash the editor.

### Document map

Request coarse line/style density tiles rather than rendering a second full editor. Map generation is progressive and revision-aware. For huge files, use sampling/downscaling buckets and a clear viewport indicator. The map may omit syntax detail when unavailable but navigation must remain correct.

### Resource budgets

Workspace scanning has bounded concurrency and ignores configured folders/patterns. Never eagerly hash or read every file merely because a folder opened. Outline and document-map work is lower priority than current viewport/search.

## Minimum verification scenarios

- Open directory containing 1M generated entries across nested folders; first UI interaction only enumerates required levels.
- Rename/delete/create race produces an error/update, never a stale phantom operation.
- Document list handles 5k tabs with virtualized rendering.
- Outline progressively fills on large source file and stale click cannot jump into unrelated text.
- Document map navigates 5 GB log while total line indexing is incomplete.
- Import three real Notepad++ functionList definitions and emit a deterministic mapping report.

## Implementation sequence

1. Mark `PR-009` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Opening a large workspace does not enumerate/index everything on UI thread
- [ ] Document map appears before full file lexing completes
- [ ] Outline navigation uses byte ranges and tolerates stale indexes
- [ ] Default startup still hides all sidebars
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-009: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-02, FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-014, FR-015, FR-016. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Workspace discovery; matching outline; progressive map. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-009-01** — Select main.rs then Cargo.toml; outline changes language and old-revision results never appear in the new document.
- [ ] **AC-009-02** — Open a folder with symlink cycles and inaccessible descendants; traversal is bounded and shows partial results.
- [ ] **AC-009-03** — Scroll before line indexing completes; minimap and outline use estimates explicitly and do not allocate per-line objects.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
