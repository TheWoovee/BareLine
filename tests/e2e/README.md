# PR021 journey automation

Source harness only; no native journey result is implied. `journeys.json` defines
the thirteen required journeys, bounded deadlines, actions and observable checks.
`journeys.schema.json` documents the versioned format; `runner.py validate` also
enforces exact journey membership, duplicate rejection and step/deadline bounds.

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
Missing, duplicate, failed or NOT_RUN observations cannot pass. A successful exit
alone cannot pass. Drivers must observe real application behavior; this manifest
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

Reuse `tests/perf`/`xtask perf` for performance and packaging's existing supply-chain
verification for release artifacts. This runner neither duplicates those tools
nor installs, signs, publishes or submits anything implicitly.
