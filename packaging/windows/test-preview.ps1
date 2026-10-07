#requires -Version 7.0
# SPDX-License-Identifier: MPL-2.0
# Synthetic packages exercise verification failures; no application is launched.
$ErrorActionPreference = 'Stop'
$scratch = Join-Path ([IO.Path]::GetTempPath()) ('bareline-preview-contract-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($scratch) | Out-Null
$version = '0.2.0'
$commit = '0123456789abcdef0123456789abcdef01234567'
$utf8 = [Text.UTF8Encoding]::new($false)
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
function Write-Inventory([string]$Directory) {
    $lines = @(Get-ChildItem -LiteralPath $Directory -File | Where-Object Name -ne 'SHA-256SUMS' | Sort-Object Name | ForEach-Object { '{0}  {1}' -f (Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant(), $_.Name })
    [IO.File]::WriteAllText((Join-Path $Directory 'SHA-256SUMS'), ($lines -join "`n") + "`n", $utf8)
}
function New-Package([string]$Name, [string]$Mode = 'preview', [bool]$Signed = $false, [bool]$EmptySbom = $false, [string]$Import = 'KERNEL32.dll', [uint16]$DllCharacteristics = 0xC160, [bool]$CetCompat = $true, [uint16]$DependentLoadFlags = 0x800) {
    $payload = Join-Path $scratch "$Name-payload"
    $output = Join-Path $scratch $Name
    [IO.Directory]::CreateDirectory($payload) | Out-Null
    foreach ($component in @('editor', 'update-helper')) {
        # Minimal PE32+: headers, then one section at RVA 0x1000 / file 0x200
        # holding the import table, the imported DLL name, a debug directory and
        # the load configuration directory.
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
        [BitConverter]::GetBytes([uint32]0x11a0).CopyTo($bytes, 152 + 112 + 80)
        [BitConverter]::GetBytes([uint32]0x50).CopyTo($bytes, 152 + 112 + 84)
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
        [BitConverter]::GetBytes([uint32]0x50).CopyTo($bytes, 0x3a0)
        [BitConverter]::GetBytes($DependentLoadFlags).CopyTo($bytes, 0x3a0 + 0x4e)
        $marker = "BARELINE-CAPABILITY|component=$component|mode=$Mode|config-version=none|config=none|source=unrecorded|version=$version|features=updates=disabled,extensions=disabled,runtime=external"
        $name = if ($component -eq 'editor') { 'bareline.exe' } else { 'bareline-update-helper.exe' }
        [IO.File]::WriteAllBytes((Join-Path $payload $name), $bytes + [Text.Encoding]::ASCII.GetBytes($marker + $commit))
    }
    [IO.File]::WriteAllText((Join-Path $payload 'LICENSE'), 'Synthetic license fixture', $utf8)
    [IO.File]::WriteAllText((Join-Path $payload 'THIRD-PARTY-NOTICES.md'), "Packaged Cargo roots: bareline, bareline-update-helper.`n# SDK and first-party licenses`nUNICODE LICENSE V3`n## Native pcre2 10.46`n## Native sljit e51eabbf", $utf8)
    $components = if ($EmptySbom) { @() } else { @('bareline', 'bareline-update-helper', 'lexilla', 'scintilla', 'pcre2', 'sljit') | ForEach-Object { @{name = $_} } }
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
# Synthetic Linux and macOS downloads in the formats packaging/linux/build-preview.sh
# and packaging/macos/bundle.sh write: a gzip-compressed POSIX ustar archive and a
# disk image that ends in a UDIF trailer.
function New-UstarEntry([string]$Name, [byte[]]$Data, [string]$Mode = '644', [string]$Type = '0') {
    return [pscustomobject]@{ Name = $Name; Data = $Data; Mode = $Mode; Type = $Type; ModificationTime = 0 }
}
function Write-UstarGzip([string]$Path, [object[]]$Entries) {
    $ascii = [Text.Encoding]::ASCII
    $tar = [IO.MemoryStream]::new()
    foreach ($entry in $Entries) {
        $header = [byte[]]::new(512)
        $ascii.GetBytes($entry.Name).CopyTo($header, 0)
        $ascii.GetBytes($entry.Mode.PadLeft(7, '0') + [char]0).CopyTo($header, 100)
        $ascii.GetBytes('0000000' + [char]0).CopyTo($header, 108)
        $ascii.GetBytes('0000000' + [char]0).CopyTo($header, 116)
        $ascii.GetBytes([Convert]::ToString([long]$entry.Data.Length, 8).PadLeft(11, '0') + [char]0).CopyTo($header, 124)
        $ascii.GetBytes([Convert]::ToString([long]$entry.ModificationTime, 8).PadLeft(11, '0') + [char]0).CopyTo($header, 136)
        $header[156] = $ascii.GetBytes($entry.Type)[0]
        $ascii.GetBytes('ustar' + [char]0 + '00').CopyTo($header, 257)
        for ($i = 148; $i -lt 156; $i++) { $header[$i] = 32 }
        $sum = 0
        foreach ($byte in $header) { $sum += $byte }
        $ascii.GetBytes([Convert]::ToString($sum, 8).PadLeft(6, '0') + [char]0 + ' ').CopyTo($header, 148)
        $tar.Write($header, 0, 512)
        $tar.Write($entry.Data, 0, $entry.Data.Length)
        $padding = (512 - $entry.Data.Length % 512) % 512
        $tar.Write([byte[]]::new($padding), 0, $padding)
    }
    # Two zero blocks end the archive; GNU tar pads it to a 10240-byte record.
    $length = [long]([Math]::Ceiling(($tar.Length + 1024) / 10240.0) * 10240)
    $tar.Write([byte[]]::new($length - $tar.Length), 0, [int]($length - $tar.Length))
    $file = [IO.File]::Create($Path)
    try {
        $gzip = [IO.Compression.GZipStream]::new($file, [IO.Compression.CompressionMode]::Compress)
        try { $gzip.Write($tar.ToArray(), 0, [int]$tar.Length) } finally { $gzip.Dispose() }
    } finally { $file.Dispose(); $tar.Dispose() }
}
function New-SyntheticElf([string]$Component, [string]$Mode = 'preview', [uint16]$Machine = 62) {
    $bytes = [byte[]]::new(64)
    $bytes[0] = 0x7f; $bytes[1] = 0x45; $bytes[2] = 0x4c; $bytes[3] = 0x46; $bytes[4] = 2; $bytes[5] = 1; $bytes[6] = 1
    [BitConverter]::GetBytes([uint16]3).CopyTo($bytes, 16)
    [BitConverter]::GetBytes($Machine).CopyTo($bytes, 18)
    $marker = "BARELINE-CAPABILITY|component=$Component|mode=$Mode|config-version=none|config=none|source=unrecorded|version=$version|features=updates=disabled,extensions=disabled,runtime=external"
    return , [byte[]]($bytes + [Text.Encoding]::ASCII.GetBytes($marker + $commit))
}
# The Linux tarball as build-preview.sh packs it; $Change edits the entries first.
function New-LinuxTarball([string]$Directory, [scriptblock]$Change) {
    $folder = "bareline-$version-linux-x64"
    $notices = "Packaged Cargo roots: bareline, bareline-extension-host.`nGenerated from Cargo.lock and registry package license files for Linux x64.`n# SDK and first-party licenses`nUNICODE LICENSE V3`n## Native pcre2 10.46`n## Native sljit e51eabbf`n## Bundled DejaVu Sans Mono 2.37 font"
    $components = @('bareline', 'bareline-extension-host', 'bareline-platform-linux', 'lexilla', 'scintilla', 'pcre2', 'sljit') | ForEach-Object { @{ name = $_ } }
    $sbom = @{ bomFormat = 'CycloneDX'; specVersion = '1.3'; components = @($components); dependencies = @(@{ ref = 'bareline'; dependsOn = @('lexilla') }) } | ConvertTo-Json -Depth 5
    $entries = [Collections.Generic.List[object]]::new()
    $entries.Add((New-UstarEntry "$folder/" ([byte[]]::new(0)) '755' '5'))
    $entries.Add((New-UstarEntry "$folder/LICENSE" ([IO.File]::ReadAllBytes((Join-Path $repositoryRoot 'LICENSE')))))
    $entries.Add((New-UstarEntry "$folder/SBOM.json" ([Text.Encoding]::UTF8.GetBytes($sbom))))
    $entries.Add((New-UstarEntry "$folder/THIRD-PARTY-NOTICES.md" ([Text.Encoding]::UTF8.GetBytes($notices))))
    $entries.Add((New-UstarEntry "$folder/bareline" (New-SyntheticElf 'editor') '755'))
    $entries.Add((New-UstarEntry "$folder/bareline-extension-host" (New-SyntheticElf 'extension-runtime') '755'))
    $entries.Add((New-UstarEntry "$folder/bareline.desktop" ([IO.File]::ReadAllBytes((Join-Path $repositoryRoot 'packaging/linux/bareline.desktop')))))
    $entries.Add((New-UstarEntry "$folder/bareline.png" ([IO.File]::ReadAllBytes((Join-Path $repositoryRoot 'packaging/linux/bareline.png')))))
    $entries.Add((New-UstarEntry "$folder/bareline.portable" ([byte[]]::new(0))))
    if ($Change) { & $Change $entries $folder }
    [IO.Directory]::CreateDirectory($Directory) | Out-Null
    Write-UstarGzip (Join-Path $Directory "bareline-$version-linux-x64.tar.gz") $entries.ToArray()
}
function New-SyntheticDiskImage([string]$Path, [int]$Length = 1049088) {
    $bytes = [byte[]]::new($Length)
    $trailer = $Length - 512
    [Text.Encoding]::ASCII.GetBytes('koly').CopyTo($bytes, $trailer)
    $bytes[$trailer + 7] = 4
    $bytes[$trailer + 10] = 2
    [IO.File]::WriteAllBytes($Path, $bytes)
}
function Find-Entry($Entries, [string]$Name) {
    return @($Entries | Where-Object { $_.Name -ceq $Name })[0]
}
try {
    foreach ($tag in @('v0.2.0-preview.1', 'v0.2.0-preview.20261005.1')) {
        & (Join-Path $PSScriptRoot 'check-preview-tag.ps1') -Version $version -Tag $tag
    }
    foreach ($tag in @('v0.2.0', 'v0.1.0-preview.1', 'v0.3.0-preview.1', 'v0.2.0-preview.0', 'v0.2.0-preview.01', 'v0.2.0-preview.1.01', 'v0.2.0-preview.1-extra')) {
        Expect-Rejection { & (Join-Path $PSScriptRoot 'check-preview-tag.ps1') -Version $version -Tag $tag } 'Expected v0.2.0-preview*'
    }
    $valid = New-Package 'valid'
    & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version -Platform windows
    & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version -Platform windows -BuildHash $commit
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version -Platform windows -BuildHash 'fedcba9876543210fedcba9876543210fedcba98' } 'Editor does not record the expected build commit*'
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version -Platform windows -RequireInstaller } 'Unexpected or missing preview artifact*'
    $configured = New-Package 'configured' 'configured'
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $configured -Version $version -Platform windows } 'Executable is not the expected core preview*'
    $signed = New-Package 'signed' 'preview' $true
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $signed -Version $version -Platform windows } 'Signed executable supplied*'
    $empty = New-Package 'empty-sbom' 'preview' $false $true
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $empty -Version $version -Platform windows } 'Expected a nonempty CycloneDX*'
    foreach ($runtime in @('VCRUNTIME140.dll', 'vcruntime140_1.dll', 'MSVCP140.dll')) {
        $dynamic = New-Package "dynamic-crt-$runtime" -Import $runtime
        Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $dynamic -Version $version -Platform windows } "Executable links the Visual C++ runtime DLL $runtime*"
    }
    $unguarded = New-Package 'no-cfg' -DllCharacteristics 0x8160
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $unguarded -Version $version -Platform windows } 'Executable lacks Control Flow Guard*'
    foreach ($flags in @(0, 0xa00)) {
        $searchable = New-Package "dependent-load-$flags" -DependentLoadFlags $flags
        Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $searchable -Version $version -Platform windows } 'Executable does not restrict dependent DLL loads to System32*'
    }
    $checksum = Join-Path $valid 'SHA-256SUMS'
    $inventory = [IO.File]::ReadAllText($checksum)
    [IO.File]::AppendAllText($checksum, ([IO.File]::ReadAllLines($checksum)[0] + "`n"), $utf8)
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version -Platform windows } 'Unexpected or duplicate preview checksum*'
    [IO.File]::WriteAllText($checksum, $inventory, $utf8)
    [IO.File]::AppendAllText((Join-Path $valid 'LICENSE'), 'tampered', $utf8)
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version -Platform windows } 'Preview checksum mismatch*'
    Write-Inventory $valid
    Expect-Rejection { & (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $valid -Version $version -Platform windows } 'Packaged document differs*'
    # The complete release: the Windows downloads with the Linux tarball and the
    # macOS disk image beside them under one inventory, as assemble-preview.ps1
    # writes it. Missing platform downloads fail unless -Platform names a subset.
    $verify = Join-Path $PSScriptRoot 'verify-preview.ps1'
    $full = New-Package 'full'
    New-LinuxTarball $full
    $dmg = Join-Path $full "bareline-$version-macos-arm64.dmg"
    New-SyntheticDiskImage $dmg
    [IO.File]::AppendAllText((Join-Path $full 'PREVIEW-NOTES.md'), "`nbareline-$version-linux-x64.tar.gz`nbareline-$version-macos-arm64.dmg", $utf8)
    Write-Inventory $full
    $dmgHash = (Get-FileHash -LiteralPath $dmg -Algorithm SHA256).Hash.ToLowerInvariant()
    & $verify -ArtifactDir $full -Version $version -BuildHash $commit -DmgSha256 $dmgHash
    Expect-Rejection { & $verify -ArtifactDir $full -Version $version -DmgSha256 ('0' * 64) } 'macOS disk image differs*'
    Expect-Rejection { & $verify -ArtifactDir $full -Version $version -Platform windows } 'Unexpected or missing preview artifact*'
    Expect-Rejection { & $verify -ArtifactDir $full -Version $version -Platform 'windows,beos' } 'Expected -Platform*'
    Expect-Rejection { & $verify -ArtifactDir $full -Version $version -Platform linux -RequireInstaller } '-RequireInstaller applies*'
    Remove-Item -LiteralPath $dmg
    Write-Inventory $full
    Expect-Rejection { & $verify -ArtifactDir $full -Version $version } 'Unexpected or missing preview artifact*'
    & $verify -ArtifactDir $full -Version $version -Platform 'windows,linux' -BuildHash $commit
    $unnamed = New-Package 'notes-without-ports'
    New-LinuxTarball $unnamed
    Write-Inventory $unnamed
    Expect-Rejection { & $verify -ArtifactDir $unnamed -Version $version -Platform windows, linux } 'Preview release notes must describe*'
    # One platform's downloads on their own, and each Linux layout rule.
    $linux = Join-Path $scratch 'linux-only'
    New-LinuxTarball $linux
    Write-Inventory $linux
    & $verify -ArtifactDir $linux -Version $version -Platform linux -BuildHash $commit
    Expect-Rejection { & $verify -ArtifactDir $linux -Version $version -Platform linux -BuildHash 'fedcba9876543210fedcba9876543210fedcba98' } 'Linux editor does not record the expected build commit*'
    $tarballCases = @(
        @{ Name = 'extra-entry'; Message = 'Linux tarball inventory is invalid*'; Change = { param($entries, $folder) $entries.Add((New-UstarEntry "$folder/README" ([byte[]]@(1)))) } },
        @{ Name = 'second-folder'; Message = 'Linux tarball inventory is invalid*'; Change = { param($entries, $folder) (Find-Entry $entries "$folder/LICENSE").Name = 'LICENSE' } },
        @{ Name = 'timestamp'; Message = 'Linux tarball metadata is not normalized*'; Change = { param($entries, $folder) (Find-Entry $entries "$folder/LICENSE").ModificationTime = 1 } },
        @{ Name = 'mode'; Message = 'Linux tarball entry has the wrong mode*'; Change = { param($entries, $folder) (Find-Entry $entries "$folder/bareline-extension-host").Mode = '644' } },
        @{ Name = 'symlink'; Message = 'Linux tarball entry has the wrong type*'; Change = { param($entries, $folder) (Find-Entry $entries "$folder/bareline.png").Type = '2' } },
        @{ Name = 'arm64'; Message = 'Expected an x86-64 ELF executable*'; Change = { param($entries, $folder) (Find-Entry $entries "$folder/bareline").Data = New-SyntheticElf 'editor' -Machine 183 } },
        @{ Name = 'configured'; Message = 'Linux executable is not the expected core preview*'; Change = { param($entries, $folder) (Find-Entry $entries "$folder/bareline-extension-host").Data = New-SyntheticElf 'extension-runtime' 'configured' } },
        @{ Name = 'marker'; Message = 'Linux portable marker must be empty*'; Change = { param($entries, $folder) (Find-Entry $entries "$folder/bareline.portable").Data = [byte[]]@(1) } },
        @{ Name = 'desktop'; Message = 'Linux tarball bareline.desktop differs*'; Change = { param($entries, $folder) (Find-Entry $entries "$folder/bareline.desktop").Data = [Text.Encoding]::ASCII.GetBytes("[Desktop Entry]`nExec=sh`n") } },
        @{ Name = 'notices'; Message = 'Missing Linux license evidence*'; Change = { param($entries, $folder) (Find-Entry $entries "$folder/THIRD-PARTY-NOTICES.md").Data = [Text.Encoding]::ASCII.GetBytes('Packaged Cargo roots: bareline.') } }
    )
    foreach ($case in $tarballCases) {
        $directory = Join-Path $scratch "linux-$($case.Name)"
        New-LinuxTarball $directory $case.Change
        Write-Inventory $directory
        Expect-Rejection { & $verify -ArtifactDir $directory -Version $version -Platform linux } $case.Message
    }
    $macos = Join-Path $scratch 'macos-only'
    [IO.Directory]::CreateDirectory($macos) | Out-Null
    New-SyntheticDiskImage (Join-Path $macos "bareline-$version-macos-arm64.dmg")
    Write-Inventory $macos
    & $verify -ArtifactDir $macos -Version $version -Platform macos
    [IO.File]::WriteAllBytes((Join-Path $macos "bareline-$version-macos-arm64.dmg"), [byte[]]::new(1049088))
    Write-Inventory $macos
    Expect-Rejection { & $verify -ArtifactDir $macos -Version $version -Platform macos } 'Expected a UDIF disk image*'
    New-SyntheticDiskImage (Join-Path $macos "bareline-$version-macos-arm64.dmg") 4096
    Write-Inventory $macos
    Expect-Rejection { & $verify -ArtifactDir $macos -Version $version -Platform macos } 'macOS disk image is implausibly small*'
    # The installer smoke waits for this event while the editor holds its log open.
    . (Join-Path $PSScriptRoot 'launch-evidence.ps1')
    $launchLog = Join-Path $scratch 'bareline.log'
    if (Read-FirstFrameEvent $launchLog $version) { throw 'A missing diagnostics log reported a first frame.' }
    $foreign = @(
        '{"event":"startup_action","action":"ReadSettings","microseconds":5}',
        '{"event":"first_frame","version":"9.9.9","build_hash":"unknown","microseconds":7,"software":true}',
        '{"event":"first_frame","version":"0.2.0","build_hash":"unknown","microseconds":"7","software":true}',
        '{"event":"first_frame","version":"0.2.0","build_hash":"unknown","microseconds":7,"software":"no"}',
        '{"event":"first_frame","vers')
    [IO.File]::WriteAllText($launchLog, ($foreign -join "`n"), $utf8)
    if (Read-FirstFrameEvent $launchLog $version) { throw 'A foreign-version, malformed or partial first_frame event was accepted.' }
    $held = [IO.FileStream]::new($launchLog, [IO.FileMode]::Append, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite)
    try {
        $bytes = $utf8.GetBytes("`n" + '{"event":"first_frame","version":"0.2.0","build_hash":"' + $commit + '","microseconds":650764,"software":false}' + "`n")
        $held.Write($bytes, 0, $bytes.Length)
        $held.Flush()
        $frame = Read-FirstFrameEvent $launchLog $version
        if (-not $frame -or $frame.microseconds -ne 650764 -or $frame.software) { throw 'A first_frame event appended by a running editor was not read.' }
    } finally {
        $held.Dispose()
    }
    Write-Output 'PASS: preview tag/version, valid package, missing installer, configured/signed binary rejection, build commit, VC++ runtime imports, missing CFG, application-directory dependent loads, empty SBOM, duplicate checksums, corruption, document mismatch, the complete Windows/Linux/macOS set and platform subsets, Linux tarball layout, metadata, ELF and license rules, macOS disk image identity, and first_frame launch evidence.'
} finally {
    $resolved = [IO.Path]::GetFullPath($scratch)
    $tempPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase) -or -not [IO.Path]::GetFileName($resolved).StartsWith('bareline-preview-contract-')) { throw 'Unsafe test cleanup path.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
