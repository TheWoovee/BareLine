# DEV-001 focused verification — 2026-09-15

**Result:** adapter-exit false PASS fixed and focused-verified. **Scope:** Python QA tooling only; generated fixtures, no editor/UI execution, no acceptance import, no Rust build or full suite.

| Check | Observed result |
|---|---|
| Focused Python adapter/evidence/process-exit tests | 27 passed in 0.403 seconds; one captured command succeeded in 631 ms |
| Original synthetic PASS response followed by exit 7 | Runner exit 1, result FAIL, retained adapter exit 7 |
| Synthetic PASS response followed by exit 0 | Runner exit 0, result PASS, retained adapter exit 0 |
| Real owned Windows parent and descendant | Both retained process handles signaled after Job cleanup; owned handles cleared |

The 27 tests include the existing evidence contracts and focused regressions for nonzero/crash exit codes, running/closed process states, wait/query failures, deadline, absent/malformed/oversized responses, incomplete/FAIL/NOT_RUN observations, result binding and cleanup invocation. Crash exit codes and API errors are injected unit cases; the native probes execute only synthetic Python processes.

## Commands and retained records

```powershell
python .github/workflows/run_test_evidence.py --output target/qualification/dev001-20260915/focused-tests.json --timeout 90 -- python -m unittest discover -s tests/e2e -p 'test_*.py'
python target/qualification/dev001-20260915/verify_native.py
```

- [Focused receipt](focused-tests.json) and [test output](focused-tests.json.stderr.log): stable source before/after and successful command exit. Nested test count is reported separately from the single captured command.
- [Windows probe report](native-probes.json): exact adapter commands, source hashes, original result bindings, observed exits and retained request/response/result hashes.
- [Probe procedure](verify_native.py.txt): inert retained copy of the procedure that ran, plus the generated adapters as `.py.txt` files.
- [Retained file manifest](retained-files.json): byte hashes and original paths. Captures retain their original target paths; sibling retained copies have identical bytes and hashes. Source hashes in the probe report identify the tested implementation independently of later documentation edits.
- [Original pre-fix reproduction](../2026-09-15-readiness-backlog/adapter-exit-negative-probe.json) remains unchanged for comparison.

The PASS result here is explicitly synthetic and is not product acceptance evidence. Baseline HEAD is `e6c1486ff0bb997d96801331b32986d612728d4f` with the local DEV-001 tooling patch; the earlier product binary was neither rebuilt nor requalified. See the [implementation note](../../implementation/production-readiness/DEV-001-ADAPTER-EXIT.md) for the patch and pending independent/final review.
