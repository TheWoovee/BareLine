# SPDX-License-Identifier: MPL-2.0
# Bareline vs Notepad++ side-by-side benchmark (plan task P2-00, FC-10 protocol).
#
# Launches both editors on an unlocked interactive desktop, so run it only on the
# reference machine or the dedicated `bareline-perf` runner. It never injects
# keystrokes: menus are driven with WM_COMMAND, Notepad++ dialogs with button
# messages and Bareline's Find panel through UI Automation. It replaces the text
# clipboard during save/copy trials and restores it afterwards.
#
# Output (under -OutputDirectory): trials.jsonl (appended per trial), raw.json (all
# trials, warm-ups, checks, pins and protocol), results.md and summary.json (medians
# per cache state and configuration, from ../compare_report.py).
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Bareline,
    [Parameter(Mandatory)][string]$NotepadPlusPlus,
    # Directory written by New-BenchFixtures.ps1 (contains fixtures.json).
    [Parameter(Mandatory)][string]$Fixtures,
    [string]$OutputDirectory,
    # FC-10: medians of 10 measured runs per cell, after one warm-up for warm cells.
    [ValidateRange(1, 100)][int]$Runs = 10,
    [ValidateSet('Warm', 'Cold', 'Both')][string]$CacheState = 'Warm',
    # Cold cells run this command before every trial with the executable and fixture
    # paths as arguments; it must evict them from the file cache and exit 0.
    [string]$ColdCacheCommand,
    [ValidateSet('launch', 'open', 'save', 'copy', 'replace', 'checks')]
    [string[]]$Scenario = @('launch', 'open', 'save', 'copy', 'replace', 'checks'),
    [ValidateRange(1, 120)][int]$IdleSeconds = 10,
    [ValidateRange(0, 30)][int]$SettleSeconds = 3,
    [string]$MachineLabel = $env:COMPUTERNAME,
    # Declares this host as the pinned FC-10 reference machine.
    [switch]$ReferenceMachine,
    [string]$Python = 'python'
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'BenchCommon.ps1')
Initialize-BenchInterop

# ---------- Inputs, pins and output layout ----------

$blSource = (Resolve-Path -LiteralPath $Bareline).Path
$nppExe = (Resolve-Path -LiteralPath $NotepadPlusPlus).Path
$nppDir = Split-Path -Parent $nppExe
$nppReceiptPath = Join-Path $nppDir 'bareline-notepadpp-pin.json'
if (-not (Test-Path -LiteralPath $nppReceiptPath)) { throw 'Notepad++ is not a pinned install; run Install-NotepadPlusPlus.ps1' }
$nppReceipt = Get-Content -LiteralPath $nppReceiptPath -Raw | ConvertFrom-Json
if ($nppReceipt.exe_sha256 -ne (Get-FileSha256 $nppExe)) { throw 'notepad++.exe no longer matches its pinned install receipt' }
if (Get-ChildItem -LiteralPath $nppDir -Recurse -Filter 'GUP.exe') { throw 'The Notepad++ auto-updater is present; reinstall the pinned build' }

$fixtureDir = (Resolve-Path -LiteralPath $Fixtures).Path
$fixtureManifest = Get-Content -LiteralPath (Join-Path $fixtureDir 'fixtures.json') -Raw | ConvertFrom-Json
$fixtureInfo = @{}
foreach ($property in $fixtureManifest.fixtures.PSObject.Properties) {
    $entry = $property.Value
    $path = Join-Path $fixtureDir $entry.file
    if ((Get-FileSha256 $path) -ne $entry.sha256) { throw "fixture does not match fixtures.json: $($entry.file)" }
    $fixtureInfo[$property.Name] = @{ Path = $path; Bytes = [long]$entry.bytes; Entry = $entry }
}

$cacheStates = @{ Warm = @('warm'); Cold = @('cold'); Both = @('warm', 'cold') }[$CacheState]
$coldCommand = $null
if ($cacheStates -contains 'cold') {
    if (-not $ColdCacheCommand) { throw 'Cold cells need -ColdCacheCommand; a cold label is never assumed' }
    $coldPath = (Resolve-Path -LiteralPath $ColdCacheCommand).Path
    $coldCommand = [ordered]@{ path = $coldPath; sha256 = Get-FileSha256 $coldPath }
}

