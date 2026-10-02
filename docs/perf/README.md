# Performance evidence: Bareline vs Notepad++

This page covers the comparable performance harness (plan task P2-00, review
PERF-01/BIZ-01), the nightly performance workflow (QA-10/P5-10) and the
dedicated runner rules (SEC-12). The first measured comparison is at the end.

**No comparative claim** ("faster", "lighter") may be published until an FC-10
qualified table exists for the release and the owner has reviewed the raw
evidence behind it. The report marks every table as either *FC-10 qualified*
or *Indicative (not FC-10)* and lists the reasons.

## Harness

| File | Role |
|---|---|
| `tests/perf/compare/Install-NotepadPlusPlus.ps1` | Installs the pinned Notepad++ portable build: checks SHA-256, size, Authenticode and file version, then removes the auto-updater. |
| `tests/perf/compare/New-BenchFixtures.ps1` | Writes deterministic fixtures and `fixtures.json` (bytes, SHA-256, line and match counts). |
| `tests/perf/compare/Invoke-Bench.ps1` | Runs the protocol against both editors. Writes `trials.jsonl`, `raw.json`, `results.md` and `summary.json`. |
| `tests/perf/compare/BenchCommon.ps1` | Shared helpers: window/menu lookup, `WM_COMMAND`, Scintilla queries, UI Automation, process samples. |
| `tests/perf/compare_report.py` | Turns `raw.json` into the Markdown table and `summary.json` (medians, min/max, ratios, FC-10 status). It also renders nightly series and regression tables. |
| `tests/perf/perf_history.py` | Nightly baseline selection and staleness checks. |

The scripts started as the review's working scripts
(`review-evidence-2026-09-30/bench-scripts`). The fixes found while running
them are kept:
- the Notepad++ Find/Replace control IDs come from `FindReplaceDlg_rc.h`
  v8.9.8.1: 1601 find, 1602 replace, 1605 regex, 1609 Replace All, 1625
  normal, 1626 extended. The review's regex ID 1627 was wrong;
- Bareline menu labels use U+2026 and are normalized with `[char]0x2026`, so
  the files stay ASCII for Windows PowerShell 5.1;
- no variable names differ only in case (`$f` used to overwrite `$F`);
- Bareline's search mode is selected **before** Replace opens (A11Y-11).

The harness sends no keystrokes. Menus are driven with `WM_COMMAND`,
Notepad++ dialogs with button messages, and Bareline's Find panel through UI
Automation. Save and copy trials replace the text clipboard, and the harness
restores it at the end.

### Endpoints

Both editors are measured the same way unless a row says otherwise. Every time
is measured from a harness stopwatch started just before `Start-Process`.

| Metric | Notepad++ | Bareline |
|---|---|---|
| `window_visible_ms` | First visible, unowned, titled top-level window | same |
| `input_idle_ms` | `WaitForInputIdle` | same (winit reports this before the first frame) |
| `first_frame_ms` | not measured (needs ETW); shown as n/a, never as a ratio | `first_frame` event appears in `diagnostics\bareline.log` |
| `ready_ms` | max(window, input idle) | max(window, input idle, first frame) |
| `idle_private_mb`, `idle_working_set_mb`, `idle_threads`, `idle_handles` | Sampled `IdleSeconds` (10 s) after ready | same |
| `first_view_ms` | Responsive window and `SCI_GETLENGTH` > 0 | Status bar shows `Document size: N bytes` |
| `loaded_ms` | `SCI_GETLENGTH` equals the file size | Status bar shows the exact byte count **and** a line count |
| `max_ui_latency_ms` | Worst `WM_NULL` round trip while loading | same |
| `loaded_private_mb`, `loaded_peak_working_set_mb` | `SettleSeconds` (3 s) after loaded | same |
| `save_to_disk_ms`, `save_to_clean_ms` | Save after a 1-byte paste until the file has the new size / `SCI_GETMODIFY` = 0 | Until the file has the new size / the tab is no longer "modified" |
| `copy_ms` | Select All, then Copy until the clipboard holds all characters | same |
| `replace_all_ms` | Replace All button returns and `SCI_GETLENGTH` has the expected size | Status bar shows the expected size. A refusal notice fails the trial. |

Replace All trials then save and count the replacements in the file on disk.
Correctness checks (regex on CRLF text, GBK, Big5, UTF-16 LE without BOM, 3 MB
UTF-8, binary) are recorded once per editor as pass/FAIL with their status text.
They are not timed.

### FC-10 protocol

- Runs happen on the pinned reference machine (`-ReferenceMachine`) from a
  **release** build.
- `-Runs 10` per cell (the default). The statistic is the **median** of
  successful runs; min and max are kept. Failed or timed-out trials are listed
  and never counted as zero.
- **Warm** cells start with one unmeasured warm-up per configuration.
  **Cold** cells call `-ColdCacheCommand` before every trial, passing the
  executable and fixture paths. That command must evict them from the file
  cache and exit 0. Its SHA-256 goes into `raw.json`. A cold label is never
  assumed without it.
