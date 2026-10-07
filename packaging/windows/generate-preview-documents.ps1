#requires -Version 7.0
# SPDX-License-Identifier: MPL-2.0
# THIRD-PARTY-NOTICES.md and SBOM.json for the Linux and macOS previews, which
# package the editor and the extension host. Same generators, notice supplements
# and SBOM normalization as build-preview.ps1, for the locked dependency graph of
# -Target. Needs pwsh, Cargo and cargo-cyclonedx 0.5.9, not a build; it runs on
# any host, so a Windows checkout can produce the Linux or macOS documents.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('x86_64-unknown-linux-gnu', 'aarch64-apple-darwin')][string]$Target,
    # A new directory for the two documents.
    [Parameter(Mandatory)][string]$OutputDir
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDir)
if (Test-Path -LiteralPath $output) { throw "Use a new documents directory: $output" }
$roots = @('bareline', 'bareline-extension-host')
$lockPath = Join-Path $root 'Cargo.lock'
$lockHash = (Get-FileHash -LiteralPath $lockPath -Algorithm SHA256).Hash
$runId = [Guid]::NewGuid().ToString('N')
$stage = Join-Path $root "target/preview-documents-$runId"
$sbomDirectory = Join-Path $stage 'cargo-sboms'
$previousOffline = $env:CARGO_NET_OFFLINE
Push-Location $root
try {
    $toolVersion = & cargo cyclonedx --version
    if ($LASTEXITCODE -ne 0 -or ($toolVersion -join "`n") -notmatch ' 0\.5\.9\s*$') {
        throw 'Install the required SBOM tool first: cargo install --locked cargo-cyclonedx --version =0.5.9'
    }
    # Metadata/notices need some workspace dependencies not used by the two binaries.
    & cargo fetch --locked --target $Target
    if ($LASTEXITCODE -ne 0) { throw "Could not fetch the locked $Target dependency graph." }
    $metadataText = & cargo metadata --locked --offline --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Could not read locked workspace metadata.' }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    [IO.Directory]::CreateDirectory($output) | Out-Null
    [IO.Directory]::CreateDirectory($sbomDirectory) | Out-Null

    $noticesPath = Join-Path $output 'THIRD-PARTY-NOTICES.md'
    & (Join-Path $PSScriptRoot 'generate-notices.ps1') -Target $Target -OutputFile $noticesPath -SdkOutputFile (Join-Path $stage 'SDK-LICENSES.md') -Roots $roots
    # As in the Windows packages: local SDK grants and the Unicode data licenses
    # follow the generated notices.
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
    & cargo metadata --locked --offline --no-default-features --filter-platform $Target --format-version 1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Locked SBOM dependency resolution failed.' }
    $sbomBaseName = "preview-$runId.cdx"
    $members = @($metadata.packages | Where-Object { $_.id -in $metadata.workspace_members })
    foreach ($member in $members) {
        $path = Join-Path (Split-Path -Parent $member.manifest_path) "$sbomBaseName.json"
        if (Test-Path -LiteralPath $path) { throw "Refusing to replace an existing SBOM: $path" }
    }
    & cargo cyclonedx --format json --all --no-default-features --target $Target --override-filename $sbomBaseName
    if ($LASTEXITCODE -ne 0) { throw 'Cargo SBOM generation failed; any partial .cdx.json files are retained.' }
    if ((Get-FileHash -LiteralPath $lockPath -Algorithm SHA256).Hash -ne $lockHash) { throw 'SBOM generation changed Cargo.lock; stopped. Review the lockfile before retrying.' }
    $sboms = @{}
    foreach ($member in $members) {
        $source = Join-Path (Split-Path -Parent $member.manifest_path) "$sbomBaseName.json"
        $destination = Join-Path $sbomDirectory "$($member.name).json"
        [IO.File]::Move($source, $destination)
        $sboms[$member.name] = $destination
    }
    & (Join-Path $PSScriptRoot 'generate-sbom.ps1') -Target $Target -CargoSbom $sboms['bareline'] -HelperCargoSbom $sboms['bareline-extension-host'] -OutputFile (Join-Path $output 'SBOM.json')
    Write-Output "Preview documents for ${Target}: $output"
    Write-Output "Staged SDK licenses and dependency SBOMs: $stage"
} finally {
    $env:CARGO_NET_OFFLINE = $previousOffline
    Pop-Location
    if ((Get-FileHash -LiteralPath $lockPath -Algorithm SHA256).Hash -ne $lockHash) { throw 'Cargo.lock changed while generating the preview documents; review it before retrying.' }
}
