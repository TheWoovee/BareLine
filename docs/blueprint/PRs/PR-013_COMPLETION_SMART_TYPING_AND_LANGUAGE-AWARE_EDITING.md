# PR-013 — Completion, Smart Typing and Language-Aware Editing

**Tracker row:** `PR-013` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-006`, `PR-003`, `PR-008`, `PR-023`  
**Primary objective:** Add fast local completion, parameter hints, paired characters, indentation and language comments without turning Bareline into an IDE.

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

- Document/open-doc word index
- Language keyword completion
- Function/parameter hint data loader
- Auto-close pairs
- Smart indentation hooks
- Comment toggle behavior: implement the `CommentProvider` hook registered by PR-006 (ADR-13)
- Completion popup UI and keyboard semantics
- Per-language enable/disable settings

## Explicit non-goals

- No LSP server bundled
- No AI completion

## Likely files / ownership

- `crates/syntax/**`
- `crates/editor-surface/**`
- `crates/ui/src/completion/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Completion sources

Define `CompletionProvider` returning revisioned items. Built-ins: current-document words, optional open-document words, language keywords and static function signatures. Providers are local/offline. Merge/dedupe by insertion text/kind with predictable priority.

### Incremental word index

Do not scan all open huge files on every keystroke. Maintain a bounded word-frequency index updated from edited/visible/scanned regions; for very large documents, completion can be partial and improve in background. Memory budget must be configurable/bounded.

### Popup behavior

The popup uses PR-023 popover and list controls. Trigger manually and optionally after configured characters. Filter/rank on typed prefix; never steal Enter/Tab when no item is actively selected under the user's policy. Accept operation is one normal edit transaction and works with multi-cursor only when replacement ranges are compatible.

### Parameter hints

Static language metadata may provide signatures. Hints derive from nearby text and lexer state; no full parser/LSP in core. If uncertain, show nothing rather than misleading data.

### Comment toggle (ADR-13)

PR-006 registered `editor.comment.toggleLine`, `editor.comment.toggleBlock` and a `CommentProvider` hook that reports unavailable. This PR implements the provider from language catalog comment tokens: line comments toggle per selected line with consistent indentation column; block comments wrap or unwrap the selection; nested and mixed selections (some lines commented, some not) follow the Notepad++ rule of commenting all when any is uncommented; multi-cursor applies as one transaction; languages without comment tokens keep the commands disabled with a reason shown in the palette.

### Smart pairs/indent

Pair insertion, overtype closing delimiter, quote behavior and newline indentation are rules by language. Respect selections (wrap selected text), multi-cursor transactions and comments/strings when lexer context is available. Do not block waiting for syntax; fall back to simple rule.

## Minimum verification scenarios

- Completion remains responsive in 1 GB file and bounded memory is asserted.
- Multi-cursor pair insertion/indent is one undo step.
- Typing quote/brace in comments/strings follows language rule without waiting for background lexing.
- Completion accept preserves correct byte ranges after a concurrent/stale provider result by rejecting/remapping revision.
- No network requests or AI dependency exists.

## Implementation sequence

1. Mark `PR-013` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Completion appears without blocking typing
- [ ] Huge file uses bounded/local indexing
- [ ] Pair insertion/backspace has deterministic tests
- [ ] Comment toggle provider implements line/block/nested/mixed cases across multi-selection as one transaction
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-013: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.3 delivery slices and acceptance

**Normative contracts:** FC-01, FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-008, FR-012. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

| Slice name | What compiles and is testable at slice end | AC cases or minimum scenarios that prove it |
|---|---|---|
| Completion requests | Bounded completion candidates carry source generation and revision. | AC-013-02; discard results after typing changes the revision. |
| Comment integration | Language registrations implement the PR-006 CommentProvider hook. | AC-013-01; undo line and block comment toggles. |
| Context-safe typing | Smart typing consults bounded local indexes and suppresses unsupported contexts. | AC-013-03; compare string/comment contexts to ordinary identifiers. |

- [ ] **AC-013-01**: Register PR-006 CommentProvider with a language; line/block toggles handle empty selections and undo correctly.
- [ ] **AC-013-02**: Type while completion computes; old-generation/revision suggestions cannot replace newer text.
- [ ] **AC-013-03**: Complete within strings and comments using bounded local indexes; suppress semantically unsupported suggestions.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
