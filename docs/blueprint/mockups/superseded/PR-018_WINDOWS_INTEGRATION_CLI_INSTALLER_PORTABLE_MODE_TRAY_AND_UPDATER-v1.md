# PR-018 — Windows Integration, CLI, Installer, Portable Mode, Tray and Updater

**Tracker row:** `PR-018` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-004`, `PR-011`, `PR-012`, `PR-015`  
**Primary objective:** Finish Windows-native distribution and integration without contaminating cross-platform core contracts.

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

- CLI parser (`lexopt`, ADR-09) and file/line/column/read-only/monitoring/no-session/no-extension options
- Single-instance named-pipe handoff before window creation, inside the startup budget (ADR-33)
- Explorer context menu optional registration
- File association UI/installer choices
- Open containing folder/terminal
- System tray modes
- Inno Setup x64 installer (per-user default, optional per-machine), portable ZIP and winget manifest (ADR-31)
- SHA-256SUMS and minisign signature for every release artifact (ADR-35)
- Update manifest/check/download/stage/apply helper using WinHTTP through `windows` (ADR-09)
- Minisign manifest verification, Authenticode verification via `WinVerifyTrust`, hash verification and rollback (ADR-35)
- Optional extension host pack (`bareline-exthost-x64`) download and verification through the same updater channel (ADR-20)
- Portable path relocation and no-registry guarantee

## Explicit non-goals

- No silent takeover of defaults
- No always-elevated process
- No MSIX/Microsoft Store package in v1 (v1.1, ADR-31)
- No `reqwest`, bundled TLS or async runtime for the update check (ADR-09)

## Likely files / ownership

- `apps/bareline/**`
- `apps/update-helper/**`
- `crates/platform-windows/**`
- `packaging/windows/**` (Inno Setup script, winget manifest, checksum and signing scripts)

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### CLI parser and IPC

`bareline.exe` parses documented options with `lexopt` before window creation (ADR-09). Reuse-instance sends a versioned `OpenRequest` over per-user named pipe to existing process, including paths/options after local validation. Receiver revalidates all data. If IPC fails safely, start new instance according to policy. CLI parsing and the handoff attempt are part of the startup budget (ADR-33): they may read no document bytes, touch no network path and must complete with a bounded pipe-connect timeout so a stuck instance cannot delay a fresh launch beyond that timeout.

### Windows shell

Installer offers opt-in context menu/file associations. Register per-user where possible; no silent default takeover. “Open containing folder/terminal” uses safe direct APIs/arguments. Shell commands quote paths correctly and never pass document path through `cmd /c` unless that is the explicit action.

### Installer/portable

The installer is an Inno Setup script in `packaging/windows/` (ADR-31). Default mode is per-user (`PrivilegesRequired=lowest`, install under `%LOCALAPPDATA%\Programs\Bareline`) with no elevation; an optional per-machine mode is offered on the first page. Requirements: clean uninstall that preserves user data unless explicitly chosen, repair/upgrade in place, no bundled third-party software, deterministic artifact from a pinned Inno Setup version. Also produce the portable ZIP and a winget manifest (`packaging/windows/winget/`) that validates with `winget validate`. Every release artifact gets an entry in `SHA-256SUMS` and a minisign signature `SHA-256SUMS.minisig` (ADR-35). MSIX for the Microsoft Store is v1.1. Portable marker relocates config/session/recovery/extensions and does not touch registry.

### Signing

Release binaries and the installer are Authenticode-signed through SignPath Foundation (free for open-source projects) with Azure Trusted Signing as fallback (ADR-35). The update manifest and `SHA-256SUMS` are signed with a minisign ed25519 key held offline by the owner; the public key is embedded in `bareline.exe` and in the update helper. Signing happens in CI on tagged releases only; unsigned local builds identify themselves as such in About.

### Updater

Editor checks signed metadata off-thread using WinHTTP through the `windows` crate; no `reqwest`, bundled TLS or async runtime (ADR-09). Order of verification: minisign signature of the manifest, then package SHA-256 against the manifest, then Authenticode via `WinVerifyTrust` on the staged executable. Stage package in private per-user directory, then hand to tiny update helper after editor exits. Helper repeats all three checks on the exact staged file handle before replacement and supports rollback. Never execute a file merely because its filename/version matches. The same channel and checks serve the optional extension host pack `bareline-exthost-x64` (ADR-20): the manager requests it, the updater downloads and verifies it, and the manager installs it under the extensions data directory.

### Tray/jump list

Tray is opt-in behavior with New/Open/Find actions through command/IPC model. Jump list/recent integration must not require pre-reading files or network paths at startup.

### Notepad++ migration helper

Add a one-time optional importer for safe user data that can be mapped: themes/UDLs, selected shortcuts, recent/session paths (subject to trust), and simple preferences. Do **not** import native plugins or blindly copy XML. Produce a report of imported/skipped items.

## Minimum verification scenarios

- CLI paths with spaces/Unicode/device-like strings are parsed and transferred correctly.
- Concurrent launches do not corrupt session or lose open requests.
- Installer upgrade preserves settings/recovery and pinned tabs.
- Portable mode writes nothing to expected installed config/registry locations.
- Tampered update manifest/package/signature is rejected before apply; rollback restores prior executable.
- Imported Notepad++ session cannot trigger untrusted UNC access without PR-015 policy.
- `winget validate` passes on the generated manifest and the manifest hash matches the released installer.
- Installer per-user mode completes on a standard user account with no UAC prompt and no writes under `Program Files` or `HKLM`.
- Extension host pack download through the updater verifies minisign and hash before extraction; a tampered pack is rejected.
- Launch with an existing instance that is hung: the pipe connect times out and a new instance appears within the startup budget.

## Implementation sequence

1. Mark `PR-018` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Portable run leaves registry untouched
- [ ] Installer uninstall preserves user data unless explicitly chosen
- [ ] Single-instance handoff handles Unicode paths
- [ ] Tampered update is rejected before execution
- [ ] Per-user Inno Setup install needs no elevation; winget manifest validates
- [ ] Every release artifact has a SHA-256SUMS entry and a minisign signature
- [ ] Update check uses WinHTTP only; no HTTP client crate or async runtime in `bareline.exe`
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-018: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-04, FC-07, FC-08, FC-09 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-025, FR-026, FR-027, FR-028, FR-031. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Package signing; verified online provider; Windows integration. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-018-01** — Reject an expired, rollback, wrong-channel, wrong-publisher or hash-mismatched update before staging application files.
- [ ] **AC-018-02** — Verify signatures on inner executables after extracting final ZIP/installer; compare reproducibility only before signing.
- [ ] **AC-018-03** — Launch CLI with non-BMP paths and arguments via authenticated same-user IPC; portable mode writes only to its configured data root.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
