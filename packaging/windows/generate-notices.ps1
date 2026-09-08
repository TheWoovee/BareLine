# SPDX-License-Identifier: MPL-2.0
# Generate local notices from the exact locked Windows dependency graph and vendored
# license texts. Refuses missing evidence; does not invent third-party license grants.
[CmdletBinding()]
param([Parameter(Mandatory)][string]$OutputFile,[string]$SdkOutputFile)
$ErrorActionPreference = 'Stop'
$metadataText = & cargo metadata --format-version 1 --locked --offline --filter-platform x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
$metadata = ($metadataText -join "`n") | ConvertFrom-Json
$roots = @('bareline','bareline-update-helper','bareline-extension-host')
foreach ($name in $roots) { if (-not ($metadata.packages | Where-Object name -eq $name)) { throw "Missing packaged Cargo root: $name" } }
$ids = [Collections.Generic.HashSet[string]]::new()
$pending = [Collections.Generic.Queue[string]]::new()
foreach ($package in $metadata.packages | Where-Object { $_.name -in 'bareline', 'bareline-update-helper', 'bareline-extension-host' }) { $pending.Enqueue($package.id) }
while ($pending.Count) {
    $id = $pending.Dequeue()
    if (-not $ids.Add($id)) { continue }
    $node = $metadata.resolve.nodes | Where-Object { $_.id -eq $id }
    foreach ($dep in $node.deps) {
        if (@($dep.dep_kinds | Where-Object { $_.kind -ne 'dev' }).Count) { $pending.Enqueue($dep.pkg) }
    }
}
$parts = [Collections.Generic.List[string]]::new()
$parts.Add('# Third-party notices')
$parts.Add('Generated from Cargo.lock and registry package license files for Windows x64. This is local source-license evidence; release review and SBOM remain required.')
$missing = [Collections.Generic.List[string]]::new()
foreach ($package in ($metadata.packages | Where-Object { $_.source -and $ids.Contains($_.id) } | Sort-Object name,version)) {
    $directory = Split-Path -Parent $package.manifest_path
    $licenses = @(Get-ChildItem -LiteralPath $directory -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)([-._].*)?$' })
    if ($package.license_file) { $licenses += Get-Item -LiteralPath (Join-Path $directory $package.license_file) }
    foreach ($folder in @('license', 'licenses')) {
        $path = Join-Path $directory $folder
        if (Test-Path -LiteralPath $path -PathType Container) { $licenses += Get-ChildItem -LiteralPath $path -File -Recurse }
    }
    $supplement = Join-Path $PSScriptRoot ("license-overrides/" + $package.name + "-" + $package.version)
    if (Test-Path -LiteralPath $supplement -PathType Container) { $licenses += Get-ChildItem -LiteralPath $supplement -File | Where-Object { $_.Name -match '^(LICENSE|AUTHORS)' } }
    $licenses = @($licenses | Sort-Object FullName -Unique)
    if (-not $licenses.Count) { $missing.Add("$($package.name) $($package.version)"); continue }
    $parts.Add("## $($package.name) $($package.version)")
    $parts.Add("Declared license: $($package.license)")
    foreach ($license in $licenses) {
        $parts.Add("### $($license.Name)")
        $parts.Add([IO.File]::ReadAllText($license.FullName))
    }
}
# Cargo metadata cannot describe the native sources compiled by the bridge.
foreach ($component in @('lexilla', 'scintilla')) {
    $license = Join-Path $PSScriptRoot "../../native/lexilla-bridge/bundled/$component/License.txt"
    if (-not (Test-Path -LiteralPath $license -PathType Leaf)) { $missing.Add("native $component"); continue }
    $parts.Add("## Native $component")
    $parts.Add([IO.File]::ReadAllText((Resolve-Path -LiteralPath $license).Path))
}
if ($missing.Count) { throw ('Missing upstream license texts: ' + ($missing -join ', ')) }
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputFile), ($parts -join "`n`n"), [Text.UTF8Encoding]::new($false))
Write-Output "Generated notices: $OutputFile"

# Preserve the repository's exact dual-license texts and identify actual SDK packages.
if (-not $SdkOutputFile) { $SdkOutputFile = Join-Path ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($OutputFile))) 'SDK-LICENSES.md' }
if ([IO.Path]::GetFullPath($SdkOutputFile) -eq [IO.Path]::GetFullPath($OutputFile)) { throw 'SDK and third-party outputs must be distinct' }
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$sdk = [Collections.Generic.List[string]]::new()
$sdk.Add('# SDK and first-party licenses')
foreach ($package in @($metadata.packages | Where-Object { -not $_.source -and $_.license -eq 'MIT OR Apache-2.0' } | Sort-Object name,version)) {
    $sdk.Add("- $($package.name) $($package.version): $($package.license)")
}
foreach ($name in @('LICENSE-SDK','LICENSE-MIT','LICENSE-APACHE')) {
    $path = Join-Path $repo $name
    $item = Get-Item -LiteralPath $path
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "SDK license must be regular: $name" }
    $sdk.Add("## $name")
    $sdk.Add([IO.File]::ReadAllText($path))
}
[IO.File]::WriteAllText([IO.Path]::GetFullPath($SdkOutputFile),($sdk -join "`n`n"),[Text.UTF8Encoding]::new($false))
Write-Output "Generated SDK license aggregation: $SdkOutputFile"