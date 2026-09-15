# PR-T15 — Performance qualification tooling

- Issue: TECH-018
- Baseline source: `e7648af1715ecf5a0214377db2f802f7e330a9e4`
- Binary baseline: unavailable; no executable was launched or benchmarked in this tooling slice.
- Dependencies: PR-T09 source/evidence identity; configured extension scenarios from PR-T10/T11; capability-ledger publication from PR-T14; owner-provisioned comparator and dedicated hardware remain pending.
- Scope: `tests/perf` manifest, orchestration, reporting, focused Python contract tests, and this note.

## Qualification contract

- Manifests must pin executable/configuration/driver/fixture bytes and the T09 source identity, plus structured hardware, OS build, power, display/DPI, fonts, wrap, syntax, storage, renderer, cache method and acquisition-cost policy.
- Reports retain every expected trial as measured, failed, invalid or unavailable. Comparative rows use only complete, order-matched cohorts with explicitly reviewed shared endpoints.
- Source drift retains raw trial artifacts but marks observations and paired rows unqualified, excludes them from regression input, and produces a nonzero native qualification exit.
- Missing owner-provisioned comparator bytes or dedicated hardware remains pending and prevents a superiority claim.
- Native execution, downloads, Cargo commands and the 26-case/three-trial registry are reserved for the root coordinator.
- Paired and native reports use `schema_version: 2`, `kind: performance_qualification_report`, stable row/coverage IDs, `qualification_status: unqualified`, `claims_eligible: false`, and explicit ineligibility reasons. This is the PR-T14 ledger import boundary.

## Evidence

- `python -m py_compile tests/perf/perf_suite.py tests/perf/make_manifest.py tests/perf/native_nightly.py tests/perf/test_perf_suite.py tests/perf/test_native_nightly.py` — passed.
- `python -m unittest discover -s tests/perf -p "test_*.py"` — 21 passed. These are parser/orchestrator contracts only; no editor or comparator was launched. The suite includes controlled inert-driver runs through manifest, raw retention and report generation in temporary Git repositories, including a source-mutation negative.
- `python tests/perf/make_manifest.py --help` — passed without creating a manifest or executing a benchmark.
- `git diff --check` — passed apart from Git's existing LF-to-CRLF worktree notices.

## Pending qualification

- The root coordinator can build Bareline, read its settings/fonts and generate ordinary scratch fixtures under the existing local authorization after integration settles.
- Comparator bytes, dedicated hardware, a reviewed cold-cache method and the external support-floor/environment matrix remain unavailable until their existing owner/environment processes provide them.
- The root coordinator must then run the 26-case native registry and matched paired cohorts on the dedicated unlocked hardware, retain the raw directories, and generate the schema-2 reports.
- A release claim and renderer/storage default decision remain unavailable until those reports contain complete matching evidence and receive owner review. Failed, timed-out, missing or unavailable work must remain in coverage rather than become numeric zeroes.
