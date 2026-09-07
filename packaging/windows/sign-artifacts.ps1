# SPDX-License-Identifier: MPL-2.0
# CI-only signing gate. No keys or certificate material are accepted from source artifacts.
[CmdletBinding()]
param([Parameter(Mandatory)][string]$ArtifactDir,[Parameter(Mandatory)][string]$SignTool,[Parameter(Mandatory)][string]$SignerCertificateThumbprint,[Parameter(Mandatory)][string]$PublisherCertificateSha256,[Parameter(Mandatory)][string]$TimestampUrl)
$ErrorActionPreference='Stop'
if($env:GITHUB_ACTIONS -ne 'true' -or $env:GITHUB_REF -notlike 'refs/tags/*' -or $env:BARELINE_SIGNING_AUTHORIZED -ne 'tagged-protected-environment'){throw 'Signing requires tagged protected CI environment'}
if($SignerCertificateThumbprint -notmatch '^[a-fA-F0-9]{40}$' -or $PublisherCertificateSha256 -notmatch '^[a-fA-F0-9]{64}$'){throw 'Explicit certificate fingerprints required'}
$timestamp=[Uri]$TimestampUrl;if($timestamp.Scheme -ne 'https' -or $timestamp.UserInfo){throw 'HTTPS timestamp endpoint required'}
$root=(Resolve-Path -LiteralPath $ArtifactDir).Path
$files=@(Get-ChildItem -LiteralPath $root -File | Where-Object Extension -eq '.exe' | Sort-Object Name)
if(-not $files.Count){throw 'No executable artifacts'}
if(Get-ChildItem -LiteralPath $root -Filter '*.zip' -File){throw 'Sign inner executables before ZIP assembly'}
$before=@();$after=@()
foreach($file in $files){if($file.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Reparse signing target'};$before+= '{0}  {1}' -f (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant(),$file.Name}
[IO.File]::WriteAllLines((Join-Path $root 'SHA-256SUMS.before-signing'),$before)
foreach($file in $files){
    if($file.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Reparse signing target'}

    & $SignTool sign /fd SHA256 /sha1 $SignerCertificateThumbprint /tr $TimestampUrl /td SHA256 $file.FullName
    if($LASTEXITCODE -ne 0){throw "Signing failed: $($file.Name)"}
    $signature=Get-AuthenticodeSignature -LiteralPath $file.FullName
    if($signature.Status -ne 'Valid' -or -not $signature.SignerCertificate){throw 'Signed artifact failed trust verification'}
    $actual=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($signature.SignerCertificate.RawData))
    if($actual -ne $PublisherCertificateSha256){throw 'Signer differs from owner publisher pin'}
    $after+= '{0}  {1}' -f (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant(),$file.Name
}
[IO.File]::WriteAllLines((Join-Path $root 'SHA-256SUMS.before-signing'),$before)
[IO.File]::WriteAllLines((Join-Path $root 'SHA-256SUMS.after-signing'),$after)
