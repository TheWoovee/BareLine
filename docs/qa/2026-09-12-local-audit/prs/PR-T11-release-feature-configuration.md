# PR-T11 — Produce a configured, verifiable release feature set

Priority P1 release qualification. Fixes TECH-014. Depends on PR-T10 for actual first-party execution and the file-safety PRs before release qualification. This PR prepares local build/configuration and validation; it does not authorize publishing, signing with owner credentials, installing on the user's machine, or creating a remote.

## Current gap

`apps/bareline/src/windows_app/extensions.rs:1022–1051` requires compile-time `BARELINE_OWNER_TRUST` to expose trusted package execution. `windows_app/update.rs::Config::compiled` requires release key/channel/metadata floor/host/paths/certificate. The clean release builds in `.github/workflows/supply-chain.yml` provide none of these. They produce reproducible unsigned preview binaries, not an operational trusted distribution. Fail-closed behavior is correct; replacing `None` with arbitrary trust is not a fix.

## Implementation

1. Add one versioned, schema-validated release configuration containing only public trust pins and endpoint/channel metadata. Keep private signing material entirely external. Validate key formats, certificate digest, semantic channel/package agreement and rollback floor at build preparation time. A release-feature build must fail clearly when configuration is absent or inconsistent; preview mode must remain explicitly supported.
2. Derive all compile-time values for editor and helper/runtime from that reviewed configuration. Emit a public build-capability manifest with configuration digest, versions, source digest, component hashes and supported features. Assert the compiled capabilities against the manifest in tests; do not infer them from an environment variable in a developer shell.
3. Prepare distinct owner artifacts for core portable/installer and optional extension runtime/catalog/components. The existing core payload intentionally excludes the runtime. Add the missing explicit runtime/component packaging stage and final inventory instead of accidentally embedding untrusted code in the editor.
4. Build reproducibility replicas with identical public configuration. Verify the final unsigned bytes and inventories before the separately authorized signing step. Keep test trust keys clearly labeled and restricted to fixture builds.
5. Test download/install/update through an isolated local fixture transport with valid and invalid signatures, stale metadata, wrong channel/certificate, cancellation, package corruption and runtime-unavailable states. Preserve production signature checks; do not contact or modify owner endpoints as part of test setup.

## Acceptance

The build must tell users and QA exactly whether extensions and updates are configured. A fixture-configured core must discover the catalog, install the separately packaged runtime, execute all three first-party extensions and reject tampering. Default preview builds must display truthful disabled reasons. Produce the concrete unsigned resource set and verification commands; list owner-controlled credentials/endpoints/signing as external prerequisites with an exact final validation sequence. Do not mark public distribution complete from local fixture success.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
