# PR-021 — Windows Product Polish, Parity Matrix and Release QA

**Tracker row:** `PR-021` in `../03_PR_TRACKER.md`  
**Depends on:** every other PR: `PR-004`, `PR-005`, `PR-006`, `PR-007`, `PR-008`, `PR-009`, `PR-010`, `PR-011`, `PR-012`, `PR-013`, `PR-014`, `PR-015`, `PR-016`, `PR-017`, `PR-018`, `PR-019`, `PR-020`, `PR-022`, `PR-023`, `PR-024`, `PR-025`, `PR-026`, `PR-027`  
**Primary objective:** Integrate the Windows v1 experience, close remaining Notepad++ core parity gaps, polish workflows and produce the open-source release with its evidence.

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

- Feature parity checklist against current Notepad++ manual categories
- End-to-end smoke tests
- Keyboard-only pass
- High DPI/dark/light pass
- Installer/portable/update pass
- Recovery/data-loss scenarios
- Extension isolation scenarios (including first-party extensions from PR-027 and the runtime pack download)
- Accessibility pass using PR-024 deliverables
- Performance report as a published comparison table with raw data (ADR-10)
- First-run/onboarding minimal copy
- About/diagnostics completeness, including license and third-party notices access
- Release notes template
- Open-source release artifacts: LICENSE files, THIRD-PARTY-NOTICES via `cargo about`, SBOM, SHA-256SUMS with minisign signature, release notes, migration notes, known issues, GitHub Release and winget submission (ADR-34, ADR-36)

## Explicit non-goals

- No new large subsystem unless parity gap is truly core

## Likely files / ownership

- `tests/e2e/**`
- `docs/parity/**`
- `packaging/windows/**`
- `release/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Release parity closure

Audit current Notepad++ manual categories and `05_NOTEPADPP_PARITY_MATRIX.md`. Every core built-in capability must end in exactly one of these states: **Implemented**, **Equivalent Different UX**, **Excluded With Reason**, **v1.1** (deferred with the ADR or issue that defers it), or **Extension-provided (first-party)** where a PR-027 extension ships the capability. “Planned” is not acceptable for Windows v1 acceptance. Rows marked v1.1 must be few and each must name its deferral decision (for example ADR-24 local history, ADR-26 inline compare view).

### End-to-end journeys

Test at least: quick plain-text edit; code/config edit; regex transform; column/multi-cursor transform; huge log search/tail; workspace navigation; UDL import; macro/external command; split/clone/sync; session crash/recovery; extension install/crash; portable usage; install/update/rollback.

### UX polish

The default empty state is immediately editable. Hide optional side/bottom panels by default. Ensure keyboard shortcuts/tooltips/status wording are consistent. No modal dialog should be used where an inline/panel flow is clearer, but do not remove familiar menu access.

### Reliability gate

No open P0/P1 data-loss, corruption, arbitrary-code/update, remote credential exposure, editor-start crash or unrecoverable session defects. Known lower-severity defects are documented with reproduction/workaround.

### Performance report

Publish the side-by-side table produced by PR-019's `xtask perf` run: exact hardware/software configuration, Notepad++ version/settings, Bareline commit/build, renderer mode, raw results per scenario and whether each target from `../01_REQUIREMENTS.md` section 7 was met. No performance number blocks the release (ADR-10); a missed target is published as missed with a tracking issue. Claims in README/site must match the table exactly. The reliability gate below is the only release gate.

### Release artifacts

Produce the open-source release (ADR-34, ADR-36): Authenticode-signed Inno Setup installer and portable ZIP for x64, `SHA-256SUMS` with its minisign signature, CycloneDX SBOM, `LICENSE` (MPL-2.0) and SDK license files, `THIRD-PARTY-NOTICES` generated by `cargo about`, release notes, migration notes and known issues. Publish as a GitHub Release, submit the winget manifest, and attach the standalone `bareline-exthost-x64` runtime pack. Verify clean Windows 10/11 machines, standard user accounts and offline use.

## Minimum verification scenarios

- Full keyboard-only smoke pass and screen-reader/high-contrast/DPI pass.
- 72-hour soak with representative open tabs/tail/search/extensions disabled/enabled does not show unbounded leak.
- Abrupt termination during edit/save/update cases recovers or preserves original as designed.
- Clean VM install/uninstall leaves no unwanted associations/services/tasks.
- Parity matrix has no unexplained missing core row.
- Every artifact in the GitHub Release has a `SHA-256SUMS` entry and the minisign signature verifies against the embedded public key.
- Fresh clone plus `cargo build --locked --release` on the pinned toolchain reproduces the released binaries or the SBOM documents every deviation.
- Controller marks `Accepted=DONE` only after independent verification/test evidence is attached.

## Implementation sequence

1. Mark `PR-021` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] Every parity row is Implemented / Equivalent Different UX / Excluded With Reason / v1.1 with its deferral decision / Extension-provided (first-party)
- [ ] No P0/P1 data-loss defect open
- [ ] All PR tracker entries other than PR-021 have Accepted or documented supersession
- [ ] Windows package is reproducible and installable
- [ ] Performance comparison table published with raw data; README claims match it
- [ ] GitHub Release carries LICENSE, THIRD-PARTY-NOTICES, SBOM, SHA-256SUMS and minisign signature; winget manifest submitted
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-021: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-01 through FC-10 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-001 through FR-031. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Integrated parity evidence; release QA; honest claim table. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-021-01** — Resolve every atomic case and every registered command to an implementation commit and independent result; exclusions retain rationale.
- [ ] **AC-021-02** — Run the key workflows with keyboard and screen reader on both Windows floor builds and mixed DPI.
- [ ] **AC-021-03** — Publish release readiness with unresolved correctness items, performance measurements and limited parity claims; mockups never count as runtime evidence.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
