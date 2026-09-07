# PR-004 — File Lifecycle, Tabs, Sessions and Crash Recovery

**Tracker row:** `PR-004` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-002`, `PR-003`, `PR-023`, `PR-025`  
**Primary objective:** Implement reliable open/save/close, the tab model and its persistence, recent files, incremental session restore, named sessions and crash recovery journals with atomic state updates, plus conflict and recovery previews built on the `crates/diff` engine.

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

- Open file service and metadata fingerprint, choosing `Resident` or `Paged` by `document.resident_max_bytes`
- Atomic save/Save As/Save Copy/Save All
- Dirty state and external revision fingerprint
- Reload From Disk, Set Read-Only toggle, Restore Last Closed Tab (ADR-22)
- `TabModel`: pin state, order, close variants as model operations, recent files, persistence (ADR-14). Tab strip UI is PR-010
- Incremental session restore after the first frame, active tab first (ADR-33)
- Versioned session schema as JSON (ADR-21)
- Named session import/export
- Recovery journal with CRC32C and checkpoint/replay
- Recovery Center view model and basic UI using PR-023 controls
- Conflict and recovery previews rendered from `crates/diff` results (ADR-12)
- `SourceChanged` banner for `Paged` sources with Reload / Keep editing (ADR-03)

## Explicit non-goals

- No workspace folder tree
- No shell integration installer
- No syntax-specific state
- No tab strip drawing, drag reorder, overflow, colors, vertical tabs or MRU switcher UI; those are PR-010

## Likely files / ownership

- `crates/file-io/**`
- `crates/session/**`
- `crates/app/src/tabs*` (tab model and persistence only)
- `crates/ui/src/recovery/**`
- `crates/ui/src/banner/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### File lifecycle state machine

Represent document file state explicitly, not through scattered booleans:

```text
Untitled
Loaded(path, disk_revision)
Saving(target, base_revision)
Conflict(path, disk_revision, document_revision)
SourceChanged(path)            // Paged sources only, ADR-03
Missing(path)
ReadOnly(path, reason)         // reason: attribute, user toggle, follow mode, unloaded regions
```

Dirty state compares ContentStateId to saved_content_state_id; undo to a saved state becomes clean, while later edits during a save remain dirty. `Set Read-Only` toggles a user reason without touching file attributes. `Reload From Disk` discards the in-memory document after the normal dirty prompt and reopens through the open flow.

### Open flow

Opening a path performs metadata/open-handle checks off the UI thread, selects `Resident` or `Paged` by size against `document.resident_max_bytes`, constructs the `ByteSource`, samples for encoding in PR-007-compatible fields, then publishes a document through `DocumentService` before full line/syntax indexing. For `Resident` sources the viewport pages are published first and the remainder streams in the background. Duplicate open policy uses canonical file identity when available, not string-only path comparison.

### Safe save flow

Capture a complete snapshot and ContentStateId; stream to a same-directory temporary file, flush, revalidate full destination fingerprint and apply the FC-04 filesystem capability/metadata policy. Newer edits remain dirty after this captured state saves. Unsupported targets offer Save Copy; no unsafe in-place fallback. Incomplete source disables Save/Save As and uses explicit partial export.

### Disk revision

Identity, size and last-write metadata are hints. Destructive replacement revalidates the full preview/save fingerprint and documents the residual arbitrary-writer race (FC-04). Conflict preserves versions and offers labelled Compare, Reload with confirmation, Keep editing and Save Copy. Missing regions follow UI-SC; never present a partial snapshot as complete.

### Tab model

`TabId` is not `DocumentId`; cloned views/tabs can reference the same document. `TabModel` owns ordered tabs, pinned region, active tab, per-tab view placement and the closed-tab history used by Restore Last Closed Tab (document identity, caret, scroll, pin state, capped at 20 entries). Close variants (close, close others, close left/right, close unchanged, close all but pinned) are model operations returning the affected tab list; PR-010 renders and drives them. Persist tab order, pinned state, split/view placement, active tab and view-state separately from file bytes.

### Incremental session restore

Session restore never runs before the first frame (ADR-33). After the first frame the app reads the session manifest body, creates tab entries with placeholder titles, opens the active document first, then opens remaining documents in MRU order on the I/O pool with a bounded concurrency of 2. The window is interactive throughout; tabs whose documents are still loading show a loading badge.

### Crash recovery

Implement FC-03 ordering: own inverse/inserted segments, flush segments, append and flush CRC journal record, then acknowledge the durable revision. Checkpoint manifests publish after all references are durable. Kill-injection tests cover every boundary.

First dirty edit schedules a sealed baseline. Complete recovery requires that baseline or a verified original; until then display 'Edits protected; full snapshot preparing'. Disk-full/unavailable storage reports the last durable time and does not claim a five-second guarantee. Cleanup follows durable save/discard reference retirement.

### Recovery center

Rows distinguish Complete, Edits only, Corrupt tail and Source unavailable. Open recovered copy never overwrites disk. Compare labels Recovered copy and Current disk. Edits-only export includes a gap report. Discard names the affected recovery and confirms irreversible removal before a durable tombstone. A corrupt tail exposes only validated data and a visible warning. See FC-03 and UI-RC states.

### SourceChanged banner

Use Reload and Export available data actions. Missing original regions are visibly unavailable; Save and Save As cannot produce a normal complete document. Export writes available fragments with explicit offsets and a gap manifest. Dirty reload requires confirmation. See FC-04 and the source-changed visual reference.

## Minimum verification scenarios

- Kill process after every journal write boundary and recover to last valid record.
- Simulate disk-full/permission-denied/AV lock during save; original file remains byte-identical.
- External writer changes a file between edit and save; conflict is detected and no overwrite occurs without user action.
- Conflict dialog shows a diff preview produced by `crates/diff`.
- Session with 500 tabs restores incrementally without blocking first interactive window; session restore shows the first frame before any document read completes.
- Pinned tabs/splits/caret/scroll survive versioned session migration.
- Restore Last Closed Tab restores document, caret and pin state.
- Malicious session entries with `..`, device paths or UNC do not trigger access before trust evaluation.
- `Paged` document truncated externally shows the SourceChanged banner and Reload reopens cleanly.

## Implementation sequence

1. Mark `PR-004` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Original file survives injected save failures
- [ ] Killing process after journal append recovers last valid edits
- [ ] Pinned tabs/session layout survive restart and schema round trip
- [ ] Recovery cache never masquerades as a saved user file
- [ ] First frame is not blocked by session restore
- [ ] Restore Last Closed Tab, Reload From Disk and Set Read-Only have command IDs and tests
- [ ] Conflict and recovery previews come from `crates/diff` and never write either side
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-004: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-03, FC-04 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-001, FR-002, FR-003, FR-017. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: UTF-8 lifecycle first; durable recovery; codec and watcher integration later. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-004-01** — Edit, save, edit again and undo to saved ContentStateId; the dirty marker clears without comparing monotonic revisions.
- [ ] **AC-004-02** — Kill the process after each segment/journal/checkpoint flush boundary; recovery stops at the last valid durable record.
- [ ] **AC-004-03** — Start a save then type; successful save clears only its captured state. Missing source regions disable Save/Save As and offer export with gaps.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.

### Staged dependency boundary

Initial PR-004 supports the PR-001 UTF-8 codec and trusted local filesystem capability contract, revalidating on every read/save. Unsupported codecs/remote capabilities fail closed. PR-007 integrates the codec matrix and PR-015 integrates watchers, tail and remote capability handling; their acceptance cases complete those integrations. Do not add a PR-004 → PR-007 dependency cycle.