if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $PSScriptRoot ('..\results\compare-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if ((Test-Path -LiteralPath $OutputDirectory) -and (Get-ChildItem -LiteralPath $OutputDirectory -Force | Select-Object -First 1)) {
    throw "Output directory must be new or empty: $OutputDirectory"
}
$workDir = Join-Path $OutputDirectory 'work'
$blRoot = Join-Path $OutputDirectory 'apps\bareline'
New-Item -ItemType Directory -Force -Path $workDir, $blRoot | Out-Null
# A private portable root: no session, settings or recovery from the user's profile.
$blExe = Join-Path $blRoot 'bareline.exe'
Copy-Item -LiteralPath $blSource -Destination $blExe
New-Item -ItemType File -Path (Join-Path $blRoot 'bareline.portable') | Out-Null
$trialLog = Join-Path $OutputDirectory 'trials.jsonl'
$utf8 = New-Object Text.UTF8Encoding($false)

$configs = [ordered]@{
    'npp-default' = @{ Application = 'notepadpp'; Exe = $nppExe; Args = @('-nosession', '-multiInst') }
    'npp-noPlugin' = @{ Application = 'notepadpp'; Exe = $nppExe; Args = @('-nosession', '-multiInst', '-noPlugin') }
    'bl-default' = @{ Application = 'bareline'; Exe = $blExe; Args = @('--no-session', '--new-instance') }
    'bl-hardware' = @{ Application = 'bareline'; Exe = $blExe; Args = @('--no-session', '--new-instance', '--hardware') }
    'bl-software' = @{ Application = 'bareline'; Exe = $blExe; Args = @('--no-session', '--new-instance', '--software') }
}
$trials = New-Object Collections.Generic.List[object]
$warmups = New-Object Collections.Generic.List[object]
$checks = New-Object Collections.Generic.List[object]

# ---------- Editor lifecycle ----------

function New-Timeout([string]$Message) { New-Object TimeoutException($Message) }

function Set-Measure($Trial, [string]$Name, $Value) {
    # Launch and open endpoints are the result of those scenarios; for the operation
    # scenarios they are only context, so they stay out of the timed metrics.
    if (@('launch', 'open') -contains $Trial.scenario) { $Trial.metrics[$Name] = $Value } else { $Trial.observations[$Name] = $Value }
}

function Reset-BarelineRoot {
    Get-ChildItem -LiteralPath $blRoot -Force | Where-Object { $_.Name -notin @('bareline.exe', 'bareline.portable') } |
        Remove-Item -Recurse -Force
}

function Invoke-ColdCache([string[]]$Paths) {
    if ($coldCommand.path -like '*.ps1') {
        & (Get-Process -Id $PID).Path -NoProfile -ExecutionPolicy Bypass -File $coldCommand.path @Paths
    } else {
        & $coldCommand.path @Paths
    }
    if ($LASTEXITCODE -ne 0) { throw "cold cache command failed with exit $LASTEXITCODE" }
}

function Start-Editor($Trial, [string[]]$Files = @()) {
    $config = $configs[$Trial.config]
    if ($config.Application -eq 'bareline') { Reset-BarelineRoot }
    if ($Trial.cache -eq 'cold') { Invoke-ColdCache (@($config.Exe) + $Files) }
    $arguments = @($config.Args)
    foreach ($file in $Files) { $arguments += '"' + $file + '"' }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $config.Exe -ArgumentList $arguments -WorkingDirectory (Split-Path -Parent $config.Exe) -PassThru
    $editor = @{ Process = $process; Watch = $watch; Window = [IntPtr]::Zero; Application = $config.Application }
    while ($editor.Window -eq [IntPtr]::Zero) {
        if ($watch.ElapsedMilliseconds -gt 60000) { throw (New-Timeout 'no main window within 60 s') }
        if ($process.HasExited) { throw "editor exited during launch with code $($process.ExitCode)" }
        Start-Sleep -Milliseconds 5
        $editor.Window = Find-MainWindow $process.Id
    }
    Set-Measure $Trial 'window_visible_ms' $watch.ElapsedMilliseconds
    [void]$process.WaitForInputIdle(60000)
    Set-Measure $Trial 'input_idle_ms' $watch.ElapsedMilliseconds
    $editor
}

function Wait-BarelineFirstFrame($Editor, $Trial) {
    # Bareline logs {"event":"first_frame","microseconds":N,"software":B} after its first present.
    $log = Join-Path $blRoot 'diagnostics\bareline.log'
    while ($Editor.Watch.ElapsedMilliseconds -lt 60000) {
        if (Test-Path -LiteralPath $log) {
            $line = Get-Content -LiteralPath $log -ErrorAction SilentlyContinue | Where-Object { $_ -like '*"event":"first_frame"*' } | Select-Object -First 1
            if ($line) {
                $Trial.metrics.first_frame_ms = $Editor.Watch.ElapsedMilliseconds
                $frame = $line | ConvertFrom-Json
                $Trial.observations.first_frame_inprocess_us = $frame.microseconds
                $Trial.observations.renderer = if ($frame.software) { 'software' } else { 'hardware' }
                return
            }
        }
        Start-Sleep -Milliseconds 10
    }
    throw (New-Timeout 'no first_frame diagnostic within 60 s')
}

