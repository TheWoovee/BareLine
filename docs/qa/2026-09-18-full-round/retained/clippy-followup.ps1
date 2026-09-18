$ErrorActionPreference = 'Stop'
$root = 'target/qualification/full-round-20260918'
cargo clippy --workspace --all-targets --locked --message-format=json 1> "$root/clippy-followup.jsonl" 2> "$root/clippy-followup.stderr.log"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
python .github/workflows/check_clippy_ratchet.py check --diagnostics "$root/clippy-followup.jsonl" --baseline .github/workflows/clippy-baseline.json --source-root . --platform Windows --target x86_64-pc-windows-msvc --toolchain 1.98.1
exit $LASTEXITCODE
