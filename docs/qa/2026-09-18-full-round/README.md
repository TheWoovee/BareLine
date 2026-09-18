# Full Windows test round — 2026-09-18

The full local automated round and native feature checks found two product defects. Both were corrected sequentially, with failing-before/passing-after regressions. Product code is committed and pushed as `de1feaf0fe9e6a7a8730e0ee98d1cad5b09b4ab6`.

## Fixes

- **Find mode:** Literal search incorrectly announced the Extended escape-sequence label. Literal now has its own label/tooltip. The three-way Literal/Extended/Regex control exposes an accessible button with its current mode value; Case and Whole Word retain checkbox semantics. Both mode controls and all three modes have regression coverage.
- **Comparison:** recomputation after a merge or Undo displayed `1 / 0` because the old hunk index survived while results were absent. The counter now displays a selected position only when it belongs to the current result. Recalculation, cancellation, invalidation and equal-file results are covered.

The complete accessibility golden initially failed after the intended Find change. Structural review found exactly 44 changed fields: the names and roles of 22 Literal-mode controls. No hierarchy, geometry, focus, value or action field changed. The reviewed baseline was updated before the final workspace rerun. This is a companion test-fixture update, not a third product defect.

## Hosted follow-up corrections

Hosted correctness on `de1feaf` exposed a pre-existing migration test that shared its profile and five-second budget across flat-tree copying and a depth-limit assertion. A busy runner correctly left the latter item Pending. Separate generated profiles now isolate entry/depth limits from wall-clock speed, with precise diagnostic and source-retention assertions; all 11 migration tests pass. Receipt 36 retains an incomplete inherited-pipe capture; the direct pinned-Cargo rerun 36b records the complete pass. Production migration logic is unchanged.

