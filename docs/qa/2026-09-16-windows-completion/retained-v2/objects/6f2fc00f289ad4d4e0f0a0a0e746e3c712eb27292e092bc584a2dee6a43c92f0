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

The separate `configured-build`/`configured-comparison` CI jobs implement this path. Their signing job consumes only `configured-signing-handoff`; it cannot sign the preview job's output. Hosted execution still needs the owner's repository configuration, protected environment and public config path.

## 2. External signing, then metadata

Have the authorized signing service sign copies of all three files under the handoff's `unsigned` directory. Keep the originals. The existing `sign-artifacts.ps1` remains restricted to tagged protected CI. Local orchestration accepts externally signed files and never impersonates that authorization.

```powershell
python scripts/release_pipeline.py verify-signed --handoff target/signing-handoff --signed-dir target/signed-inner
python scripts/release_pipeline.py prepare-metadata --handoff target/signing-handoff --signed-dir target/signed-inner --expires-unix <reviewed-Unix-expiry> --output target/metadata-signing
```

The byte check permits only the PE checksum/security directory and terminal certificate append to differ. It does **not** establish certificate validity. The metadata phase uses the actual signed hashes/lengths and content-addressed component filenames. It creates `signing-request.json` with separate signing roles. Review authority version/revocations and expiry; existing deployed root policy must never be reset to an earlier version. If rotating roots, supply the old-and-new-signed transition chain alongside the signed authority.

Sign `bareline.update.json` → `bareline.update.minisig` and `runtime.json` → `runtime.minisig` with the release key; `catalog.json` → `catalog.json.minisig` with the catalog key; and `bareline.release-authority.json` → `bareline.release-authority.minisig` with the offline root. Public deterministic test-vector keys are rejected by configured builds.

## 3. Assemble verified delivery

Build the small shared verifier once, and generate the complete notices/SBOM/SDK and user documents for the actual candidate. The documents directory must contain LICENSE, SDK-LICENSES.md, THIRD-PARTY-NOTICES.md, SBOM.json, RELEASE-NOTES.md, MIGRATION-NOTES.md and KNOWN-ISSUES.md.

```powershell
cargo build --locked -p bareline-distribution --example verify-authority
./packaging/windows/assemble-configured.ps1 -Handoff target/signing-handoff -Delivery target/metadata-signing -Documents target/release-documents -OutputDir target/assembled-release -AuthorityVerifier target/debug/examples/verify-authority.exe -Iscc C:/tools/InnoSetup/ISCC.exe
```

Assembly rechecks the unsigned/signed relationship, actual Windows publisher, root lineage/floors/expiry, update/runtime signatures and hashes, and catalog/package contents. The same checks apply to the staged copies. The portable ZIP and installer carry exact bootstrap authority files. The runtime and content-addressed components remain external to the core package. The installer is still unsigned at this phase.

## 4. Final signing and verification

Have the authorized service sign the installer. Then run `build.ps1 -FinalInventory`, sign `SHA-256SUMS` with the authorized release key, and run:

```powershell
./packaging/windows/verify-release.ps1 -ArtifactDir target/assembled-release -Minisign C:/tools/minisign.exe -ReleasePublicKey <current-authorized-release-key> -PublisherCertificateSha256 <current-authorized-certificate-digest> -Version 0.1.0 -ReleaseConfig target/signing-handoff/public-release-config.json -AuthorityVerifier target/debug/examples/verify-authority.exe
```

Verification includes the actual portable inner executable, external runtime, signed metadata and catalog packages. Clean-machine installation/uninstallation, interrupted update/recovery, sustained operation and the final readiness decision remain qualification work. Successful packaging is not release approval. A compromised sole offline root requires an independently authenticated replacement installer or explicit manual trust reset.
