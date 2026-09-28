#requires -Version 7.0
# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ArtifactDir,
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [switch]$DisposableMachine
)
$ErrorActionPreference = 'Stop'
$hostedRunner = $env:GITHUB_ACTIONS -eq 'true' -and $env:RUNNER_ENVIRONMENT -eq 'github-hosted' -and $env:RUNNER_TEMP
if (-not $IsWindows -or (-not $hostedRunner -and -not $DisposableMachine)) {
    throw 'Installer lifecycle checks require a disposable GitHub-hosted Windows runner or an explicit -DisposableMachine opt-in on a clean test VM.'
}
$registration = '{B91880A1-9E41-4868-B472-DF08FD48B7E4}_is1'
$registryPaths = @("HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$registration", "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$registration", "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\$registration")
$optionalKeys = @('HKCU:\Software\Classes\*\shell\Bareline', 'HKCU:\Software\Classes\Applications\bareline.exe', 'HKLM:\Software\Classes\*\shell\Bareline', 'HKLM:\Software\Classes\Applications\bareline.exe')
foreach ($path in $registryPaths + $optionalKeys) {
    if (Test-Path -LiteralPath $path) { throw "Existing Bareline registration found; lifecycle test refused: $path" }
}
foreach ($base in @($env:APPDATA, $env:LOCALAPPDATA, $env:ProgramFiles, ${env:ProgramFiles(x86)})) {
    if ($base -and (Test-Path -LiteralPath (Join-Path $base 'Bareline'))) { throw "Existing Bareline profile or installation found under $base; lifecycle test refused." }
}
if (-not $env:LOCALAPPDATA -or (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'Programs/Bareline'))) { throw 'Existing per-user installation or missing local profile root; lifecycle test refused.' }
if (Get-Process -Name bareline,bareline-update-helper -ErrorAction SilentlyContinue) { throw 'Existing Bareline process found; lifecycle test refused.' }
$artifacts = (Resolve-Path -LiteralPath $ArtifactDir).Path
& (Join-Path $PSScriptRoot 'verify-preview.ps1') -ArtifactDir $artifacts -Version $Version -RequireInstaller
$scratchBase = if ($hostedRunner) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$scratch = Join-Path $scratchBase ('bareline-installer-check-' + [Guid]::NewGuid().ToString('N'))
$installed = Join-Path $scratch 'installed'
$portable = Join-Path $scratch 'portable'
[IO.Directory]::CreateDirectory($scratch) | Out-Null
$installer = Join-Path $artifacts "bareline-$Version-windows-x64-setup.exe"

function Invoke-Setup([string]$Path, [string[]]$Arguments) {
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(120000)) { $process.Kill($true); throw 'Installer operation timed out.' }
    if ($process.ExitCode -ne 0) { throw "Installer operation failed: $($process.ExitCode)" }
}
function Test-Launch([string]$Directory) {
    $document = Join-Path $scratch ('open-' + [Guid]::NewGuid().ToString('N') + '.txt')
    [IO.File]::WriteAllText($document, "Bareline installed-package launch check.`n", [Text.UTF8Encoding]::new($false))
    $process = Start-Process -FilePath (Join-Path $Directory 'bareline.exe') -ArgumentList @('--', "`"$document`"") -WorkingDirectory $Directory -WindowStyle Hidden -PassThru
    try {
        if ($process.WaitForExit(5000)) { throw "Packaged editor exited during launch: $($process.ExitCode)" }
        if (-not $process.WaitForInputIdle(15000)) { throw 'Packaged editor did not initialize its message loop.' }
    } finally {
        if (-not $process.HasExited) { $process.Kill($true); $process.WaitForExit() }
    }
    $helperOutput = & (Join-Path $Directory 'bareline-update-helper.exe') --apply 2>&1 | Out-String
    if ($LASTEXITCODE -ne 1 -or $helperOutput -notmatch 'updates are disabled in this unsigned preview build') { throw 'Packaged preview update helper did not refuse updates.' }
}

[IO.Compression.ZipFile]::ExtractToDirectory((Join-Path $artifacts "bareline-$Version-windows-x64-portable.zip"), $portable)
Test-Launch $portable
$setupArguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/CURRENTUSER', '/NOICONS', '/TASKS=""', "/DIR=`"$installed`"")
Invoke-Setup $installer ($setupArguments + "/LOG=`"$(Join-Path $scratch 'install.log')`"")
if (-not (Test-Path -LiteralPath $registryPaths[0])) { throw 'Per-user installer registration missing.' }
if (Test-Path -LiteralPath (Join-Path $installed 'bareline.portable')) { throw 'Installer unexpectedly selected portable mode.' }
$sentinel = Join-Path $installed 'data/retained-user-data.txt'
[IO.Directory]::CreateDirectory((Split-Path -Parent $sentinel)) | Out-Null
[IO.File]::WriteAllText($sentinel, 'retain this user-created file')
Test-Launch $installed
$profileSentinel = Join-Path $env:LOCALAPPDATA 'Bareline/installer-lifecycle-retained.txt'
[IO.Directory]::CreateDirectory((Split-Path -Parent $profileSentinel)) | Out-Null
[IO.File]::WriteAllText($profileSentinel, 'retain this installed profile file')
# Reinstall the same preview to exercise replacement and user-data preservation.
Invoke-Setup $installer ($setupArguments + "/LOG=`"$(Join-Path $scratch 'reinstall.log')`"")
foreach ($name in @('bareline.exe', 'bareline-update-helper.exe', 'LICENSE', 'THIRD-PARTY-NOTICES.md', 'SBOM.json')) {
    if ((Get-FileHash -LiteralPath (Join-Path $installed $name)).Hash -ne (Get-FileHash -LiteralPath (Join-Path $portable $name)).Hash) { throw "Installed payload differs from verified portable payload: $name" }
}
foreach ($path in $optionalKeys) { if (Test-Path -LiteralPath $path) { throw "Unchecked optional registration was added: $path" } }
if ([IO.File]::ReadAllText($sentinel) -ne 'retain this user-created file') { throw 'Reinstall changed user data.' }
Invoke-Setup (Join-Path $installed 'unins000.exe') @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$(Join-Path $scratch 'uninstall.log')`"")
if (Test-Path -LiteralPath (Join-Path $installed 'bareline.exe')) { throw 'Uninstall retained the installed executable.' }
foreach ($path in $registryPaths) { if (Test-Path -LiteralPath $path) { throw "Uninstall retained product registration: $path" } }
if ([IO.File]::ReadAllText($sentinel) -ne 'retain this user-created file') { throw 'Uninstall changed user data.' }
if ([IO.File]::ReadAllText($profileSentinel) -ne 'retain this installed profile file') { throw 'Uninstall changed the installed user profile.' }
Write-Output "PASS: portable/installed launch, disabled updater, exact installed bytes, same-version reinstall, optional registration defaults, uninstall and retained user data. Logs: $scratch"
