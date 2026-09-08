# Integration batch 09 — 2026-09-08

Status: automated gate passed after exact repairs; package completion labels remain subject to full-scope review. No manual QA, native performance benchmark, remote publication, signing, or release acceptance was executed.

Source checkpoint: `58a48c08142221dc2ffc8b7989c2543791349068` plus the recorded compiler and regression repairs in the following integration commit. The isolated `closure-final` branch starts at that unverified checkpoint; it requires these repairs before its own gate.

## Automated evidence

The combined affected gate used offline, locked Cargo with `bareline/perf-spans,bareline/qa-inventory`. Passed targets were preserved; only failed targets and directly affected selection consumers were rerun.

| Target | Passing tests | Evidence |
| --- | ---: | --- |
| Native application | 28 | `target/integration-batch-09-native-final.log` |
| Application model | 84 | `target/integration-batch-09-rerun5.log` |
| Diff | 23 | same; 1 existing ignored test |
| Document | 20 | `target/integration-batch-09-targeted.log` |
| Editor surface | 42 | same |
| File I/O | 58 | `target/integration-batch-09-rerun5.log` |
| Recovery process-death integration | 3 | same |
| Windows backend | 48 | same; 2 existing ignored tests; 5 nested child reruns excluded |
| Recording renderer | 1 | same |
| Search | 33 | same |

The focused selection follow-up passed 3 paged peer tests and 14 workspace tests (`target/integration-batch-09-selection.log`). These are reruns, not additional coverage. Comparing successful function names against `0b65e2c` identifies 36 newly passing functions, bringing cumulative recorded distinct Rust coverage from 411 to 447. No outstanding observed failure remains in this batch.

Default native build: PASS, 28.95 seconds, `target/integration-batch-09-build.log`. SHA-256 of `target/debug/bareline.exe`: `0879F8F63A0A208E693AE6EB68156D656C84E5E33ACF6550D1DCDF962AB6440D`. Three warning groups remain; this is not a warning-free claim.

## Resolved gate findings

Compiler repairs corrected a new tree-field test pattern, closure error type inference, fingerprint and paged-surface field paths, and use of the existing public budget claim API.

History admission initially copied exact-sized storage on every growth. Geometric growth now charges both old and new allocations during replacement. The unchanged 100,000-edit/undo/redo oracle passed; the document target fell from 422.34 seconds to 1.28 seconds. Quota fixtures now include explicit charged storage, and paged lease fixtures no longer invoke a resident-only sealing operation.

Streamed indentation now treats actual empty terminated rows consistently with resident commands. The navigation fixture accepts a synchronously ready result only after validating its exact identity, source coordinates, frame readiness, and unchanged selection.

Large compare merges retain preparation while the destination is busy and wait for the UI to install the worker reply after durable completion. Linked promotion retains its selection-validation token through transient lock contention until the authoritative result is applied. The original large Unicode merge, tail-byte, undo, linked selection, tab identity, and accessibility golden assertions all pass unchanged.

PR006 remains in progress for authoritative paged non-transform operations. PR008/013/015 and other remaining work in `closure-final` are outside this tested source state. Passing this gate does not constitute physical accessibility, release, or user acceptance.