The independent clean-build gate also found 225 differing editor bytes: anonymous C++ RTTI identifiers and derived PE/debug hashes; machine code and resource sections, update helper and extension host were identical. The two replicas used different rolling Windows images. The bridge now normalizes the MSVC source prefix and sorts translation units. A small executable probe differs before normalization and matches byte-for-byte afterward, and all 25 syntax tests pass. The original hosted failure, downloaded inventories and binary comparison are retained. Microsoft also uses [source-path trimming for deterministic C++ builds](https://github.com/microsoft/WindowsAppSDK/blob/main/Directory.Build.props). The follow-up Clippy gate retains the same 755 occurrences.

These follow-ups are committed as `59f399a`. The [final Supply chain workflow](https://github.com/TheWoovee/BareLine/actions/runs/35340423870) passed: dependency policy, RustSec audit, SBOMs, two independent clean builds and the unchanged strict executable/package equality checks. Downloaded replica manifests match, with both the passing job log and original failure retained. Configured-release and signing jobs were skipped and remain pending. The path-normalization probe and passing follow-up support the correction; the different original runner images alone do not prove the cause of the initial path difference.

## Automated verification

All 25 initial checks passed, with unchanged source during each capture. The initial workspace passed in 486.057 seconds. After the two product corrections and their golden update, the complete locked workspace passed again in **107.595 seconds**. All final formatting, portability, toolchain and exact Clippy gates passed. Clippy retains **755 reviewed Windows occurrences**, with no new or stale debt; it is not warning-free.

| Coverage | Result / receipts |
|---|---|
| Complete workspace, including native semantics and process-death checks | PASS: 01, final 30b |
| Diagnostic file-I/O dispatch, real save boundary, recovery probe | PASS: 04–06 |
| E2E/release/performance/soak tooling | PASS: 123 / 14 / 21 / 5 tests; 11–14 |
| Evidence capture and Clippy self-tests | PASS: 15–16 |
| Package, runtime boundary, journey manifest, dependency policy | PASS: 17, 19–21; dependency warnings retained |
| Synthetic visual bitmap oracles | PASS: 10 cases; 18 |
| Actual first-party Fast and Large | PASS: 3/3 and 2/2, including 1 GiB JSON and 5 GiB hex; 22 |
| Hardware/software hidden render and performance smoke | PASS: 23; uncontrolled local cache, not a benchmark qualification |
| Opt-in divergent multi-GiB diff traversal | PASS: 24; 4 GiB total data |
| Complete nonshipping release fixture | PASS: 25; pinned test trust/local transport, tamper checks, actual connected installation and installed-host Fast |
| Focused fix regressions | PASS: 9 Find and 6 comparison tests; 27, 29 |
| Final formatting/portability/toolchain/Clippy | PASS: 31b, 32; follow-up Clippy 38 |
| Hosted-follow-up focused regressions | PASS: 11 migration tests (36b), 25 syntax tests (37), MSVC executable probe |

Receipts 26 and 28 intentionally reproduce the original defects. Receipt 30 records the outdated golden failure; 30a intentionally captures its review candidate. They are preserved alongside passing reruns, not discarded. Nested Rust summaries include child test processes; their totals must not be added as a count of unique tests.

## Final hosted verification

Final hosted [Correctness](https://github.com/TheWoovee/BareLine/actions/runs/35340423863) and [Supply chain](https://github.com/TheWoovee/BareLine/actions/runs/35340423870) both pass on `59f399a`: complete Windows/Linux/macOS workspace jobs, actual Windows first-party release-host checks and performance smoke, dependency/audit/SBOM checks, and independent executable/package reproducibility. Hosted neutral-platform checks do not qualify native Linux/macOS product behavior. Configured-release/signing jobs remain skipped. The earlier hosted failures and final passing job logs/manifests are retained separately.

## Native results

Testing uses generated scratch files and isolated portable profiles. Initial native checks used a byte-identical copy of installed source `1533223`, SHA-256 `dba08ec176ef2f2f89b59c8eb23eb97bb149adc0b65cbd088306e5079bcb0d8d`.

Unicode typing, caret Tab, selection indentation/Shift+Tab, horizontal/vertical split, distinct-pane edits, divider resize, shared Undo/Redo, close split, exact UTF-8 save/reopen, Find/Replace and one-step Undo passed. The saved 39-byte fixture matches exactly. These checks observed stable ordinary editing snapshots; no high-frame-rate blinking measurement was performed.

The requested **two-file comparison** used replacement, deletion and insertion fixtures. Compare with disk file reported exactly three differences. Forward/back navigation, single-hunk right-to-left merge, automatic recomparison, exact saved bytes, one Undo, preservation of the source file and close-comparison cleanup passed. The disk source is intentionally read-only; that path disables left-to-right copying.

Corrected runtime source `de1feaf`, SHA-256 `b2c67a4ee9ed1ecb5ff5a88c4db1b4ff4b7571c18c2a4ef52d03d5f61c969161`, passed all three Find modes on both mode buttons, then comparison between **two writable open files**. Both copy directions changed only the selected version line, exact saved destination/source bytes matched, and one Undo restored each original. The native captures show **0 / 0 during recomputation**, then valid 1 / 2 or 1 / 3 counters. Next/Previous navigation, close-comparison cleanup, pointer close of the generated clone and clean app exit passed. The active `right.txt` occupies the UI left pane, so the retained observations name both pane direction and physical destination file to avoid ambiguity.

The helper sometimes reports stale focused-pane/file-dialog metadata. An indexed blank-tab close selected the tab in its immediate snapshot; keyboard close and direct pointer close succeeded. These observations are retained as helper/qualification limits, not established additional product defects. OS dialog captures containing unrelated personal folder names remain local and are intentionally excluded from shared evidence.

The final build from `59f399a`, SHA-256 `cfb5068192594ecf2e4925f6ebcffad05fd7161f68eafa85cb74001ca43b5ba8`, passed a fresh isolated-profile smoke: restoration, three expected differences, both merge controls available, recomparison, comparison cleanup, correct Literal button names/roles, Find cleanup and clean exit. Its fixtures remain byte-identical to the originals. The complete merge/save/Undo journey above retains the preceding product-equivalent runtime identity; the final smoke does not claim a second full journey or a new capture of the fast pending state.

## Build and installation

The explicit unsigned-preview release build (`--no-default-features`) passed on clean source `59f399a` in 5m25s (40). Packaging passed (41), and per-user installation passed (42), with unchanged source during every capture. All five installed payload hashes and lengths, Start Menu target and uninstall registration match; observed file-association keys stayed unchanged.

- Installer: `dist/windows/0.1.0-full-round-final-20260918/bareline-0.1.0-windows-x64-setup.exe`
- Installer SHA-256: `6d4c49bb164e2aee1a194e06337eb53c6efcd672e371804e8102bc8cb84bbcc6`
- Installed editor: `C:\Users\Woovee\AppData\Local\Programs\Bareline\bareline.exe`
- Editor SHA-256: `cfb5068192594ecf2e4925f6ebcffad05fd7161f68eafa85cb74001ca43b5ba8`

The earlier corrected build/package/install (33-35) remains separately recorded under source `de1feaf`; it was superseded after the build reproducibility follow-up. This is a local unsigned preview, not a signed production release.

## Evidence and limits

Raw receipts, requests, terminal status, logs, source identities, native observations and scripts are retained under [retained](retained). [SHA256.json](retained/SHA256.json) inventories their exact bytes. Large generated fixtures and executable payloads remain local, identified by hashes. Earlier source/binary identities remain distinct from the corrected candidate. Hosted results at the start of the round passed on `b70f5e8`; final-code hosted results are recorded separately.

This local pass does not complete production acceptance. Physical IME/screen-reader/high-contrast/mixed-DPI coverage, disposable Windows 10/11 lifecycle tests, controlled performance comparisons, the full 72-hour workload soak, real shipping keys/endpoints/signing and independent acceptance review remain. Linux/macOS native product qualification is owner-deferred. Readiness counts stay **7 implemented/dispositioned, 41 active work packages, 1 deferred**; the active packages are not 41 known defects.
