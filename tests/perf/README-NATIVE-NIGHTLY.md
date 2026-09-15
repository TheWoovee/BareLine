# Full native nightly series

Source definition only. No workflow, benchmark, desktop automation, cache reset or
download was executed while implementing this series. The integration coordinator
owns source verification; actual performance acceptance remains a later phase.

The existing hosted launch/source series remains separate. Enable the full native
job only on a dedicated self-hosted Windows runner labelled `bareline-perf`, with
an explicitly available interactive desktop. Repository variables:

- `BARELINE_PERF_NATIVE_ENABLED=true` enables the full registry job.
- `BARELINE_PERF_NATIVE_PLAN` is the absolute provisioned plan path.
- `BARELINE_PERF_NATIVE_PLAN_SHA256` pins that plan.
- `BARELINE_PERF_ISSUES_ENABLED=true` separately enables informational issue updates.

The runner never unlocks a desktop or reboots the machine. Its shared concurrency
group prevents overlapping native series. The full registry has 26 cases: every
scenario in `perf_suite.SCENARIOS`, with hardware and software for cold/warm launch,
empty idle and scroll, and hardware for the remaining scenarios. Each case uses
exactly three trials; each trial is bounded to 300 seconds including adapter setup.
The overall job has an eight-hour ceiling. A missing prerequisite produces an
`unavailable` coverage record; started but failed trials remain `failed`, with raw
receipts retained. Neither case produces invented numeric measurements.

Provision a version-1 JSON plan with these fields (paths and hashes are real local
artifacts supplied by the operator, not downloaded or guessed by this tool):

```json
{
  "schema_version": 1,
  "interactive_desktop_ready": true,
  "machine_id": "ACTUAL-HOSTNAME",
  "configuration": "stable-reviewed-profile-id",
  "qualification_environment": {
    "hardware": {
      "cpu_model": "actual model",
      "logical_cpus": 16,
      "physical_memory_bytes": 34359738368,
      "storage_model": "actual volume/device model"
    },
    "os": {
      "name": "Windows",
      "release": "11",
      "version": "actual platform.version() value",
      "build": "actual build component",
      "architecture": "AMD64"
    },
    "power": {"mode": "actual plan", "source": "AC"},
    "display": {"dpi": 96, "scale_percent": 100},
    "fonts": [
      {"family": "actual family", "version": "actual version", "path": "font file", "sha256": "SHA256"}
    ],
    "editor": {"theme": "light", "wrap": "off", "syntax": "plain text"},
    "acquisition": {
      "setup_compilation": {"status": "measured", "duration_us": 123, "output_bytes": 456},
      "download": {"status": "unavailable", "reason": "offline provisioned comparator"}
    },
    "cold_cache": {
      "method": "reviewed method named by the pinned cold-cache plan",
      "plan_sha256": "same SHA256 as cold_cache below"
    }
  },
  "repetitions": 3,
  "settings": {"path": "absolute settings.toml path", "sha256": "SHA256"},
  "fixtures": {
    "open_10mb": {"path": "absolute generated fixture path", "sha256": "SHA256"}
  },
  "cold_cache": {"path": "absolute cache preparation plan path", "sha256": "SHA256"},
  "extensions": {
    "path": "absolute installed signed fixture inventory path",
    "sha256": "SHA256",
    "command": "owner/command"
  }
}
```

The runner verifies the hostname, OS identity/build, architecture, logical CPU
count and every font-file hash before launching a case. The remaining hardware,
power, display and editor fields are explicit operator pins retained in every
manifest and report. If the plan is absent or any case is unavailable/failed,
`native_nightly.py` writes `native-report.json` and exits nonzero so the workflow
can retain the report without presenting the run as successful.

`native-report.json` uses `schema_version: 2` and
`kind: performance_qualification_report`. It includes exact source identity
before/after, the executable path/version/SHA-256 when available, structured
configuration, stable coverage and row IDs, explicit ineligibility reasons and
`claims_eligible: false`. Missing and failed cases carry no numeric substitute.
If source identity is unavailable or changes during any case, raw observations
remain in that case report but its rows are unqualified, excluded from rolling
regression input, and the native command exits nonzero.

The fixture map must provide `open_10mb`, `open_100mb`, `open_1gb`, `open_5gb`,
`long_line`, `scroll`, `edit_to_paint`, `syntax_viewport`, `literal_search`,
`regex_search`, `search_cancel`, `result_jump`, `save`, `save_as`, `workspace_scan`,
`tail_append`, and `extensions_memory`. Shared fixtures are allowed where the
scenario semantics match. Named sizes are checked against exact byte counts.
Syntax requires a supported language fixture; result jump requires PERF_NEEDLE;
search/cancel scan for the absent PERF_ABSENT_TOKEN. A small search may finish
before cancellation and is then excluded, not relabelled as a cancellation result.
The existing cache protocol and production extension trust checks remain mandatory.

The workflow pins the just-built executable, exact PE version, Python and current
driver sources into each generated manifest. Settings, fixtures, cold method and
extension inventory retain their separately provisioned pins. Native reports are
not mixed with the ephemeral hosted series. Only complete three-trial observations
enter native rolling rows; the report retains coverage and each raw report hash.
Both series require exactly the previous seven reports for regression processing.
Configuration, fixture, method, machine and sampling-profile changes invalidate the
baseline cohort. P50 changes above 10% plus observed baseline noise are
informational; numeric findings never fail CI. This is not comparative marketing
evidence and does not decide renderer or resident defaults.

# Disk and acquisition accounting

Both application adapters use the same bounded metadata observer. The measured
child receives isolated TEMP/TMP, LOCALAPPDATA and APPDATA paths under its owned
trial root; the editor's configured recovery and extension paths already point
there. This captures recovery journals, spill/transcode temporary files and runtime
user-cache writes without inspecting personal directories.

The observer records actual before/after logical file bytes and maximum observed
growth separately for recovery, extensions, temporary/spill, user cache and other
owned data. It also records released bytes, sample count and the 100 ms interval.
Snapshots are non-atomic metadata traversals, bounded to 100,000 entries and depth
64. Links/reparse points, inaccessible metadata, non-regular objects and quota
exhaustion fail the measurement instead of publishing partial totals. Files
deleted between enumeration and stat no longer contribute to the current sample.
Sampling may miss short-lived peaks; logical file lengths do not claim physical
allocation/slack or filesystem metadata overhead. A zero is emitted only from an
actual empty/no-growth observation, never as an unavailable measurement.

Installed extension acquisition is an offline verified copy into the owned tree.
`extension_acquisition_local_copy_bytes` and `extension_acquisition_local_copy_us`
measure that actual transfer separately from subsequent runtime/cache growth.
Download bytes remain null with an explicit unavailable status in provenance:
this adapter runs no downloader and cannot reconstruct historical acquisition
traffic from an installed directory. Actual online acquisition evidence belongs to
the separately authorized installer/download run; it is not invented as zero here.
