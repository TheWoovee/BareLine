#requires -Version 7.0
# SPDX-License-Identifier: MPL-2.0
# Combines the Windows, Linux and macOS preview artifacts into the one download
# set that the workflow attests and publishes: each part checked as it arrives,
# every file once, release notes extended for the Linux and macOS downloads, and
# the single SHA-256SUMS that covers all of them. The complete set is verified
# before it is written out for attestation.
[CmdletBinding()]
param(
    # unsigned-windows-preview: the packages, documents, notes and their own SHA-256SUMS.
    [Parameter(Mandatory)][string]$WindowsDir,
    # unsigned-linux-preview: only bareline-<version>-linux-x64.tar.gz.
    [Parameter(Mandatory)][string]$LinuxDir,
    # unsigned-macos-preview: only bareline-<version>-macos-arm64.dmg.
    [Parameter(Mandatory)][string]$MacosDir,
    [Parameter(Mandatory)][string]$OutputDir,
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [switch]$RequireInstaller,
    [ValidatePattern('^([0-9a-f]{40})?$')][string]$BuildHash,
    # SHA-256 the macOS job recorded after mounting and checking the disk image.
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-fA-F]{64}$')][string]$DmgSha256,
    # SHA-256 the Linux job reported for the tarball it extracted and ran.
    [ValidatePattern('^([0-9a-fA-F]{64})?$')][string]$TarballSha256
)
$ErrorActionPreference = 'Stop'
$verify = Join-Path $PSScriptRoot 'verify-preview.ps1'
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDir)
if (Test-Path -LiteralPath $output) { throw "Use a new preview output directory: $output" }
$tarballName = "bareline-$Version-linux-x64.tar.gz"
$dmgName = "bareline-$Version-macos-arm64.dmg"

# The Windows part still matches the inventory its own job wrote.
& $verify -ArtifactDir $WindowsDir -Version $Version -Platform windows -RequireInstaller:$RequireInstaller -BuildHash $BuildHash
$parts = @(
    @{ Directory = $LinuxDir; Name = $tarballName },
    @{ Directory = $MacosDir; Name = $dmgName }
)
foreach ($part in $parts) {
    $names = @(Get-ChildItem -LiteralPath $part.Directory -Force | ForEach-Object Name)
    if ($names.Count -ne 1 -or $names[0] -cne $part.Name) { throw "Expected only $($part.Name) in $($part.Directory); found: $($names -join ', ')" }
}
if ($TarballSha256 -and (Get-FileHash -LiteralPath (Join-Path $LinuxDir $tarballName) -Algorithm SHA256).Hash -ne $TarballSha256) { throw 'The Linux tarball differs from the one the Linux job extracted and ran.' }

[IO.Directory]::CreateDirectory($output) | Out-Null
foreach ($file in @(Get-ChildItem -LiteralPath $WindowsDir -File | Where-Object Name -ne 'SHA-256SUMS')) {
    [IO.File]::Copy($file.FullName, (Join-Path $output $file.Name), $false)
}
foreach ($part in $parts) {
    $destination = Join-Path $output $part.Name
    if (Test-Path -LiteralPath $destination) { throw "Two preview parts provide $($part.Name)." }
    [IO.File]::Copy((Join-Path $part.Directory $part.Name), $destination, $false)
}

$ports = @'

## Linux and macOS

These downloads are earlier in testing than the Windows build; "Preview limitations" in the repository README says what has been verified on each.

- `@TARBALL@`: the editor and the extension host for 64-bit Linux with glibc 2.39 or later (built on Ubuntu 24.04) and an X11 or Wayland session. Extract it and run `./bareline` in the extracted folder. The included `bareline.portable` keeps settings, sessions and recovery in the `data` folder beside it.
- `@DMG@`: Bareline.app for Apple silicon on macOS 13 or later. Open the disk image and drag Bareline to Applications. The app is ad-hoc signed and not notarized, so macOS blocks the first launch: Control-click the app and choose Open (on macOS 15 and later, try to open it once, then choose Open Anyway in System Settings > Privacy & Security), or run `xattr -dr com.apple.quarantine /Applications/Bareline.app`.

Each of these packages carries its own LICENSE, THIRD-PARTY-NOTICES.md and SBOM.json for its platform's dependencies; the separately published copies describe the Windows build. Automatic updates and extension loading are disabled on Linux and macOS too.
'@
$notesPath = Join-Path $output 'PREVIEW-NOTES.md'
[IO.File]::AppendAllText($notesPath, $ports.Replace("`r`n", "`n").Replace('@TARBALL@', $tarballName).Replace('@DMG@', $dmgName) + "`n", [Text.UTF8Encoding]::new($false))

$inventory = @(Get-ChildItem -LiteralPath $output -File | Where-Object Name -ne 'SHA-256SUMS' | Sort-Object Name | ForEach-Object {
    '{0}  {1}' -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $_.Name
})
[IO.File]::WriteAllText((Join-Path $output 'SHA-256SUMS'), (($inventory -join "`n") + "`n"), [Text.UTF8Encoding]::new($false))
& $verify -ArtifactDir $output -Version $Version -RequireInstaller:$RequireInstaller -BuildHash $BuildHash -DmgSha256 $DmgSha256
Write-Output "Complete unsigned preview download set and SHA-256SUMS: $output"
