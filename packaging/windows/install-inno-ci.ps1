#requires -Version 7.0
# SPDX-License-Identifier: MPL-2.0
# Disposable GitHub-hosted runners only. Never changes a developer installation.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or $env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or -not $env:RUNNER_TEMP) {
    throw 'Inno acquisition is restricted to disposable GitHub-hosted Windows runners. Locally, supply an existing Inno Setup 6.4.3 compiler with -Iscc.'
}
$directory = Join-Path $env:RUNNER_TEMP ('bareline-inno-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($directory) | Out-Null
$installer = Join-Path $directory 'innosetup-6.4.3.exe'
# Official immutable release asset and SHA-256 from its GitHub release metadata:
# https://github.com/jrsoftware/issrc/releases/tag/is-6_4_3
$url = 'https://github.com/jrsoftware/issrc/releases/download/is-6_4_3/innosetup-6.4.3.exe'
$expectedHash = 'f3c42116542c4cc57263c5ba6c4feabfc49fe771f2f98a79d2f7628b8762723b'
Invoke-WebRequest -Uri $url -OutFile $installer
if ((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Pinned Inno Setup installer SHA-256 mismatch.' }
$destination = Join-Path $directory 'compiler'
$process = Start-Process -FilePath $installer -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/CURRENTUSER', "/DIR=`"$destination`"") -WindowStyle Hidden -PassThru
if (-not $process.WaitForExit(120000)) { $process.Kill($true); throw 'Inno Setup installation timed out.' }
if ($process.ExitCode -ne 0) { throw "Inno Setup installation failed: $($process.ExitCode)" }
$compiler = Join-Path $destination 'ISCC.exe'
& $compiler '/Q' '/O-' (Join-Path $PSScriptRoot 'compiler-probe.iss') | Out-Host
if ($LASTEXITCODE -ne 0) { throw 'Installed Inno compiler did not pass the 6.4.3 version probe.' }
Write-Output $compiler
