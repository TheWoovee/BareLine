# Configured Windows release phases

Use PowerShell 7 and Python 3.12 or later. These phases are local, sequential and reusable after interruption. Each phase writes a new directory; retain earlier failed attempts. No phase publishes or bypasses the protected signing process.

## Publisher identity rule

Bareline uses one publisher identity and one separate signing-certificate pin:

- `trust.publisher` is the publisher identity. The release pipeline writes it into every signed manifest (`bareline.update.json`, `runtime.json`, catalog entries and component manifests), and the app, the update helper and `verify-authority` compare manifests against exactly this string. It is never a certificate digest.
- `trust.authenticode_subject` and `trust.authenticode_issuers` pin the Authenticode signer: the leaf certificate's subject and its issuing CA, compared as Windows simple display names (the names `X509Certificate2.GetNameInfo(SimpleName, ...)` reports), plus the code-signing EKU. `authenticode_issuers` is a rotation list of up to eight issuing CAs. Leaf certificate hashes are not pinned, so certificate renewals and Azure Trusted Signing's short-lived certificates keep working.
- Authenticode never authorizes a file on its own. The signed SHA-256 of the exact file is always required as well: the update and runtime manifests pin `bareline.exe` and the runtime, and the signed release authority's `update_helper_sha256` pins the installed helper. This also covers shared signing certificates such as SignPath Foundation's.

The signed release authority carries the Authenticode pin and the helper hash, so an offline-root-signed authority can rotate the pin. Explicit user-initiated flows (checking for and applying an update, installing the runtime) fetch revocation evidence online. Launch, acknowledgement and recovery paths do not require online revocation; they rely on the signed hash plus the signature.

### What CI covers, and the deferred helper end-to-end job

CI checks the identity rule without the real update helper binary:

- `scripts/test_release_pipeline.py` runs the release pipeline on synthetic signed executables. It signs the output with ephemeral keys and verifies it with `verify-authority`, which uses `core_update_policy` and `verify_manifest`, the same code the app and helper use. Its metadata test asserts that every manifest carries `trust.publisher` and that the authority carries the signer pin and the helper hash.
- The `bareline-distribution` unit tests cover `core_update_policy` and `PublisherPin`. The `bareline-platform-windows` tests cover resolving the signed authority, refusing a helper whose bytes differ from `update_helper_sha256` or that has no signed pin, Authenticode rejection of unsigned files, file-ID comparison and the revocation mode chosen for helper launches.

**Deferred:** no CI job runs the real `bareline-update-helper.exe --apply`, `--acknowledge` or `--recover` on pipeline output. No existing Windows job can do this without an extra build, for two reasons:

- The helper refuses every build mode except `configured` (SEC-18). `release_config.py` rejects nonshipping keys in configured mode, so a CI helper would need a separate configured-release build with test trust, which no job performs.
- `--apply` requires an Authenticode signature that chains to a trusted root and matches the compiled signer pin. CI has no such signing certificate, and trusting a test root would change the runner's system trust settings.

Until this is automated, the real helper flow is checked in the disposable-VM `install_update_rollback` lab journey (`tests/e2e/WINDOWS-LAB.md`) with a signed configured candidate. **Follow-up:** add a Windows CI job that builds a configured helper against an ephemeral test trust configuration, then runs `--apply`, `--acknowledge` and `--recover` on the pipeline's signed-delivery output. The job needs a code-signing test root trusted only inside that job's disposable environment, and a build mode the product accepts for it without weakening SEC-18.

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

Have the authorized signing service sign copies of all three files under the handoff's `unsigned` directory. Keep the originals. The existing `sign-artifacts.ps1` remains restricted to tagged protected CI. Local orchestration accepts externally signed files and never impersonates that authorization.

```powershell
python scripts/release_pipeline.py verify-signed --handoff target/signing-handoff --signed-dir target/signed-inner
python scripts/release_pipeline.py prepare-metadata --handoff target/signing-handoff --signed-dir target/signed-inner --expires-unix <reviewed-Unix-expiry> --authority-expires-unix <reviewed-authority-expiry> --output target/metadata-signing
```

Add `--root-transitions <old-and-new-signed-chain.json>` when rotating roots. Clients recheck every transition's expiry whenever they accept new metadata, so `prepare-metadata` refuses a chain with any transition that expires before `--authority-expires-unix`.

The byte check permits only the PE checksum/security directory and terminal certificate append to differ. It does **not** establish certificate validity. The metadata phase uses the actual signed hashes/lengths and content-addressed component filenames, writes `trust.publisher` into every manifest and records the Authenticode pin and the signed helper's SHA-256 in the release authority. It requires a future expiry and creates `signing-request.json` with metadata hashes and separate signing roles. Review authority version/revocations and expiry; existing deployed root policy must never be reset to an earlier version. If rotating roots, supply the old-and-new-signed transition chain alongside the signed authority.

### Trust-state lifecycle

