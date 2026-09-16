# Implementation status

**Updated 2026-09-16. Windows development preview; production acceptance is still incomplete.**

Windows x64 is the current target. The owner deferred real Linux/macOS qualification to the next update and delegated routine product choices. Work was performed sequentially, with focused checks followed by consolidated workspace runs and release builds.

## Current counts

| Inventory | Current state |
|---|---|
| 49 top-level readiness items | **7 implemented/dispositioned, 41 active, 1 owner-deferred** |
| Development packages still open | **3: DEV-003, DEV-004, DEV-006**, with their remaining scope below |
| Capability families | **41 implemented awaiting qualification, 7 deferred, 4 intentional limits** |
| Fully qualified capability families | **0**; no synthetic tests were imported as full acceptance |
| Native product procedures | **13 paths authored; the 3 new lab paths have headless checks only.** 4 ordinary journeys passed the installed-app smoke subset on 2026-09-16. |
| Acceptance definitions | **81 AC, 199 minimum scenarios**; reviewed complete mappings remain incomplete |
| Command work definitions | **499 concrete proposed command plans / 1,497 outcomes**; regenerate for the final configured candidate |

The 41 active items are work packages, not 41 known product defects. [BACKLOG.json](implementation/production-readiness/BACKLOG.json) is the machine-readable queue; [BACKLOG.md](implementation/production-readiness/BACKLOG.md) describes every completion criterion. Counts must not be added into a readiness percentage.

## Delivered in this Windows pass

- Offline recovery-root policy now reaches editor, helper, catalog, runtime restore and queued invocation. Signed authority/transition files are verified and carried through packaging.
- Configured release tooling builds and compares independent payloads, retains a signing handoff, checks signed PE changes, generates metadata from actual signed bytes, and verifies assembly. The protected signing workflow consumes only configured handoffs.
- Evidence tooling supports native, Rust/Python, real-host, packaging, performance and manual producers; required environment cells; retained producer replay; and separate prerequisite/final closure. Missing, failed or self-reviewed evidence cannot become acceptance.
- Seven native procedures and their fixture oracles were added: columns, UDL, split/clone/sync, workspace, portable, huge-log tail and macro/Run.
- UDL definitions now persist atomically and reload with bounded storage and association rules. Column measurement now accepts DirectWrite fallback-font geometry while preserving tabs and rejecting wrapped rows.
- A bounded core soak executor records process identity, progress, handles, threads, memory, disk and worker queues. It explicitly refuses to certify a core-only or short run as the full release soak.
- Known limits and Windows scope are dispositioned in [ADR-47 and the limits register](implementation/production-readiness/KNOWN-LIMITS-20260916.md).

## Menu, recovery and Tab follow-up

Menu repaint caching, delayed routine recovery notices and caret Tab handling are implemented. Focused Win32 menu, recovery, effective-keymap and split-view tests passed, including resident/paged input and shared undo. The corrected debug candidate passed native Tab insertion, selected indentation/Shift+Tab, both-pane editing, synchronization and divider resizing. Final optimized packaging/installation is in progress. [Follow-up report](qa/2026-09-16-menu-tab-split/README.md). Readiness counts remain unchanged.

## Verification

The [full-suite and refreshed installation report](qa/2026-09-16-full-suite/README.md) records the complete Rust workspace runs, final app/file-I/O regression coverage, 123 E2E-tooling tests, 14 release-tooling tests, 21 performance-tooling tests and 5 soak-tooling tests. Formatting, portability, toolchain, packaging, runtime boundaries and the exact Clippy debt ratchet passed. Both actual first-party Fast **3/3** and Large **2/2** passed, including 1 GiB JSON and 5 GiB hex fixtures. The complete nonshipping release-fixture integration and hardware/software hidden render smoke passed. Native Clippy qualification retains 755 reviewed Windows occurrences and 420 each on Linux/macOS; this is reviewed debt, not warning-free Clippy.

The run fixed two recovery defects (checkpoint-slot reuse before cleanup acknowledgment, and warnings delivered to the wrong workspace) plus portability and clean-CI packaging issues. Earlier failures and every source identity are retained. The final warning-routing change passed all 150 app tests and 96 file-I/O unit tests plus process-death/doc checks. These are implementation checks, not final product acceptance.

