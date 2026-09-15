# PR-T09 — Reliable synchronization and test evidence

- Issues: TECH-011/012
- Baseline source: `7c1d60ec`
- Scope: `ActorGate` contract tests, journey diagnostics, and truthful run/result accounting.

## Contract decisions

- Synchronization tests assert predicate progress, notifications, cancellation, and bounded completion without requiring a production condition variable to sleep for a minimum duration.
- Journey evidence preserves failed aggregates and labels retries separately. Run identity binds the executable, HEAD, and working-tree source manifest.
- Top-level journey totals exclude child fixture/subprocess summaries.
- p0-7 and p4-4 diagnostics record provider-observed editor/window/frame state before deciding whether a failure is harness setup, product behavior, timeout, or environment blockage.
- Every run writes a unique JSON ledger under `target/journey/results`; `--retry-of=<run-id>` links a retry without replacing the earlier result. The ledger hashes the exact executable plus a framed manifest of the HEAD-relative tracked diff and every untracked path/content, then records only explicit top-level journey outcomes.
- p0-7 records its isolated roots, PID/HWND, foreground state, window/client bounds, DPI, renderer, live UI Automation focused element/TextPattern document and caret/selection, native menu injection path, diagnostic tail and complete dialog/control tree. The provider has no dirty property, so that field is explicitly unavailable; exact document text is required before Close. Prompt absence is retained as a timeout with screenshots and context.
- p4-4 retains before/after screenshots, the exact sampled glyph region, input text hash/UTF-16 unit count/channel, live TextPattern document/caret, stable-frame receipt, window geometry/DPI/renderer and saturation values. It evaluates the color-glyph assertion only after UIA observes the intended emoji and the frame receipt is stable. Pixel change alone is recorded only as an input attempt.
- `.github/workflows/run_test_evidence.py` wraps one top-level Cargo command, captures HEAD plus a fingerprinted dirty-source manifest before launch, excludes its own three output paths, and records the post-run identity and whether source changed. It preserves separate raw stdout/stderr with hashes, retains spawn errors and timeouts as harness failures, records the actual process result as one top-level result, and lists nested/parent `test result:` lines without adding their counts. It refuses to overwrite a prior run.

## Focused checks

- Root owns all Cargo and native runs after source review.
- ActorGate normal scheduling, loaded scheduling, notification-before-wait, notification-during-wait, injected spurious notification, cancellation, and idle watchdog checks.
- `cargo test -p bareline-editor-surface actor_gate_ --offline --locked`
- `cargo test -p xtask --bin xtask journey::evidence_tests --offline --locked` validates failure classification and explicit top-level accounting without parsing child fixture output.
- `python .github/workflows/run_test_evidence.py --self-test` exercises a failed parent command whose raw output contains both child and parent summaries, retains a spawn failure, and proves retry evidence cannot overwrite it.
- Isolated p0-7 and p4-4 runs plus the full native suite from fresh private profiles remain pending the final integrated binary.

## Unresolved qualification

- Do not alter close behavior or weaken the emoji detector without a retained reproduction proving the product or detector defect.
