# SPDX-License-Identifier: MPL-2.0
# Only the process launched by this driver is ever stopped or restarted.
function Start-OwnedEditor([string]$executable=$script:request.executable,[string[]]$arguments=@('--software','--no-session','--no-extensions','--new-instance')) {
 if($script:process -and -not $script:process.HasExited){throw 'Previous owned editor is still running'}
 if((Hash-File $executable) -cne $script:request.binary_sha256){throw 'Pinned editor changed before launch'}
 [JourneyInput]::Desktop();$script:launchNumber++
 $stdout=Join-Path $script:scratch ('editor-'+$script:launchNumber+'.stdout.log')
 $stderr=Join-Path $script:scratch ('editor-'+$script:launchNumber+'.stderr.log')
 if((Test-Path -LiteralPath $stdout) -or (Test-Path -LiteralPath $stderr)){throw 'Launch output already exists'}
 if($script:process){$script:process.Dispose()}
 $script:process=Start-Process -FilePath $executable -ArgumentList $arguments -WorkingDirectory $script:scratch -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
 $script:extraArtifacts.Add($stdout);$script:extraArtifacts.Add($stderr)
 $script:editorHandle=$script:process.Handle;$script:window=[IntPtr]::Zero
 $deadline=[DateTime]::UtcNow.AddSeconds(10)
 do {
  if($script:process.HasExited){throw 'Editor exited before publishing its window'}
  [JourneyInput]::Desktop();$script:process.Refresh();$script:window=$script:process.MainWindowHandle
  if($script:window -ne [IntPtr]::Zero){break};Start-Sleep -Milliseconds 100
 } while([DateTime]::UtcNow -lt $deadline)
 if($script:window -eq [IntPtr]::Zero){throw 'Owned editor window did not appear'}
 [JourneyInput]::Focus($script:window);Start-Sleep -Milliseconds 400;Guard
 $initialLayout=[JourneyInput]::KeyboardLayout($script:window)
 [JourneyInput]::FixtureKeyboard($script:window)
 $dpi=[JourneyInput]::GetDpiForWindow($script:window)
 if($dpi -ne ([int]$script:request.dpi*96/100)){throw 'Observed window DPI differs from requested cell'}
 Record ('owned launch '+$script:launchNumber) @{pid=$script:process.Id;executable=$executable;binary_sha256=(Hash-File $executable);dpi=$dpi;arguments=$arguments;initial_keyboard_layout=$initialLayout;fixture_keyboard_layout=[JourneyInput]::KeyboardLayout($script:window)}
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
