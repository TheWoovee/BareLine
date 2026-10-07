# SPDX-License-Identifier: MPL-2.0
# Dot-source only. verify-preview.ps1's checks of the Linux tarball and the macOS
# disk image. They use only APIs that Windows PowerShell 5.1 also has, so they
# can be exercised on a machine without PowerShell 7.

function Get-PortSha256([byte[]]$Bytes) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($sha.ComputeHash($Bytes)).Replace('-', '').ToLowerInvariant() } finally { $sha.Dispose() }
}

function Get-PortFileSha256([string]$Path) {
    $sha = [Security.Cryptography.SHA256]::Create()
    $stream = [IO.File]::OpenRead($Path)
    try { return [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '').ToLowerInvariant() } finally { $stream.Dispose(); $sha.Dispose() }
}

# Text compared across checkouts: a Windows checkout may have CRLF line endings.
function ConvertTo-PortLf([byte[]]$Bytes) {
    return [Text.Encoding]::UTF8.GetString($Bytes).Replace("`r`n", "`n")
}

function Read-PortOctal([byte[]]$Bytes, [int]$Offset, [int]$Length, [string]$Field) {
    $text = [Text.Encoding]::ASCII.GetString($Bytes, $Offset, $Length).Trim([char]0, ' ')
    if ($text -notmatch '^[0-7]{1,12}$') { throw "Linux tarball header has an invalid $Field field." }
    return [Convert]::ToInt64($text, 8)
}

function Read-PortString([byte[]]$Bytes, [int]$Offset, [int]$Length) {
    $end = [Array]::IndexOf($Bytes, [byte]0, $Offset, $Length)
    if ($end -lt 0) { $end = $Offset + $Length }
    return [Text.Encoding]::UTF8.GetString($Bytes, $Offset, $end - $Offset)
}

# A POSIX ustar archive as GNU tar --format=ustar writes it: 512-byte headers
# with valid checksums, data padded to 512 bytes, then zero blocks to the end.
function Read-UstarArchive([byte[]]$Bytes) {
    $entries = [Collections.Generic.List[object]]::new()
    $offset = 0
    while ($true) {
        if ($offset + 512 -gt $Bytes.Length) { throw 'Linux tarball is truncated.' }
        $sum = 0
        $zero = $true
        for ($i = 0; $i -lt 512; $i++) {
            $byte = $Bytes[$offset + $i]
            if ($byte -ne 0) { $zero = $false }
            if ($i -ge 148 -and $i -lt 156) { $sum += 32 } else { $sum += $byte }
        }
        if ($zero) { break }
        if ((Read-PortOctal $Bytes ($offset + 148) 8 'checksum') -ne $sum) { throw 'Linux tarball header checksum mismatch.' }
        if ([Text.Encoding]::ASCII.GetString($Bytes, $offset + 257, 8) -cne ('ustar' + [char]0 + '00')) { throw 'Linux tarball must use the POSIX ustar format.' }
        $name = Read-PortString $Bytes $offset 100
        $prefix = Read-PortString $Bytes ($offset + 345) 155
        if ($prefix) { $name = "$prefix/$name" }
        $size = Read-PortOctal $Bytes ($offset + 124) 12 'size'
        $start = $offset + 512
        if ($start + $size -gt $Bytes.Length) { throw 'Linux tarball is truncated.' }
        $data = [byte[]]::new($size)
        [Array]::Copy($Bytes, $start, $data, 0, $size)
        $entries.Add([pscustomobject]@{
            Name = $name
            Type = [char]$Bytes[$offset + 156]
            Mode = Read-PortOctal $Bytes ($offset + 100) 8 'mode'
            Uid = Read-PortOctal $Bytes ($offset + 108) 8 'uid'
            Gid = Read-PortOctal $Bytes ($offset + 116) 8 'gid'
            ModificationTime = Read-PortOctal $Bytes ($offset + 136) 12 'mtime'
            LinkName = Read-PortString $Bytes ($offset + 157) 100
            UserName = Read-PortString $Bytes ($offset + 265) 32
            GroupName = Read-PortString $Bytes ($offset + 297) 32
            Data = $data
        })
        $offset = $start + [int]([Math]::Ceiling($size / 512.0) * 512)
    }
    # The end-of-archive marker and the record padding after it are all zeros.
    for ($i = $offset; $i -lt $Bytes.Length; $i++) {
        if ($Bytes[$i] -ne 0) { throw 'Linux tarball has data after its end-of-archive marker.' }
    }
    if ($Bytes.Length - $offset -lt 1024) { throw 'Linux tarball lacks its end-of-archive marker.' }
    return , $entries.ToArray()
}

