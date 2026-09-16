# SPDX-License-Identifier: MPL-2.0
. (Join-Path $PSScriptRoot 'native_regex_transform.ps1')
function Confirm-FixtureCommand {
 $deadline=[DateTime]::UtcNow.AddSeconds(5);$dialog=[IntPtr]::Zero
 do {
  [JourneyInput]::Desktop();$foreground=[JourneyInput]::GetForegroundWindow()
  if($foreground -ne $script:window -and [JourneyInput]::Owner($foreground) -eq $script:process.Id){$dialog=$foreground;break}
  if($foreground -ne $script:window){throw 'Lost foreground before external consent'}
  Start-Sleep -Milliseconds 50
 }while([DateTime]::UtcNow -lt $deadline)
 if($dialog -eq [IntPtr]::Zero){throw 'External command consent did not appear'}
 Guard $dialog
 $texts=@(Elements $dialog | Where-Object {$_.Current.Name.StartsWith('Run this direct executable command?')})
 if($texts.Count -ne 1 -or -not $texts[0].Current.Name.Contains($script:macroFixture.python) -or -not $texts[0].Current.Name.Contains('external_fixture.py') -or -not $texts[0].Current.Name.Contains($script:scratch)){throw 'External command consent differs from generated fixture'}
 $yes=@(Elements $dialog | Where-Object {$_.Current.AutomationId -eq '6' -and $_.Current.Name.Replace('&','') -ceq 'Yes' -and $_.Current.IsEnabled})
 if($yes.Count -ne 1){throw 'Owned external consent Yes button missing'}
 Record 'external explicit consent' @{message=$texts[0].Current.Name;button=(Record-Element $yes[0])}
 [JourneyInput]::DialogButton($dialog,[IntPtr]$yes[0].Current.NativeWindowHandle)
 $deadline=[DateTime]::UtcNow.AddSeconds(5)
 do {
  [JourneyInput]::Desktop()
  if([JourneyInput]::GetForegroundWindow() -eq $script:window){Guard;return}
  if([JourneyInput]::Owner([JourneyInput]::GetForegroundWindow()) -ne $script:process.Id){throw 'Lost foreground after consent'}
  Start-Sleep -Milliseconds 50
 }while([DateTime]::UtcNow -lt $deadline)
 throw 'External command consent did not close'
}
function Run-Macro {
 Step 's1' 'Recording captures explicit Unicode-safe edit and literal-search arguments; the saved macro and exact transformed document are retained.' {
  Focus-Editor;Key 79 $true;File-Dialog $script:saved;Key 36 $true
  Regex-Menu 'Start Macro Recording';Focus-Editor;Text $script:macroFixture.prefix
  Regex-Menu 'Normal Search Mode';Regex-Field 'Find' $script:macroFixture.query 'macro query'
  $null=Regex-Status 'Find results: 1 matches' 'macro search complete';Key 13;Key 27
  Focus-Editor;Text $script:macroFixture.replacement
  Expect-Text $script:macroFixture.final 'macro recorded result'
  Regex-Menu 'Stop Macro Recording';Regex-Menu 'Save Macros'
  $macro=Join-Path $script:profile 'macros/macro-01.toml';$deadline=[DateTime]::UtcNow.AddSeconds(5)
  do {Guard;if([IO.File]::Exists($macro) -and (Get-Item -LiteralPath $macro).Length -gt 0){break};Start-Sleep -Milliseconds 50}while([DateTime]::UtcNow -lt $deadline)
  if(-not [IO.File]::Exists($macro)){throw 'Recorded macro was not saved'}
  Record 'macro persisted' @{path=$macro;sha256=(Hash-File $macro)};$script:extraArtifacts.Add($macro)
  Focus-Editor;Key 83 $true;Expect-File $script:saved $script:utf8.GetBytes($script:macroFixture.final) 'macro recorded saved'
 }
 Step 's2' 'A fresh editor reloads the persisted macro and replays it against a reset owned fixture with the exact expected result.' {
  Close-OwnedEditor
  [IO.File]::WriteAllText($script:saved,$script:macroFixture.initial,$script:utf8)
  Start-OwnedEditor;Focus-Editor;Key 79 $true;File-Dialog $script:saved;Key 36 $true
  Regex-Menu 'Play Selected Macro';Expect-Text $script:macroFixture.final 'macro replayed result'
  Focus-Editor;Key 83 $true;Expect-File $script:saved $script:utf8.GetBytes($script:macroFixture.final) 'macro replay saved'
 }
 Step 's3' 'A confirmed direct command preserves literal argv, exposes a working output location and cancels its observed parent and descendant through the editor.' {
  Regex-Menu 'Load External Command Definition';File-Dialog (Join-Path $script:scratch 'external-command.toml')
  $deadline=[DateTime]::UtcNow.AddSeconds(5)
  do {
   Guard;try{$null=Regex-MenuItem 'Run Loaded External Command';break}catch{Start-Sleep -Milliseconds 50}
  }while([DateTime]::UtcNow -lt $deadline)
  Regex-Menu 'Run Loaded External Command';Confirm-FixtureCommand
  $receipt=Join-Path $script:scratch 'external-receipt.json';$deadline=[DateTime]::UtcNow.AddSeconds(5)
  do{Guard;if([IO.File]::Exists($receipt)){break};Start-Sleep -Milliseconds 50}while([DateTime]::UtcNow -lt $deadline)
  if(-not [IO.File]::Exists($receipt) -or (Get-Item -LiteralPath $receipt).Length -gt 16384){throw 'External fixture receipt missing/excessive'}
  $value=Get-Content -LiteralPath $receipt -Raw -Encoding UTF8 | ConvertFrom-Json
  $parent=Get-CimInstance Win32_Process -Filter ('ProcessId='+$value.pid)
  $child=Get-CimInstance Win32_Process -Filter ('ProcessId='+$value.child_pid)
  if($parent.ParentProcessId -ne $script:process.Id -or $child.ParentProcessId -ne $value.pid -or $parent.ExecutablePath -ine $script:macroFixture.python -or $child.ExecutablePath -ine $script:macroFixture.python){throw 'External process identities are outside the owned editor tree'}
  $parentHandle=[Diagnostics.Process]::GetProcessById($value.pid);$childHandle=[Diagnostics.Process]::GetProcessById($value.child_pid)
  try {
   Record 'external literal argv' $value;$script:extraArtifacts.Add($receipt)
   $link=Regex-Element ($script:saved+':2:1') 'ControlType.ListItem';Guard;$link.SetFocus();Key 13
   $deadline=[DateTime]::UtcNow.AddSeconds(5)
   do {Guard;$actual=Record-Element (Editor);if($actual.caret_utf16 -eq ($script:macroFixture.final.IndexOf("`n")+1)){break};Start-Sleep -Milliseconds 50}while([DateTime]::UtcNow -lt $deadline)
   Record 'external output navigation' $actual
   Regex-Menu 'Cancel External Command'
   $parentExit=$parentHandle.WaitForExit(5000);$childExit=$childHandle.WaitForExit(5000)
   Record 'external cancelled tree' @{parent_id=$value.pid;child_id=$value.child_pid;parent_exited=$parentExit;child_exited=$childExit}
   if(-not $parentExit -or -not $childExit){throw 'External cancellation left an owned process alive'}
  }finally{$parentHandle.Dispose();$childHandle.Dispose()}
 }
}
