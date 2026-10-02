# SPDX-License-Identifier: MPL-2.0
# Renders the six whole-window cells (100/150/200% x light/dark) offscreen through
# the production Direct2D/DirectWrite renderer and compares them with the reviewed
# baselines. No window is shown, no desktop is captured and no input is sent.
param(
    # Fresh directory for the captured BMPs; defaults under the ignored tests/visual/results.
    [string]$Output,
    # Directory for report.json and diff images.
    [string]$Report,
    # Capture only; regenerate.ps1 promotes the capture after review.
    [switch]$CaptureOnly,
    # Warn instead of failing while no reviewed baselines are committed.
    [switch]$AllowMissingBaselines
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not $Output) { $Output = Join-Path $root ('tests/visual/results/' + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')) }
$Output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Output)
if (Test-Path -LiteralPath $Output) { throw "Capture directory already exists: $Output" }
[IO.Directory]::CreateDirectory($Output) | Out-Null
$env:BARELINE_VISUAL_OUTPUT = $Output
Push-Location $root
try {
    # The capture test is ignored by default: its pixels depend on the system fonts.
    cargo test -p bareline --locked --bin bareline -- --ignored --exact windows_app::visual_baselines::capture_whole_window_cells
    if ($LASTEXITCODE) { throw 'Offscreen visual capture failed' }
} finally {
    Pop-Location
    Remove-Item Env:BARELINE_VISUAL_OUTPUT
}
Write-Output "Captured cells in $Output"
if ($CaptureOnly) { return }
$arguments = @((Join-Path $PSScriptRoot 'compare.py'), 'compare', '--actual', $Output)
if ($Report) { $arguments += @('--report', $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Report)) }
if ($AllowMissingBaselines) { $arguments += '--allow-missing-baselines' }
python @arguments
if ($LASTEXITCODE) { throw 'Visual cells differ from the reviewed baselines beyond tolerance; see the report and diff images' }
