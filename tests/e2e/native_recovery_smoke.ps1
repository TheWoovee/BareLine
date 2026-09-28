# SPDX-License-Identifier: MPL-2.0
# Process-crash recovery only; no save-boundary injection or installation.
param([Parameter(Mandatory=$true)][string]$RequestPath)
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'native_driver.ps1') -RequestPath $RequestPath -LibraryOnly
. (Join-Path $PSScriptRoot 'native_session.ps1')
. (Join-Path $PSScriptRoot 'native_regex_transform.ps1')
. (Join-Path $PSScriptRoot 'native_lab.ps1')
$request=Get-Content -LiteralPath $RequestPath -Raw -Encoding UTF8 | ConvertFrom-Json
$scratch=$request.scratch;$utf8=[Text.UTF8Encoding]::new($false)
$timer=[Diagnostics.Stopwatch]::StartNew();$records=[Collections.Generic.List[object]]::new()
$extraArtifacts=[Collections.Generic.List[string]]::new()
$process=$null;$window=[IntPtr]::Zero;$launchNumber=0;$programNumber=0;$probeNumber=0
$failure=$null;$originalPid=$null;$recoveryPid=$null;$durable=0;$restored=0;$cleanExit=$false
try {
 if((Hash-File $request.recovery_probe) -cne $request.probe_sha256){throw 'Recovery probe hash differs'}
 $local=Join-Path $scratch 'local';$profile=Join-Path $local 'Bareline'
 $roaming=Join-Path $scratch 'roaming';$temp=Join-Path $scratch 'temp'
 foreach($directory in @($profile,$roaming,$temp)){[IO.Directory]::CreateDirectory($directory)|Out-Null}
 [IO.File]::WriteAllText((Join-Path $profile 'settings.toml'),"schema_version=1`n[theme]`nmode='dark'`n[files]`ndefault_eol='lf'`nautosave_seconds=0`n",$utf8)
 [IO.File]::WriteAllBytes((Join-Path $profile '.bareline-diagnostic'),[byte[]]@())
 $env:LOCALAPPDATA=$local;$env:APPDATA=$roaming;$env:TEMP=$temp;$env:TMP=$temp
 $saved='saved recovery document';$untitled='Untitled recovery '+[char]0x6587+[char]0x00E9
 $lab=@{paths=@{recovery_probe=$request.recovery_probe};saved_hex=([BitConverter]::ToString($utf8.GetBytes($saved))).Replace('-','');saved_text=$saved+'!';untitled=$untitled}
 $arguments=@('--software','--no-session','--no-extensions','--new-instance','--diag=handles','--diagnostic-root',('"'+$profile+'"'))
 Start-OwnedEditor $request.executable $arguments
 $originalPid=$process.Id
 Lab-CrashPrepare;$durable=2
 # Both exact edits were read from checkpoint copies before killing this PID.
 Guard
 $created=$process.StartTime.ToUniversalTime().ToString('o')
 $process.Kill();if(-not $process.WaitForExit(5000)){throw 'Owned process did not terminate'}
 Record 'owned process crash' @{pid=$originalPid;created_utc=$created;durable_documents=$durable}
 Start-OwnedEditor $request.executable $arguments
 $recoveryPid=$process.Id
 if($recoveryPid -eq $originalPid){throw 'Recovery process identity was reused'}
 Lab-Restore 'crash-saved.txt;' $lab.saved_text ($utf8.GetBytes($lab.saved_text)) 'saved'
 $restored++
 Lab-Restore 'not yet saved to disk;' $lab.untitled ($utf8.GetBytes($lab.untitled)) 'untitled'
 $restored++
 Expect-File $crashSaved ($utf8.GetBytes($saved)) 'original saved file remains unchanged'
 Close-OwnedEditor;$cleanExit=$true
}catch{
 $failure=$_.Exception.Message
 Record 'recovery smoke failure' @{error=$failure;stack=$_.ScriptStackTrace}
}finally{
 Write-Json (Join-Path $scratch 'driver-result.json') @{schema_version=1;status=$(if($failure){'FAIL'}else{'PASS'});failure=$failure;
  original_pid=$originalPid;recovery_pid=$recoveryPid;durable_documents=$durable;restored_documents=$restored;clean_exit=$cleanExit}
}
if($failure){exit 1}
