# Reusing the focused defect evidence

These are audit probes, not production fixes or a replacement regression suite. The recorded outputs were produced on the source snapshot in `source-manifest.json`. Port their state/barrier cases into the owning crate when implementing the corresponding PR.

## Task cancellation, worker loss and scheduling

From the `Bareline-Editor` root in PowerShell, compile `task_probe.rs` using the installed Rust toolchain:

```powershell
$auditEvidence = Join-Path (Get-Location) 'docs/qa/2026-09-12-local-audit/evidence'
rustc --edition=2024 (Join-Path $auditEvidence 'task_probe.rs') -o (Join-Path $auditEvidence 'task_probe-current.exe')
if ($LASTEXITCODE -ne 0) { throw 'Probe compilation failed' }
& (Join-Path $auditEvidence 'task_probe-current.exe')
```

The relative include reads the current `crates/app/src/task.rs`. The intentional worker panic happens in this standalone process with Rust's default unwind behavior. It is not an editor crash experiment. Current `task-probe.log` shows success returned after cancellation, Drop without cancellation, stale worker metrics, and a runnable job stranded behind the busy worker. Some observation waits are bounded exploratory probes; final regressions must use state/barrier synchronization per PR-T06/T07.

## Migration and cache sweep

`launch_probe.rs` contains an unchanged extraction of the audited helper functions. **It is frozen evidence.** After a fix, compare or replace that extraction from current `launch.rs` before using it as a before/after check; executing the frozen file cannot validate the new implementation.

Compile with `rustc --edition=2024`, as above. Run the executable with its working directory set to a **new, uniquely named generated fixture directory**, not the project root. Its hard-coded relative suffix is `docs/qa/2026-09-12-local-audit/evidence/launch-probe-fixture`. Pre-create only the following layout under that unique directory:

```text
docs/qa/2026-09-12-local-audit/evidence/launch-probe-fixture/
  outside-temp/cache-4294967294-1/keep.txt   (generated sentinel)
  temp/Bareline-junction                   (junction to that outside-temp)
```

Use PowerShell `New-Item -ItemType Junction` for the junction. Resolve and verify both absolute locations remain below the unique fixture directory before execution. The helper intentionally deletes the generated sentinel through the junction; no personal profile, shared TEMP directory or pre-existing file belongs in this fixture. The probe creates `roaming`/`local` itself, holds a generated settings file with a no-sharing handle, attempts migration twice, and prints whether retry recovered the file. Use a fresh directory each run. Do not recursively clean up an unchecked path or junction.

## Save As conflict and concurrent replacement

`save_probe.rs` calls actual document/file-io code and delegates its injected interleaving to `WindowsFileSystem.commit`. Build the current selected dependencies from the project root:

```powershell
$auditEvidence = Join-Path (Get-Location) 'docs/qa/2026-09-12-local-audit/evidence'
$artifactLines = & cargo build --offline --locked -p bareline --message-format=json
if ($LASTEXITCODE -ne 0) { throw 'Current dependency build failed' }
$artifactRows = $artifactLines | ForEach-Object { $_ | ConvertFrom-Json }
$rustArgs = @('--edition=2024', (Join-Path $auditEvidence 'save_probe.rs'), '-L', ('dependency=' + (Join-Path (Get-Location) 'target/debug/deps')))
foreach ($crateName in @('bareline_document', 'bareline_file_io', 'bareline_platform', 'bareline_platform_windows')) {
    $rlibs = @($artifactRows | Where-Object { $_.reason -eq 'compiler-artifact' -and $_.target.name -eq $crateName } | ForEach-Object { $_.filenames } | Where-Object { $_.EndsWith('.rlib') })
    if ($rlibs.Count -ne 1) { throw "Ambiguous current artifact for $crateName" }
    $rustArgs += @('--extern', ($crateName + '=' + $rlibs[0]))
}
$rustArgs += @('-o', (Join-Path $auditEvidence 'save_probe-current.exe'))
& rustc @rustArgs
if ($LASTEXITCODE -ne 0) { throw 'Save probe compilation failed' }
```

Run it from a new uniquely named generated directory. Create its parent suffix `docs/qa/2026-09-12-local-audit/evidence` there first; leave `save-probe-fixture` absent, since the program uses `create_dir` to reject reuse. The executable creates only its own `existing.txt`/staging files. Recorded output demonstrates that a different existing Save As destination conflicts and that bytes injected immediately before real Windows replacement are lost while save returns success. This is controlled interleaving, not a random native concurrency test. After PR-T03/T04 change the public API, update the probe to the new prepared-destination/receipt contract and preserve the original log for comparison.

The commands above are reproducibility instructions. The retained dated logs, not the existence of these instructions, establish which checks were executed during this audit. Use [TEST_REPORT.md](../TEST_REPORT.md) for exact source, binary and scope provenance.
