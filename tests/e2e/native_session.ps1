# SPDX-License-Identifier: MPL-2.0
# Only the process launched by this driver is ever stopped or restarted.
function Read-OwnedFirstFrame([string]$path) {
 if(-not [IO.File]::Exists($path)){return $null}
 # Redirected output remains open for writing while the editor starts.
 $stream=[IO.File]::Open($path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::ReadWrite)
 $reader=$null
 try {
  if($stream.Length -gt 262144){throw 'Owned startup output exceeds bound'}
  $reader=[IO.StreamReader]::new($stream)
  $buffer=New-Object char[] 262145
  $count=$reader.ReadBlock($buffer,0,$buffer.Length)
  if($count -gt 262144){throw 'Owned startup output exceeds bound'}
  $text=[string]::new($buffer,0,$count)
 } finally {if($reader){$reader.Dispose()}else{$stream.Dispose()}}
 foreach($line in ($text -split "`r?`n")) {
  try{$row=$line | ConvertFrom-Json -ErrorAction Stop}catch{continue}
  if($row.event -ceq 'first_frame' -and ($row.microseconds -is [int] -or $row.microseconds -is [long]) -and $row.microseconds -ge 0 -and $row.software -is [bool]){return $row}
 }
 return $null
}
function Wait-OwnedWindowReady([string]$stdout,[string]$stderr,[int]$timeoutMilliseconds=10000,[switch]$AnyVisibility) {
 if($timeoutMilliseconds -lt 1 -or $timeoutMilliseconds -gt 10000){throw 'Invalid startup deadline'}
 $deadline=[DateTime]::UtcNow.AddMilliseconds($timeoutMilliseconds)
 do {
  if($script:process.HasExited){throw 'Editor exited before rendering its first frame'}
  [JourneyInput]::Desktop()
  if($AnyVisibility) {
   # MainWindowHandle skips hidden windows; take the one owned top-level window as created.
   $candidates=@([JourneyInput]::ProcessWindows([uint32]$script:process.Id))
   $script:window=if($candidates.Count -eq 1){$candidates[0]}else{[IntPtr]::Zero}
  } else {$script:process.Refresh();$script:window=$script:process.MainWindowHandle}
  if((Test-Path -LiteralPath $stderr) -and (Get-Item -LiteralPath $stderr).Length -gt 262144){throw 'Owned startup error output exceeds bound'}
  $frame=Read-OwnedFirstFrame $stdout
  if($script:window -ne [IntPtr]::Zero -and $null -ne $frame) {
   if([JourneyInput]::Owner($script:window) -ne $script:process.Id){throw 'Owned startup window identity differs'}
   Record 'owned first frame ready' @{pid=$script:process.Id;window=$script:window.ToInt64();frame=$frame}
   return
  }
  Start-Sleep -Milliseconds 50
 } while([DateTime]::UtcNow -lt $deadline)
 throw 'Owned editor window and first frame were not ready before the startup deadline'
}
function Start-OwnedEditor([string]$executable=$script:request.executable,[string[]]$arguments=@('--software','--no-session','--no-extensions','--new-instance'),[ValidateSet('Hidden','Normal')][string]$windowStyle='Hidden',[switch]$RequireShown) {
 if($arguments -notcontains '--new-instance'){$arguments+='--new-instance'}
 if($script:process -and -not $script:process.HasExited){throw 'Previous owned editor is still running'}
 if((Hash-File $executable) -cne $script:request.binary_sha256){throw 'Pinned editor changed before launch'}
 [JourneyInput]::Desktop();$script:launchNumber++
 $stdout=Join-Path $script:scratch ('editor-'+$script:launchNumber+'.stdout.log')
 $stderr=Join-Path $script:scratch ('editor-'+$script:launchNumber+'.stderr.log')
 if((Test-Path -LiteralPath $stdout) -or (Test-Path -LiteralPath $stderr)){throw 'Launch output already exists'}
 if($script:process){$script:process.Dispose()}
 $script:process=Start-Process -FilePath $executable -ArgumentList $arguments -WorkingDirectory $script:scratch -WindowStyle $windowStyle -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
 $script:extraArtifacts.Add($stdout);$script:extraArtifacts.Add($stderr)
 $script:editorHandle=$script:process.Handle;$script:window=[IntPtr]::Zero
 # Winit may publish its handle before the initial renderer/window transition.
 # Observe the owned first frame before the one focus request. Never retry focus
 # after another application takes it or submit input to a foreign window.
 Wait-OwnedWindowReady $stdout $stderr -AnyVisibility:$RequireShown
 # Observed before the one focus request below shows and raises the window.
 $script:launchWindow=@{visible=[JourneyInput]::IsWindowVisible($script:window);iconic=[JourneyInput]::IsIconic($script:window);style=$windowStyle}
 if($RequireShown) {
  # ISSUE-030: the window was found regardless of visibility and has rendered its
  # first frame; it must show itself, unminimized, without the driver's focus request.
  $atFirstFrame=$script:launchWindow;$shown=[Diagnostics.Stopwatch]::StartNew()
  while(-not ([JourneyInput]::IsWindowVisible($script:window) -and -not [JourneyInput]::IsIconic($script:window))) {
   [JourneyInput]::Desktop()
   if($script:process.HasExited){throw 'Relaunched window was not shown: the editor exited after its first frame'}
   if($shown.ElapsedMilliseconds -ge 3000) {
    Record ('owned launch '+$script:launchNumber+' window not shown') @{pid=$script:process.Id;window=$script:window.ToInt64();visible=[JourneyInput]::IsWindowVisible($script:window);iconic=[JourneyInput]::IsIconic($script:window)}
    throw 'Relaunched window was not shown: it stayed hidden or minimized for 3 s after its first frame'
   }
   Start-Sleep -Milliseconds 50
  }
  $script:launchWindow=@{visible=[JourneyInput]::IsWindowVisible($script:window);iconic=[JourneyInput]::IsIconic($script:window);style=$windowStyle;discovery='any-visibility';at_first_frame=$atFirstFrame;show_wait_ms=$shown.ElapsedMilliseconds}
 }
 [JourneyInput]::Focus($script:window);Start-Sleep -Milliseconds 400;Guard
 $initialLayout=[JourneyInput]::KeyboardLayout($script:window)
 [JourneyInput]::FixtureKeyboard($script:window)
 $dpi=[JourneyInput]::GetDpiForWindow($script:window)
 if($dpi -ne ([int]$script:request.dpi*96/100)){throw 'Observed window DPI differs from requested cell'}
 Record ('owned launch '+$script:launchNumber) @{pid=$script:process.Id;executable=$executable;binary_sha256=(Hash-File $executable);dpi=$dpi;arguments=$arguments;window_before_focus=$script:launchWindow;initial_keyboard_layout=$initialLayout;fixture_keyboard_layout=[JourneyInput]::KeyboardLayout($script:window)}
}
function Close-OwnedEditor {
 Guard
 $menu=[JourneyInput]::GetMenu($script:window);$file=[IntPtr]::Zero
 for($i=0;$i -lt [JourneyInput]::GetMenuItemCount($menu);$i++){if([JourneyInput]::Label($menu,$i) -eq 'File'){$file=[JourneyInput]::GetSubMenu($menu,$i);break}}
 $matches=@()
 for($i=0;$i -lt [JourneyInput]::GetMenuItemCount($file);$i++){if([JourneyInput]::Label($file,$i) -eq 'Exit'){$matches+=@{id=[JourneyInput]::GetMenuItemID($file,$i);state=[JourneyInput]::GetMenuState($file,[uint32]$i,0x400)}}}
 if($matches.Count -ne 1 -or ($matches[0].state -band 3)){throw 'Exit command missing, ambiguous or disabled'}
 if(-not [JourneyInput]::PostMessageW($script:window,0x111,[UIntPtr]$matches[0].id,[IntPtr]::Zero)){throw 'Exit submission failed'}
 if(-not $script:process.WaitForExit(8000)){throw 'Clean editor Exit timed out'}
 $code=[JourneyInput]::ExitCode($script:editorHandle)
 Record ('owned exit '+$script:launchNumber) @{pid=$script:process.Id;exit_code=$code}
 if($code -ne 0){throw 'Editor exited with a failure status'}
}
