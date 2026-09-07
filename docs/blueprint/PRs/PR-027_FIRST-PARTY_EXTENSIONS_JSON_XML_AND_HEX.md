# PR-027 — First-Party Extensions: JSON, XML and Hex

**Tracker row:** `PR-027` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-016`  
**Primary objective:** Ship the first-party extension seed set (ADR-25): JSON tools, XML tools and a read-only Hex view as `.blex` WebAssembly components under MIT OR Apache-2.0, published to the signed `bareline-extensions` catalog and doubling as the SDK reference implementations with a documented walkthrough.

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

- `extensions/json-tools`: format, minify, validate with error position, tree view panel
- `extensions/xml-tools`: format, validate, XPath query panel
- `extensions/hex-view`: read-only bytes and text panes over snapshot ranges, goto offset
- `.blex` packaging, `manifest.toml` with minimal capabilities for each
- Publication to the `bareline-extensions` catalog with signed index entries
- SDK walkthrough in `docs/extensions/` using these three as worked examples
- Extension test harness that runs each component against the PR-016 host in CI

## Explicit non-goals

- No hex editing in v1; the Hex view is read-only
- No JSON schema store or remote schema downloads
- No XSLT engine
- No new host API surface; if the SDK lacks something, file a PR-016 follow-up and stub around it

## Likely files / ownership

- `extensions/json-tools/**`
- `extensions/xml-tools/**`
- `extensions/hex-view/**`
- `docs/extensions/**`
- `tests/extensions/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Common shape

Each extension is a Rust crate compiled to a WebAssembly component against the PR-016 SDK, packaged as `.blex` with `manifest.toml` declaring ID (`org.bareline.json-tools` and so on), version, host API range, contributed commands and panels, and requested capabilities. All three are MIT OR Apache-2.0 so authors can copy them freely. Each command has a stable ID such as `ext.json.format`.

### Capabilities

- json-tools: `document.read`, `document.edit`, `ui.panel`
- xml-tools: `document.read`, `document.edit`, `ui.panel`
- hex-view: `document.read`, `ui.panel`

No extension requests `network`, `process.spawn` or `workspace.write`.

### JSON tools

Format/minify stream token ranges and preserve numeric lexemes by default, including large integers and exponents. Validation reports precise TextOffset and line hints. Lazy tree nodes have depth/node/memory caps. Large formatter edits use BeginEdits/AppendChunk/CommitEdits staging with one validated base revision, never one oversized IPC frame.

### XML tools

Use bounded streaming parsing with DTD, external entities, remote resolution and XInclude disabled. FC-07 defines the v1 XPath subset: child/descendant paths, attributes/text, namespace-aware names, equality predicates and positional predicates. Unsupported axes/functions return an explicit unsupported error. Enforce depth, node, time and output limits; no extension network capability.

### Hex view

Use read_original_bytes with RawOffset and original SourceGeneration, not the decoded text snapshot API. Label Original file bytes and disclose exclusion of unsaved text edits. A separately named Current encoded preview can show the edited text encoded to its target with explicit failure policy. Read only visible ranges plus bounded overscan; unavailable original bytes display gaps. Editing is disabled in v1.

### SDK walkthrough

`docs/extensions/` contains: project template, manifest reference, capability model, the command and panel APIs, edit transactions and revision conflicts, packaging and signing, local sideload, catalog submission by pull request. The walkthrough is written against json-tools and must let a new author build a working formatter in under an hour.

### Catalog publication

The `bareline-extensions` repository holds `catalog.json` (signed with the owner's minisign key per ADR-35) and per-extension metadata. This PR adds the three entries, the release workflow that builds and hashes the `.blex` files, and the review checklist for third-party submissions.

### Harness

`tests/extensions/` starts the PR-016 host in a test mode, installs each `.blex` from the build output, runs the commands against fixtures and checks results. Crash and hang injection reuses PR-016 test hooks.

## Minimum verification scenarios

- JSON and XML format, minify and validate golden tests on fixture corpora including deeply nested, Unicode and malformed inputs with expected error positions.
- 1 GB generated JSON is validated with bounded memory through snapshot ranges; the tree panel expands the root without materializing the whole document.
- Hex view over the 5 GB fixture reads only the visible range and goto offset is immediate.
- Killing the extension host while a formatter is running leaves the editor and document intact; the pending `ApplyEdits` is rejected as stale or never arrives.
- Catalog install path end to end: fetch signed index, verify, download `.blex`, verify hash, install, enable, run a command.
- Sideload of a `.blex` with a modified hash is refused with a clear message.
- A new author follows the walkthrough in a clean environment and produces a working extension; record the elapsed time in the PR evidence.

## Implementation sequence

1. Mark `PR-027` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] All three extensions install from the catalog and by sideload, and run their commands
- [ ] Each extension declares only the capabilities listed above
- [ ] Hex view and JSON validate work on multi-GB fixtures with bounded memory
- [ ] SDK walkthrough verified by a fresh author within one hour, time recorded
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-027: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.3 delivery slices and acceptance

**Normative contracts:** FC-01, FC-07 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-022. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

| Slice name | What compiles and is testable at slice end | AC cases or minimum scenarios that prove it |
|---|---|---|
| JSON fidelity | First-party formatter preserves numeric lexemes and incrementally indexes bounded trees. | AC-027-01; format large integers and exponent notation. |
| XML security | XML parsing and bounded XPath reject external entities and unsupported constructs. | AC-027-02; attempt remote DTD and excessive depth. |
| Hex domains | Hex panel distinguishes original disk generation from current encoded preview. | AC-027-03; inspect dirty UTF-16 text in original-byte mode. |

- [ ] **AC-027-01**: Format JSON containing large integers and exponent lexemes; preserve numeric values and documented lexical policy.
- [ ] **AC-027-02**: Reject XML DTD/external entities/XInclude and unsupported XPath syntax; network stays inaccessible and depth/time limits hold.
- [ ] **AC-027-03**: Open Hex original-byte mode on UTF-16 with unsaved text edits; display original generation explicitly and distinguish an encoded edited preview.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