After correcting asynchronous test setup and teardown assumptions, the complete Windows workspace passed again on `6badda3` in **154.953 seconds**, followed by the exact Clippy ratchet. The final local run retained unchanged source throughout; only report files were uncommitted.

Final hosted [Correctness](https://github.com/TheWoovee/BareLine/actions/runs/35088391989) and [Supply chain](https://github.com/TheWoovee/BareLine/actions/runs/35088392024) workflows passed on the same code revision: all three host jobs, actual Windows release-host checks and performance smoke, dependency/audit/SBOM checks, independent clean builds and executable/package reproducibility. Skipped configured-release/signing jobs remain pending.

The refreshed unsigned Windows preview installer was built, installed per user, and all five payload hashes plus Start Menu/uninstall registration verified. Column editing, plain text, code/config and regex replacement passed **12/12 native steps**, including clean exits, against the refreshed installed executable in the dark/software/100% DPI cell. [Resume checklist](UNLOCK_CHECKLIST.md).

Current installer: `dist/windows/0.1.0-full-suite-20260916/bareline-0.1.0-windows-x64-setup.exe`. Installed application: `C:\Users\Woovee\AppData\Local\Programs\Bareline\bareline.exe`. This is an unsigned local preview; signed release, clean-VM lifecycle and full qualification remain pending. The full automated suite and local smoke do not change the 49-item readiness counts.

## What remains

| Work | Why it remains / next action |
|---|---|
| DEV-003 | All 13 driver paths are authored. Verify the six remaining ordinary procedures and execute the three new lab paths against explicit real VM/signed inputs; no new native pass is claimed. |
| DEV-004 | All 499 historical commands now have proposed source-backed outcomes; all 81 ACs have concrete recipes and 199 minimum scenarios have fixture variants. Execute, review complete mappings/inapplicability and regenerate for the final composed inventory. |
| DEV-006 / OPS-003 | Extension and post-soak recovery integration is implemented. Run the native shakedown and 72 actual uninterrupted hours with complete workloads, then obtain independent assessment. |
| Release trust | Real public release/catalog/offline-root keys, publisher certificate identity, hosting paths and signing access are required. Keep private keys outside the repository. |
| Windows qualification | Disposable standard-user Windows 10/11 environments, destructive install/update/recovery tests, physical IME/AT/high-contrast/mixed-DPI coverage, remaining fault/security checks and controlled performance comparisons. |
| Final review/documents | Independent review, clean-author SDK walkthrough and candidate-specific SBOM, release/migration/known-issue notes and final signed inventory. |
| Linux/macOS | Owner-deferred to the next update; mocks do not establish real-host qualification. |

### Work that needs no owner input

The identified local implementation gaps in DEV-003/004/006 have been addressed sequentially: complete historical command/acceptance procedure authoring, the three lab driver paths, a diagnostic-only real save-boundary signal and recovery probe, and extension-inclusive/post-soak recovery integration. Soak identity now compares the heartbeat to the kernel Job creation timestamp. See [the local completion report](qa/2026-09-16-local-completion/README.md).

No further product decision is needed for these changes. Their remaining steps need actual available native/VM environments, pinned signed inputs, elapsed soak time or independent observations/review. Those runs may reveal further fixes; source and headless checks do not establish native correctness. All three development packages therefore remain open under their original execution/review completion criteria.

The earlier Base64/URL core oracles remain scoped evidence: eight success and fourteen negative vectors, four Rust checks. [Historical focused report](qa/2026-09-16-utility-oracles/README.md). Top-level counts remain **7 completed/dispositioned, 41 active, 1 deferred**; the 41 are qualification/release work packages, not 41 known defects.

Changes are committed and pushed to the existing remote primary branch, `origin/master` (this repository has no `main` branch). Signing, public release and production approval remain pending. Earlier checkpoints are preserved in [the prior status record](implementation/production-readiness/STATUS-BEFORE-WINDOWS-COMPLETION.md) and [the September 15 historical tracker](IMPLEMENTATION_STATUS_HISTORY_20260915.md).
