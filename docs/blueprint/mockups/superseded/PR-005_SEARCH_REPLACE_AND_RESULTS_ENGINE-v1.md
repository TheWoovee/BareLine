# PR-005 — Search, Replace and Results Engine

**Tracker row:** `PR-005` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-002`, `PR-003`, `PR-023`  
**Primary objective:** Deliver literal, extended and PCRE2 regex search across the current document, open documents and folders, replace within the current document, selection and open documents, five independent mark styles, streaming results and cancellation. Replace in files and workspace replace are PR-026 (ADR-16).

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

- SearchQuery/Scope/Options contracts
- Fast literal scanner over bounded UTF-8-aligned windows
- PCRE2 bridge with capture replacement and partial-match streaming for multi-line patterns (ADR-18)
- Find next/previous/count
- Multi-style Mark: five independent mark styles, clear one or all (ADR-22)
- Replace one/all/in selection/in open documents
- Open-doc and folder/workspace file scanning API (search only)
- Streaming grouped SearchResult batches
- Cancellation and stale-revision handling
- Compact find bar + full search panel using PR-023 controls and virtual list

## Explicit non-goals

- No bookmark line commands beyond mark hooks
- No workspace tree UI requirement
- No extension search providers yet
- No replace in files or workspace replace; that is PR-026 (ADR-16)

## Likely files / ownership

- `crates/search/**`
- `native/pcre2/**`
- `crates/ui/src/search/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Search request/result contracts

Define immutable `SearchQuery { pattern, mode, case, whole_word, scope, direction, wrap, filters, resource_limits }` and a cancellable `SearchJobId`. Stream `SearchBatch` entries containing source identity, revision (for open docs), byte match range, line hint and a bounded excerpt. Results must never own the whole matched file. Folder scans initially use the PR-001 codec interface with UTF-8 support; PR-007 adds other codecs and owns the integration acceptance. Unsupported codecs are visible skips until then.

### Chunking

Windows are at most 16 MiB. Prefer line ends, but split long lines at valid UTF-8 boundaries. Literal overlap includes possible cross-window prefixes. Case folding maintains a TextOffset map. Line hints may be unknown; never invent an exact count. See FC-05.

### Literal search

Use bounded byte scanners for the case-sensitive path. Every literal can cross a window boundary, including literals without newlines; retain the required prefix/suffix context. Unicode folding maps matches back to text offsets. Deduplicate overlap hits by absolute start/range and reject stale generations.

### Regex

Use an audited PCRE2 wrapper with match/depth/heap and cancellation limits. Context-preserving adapters must cover partial reruns, lookbehind, anchors, empty matches and EOF. A string heuristic cannot classify pattern semantics. Publish the tested streaming catalog and explicit UnsupportedStreaming result; a bounded full-subject fallback is allowed only within the documented budget. A limit or unsupported semantic makes results incomplete and disables replacement. No override removes process memory or UI safety limits. See FC-05.

### Open-document search

Search a stable `DocumentSnapshot`. Results carry its revision. On navigation after later edits, attempt position remapping through edit history when available; otherwise revalidate nearby content and mark stale results rather than jumping to a wrong byte.

### Workspace search

Walk directories with ignore/filter policy, bounded queue and bounded worker count. Skip binary/NUL-heavy files by default with an explicit include option. Follow symlinks only under configured safe policy. Do not open thousands of UI documents to search them.

### Mark styles

Provide `MarkStyle::{One, Two, Three, Four, Five}` with commands `search.mark.style1` through `search.mark.style5`, `search.mark.clearStyleN` and `search.mark.clearAll`. Marks are decorations stored per `EditorViewState`, never in the document, and map through edit deltas like bookmarks. Each style has its own semantic theme token (`mark.style1` to `mark.style5`). Mark All from the search panel writes into the currently selected style.

### Results UI

Bottom panel model is the PR-023 virtual list grouped by file. Rendering thousands/millions of hits may summarize after a configurable result cap while continuing count if requested. A result click resolves file/byte position directly; it must not re-run whole-file syntax or line indexing synchronously.

### Replace

Build all proposed edits against snapshots before mutation. Current-document edits use one transaction. Linked replacement across open documents stages inverse data, validates all revisions and commits under UndoGroupId; stale input aborts preparation without partial mutation. Large edits stage on disk. Workspace disk files use PR-026 per-file transactions and receipts, with explicit non-global atomicity. Incomplete searches cannot drive either path.

## Minimum verification scenarios

- Matches adjacent to 16 MiB chunk boundaries for literal and single-line regex modes.
- A multi-line regex match that crosses a chunk boundary is found via partial-match streaming.
- Regex captures/backreferences/lookarounds and common Notepad++ replacement patterns fixtures.
- Cancel a search over generated 20 GB corpus and observe stop acknowledgement within target budget.
- Search result navigation in 1 GB file stays bounded and does not trigger full-file parse.
- Replace-all in 100 open docs is one undo transaction per document and reports partial failures.
- Mark styles 1 to 5 apply and clear independently and survive edits above the marked lines.

## Implementation sequence

1. Mark `PR-005` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Regex capture/backreference compatibility tests pass
- [ ] Multi-line regex crossing a chunk boundary is found
- [ ] Cancel becomes effective without UI freeze
- [ ] Result click navigates by emitted byte range
- [ ] 50MB+ result navigation does not trigger full-file parse
- [ ] Five mark styles work independently
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-005: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-05, FC-06 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-007. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: UTF-8 search; bounded regex adapters; linked open-document replace. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-005-01** — Match across a 16 MiB window in a 20 MiB line; case-fold results map to correct original TextOffsets.
- [ ] **AC-005-02** — Exercise lookbehind, anchors, empty matches and partial regex contexts; unsupported cases report incomplete and disable replacement.
- [ ] **AC-005-03** — Replace in two dirty open documents while one revision changes; prepare aborts before either mutates; successful linked undo reverses both.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
