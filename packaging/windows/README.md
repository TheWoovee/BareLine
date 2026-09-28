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

To also build an installer, install **Inno Setup 6.4.3** and supply its compiler explicitly:

```powershell
pwsh -File ./packaging/windows/build-preview.ps1 -OutputDir dist/preview-with-setup -Installer -Iscc 'C:/Program Files (x86)/Inno Setup 6/ISCC.exe'
```

The script can be invoked by absolute path from any working directory. Relative `-OutputDir` and `-TargetDir` paths resolve against the source root. The default package directory is `dist/preview`; choose a new output directory for each run. The default Cargo build directory is `target/preview-build` and can be reused for incremental builds.

The command builds `bareline` and `bareline-update-helper` with `--locked --release --no-default-features`, generates notices for those two dependency roots, generates and normalizes their current CycloneDX SBOM, and assembles:

- `bareline-<version>-windows-x64-portable.zip`
- `bareline-<version>-windows-x64-setup.exe` when `-Installer` is supplied
- `SHA-256SUMS` covering the package files

Versions come from the Cargo workspace. Packages contain both executables, `LICENSE`, `THIRD-PARTY-NOTICES.md`, and `SBOM.json`. The portable ZIP also contains `bareline.portable`. Extract the entire ZIP into a writable folder and run `bareline.exe`; the adjacent marker keeps settings, sessions and recovery in `data/`. The installer defaults to a per-user installation and offers optional Explorer/editor registrations without changing Windows defaults. Uninstallation retains user data.

Both packages require the **Microsoft Visual C++ v14 Redistributable (x64)** on the destination computer. The installer does not install this runtime automatically.

The packaged `THIRD-PARTY-NOTICES.md` combines dependency notices, local SDK license texts and the Unicode data license texts. Identical Unicode licenses are included once.

Preview packages are unsigned. Updates and extension loading are disabled, and the separate extension runtime is not included. Windows may warn about an unsigned application. This command does not configure trust, sign, publish or install the application.

Staging is retained under a unique `target/preview-packaging-*` directory, including the payload, raw dependency SBOMs and `SDK-LICENSES.md`. Cargo CycloneDX briefly writes unique ignored `*.cdx.json` files beside workspace manifests, then the script moves only those generated files into staging. Failed runs retain their generated files for inspection. Existing output directories are refused, and the script checks that `Cargo.lock` did not change. It does not delete build outputs or user files.

The portable ZIP uses stable file order and timestamps for identical payload bytes. The build also normalizes source/target paths and MSVC PE timestamps; matching binaries across machines still requires matching toolchains, build tools and environment. No reproducibility or signing qualification is implied by a successful preview build.

## Package an existing payload

For an already prepared payload containing the five required files listed above:

```powershell
./packaging/windows/build.ps1 -PayloadDir target/payload -Version 0.1.0 -OutputDir dist/local-package
```

Add `-Installer -Iscc <path>` for setup. The existing packager verifies the actual Inno Setup 6.4.3 compiler through an ISPP probe. `generate-notices.ps1` gathers license texts from the locked Windows dependency graph and checked-in upstream license supplements; `generate-sbom.ps1` combines editor/helper CycloneDX output with native source and packaging file inventories.

## Configured releases

[CONFIGURED-RELEASE.md](CONFIGURED-RELEASE.md) describes the separate build, compare, external signing, metadata, assembly and verification workflow. It requires real public release configuration and owner-controlled signing infrastructure. Preview artifacts must not be presented as signed configured releases.

Generate a winget manifest only from the final installer and its actual HTTPS release URL using `winget/generate.ps1`. Manifest validation does not publish it.
