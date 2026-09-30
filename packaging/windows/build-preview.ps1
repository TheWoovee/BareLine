#requires -Version 7.0
# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param(
    [string]$OutputDir = 'dist/preview',
    [string]$TargetDir = 'target/preview-build',
    [switch]$Installer,
    [string]$Iscc,
    # The v<version>-preview.<n> tag this build is published as; defaults to
    # the pushed tag in GitHub Actions. Untagged builds are labelled -dev.
    [string]$PreviewTag
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'build-provenance.ps1')
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$target = 'x86_64-pc-windows-msvc'

function Get-RepoPath([string]$Path) {
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $root $Path))
}

if (-not $IsWindows) { throw 'Preview packaging requires Windows and the MSVC build tools.' }
$null = Get-Command cargo -ErrorAction Stop
$output = Get-RepoPath $OutputDir
$cargoTarget = Get-RepoPath $TargetDir
if (Test-Path -LiteralPath $output) { throw "Use a new preview output directory: $output" }
if ($Installer) {
    if (-not $Iscc) { throw 'Installer requires -Iscc with the path to Inno Setup 6.4.3 ISCC.exe.' }
    $Iscc = (Resolve-Path -LiteralPath $Iscc).Path
    if (-not (Test-Path -LiteralPath $Iscc -PathType Leaf)) { throw 'Expected an ISCC.exe file.' }
    & $Iscc '/Q' '/O-' (Join-Path $PSScriptRoot 'compiler-probe.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Expected Inno Setup 6.4.3; compiler preflight failed before building.' }
} elseif ($Iscc) {
    throw '-Iscc requires -Installer.'
}

