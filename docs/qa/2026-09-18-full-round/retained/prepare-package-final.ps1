$ErrorActionPreference = 'Stop'
$repo = (Get-Location).Path
$payload = Join-Path $repo 'target/package-20260918-full-round-final/payload'
$output = Join-Path $repo 'dist/windows/0.1.0-full-round-final-20260918'
if ((Test-Path -LiteralPath $payload) -or (Test-Path -LiteralPath $output)) { throw 'Expected fresh package directories' }
$build = Get-Content -LiteralPath 'target/qualification/full-round-20260918/40-ci-followup-preview-build.json' -Raw | ConvertFrom-Json
if ($build.exit_code -ne 0 -or $build.source_changed_during_run) { throw 'Current build must pass with stable source' }
[IO.Directory]::CreateDirectory($payload) | Out-Null
Copy-Item -LiteralPath 'target/release/bareline.exe','target/release/bareline-update-helper.exe','LICENSE' -Destination $payload
& ./packaging/windows/generate-notices.ps1 -OutputFile (Join-Path $payload 'THIRD-PARTY-NOTICES.md') -Roots bareline,bareline-update-helper
$lockBefore = (Get-FileHash -LiteralPath 'Cargo.lock' -Algorithm SHA256).Hash
$cyclonedx = Join-Path $repo 'target/package-20260908/cyclonedx/cargo-cyclonedx.exe'
& $cyclonedx cyclonedx --format json --all --no-default-features --target x86_64-pc-windows-msvc --override-filename bareline-full-round-final-20260918.cdx
if ($LASTEXITCODE -ne 0) { throw 'CycloneDX generation failed' }
if ((Get-FileHash -LiteralPath 'Cargo.lock' -Algorithm SHA256).Hash -cne $lockBefore) { throw 'CycloneDX changed Cargo.lock' }
& ./packaging/windows/generate-sbom.ps1 -CargoSbom 'apps/bareline/bareline-full-round-final-20260918.cdx.json' -HelperCargoSbom 'apps/update-helper/bareline-full-round-final-20260918.cdx.json' -OutputFile (Join-Path $payload 'SBOM.json')
& ./packaging/windows/build.ps1 -PayloadDir $payload -Version '0.1.0' -OutputDir $output -Installer -Iscc (Join-Path $repo 'target/package-20260908/inno-archive/tools/ISCC.exe')
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed' }
$files = @('bareline.exe','bareline-update-helper.exe','LICENSE','THIRD-PARTY-NOTICES.md','SBOM.json')
$inventory = @($files | ForEach-Object { $file = Get-Item -LiteralPath (Join-Path $payload $_); [ordered]@{name=$file.Name;length=$file.Length;sha256=(Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()} })
[ordered]@{version='0.1.0';mode='unsigned-preview';lock_sha256=$lockBefore.ToLowerInvariant();payload=$payload;output=$output;files=$inventory} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath 'target/qualification/full-round-20260918/package-manifest-final.json' -Encoding utf8
Write-Output $output
