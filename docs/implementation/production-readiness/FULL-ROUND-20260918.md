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
