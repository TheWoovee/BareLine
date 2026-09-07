# PR-019 — Huge-File Optimization and Performance Benchmark Suite

**Tracker row:** `PR-019` in `../03_PR_TRACKER.md`  
**Depends on:** `PR-002`, `PR-003`, `PR-005`, `PR-008`, `PR-009`, `PR-015`  
**Primary objective:** Complete the `xtask perf` scenario set that PR-001 started, profile and harden the complete huge-file path, decide the default renderer mode and resident threshold from data, and publish the side-by-side table that substantiates “faster/lighter” claims. Performance numbers are targets tracked continuously, never merge gates (ADR-10).

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

- Fixture generator for 10MB/100MB/1GB/5GB logs and long lines
- Complete the `xtask perf` scenario set on the PR-001 skeleton: open/first-paint, scroll, edit latency, search, save, workspace scan, tail append
- Scroll/input frame instrumentation
- Search throughput/result-jump benchmarks
- Syntax viewport latency and cache tuning
- Sparse line index tuning and `document.resident_max_bytes` default tuning (ADR-01)
- Memory collection: private bytes targeted, working set reported (ADR-10)
- Notepad++ side-by-side benchmark harness/instructions and the published comparison table
- Renderer default-mode decision from hardware vs software measurements (ADR-32)
- Extend the PR-001 nightly perf job and regression-issue automation to the full scenario set

## Explicit non-goals

- Do not redesign features solely to win synthetic benchmarks
- Do not commit giant fixture files
- Do not turn any performance number into a merge gate or CI failure (ADR-10)

## Likely files / ownership

- `xtask/**`
- `tests/perf/**`
- `crates/document/**`
- `crates/search/**`
- `crates/editor-surface/**`
- `crates/syntax/**`

Do not treat this list as permission to create circular dependencies. If a shared contract is missing, place it in the lowest-level neutral crate that owns the concept.

## Detailed design and interfaces

### Benchmark harness

`xtask perf` already exists from PR-001 with cold launch and idle private bytes, JSON output under `tests/perf/results/`, a nightly GitHub Actions job and automatic regression issues above 10 percent (ADR-10). This PR extends it: reproducible fixture generation, the full scenario list below, machine/OS/build/backend/config metadata on every record, and a Notepad++ runner that drives current stable Notepad++ with plugins disabled and comparable settings on the same machine. Never publish a “faster” claim from different machines/configurations.

### Scenarios

At minimum measure cold/warm launch, empty idle private bytes and working set, 10 MB/100 MB/1 GB/5 GB open-to-interactive, scroll frame/stall distribution, edit latency, syntax viewport readiness, literal/regex search throughput/cancel latency, result jump latency, save/Save As, workspace scan and monitoring append throughput. Run the launch, idle and scroll scenarios in both renderer modes (ADR-32).

### Instrumentation

Add lightweight opt-in spans/counters for UI frame, document read/index, shape/layout, syntax queue, search batches and memory/cache sizes. Instrumentation is disabled/low-cost in normal release. Capture long main-thread stalls with command/source label.

### Optimization order

1. eliminate accidental full-file work/copies;
2. remove UI-thread blocking;
3. bound caches/allocations;
4. optimize hot algorithms/data layout;
5. renderer/layout caching;
6. only then micro-optimize.

Do not weaken correctness/data safety or disable core features globally to win benchmarks. Huge files may defer non-visible work, but visible editing/search must remain functional.

### Performance targets and regression tracking

Targets (ADR-10): time metrics at or below 0.8× Notepad++ P50 on the same machine; idle private bytes at or below 1.0× Notepad++ P50 with an engineering budget of 25 MB; working set reported but not targeted because shared DLL pages dominate it. Absolute engineering budgets from `../01_REQUIREMENTS.md` section 7 are the numbers agents design against.

Tracking, not gating: no performance number fails a PR or a CI run. The nightly job appends results to `tests/perf/results/` as JSON with a noise-tolerance band per scenario; a regression above 10 percent against the rolling baseline opens a tracking issue automatically with the offending commit range. Regressions are explained or fixed through normal PRs. Release readiness is a published comparison table with raw data and methodology, and marketing may claim "faster and lighter" only for metrics where that table shows it.

