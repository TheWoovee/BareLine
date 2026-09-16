# Bounded Windows soak

`runner.py` supervises one owned Windows editor and `native_soak.ps1` exercises generated open/edit/save/search/tail/close cycles. It requires an available desktop and stops on lost foreground, physical Escape, lock/non-default input desktop or a `STOP` file in its output directory. Never reclaim another application to make a run pass.

```powershell
python tests/soak/runner.py --executable C:/qualification/candidate/bareline.exe --sha256 ACTUAL_SHA256 --output target/qualification/soak-new --duration-seconds 180 --cadence-seconds 15 --dpi 100
```

Use a new output directory. Pin immutable candidate bytes. Duration is 30–259200 seconds; cadence is 5–60 seconds. Source and binary drift, editor replacement, unavailable metrics, a stalled workload, output quotas or unsuccessful cleanup fail the run. Do not concatenate restarted captures into one uptime claim.

The supervisor retains Job/process identities, memory and workload samples in at most 32 MiB/52,000 rows. Scratch is capped at 512 MiB/8,192 entries with reparse boundaries rejected. The driver retains a 32-checkpoint ring and at most 16 MiB of cycle history. Opt-in app diagnostics retain bounded handle history plus the latest task/extension/recovery queue snapshot. Counter absence is not zero. Growth alarms are shakedown diagnostics, not published performance targets.

Optional `--extensions-lab-config C:/lab/extensions.json` installs the explicitly pinned signed runtime/packages through the real manager and runs JSON/XML/Hex plus Undo/provenance checks on every cycle. `--recovery-lab-config C:/lab/crash.json` verifies durable saved/Untitled edits, terminates the same soaked editor at an observed diagnostic save boundary and restores through Recovery Center. Both follow [the lab input contract](../e2e/WINDOWS-LAB.md) and require the explicitly bound disposable VM. Missing flags keep those workloads NOT_RUN.

The uninterrupted timer stops **before** recovery. Its original PID/kernel creation timestamp remain in the report; the recovered editor gets a separate PID. The supervisor rejects returning to a soak after entering recovery. Job creation timestamps must match heartbeat identity; PID reuse cannot continue a run. Extension completion counts must equal completed core cycles.

**Current qualification:** five headless supervisor tests pass. The integrations are implemented, but no native shakedown or 72-hour run is claimed. `ready_for_independent_assessment` requires full duration and all workloads; `full_release_soak_qualified` remains false because a report cannot approve its own independent assessment. A core-only or short run cannot close the release gate.
