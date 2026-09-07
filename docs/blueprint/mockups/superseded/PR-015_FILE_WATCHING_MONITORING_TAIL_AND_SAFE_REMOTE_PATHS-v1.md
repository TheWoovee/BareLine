# PR-015 — File Watching, Monitoring/Tail and Safe Remote Paths

**Tracker row:** `PR-015` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-004`, `PR-007`  
**Primary objective:** Implement robust external-change detection, log following and trust-aware UNC/network behavior.

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

- File watcher abstraction and Windows implementation (own `ReadDirectoryChangesW` wrapper, ADR-09)
- Debounced external modify/delete/rename detection
- Conflict UI hooks, including the `SourceChanged` banner for Paged sources (ADR-03)
- Monitoring/tail mode on a Paged `ByteSource` with rotation/truncation detection (ADR-01)
- Pause follow when scrolled away
- Network/UNC trust policy and prompt model
- Do not auto-touch untrusted remote paths restored from session

## Explicit non-goals

- No network file transfer client
- No SSH/SFTP

## Likely files / ownership

- `crates/file-io/**`
- `crates/platform/**`
- `crates/platform-windows/**`
- `crates/ui/src/file_conflict/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### File watcher abstraction

Expose normalized events (`Modified`, `Renamed`, `Removed`, `Created`, `Overflow/RescanNeeded`) with file identity where available. The Windows implementation is Bareline's own `ReadDirectoryChangesW` wrapper in `platform-windows` using overlapped I/O on a single watcher thread; do not add the `notify` crate (ADR-09). Core consumes neutral events only. Debounce bursts and coalesce without hiding meaningful replace/rename sequences.

### External-change policy

For clean open documents: configurable auto-reload or prompt. For dirty documents: never auto-overwrite in-memory edits; enter conflict state and offer Compare/Reload/Keep/Save As. Recheck disk revision on focus/save even if watcher missed events.

Resident documents (ADR-01) always use this conflict flow because their bytes are fully owned. Paged documents additionally handle `SourceChanged` (ADR-03): when identity, size or last-write time of the underlying file changes, loaded pages stay usable, unloaded regions report `Unavailable`, and the UI shows a banner with **Reload** and **Keep editing (read-only for unloaded regions)**. The watcher feeds this transition; a read-side mismatch may also trigger it if the watcher missed the event.

### Monitoring/tail

Follow mode runs on a Paged `ByteSource` opened with `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE` (ADR-01). It tracks file identity and current length and reads only appended bytes, so memory grows with the viewport and index, not with the log. Truncation or rotation transitions the document to `SourceChanged` (ADR-03); tail mode then offers **Reopen and follow**, which reopens the new file at the same path and resumes. If user scrolls away, pause auto-scroll but continue bounded ingestion/indexing. Editing is locked by default in follow mode.

### Remote/UNC trust

Never follow a UNC/network/session-supplied path just because it appears in recovery/session/extension metadata. Classify path source and trust state before OS access. Interactive explicit File Open can be considered user-authorized for that action; background restore/update/extension access requires policy/confirmation.

Avoid shell/icon/metadata calls that can implicitly authenticate to untrusted remote hosts before trust decision. Log blocked destination in sanitized form without document content/credentials.

### Watcher budgets

Do not allocate one OS watcher per file when a directory watcher can cover many. Workspace roots and open files share watcher service. Overflow triggers bounded reconciliation, not full synchronous rescan on UI thread.

## Minimum verification scenarios

- External save/atomic-replace/rename/delete patterns from common editors are detected correctly.
- Dirty buffer is never silently replaced by external change.
- Tail a rapidly growing multi-GB log with bounded memory and rotation/truncation handling.
- Rotate or truncate the log during tail: document enters `SourceChanged`, "Reopen and follow" resumes on the new file, and the process never crashes or reads stale bytes as new.
- Malicious session referencing UNC does not cause SMB/credential access at startup.
- Watcher overflow recovers state without freezing UI.
- Watcher thread survives directory deletion and re-creation without leaking handles.

## Implementation sequence

1. Mark `PR-015` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Rapid save storms are coalesced
- [ ] Tail of growing GB log remains responsive
- [ ] UNC path from session cannot cause silent credential-bearing access
- [ ] External changes never overwrite dirty buffer silently
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-015: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-02, FC-04 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-018. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Watcher generations; tail continuity; remote capability integration. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-015-01** — Append to a log with a partial final page; verify continuity before accepting the new generation, without falsely entering SourceChanged.
- [ ] **AC-015-02** — Rotate, truncate and rewrite same-size content; uncertain continuity becomes SourceChanged and preserves owned edits.
- [ ] **AC-015-03** — Pause tail scrolling while appends continue; unlock stops following and captures a fixed generation after confirmation.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
