# Local audit implementation: closing record

All 31 PR packages (18 technical and 13 UI/UX) have reviewed implementations integrated locally. Existing Sol 5.6 medium agents implemented corrections; root reviewed their exact diffs and returned failures for correction. Independent Sol technical and native reviewers confirmed the closing findings. This records completed implementation separately from the remaining qualification limits below.

The [original PR index](../../qa/2026-09-12-local-audit/PR_INDEX.md), per-PR plans in this directory, [patch ledger](status.json), and [closing review](reviews/R8-known-fixes.md) retain implementation context, dependencies, correction commits and acceptance evidence. No remote repository or hosted PR was required. Original HEAD `51cd4c0d1651fce815c67ed268e42999a9c2bb33` and the original index are unchanged.

## Closing defects addressed

| Finding | Implemented behavior | Verification |
|---|---|---|
| Hidden native save consent | Reveal the supplied TaskDialog at its creation callback, preserving native layout, focus handling, choices and cancellation. Both TaskDialog entry points share the callback. | Independent single-run p0-7 PASS; visible Cancel preserves dirty text and disk bytes; Save writes exact bytes; About is visible. |
| Paged save remained on “Saving…” after disk commit | Consume exact tracked document/generation save terminals while keeping maintenance protected by worker busy state. Preserve a newer status or concurrent resident save banner. | 8 terminal tests, retirement retry, normal save test; native forced-paged exact-byte save clears the status. |
| Restore left a stale recovery entry/notice | Retire the restored row on its typed terminal; fence older scans and suppress the claimed directory only while its restored document remains open. Preserve other checkpoints and failed restores. | 13 recovery controller tests; native exact restore followed by an empty refreshed list and resolved notice. |
| Split/tab/comparison closure lost Undo history | Transfer sole history ownership before retirement and refuse comparison close while pending work can still publish. | 16 view tests, including queued secondary edits and surviving Undo/Redo; native split/tab/compare checks. |
| Stale external-change warning after clean reopen | Resolve only after exact current fingerprint verification; retain warnings while asynchronous reload is pending. | 4 contained tests and native save/close/reopen confirmation. |
| Folder summary mislabelled files searched as files with matches | Display the existing searched-file count with accurate wording. | Native reviewed replacement/rollback retained; corrected wording confirmed. |

Common-controls initialization alone did **not** resolve hidden consent. The failed hypothesis and invalid manifestless library-host test remain recorded; the latter was newly introduced and removed after actual manifest-bearing app startup was verified. Existing tests were not weakened.

## Final gates and exact boundaries

All receipt paths below are relative to `target/qualification/`.

| Gate | Result | Receipt |
|---|---|---|
| Full workspace correctness suite | PASS, 73 crate/doc-test summaries, 234.9 seconds | `final-workspace-tests-r8.json` |
| Integrated workspace build | PASS | `final-workspace-build-r8.json` |
| Affected views after lint-only cleanup | PASS, 16/16 | `final-U08-views-r9.json` |
| Complete Clippy census and exact debt ratchet | PASS; 756 reviewed occurrences, no new or stale debt | `final-clippy-census-r9.json`, `final-ratchet-r9.json` |
| Formatting | PASS | `final-format-r9.json` |
| Final workspace build | PASS | `final-workspace-build-r9.json` |
| Retained-stream integrity | All seven gate receipts have successful exits, unchanged run sources and matching stream hashes | `final-receipt-integrity-r9.json` |

The full suite covered all functional fixes at snapshot `f5cd57666fdeadf7da59ff35c86934f1b78bfd05`, source `b5e8c35e22a10d6a98995c602790a4930ed77ed7deeaab4ec596b05ca2217ab4`. Its lint ratchet found four style warnings in the view controller and three obsolete baseline entries. Root reviewed the subsequent equivalent iterator/conditional cleanup and deletion of exactly those entries. Only the affected view cohort, failed lint gate, formatting and required build were rerun; the full suite was not repeated for that mechanical cleanup.

Final code snapshot: `800beef05b6fcb7850573e1e3ddc439abdc9af47`. Final build source: `93d1eb8d4cec5c900f530f77545941be727a0bf1f6908044eb9d68eef9a107d7`. Artifact manifest: `final-artifacts-r9.json`.

- App: `25199dcd714690ad0f2c3017e46aa36eb4771897f92ba18a81dcd4d751a396aa`.
- xtask: `97e49436ccdd6754e0351a60ab69be5f2d09cfe00776670be227349f33dd7777`.

Native evidence retains its actual executable boundaries: closing consent checks used app `44abd1e3...`; the other R8 fixes used `9eb824f6...`. The final source differs only by the reviewed lint cleanup described above. Later documentation/status edits do not change these recorded build identities. Earlier failed runs remain failures, not overwritten passes.

## Native completion and remaining qualification limits

Manual disposable-document Exit/discard/restart **passed**: one Exit, visible discard consent, process termination, and same-profile restart with neither the discarded marker nor recovery state returning. This verifies settled normal UI behavior, not busy/deferred resumption. Save Copy overwrite **passed** with visible Replace/Cancel consent, exact generated destination bytes and preserved source-tab identity. Evidence is under `final-native-audit/final-consent-manual/`, including `exit-restart-result.json` and `savecopy-overwrite-result.json`.

The [native report](../../../target/qualification/final-native-audit/report.md), [coverage matrix](../../../target/qualification/final-native-audit/coverage-matrix-r8-paused.json), and [independent technical report](../../../target/qualification/final-technical-audit/report.md) contain detailed observations and exact evidence paths. Previously completed unaffected workflows were retained instead of rerun for every correction.

No final 13-journey aggregate pass is claimed:

- The final automated p0-3 attempt stopped **before input or Exit** because UIA focus remained on the top-level window. Its failure is retained as `final-native-audit/journey-p0-3-final-workspace-r8-retry-receipt.json`. A future driver follow-up must establish and verify Editor focus before input, preserve the exact text oracle and single-Exit rule, and distinguish an accessibility-provider mismatch from a product input failure. Native busy-resumption is not established by an idle manual Exit; contained deferred-close tests cover that source invariant.
- The synthetic astral SendInput path still fails before rendering assertions. Normal UI emoji input passes. A follow-up must preserve Unicode scalars through the Windows/winit input route for every focused field, or use a separately qualified normal input route; do not weaken the exact text oracle or substitute a passing BMP fixture.
- Raw split-provider focus, Run cancellation, notification failure/overflow, physical IME/assistive technology/mixed-DPI, supported-OS floor/clean-VM, other hosts, production signing/configuration and controlled comparator performance retain their scoped unexecuted qualifications. See the coverage matrix and [unlock checklist](../../UNLOCK_CHECKLIST.md) for resumption context. Implemented capabilities are not automatically marked tested.

Recovery **Export Edits** intentionally emits owned transaction segments plus an incomplete gaps report. Its zero-byte edit segment in the one-byte checkpoint fixture was not a full-document export failure; Restore reconstructed the complete content.
