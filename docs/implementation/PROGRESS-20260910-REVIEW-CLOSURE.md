# Progress — 2026-09-10 review-closure pass

Plan: [PLAN-20260910-REVIEW-CLOSURE.md](PLAN-20260910-REVIEW-CLOSURE.md).

## Baseline verification (before changes)

- `cargo check --workspace --all-targets`: clean.
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets --locked` with the CI baseline
  allowlist: clean.
- Last recorded full suite (2026-09-09): 611 passed, 0 failed, 7 ignored.

## Verified defects closed

- **Character Sets picker (P1-7 residual)** — new
  `apps/bareline/src/windows_app/charsets.rs` overlay: family-grouped list from
  `bareline_app::encoding::character_sets()`, live filter through
  `charset_matches()`, Up/Down/PageUp/PageDown, IME and clipboard editing,
  Enter or click applies through the existing `encoding.interpret.*` command so
  the dirty confirmation and busy checks stay in one place. `encoding.charsets`
  is dispatched from the curated Encoding menu (`encoding_dispatch` previously
  answered "Unknown encoding command").
  Test: `charsets::tests::filter_finds_ansi_by_fragment_and_family`.
- **LEAK-06 execution budget** — `ExtensionSession` now stores the invocation's
  `ExecutionBudget` (`new_with_budget`); `start()` stamps pending deadlines
  from `budget.timeout_ms()` instead of the 5 s interactive constant, and the
  shell passes the declared budget at session creation. Background commands now
  survive their declared 120 s; interactive work still expires at 5 s.
  Test: `broker::tests::background_budget_keeps_pending_work_alive_past_the_interactive_deadline`.
- **LEAK-07 watch leak** — `watch.rs` removes `reopen_pane` entries in the
  closed-before-reopen and reopen-error branches, not only on success.
- **UX-19 menu leak + Run… prompt** — settings actions are registered
  `internal` (only `settings.open` stays user-facing); `macro.manager_close`
  and `run.cancel` emit `CommandState::not_applicable` when they do not apply
  (closing a closed manager, cancelling with nothing running); extension
  panel-only actions are `internal` so only "Manage Extensions" and the `ext.*`
  features stay in menus. New `run.prompt` (F5, Run menu) opens a prompt
  (`run_prompt.rs`) that parses a quoted command line requiring an absolute
  program and launches through the existing confirmation/job path in Direct
  mode — never a shell.
  Tests: `extensions::tests::panel_actions_are_internal_and_only_manage_stays_user_facing`,
  `windows_app::macros::tests::inapplicable_manager_and_run_commands_are_hidden_not_greyed`,
  `settings::behavior_tests::only_the_settings_page_opener_is_user_facing`,
  `run_prompt::tests::parses_quoted_programs_and_arguments_and_requires_an_absolute_path`.
- **a11y golden time drift (found during this pass)** — the reviewed
  `tests/a11y/native-semantic.json` embedded wall-clock "N days ago" strings,
  so it drifted every day and failed before any fix. The test now normalizes
  relative ages to `<age>` on capture and compare; the golden was regenerated
  once and all 11 changed fixtures were reviewed as age-only diffs.
  Test: `windows_app::accessibility::tests::complete_native_semantic_json_golden`.

## Phase 2

- **P2-3 determinate indexing progress** — `GlobalNavigation::index_progress()`
  reports the retained sparse line index's scanned fraction;
  `PagedEditorSurface::index_fraction()` exposes it, the paged status shows
  "Large file · indexing N%", and the footer paints a determinate fill over the
  track. The INS/RO badge now shows a "Read-only document" hover tooltip.
- **P4-9 branding audit** — `packaging/windows/bareline.ico` contains all nine
  standard sizes (16/20/24/32/40/48/64/128/256) and is already the window/tray/
  About/installer icon. No new asset needed.
- **Clippy burndown** — verified zero hits for `permissions_set_readonly_false`
  and `unnecessary_unwrap`; both were removed from `CLIPPY_BASELINE_ALLOW`
  (31 categories remain). The one intentional test use keeps a local
  `#[allow]` with a comment.

