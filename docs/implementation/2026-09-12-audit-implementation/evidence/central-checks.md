# Central contained checks

All checks run in the original integration checkout. The original index remains unchanged. A one-time workspace-package artifact invalidation removed incompatible outputs from prior worktree builds; registry dependencies were retained. Further checks reuse only this checkout's target.

## T06, source snapshot 9545d3347216c54e0820e8c21e163d7e0298af32

`cargo test -p bareline-app --lib task::tests --offline --locked`: passed, 11 tests, zero failures. Native T17 changes were integrated during this command; they do not change the tested app library sources.

An initial accessibility filter `scheduled_read_clears_pending` matched zero tests and is not verification. The exact scheduled cleanup test is run separately and recorded below when complete.

`cargo test -p bareline-app --lib accessibility::tests::paged_read_terminal_cleanup_rejects_stale_results --offline --locked -- --exact`: passed, one scheduled failure/stale-result cleanup test. Log: `T06-accessibility-exact-central.log`.

## T17, source snapshot 6fdaf57ed55f99c2146d8d647fa41d8721efe608

`cargo test -p bareline --bin bareline windows_app::launch::tests --offline --locked`: native test binary compiled; eight launch tests passed. Log: `T17-launch-central.log`.

`cargo test -p bareline --bin bareline windows_app::instance::tests::portable_handle_diagnostics_stay_out_of_single_instance_forwarding --offline --locked -- --exact`: one test passed. Log: `T17-instance-central.log`.

The retained migration receipt consumer is intentionally pending T02 and currently emits one dead-code warning. No full suite or release build was run for these PRs.

## T03, corrected source snapshot 1708befe0a119506cab01d854467cc8fc4130335

The initial contained file-io compile failed because the fault-transition test seam still passed the old fingerprint option. The implementor corrected that caller; the original log `T03-preflight-central.log` is retained. The corrected preflight cohort passed two tests (`T03-preflight-corrected-central.log`).

Native compilation and the real Save All read-only-rejection/next-entry test passed (`T03-save-all-central.log`, one test). Encoding save passed one test; paged edit/undo/save passed one test; close identity passed two tests (`T03-encoding-central.log`, `T03-paged-save-central.log`, `T03-close-central.log`).

The expanded Save Copy/history test failed on its dirty-state assertion after a newly added insert/undo sequence (`T03-save-copy-central.log`). Returned to the implementor to distinguish undo grouping in the fixture from save behavior. This case remains pending; the already passing unrelated cases will not be rerun solely for that correction.

## T03 final corrections

The Save Copy/history fixture now isolates stale-revision behavior from undo grouping and compares the Windows canonical prepared destination. The stale prepared-save case passed (one test, both document modes); final Save Copy/history passed (one test, both document modes). Logs: `T03-stale-revision-central.log`, `T03-save-copy-final-central.log`. Intermediate failures remain retained. Nine targeted T03 cases now pass overall.

## U01, source snapshot 97fcf41ce355874358129453cf2a2f77d2ee0095

Native compilation and eight contained tests passed: accessibility four, Run prompt two, Go To two. These cover Unicode text bounds, modal ownership and identity, existing Find focus, semantic JSON, prompt parsing and visible button focus. Logs: `U01-accessibility-central.log`, `U01-run-prompt-central.log`, `U01-goto-central.log`. Physical native interaction and screen-reader qualification remain in final audit scope.

## T07, corrected source snapshot 1e54c028dda757408084c1aad1cee601a8142394

Platform executor tests passed 5/5. The first app compile failed on callback ownership (T07-task-central.log); the reviewed Arc-clone correction fixed that compile. App task tests passed 12/12 (T07-task-corrected-central.log). Actual paged unwind completion/wakeup and gated save with concurrent edit/undo each passed (T07-paged-unwind-central.log, T07-paged-fairness-central.log). No full gate ran. The unused test-only worker_count helper is returned for removal without a repeated behavioral run.

## T01, source snapshot c3c13f19e377729809451739a4c5026302b3e16c

