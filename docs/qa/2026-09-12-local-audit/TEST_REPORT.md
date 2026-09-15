# Fresh test and review evidence

Date: 2026-09-12, Windows host, Rust/Cargo 1.98.1. Project: `D:/Notepad_REq/Bareline_Product_Blueprint/Bareline-Editor`. This report uses current local source and generated fixtures. No remote repository was searched. All Cargo dependency operations used the existing lockfile; advisory policy was explicitly offline.

## Reproducible source identity

- Local HEAD: `51cd4c0`, plus the pre-existing working-tree changes recorded in [working-tree-status.txt](evidence/working-tree-status.txt).
- [source-manifest.json](evidence/source-manifest.json) hashes 414 application/core/extension/build/packaging files; its SHA-256 is `a560aae60c4cadcf262568b1b04ca98e8467026a2a313bc01da28358958ffbe3`.
- [vendor-manifest.json](evidence/vendor-manifest.json) separately hashes the 22 checked-in vendored files. The locked dependency graph is [cargo-metadata.json](evidence/cargo-metadata.json).
- All 414 source-file hashes and 22 vendored-file hashes were checked again after the technical audit: **zero changed** ([verification receipt](evidence/source-verification.json)). Audit artifacts and builds are new; production source edits are not part of this review.
- Sol's initial native suite used SHA-256 `98f584cfc06fcfea70efe75221adaade0db10e4d76552d5fb7c9745c6a1711e7`, whose timestamp was newer than product source. The product was then rebuilt, and manual defect reproduction used isolated copy SHA-256 `c6b2318bcb92aaa47525320689971c690550c52358fb09cc7716409eb9f4c58f`.
- A later single-package local build produced `e3f283c8cc61ccae5d1549a1136b899d02abf3cf95a27c9fc7be8a42d7128702`. Do not relabel the earlier native receipts as execution of that binary: their hashes remain explicit. All builds used the same audited product source snapshot.

## Automated runs

| Check | Actual result | Evidence |
|---|---|---|
| `cargo build -p bareline -p xtask --locked` | Passed on retry; first attempt could not replace the executable while the native test owned it | [initial](evidence/cargo-build.log), [successful retry](evidence/cargo-build-retry.log) |
| `cargo test --workspace --all-targets --locked --no-fail-fast` | **612 passed, 1 failed, 7 ignored**, excluding five nested subprocess fixture summaries; exit 101 | [full raw log](evidence/cargo-test-workspace.log) |
| Failed `ActorGate` test, 10 isolated reruns | 10 passed; original failure remains recorded as a flaky test | [reruns](evidence/actor-gate-reruns.log) |
| `cargo fmt --all --check` | Passed | [log](evidence/cargo-fmt.log) |
| Strict workspace Clippy with `-D warnings` | Failed on existing lint debt; this was not a complete diagnostic census because compilation stopped | [log](evidence/clippy-strict.log) |
| Workspace Clippy with the exact 31-category CI allowlist | Passed | [log](evidence/clippy-ci-baseline.log) |
| `python .github/workflows/check_portability.py` | Passed, including negative fixtures; does not run Linux/macOS native UI | [log](evidence/portability.log) |
| `cargo deny --offline check` | Advisories, bans, licenses and sources passed against cached local data | [log](evidence/cargo-deny-offline.log) |
| E2E Python harness contracts | 6 passed; no native journey execution inferred | [log](evidence/python-e2e.log) |
| Performance Python harness contracts | 13 passed; no product performance qualification inferred | [log](evidence/python-perf.log) |
| `xtask qa validate` | Manifest valid; no journey executed by this command | [log](evidence/qa-validate.log) |
| Current WASI first-party release build | JSON, XML and Hex built successfully, offline and locked | [log](evidence/first-party-build.log) |
| Opt-in first-party integration with debug host | 1 passed, 3 failed; XML timeout before first broker request; large JSON exceeded its parent budget | [log](evidence/first-party-integration.log) |
| Debug host, small authenticated component test rerun alone and serially | Failed again on XML exit 124 (12.97 seconds); parallel test contention alone does not explain the failure | [log](evidence/first-party-debug-small-serial.log) |
| Opt-in first-party integration with release host, serial | **4 passed**, 0 failed; 95.13 seconds | [log](evidence/first-party-release-integration.log) |
| Ignored multi-GB divergent diff traversal | Passed; 4,294,967,296 synthesized bytes traversed with reported 256 KiB source/512 KiB window budgets | [log](evidence/diff-multi-gb.log) |
| Initial native journey aggregate and isolated close-case reruns | 12/13 aggregate; failed close-prompt case then passed 3/3 isolated reruns; initial binary identity above | [aggregate](ui-ux/evidence/journey-all.txt), [reruns](ui-ux/evidence/journey-p0-7-reruns.txt) |
| Final native journey aggregate on rebuilt current binary | **11/13**; close prompt absent in p0-7 and no color-saturation change in p4-4 (165 before/after); causes unresolved | [current raw log](ui-ux/evidence/journey-current-binary.log) |
| Exploratory native workflow interaction | Sol report records exact successful substeps, reproduced defects, blocked cases and limitations | [coverage](ui-ux/coverage.md), [findings](ui-ux/findings.md) |

