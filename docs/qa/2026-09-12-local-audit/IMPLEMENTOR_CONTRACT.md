# Rules for implementing this audit's PR plans

1. Work in the latest `Bareline-Editor` local working tree. HEAD alone is insufficient: this audit reviewed many uncommitted fixes. Read `AGENTS.md`, your assigned brief, the current findings and only linked dependency contracts. Use the Graft context graph, then check the current exact code because line numbers may move.
2. Create the implementation/evidence note before changing code. Identify your PR, issue IDs, baseline source/binary hash and dependencies. Keep each PR independently reviewable. Do not create a remote, push, publish or sign owner artifacts without a separate owner request.
3. A plan is a work package, not authority to discard unrelated changes. Do not reset or replace the user's working tree, revive old closure code, or duplicate a fix owned by another PR. Coordinate common files in the dependency order below.
4. Preserve document byte provenance, encoding/BOM, one-undo transaction semantics, dirty-state correctness, save conflict protection, recovery ownership, bounded resource use, platform isolation and production trust checks. Do not solve a UI symptom by disabling a feature, suppressing an error or weakening validation.
5. Reproduce the issue before the fix using generated scratch fixtures. Prefer exact state/barrier tests to sleeps, mocks that only mirror the implementation, or broad screenshot pixel claims. After wiring, run the focused tests and one integration build. Shared file-safety changes need the meaningful lifecycle regression cohort.
6. Verify user-visible behavior in the actual native app where a brief calls for it. Headless/RecordingBackend, UIA and physical mouse/keyboard tests establish different things. UIA action routing must not mutate a document behind a modal overlay. Preserve failed and blocked runs rather than manufacturing a pass.
7. Match the applicable current blueprint mockup, after viewing it, for composition and hierarchy. Written interaction contracts govern behavior. Document unresolved mockup/behavior conflicts. Keep light/dark/high-contrast, keyboard, accessible names/focus and scaled hit targets in scope for UI work.
8. A PR is done only when its explicit before/after reproduction passes, invariants remain true, evidence is recorded, user-facing states/errors are verified, and the capability ledger is updated. A later task must be able to resume from the brief and evidence without conversation history. If a required environment is unavailable, state the exact unexecuted check and retain its pending status.

## Implementation order and ownership

| Wave | PRs | Reason and constraints |
|---|---|---|
| 0 — launch modes | T17 | Separate mode selection before T01/T02 modify launch initialization |
| 1 — data ownership | T01; then T02/T05 | Confine deletion before adding migration/retirement reuse; T02/T05 can separate modules after shared guard interfaces settle |
| 2 — save correctness | T03 then T04 | Carry approved destination identity first; preserve concurrent displaced bytes before declaring save safe |
| 3 — worker contracts | T06 then T07 then T08 | Fix terminal semantics before shared scheduling and additional adoption |
| 3b — launch request ownership | T18 | Consume typed terminal outcomes and release failed/closed request slots |
| 4 — UI/input correctness | U01, then U07/U08; U09; U02/U05/U06/U10; U03/U04/U11/U12/U13 | U01 establishes modal ownership; U07/U08 use it for search/recovery and pane providers; U09 binds results to document state. U02/U05 share notification/loading rules. U11 follows U01/U07/U08 and preserves the main comparison editors. Input/data-state fixes precede polish; T03 owns Save As |
| 5 — qualification | T09/T10, then T11 | Test reliability and actual components before configured release qualification |
| 6 — maintainability/evidence | T12/T13/T14 | Manifest and lint tooling can be independent; ledger completion waits for the evidence it claims |
| 6b — lifecycle boundary | T16 | Refactor only after the behavioral/lifecycle contracts have stabilized |
| 7 — product claim | T15 plus outstanding native matrix | Measure stable release behavior and retain unresolved owner/environment prerequisites |

PR labels are planned local work packages, not remote pull requests that have already been created. P1 qualification does not mean an active security exploit; see each finding's evidence label.
