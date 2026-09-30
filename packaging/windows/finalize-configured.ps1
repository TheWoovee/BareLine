# SPDX-License-Identifier: MPL-2.0
# Resumable external-signing handoff. Never handles private keys or publishes.
[CmdletBinding(DefaultParameterSetName='Prepare')]
param(
    [Parameter(Mandatory)][string]$Handoff,
    [Parameter(Mandatory,ParameterSetName='Prepare')][string]$Assembled,
    [Parameter(Mandatory,ParameterSetName='Prepare')][string]$SignedInstaller,
    [Parameter(Mandatory,ParameterSetName='Verify')][string]$Prepared,
    [Parameter(Mandatory,ParameterSetName='Verify')][string]$InventorySignature,
    [Parameter(Mandatory,ParameterSetName='Verify')][string]$Minisign,
    [Parameter(Mandatory)][string]$AuthorityVerifier,
    [Parameter(Mandatory)][string]$OutputDir
)
$ErrorActionPreference = 'Stop'
$pipeline = Join-Path $PSScriptRoot '../../scripts/release_pipeline.py'
. (Join-Path $PSScriptRoot 'authority-payload.ps1')
. (Join-Path $PSScriptRoot 'release-layout.ps1')
function Assert-RegularPath([string]$Path) {
    $item = Get-Item -LiteralPath $Path
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Regular file required: $Path" }
    $parent = $item.Directory
    while ($parent) {
        if ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse ancestor rejected' }
        $parent = $parent.Parent
    }
    $item.FullName
}
function New-PhaseDirectory([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    if (Test-Path -LiteralPath $full) { throw 'Use a new phase output directory; failed attempts are retained' }
    $parent = [IO.DirectoryInfo]::new([IO.Path]::GetDirectoryName($full))
    while ($parent) {
        if ($parent.Exists -and ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Reparse output ancestor rejected' }
        $parent = $parent.Parent
    }
    [IO.Directory]::CreateDirectory($full).FullName
}
if ($PSCmdlet.ParameterSetName -eq 'Prepare') {
    & python $pipeline verify-handoff --handoff $Handoff
    if ($LASTEXITCODE) { throw 'Configured handoff verification failed' }
    $configPath = Assert-RegularPath (Join-Path $Handoff 'public-release-config.json')
    $config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
    foreach ($name in (Get-RequiredReleaseFiles $config.distribution.version)) {
        $null = Assert-RegularPath (Join-Path $Assembled $name)
    }
    $authority = Test-AuthorityPayload $Assembled $configPath $AuthorityVerifier
    $installerPath = Assert-RegularPath $SignedInstaller
    Test-AuthenticodePublisher $installerPath $authority.authority.authenticode_subject @($authority.authority.authenticode_issuers)
    & python $pipeline prepare-final --handoff $Handoff --assembled $Assembled --signed-installer $installerPath --output $OutputDir
    if ($LASTEXITCODE) { throw 'Final inventory preparation failed' }
    Write-Output "Sign $OutputDir/artifacts/SHA-256SUMS externally with the authorized release key, then invoke the verification phase. No release approval has been recorded."
    return
}
& python $pipeline verify-final-request --prepared $Prepared --handoff $Handoff
if ($LASTEXITCODE) { throw 'Prepared final inventory changed' }
$configPath = Assert-RegularPath (Join-Path $Handoff 'public-release-config.json')
$inventorySignaturePath = Assert-RegularPath $InventorySignature
$authority = Test-AuthorityPayload (Join-Path $Prepared 'artifacts') $configPath $AuthorityVerifier
$config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
$output = New-PhaseDirectory $OutputDir
$artifacts = New-PhaseDirectory (Join-Path $output 'artifacts')
$request = Get-Content -LiteralPath (Join-Path $Prepared 'finalization-request.json') -Raw | ConvertFrom-Json
foreach ($property in $request.files.PSObject.Properties) {
    if ($property.Name -cnotmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$') { throw 'Unsafe prepared artifact filename' }
    $source = Assert-RegularPath (Join-Path (Join-Path $Prepared 'artifacts') $property.Name)
    $target = Join-Path $artifacts $property.Name
    [IO.File]::Copy($source, $target, $false)
    if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $property.Value.sha256 -or
        (Get-Item -LiteralPath $target).Length -ne $property.Value.bytes) { throw 'Prepared artifact changed while copying' }
}
[IO.File]::Copy($configPath, (Join-Path $output 'public-release-config.json'), $false)
if ((Get-FileHash -LiteralPath (Join-Path $output 'public-release-config.json')).Hash -ne $request.configuration.sha256) { throw 'Prepared config changed while copying' }
[IO.File]::Copy($inventorySignaturePath, (Join-Path $artifacts 'SHA-256SUMS.minisig'), $false)
& (Join-Path $PSScriptRoot 'verify-release.ps1') -ArtifactDir $artifacts -Minisign $Minisign -ReleasePublicKey $authority.authority.release_public_key -AuthenticodeSubject $authority.authority.authenticode_subject -AuthenticodeIssuers @($authority.authority.authenticode_issuers) -Version $config.distribution.version -ReleaseConfig (Join-Path $output 'public-release-config.json') -AuthorityVerifier $AuthorityVerifier
$receipt = [ordered]@{
    schema_version = 1; kind = 'verified_configured_release'; verified_utc = [DateTimeOffset]::UtcNow.ToString('o')
    source_sha256 = $request.source_sha256; unsigned_handoff_sha256 = $request.unsigned_handoff_sha256
    inventory_sha256 = (Get-FileHash -LiteralPath (Join-Path $artifacts 'SHA-256SUMS')).Hash.ToLowerInvariant()
    inventory_signature_sha256 = (Get-FileHash -LiteralPath (Join-Path $artifacts 'SHA-256SUMS.minisig')).Hash.ToLowerInvariant()
    release_approved = $false; clean_machine_acceptance = 'required'; published = $false
}
[IO.File]::WriteAllText((Join-Path $output 'verification.json'), ($receipt | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
Write-Output "Final signed artifacts verified: $artifacts. Clean-machine qualification and owner release approval remain required; nothing was published."