function Get-DocumentState($Editor) {
    # Notepad++: Scintilla byte length; Bareline: the status bar's size and line count.
    if ($Editor.Application -eq 'notepadpp') {
        $scintilla = Get-NppEditor $Editor.Window
        return @{ Bytes = (Get-SciValue $scintilla $SCI_GETLENGTH); Lines = (Get-SciValue $scintilla $SCI_GETLINECOUNT); Status = '' }
    }
    $status = Get-StatusText $Editor.Window
    $state = @{ Bytes = -1; Lines = -1; Status = $status }
    if ($status -match 'Document size: (\d+) bytes') { $state.Bytes = [long]$Matches[1] }
    if ($status -match 'Document size: \d+ bytes, (\d+) lines') { $state.Lines = [long]$Matches[1] }
    $state
}

function Wait-DocumentLoaded($Editor, $Trial, [long]$Bytes, [int]$CapMs) {
    # First view: responsive window with content. Loaded: the whole document is
    # available (Scintilla length equals the file / Bareline reports its line count).
    $firstView = $null; $maxLatency = 0; $state = $null
    while ($Editor.Watch.ElapsedMilliseconds -lt $CapMs) {
        $latency = Measure-UiRoundTrip $Editor.Window 5000
        if ($latency -lt 0) { $latency = 5000 }
        $maxLatency = [math]::Max($maxLatency, $latency)
        $state = Get-DocumentState $Editor
        if ($null -eq $firstView -and $state.Bytes -gt 0) { $firstView = $Editor.Watch.ElapsedMilliseconds }
        $done = if ($Editor.Application -eq 'notepadpp') { $state.Bytes -eq $Bytes } else { $state.Bytes -eq $Bytes -and $state.Lines -ge 0 }
        if ($done) {
            Set-Measure $Trial 'first_view_ms' $firstView
            Set-Measure $Trial 'loaded_ms' $Editor.Watch.ElapsedMilliseconds
            Set-Measure $Trial 'max_ui_latency_ms' $maxLatency
            $Trial.observations.lines = $state.Lines
            return
        }
        Start-Sleep -Milliseconds 20
    }
    $Trial.observations.first_view_ms_before_timeout = $firstView
    throw (New-Timeout "not loaded within $([int]($CapMs / 1000)) s; last state: bytes=$($state.Bytes) lines=$($state.Lines) $($state.Status)")
}

function Wait-Bytes($Editor, [long]$Bytes, [int]$CapMs) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $CapMs) {
        if ((Get-DocumentState $Editor).Bytes -eq $Bytes) { return $watch.ElapsedMilliseconds }
        Start-Sleep -Milliseconds 20
    }
    -1
}

function Test-Modified($Editor) {
    if ($Editor.Application -eq 'notepadpp') { return (Get-SciValue (Get-NppEditor $Editor.Window) $SCI_GETMODIFY) -eq 1 }
    Test-BarelineModified $Editor.Window
}

function Get-Notice($Editor) {
    if ($Editor.Application -eq 'bareline') { return Get-BarelineNotices $Editor.Window }
    (Get-OwnedDialogTitles $Editor.Process.Id) -join '; '
}

function Save-AndWaitClean($Editor, [int]$CapMs) {
    Send-MenuCommand $Editor.Window '/File/Save'
    $clean = Wait-Condition { -not (Test-Modified $Editor) } $CapMs 50
    if ($clean -lt 0) { throw (New-Timeout 'document still modified after Save') }
}

# ---------- Replace All drivers ----------

