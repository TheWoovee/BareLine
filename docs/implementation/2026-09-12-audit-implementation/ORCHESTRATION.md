# Implementation orchestration — 12 September 2026

Owner request: implement all 31 local audit PR plans with GPT-5.6 Sol agents at medium effort, use parallel work where dependencies permit, have the root agent review and accept or return corrections, then commission fresh Sol medium agents to repeat technical and native testing.

## Verification policy (current owner instruction)

The owner explicitly prohibited full build/test gates for every fix. This overrides earlier brief wording about per-PR integration builds/full Clippy. Implementors run only necessary contained tests or checks. Root coordinates shared build resources and performs the full integrated build/test qualification after fixes are integrated. Documentation/cosmetic work does not earn a redundant compile. New external dependencies require the applicable targeted policy check; no arbitrary dependency or runtime additions.

The owner also requested conservative token and resource use. Run at most four implementation agents concurrently and one Cargo command at a time. Use fresh Sol medium agents with compact handoffs for new unrelated PRs now that the original agent histories are long. Reuse the original implementor for corrections and closely related work already in progress. Reuse the current Graft graph for context and Cargo incremental artifacts. Root reviews bounded patches and their actual call paths; it does not repeat the whole audit for each PR. Do not start extra speculative agents or repeat passing checks without a changed risk.

**Build-cache correction:** initial serialized worktree checks still reused incompatible check metadata across source paths. All subsequent Cargo verification runs only in the original integration checkout after root source review and patch application. Workers keep implementation and standalone/script probes isolated. Root invalidates only affected workspace-package artifacts once when establishing the central source path, preserving external dependency cache. A preliminary worker compile is not a final integrated result.

## Isolation and acceptance

- The original checkout remains the integration workspace, including the owner's pre-existing uncommitted changes. No remote operations, publication, reset, or removal of those changes.
- A private local snapshot commit captures the current working tree without moving its branch or index. Isolated worktrees start from this snapshot, not from the older public HEAD. Each worktree contains only its assigned PR changes above that baseline.
- Implementors read `AGENTS.md`, their brief, and necessary linked contracts. Graft context comes first; use the original checkout's current graph if an isolated worktree has no graph. Old closure worktrees are not current evidence.
- Every PR gets `PR-<id>.md` here before implementation, recording baseline, scope, decisions, changed files, exact contained checks and unresolved qualifications. Do not edit the shared status files from a worker worktree.
- Workers commit only their changes in their own worktrees and provide commit, base, diff summary and a review request. Root reviews the patch for behavior, state/error/race ownership, test substance, native implications and unintended changes. Findings are sent back to the same agent.
- Root applies accepted patches to the integration working tree; it does not stage or commit the owner's existing changes. Interface dependencies are integrated before their dependents. Conflicting changes are rebased/reviewed rather than silently overwritten.
- Root alone runs Cargo in `D:/Notepad_REq/Bareline_Product_Blueprint/Bareline-Editor`, using its existing target. Workers submit exact proposed contained commands with their ready patch. Do not start a full gate or compete for native desktop control. A worker may run parser/script tests or a standalone bounded probe without a Cargo lease.
- Review acceptance is distinct from final qualification. States: queued, implementing, review, corrections, accepted-pending-integration, integrated-pending-final, verified, or external-qualification-pending. Never count a missing environment as a pass.

## Dependency lanes

1. Launch/data: T17 → T01 → T02; T05 after owned-deletion contract. T18 after T06 and launch changes.
2. Save: T03 → T04; T16 after save/recovery/worker contracts stabilize.
3. Workers: T06 → T07 → T08; coordinate paged service interfaces with Save/Recovery.
4. Input/accessibility: U01 → U07/U08; U09 document-bound search; U11 after focus/pane foundations.
5. Independent settings/menu/UI: U10, U06, U03/U04; U02/U05 coordinated with lifecycle notifications; U12/U13 after relevant UI interfaces settle.
6. Tooling/release: T12; T09/T10/T13 tooling; T11 after actual component/worker contracts; T14 current evidence ledger; T15 final controlled performance qualification.

The source of scope is [the audit PR index](../../qa/2026-09-12-local-audit/PR_INDEX.md). Root owns the live `status.json` and acceptance notes. The final fresh-agent audit must exercise the original repro cases and normal save/edit/reopen, cancellation, session, search/replace, macro/process, compare, accessibility and layout workflows, retaining evidence and exact limitations.

## Private-worktree stash ownership

Git stash is repository-global, even across separate worktrees. On 12 September the workers lane accidentally applied the modal lane's U08 stash via implicit pop. All deltas were preserved: U08 pinned at refs/codex/recovered-u08 and exported to audit-implementation-worktrees/recovery-evidence; modal WIP remained intact; workers restored only accidental U08 paths and reapplied its exact T18 stash hash. Original integration was unaffected. Future parking must use captured exact commit IDs and apply without implicit pop/drop, or a private patch/ref. Never identify work solely by stash stack index.