- Configuration order rotates every run, so no editor always goes first.
- Launch configurations are `npp-default`, `npp-noPlugin`, `bl-hardware` and
  `bl-software`. File and operation cells compare `npp-default` with
  `bl-default` (the shipped renderer, which is recorded per trial). Ratios use
  Notepad++ `-noPlugin` when it was measured (ADR-10), otherwise the default.
- A table is *FC-10 qualified* only when all of these hold: 10 runs, warm and
  cold both measured with a pinned cold command, the reference machine was
  declared, and no trial failed.

### Pinned Notepad++

| | |
|---|---|
| Version | 8.9.8.1 x64 portable |
| URL | `https://github.com/notepad-plus-plus/notepad-plus-plus/releases/download/v8.9.8.1/npp.8.9.8.1.portable.x64.zip` |
| SHA-256 | `beddf5548e75c97930d1575ba1c7d113e6e2b33d0a200310e734ca08e225f414` (matches the release asset digest) |
| Size | 8,250,001 bytes |

The installer rejects any other archive. It also requires a valid Authenticode
signature and file version 8.9.8.1, and deletes `updater\` (GUP.exe). The
benchmark refuses an install whose `notepad++.exe` no longer matches its
receipt, or that has an updater again.

To move to a newer 8.9.x build, change the four pin values in
`Install-NotepadPlusPlus.ps1` in one reviewed commit, using the GitHub release
asset digest. Old and new tables must never be mixed.

### Running it

On an unlocked interactive desktop (reference machine or the dedicated runner):

```powershell
./tests/perf/compare/Install-NotepadPlusPlus.ps1 -Destination C:\bench\npp
./tests/perf/compare/New-BenchFixtures.ps1 -Destination C:\bench\fixtures   # -IncludeHuge adds 300 MB
cargo build -p bareline --release --locked
./tests/perf/compare/Invoke-Bench.ps1 -Bareline target\release\bareline.exe `
    -NotepadPlusPlus C:\bench\npp\notepad++.exe -Fixtures C:\bench\fixtures `
    -CacheState Both -ColdCacheCommand C:\bench\evict.ps1 -ReferenceMachine
