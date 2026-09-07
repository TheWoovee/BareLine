# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$PayloadDir,
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [Parameter(Mandatory)][string]$OutputDir,
    [string]$Iscc,
    [switch]$Installer
)
$ErrorActionPreference = 'Stop'
$payload = (Resolve-Path -LiteralPath $PayloadDir).Path
[System.IO.Directory]::CreateDirectory([System.IO.Path]::GetFullPath($OutputDir)) | Out-Null
$output = (Resolve-Path -LiteralPath $OutputDir).Path
$names = @('bareline.exe', 'bareline-update-helper.exe', 'LICENSE', 'THIRD-PARTY-NOTICES.md', 'SBOM.json')
foreach ($name in $names) {
    $item = Get-Item -LiteralPath (Join-Path $payload $name)
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Payload must contain regular files: $name" }
}
# Fixed order/timestamp, no compression: reproducible container for identical input bytes.
Add-Type -AssemblyName System.IO.Compression
$zipPath = Join-Path $output "bareline-$Version-windows-x64-portable.zip"
$stream = [IO.File]::Open($zipPath, [IO.FileMode]::CreateNew)
try {
    $zip = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Create, $true)
    try {
        foreach ($name in ($names + 'bareline.portable' | Sort-Object)) {
            $entry = $zip.CreateEntry($name, [IO.Compression.CompressionLevel]::NoCompression)
            $entry.LastWriteTime = [DateTimeOffset]::new(1980,1,1,0,0,0,[TimeSpan]::Zero)
            $destination = $entry.Open()
            try {
                if ($name -ne 'bareline.portable') {
                    $source = [IO.File]::OpenRead((Join-Path $payload $name))
                    try { $source.CopyTo($destination) } finally { $source.Dispose() }
                }
            } finally { $destination.Dispose() }
        }
    } finally { $zip.Dispose() }
} finally { $stream.Dispose() }
if ($Installer) {
    if (-not $Iscc) { throw 'Installer requires explicit path to pinned Inno Setup 6.4.3 ISCC.exe' }
    $compiler = Get-Item -LiteralPath $Iscc
    if ($compiler.VersionInfo.ProductVersion -notmatch '^6\.4\.3(?:\D|$)') { throw 'Expected pinned Inno Setup 6.4.3' }
    & $compiler.FullName "/DAppVersion=$Version" "/DPayloadDir=$payload" "/DOutputDir=$output" (Join-Path $PSScriptRoot 'bareline.iss')
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed: $LASTEXITCODE" }
}
$inventory = foreach ($file in (Get-ChildItem -LiteralPath $output -File | Where-Object { $_.Name -match '\.(zip|exe)$' } | Sort-Object Name)) {
    '{0}  {1}' -f (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $file.Name
}
[IO.File]::WriteAllText((Join-Path $output 'SHA-256SUMS'), (($inventory -join "`n") + "`n"), [Text.UTF8Encoding]::new($false))
Write-Output "Local unsigned package assembled: $zipPath"
Write-Output 'Release requires owner-approved signing and final signed-byte inventory regeneration.'
