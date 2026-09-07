# PR closure checkpoint — batch 03, 2026-09-08

This local checkpoint follows 021d8cb. PR001, PR004, PR014 and PR022 are implementation complete pending acceptance, based on their owning scope matrices and the combined gate. PR011 and PR025 retain that state. This is not whole-product completion. Manual desktop QA remains paused until development is complete; no remote operation occurred.

## Verification

The combined offline test command selected bareline, app, document, commands, settings, UI, file-io, editor-surface, syntax, renderer, renderer-recording, diagnostics, macros, search, extensions-protocol, extension-sdk, first-party-common, JSON/XML/Hex tools, platform-windows, distribution and xtask, with `--features bareline/perf-spans --no-fail-fast`. One authorized lockfile refresh added the optional tracing dependency edges; subsequent commands used `--locked`.

The full gate yielded 316 passes and two failures. Exact repairs restored synchronous resident-view state while retaining deferred paged restoration, and made the grouped-worker fixture handle the documented bounded Saturated outcome without losing its mutation. No assertions were weakened. The targeted native/document rerun passed 10/10 and 13/13. Effective total: **318 passed, zero outstanding failures**, excluding four supervised child-fixture reruns; the two parent-ignored subprocess fixtures are invoked by the Windows tests.

App 53/53 includes recovery restart/provenance, Save Copy alias protection, MRU retention, ordered input/power/search recording through persistence and playback, and acknowledged search identity. Windows backend 35/35 includes atomic rename/rollback and provider identity. Six pure Python performance-contract tests and the portability guard/negative fixtures passed. No native benchmark adapter or desktop QA was launched.

`cargo build -p bareline --offline --locked` passed in the default feature configuration. Native SHA-256: `06AB2DDE36C4AC89C102A77CDE903FD33FB421FEA9F31371CF419A76B4A74F46`. Renderer, recording renderer and diagnostics all-target scoped Clippy passed with optional perf-spans enabled and warnings denied. Local raw logs are ignored `target/integration-batch-03*.log`.

## Scope and limits

The PR001 startup/frame spans compile away by default; native frame, configuration and font consumers are connected. PR004 includes the actual Recovery Center comparison/keyboard/accessibility flow and lossless recent-file persistence. PR014 records one globally sequenced stream of acknowledged input, power and search actions; manager input/draw order, workspace interpolation and accessibility are connected. PR022 delivers skeletons, portable contracts, the architecture guard and three-OS CI definitions, while foreign execution remains unverified.

The default native build retains eight warning groups: portable shell state, legacy view accessors/receipt APIs, and unconsumed workspace-panel accessibility helpers/constants. Those owning PR018/PR009/PR024 integrations remain in progress; warnings were not suppressed. Other PRs retain their ordinary gaps, including incomplete paged language consumers, manager argument controls, native folder-search controls and distribution migration/shell consumers.

Disconnected files retained locally but excluded from this checkpoint: native `migration.rs`, native `shell_integration.rs`, and platform-windows `shell_integration.rs`. Packaging/workflow/benchmark scripts are source checkpoints only; passing editor tests do not verify installation, signing, native benchmarks or foreign CI. Physical input, assistive technology, visuals, controlled performance and foreign-host acceptance remain pending.