- **Separate authority expiry (SEC-02).** `--authority-expires-unix` sets the release authority's own lifetime. It must be at least one year ahead and not before the metadata expiry. Installed clients keep using the authority between releases. Checking for and applying updates, opening catalogs and installing packages or the runtime need an unexpired authority. Restoring and running already-installed extensions, the health acknowledgement and recovery do not; an expired authority never disables them.
- **Delivered with each core update (SEC-02).** `bareline.update.json` carries `authority_sha256` (and `root_transitions_sha256` when a chain is supplied). The current release key signs these digests, and the offline root must still verify the delivered authority, which can never lower the accepted root version. The authority pins the helper it ships with. The update host serves `bareline.release-authority.json`, `bareline.release-authority.minisig`, `bareline-update-helper.exe` and, when rotating, `bareline.root-transitions.json` in the same directory as the configured manifest path (`signing-request.json` lists them). The helper installs them together with a journal and rolls back on failure or after a crash. To change the Authenticode pin, first ship a release signed with the current certificate whose authority accepts both the old and new issuers; sign later releases with the new certificate. This is a one-way manifest schema change: clients built before it reject the new fields, because the core manifest refuses unknown fields. No configured release predates it; only unsigned previews, which never update, have been published (through `v0.1.0-preview.20260928.1`).
- **Per-artifact floors (SEC-03).** The authority's `minimum_metadata_version` binds the core executable only. The runtime and the catalogs keep their own ledgers in the extension storage. Optional `minimum_runtime_metadata_version` and `minimum_catalog_metadata_version` authority fields raise their floors deliberately, for example to revoke an old runtime.
- **Per-user state (SEC-04).** Ledgers, the update lock and the failed-launch counter live in `%LOCALAPPDATA%\Bareline\update\<installation key>` (portable: `data\update`). The installation is only read, apart from the helper replacing its executables. Existing ledgers in the installation are still honored. A per-machine installation keeps verifying its authority and running extensions; its executables are replaced by running a newer installer, because a standard user cannot write Program Files.
- **Rollback (SEC-09).** A freshly updated build that fails to reach a healthy frame three times in a row is restored to the build the update replaced on the next launch, and the helper then starts the editor again. Only launches of the exact build the update journal installed count, while the build it replaced is still retained; reinstalling or replacing the editor stops the count. Automatic rollback is attempted once: if it fails, later launches start normally and report that it failed. The **Roll Back Last Update** command (command palette) does the same on request after the editor closes. `--recover` needs the update journal or the per-user ledger entry for the running build, restores only that exact retained build (never an older generation), and never lowers a metadata floor.
- **Retention (SEC-17).** After a healthy acknowledgement, retained evidence (`*.retained-<generation>`) is pruned to the newest two generations. The uninstaller removes helper-created files, ledgers and the lock.

Sign `bareline.update.json` → `bareline.update.minisig` and `runtime.json` → `runtime.minisig` with the release key; `catalog.json` → `catalog.json.minisig` with the catalog key; and `bareline.release-authority.json` → `bareline.release-authority.minisig` with the offline root. Public deterministic test-vector keys are rejected by configured builds.

When configured CI signing is enabled, it performs the inner-executable signing, Windows publisher validation and byte comparison, then generates `configured-metadata-signing-handoff`. This artifact contains `handoff/` (the original compared bytes/configuration), `delivery/` (signed executables, components, unsigned metadata and signing request) and `signing-evidence/` (before/after hashes and the SignPath request ID when applicable). Download and retain that complete artifact. Sign the four metadata files in `delivery/` using their separate authorized roles; use that directory as `-Delivery` below. The offline root key never enters this workflow.

## 3. Assemble verified delivery

Build the small shared verifier once, and generate the complete notices/SBOM/SDK and user documents for the actual candidate. The documents directory must contain LICENSE, SDK-LICENSES.md, THIRD-PARTY-NOTICES.md, SBOM.json, RELEASE-NOTES.md, MIGRATION-NOTES.md and KNOWN-ISSUES.md.

```powershell
cargo build --locked -p bareline-distribution --example verify-authority
./packaging/windows/assemble-configured.ps1 -Handoff target/signing-handoff -Delivery target/metadata-signing -Documents target/release-documents -OutputDir target/assembled-release -AuthorityVerifier target/debug/examples/verify-authority.exe -Iscc C:/tools/InnoSetup/ISCC.exe
```

Assembly rechecks the unsigned/signed relationship, the Windows signer against the authority's Authenticode pin, the helper against the authority's helper hash, root lineage/floors/expiry, update/runtime signatures and hashes, and catalog/package contents. The same checks apply to the staged copies. The portable ZIP and installer carry exact bootstrap authority files. The runtime and content-addressed components remain external to the core package. The installer is still unsigned at this phase.

## 4. Final signing, inventory and verification

Have the authorized service sign a **copy** of `bareline-0.1.0-windows-x64-setup.exe`. Retain the original unsigned assembly. Prepare the final-byte inventory:

