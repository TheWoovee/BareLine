# SPDX-License-Identifier: MPL-2.0
# Shared bootstrap layout. Verification uses the same Rust trust code as the app.
function Get-AuthorityPayloadFiles([string]$Directory, [switch]$Required) {
    $names = @('bareline.release-authority.json', 'bareline.release-authority.minisig')
    $present = @($names | Where-Object { Test-Path -LiteralPath (Join-Path $Directory $_) })
    $transition = Test-Path -LiteralPath (Join-Path $Directory 'bareline.root-transitions.json')
    if ($Required -or $present.Count -or $transition) {
        if ($present.Count -ne 2) { throw 'Both signed release authority files are required' }
        if ($transition) { $names += 'bareline.root-transitions.json' }
        foreach ($name in $names) {
            $item = Get-Item -LiteralPath (Join-Path $Directory $name)
            if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Unsafe authority file: $name" }
        }
        return $names
    }
}

# Authenticode identity rule shared with the product (SEC-08): the signer's subject and
# issuer simple display names must match the pin, and the leaf must carry the code-signing
# EKU. Never a leaf certificate hash: renewals and short-lived leaves keep the pin.
function Test-AuthenticodePublisher([string]$Path, [string]$Subject, [string[]]$Issuers) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or -not $signature.SignerCertificate) { throw "Invalid Authenticode: $Path" }
    $certificate = $signature.SignerCertificate
    $simple = [Security.Cryptography.X509Certificates.X509NameType]::SimpleName
    if (-not $Subject -or -not $Issuers -or $certificate.GetNameInfo($simple, $false) -cne $Subject -or
        $certificate.GetNameInfo($simple, $true) -cnotin $Issuers) { throw "Unexpected publisher: $Path" }
    $usages = @($certificate.Extensions | Where-Object { $_ -is [Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension] } |
        ForEach-Object { $_.EnhancedKeyUsages } | ForEach-Object { $_.Value })
    if ('1.3.6.1.5.5.7.3.3' -notin $usages) { throw "Signer lacks the code-signing EKU: $Path" }
}

function Test-AuthorityPayload([string]$Directory, [string]$Config, [string]$Verifier) {
    if (-not $Config -or -not $Verifier) { throw 'Authority requires explicit release config and verifier executable' }
    & python (Join-Path $PSScriptRoot '../../scripts/release_config.py') validate --config $Config | Out-Null
    if ($LASTEXITCODE) { throw 'Invalid public release configuration' }
    $configData = Get-Content -LiteralPath $Config -Raw | ConvertFrom-Json
    if ($configData.mode -ne 'configured') { throw 'Shipping authority requires configured release mode' }
    $null = @(Get-AuthorityPayloadFiles $Directory -Required)
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $result = & $Verifier --config $Config --directory $Directory --now $now
    if ($LASTEXITCODE) { throw 'Signed bootstrap authority verification failed' }
    $result | ConvertFrom-Json
}
