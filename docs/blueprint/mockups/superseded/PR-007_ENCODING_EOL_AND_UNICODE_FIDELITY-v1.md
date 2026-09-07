# PR-007 — Encoding, EOL and Unicode Fidelity

**Tracker row:** `PR-007` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-002`, `PR-004`  
**Primary objective:** Add encoding detection, the streaming open-time transcoder that keeps the document internally UTF-8, interpret-as versus convert-to commands, streaming save re-encoders and EOL handling, while every untouched file round-trips byte-identically (ADR-02).

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

- EncodingState and detector confidence
- Detection order: BOM, strict UTF-8 test, bounded legacy detector sample
- Streaming codecs feed disk-backed TranscodedSource storage with bounded raw/text mapping. encoding_rs covers supported legacy codecs; dedicated UTF-16/UTF-32 and true Latin-1 adapters cover its gaps.
- Opaque-span provenance retains invalid original bytes without occupying reserved user code points.
- Interpret-as (re-run open transcode from original bytes) versus convert-to (change save target only) commands
- Streaming re-encoder on save with refuse-lossy default
- CRLF/LF/CR/mixed EOL detection and conversion
- Binary/NUL warning path
- Status bar encoding/EOL integration

## Explicit non-goals

- No hex editor
- No full Unicode normalization toolkit beyond optional simple command
- No mmap or raw-byte views of legacy encodings; the transcoder is the only path.

## Likely files / ownership

- `crates/file-io/**`
- `crates/document/**`
- `tests/encoding/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Internal UTF-8 contract

The text view is valid UTF-8. Original bytes are separately addressable using RawOffset and generation; TextOffset is never implicitly interchangeable. Invalid sequences render as tagged opaque U+FFFD spans whose side table owns the original bytes. Ordinary U+FFFD and private-use characters have no tag. Untouched same-encoding spans copy original bytes rather than relying on decoder/encoder bijection.

PR-002 provides bounded disk-backed TranscodedSource storage. PR-007 supplies streaming codecs and integrates file open, save and folder search through the PR-001 contract. The exact supported matrix, quota and conversion policy are in FC-01/02.

### Encoding state

Track `EncodingState { detected, confidence, bom, user_override, save_target, had_decode_errors, escaped_byte_count }`. Distinguish **interpret bytes as X** from **convert/save as X**. Opening never rewrites bytes on disk.

### Detection

Use BOM first, then strict UTF-8 test, then a bounded detector/sample for legacy encodings. Detection result is advisory and visible. Do not scan a 5 GB file simply to decide encoding. If later chunks invalidate an optimistic UTF-8 detection while the transcode is still streaming, surface the problem and offer interpret-as; never silently switch an edited document.

### Interpret-as versus convert-to

**Interpret as X** discards the current transcode and re-runs the open pipeline from the original source bytes with encoding X. It is refused when the document is dirty unless the user confirms discarding edits. It never touches disk. **Convert to X** changes only `save_target`; document bytes stay UTF-8 and the next save re-encodes. Both are stable command IDs and appear in the status bar encoding menu.

### Save conversion

Stream pieces. For unchanged same-encoding spans, copy original bytes; encode changed text with the dedicated supported target encoder. Refuse unresolved opaque bytes on encoding conversion and show offending ranges. Only an explicit user decision replaces unrepresentable text. BOM and EOL choices are deliberate. Never assemble a whole-file string in RAM; preserve original file and recovery data on quota/disk failure.

### EOL

Detect CRLF/LF/CR per logical line and preserve mixed endings by default. `EolState` tracks dominant/observed forms but does not normalize on open. Convert whole file/selection is a normal edit transaction or streaming-save transform with undo semantics defined.

### Binary detection

Sample NUL/control-byte density. Warn and offer read-only text/Hex extension instead of rendering random binary as ordinary text. Binary warning must not block deliberate opening.

## Minimum verification scenarios

- Round-trip representative UTF-8/BOM, UTF-16LE/BE, UTF-32, Windows-1252, Shift-JIS, GBK, Big5, EUC-JP and other chosen supported pages byte-identically when untouched.
- Legacy file containing undecodable bytes round-trips byte-identically through tagged original-byte provenance.
- UTF-16 2 GB generated fixture transcodes in a streaming pass with bounded memory and viewport-first display.
- Interpret-as switches from Windows-1252 to Shift-JIS without touching disk.
- Edit near transcode chunk boundaries in a multibyte legacy encoding and save/reopen correctly.
- Attempt conversion containing unrepresentable characters; save is refused without corrupting original.
- Mixed CRLF/LF/CR stays mixed after unrelated edit.
- Huge file encoding detection reads only bounded samples before first viewport.

## Implementation sequence

1. Mark `PR-007` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Fixtures round-trip without unintended byte changes
- [ ] Every supported encoding round-trips byte-identically when untouched, including undecodable bytes
- [ ] Converting encoding intentionally changes bytes and updates state
- [ ] Mixed-EOL file is not normalized on ordinary save
- [ ] Multi-GB save path streams without materializing whole Unicode string
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-007: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-01, FC-02, FC-04 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-009. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Codec implementation; staged integration with lifecycle and search. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-007-01** — Round-trip invalid UTF-8, genuine U+FFFD/private-use characters and noncanonical legacy sequences without provenance collisions.
- [ ] **AC-007-02** — Open/save UTF-16LE/BE, UTF-32LE/BE and true Latin-1 fixtures with BOM and mixed EOL; exact unedited bytes survive.
- [ ] **AC-007-03** — Transcode a generated 5 GiB UTF-16 file under the RAM budget; disk quota exhaustion is visible and original data remains intact.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