function Invoke-NppReplaceAll($Editor, [string]$Find, [string]$Replace, [int]$ModeId, [int]$CapMs) {
    Send-MenuCommand $Editor.Window '/Search/Replace...'
    $dialog = [IntPtr]::Zero
    $waited = Wait-Condition {
        $script:nppDialog = [BenchNative]::Windows([uint32]$Editor.Process.Id, $true) | Where-Object {
            [BenchNative]::Cls($_) -eq '#32770' -and [BenchNative]::Text($_) -match 'Replace' } | Select-Object -First 1
        [bool]$script:nppDialog } 5000
    if ($waited -lt 0) { throw 'Notepad++ Replace dialog did not open' }
    $dialog = $script:nppDialog
    [void][BenchNative]::Send([BenchNative]::GetDlgItem($dialog, $ModeId), $BM_CLICK, 0, 3000)
    foreach ($option in @($NPP_IN_SELECTION, $NPP_DOT_MATCHES_NEWLINE)) {
        $control = [BenchNative]::GetDlgItem($dialog, $option)
        if ($control -ne [IntPtr]::Zero -and [BenchNative]::Ask($control, $BM_GETCHECK, 2000) -eq 1) { [void][BenchNative]::Send($control, $BM_CLICK, 0, 3000) }
    }
    foreach ($pair in @(@($NPP_FIND_WHAT, $Find), @($NPP_REPLACE_WITH, $Replace))) {
        $edit = [BenchNative]::FindWindowExW([BenchNative]::GetDlgItem($dialog, $pair[0]), [IntPtr]::Zero, 'Edit', $null)
        if (-not [BenchNative]::SetText($edit, $pair[1])) { throw "could not set Notepad++ field $($pair[0])" }
    }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    # Replace All runs synchronously inside the button message.
    if ([BenchNative]::Send([BenchNative]::GetDlgItem($dialog, $NPP_REPLACE_ALL), $BM_CLICK, 0, [uint32]$CapMs) -lt 0) {
        throw (New-Timeout 'Notepad++ Replace All did not return')
    }
    $elapsed = $watch.ElapsedMilliseconds
    [void][BenchNative]::PostMessageW($dialog, $WM_CLOSE, [UIntPtr]::Zero, [IntPtr]::Zero)
    $elapsed
}

function Open-BarelineReplace($Editor, [string]$ModePath) {
    # Select the search mode before opening Replace: the panel's fields drop out of
    # UI Automation after a mode change while it is open (review A11Y-11).
    Send-MenuCommand $Editor.Window $ModePath
    Start-Sleep -Milliseconds 700
    for ($attempt = 0; $attempt -lt 2; $attempt++) {
        Send-MenuCommand $Editor.Window '/Search/Replace...'
        if ((Wait-Condition { [bool](Find-UiaByName $Editor.Window 'Replace with') } 8000 200) -ge 0) { return }
    }
    throw 'Bareline Replace panel is not available through UI Automation'
}

function Invoke-BarelineReplaceAll($Editor, $Trial, [string]$Find, [string]$Replace, [string]$ModePath, [long]$ExpectBytes, [int]$CapMs) {
    Open-BarelineReplace $Editor $ModePath
    Set-UiaValue $Editor.Window 'Find' $Find; Start-Sleep -Milliseconds 400
    Set-UiaValue $Editor.Window 'Replace with' $Replace; Start-Sleep -Milliseconds 400
    Invoke-UiaButton $Editor.Window 'Find Next'
    $found = Wait-Condition { (Get-StatusText $Editor.Window) -match 'Find results: (1 of|\d+ match|No matches)' } 60000 100
    if ($Trial) { $Trial.observations.find_first_ms = $found }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    Invoke-UiaButton $Editor.Window 'Replace All in Current Document'
    while ($watch.ElapsedMilliseconds -lt $CapMs) {
        if ($ExpectBytes -ge 0 -and (Get-DocumentState $Editor).Bytes -eq $ExpectBytes) { return $watch.ElapsedMilliseconds }
        $notice = Get-BarelineNotices $Editor.Window
        if ($notice -match 'not applied|Budget|limit|failed') { throw "Replace All refused: $notice" }
        if ($ExpectBytes -lt 0 -and $watch.ElapsedMilliseconds -ge 2000) { return $null }
        Start-Sleep -Milliseconds 100
    }
    $Editor.Process.Refresh()
    throw (New-Timeout ("Replace All incomplete after $([int]($CapMs / 1000)) s; cpu=$([math]::Round($Editor.Process.TotalProcessorTime.TotalSeconds, 1)) s; " +
            "status=$(Get-StatusText $Editor.Window); notices=$(Get-BarelineNotices $Editor.Window)"))
}

# ---------- Scenario bodies (one measured trial each) ----------

