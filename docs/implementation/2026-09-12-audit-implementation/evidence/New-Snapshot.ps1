param([string]$Label = 'integrated')
$ErrorActionPreference = 'Stop'
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../../..'))
Push-Location -LiteralPath $repoRoot
try {
    $actualRoot = (git rev-parse --show-toplevel).Trim()
    if ([IO.Path]::GetFullPath($actualRoot) -ne $repoRoot) { throw 'Unexpected integration root' }
    $indexPath = git rev-parse --git-path index
    $indexHash = (Get-FileHash -LiteralPath $indexPath -Algorithm SHA256).Hash
    $parentCommit = git rev-parse refs/codex/audit-20260912/baseline
    $snapshotIndex = Join-Path (git rev-parse --absolute-git-dir) ('codex-audit-' + [guid]::NewGuid().ToString('N') + '.index')
    $previousIndexEnv = $env:GIT_INDEX_FILE
    try {
        $env:GIT_INDEX_FILE = $snapshotIndex
        git read-tree $parentCommit
        if ($LASTEXITCODE -ne 0) { throw 'Snapshot read-tree failed' }
        git -c core.safecrlf=false add -A -- . ':(exclude)docs/qa/**/evidence/**' ':(exclude)docs/implementation/2026-09-12-audit-implementation/evidence/**' 2> (Join-Path $PSScriptRoot 'snapshot-latest-add.log')
        if ($LASTEXITCODE -ne 0) { throw 'Snapshot add failed' }
        $tree = git write-tree
        if ($LASTEXITCODE -ne 0) { throw 'Snapshot write-tree failed' }
        $commit = git -c user.name='Codex Local Snapshot' -c user.email='codex-local-snapshot@localhost' commit-tree $tree -p $parentCommit -m ('Local reviewed audit integration: ' + $Label)
        if ($LASTEXITCODE -ne 0) { throw 'Snapshot commit failed' }
        git update-ref refs/codex/audit-20260912/baseline $commit $parentCommit
        if ($LASTEXITCODE -ne 0) { throw 'Snapshot ref update failed' }
    } finally {
        if ($null -eq $previousIndexEnv) { Remove-Item Env:GIT_INDEX_FILE -ErrorAction SilentlyContinue }
        else { $env:GIT_INDEX_FILE = $previousIndexEnv }
    }
    if ((Get-FileHash -LiteralPath $indexPath -Algorithm SHA256).Hash -ne $indexHash) { throw 'Original index changed' }
    [PSCustomObject]@{label=$Label;commit=$commit;parent=$parentCommit;tree=$tree;original_index_unchanged=$true} | ConvertTo-Json
} finally { Pop-Location }
