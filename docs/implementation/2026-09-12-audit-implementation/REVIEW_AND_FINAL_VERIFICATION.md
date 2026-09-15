# Review and final verification contract

## Root review of each PR

Record the worker's base and commit, exact changed files and necessary contained evidence. Inspect the actual patch and affected caller contracts. Verify that the original failure is addressed, cancellation/error/stale-result paths terminate correctly, document bytes/history/identity and recovery ownership remain valid, and unrelated working-tree changes are preserved. Review test substance, not just a pass count. Return concrete file/symbol findings to the worker; record the correction and re-review outcome.

Accept implementation separately from final qualification. Apply only the reviewed worker delta to the original integration working tree after `git apply --check`. Keep the original index untouched. A patch that depends on an unintegrated interface waits; a conflict goes through an explicit merge/review. Do not silently drop a hunk or copy a whole stale file over integrated work.

## Final integrated gates (after the fixes are addressed)

1. Freeze a final source manifest and retain exact binary/configuration hashes. Refresh the Graft graph once after major integrated changes.
2. Run one full integrated workspace build and full correctness suite. Retain failures and any necessary rerun as separate evidence. Run formatting, the completed lint ratchet, local dependency policy and the existing harness contracts without duplicating equivalent gates.
3. Build actual current first-party WASI components and run their supported release-host integration cohort, including the existing bounded large-workload cases. Preserve the distinction between synthetic sources and native disk-file performance.
4. Have fresh GPT-5.6 Sol medium agents perform independent technical and native regression audits. They read the original findings and original reproduction evidence plus final PR receipts, not just implementor summaries. The native agent alone owns desktop control and uses generated files/private profiles. The technical agent rechecks data-safety/task/launch/recovery failure cases and the explicit acceptance gaps.
5. Repeat the original 13 native journeys and ordinary resident/paged save/reopen, overwrite/cancel/copy/all, modal/UIA input, search/replacement/result switching, recovery/discard, session, macro/process output, settings/revert, comparison/undo and relevant layout checks. Retain screenshots, UIA trees, file bytes and exact failed/blocked steps. Do not label a UIA-only result as physical Narrator/IME verification.
6. Send confirmed regressions back to the implementing Sol agent. Run contained corrections first; broaden only when their integration risk justifies it. Close a finding only after review and actual appropriate verification.

Release signing, clean-VM/platform-floor/physical hardware checks and any controlled comparator case unavailable on this host stay explicitly qualified. The code, local fixture configuration and evidence tooling still must be completed; missing owner credentials never justify weakening trust or claiming a released feature.

## Final integrated gate corrections — first review

At snapshot bc5baab471751681f88486a1a6e97a08c031cf02, the full shell suite compiled but failed 4 of 152 cases. The remaining workspace run used --exclude bareline --no-fail-fast to continue coverage without repeating that shell target; it exposed 5 further failures. Both raw runs are retained in target/qualification/final-workspace-tests-r1.json and final-workspace-other-tests-r1.json. No full-suite pass is claimed.

Accepted U10 correction bc5baab..5d9c73f5: actual peer failure is already retained by the U02 typed notification store. The fixture now proves request/outcome drainage and checks persistent Error/Outcome details including the original injected failure and retained recovery authority. It no longer depends on the superseded workspace.message route.

Accepted U08 correction bc5baab..d87bfae3: the modal fixture now checks both real 64-bit split-pane providers are inert, modal focus remains on the options control, and dismissal restores the selected editor provider and re-enables both panes. This retains and strengthens the background-isolation invariant after the flat provider ID was replaced. The complete semantic golden already passed in the full shell run.

The final portability check failed on a Windows path conversion in shared profile migration. Promotion also exposed an actual resident-spill/transcode registered-root mismatch. These and the remaining save/indexing failures are under Sol correction; focused checks and the next full gate will use the corrected source. Earlier failures are not overwritten.

Accepted T01/T02 final correction 642e6dae..0dcf899e: prepare_resident now resolves its nested Transcode producer to the unique registered transcode sibling instead of using the resident-spill root. The existing registered root/kind restrictions and ownership proofs remain intact; unknown and ambiguous routing is rejected, while custom unregistered cache handling retains its prior semantics. Both original promotion regressions exercise this production path. Migration fingerprints now use SerializedPath identity_bytes, verified to return the same raw UTF-16LE/Unix filename bytes without labels or encoding prefixes; errors propagate instead of lossy fallback. The new producer-root policy test passed at f4132f5b.

