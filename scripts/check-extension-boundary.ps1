# PR-016 / ADR-09: optional Wasmtime/WASI may never enter the editor graph.
$ErrorActionPreference = 'Stop'
$editorTree = & cargo tree -p bareline --target x86_64-pc-windows-msvc --edges normal,build --prefix none
if ($LASTEXITCODE -ne 0) { throw 'Unable to inspect editor dependency graph.' }
$forbidden = $editorTree | Select-String '^(wasmtime|wasmtime-wasi|tokio|async-std|smol)( |-)'
if ($forbidden) { throw "Forbidden editor runtime dependency: $forbidden" }
$hostTree = & cargo tree -p bareline-extension-host --target x86_64-pc-windows-msvc --edges normal,build --prefix none
if ($LASTEXITCODE -ne 0 -or -not ($hostTree | Select-String '^wasmtime v')) { throw 'Optional host runtime dependency is missing.' }
Write-Output 'Editor runtime boundary passed; Wasmtime/WASI remain in optional host.'
