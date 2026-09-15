# PR-U01 — Modal focus, input ownership, and dismissal

## Baseline

- Rebased integration commit: `7b178da5c4b9d01fcce24653f45c0c4a73352d3d`
- Worktree: `audit-implementation-worktrees/modal`

## Scope

Establish shared modal ownership for Run, Go To, and Compare Options so keyboard, pointer, IME, and accessibility input cannot fall through to the document. Publish the visible modal controls through UI Automation, restore the invoker after dismissal, and preserve native-dialog focus behavior.

Search, recovery, and split-provider expansion remain assigned to PR-U07 and PR-U08.

## Decisions

- A single `ModalDescriptor` owns the active custom modal, semantic IDs, keyboard focus, text owner, invoker, dismissal policy, and background inertness. Opening another custom modal dismisses the prior owner first.
- Run and Go To dismiss on outside click. Compare Options blocks outside input and closes through Done or Escape. Escape first cancels a live IME composition; the next Escape dismisses the surface.
- Each modal open and text change receives a shell-wide, monotonic, non-recycling identity token. UIA text providers use a modal-only namespace with that token, and editor selection rejects the namespace after dismissal. This prevents an old provider range from changing either a reopened field or the document.
- Run, Go To, and the Compare color editor publish their own bounded text providers. While a modal is live, editor text providers and background accessibility actions are unavailable.
- Run and Go To expose visible submit/cancel buttons, cycle field and buttons with Tab/Shift+Tab, and draw the focus cue on the actual keyboard owner.
- Modifier changes are captured before modal routing so press and release state are current for shortcuts and reverse traversal.
- Native platform confirmations remain native; the custom modal layer does not replace their focus behavior. Modal input changes do not rewrite document history or document bytes.

## Changed files

- `apps/bareline/src/windows_app/modal.rs`: shared modal lifecycle, focus, input capture, generations, and dismissal.
- `apps/bareline/src/windows_app.rs`: shell modal state, early event ownership, background command guard, and focused rendering.
- `apps/bareline/src/windows_app/{run_prompt,goto,compare,macros}.rs`: modal activation, keyboard/pointer/IME routing, focus cues, and Compare Options ownership.
- `apps/bareline/src/windows_app/accessibility.rs`: modal UIA groups, fields, buttons, focus, inert background, text providers, and stale-action rejection.
- `crates/platform-windows/src/accessibility.rs`: text-selection actions for text fields.
- `crates/ui/src/text_field.rs`: bounded selection and composition state used by UIA.
- `xtask/src/journey.rs` and `docs/qa/JOURNEYS.md`: Run/Go To open-and-Escape journey coverage.

## Focused checks

- Direct `rustfmt --edition 2024` completed for each changed Rust file.
- `git diff --check` completed without whitespace errors.
- Focused regressions cover modifier press/release, distinct modal generations, stale provider identity after reopen/dismissal, bounded Unicode reads from every byte start, modal UIA focus/text/background ownership, and visible Run/Go To button focus cues.
- No Cargo command or native desktop run was performed in this isolated worktree under the integration resource policy.

## Unresolved qualifications

- Root must run the contained Cargo checks in the integrated checkout.
- A fresh native audit must verify Narrator/UIA focus, Compare Options Done ownership, outside-click behavior, IME Escape behavior, and the updated `p1-5` journey against the integrated binary.
- Search, recovery, and split-provider expansion remain with PR-U07/PR-U08.
