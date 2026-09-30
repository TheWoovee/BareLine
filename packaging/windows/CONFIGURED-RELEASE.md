# Configured Windows release phases

Use PowerShell 7 and Python 3.12 or later. These phases are local, sequential and reusable after interruption. Each phase writes a new directory; retain earlier failed attempts. No phase publishes or bypasses the protected signing process.

## 1. Build and compare

Supply the real public release configuration, including separate release/catalog/offline-root keys. Keep private signing keys outside the repository. Install the pinned toolchain's `wasm32-wasip2` target before building. The builder requires a new target directory, records its environment, normalizes source/target paths and MSVC PE timestamps, and builds the external runtime and three components.

```powershell
./packaging/windows/build-configured.ps1 -Config C:/owner-public/release.json -OutputDir target/configured-a -TargetDir target/build-a
./packaging/windows/build-configured.ps1 -Config C:/owner-public/release.json -OutputDir target/configured-b -TargetDir target/build-b
python scripts/release_pipeline.py compare --config C:/owner-public/release.json --first target/configured-a --second target/configured-b --output target/signing-handoff
python scripts/release_pipeline.py verify-handoff --handoff target/signing-handoff
```

For release qualification, use independently provisioned clean build environments; two local output directories establish a byte comparison only. The handoff preserves both build receipts, original unsigned executables and component bytes. `-ExecutablesOnly` is a diagnostic build and cannot pass the shipping comparison. Preview/fixture capabilities, shared artifact files, mismatched source/config/compiler/bytes and incomplete components fail the comparison.

The separate `configured-build`/`configured-comparison` CI jobs implement this path. Their signing job consumes only `configured-signing-handoff`; it cannot sign the preview job's output. Configured tags must be exactly `v<distribution.version>` (for example `v0.1.0`); preview tags skip configured builds/signing. Hosted execution still needs the owner's repository configuration, protected environment and public config path.

## 2. External signing, then metadata

Have the authorized signing service sign copies of all three files under the handoff's `unsigned` directory. Keep the originals. Local orchestration accepts externally signed files and never impersonates that authorization.

```powershell
python scripts/release_pipeline.py verify-signed --handoff target/signing-handoff --signed-dir target/signed-inner
python scripts/release_pipeline.py prepare-metadata --handoff target/signing-handoff --signed-dir target/signed-inner --expires-unix <reviewed-Unix-expiry> --output target/metadata-signing
```

The byte check permits only the PE checksum/security directory and terminal certificate append to differ. It does **not** establish certificate validity. The metadata phase uses the actual signed hashes/lengths and content-addressed component filenames. It requires a future expiry and creates `signing-request.json` with metadata hashes and separate signing roles. Review authority version/revocations and expiry; existing deployed root policy must never be reset to an earlier version. If rotating roots, supply the old-and-new-signed transition chain alongside the signed authority.

Sign `bareline.update.json` → `bareline.update.minisig` and `runtime.json` → `runtime.minisig` with the release key; `catalog.json` → `catalog.json.minisig` with the catalog key; and `bareline.release-authority.json` → `bareline.release-authority.minisig` with the offline root. Public deterministic test-vector keys are rejected by configured builds.

When configured CI signing is enabled, it performs the inner-executable signing, Windows publisher validation and byte comparison, then generates `configured-metadata-signing-handoff`. This artifact contains `handoff/` (the original compared bytes/configuration), `delivery/` (signed executables, components, unsigned metadata and signing request) and `signing-evidence/` (before/after hashes and the SignPath request ID when applicable). Download and retain that complete artifact. Sign the four metadata files in `delivery/` using their separate authorized roles; use that directory as `-Delivery` below. The offline root key never enters this workflow.

## 3. Assemble verified delivery

Build the small shared verifier once, and generate the complete notices/SBOM/SDK and user documents for the actual candidate. The documents directory must contain LICENSE, SDK-LICENSES.md, THIRD-PARTY-NOTICES.md, SBOM.json, RELEASE-NOTES.md, MIGRATION-NOTES.md and KNOWN-ISSUES.md.

```powershell
cargo build --locked -p bareline-distribution --example verify-authority
./packaging/windows/assemble-configured.ps1 -Handoff target/signing-handoff -Delivery target/metadata-signing -Documents target/release-documents -OutputDir target/assembled-release -AuthorityVerifier target/debug/examples/verify-authority.exe -Iscc C:/tools/InnoSetup/ISCC.exe
```

Assembly rechecks the unsigned/signed relationship, actual Windows publisher, root lineage/floors/expiry, update/runtime signatures and hashes, and catalog/package contents. The same checks apply to the staged copies. The portable ZIP and installer carry exact bootstrap authority files. The runtime and content-addressed components remain external to the core package. The installer is still unsigned at this phase.

