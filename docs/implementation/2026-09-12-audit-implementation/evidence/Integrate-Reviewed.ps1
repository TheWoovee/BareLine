param(
    [Parameter(Mandatory)][ValidatePattern('^[TU][0-9]{2}$')][string]$Pr,
    [Parameter(Mandatory)][ValidatePattern('^[a-f0-9]{40}$')][string]$Base,
    [Parameter(Mandatory)][ValidatePattern('^[a-f0-9]{40}$')][string]$Commit
)
$ErrorActionPreference = 'Stop'
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../../..'))
$statusPath = Join-Path $PSScriptRoot '../status.json'
$status = Get-Content -LiteralPath $statusPath -Raw | ConvertFrom-Json
$row = $status.prs | Where-Object pr -eq $Pr
if ($row.state -ne 'accepted-pending-integration') { throw 'Root review acceptance must be recorded before applying' }
$originalBase = if ($row.commit -and $row.commit -eq $Base) { $row.base } else { $Base }
Push-Location -LiteralPath $repoRoot
try {
    if ([IO.Path]::GetFullPath((git rev-parse --show-toplevel).Trim()) -ne $repoRoot) { throw 'Unexpected integration root' }
    $indexPath = git rev-parse --git-path index
    $indexHash = (Get-FileHash -LiteralPath $indexPath -Algorithm SHA256).Hash
    $patchPath = Join-Path $PSScriptRoot ('accepted-' + $Pr + '-' + $Commit.Substring(0,12) + '.patch')
    git diff --check $Base $Commit
    if ($LASTEXITCODE -ne 0) { throw 'Patch whitespace check failed' }
    git diff --binary ('--output=' + $patchPath) $Base $Commit
    if ($LASTEXITCODE -ne 0) { throw 'Patch export failed' }
    git apply --check --whitespace=error $patchPath
    if ($LASTEXITCODE -ne 0) { throw 'Patch does not apply cleanly; explicit review/merge required' }
    git apply --whitespace=error $patchPath
    if ($LASTEXITCODE -ne 0) { throw 'Patch application failed' }
    if ((Get-FileHash -LiteralPath $indexPath -Algorithm SHA256).Hash -ne $indexHash) { throw 'Original index changed' }
    $row.state = 'integrated-pending-final'
    $row.base = $originalBase
    $row.commit = $Commit
    $status | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $statusPath -Encoding utf8
    [PSCustomObject]@{pr=$Pr;commit=$Commit;state=$row.state;original_index_unchanged=$true} | ConvertTo-Json
} finally { Pop-Location }