function Measure-Launch($Trial) {
    $editor = Start-Editor $Trial
    try {
        $ready = [math]::Max($Trial.metrics.window_visible_ms, $Trial.metrics.input_idle_ms)
        if ($editor.Application -eq 'bareline') {
            # winit reports input idle before the first frame; the frame is the fair "ready".
            Wait-BarelineFirstFrame $editor $Trial
            $ready = [math]::Max($ready, $Trial.metrics.first_frame_ms)
        }
        $Trial.metrics.ready_ms = $ready
        $wait = $ready + $IdleSeconds * 1000 - $editor.Watch.ElapsedMilliseconds
        if ($wait -gt 0) { Start-Sleep -Milliseconds $wait }
        $sample = Get-ProcessSample $editor.Process
        $Trial.metrics.idle_private_mb = $sample.private_mb
        $Trial.metrics.idle_working_set_mb = $sample.working_set_mb
        $Trial.metrics.idle_threads = $sample.threads
        $Trial.metrics.idle_handles = $sample.handles
    } finally { Stop-BenchProcess $editor.Process }
}

function Measure-Open($Trial) {
    $fixture = $fixtureInfo[$Trial.fixture]
    $cap = if ($fixture.Bytes -gt 100MB) { 120000 } else { 60000 }
    $editor = Start-Editor $Trial @($fixture.Path)
    try {
        Wait-DocumentLoaded $editor $Trial $fixture.Bytes $cap
        Start-Sleep -Seconds $SettleSeconds
        $sample = Get-ProcessSample $editor.Process
        $Trial.metrics.loaded_private_mb = $sample.private_mb
        $Trial.metrics.loaded_peak_working_set_mb = $sample.peak_working_set_mb
        $Trial.observations.cpu_s = $sample.cpu_s
        $Trial.observations.threads = $sample.threads
    } finally { Stop-BenchProcess $editor.Process }
}

function New-WorkCopy($Trial) {
    $fixture = $fixtureInfo[$Trial.fixture]
    $target = Join-Path $workDir ('{0}-{1}-{2}-{3}{4}' -f $Trial.scenario, $Trial.config, $Trial.cache, $Trial.run, [IO.Path]::GetExtension($fixture.Path))
    Copy-Item -LiteralPath $fixture.Path -Destination $target -Force
    $target
}

function Measure-Save($Trial) {
    $fixture = $fixtureInfo[$Trial.fixture]
    $target = New-WorkCopy $Trial
    $editor = Start-Editor $Trial @($target)
    try {
        Wait-DocumentLoaded $editor $Trial $fixture.Bytes 60000
        Set-Clipboard -Value 'X'
        Send-MenuCommand $editor.Window '/Edit/Paste'
        if ((Wait-Bytes $editor ($fixture.Bytes + 1) 30000) -lt 0) { throw (New-Timeout '1-byte paste was not applied') }
        if ((Wait-Condition { Test-Modified $editor } 5000 20) -lt 0) { throw 'document never showed as modified' }
        $watch = [Diagnostics.Stopwatch]::StartNew()
        Send-MenuCommand $editor.Window '/File/Save'
        while ($watch.ElapsedMilliseconds -lt 120000 -and -not ($Trial.metrics.Contains('save_to_disk_ms') -and $Trial.metrics.Contains('save_to_clean_ms'))) {
            if (-not $Trial.metrics.Contains('save_to_disk_ms') -and (Get-Item -LiteralPath $target).Length -eq $fixture.Bytes + 1) {
                $Trial.metrics.save_to_disk_ms = $watch.ElapsedMilliseconds
            }
            if (-not $Trial.metrics.Contains('save_to_clean_ms') -and -not (Test-Modified $editor)) {
                $Trial.metrics.save_to_clean_ms = $watch.ElapsedMilliseconds
            }
            Start-Sleep -Milliseconds 10
        }
        if (-not $Trial.metrics.Contains('save_to_clean_ms') -or -not $Trial.metrics.Contains('save_to_disk_ms')) {
            throw (New-Timeout "save incomplete after 120 s; notices: $(Get-Notice $editor)")
        }
    } finally { Stop-BenchProcess $editor.Process; Remove-Item -LiteralPath $target -Force -ErrorAction SilentlyContinue }
}

