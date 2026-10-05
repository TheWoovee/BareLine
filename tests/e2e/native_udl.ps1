# SPDX-License-Identifier: MPL-2.0
. (Join-Path $PSScriptRoot 'native_regex_transform.ps1')
. (Join-Path $PSScriptRoot 'native_code_config.ps1')
function Run-Udl {
 Step 's1' 'Native Notepad++ UDL import reports its mappings; the validated definition is durably installed under the isolated profile.' {
  Regex-Menu 'Import User-defined Language';File-Dialog (Join-Path $script:scratch 'fixture-udl.xml')
  $null=Regex-Status $script:udlFixture.import_status 'udl import report'
  $path=Join-Path $script:profile 'languages/qa-fixture.json'
  if(-not [IO.File]::Exists($path) -or (Get-Item -LiteralPath $path).Length -gt 131072){throw 'Durable UDL definition missing or excessive'}
  $definition=Get-Content -LiteralPath $path -Raw -Encoding UTF8 | ConvertFrom-Json
  Record 'udl installed definition' $definition;$script:extraArtifacts.Add($path)
  Key 27
 }
 Step 's2' 'Opening the matching extension applies the installed keyword/string/number styles to exact source text.' {
  Focus-Editor;Key 79 $true;File-Dialog $script:saved
  Expect-Text $script:udlFixture.initial 'udl sample'
  Observe-Highlighting
  Copy-Item -LiteralPath (Join-Path $script:scratch 'code-highlighting.png') -Destination (Join-Path $script:scratch 'udl-before-restart.png')
  $script:extraArtifacts.Add((Join-Path $script:scratch 'udl-before-restart.png'))
 }
 Step 's3' 'A fresh owned editor loads the installed catalog and reapplies the imported language to the matching extension.' {
  Close-OwnedEditor;Start-OwnedEditor
  Focus-Editor;Key 79 $true;File-Dialog $script:saved
  Expect-Text $script:udlFixture.initial 'udl restarted sample'
  Observe-Highlighting
  # Give the second raw observation its own identity, preserving the first.
  $last=$script:records[$script:records.Count-1]
  if($last.stage -cne 'code highlighting'){throw 'Restarted highlighting observation missing'}
  $last.stage='udl restarted highlighting'
  Record 'udl restart complete' @{definition_sha256=(Hash-File (Join-Path $script:profile 'languages/qa-fixture.json'))}
  Expect-File $script:saved $script:utf8.GetBytes($script:udlFixture.initial) 'udl source unchanged'
 }
}
