# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Config,
    [Parameter(Mandatory)][string]$OutputDir,
    [string]$TargetDir
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$output = [IO.Path]::GetFullPath($OutputDir)
if (Test-Path -LiteralPath $output) { throw 'Use a new configured-build output directory' }
[IO.Directory]::CreateDirectory($output) | Out-Null
$prepared = Join-Path $output 'release-config.prepared'
& python (Join-Path $root 'scripts/release_config.py') prepare --config $Config --mode configured --output $prepared
if ($LASTEXITCODE) { throw 'Public release configuration validation failed' }
$previous = $env:BARELINE_PREPARED_RELEASE_CONFIG
$previousTarget = $env:CARGO_TARGET_DIR
try {
    $env:BARELINE_PREPARED_RELEASE_CONFIG = $prepared
    $requestedTarget = if ($TargetDir) { $TargetDir } elseif ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    $cargoTarget = if ([IO.Path]::IsPathRooted($requestedTarget)) { [IO.Path]::GetFullPath($requestedTarget) } else { [IO.Path]::GetFullPath((Join-Path $root $requestedTarget)) }
    $env:CARGO_TARGET_DIR = $cargoTarget
    & python (Join-Path $root 'scripts/release_config.py') verify-prepared --prepared $prepared --mode configured
    if ($LASTEXITCODE) { throw 'Source/config identity changed before configured build' }
    Push-Location $root
    try {
        cargo build --release --locked -p bareline -p bareline-update-helper -p bareline-extension-host --features bareline/configured-release,bareline-update-helper/configured-release,bareline-extension-host/configured-release
        if ($LASTEXITCODE) { throw 'Configured release build failed' }
    } finally { Pop-Location }
} finally {
    $env:BARELINE_PREPARED_RELEASE_CONFIG = $previous
    $env:CARGO_TARGET_DIR = $previousTarget
}
& python (Join-Path $root 'scripts/release_config.py') verify-prepared --prepared $prepared --mode configured
if ($LASTEXITCODE) { throw 'Source/config identity changed during configured build' }
$artifacts = Join-Path $output 'unsigned-executables'
[IO.Directory]::CreateDirectory($artifacts) | Out-Null
foreach ($name in @('bareline.exe', 'bareline-update-helper.exe', 'bareline-extension-host.exe')) {
    [IO.File]::Copy((Join-Path $cargoTarget "release/$name"), (Join-Path $artifacts $name), $false)
}
& python (Join-Path $root 'scripts/release_config.py') manifest --prepared $prepared --artifact "editor=$(Join-Path $artifacts 'bareline.exe')" --artifact "helper=$(Join-Path $artifacts 'bareline-update-helper.exe')" --artifact "runtime=$(Join-Path $artifacts 'bareline-extension-host.exe')" --output (Join-Path $output 'build-capabilities.json')
if ($LASTEXITCODE) { throw 'Capability assertion or manifest generation failed' }
& python (Join-Path $root 'scripts/release_config.py') inventory --prepared $prepared --kind core --artifact "editor=$(Join-Path $artifacts 'bareline.exe')" --artifact "helper=$(Join-Path $artifacts 'bareline-update-helper.exe')" --output (Join-Path $output 'core-inventory.json')
if ($LASTEXITCODE) { throw 'Core inventory generation failed' }
Write-Output 'Configured unsigned executables and public capability manifest produced.'
Write-Output 'No signing, packaging, publishing, installation, or endpoint contact was performed.'