$lockPath = Join-Path $root 'Cargo.lock'
$lockHash = (Get-FileHash -LiteralPath $lockPath -Algorithm SHA256).Hash
$runId = [Guid]::NewGuid().ToString('N')
$stage = Join-Path $root "target/preview-packaging-$runId"
$payload = Join-Path $stage 'payload'
$sbomDirectory = Join-Path $stage 'cargo-sboms'
$previousTarget = $env:CARGO_TARGET_DIR
$previousPrepared = $env:BARELINE_PREPARED_RELEASE_CONFIG
$previousOffline = $env:CARGO_NET_OFFLINE
$previousEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
$previousFlags = $env:RUSTFLAGS
$previousBuildHash = $env:BARELINE_BUILD_HASH
$previousBuildVersion = $env:BARELINE_BUILD_VERSION
Push-Location $root
try {
    $toolVersion = & cargo cyclonedx --version
    if ($LASTEXITCODE -ne 0 -or ($toolVersion -join "`n") -notmatch ' 0\.5\.9\s*$') {
        throw 'Install the required SBOM tool first: cargo install --locked cargo-cyclonedx --version 0.5.9'
    }
    $env:CARGO_TARGET_DIR = $cargoTarget
    $env:BARELINE_PREPARED_RELEASE_CONFIG = $null
    $baseFlags = if ($previousEncodedFlags) { @($previousEncodedFlags.Split([char]31)) } elseif ($previousFlags) { @([regex]::Split($previousFlags.Trim(), '\s+')) } else { @() }
    # These replace the .cargo/config.toml target flags, so repeat its static
    # CRT, Control Flow Guard and CET flags; verify-preview.ps1 checks the PE.
    $hardeningFlags = @('-C', 'target-feature=+crt-static', '-C', 'control-flow-guard')
    $env:CARGO_ENCODED_RUSTFLAGS = (($baseFlags + $hardeningFlags + (Get-ReleaseRemapFlags $root $cargoTarget) + @('-C', 'link-arg=/Brepro')) -join [char]31)

    # Metadata/notices need some workspace dependencies not used by the two binaries.
    & cargo fetch --locked --target $target
    if ($LASTEXITCODE -ne 0) { throw 'Could not fetch the locked Windows dependency graph.' }
    $metadataText = & cargo metadata --locked --offline --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Could not read locked workspace metadata.' }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $version = ($metadata.packages | Where-Object name -eq 'bareline').version
    $helperVersion = ($metadata.packages | Where-Object name -eq 'bareline-update-helper').version
    if ($version -notmatch '^\d+\.\d+\.\d+$' -or $version -ne $helperVersion) { throw 'Editor/helper versions must match and use x.y.z.' }

    # About, --version and diagnostics identify the exact commit and preview.
    $buildHash = (& git -C $root rev-parse --verify HEAD) -join ''
    if ($LASTEXITCODE -ne 0 -or $buildHash -cnotmatch '^[0-9a-f]{40}$') { throw 'Preview packaging records the source commit; run it from a Git checkout.' }
    if (-not $PreviewTag -and $env:GITHUB_REF_TYPE -eq 'tag') { $PreviewTag = $env:GITHUB_REF_NAME }
    if ($PreviewTag) {
        & (Join-Path $PSScriptRoot 'check-preview-tag.ps1') -Version $version -Tag $PreviewTag | Out-Null
        $buildVersion = $PreviewTag.Substring(1)
    } else {
        $buildVersion = "$version-dev+$($buildHash.Substring(0, 12))"
    }
    $env:BARELINE_BUILD_HASH = $buildHash
    $env:BARELINE_BUILD_VERSION = $buildVersion

    [IO.Directory]::CreateDirectory($payload) | Out-Null
    [IO.Directory]::CreateDirectory($sbomDirectory) | Out-Null
    # Build evidence stays in staging; the published asset set is fixed.
    [IO.File]::WriteAllText((Join-Path $stage 'build-tools.json'), ((Get-BuildToolIdentity $root $Iscc) | ConvertTo-Json) + "`n", [Text.UTF8Encoding]::new($false))
    Write-Output "Building unsigned preview $buildVersion from $buildHash (updates and extension loading disabled)."
    & cargo build --locked --release --target $target -p bareline -p bareline-update-helper --no-default-features
    if ($LASTEXITCODE -ne 0) { throw 'Preview build failed. Check the pinned Rust toolchain and MSVC/Windows SDK installation.' }
    foreach ($name in @('bareline.exe', 'bareline-update-helper.exe')) {
        [IO.File]::Copy((Join-Path $cargoTarget "$target/release/$name"), (Join-Path $payload $name), $false)
    }
    [IO.File]::Copy((Join-Path $root 'LICENSE'), (Join-Path $payload 'LICENSE'), $false)
    & (Join-Path $PSScriptRoot 'generate-notices.ps1') -OutputFile (Join-Path $payload 'THIRD-PARTY-NOTICES.md') -SdkOutputFile (Join-Path $stage 'SDK-LICENSES.md') -Roots bareline,bareline-update-helper
    # Both package formats ship this notices file; retain local SDK grants and
    # Unicode data licenses here as well as the separately staged SDK document.
    $noticesPath = Join-Path $payload 'THIRD-PARTY-NOTICES.md'
    $utf8 = [Text.UTF8Encoding]::new($false)
    [IO.File]::AppendAllText($noticesPath, "`n`n" + [IO.File]::ReadAllText((Join-Path $stage 'SDK-LICENSES.md')), $utf8)
    [IO.File]::AppendAllText($noticesPath, "`n`n# Unicode data licenses`n`nData used by bareline-unicode-fold and bareline-search.`n", $utf8)
    $unicodeTexts = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($relative in @('crates/unicode-fold/data/LICENSE-UNICODE', 'crates/search/data/LICENSE-UNICODE')) {
        $licenseText = [IO.File]::ReadAllText((Join-Path $root $relative))
        if ($unicodeTexts.Add($licenseText)) {
            [IO.File]::AppendAllText($noticesPath, "`n`n## $relative`n`n$licenseText", $utf8)
        }
    }

    # cargo-cyclonedx 0.5.9 has no --locked or output-directory option. Check
    # locked metadata first; use offline resolution and unique ignored filenames.
    $env:CARGO_NET_OFFLINE = 'true'
    & cargo metadata --locked --offline --no-default-features --filter-platform $target --format-version 1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Locked SBOM dependency resolution failed.' }
    $sbomBaseName = "preview-$runId.cdx"
    $members = @($metadata.packages | Where-Object { $_.id -in $metadata.workspace_members })
    foreach ($member in $members) {
        $path = Join-Path (Split-Path -Parent $member.manifest_path) "$sbomBaseName.json"
        if (Test-Path -LiteralPath $path) { throw "Refusing to replace an existing SBOM: $path" }
    }
    & cargo cyclonedx --format json --all --no-default-features --target $target --override-filename $sbomBaseName
    if ($LASTEXITCODE -ne 0) { throw 'Cargo SBOM generation failed; any partial .cdx.json files are retained.' }
    if ((Get-FileHash -LiteralPath $lockPath -Algorithm SHA256).Hash -ne $lockHash) { throw 'SBOM generation changed Cargo.lock; packaging stopped. Review the lockfile before retrying.' }
    $sboms = @{}
    foreach ($member in $members) {
        $source = Join-Path (Split-Path -Parent $member.manifest_path) "$sbomBaseName.json"
        $destination = Join-Path $sbomDirectory "$($member.name).json"
        [IO.File]::Move($source, $destination)
        $sboms[$member.name] = $destination
    }
    & (Join-Path $PSScriptRoot 'generate-sbom.ps1') -CargoSbom $sboms['bareline'] -HelperCargoSbom $sboms['bareline-update-helper'] -OutputFile (Join-Path $payload 'SBOM.json')

    if (Test-Path -LiteralPath $output) { throw "Use a new preview output directory: $output" }
    & (Join-Path $PSScriptRoot 'build.ps1') -PayloadDir $payload -Version $version -OutputDir $output -Installer:$Installer -Iscc $Iscc
    foreach ($name in @('LICENSE', 'THIRD-PARTY-NOTICES.md', 'SBOM.json')) {
        [IO.File]::Copy((Join-Path $payload $name), (Join-Path $output $name), $false)
    }
    $previewNotes = @'
# Bareline @BUILD_VERSION@ unsigned preview

Built from commit @BUILD_HASH@. Help > About shows the same version and commit.

This is an unsigned preview of the Windows x64 core editor, not a stable or signed release. Automatic updates and extension loading are disabled; the external extension runtime is not included.

Targets 64-bit Windows 10 22H2 (build 19045) or later, including Windows 11; the installer refuses older builds. Final clean-client qualification remains pending. The C and C++ runtime is linked into the executables, so the Microsoft Visual C++ Redistributable is not required. Windows may warn about unsigned software.

Use the setup executable for a per-user installation, or extract the entire portable ZIP into a writable directory and run bareline.exe. Keep bareline.portable beside the portable executable so settings, sessions and recovery use its adjacent data directory. Optional Explorer/editor registrations do not change Windows defaults. Uninstallation retains user data.

SHA-256SUMS covers every download except itself. These unsigned hashes detect corrupted downloads; they are not a publisher signature. Downloads published by the GitHub preview workflow also carry GitHub build-provenance attestations: `gh attestation verify <file> --repo @REPOSITORY@` checks that a file was built by that workflow from this commit. LICENSE, THIRD-PARTY-NOTICES.md and SBOM.json are provided separately and inside both packages. See the repository README for the current feature scope, maturity labels and "Known issues in this preview", and [CODE_SIGNING.md](@SIGNING_POLICY@) for the unsigned preview policy.
'@
    $repository = 'TheWoovee/BareLine'
    $signingPolicy = "https://github.com/$repository/blob/master/CODE_SIGNING.md"
    if ($env:GITHUB_REPOSITORY -match '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$' -and $env:GITHUB_SHA -match '^[a-fA-F0-9]{40}$') {
        $repository = $env:GITHUB_REPOSITORY
        $signingPolicy = "https://github.com/$($env:GITHUB_REPOSITORY)/blob/$($env:GITHUB_SHA)/CODE_SIGNING.md"
    }
    [IO.File]::WriteAllText((Join-Path $output 'PREVIEW-NOTES.md'), $previewNotes.Replace('@BUILD_VERSION@', $buildVersion).Replace('@BUILD_HASH@', $buildHash).Replace('@SIGNING_POLICY@', $signingPolicy).Replace('@REPOSITORY@', $repository) + "`n", [Text.UTF8Encoding]::new($false))
    $inventory = @(Get-ChildItem -LiteralPath $output -File | Where-Object Name -ne 'SHA-256SUMS' | Sort-Object Name | ForEach-Object {
        '{0}  {1}' -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $_.Name
    })
    [IO.File]::WriteAllText((Join-Path $output 'SHA-256SUMS'), (($inventory -join "`n") + "`n"), [Text.UTF8Encoding]::new($false))
    & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $output -Version $version -RequireInstaller:$Installer -BuildHash $buildHash
    Write-Output "Unsigned preview packages and SHA-256SUMS: $output"
    Write-Output "Staged payload, SDK licenses, dependency SBOMs and build-tools.json: $stage"
} finally {
    $env:CARGO_TARGET_DIR = $previousTarget
    $env:BARELINE_PREPARED_RELEASE_CONFIG = $previousPrepared
    $env:CARGO_NET_OFFLINE = $previousOffline
    $env:CARGO_ENCODED_RUSTFLAGS = $previousEncodedFlags
    $env:RUSTFLAGS = $previousFlags
    $env:BARELINE_BUILD_HASH = $previousBuildHash
    $env:BARELINE_BUILD_VERSION = $previousBuildVersion
    Pop-Location
    if ((Get-FileHash -LiteralPath $lockPath -Algorithm SHA256).Hash -ne $lockHash) { throw 'Cargo.lock changed during preview packaging; review it before retrying. No lockfile was restored or overwritten by this script.' }
}
