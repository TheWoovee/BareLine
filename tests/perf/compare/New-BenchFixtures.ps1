# SPDX-License-Identifier: MPL-2.0
# Generates the deterministic comparison fixtures and fixtures.json (sizes, SHA-256,
# line and match counts). Same inputs always give byte-identical files, so results
# from different machines use the same documents. Never launches an editor.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Destination,
    # Adds the 300 MB log (2.8M lines) used by the line-indexing probe.
    [switch]$IncludeHuge
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
. (Join-Path $PSScriptRoot 'BenchCommon.ps1')

$Destination = [IO.Path]::GetFullPath($Destination)
if ((Test-Path -LiteralPath $Destination) -and (Get-ChildItem -LiteralPath $Destination -Force | Select-Object -First 1)) {
    throw "Fixture destination must be empty: $Destination"
}
New-Item -ItemType Directory -Force -Path $Destination | Out-Null
$ascii = [Text.Encoding]::ASCII
$manifest = [ordered]@{ schema_version = 1; generator = 'bareline-bench-fixtures-v1'; fixtures = [ordered]@{} }

function Write-Fixture([string]$Name, [string]$File, [string]$Purpose, [scriptblock]$Body, [hashtable]$Extra = @{}) {
    $path = Join-Path $Destination $File
    $stream = New-Object IO.FileStream($path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None, 1MB)
    try { & $Body $stream } finally { $stream.Dispose() }
    $entry = [ordered]@{ file = $File; purpose = $Purpose; bytes = (Get-Item -LiteralPath $path).Length; sha256 = Get-FileSha256 $path }
    foreach ($key in ($Extra.Keys | Sort-Object)) { $entry[$key] = $Extra[$key] }
    $manifest.fixtures[$Name] = $entry
    Write-Output ('{0,-18} {1,14:N0} bytes  {2}' -f $Name, $entry.bytes, $File)
}

function Write-Ascii([IO.Stream]$Stream, [string]$Text) {
    $bytes = $ascii.GetBytes($Text)
    $Stream.Write($bytes, 0, $bytes.Length)
}

function Write-LogLines([IO.Stream]$Stream, [int]$Count) {
    # Fixed 112-byte lines with exactly one "INFO" each; buffered in 4 MB chunks.
    $builder = New-Object Text.StringBuilder(4MB)
    for ($index = 0; $index -lt $Count; $index++) {
        $seconds = $index % 86400
        $line = [string]::Format('2026-09-30T{0:D2}:{1:D2}:{2:D2}.{3:D3}Z INFO  [worker-{4:D2}] request={5:D8} path=/api/v1/items/{6:D4} status=200 ',
            [int][math]::Floor($seconds / 3600), [int][math]::Floor(($seconds % 3600) / 60), $seconds % 60, $index % 1000,
            $index % 16, $index, $index % 10000)
        [void]$builder.Append($line.PadRight(111, '.')).Append("`n")
        if ($builder.Length -ge 4MB - 256) { Write-Ascii $Stream $builder.ToString(); [void]$builder.Clear() }
    }
    Write-Ascii $Stream $builder.ToString()
}

$logLines = 468100
Write-Fixture 'log_50mb' 'log_50mb.log' 'open, save after a 1-byte edit, Replace All INFO -> INFO! (one match per line)' {
    param($stream) Write-LogLines $stream $logLines
} @{ lines = $logLines; matches = @{ INFO = $logLines } }

$sixLines = 62000
Write-Fixture 'text_6mb' 'text_6mb.txt' 'open; Select All + Copy (101-byte lines)' {
    param($stream)
    $filler = 'the quick brown fox jumps over the lazy dog ' * 3
    $builder = New-Object Text.StringBuilder(4MB)
    for ($index = 0; $index -lt $sixLines; $index++) {
        [void]$builder.Append(('line {0:D8} {1}' -f $index, $filler).Substring(0, 100)).Append("`n")
        if ($builder.Length -ge 4MB - 256) { Write-Ascii $stream $builder.ToString(); [void]$builder.Clear() }
    }
    Write-Ascii $stream $builder.ToString()
} @{ lines = $sixLines }

$longBytes = 10485760
Write-Fixture 'long_line_10mb' 'long_line_10mb.txt' 'open a single 10 MiB line without a newline' {
    param($stream)
    $unit = $ascii.GetBytes('abcdefghijklmnopqrstuvwxyz0123456789' * 29127)
    $written = 0
    while ($written -lt $longBytes) {
        $count = [math]::Min($unit.Length, $longBytes - $written)
        $stream.Write($unit, 0, $count); $written += $count
    }
} @{ lines = 1 }

