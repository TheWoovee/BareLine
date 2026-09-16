# PR021 journey automation

## Current adapter coverage — 2026-09-16

All thirteen paths are authored. The [installed-preview smoke run](../../docs/qa/2026-09-16-windows-install-smoke/README.md) passed plain_text, code_config, regex_transform and column_multi_cursor: 12/12 steps in one dark/software/100% DPI cell, including clean exits. Six ordinary procedures still await native execution. Crash/recovery, extension isolation and install/update/rollback have headless checks only and require explicit pinned [lab inputs](WINDOWS-LAB.md). A local preview installation does not qualify the VM install/update/rollback journey.

Fixture generation/validation tests are synthetic; the most recent consolidated tooling run passed 122 E2E tests. Native input pins an already installed English layout only on the owned editor thread and uses extended navigation scan codes. It stops on foreground loss, physical Escape or desktop unavailability. It captures only owned-window screenshots, never the full desktop.

See [typed producers and final closure](TYPED-EVIDENCE.md), [soak runner](../soak/README.md), and the complete planning-only acceptance/command work catalog under docs/implementation/production-readiness/QUALIFICATION_WORK_DEFINITIONS.json. Descriptions and counts in the older checkpoints below are historical.


## Historical runner and adapter checkpoints

The manifest and resolver cover thirteen journeys. At this earlier checkpoint the native adapter
implements **plain_text, code_config and regex_transform**; the other ten return explicit NOT_RUN. The
new-file defaults repair passes the default UTF-8/CRLF cell, all three steps and
clean editor Exit. Additional native encoding cells remain pending. No full journey qualification is implied. `journeys.json` defines
the thirteen required journeys, bounded deadlines, actions and observable checks.
`journeys.schema.json` documents the versioned format; `runner.py validate` also
enforces exact journey membership, duplicate rejection and step/deadline bounds.
Each journey's `cases` list is a provisional traceability hint. It cannot qualify
an acceptance case. Only a reviewed `qualification_mappings` entry can bind a
complete acceptance outcome to explicit required steps. An unmapped or partially
covered case remains unresolved.

Run source validation through `cargo xtask qa validate`. The coordinator may add
`python -m unittest discover -s tests/e2e -p test_runner.py` to the shared gate.
These tests do not start editors or establish acceptance.

Execution is opt-in: `cargo xtask qa run <journey> --commit <full-sha>
--reviewer <name> --os-build <exact-build> --hardware <configuration>
--executable <absolute-pinned-exe> --dpi <monitor-scales> --mode keyboard
--theme light --adapter <absolute-driver-executable> <literal-arguments>`.
Each invocation runs one journey. Repeat on both Windows floor builds and mixed
DPI; record keyboard, screen reader and themes separately. Never combine these
environments into a single implied pass. Installation/recovery journeys require
a disposable VM and generated scratch data; never point a driver at personal apps.

The reviewed adapter receives one final argument: an absolute request JSON path.
It must consume the supplied pinned executable and scratch directory, implement
every action, and atomically write the requested response path before exiting:
`{"schema_version":1,"journey":"plain_text","steps":[{"id":"s1",
"status":"PASS","observed":"actual measured observation"}, ...]}`.
Missing, duplicate, failed or NOT_RUN observations cannot pass. PASS also requires
adapter exit code zero. Results retain adapter_exit_code before cleanup; null
means no terminal status was established (for example, timeout or an API error).
Evidence import rejects nonzero or missing exit codes, so older native runner
results without this field require recapture. A successful exit alone cannot pass.
Drivers must observe real application behavior; this manifest
is not an executable UI driver and does not supply physical screen-reader/IME
automation. Driver availability and execution evidence remain explicit handoffs
to coordinated QA, never synthetic PASS adapters.

The runner reuses PR019's Windows OwnedProcessTree: suspended creation, assignment
before resume, no breakaway and kill-on-close. Timeout, errors, normal completion
and outer runner termination close the owned Job; unrelated applications are not
targeted. One journey is capped at 600 seconds and its response at 256 KiB. The
xtask wrapper may use a 660-second outer deadline. Adapter stdout is not evidence;
the bounded response is. Requests/results are exclusive new UUID directories in
`results/`, retaining binary hash, commit, reviewer, OS, hardware, mode and DPI.
The runner's final JSON line binds the absolute result path to its SHA-256 for
T09 capture.

Reuse `tests/perf`/`xtask perf` for performance and packaging's existing supply-chain
verification for release artifacts. This runner neither duplicates those tools
nor installs, signs, publishes or submits anything implicitly.

