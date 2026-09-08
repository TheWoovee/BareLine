# SPDX-License-Identifier: MPL-2.0
# Read-only, fail-closed final release inventory validation. Never signs or publishes.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ArtifactDir,
    [Parameter(Mandatory)][string]$Minisign,
    [Parameter(Mandatory)][string]$ReleasePublicKey,
    [Parameter(Mandatory)][string]$PublisherCertificateSha256,
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $ArtifactDir).Path
if ((Get-Item -LiteralPath $root).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse artifact directory rejected' }
. (Join-Path $PSScriptRoot 'release-layout.ps1')
$required = @(Get-RequiredReleaseFiles $Version)
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
            if ($inner.Count -ne 6) { throw 'Unexpected portable payload entry' }
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
Write-Output 'Inventory signature, listed hashes, executable publishers and ZIP inner executable publishers verified.'
Write-Output 'Installer extraction and installed-payload signature check remain a separate clean-VM acceptance step.'
