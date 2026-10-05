# SPDX-License-Identifier: MPL-2.0
# Headless readiness checks. No editor, input injection, or real window calls.
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'native_session.ps1')
Add-Type @'
using System;
public static class JourneyInput {
 public static uint WindowOwner=123;
 public static int DesktopChecks=0;
 public static void Desktop(){DesktopChecks++;}
 public static uint Owner(IntPtr window){return WindowOwner;}
}
'@
$temporaryRoot=[IO.Path]::GetTempPath()
$scratch=[IO.Path]::GetFullPath((Join-Path $temporaryRoot ('bareline-startup-test-'+[Guid]::NewGuid().ToString('N'))))
if(-not $scratch.StartsWith([IO.Path]::GetFullPath($temporaryRoot),[StringComparison]::OrdinalIgnoreCase)){throw 'Unsafe scratch path'}
[IO.Directory]::CreateDirectory($scratch)|Out-Null
$stdout=Join-Path $scratch 'stdout.log';$stderr=Join-Path $scratch 'stderr.log'
$valid='{"event":"first_frame","microseconds":650764,"software":true}'
$records=[Collections.Generic.List[object]]::new()
function Record([string]$stage,$details){$script:records.Add(@{stage=$stage;details=$details})}
function Must-Fail([scriptblock]$action,[string]$reason){
 $failed=$false
 try{& $action}catch{if(-not $_.Exception.Message.Contains($reason)){throw};$failed=$true}
 if(-not $failed){throw "Expected rejection: $reason"}
}
function Fake-Process([int]$publishAfter=0) {
 $value=[pscustomobject]@{Id=123;HasExited=$false;MainWindowHandle=[IntPtr]1;Refreshes=0;PublishAfter=$publishAfter}
 $value | Add-Member -MemberType ScriptMethod -Name Refresh -Value {
  $this.Refreshes++
  if($this.PublishAfter -gt 0 -and $this.Refreshes -eq $this.PublishAfter){[IO.File]::WriteAllText($script:stdout,$script:valid)}
 }
 return $value
}
try {
 if($null -ne (Read-OwnedFirstFrame $stdout)){throw 'Missing first frame was accepted'}
 foreach($text in @('{"event":"first_frame"','{"event":"other","microseconds":2,"software":true}','{"event":"first_frame","microseconds":true,"software":true}')) {
  [IO.File]::WriteAllText($stdout,$text)
  if($null -ne (Read-OwnedFirstFrame $stdout)){throw 'Incomplete/wrong first frame was accepted'}
 }
 [IO.File]::WriteAllText($stdout,$valid)
 if((Read-OwnedFirstFrame $stdout).microseconds -ne 650764){throw 'Complete first frame was rejected'}
 # Start-Process keeps redirected output open for writing during startup.
 $writer=[IO.File]::Open($stdout,[IO.FileMode]::Open,[IO.FileAccess]::Write,[IO.FileShare]::ReadWrite)
 try {
  if((Read-OwnedFirstFrame $stdout).microseconds -ne 650764){throw 'First frame with an active log writer was rejected'}
 } finally {$writer.Dispose()}
 [IO.File]::WriteAllText($stdout,'')
 $process=Fake-Process 3
 Wait-OwnedWindowReady $stdout $stderr 1000
 if($process.Refreshes -ne 3 -or $records.Count -ne 1){throw 'Window was considered ready before the actual first frame'}
 [IO.File]::WriteAllText($stdout,'')
 $process=Fake-Process
 Must-Fail {Wait-OwnedWindowReady $stdout $stderr 50} 'startup deadline'
 $process.HasExited=$true
 Must-Fail {Wait-OwnedWindowReady $stdout $stderr 50} 'exited before rendering'
 $process=Fake-Process;[JourneyInput]::WindowOwner=999
 [IO.File]::WriteAllText($stdout,$valid)
 Must-Fail {Wait-OwnedWindowReady $stdout $stderr 50} 'identity differs'
 [IO.File]::WriteAllText($stdout,('x'*262145))
 Must-Fail {Read-OwnedFirstFrame $stdout} 'exceeds bound'
 # Load only the real observation loop; no UIA initialization or input helpers.
 $tokens=$null;$parseErrors=$null
 $driver=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'native_driver.ps1'),[ref]$tokens,[ref]$parseErrors)
 if($parseErrors.Count){throw 'Native driver parse error'}
 $observe=$driver.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Expect-Text'},$false)
 if(-not $observe){throw 'Text observation loop missing'}
 . ([scriptblock]::Create($observe.Extent.Text))
 function Guard { $script:guardReads++;if($script:loseFocus -and $script:guardReads -gt 1){throw 'Lost foreground'} }
 function Editor {return 'Synthetic editor'}
 function Record-Element($element) {
  $script:textReads++
  if($script:textReads -eq 1){throw [Runtime.InteropServices.COMException]::new('Synthetic provider pending',$script:readHresult)}
  return @{text=$script:expectedText}
 }
 foreach($expectedText in @('restored text','')) {
  $textReads=0;$guardReads=0;$loseFocus=$false;$readHresult=-2147483638
  Expect-Text $expectedText 'synthetic pending text'
  if($textReads -ne 2 -or $guardReads -ne 2){throw 'Pending text was accepted without a guarded fresh observation'}
 }
 $textReads=0;$guardReads=0;$readHresult=-2147467259
 Must-Fail {Expect-Text 'text' 'synthetic fatal error'} 'Synthetic provider pending'
 if($textReads -ne 1){throw 'Non-pending error was retried'}
 $textReads=0;$guardReads=0;$readHresult=-2147483638;$loseFocus=$true
 Must-Fail {Expect-Text 'text' 'synthetic focus loss'} 'Lost foreground'
 if($textReads -ne 1){throw 'Text observation continued after focus loss'}
 Write-Output 'Native startup readiness checks passed.'
}finally{
 $resolved=[IO.Path]::GetFullPath($scratch)
 if($resolved -ne $scratch -or -not $resolved.StartsWith([IO.Path]::GetFullPath($temporaryRoot),[StringComparison]::OrdinalIgnoreCase)){throw 'Unsafe cleanup path'}
 Remove-Item -LiteralPath $resolved -Recurse -Force
}
