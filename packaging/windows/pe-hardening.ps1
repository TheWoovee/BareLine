# SPDX-License-Identifier: MPL-2.0
# Shared PE32+ hardening checks for shipped executables: no Visual C++ runtime
# DLL imports (static CRT), Control Flow Guard, and CET shadow-stack
# compatibility. Reads headers only; nothing is loaded or executed.
function Get-PeFileOffset([byte[]]$Bytes, [long]$SectionTable, [int]$SectionCount, [long]$Rva) {
    for ($index = 0; $index -lt $SectionCount; $index++) {
        $header = $SectionTable + 40 * $index
        $virtualSize = [long][BitConverter]::ToUInt32($Bytes, $header + 8)
        $virtualAddress = [long][BitConverter]::ToUInt32($Bytes, $header + 12)
        $rawSize = [long][BitConverter]::ToUInt32($Bytes, $header + 16)
        $rawPointer = [long][BitConverter]::ToUInt32($Bytes, $header + 20)
        if ($Rva -ge $virtualAddress -and $Rva -lt $virtualAddress + [Math]::Max($virtualSize, $rawSize)) {
            $offset = $Rva - $virtualAddress + $rawPointer
            if ($Rva - $virtualAddress -ge $rawSize -or $offset -ge $Bytes.Length) { throw 'Executable data directory lies outside its file data.' }
            return $offset
        }
    }
    throw 'Executable data directory lies outside every section.'
}

function Get-PeAsciiName([byte[]]$Bytes, [long]$Offset) {
    $end = $Offset
    while ($end -lt $Bytes.Length -and $Bytes[$end] -ne 0 -and $end - $Offset -lt 260) { $end++ }
    if ($end -ge $Bytes.Length -or $Bytes[$end] -ne 0) { throw 'Executable import name is malformed.' }
    return [Text.Encoding]::ASCII.GetString($Bytes, $Offset, $end - $Offset)
}

function Assert-HardenedExecutable([byte[]]$Bytes, [string]$Name) {
    $pe = [long][BitConverter]::ToInt32($Bytes, 0x3c)
    if ($pe -lt 64 -or $pe -gt $Bytes.Length - 264 -or [BitConverter]::ToUInt32($Bytes, $pe) -ne 0x4550 -or [BitConverter]::ToUInt16($Bytes, $pe + 24) -ne 0x20b) { throw "Expected x64 PE32+ executable: $Name" }
    $sectionCount = [int][BitConverter]::ToUInt16($Bytes, $pe + 6)
    $optional = $pe + 24
    $sectionTable = $optional + [BitConverter]::ToUInt16($Bytes, $pe + 20)
    if ($sectionTable + 40 * $sectionCount -gt $Bytes.Length) { throw "Executable section table is truncated: $Name" }
    $directoryCount = [BitConverter]::ToUInt32($Bytes, $optional + 108)

    # IMAGE_DLLCHARACTERISTICS_GUARD_CF: set by the linker for /guard:cf.
    if (([BitConverter]::ToUInt16($Bytes, $optional + 70) -band 0x4000) -eq 0) { throw "Executable lacks Control Flow Guard: $Name" }

    # Import (1) and delay-import (13) directories must not name the
    # redistributable runtime; the C and C++ runtime are linked statically.
    $dlls = [Collections.Generic.List[string]]::new()
    if ($directoryCount -gt 1 -and [BitConverter]::ToUInt32($Bytes, $optional + 120) -ne 0) {
        $descriptor = Get-PeFileOffset $Bytes $sectionTable $sectionCount ([BitConverter]::ToUInt32($Bytes, $optional + 120))
        $end = $descriptor + 20 * 4096
        while ($true) {
            if ($descriptor -ge $end -or $descriptor + 20 -gt $Bytes.Length) { throw "Executable import table is malformed: $Name" }
            $nameRva = [BitConverter]::ToUInt32($Bytes, $descriptor + 12)
            if ($nameRva -eq 0) { break }
            $dlls.Add((Get-PeAsciiName $Bytes (Get-PeFileOffset $Bytes $sectionTable $sectionCount $nameRva)))
            $descriptor += 20
        }
    }
    if ($directoryCount -gt 13 -and [BitConverter]::ToUInt32($Bytes, $optional + 112 + 13 * 8) -ne 0) {
        $descriptor = Get-PeFileOffset $Bytes $sectionTable $sectionCount ([BitConverter]::ToUInt32($Bytes, $optional + 112 + 13 * 8))
        $end = $descriptor + 32 * 4096
        while ($true) {
            if ($descriptor -ge $end -or $descriptor + 32 -gt $Bytes.Length) { throw "Executable delay-import table is malformed: $Name" }
            $nameRva = [BitConverter]::ToUInt32($Bytes, $descriptor + 4)
            if ($nameRva -eq 0) { break }
            $dlls.Add((Get-PeAsciiName $Bytes (Get-PeFileOffset $Bytes $sectionTable $sectionCount $nameRva)))
            $descriptor += 32
        }
    }
    foreach ($dll in $dlls) {
        if ($dll -match '^(vcruntime140.*|msvcp140.*)\.dll$') { throw "Executable links the Visual C++ runtime DLL ${dll}: $Name" }
    }

    # IMAGE_DEBUG_TYPE_EX_DLLCHARACTERISTICS (20) carrying
    # IMAGE_DLLCHARACTERISTICS_EX_CET_COMPAT (0x1), written by /CETCOMPAT.
    $cet = $false
    if ($directoryCount -gt 6 -and [BitConverter]::ToUInt32($Bytes, $optional + 112 + 6 * 8) -ne 0) {
        $debug = Get-PeFileOffset $Bytes $sectionTable $sectionCount ([BitConverter]::ToUInt32($Bytes, $optional + 112 + 6 * 8))
        $entries = [int][Math]::Min([Math]::Floor([double][BitConverter]::ToUInt32($Bytes, $optional + 112 + 6 * 8 + 4) / 28), 64)
        for ($index = 0; $index -lt $entries; $index++) {
            $entry = $debug + 28 * $index
            if ($entry + 28 -gt $Bytes.Length) { throw "Executable debug directory is truncated: $Name" }
            $data = [long][BitConverter]::ToUInt32($Bytes, $entry + 24)
            if ([BitConverter]::ToUInt32($Bytes, $entry + 12) -eq 20 -and [BitConverter]::ToUInt32($Bytes, $entry + 16) -ge 4 -and $data + 4 -le $Bytes.Length) {
                $cet = $cet -or (([BitConverter]::ToUInt32($Bytes, $data) -band 0x1) -ne 0)
            }
        }
    }
    if (-not $cet) { throw "Executable is not CET shadow-stack compatible: $Name" }
}
