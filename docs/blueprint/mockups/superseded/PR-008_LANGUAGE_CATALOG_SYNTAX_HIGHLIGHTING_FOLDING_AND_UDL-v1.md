# PR-008 — Language Catalog, Syntax Highlighting, Folding and UDL

**Tracker row:** `PR-008` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-002`, `PR-003`, `PR-007`  
**Primary objective:** Implement language detection, viewport-first highlighting/folding with Lexilla as the primary lexer and the native Rust UDL engine as the fallback lexer (ADR-17), fold commands, and user-defined language creation/import without reintroducing Scintilla as the editing buffer.

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

- Lexilla bridge build and safe Rust wrapper
- Native Rust UDL engine that doubles as the fallback lexer (ADR-17)
- Catalog entries for the fifteen most common languages in both Lexilla and native form
- Language catalog metadata/ext/shebang detection
- Viewport-range lex requests and sparse per-line state checkpoints
- Syntax span cache keyed by revision/range
- Folding model and gutter UI
- Fold All, Unfold All, Fold Level 1..8 and Toggle current fold commands (ADR-22)
- Language override/status menu
- UDL schema/editor/import/export
- Notepad++ UDL XML importer with mapping report
- Theme token mapping

## Explicit non-goals

- No full semantic IDE/LSP
- No function outline; separate PR

## Likely files / ownership

- `crates/syntax/**`
- `native/lexilla-bridge/**`
- `languages/**`
- `crates/ui/src/language/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Language catalog

Define versioned language metadata separate from lexer binaries: stable language ID, names/extensions/globs, shebang/modeline rules, lexer binding, comment tokens, brace pairs, fold capability, indentation rules, keyword sets, completion source and outline definition.

### Lexilla bridge

Link Lexilla through a C++ shim and Rust FFI. Do not expose Scintilla document objects. Implement the `IDocument` accessor callbacks required to lex requested ranges; expect roughly two dozen methods, not a handful, and fuzz every one of them across the FFI boundary. FFI input/output buffers are bounded and validated; panic may not cross FFI. Because the document is internally UTF-8 (ADR-02), the shim reports code page UTF-8 and never performs DBCS lead-byte logic.

### Native UDL engine as fallback lexer (ADR-17)

The UDL engine is native Rust: keyword sets, operators, delimiters, comment and string rules, number rules, folding rules and style mapping. The same engine lexes any language whose catalog entry carries a native definition. The fifteen most common languages ship with both a Lexilla binding and a native definition. Selection rule per language: Lexilla is primary. If viewport-first Lexilla styling cannot show a styled first viewport within 100 ms on the 1 GB fixture, the catalog entry for that language sets `lexer = "native"` by default. The user can switch per language in settings.

### Incremental styling

Lexilla lexers run forward from a known state, so store per-line lexer state sparsely: one checkpoint every N lines (N is a tuning parameter, default 256) holding style and `LineState` at that line, plus dense state only for the region around the viewport and recent edits. To style a region, restart from the nearest checkpoint at or before it; if none exists, restart from the nearest preceding blank-line anchor and mark the result provisional until a background pass from a real checkpoint confirms it. On edit, invalidate affected range plus language-specific look-back; prioritize viewport and nearby lines, then background-fill farther regions. A huge file must become readable immediately as plain themed text while styles arrive progressively. Never store one heap allocation per line.

Store style runs keyed by document revision + byte range. Never store one heap allocation per token when a compact run array can be used.

### Folding

Folds are derived metadata, not document mutations. Maintain fold headers/levels incrementally. Collapsing a fold changes `EditorViewState`; cloned view can have different folding state. For not-yet-indexed huge regions, show fold affordances only where known instead of scanning the entire file synchronously.

Commands with stable IDs: `view.fold.all`, `view.fold.unfoldAll`, `view.fold.level1` through `view.fold.level8`, `view.fold.toggleCurrent`. Fold Level N acts on folds known so far and continues applying as background indexing discovers more headers; the UI indicates when folding is still incomplete.

### UDL

Bareline UDL schema is JSON/TOML-friendly and versioned. Import Notepad++ UDL XML with explicit mapping report: imported, approximated, unsupported. UDL editing validates regex/keyword limits and live-previews on an isolated sample; malformed UDL can never crash the main editor.

### Detection order

Explicit user override > workspace association > filename/glob > shebang/modeline > content heuristic > plain text. Store manual override in session/workspace metadata, not file content unless user asks.

## Minimum verification scenarios

- Catalog parity audit against current Notepad++ built-in language families.
- Lexing fixture snapshots for representative languages and multiline-state edits.
- Edit near top of 1 GB file invalidates bounded regions and viewport re-highlights without full relaunch.
- Collapse/expand fold on large file cannot freeze UI.
- Import several real Notepad++ UDL XML fixtures and emit deterministic compatibility report.
- Fuzz malformed lexer/UDL inputs across FFI boundary.
- Native UDL fallback produces styled viewport within 100 ms for each top-15 language on 1 GB fixture.
- Fold Level N commands act on folds known so far and continue as indexing completes.

## Implementation sequence

1. Mark `PR-008` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Changing one line does not re-lex multi-GB file synchronously
- [ ] Visible syntax appears progressively
- [ ] Fold state survives edits when anchors remain valid
- [ ] UDL import reports unsupported fields rather than dropping silently
- [ ] Every top-15 language has both a Lexilla binding and a native UDL definition
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-008: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-01, FC-02, FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-010, FR-011. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Named language catalog; provisional syntax; bounded parsing. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-008-01** — Validate paired Lexilla/native definitions for all 15 languages listed in FC-09; publish coverage and known grammar gaps.
- [ ] **AC-008-02** — Open in the middle of a multiline string; provisional highlighting is labelled until a valid lexer checkpoint resolves context.
- [ ] **AC-008-03** — Import malformed/oversized UDL input; reject unsafe structure and preserve the previous definition.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