Ownership policy passed 8/8 (T01-policy-central.log). Actual Windows cleanup passed 6/6 (T01-windows-central.log): nested successful handle deletion, reparse junction rejection with external bytes intact, replaced root, finalization candidate replacement, locked partial purge and blocked-primary/fallback full-sweep retry. Launch integration compile/cohort is running; no full suite was invoked.

T01 native integration compile completed; current launch cohort passed 7/7 (T01-launch-central.log). The old wildcard-cleanup test was replaced by the policy/native cleanup cohorts above. The retained take_completion consumer remains intentionally pending active T02.

## U07 and T08, corrected source snapshot 96f3566661cb1f397a89106f7aef00927793c0c0

U07 initial 10 tests passed: Search panel 3, tab/provider 1, search provider 1, recovery provider 1, recovery transitions 4. Final active-layer regressions passed 2/2 (U07-legacy-layer-corrected-central.log). The preceding legacy-layer compile failure came from T08 missing Arc, corrected by its implementor. Both capture-stop logs are intentional candidate generation, not passing tests. Corrected 97-case baseline structurally reviewed before installation; exact assertion follows.

T08 compile failures (watchdog channel type then inventory Arc import) were returned and corrected. Actual cohorts now pass: manager 10/10, inventory 5/5, real transport spawn refusal 1/1, failed watchdog transfer terminates/reaps controlled child 1/1, host pre-execution watchdog refusal 1/1, safe component/CPU-loop termination 1/1. Host commands also enumerate other test targets with zero matching tests; the named library tests each actually ran and passed. No first-party or full suite pass is claimed here; release actual components stay T10/final scope.

T12 root static check passed for 31 workspace members, resolved dependency rust-version metadata, workflow versions and negative fixtures. Evidence: T12-static-central.log and T12-cargo-metadata-central.json. Installed pinned Rust 1.98.1 matches the declared minimum; final full qualification will cover that same compiler without another download.

The explicitly reviewed U07 baseline assertion passed 1/1 on snapshot 7a17953c07961d2d8ea03f9933ae0845b2657581 (U07-golden-assert-central.log; all 97 semantic cases internally validated).

## U06, source snapshot 07ff373f00a18ef77895962fb9f1bc370eeff3ac

The rendered stale-context Close and shared Invoke/Escape effect regression passed (U06-manager-close-central.log, 1 test). The initial native runtime state fixture failed because an empty macro is invalid, not because Close was disabled. Root returned that fixture and reviewed a valid minimal command plus a real Running/Waiting playback assertion. Corrected six-state test passed (U06-close-state-corrected-central.log, 1 test). Native pointer/UIA/Escape focus-restoration remains final audit scope. No whole application gate was run.

T18 initial tracked-open compile failed on five unindexed child-module PendingIo initializers; retained T18-tracked-open-central.log. Corrected source snapshot 4c746299: T18-tracked-open-corrected-central 1 pass; T18-launch-central 5 pass; T18-follow-central 1 pass; T18-ipc-central 1 pass. Actual request, paged navigation, owner-follow and abandoned-sender contracts exercised. Final native handoff journeys remain pending.

U08 source integrated on 232b037; initial U08-panes-central compile failed (test Input import/private dispatcher), retained. After ce9ec5db at snapshot190410a7, U08-panes-corrected-central failed because its wrapper invoked the tab-only route. Independent U08-paged-central 1 pass; U08-generation-central 1 pass; U08-retirement-central 1 pass; U08-provider-central 11 pass; U08-tab-id-central 1 pass. Root reviewed shared production close extraction ba572832, snapshot7c1d60ec, and U08-panes-route-corrected-central 1 pass. Total 16 passing focused cases; earlier failures remain separate. Combined semantic baseline review deferred until U11 to avoid repeated baseline churn. Fresh native split/layout/TextPattern journeys remain pending.

T02 initial engine aborted with stack overflow plus source-drift state mismatch (T02-engine-central; isolated T02-generation-diagnostic-central retained). Reviewed correction d5d35cee: T02-engine-corrected-central 9 pass. Native migration4 pass and native staged resume1 pass at dc360ff; native-retention first failed without detail, corrected run at d5d35cee still failed Access denied before Published (T02-native-retention-corrected-central); diagnosis remains open. T02-manager-central12 pass1 failed from incomplete positive fixture; corrected deferred permission1 pass. T02-macros-central4 pass; T02-settings-central3 pass. T02-settings-draft-central compile failed because fixture referenced inaccessible sibling controller helper; correction pending. No earlier failure overwritten or counted as qualification.

