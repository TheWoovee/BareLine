# SPDX-License-Identifier: MPL-2.0
# NONSHIPPING fixture trust only. No network endpoints are contacted.
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$fixtureEvidence = Join-Path $root ('target/release-fixture/' + [Guid]::NewGuid().ToString('N'))
$prepared = Join-Path $fixtureEvidence 'release-config.prepared'
& python (Join-Path $root 'scripts/release_config.py') prepare --config (Join-Path $root 'release/fixtures/nonshipping.release-config.json') --mode fixture --output $prepared
if ($LASTEXITCODE) { throw 'Fixture configuration preparation failed' }
$previous = $env:BARELINE_PREPARED_RELEASE_CONFIG
$previousPackageDir = $env:BARELINE_RELEASE_FIXTURE_PACKAGE_DIR
$previousRuntime = $env:BARELINE_RELEASE_FIXTURE_RUNTIME
$previousOutput = $env:BARELINE_RELEASE_FIXTURE_OUTPUT

function Invoke-RequiredCargoTest {
    param([Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][string[]]$Arguments)
    $lines = $null
    & cargo @Arguments | Tee-Object -Variable lines | Out-Host
    $exitCode = $LASTEXITCODE
    if ($exitCode) { throw "$Name failed with exit $exitCode" }
    $text = (@($lines) | ForEach-Object { "$_" }) -join "`n"
    $summaries = [regex]::Matches($text, 'test result: (?:ok|FAILED)\. (?<passed>\d+) passed; (?<failed>\d+) failed;')
    $passed = ($summaries | ForEach-Object { [int]$_.Groups['passed'].Value } | Measure-Object -Sum).Sum
    $failed = ($summaries | ForEach-Object { [int]$_.Groups['failed'].Value } | Measure-Object -Sum).Sum
    if ($summaries.Count -eq 0 -or $passed -lt 1 -or $failed -ne 0) {
        throw "$Name did not execute at least one successful test (passed=$passed failed=$failed summaries=$($summaries.Count))"
    }
}

