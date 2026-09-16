# Full Windows validation — 2026-09-16

The owner authorized the full suite after integrating the fixes. Work remains sequential. Run the complete Rust workspace suite, diagnostic-feature checks, Python E2E/release/performance/soak tooling, formatting, portability/toolchain checks, Clippy debt ratchet, Windows packaging checks and actual first-party runtime qualification. Retain failures and investigate before scoped reruns. Existing installed-app smoke evidence remains identified separately; VM/signing/physical-device and 72-hour qualification cannot be inferred from automated suite success.

## Initial CI failure and correction

Correctness run `35078535683` stopped on all three hosts at the portability guard. The newly added QA save-fault implementation and read-only recovery probe imported Windows metadata directly from the neutral file-I/O crate. Move these diagnostic implementations into the executable composition root. Keep only a platform-neutral, fail-closed callback in file-I/O; install it explicitly in diagnostic editor builds. Preserve reparse-point rejection and the configured-release guard.

Contracts: [portability guard](../../../.github/workflows/check_portability.py), [correctness workflow](../../../.github/workflows/ci.yml), [Windows lab inputs](../../../tests/e2e/WINDOWS-LAB.md), [evidence retention](../../../tests/e2e/EVIDENCE_RETENTION.md).

The clean-build replicas in supply-chain run `35078535715` built successfully, then failed during offline notice metadata: a dev-only dependency (`base64ct 1.8.3`) had not been fetched by the release build. Fetch the exact locked Windows graph before offline notice generation. Keep offline notice generation and lock immutability enforced. The correctness workflow now uploads first-party telemetry only when that step actually ran, preventing a second misleading missing-artifact failure after an earlier gate stopped the job.

Evidence root: `target/qualification/full-suite-20260916`. Portability and the complete Rust workspace run passed after relocating diagnostics. The first workspace invocation was refused by `--locked` before execution; Cargo then recorded the new optional direct serde edge without changing any dependency version. Remaining suites are in progress.
