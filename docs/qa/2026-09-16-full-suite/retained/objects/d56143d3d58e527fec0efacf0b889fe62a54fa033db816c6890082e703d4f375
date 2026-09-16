# Full Windows validation and refreshed installation — 2026-09-16

## Local outcome

The authorized full automated Windows suite passed, with failures investigated and corrected sequentially. Two recovery defects were fixed: delayed retirement acknowledgment could clear a newer checkpoint's status after directory reuse, and the global cleanup-warning queue could deliver another workspace's failure. The former has a retained failing-before/passing-after regression; the latter has a two-owner isolation regression and the unchanged workspace terminal-banner assertion.

The refreshed unsigned preview installer is installed locally. All five installed payload files match their package hashes. Four sequential installed-app journeys passed **12/12 native steps**, including clean exits: column editing, Unicode plain text, code/config highlighting and completion, and regex replacement/Undo. The owned-window code-highlighting screenshot was inspected. No personal document/profile was used; optional Explorer/association tasks remained deselected.

## Executed checks

| Scope | Result / evidence |
|---|---|
| Complete Rust workspace | Passed in receipts 03, 32 and the final consolidated **63** on `6badda3` (**154.953 seconds**). Receipt 63 includes all recovery and test corrections; source remained unchanged during execution. |
| Final warning-routing change | Receipt 37: all 150 app tests, 96 file-I/O unit tests, and process-death/doc checks passed. The two-owner regression preserves actual cleanup failure delivery. |
| Final test corrections | All **150 app tests** passed after queued-input fixture corrections (59); all **72 editor-surface tests** passed after linked-peer timing correction (62). The complete workspace then passed (63), followed by the final Clippy census and exact ratchet (64–65). |
| Diagnostic features | Neutral callback dispatch, real QA save-boundary arm/marker/timeout/single-use behavior, and read-only recovery probe checks passed (04–06). |
| Tooling | 123 E2E-tooling, 14 release-tooling, 21 performance-tooling and 5 soak-tooling tests passed (11–14). These are tooling tests, not native/72-hour qualification. |
| Quality gates | Formatting, portability, toolchain, capture/Clippy self-tests, package checks, ten synthetic visual checks, runtime boundary and journey manifest passed. Windows Clippy retains exactly **755 reviewed occurrences**, with no new or stale debt. |
| Dependencies / package metadata | Local cargo-deny policy passed with recorded warnings. Locked Windows fetch, offline notices, real CycloneDX generation/normalization and dependency-free offline version lookup passed. Hosted evidence is identified separately below. |
| Actual extension host | All five Fast/Large tests passed twice (22, 36): Fast **3/3**, Large **2/2**, including **1 GiB JSON** and **5 GiB hex** fixtures. Executable/component hashes and nested run manifests are retained. |
| Opt-in multi-GiB diff | Receipt 66 passed the explicitly ignored full-traversal test: **4 GiB** total generated page input (two 2 GiB sources), with 256 KiB source and 512 KiB window budgets. This is a bounded algorithm check, not a controlled disk-throughput comparison. |
| Nonshipping release integration | Receipt 25 passed the complete fixture trust/catalog/update pipeline: signature and tamper rejection, fixture-configured binaries, connected component installation, installed-artifact Fast tests and capability/resource manifests. This uses test trust and local transport. |
| Current preview binaries | Receipt 41 passed the optimized editor/helper/extension-host build with `--locked --no-default-features`, from clean `6585c4a`. |
| Render / installed smoke | Receipt 35 passed hardware/software hidden render smoke. Receipts 42–47 built the refreshed installer, installed it and passed the four native journeys against the installed path. |

## Other corrections found during the run

- Moved Windows-specific QA save-fault logic and the recovery inspection example from neutral file-I/O into the executable composition root; kept a fail-closed callback and all reparse checks.
- Guarded the complete Windows adapter crate on Windows targets, so unguarded child modules cannot enter Linux/macOS builds.
- Forced the locked PCRE2 bundled implementation; Linux's older system library lacked the compiled-pattern limit API used by search.
- Fixed clean-run dependency preparation before offline notices, explicit `bom.json` output naming, and workspace-only metadata for the package version.
- Avoided a misleading missing-artifact error when first-party qualification had never run.
- Qualified Linux/macOS Clippy from their actual native compiler output. Each has **223 distinct identities / 420 occurrences**; all match previously reviewed Windows source/item/lint/diagnostic/context. Existing per-occurrence justifications were preserved with native counts and census hashes.
- Corrected test timing assumptions without changing product admission or assertions: bounded retries of rejected grouped/metadata submissions, pumping queued fixture input before expecting completion notifications, and inspecting a linked peer before consuming its source completion.
- Reused bounded temporary-directory cleanup retries after asynchronous workspace teardown. Unexpected errors and deadline expiry still fail; product cleanup is unchanged.

