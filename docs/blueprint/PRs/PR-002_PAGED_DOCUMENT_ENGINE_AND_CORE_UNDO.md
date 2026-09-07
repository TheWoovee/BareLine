# PR-002 — Paged Document Engine and Core Undo

**Tracker row:** `PR-002` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-001`  
**Primary objective:** Implement the storage engine that makes Bareline fundamentally different from whole-buffer editors: resident-or-paged `ByteSource`, edit store, balanced piece tree, sparse line index, revisions, snapshots, the `DocumentService` actor and transaction undo/redo.

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

- `ByteSource` trait with `Resident` and `Paged` implementations, plus an in-memory source for untitled docs (ADR-01)
- Viewport-first streaming load for `Resident` sources: requested pages are published first, the remainder fills in the background
- `document.resident_max_bytes` setting, default 256 MiB, selecting `Resident` below and `Paged` above
- `SourceChanged` state for `Paged` sources (ADR-03)
- PieceTree with valid text-view offsets and CR/LF/CRLF-aware Known/Unknown aggregates (ADR-02); invalid bytes are represented by tagged opaque spans with original-byte provenance
- Append-only EditStore
- SparseLineIndex with viewport-prefetch API
- Immutable DocumentSnapshot
- EditTransaction validation and apply
- Undo/redo transaction log
- `DocumentService` actor: one logical actor per open document, scheduled on a bounded shared pool (default 2 workers, configurable up to logical CPUs); an actor never owns an OS thread (FC-06, ADR-11); serialize transactions and publish snapshots
- Property/fuzz-style random edit tests

## Explicit non-goals

- No UI selection drawing
- PR-007 supplies non-UTF-8 codecs through PR-001 interfaces; PR-002 owns bounded disk-backed source storage
- No save-to-disk replacement logic

## Likely files / ownership

- `crates/document/**`
- `tests/huge-files/**`
- `tests/property/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Sizing note

Budget roughly two to three times the v1.1 effort across PR-002/004/007 for side tables, disk-backed stores, sealed baselines and receipts. Ship UTF-8 Resident core first with a UTF-8-only, Resident-only path so the editor is usable early. Follow with Paged sources and disk stores; Aggregates and budget integration; keep Paged and legacy-codec integration in those later slices.

### Source and storage model

Document has one valid UTF-8 text view and a separate original-byte domain. Pieces refer to generation-tagged base spans, disk-backed transcode spans or owned edit segments. Invalid bytes retain provenance (FC-01). Never use LF-only counts for mixed EOL; CRLF boundary state and Known/Unknown aggregates are required.

ByteSource exposes nonblocking Ready/Pending/Unavailable ranges. Resident streams viewport-first and is sealed only after complete copy validation. Paged reads through a bounded shared cache. Unread data can become unavailable if the generation changes. Transcode and recovery baselines can occupy bounded disk storage without becoming full-file RAM allocations. No mmap in v1. FC-02 defines exact budgets and state semantics.

### FC-02 settings keys

| Setting key | Default |
|---|---|
| `document.resident_max_bytes` | 256 MiB |
| `document.page_size_bytes` | 1 MiB |
| `document.page_cache_bytes` | 64 MiB |
| `document.aggregate_cache_bytes` | 256 MiB |
| `undo.aggregate_ram_bytes` | 128 MiB |
| `search.results_ram_bytes` | 64 MiB |
| `transcode.temp_quota_bytes` | min(20 GiB, 20% free) |
| `clipboard.history.max_entries` | 20 |
| `clipboard.history.max_total_bytes` | 16 MiB |
| `clipboard.history.max_entry_bytes` | 4 MiB |

Show effective values in Settings. Enforce aggregate caps across tabs; spill eligible owned chunks and undo payloads to disk. Recompute the transcode quota from free space before and during growth. User policy owns resource limits; workspace overrides remain restricted by FC-09.

### DocumentService

`DocumentService` is one logical actor per open document, scheduled on a bounded shared pool (default 2 workers, configurable up to logical CPUs); an actor never owns an OS thread (FC-06, ADR-11). Its message API is:

```text
apply(EditTransaction) -> Result<Revision, TransactionError>
snapshot() -> DocumentSnapshot
undo() / redo() -> Result<Revision>
subscribe() -> RevisionReceiver   // notifies UI and background services of new revisions
prefetch(range)                   // line index and page prefetch around a viewport
```

All mutations go through `apply`. Callers never hold a lock across I/O, regex or paint. UI and background consumers hold snapshots, never the live tree.

### SourceChanged

Detect changed source generation on reads and through watcher/revalidation events. Paged and incomplete Resident sources fail closed for missing original regions. Owned snapshot spans stay valid; inverse bytes for deleted or replaced ranges are copied into owned edit/recovery segments before the transaction commits; view-cache retention is never an undo strategy (FC-03). Only sealed Resident sources are independent of disk changes. PR-004 disables ordinary save when data is missing and provides explicit partial export with a gap manifest. Tail append continuity is integrated by PR-015 under FC-04.

### Piece tree

Use a measured balanced tree (B+/rope-style) whose internal nodes cache at least total byte length and newline count. Leaf target size/fan-out is an implementation parameter, not a public format. Required operations are logarithmic in piece count:

```text
split_at(byte_offset)
insert(byte_offset, SourceSpan)
delete(range)
replace(range, SourceSpan)
read_chunks(range) -> iterator
count_newlines(range)
```

Coalesce adjacent pieces only when they reference contiguous bytes from the same immutable source and coalescing does not erase an undo boundary that must be retained.

### Sparse line index

Do not pre-scan a multi-GB file on open. Maintain checkpoints discovered by viewport reads/background scanning. Required queries:

- byte offset -> known/estimated line plus bounded refinement;
- line -> byte range with cancellable forward scan when not indexed;
- prefetch line/byte region around viewport;
- invalidate/adjust checkpoints after edits.

The API must expose when a line count is exact versus still being discovered so the UI can show `Lines: indexing…` rather than lying. For `Resident` sources the background fill also completes the line index; for `Paged` sources the index stays sparse.

### Revisions and snapshots

Every committed edit increments `Revision`. `DocumentSnapshot` is immutable and can be sent to background readers without holding the editor's mutation lock. A background result must carry the revision it was derived from. Stale results are either remapped by an explicit edit map or discarded; silent application to a newer revision is forbidden.

### Edit transactions

`EditTransaction` receives non-overlapping ranges expressed against one base revision. Normalize ranges in descending byte order for mutation, but preserve semantic order for resulting selections. Reject out-of-bounds ranges, overlapping edits after normalization, and invalid UTF-8 boundary requests where the caller promised text boundaries.

### Undo/redo

Store inverse transaction data, selection before/after and edit source. Typing merge is time/command-boundary aware: ordinary adjacent typing may merge; paste, macro, multi-cursor, replace-all, extension edits and explicit cursor moves create boundaries. Undo must never depend on the current file still existing on disk.

### Memory rules

- Opening a file must not copy its full contents before the first viewport is interactive, and a file above the resident threshold is never fully materialized (ADR-01).
- Edit storage grows roughly with inserted/replaced content, not base-file size.
- Undo retention is configurable by memory/change budget, with old clean checkpoints discardable only when semantics remain correct.
- No line index stores one heap object per line for huge files.
- The `Paged` cache is bounded by `document.page_cache_bytes` and evicts LRU pages.

## Minimum verification scenarios

- Property test 100k random insert/delete/replace operations against a simple `Vec<u8>` oracle.
- Same random sequence with undo-all then redo-all produces byte-identical results and correct selections.
- 100 MB fixture opens `Resident` with the first viewport published before the full read completes.
- 1 GB fixture opens `Paged` with a bounded cache; open generated 1 GB/5 GB streamed fixture and assert resident memory does not scale linearly with base size.
- `Paged` source truncated externally yields `SourceChanged` and `Unavailable` reads, never a crash.
- Two threads submit transactions through `DocumentService` and ordering is serialized with monotonic revisions.
- Newline mapping correctness across CRLF/LF/CR byte sequences over UTF-8 input, including opaque spans retaining invalid original bytes.
- Snapshot reader continues to see its original revision while mutations proceed.
- Corrupt/out-of-range transaction is rejected without partial mutation.

## Implementation sequence

1. Mark `PR-002` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] 1 GB fixture opens `Paged` without full materialization; 100 MB fixture opens `Resident` viewport-first
- [ ] `DocumentService` serializes edits and publishes snapshots
- [ ] External truncation of a `Paged` source produces `SourceChanged`, not a crash
- [ ] Random edit property tests match reference implementation
- [ ] Multi-edit transaction is atomic and undoable
- [ ] Line/byte mapping remains correct after random insert/delete sequences
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-002: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.3 delivery slices and acceptance

**Normative contracts:** FC-01, FC-02, FC-03, FC-06 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-005, FR-009. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

| Slice name | What compiles and is testable at slice end | AC cases or minimum scenarios that prove it |
|---|---|---|
| UTF-8 Resident core | Resident-only valid UTF-8 source, piece tree and owned inverse segments support edit/undo snapshots. | AC-002-01 on Resident fixtures; edit, delete and undo without reading evicted view data. |
| Paged sources and disk stores | Paged ByteSource and bounded disk-backed chunks compile with generation-aware availability. | AC-002-02 and AC-002-03; force cache eviction and source replacement. |
| Aggregates and budget integration | CRLF boundary aggregates, sparse Pending lookups and aggregate spilling work across tabs. | AC-002-01 across page boundaries; exceed aggregate cache and undo caps. |

- [ ] **AC-002-01**: Generate random CR/LF/CRLF edits, including separators split across pieces; line counts match the reference model.
- [ ] **AC-002-02**: Delete a range, evict its base pages and externally replace the source; undo restores owned inverse bytes exactly.
- [ ] **AC-002-03**: Race a Resident background fill with truncation; missing old-generation bytes return Unavailable and never new contents.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
