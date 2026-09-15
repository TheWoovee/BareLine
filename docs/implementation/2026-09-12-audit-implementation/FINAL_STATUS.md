# Local audit implementation and qualification status

All 31 PR packages are implemented, reviewed and accepted locally. The confirmed closing app defects passed independent native checks; full workspace tests passed, followed by contained verification of lint-only cleanup, a clean lint ratchet and the final build. See the [final acceptance record](FINAL_ACCEPTANCE.md) for exact commits, artifacts, test boundaries and remaining qualification gaps. Original HEAD and index remain unchanged.

## Historical R8 closing pass (superseded by final acceptance)

All 31 original PR packages are integrated locally. Four additional known corrections are now independently source-reviewed, tested and natively confirmed: paged-save status (T16), restored recovery rows/notices (U05), comparison close/history (U08), and folder-summary wording (U04). Normal UI emoji input also passed. See [R8 review](reviews/R8-known-fixes.md) and the native report under `target/qualification/final-native-audit/`.

The sole confirmed blocker in this finite closing pass is the hidden save-consent TaskDialog (T05). Common-controls initialization did not fix it: the actual app starts with its embedded v6 manifest, but one exact Close still creates an invisible owned dialog with a disabled parent. The manifestless platform-library initialization test fails and is not evidence of actual app startup failure. A scoped native lifecycle visibility correction is in progress. No dialog pass or final aggregate is claimed.

R8 contained results: T16 terminal 8/8, retirement retry 1/1, normal paged save 1/1; U08 views 16/16; U05 recovery 13/13. The combined build passed at snapshot `b39eed91795089e1a4a2ada6e302f6e50d14a9ef`, source `7088692de23fcb54c218fed45b1047efaa30acd8a49469c37622ac169c05c6ce`. App SHA-256 `9eb824f6d691e3f81801143e24add349cd267a5dad5a5ee4a87b8c29ad3300c2`, xtask `cb97d3bd767e811775176b601a214deaecb88c1e8d07d2c456ae6de5c26104ea`; manifest `target/qualification/final-debug-artifacts-r8.json`. Earlier R6 full gates do not cover R7/R8 corrections.

At the owner's resource limit, the broad audit is closed to new exploration. Reuse existing Sol agents, retain prior unaffected passes, and check agent status no more than every ten minutes. Native confirmations are in `evidence/r8-paged-save-terminal-pass.json`, `r8-recovery-refresh-pass.json`, `r8-compare-close-pass.json`, `r8-folder-wording-pass.json`, and `r8-normal-ui-emoji-pass.json` under the native audit directory. Hidden consent failure remains `journey-p0-7-r8-app-retry-receipt.json`. Full final gates will run once the last correction is accepted.

The synthetic astral SendInput journey still fails before rendering assertions; the normal UI emoji pass qualifies a separate input route. Recovery Export Edits intentionally reports unavailable original ranges and owned transaction segments. Unexecuted physical/platform, production-signing, and other-host checks remain unverified.
## Historical R6 integrated build and gates

Frozen build snapshot: `d7e5b5354f63b388eb89bbf96aaa43a537917119`. T09 source: `78c332699e5cc62cd7a146c0f3f64637daae2312cae4e2b1331324cb615e1ea8`. Later documentation does not relabel this build source. Exact artifacts and old-process distinction are recorded in `target/qualification/final-debug-artifacts-r6.json`.

| Final correction gate | Result | Receipt under `target/qualification/` |
|---|---|---|
| Full workspace tests | PASS; 73 crate/doc-test summaries, including semantic golden; 116.5 seconds | `final-workspace-tests-r6.json` |
| Full workspace build with QA inventory | PASS; current Close/Exit and journey corrections included | `final-workspace-build-r6b.json` |
| Complete Clippy census and exact debt ratchet | PASS; 401 reviewed IDs / 762 occurrences; no new or stale debt | `final-clippy-census-r6.json`, `final-ratchet-r6b.json` |
| Formatting | PASS on the final Rust correction source | `final-format-r6.json` |

The census preceded only documentation/status updates and removal of one resolved warning entry. Sol removed exactly the obsolete old busy-branch warning, root reviewed the deletion, and the same complete census passed the ratchet. The original stale-entry failure remains `final-ratchet-r6.json`; no new warnings were added to the baseline.

The first build attempt compiled but Windows refused to replace the still-running old executable. Root preserved its verified bytes as `target/debug/bareline-pre-r5-running-d3aad223.exe` and left process 12040 running without UI input. The incremental retry passed at the same source. Both `final-workspace-build-r6.json` and `final-running-artifact-retention-r6.json` retain this history. The open old process does not contain the fixes.

