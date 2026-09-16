# SPDX-License-Identifier: MPL-2.0
# Resumable phase after external signing. Never signs, installs or publishes.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Handoff,
    [Parameter(Mandatory)][string]$Delivery,
    [Parameter(Mandatory)][string]$Documents,
    [Parameter(Mandatory)][string]$OutputDir,
    [Parameter(Mandatory)][string]$AuthorityVerifier,
    [Parameter(Mandatory)][string]$Iscc
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$configPath = Join-Path $Handoff 'public-release-config.json'
& python (Join-Path $repo 'scripts/release_pipeline.py') verify-signed --handoff $Handoff --signed-dir $Delivery
if ($LASTEXITCODE) { throw 'Signed images differ from the compared configured handoff' }
. (Join-Path $PSScriptRoot 'authority-payload.ps1')
$authority = Test-AuthorityPayload $Delivery $configPath $AuthorityVerifier
$config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
$now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
& $AuthorityVerifier --config $configPath --directory $Delivery --delivery-directory $Delivery --now $now | Out-Null
if ($LASTEXITCODE) { throw 'Runtime/update/catalog metadata or package bytes failed verification' }
foreach ($name in @('bareline.exe', 'bareline-update-helper.exe', 'bareline-extension-host.exe')) {
    $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $Delivery $name)
    if ($signature.Status -ne 'Valid' -or -not $signature.SignerCertificate) { throw "Unverified publisher: $name" }
    $hash = [Security.Cryptography.SHA256]::Create()
    try { $fingerprint = ([BitConverter]::ToString($hash.ComputeHash($signature.SignerCertificate.RawData))).Replace('-', '') }
    finally { $hash.Dispose() }
    if ($fingerprint -ne $authority.authority.publisher_certificate_sha256) { throw "Unexpected publisher: $name" }
}
$documentNames = @('LICENSE', 'SDK-LICENSES.md', 'THIRD-PARTY-NOTICES.md', 'SBOM.json', 'RELEASE-NOTES.md', 'MIGRATION-NOTES.md', 'KNOWN-ISSUES.md')
foreach ($name in $documentNames) {
    $item = Get-Item -LiteralPath (Join-Path $Documents $name)
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or -not $item.Length) { throw "Missing regular release document: $name" }
    if ($item.Length -gt 16MB) { throw "Release document size limit: $name" }
    if ((Get-Content -LiteralPath $item.FullName -Raw) -match '\{\{[^}]+\}\}') { throw "Unfilled release document: $name" }
}
$sbom = Get-Content -LiteralPath (Join-Path $Documents 'SBOM.json') -Raw | ConvertFrom-Json
if ($sbom.bomFormat -ne 'CycloneDX' -or -not $sbom.components) { throw 'Nonempty CycloneDX SBOM required' }
$output = [IO.Path]::GetFullPath($OutputDir)
$staging = $output + '.staging'
foreach ($path in @($output, $staging)) {
    if (Test-Path -LiteralPath $path) { throw 'Use new phase output paths; previous phases remain retained' }
    $parent = [IO.DirectoryInfo]::new([IO.Path]::GetDirectoryName($path))
    while ($parent) {
        if ($parent.Exists -and ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Reparse output ancestor' }
        $parent = $parent.Parent
    }
}
[IO.Directory]::CreateDirectory($staging) | Out-Null
[IO.Directory]::CreateDirectory($output) | Out-Null
foreach ($name in @('bareline.exe','bareline-update-helper.exe')) { [IO.File]::Copy((Join-Path $Delivery $name), (Join-Path $staging $name), $false) }
foreach ($name in $documentNames) {
    [IO.File]::Copy((Join-Path $Documents $name), (Join-Path $output $name), $false)
    if ($name -in @('LICENSE','THIRD-PARTY-NOTICES.md','SBOM.json')) { [IO.File]::Copy((Join-Path $Documents $name), (Join-Path $staging $name), $false) }
}
$authorityNames = @(Get-AuthorityPayloadFiles $Delivery -Required)
foreach ($name in $authorityNames) {
    [IO.File]::Copy((Join-Path $Delivery $name), (Join-Path $staging $name), $false)
    [IO.File]::Copy((Join-Path $Delivery $name), (Join-Path $output $name), $false)
}
$deliveryNames = @('bareline.update.json','bareline.update.minisig','runtime.json','runtime.minisig','catalog.json','catalog.json.minisig')
$catalog = Get-Content -LiteralPath (Join-Path $Delivery 'catalog.json') -Raw | ConvertFrom-Json
foreach ($entry in $catalog.entries) {
    if ($entry.sha256 -cnotmatch '^[a-f0-9]{64}$') { throw 'Invalid catalog digest' }
    $deliveryNames += $entry.sha256 + '.blex'
}
foreach ($name in $deliveryNames) {
    [IO.File]::Copy((Join-Path $Delivery $name), (Join-Path $staging $name), $false)
    [IO.File]::Copy((Join-Path $staging $name), (Join-Path $output $name), $false)
}
[IO.File]::Copy((Join-Path $Delivery 'bareline-extension-host.exe'), (Join-Path $staging 'bareline-extension-host.exe'), $false)
[IO.File]::Copy((Join-Path $Delivery 'bareline-extension-host.exe'), (Join-Path $output 'bareline-exthost-x64.exe'), $false)
# Revalidate the copy used by the packager, rather than trusting earlier reads.
& python (Join-Path $repo 'scripts/release_pipeline.py') verify-signed --handoff $Handoff --signed-dir $staging
if ($LASTEXITCODE) { throw 'Signing inputs changed while staging' }
& $AuthorityVerifier --config $configPath --directory $staging --delivery-directory $staging --now $now | Out-Null
if ($LASTEXITCODE) { throw 'Staged signed metadata or packages changed' }
foreach ($name in @('bareline.exe','bareline-update-helper.exe','bareline-extension-host.exe')) {
    if ((Get-FileHash -LiteralPath (Join-Path $Delivery $name)).Hash -ne (Get-FileHash -LiteralPath (Join-Path $staging $name)).Hash) { throw 'Staged executable changed' }
    $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $staging $name)
    if ($signature.Status -ne 'Valid' -or -not $signature.SignerCertificate) { throw 'Staged publisher verification failed' }
    $hash = [Security.Cryptography.SHA256]::Create()
    try { $fingerprint = ([BitConverter]::ToString($hash.ComputeHash($signature.SignerCertificate.RawData))).Replace('-', '') }
    finally { $hash.Dispose() }
    if ($fingerprint -ne $authority.authority.publisher_certificate_sha256) { throw 'Staged publisher changed' }
}
& (Join-Path $PSScriptRoot 'build.ps1') -PayloadDir $staging -Version $config.distribution.version -OutputDir $output -ReleaseConfig $configPath -AuthorityVerifier $AuthorityVerifier -Installer -Iscc $Iscc
Write-Output 'Packages assembled from verified configured inputs. Installer signature, final inventory signature and final verification remain required.'