### Data-driven defaults

Two settings defaults are decided by this PR from measurements and the rationale is committed next to the results:

- **Renderer mode (ADR-32).** Direct2D hardware is the shipped default unless the software render-target mode is within 10 percent on frame time for the standard scroll workload and saves more than 5 MB idle private bytes, in which case software becomes the default. The mode stays a user setting either way.
- **Resident threshold (ADR-01).** Tune `document.resident_max_bytes` (initial 256 MiB) by measuring open-to-interactive and memory for Resident versus Paged sources across the fixture sizes; record the chosen value and the crossover data.

### Profiling

Document Windows ETW/WPA, heap/profile and Rust sampling workflow. PR should produce evidence identifying dominant launch/RSS/huge-file costs and implement the highest-value fixes within scope.

## Minimum verification scenarios

- Benchmark runner can execute twice and produce comparable schema/data.
- 5 GB open does not allocate 5 GB nor perform full syntax/line scan before first viewport.
- Search and indexing cancellation meet bounded acknowledgement target.
- No >100 ms UI-thread stalls during defined 1 GB scroll/search workload unless OS-level external event is documented.
- Publish side-by-side raw Bareline/Notepad++ baseline table with methodology, not marketing-only percentages.

## Implementation sequence

1. Mark `PR-019` → **Selected = DONE**, **Implemented = IN_PROGRESS** in the tracker.
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

- [ ] 1GB/5GB targets from requirements measured and published in the side-by-side Bareline/Notepad++ table with raw data and methodology
- [ ] No full-file copy introduced by syntax/search
- [ ] Raw results include machine/build metadata
- [ ] Performance findings lead to code fixes within this PR scope where safe
- [ ] Renderer default mode decided from data and recorded in tests/perf/results/
- [ ] `document.resident_max_bytes` default tuned from data with rationale recorded
- [ ] No CI job fails on a performance number; regressions open tracking issues instead
- [ ] `cargo fmt --all -- --check` passes for touched code.
- [ ] Relevant `cargo clippy` target(s) pass with warnings denied.
- [ ] Focused automated tests are recorded in the tracker evidence field.
- [ ] No unrelated UI redesign or feature creep was introduced.
- [ ] New persistent/API contracts are documented in code and versioned if applicable.

## Tracker update example

```text
PR-019: Selected=DONE; Implemented=DONE; Verified=NOT_STARTED; Tested=DONE/NOT_STARTED per workflow; Accepted=NOT_STARTED
Evidence: impl=<commit>; tests=<commands + counts>; notes=<known follow-up if any>
```

## v1.2 review resolutions and delivery slices

**Normative contracts:** FC-02, FC-06, FC-10 in [09_FOUNDATION_CONTRACTS.md](../09_FOUNDATION_CONTRACTS.md). **Requirement coverage:** FR-001, FR-007, FR-018. Read [interaction states](../11_UI_INTERACTION_SPEC.md) for affected UI.

Deliver independently reviewable increments in this order: Benchmark methodology; populated tabs; measured claims. Each increment needs a compiling contract and its own evidence; this numbered brief is a feature package, not a requirement for one oversized code review. Do not mark the full package complete while later integration is missing.

- [ ] **AC-019-01** — Run paired baseline trials with pinned executable hashes, alternating app order and separate first-frame/editable/full-load timings.
- [ ] **AC-019-02** — Measure 100 and 500 tabs plus extension process memory; report peak total private bytes, disk growth and downloads separately.
- [ ] **AC-019-03** — Publish all raw runs, sample counts, P50/P95 and timeouts; hosted CI noise never becomes a comparative marketing claim.

Evidence for these cases is **NOT_STARTED**. Record commit, fixture, OS/build, command, result and reviewer in [acceptance and traceability](../10_ACCEPTANCE_AND_TRACEABILITY.md); document edits and images are not application test results.
