# PR-019 source benchmark methodology

Build once with the intended profile, then run the built harness directly to avoid recompiling during samples:

```powershell
cargo build -p xtask --release
.\target\release\xtask.exe perf document --bytes 1048576 --samples 2
.\target\release\xtask.exe perf document --bytes 10485760 --samples 5
.\target\release\xtask.exe perf document --bytes 104857600 --samples 5
.\target\release\xtask.exe perf document --bytes 1073741824 --samples 5
.\target\release\xtask.exe perf document --bytes 5368709120 --samples 5 --long-line
```

The last two commands scan the full fixture on each sample and require corresponding free disk space; run separately after development settles. Default smoke is 1 MiB, one sample of each source policy. JSON raw results are in ignored `tests/perf/results/`. Generation creates a fresh file exclusively, with 128-byte ASCII lines or one long ASCII line, and deletes it after measurement. OS cache state is uncontrolled and generation warms it. These measurements are neither cold open nor end-to-end editing latency. Sample order alternates resident/paged. Sources above 256 MiB run only paged. The absent single-byte literal avoids result-cap truncation and boundary ambiguity. Budget values count reservations, not full process memory.

For the remaining release comparison, pin executable SHA-256 hashes, exact OS/build/CPU, renderer/config, Notepad++ stable version with plugins disabled and fixture hashes. Alternate application order within each pair on the same machine. Record all trials, timeouts, exclusions and sample counts; report P50/P95 separately for first frame, editable viewport, full load, scroll, edit-to-paint, syntax, search/cancel/jump, save, workspace and append. Measure private bytes and working set independently, with process-tree totals for extensions and 100/500 populated tabs. Keep hosted CI as a separate noise series. Preserve hardware default unless software frame time is within 10% and saves >5 MB idle private bytes. Retain 256 MiB threshold until measured crossover data supports changing it. Do not infer marketing comparisons from this source harness.

Use WPR/ETW and WPA CPU, file I/O and memory views around separately labelled workloads to identify blocking reads, allocation peaks and frame stalls; record tool profile and trace alongside raw data. Private-byte point samples here are not peak values. Instrumented or profiled runs must be a separate series from normal release timings.

Add `--product` to include one fixed 61,440-byte product regex search, two shared-snapshot open-document searches, real diff first callback/total cost, and disk-backed recovery baseline/durable-edit/replay measurement. This optional set runs once, irrespective of source `--samples`. It preserves completeness (including coarse/time) and owned recovery artifacts; it never substitutes an incomplete diff for an exact result. Source samples now include edit, undo and redo costs. These product workloads use actual owning modules; only the separately named absent-literal paged traversal remains harness code.