## U09 and T02 settings fixture — snapshot 91cc150ceb2e72c61208bbc7c155e5f231ee246c
- T02-settings-draft-corrected-central: 1 passed; explicit SettingsController construction fixes the prior sibling-helper compile error.
- U09-app-source-central, U09-app-cancel-scope-central, U09-app-terminal-central: 3 passed (one each). Bound document/revision ownership, cancellation with detach/rebind, and stale terminal result rejection.
- U09-split-source-central, U09-paged-source-central, U09-split-selection-central: 3 passed (one each). Actual pane activation retires prior results, the large paged source preserves pane-local search/replace behavior, and selection scope comes from the active split view.
- U09 total: 6 contained behavioral tests passed. Final native coverage and reviewed combined semantic baseline remain pending after U11. No full gate was run.

## T02 native directory publication correction — snapshot ec0d5f488bce0f523850889969b51a9f07e2dd79
- T02-engine-native-publication-central: all 10 migration engine tests passed, including failed quarantine, source absence, and reader authority across retry.
- T02-native-retention-final-central: actual Windows directory migration/retention case passed. The earlier Access Denied result is resolved by the reviewed root-pinned publication correction.
- Passing prior native migration and staged-resume checks remain recorded; their unchanged adapter boundaries were not rerun. No full gate was run.

## T04 initial contained compile — snapshot 5cf9df30ffdec3f038ca93675ff0eeaad06448e9
- T04-fault-transitions-central failed before running tests: E0282/E0283 at crates/platform/src/lib.rs:540 and :546, ambiguous Result error types in Option.map().transpose. Sent to implementor for bounded annotation correction including analogous native patterns.
- Subsequent native race and cleanup-restart commands in the serial chain did not run. Their evidence names remain unused.

## T04 continued contained qualification
- 67fbae06: T04-fault-transitions-corrected-central passed 3 top-level cases, including storage-full and process-death transitions. Native target compile initially failed Read/Write by_ref ambiguity and missing Cancellation imports; isolated corrections applied.
- a323466b: T04-native-race-corrected-central failed expected conflict; cleanup-restart exposed Rust OpenOptions create_new validation lacking write(true). Both runtime cases retained, not passed.
- a323466b: app recovery-discovery and cleanup-owner focused cases passed (1 each). Selection fixture failed because its shared fake adapter gives both paths the same file identity; production dedup was preserved.
- 676dd846: selection fixture corrected with its own distinct-identity adapter; T04-app-selection-corrected-central passed (1). New native diagnostic printing temporarily failed compilation because Saved lacks Debug; fixed without changing product types.
- ca744a6b: T04-native-race-diagnostics-corrected-central and T04-native-cleanup-restart-runtime-corrected-central both execute but fail at actual ReplaceFileW with OS1175 (Unable to remove the file to be replaced). Direct cleanup fixture also reproduces without open_utf8. Implementor investigating own handle/sharing boundary; these are unresolved.
- All initial failure logs retained. No full gate run. Remaining native save/cancel/crash and actual shell conflict-action checks have not yet run.

## U12 and T09 focused checks
- U12-tooltip-central on ca744a6b: 3 passed (shared tooltip/UIA names and selected states, one-shot deadlines/dismissal, measured wrapped DrawOp content).
- U12-short-replace-central first failed because 160px cannot fit replacement field plus summary/status. Reviewed fixture-only correction uses 180px; U12-short-replace-corrected-central on 56b71f73 passed (1), including 120px suppression. Production overlap guard unchanged.
- U12-vertical-geometry-central initially did not run: three T04 shared Shell fixture PathBuf moves failed compilation. Reviewed test-only clones integrated; corrected run is pending.
- T09 accounting script self-test on 8c38e8b passed, preserving nested summary text, failed top-level command, spawn-error receipt and overwrite refusal.
- T09-actor-central on 8c38e8b: 5 passed (notification before/during wait, spurious notification, cancellation, idle watchdog), using the shared production predicate implementation. Loaded scheduling and native p0-7/p4-4 remain final-audit checks; xtask pure evidence tests still pending.

