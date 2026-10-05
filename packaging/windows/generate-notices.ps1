# SPDX-License-Identifier: MPL-2.0
# Generate local notices from the exact locked Windows dependency graph and vendored
# license texts. Refuses missing evidence; does not invent third-party license grants.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OutputFile,
    [string]$SdkOutputFile,
    [ValidateNotNullOrEmpty()][ValidateSet('bareline','bareline-update-helper','bareline-extension-host')]
    [string[]]$Roots = @('bareline','bareline-update-helper','bareline-extension-host')
)
$ErrorActionPreference = 'Stop'
$metadataText = & cargo metadata --format-version 1 --locked --offline --filter-platform x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
$metadata = ($metadataText -join "`n") | ConvertFrom-Json
foreach ($name in $roots) { if (-not ($metadata.packages | Where-Object name -eq $name)) { throw "Missing packaged Cargo root: $name" } }
$ids = [Collections.Generic.HashSet[string]]::new()
$pending = [Collections.Generic.Queue[string]]::new()
foreach ($package in $metadata.packages | Where-Object { $_.name -in $Roots }) { $pending.Enqueue($package.id) }
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
$parts.Add('Packaged Cargo roots: ' + (($Roots | Sort-Object -Unique) -join ', ') + '.')
$parts.Add('Generated from Cargo.lock and registry package license files for Windows x64. This is local source-license evidence; release review and SBOM remain required.')
$missing = [Collections.Generic.List[string]]::new()
# Cargo path patches have no registry source, but still require upstream notices.
$vendorRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../vendor')) + [IO.Path]::DirectorySeparatorChar
foreach ($package in ($metadata.packages | Where-Object {
    $ids.Contains($_.id) -and ($_.source -or [IO.Path]::GetFullPath($_.manifest_path).StartsWith($vendorRoot, [StringComparison]::OrdinalIgnoreCase))
} | Sort-Object name,version)) {
    $directory = Split-Path -Parent $package.manifest_path
    # A vendored path patch keeps upstream attribution (AUTHORS) beside its license files.
    $pattern = if ($package.source) { '^(LICENSE|LICENCE|COPYING|NOTICE)([-._].*)?$' } else { '^(LICENSE|LICENCE|COPYING|NOTICE|AUTHORS)([-._].*)?$' }
    $licenses = @(Get-ChildItem -LiteralPath $directory -File | Where-Object { $_.Name -match $pattern })
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
# Cargo metadata cannot describe the native C/C++ sources that crates compile
# (Lexilla and Scintilla in the bridge; PCRE2 and its SLJIT JIT in pcre2-sys).
$native = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'native-components.psd1')
foreach ($component in $native.Components) {
    if (-not ($metadata.packages | Where-Object name -eq $component.CargoPackage)) { throw "Unknown Cargo package for native $($component.Name): $($component.CargoPackage)" }
    $carriers = @($metadata.packages | Where-Object { $ids.Contains($_.id) -and $_.name -eq $component.CargoPackage })
    if (-not $carriers.Count) { continue }
    if ($component.CargoVersion -and @($carriers | Where-Object version -ne $component.CargoVersion).Count) {
        throw "Native $($component.Name) $($component.Version) is recorded for $($component.CargoPackage) $($component.CargoVersion); update native-components.psd1 and its license texts"
    }
    $parts.Add("## Native $($component.Name) $($component.Version)")
    $parts.Add("Declared license: $($component.License). Compiled by $($component.CargoPackage).")
    foreach ($relative in $component.LicenseFiles) {
        $license = Join-Path $PSScriptRoot "../../$relative"
        if (-not (Test-Path -LiteralPath $license -PathType Leaf)) { $missing.Add("native $($component.Name)"); continue }
        $parts.Add("### $(Split-Path -Leaf $relative)")
        $parts.Add([IO.File]::ReadAllText((Resolve-Path -LiteralPath $license).Path))
    }
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