## Native adapter implementation (DEV-003)

Pass the absolute Python executable and absolute path to `native_adapter.py` as
the runner's adapter argv. The runner appends its absolute request path:

```text
python tests/e2e/runner.py run plain_text --commit <full-sha> --reviewer <name> --os-build <build> --hardware <configuration> --executable <absolute-pinned-exe> --dpi 100 --mode keyboard --theme dark --adapter <absolute-python> <absolute-native_adapter.py>
```

The default fixture requests CRLF. `--new-file-eol lf` after the adapter path is
an explicitly separate LF control; it does not establish CRLF correctness.
Use `--new-file-encoding utf-8-bom`, `utf-16le` or `utf-16be` after the adapter
path to request an explicit encoding/BOM cell. The default is `utf-8` without a
BOM. EOL and encoding fixture identity are bound into every result step; a pass
for one cell cannot stand in for another. [NEW-FILE-DEFAULTS evidence](../../docs/qa/2026-09-15-new-file-defaults/README.md)
records the repaired UTF-8/CRLF pass and the LF/BOM foreground stop. Keep the
historical CRLF failures; remaining encoding UI cells still require capture.

The adapter checks canonical manifest identity, executable hash before/after,
fresh sibling scratch/response paths, ancestor symlinks/junctions, source drift,
driver exit, observed editor exit, and artifact hashes. It launches
`native_driver.ps1` in a nested owned Job. Generated profiles select light/dark,
the requested encoding/BOM and EOL; the driver records actual HWND DPI and refuses an
active high-contrast or unavailable/non-default input desktop. It supports one
keyboard/DPI cell per run. Screen reader, physical IME, pointer, high contrast,
mixed-DPI, signed-package and disposable-VM procedures remain pending.

The native procedure checks owned HWND/provider identity, observed focus,
complete Unicode SendInput, exact text, actual modern Save As and legacy Open
filename controls, saved byte hashes, selected-tab dirty state, Close/reopen,
Undo/Redo and one clean File > Exit. Read-only observation polling is bounded;
submitted actions are not repeated to obtain a pass. Lost foreground or physical
Escape stops the run. Remaining steps become NOT_RUN after failure. The outer
runner and nested Job terminate only owned processes on failure/timeout.

Responses are published atomically without overwriting an existing result.
Scratch retains native checkpoints, command traces, editor logs, fixture bytes
and settings. The native response's artifact hashes are checked before the
adapter binds them into runner step results. Use the T09 capture wrapper to bind
the dirty source tree as well. Do not import the LF control as full AC evidence;
plain_text does not yet have a reviewed complete-case mapping.

Focused source checks: `python -m unittest discover -s tests/e2e -p test_native_adapter.py` and `python -m unittest discover -s tests/e2e -p test_code_config.py`. Windows PowerShell 5.1 can run `test_native_visual.ps1` for bounded synthetic bitmap checks.
They do not launch editors or establish product acceptance. See
[DEV-003 implementation and remaining work](../../docs/implementation/production-readiness/DEV-003-PRODUCT-ADAPTER.md)
and [retained native results](../../docs/qa/2026-09-15-dev003-product-adapter/README.md).

## Code/config procedure

Run the same adapter with `runner.py run code_config` and the pinned executable.
This procedure currently supports keyboard/native-command automation at **100%
DPI**, with light/dark oracles; only the dark/software host cell has been captured.
The fixed generated Rust fixture is UTF-8 without BOM and LF. New-file fixture
options apply only to plain_text; code_config rejects non-default overrides.

The adapter writes a bounded fixture from `code_config_fixture.py`. The driver
opens it through the native dialog, observes `Language: Rust`, invokes the owned
Unfold All menu command, and captures the owned client area. It obtains token
rectangles from UIA TextPattern and compares actual keyword, string, number and
comment pixel colors against independently fixed theme expectations. Missing
geometry/pixels, plain text, wrong token colors, or only a language label cannot
pass. `native_visual.cs` bounds the capture and token areas and frees its resources.
No screenshot of the entire desktop is taken.

Ctrl+Space must expose the expected document-word candidate, Down must select it,
and Enter must accept it. Exact checkpoints verify indentation, surrounding
Unicode text, one completion Undo/Redo, unsaved disk preservation and final native
Save/Close/Open bytes. The adapter rechecks required checkpoint content, fixture,
source and artifact hashes; a PASS-shaped response without those observations is
rejected. A valid native step also needs its screenshot/final file artifact.