## Failure history and source identity

The [execution summary](execution-summary.json) identifies every local receipt and original source state. Receipt 02 was refused by `--locked` before tests ran; the lockfile then recorded the new optional direct serde edge without changing dependency versions. Receipt 27 deliberately reproduces the slot-retirement defect before its correction. Both failures remain retained.

Receipt 29 completed SBOM generation but reports source-manifest drift because the generator created 31 untracked `bom.json` outputs. Those generated files were moved into the evidence directory afterward. This is recorded as artifact generation, not a stable-source test pass. The later packaging run uses ignored `.cdx.json` outputs and retained stable source throughout.

Receipt 50's compiler succeeded, but command capture reported `inherited_pipe_open`. It remains a capture failure; the exact retry (52) completed normally and its ratchet (51) passed. Across all 66 local receipts these four exceptions (02, 27, 29, 50) remain explicit; they were not relabeled as passes.

The workspace command reports nine explicitly ignored entries: five release-component tests executed separately by the actual first-party runner, the multi-GiB diff test executed in receipt 66, and three controlled subprocess fixtures invoked by the passing containment/guard-transfer parent tests. They are not nine unexecuted feature tests.

No receipt was relabeled after a commit. The final consolidated workspace and Clippy runs use code at `6badda3bba8038cec1f7c470555bae4c811af3ca`, with only report files uncommitted and stable source throughout. Earlier checks retain their original revisions. The installer binaries were built from `6585c4a`; package/installation/native smoke ran with `dc1bf23`. Changes after the binary build are native baseline qualification, test-only corrections and documentation, so no further installer rebuild was needed. Full-suite success does not imply every check ran against one source identity.

## Hosted CI

Both final workflows passed on **`6badda3bba8038cec1f7c470555bae4c811af3ca`**:

- [Correctness — 35088391989](https://github.com/TheWoovee/BareLine/actions/runs/35088391989): Windows, Linux and macOS jobs all passed. Windows includes the complete workspace, actual release-host qualification, editor build and performance smoke. These hosted neutral-platform checks do not constitute Linux/macOS native product qualification.
- [Supply chain — 35088392024](https://github.com/TheWoovee/BareLine/actions/runs/35088392024): dependency policy, RustSec audit, SBOM generation, two independent Windows builds and reproducibility comparison passed. Executable and portable-package hashes match between replicas and also match the earlier product-source run at `6585c4a`.

The [hosted status history](hosted-ci.json) retains the exact jobs, revisions, timestamps and run links. Raw final job logs, native Clippy censuses, dependency/SBOM evidence, runtime qualification artifacts and replica hash manifests are in the retained bundle. Earlier failed runs remain part of the history; configured-release and signing jobs were intentionally skipped and remain unqualified.

## Refreshed installer

- Installer: `dist/windows/0.1.0-full-suite-20260916/bareline-0.1.0-windows-x64-setup.exe` (**5,151,386 bytes**).
- Installer SHA-256: `73af9647ab0e967b09db901173bb846705ed2e863d762cb76b5e955b8d813311`.
- Installed editor: `C:\Users\Woovee\AppData\Local\Programs\Bareline\bareline.exe`.
- Editor SHA-256: `64bb428b5bc61dd2f6042a15ba03314af0e4f21290b0d210091ee9bc4d80adb4`.
- Native cell: Windows x64 **26200.9457**, keyboard, dark theme, software renderer, **100% / 96 DPI**; Ryzen 5 3600, 128 GiB RAM.

The earlier installer/evidence remains available in the [prior installed smoke report](../2026-09-16-windows-install-smoke/README.md). The current package is an unsigned local preview.

## Remaining release qualification

Signed release identity/assets and real endpoints, disposable Windows 10/11 install/update/recovery tests, physical IME/assistive-technology/high-contrast/mixed-DPI coverage, controlled performance comparisons, 72 uninterrupted hours of complete soak workloads, complete acceptance mappings and independent review remain open. Linux/macOS native product qualification remains owner-deferred. No synthetic, headless, short-run or self-reviewed observation was promoted to final acceptance.

Top-level readiness remains **49 = 7 implemented/dispositioned + 41 active + 1 deferred**. These are release/qualification work packages, not 41 known product defects.

## Retention

The [collection plan](collection-plan.json) retains execution receipts, raw logs, native results/fixtures/screenshots, actual-host manifests, release-fixture reports and CI evidence. The [release-fixture inventory](release-fixture-inventory.json) records which binary assets remain local with their exact hashes; generated distribution artifacts and large fixtures remain in local `dist` and `target` directories. The [collection receipt](retention-receipt.json) pins the immutable manifest; the [verification result](retention-verification.json) checks retained byte integrity, not release approval, full binary replay or independent acceptance.