function Assert-PortElfExecutable([byte[]]$Bytes, [string]$Name, [string]$Component, [string]$Version, [string]$BuildHash) {
    if ($Bytes.Length -lt 64 -or $Bytes[0] -ne 0x7f -or $Bytes[1] -ne 0x45 -or $Bytes[2] -ne 0x4c -or $Bytes[3] -ne 0x46 -or $Bytes[4] -ne 2 -or $Bytes[5] -ne 1) {
        throw "Expected an x86-64 ELF executable: $Name"
    }
    # e_type: executable or position-independent executable; e_machine: x86-64.
    $type = [BitConverter]::ToUInt16($Bytes, 16)
    if (($type -ne 2 -and $type -ne 3) -or [BitConverter]::ToUInt16($Bytes, 18) -ne 62) { throw "Expected an x86-64 ELF executable: $Name" }
    $marker = "BARELINE-CAPABILITY|component=$Component|mode=preview|config-version=none|config=none|source=unrecorded|version=$Version|features=updates=disabled,extensions=disabled,runtime=external"
    $text = [Text.Encoding]::ASCII.GetString($Bytes)
    if (-not $text.Contains($marker) -or $text.Contains('|mode=configured|') -or $text.Contains('|mode=fixture|')) { throw "Linux executable is not the expected core preview: $Name" }
    if ($BuildHash -and $Component -eq 'editor' -and -not $text.Contains($BuildHash)) { throw 'Linux editor does not record the expected build commit.' }
}

# bareline-<version>-linux-x64.tar.gz: one top-level folder with exactly the
# packaged files, normalized metadata (gzip -n, zero times, owner 0, fixed
# modes), x86-64 ELF preview executables and the repository's desktop entry,
# icon and license.
function Assert-LinuxPreviewTarball {
    param([string]$Path, [string]$Version, [string]$BuildHash, [string]$RepositoryRoot)
    $compressed = [IO.File]::ReadAllBytes($Path)
    # gzip -n: deflate, no stored name or comment, zero modification time.
    if ($compressed.Length -lt 18 -or $compressed[0] -ne 0x1f -or $compressed[1] -ne 0x8b -or $compressed[2] -ne 8) { throw 'Linux tarball is not gzip-compressed.' }
    if (($compressed[3] -band 0x18) -or [BitConverter]::ToUInt32($compressed, 4) -ne 0) { throw 'Linux tarball gzip header is not normalized (use gzip -n).' }
    $source = [IO.MemoryStream]::new($compressed)
    $gzip = [IO.Compression.GZipStream]::new($source, [IO.Compression.CompressionMode]::Decompress)
    $tar = [IO.MemoryStream]::new()
    try { $gzip.CopyTo($tar) } finally { $gzip.Dispose(); $source.Dispose() }
    $entries = Read-UstarArchive $tar.ToArray()
    $tar.Dispose()

    $folder = "bareline-$Version-linux-x64"
    $executable = [Convert]::ToInt64('755', 8)
    $regular = [Convert]::ToInt64('644', 8)
    $expected = [ordered]@{
        "$folder/" = $executable
        "$folder/bareline" = $executable
        "$folder/bareline-extension-host" = $executable
        "$folder/bareline.portable" = $regular
        "$folder/LICENSE" = $regular
        "$folder/THIRD-PARTY-NOTICES.md" = $regular
        "$folder/SBOM.json" = $regular
        "$folder/bareline.desktop" = $regular
        "$folder/bareline.png" = $regular
    }
    $names = @($entries | ForEach-Object { $_.Name })
    if ($names.Count -ne $expected.Count -or @($names | Sort-Object -Unique).Count -ne $names.Count -or @(Compare-Object @($expected.Keys) $names).Count) {
        throw "Linux tarball inventory is invalid: $($names -join ', ')"
    }
    $contents = @{}
    foreach ($entry in $entries) {
        $type = if ($entry.Name -eq "$folder/") { '5' } else { '0' }
        if ([string]$entry.Type -ne $type -or $entry.LinkName) { throw "Linux tarball entry has the wrong type: $($entry.Name)" }
        if ($entry.Mode -ne $expected[$entry.Name]) { throw "Linux tarball entry has the wrong mode: $($entry.Name)" }
        if ($entry.Uid -ne 0 -or $entry.Gid -ne 0 -or $entry.UserName -or $entry.GroupName -or $entry.ModificationTime -ne 0) { throw "Linux tarball metadata is not normalized: $($entry.Name)" }
        if ($type -eq '0') { $contents[$entry.Name.Substring($folder.Length + 1)] = $entry.Data }
    }
    if ($contents['bareline.portable'].Length -ne 0) { throw 'Linux portable marker must be empty.' }
    Assert-PortElfExecutable $contents['bareline'] 'bareline' 'editor' $Version $BuildHash
    Assert-PortElfExecutable $contents['bareline-extension-host'] 'bareline-extension-host' 'extension-runtime' $Version ''
    foreach ($pair in @(@('LICENSE', 'LICENSE'), @('bareline.desktop', 'packaging/linux/bareline.desktop'))) {
        $source = [IO.File]::ReadAllBytes((Join-Path $RepositoryRoot $pair[1]))
        if ((ConvertTo-PortLf $contents[$pair[0]]) -cne (ConvertTo-PortLf $source)) { throw "Linux tarball $($pair[0]) differs from $($pair[1])." }
    }
    if ((Get-PortSha256 $contents['bareline.png']) -ne (Get-PortFileSha256 (Join-Path $RepositoryRoot 'packaging/linux/bareline.png'))) { throw 'Linux tarball bareline.png differs from packaging/linux/bareline.png.' }
    $notices = [Text.Encoding]::UTF8.GetString($contents['THIRD-PARTY-NOTICES.md'])
    foreach ($text in @('Packaged Cargo roots: bareline, bareline-extension-host.', 'license files for Linux x64.', '# SDK and first-party licenses', 'UNICODE LICENSE V3', '## Native pcre2 ', '## Native sljit ', '## Bundled DejaVu Sans Mono')) {
        if (-not $notices.Contains($text)) { throw "Missing Linux license evidence: $text" }
    }
    $sbom = [Text.Encoding]::UTF8.GetString($contents['SBOM.json']) | ConvertFrom-Json
    if ($sbom.bomFormat -ne 'CycloneDX' -or -not $sbom.specVersion -or -not @($sbom.components).Count -or -not @($sbom.dependencies).Count) { throw 'Expected a nonempty CycloneDX dependency SBOM in the Linux tarball.' }
    $componentNames = @($sbom.components | ForEach-Object { $_.name })
    foreach ($name in @('bareline', 'bareline-extension-host', 'bareline-platform-linux', 'lexilla', 'scintilla', 'pcre2', 'sljit')) {
        if ($componentNames -notcontains $name) { throw "Missing packaged component in the Linux SBOM: $name" }
    }
}