```powershell
./packaging/windows/finalize-configured.ps1 -Handoff target/signing-handoff -Assembled target/assembled-release -SignedInstaller target/external-signing/bareline-0.1.0-windows-x64-setup.exe -OutputDir target/final-inventory -AuthorityVerifier target/debug/examples/verify-authority.exe
```

This checks the installer's Windows signer against the verified authority's Authenticode pin, allows only Authenticode changes to the original installer (including Inno's PE32 bootstrap), and binds the portable editor/helper and external runtime to the original compared unsigned executables. It creates `artifacts/`, a public configuration copy and `finalization-request.json`; the original assembly remains unchanged. Sign `target/final-inventory/artifacts/SHA-256SUMS` with the current authority's release key, saving the detached signature **outside** `artifacts/`, then resume:

```powershell
./packaging/windows/finalize-configured.ps1 -Handoff target/signing-handoff -Prepared target/final-inventory -InventorySignature target/external-signing/SHA-256SUMS.minisig -Minisign C:/tools/minisign.exe -OutputDir target/verified-release -AuthorityVerifier target/debug/examples/verify-authority.exe
```

The original handoff is required again: the prepared request cannot replace its source identity or public trust configuration. Changed, added or removed prepared files fail before the detached signature is imported. This phase copies to a new directory, then runs `verify-release.ps1` with the verified authority's current release key and Authenticode pin. Verification includes inventory signatures, all executable publishers, actual portable editor/helper, external runtime, signed metadata and catalog packages. Only successful verification creates `verification.json` beside `artifacts/`; it explicitly leaves release approval and clean-machine acceptance pending. Failed attempts remain for diagnosis and cannot be reused as new phase outputs.

After clean-machine installation/uninstallation, interrupted update/recovery and sustained-operation qualification, the owner can approve publication of the exact `target/verified-release/artifacts/` bytes. The configuration/request/verification records are retained evidence, outside the distribution asset set. The configured update host must separately serve the signed metadata and matching raw `bareline.exe` extracted from the verified portable ZIP at the configured paths; attaching a GitHub Release ZIP alone does not create that update endpoint. No helper publishes or grants approval. A compromised sole offline root requires an independently authenticated replacement installer or explicit manual trust reset.

## CI owner configuration and remaining dependencies

Leave `BARELINE_SIGNING_ENABLED` unset until enrollment, public pins and provider policy are ready. Common repository/environment variables are `BARELINE_RELEASE_CONFIG_PATH` (a real checked-in configured public JSON), `BARELINE_PROTECTED_RELEASE_ENVIRONMENT=true` (only after the branch rules and `release-signing` environment are established), `BARELINE_METADATA_EXPIRES_UNIX` (reviewed future Unix expiry), `BARELINE_AUTHORITY_EXPIRES_UNIX` (reviewed release-authority expiry, at least one year ahead and not before the metadata expiry), and `BARELINE_SIGNING_PROVIDER` (`signpath` or `certstore`). Tag `v<version>` must point to a commit on the protected default branch. Every privileged signing request remains behind `release-signing` approval. The signing jobs read the Authenticode pin from the compared public configuration. Before signing, they also check it against two protected-environment variables that act as an independent anchor: `BARELINE_AUTHENTICODE_SUBJECT` (the signer's simple display name) and `BARELINE_AUTHENTICODE_ISSUERS` (the issuing CAs, `|`-separated, in configuration order). There is no certificate-digest variable.

The SignPath route uploads exactly the three inner executables and submits that same run's immutable artifact ID using a commit-pinned SignPath action. Configure `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`, `SIGNPATH_SIGNING_POLICY_SLUG`, optional `SIGNPATH_INNER_ARTIFACT_CONFIGURATION_SLUG`, and the environment secret `SIGNPATH_API_TOKEN`. The approved SignPath artifact configuration must accept a ZIP with `bareline.exe`, `bareline-update-helper.exe` and `bareline-extension-host.exe` at its root. Enrollment, the GitHub application, signing-policy reviewers and a production certificate are provider/owner prerequisites; merely committing this adapter does not establish them. See [SignPath's GitHub integration contract](https://docs.signpath.io/trusted-build-systems/github).

The alternative `certstore` route needs an owner-provisioned certificate and tool on the signing runner, plus `BARELINE_SIGNTOOL`, `BARELINE_SIGNER_THUMBPRINT` and `BARELINE_TIMESTAMP_URL`. A fresh hosted runner has no owner's signing certificate; the workflow does not import or fabricate one.

The SignPath adapter currently covers inner executables. Installer signing after external metadata approval still needs a provider-approved signing route; for SignPath Open Source this must preserve its GitHub-hosted origin requirements. Separate production release/catalog/offline-root keys, an actual update host, reviewed documents/SBOM, installer signing and clean-machine qualification remain external dependencies. This implementation has synthetic byte/tamper tests and syntax validation; an end-to-end production signing/verification run requires those real inputs.
