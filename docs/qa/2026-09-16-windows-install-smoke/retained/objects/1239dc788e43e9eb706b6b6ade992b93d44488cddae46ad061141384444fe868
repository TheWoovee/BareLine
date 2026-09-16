# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param([Parameter(Mandatory)][string]$CargoSbom,[string]$HelperCargoSbom,[Parameter(Mandatory)][string]$OutputFile)
$ErrorActionPreference='Stop'
$repo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$sbom=Get-Content -LiteralPath $CargoSbom -Raw | ConvertFrom-Json -AsHashtable
if ($sbom.bomFormat -ne 'CycloneDX') {throw 'Expected CycloneDX input'}
if(-not $HelperCargoSbom){$HelperCargoSbom=Join-Path $repo 'apps/update-helper/bom.json'}
$helper=Get-Content -LiteralPath $HelperCargoSbom -Raw | ConvertFrom-Json -AsHashtable
if($helper.bomFormat -ne 'CycloneDX'){throw 'Expected helper CycloneDX input'}
$sbom.components=@($sbom.components)+@($helper.components)+@($sbom.metadata.component,$helper.metadata.component | Where-Object {$_})
$sbom.dependencies=@($sbom.dependencies)+@($helper.dependencies)
# Remove build-local metadata, retaining package identities, licenses and dependency edges.
$sbom.Remove('serialNumber'); $sbom.Remove('metadata')
$refs=@{}
function Register-Components($components) {
    foreach ($component in $components) {
        $component.Remove('evidence'); $component.Remove('properties')
        # Local Cargo download URLs describe the build checkout, not package identity.
        # Preserve any target subpath while removing only the local URL qualifier.
        if ($component.purl -match '^([^?#]+)\?download_url=file://[^#]*(#.*)?$') {
            $component.purl = $Matches[1] + $Matches[2]
        }
        if ($component['bom-ref']) {
            $identity = if ($component.purl) { $component.purl } else { "component:$($component.name)@$($component.version)" }
            # Keep Cargo's target discriminator distinct from its parent package.
            if ($component['bom-ref'] -match ' ([-A-Za-z0-9]+-target-\d+)$') { $identity += ':' + $Matches[1] }
            $refs[$component['bom-ref']] = $identity
        }
        if ($component.components) { Register-Components $component.components }
    }
}
Register-Components $sbom.components
$native=@()
foreach ($component in @('lexilla','scintilla')) {
    $directory=Join-Path $repo "native/lexilla-bridge/bundled/$component"
    $files=@(Get-ChildItem -LiteralPath $directory -File -Recurse | Sort-Object FullName | ForEach-Object {
        [ordered]@{path=[IO.Path]::GetRelativePath($repo,$_.FullName).Replace('\','/');sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}
    })
    $native+= [ordered]@{name=$component;licenseFile="native/lexilla-bridge/bundled/$component/License.txt";files=$files}
}
$packaging=@(Get-ChildItem -LiteralPath $PSScriptRoot -File -Recurse | Sort-Object FullName | ForEach-Object {
    [ordered]@{path=[IO.Path]::GetRelativePath($repo,$_.FullName).Replace('\','/');sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}
})
function Normalize($value) {
    if ($value -is [Collections.IDictionary]) { $result=[ordered]@{}; foreach($key in @($value.Keys | Sort-Object)) {$result[$key]=Normalize $value[$key]}; return $result }
    if ($value -is [string]) { if($refs.ContainsKey($value)){return $refs[$value]}; if ($value.Contains($repo,[StringComparison]::OrdinalIgnoreCase) -or $value -match '(file://|(?<![A-Za-z])[A-Za-z]:[\\/])') {throw 'Absolute developer path remains in SBOM'}; return $value }
    if ($value -is [Collections.IEnumerable]) {return ,@($value | ForEach-Object {Normalize $_} | Sort-Object {$_ | ConvertTo-Json -Compress -Depth 100})}
    return $value
}
$sbom=Normalize $sbom
$sbom.components=@($sbom.components | Group-Object {$_['bom-ref']} | ForEach-Object {$_.Group[0]})
$sbom.dependencies=@($sbom.dependencies | Group-Object {$_.ref} | ForEach-Object {[ordered]@{ref=$_.Name;dependsOn=@($_.Group | ForEach-Object {$_.dependsOn} | Sort-Object -Unique)}})
foreach($component in $native){
    $sbom.components+= [ordered]@{type='library';name=$component.name;'bom-ref'="native:$($component.name)";licenses=@(@{license=@{name='License for Lexilla, Scintilla, and SciTE'}});components=@($component.files | ForEach-Object {[ordered]@{type='file';name=$_.path;'bom-ref'="source:$($_.path)";hashes=@(@{alg='SHA-256';content=$_.sha256})}})}
}
$sbom.components+=@($packaging | ForEach-Object {[ordered]@{type='file';name=$_.path;'bom-ref'="source:$($_.path)";hashes=@(@{alg='SHA-256';content=$_.sha256})}})
$result=Normalize $sbom
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputFile),($result|ConvertTo-Json -Depth 100),[Text.UTF8Encoding]::new($false))