# bareline-<version>-macos-arm64.dmg cannot be mounted here; the macOS job
# mounts it and checks the app. This checks it is a plausible UDIF image and,
# given the SHA-256 that job recorded, that it is the same file.
function Assert-MacosPreviewDiskImage {
    param([string]$Path, [string]$ExpectedSha256)
    $length = (Get-Item -LiteralPath $Path).Length
    if ($length -lt 1MB) { throw "macOS disk image is implausibly small: $length bytes." }
    $trailer = [byte[]]::new(512)
    $stream = [IO.File]::OpenRead($Path)
    try {
        $null = $stream.Seek(-512, [IO.SeekOrigin]::End)
        $read = 0
        while ($read -lt 512) {
            $count = $stream.Read($trailer, $read, 512 - $read)
            if ($count -le 0) { throw 'macOS disk image is truncated.' }
            $read += $count
        }
    } finally { $stream.Dispose() }
    # The UDIF trailer: 'koly', version 4, header size 512, both big-endian.
    $version = ([int]$trailer[4] -shl 24) -bor ([int]$trailer[5] -shl 16) -bor ([int]$trailer[6] -shl 8) -bor [int]$trailer[7]
    $headerSize = ([int]$trailer[8] -shl 24) -bor ([int]$trailer[9] -shl 16) -bor ([int]$trailer[10] -shl 8) -bor [int]$trailer[11]
    if ([Text.Encoding]::ASCII.GetString($trailer, 0, 4) -cne 'koly' -or $version -ne 4 -or $headerSize -ne 512) { throw 'Expected a UDIF disk image (koly trailer).' }
    if ($ExpectedSha256 -and (Get-PortFileSha256 $Path) -ne $ExpectedSha256.ToLowerInvariant()) { throw 'macOS disk image differs from the one the macOS job mounted and checked.' }
}
