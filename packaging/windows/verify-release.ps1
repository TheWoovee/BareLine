# SPDX-License-Identifier: MPL-2.0
# Read-only, fail-closed final release inventory validation. Never signs or publishes.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ArtifactDir,
    [Parameter(Mandatory)][string]$Minisign,
    [Parameter(Mandatory)][string]$ReleasePublicKey,
    [Parameter(Mandatory)][string]$PublisherCertificateSha256,
    [Parameter(Mandatory)][string]$ReleaseConfig,
    [Parameter(Mandatory)][string]$AuthorityVerifier,
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $ArtifactDir).Path
if ((Get-Item -LiteralPath $root).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse artifact directory rejected' }
. (Join-Path $PSScriptRoot 'release-layout.ps1')
$required = @(Get-RequiredReleaseFiles $Version)
. (Join-Path $PSScriptRoot 'authority-payload.ps1')
$authorityNames = @(Get-AuthorityPayloadFiles $root -Required)
$authority = Test-AuthorityPayload $root $ReleaseConfig $AuthorityVerifier
if ($authority.authority.release_public_key -cne $ReleasePublicKey -or
    $authority.authority.publisher_certificate_sha256 -ne $PublisherCertificateSha256) { throw 'Final pins do not match verified authority' }
$required += $authorityNames
$sums = Join-Path $root 'SHA-256SUMS'
& $Minisign -V -P $ReleasePublicKey -m $sums -x (Join-Path $root 'SHA-256SUMS.minisig')
if ($LASTEXITCODE -ne 0) { throw 'Checksum inventory minisign verification failed' }
if ($PublisherCertificateSha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'Pinned SHA256 certificate fingerprint required' }
function Test-Publisher([string]$Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or -not $signature.SignerCertificate) { throw "Invalid Authenticode: $Path" }
    $hash = [Security.Cryptography.SHA256]::Create()
    try { $fingerprint = ([BitConverter]::ToString($hash.ComputeHash($signature.SignerCertificate.RawData))).Replace('-', '') } finally { $hash.Dispose() }
    if ($fingerprint -ne $PublisherCertificateSha256) { throw "Unexpected publisher: $Path" }
}
$seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($line in [IO.File]::ReadAllLines($sums)) {
    if ($line -notmatch '^([a-f0-9]{64})  ([A-Za-z0-9][A-Za-z0-9._-]*)$') { throw 'Malformed checksum inventory' }
    $expected = $Matches[1]; $name = $Matches[2]
    if (-not $seen.Add($name)) { throw 'Duplicate artifact' }
    $file = Join-Path $root $name
    $item = Get-Item -LiteralPath $file
    if ($item.PSIsContainer -or $item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse artifact rejected' }
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $expected) { throw "Hash mismatch: $name" }
    if ($name.EndsWith('.exe')) { Test-Publisher $file }
    if ($name.EndsWith('.zip')) {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $archive = [IO.Compression.ZipFile]::OpenRead($file)
        try {
            $inner = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
            foreach ($entry in $archive.Entries) {
                if (-not $inner.Add($entry.FullName)) { throw 'Duplicate ZIP entry' }
                if ($entry.FullName -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$') { throw 'Unexpected nested or unsafe ZIP path' }
                if ($entry.FullName -in $authorityNames) {
                    $inputStream = $entry.Open()
                    $hash = [Security.Cryptography.SHA256]::Create()
                    try { $digest = ([BitConverter]::ToString($hash.ComputeHash($inputStream))).Replace('-', '') }
                    finally { $inputStream.Dispose(); $hash.Dispose() }
                    if ($digest -ne (Get-FileHash -LiteralPath (Join-Path $root $entry.FullName) -Algorithm SHA256).Hash) { throw 'Packaged authority differs from verified release authority' }
                }
                if ($entry.Name.EndsWith('.exe')) {
                    $temp = [IO.Path]::GetTempFileName()
                    try {
                        $inputStream = $entry.Open(); $outputStream = [IO.File]::OpenWrite($temp)
                        try { $inputStream.CopyTo($outputStream) } finally { $inputStream.Dispose(); $outputStream.Dispose() }
                        Test-Publisher $temp
                    } finally { Remove-Item -LiteralPath $temp -Force }
                }
            }
            foreach ($name in @('bareline.exe','bareline-update-helper.exe','bareline.portable','LICENSE','THIRD-PARTY-NOTICES.md','SBOM.json')) { if (-not $inner.Contains($name)) { throw "Missing portable payload: $name" } }
            foreach ($name in $authorityNames) { if (-not $inner.Contains($name)) { throw "Missing portable authority: $name" } }
            if ($inner.Count -ne (6 + $authorityNames.Count)) { throw 'Unexpected portable payload entry' }
        } finally { $archive.Dispose() }
    }
}
foreach ($name in $required) { if (-not $seen.Contains($name)) { throw "Missing required release artifact: $name" } }
foreach ($file in Get-ChildItem -LiteralPath $root -Force) {
    if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Unexpected directory/reparse artifact: $($file.Name)" }
    if ($file.Name -notin 'SHA-256SUMS','SHA-256SUMS.minisig' -and -not $seen.Contains($file.Name)) { throw "Unlisted artifact: $($file.Name)" }
}
$sbom = Get-Content -LiteralPath (Join-Path $root 'SBOM.json') -Raw | ConvertFrom-Json
if ($sbom.bomFormat -ne 'CycloneDX' -or -not $sbom.specVersion -or -not $sbom.components) { throw 'Valid nonempty CycloneDX SBOM required' }
foreach ($name in @('RELEASE-NOTES.md','MIGRATION-NOTES.md','KNOWN-ISSUES.md','SDK-LICENSES.md','THIRD-PARTY-NOTICES.md')) {
    $text = Get-Content -LiteralPath (Join-Path $root $name) -Raw
    if ([string]::IsNullOrWhiteSpace($text) -or $text -match '\{\{[^}]+\}\}') { throw "Incomplete release document: $name" }
}
if ($seen.Count -eq 0) { throw 'Empty inventory' }
# Verify update/runtime/catalog semantics against the actual final portable and
# external runtime bytes, using the same parsers and policies as the product.
$deliveryScratch = Join-Path ([IO.Path]::GetTempPath()) ('bareline-final-delivery-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($deliveryScratch) | Out-Null
try {
    $archive = [IO.Compression.ZipFile]::OpenRead((Join-Path $root "bareline-$Version-windows-x64-portable.zip"))
    try {
        foreach ($entry in $archive.Entries) {
            if ($entry.FullName -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$' -or $entry.Length -gt 1GB) { throw 'Unsafe final portable entry' }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $deliveryScratch $entry.FullName), $false)
        }
    } finally { $archive.Dispose() }
    $metadataNames = @('bareline.update.json','bareline.update.minisig','runtime.json','runtime.minisig','catalog.json','catalog.json.minisig')
    foreach ($name in $metadataNames) { [IO.File]::Copy((Join-Path $root $name), (Join-Path $deliveryScratch $name), $false) }
    [IO.File]::Copy((Join-Path $root 'bareline-exthost-x64.exe'), (Join-Path $deliveryScratch 'bareline-extension-host.exe'), $false)
    $catalogFile = Get-Item -LiteralPath (Join-Path $root 'catalog.json')
    if ($catalogFile.Length -gt 1MB) { throw 'Catalog size limit' }
    $catalog = Get-Content -LiteralPath $catalogFile.FullName -Raw | ConvertFrom-Json
    foreach ($entry in $catalog.entries) {
        if ($entry.sha256 -cnotmatch '^[a-f0-9]{64}$') { throw 'Unsafe catalog digest' }
        $name = $entry.sha256 + '.blex'
        if (-not $seen.Contains($name)) { throw 'Catalog package absent from signed inventory' }
        [IO.File]::Copy((Join-Path $root $name), (Join-Path $deliveryScratch $name), $false)
    }
    & $AuthorityVerifier --config $ReleaseConfig --directory $deliveryScratch --delivery-directory $deliveryScratch --now ([DateTimeOffset]::UtcNow.ToUnixTimeSeconds()) | Out-Null
    if ($LASTEXITCODE) { throw 'Final update/runtime/catalog semantics or payload bytes failed verification' }
} finally {
    $resolvedScratch = [IO.Path]::GetFullPath($deliveryScratch)
    $tempPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolvedScratch.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase) -or -not ([IO.Path]::GetFileName($resolvedScratch)).StartsWith('bareline-final-delivery-')) { throw 'Unsafe final verification cleanup target' }
    Remove-Item -LiteralPath $resolvedScratch -Recurse -Force
}
Write-Output 'Inventory, publisher pins, packaged authority, signed update/runtime/catalog metadata and actual payload bytes verified.'
Write-Output 'Installer extraction and installed-payload signature check remain a separate clean-VM acceptance step.'
