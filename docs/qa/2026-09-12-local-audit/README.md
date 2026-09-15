# Bareline local project audit — 12 September 2026

**The current app has working core features, but important data-safety and usability defects remain.** This audit reviewed the latest local `Bareline-Editor` working tree, including uncommitted changes. The closure worktrees were not treated as current, and no remote repository was searched or used.

The requested **GPT-5.6 Sol agent at medium effort** performed the independent native UI/UX and user-journey audit. The primary audit reviewed implementation, ran the workspace/build/security gates, tested real first-party components and authored isolated defect reproductions. The deliverable contains **22 technical finding entries, 11 UI/UX finding groups and 31 implementor-ready PR plans**. Some UI entries cross-reference technical findings and some groups contain multiple presentation gaps; do not add these counts as distinct defects. Production source was not changed.

## Read and implement

- [Technical findings and reproductions](technical/findings.md): data ownership, save/recovery, workers, launch lifecycle, architecture, build and release qualification.
- [UI/UX findings](ui-ux/findings.md): native observations, expected/actual behavior and source traces.
- [Native workflow coverage](ui-ux/coverage.md): exactly what was exercised and what remains unverified.
- [User workflow and product impact](WORKFLOW_IMPACT.md): how the findings block work, reduce confidence or leave product claims unqualified.
- [Test report and retained evidence](TEST_REPORT.md): raw commands/results, binary/source hashes and limitations.
- [PR index](PR_INDEX.md): all individual implementation briefs.
- [Implementor contract and dependency order](IMPLEMENTOR_CONTRACT.md): shared invariants, ownership, review and completion rules.

## Highest-priority work

1. Confine cache cleanup and preserve concurrent file writers. A generated junction fixture was deleted outside the intended TEMP root, and a controlled save interleaving overwrote another writer's bytes while reporting success.
2. Fix Save As overwrite consent and resumable profile migration. Both failed in current-code reproductions; Save As also failed through the real native dialog.
3. Prevent input from editing behind Run/Go To and Compare options overlays. Their visible controls and active input ownership do not agree with the accessibility tree.
4. Make recovery retirement, background task outcomes and launch requests explicit and recoverable. Failed/canceled work must free capacity and cannot silently report cleanup complete.
5. Fix remaining notifications, menu density/theme, focus and recovery-loading states, then finish the outstanding native and release qualification.

## What the tests establish

The app builds. The fresh workspace run had **612 top-level tests pass, one timing assertion fail and seven tests ignored**. The failed timing test passed ten isolated reruns; the original failure is retained. CI-baseline Clippy, formatting, the portability guard and cached offline dependency checks pass; unrestricted strict Clippy still reports inherited debt.

All four real first-party component tests subsequently passed in the release profile, including the 1 GiB JSON workload and bounded 5 GiB Hex navigation. The explicitly run multi-GB diff test also passed. The final rebuilt-binary native journey run passed **11 of 13**, with unresolved close-prompt and emoji-color assertions. These results and the successful exploratory workflows are retained separately in the native coverage document. Functional/resource results are not a controlled Notepad++ performance comparison.

Findings distinguish runtime reproductions, native observations, source-confirmed paths, maintenance debt and missing qualification. A broad review cannot prove the absence of further defects. Unexecuted physical input, screen-reader, platform, signing and performance cases remain explicit instead of being counted as passes.

The briefs are **local planned PR work packages**, not published pull requests. Each provides the problem/evidence, current code ownership, ordered changes, state/error/race rules, tests, dependencies and acceptance criteria. Do not implement historical September 8 findings blindly: several were already fixed before this audit.
