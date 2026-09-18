# Full Windows verification round — 2026-09-18

The owner requests one complete testing round and sequential correction of any issues found. Windows is the product target. Start from `b70f5e8`, whose product source includes the menu/recovery/Tab correction at `1533223`. Preserve existing evidence and personal profiles/documents.

## Scope and sequence

1. Check current repository and hosted results; run the complete locked Rust workspace suite once.
2. Exercise diagnostic features and tooling, formatting, portability, toolchain, exact Clippy debt ratchet, packaging checks, runtime boundary, dependency policy and actual first-party Fast/Large components. Run performance/render smoke and the opt-in multi-GiB diff regression.
3. Use an isolated portable copy of the installed release executable for native feature checks, including caret Tab, selection indentation, horizontal/vertical split, independent edits, close split, Find/Replace, save/reopen and status/menu stability. Use the computer-use skill and stop on a real input-control interruption.
4. Diagnose failures before retrying. Fix one issue at a time with focused regressions, then rerun affected integration gates. Avoid rebuilding unchanged release binaries.
5. Retain receipts and observations in `docs/qa/2026-09-18-full-round`; update implementation status with actual results and limitations. If product code changes, build and install the corrected preview after validation.

## Boundaries

Use fresh immutable outputs under `target/qualification/full-round-20260918`. Do not edit source during evidence capture. Existing shipping-signature/endpoint, disposable VM, physical IME/assistive-technology/mixed-DPI and 72-hour soak requirements remain separate release qualifications; a local test pass does not close them. Linux/macOS native product qualification remains owner-deferred.

## Results

All 25 initial automated checks passed with stable source. Native editing, split layout, save/reopen and replacement checks passed. The owner additionally requested two-file comparison: generated replacement/deletion/insertion fixtures produced exactly three differences, navigation worked, copying one difference changed only the intended line, and saved Undo restored the original bytes while preserving the other file.

## Corrections identified in native testing

1. Find mode: Literal mode falls through to the Extended tooltip/accessibility name. Supply the correct Literal name and compact tooltip. Represent the three-way mode cycle as an accessible button, preserving its current mode value and invocation behavior; Case and Whole Word remain checkboxes. Cover both Find/Replace mode controls and all three modes.
2. Comparison counter: recomparison clears the result but retains the previously selected index, rendering `1 / 0` after a merge or Undo. Report a selected position only when that index belongs to the current result. Preserve remembered-hunk navigation. Cover exact results, pending recalculation, invalidation and an equal-file result.

Apply and verify these sequentially. Retain the initial native observations and intentional failing regressions. Run the final complete workspace and quality gates, build the corrected unsigned preview once, verify the actual native mode controls and two writable open-document comparison, then refresh the authorized local installation.

Focused failing-before/passing-after regressions confirm both corrections (9 Find tests and 6 comparison tests pass). The final workspace run initially caught the complete semantic golden needing its corresponding update. Reviewed the candidate structurally: only 22 Literal-mode control names and their 22 roles change; all hierarchy, focus, values, geometry and actions remain identical. The initial failure and candidate capture are retained before the final rerun.

Final locked workspace rerun passed in 107.595 seconds with stable source. Formatting, portability and toolchain gates passed; the final Clippy ratchet retains 755 reviewed Windows occurrences with no new or stale debt. Native corrected-release comparison and installer delivery follow this validated code commit.

Hosted follow-up found a pre-existing migration-test isolation/timing flaw: its depth assertion runs after migrating the earlier flat-tree fixture under a shared five-second budget, so a loaded Windows runner can legitimately leave macros Pending. Separate the flat/deep fixtures, disable only their wall-clock budget (entry/depth limits remain active), and assert the precise rejection reasons. Production migration behavior and installed executable stay unchanged. Hosted executable reproducibility also failed; inspect its retained diagnostic before deciding the next correction.

Migration isolation passes all 11 tests through the pinned Cargo executable; receipt 36 retains an incomplete inherited-pipe capture, and 36b is the complete passing rerun. Reproducibility diagnostics show identical executable code/resource sections and helper/extension-host bytes, but 225 editor bytes differ in anonymous C++ type names and derived PE/debug hashes. The two CI replicas used different rolling Windows images. A bounded MSVC probe confirms checkout paths change anonymous namespace identifiers and `/d1trimfile` makes those identifiers stable. Normalize the native bridge source prefix for MSVC and sort library translation units; retain strict whole-file equality in CI and its original failure.

The MSVC executable probe produces different full executable hashes before path normalization and identical hashes afterward. The native bridge correction passes all 25 syntax tests; the follow-up Clippy gate still has 755 reviewed occurrences and no new/stale debt. These scoped gates avoid another redundant local full-suite run; hosted full correctness and independent clean builds will validate the follow-up commit.

## Delivery and retained evidence

The corrected `de1feaf` optimized app passed both Literal/Extended/Regex controls and the requested two-writable-file comparison: three differences, next/previous, both merge directions, exact source/destination bytes, one-step Undo, the corrected pending counter and clean exit. The `59f399a` final build passed an additional isolated-profile comparison/Find smoke. The rebuilt unsigned preview was installed per user with matching hashes/lengths for all five files, correct shortcut/uninstall registration and unchanged observed associations.

Final supply-chain run [35340423870](https://github.com/TheWoovee/BareLine/actions/runs/35340423870) passes dependency/audit/SBOM checks and independent executable/package reproducibility. Strict hash comparisons were preserved. Configured release and signing remain skipped. The original failures, probe, passing replica manifests, every local receipt and separate binary identities are retained in the [full-round report](../../qa/2026-09-18-full-round/README.md).

This completes the bounded local round and authorized preview refresh, not final production acceptance. Counts remain 7 implemented/dispositioned, 41 active work packages and 1 owner-deferred; physical/VM/soak/release-trust/independent-review requirements remain explicit.

Final hosted correctness [35340423863](https://github.com/TheWoovee/BareLine/actions/runs/35340423863) also passed on `59f399a`: all three workspace jobs, Windows first-party release-host qualification and performance smoke. Both final workflows are green; no failed gate from this bounded round remains unresolved.