R6 app SHA-256: `ee4e16b0f9b83a1b70a70f3a6807c6987c24ac93f2084c6a656bb10d4dca49c1`. The later R6d xtask is `0d4bd0fa1a6b764a3e1702babc334c3b5851d0346938b1fdbd8ed95c9790e9ad`, built at snapshot `0751cfa082217ecc16d4d11bb18f43a066593398` / source `05185b4711e1ef54f5e7540c4e81287497cef3c66fe6e64e3d77adb4f57c12df`. Earlier artifact manifests remain historical. Neither executable contains the pending R7 batch.

## Completed verification

The first complete Rust/app gate checkpoint is private snapshot `56e5934f81764ec2a2bf11c9cc06938144106e32`, original HEAD `51cd4c0d1651fce815c67ed268e42999a9c2bb33`, T09 dirty-source SHA-256 `48aad6978f8398505168187633857f782e120bfb9b6ed2aca8293fefb46a9fe8`. The table below records those checks and explicitly named later reporter results. It does not claim that this old build contains the later Close/Exit fix. Earlier failed runs remain retained; they were not overwritten or counted as passes.

| Check | Actual result | Retained receipt relative to repository root |
|---|---|---|
| Full workspace tests | PASS, including the complete semantic golden | `target/qualification/final-workspace-tests-r2.json` |
| Full workspace build, including QA inventory | PASS | `target/qualification/final-workspace-build-r1.json` |
| Formatting | PASS | `docs/implementation/2026-09-12-audit-implementation/evidence/final-format-central-r3.json` |
| Full Clippy census and exact ratchet | PASS; 402 reviewed distinct debt entries / 764 raw occurrences | `target/qualification/final-clippy-census-r2.json`, `final-ratchet-r2.json` |
| Portability and 31-member toolchain contracts | PASS for the local host and static architecture checks | `target/qualification/final-portability-r2.json`, `final-toolchain-static-r1.json`, `final-cargo-metadata-r1.json`, `final-toolchain-resolved-r1.json` |
| Dependency policy | PASS using cached offline advisory data; retained duplicate and unused-license warnings | `target/qualification/final-dependency-policy-r1.json` |
| Evidence/AC adapters and perf contracts | PASS: 13 adapter/resolver and 21 perf tests | `target/qualification/final-python-contracts-r1.json`, `final-perf-contracts-r1.json` |
| Connected nonshipping release delivery | Actual signed-fixture catalog/runtime/three-component installation and tamper checks PASS; installed Fast suite 3/3 PASS | `target/qualification/t11-connected-fixture-r2.json` retains the final reporter failure; the reporter correction is explained below |
| Installed Large suite | PASS 2/2 using the same retained installed artifacts, with before/after hashes unchanged | `target/qualification/final-first-party-large-r1.json`, `final-first-party-large-qualification.json` |
| Release reporter correction | PASS against the original retained source, prepared config, T10 execution and installed/package/inventory bytes | `target/qualification/final-release-reporter-r3.json`, `fixture-delivery-report-r3.json`, `fixture-delivery-reporter-provenance-r3.json` |
| Isolated runtime inventory | PASS: schema 2, 499 unique commands, exact binary identity | `target/qualification/final-native-audit/runtime-inventory-isolated-receipt.json` |
| Performance smoke | PASS: one hidden first-frame sample per requested renderer; debug, uncontrolled cache | `target/qualification/final-native-audit/perf-smoke-receipt.json` |

The first-party JSON case uses a real generated 1 GiB file. The 5 GiB Hex case uses a bounded synthetic provider (576 bytes read), so it does not establish native 5 GiB disk throughput. The perf smoke does not establish comparative performance, repeatability, release startup budgets or hardware qualification.

## Final corrections and review

The first integrated gates found nine failures. Root review sent them back to the responsible Sol implementors. Corrections included the actual resident-spill/transcode producer-root routing defect, a lossless cross-platform migration path codec, and stale modal, notification, indexing and prepared-save test fixtures. The original reproductions and focused corrections passed before the final full workspace suite. Detailed review and original failures remain in [REVIEW_AND_FINAL_VERIFICATION.md](REVIEW_AND_FINAL_VERIFICATION.md).

The final release workflow completed real installation and Fast execution but its reporting step rejected valid Windows extended paths (`\\?\D:\...`). Reviewed correction `56e5934..a47369a` canonicalizes drive/UNC spellings for containment comparisons and rejects unsupported device namespaces, outside-root/missing/nonregular files and reparse traversal. It preserves exact source, execution, manifest and artifact binding. Root reran the corrected reporter against the retained artifacts and kept both the failed workflow receipt and the successful reporter-only receipt. No release rebuild or runtime re-execution was needed. The correction is integrated; `target/qualification/final-release-contracts-r2.json` records its contained checks in the original checkout.