## 4. Final signing, inventory and verification

Have the authorized service sign a **copy** of `bareline-0.1.0-windows-x64-setup.exe`. Retain the original unsigned assembly. Prepare the final-byte inventory:

```powershell
./packaging/windows/finalize-configured.ps1 -Handoff target/signing-handoff -Assembled target/assembled-release -SignedInstaller target/external-signing/bareline-0.1.0-windows-x64-setup.exe -OutputDir target/final-inventory -AuthorityVerifier target/debug/examples/verify-authority.exe
```

This checks the installer's Windows publisher against the verified authority, allows only Authenticode changes to the original installer (including Inno's PE32 bootstrap), and binds the portable editor/helper and external runtime to the original compared unsigned executables. It creates `artifacts/`, a public configuration copy and `finalization-request.json`; the original assembly remains unchanged. Sign `target/final-inventory/artifacts/SHA-256SUMS` with the current authority's release key, saving the detached signature **outside** `artifacts/`, then resume:

```powershell
./packaging/windows/finalize-configured.ps1 -Handoff target/signing-handoff -Prepared target/final-inventory -InventorySignature target/external-signing/SHA-256SUMS.minisig -Minisign C:/tools/minisign.exe -OutputDir target/verified-release -AuthorityVerifier target/debug/examples/verify-authority.exe
```

The original handoff is required again: the prepared request cannot replace its source identity or public trust configuration. Changed, added or removed prepared files fail before the detached signature is imported. This phase copies to a new directory, then runs `verify-release.ps1` with the verified authority's current release key and publisher. Verification includes inventory signatures, all executable publishers, actual portable editor/helper, external runtime, signed metadata and catalog packages. Only successful verification creates `verification.json` beside `artifacts/`; it explicitly leaves release approval and clean-machine acceptance pending. Failed attempts remain for diagnosis and cannot be reused as new phase outputs.

After clean-machine installation/uninstallation, interrupted update/recovery and sustained-operation qualification, the owner can approve publication of the exact `target/verified-release/artifacts/` bytes. The configuration/request/verification records are retained evidence, outside the distribution asset set. The configured update host must separately serve the signed metadata and matching raw `bareline.exe` extracted from the verified portable ZIP at the configured paths; attaching a GitHub Release ZIP alone does not create that update endpoint. No helper publishes or grants approval. A compromised sole offline root requires an independently authenticated replacement installer or explicit manual trust reset.

## CI owner configuration and remaining dependencies

Leave `BARELINE_SIGNING_ENABLED` unset until enrollment, public pins and provider policy are ready. Common repository/environment variables are `BARELINE_RELEASE_CONFIG_PATH` (a real checked-in configured public JSON), `BARELINE_PROTECTED_RELEASE_ENVIRONMENT=true` (only after the branch rules and `release-signing` environment are established), `BARELINE_PUBLISHER_SHA256`, `BARELINE_METADATA_EXPIRES_UNIX` (reviewed future Unix expiry), and `BARELINE_SIGNING_PROVIDER` (`signpath`, the only supported provider). Tag `v<version>` must point to a commit on the protected default branch. Every privileged signing request remains behind `release-signing` approval.

The SignPath route uploads exactly the three inner executables and submits that same run's immutable artifact ID using a commit-pinned SignPath action. Configure `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`, `SIGNPATH_SIGNING_POLICY_SLUG`, optional `SIGNPATH_INNER_ARTIFACT_CONFIGURATION_SLUG`, and the environment secret `SIGNPATH_API_TOKEN`. The approved SignPath artifact configuration must accept a ZIP with `bareline.exe`, `bareline-update-helper.exe` and `bareline-extension-host.exe` at its root. Enrollment, the GitHub application, signing-policy reviewers and a production certificate are provider/owner prerequisites; merely committing this adapter does not establish them. See [SignPath's GitHub integration contract](https://docs.signpath.io/trusted-build-systems/github).

There is no certificate-store signing route: a fresh GitHub-hosted runner has no owner signing certificate, and the workflow does not import or fabricate one.

The SignPath adapter currently covers inner executables. Installer signing after external metadata approval still needs a provider-approved signing route; for SignPath Open Source this must preserve its GitHub-hosted origin requirements. Separate production release/catalog/offline-root keys, an actual update host, reviewed documents/SBOM, installer signing and clean-machine qualification remain external dependencies. This implementation has synthetic byte/tamper tests and syntax validation; an end-to-end production signing/verification run requires those real inputs.