Snapshot 8c38e8b520bb85cc141f6842dfaea7588f39f389: T09 actor 5 passed; U12 corrected vertical geometry 1 passed; T04 actual Shell compare, separate-pump failure drain, and Save Elsewhere 3 passed. See respective central JSON/log receipts. Native OS 1175 remains unresolved pending R6 reproduction.

Snapshot325c7c1fb7570cea5a2aaf7042c80bae4299c77d: T04 native replacement race and cleanup restart now PASS (2); copy/boundary cancellation, independent orphan copy, cleanup warning, malformed restart PASS (5). Native substituted transaction fixture FAILED (0 records expected1), returned to same agent; safety qualification remains pending. T09 xtask evidence pure cases PASS (2). Prior failure logs retained.
T04 native sharing violation 1 PASS and replacement process-death cohort 2 top-level PASS on snapshot325c7c1fb7570cea5a2aaf7042c80bae4299c77d. These are actual Windows calls, separate from prior neutral helper transitions.

T04 native-substitution-corrected-central: 1 PASS on642d82f75d8e586957f56e99cab2ecedfc62fde8. All proposed T04 contained cases now pass, including 11 actual native top-level cases across correction receipts; prior failures remain recorded. Final integrated/native user workflows still pending. T05-retirement-central failed before execution on87dadb08b1c1c9b0650b9f7874e737be2e047096: 3 fixture CacheDirectoryLease constructors omitted T02 fields; returned to implementor for full constructor census.

T05 on51793aef4d16fd56cf365da3c0e59adb49971a0a: corrected retirement1 PASS; small recovered document1 PASS; resident+untitled1 PASS; native manifest-gone primitive1 PASS. Earlier resident generation1 PASS on4d2d60c. Paged restart FAILED OS3 during reopened recovery. Shell cohort2 PASS/3 FAIL (read-only undo-close missing; 2 recovery-checkpoint waits). Returned exact logs to implementor; no further retries until corrections.

T13 root standalone checker self-test PASS both private reviewed tip5c19a918 and integrated64ab47fc. Full census not run. T05 diagnostic79f389e03: paged failure narrowed to Retain paged baseline <new-generation>/source-1 OS3; clean read-only Shell close now PASS, other2 checkpoint-wait failures unchanged. Same implementor investigating; prior failed reruns retained.

U10 on snapshot 8d8a17afa739b235a0bc634e8aa461a8212f87e8: U10-revert-contract-central PASS 9; U10-commands-central PASS 1; U10-shell-check-central PASS (existing unused ProfileInitializationRuntime::completion warning). Real worker/disk plus gated same-path ownership and external-change checks passed; these do not establish native atomic replacement behavior. No full suites run.
U13-known-lines-central on snapshot6e30cff06519f51c59a62f017af787fcca27c774 PASS1 actual forced-paged sparse-index near-EOF/EOF/viewport transition test. Final native/DPI semantics pending.

T05 snapshot67ee505ba9fc5e9754b237c6b18ea5fd1916d90a: hold-retirement-central PASS2 (failure/retry ownership and forced lost-wake); paged-held-source-central PASS1 (opaque bytes, restart, discard/Undo Close and fresh baseline); shell-new-dirty-central PASS1; shell-moved-owner-central PASS1. All previously outstanding T05 focused failures resolved. Prior passed unaffected checks retained; final integrated/native qualification remains pending.
U03 on18587be1: U03-native-menu-corrected-central PASS3; U03-shell-check-central PASS. First compilefailed beforetestswereexecuted andretained. Two unusedreturnwarnings correctedbydae79ee without repeatingunchanged tests. NativeGDI/pixels/DPI/navigation finalpending.

U13-responsive-known-lines-central on50c188 FAILED initial-prefix fixture assumption, not the responsiveness assertion. Reviewed test-only correction35334b6; U13-responsive-prefix-central on6e8a4e9 PASS1. U11-search-pointer-central on6e8a4e9 failed before test execution: SessionLayout initializer in crates/app/src/views.rs568 omits three new bottom-dock fields. Returned to same implementor. Remaining serial U11 commands did not run.

