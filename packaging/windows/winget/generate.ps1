# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][uri]$InstallerUrl,
    [Parameter(Mandatory)][string]$OutputDir,
    [switch]$Validate
)
$ErrorActionPreference = 'Stop'
if ($InstallerUrl.Scheme -ne 'https' -or $InstallerUrl.UserInfo -or $InstallerUrl.AbsoluteUri.Contains("'")) { throw 'Expected HTTPS release artifact URL without credentials or quotes' }
$hash = (Get-FileHash -LiteralPath $Installer -Algorithm SHA256).Hash
[IO.Directory]::CreateDirectory([IO.Path]::GetFullPath($OutputDir)) | Out-Null
$manifest = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.singleton.1.6.0.schema.json
PackageIdentifier: Bareline.Bareline
PackageVersion: $Version
PackageLocale: en-US
Publisher: Bareline
PackageName: Bareline
License: MPL-2.0
ShortDescription: Native text editor
InstallerType: inno
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
UpgradeBehavior: install
Installers:
- Architecture: x64
  InstallerUrl: '$($InstallerUrl.AbsoluteUri)'
  InstallerSha256: $hash
  InstallerSwitches:
    Custom: /CURRENTUSER
ManifestType: singleton
ManifestVersion: 1.6.0
"@
[IO.File]::WriteAllText((Join-Path ([IO.Path]::GetFullPath($OutputDir)) 'Bareline.Bareline.yaml'), $manifest, [Text.UTF8Encoding]::new($false))
if ($Validate) {
    & winget validate --manifest $OutputDir
    if ($LASTEXITCODE -ne 0) { throw "winget validate failed: $LASTEXITCODE" }
}
