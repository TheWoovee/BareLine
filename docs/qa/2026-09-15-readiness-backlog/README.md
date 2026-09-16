# Expanded production-readiness audit — 2026-09-15

## Result and scope

The [execution backlog](../../implementation/production-readiness/BACKLOG.md) contains **49 work packages**: nine development/diagnosis, 27 product qualification, nine release, three environment/reliability and one scope investigation. It traces all 81 atomic acceptance cases, 199 minimum scenarios from the 27 original PR briefs, all 52 parity families, and the historical 499-command inventory.

This expands the [earlier status audit](../2026-09-15-status-audit/README.md). Its historical receipt checks remain intact. This pass inspected the linked contract/minimum-scenario authorities and targeted source, enumerated missing evidence coverage, and ran one isolated synthetic negative probe. It did not rerun the full Rust suite or perform native editor acceptance, signing, installation or publication. Product defects discovered during future qualification can add work; this is a complete inventory of identified contractual obligations, not a proof that no other defects exist.

## Findings requiring development

| Finding | Evidence | Work and release significance |
|---|---|---|
| **RF-001: failed adapter can produce a PASS journey** | `tests/e2e/runner.py:307-354` waits for process termination and reads PASS observations without checking exit status. `OwnedProcessTree.alive` exposes only running state. The isolated adapter wrote a clearly labeled synthetic PASS response and exited **7**; the runner returned **0/PASS**. | **DEV-001, confirmed defect.** Check producer exit before accepting evidence. This affects confidence in new acceptance capture; it is not an editor data-loss reproduction. |
| **RF-002: offline recovery-root policy is outside configured preparation** | `platform-windows/update.rs:464-581` enables root authority/rotation only under `BARELINE_OFFLINE_ROOT_PUBLIC_KEY` and reads `BARELINE_ROOT_VERSION_FLOOR`. Strict release schema, `derive_values`, shared build keys and `build-configured.ps1` do not supply those fields. FC-08 requires independently controlled recovery authority. | **DEV-007, confirmed integration gap.** Add validated root policy propagation and artifact/config binding. `parity-044` is reopened. No claim is made that ordinary signature verification is bypassed. |
| **RF-003: insufficient acceptance definitions** | Only 23 of 81 AC IDs appear in provisional journey `cases`; 58 have no association. One reviewed mapping exists, and no command mappings exist. | **DEV-003/004.** Build actual procedures and complete mappings. Thirteen three-step product journey declarations cannot be counted as all AC/command coverage. |
| **RF-004: evidence model cannot aggregate required contexts directly** | `runner.py:357-492` keys resolution by a single case ID, rejects duplicates and binds non-excluded evidence to one inventory binary/source. Its validators accept only the fixed native product journeys for qualifying AC records. | **DEV-005.** Add typed producers and environment/artifact-aware aggregation for Windows floor/DPI/AT, core properties, packaging, performance and actual foreign-host evidence. Preserve strict integrity checks and complete mapped semantics. |
| **RF-005: shipping pipeline is not configured-release orchestration** | `.github/workflows/supply-chain.yml:70-76` explicitly builds preview executables; its signing job consumes that payload. The separate local configured builder exists. | **DEV-008.** Keep preview jobs honest and wire a separate configured shipping path with capability validation and full release inventory. This is a pipeline integration task, not evidence that a preview has been published. |
| **RF-006: retained native prerequisites remain unresolved** | Final acceptance records p0-3 stopping before input/Exit, synthetic astral input failing before paint assertions, unqualified raw split-provider focus and native busy Close/Exit resumption. | **DEV-002/003.** Diagnose controller/provider/input boundaries, repair demonstrated faults and preserve exact oracles. Normal UI emoji and idle manual Exit passes remain scoped historical evidence. |

### Reproduction boundary for RF-001

**Subsequent fix, 2026-09-15:** DEV-001 is now implemented and focused-verified. [27 tests and Windows process probes](../2026-09-15-dev001-adapter-exit/README.md) confirm exit-7 now produces FAIL/runner exit 1. The original reproduction below is preserved as historical evidence; no product acceptance was added.

The [negative-probe record](adapter-exit-negative-probe.json) preserves the command, expected failure, actual exit/status and the generated result binding. The [adapter source](synthetic_exit_7.py.txt) writes explicitly labeled **SYNTHETIC NEGATIVE PROBE ONLY** observations, then exits 7. Python was used as the pinned-file identity; Bareline was never launched. No evidence adapter/import step was called, so this result does not enter the parity evidence index.

The producer's generated result is retained under `tests/e2e/results/d7efc5ed-3607-489d-bd00-1e238c0433b0/`, with a byte-identical [synthetic negative result copy](synthetic-negative-result.json) beside this report. Treat it as a regression reproduction only. Fixing the exit-status defect must make this exact class fail; do not delete the failure history or weaken observation requirements.

## Required work beyond the original seven-part summary

- Explicit root-policy distribution, old+new root rotation, revoked-key/expiry/freeze vectors, and recovery after compromised signing authority.
- Native input/control tests at 125%, 250% and 300% where the linked contracts require them, plus the earlier 100/150/200% and real mixed-DPI coverage.
- Real 5 GiB transcode/UIA/Hex/map tests; 20 GB search/hash cancellation; 10,000 carets and workspace files; 5,000 tabs and lazy 1M-node/entry controls. Historical synthetic providers do not substitute for disk workloads.
- Release/SDK documentation, a fresh extension-author walkthrough, valid/invalid installed signer tests, clean per-user/per-machine behavior and update-safe session migration.
- Current dependency/security review, parser/FFI/fault/race/privacy evidence, no-remote-contact checks, controlled comparator sampling, foreign-host checks and the 72-hour soak.
- Durable release evidence export and an independent readiness decision after final candidate qualification.

These requirements are assigned individually in [COVERAGE.md](../../implementation/production-readiness/COVERAGE.md). Existing valid evidence should be reused after applicability review; an unchecked planning item does not mean that its code is absent or its test has never passed.

## Deliberate limits and historical wording

Do not reopen the shared bottom dock, Find tooltips, estimated `~` gutter cue, paged lifecycle ownership or consent visibility merely because old notes say pending. The old offscreen UIA implementation-gap wording is superseded by bounded source/provider support in PR-024; verify real screen-reader behavior and long-selection limits. Low-integrity/AppContainer and broad refactoring remain an explicit investigation/disposition, not an assumed mandatory redesign. The seven v1.1 deferrals and four exclusions remain unchanged.

The [coverage-validation record](coverage-validation.json) verifies complete inventory IDs and an acyclic dependency graph. Fresh contract-test passes from the prior audit remain historical checks; the synthetic probe here intentionally demonstrated a failing correctness boundary.
