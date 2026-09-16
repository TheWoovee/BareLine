# SPDX-License-Identifier: MPL-2.0
. (Join-Path $PSScriptRoot 'native_regex_transform.ps1')
function Log-View([string]$expected,[string]$stage,[bool]$selection=$false,[bool]$contains=$false) {
 $deadline=[DateTime]::UtcNow.AddSeconds(20);$lastError=$null
 do {
  Guard
  try {
   $pattern=(Editor).GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
   $ranges=@($pattern.GetVisibleRanges())
   if($ranges.Count -lt 1 -or $ranges.Count -gt 128){throw 'Log visible ranges missing/excessive'}
   $text=$ranges[0].GetText(8192);$selected=@($pattern.GetSelection() | ForEach-Object {$_.GetText(256)})
   $actual=@{visible=$text;selected=$selected}
   $matches=if($selection){$selected.Count -eq 1 -and $selected[0] -ceq $expected}elseif($contains){$text.Contains($expected)}else{$text.StartsWith($expected,[StringComparison]::Ordinal)}
   if($matches){Record $stage $actual;return}
  } catch {$lastError=$_.Exception.Message}
  Start-Sleep -Milliseconds 100
 }while([DateTime]::UtcNow -lt $deadline)
 Record $stage @{actual=$actual;error=$lastError};throw ('Paged log observation failed: '+$stage)
}
function Run-HugeLog {
 Step 's1' 'A generated 512 MiB log opens as a bounded editable viewport; one edit/Undo restores it and observed private-memory growth stays below 192 MiB.' {
  $script:process.Refresh();$baseline=$script:process.PrivateMemorySize64
  Focus-Editor;Key 79 $true;File-Dialog $script:saved
  Log-View '0123456789abcdef' 'log initial viewport'
  Focus-Editor;Key 36 $true;Text 'X';Log-View 'X0123456789abcdef' 'log edited viewport'
  Key 90 $true;Log-View '0123456789abcdef' 'log Undo viewport';Expect-Dirty $false 'log Undo clean'
  $script:process.Refresh();$after=$script:process.PrivateMemorySize64
  Record 'log memory bound' @{baseline=$baseline;after=$after;limit=$script:logFixture.private_growth_limit}
  if($after-$baseline -gt $script:logFixture.private_growth_limit){throw 'Log private-memory growth exceeded bound'}
 }
 Step 's2' 'Complete literal search reports one global result and selects the unique Unicode marker that crosses the generated boundary.' {
  Regex-Menu 'Normal Search Mode'
  Regex-Field 'Find' $script:logFixture.needle 'log query'
  $deadline=[DateTime]::UtcNow.AddSeconds(40)
  do {
   Guard;$status=@(Elements | Where-Object {$_.Current.Name -ceq 'Find results: 1 matches'})
   if($status.Count -eq 1){break};Start-Sleep -Milliseconds 100
  }while([DateTime]::UtcNow -lt $deadline)
  if($status.Count -ne 1){throw 'Global log search did not complete with exactly one match'}
  Record 'log complete search' (Record-Element $status[0])
  Key 13;Focus-Editor;Log-View $script:logFixture.needle 'log boundary selection' $true
  Key 27
 }
 Step 's3' 'Monitoring displays a flushed append; rotating only the owned fixture then explicitly reopening follows the replacement without stale content.' {
  Regex-Menu 'Follow New Content'
  $stream=[IO.File]::Open($script:saved,[IO.FileMode]::Append,[IO.FileAccess]::Write,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
  try{$bytes=$script:utf8.GetBytes($script:logFixture.append);$stream.Write($bytes,0,$bytes.Length);$stream.Flush($true)}finally{$stream.Dispose()}
  Log-View $script:logFixture.append.TrimEnd("`n") 'log appended viewport' $false $true
  $old=[IO.Path]::GetFullPath($script:saved);$rotated=[IO.Path]::GetFullPath((Join-Path $script:scratch 'log-rotated.txt'))
  $owned=[IO.Path]::GetFullPath($script:scratch)+[IO.Path]::DirectorySeparatorChar
  if(-not $old.StartsWith($owned,[StringComparison]::OrdinalIgnoreCase) -or -not $rotated.StartsWith($owned,[StringComparison]::OrdinalIgnoreCase) -or (Test-Path -LiteralPath $rotated)){throw 'Log rotation targets refused'}
  Move-Item -LiteralPath $old -Destination $rotated
  $script:extraArtifacts.Add($rotated)
  [IO.File]::WriteAllText($script:saved,$script:logFixture.rotated,$script:utf8)
  Record 'log rotation' @{old_bytes=(Get-Item -LiteralPath $rotated).Length;new_bytes=(Get-Item -LiteralPath $script:saved).Length}
  Regex-Menu 'Reopen and Follow'
  Log-View $script:logFixture.rotated 'log rotated viewport'
  Expect-File $script:saved $script:utf8.GetBytes($script:logFixture.rotated) 'log rotated bytes'
 }
}