Later T09 R4 corrected Rust/Python source-manifest framing and passed three focused tests, an xtask build, a real native smoke and an actual adapter import. The adapter output contains one status-only observation and `evidence: []`; it claims no acceptance criteria. Receipts are `final-T09-evidence-r4.json`, `final-xtask-build-r4.json`, `final-T09-actual-adapter-r4.json` and `final-native-audit/journey-smoke-parity-r4-receipt.json` under `target/qualification`. The matching smoke source is `196a45aeeb14c176c9911f372c136d8480f05db7503835a892d23221b4eb8d69`; historical mismatched framing remains documented, not rewritten.

T05 R5 retains one Close/Exit intent while a worker is busy and resumes it from the completion wake. Root returned the first proposal because its synchronous busy MessageBox could block resumption. The accepted correction keeps the precise application phase or document owner/revision, preserves Cancel/save-failure/discard safety, and does not generate repeated wakeups. The corrected binary-target test cohort passed 8/8 in `final-T05-deferred-close-r5c.json`; independent Sol review found no additional safety/resumption gap. See [T05 review](reviews/T05-R5.md). Native one-command resumption still requires the current app build and a permitted retry.

T09 R5 corrects actual editor-provider discovery, selection-aware Unicode input observation, bounded partial-delivery waiting, retained prerequisite diagnostics, single Close/Exit injection and durable-retirement/restart checks. Root returned the initial proposal for those corrections, accepted `55bc4c9f99c51e3104771c8e38ae3e3fdcfb8c29`, and integrated it at private snapshot `f1b84b9b841cde150761e3decf879861395280c2`. Four focused helper tests and formatting passed with unchanged source `1ee07f9dd8d95da32bbc1bfd1859ea982425a95cb9d0387fa8758465841b0322`. Independent Sol review found no concrete false-pass, safety or resource defect. See [T09 review](reviews/T09-R5.md). These headless results do not qualify the affected native journeys.

## Native audit history and authorized continuation

The initial locked-desktop aggregate remains 5/13 with environment-confounded failures. After the owner explicitly unlocked the PC, a separately recorded aggregate passed **10/13**, with failures in p0-3, p0-7 and p4-4. The old application used for those observations has SHA-256 `d3aad223a63ad0b3caca07e156471870a06f10c5a8d4bc562c385f7c494ba2bd` and predates T05 R5.

The p0-3 failure checked cleanup without observing a consent prompt; the app was still open with recovery preparing. Source inspection confirmed the lost busy-close intent. The legacy oracle also assumed immediate physical deletion, while T05 allows exit after durable retirement suppresses recovery and garbage collection may remain pending. A corrected journey must prove actual consent, production recovery suppression and no resurrection on restart. The p0-7 observation inspected the top-level window instead of an editor TextPattern, and p4-4 never delivered the intended text, so that attempt could not qualify emoji rendering. These are retained failures, not retroactive passes.

Partial manual checks passed for modal isolation/focus return, Find/Replace All and one undo, Settings autosave/Revert, Recovery Center empty state, macro record/play, Output dock and Unicode/emoji. These do not complete the full U01–U13 matrix. The auditor also observed a Pane 2 active indicator with a Pane 1 focus report. Independent source review found no confirmed controller defect; finer pane-body/provider/caret capture was interrupted by physical Escape. [U08 diagnosis](../../../target/qualification/final-native-audit/U08-focus-diagnosis.md) records the required same-interval follow-up. No speculative correction or acceptance is claimed.

Computer Use explicitly reported the owner's physical stop, and all native control ceased. The owner has since explicitly resumed testing again. Remaining manual workflows include resident/paged save/reopen/overwrite/copy/all/cancel, compare/merge/undo, harmless Run/output/cancel, folder reviewed replacement/rollback, recovery restore/export/discard/session, partial-index gutter, notification failure/overflow and detailed menu/split-provider checks. The Sol auditor will record new evidence on the current build; any further stop or lock ends input again.

The [native report](../../../target/qualification/final-native-audit/report.md) preserves exact observations and the [technical report](../../../target/qualification/final-technical-audit/report.md) preserves independent review and correction addenda. Raw aggregates are `target/journey/results/19068-1789224444381892200.json` (locked) and `14532-1789228400489603200.json` (post-unlock). [UNLOCK_CHECKLIST.md](../../UNLOCK_CHECKLIST.md) has the actionable resume order. Use generated files and isolated profiles, diagnose before retrying, and qualify only fully executed steps.

Production signing/configuration/endpoints, clean-VM and supported-platform-floor checks, other actual OS hosts, physical IME/assistive technology/mixed-DPI checks, and controlled comparator performance remain explicit external qualifications. Runtime command inventory alone does not close individual acceptance criteria or promote every capability to tested.