U11 on662cba3: corrected Search pointer1 PASS and session layout1 PASS. Dock Shell cohort failed compilation before execution: missing SemanticRole::TabList at dock.rs380 and borrow overlap across sync_bottom_dock at windows_app.rs3831. Same implementor correction queued; body-focus test not run.

T10 Fast on frozen e063214c: actual release compile PASS226.927s, actual Fast execution2 PASS/1 FAIL12.52s (raw wrapper elapsed14.345s), source chain verified unchanged1bdfd13d57dab3dbbe7e497ed049998739ae3ab173ed87718e07904659eb794c. Retained target/first-party/20260912T110035.7818574Z-38084-bc3ac54c/run.json and all raw receipts. Semantics/provenance and concurrency passed; timeout child unexpectedly exited successfully at first_party.rs559. Cancellation branch exited1; timeout/recovery-after-timeout unqualified. No host phase telemetry reached raw stderr, only parent records. Both defects queued to same implementor.

U11 onf2c7f833: dock cohort9 PASS/1 FAIL (empty Output focus editor2 vs old tab80003 expectation; real empty/body ownership and populated retained-focus issue found in root reviewU11-R3). Independent body-focus helper1 PASS; app view/session cohort5 PASS. New unused MacrosRuntime::draw_output warning queued for removal. No full gate run.

At e7648af: U11-output-focus-r3-central PASS1 (empty/populated/header/editor transitions). U04-menu-state-central FAIL:6PASS1FAIL, composed File15 rows exceeds limit14; same implementor R3. U04-menu-template-central PASS2. Native combined qualification remains pending.

T10 R2 actual Fast PASS3 at frozen fac341a (source3ef142ce48a54e4e3073c8d86901716bae214d5a2cd0cb0946b6ca4abb2f7db4). Retained target/first-party/20260912T114438.0346930Z-41892-1ace1db1/run.json: chain unchanged/available; compile242.064s, execution12.14s. Actual child phase_start160/phase176/component20; cancel restricted exit1 and timeout restricted exit124; next-launch recovery passed. Counts are3 top-level tests, not the nested event count. Full final qualification remains pending.

U04-composed-menu-r3-central PASS1 at acbd4bee: actual composed Shell File/Search fits the retained supported-height limits. Previous6passing state/routing tests and2menu-template tests remain valid; native popup qualification pending.

## Root integrated contained checks — T11/T15/T16

At snapshot `0d285d977a5ad08ec28f4024490099fe57146e40`, T15 Python contracts passed 21/21 under `target/qualification/t15-python-integrated-r2.json`; actual T09 source identity remained available and unchanged. T11 release tooling contracts passed (10 invalid configurations plus prepared/source/receipt, capability inventory, duplicate-key and linked fixture-report parser probes) under `target/qualification/t11-python-integrated-r3.json`. These are tooling checks, not editor/release qualification.

The first T16 contained command stopped before compilation because T11 app dev dependencies required lockfile links. Root ran offline Cargo metadata and reviewed the exact delta: only existing base64, blake2, ed25519-dalek and sha2 package links were added to bareline; no version or package upgrades. The failure receipt remains `T16-lifecycle-central.json`. A new contained run follows the lockfile snapshot.

At snapshot `c9a082218adc97e0bf1271139df6dea980f0dc3b`, T16 lifecycle cohort compiled and ran four tests, all failed: incomplete simulated commit fixture hid two save results, undersized open scratch prevented the release-gate fixture, and undersized recovery recipe budget prevented restart. Root sent actual failures back; assertions must remain substantive. `T16-lifecycle-central-r3.json` retains the run. The editor-surface peer command then exposed 14 incomplete API migration compile errors in read-handle imports, generic completion/result types, remaining actor locks and document field accesses; `T16-surface-peer-central.json` retains that failure. No lifecycle behavioral pass is claimed yet. One coherent worker correction and contained rerun are pending; full gates remain deferred.

## T16 accepted contained cohort

