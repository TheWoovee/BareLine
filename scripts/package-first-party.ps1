# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param([string]$OutputDir, [string]$TargetDir)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path $PSScriptRoot -Parent
$componentTarget = if ($TargetDir) { [IO.Path]::GetFullPath($TargetDir) } elseif ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) } else { Join-Path $taskRoot 'target' }
$taskOutput = if ($OutputDir) { [IO.Path]::GetFullPath($OutputDir) } else { Join-Path $taskRoot 'target/extension-packages' }
if (Test-Path -LiteralPath $taskOutput) { throw 'Use a new first-party package output directory' }
New-Item -ItemType Directory $taskOutput | Out-Null
Add-Type -AssemblyName System.IO.Compression
foreach ($taskName in @('json-tools', 'xml-tools', 'hex-view')) {
    $taskComponent = Join-Path $componentTarget ('wasm32-wasip2/release/bareline_' + $taskName.Replace('-', '_') + '.wasm')
    if (!(Test-Path -LiteralPath $taskComponent)) { throw 'Build components using test-first-party.ps1 first' }
    $taskArchive = Join-Path $taskOutput ($taskName + '.blex')
    $taskStream = [System.IO.File]::Open($taskArchive, [System.IO.FileMode]::CreateNew)
    try {
        $taskZip = [System.IO.Compression.ZipArchive]::new($taskStream, [System.IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($taskEntry in @(@('manifest.toml', (Join-Path $taskRoot "extensions/$taskName/manifest.toml")), @('extension.wasm', $taskComponent))) {
                $taskItem = $taskZip.CreateEntry($taskEntry[0], [System.IO.Compression.CompressionLevel]::NoCompression)
                $taskItem.LastWriteTime = [DateTimeOffset]::new(2020, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
                $taskInput = [System.IO.File]::OpenRead($taskEntry[1])
                $taskDestination = $taskItem.Open()
                try { $taskInput.CopyTo($taskDestination) } finally { $taskDestination.Dispose(); $taskInput.Dispose() }
            }
        } finally { $taskZip.Dispose() }
    } finally { $taskStream.Dispose() }
    Get-FileHash -Algorithm SHA256 -LiteralPath $taskArchive
}
Write-Output 'Unsigned first-party component packages created; catalog metadata and signatures are separate owner artifacts.'
