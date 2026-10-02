# SPDX-License-Identifier: MPL-2.0
# Dot-sourced by the owned driver. Focused regressions for open manual-QA defects:
# ISSUE-005, ISSUE-008, U08, PR-T05 and ISSUE-030. Each step starts its own owned
# editor so one failure does not hide the others.
. (Join-Path $PSScriptRoot 'native_split.ps1')
. (Join-Path $PSScriptRoot 'native_huge_log.ps1')
$regressionClean=$true;$regressionFailed=$false
function Regression-Stop {
 if($script:process -and -not $script:process.HasExited) {
  # A passing step leaves no dirty document, so a clean Exit is expected. After
  # a failure, stop only the owned editor we launched.
  $closed=$false
  if($script:regressionClean){try{Close-OwnedEditor;$closed=$true}catch{Record 'regression close failure' @{error=$_.Exception.Message}}}
  if(-not $closed){$script:process.Kill();[void]$script:process.WaitForExit(5000)}
 }
}
function Regression-Fresh([string[]]$arguments=@('--software','--no-session','--no-extensions','--new-instance')) {
 Regression-Stop
 $script:editorPane=0
 Start-OwnedEditor $script:request.executable $arguments
}
function Regression-Step([string]$id,[string]$observation,[scriptblock]$action) {
 $script:failed=$false
 Step $id $observation $action
 $script:regressionClean=-not $script:failed
 if($script:failed){$script:regressionFailed=$true}
}
function Regression-FocusedIs($element) {
 $focused=[System.Windows.Automation.AutomationElement]::FocusedElement
 if($null -eq $focused -or $focused.Current.ProcessId -ne $script:process.Id){return $false}
 return [System.Windows.Automation.Automation]::Compare($focused,$element)
}
function Regression-FindFocus([string]$stage) {
 $deadline=[DateTime]::UtcNow.AddSeconds(3);$actual=$null
 do {
  Guard
  $field=Regex-Element 'Find' 'ControlType.Edit'
  $actual=Record-Element $field $true
  $actual | Add-Member -NotePropertyName focused_is_field -NotePropertyValue (Regression-FocusedIs $field)
  if($actual.focus -and $actual.focused_is_field -and $actual.text -ceq $script:regressionFixture.query){Record $stage $actual;return}
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 Record $stage $actual;throw 'Find field was not the focused UIA element with the typed value'
}
function Regression-EditorFocus([string]$stage) {
 # Observe only: Focus-Editor would move focus itself and hide the defect.
 $deadline=[DateTime]::UtcNow.AddSeconds(3);$actual=$null
 do {
  Guard;$editor=Editor;$actual=Record-Element $editor
  if($actual.focus -and (Regression-FocusedIs $editor)){Record $stage $actual;return}
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 Record $stage $actual;throw 'Editor did not regain UIA focus'
}
function Regression-Dialog([string]$stage) {
 $deadline=[DateTime]::UtcNow.AddSeconds(5);$dialog=[IntPtr]::Zero
 do {
  [JourneyInput]::Desktop();$foreground=[JourneyInput]::GetForegroundWindow()
  if($foreground -ne $script:window -and [JourneyInput]::Owner($foreground) -eq $script:process.Id){$dialog=$foreground;break}
  if($foreground -ne $script:window){throw 'Save prompt was not owned by the editor'}
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 if($dialog -eq [IntPtr]::Zero){throw 'Save prompt did not appear'}
 Guard $dialog
 $texts=@(Elements $dialog | ForEach-Object {$_.Current.Name} | Where-Object {$_})
 Record $stage @{owned=$true;dialog=$dialog.ToInt64();texts=$texts}
 if(@($texts | Where-Object {$_.StartsWith('Save changes to ')}).Count -ne 1){throw 'Save prompt text missing or ambiguous'}
 return $dialog
}
function Regression-WaitMain {
 $deadline=[DateTime]::UtcNow.AddSeconds(5)
 do {
  [JourneyInput]::Desktop();$foreground=[JourneyInput]::GetForegroundWindow()
  if($foreground -eq $script:window){Guard;return}
  if([JourneyInput]::Owner($foreground) -ne $script:process.Id){throw 'Lost foreground after the save prompt'}
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 throw 'Save prompt did not close'
}
function Regression-Menus([string]$stage) {
 Guard
 $menu=[JourneyInput]::GetMenu($script:window);$top=@();$disabled=@();$exit=@()
 for($i=0;$i -lt [JourneyInput]::GetMenuItemCount($menu);$i++) {
  $label=[JourneyInput]::Label($menu,$i);$top+=$label
  if([JourneyInput]::GetMenuState($menu,[uint32]$i,0x400) -band 3){$disabled+=$label}
  if($label -eq 'File') {
   $file=[JourneyInput]::GetSubMenu($menu,$i)
   for($j=0;$j -lt [JourneyInput]::GetMenuItemCount($file);$j++){if([JourneyInput]::Label($file,$j) -eq 'Exit'){$exit+=[JourneyInput]::GetMenuState($file,[uint32]$j,0x400)}}
  }
 }
 $value=@{window_enabled=[JourneyInput]::IsWindowEnabled($script:window);top_level=$top;disabled_top_level=$disabled;exit_enabled=($exit.Count -eq 1 -and -not ($exit[0] -band 3))}
 Record $stage $value
 if(-not $value.window_enabled -or $disabled.Count -ne 0 -or -not $value.exit_enabled -or $top.Count -eq 0){throw ('Window or menus disabled: '+$stage)}
}
function Regression-PaneFocus([int]$pane,[string]$stage) {
 $deadline=[DateTime]::UtcNow.AddSeconds(3);$panes=@()
 do {
  Guard;$panes=@();$focused=@()
  foreach($n in @(1,2)) {
   $items=@(Elements | Where-Object {$_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Edit -and $_.Current.Name.StartsWith(('Pane '+$n+', '))})
   if($items.Count -ne 1){throw ('Pane provider missing or ambiguous: '+$n)}
   $value=Record-Element $items[0]
   $value | Add-Member -NotePropertyName uia_focused -NotePropertyValue (Regression-FocusedIs $items[0])
   $panes+=,$value
   if($value.focus){$focused+=$n}
  }
  if($focused.Count -eq 1 -and $focused[0] -eq $pane -and $panes[$pane-1].uia_focused){Record $stage @{focused_pane=$pane;panes=$panes};return}
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 Record $stage @{focused_pane=$(if($focused.Count -eq 1){$focused[0]}else{$null});expected_pane=$pane;panes=$panes}
 throw ('Split-pane focus differs: '+$stage)
}
function Regression-Trace {
 $path=Join-Path $script:scratch 'command-trace.jsonl';$rows=@()
 if([IO.File]::Exists($path)) {
  $stream=[IO.File]::Open($path,[IO.FileMode]::Open,[IO.FileAccess]::Read,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
  $reader=[IO.StreamReader]::new($stream)
  try{$text=$reader.ReadToEnd()}finally{$reader.Dispose()}
  if($text.Length -gt 65536){throw 'Close command trace exceeds bound'}
  foreach($line in ($text -split "`r?`n")){if($line){try{$rows+=($line | ConvertFrom-Json -ErrorAction Stop)}catch{}}}
 }
 ,$rows
}
function Regression-BusyPrefix {
 $stream=[IO.File]::Open($script:busy,[IO.FileMode]::Open,[IO.FileAccess]::Read,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
 try {
  $buffer=New-Object byte[] 256;$count=$stream.Read($buffer,0,$buffer.Length)
  $prefix=$script:utf8.GetString($buffer,0,$count);$line=$prefix.IndexOf("`n")
  @{bytes=$stream.Length;prefix=$(if($line -ge 0){$prefix.Substring(0,$line+1)}else{$prefix})}
 } finally {$stream.Dispose()}
}
function Run-Regressions {
 Regression-Step 's1' 'ISSUE-005: Ctrl+F moves UIA focus to the Find field, which reports the typed value; the match count is announced and Escape returns focus to the Editor.' {
  Regression-Fresh
  Focus-Editor;Key 79 $true;File-Dialog $script:saved
  Expect-Text $script:regressionFixture.initial 'find source opened'
  Key 70 $true;Text $script:regressionFixture.query
  Regression-FindFocus 'find focus announced'
  $null=Regex-Status $script:regressionFixture.find_status 'find count announced'
  Key 27;Regression-EditorFocus 'find escape editor focus'
 }
 Regression-Step 's2' 'ISSUE-008: Ctrl+W on a dirty Untitled shows an owned save prompt; after Cancel and after Don''t Save the window, every top-level menu and File > Exit stay enabled.' {
  Regression-Fresh
  Focus-Editor;Expect-Text '' 'untitled empty'
  Text $script:regressionFixture.untitled
  Expect-Text $script:regressionFixture.untitled 'untitled typed';Expect-Dirty $true 'untitled dirty'
  Key 87 $true;$dialog=Regression-Dialog 'close prompt cancel'
  [JourneyInput]::Key($dialog,27,$false,$false);Regression-WaitMain
  Regression-Menus 'menus after cancel'
  Expect-Text $script:regressionFixture.untitled 'untitled kept after cancel';Expect-Dirty $true 'untitled still dirty'
  Key 87 $true;$dialog=Regression-Dialog 'close prompt discard'
  # Alt+N is the Don't Save mnemonic ("Do&n't Save").
  [JourneyInput]::ModifiedKey($dialog,78,$false,$false,$true);Regression-WaitMain
  Regression-Menus 'menus after discard'
  $deadline=[DateTime]::UtcNow.AddSeconds(5)
  do {
   Guard;$modified=@(Elements | Where-Object {$_.Current.ControlType -eq [System.Windows.Automation.ControlType]::TabItem -and $_.Current.Name.EndsWith(', modified')} | ForEach-Object {$_.Current.Name})
   if($modified.Count -eq 0){break};Start-Sleep -Milliseconds 50
  } while([DateTime]::UtcNow -lt $deadline)
  Record 'untitled discarded' @{modified_tabs=$modified}
  if($modified.Count -ne 0){throw 'Don''t Save left a modified tab'}
 }
 Regression-Step 's3' 'U08: after Clone and each F6 exactly one pane provider has keyboard and UIA focus; typing lands at that pane''s own caret and both panes show the shared text.' {
  Regression-Fresh
  Focus-Editor;Key 79 $true;File-Dialog $script:saved
  Expect-Text $script:regressionFixture.initial 'split source opened'
  Regex-Menu 'Clone to Other View'
  Observe-Panes $script:regressionFixture.initial 'split source cloned'
  $script:editorPane=1;Focus-Editor;Key 35 $true
  $script:editorPane=2;Focus-Editor;Key 36 $true
  Regression-PaneFocus 2 'pane 2 focused'
  Key 117;Regression-PaneFocus 1 'F6 to pane 1'
  Key 117;Regression-PaneFocus 2 'F6 to pane 2'
  Text 'A';Observe-Panes ('A'+$script:regressionFixture.initial) 'pane 2 typed at its caret'
  Key 117;Regression-PaneFocus 1 'F6 back to pane 1'
  Text 'Z';Observe-Panes $script:regressionFixture.split_edited 'pane 1 typed at its caret'
  Key 90 $true;Key 90 $true;Observe-Panes $script:regressionFixture.initial 'split edits undone'
  Regex-Menu 'Close Split View';$script:editorPane=0
  Focus-Editor;Expect-Text $script:regressionFixture.initial 'split collapsed';Expect-Dirty $false 'split clean'
 }
 Regression-Step 's4' 'PR-T05: Ctrl+W during a running paged save is deferred as busy and completes once after the save without a prompt; the saved bytes include the edit.' {
  Regression-Fresh
  Focus-Editor;Key 79 $true;File-Dialog $script:busy
  Log-View $script:regressionFixture.busy_line 'busy initial viewport'
  Focus-Editor;Key 36 $true;Text $script:regressionFixture.busy_edit
  Log-View ($script:regressionFixture.busy_edit+$script:regressionFixture.busy_line) 'busy edited viewport'
  Expect-Dirty $true 'busy dirty'
  Key 83 $true;Key 87 $true
  $deadline=[DateTime]::UtcNow.AddSeconds(90);$tabs=@()
  do {
   [JourneyInput]::Desktop();$foreground=[JourneyInput]::GetForegroundWindow()
   if($foreground -ne $script:window -and [JourneyInput]::Owner($foreground) -eq $script:process.Id){throw 'A prompt appeared for the busy close'}
   Guard
   $tabs=@(Elements | Where-Object {$_.Current.ControlType -eq [System.Windows.Automation.ControlType]::TabItem -and $_.Current.Name.Contains('busy-document.txt')} | ForEach-Object {$_.Current.Name})
   if($tabs.Count -eq 0){break};Start-Sleep -Milliseconds 100
  } while([DateTime]::UtcNow -lt $deadline)
  Record 'busy close completed' @{busy_tabs=$tabs.Count;tabs=$tabs}
  if($tabs.Count -ne 0){throw 'Busy document tab remained open after the save'}
  $trace=Regression-Trace
  Record 'busy close trace' @{records=$trace}
  $queued=@($trace | Where-Object {$_.event -eq 'qa_close_command' -and $_.stage -eq 'queued' -and $_.detail -eq 'document'})
  $deferred=@($trace | Where-Object {$_.event -eq 'qa_close_command' -and $_.stage -eq 'deferred' -and $_.detail -eq 'document-busy'})
  if($queued.Count -eq 0){throw 'Close command trace missing'}
  if($deferred.Count -eq 0){throw 'Busy precondition not established: the save completed before Close'}
  $value=Regression-BusyPrefix;Record 'busy saved prefix' $value
  if($value.prefix -cne ($script:regressionFixture.busy_edit+$script:regressionFixture.busy_line) -or $value.bytes -ne ($script:regressionFixture.busy_bytes+$script:regressionFixture.busy_edit.Length)){throw 'Busy save bytes differ'}
 }
 Regression-Step 's5' 'ISSUE-030: after a clean Exit with session restore enabled, each relaunch shows a visible, non-minimized window with a first frame and restores the document.' {
  # Earlier steps save a session on Exit too; start this one from the fixture only.
  $session=Join-Path $script:profile 'session.json'
  Regression-Stop
  if([IO.File]::Exists($session)){Remove-Item -LiteralPath $session}
  Regression-Fresh @('--software','--no-extensions','--new-instance')
  Focus-Editor;Key 79 $true;File-Dialog $script:saved
  Expect-Text $script:regressionFixture.initial 'session source opened'
  Close-OwnedEditor
  if(-not [IO.File]::Exists($session)){throw 'Session was not saved on a clean Exit'}
  for($launch=1;$launch -le $script:regressionFixture.relaunches;$launch++) {
   if($launch -gt 1){Close-OwnedEditor}
   # -RequireShown finds the window whether or not it is visible, so a window that
   # renders but never shows fails here as a product defect, not a startup timeout.
   Start-OwnedEditor $script:request.executable @('--software','--no-extensions','--new-instance') 'Normal' -RequireShown
   Record ('relaunch '+$launch+' window shown') $script:launchWindow
   if($script:launchWindow.discovery -cne 'any-visibility' -or -not $script:launchWindow.visible -or $script:launchWindow.iconic){throw 'Relaunched window was not shown'}
   Focus-Editor;Expect-Text $script:regressionFixture.initial ('relaunch '+$launch+' restored text')
  }
 }
 $script:failed=$script:regressionFailed
}
