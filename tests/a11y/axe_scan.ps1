# SPDX-License-Identifier: MPL-2.0
# Axe.Windows CLI scan of the editor's main window (QA-13). Downloads the pinned
# CLI release and verifies its size and SHA-256 before extracting it, launches one
# owned editor on an isolated profile, scans it by process ID and stops only that
# process. No input is sent. Used by the non-required native-journeys workflow.
param(
    [Parameter(Mandatory = $true)][string]$Executable,
    # Fresh directory for the profile, editor logs and Axe results.
    [Parameter(Mandatory = $true)][string]$Output,
    # Exit 0 with a warning, instead of failing, while no reviewed digest is pinned.
    [switch]$SkipWhenUnpinned
)
$ErrorActionPreference = 'Stop'
$AxeVersion = '2.4.2'
$AxeUrl = "https://github.com/microsoft/axe-windows/releases/download/v$AxeVersion/AxeWindowsCLI-$AxeVersion.zip"
$AxeBytes = 34751526
# SHA-256 of AxeWindowsCLI-2.4.2.zip. The release publishes no digest, so a
# maintainer records it after downloading and reviewing the archive once. The
# scan refuses to run an unverified download.
$AxeSha256 = ''
if ($AxeSha256 -notmatch '^[0-9a-f]{64}$') {
    $message = "Axe.Windows CLI $AxeVersion has no reviewed SHA-256 pinned in tests/a11y/axe_scan.ps1; scan not run."
    if ($SkipWhenUnpinned) { Write-Output "::warning::$message"; return }
    throw $message
}
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$Output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Output)
if (Test-Path -LiteralPath $Output) { throw "Output directory already exists: $Output" }
$tool = Join-Path $Output 'tool'; $results = Join-Path $Output 'axe'; $profileRoot = Join-Path $Output 'profile'
foreach ($directory in @($tool, $results, (Join-Path $profileRoot 'local'), (Join-Path $profileRoot 'roaming'), (Join-Path $profileRoot 'temp'))) {
    [IO.Directory]::CreateDirectory($directory) | Out-Null
}
$archive = Join-Path $Output "AxeWindowsCLI-$AxeVersion.zip"
Invoke-WebRequest -Uri $AxeUrl -OutFile $archive -UseBasicParsing
if ((Get-Item -LiteralPath $archive).Length -ne $AxeBytes) { throw 'Axe.Windows CLI archive size differs from the pinned release' }
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $AxeSha256) { throw 'Axe.Windows CLI digest mismatch' }
Expand-Archive -LiteralPath $archive -DestinationPath $tool
$cli = @(Get-ChildItem -LiteralPath $tool -Recurse -File -Filter 'AxeWindowsCLI.exe')
if ($cli.Count -ne 1) { throw 'AxeWindowsCLI.exe missing or ambiguous in the pinned archive' }

# An isolated profile: the scan never reads or changes the runner user's settings.
$env:LOCALAPPDATA = Join-Path $profileRoot 'local'; $env:APPDATA = Join-Path $profileRoot 'roaming'
$env:TEMP = Join-Path $profileRoot 'temp'; $env:TMP = $env:TEMP
$stdout = Join-Path $Output 'editor.stdout.log'; $stderr = Join-Path $Output 'editor.stderr.log'
$editor = Start-Process -FilePath $Executable -ArgumentList @('--software', '--no-session', '--no-extensions', '--new-instance') `
    -WorkingDirectory $Output -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(30); $ready = $false
    do {
        if ($editor.HasExited) { throw 'Editor exited before its first frame' }
        $editor.Refresh()
        $stream = [IO.File]::Open($stdout, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
        try { $text = [IO.StreamReader]::new($stream).ReadToEnd() } finally { $stream.Dispose() }
        $ready = $editor.MainWindowHandle -ne [IntPtr]::Zero -and $text.Contains('"event":"first_frame"')
        if (-not $ready) { Start-Sleep -Milliseconds 100 }
    } while (-not $ready -and [DateTime]::UtcNow -lt $deadline)
    if (-not $ready) { throw 'Editor window and first frame were not ready within 30 seconds' }
    $scan = Start-Process -FilePath $cli[0].FullName -ArgumentList @('--processId', $editor.Id, '--outputDirectory', $results, '--verbosity', 'default') `
        -PassThru -NoNewWindow -RedirectStandardOutput (Join-Path $Output 'axe.stdout.log') -RedirectStandardError (Join-Path $Output 'axe.stderr.log')
    if (-not $scan.WaitForExit(180000)) { $scan.Kill(); throw 'Axe.Windows scan exceeded 180 seconds' }
    $code = $scan.ExitCode
} finally {
    if (-not $editor.HasExited) { $editor.Kill(); [void]$editor.WaitForExit(5000) }
}
$files = @(Get-ChildItem -LiteralPath $results -File | ForEach-Object { $_.Name })
[IO.File]::WriteAllText((Join-Path $Output 'axe-summary.json'), (@{
    axe_version = $AxeVersion; axe_sha256 = $AxeSha256; exit_code = $code; results = $files
    editor_sha256 = (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash.ToLowerInvariant()
} | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
Get-Content -LiteralPath (Join-Path $Output 'axe.stdout.log')
# Axe.Windows CLI: 0 = no errors found, 1 = accessibility errors found; anything else is a tool failure.
if ($code -eq 1) { throw "Axe.Windows found accessibility errors; see $results" }
if ($code -ne 0) { throw "Axe.Windows CLI failed with exit code $code" }
Write-Output "Axe.Windows found no errors in the main window."
