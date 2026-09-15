# PR-T13 — Prevent new lint debt without hiding existing debt globally

Priority P3. Fixes TECH-016. Dependencies: land after shared-core changes settle or keep this PR restricted to tooling and narrowly scoped annotations. Files: `.github/workflows/ci.yml`, `clippy.toml`, focused debt locations, a new local checker if needed.

## Current state

Strict `cargo clippy --workspace --all-targets --locked -- -D warnings` fails. The same command with CI's 31-category `CLIPPY_BASELINE_ALLOW` succeeds. The workflow claims a ratchet, but global category suppression accepts unlimited new instances of those categories. This is maintenance debt, not evidence that every warning is a user-visible bug. `../evidence/clippy-strict.log` contains a partial failing compilation, so its warning count is not an exhaustive census.

## Implementation

1. Capture a complete current diagnostic census with stable source/function IDs, lint code and justification; avoid raw line-number-only baselines. Prioritize correctness/resource warnings separately from style.
2. Replace whole-workspace category allows with narrowly scoped, justified `#[expect]`/`#[allow]` where the supported compiler permits, or a structured baseline diff that fails only new instances and drops resolved entries. Intentional false positives such as a vector of ranges require a reason.
3. Add a self-test fixture that introduces a new instance of an existing debt category and proves the checker rejects it. Add removal/rename/line-shift fixtures to show that valid code movement does not silently reset the debt budget.
4. Burn down a small coherent category only if its edits are mechanically reviewable; avoid rewriting unrelated large modules in this tooling PR. Keep a follow-up list for design-affecting items such as large enums and many arguments.
5. Update CI comments and local commands to the actual guarantee. Retain strict handling of previously clean lint categories and existing format checks.

## Acceptance

The current reviewed debt can pass, but adding a new warning of an already-baselined type fails. Removing debt reduces the baseline. Correctness warnings cannot be added by extending a global allowlist. Full raw Clippy and the ratchet's result are both retained as evidence. Run the checker self-tests and one full Clippy gate; no unnecessary application test loop for annotation-only changes.

[Implementor contract](../IMPLEMENTOR_CONTRACT.md) · [Audit findings](../technical/findings.md) · [Test evidence](../TEST_REPORT.md)