function Measure-Copy($Trial) {
    $fixture = $fixtureInfo[$Trial.fixture]
    $editor = Start-Editor $Trial @($fixture.Path)
    try {
        Wait-DocumentLoaded $editor $Trial $fixture.Bytes 60000
        $sentinel = 'bareline-bench-' + [guid]::NewGuid().ToString('n')
        Set-Clipboard -Value $sentinel
        Send-MenuCommand $editor.Window '/Edit/Select All'
        Start-Sleep -Milliseconds 500
        $watch = [Diagnostics.Stopwatch]::StartNew()
        Send-MenuCommand $editor.Window '/Edit/Copy'
        while ($watch.ElapsedMilliseconds -lt 30000) {
            $text = Get-Clipboard -Raw
            if ($text -and $text -ne $sentinel) {
                $Trial.metrics.copy_ms = $watch.ElapsedMilliseconds
                $Trial.observations.clipboard_chars = $text.Length
                $Trial.observations.clipboard_complete = ($text.Length -eq $fixture.Bytes)
                if ($text.Length -ne $fixture.Bytes) { throw "clipboard holds $($text.Length) of $($fixture.Bytes) characters" }
                return
            }
            Start-Sleep -Milliseconds 20
        }
        throw (New-Timeout "clipboard unchanged after 30 s; notices: $(Get-Notice $editor)")
    } finally { Stop-BenchProcess $editor.Process }
}

$replaceCases = @{
    commas_csv = @{ Find = ','; Replace = ';;' }
    log_50mb = @{ Find = 'INFO'; Replace = 'INFO!' }
}

function Measure-Replace($Trial) {
    $fixture = $fixtureInfo[$Trial.fixture]
    $case = $replaceCases[$Trial.fixture]
    $count = [long]$fixture.Entry.matches.($case.Find)
    $expect = $fixture.Bytes + $count * ($case.Replace.Length - $case.Find.Length)
    $target = New-WorkCopy $Trial
    $editor = Start-Editor $Trial @($target)
    try {
        Wait-DocumentLoaded $editor $Trial $fixture.Bytes 60000
        if ($editor.Application -eq 'notepadpp') {
            $sent = Invoke-NppReplaceAll $editor $case.Find $case.Replace $NPP_MODE_NORMAL 300000
            $applied = Wait-Bytes $editor $expect 30000
            if ($applied -lt 0) { throw "Notepad++ document is not $expect bytes after Replace All" }
            $Trial.metrics.replace_all_ms = $sent + $applied
        } else {
            $Trial.metrics.replace_all_ms = Invoke-BarelineReplaceAll $editor $Trial $case.Find $case.Replace '/Search/Mode/Literal Search Mode' $expect 180000
        }
        # Correctness: the saved bytes contain every replacement.
        Save-AndWaitClean $editor 120000
        $text = [IO.File]::ReadAllText($target)
        $replaced = Get-TextCount $text $case.Replace
        $Trial.observations.verified_on_disk = ($text.Length -eq $expect -and $replaced -eq $count)
        if (-not $Trial.observations.verified_on_disk) { throw "saved file has $replaced of $count replacements" }
    } finally { Stop-BenchProcess $editor.Process; Remove-Item -LiteralPath $target -Force -ErrorAction SilentlyContinue }
}

# ---------- Protocol ----------

function Add-Record($Record, [bool]$Measured) {
    $line = ConvertTo-Json -InputObject $Record -Depth 8 -Compress
    [IO.File]::AppendAllText($trialLog, $line + "`n", $utf8)
    if ($Measured) { $trials.Add($Record) } else { $warmups.Add($Record) }
}