try {
    $env:BARELINE_PREPARED_RELEASE_CONFIG = $prepared
    Push-Location $root
    try {
        cargo test --locked --release -p bareline --features bareline/fixture-release --no-run fixture_config_drives_catalog_runtime_and_three_component_install_with_tamper_rejection
        if ($LASTEXITCODE) { throw 'Fixture-specific app test compile failed' }
        cargo build --locked --release --target wasm32-wasip2 -p bareline-json-tools -p bareline-xml-tools -p bareline-hex-view
        if ($LASTEXITCODE) { throw 'First-party release component build failed' }
        $componentPackages = Join-Path $fixtureEvidence 'component-packages'
        & scripts/package-first-party.ps1 -OutputDir $componentPackages
        if ($LASTEXITCODE) { throw 'First-party package assembly failed' }
        Invoke-RequiredCargoTest 'Signed update metadata fixtures' @('test', '--locked', '-p', 'bareline-distribution', '--test', 'signed_metadata')
        Invoke-RequiredCargoTest 'Local catalog install/update fixtures' @('test', '--locked', '-p', 'bareline-extensions-protocol', 'package::signed_tests::signed_offline_install_tampering_freshness_and_remove')
        Invoke-RequiredCargoTest 'Local update transport fixtures' @('test', '--locked', '-p', 'bareline-platform-windows', 'update::stage_tests::local_fixture_transport_rejects_signature_policy_certificate_cancel_corruption_and_unavailable_runtime')
        Invoke-RequiredCargoTest 'Preview disabled-reason fixture' @('test', '--locked', '-p', 'bareline', 'release_without_owner_pins_cannot_open_or_install_catalog')
        cargo build --locked --release -p bareline -p bareline-update-helper -p bareline-extension-host --features bareline/fixture-release,bareline-update-helper/fixture-release,bareline-extension-host/fixture-release
        if ($LASTEXITCODE) { throw 'Fixture-configured executable build failed' }
        $fixtureExecutables = Join-Path $fixtureEvidence 'unsigned-executables'
        [IO.Directory]::CreateDirectory($fixtureExecutables) | Out-Null
        foreach ($name in @('bareline.exe', 'bareline-update-helper.exe', 'bareline-extension-host.exe')) {
            [IO.File]::Copy((Join-Path $root "target/release/$name"), (Join-Path $fixtureExecutables $name), $false)
        }
        $deliveryEvidence = Join-Path $fixtureEvidence 'connected-delivery'
        $env:BARELINE_RELEASE_FIXTURE_PACKAGE_DIR = $componentPackages
        $env:BARELINE_RELEASE_FIXTURE_RUNTIME = Join-Path $fixtureExecutables 'bareline-extension-host.exe'
        $env:BARELINE_RELEASE_FIXTURE_OUTPUT = $deliveryEvidence
        Invoke-RequiredCargoTest 'Connected fixture-configured catalog/runtime/component delivery' @('test', '--locked', '--release', '-p', 'bareline', '--features', 'bareline/fixture-release', 'fixture_config_drives_catalog_runtime_and_three_component_install_with_tamper_rejection', '--', '--nocapture')
        $installedManifest = Join-Path $deliveryEvidence 'installed-artifacts.json'
        $installed = Get-Content -Raw -LiteralPath $installedManifest | ConvertFrom-Json
        $existingT10Runs = @((Get-ChildItem -LiteralPath (Join-Path $root 'target/first-party') -Directory -ErrorAction SilentlyContinue).Name)
        & scripts/test-first-party.ps1 -Suite Fast -HostPath $installed.artifacts.host.path -JsonPath $installed.artifacts.json.path -XmlPath $installed.artifacts.xml.path -HexPath $installed.artifacts.hex.path
        if ($LASTEXITCODE) { throw 'T10 installed-artifact three-component runner failed' }
        $newT10Runs = @(Get-ChildItem -LiteralPath (Join-Path $root 'target/first-party') -Directory | Where-Object Name -notin $existingT10Runs)
        if ($newT10Runs.Count -ne 1) { throw "Expected one retained T10 run, observed $($newT10Runs.Count)" }
        $t10Manifest = Join-Path $newT10Runs[0].FullName 'run.json'
        & packaging/windows/assemble-extension-assets.ps1 -Runtime $env:BARELINE_RELEASE_FIXTURE_RUNTIME -Catalog (Join-Path $deliveryEvidence 'catalog.json') -CatalogSignature (Join-Path $deliveryEvidence 'catalog.minisig') -JsonTools (Join-Path $componentPackages 'json-tools.blex') -XmlTools (Join-Path $componentPackages 'xml-tools.blex') -HexView (Join-Path $componentPackages 'hex-view.blex') -PreparedConfig $prepared -OutputDir (Join-Path $fixtureEvidence 'delivery-assets')
        if ($LASTEXITCODE) { throw 'Connected fixture resource inventory failed' }
        & python scripts/release_config.py manifest --prepared $prepared --artifact "editor=$(Join-Path $fixtureExecutables 'bareline.exe')" --artifact "helper=$(Join-Path $fixtureExecutables 'bareline-update-helper.exe')" --artifact "runtime=$(Join-Path $fixtureExecutables 'bareline-extension-host.exe')" --output (Join-Path $fixtureEvidence 'build-capabilities.json')
        if ($LASTEXITCODE) { throw 'Fixture capability assertion failed' }
        & python scripts/release_config.py verify-manifest --manifest (Join-Path $fixtureEvidence 'build-capabilities.json') --artifact-dir $fixtureExecutables
        if ($LASTEXITCODE) { throw 'Fixture capability manifest byte verification failed' }
        & python scripts/release_config.py fixture-report --prepared $prepared --t10-manifest $t10Manifest --installed-manifest $installedManifest --capability-manifest (Join-Path $fixtureEvidence 'build-capabilities.json') --inventory (Join-Path $fixtureEvidence 'delivery-assets/runtime-inventory.json') --inventory (Join-Path $fixtureEvidence 'delivery-assets/catalog-inventory.json') --inventory (Join-Path $fixtureEvidence 'delivery-assets/components-inventory.json') --output (Join-Path $fixtureEvidence 'fixture-delivery-report.json')
        if ($LASTEXITCODE) { throw 'Fixture execution/resource provenance report failed' }
    } finally { Pop-Location }
} finally {
    $env:BARELINE_PREPARED_RELEASE_CONFIG = $previous
    $env:BARELINE_RELEASE_FIXTURE_PACKAGE_DIR = $previousPackageDir
    $env:BARELINE_RELEASE_FIXTURE_RUNTIME = $previousRuntime
    $env:BARELINE_RELEASE_FIXTURE_OUTPUT = $previousOutput
}
Write-Output 'PASS: nonshipping local fixture trust and transport checks; no network or owner endpoint used.'