```

The output goes to `tests/perf/results/compare-<time>/` (ignored by git) unless
`-OutputDirectory` is given:
- `trials.jsonl`: one line per trial, appended as it runs;
- `raw.json`: every trial, the warm-ups, the checks, the application and
  fixture pins, the machine and the protocol;
- `results.md`: the table;
- `summary.json`: medians, ratios, the qualification and the failures.

To rebuild the table from raw data:
`python tests/perf/compare_report.py comparison raw.json --markdown results.md --json summary.json`.
`-Scenario launch,open` limits a run. `-Runs 1` gives a quick smoke run
(always marked indicative).

### Publishing results

1. Run the full protocol on the reference machine for the release commit.
2. Commit `results.md` as `docs/perf/results/<version>.md`, together with
   `summary.json`. Attach `raw.json` to the GitHub release.
3. Any comparative sentence must link that table and quote the qualified
   medians. Indicative tables (like the one below) may be used only as
   engineering input.

## Nightly workflow (`.github/workflows/perf-nightly.yml`)

| Job | Runs on | Default |
|---|---|---|
| `measure` | hosted `windows-latest` | **on**; set `BARELINE_PERF_ENABLED=false` to stop it |
| `native-measure` | self-hosted `bareline-perf` | off; `BARELINE_PERF_NATIVE_ENABLED=true` (see `tests/perf/README-NATIVE-NIGHTLY.md`) |
| `compare-notepadpp` | self-hosted `bareline-perf` | off; `BARELINE_PERF_COMPARE_ENABLED=true`. `BARELINE_PERF_COLD_CACHE_COMMAND` adds cold cells. |
| `report-regressions` | hosted | on for every enabled series. `BARELINE_PERF_ISSUES_ENABLED=false` stops issue updates; `BARELINE_PERF_STALE_DAYS` (default 3) sets the staleness limit. |

- Every table is published as the **job summary**: hosted, native, the
  Notepad++ comparison, and the baseline and regression status.
- The perf jobs no longer use `continue-on-error`. A broken build, a failed
  native case or a missing report makes the run red. Regression numbers only
  annotate (`::warning`) and update the tracking issue; they never fail CI.
- The hosted summary records the source identity before the build. Before
  this, hosted reports had no identity, so `regress` rejected every one and no
  hosted regression could ever be reported.
- `perf_history.py` picks the newest seven comparable reports (same machine,
  configuration, fixtures, settings and valid source identity) from the last
  15 completed runs on `master`. It lists why each other run was excluded.
  With fewer than seven it **warns and still records** the run; the rolling
  comparison starts once seven exist.
- **Staleness:** when the newest baseline is older than the limit, the job
  opens or updates the "Stale <series> performance baseline" issue and fails.
  The issue closes once baselines are fresh again. The check can only run when
  the workflow itself runs. GitHub disables schedules in repositories with no
  activity for 60 days, so re-enable the workflow if runs stop entirely.

## Dedicated runner (SEC-12)

The `bareline-perf` label is used in a public repository's workflow. The
workflow already ensures that both self-hosted jobs run only when all of these
hold:
- `github.repository == 'TheWoovee/BareLine'`;
- `github.ref == 'refs/heads/master'`;
- the event is `schedule` or `workflow_dispatch`, so never `pull_request` or
  a fork;
- the job has a timeout and only `contents: read` permission;
- checkout does not persist credentials.

If the default branch is renamed, update `github.ref` in both jobs (the
contract test in `tests/perf/test_perf_nightly_workflow.py` checks it).

The runner settings belong to the repository owner and must also be set:

- **Runner group.** Runner groups exist only for organizations; the repository
  is currently under a personal account. If it moves to an organization, put
  the runner in a dedicated group with:
  - repository access limited to `TheWoovee/BareLine`;
  - "Allow public repositories" enabled for this group only;
  - **Selected workflows** set to
    `TheWoovee/BareLine/.github/workflows/perf-nightly.yml@refs/heads/master`.

  While it stays under a personal account, register the runner at repository
  level only and use the `bareline-perf` label nowhere else.
- **Ephemeral / JIT runners.** Register each runner just in time
  (`POST /repos/{owner}/{repo}/actions/runners/generate-jitconfig`, or
  `config.cmd --ephemeral`), so it takes exactly one job and is then
  deregistered. Reset the machine or its account profile between jobs.
- **Fork pull requests.** In Settings > Actions > General, set "Require
  approval for all outside collaborators" for fork pull request workflows.
- **Local account.** Run the runner as a dedicated standard user, not an
  administrator, with an unlocked interactive session. No repository or
  organization secrets are needed; give it none.
- **Network.** The comparison job downloads only the pinned Notepad++
  archive, and verifies it by hash. The runner needs no inbound access.

## First comparison: 2026-09-30 (indicative, not FC-10)

Measured during the review with the pre-harness scripts:
- Bareline 0.1.0 at `5571693`: release build (fat LTO), portable profile;
- Notepad++ 8.9.8.1 portable: checksum and signature verified, updater
  removed;
- AMD Ryzen 5 3600, 128 GB RAM, NVMe SSD, RTX 4070 SUPER, Windows 11 Pro
  26200, 100% DPI;
- warm file cache, interleaved runs, no cold runs.

Source: `BENCHMARK_BARELINE_VS_NOTEPADPP_2026-09-30.md` in the product
blueprint. Not qualified: fewer than 10 runs, warm only, UIA polling on the
Bareline side, and no ETW first paint for Notepad++. Use it as the baseline
the Phase 2 tasks must beat, not as a public claim.

**Launch and footprint** (empty document, median of 7 interleaved warm runs):

| Metric | Notepad++ | Notepad++ `-noPlugin` | Bareline hardware (then the default) | Bareline `--software` |
|---|---:|---:|---:|---:|
| Process start to window visible | 282 ms | 267 ms | 79 ms | 79 ms |
| First painted frame | n/a | n/a | 282 ms | 109 ms |
| Process start to input idle | 550 ms | 537 ms | 80 ms | 80 ms |
| Idle private bytes (3 s after ready) | 60.0 MB | 59.4 MB | 57.1 MB | 23.8 MB |
| Threads | 19 | 19 | 36 | 20 |
| Handles | 418 | 418 | 488 | 304 |

**Opening files** (median of 3 runs; 300 MB: 2 runs):

| File | Notepad++ loaded | Notepad++ memory | Bareline first view | Bareline loaded | Bareline memory |
|---|---:|---:|---:|---:|---:|
| 6 MB, 62k lines | 567 ms | 75 MB | 412 ms | 808 ms | 72 MB |
| 50 MB log, 468k lines | 645 ms | 166 MB | 414 ms | 4,121 ms | 160 MB |
| 10 MB single line | 841 ms | 180 MB | 424 ms | 1,091 ms | 77 MB |
| 300 MB log, 2.8M lines | ~920 ms | 386 MB | 413-443 ms | not finished after 90 s | 137 MB |

**Operations:**

| Operation | Notepad++ | Bareline |
|---|---|---|
| Save 50 MB after a 1-byte edit | 62-91 ms | 198-219 ms on disk; 338-402 ms until clean |
| Select All + Copy, 6 MB | 39 ms | refused (clipboard limit) |
| Replace All, 11,979 matches | 86 ms | refused (BudgetExceeded) |
| Replace All, 468,100 matches in 50 MB | 1.6 s | stalled for over 2 min with no error |
| Regex `(?m) end.*$` on CRLF | CRLF kept | converted to LF |
| Regex `(?m)end$` on CRLF | 3 replaced | 0 matches |
| GBK / Big5 / UTF-16 LE without BOM | detected | mis-detected |

Against ADR-10 (at most 0.8x the Notepad++ time, at most 1.0x its idle memory):
- launch to ready and idle memory are met: software 0.20x and 0.40x;
  hardware 0.53x and 0.96x;
- every full-load and save figure misses: 1.3x to 6.4x.

The Phase 2 targets in the implementation plan come from these numbers.
Re-measure them with this harness under FC-10.
