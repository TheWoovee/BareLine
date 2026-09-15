# PR-T10 — Make real first-party component execution part of qualification

Priority P2 qualification. Fixes TECH-013. Depends on PR-T08 when worker launch changes land. Primary files: `scripts/test-first-party.ps1`, `apps/extension-host/tests/first_party.rs`, host `src/lib.rs`, extension transport, `extensions/{json-tools,xml-tools,hex-view}`, and CI. Read the final audit test report for both debug and release outcomes before implementation.

## Gap and current evidence

Four real-component integration tests are `#[ignore]`; normal `cargo test --workspace` and correctness CI skip them. Fresh WASI release artifacts were built in this audit. Opt-in debug-host execution passed the bounded 5 GiB Hex case but failed three tests, including XML invocation exit 124 before any broker request and the 1 GiB JSON timeout. The prescribed release host profile then passed **all four tests** serially (95.13 s overall; 88.588 s for the recorded JSON workload). These are execution/profile-sensitive observations. Do not invent a broken release component or “fix” its behavior merely to disguise debug overhead; the remaining demonstrated gap is automated real-component coverage and a clear supported test profile.

A separate serial rerun of only the small authenticated component test also failed in the debug host on XML exit 124 (12.97 s; [raw log](../evidence/first-party-debug-small-serial.log)). Parallel test contention alone therefore does not explain that failure. Compilation/initialization overhead remains a hypothesis until phase telemetry measures it; do not state it as a proven root cause.

## Implementation

1. Split first-party integration into a fast release-profile suite and opt-in large-workload qualification. Fast suite must build the actual current WASI components and exercise JSON format/validate/tree, XML format/XPath/error handling and Hex original-byte navigation across the authenticated process boundary. Do not replace this with WAT-only host tests or pure library calls.
2. Record per-phase timings and sizes: authenticated connection, component hash/read, compile, instantiate, first broker request, execution, response and shutdown. Correlate parent and host deadlines. Determine which phase caused the observed timeout before changing a budget.
3. Keep interactive and background budget semantics explicit. If compilation consumes the interactive budget, either prewarm/cache safely by authenticated component hash and runtime version or expose a bounded preparing phase with cancellation; execution must still have its own resource limits. Never extend all timeouts arbitrarily or bypass the watchdog to hide a failure.
4. For large JSON/XML, verify streaming/linear scanning and bounded broker batching. Preserve original encoding, byte provenance, undo grouping and result-size limits. Use the existing 1 GiB JSON and 5 GiB synthetic Hex fixtures; separate setup cost from execution and explain the coverage of synthetic sources.
5. Add the fast real-component suite to a Windows release-profile CI job/local gate. Keep long performance qualifications informational per the blueprint, but a semantic failure must be recorded and triaged. Emit artifact hashes, runtime profile and actual observations.

## Acceptance

All small signed/test-authenticated components must actually execute, yield valid expected edits/results, reject invalid permissions, and cancel/timeout cleanly without harming the editor. The large suite must produce either successful bounded evidence or an explicit non-shipping limitation with a further fix PR; no ignored case becomes an implied pass. Run serially for baseline and then under controlled concurrency to separate resource contention. Document the appropriate developer command, warm/cold behavior and which build configurations support the feature. Keep the production trust boundary intact.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
