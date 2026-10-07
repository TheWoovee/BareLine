#requires -Version 7.0
# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ArtifactDir,
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [switch]$RequireInstaller,
    # The commit the editor must record for About and diagnostics.
    [ValidatePattern('^([0-9a-f]{40})?$')][string]$BuildHash,
    # The platforms whose downloads the directory must hold: windows, linux and
    # macos, as an array or one comma-separated value. A published preview holds
    # all three, which is the default; the Windows build job and local checks of
    # one platform's downloads name the platforms they have.
    [string[]]$Platform = @('windows', 'linux', 'macos'),
    # SHA-256 that the macOS job recorded for the disk image it mounted and checked.
    [ValidatePattern('^([0-9a-fA-F]{64})?$')][string]$DmgSha256
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'pe-hardening.ps1')
. (Join-Path $PSScriptRoot 'port-assets.ps1')
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$platforms = @($Platform | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim().ToLowerInvariant() } | Where-Object { $_ } | Sort-Object -Unique)
if (-not $platforms.Count -or @($platforms | Where-Object { $_ -notin @('windows', 'linux', 'macos') }).Count) { throw "Expected -Platform windows, linux and/or macos; received '$($Platform -join ',')'." }
if ($RequireInstaller -and 'windows' -notin $platforms) { throw '-RequireInstaller applies to the windows platform.' }
$directory = (Resolve-Path -LiteralPath $ArtifactDir).Path
if ((Get-Item -LiteralPath $directory).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse artifact directory rejected.' }
$zipName = "bareline-$Version-windows-x64-portable.zip"
$installerName = "bareline-$Version-windows-x64-setup.exe"
$tarballName = "bareline-$Version-linux-x64.tar.gz"
$dmgName = "bareline-$Version-macos-arm64.dmg"
$documents = @('LICENSE', 'THIRD-PARTY-NOTICES.md', 'SBOM.json', 'PREVIEW-NOTES.md')
$required = @('SHA-256SUMS')
if ('windows' -in $platforms) {
    $required += @($zipName) + $documents
    if ($RequireInstaller -or (Test-Path -LiteralPath (Join-Path $directory $installerName))) { $required += $installerName }
}
if ('linux' -in $platforms) { $required += $tarballName }
if ('macos' -in $platforms) { $required += $dmgName }
$files = @(Get-ChildItem -LiteralPath $directory -Force)
$difference = @(Compare-Object ($required | Sort-Object) ($files.Name | Sort-Object))
if ($difference.Count) {
    $details = @(
        @($difference | Where-Object SideIndicator -eq '<=' | ForEach-Object { "missing $($_.InputObject)" }) +
        @($difference | Where-Object SideIndicator -eq '=>' | ForEach-Object { "unexpected $($_.InputObject)" })
    )
    throw "Unexpected or missing preview artifact for $($platforms -join ', '): $($details -join '; ')."
}
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

if ('windows' -in $platforms) {
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
    foreach ($name in @('bareline', 'bareline-update-helper', 'lexilla', 'scintilla', 'pcre2', 'sljit')) {
        if ($name -notin $sbom.components.name) { throw "Missing packaged component in SBOM: $name" }
    }
    $notices = [IO.File]::ReadAllText((Join-Path $directory 'THIRD-PARTY-NOTICES.md'))
    foreach ($text in @('Packaged Cargo roots: bareline, bareline-update-helper.', '# SDK and first-party licenses', 'UNICODE LICENSE V3', '## Native pcre2 ', '## Native sljit ')) {
        if (-not $notices.Contains($text)) { throw "Missing preview license evidence: $text" }
    }
    $notes = [IO.File]::ReadAllText((Join-Path $directory 'PREVIEW-NOTES.md'))
    if (-not $notes.Contains('unsigned preview') -or -not $notes.Contains($Version)) { throw 'Preview release notes must identify the unsigned scope and version.' }
    foreach ($name in @($tarballName, $dmgName)) {
        if ($name -in $required -and -not $notes.Contains($name)) { throw "Preview release notes must describe $name." }
    }
}
if ('linux' -in $platforms) {
    Assert-LinuxPreviewTarball -Path (Join-Path $directory $tarballName) -Version $Version -BuildHash $BuildHash -RepositoryRoot $repositoryRoot
}
if ('macos' -in $platforms) {
    Assert-MacosPreviewDiskImage -Path (Join-Path $directory $dmgName) -ExpectedSha256 $DmgSha256
}
$checked = [Collections.Generic.List[string]]::new()
$checked.Add('preview inventory and SHA-256')
if ('windows' -in $platforms) { $checked.Add('Windows portable layout, x64 preview capabilities, static CRT/CFG/CET/System32-dependent-load hardening, licenses and CycloneDX SBOM') }
if ('linux' -in $platforms) { $checked.Add('Linux tarball layout and normalized metadata, x86-64 ELF preview capabilities, desktop entry, licenses and CycloneDX SBOM') }
if ('macos' -in $platforms) { $checked.Add($(if ($DmgSha256) { 'macOS UDIF disk image identical to the one the macOS job checked' } else { 'macOS UDIF disk image (not mounted; no recorded hash given)' })) }
Write-Output ('PASS: ' + ($checked -join '; ') + '.')
