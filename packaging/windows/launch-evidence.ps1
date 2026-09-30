# SPDX-License-Identifier: MPL-2.0
# Dot-source only. Reads the editor's diagnostics log (<data root>/diagnostics/
# bareline.log) while the editor may still append to it; nothing is launched.

# Returns the first well-formed first_frame event for the expected version, or
# $null while none has been written. Partial or unrelated lines are skipped.
function Read-FirstFrameEvent([string]$LogPath, [string]$Version) {
    if (-not (Test-Path -LiteralPath $LogPath -PathType Leaf)) { return $null }
    $stream = [IO.FileStream]::new($LogPath, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
    try {
        $reader = [IO.StreamReader]::new($stream, [Text.UTF8Encoding]::new($false))
        while ($null -ne ($line = $reader.ReadLine())) {
            if (-not $line.StartsWith('{"event":"first_frame"')) { continue }
            try { $row = $line | ConvertFrom-Json } catch { continue }
            $micros = $row.microseconds
            if ($row.version -ceq $Version -and ($micros -is [int] -or $micros -is [long]) -and $micros -ge 0 -and $row.software -is [bool]) {
                return $row
            }
        }
    } finally {
        $stream.Dispose()
    }
    return $null
}
