#requires -Version 7.0
# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ArtifactDir,
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [switch]$RequireInstaller,
    # The commit the editor must record for About and diagnostics.
    [ValidatePattern('^([0-9a-f]{40})?$')][string]$BuildHash
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'pe-hardening.ps1')
$directory = (Resolve-Path -LiteralPath $ArtifactDir).Path
if ((Get-Item -LiteralPath $directory).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse artifact directory rejected.' }
$zipName = "bareline-$Version-windows-x64-portable.zip"
$installerName = "bareline-$Version-windows-x64-setup.exe"
$documents = @('LICENSE', 'THIRD-PARTY-NOTICES.md', 'SBOM.json', 'PREVIEW-NOTES.md')
$required = @($zipName, 'SHA-256SUMS') + $documents
if ($RequireInstaller -or (Test-Path -LiteralPath (Join-Path $directory $installerName))) { $required += $installerName }
$files = @(Get-ChildItem -LiteralPath $directory -Force)
if (@(Compare-Object ($required | Sort-Object) ($files.Name | Sort-Object)).Count) { throw 'Unexpected or missing preview artifact.' }
foreach ($file in $files) {
    if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $file.Length -eq 0) { throw "Preview assets must be nonempty regular files: $($file.Name)" }
}
$seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
foreach ($line in [IO.File]::ReadAllLines((Join-Path $directory 'SHA-256SUMS'))) {
    if ($line -cnotmatch '^([a-f0-9]{64})  ([A-Za-z0-9][A-Za-z0-9._-]*)$') { throw 'Malformed preview checksum inventory.' }
    $hash = $Matches[1]; $name = $Matches[2]
    if ($name -eq 'SHA-256SUMS' -or $name -notin $required -or -not $seen.Add($name)) { throw 'Unexpected or duplicate preview checksum entry.' }
    if ((Get-FileHash -LiteralPath (Join-Path $directory $name) -Algorithm SHA256).Hash -ne $hash) { throw "Preview checksum mismatch: $name" }
}
if ($seen.Count -ne $required.Count - 1) { throw 'Preview checksum inventory is incomplete.' }

function Assert-PreviewExecutable([byte[]]$Bytes, [string]$Component) {
    if ($Bytes.Length -lt 256 -or $Bytes[0] -ne 0x4d -or $Bytes[1] -ne 0x5a) { throw 'Expected Windows executable.' }
    $offset = [BitConverter]::ToInt32($Bytes, 0x3c)
    if ($offset -lt 64 -or $offset -gt $Bytes.Length - 264 -or [BitConverter]::ToUInt32($Bytes, $offset) -ne 0x4550 -or [BitConverter]::ToUInt16($Bytes, $offset + 4) -ne 0x8664 -or [BitConverter]::ToUInt16($Bytes, $offset + 24) -ne 0x20b) { throw 'Expected x64 PE32+ executable.' }
    # The PE security directory must be empty for this explicitly unsigned path.
    if ([BitConverter]::ToUInt64($Bytes, $offset + 24 + 112 + 4 * 8) -ne 0) { throw 'Signed executable supplied to unsigned preview verification.' }
    $marker = "BARELINE-CAPABILITY|component=$Component|mode=preview|config-version=none|config=none|source=unrecorded|version=$Version|features=updates=disabled,extensions=disabled,runtime=external"
    $text = [Text.Encoding]::ASCII.GetString($Bytes)
    if (-not $text.Contains($marker) -or $text.Contains('|mode=configured|') -or $text.Contains('|mode=fixture|')) { throw "Executable is not the expected core preview: $Component" }
    if ($BuildHash -and $Component -eq 'editor' -and -not $text.Contains($BuildHash)) { throw 'Editor does not record the expected build commit.' }
    Assert-HardenedExecutable $Bytes $Component
}

$expectedEntries = @('bareline.exe', 'bareline-update-helper.exe', 'LICENSE', 'THIRD-PARTY-NOTICES.md', 'SBOM.json', 'bareline.portable')
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::OpenRead((Join-Path $directory $zipName))
try {
    if ($archive.Entries.Count -ne $expectedEntries.Count -or @(Compare-Object ($expectedEntries | Sort-Object) ($archive.Entries.FullName | Sort-Object)).Count) { throw 'Portable ZIP inventory is invalid.' }
    foreach ($entry in $archive.Entries) {
        if ($entry.LastWriteTime.Year -ne 1980 -or $entry.LastWriteTime.Month -ne 1 -or $entry.LastWriteTime.Day -ne 1) { throw 'Portable ZIP timestamp is not normalized.' }
        $stream = $entry.Open()
        try {
            if ($entry.FullName -eq 'bareline.portable') {
                if ($entry.Length -ne 0) { throw 'Portable marker must be empty.' }
            } elseif ($entry.FullName.EndsWith('.exe')) {
                $memory = [IO.MemoryStream]::new()
                try { $stream.CopyTo($memory); $component = if ($entry.FullName -eq 'bareline.exe') { 'editor' } else { 'update-helper' }; Assert-PreviewExecutable $memory.ToArray() $component } finally { $memory.Dispose() }
            } else {
                $hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($stream))
                if ($hash -ne (Get-FileHash -LiteralPath (Join-Path $directory $entry.FullName) -Algorithm SHA256).Hash) { throw "Packaged document differs from release asset: $($entry.FullName)" }
            }
        } finally { $stream.Dispose() }
    }
} finally { $archive.Dispose() }
$sbom = Get-Content -LiteralPath (Join-Path $directory 'SBOM.json') -Raw | ConvertFrom-Json
if ($sbom.bomFormat -ne 'CycloneDX' -or -not $sbom.specVersion -or -not $sbom.components.Count -or -not $sbom.dependencies.Count) { throw 'Expected a nonempty CycloneDX dependency SBOM.' }
foreach ($name in @('bareline', 'bareline-update-helper', 'lexilla', 'scintilla')) {
    if ($name -notin $sbom.components.name) { throw "Missing packaged component in SBOM: $name" }
}
$notices = [IO.File]::ReadAllText((Join-Path $directory 'THIRD-PARTY-NOTICES.md'))
foreach ($text in @('Packaged Cargo roots: bareline, bareline-update-helper.', '# SDK and first-party licenses', 'UNICODE LICENSE V3')) {
    if (-not $notices.Contains($text)) { throw "Missing preview license evidence: $text" }
}
$notes = [IO.File]::ReadAllText((Join-Path $directory 'PREVIEW-NOTES.md'))
if (-not $notes.Contains('unsigned preview') -or -not $notes.Contains($Version)) { throw 'Preview release notes must identify the unsigned scope and version.' }
Write-Output 'PASS: preview inventory, SHA-256, portable layout, x64 preview capabilities, static CRT/CFG/CET hardening, licenses and CycloneDX SBOM.'
