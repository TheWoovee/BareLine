# SPDX-License-Identifier: MPL-2.0
# Synthetic fixtures only: does not represent a shippable editor package.
$ErrorActionPreference = 'Stop'
$taskRoot = Join-Path ([IO.Path]::GetTempPath()) ('bareline-package-test-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($taskRoot) | Out-Null
try {
    $payload = Join-Path $taskRoot 'payload'
    [IO.Directory]::CreateDirectory($payload) | Out-Null
    foreach ($name in @('bareline.exe', 'bareline-update-helper.exe', 'LICENSE', 'THIRD-PARTY-NOTICES.md', 'SBOM.json')) { [IO.File]::WriteAllText((Join-Path $payload $name), "fixture: $name") }
    $one = Join-Path $taskRoot 'one'; $two = Join-Path $taskRoot 'two'
    & (Join-Path $PSScriptRoot 'build.ps1') -PayloadDir $payload -Version 0.1.0 -OutputDir $one
    & (Join-Path $PSScriptRoot 'build.ps1') -PayloadDir $payload -Version 0.1.0 -OutputDir $two
    $zipName = 'bareline-0.1.0-windows-x64-portable.zip'
    if ((Get-FileHash -LiteralPath (Join-Path $one $zipName)).Hash -ne (Get-FileHash -LiteralPath (Join-Path $two $zipName)).Hash) { throw 'ZIP reproducibility failure' }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead((Join-Path $one $zipName))
    try {
        if ($archive.Entries.Count -ne 6 -or -not $archive.GetEntry('bareline.portable')) { throw 'Portable marker/inventory missing' }
        foreach ($entry in $archive.Entries) {
            if ($entry.Name -eq 'bareline.portable') { continue }
            $reader = [IO.StreamReader]::new($entry.Open())
            try { if ($reader.ReadToEnd() -ne "fixture: $($entry.Name)") { throw 'Payload bytes changed' } } finally { $reader.Dispose() }
        }
    } finally { $archive.Dispose() }
    $refused = $false
    try { & (Join-Path $PSScriptRoot 'build.ps1') -PayloadDir $payload -Version 0.1.0 -OutputDir $one } catch { $refused = $true }
    if (-not $refused) { throw 'Existing artifact overwritten' }
    Remove-Item -LiteralPath (Join-Path $payload 'LICENSE')
    $refused = $false
    try { & (Join-Path $PSScriptRoot 'build.ps1') -PayloadDir $payload -Version 0.1.0 -OutputDir (Join-Path $taskRoot 'missing') } catch { $refused = $true }
    if (-not $refused) { throw 'Missing payload accepted' }
    Write-Output 'PASS: deterministic ZIP, extracted bytes, marker, refuse overwrite and missing payload.'
} finally {
    $resolved = [IO.Path]::GetFullPath($taskRoot)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or -not ([IO.Path]::GetFileName($resolved)).StartsWith('bareline-package-test-')) { throw 'Unsafe test cleanup target' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
