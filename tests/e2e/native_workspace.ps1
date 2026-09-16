# SPDX-License-Identifier: MPL-2.0
. (Join-Path $PSScriptRoot 'native_regex_transform.ps1')
function Workspace-Focus([string]$name) {
 $element=Regex-Element $name 'ControlType.TreeItem';Guard;$element.SetFocus()
 $deadline=[DateTime]::UtcNow.AddSeconds(3)
 do {
  Guard;$element=Regex-Element $name 'ControlType.TreeItem'
  if($element.Current.HasKeyboardFocus){Record ('workspace focus '+$name) (Record-Element $element);return}
  Start-Sleep -Milliseconds 50
 }while([DateTime]::UtcNow -lt $deadline)
 throw ('Workspace row did not acquire focus: '+$name)
}
function Workspace-Tree([string[]]$expected,[string]$stage) {
 $deadline=[DateTime]::UtcNow.AddSeconds(5)
 do {
  Guard
  $names=@(Elements | Where-Object {$_.Current.ControlType -eq [System.Windows.Automation.ControlType]::TreeItem -and -not $_.Current.Name.StartsWith('function ')} | ForEach-Object {$_.Current.Name} | Sort-Object)
  if(($names -join '|') -ceq ($expected -join '|')){Record $stage @{names=$names};return}
  Start-Sleep -Milliseconds 50
 }while([DateTime]::UtcNow -lt $deadline)
 Record $stage @{names=$names};throw 'Workspace rows differ from isolated fixture'
}
function Workspace-Target([string]$stage) {
 Workspace-Focus 'function target';Key 13
 $deadline=[DateTime]::UtcNow.AddSeconds(5)
 do {
  Guard;$actual=Record-Element (Editor)
  if($actual.text -ceq $script:workspaceFixture.initial -and $actual.caret_utf16 -eq $script:workspaceFixture.target_offset){Record $stage $actual;return}
  Start-Sleep -Milliseconds 50
 }while([DateTime]::UtcNow -lt $deadline)
 Record $stage $actual;throw 'Outline target offset differs'
}
function Run-Workspace {
 Step 's1' 'An explicitly chosen scratch workspace expands to exactly its two generated files.' {
  Regex-Menu 'Open Workspace Folder';File-Dialog $script:workspaceRoot $true
  Workspace-Focus 'workspace-fixture';Key 39
  Workspace-Tree @('main.rs','notes.txt','workspace-fixture') 'workspace tree'
 }
 Step 's2' 'Tree activation opens the exact Rust fixture; outline activation selects the target declaration offset.' {
  Workspace-Focus 'main.rs';Key 13
  Expect-Text $script:workspaceFixture.initial 'workspace opened'
  Regex-Menu 'Toggle Outline';Workspace-Target 'workspace target'
 }
 Step 's3' 'Native rename of the closed scratch entry refreshes the tree; reopening selects the same exact declaration under its new path.' {
  Focus-Editor;Key 87 $true
  Workspace-Focus 'main.rs'
  $old=$script:saved;$script:saved=Join-Path $script:workspaceRoot 'renamed.rs'
  Regex-Menu 'Rename Selected Entry';File-Dialog $script:saved
  Regex-Menu 'Refresh Workspace'
  Workspace-Tree @('notes.txt','renamed.rs','workspace-fixture') 'workspace renamed tree'
  Workspace-Focus 'renamed.rs';Key 13
  Expect-Text $script:workspaceFixture.initial 'workspace renamed opened'
  Workspace-Target 'workspace renamed target'
  Expect-File $script:saved $script:utf8.GetBytes($script:workspaceFixture.initial) 'workspace renamed bytes'
  Record 'workspace rename paths' @{old_exists=[IO.File]::Exists($old);new_path=$script:saved;sha256=(Hash-File $script:saved)}
 }
}
