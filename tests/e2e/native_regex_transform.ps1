# SPDX-License-Identifier: MPL-2.0
# Dot-sourced by the owned driver; all actions retain its foreground/desktop guards.
Add-Type -AssemblyName System.Drawing
if(-not ('JourneyFrame' -as [type])){Add-Type -Path (Join-Path $PSScriptRoot 'native_visual.cs') -ReferencedAssemblies System.Drawing,System,System.Core}

function Regex-MenuItem([string]$label) {
 Guard
 $queue=[Collections.Generic.Queue[IntPtr]]::new();$queue.Enqueue([JourneyInput]::GetMenu($script:window))
 $matches=@();$visited=0
 while($queue.Count -gt 0) {
  $menu=$queue.Dequeue()
  for($i=0;$i -lt [JourneyInput]::GetMenuItemCount($menu);$i++) {
   $visited++;if($visited -gt 1024){throw 'Native menu discovery exceeded bound'}
   $child=[JourneyInput]::GetSubMenu($menu,$i)
   if($child -ne [IntPtr]::Zero){$queue.Enqueue($child)}
   if([JourneyInput]::Label($menu,$i) -ceq $label){$matches+=@{label=$label;id=[JourneyInput]::GetMenuItemID($menu,$i);state=[JourneyInput]::GetMenuState($menu,[uint32]$i,0x400)}}
  }
 }
 if($matches.Count -ne 1 -or ($matches[0].state -band 3)){throw "Native command missing, ambiguous or disabled: $label"}
 Guard;return $matches[0]
}
function Regex-Menu([string]$label) {
 $item=Regex-MenuItem $label
 Record 'regex native command' $item
 if(-not [JourneyInput]::PostMessageW($script:window,0x111,[UIntPtr]$item.id,[IntPtr]::Zero)){throw "Command submission failed: $label"}
}
function Regex-Element([string]$name,[string]$type) {
 $deadline=[DateTime]::UtcNow.AddSeconds(5)
 do {
  Guard
  $matches=@(Elements | Where-Object {$_.Current.Name -ceq $name -and $_.Current.ControlType.ProgrammaticName -ceq $type})
  if($matches.Count -eq 1){return $matches[0]}
  if($matches.Count -gt 1){throw "Ambiguous regex control: $name"}
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 Record 'regex control missing' (Regex-Snapshot);throw "Regex control missing: $name"
}
function Regex-Snapshot {
 @(Elements | ForEach-Object {Record-Element $_ ($_.Current.Name -in @('Find','Replace with'))})
}
function Regex-Field([string]$name,[string]$value,[string]$stage) {
 $field=Regex-Element $name 'ControlType.Edit'
 Guard;$field.SetFocus()
 $deadline=[DateTime]::UtcNow.AddSeconds(3)
 do {
  Guard
  $field=Regex-Element $name 'ControlType.Edit'
  if($field.Current.HasKeyboardFocus){break}
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 if(-not $field.Current.HasKeyboardFocus){throw "Regex field focus unavailable: $name"}
 Key 65 $true;Text $value
 $deadline=[DateTime]::UtcNow.AddSeconds(3)
 do {
  Guard;$actual=Record-Element (Regex-Element $name 'ControlType.Edit') $true
  if($actual.text -ceq $value -and $actual.focus){Record $stage $actual;return}
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 Record $stage $actual;throw "Exact regex field input mismatch: $name"
}
function Regex-Status([string]$value,[string]$stage,[bool]$prefix=$false) {
 $deadline=[DateTime]::UtcNow.AddSeconds(8)
 do {
  Guard
  $matches=@(Elements | Where-Object {if($prefix){$_.Current.Name.StartsWith($value,[StringComparison]::Ordinal)}else{$_.Current.Name -ceq $value}})
  if($matches.Count -eq 1){$actual=Record-Element $matches[0];Record $stage $actual;return $actual}
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 Record 'regex status mismatch' (Regex-Snapshot);throw "Expected regex status was not observed: $value"
}
function Regex-Preview {
 $status=Regex-Status $script:regexFixture.preview_status 'regex preview ready'
 # Construct the selected marker without relying on the script file encoding.
 $expected=@(('['+[char]0x2713+'] '+('Open document: '+$script:saved).Substring(0,[Math]::Min(200,('Open document: '+$script:saved).Length))))+@($script:regexFixture.preview_rows)
 $rows=@()
 foreach($label in $expected){$rows+=Record-Element (Regex-Element $label 'ControlType.ListItem')}
 $all=@(Elements | Where-Object {$_.Current.ControlType -eq [System.Windows.Automation.ControlType]::ListItem -and $_.Current.Name.StartsWith('['+[char]0x2713+'] ')})
 if($all.Count -ne 3){throw 'Unexpected extra/missing selected preview rows'}
 Record 'regex capture preview' @{status=$status;rows=$rows}
 Guard;$frame=[JourneyFrame]::Capture($script:window)
 try {$frame.Save((Join-Path $script:scratch 'regex-preview.png'))} finally {$frame.Dispose()}
}
function Run-RegexTransform {
 Step 's1' 'Generated multiline Unicode source opens unchanged; native regex mode, exact PCRE2 query and complete two-match count are observed.' {
  Focus-Editor;Key 79 $true;File-Dialog $script:saved
  Expect-Text $script:regexFixture.initial 'regex opened exact text';Expect-Dirty $false 'regex opened clean'
  Regex-Menu 'Regular Expression Search Mode'
  $null=Regex-Element 'Find' 'ControlType.Edit'
  Regex-Menu 'Replace'
  $null=Regex-Element 'Replace with' 'ControlType.Edit'
  Regex-Field 'Find' $script:regexFixture.pattern 'regex query input'
  $mode=Regex-MenuItem 'Regular Expression Search Mode'
  if(-not ($mode.state -band 8)){throw 'Native Regular Expression Search Mode menu is not checked'}
  $mode.checked=$true;Record 'regex mode' $mode
  $null=Regex-Status 'Find results: 2 matches' 'regex match count'
  Focus-Editor
  Expect-Text $script:regexFixture.initial 'regex setup preserved text'
  Expect-File $script:saved $script:utf8.GetBytes($script:regexFixture.initial) 'regex setup preserved bytes'
 }
 Step 's2' 'Read-only preview expands named and numbered captures for exactly two multiline matches; Apply changes one open document, native Save writes exact expected UTF-8/LF bytes.' {
  Regex-Field 'Replace with' $script:regexFixture.replacement 'regex replacement input'
  Regex-Menu 'Replace in Files';File-Dialog (Split-Path $script:saved) $true
  Regex-Preview
  Expect-Text $script:regexFixture.initial 'regex preview preserved text'
  Expect-File $script:saved $script:utf8.GetBytes($script:regexFixture.initial) 'regex preview preserved bytes'
  Regex-Menu 'Apply Reviewed Replacements'
  $null=Regex-Status $script:regexFixture.applied_prefix 'regex replacement count' $true
  Regex-Menu 'Close Replacement Preview'
  Key 27;Focus-Editor
  Expect-Text $script:regexFixture.replaced 'regex replaced exact text';Expect-Dirty $true 'regex replaced dirty'
  Expect-File $script:saved $script:utf8.GetBytes($script:regexFixture.initial) 'regex unsaved replacement preserved disk'
  Key 83 $true
  Expect-File $script:saved $script:utf8.GetBytes($script:regexFixture.replaced) 'regex saved replacement bytes';Expect-Dirty $false 'regex replaced saved clean'
  Copy-Item -LiteralPath $script:saved -Destination (Join-Path $script:scratch 'regex-replaced.bin')
 }
 Step 's3' 'One native Undo restores the entire original document; disk retains the replacement until Save then exactly restores original Unicode and LF bytes.' {
  Focus-Editor;Key 90 $true
  Expect-Text $script:regexFixture.initial 'regex single Undo';Expect-Dirty $true 'regex Undo dirty'
  Expect-File $script:saved $script:utf8.GetBytes($script:regexFixture.replaced) 'regex Undo before save preserved disk'
  Key 83 $true
  Expect-File $script:saved $script:utf8.GetBytes($script:regexFixture.initial) 'regex restored original bytes';Expect-Dirty $false 'regex restored clean'
 }
}
