# SPDX-License-Identifier: MPL-2.0
# Regenerates the visual baselines from a fresh offscreen capture. Run it on the
# reference image (GitHub-hosted windows-2022) or download the capture artifact of
# the native-journeys visual job and pass it with -Capture. Review every changed
# PNG before committing: baselines are reviewed like code.
param(
    [Parameter(Mandatory = $true)][string]$Reviewer,
    # An existing capture directory (six BMPs) to promote instead of capturing now.
    [string]$Capture
)
$ErrorActionPreference = 'Stop'
if (-not $Reviewer.Trim()) { throw 'A reviewer name is required' }
if (-not $Capture) {
    $Capture = Join-Path (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path ('tests/visual/results/' + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ'))
    & (Join-Path $PSScriptRoot 'capture.ps1') -Output $Capture -CaptureOnly
}
python (Join-Path $PSScriptRoot 'compare.py') promote --actual ($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Capture)) --reviewer $Reviewer
if ($LASTEXITCODE) { throw 'Baseline promotion failed' }
git -C $PSScriptRoot status --short -- baselines
Write-Output 'Review each changed baseline PNG (and manifest.json) in the diff before committing it.'
