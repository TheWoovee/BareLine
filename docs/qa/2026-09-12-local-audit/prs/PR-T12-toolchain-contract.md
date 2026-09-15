# PR-T12 — Make Rust support metadata match tested toolchains

Priority P3. Fixes TECH-015. Dependencies: none. Files: workspace/member Cargo manifests, `rust-toolchain.toml`, `.github/workflows/ci.yml`, README/development docs. No application behavior change intended.

## Evidence

Root `Cargo.toml` declares workspace `rust-version=1.85`, but only distribution inherits it; host declares 1.95, and 29 other members have no effective minimum-version metadata. The pinned local/CI toolchain is 1.98.1. `../evidence/cargo-metadata.json` is the actual resolved package metadata. Do not claim the entire editor supports Rust 1.85 merely because the root has that field. Conversely, do not claim a verified minimum compiler from a successful build on the newer compiler.

## Implementation

1. Decide and document the supported policy: pinned development/release compiler versus a separately tested minimum for reusable SDK/neutral crates. The default recommendation is to state 1.98.1 as the supported workspace compiler until an older toolchain has actually passed qualification.
2. Add intentional `rust-version.workspace=true` to member manifests that share the workspace policy. Preserve a distinct SDK/minimum only with a focused compatibility job and dependency support evidence. Keep host's Wasmtime requirement explicit.
3. Reconcile root metadata, toolchain pin, CI install version, packaging reproducibility records and contributor instructions. Add a small manifest consistency check that rejects accidental missing inheritance or an unsupported dependency minimum.
4. If retaining 1.85 for any crate, test that exact toolchain against its supported targets/features with the locked graph, and fix the code/dependency range or raise its declared minimum. Do not weaken lockfile/reproducibility constraints to make an old compiler pass.

## Acceptance

`cargo metadata --locked` reports an intentional minimum for all 31 members; contributor docs explain what is guaranteed. The supported compiler completes the normal focused gates. Any older minimum has its own actual test evidence. No blanket library downgrade or needless dependency churn is part of this PR. A reader must be able to select the right toolchain without interpreting contradictory manifests.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
