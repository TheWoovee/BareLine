# PR-019 source benchmark methodology

Current source handoff (2026-09-08): the commands below are deferred until the
integration coordinator releases the benchmark phase. No new measurements, app
launches or manual QA were performed for this tooling update.

The native/paired entry points are `make_manifest.py`, `perf_suite.py run`, and
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
retained. Use `--native-only` for all native scenarios; Notepad++ pins are then
omitted. Fixtures of different sizes need separate manifests. `make_fixture.py
<new-path> --bytes <count> [--long-line]` creates an exclusive 64 KiB-buffered ASCII
fixture plus hash/byte-length receipt; use 10485760, 104857600, 1073741824 and
5368709120 for the size series, and `.rs` for the syntax scenario. It refuses
existing files and insufficient disk headroom. Result-jump fixtures contain
PERF_NEEDLE; search/cancel use the absent PERF_ABSENT_TOKEN for complete scans.

The orchestrator alternates application order within every pair, drains bounded
stdout/stderr concurrently, enforces per-trial deadlines, retains failed/missing
trials and writes create-new raw artifacts. Reports preserve independent
application observations. Ratios require an explicit reviewed `comparable_metrics`
list and complete matched pairs. Only Save includes a shared default endpoint,
`save_to_clean_ack_us`: dispatch Save after the same PERF_SAVE insertion, through
the editor acknowledging clean state. This is neither flush durability nor paint
completion. Other generated comparison lists are empty. A matching metric
name is insufficient: Bareline present receipts and Notepad++ Scintilla message
roundtrips have different endpoints. Search primitives and termination boundaries
also differ. No report automatically becomes eligible for marketing claims.

`bareline_driver.py` launches only its suspended/Job-contained process, with
session disabled and all state under the temporary performance root. Extensions
are disabled except for the explicit signed-fixture extension scenario.
It reads the native create-new result receipt after successful process exit and
adds the same process-tree sampler used by the Notepad++ adapter. The marker file
`.bareline-perf` is required; ordinary launches never enable this workload path.
Native deadlines are bounded to 120 seconds. Save As uses a new file within the
owned root. The wrappers never operate on the supplied original fixture.

Native source workloads cover launch, ten-second empty idle, size/long-line open,
120 alternating scroll steps with real movement, edit-to-present, syntax completion
for the current real document identity/range followed by presentation, full
resident/paged Find, tracked worker-terminal cancellation, result jump, Save/Save
As, full folder-scan completion, committed tail append followed by presentation,
100/500 populated tabs, and an actual extension invocation through its Drained
lifecycle receipt plus successful broker outcome for the same generation. Cleanup
alone never counts as invocation success. Cancel races that finish before cancellation are excluded.
Incomplete/capped/failed search results are not converted to successful timings.
Native first-present uses process-independent QPC from the adapter launch origin;
`source_ready_us` and `resident_full_load_us` are separate milestones. Paged full
materialization is deliberately inapplicable and omitted. See
README-WINDOWS-ADAPTERS.md for the Notepad++ endpoint semantics and constraints.
Memory peaks are maxima of complete sampled live Job totals, not OS lifetime peaks.
Missing process samples fail the measurement; sampling can miss short-lived peaks.

Warm launch completes one unmeasured application launch first. Cold launch requires
`--cache-plan <plan.json>`: a separately pinned preparation executable/argv,
support-file hashes and 1..120 second deadline. It runs after copies/hash checks,
before measured launch, with `{application}`, `{fixture}` and `{output}` paths.
It must emit one `cache_prepared` JSON event with state `cold`, method,
evidence_path and evidence_sha256; the evidence must be bounded UTF-8 text in the
owned root and is embedded in retained raw output. This is an explicit external
environment contract, not an implemented OS-cache reset. No cold label is accepted
without that receipt; the release reviewer must validate the pinned method.

Extension runs require `--extension-inventory` and `--extension-command owner/id`.
The inventory pins each relative installed file under its own directory; the
wrapper copies at most 4096 files/512 MiB into the isolated extensions root.
Normal production trust, enabled-command and runtime authentication checks remain
in force. A build with no compiled owner trust correctly cannot measure extensions.
The sampler must observe a child process; otherwise no total is published. Fixture
disk bytes are measured; acquisition/download cost is outside this offline driver.

Notepad++ lacks presentation, Find-dialog cancellation, workspace/tail, populated
tabs and extension-runtime adapters. These cannot become cross-application ratios;
use native-only evidence and explicitly preserve missing comparator signals. Cold
protocol validation, trusted signed extension artifacts, actual runtime receipts,
same-machine paired runs and default-policy decisions remain external acceptance
work. Source/product harness measurements cannot replace native evidence.

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
