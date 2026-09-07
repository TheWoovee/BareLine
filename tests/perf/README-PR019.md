# PR-019 source benchmark methodology

Current source handoff (2026-09-08): the commands below are deferred until the
integration coordinator releases the benchmark phase. No new measurements, app
launches or manual QA were performed for this tooling update.

The paired entry points are `make_manifest.py`, `perf_suite.py run`, and
`perf_suite.py report`. Manifest preparation hashes the selected Python executable,
driver/support sources, application binaries, configurations and fixture. It does
not launch applications. Both applications require exact four-part PE versions;
the adapters check x64 architecture and version before launching. Supply a generated
fixture and explicit expected Scintilla text-view byte length (which may differ
from encoded disk length). Each driver makes its own temporary fixture copy.

```powershell
python tests/perf/make_manifest.py --help
python tests/perf/perf_suite.py run <reviewed-manifest.json> tests/perf/results
python tests/perf/perf_suite.py report <printed-run-directory> <new-report.json>
```

Manifest preparation requires `--bareline`, `--bareline-version`,
`--bareline-config` (settings.toml), corresponding `--notepadpp` options
(config.xml), `--fixture`, `--expected-text-bytes`, one or more `--scenario`,
`--machine-id`, `--configuration`, and `--destination`. `--renderer` selects the
Bareline renderer; repeat the series separately for software and hardware. The
configuration identity must describe OS/build, CPU, display/DPI, theme/font,
wrapping, syntax and power settings. Exact copied configuration hashes are also
retained. The generated manifest uses uncontrolled/warmed cache labels and never
claims a cold-cache protocol. Fixtures of different sizes need separate manifests.

The orchestrator alternates application order within every pair, drains bounded
stdout/stderr concurrently, enforces per-trial deadlines, retains failed/missing
trials and writes create-new raw artifacts. Reports preserve independent
application observations. Ratios require an explicit reviewed `comparable_metrics`
list and complete matched pairs; the generated list is empty. A matching metric
name is insufficient: Bareline present receipts and Notepad++ Scintilla message
roundtrips have different endpoints. Search primitives and termination boundaries
also differ. No report automatically becomes eligible for marketing claims.

`bareline_driver.py` launches only its suspended/Job-contained process, with
session/extensions disabled and all state under the temporary performance root.
It reads the native create-new result receipt after successful process exit and
adds the same process-tree sampler used by the Notepad++ adapter. The marker file
`.bareline-perf` is required; ordinary launches never enable this workload path.
Native deadlines are bounded to 120 seconds. Save As uses a new file within the
owned root. The wrappers never operate on the supplied original fixture.

Native source workloads: open/long-line viewport, 120 bounded scroll steps,
edit-to-present, resident literal/regex completed Find, result jump, Save As and
100/500 populated tabs. The paired manifest builder exposes only scenarios both
adapters accept. Native paged search fails closed rather than measuring a preview.
Notepad++ supports additional isolated Save and launch/idle adapters; see
README-WINDOWS-ADAPTERS.md for exact receipt semantics and configuration constraints.
Memory peaks are maxima of complete sampled live Job totals, not OS lifetime peaks.
Missing process samples fail the measurement; sampling can miss short-lived peaks.

Still unsupported by the native paired driver: controlled cold launch/cache
preparation, comparable full-load milestone, syntax-ready acknowledgement, full
paged Find/cancellation, workspace scan, tail append, extension-runtime totals and
comparable Save/Save As completion. Notepad++ also lacks paint completion,
Find-dialog cancellation, workspace/tail and populated-tab adapters. These remain
explicit source/evidence gaps, not zero measurements. Source/product harness
measurements below are distinct and cannot fill native endpoint gaps.

The opt-in nightly series remains Bareline-only hosted evidence. Regression
reporting requires a separate repository opt-in and exactly the previous seven
completed runs, with all seven reports present and matching configuration. It uses
P50 increase above 10% and observed baseline noise, retaining commit history. Missing
history produces no issue. Numeric findings never fail CI. This workflow definition
does not authorize running it or publishing an issue in the current session.

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
