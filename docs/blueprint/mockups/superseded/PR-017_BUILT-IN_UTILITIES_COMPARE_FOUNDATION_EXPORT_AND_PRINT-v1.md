# PR-017 — Built-In Utilities, Compare Workspace, Export and Print

**Tracker row:** `PR-017` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-003`, `PR-007`, `PR-010`, `PR-011`, `PR-012`, `PR-023`, `PR-025`  
**Primary objective:** Cover small but important Notepad++ utility workflows in-core, and deliver the side-by-side Compare Workspace UI on top of the `crates/diff` engine from PR-025, while keeping normal editing lightweight.

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

- MD5/SHA1/SHA256/SHA512 commands
- Base64 and URL encode/decode
- Document summary/statistics
- Syntax-colored HTML/RTF export
- Print/print selection with line numbers/header/footer
- First-class side-by-side Compare Workspace using normal Bareline editor panes, consuming `crates/diff` (PR-025, ADR-12)
- Line + intraline difference rendering and difference overview/navigation from `DiffHunk` data
- Configurable semantic compare colors for Light, Dark and System themes
- Synchronized aligned scrolling and optional horizontal sync
- Undoable left↔right merge/copy actions
- Compare options UI for ignore whitespace/case/blank-line/EOL and tab normalization (options are evaluated by `crates/diff`)
- Compare current document against open file, disk file, last-saved version, recovery snapshot, or external-conflict version
- Compare Workspace session persistence

## Explicit non-goals

- No full binary hex editor in core
- No PDF generation engine
- No diff algorithm implementation here; `CompareOptions`, `DiffHunk`, `CompareResult`, bounded algorithms, coarse fallback and cancellation are PR-025 in `crates/diff` (ADR-12)
- Inline (unified) view and folder compare are v1.1 (ADR-26)

## Likely files / ownership

- `crates/app/src/util*`
- `crates/app/src/compare*`
- `crates/file-io/**`
- `crates/ui/src/compare/**`
- `crates/platform*/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Utility service boundary

Built-in utilities are commands/services, not permanent background subsystems. Hash/encode/statistics jobs stream ranges/files and report progress/cancel; they do not duplicate whole large files.

### Hashing

Support MD5/SHA-1/SHA-256/SHA-512 for parity with clear UI labeling that MD5/SHA-1 are legacy integrity hashes. File hashing reads sequential chunks off UI thread. Selection hashing reads snapshot chunks and reports source revision.

### Encoding utilities

Base64 and URL encode/decode operate on selection/current document according to command and create normal edit transaction. Reject invalid decode cleanly without partial edits unless user chooses best-effort mode.

### Compare Workspace

Compare is a Windows-v1 built-in feature, not merely an extension example. Reuse PR-010 split/editor synchronization primitives, PR-012 semantic theme tokens and PR-023 controls. Do not build a parallel editor or pane framework.

The diff engine is `crates/diff` from PR-025 (ADR-12). This PR calls its compare service with two immutable document snapshots plus `CompareOptions`, receives revision-tagged `CompareResult { hunks, completeness, stats }` asynchronously, and paints from it. Do not reimplement or fork any diff algorithm here; if the engine lacks a capability, extend `crates/diff` through a small compatible change coordinated with PR-025's contract. When the engine reports `CompareCompleteness::Coarse`, the UI labels the result as coarse and keeps navigation working.

The UI opens two equal normal editor panes, vertically by default, with source labels, swap, options, previous/next difference, difference counter, recompare and close actions. Changed blocks remain visually aligned through non-document spacer metadata. Vertical synchronized scrolling is default; horizontal synchronization is optional. Provide diff gutters and an overview navigator.

Semantic states are Added, Removed, Changed, Moved/Aligned and Current Difference. Their backgrounds, accents, gutters, overview markers and optional connector colors are configurable separately through semantic theme tokens. Light, Dark and System themes have accessible defaults; System follows Windows theme changes. User overrides persist and a reset-to-theme-default action is available. Never communicate state by color alone.

Expose ignore-leading/trailing-whitespace, ignore-all-whitespace, ignore-blank-lines, ignore-case, ignore-EOL-style and optional tab normalization as UI toggles mapped one-to-one onto `CompareOptions`. Edits schedule debounced recompare; stale compare results whose source revisions no longer match are discarded before paint.

Provide Copy/Accept Left→Right and Right→Left per hunk/selection. These are standard undoable document transactions, leave the destination dirty, and never auto-save. Support compare against another open document, disk file, last-saved version, recovery snapshot and external-conflict version. Binary compare is not in core v1. Inline (unified) view and folder compare are v1.1 (ADR-26); do not add menu entries or marketing copy for them.

### Export

Syntax-colored HTML/RTF export consumes style spans from syntax service and streams output. Never treat source text as markup without escaping. Print path produces paginated layout with header/footer/line-number options and uses platform print adapter.

### Statistics

Report bytes, decoded characters/graphemes (when meaningful), words and lines. For huge documents, exact expensive metrics can run asynchronously and show progress; already-known byte size is instant.

## Minimum verification scenarios

- Hash 20 GB generated file with bounded memory/cancel.
- Base64/URL invalid input does not partially edit selection.
- Diff ordinary source files with line and intraline highlighting, navigation and aligned synchronized scrolling.
- Configure distinct compare colors in Light, Dark and System themes; switch Windows theme live and preserve overrides.
- Execute left→right/right→left hunk merge; verify one-step undo and no implicit save.
- Exercise ignore whitespace/case/blank/EOL options with deterministic fixtures.
- Edit while compare is running and verify stale revision results never paint.
- Compare Workspace stays responsive and shows Coarse completeness when `crates/diff` reports it for a generated multi-GB/highly-different fixture; cancel from the toolbar stops the job.
- Compare Workspace restores both sources, layout, options and sync-scroll choice from a session round trip.
- HTML export escapes `<>&` and does not allow source-controlled script injection.
- Print preview/print adapter handles DPI/font/line wrapping and cancellation.

## Implementation sequence

1. Mark `PR-017` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Hash outputs match standard test vectors
- [ ] Export preserves text and escapes markup
- [ ] Print path works without blocking editor state
- [ ] Side-by-side compare shows added/removed/changed + intraline differences with next/previous navigation
- [ ] Vertical aligned synchronized scrolling works without jitter/feedback loops
- [ ] Compare colors are configurable through semantic tokens for Light/Dark/System and remain accessible without color alone
- [ ] Left↔right hunk copy/accept is undoable and never auto-saves
- [ ] Ignore whitespace/case/blank/EOL toggles map one-to-one onto `CompareOptions` and are covered by UI fixtures
- [ ] Stale background diff results are discarded after edits
- [ ] Compare Workspace consumes `crates/diff` only; no diff algorithm code exists outside PR-025's crate
- [ ] Coarse completeness from the engine is labeled in the UI and navigation still works
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-017: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-04, FC-06 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-021, FR-021A. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Compare UI; source versions; export and print. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-017-01** — Copy a hunk left-to-right then undo; source labels, dirty state and selection match the actual target revision.
- [ ] **AC-017-02** — Choose ignore options and recompare; cancellation is distinct from a completed coarse result and stale merge buttons disable.
- [ ] **AC-017-03** — Print/export Unicode with syntax/theme options; unsupported printer/filesystem errors preserve the original and show a clear retry path.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
