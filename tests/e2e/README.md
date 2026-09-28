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

The ordinary drivers support keyboard input with light or dark themes. Some
visual checks require 100% DPI. Crash/recovery, extension isolation, and
install/update/rollback require the explicit disposable-machine inputs in
[WINDOWS-LAB.md](WINDOWS-LAB.md).

`utility_command_oracles.json` contains exact conversion vectors also consumed
by the Rust core tests. `evidence_json.py` and `lab_fixture.py` provide bounded
JSON and scratch-file validation shared with the soak runner.