The corrected lifecycle cohort passed 4/4 in `T16-lifecycle-central-r4.json` at snapshot `05fecd863427da7a6936e42325c3f801b80affbd`. The actual surface peer, history spill, opaque linked undo/restart and cleanup-hold registration checks each passed one test; the first linked-undo filter selected zero tests and was corrected in `T16-linked-undo-central-r2.json` (the zero is not counted). After read-handle compatibility integration at `537b0534e208a97ebf3a4828d58bfb4286dfcfb6`, the four app contracts passed: paged editing/navigation/save, read-only reload, opaque recovery restart and retryable recovery retirement. Total substantive T16 contained coverage: 12 passing tests. Final native and full integration remain pending.

At snapshot `eb144de4023d8b7e8d96f9f4b88c851200c51488`, merged Shell compilation succeeds after narrow T16/T14/U02/T11 corrections. U02 contained checks passed: toast9, modal4, recovery6 and notification synchronization1 (20 tests). T14's Python adapter13 passed at `bb15b41b4af8a40e545cea88b48faf8a92d1ace0`. Inventory Rust cohort passed8/10; the two route-validation failures both identify registered search.mode.extended/literal/regex missing from validated dispatch coverage. Original failed `T14-inventory-central.json` is retained and the U04 owner is correcting the actual routing contract; export and final evidence remain pending until the route cohort passes.

T11 connected fixture r1: FAIL, retained at target/qualification/t11-connected-fixture-r1.json (452317 ms), source snapshot 89d5463f3087b733fe461b4a0e3a6e589fa29db0. T09 source digest unchanged: 45c9e268440f22dce2466f44207eda9d620c0a4c176709e50432c27ada059d05. WASI packages and three release executables built; signed metadata, local transport and preview-disabled fixtures passed. Protocol filter executed zero tests, so it is not a pass. Connected app fixture failed to compile because Path was missing; installed delivery and installed-component T10 execution did not happen. Corrective implementation requested; retain failed receipt and rerun under a new name after source settles.

U05 integrated source snapshot8ce5abc339c55a0ff9b85e80ff3cc4353b3458d5. U05-recovery-central FAIL at compile: five PendingIo initializers in workspace/encoding.rs and workspace/remote.rs omit new recovery_restore_request. No tests executed; sent exact sites to original implementor. Full31 packages integrated pending contained correction/final verification; original index unchanged and T16 .as_deref lookup preserved.

U05-recovery-central-r2 FAIL compile on3 missed rows() calls; narrow a830d correction integrated. U05-recovery-central-r3 PASS12 at16e763043b000d09e1b0650bd857785c4b5cfa13. U05-restore-terminal-central PASS2 atfeba4e92f4a4efc485c7e77ec98ffc901e4ef067. U05-paged-restore-central FAIL after behavioral assertions, AccessDenied on fixture cleanup; U05-resident-restore-central FAIL because success receipt reports empty document revision0 before recovered insertion publishes revision1. Real timing correction and bounded cleanup are assigned to implementor. All failures retained; neither real restore fixture counted as pass.

Combined semantic candidate captured intentionally via expected test failure;97cases/25changed. Candidate and full delta retained under target/qualification; reviews/final-semantic-R1.md records non-blessed findings. Windows console could not print Unicode from the review helper, but its full delta file was written before output; switched only diagnostic printing to escaped Unicode.

U05-resident-restore-central-r2 PASS1 and U05-paged-restore-central-r2 PASS1 atf30d4aa1fb84db4399d9f2cd49c3724f5cfd6b4e. Exact resident recovered insertion receipt now matches the published revision; paged behavioral and bounded cleanup assertions pass. Earlier failures remain retained.

U05-recovery-central-r4 PASS12 and U11-semantic-viewport-central PASS1 at32b04b53ad79838a7925c5f6b038094b368684b1. Final semantic R2 candidate reviewed and installed as exact bytes; candidate captures remain intentional failures, no native qualification inferred.

Final formatting: first silent command reached a logging-helper missing-file error (no formatting diagnostics), retained as harness failure; helper now precreates its unique empty log. final-format-central-r2 exits0, PASS, no Rust formatting edits required. Combined semantic golden reviewed/installed. Full Clippy census begins after final docs and graph refresh.
