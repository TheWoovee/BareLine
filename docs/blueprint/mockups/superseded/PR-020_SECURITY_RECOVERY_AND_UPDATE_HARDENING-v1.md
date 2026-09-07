# PR-020 — Security, Recovery and Update Hardening

**Tracker row:** `PR-020` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-004`, `PR-015`, `PR-016`, `PR-018`, `PR-026`  
**Primary objective:** Threat-model and harden the paths most likely to cause data loss or security regressions: files, sessions, recovery, archives, remote paths, extensions and updates.

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

- Threat model document in code repo
- Path canonicalization/reparse escape checks
- Safe archive extraction
- Session/recovery path validation
- Update TOCTOU defenses
- Minisign and Authenticode verification tests, key rotation and revocation procedure in `SECURITY.md` (ADR-35)
- Extension capability abuse tests
- Fuzz parsers for session/manifest/journal where practical
- Security and crash logging/redaction: metadata only, never document bytes (ADR-08)
- Failure injection for disk/lock/power-like interruption, including Paged-source truncation and `SourceChanged` handling (ADR-01, ADR-03)
- Supply-chain controls: `cargo deny`, `cargo audit`, `cargo --locked`, pinned toolchain, SBOM via `cargo cyclonedx`, reproducible-build check
- SignPath signing pipeline hardening

## Explicit non-goals

- No broad cryptography invention; use established libraries/OS APIs

## Likely files / ownership

- `crates/session/**`
- `crates/extensions-protocol/**`
- `apps/update-helper/**`
- `crates/platform-windows/**`
- `tests/security/**`
- `SECURITY.md`, `deny.toml`, `.github/workflows/**` (supply-chain jobs)

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Threat model review

Create explicit threat table for: malicious text/UDL/session/workspace files, path traversal/reparse points, UNC credential exposure, update supply chain/TOCTOU, extension package/RPC, recovery data tampering, command interpolation, archive extraction, unsafe FFI, and external truncation or replacement of a Paged `ByteSource` while open (ADR-01). For the last item verify that the document transitions to `SourceChanged`, unloaded regions report `Unavailable`, no stale or foreign bytes are ever presented as document content, and the process never crashes (ADR-03).

### Path handling

Centralize path classification/canonicalization where possible without resolving untrusted remote paths prematurely. Reject archive entries that are absolute, contain parent traversal, device namespace tricks or symlink/reparse escapes. Use directory handles/final-path checks where a TOCTOU matters.

### Recovery/session hardening

Fuzz parsers; cap lengths/counts before allocation; generated IDs determine journal paths. Session paths are values only. Corrupt one entry must not discard all recoverable state. Recovery files get user-only ACL where supported and cleanup is race-safe.

### Update hardening

Verify signed metadata, package cryptographic hash and code signature at download/stage/apply boundaries. Keep staged directory non-user-writable by other principals. Re-open/re-verify the exact handle/file being executed/applied where possible to prevent swap races. Rollback metadata is similarly authenticated/local protected.

Concretely (ADR-35): the update manifest and `SHA-256SUMS` carry minisign ed25519 signatures verified against the public key embedded in `bareline.exe` and the update helper; installers and executables carry Authenticode signatures produced by the SignPath Foundation pipeline (Azure Trusted Signing fallback) and verified with `WinVerifyTrust`. Add tests with a known-good key pair for: valid signature accepted, altered payload rejected, wrong key rejected, expired or revoked key rejected. Write the key rotation and revocation procedure into `SECURITY.md`: how a new public key is shipped inside a release signed by the old key, how a compromised key is revoked through the manifest, and who holds the offline key. Harden the signing pipeline: signing runs only on tagged builds from the protected default branch, artifacts are hashed before and after signing, and the hashes are published.

### Crash and security logging

The release crash handler (`panic = "abort"`, ADR-08) writes version, build hash, panic location, renderer mode and backend state only. Security logs record blocked paths, packages and capabilities in sanitized form. Add a redaction test that seeds documents with sentinel strings, triggers a crash and a set of blocked operations, and greps every log and crash file for the sentinels.

### Supply chain

CI runs `cargo deny` (licenses and advisories from `deny.toml`), `cargo audit`, and builds with `cargo --locked` on the toolchain pinned in `rust-toolchain.toml`. Release jobs emit a CycloneDX SBOM via `cargo cyclonedx`. A reproducible-build job builds the same commit twice in clean environments and compares artifact hashes; any difference must be explained by a documented nondeterminism source (for example embedded timestamps, which must then be removed or fixed).

### Extension hardening

Fuzz protocol decoder, enforce frame/message/rate/resource limits, deny unknown capabilities by default and ensure host process token/job restrictions practical on Windows. Extension-controlled strings never become shell commands/paths without validation.

### FFI/unsafe audit

Inventory every `unsafe` block in Windows, Lexilla, PCRE2 and runtime bridge. Each has safety comment/invariant. Add sanitizer/fuzz jobs for C/C++ bridge where feasible. Panic/unwind never crosses FFI.

### Security response surface

About/Diagnostics exposes dependency/build versions needed for vulnerability reports without leaking document/recovery content. Document update signing key rotation/revocation procedure.

## Minimum verification scenarios

- Corpus fuzz session/UDL/extension package/RPC parsers without crash/OOM.
- Zip-slip, symlink/reparse, absolute/device-path tests cannot escape staging/install roots.
- UNC-in-session startup produces zero remote access until explicitly trusted.
- Simulated package swap between verify/apply is detected.
- Compromised extension cannot directly mutate editor memory/file without broker permission.
- Security test matrix maps each known Notepad++ issue class researched in baseline to a Bareline control/regression test.
- Two clean CI builds of the same commit produce identical artifact hashes, or every difference is traced to a documented nondeterminism source.
- Minisign verification rejects altered payloads, wrong keys and revoked keys; a key rotation dry run succeeds end to end.
- Sentinel strings placed in open documents never appear in crash files or security logs.
- Truncating a Paged source externally during an edit yields `SourceChanged`, no crash and no foreign bytes in the buffer.

## Implementation sequence

1. Mark `PR-020` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Known classes seen in recent Notepad++ security fixes have explicit regression tests
- [ ] Untrusted paths cannot escape designated dirs
- [ ] Corrupt recovery/session data fails closed while preserving recoverable records
- [ ] Security logs and crash files contain no document text
- [ ] Minisign and Authenticode verification paths have positive and negative tests; `SECURITY.md` documents key rotation and revocation
- [ ] `cargo deny`, `cargo audit`, `--locked` builds and SBOM generation run in CI; reproducible-build check passes or differences are documented
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-020: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-03, FC-04, FC-07, FC-08 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-017, FR-022, FR-027, FR-029, FR-031. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Fault injection; trust attacks; durable receipt reconciliation. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-020-01** — Inject disk-full and process death at every recovery/replace receipt transition; reconcile originals, backups and committed targets by fingerprint.
- [ ] **AC-020-02** — Test archive traversal, reparse escapes, IPC spoofing, stale signed metadata and compromised release-key recovery.
- [ ] **AC-020-03** — Verify diagnostics contain no document text or sensitive path data and no telemetry upload occurs.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
