param(
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_-]+$')][string]$Name,
    [Parameter(Mandatory)][string[]]$CargoArguments
)
$ErrorActionPreference = 'Stop'
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../../..'))
Push-Location -LiteralPath $repoRoot
try {
    if ([IO.Path]::GetFullPath((git rev-parse --show-toplevel).Trim()) -ne $repoRoot) {
        throw 'Unexpected integration checkout'
    }
    $logPath = Join-Path $PSScriptRoot ($Name + '.log')
    if (Test-Path -LiteralPath $logPath) { throw 'Use a new evidence name rather than overwriting an earlier run' }
    [IO.File]::WriteAllText($logPath, '')
    & cargo @CargoArguments 2>&1 | Tee-Object -FilePath $logPath
    $result = $LASTEXITCODE
    $receipt = [ordered]@{
        arguments = $CargoArguments
        completed_utc = [DateTime]::UtcNow.ToString('o')
        integration_snapshot = (git rev-parse refs/codex/audit-20260912/baseline).Trim()
        exit_code = $result
        zero_tests = [bool](Select-String -LiteralPath $logPath -Pattern '^running 0 tests$' -Quiet)
        log = $logPath
    }
    $receipt | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $PSScriptRoot ($Name + '.json')) -Encoding utf8
    if ($result -ne 0) { throw ('Contained check failed; see ' + $logPath) }
    if ($receipt.zero_tests) { Write-Warning 'A target ran zero tests; inspect the filter before counting verification' }
} finally { Pop-Location }
