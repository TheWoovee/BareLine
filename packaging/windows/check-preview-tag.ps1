#requires -Version 7.0
# SPDX-License-Identifier: MPL-2.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$')][string]$Version,
    [Parameter(Mandatory)][string]$Tag
)
$ErrorActionPreference = 'Stop'
$pattern = '^v' + [regex]::Escape($Version) + '-preview\.[1-9][0-9]*(\.(0|[1-9][0-9]*))*$'
if ($Tag -cnotmatch $pattern) { throw "Expected v$Version-preview.<number>[.<number>...] matching the Cargo version; received '$Tag'." }
Write-Output "Validated unsigned preview tag: $Tag"