The failing workspace assertion was `paged_view::peer_tests::actor_gate_wait_returns_within_the_timeout_when_the_worker_is_idle`, at `paged_view.rs:3650`: elapsed 19.9607 ms against a strict >=20 ms expectation. A passing rerun does not erase the initial failure. Two of the seven ignored tests are intentional child-process entry fixtures exercised by their parent containment test; the other five were subsequently run explicitly (four real components and one multi-GB diff).

The 1 GiB JSON release test reported 88.588 seconds, 16,384 broker requests, maximum chunk 65,536 bytes. The 5 GiB Hex test uses an actual bounded range provider over a generated source, not a 5 GiB disk-file open/UI test. The multi-GB diff test also uses synthesized pages. These are valuable functional/resource checks but must not be presented as disk throughput or Notepad++ comparisons.

## New issue reproductions

| Probe | Actual observation | Scope |
|---|---|---|
| [task_probe.rs](evidence/task_probe.rs) / [log](evidence/task-probe.log) | Canceled task yields buffered success; Drop fails to cancel; a panic loses the worker and terminal result; private queue stalls behind blocked worker | Includes exact current task module; controlled panic in standalone process, not in the user's editor |
| [launch_probe.rs](evidence/launch_probe.rs) / [log](evidence/launch-probe.log) | Failed migration is never retried; temp sweep follows a junction and deletes the generated sentinel outside its supplied temp root | Exact current helper functions extracted unchanged; every created/deleted target remained inside the verified audit fixture |
| [save_probe.rs](evidence/save_probe.rs) / [log](evidence/save-probe.log) | Existing Save As destination conflicts; a writer injected between preflight and real Windows replacement is overwritten while save succeeds | Linked against current Cargo-selected library artifacts; actual production save and Windows commit, controlled interleaving adapter |
| Sol native Save As | Accepting overwrite yields conflict, dirty tab and retained staging file | Real Windows dialog and scratch files; exact screenshot/tree evidence in native report |
| [`--version` subprocess](evidence/cli-side-effects.json) | Prints version with exit 0 but moves a generated settings file and deletes the cache sentinel | Actual current binary, isolated profile/TEMP environment; informational command side effects reproduced |

Recompile probes from their source if needed; [reproduction instructions](evidence/REPRODUCE.md) describe compilation, artifact selection and fixture setup. `task_probe.rs` uses a relative include. The launch probe's helper extraction is frozen evidence; compare it to current `launch.rs` before reuse. For the save probe, derive `--extern` paths from `cargo build -p bareline --message-format=json` ([artifact listing](evidence/cargo-artifacts.jsonl)), not by choosing an arbitrary `.rlib` in the incremental cache. Each filesystem probe expects a fresh fixture root; use a new owned directory rather than deleting/reusing user data.

## Technical coverage and limitations

| Area | Reviewed/executed in this pass | Remaining qualification |
|---|---|---|
| Document, undo, source provenance, paging | Full workspace behavioral suites; source orientation and current lifecycle/actor interfaces; ignored large diff executed | Native multi-GB edits, memory pressure, real slow/error-prone volumes |
| Open/save/reload/recovery/session | Focused source paths, crash-recovery tests, migration/janitor/save interleaving probes, native save/close/recovery observations | Power-loss/VM install matrix, delayed/disallowed recovery purge native repro, all permission/volume combinations |
| Search/replace/diff | Workspace tests (including search 40 tests), paged/folder/regex capability paths, multi-GB synthetic traversal; Sol native coverage separately | Complete native folder replacement/rollback matrix and real long-line/slow-volume workloads |
| Syntax/language/first-party extensions | Syntax/UDL tests, broker/package/isolation/sandbox tests, actual WASI release components across host boundary | Owner-signed catalog/runtime installation and UI-level feature discoverability |
| Commands/macros/processes | Commands/macros tests, shell-placeholder validation, worker/transport ownership review, native Run/manager inspection | Full external-tool workflow under every policy and physical input path |
| Rendering/settings/accessibility | Recording/semantic/themed tests, source ownership review, Sol native explorer/UIA checks | Physical IME, dead keys/AltGr, Narrator, high contrast, mixed DPI, all window sizes; headless pass is not native qualification |
| Packaging/security/dependencies | Local build, offline dependency policy, build/workflow/manifests and trust configuration review, parser mutation corpus | Fresh online advisory refresh, owner signing, clean-VM installer/update/rollback, both Windows support floors |
| Platform abstraction | Neutral guard and current Windows compile | Actual macOS/Linux host compilation and native implementation; Windows-first product scope remains explicit |
| Performance/product parity | Existing harness contracts, current capability/status evidence, release component timing | Matched controlled release comparison, complete parity evidence import and owner product-claim approval |

No comprehensive fuzzing, sanitizer campaign, hostile environment exhaustion, online endpoint probing or remote CI run occurred. Those are explicitly outside the evidence, not inferred successes. Plans distinguish reproduced defects from maintenance and release qualification work.
