# SPDX-License-Identifier: MPL-2.0
# Reuse bounded native control discovery, exact input and foreground guards.
. (Join-Path $PSScriptRoot 'native_regex_transform.ps1')
function Run-Column {
 Step 's1' 'Three empty UIA selections follow display column four across a tab, wide glyph and short line; source bytes remain exact.' {
  Focus-Editor;Key 79 $true;File-Dialog $script:saved
  Expect-Text $script:columnFixture.initial 'column opened'
  Key 36 $true;Key 39;Key 39
  Record 'column anchor' (Record-Element (Editor))
  [JourneyInput]::ModifiedKey($script:window,40,$false,$true,$true)
  [JourneyInput]::ModifiedKey($script:window,40,$false,$true,$true)
  $deadline=[DateTime]::UtcNow.AddSeconds(5)
  do {
   Guard;$actual=Record-Element (Editor)
   $positions=@($actual.selections | ForEach-Object {$_.start_utf16})
   if($positions.Count -eq 3 -and ($positions -join ',') -ceq ($script:columnFixture.selection_offsets -join ',') -and @($actual.selections | Where-Object {$_.text -cne ''}).Count -eq 0){break}
   Start-Sleep -Milliseconds 50
  } while([DateTime]::UtcNow -lt $deadline)
  Record 'column selections' $actual
  if($positions.Count -ne 3 -or ($positions -join ',') -cne ($script:columnFixture.selection_offsets -join ',')){throw 'Display-column selection offsets differ from fixture'}
  Expect-File $script:saved $script:utf8.GetBytes($script:columnFixture.initial) 'column original disk'
 }
 Step 's2' 'One native Column Editor Apply inserts at every display column and pads the short line; Save writes exact bytes.' {
  Regex-Menu 'Column Editor'
  Regex-Field 'Repeated text' $script:columnFixture.text 'column insertion input'
  Key 13
  Expect-Text $script:columnFixture.inserted 'column inserted';Expect-Dirty $true 'column inserted dirty'
  Expect-File $script:saved $script:utf8.GetBytes($script:columnFixture.initial) 'column unsaved disk'
  Focus-Editor;Key 83 $true
  Expect-File $script:saved $script:utf8.GetBytes($script:columnFixture.inserted) 'column inserted disk'
  Copy-Item -LiteralPath $script:saved -Destination (Join-Path $script:scratch 'column-inserted.bin')
 }
 Step 's3' 'One native Undo restores every original row in one transaction; Save restores original UTF-8/LF bytes.' {
  Focus-Editor;Key 90 $true
  Expect-Text $script:columnFixture.initial 'column one Undo'
  Expect-File $script:saved $script:utf8.GetBytes($script:columnFixture.inserted) 'column Undo disk unchanged'
  Key 83 $true
  Expect-File $script:saved $script:utf8.GetBytes($script:columnFixture.initial) 'column restored disk'
  Expect-Dirty $false 'column restored clean'
 }
}
