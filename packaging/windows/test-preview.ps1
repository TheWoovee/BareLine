#requires -Version 7.0
# SPDX-License-Identifier: MPL-2.0
# Synthetic packages exercise verification failures; no application is launched.
$ErrorActionPreference = 'Stop'
$scratch = Join-Path ([IO.Path]::GetTempPath()) ('bareline-preview-contract-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($scratch) | Out-Null
$version = '0.1.0'
$commit = '0123456789abcdef0123456789abcdef01234567'
$utf8 = [Text.UTF8Encoding]::new($false)
function Write-Inventory([string]$Directory) {
    $lines = @(Get-ChildItem -LiteralPath $Directory -File | Where-Object Name -ne 'SHA-256SUMS' | Sort-Object Name | ForEach-Object { '{0}  {1}' -f (Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant(), $_.Name })
    [IO.File]::WriteAllText((Join-Path $Directory 'SHA-256SUMS'), ($lines -join "`n") + "`n", $utf8)
}
function New-Package([string]$Name, [string]$Mode = 'preview', [bool]$Signed = $false, [bool]$EmptySbom = $false, [string]$Import = 'KERNEL32.dll', [uint16]$DllCharacteristics = 0xC160, [bool]$CetCompat = $true) {
    $payload = Join-Path $scratch "$Name-payload"
    $output = Join-Path $scratch $Name
    [IO.Directory]::CreateDirectory($payload) | Out-Null
    foreach ($component in @('editor', 'update-helper')) {
        # Minimal PE32+: headers, then one section at RVA 0x1000 / file 0x200
        # holding the import table, the imported DLL name and a debug directory.
        $bytes = [byte[]]::new(1024)
        $bytes[0] = 0x4d; $bytes[1] = 0x5a
        [BitConverter]::GetBytes([int]128).CopyTo($bytes, 0x3c)
        [BitConverter]::GetBytes([uint32]0x4550).CopyTo($bytes, 128)
        [BitConverter]::GetBytes([uint16]0x8664).CopyTo($bytes, 132)
        [BitConverter]::GetBytes([uint16]1).CopyTo($bytes, 134)
        [BitConverter]::GetBytes([uint16]240).CopyTo($bytes, 148)
        [BitConverter]::GetBytes([uint16]0x20b).CopyTo($bytes, 152)
        [BitConverter]::GetBytes($DllCharacteristics).CopyTo($bytes, 152 + 70)
        [BitConverter]::GetBytes([uint32]16).CopyTo($bytes, 152 + 108)
        [BitConverter]::GetBytes([uint32]0x1000).CopyTo($bytes, 152 + 112 + 8)
        [BitConverter]::GetBytes([uint32]40).CopyTo($bytes, 152 + 112 + 12)
        [BitConverter]::GetBytes([uint32]0x1040).CopyTo($bytes, 152 + 112 + 48)
        [BitConverter]::GetBytes([uint32]28).CopyTo($bytes, 152 + 112 + 52)
        if ($Signed) { [BitConverter]::GetBytes([uint32]512).CopyTo($bytes, 296) }
        $section = 152 + 240
        [Text.Encoding]::ASCII.GetBytes('.rdata').CopyTo($bytes, $section)
        foreach ($pair in @(@(8, 0x200), @(12, 0x1000), @(16, 0x200), @(20, 0x200))) { [BitConverter]::GetBytes([uint32]$pair[1]).CopyTo($bytes, $section + $pair[0]) }
        [BitConverter]::GetBytes([uint32]0x1080).CopyTo($bytes, 0x200)
        [BitConverter]::GetBytes([uint32]0x1100).CopyTo($bytes, 0x200 + 12)
        [BitConverter]::GetBytes([uint32]0x1080).CopyTo($bytes, 0x200 + 16)
        [Text.Encoding]::ASCII.GetBytes($Import).CopyTo($bytes, 0x300)
        [BitConverter]::GetBytes([uint32]20).CopyTo($bytes, 0x240 + 12)
        [BitConverter]::GetBytes([uint32]4).CopyTo($bytes, 0x240 + 16)
        [BitConverter]::GetBytes([uint32]0x1180).CopyTo($bytes, 0x240 + 20)
        [BitConverter]::GetBytes([uint32]0x380).CopyTo($bytes, 0x240 + 24)
        if ($CetCompat) { [BitConverter]::GetBytes([uint32]1).CopyTo($bytes, 0x380) }
        $marker = "BARELINE-CAPABILITY|component=$component|mode=$Mode|config-version=none|config=none|source=unrecorded|version=$version|features=updates=disabled,extensions=disabled,runtime=external"
        $name = if ($component -eq 'editor') { 'bareline.exe' } else { 'bareline-update-helper.exe' }
        [IO.File]::WriteAllBytes((Join-Path $payload $name), $bytes + [Text.Encoding]::ASCII.GetBytes($marker + $commit))
    }
    [IO.File]::WriteAllText((Join-Path $payload 'LICENSE'), 'Synthetic license fixture', $utf8)
    [IO.File]::WriteAllText((Join-Path $payload 'THIRD-PARTY-NOTICES.md'), "Packaged Cargo roots: bareline, bareline-update-helper.`n# SDK and first-party licenses`nUNICODE LICENSE V3", $utf8)
    $components = if ($EmptySbom) { @() } else { @('bareline', 'bareline-update-helper', 'lexilla', 'scintilla') | ForEach-Object { @{name = $_} } }
    $sbom = @{bomFormat = 'CycloneDX'; specVersion = '1.3'; components = @($components); dependencies = @(@{ref = 'bareline'; dependsOn = @('lexilla')})}
    [IO.File]::WriteAllText((Join-Path $payload 'SBOM.json'), ($sbom | ConvertTo-Json -Depth 5), $utf8)
    & (Join-Path $PSScriptRoot 'build.ps1') -PayloadDir $payload -Version $version -OutputDir $output | Out-Host
    foreach ($name in @('LICENSE', 'THIRD-PARTY-NOTICES.md', 'SBOM.json')) { [IO.File]::Copy((Join-Path $payload $name), (Join-Path $output $name)) }
    [IO.File]::WriteAllText((Join-Path $output 'PREVIEW-NOTES.md'), "Bareline $version unsigned preview", $utf8)
    Write-Inventory $output
    return $output
}
function Expect-Rejection([scriptblock]$Action, [string]$MessagePattern) {
    try { & $Action; throw 'Expected verification rejection was not raised.' } catch {
        if ($_.Exception.Message -notlike $MessagePattern) { throw }
    }
}
try {
    foreach ($tag in @('v0.1.0-preview.1', 'v0.1.0-preview.20260928.1')) {
        & (Join-Path $PSScriptRoot 'check-preview-tag.ps1') -Version $version -Tag $tag
    }
    foreach ($tag in @('v0.1.0', 'v0.2.0-preview.1', 'v0.1.0-preview.0', 'v0.1.0-preview.01', 'v0.1.0-preview.1.01', 'v0.1.0-preview.1-extra')) {
        Expect-Rejection { & (Join-Path $PSScriptRoot 'check-preview-tag.ps1') -Version $version -Tag $tag } 'Expected v0.1.0-preview*'
    }
    $valid = New-Package 'valid'
    & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version
    & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version -BuildHash $commit
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version -BuildHash 'fedcba9876543210fedcba9876543210fedcba98' } 'Editor does not record the expected build commit*'
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version -RequireInstaller } 'Unexpected or missing preview artifact*'
    $configured = New-Package 'configured' 'configured'
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $configured -Version $version } 'Executable is not the expected core preview*'
    $signed = New-Package 'signed' 'preview' $true
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $signed -Version $version } 'Signed executable supplied*'
    $empty = New-Package 'empty-sbom' 'preview' $false $true
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $empty -Version $version } 'Expected a nonempty CycloneDX*'
    foreach ($runtime in @('VCRUNTIME140.dll', 'vcruntime140_1.dll', 'MSVCP140.dll')) {
        $dynamic = New-Package "dynamic-crt-$runtime" -Import $runtime
        Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $dynamic -Version $version } "Executable links the Visual C++ runtime DLL $runtime*"
    }
    $unguarded = New-Package 'no-cfg' -DllCharacteristics 0x8160
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $unguarded -Version $version } 'Executable lacks Control Flow Guard*'
    $checksum = Join-Path $valid 'SHA-256SUMS'
    $inventory = [IO.File]::ReadAllText($checksum)
    [IO.File]::AppendAllText($checksum, ([IO.File]::ReadAllLines($checksum)[0] + "`n"), $utf8)
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version } 'Unexpected or duplicate preview checksum*'
    [IO.File]::WriteAllText($checksum, $inventory, $utf8)
    [IO.File]::AppendAllText((Join-Path $valid 'LICENSE'), 'tampered', $utf8)
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version } 'Preview checksum mismatch*'
    Write-Inventory $valid
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version } 'Packaged document differs*'
    Write-Output 'PASS: preview tag/version, valid package, missing installer, configured/signed binary rejection, build commit, VC++ runtime imports, missing CFG, empty SBOM, duplicate checksums, corruption and document mismatch.'
} finally {
    $resolved = [IO.Path]::GetFullPath($scratch)
    $tempPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase) -or -not [IO.Path]::GetFileName($resolved).StartsWith('bareline-preview-contract-')) { throw 'Unsafe test cleanup path.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
