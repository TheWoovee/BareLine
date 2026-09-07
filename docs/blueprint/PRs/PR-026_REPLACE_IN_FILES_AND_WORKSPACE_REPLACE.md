# PR-026 — Replace in Files and Workspace Replace

**Tracker row:** `PR-026` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-004`, `PR-005`, `PR-007`  
**Primary objective:** Deliver preview-first replace across folder/workspace files and open documents (ADR-16) with per-file atomic writes through the PR-004 save path, encoding and EOL preservation through PR-007, bounded memory, safe interruption and an exact summary.

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

- `ReplaceInFilesJob` built on PR-005 `SearchQuery` and folder/workspace scanning
- Preview model: per-file change list with before/after excerpts, include/exclude toggles
- Replace in open documents through normal `EditTransaction`s, one undo per document
- Replace in closed files through the PR-004 atomic save path, file by file
- Encoding, BOM and EOL preservation through PR-007 transcode/encode services
- Regex capture groups and case-preserving replacement in replacements
- Binary/NUL-heavy files skipped by default with explicit include option
- Optional backup-on-replace (copy beside file or into backup folder)
- Cancellation that leaves completed files valid and untouched files unchanged
- Exact summary report: files changed, matches replaced, skipped, failed with reasons
- Commands `search.replaceInFiles`, `search.replaceInWorkspace`, `search.replacePreview.toggleAll`

## Explicit non-goals

- No version-control integration or commit of changes
- No undo for closed files beyond the optional backup copy
- No search engine changes; matching stays in PR-005

## Likely files / ownership

- `crates/search/src/replace_files*`
- `crates/ui/src/search/replace_preview*`
- `tests/search/replace_files/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Job model

`ReplaceInFilesJob { query: SearchQuery, replacement: ReplacementTemplate, scope, options: ReplaceOptions, cancel: CancelToken }` runs on the search pool. Phase 1 streams `SearchBatch`es into a `ReplacePreview` grouped by file with bounded excerpts. Phase 2 applies only the files and matches the user left included. No bytes are written before Phase 2 starts.

### Preview

Preview is mandatory for workspace disk replacement. Capture identity, generation, full hash, encoding/EOL, selected match ranges and open-document revision. Display complete/partial status and bounded Before/After excerpts. Incomplete searches disable Apply. A count-only fast path does not waive review of changed contents.

### Open documents

If a file is open, the replacement is applied to its `DocumentService` as one `EditTransaction` with `EditSource::ReplaceInFiles`, producing one undo record. The document becomes dirty; it is not saved automatically.

### Closed files

Reopen and verify the exact previewed identity/full fingerprint before staging. Do not rematch newly changed content. If unchanged, apply the selected preview ranges through original-byte-preserving encoding and PR-004 safe replacement; revalidate immediately before commit. If a file became open/dirty, require its matching document revision or skip for re-preview. Use bounded disk staging.

### Interruption

Cancel between chunks and safe per-file commit points. Committed files remain changed; unopened/incomplete files remain untouched. Persist receipt states so a crash cannot erase the summary. Reconcile a replacement completed before its receipt commit by fingerprint rather than replaying blindly. No global disk transaction or global undo is promised.

### Backup option

Backups default on. Use collision-resistant job ID and file ID names with create-new semantics, never overwrite a prior <name>.bak. Flush the original backup before replacement. The UI names the backup policy; disabling backups requires an explicit per-job choice. Record prepared/committed/failed states in a durable per-file receipt and reconcile interrupted commits using target/backup fingerprints (FC-05).

### Safety

Paths come from the scanner and are revalidated before write. UNC and untrusted paths follow the PR-015 trust policy when that PR has landed; until then they are excluded from write scope with a summary note. Read-only files are reported as skipped unless the option to clear the attribute is set.

## Minimum verification scenarios

- Interrupt a workspace replace over 10,000 generated files midway; already-replaced files are valid, untouched files are byte-identical, and the summary is exact.
- Replace across 100 open documents produces exactly one undo record per document and reports partial failures when two documents are made read-only mid-run.
- Windows-1252 and UTF-16 LE files with mixed EOL are replaced and round-trip encoding, BOM and EOL byte-identically outside the replaced spans.
- Preview shows every change before any write; unchecking a file or a match excludes it from Phase 2.
- Regex replacement with capture groups and case-preserving option produces expected output on fixtures.
- Binary file in scope is skipped by default and included only with the explicit option.
- Disk-full injected during one file leaves that file untouched and continues with the rest, reporting the failure.

## Implementation sequence

1. Mark `PR-026` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] No file is written without preview confirmation unless the user disabled preview
- [ ] Encodings, BOM and EOL are preserved outside replaced spans
- [ ] Interruption leaves every file either fully replaced or untouched, with an exact summary
- [ ] Open documents receive one undo record each and are not auto-saved
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-026: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.3 delivery slices and acceptance

**Normative contracts:** FC-04, FC-05 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-007. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

| Slice name | What compiles and is testable at slice end | AC cases or minimum scenarios that prove it |
|---|---|---|
| Workspace preview | Read-only preview records fingerprints, options and selected matches. | AC-026-01; modify a file after preview and require re-review. |
| Revalidation and staging | Dirty open revisions and closed-file fingerprints gate unique backup staging. | AC-026-02; apply a mixed open/disk selection. |
| Durable receipts | Per-file receipt replay reconciles uncertain replacement outcomes by fingerprint. | AC-026-03; interrupt between replacement and receipt commit. |

- [ ] **AC-026-01**: Preview then externally modify a file; Apply skips it for re-review instead of re-matching and changing unseen contents.
- [ ] **AC-026-02**: Replace in a mix of dirty open files and disk files; revisions/fingerprints are checked, backups unique and per-file receipts durable.
- [ ] **AC-026-03**: Kill between replacement and receipt commit; restart reconciles by target/backup fingerprints and never reapplies blindly.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
