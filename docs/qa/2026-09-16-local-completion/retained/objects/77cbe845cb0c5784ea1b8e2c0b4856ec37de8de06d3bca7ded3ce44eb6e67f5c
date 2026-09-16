# SPDX-License-Identifier: MPL-2.0
param([Parameter(Mandatory=$true)][string]$RequestPath)
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'native_driver.ps1') -RequestPath $RequestPath -LibraryOnly
. (Join-Path $PSScriptRoot 'native_session.ps1')
. (Join-Path $PSScriptRoot 'native_lab.ps1')
$request=Get-Content -LiteralPath $RequestPath -Raw -Encoding UTF8 | ConvertFrom-Json
$scratch=$request.scratch;$lab=Get-Content -LiteralPath (Join-Path $scratch 'lab-fixture.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$timer=[Diagnostics.Stopwatch]::StartNew();$records=[Collections.Generic.List[object]]::new();$steps=[Collections.Generic.List[object]]::new()
$extraArtifacts=[Collections.Generic.List[string]]::new();$process=$null;$window=[IntPtr]::Zero;$failed=$false
$launchNumber=0;$programNumber=0;$probeNumber=0;$utf8=[Text.UTF8Encoding]::new($false)
$cleanup=@{editor_exited=$false;editor_exit_code=$null}
try {
 Lab-VM
 [JourneyInput]::Desktop()
 if([JourneyInput]::HighContrast()){throw 'High contrast does not match the requested light/dark lab cell'}
 $local=Join-Path $scratch 'local';$profile=Join-Path $local 'Bareline';$roaming=Join-Path $scratch 'roaming';$temp=Join-Path $scratch 'temp'
 foreach($path in @($profile,$roaming,$temp)){[IO.Directory]::CreateDirectory($path)|Out-Null}
 $env:LOCALAPPDATA=$local;$env:APPDATA=$roaming;$env:TEMP=$temp;$env:TMP=$temp
 $env:BARELINE_QA_SAVE_ARM=Join-Path $scratch 'save.arm'
 [IO.File]::WriteAllText((Join-Path $profile 'settings.toml'),("schema_version=1`n[theme]`nmode='"+$request.theme+"'`n[files]`ndefault_eol='lf'`nautosave_seconds=0`n"),$utf8)
 if($request.journey.id -ne 'install_update_rollback'){Start-OwnedEditor $request.executable @('--software')}
 switch($request.journey.id){
  'crash_recovery' {Run-CrashRecovery}
  'extension_isolation' {Run-ExtensionIsolation}
  'install_update_rollback' {Run-InstallUpdate}
  default {throw 'Unknown lab journey'}
 }
}catch{$failed=$true;Record 'lab failure' @{error=$_.Exception.Message;stack=$_.ScriptStackTrace}}
finally {
 if($process -and -not $process.HasExited -and -not $failed){try{Close-OwnedEditor}catch{$failed=$true;Record 'cleanup failure' @{error=$_.Exception.Message}}}
 if($process){
  if(-not $process.HasExited){$process.Kill();$process.WaitForExit(5000)|Out-Null}
  if($process.HasExited){$cleanup=@{editor_exited=$true;editor_exit_code=[JourneyInput]::ExitCode($editorHandle)}}
 }
 foreach($id in @('s1','s2','s3')){if(@($steps | Where-Object {$_['id'] -eq $id}).Count -eq 0){$steps.Add(@{id=$id;status=$(if($steps.Count -eq 0 -and $failed){'FAIL'}else{'NOT_RUN'});observed='Lab prerequisite failed; see retained observations.'})}}
 if($failed -and @($steps | Where-Object {$_['status'] -eq 'FAIL'}).Count -eq 0){$steps[$steps.Count-1].status='FAIL';$steps[$steps.Count-1].observed='Owned cleanup failed; see diagnostics.'}
 Record 'terminal cleanup' $cleanup
 # Program/probe logs remain in bounded scratch; retain the compact observation and key proof artifacts here.
 $artifacts=@();$paths=@((Join-Path $scratch 'native-observations.json'),(Join-Path $scratch 'lab-fixture.json'))+@($extraArtifacts | Where-Object {$_ -notmatch 'program-\d+\.out'})
 foreach($path in @($paths | Select-Object -Unique)){if([IO.File]::Exists($path)){$artifacts+=@{path=$path;sha256=(Hash-File $path)}}}
 Write-Json (Join-Path $scratch 'native-response.json') @{schema_version=1;journey=$request.journey.id;binary_sha256=$request.binary_sha256;fixture=$lab.identity;steps=@($steps.ToArray());cleanup=$cleanup;artifacts=$artifacts}
 if($process){$process.Dispose()}
}
