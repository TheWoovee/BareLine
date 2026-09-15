# PR-T10 — First-party runtime qualification

## Scope and baseline

- Issue: TECH-013.
- Baseline source: `64ab47fc89a5369d3fbb587da35699d35f6d8b88` (`codex/audit-20260912-t10`).
- Audit source manifest SHA-256: `a560aae60c4cadcf262568b1b04ca98e8467026a2a313bc01da28358958ffbe3` (the audit report's reviewed working-tree snapshot; this implementation branch is the exact locally reviewed integration commit named above).
- Dependency: PR-T08 worker launch changes are already included in the baseline; this delta preserves those terminal and cancellation semantics.
- Contracts read: `AGENTS.md`, the audit implementor contract, PR-T10, TECH-013 in the technical findings, and the first-party entries and limitations in the final audit test report.

## Contract decisions

- The fast qualification will build and execute current `wasm32-wasip2` release components through the authenticated Windows host boundary.
- Fast semantic coverage includes JSON format/validate/tree, XML format/XPath/error handling, and Hex navigation against original bytes, plus permission rejection and bounded cancellation/timeout behavior.
- Existing 1 GiB JSON and 5 GiB synthetic Hex checks remain a separate large qualification. Large performance measurements are informational; semantic failures remain test failures.
- Qualification telemetry will identify authentication, component hash/read, compile, instantiate, first broker request, execution, response, and shutdown timing, together with profile, component hashes, byte counts, and parent/host deadlines.
- Existing timeout budgets will remain unchanged until the measured failing phase is known. Production trust, sandboxing, permission checks, cancellation, result-size limits, encoding, byte provenance, and one-undo edit semantics remain enforced.

## Evidence

### Audit observations before budget decisions

- The audited release host passed all four prior ignored tests serially in 95.13 s. Its 1 GiB generated JSON validation took 88.588 s, issued 16,384 broker reads, and capped each read at 65,536 bytes.
- The debug host failed XML before its first broker request, including a serial rerun that exited 124 after 12.97 s. The debug 1 GiB JSON case also exceeded its parent budget. These observations do not prove a release-component defect or that parallel contention was the cause.
- The first integrated fast run at frozen source `e063214c62505e7fdefb99bad7c435cdad5d0ea0` retained two passes and one failure. The timeout fixture withheld its first broker response; the host-side pipe's 5-second read timeout won a race with the 5-second runtime watchdog, the JSON component handled that broker error, and the process returned success. The same run proved that restricted native creation did not inherit the requested stderr handle, because parent case records were retained but all child phase records were absent.
- This change does not alter `INTERACTIVE_TIMEOUT_MS` (5,000), `BACKGROUND_TIMEOUT_MS` (120,000), fuel, memory, process-count, frame, broker reply, panel, or staged-edit limits.

### Implemented qualification

- `scripts/test-first-party.ps1` now defaults to `-Suite Fast`; `-Suite Large` selects only the existing 1 GiB JSON and 5 GiB synthetic Hex cases, and `-Suite All` runs both serially. Every invocation creates a collision-resistant directory under `target/first-party/` and uses T09's non-overwriting evidence wrapper for target setup, component build, a bounded release `--no-run` host/test compile, and each direct test-executable invocation. The 300-second fast watchdog therefore covers execution after compilation. `run.json` binds the run to HEAD plus the dirty-source fingerprint, host/test/component hashes and sizes, raw output receipts, expected/observed test counts, timestamps, and terminal failure. Qualification also fails if any build/test receipt lacks its before/after source identity, changes identity while running, or does not continue the preceding stage's HEAD and source-manifest fingerprint.
- The fast suite executes actual JSON validate/format/minify/tree, XML validate/format/XPath/parser/DTD errors, and Hex original-generation navigation. It restores exact large-integer minification, successful XML validation, XPath descendant attributes and missing attributes, exact `xml:space` output, and unavailable-original-byte messaging. It checks one full-document edit in one broker transaction, edit capability/scope on every staging request, bounded aggregate staging, Unicode and XML encoding-declaration preservation, simulated broker permission denial, unavailable original bytes, watchdog timeout, parent cancellation, recovery on the next launch, and a controlled two-component concurrent run. Production permission authorization remains covered by the protocol broker tests; the component case proves denial propagation across the authenticated boundary.
- Qualification processes use the same restricted-token launcher, kill-on-close job, 512 MiB job/process cap, one-process limit, minimal environment, component/runtime file grants, authenticated named pipe, component hash check, WASI authority restrictions, and existing execution budgets as the production path. Local components remain below the production package-signature gate; the test does not add a trust bypass.
- The runtime watchdog remains the authoritative overall execution deadline. Host-side broker I/O uses the same budget plus a fixed one-second grace, beyond the watchdog's 50 ms trap grace, so a component cannot convert a simultaneous broker read timeout into a successful invocation by handling the broker error. The interactive/background execution budgets themselves remain 5,000/120,000 ms.
- Opt-in JSON-line telemetry reports the authenticated connection, component hash/read, runtime initialization, compile, instantiate, first broker request, first broker response, execution, and parent-observed shutdown. Restricted qualification hosts write through a 64 KiB sink to one pre-created receipt file whose DACL grants the restricted SID write access to that exact file only; the parent replays it into retained stderr after exit. Tests reject missing, oversized, or wrongly correlated child records, require the full phase set for successful cases, and retain the last started response phase for cancellation and timeout cases. Every child phase/start and parent case/interruption record carries the same unique case ID and host PID, so concurrent output remains attributable. Case records include component SHA-256/size, release profile, text/original sizes, requested and response bytes, request count, and the host and parent deadlines.
- Windows correctness CI runs exactly three selected fast tests in the release profile and retains the unique evidence directory. The opt-in nightly performance workflow runs exactly two selected large tests as informational work and retains its unique evidence directory; zero or unexpected selection counts fail, and the command exits nonzero on any semantic failure even though the performance job does not gate correctness.

### Focused checks performed in the private worktree

- `rustfmt --edition 2024 --check apps/extension-host/src/lib.rs apps/extension-host/src/main.rs apps/extension-host/tests/first_party.rs crates/platform-windows/src/lib.rs crates/platform-windows/src/process.rs` — passed after formatting.
- PowerShell AST parse of `scripts/test-first-party.ps1` — passed.
- Standalone synthetic-receipt checks of the script's source-identity validator — passed for a stable chain and rejected unavailable, changed-during-stage, and between-stage identities.
- `python .github/workflows/run_test_evidence.py --self-test` — passed; retained failures, spawn errors, hashes, and no-overwrite behavior remain intact for the reused T09 wrapper.
- Python/PyYAML parse of `.github/workflows/ci.yml` and `.github/workflows/perf-nightly.yml` — passed.
- `git diff --check` — passed (Git reported only the repository's Windows line-ending conversion notices).
- No Cargo command, component build, native host execution, or desktop operation ran in this private worktree, as required by the serialized integration contract. Therefore no new runtime timing or semantic pass is claimed here.

### Focused commands proposed for the root reviewer

Run from the integrated original checkout with its shared incremental cache, serially:

```powershell
./scripts/test-first-party.ps1 -Suite Fast
```

This is the release correctness gate. Inspect the printed `target/first-party/<run-id>/run.json` and its retained test stdout/stderr for three selected passing tests and every expected correlated JSON phase/case record, including `profile="release"`, hashes, byte counts, 5,000 ms host deadlines, 7,000 ms parent deadlines, timeout exit 124, cancellation, and the successful post-interruption launch.

Run the large informational qualification separately:

```powershell
./scripts/test-first-party.ps1 -Suite Large
```

Inspect the printed unique run manifest and raw logs for two selected passing tests, the 1 GiB JSON request count/chunk bound, the 5 GiB synthetic-source marker and single bounded original-byte request, component hashes, and correlated phase timing. A nonzero result is a semantic failure or explicit non-shipping limitation and must be triaged; timing regression alone remains informational.

If a focused compile/test is needed while correcting integration, use the same commands the scripts select rather than a workspace gate:

```powershell
cargo build --locked --release --target wasm32-wasip2 -p bareline-json-tools -p bareline-xml-tools -p bareline-hex-view
cargo test --locked --release -p bareline-extension-host --test first_party fast_release_ -- --ignored --test-threads=1 --nocapture
```
