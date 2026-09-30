# SPDX-License-Identifier: MPL-2.0
# Installs the pinned Notepad++ portable build used as the comparison baseline.
# The archive must match the pinned SHA-256 and size, notepad++.exe must carry a
# valid Authenticode signature and the pinned file version, and the auto-updater
# (updater\GUP.exe) is removed so the baseline cannot change between runs.
# Pass -Archive to install from an already downloaded zip without network access.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Destination,
    [string]$Archive
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

# Bump all four values together, from the GitHub release asset digest, in one reviewed change.
$pin = [ordered]@{
    version = '8.9.8.1'
    url = 'https://github.com/notepad-plus-plus/notepad-plus-plus/releases/download/v8.9.8.1/npp.8.9.8.1.portable.x64.zip'
    archive_sha256 = 'beddf5548e75c97930d1575ba1c7d113e6e2b33d0a200310e734ca08e225f414'
    archive_bytes = 8250001
}
$receiptName = 'bareline-notepadpp-pin.json'

function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

function Test-Installed([string]$Directory) {
    $receipt = Join-Path $Directory $receiptName
    $exe = Join-Path $Directory 'notepad++.exe'
    if (-not (Test-Path -LiteralPath $receipt) -or -not (Test-Path -LiteralPath $exe)) { return $false }
    $recorded = Get-Content -LiteralPath $receipt -Raw | ConvertFrom-Json
    $recorded.archive_sha256 -eq $pin.archive_sha256 -and $recorded.exe_sha256 -eq (Get-Sha256 $exe) -and
        -not (Get-ChildItem -LiteralPath $Directory -Recurse -Filter 'GUP.exe' -ErrorAction SilentlyContinue)
}

$Destination = [IO.Path]::GetFullPath($Destination)
if (Test-Installed $Destination) {
    Write-Output "Pinned Notepad++ $($pin.version) already installed at $Destination"
    return
}
if ((Test-Path -LiteralPath $Destination) -and (Get-ChildItem -LiteralPath $Destination -Force | Select-Object -First 1)) {
    throw "Destination is not empty and holds no valid pinned install: $Destination"
}

$download = $null
if (-not $Archive) {
    $download = Join-Path ([IO.Path]::GetTempPath()) ("npp-{0}-{1}.zip" -f $pin.version, [guid]::NewGuid().ToString('n'))
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -Uri $pin.url -OutFile $download -UseBasicParsing
    $Archive = $download
}
try {
    $length = (Get-Item -LiteralPath $Archive).Length
    $digest = Get-Sha256 $Archive
    if ($length -ne $pin.archive_bytes -or $digest -ne $pin.archive_sha256) {
        throw "Notepad++ archive does not match the pin: $length bytes, SHA-256 $digest"
    }
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Expand-Archive -LiteralPath $Archive -DestinationPath $Destination
} finally {
    if ($download) { Remove-Item -LiteralPath $download -Force -ErrorAction SilentlyContinue }
}

$exe = Join-Path $Destination 'notepad++.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw 'The pinned archive has no notepad++.exe at its root' }
$info = (Get-Item -LiteralPath $exe).VersionInfo
$fileVersion = '{0}.{1}.{2}.{3}' -f $info.FileMajorPart, $info.FileMinorPart, $info.FileBuildPart, $info.FilePrivatePart
if ($fileVersion -ne $pin.version) { throw "notepad++.exe reports $fileVersion, expected $($pin.version)" }
$signature = Get-AuthenticodeSignature -LiteralPath $exe
if ($signature.Status -ne 'Valid') { throw "notepad++.exe signature is $($signature.Status)" }

# No auto-updater: a baseline that updates itself is not a pinned baseline.
$updater = Join-Path $Destination 'updater'
if (Test-Path -LiteralPath $updater) { Remove-Item -LiteralPath $updater -Recurse -Force }
if (Get-ChildItem -LiteralPath $Destination -Recurse -Filter 'GUP.exe') { throw 'The Notepad++ updater is still present' }

$receipt = [ordered]@{
    version = $pin.version
    url = $pin.url
    archive_sha256 = $pin.archive_sha256
    archive_bytes = $pin.archive_bytes
    exe_sha256 = Get-Sha256 $exe
    file_version = $fileVersion
    signature_status = [string]$signature.Status
    signer = $signature.SignerCertificate.Subject
    updater_removed = $true
    installed_utc = [DateTime]::UtcNow.ToString('o')
}
[IO.File]::WriteAllText((Join-Path $Destination $receiptName), ($receipt | ConvertTo-Json), (New-Object Text.UTF8Encoding($false)))
Write-Output "Installed pinned Notepad++ $($pin.version) at $Destination"