Accepted U13 fixture correction bc5baab..642e6dae: the existing read-only incomplete-prefix test now checks the current estimated/indexing status and approximate gutter marker, then checks those disappear after completion with the same document and exact total bytes. The previous indexing ellipsis literal was stale.

Accepted T04-related save fixture correction 01ccd8b6..5a626fe5: the captured source is BOM + ba + CRLF after undoing the later c edit, so both retained proposed copies are asserted against those exact bytes. Search and paged-save doubles now implement the prepared-commit interface, restoring the intended commit/receipt failure, post-commit cancellation, and blocked-save/peer-edit timing points. Assertions retain original/proposed bytes and recovery/reconciliation behavior. Production replacement logic is unchanged.

The U10 test namespace compile failure at b762aab is retained as final-U08-modal-central.log; 5d9c73f5..01ccd8b6 qualifies the actual NotificationKind path. Focused reruns follow integration.

Two agents briefly shared the new promotion worktree. Both staged only their own exact files. Actual verified history is bc5baab -> 642e6dae (U13 only) -> 0dcf899e (T01/T02 only). Root applied these as separate ordered deltas and preserved the original integration index.

The first promotion correction was incomplete: the root exact comparison reproduction still failed at 127e99ed. Independent root/technical review confirmed SpillOwnedResident also invokes prepare_original_baseline and the original-file paged-open path. Accepted 0dcf899e..46f9f791 centralizes registered Transcode routing in DiskTranscoder::new, which continuation delegates through, and removes the earlier caller-specific decision. All raw/text/map paths and Directory cleanup use the resolved cache; quotas and custom paths retain prior behavior. A real producer-chain fixture exercises resident and original-baseline conversion. The failed attempt remains retained; focused verification follows. The worker's reported short hash was mistyped; root verified full commit 46f9f791a55cef05950bbdb59eaa4598b8b21c67 and its exact parent before integration.

At snapshot 77075af768f3efdc6ca99879d558eb8bddc81d7e, both original promotion regressions now PASS: final-T01-compare-promotion-central-r2 and final-T01-views-promotion-central. The new resident/original-baseline producer-chain regression also passes. All nine previously failing integrated scenarios now have passing focused corrections, plus the 10 migration and new root-policy/producer-chain cases. Windows portability and static/resolved 31-member toolchain contracts pass. The local offline dependency policy passes advisories/bans/licenses/sources with cached data and retained duplicate/unused-license warnings. The next full workspace run, final Clippy reconciliation, build and native/installed-runtime qualification remain required.

## Current final-gate result — 2026-09-12

The full workspace tests and build now PASS at the final Rust/app checkpoint 56e5934. The exact full Clippy ratchet, portability/toolchain checks, offline dependency policy and Python contracts pass. The connected nonshipping release installation and installed Fast 3/3 and Large 2/2 pass. Root reviewed and integrated the Python-only extended-path reporter correction a47369a, with successful retained-artifact verification and a contained original-checkout contract rerun; no release rebuild was repeated.

The owner subsequently unlocked the desktop. The fresh Sol audit's separate post-unlock aggregate passed 10/13 and completed partial manual coverage, then physical Escape stopped Computer Use. The original locked 5/13 aggregate and all later failures remain retained. The resumed audit exposed a lost Close/Exit intent during worker I/O, corrected through Sol implementation and root review with 8/8 focused headless tests and independent acceptance. T09 source-framing parity passed an actual smoke/adapter import; further journey prerequisite/consent corrections passed 4/4 helper tests and independent review. A split-pane UIA focus discrepancy remains unresolved because same-interval diagnostics were interrupted; no source-confirmed fix is claimed.

The later T05/T09 changes require an identified final grouped build and correctness gate; the earlier full pass at 56e5934 does not qualify new source. Full receipts, exact source/binary identity boundaries, external qualification limits and actionable resume steps are in [FINAL_STATUS.md](FINAL_STATUS.md) and [UNLOCK_CHECKLIST.md](../../UNLOCK_CHECKLIST.md). Native one-command Close/Exit, corrected journeys and the remaining manual workflows require an explicit instruction to resume after the physical stop. No native failure is converted to a pass.

## Final executed disposition — 2026-09-12

All 31 local implementation packages are accepted. See [FINAL_ACCEPTANCE.md](FINAL_ACCEPTANCE.md) for the full-suite R8 boundary, reviewed lint-only R9 cleanup, affected 16/16 views, successful exact ratchet/format/final build, independent native confirmations, and remaining qualification gaps. The full suite was not repeated for equivalent lint restructuring. Failed diagnostics and journeys remain retained. Original HEAD and index are preserved.
