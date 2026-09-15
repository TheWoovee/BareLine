# PR-T15 — Qualify the product's speed and resource claims

Priority P1 before making the comparative product claim; qualification, not a proven regression. Fixes TECH-018. Depends on P1 data-safety fixes and PR-T10/T11 for configured extension scenarios. Reuse `tests/perf` and the existing native driver/receipt formats. Do not create a parallel benchmark framework.

## Why this is still needed

The blueprint's leading promise is faster startup, lower weight and useful Notepad++ parity. Unit tests, synthetic rendering and debug smoke cannot establish it. This audit executed the first-party release workload (1 GiB JSON validation in 88.588 seconds on this machine, 16,384 requests of up to 64 KiB), but that is one functional workload, not a controlled comparative product benchmark. The comparative release qualification remains absent here.

## Implementation/qualification sequence

1. Create a concrete local manifest pinning both release executables, exact versions/hashes, source/configuration, hardware, OS build, power mode, DPI, fonts, syntax/wrap options, fixtures and cold-cache method. Obtain comparator bytes through the already approved owner process; do not invent a hash or silently download/run unknown executables.
2. Run the existing full registry with hardware/software renderer modes, cold/warm startup, empty idle memory, open sizes, giant lines, edit-to-paint, scroll, search/cancel/result jump, save/save-as, workspace scan, tail and optional runtime memory. Use generated scratch data and isolated profile/TEMP directories.
3. Record latency distributions and peak/steady memory, thread/handle counts, temporary/recovery/cache growth, cancellation completion and UI responsiveness. Keep setup/compilation/download cost separate from ongoing operation while reporting both. A timeout or skipped scenario stays unavailable/failed, never zero.
4. Compare matched cohorts only. Inspect the actual release build for expensive paths flagged in this audit: queued jobs behind blocked workers, synchronous recovery materialization/retirement, and first-party cold compilation/broker batching. Create a separate code-fix PR for a measured regression with its trace, not a broad speculative optimization.
5. Publish the local table and limitations in the capability ledger. Choose renderer/storage defaults from the evidence as required by the blueprint. Performance remains informational for merges; do not turn a noisy timing threshold into a developer-blocking CI gate.

## Acceptance

Every advertised comparative claim has a reproducible paired row and retained raw receipts; missing measurements prevent that claim. Repeated samples use the existing plan's counts and support floor/environment matrix. Functional failures discovered during measurement remain bugs regardless of speed. This PR can complete its tooling/reporting slice while marking owner-provisioned comparator/hardware cases pending, but it cannot claim product superiority until those cases actually run.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
