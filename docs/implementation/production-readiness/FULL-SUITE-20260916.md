# Full Windows validation — 2026-09-16

The owner authorized the full suite after integrating the fixes. Work remains sequential. Run the complete Rust workspace suite, diagnostic-feature checks, Python E2E/release/performance/soak tooling, formatting, portability/toolchain checks, Clippy debt ratchet, Windows packaging checks and actual first-party runtime qualification. Retain failures and investigate before scoped reruns. Existing installed-app smoke evidence remains identified separately; VM/signing/physical-device and 72-hour qualification cannot be inferred from automated suite success.

## Initial CI failure and correction

Correctness run `35078535683` stopped on all three hosts at the portability guard. The newly added QA save-fault implementation and read-only recovery probe imported Windows metadata directly from the neutral file-I/O crate. Move these diagnostic implementations into the executable composition root. Keep only a platform-neutral, fail-closed callback in file-I/O; install it explicitly in diagnostic editor builds. Preserve reparse-point rejection and the configured-release guard.

Contracts: [portability guard](../../../.github/workflows/check_portability.py), [correctness workflow](../../../.github/workflows/ci.yml), [Windows lab inputs](../../../tests/e2e/WINDOWS-LAB.md), [evidence retention](../../../tests/e2e/EVIDENCE_RETENTION.md).

The clean-build replicas in supply-chain run `35078535715` built successfully, then failed during offline notice metadata: a dev-only dependency (`base64ct 1.8.3`) had not been fetched by the release build. Fetch the exact locked Windows graph before offline notice generation. Keep offline notice generation and lock immutability enforced. The correctness workflow now uploads first-party telemetry only when that step actually ran, preventing a second misleading missing-artifact failure after an earlier gate stopped the job.

Evidence root: `target/qualification/full-suite-20260916`. Portability and the complete Rust workspace run passed after relocating diagnostics. The first workspace invocation was refused by `--locked` before execution; Cargo then recorded the new optional direct serde edge without changing any dependency version. Remaining suites are in progress.

## Extended checkpoint

Diagnostic dispatch/boundary/recovery checks, rustfmt, toolchain, the exact 755-occurrence Clippy debt ratchet, 123 E2E-tooling tests, 14 release-tooling tests, 21 performance-tooling tests, 5 soak-tooling tests, packaging, synthetic visual checks, runtime boundary and dependency policy passed. Actual first-party Fast 3/3 and Large 2/2 passed, including 1 GiB JSON and 5 GiB hex fixtures.

The next remote correctness run exposed two Windows-only modules (`menu_bar` and `owned_cache`) declared without individual target guards. Guard the entire Windows adapter crate with `#![cfg(windows)]`, preserving its existing Windows behavior while preventing future unguarded child modules from reaching other targets. Real Linux/macOS compilation remains checked by the hosted native compiler jobs, not by pretending to change the local compiler's operating system.

## Hosted follow-up findings

After the adapter correction, Linux/macOS workspace compilation passed. Linux test linking selected the system PCRE2, which lacks `pcre2_set_max_pattern_compiled_length_8`. Require the bundled implementation from the locked `pcre2-sys` dependency through its supported `PCRE2_SYS_STATIC=1` switch, keeping the search memory limit intact.

macOS execution exposed a shared resident-recovery race: an old retirement acknowledgment can arrive after a new checkpoint reuses the alternating directory name, clearing the new checkpoint status. Gate further checkpoint preparation on acknowledged retirement and an empty pending-retirement list. Add a deterministic delayed-acknowledgment regression and run the real rotation/discard tests.

The clean-build notices now pass. SBOM generation then revealed that cargo-cyclonedx's default output name does not match the workflow's `bom.json` input. Set `--override-filename bom` explicitly in both generation steps and verify the actual merged package SBOM with the pinned generator. Keep lockfile and missing-output checks.

The complete nonshipping release-fixture pipeline passed with stable source at `a576de5`: actual fixture-configured editor/helper/runtime binaries, signed fixture catalog/packages, tamper rejection, installed-artifact Fast tests and verified capability/resource manifests. The pinned PCRE2 search suite passed. The deterministic retirement test failed before the fix (`checkpoint started before retirement acknowledgment`), then all four resident recovery tests passed after it. Fresh editor/helper SBOMs and their normalized merge passed with the lockfile unchanged. Final consolidated checks and hosted reruns follow these corrections.

## Final hosted integration findings

The full Windows workspace suite, normal preview build, hardware/software hidden render smoke and real first-party All suite passed on `c33cb6f`. macOS workspace tests passed too. Linux exposed a process-global recovery warning queue: a workspace can consume another workspace's asynchronous cleanup failure. Route each warning using the originating notification owner, retain the bounded queue and real failure reporting, and test isolation between two owners. Do not relax the workspace's terminal-banner assertion.

The macOS native Clippy census has 223 distinct identities / 420 occurrences, every one matching the existing reviewed Windows source/item/lint/context. Add that exact native qualification with the existing per-occurrence justifications and retained census hash. Linux still needs its own actual native census.

The clean-build SBOM correction succeeded. The last version lookup used unrestricted offline metadata, unnecessarily resolving other platforms after a Windows-only fetch. Use `--no-deps` for that workspace package-version lookup; retain offline/locked checks. No dependency graph is needed to read the editor package version.

The final warning-routing checks passed all 150 app tests, 96 file-I/O unit tests and process-death/doc checks. Windows Clippy remains 755 reviewed occurrences with no new/stale debt. The Linux native census from run `35084447795`, job `104755857201`, also has 223 distinct identities / 420 occurrences; all match the existing Windows review fingerprints and justifications. Retain its own native counts, source and census hash. The current normal preview build passed on `6585c4a`. The earlier hosted Windows correctness job passed completely on `c33cb6f`; later source is identified separately.

Linux then passed its qualified Clippy gate and exposed a timing assumption in `power_carets_typing_bookmarks_and_group_undo_use_committed_snapshots`: a completion receipt can precede scheduler-slot release, so immediate grouped Undo may return the documented transient `Saturated` admission result. Match the existing document-service test contract: bounded retry of only that pre-admission condition, with fresh deadlines and all snapshot/bookmark/group-Undo assertions unchanged. This is a test-only correction and does not require rebuilding the installed product.
