# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param([Parameter(Mandatory)][string]$First,[Parameter(Mandatory)][string]$Second)
$ErrorActionPreference='Stop'
$names=@('bareline.exe','bareline-update-helper.exe','LICENSE','THIRD-PARTY-NOTICES.md','SBOM.json')
foreach($name in $names){
    $one=Get-Item -LiteralPath (Join-Path $First $name);$two=Get-Item -LiteralPath (Join-Path $Second $name)
    if($one.PSIsContainer -or $two.PSIsContainer -or ($one.Attributes -band [IO.FileAttributes]::ReparsePoint) -or ($two.Attributes -band [IO.FileAttributes]::ReparsePoint)){throw "Unsafe payload: $name"}
    if((Get-FileHash -LiteralPath $one.FullName -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath $two.FullName -Algorithm SHA256).Hash){throw "Independent payloads differ: $name"}
}
'PASS: independent normalized payload bytes match'
