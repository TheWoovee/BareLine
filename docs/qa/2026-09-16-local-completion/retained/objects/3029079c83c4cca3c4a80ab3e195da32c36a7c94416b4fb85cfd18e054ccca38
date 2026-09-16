# SPDX-License-Identifier: MPL-2.0
# Shared lab actions; no action runs when this file is dot-sourced.
. (Join-Path $PSScriptRoot 'native_regex_transform.ps1')
function Lab-VM {
 $identity=Get-CimInstance Win32_ComputerSystemProduct
 $machine=Get-CimInstance Win32_ComputerSystem
 $principal=[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
 if($identity.UUID -ine $script:lab.machine_uuid -or ($machine.Model+' '+$machine.Manufacturer) -notmatch 'Virtual|VMware|QEMU|KVM|Parallels' -or $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){throw 'This procedure requires the explicitly bound disposable VM and an unelevated standard-user token'}
 Record 'disposable VM' @{uuid=$identity.UUID;model=$machine.Model;snapshot_attestation=$script:lab.snapshot_id;elevated=$false}
}
function Lab-Invoke($element) {
 Guard;if(-not $element.Current.IsEnabled){throw 'Lab control disabled'}
 $pattern=$null
 Record 'lab invoked control' (Record-Element $element)
 if($element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)){$pattern.Invoke()}
 elseif($element.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern,[ref]$pattern)){$pattern.Select()}
 elseif($element.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern,[ref]$pattern)){$pattern.Toggle()}
 else{throw 'Lab control has no supported invocation pattern'}
}
function Lab-Button([string]$name){Lab-Invoke (Regex-Element $name 'ControlType.Button')}
function Lab-PackageButton([string]$label,[string]$button,[string]$type='ControlType.Button') {
 $row=Regex-Element $label 'ControlType.ListItem';$bounds=$row.Current.BoundingRectangle
 $matches=@(Elements | Where-Object {$_.Current.Name -ceq $button -and $_.Current.ControlType.ProgrammaticName -ceq $type -and $_.Current.BoundingRectangle.Left -ge $bounds.Left -and $_.Current.BoundingRectangle.Left -lt $bounds.Right})
 if($matches.Count -ne 1){throw 'Package control not uniquely bound to reviewed card'}
 Lab-Invoke $matches[0]
}
function Lab-Program([string]$exe,[string[]]$arguments,[int]$seconds=20,[int]$expected=0) {
 [JourneyInput]::Desktop();$script:programNumber++
 $out=Join-Path $script:scratch ('program-'+$script:programNumber+'.out');$err=$out+'.err'
 $child=Start-Process -FilePath $exe -ArgumentList $arguments -WorkingDirectory $script:scratch -WindowStyle Hidden -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
 $handle=$child.Handle
 try {
  $deadline=[DateTime]::UtcNow.AddSeconds($seconds)
  while(-not $child.WaitForExit(100)){
   [JourneyInput]::Desktop();if([DateTime]::UtcNow -gt $deadline){throw 'Owned lab program deadline exceeded'}
   foreach($path in @($out,$err)){if((Test-Path -LiteralPath $path) -and (Get-Item -LiteralPath $path).Length -gt 262144){throw 'Lab program output limit'}}
  }
  $code=[JourneyInput]::ExitCode($handle)
  Record 'owned lab program exit' @{pid=$child.Id;created_utc=$child.StartTime.ToUniversalTime().ToString('o');executable=$exe;arguments=$arguments;exit_code=$code}
  if($code -ne $expected){throw "Lab program exit $code differs from $expected"}
 }finally{if(-not $child.HasExited){$child.Kill();$child.WaitForExit(5000)|Out-Null};$child.Dispose()}
 if((Get-Item -LiteralPath $out).Length -gt 262144 -or (Get-Item -LiteralPath $err).Length -gt 262144){throw 'Lab program output limit'}
 $script:extraArtifacts.Add($out);$script:extraArtifacts.Add($err)
 return $out
}
function Lab-Probe {
 $script:probeNumber++
 if($script:probeNumber -gt 12){throw 'Recovery probe attempt bound'}
 $destination=Join-Path $script:scratch ('probe-'+$script:probeNumber)
 $path=Lab-Program $script:lab.paths.recovery_probe @(('"'+(Join-Path $script:profile 'recovery')+'"'),('"'+$destination+'"'))
 Get-Content -LiteralPath $path -Raw -Encoding UTF8 | ConvertFrom-Json
}
function Lab-Durable {
 $expected=@((Hash-Text $script:lab.saved_text),(Hash-Text $script:lab.untitled))
 for($attempt=0;$attempt -lt 10;$attempt++) {
  Guard
  if(-not (Test-Path -LiteralPath (Join-Path $script:profile 'recovery'))){Start-Sleep -Milliseconds 1000;continue}
  try{$result=Lab-Probe}catch{Record 'incomplete recovery probe' @{error=$_.Exception.Message};Start-Sleep -Milliseconds 1000;continue}
  $rows=@($result.rows | Where-Object {$_.status -ceq 'Complete' -and $_.last_durable -and $_.recovered.sha256 -in $expected})
  if(@($rows.recovered.sha256 | Select-Object -Unique).Count -eq 2){Record 'durable saved and untitled' @{rows=$rows};return}
  Start-Sleep -Milliseconds 1000
 }
 throw 'Both exact durable recovery copies were not observed'
}
function Hash-Text([string]$text) {
 $sha=[Security.Cryptography.SHA256]::Create()
 try{([BitConverter]::ToString($sha.ComputeHash($script:utf8.GetBytes($text)))).Replace('-','').ToLowerInvariant()}finally{$sha.Dispose()}
}
function Lab-CrashPrepare {
 $script:crashSaved=Join-Path $script:scratch 'crash-saved.txt'
 $hex=$script:lab.saved_hex;[byte[]]$raw=for($i=0;$i -lt $hex.Length;$i+=2){[Convert]::ToByte($hex.Substring($i,2),16)}
 [IO.File]::WriteAllBytes($script:crashSaved,$raw)
 Focus-Editor;Key 79 $true;File-Dialog $script:crashSaved
 Key 35 $true;Text '!';Expect-Text $script:lab.saved_text 'saved fixture accepted edit'
 Key 78 $true;Text $script:lab.untitled;Expect-Text $script:lab.untitled 'untitled accepted edit'
 Lab-Durable
}
function Lab-CrashTerminate {
 # Switch to the saved fixture by exact native tab name, not an assumed index.
 $tabs=@(Elements | Where-Object {$_.Current.ControlType -eq [System.Windows.Automation.ControlType]::TabItem -and $_.Current.Name.Contains('crash-saved.txt')})
 if($tabs.Count -ne 1){throw 'Saved fixture tab not unique'}
 Lab-Invoke $tabs[0];Focus-Editor;Expect-Text $script:lab.saved_text 'saved fixture selected'
 $arm=Join-Path $script:scratch 'save.arm';$token=[Guid]::NewGuid().ToString('N')
 Write-Json $arm @{point=$script:lab.save_point;target=$script:crashSaved;token=$token}
 Key 83 $true
 $marker=Join-Path $script:scratch 'save.reached.json';$deadline=[DateTime]::UtcNow.AddSeconds(10)
 while(-not [IO.File]::Exists($marker)){Guard;if([DateTime]::UtcNow -gt $deadline){throw 'No real save boundary acknowledgement (requires qa-faults candidate)'};Start-Sleep -Milliseconds 50}
 if((Get-Item -LiteralPath $marker).Length -gt 8192){throw 'Boundary marker bound'}
 $reached=Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
 if($reached.pid -ne $script:process.Id -or $reached.token -cne $token -or $reached.point -cne $script:lab.save_point -or $reached.target -cne $script:crashSaved){throw 'Stale/foreign save boundary acknowledgement'}
 Record 'observed save boundary' $reached;$script:extraArtifacts.Add($marker)
 Guard;$pidValue=$script:process.Id;$created=$script:process.StartTime.ToUniversalTime().ToString('o')
 $script:process.Kill();if(-not $script:process.WaitForExit(5000)){throw 'Owned editor did not terminate'}
 Record 'owned editor terminated' @{pid=$pidValue;created_utc=$created;boundary=$reached.point;exit_code=[JourneyInput]::ExitCode($script:editorHandle)}
 Remove-Item -LiteralPath $arm
 [byte[]]$original=for($i=0;$i -lt $script:lab.saved_hex.Length;$i+=2){[Convert]::ToByte($script:lab.saved_hex.Substring($i,2),16)}
 $actual=[IO.File]::ReadAllBytes($script:crashSaved)
 if([Convert]::ToBase64String($actual) -cnotin @([Convert]::ToBase64String($original),[Convert]::ToBase64String([byte[]]($original+33)))){throw 'Save survived with partial or unrelated bytes'}
 Record 'atomic saved bytes' @{path=$script:crashSaved;sha256=(Hash-File $script:crashSaved);bytes=$actual.Length}
}
function Lab-Restore([string]$fragment,[string]$expected,[byte[]]$raw,[string]$name) {
 if(@(Elements | Where-Object {$_.Current.Name -ceq 'Recovery Center' -and $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Group}).Count -eq 0){Regex-Menu 'Recovery Center'}
 $deadline=[DateTime]::UtcNow.AddSeconds(10)
 do {Guard;$rows=@(Elements | Where-Object {$_.Current.ControlType -eq [System.Windows.Automation.ControlType]::ListItem -and $_.Current.Name.Contains($fragment) -and $_.Current.Name.Contains('Complete')});if($rows.Count -eq 1){break};Start-Sleep -Milliseconds 100}while([DateTime]::UtcNow -lt $deadline)
 if($rows.Count -ne 1){throw 'Exact complete recovery row missing/ambiguous'}
 Lab-Invoke $rows[0];Lab-Button 'Restore recovered copy';Focus-Editor;Expect-Text $expected ('restored '+$name)
 $destination=Join-Path $script:scratch ('recovered-'+$name+'.txt')
 Key 83 $true $true;File-Dialog $destination;Expect-File $destination $raw ('recovered '+$name+' bytes')
 $script:extraArtifacts.Add($destination)
}
function Lab-CrashRecover {
 Start-OwnedEditor $script:request.executable @('--software')
 [byte[]]$raw=for($i=0;$i -lt $script:lab.saved_hex.Length;$i+=2){[Convert]::ToByte($script:lab.saved_hex.Substring($i,2),16)}
 Lab-Restore 'crash-saved.txt;' $script:lab.saved_text ([byte[]]($raw+33)) 'saved'
 Lab-Restore 'not yet saved to disk;' $script:lab.untitled ($script:utf8.GetBytes($script:lab.untitled)) 'untitled'
}
function Run-CrashRecovery {
 Step 's1' 'Actual checkpoint copies contain both exact acknowledged edits.' {Lab-CrashPrepare}
 Step 's2' 'Owned editor terminated only after the bound save signal; original-or-complete bytes survived.' {Lab-CrashTerminate}
 Step 's3' 'Real Recovery Center restored both documents; saved outputs preserve accepted bytes/provenance.' {Lab-CrashRecover}
}
function Lab-InstallExtensions {
 Regex-Menu 'Manage Extensions';Lab-Button 'Install runtime';File-Dialog $script:lab.paths.runtime
 $null=Regex-Status 'Verified runtime installed; host stopped' 'runtime trust accepted'
 Lab-Button 'Open signed catalog';File-Dialog $script:lab.paths.catalog
 $null=Regex-Status 'Verified catalog loaded. Select a package, then Install Selected Package.' 'catalog trust accepted'
 foreach($package in $script:lab.packages) {
  Lab-Invoke (Regex-Element 'Discover' 'ControlType.Tab')
  $buttons=@(Elements | Where-Object {$_.Current.Name -ceq 'Install or update' -and $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button} | Sort-Object {$_.Current.BoundingRectangle.Left})
  if($buttons.Count -ne 3){throw 'Reviewed three-package catalog differs'}
  Lab-Invoke $buttons[$package.catalog_index]
  $null=Regex-Status 'Installed. Review permissions before enabling.' 'package installed disabled'
  Lab-PackageButton $package.installed_label 'Permissions'
  $review=@(Elements | Where-Object {$_.Current.Name.Contains(' requests: ')})
  if($review.Count -ne 1){throw 'Explicit permission review missing'}
  Record 'reviewed package permissions' (Record-Element $review[0])
  Lab-Button 'Approve reviewed';$null=Regex-Status 'Permissions saved' 'permission grant durable'
 }
 Record 'reviewed installed packages' @{packages=$script:lab.packages;asset_bindings=$script:lab.assets}
 Lab-Button 'Close'
}
function Lab-Panel([string]$expected,[bool]$contains=$false) {
 $deadline=[DateTime]::UtcNow.AddSeconds(10)
 do {
  Guard;$elements=@(Elements | Where-Object {$_.Current.Name -ceq 'Extension result'})
  if($elements.Count -eq 1){
   $pattern=$null;$value=$elements[0].Current.HelpText
   if($elements[0].TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern,[ref]$pattern)){$value=$pattern.Current.Value}
   if(($contains -and $value.Contains($expected)) -or (-not $contains -and $value -ceq $expected)){return @{name='Extension result';value=$value}}
  };Start-Sleep -Milliseconds 100
 }while([DateTime]::UtcNow -lt $deadline)
 throw 'Exact extension panel result missing'
}
function Lab-ExtensionCycle {
 $path=Join-Path $script:scratch 'extension.json';$before='{ "n": 9007199254740993 }';$after='{"n":9007199254740993}'
 [IO.File]::WriteAllText($path,$before,$script:utf8)
 Focus-Editor;Key 79 $true;File-Dialog $path;Regex-Menu 'JSON: Minify'
 Expect-Text $after 'json extension result';Key 90 $true;Expect-Text $before 'json single undo';Expect-Dirty $false 'json undo clean';Key 87 $true
 $path=Join-Path $script:scratch 'extension.xml';$xml='<root><n>1</n></root>'
 [IO.File]::WriteAllText($path,$xml,$script:utf8);Key 79 $true;File-Dialog $path;Regex-Menu 'XML: Validate'
 Regex-Menu 'Manage Extensions';Record 'xml extension result' (Lab-Panel 'Valid XML; external resolution disabled');Lab-Button 'Close'
 Focus-Editor;Expect-Text $xml 'XML preserved source';Key 87 $true
 $path=Join-Path $script:scratch 'extension-hex.txt';[IO.File]::WriteAllBytes($path,[byte[]](255,254,65,0));Key 79 $true;File-Dialog $path
 Focus-Editor;Key 65 $true;Text 'B';Regex-Menu 'Hex: Original Bytes';Regex-Menu 'Manage Extensions'
 $panel=Lab-Panel 'FF FE 41 00' $true
 if(-not $panel.value.Contains('Unsaved text edits are excluded')){throw 'Hex original provenance label missing'}
 Record 'hex extension result' $panel;Lab-Button 'Close';Focus-Editor;Expect-Text 'B' 'hex preserves live edit';Key 90 $true;Expect-Text 'A' 'hex undo';Key 87 $true
}
function Lab-Host([bool]$wait=$true) {
 $deadline=[DateTime]::UtcNow.AddSeconds($(if($wait){8}else{0}))
 do {
  Guard;$children=@(Get-CimInstance Win32_Process -Filter ('ParentProcessId='+$script:process.Id) | Where-Object {$_.ExecutablePath -and $_.ExecutablePath.StartsWith($script:profile,[StringComparison]::OrdinalIgnoreCase) -and (Hash-File $_.ExecutablePath) -ceq $script:lab.host_sha256})
  if($children.Count -gt 1){throw 'Multiple owned runtime hosts'}
  if($children.Count -eq 1){return $children[0]}
  if(-not $wait){return $null};Start-Sleep -Milliseconds 50
 }while([DateTime]::UtcNow -lt $deadline)
 throw 'Owned pinned extension host not observed alive'
}
function Lab-CrashRevoke {
 $path=Join-Path $script:scratch 'extension-crash.json';$value=(' '*1048576)+'{"n":1}'
 [IO.File]::WriteAllText($path,$value,$script:utf8);Key 79 $true;File-Dialog $path;Regex-Menu 'JSON: Minify'
 $hostRow=Lab-Host;$child=[Diagnostics.Process]::GetProcessById($hostRow.ProcessId);$null=$child.Handle
 try {
  if($child.HasExited -or [Math]::Abs(($child.StartTime.ToUniversalTime()-$hostRow.CreationDate.ToUniversalTime()).TotalMilliseconds) -gt 1){throw 'Host identity changed before termination'}
  Guard;$child.Kill();if(-not $child.WaitForExit(5000)){throw 'Owned host termination timed out'}
  Record 'owned host terminated' @{pid=$child.Id;created_utc=$hostRow.CreationDate.ToUniversalTime().ToString('o');parent_pid=$script:process.Id;sha256=$script:lab.host_sha256}
 }finally{$child.Dispose()}
 Regex-Menu 'Manage Extensions';$package=@($script:lab.packages | Where-Object {$_.kind -ceq 'json'})[0]
 Lab-PackageButton $package.installed_label 'Disable extension' 'ControlType.CheckBox'
 $null=Regex-Status 'Permissions saved' 'revocation durable';Lab-Button 'Close'
 $deadline=[DateTime]::UtcNow.AddSeconds(5)
 while(Lab-Host $false){if([DateTime]::UtcNow -gt $deadline){throw 'Revoked host did not stop'};Start-Sleep -Milliseconds 50}
 # A disabled native route may refuse invocation or report the explicit denial.
 $menuDenied=$false
 try{Regex-Menu 'JSON: Minify'}catch{if($_.Exception.Message -like 'Native command missing, ambiguous or disabled:*'){$menuDenied=$true}else{throw}}
 if(-not $menuDenied){Regex-Menu 'Manage Extensions';$null=Regex-Status 'Install and enable an extension contributing this command' 'revoked capability refusal';Lab-Button 'Close'}
 if(Lab-Host $false){throw 'Revoked host remains alive'}
 Focus-Editor;Key 78 $true;Text 'alive';Expect-Text 'alive' 'editor survives host crash';Key 90 $true;Key 87 $true
 Record 'revoked extension denied' @{native_disabled=$menuDenied;host_alive=$false;editor_pid=$script:process.Id}
 # Save remaining generated edits to avoid a dirty-close prompt during owned cleanup.
 Focus-Editor;Key 83 $true
}
function Run-ExtensionIsolation {
 Step 's1' 'Signed runtime/catalog/packages installed through the real manager with explicit permission review.' {Lab-InstallExtensions}
 Step 's2' 'Actual JSON, XML and original-byte Hex results plus editor Undo/responsiveness observed.' {Lab-ExtensionCycle}
 Step 's3' 'Only the owned pinned host was terminated; editor survived and disabled capability was denied.' {Lab-CrashRevoke}
}
function Lab-Signature([string]$path) {
 $signature=Get-AuthenticodeSignature -LiteralPath $path
 if($signature.Status -ne 'Valid' -or -not $signature.SignerCertificate){throw 'Valid Authenticode signature required'}
 $sha=[Security.Cryptography.SHA256]::Create()
 try{$pin=([BitConverter]::ToString($sha.ComputeHash($signature.SignerCertificate.RawData))).Replace('-','').ToLowerInvariant()}finally{$sha.Dispose()}
 if($pin -cne $script:lab.publisher_sha256){throw 'Authenticode publisher pin differs'}
 Record 'publisher verified' @{path=$path;sha256=(Hash-File $path);publisher_sha256=$pin}
}
function Lab-SystemInventory {
 # Read-only global baselines on the disposable VM; compare them after uninstall.
 $associations=@()
 foreach($path in @('HKCU:\Software\Classes\*\shell\Bareline','HKCU:\Software\Classes\Applications\bareline.exe','HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.txt\UserChoice')) {
  if(Test-Path -LiteralPath $path){$item=Get-ItemProperty -LiteralPath $path;$associations+=@{path=$path;values=($item | Select-Object * -ExcludeProperty PS* | ConvertTo-Json -Compress)}}
 }
 @{services=@(Get-CimInstance Win32_Service | Select-Object Name,PathName,StartMode | Sort-Object Name);tasks=@(Get-ScheduledTask | Select-Object TaskPath,TaskName | Sort-Object TaskPath,TaskName);associations=$associations}
}
function Lab-WaitHash([string]$path,[string]$hash) {
 $deadline=[DateTime]::UtcNow.AddSeconds(20)
 do{[JourneyInput]::Desktop();if([IO.File]::Exists($path)){try{if((Hash-File $path) -ceq $hash){return}}catch{}};Start-Sleep -Milliseconds 100}while([DateTime]::UtcNow -lt $deadline)
 throw 'Expected complete file digest not observed within deadline'
}
function Run-InstallUpdate {
 Step 's1' 'Pinned signed installer installed into the owned VM directory with association tasks explicitly disabled.' {
  $script:systemBefore=Lab-SystemInventory
  if(@($script:systemBefore.associations | Where-Object {$_.path -like '*Classes*'}).Count){throw 'VM snapshot has a pre-existing Bareline registration'}
  $script:installation=Join-Path $script:scratch 'installed'
  Lab-Signature $script:lab.paths.installer
  $null=Lab-Program $script:lab.paths.installer @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/CURRENTUSER','/TASKS=""',('/DIR="'+$script:installation+'"'),('/LOG="'+(Join-Path $script:scratch 'installer.log')+'"')) 45
  $exe=Join-Path $script:installation 'bareline.exe';$helper=Join-Path $script:installation 'bareline-update-helper.exe'
  Lab-WaitHash $exe $script:request.binary_sha256;Lab-WaitHash $helper $script:lab.helper_sha256
  Lab-Signature $exe;Lab-Signature $helper
  $after=Lab-SystemInventory
  if(($after.associations | ConvertTo-Json -Depth 8 -Compress) -cne ($script:systemBefore.associations | ConvertTo-Json -Depth 8 -Compress)){throw 'Default install changed association choices'}
  Start-OwnedEditor $exe @('--software')
  Record 'signed installation verified' @{editor_sha256=(Hash-File $exe);helper_sha256=(Hash-File $helper);associations=$after.associations}
 }
 Step 's2' 'Actual editor verifies the configured update, applies on exit; failed activation is observed and supported helper restores the working version.' {
  Regex-Menu 'Check for Updates'
  $null=Regex-Status 'Verified update ready. Choose Apply Update on Exit.' 'verified update ready'
  $pending=Join-Path $script:installation 'bareline.pending.exe'
  Lab-WaitHash $pending $script:lab.update_sha256;Lab-Signature $pending
  Record 'verified update staged' @{sha256=(Hash-File $pending)}
  Regex-Menu 'Apply Update on Exit';Close-OwnedEditor
  $exe=Join-Path $script:installation 'bareline.exe';$helper=Join-Path $script:installation 'bareline-update-helper.exe'
  Lab-WaitHash $exe $script:lab.update_sha256
  $journal=Join-Path $script:installation 'bareline.update-journal';$backup=Join-Path $script:installation 'bareline.rollback.exe'
  Lab-WaitHash $backup $script:request.binary_sha256
  if(-not [IO.File]::Exists($journal)){throw 'Actual durable update journal missing'}
  Copy-Item -LiteralPath $journal -Destination (Join-Path $script:scratch 'update-journal.txt');$script:extraArtifacts.Add((Join-Path $script:scratch 'update-journal.txt'))
  $null=Lab-Program $exe @('--software') 10 $script:lab.activation_failure_exit_code
  Record 'activation failure observed' @{sha256=(Hash-File $exe);expected_exit_code=$script:lab.activation_failure_exit_code;backup_sha256=(Hash-File $backup)}
  $null=Lab-Program $helper @('--recover')
  Lab-WaitHash $exe $script:request.binary_sha256
  Record 'rollback bytes verified' @{sha256=(Hash-File $exe)}
 }
 Step 's3' 'Restored editor launches and edits correctly; real uninstaller preserves generated user data and baseline services/tasks/association choices.' {
  $exe=Join-Path $script:installation 'bareline.exe';Start-OwnedEditor $exe @('--software')
  Focus-Editor;Text 'restored';Expect-Text 'restored' 'restored editor verified';Key 90 $true;Close-OwnedEditor
  $sentinel=Join-Path $script:profile 'user-sentinel.txt';[IO.File]::WriteAllText($sentinel,'keep user data',$script:utf8)
  $uninstaller=Join-Path $script:installation 'unins000.exe'
  $null=Lab-Program $uninstaller @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',('/LOG="'+(Join-Path $script:scratch 'uninstaller.log')+'"')) 45
  if([IO.File]::Exists($exe) -or [IO.File]::Exists($uninstaller)){throw 'Uninstaller left owned installed program'}
  Expect-File $sentinel $script:utf8.GetBytes('keep user data') 'uninstall preserved profile'
  $after=Lab-SystemInventory
  if(($after | ConvertTo-Json -Depth 8 -Compress) -cne ($script:systemBefore | ConvertTo-Json -Depth 8 -Compress)){throw 'Uninstall changed baseline services/tasks/association choices'}
  Record 'uninstall inventory verified' $after
 }
}