Source tests and synthetic bitmaps are not product evidence. The [code_config
record](../../docs/qa/2026-09-15-code-config/README.md) separates interrupted/failed
captures from the passing run and its renderer/accessibility fix. Other languages,
physical screen readers/IME, split geometry, OS floors, DPI and the final aggregate
remain separate qualification. The provisional AC hints in journeys.json are
unchanged and do not establish complete acceptance coverage.

## Regex transform procedure

Run the same adapter with `runner.py run regex_transform`. `regex_transform_fixture.py` defines fixed UTF-8/LF input, a multiline PCRE2 pattern, named/numbered capture references and independently expected preview rows/text/bytes. Non-default new-file overrides are rejected for this fixed source fixture.

`native_regex_transform.ps1` opens the generated file, uses the native Regex and Replace commands, observes the checked mode menu and exact complete count, and reads focused Find fields through UIA ValuePattern. A dedicated scratch folder contains only the open input file. The real folder picker is verified by its Folder label/Edit HWND and one owned Select Folder button activation. The historical native capture used mode-menu state because the mode controls inherited a generic accessible command label. The ten-defect batch fixes that label projection; fresh native/assistive verification remains pending.

The actual Replacement Preview must expose the one source and two expanded capture rows plus complete count before Apply. The driver captures only the owned client window, verifies preview does not mutate text/disk, observes one open-document apply and its exact count, and saves the expected transformed bytes. One Undo restores the original text; Save restores the original file. Both byte versions are retained and rechecked by the adapter. Failed/missing/duplicate observations, changed captures/counts, partial Undo, source/artifact drift and unclean Exit cannot pass. Initial Editor provider discovery is bounded to five seconds; actions are never replayed by polling.

Focused oracle checks: `python -m unittest discover -s tests/e2e -p test_regex_transform.py`. The [retained native result](../../docs/qa/2026-09-15-regex-transform/README.md) passes one dark/software/100% DPI cell and preserves all failed attempts. Ten procedures and the broader environment/acceptance qualification remain open.

## Strict evidence import — 2026-09-15

`evidence_json.py` rejects duplicate JSON fields at every object depth. Schemas and success counters require actual integers, excluding booleans and floats. Native results must contain the exact reviewed journey definition and match the requested journey. New runs rehash the pinned executable after owned-process cleanup and fail on drift.

Imported fixtures must appear in the captured step artifacts, stay inside the captured scratch directory and retain their hashes. Each result allows at most 128 distinct artifacts. Native evidence preserves captured `os_build`, `hardware`, `mode`, `theme` and `dpi`; the resolver rechecks them and CLI arguments cannot relabel them. These captured labels do not establish coverage of all required environment cells.

Reviewer names must differ after Unicode normalization, whitespace folding and case folding; this remains a human assertion. Each raw capture must contain exactly one complete result binding, using either structured JSON or the legacy path/hash pair. Duplicate, conflicting and mixed bindings fail.

Actual reads are bounded: 16 MiB per evidence JSON file and per stdout/stderr stream; 256 KiB for native adapter responses. Log hashes cover the same bytes used for result binding. Oversized input fails. Legacy evidence with missing captured identity cannot be repaired by relabeling it: preserve the original, and recapture when the required identity is absent.

Focused negative/control checks: `python -m unittest discover -s tests/e2e -p test_readiness_defects.py`. [Batch evidence](../../docs/qa/2026-09-15-ten-defects/README.md) separates these synthetic checks from native product acceptance.

## Environment coverage and durable retention

The twenty-task batch adds required environment matrices, distinct case/cell results, FAIL/NOT_RUN retention and authority-bound cell exclusions. A missing matrix leaves completeness unresolved. `evidence_bundle.py collect|verify|report` retains exact dependencies and verifies relocated bundles without original cache access. Follow the [environment and retention workflow](EVIDENCE_RETENTION.md); byte integrity and reported progress do not attest product acceptance.


## Lab and procedure completion — 2026-09-16

All thirteen journey paths are authored; the three new lab paths require explicit pinned inputs and have headless checks only. See [Windows lab inputs](WINDOWS-LAB.md). Native execution and independent review remain open. The command catalogue now has concrete proposed outcomes for all 499 historical commands (1,497 outcomes), 81 acceptance recipes and fixture variants for all 199 minimum scenarios. Every definition stays NOT_RUN/pending review; unknown final runtime commands stay unbound. Regenerate from the final configured composed inventory before qualification.
