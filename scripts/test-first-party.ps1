# SPDX-License-Identifier: MPL-2.0
$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..')
try {
    rustup target add wasm32-wasip2
    if ($LASTEXITCODE) { throw 'WASI target installation failed' }
    cargo build --locked --release --target wasm32-wasip2 -p bareline-json-tools -p bareline-xml-tools -p bareline-hex-view
    if ($LASTEXITCODE) { throw 'Component build failed' }
    cargo test --locked --release -p bareline-extension-host --test first_party -- --ignored --nocapture
    if ($LASTEXITCODE) { throw 'Isolated component verification failed' }
} finally { Pop-Location }

