# Native regression tests

This directory contains Windows UI drivers, generated fixture recipes, and
headless tests for their input validation and result checks. The thirteen
journeys in `journeys.json` cover editing, search, files, workspace navigation,
extensions, and installation. Each run records step observations and uses a
fresh scratch directory under the ignored `tests/e2e/results/` directory.

Run the headless regression suite and validate the journey manifest:

```powershell
python -m unittest discover -s tests/e2e -p 'test_*.py'
python tests/e2e/runner.py validate
```

To run a native journey, build the editor and use an unlocked Windows desktop.
Pass absolute paths for the editor, Python executable, and adapter:

```powershell
cargo build -p bareline --locked
$python = (Get-Command python).Source
$editor = (Resolve-Path target/debug/bareline.exe).Path
$adapter = (Resolve-Path tests/e2e/native_adapter.py).Path
python tests/e2e/runner.py run plain_text --commit (git rev-parse HEAD) `
  --reviewer local --os-build ([Environment]::OSVersion.VersionString) `
  --hardware local-desktop --executable $editor --dpi 100 --mode keyboard `
  --theme dark --adapter $python $adapter
```

`cargo xtask qa` forwards the same arguments to the runner. Set
`BARELINE_QA_PYTHON` to an absolute Python path if needed. The runner applies a
deadline, bounds captured output, checks the adapter's exit status, verifies
each step, and rechecks the editor's hash after process cleanup. Unsupported
modes report `NOT_RUN`; they cannot count as a passing journey.

Use `--output C:/test-runs/new-run` before `--adapter` to keep a run outside the
checkout. The destination must not already exist; earlier results are preserved.

The [Native journeys workflow](../../.github/workflows/native-journeys.yml) runs
the four journeys that have passed natively (`plain_text`, `code_config`,
`regex_transform`, `column_multi_cursor`) three times each on a release build,
nightly, for `v*` tags and on manual dispatch, never on pull requests.
`journey_matrix.py` schedules the attempts and writes a per-journey pass count
and flake rate to the job summary. A journey that never passes fails the run; a
flaky one is a warning. It is not a required check.

The ordinary drivers support keyboard input with light or dark themes. Some
visual checks require 100% DPI. Crash/recovery, extension isolation, and
install/update/rollback require the explicit disposable-machine inputs in
[WINDOWS-LAB.md](WINDOWS-LAB.md).

`utility_command_oracles.json` contains exact conversion vectors also consumed
by the Rust core tests. `evidence_json.py` and `lab_fixture.py` provide bounded
JSON and scratch-file validation shared with the soak runner.

`recovery_smoke.py` tests process-crash recovery on an isolated diagnostic
profile using the normal editor executable. Build the read-only probe with
`cargo build --locked -p bareline --example recovery_inspect`, then pass absolute
`--executable`, `--probe`, and fresh `--output` paths plus each binary's SHA-256
with `--sha256` and `--probe-sha256`. Copy an editor from a portable package into
a separate directory first so its portable marker cannot redirect the profile.
The test inspects exact saved and Untitled checkpoints before terminating its
owned editor, then restores both through Recovery Center and verifies their
saved bytes. It requires the desktop for at most three minutes and stops on
focus loss, physical Escape, or a `STOP` file in the output directory. It does
not inject save-boundary faults or qualify power-loss recovery.