$csvRows = 2000
Write-Fixture 'commas_csv' 'commas.csv' 'Replace All "," -> ";;" (six commas per row)' {
    param($stream)
    $builder = New-Object Text.StringBuilder
    for ($index = 0; $index -lt $csvRows; $index++) {
        [void]$builder.Append(('r{0:D5},alpha,beta,gamma,delta,epsilon,{1}' -f $index, (($index * 7919) % 100000))).Append("`n")
    }
    Write-Ascii $stream $builder.ToString()
} @{ lines = $csvRows; matches = @{ ',' = $csvRows * 6 } }

Write-Fixture 'crlf_regex' 'crlf_regex.txt' 'regex Replace All on CRLF text must keep CRLF' {
    param($stream) Write-Ascii $stream "alpha end`r`nbeta end`r`ngamma end`r`n"
} @{ lines = 3 }

# Encoding detection checks. Code points are spelled out to keep this file ASCII.
if ('System.Text.CodePagesEncodingProvider' -as [type]) {
    [Text.Encoding]::RegisterProvider([Text.CodePagesEncodingProvider]::Instance)
}
function Get-CodePointText([int[]]$CodePoints, [int]$Repeat) {
    $line = -join ($CodePoints | ForEach-Object { [char]$_ })
    (($line + "`r`n") * $Repeat)
}
# Simplified: "Chinese encoding test, this is a simplified Chinese text file."
$simplified = Get-CodePointText @(0x4E2D, 0x6587, 0x7F16, 0x7801, 0x6D4B, 0x8BD5, 0xFF0C, 0x8FD9, 0x662F, 0x4E00, 0x4E2A,
    0x7B80, 0x4F53, 0x4E2D, 0x6587, 0x6587, 0x672C, 0x6587, 0x4EF6, 0x3002) 200
# Traditional: the same sentence in traditional characters.
$traditional = Get-CodePointText @(0x4E2D, 0x6587, 0x7DE8, 0x78BC, 0x6E2C, 0x8A66, 0xFF0C, 0x9019, 0x662F, 0x4E00, 0x500B,
    0x7E41, 0x9AD4, 0x4E2D, 0x6587, 0x6587, 0x672C, 0x6A94, 0x6848, 0x3002) 200
foreach ($case in @(
        @{ Name = 'enc_gbk'; File = 'enc_gbk.txt'; Encoding = [Text.Encoding]::GetEncoding(936); Text = $simplified; Expect = 'GB2312|GBK|GB18030|936' },
        @{ Name = 'enc_big5'; File = 'enc_big5.txt'; Encoding = [Text.Encoding]::GetEncoding(950); Text = $traditional; Expect = 'Big5|950' },
        @{ Name = 'enc_utf16le_nobom'; File = 'enc_utf16le_nobom.txt'; Encoding = (New-Object Text.UnicodeEncoding($false, $false));
            Text = ("UTF-16 little endian text without a byte order mark`r`n" * 200); Expect = 'UTF-16' },
        @{ Name = 'enc_utf8_ae_3mb'; File = 'enc_utf8_ae_3mb.txt'; Encoding = (New-Object Text.UTF8Encoding($false));
            Text = (('a' + [char]0xE9) * 1000000); Expect = 'UTF-8' })) {
    $bytes = $case.Encoding.GetBytes($case.Text)
    Write-Fixture $case.Name $case.File 'encoding detection' { param($stream) $stream.Write($bytes, 0, $bytes.Length) } @{ expect_encoding = $case.Expect }
}
Write-Fixture 'binary_256' 'binary_256.bin' 'binary bytes 0x00-0xFF: must open without a blocking prompt' {
    param($stream) $all = [byte[]](0..255); $stream.Write($all, 0, $all.Length)
} @{ expect_encoding = '' }

if ($IncludeHuge) {
    $hugeLines = $logLines * 6
    Write-Fixture 'log_300mb' 'log_300mb.log' 'open a 300 MB log; full line count and UI latency while indexing' {
        param($stream) Write-LogLines $stream $hugeLines
    } @{ lines = $hugeLines; matches = @{ INFO = $hugeLines } }
}

[IO.File]::WriteAllText((Join-Path $Destination 'fixtures.json'), ($manifest | ConvertTo-Json -Depth 6), (New-Object Text.UTF8Encoding($false)))
Write-Output "Wrote $($manifest.fixtures.Count) fixtures and fixtures.json to $Destination"
