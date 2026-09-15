# Implementation plan — 2026-09-10 review-closure pass

Source: the 2026-09-10 project review (`docs/qa/REVIEW-20260908-CODE-AND-UX.md`,
`docs/qa/evidence/review-20260908/review_security.md`,
`docs/implementation/PROGRESS-20260908-REVIEW-FIXES.md`) plus two security
findings the 2026-09-08 fix plan never listed (LEAK-06, LEAK-07) and one dead
menu command (`encoding.charsets`). Baseline `51cd4c0` with the review-fix
working tree.

Ordering: verified defects first, then the deferred user-visible items, then
code health, then acceptance that needs an unlocked desktop.

## Phase 0 — bookkeeping

- `.gitignore`: ignore `/dist/` and agent/tool state (`.claude/`, `.cursor/`,
  `.gemini/`, `.grok/`, `.mcp.json`, `opencode.json`, `GEMINI.md`,
  `*.cdx.json`).
- `docs/UNLOCK_CHECKLIST.md`: add the newest lock-blocked rows (journey suite,
  wave-3 native checks, UIA qualifications).
- `docs/IMPLEMENTATION_STATUS.md`: correct PR-011/PR-009 wording that claims
  P1-7/UX-19 were fully fixed.

## Phase 1 — verified defects

| Item | Files | Acceptance |
|---|---|---|
| Character Sets picker | `apps/bareline/src/windows_app/encoding.rs`, overlay plumbing | Encoding menu opens a searchable family list; ANSI/Windows-1252 selectable; selection runs the interpret path with dirty confirmation |
| LEAK-06 execution budget | `crates/extensions-protocol/src/{lib,broker}.rs`, `apps/bareline/src/windows_app/extensions.rs` | Background pending request survives past 5 s; interactive still expires at 5 s |
| LEAK-07 reopen leak | `apps/bareline/src/windows_app/watch.rs` | `reopen_pane` entries removed on every terminal branch |
| UX-19 menu leak | `crates/settings/src/lib.rs`, `crates/app/src/{macros,menus}.rs`, `apps/bareline/src/windows_app/extensions.rs` | UI-only and inapplicable actions never render as menu items |
| Run… (F5) prompt | `apps/bareline/src/windows_app/macros.rs`, overlay plumbing | F5 opens a prompt; a typed direct command runs through the existing confirmation path |

## Phase 2 — deferred user-visible items

- Determinate indexing progress bar plus RO-badge tooltip.
- Estimated gutter: italic style (renderer face extension) or tooltip fallback.
- Find/replace toggle tooltips and viewport highlight-all.
- Bottom dock for Output/Search results/Compare (`DockLayout` already computes
  the bottom rect; the shell does not route panels into it).
- Branding audit (icon sizes, About wordmark).

## Phase 3 — code health

- Clippy: fix the real-lint categories (`permissions_set_readonly_false`,
  `unnecessary_unwrap`) and burn down the rest of `CLIPPY_BASELINE_ALLOW`.
- Spawn migration onto the bounded task pool.
- Extension-host sandbox follow-ups (mitigation policy, low IL, optional
  AppContainer).
- Full `editor-surface`/`file-io` lifecycle-type independence.

## Phase 4 — acceptance

Native checks require an unlocked desktop: `cargo xtask journey all`, the
wave-3 (a)–(f) checks, UIA qualification, and the signed-release gates. Record
evidence in `PROGRESS-20260910-REVIEW-CLOSURE.md` and the owning PR notes.
