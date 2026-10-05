# SPDX-License-Identifier: MPL-2.0
. (Join-Path $PSScriptRoot 'native_regex_transform.ps1')
function Portable-Inventory([string]$root) {
 $items=@();$total=0;$prefix=[IO.Path]::GetFullPath($root)+[IO.Path]::DirectorySeparatorChar
 foreach($file in Get-ChildItem -LiteralPath $root -Recurse -File) {
  if(($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -or -not $file.FullName.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase)){throw 'Portable inventory path refused'}
  $total+=$file.Length
  if($items.Count -ge 4096 -or $total -gt 2147483648){throw 'Portable inventory budget exceeded'}
  $items+=@{path=$file.FullName.Substring($prefix.Length);sha256=(Hash-File $file.FullName);bytes=$file.Length}
 }
 @($items | Sort-Object {$_['path']})
}
function Portable-Containment([string]$stage) {
 $files=@(foreach($root in @($script:local,$script:roaming)){Get-ChildItem -LiteralPath $root -Recurse -File | ForEach-Object {$_.FullName}})
 Record $stage @{external_profile_files=$files;portable_root=$script:portableRoot}
 if($files.Count -ne 0){throw 'Persistent state escaped portable data root'}
}
function Run-Portable {
 Step 's1' 'The pinned editor runs from an isolated portable layout; editing its own settings through the editor persists exact defaults under data, with no installed-profile files.' {
  Focus-Editor;Key 79 $true;File-Dialog $script:settings
  Key 65 $true
  $lines=$script:portableFixture.settings -split "`n",0,'SimpleMatch'
  for($i=0;$i -lt $lines.Count;$i++){if($i -gt 0){Key 13};Text $lines[$i]}
  Key 83 $true
  Expect-File $script:settings $script:utf8.GetBytes($script:portableFixture.settings) 'portable settings edited'
  Key 87 $true;Key 79 $true;File-Dialog $script:saved
  Expect-Text $script:portableFixture.document 'portable session source'
  Portable-Containment 'portable containment before'
 }
 Step 's2' 'Clean Exit writes the portable session; the entire owned package and state are moved to a new directory with identical file identities.' {
  Close-OwnedEditor
  $before=Portable-Inventory $script:portableRoot
  if(-not [IO.File]::Exists((Join-Path $script:profile 'session.json'))){throw 'Portable session file missing'}
  $old=[IO.Path]::GetFullPath($script:portableRoot);$destination=[IO.Path]::GetFullPath((Join-Path $script:scratch 'portable-relocated'))
  $owned=[IO.Path]::GetFullPath($script:scratch)+[IO.Path]::DirectorySeparatorChar
  if(-not $old.StartsWith($owned,[StringComparison]::OrdinalIgnoreCase) -or -not $destination.StartsWith($owned,[StringComparison]::OrdinalIgnoreCase) -or (Test-Path -LiteralPath $destination)){throw 'Portable move targets refused'}
  Move-Item -LiteralPath $old -Destination $destination
  $script:portableRoot=$destination;$script:profile=Join-Path $destination 'data';$script:settings=Join-Path $script:profile 'settings.toml'
  $after=Portable-Inventory $destination
  Record 'portable relocation' @{old_exists=[IO.Directory]::Exists($old);inventory_before=$before;inventory_after=$after}
  if(($before | ConvertTo-Json -Depth 5 -Compress) -cne ($after | ConvertTo-Json -Depth 5 -Compress)){throw 'Portable relocation changed package/state bytes'}
 }
 Step 's3' 'The relocated editor restores its local session without installation; a newly saved document uses the persisted UTF-16BE default and all profile state stays portable.' {
  Start-OwnedEditor (Join-Path $script:portableRoot 'bareline.exe') @('--software','--no-extensions','--new-instance')
  Focus-Editor;Expect-Text $script:portableFixture.document 'portable restored session'
  Key 78 $true;Text $script:portableFixture.new_text
  Key 83 $true $true;$target=Join-Path $script:scratch 'relocated-new.txt';File-Dialog $target
  $encoding=[Text.UnicodeEncoding]::new($true,$true,$true)
  [byte[]]$expected=$encoding.GetPreamble()+$encoding.GetBytes($script:portableFixture.new_text)
  Expect-File $target $expected 'portable relocated encoding';$script:extraArtifacts.Add($target)
  Portable-Containment 'portable containment after'
  $script:extraArtifacts.Add((Join-Path $script:profile 'session.json'))
 }
}
