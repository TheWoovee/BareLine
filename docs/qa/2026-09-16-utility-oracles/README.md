# Utility command oracle checks — 2026-09-16

Four previously generic command definitions now have exact procedures: Base64 encode/decode and URL encode/decode. Twelve success/disabled/failure plans share eight success vectors and fourteen negative vectors with the actual Rust utility core tests. Unicode selection offsets, unchanged surrounding bytes, whole-document output, snapshot immutability, Undo/Redo, dirty state, cancellation, output budgets, invalid decode and stale-result rejection are checked.

## Focused results

| Receipt | Result |
|---|---|
| 01-python | 4 plan/reference-codec tests passed; unknown commands remain unbound and no plan is imported as acceptance. |
| 02-rust | Initial test-helper compile failure: `unwrap_err` required a Debug success type. Preserved as failed history. |
| 03-rust | Corrected helper; all 4 selected core contract tests passed, 146 filtered out. |
| 04-clippy | Targeted app library/test check passed; no diagnostics in the new test module. Existing repository warnings remain. |

Formatting and `git diff --check` passed. No full suite, production build, desktop automation, installation or release operation was run.

These are core checks and proposed native observations. Native routes, read-only behavior and saved-file observations still require actual execution and independent review; no runtime command or AC is newly qualified. The remaining 495 historical commands still need concrete plans. The top-level queue remains **7 completed/dispositioned, 41 active, 1 owner-deferred**.

## Retained artifacts

The [collection plan](collection-plan.json) includes the exact receipts and raw logs, fixture catalogue, source, generated definitions and backlog. The [bundle pointer](retained/bundle.json) and [verification](verification.json) retain the digest and byte-integrity result. Source identities belong to each original receipt; subsequent documentation updates are not relabelled as the tested source. Nothing in this record approves production release.

Manifest SHA-256: `7c9824c3cde9eae375775763824731eab73ec2e66f8fa4e3e0f40a790a62eac4`. All four captures report stable source during execution.
