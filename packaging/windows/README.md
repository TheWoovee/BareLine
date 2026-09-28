# Windows packaging

## Build an unsigned preview

Use Windows x64, PowerShell 7, the Rust toolchain pinned in [rust-toolchain.toml](../../rust-toolchain.toml), and Visual Studio Build Tools with the C++ workload and Windows SDK. Git is optional: an extracted source ZIP works too. Install the pinned SBOM tool once:

```powershell
cargo install --locked cargo-cyclonedx --version 0.5.9
```

From the source directory, build the portable ZIP:

```powershell
pwsh -File ./packaging/windows/build-preview.ps1
```

To also build an installer, install [Inno Setup 6.4.3](https://github.com/jrsoftware/issrc/releases/tag/is-6_4_3) and supply its compiler explicitly. The command checks the pinned compiler before starting the Rust build:

```powershell
pwsh -File ./packaging/windows/build-preview.ps1 -OutputDir dist/preview-with-setup -Installer -Iscc 'C:/Program Files (x86)/Inno Setup 6/ISCC.exe'
```

The script can be invoked by absolute path from any working directory. Relative `-OutputDir` and `-TargetDir` paths resolve against the source root. The default package directory is `dist/preview`; choose a new output directory for each run. The default Cargo build directory is `target/preview-build` and can be reused for incremental builds.

The command builds `bareline` and `bareline-update-helper` with `--locked --release --no-default-features`, generates notices for those two dependency roots, generates and normalizes their current CycloneDX SBOM, and assembles:

- `bareline-<version>-windows-x64-portable.zip`
- `bareline-<version>-windows-x64-setup.exe` when `-Installer` is supplied
- `LICENSE`, `THIRD-PARTY-NOTICES.md`, `SBOM.json`, and `PREVIEW-NOTES.md` as separate downloads
- `SHA-256SUMS` covering every output file except itself

Versions come from the Cargo workspace. Packages contain both executables, `LICENSE`, `THIRD-PARTY-NOTICES.md`, and `SBOM.json`. The portable ZIP also contains `bareline.portable`. Extract the entire ZIP into a writable folder and run `bareline.exe`; the adjacent marker keeps settings, sessions and recovery in `data/`. The installer defaults to a per-user installation and offers optional Explorer/editor registrations without changing Windows defaults. Uninstallation retains user data.

Both packages require the **Microsoft Visual C++ v14 Redistributable (x64)** on the destination computer. The installer does not install this runtime automatically.

The packaged `THIRD-PARTY-NOTICES.md` combines dependency notices, local SDK license texts and the Unicode data license texts. Identical Unicode licenses are included once.

Preview packages are unsigned. Updates and extension loading are disabled, and the separate extension runtime is not included. Windows may warn about an unsigned application. This command does not configure trust, sign, publish or install the application.

Staging is retained under a unique `target/preview-packaging-*` directory, including the payload, raw dependency SBOMs and `SDK-LICENSES.md`. Cargo CycloneDX briefly writes unique ignored `*.cdx.json` files beside workspace manifests, then the script moves only those generated files into staging. Failed runs retain their generated files for inspection. Existing output directories are refused, and the script checks that `Cargo.lock` did not change. It does not delete build outputs or user files.

The portable ZIP uses stable file order and timestamps for identical payload bytes. The build also normalizes source/target paths and MSVC PE timestamps; matching binaries across machines still requires matching toolchains, build tools and environment. No reproducibility or signing qualification is implied by a successful preview build.

The builder runs `verify-preview.ps1` before reporting success. It checks the complete inventory and hashes, ZIP contents and normalized timestamps, x64 executable headers and unsigned preview capability markers, license evidence and dependency SBOM. To verify downloaded artifacts again:

```powershell
./packaging/windows/verify-preview.ps1 -ArtifactDir dist/preview -Version 0.1.0 -RequireInstaller
```

These checks validate package consistency; the unsigned checksums do not authenticate a publisher. See [CODE_SIGNING.md](../../CODE_SIGNING.md) for the preview signing policy.

## GitHub preview releases

The [Unsigned Windows preview workflow](../../.github/workflows/preview-release.yml) builds and checks portable and installer packages on a disposable Windows runner. Changes to packaging trigger it on pull requests. `workflow_dispatch` can validate a selected branch and retain downloadable Actions artifacts without publishing a release.

A pushed tag such as `v0.1.0-preview.20260928.1` requests a GitHub prerelease. The `x.y.z` portion must match the Cargo workspace version, and the preview suffix consists of dot-separated numeric identifiers without leading zeros. Before tagging, merge the candidate into the default branch and wait for successful `ci.yml` and `supply-chain.yml` push runs for that exact commit. The publication job checks this ancestry and both results; missing or incomplete checks stop publication.

The workflow uses the pinned Rust and cargo-cyclonedx versions and downloads Inno Setup 6.4.3 from its official release, verifying its pinned SHA-256 before installation. It tests core persistence/distribution contracts and the package verifier, builds with `build-preview.ps1`, then checks portable launch and installer lifecycle behavior. Only a tag push can enter the publication job. That job creates a draft marked **unsigned preview**, downloads and rechecks every uploaded asset against the tested bytes, then publishes it as a prerelease with `latest=false`. An existing release or draft is never overwritten; a failed upload/verification leaves a draft for inspection.

No personal access token or signing credentials are needed. Read-only build jobs use the standard Actions token; only publication receives `contents: write` and `actions: read`. Repository policy must permit those built-in token permissions. Manual and pull-request builds cannot publish.

## Disposable-machine installer checks

`test-preview-installer-ci.ps1` tests portable and installed editor startup, disabled updater behavior, exact installed bytes, same-version reinstallation, unchecked Explorer/editor registrations, uninstallation, and preservation of adjacent user-created files and the installed profile. It defaults to GitHub-hosted Windows runners. It refuses any pre-existing Bareline installation, registry registration, process, or local/roaming profile. Logs and scratch data are retained.

For client-OS qualification, use a fresh disposable Windows 10 22H2 or Windows 11 VM snapshot with PowerShell 7 and the Microsoft Visual C++ v14 Redistributable (x64). Copy the complete verified artifact directory and matching source scripts into that VM, then explicitly opt in:

```powershell
pwsh -File ./packaging/windows/test-preview-installer-ci.ps1 -ArtifactDir C:/preview-downloads -Version 0.1.0 -DisposableMachine
```

This installs, launches, reinstalls and uninstalls Bareline in the VM. Retain the printed log directory and restore the VM snapshot before repeating. Never use the opt-in on a personal or shared computer. Hosted Windows Server checks do not qualify clean Windows 10/11 machines, and they do not test a machine missing the Visual C++ runtime. Interactive editing, accessibility, file associations selected explicitly, cross-version upgrades, and the supported client-OS matrix require separate tests.

## Package an existing payload

For an already prepared payload containing the five required files listed above:

```powershell
./packaging/windows/build.ps1 -PayloadDir target/payload -Version 0.1.0 -OutputDir dist/local-package
```

Add `-Installer -Iscc <path>` for setup. The existing packager verifies the actual Inno Setup 6.4.3 compiler through an ISPP probe. `generate-notices.ps1` gathers license texts from the locked Windows dependency graph and checked-in upstream license supplements; `generate-sbom.ps1` combines editor/helper CycloneDX output with native source and packaging file inventories.

## Configured releases

[CONFIGURED-RELEASE.md](CONFIGURED-RELEASE.md) describes the separate build, compare, external signing, metadata, assembly and verification workflow. It requires real public release configuration and owner-controlled signing infrastructure. Preview artifacts must not be presented as signed configured releases.

Generate a winget manifest only from the final installer and its actual HTTPS release URL using `winget/generate.ps1`. Manifest validation does not publish it.
