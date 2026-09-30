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
# Native C/C++ code compiled by crates; the Cargo SBOM lists only those crates.
$native=@((Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'native-components.psd1')).Components | ForEach-Object {
    $files=@(if ($_.SourceDirectory) { Get-ChildItem -LiteralPath (Join-Path $repo $_.SourceDirectory) -File -Recurse | Sort-Object FullName | ForEach-Object {
        [ordered]@{path=[IO.Path]::GetRelativePath($repo,$_.FullName).Replace('\','/');sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}
    } })
    $_ + @{Files=$files}
})
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
    $license=if($component.License -cmatch ' (WITH|OR|AND) '){@{expression=$component.License}}elseif($component.License -match '^[A-Za-z0-9.+-]+$'){@{license=@{id=$component.License}}}else{@{license=@{name=$component.License}}}
    $entry=[ordered]@{type='library';name=$component.Name;version=$component.Version;'bom-ref'="native:$($component.Name)";description=$component.Evidence;licenses=@($license)}
    if($component.Cpe){$entry['cpe']=$component.Cpe}
    if($component.Purl){$entry['purl']=$component.Purl}
    if($component.Files.Count){$entry['components']=@($component.Files | ForEach-Object {[ordered]@{type='file';name=$_.path;'bom-ref'="source:$($_.path)";hashes=@(@{alg='SHA-256';content=$_.sha256})}})}
    # Record the native library under the crate that compiles it; refuse a stale identity.
    $carriers=@($sbom.components | Where-Object {$_['name'] -eq $component.CargoPackage})
    if(-not $carriers.Count){throw "Missing Cargo component for native $($component.Name): $($component.CargoPackage)"}
    if($component.CargoVersion -and @($carriers | Where-Object {$_['version'] -ne $component.CargoVersion}).Count){throw "Native $($component.Name) $($component.Version) is recorded for $($component.CargoPackage) $($component.CargoVersion); update native-components.psd1"}
    foreach($carrier in $carriers){
        $edge=@($sbom.dependencies | Where-Object {$_['ref'] -eq $carrier['bom-ref']})
        if($edge.Count){$edge[0]['dependsOn']=@(@($edge[0]['dependsOn'] | Where-Object {$_})+"native:$($component.Name)")}
        else{$sbom.dependencies+= [ordered]@{ref=$carrier['bom-ref'];dependsOn=@("native:$($component.Name)")}}
    }
    $sbom.components+=$entry
}
$sbom.components+=@($packaging | ForEach-Object {[ordered]@{type='file';name=$_.path;'bom-ref'="source:$($_.path)";hashes=@(@{alg='SHA-256';content=$_.sha256})}})
$result=Normalize $sbom
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputFile),($result|ConvertTo-Json -Depth 100),[Text.UTF8Encoding]::new($false))