function Invoke-Trial([string]$Name, [string]$FixtureName, [string]$Cache, [string]$Config, [int]$Run, [int]$Order, [scriptblock]$Body) {
    $trial = [ordered]@{
        id = '{0}/{1}/{2}/{3}/{4}' -f $Name, $(if ($FixtureName) { $FixtureName } else { '-' }), $Cache, $Config, $Run
        scenario = $Name; fixture = $FixtureName; cache = $Cache; config = $Config
        application = $configs[$Config].Application; run = $Run; order = $Order
        started_utc = [DateTime]::UtcNow.ToString('o'); status = 'ok'
        metrics = [ordered]@{}; observations = [ordered]@{}; error = $null
    }
    Write-Host ("{0,-60}" -f $trial.id) -NoNewline
    try { & $Body $trial }
    catch [TimeoutException] { $trial.status = 'timeout'; $trial.error = $_.Exception.Message }
    catch { $trial.status = 'failed'; $trial.error = $_.Exception.Message }
    Write-Host (' {0} {1}' -f $trial.status, (($trial.metrics.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' '))
    Add-Record $trial ($Run -gt 0)
    Start-Sleep -Milliseconds 700
}

function Invoke-Series([string]$Name, [string]$FixtureName, [string]$Cache, [string[]]$ConfigNames, [scriptblock]$Body) {
    if ($Cache -eq 'warm') {
        foreach ($config in $ConfigNames) { Invoke-Trial $Name $FixtureName $Cache $config 0 0 $Body }
    }
    for ($run = 1; $run -le $Runs; $run++) {
        # Rotate the start position each run so no configuration always goes first.
        $offset = ($run - 1) % $ConfigNames.Count
        for ($position = 0; $position -lt $ConfigNames.Count; $position++) {
            $config = $ConfigNames[($offset + $position) % $ConfigNames.Count]
            Invoke-Trial $Name $FixtureName $Cache $config $run $position $Body
        }
    }
}

function Add-Check([string]$Name, [string]$FixtureName, [string]$Config, $Passed, [string]$Detail) {
    $checks.Add([ordered]@{ name = $Name; fixture = $FixtureName; config = $Config; application = $configs[$Config].Application
            passed = $Passed; detail = $Detail })
}

function ConvertTo-Visible([byte[]]$Bytes) {
    ([Text.Encoding]::UTF8.GetString($Bytes)) -replace "`r", '\r' -replace "`n", '\n'
}

function Invoke-Checks {
    # Correctness observations from the review (not timed): regex on CRLF text and
    # encoding detection. Notepad++ runs the same inputs as the reference behaviour.
    $regexCases = @(
        @{ Name = 'regex (?m) end.*$ -> "" keeps CRLF'; Find = '(?m) end.*$'; Replace = ''; Expect = "alpha`r`nbeta`r`ngamma`r`n" },
        @{ Name = 'regex (?m)end$ -> END matches before CRLF'; Find = '(?m)end$'; Replace = 'END'; Expect = "alpha END`r`nbeta END`r`ngamma END`r`n" })
    foreach ($case in $regexCases) {
        foreach ($config in @('npp-default', 'bl-default')) {
            $trial = [ordered]@{ scenario = 'checks'; fixture = 'crlf_regex'; cache = 'warm'; config = $config; run = 0; metrics = [ordered]@{}; observations = [ordered]@{} }
            $target = New-WorkCopy $trial
            $editor = $null
            try {
                $editor = Start-Editor $trial @($target)
                Wait-DocumentLoaded $editor $trial $fixtureInfo.crlf_regex.Bytes 30000
                if ($editor.Application -eq 'notepadpp') { [void](Invoke-NppReplaceAll $editor $case.Find $case.Replace $NPP_MODE_REGEX 30000) }
                else { [void](Invoke-BarelineReplaceAll $editor $null $case.Find $case.Replace '/Search/Mode/Regular Expression Search Mode' -1 10000) }
                if (Test-Modified $editor) { Save-AndWaitClean $editor 15000 }
                $bytes = [IO.File]::ReadAllBytes($target)
                Add-Check $case.Name 'crlf_regex' $config ((ConvertTo-Visible $bytes) -eq (ConvertTo-Visible ([Text.Encoding]::ASCII.GetBytes($case.Expect)))) (ConvertTo-Visible $bytes)
            } catch {
                Add-Check $case.Name 'crlf_regex' $config $false ("error: " + $_.Exception.Message)
            } finally {
                if ($editor) { Stop-BenchProcess $editor.Process }
                Remove-Item -LiteralPath $target -Force -ErrorAction SilentlyContinue
            }
        }
    }
    foreach ($name in @('enc_gbk', 'enc_big5', 'enc_utf16le_nobom', 'enc_utf8_ae_3mb', 'binary_256')) {
        if (-not $fixtureInfo.ContainsKey($name)) { continue }
        $expect = [string]$fixtureInfo[$name].Entry.expect_encoding
        foreach ($config in @('npp-default', 'bl-default')) {
            $trial = [ordered]@{ scenario = 'checks'; fixture = $name; cache = 'warm'; config = $config; run = 0; metrics = [ordered]@{}; observations = [ordered]@{} }
            $editor = $null
            try {
                $editor = Start-Editor $trial @($fixtureInfo[$name].Path)
                Start-Sleep -Seconds 3
                $alive = -not $editor.Process.HasExited
                $status = if ($alive) { Get-StatusText $editor.Window } else { '' }
                $dialogs = @(if ($alive) { Get-OwnedDialogTitles $editor.Process.Id })
                $passed = $alive -and $dialogs.Count -eq 0 -and ($expect -eq '' -or $status -match $expect)
                $detail = "status=[$status] owned dialogs=[$($dialogs -join '; ')] alive=$alive"
                Add-Check "encoding $name (expect $(if ($expect) { $expect } else { 'no prompt' }))" $name $config $passed $detail
            } catch {
                Add-Check "encoding $name" $name $config $false ("error: " + $_.Exception.Message)
            } finally { if ($editor) { Stop-BenchProcess $editor.Process } }
        }
    }
}

function Get-MachineInfo {
    $os = Get-CimInstance Win32_OperatingSystem
    [ordered]@{
        label = $MachineLabel
        cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()
        logical_cpus = [Environment]::ProcessorCount
        memory_bytes = (Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory
        os = "$($os.Caption) $($os.Version)"
        power_scheme = ((& powercfg /getactivescheme) -join ' ').Trim()
        powershell = $PSVersionTable.PSVersion.ToString()
    }
}

# ---------- Run ----------

$savedClipboard = Get-Clipboard -Raw
$startedUtc = [DateTime]::UtcNow.ToString('o')
try {
    foreach ($cache in $cacheStates) {
        if ($Scenario -contains 'launch') {
            Invoke-Series 'launch' '' $cache @('npp-default', 'npp-noPlugin', 'bl-hardware', 'bl-software') ${function:Measure-Launch}
        }
        if ($Scenario -contains 'open') {
            foreach ($name in @('text_6mb', 'log_50mb', 'long_line_10mb', 'log_300mb')) {
                if ($fixtureInfo.ContainsKey($name)) { Invoke-Series 'open' $name $cache @('npp-default', 'bl-default') ${function:Measure-Open} }
            }
        }
        if ($Scenario -contains 'save') { Invoke-Series 'save' 'log_50mb' $cache @('npp-default', 'bl-default') ${function:Measure-Save} }
        if ($Scenario -contains 'copy') { Invoke-Series 'copy' 'text_6mb' $cache @('npp-default', 'bl-default') ${function:Measure-Copy} }
        if ($Scenario -contains 'replace') {
            foreach ($name in @('commas_csv', 'log_50mb')) { Invoke-Series 'replace' $name $cache @('npp-default', 'bl-default') ${function:Measure-Replace} }
        }
    }
    if ($Scenario -contains 'checks') { Invoke-Checks }
} finally {
    if ($null -ne $savedClipboard) { Set-Clipboard -Value $savedClipboard }
}

$bareInfo = (Get-Item -LiteralPath $blExe).VersionInfo
$raw = [ordered]@{
    schema_version = 1
    kind = 'bareline_notepadpp_comparison'
    harness = [ordered]@{
        files = [ordered]@{
            'BenchCommon.ps1' = Get-FileSha256 (Join-Path $PSScriptRoot 'BenchCommon.ps1')
            'Invoke-Bench.ps1' = Get-FileSha256 $PSCommandPath
        }
        timing_origin = 'harness Stopwatch started immediately before Start-Process'
    }
    protocol = [ordered]@{
        name = 'FC-10'; runs = $Runs; cache_states = $cacheStates; idle_seconds = $IdleSeconds
        settle_seconds = $SettleSeconds; warmup = 'one unmeasured trial per configuration before warm cells'
        interleaving = 'configuration order rotates every run'; cold_cache_command = $coldCommand
        reference_machine = [bool]$ReferenceMachine; scenarios = $Scenario
    }
    machine = Get-MachineInfo
    applications = [ordered]@{
        bareline = [ordered]@{ path = $blSource; sha256 = Get-FileSha256 $blExe; version = $bareInfo.FileVersion; portable = $true }
        notepadpp = [ordered]@{ path = $nppExe; sha256 = $nppReceipt.exe_sha256; version = $nppReceipt.file_version; pin = $nppReceipt }
    }
    configurations = $configs
    fixtures = $fixtureManifest.fixtures
    started_utc = $startedUtc
    finished_utc = [DateTime]::UtcNow.ToString('o')
    trials = $trials.ToArray()
    warmups = $warmups.ToArray()
    checks = $checks.ToArray()
}
$rawPath = Join-Path $OutputDirectory 'raw.json'
[IO.File]::WriteAllText($rawPath, (ConvertTo-Json -InputObject $raw -Depth 10), $utf8)
# Scratch copies and the private portable root are not evidence; raw.json pins the binary.
Remove-Item -LiteralPath $workDir, $blRoot -Recurse -Force -ErrorAction SilentlyContinue

$report = Join-Path $PSScriptRoot '..\compare_report.py'
& $Python $report comparison $rawPath --markdown (Join-Path $OutputDirectory 'results.md') --json (Join-Path $OutputDirectory 'summary.json')
if ($LASTEXITCODE -ne 0) { throw "compare_report.py failed; raw data is in $rawPath" }
Get-Content -LiteralPath (Join-Path $OutputDirectory 'results.md')
$failed = @($trials | Where-Object { $_.status -ne 'ok' }).Count
if ($failed) { Write-Warning "$failed trial(s) failed or timed out; see results.md" }
