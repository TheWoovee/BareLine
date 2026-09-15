# PR-T11 — Release feature configuration

## Final integrated verification — 2026-09-12

The root completed the connected nonshipping release build, real catalog/runtime/three-component installation, tamper checks, installed Fast 3/3 and installed Large 2/2. The final workflow's extended-Windows-path reporting failure was corrected in reviewed `a47369a108295d487600da7efe6fca0ad430a8e8`. The corrected reporter validated the exact retained source/configuration/executed artifacts without another build. See [final status](FINAL_STATUS.md) and [T11 final review](reviews/T11-R4.md) for actual receipts and qualification limits. This supersedes the historical local-runtime-pending statements below; owner production signing/distribution checks remain pending.

## Scope

- Issue: TECH-014.
- Baseline source commit: `67ee505ba9fc5e9754b237c6b18ea5fd1916d90a`.
- Dependencies: reviewed T05 integration in the baseline; T10 first-party runtime qualification remains separately owned and is not repeated here.
- Boundaries: local configuration, validation, packaging metadata, fixture coverage, and public evidence only. No owner credentials, private signing material, owner endpoint contact, publication, deployment, installation, or signing.

## Implementation record

- Added `release-config-v1`, a strict JSON Schema plus one semantic validator. It rejects unknown/duplicate fields, malformed Minisign keys, shared keys, known nonshipping pins in configured mode, malformed certificate hashes, inconsistent channel/version/floor data, unsafe paths, IP/local/reserved/mixed-case hosts, and any fixture endpoint other than `fixture.invalid`.
- Added explicit build modes. Default Cargo builds compile `preview` with updates/extensions disabled and the external runtime truthfully declared. `configured-release` requires a generated configured receipt; `fixture-release` requires fixture mode, embeds the nonshipping assertion, and cannot pass configured packaging validation.
- Removed the combined `BARELINE_OWNER_TRUST` developer-shell input. The receipt carries canonical validated JSON plus derived values. Every configured build-script execution calls the single validator to rederive and compare all fields, config digest and the named T09/T10 `git-status-diff-untracked-v1` source identity before compiling a public assertion. Cargo watches the enumerated source inputs and consumes the validator's returned receipt bytes, so it cannot parse a different prevalidation read.
- Added strict capability-manifest verification: exact schema/roles/filenames, hashes, nonzero sizes, claims, paths, and per-component embedded assertions. Separate core, runtime, catalog/signature and component inventories now carry the same config/source/version/feature provenance.
- Made first-party `.blex` creation deterministic and overwrite-refusing. Added an explicit extension-resource assembly step which keeps the runtime, catalog, and three components outside the editor/core archive.
- Refactored update retrieval through one verification pipeline. Production still injects HTTPS-only WinHTTP and held-file WinVerifyTrust; the test-only local byte transport injects neither network nor production trust and exercises valid signature, invalid signature, stale floor, wrong channel, wrong certificate, cancellation, package corruption, and unavailable artifact/runtime states.
- Added one root-owned fixture runner which builds optimized nonshipping fixture artifacts, installs all three through a signed generated catalog, and installs the separate runtime through production orchestration with an explicitly fixture-only publisher verifier. The retained installed host and component paths are passed once to T10's existing Fast executor; the final report requires their paths, hashes and sizes to match the installation receipt, package contents, capability manifest and inventories.

## Static evidence

- `python scripts/test_release_config.py` — PASS: all modes, 10 invalid configs, duplicate-key rejection, same-byte validator handoff, source-change invalidation, exact T09/T10 identity parity, tamper-bound preparation, and strict manifest role/claim/path/byte negatives.
- `python scripts/release_config.py validate --config release/preview.release-config.json` — PASS.
- `python scripts/release_config.py validate --config release/fixtures/nonshipping.release-config.json` — PASS.
- Fixture preparation and revalidation probe — PASS; canonical config and named source identity matched, with no endpoint contacted. This parser receipt is not runtime qualification.
- PowerShell parser accepted `build-configured.ps1`, `assemble-extension-assets.ps1`, `test-release-fixture.ps1`, `test-first-party.ps1`, and `package-first-party.ps1`.
- A bounded synthetic parser/report probe accepted the actual four-field T09/T10 source-receipt shape, rejected a changed source identity and default artifact inputs, and linked installed/package/inventory/capability hashes. It did not execute or qualify a runtime.
- Synthetic assembly probe produced separate runtime, catalog, and three-component assets plus their three unsigned JSON inventories, then removed its scratch tree.
- Python AST parsing, exact `rustfmt`, and `git diff --check` passed.

## Root-owned runtime checks

Run serially from the integrated original checkout after source review:

```powershell
python scripts/test_release_config.py
packaging\windows\test-release-fixture.ps1
$env:BARELINE_PREPARED_RELEASE_CONFIG = $null
cargo test --locked -p bareline release_without_owner_pins_cannot_open_or_install_catalog
cargo build --release --locked -p bareline -p bareline-update-helper -p bareline-extension-host --no-default-features
```

Then use an owner-reviewed public configured JSON (no private keys) to prove the configured-feature fail/pass boundary and identical replicas:

```powershell
cargo build --release --locked -p bareline --features bareline/configured-release
# Expected failure: BARELINE_PREPARED_RELEASE_CONFIG was not supplied.
packaging\windows\build-configured.ps1 -Config C:\owner-public\bareline-release.json -OutputDir target\configured-a -TargetDir target\configured-target-a
packaging\windows\build-configured.ps1 -Config C:\owner-public\bareline-release.json -OutputDir target\configured-b -TargetDir target\configured-target-b
```

The precise unsigned component, assembly, signing, verification, and native-matrix sequence is recorded in `packaging/windows/README.md`. T10 owns actual release-profile execution and budgets; the T11 fixture delegates its real component execution to T10's `-Suite Fast` runner without copying or weakening it.

## Risks and pending evidence

- Cargo compilation, Clippy, Rust unit/integration tests, configured binary assertion scanning, native WinHTTP/WinVerifyTrust, helper execution, runtime installation, first-party component execution, replica byte comparison, installer/portable assembly, and final release verification are pending root/owner execution.
- No real owner production config was available. The configured path is therefore implemented and locally parser-probed, but public distribution remains unqualified.
- The capability manifest covers unsigned executable bytes. Signing changes bytes, so final signed inventories and signatures must be regenerated and independently verified.
- Authenticode and Minisign private material remains external. No owner endpoint, certificate store, installer, desktop process, or user profile was contacted or changed.

## Qualification status

Public distribution remains unqualified until owner-controlled production configuration, endpoint availability, signing, independent reproducibility, and the final native validation sequence are completed.
