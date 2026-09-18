$ErrorActionPreference = 'Stop'
cargo fmt --all --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
python .github/workflows/check_portability.py
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
python .github/workflows/check_toolchain_contract.py --metadata target/qualification/full-round-20260918/02-metadata.json.stdout.log
exit $LASTEXITCODE
