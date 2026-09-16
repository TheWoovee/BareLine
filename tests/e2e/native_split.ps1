# SPDX-License-Identifier: MPL-2.0
. (Join-Path $PSScriptRoot 'native_regex_transform.ps1')
function Observe-Panes([string]$expected,[string]$stage,[bool]$sync=$false) {
 $deadline=[DateTime]::UtcNow.AddSeconds(5);$panes=@()
 do {
  Guard;$panes=@();$ready=$true
  foreach($n in @(1,2)) {
   $items=@(Elements | Where-Object {$_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Edit -and $_.Current.Name.StartsWith(('Pane '+$n+', '))})
   if($items.Count -ne 1){$ready=$false;break}
   $value=Record-Element $items[0]
   $pattern=$items[0].GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
   $visible=@($pattern.GetVisibleRanges())
   if($visible.Count -lt 1 -or $visible.Count -gt 128){throw 'Pane visible ranges missing/excessive'}
   $value | Add-Member -NotePropertyName visible -NotePropertyValue $visible[0].GetText(8192)
   $panes+=,$value
   if($value.text -cne $expected){$ready=$false}
  }
  if($ready -and $sync){$ready=($panes[0].focus -and -not $panes[1].focus -and (($panes[0].visible -split "`n")[0] -ceq ($panes[1].visible -split "`n")[0]))}
  if($ready){Record $stage @{panes=$panes};return}
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 Record $stage @{panes=$panes};throw ('Pane observation failed: '+$stage)
}
function Run-Split {
 Step 's1' 'Clone to Other View publishes two distinct pane providers showing the exact same Unicode fixture.' {
  Focus-Editor;Key 79 $true;File-Dialog $script:saved
  Expect-Text $script:splitFixture.initial 'split opened'
  Regex-Menu 'Clone to Other View'
  Observe-Panes $script:splitFixture.initial 'split cloned'
 }
 Step 's2' 'Editing and one Undo from each pane update both views exactly; the saved fixture stays unchanged.' {
  $script:editorPane=1;Focus-Editor;Key 35 $true;Text 'PRIMARY'
  Observe-Panes $script:splitFixture.first 'split primary edit'
  Key 90 $true;Observe-Panes $script:splitFixture.initial 'split primary Undo'
  $script:editorPane=2;Focus-Editor;Key 35 $true;Text 'SECONDARY'
  Observe-Panes $script:splitFixture.second 'split secondary edit'
  Key 90 $true;Observe-Panes $script:splitFixture.initial 'split secondary Undo'
  Expect-File $script:saved $script:utf8.GetBytes($script:splitFixture.initial) 'split unchanged disk'
 }
 Step 's3' 'Synchronized vertical scrolling changes the visible global row in both panes while primary editor focus remains unchanged.' {
  Regex-Menu 'Synchronize Vertical Scrolling'
  $menu=Regex-MenuItem 'Synchronize Vertical Scrolling';$menu.checked=($menu.state -band 8) -ne 0
  Record 'split sync menu' $menu
  if(-not $menu.checked){throw 'Vertical synchronization is not checked'}
  $script:editorPane=1;Focus-Editor;Key 36 $true
  Observe-Panes $script:splitFixture.initial 'split scroll before' $true
  Key 34
  Observe-Panes $script:splitFixture.initial 'split scroll after' $true
  $frame=[JourneyFrame]::Capture($script:window)
  try{$frame.Save((Join-Path $script:scratch 'split-synchronized.png'))}finally{$frame.Dispose()}
  $script:extraArtifacts.Add((Join-Path $script:scratch 'split-synchronized.png'))
  Regex-Menu 'Close Split View';$script:editorPane=0
  Focus-Editor;Expect-Text $script:splitFixture.initial 'split collapsed'
 }
}
