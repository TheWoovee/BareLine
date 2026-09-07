# PR-016 — Extension Platform and Isolated Plugin Manager

**Tracker row:** `PR-016` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-001`, `PR-011`, `PR-012`  
**Primary objective:** Replace fragile in-process plugin coupling with a versioned, isolated extension system and clean manager UX.

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

- .blex manifest/package format
- extensions-protocol and extension SDK crates (MIT OR Apache-2.0, ADR-34) with version negotiation
- bareline-extension-host process built on `wasmtime` component model, shipped as the optional signed `bareline-exthost-x64` pack (ADR-20)
- Runtime pack download/verify/install flow with "Runtime not installed" state and progress
- Named-pipe framed RPC
- Capability grants and prompts
- Command/panel/document transaction APIs
- Extension manager with Installed / Discover / Updates / Disabled tabs, permission chips and "Runs isolated" copy
- Install/update/disable/uninstall manager
- Package hash/signature/catalog verification (minisign-signed JSON catalog, ADR-35)
- Crash/timeout isolation and diagnostics

## Explicit non-goals

- No direct loading of Notepad++ DLL plugins
- No unrestricted native executable plugin by default
- No bundling of the WASM runtime into the core installer or `bareline.exe`
- No first-party extensions here; they are PR-027 and consume this API

## Likely files / ownership

- `crates/extensions-protocol/**`
- `crates/extension-sdk/**`
- `apps/extension-host/**`
- `crates/ui/src/extensions/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Package and manifest

`.blex` is a zip-like package with safe extraction rules or can remain archive-read-only. `manifest.toml` includes extension ID (reverse-domain or stable namespace), version, host API range, entry component, commands/panels contributions, requested capabilities, publisher/signature metadata and package hash.

Extension ID/version are data, never filesystem paths. Installation directory name is derived/sanitized by Bareline.

### Runtime isolation

`bareline-extension-host.exe` is a separate process. Execution is WebAssembly Component Model/WASI through `wasmtime` and `wasmtime-wasi` with only brokered capabilities (ADR-20). `wasmtime` is approved only in the host binary, never in `bareline.exe` (ADR-09). If a future native-extension bridge is added, it must be a separate host process with stronger warnings; never load arbitrary DLL into `bareline.exe`.

### Runtime pack distribution

Use the VerifiedPackageSource contract from PR-001. PR-016 delivers signed offline install and a fail-closed online-unavailable provider. PR-018 later integrates verified downloads; enabling an extension then shows download size, verification and Cancel before installation. Installed files persist while disabled and are removed only by explicit Remove runtime. The core editor stays usable when install/verification fails. FC-07/08 govern expiration, publisher/hash checks, protocol compatibility and safe extraction.

### SDK and licensing

`crates/extension-sdk` wraps the protocol for extension authors (WIT world, Rust bindings, manifest builder, test harness). SDK, protocol crates and first-party extensions are MIT OR Apache-2.0 so extension authors face no copyleft (ADR-34). PR-027 first-party extensions are the reference consumers of this SDK; keep the API stable enough that they need no private hooks.

### Protocol

Versioned postcard frames over authenticated same-user named pipe, with ACL, session nonce and expected-process validation. Cap at 8 MiB before allocation, 1 MiB payload chunks and bounded per-extension pending requests. Every message carries request ID, extension ID, protocol version and capability context. Cancel/timeouts release staged data (FC-07).

### Document API

read_text_range uses TextOffset and document revision; read_original_bytes uses RawOffset and source generation. Capabilities and range availability apply to both. Small ApplyEdits and chunked staged BeginEdits/AppendChunk/CommitEdits paths validate all ranges and base revision before atomic document commit. No extension gets pointers or best-effort stale edits. Enforce per-extension 128 MiB memory and default 5-second interactive deadline with epoch interruption; longer jobs require explicit declared background execution.

### Capabilities

At minimum: document.read, document.edit, workspace.read, workspace.write, network, process.spawn, ui.panel, settings. Permissions are granted per extension and revocable. Catalog metadata may recommend capabilities but cannot pre-grant them.

### Lifecycle

Host starts lazily when an enabled extension needs execution. If no extensions are enabled, no host process/thread/runtime memory remains. Detect crash/hang; disable or restart according to policy and show concise diagnostics. One bad extension must not block editor startup.

### Catalog/update

The catalog index is a JSON document (`catalog.json`, ADR-21) signed with minisign (ADR-35) and hosted in the public GitHub repository `bareline-extensions`, where new entries and updates arrive as pull requests reviewed by maintainers (ADR-36). The editor embeds the catalog public key and verifies the index signature before parsing; packages are hash/signature verified before extraction. Sideload shows publisher/hash/capabilities. Update cannot silently add new dangerous capabilities; require approval.

### Manager UI

Tabs are **Installed / Discover / Updates / Disabled**. Each row shows name, publisher, version, a chip per granted capability (for example `document.edit`, `network`) and the fixed copy "Runs isolated in a separate process". Discover is empty with a clear message when the runtime pack or the catalog is unavailable offline. Built with PR-023 controls.

## Minimum verification scenarios

- Kill/hang/fuzz extension host while editing; editor and documents remain intact.
- Oversized/malformed frames rejected before large allocation.
- Extension requests workspace write/network/process without grant and receives deterministic denial.
- Stale revision edit cannot apply.
- Extension update adding capability requires explicit re-approval.
- Zero enabled extensions => host stopped, no WASM runtime in the editor process. Previously installed runtime files remain until explicit Remove runtime; a fresh core install has none.
- Tampered `bareline-exthost-x64` pack signature or hash is rejected before extraction; the editor stays usable.
- Catalog index with an invalid minisign signature is rejected before parsing.

## Implementation sequence

1. Mark `PR-016` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Killing extension host leaves editor alive and documents intact
- [ ] Extension cannot edit without granted capability and valid revision transaction
- [ ] Disabled extensions cost no persistent editor-process memory
- [ ] Protocol backward-compat test fixtures exist
- [ ] Runtime pack is downloaded only on first enable and only after signature and hash verification
- [ ] Manager shows Installed / Discover / Updates / Disabled with permission chips and "Runs isolated" copy
- [ ] SDK and protocol crates carry MIT OR Apache-2.0 license metadata
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-016: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-07, FC-08 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-022. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Verified offline runtime; capabilities; host quotas. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-016-01** — Install a signed offline runtime and extension using the PR-001 provider contract; no PR-018 online code is required.
- [ ] **AC-016-02** — Disable all extensions; host exits and installed files remain. Explicit Remove runtime deletes only its verified owned files.
- [ ] **AC-016-03** — Send oversized frames, unauthorized capabilities and a CPU loop; reject/terminate the offending extension within per-extension limits.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
