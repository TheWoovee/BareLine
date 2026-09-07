# SPDX-License-Identifier: MPL-2.0
# Read-only, fail-closed final release inventory validation. Never signs or publishes.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ArtifactDir,
    [Parameter(Mandatory)][string]$Minisign,
    [Parameter(Mandatory)][string]$ReleasePublicKey,
    [Parameter(Mandatory)][string]$PublisherCertificateSha256
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $ArtifactDir).Path
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
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse artifact rejected' }
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $expected) { throw "Hash mismatch: $name" }
    if ($name.EndsWith('.exe')) { Test-Publisher $file }
    if ($name.EndsWith('.zip')) {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $archive = [IO.Compression.ZipFile]::OpenRead($file)
        try {
            foreach ($entry in $archive.Entries) {
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
        } finally { $archive.Dispose() }
    }
}
foreach ($file in Get-ChildItem -LiteralPath $root -File) {
    if ($file.Extension -in '.exe', '.zip' -and -not $seen.Contains($file.Name)) { throw "Unlisted artifact: $($file.Name)" }
}
if ($seen.Count -eq 0) { throw 'Empty inventory' }
Write-Output 'Inventory signature, listed hashes, executable publishers and ZIP inner executable publishers verified.'
Write-Output 'Installer extraction and installed-payload signature check remain a separate clean-VM acceptance step.'
