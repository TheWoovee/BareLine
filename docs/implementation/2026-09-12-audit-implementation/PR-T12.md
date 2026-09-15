# PR-T12: Toolchain contract

- **Baseline:** `c3c13f19e377729809451739a4c5026302b3e16c`
- **Scope:** Align workspace Rust version and edition inheritance, pinned toolchain and CI declarations, contributor documentation, and a static manifest consistency check.
- **Policy:** Describe the compiler version currently pinned and tested as supported. Treat any older minimum as unqualified until it passes the final integrated compatibility run.

## Evidence

- rustc --version --verbose: rustc 1.98.1 (48a229cea 2026-09-01),
  host x86_64-pc-windows-msvc.
- rustup show active-toolchain: repository override
  1.98.1-x86_64-pc-windows-msvc.
- rustup toolchain list: installed stable, 1.97.1, and 1.98.1; no toolchain
  was downloaded.
- python .github/workflows/check_toolchain_contract.py: passed; all 31
  workspace members inherit Rust 1.98.1 and edition 2024; missing-inheritance
  and future-dependency negative fixtures passed.
- The same checker with --metadata and the local audit evidence
  cargo-metadata.json passed against its 409-package resolved graph. The highest
  declared dependency requirement in that evidence is Rust 1.95.0 (Wasmtime 48
  and related packages).
- python -m py_compile .github/workflows/check_toolchain_contract.py: passed.
- git diff --check: passed (Git emitted expected working-copy line-ending
  notices).

The checked-in toolchain, explicit correctness/performance CI declarations, and
workspace package metadata now agree on 1.98.1. The extension-host manifest
inherits that supported policy; contributor documentation separately records
Wasmtime 48's Rust 1.95 dependency floor without presenting it as a qualified
workspace minimum. The supply-chain job selects Python 3.12 through the pinned
setup-python v5.6.0 action before running the manifest checker.

## Deferred qualification

- Supported/minimum-toolchain build and test qualification on the final
  integrated tree. Both names refer to the same pinned Rust 1.98.1 compiler, so
  this requires one run and no older-toolchain download.
- Full native workspace checks owned by integration.
- Fresh cargo metadata --locked validation on the final integrated dependency
  graph; CI feeds that output to the static contract checker.