## Deferred with reasons

- **P2-2 italic estimated gutter** — true italic needs a face/style field
  through `DrawOp::Text` and every backend; the existing faded colour remains
  the in-scope approximation.
- **P4-5 find-bar tooltips / highlight-all** — `FindController::draw` receives
  no pointer or hover state, so tooltips need a signature ripple through the
  app layer; search marks already highlight matches.
- **P3-1 bottom dock for Search/Compare** — the Output band is already reserved
  by `editor_bounds` (`macros.height()`); moving Search-folder results and
  Compare into a shared bottom dock remains a cross-file refactor.
- **P5-7, P5-8, P5-11 remainder** — full `editor-surface`/`file-io` type
  split, the remaining spawn-site migration, and low-IL/AppContainer remain
  multi-day items and were not attempted in this pass.

## Verification (after changes)

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` with the
  reduced CI allowlist: clean.
- `cargo test --workspace --all-targets --locked --no-fail-fast`:
  **616 passed, 0 failed, 7 ignored** (46 harness batches); a first full run
  after the source changes reported 614 passed, 0 failed, 7 ignored.
- Native checks (journeys, UIA, wave-3) remain blocked on an unlocked desktop;
  see `docs/UNLOCK_CHECKLIST.md`.

## Native verification (desktop unlocked, 2026-09-10)

`cargo xtask journey all` ran four times: **13/13 passed three consecutive
times** after one run that surfaced two real issues (below). The suite covers
smoke, P0-3 recovery/discard, P0-4 SW_SHOW, P0-5 recovery editing, P0-7 close
prompt, P1-1/P1-2 menus, P1-5 Go To Line, P3-1/P3-6a docks/settings, P4-1 dirty
marker, P4-2 fresh Untitled, P4-4 colour emoji.

### Product fixes found by the native runs

- **P0-3 discard left a retired journal generation** — `discard_now()` purged
  the active generation while the asynchronous retirement worker still held
  Windows file handles, so `remove_dir_all` failed silently and a retired
  `paged-…-g0` directory (with plaintext `source-1/text.utf8`) survived an
  explicit Discard. It now cancels, drains the pending checkpoint and
  retirement workers, then deletes both generations with bounded retries.
  New regression:
  `resident_recovery::journal_tests::discard_now_removes_every_generation_even_mid_rotation`.
- **a11y golden wall-clock drift** (already listed above).

### Journey-runner hardening (xtask, not product)

The runner had never been executed before. Fixes: foreground claiming via
`AttachThreadInput` without the Alt tap (the tap left Win32 menu-mode active,
and posted `WM_COMMAND`s were then ignored); `SendMessage`/`PostMessage`
interaction corrected; menu path lookup retries across rebuilds; menu paths
updated to the curated labels (`View ▸ Panels ▸ Toggle Workspace`,
`Settings ▸ Settings`); typing alternates posted `WM_CHAR` with SendInput and
retries until the first editor line changes; a stable-window wait before close
commands; save prompts are awaited by button existence; `click_button` only
clicks Button-class children (dialog body text used to match first); the
recovery-directory counter counts `paged-*` journal generations instead of
arbitrary leaf directories; the P0-3 discard journey uses the acceptance path
(File ▸ Exit, Don't Save); the P4-1 marker threshold/region are corrected.

### Remaining lock/manual scope

The wave-3 (e) editor/tab context menus and (f) toast/status pickers are not
yet encoded as journeys, and the PR-024 UIA/Narrator checks still need the
assisted walkthrough; both stay in `docs/UNLOCK_CHECKLIST.md`.

## Tests added this pass

`broker::background_budget_keeps_pending_work_alive_past_the_interactive_deadline`,
`charsets::filter_finds_ansi_by_fragment_and_family`,
`extensions::panel_actions_are_internal_and_only_manage_stays_user_facing`,
`macros::inapplicable_manager_and_run_commands_are_hidden_not_greyed` (bin),
`settings::only_the_settings_page_opener_is_user_facing`,
`run_prompt::parses_quoted_programs_and_arguments_and_requires_an_absolute_path`.
