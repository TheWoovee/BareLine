# PR-U01 — Establish modal focus, input ownership, and dismissal

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

Run and Go To draw focused fields but publish no UI Automation nodes. Assistive text therefore edits the document behind them. Compare Options is visually modal while the background editor can receive pointer/keyboard commands, Escape does not dismiss it, and its visible Done button is absent from UIA. Users can unknowingly alter a document while acting on a modal surface.

## Scope

- `apps/bareline/src/windows_app/accessibility.rs`
- `apps/bareline/src/windows_app/run_prompt.rs`
- `apps/bareline/src/windows_app/goto.rs`
- `apps/bareline/src/windows_app/compare.rs`
- `apps/bareline/src/windows_app.rs` transient routing
- `crates/ui/src/focus.rs` or the current focus-layer owner
- Native journey additions in `xtask/src/journey.rs` and `docs/qa/JOURNEYS.md`

## Ordered implementation

1. Add one shell-level modal descriptor containing surface id, stable semantic ids, active text owner, invoker, dismissal policy, and inert-background state.
2. Route pointer, raw keyboard, IME, TextPattern, SetValue, and Invoke through that descriptor before editor routing. Events that cannot be accepted by the live modal must return unavailable/handled rather than fall through.
3. Publish Run and Go To groups, fields, help/status text, values, bounds, and focused state. Make the actual prompt `TextField` the accessibility text source.
4. Register Compare Options, both tabs, every option, Copy actions, and Done in the same modal system. Disable editor, tabs, menu/status semantics while it is open.
5. On open, record the invoker and emit one focus-changed event. On submit, Done, outside-dismiss where allowed, or Escape, close exactly once, clear composition/state, and restore the invoker if it still exists.
6. Define Escape consistently: cancel a nonempty IME preedit first; otherwise close on the first press. Fix Run and Compare Options against this contract.

## State, error, and race handling

- Accessibility snapshots must read the same modal state used by event dispatch; do not infer focus independently.
- A closed surface must never remain the text provider after an asynchronous redraw.
- Opening an incompatible modal first closes or rejects the earlier one deterministically.
- If the invoker disappears, restore focus to the active surviving editor.
- UIA input during close/open races must either apply to the live owning field or return unavailable; it must never fall through to the editor.
- Preserve IME preedit text within the owning surface. Escape cancels active composition first only when preedit is actually nonempty; the next Escape closes.

## Tests and acceptance

- Provider tests assert exactly one focused node for every transient combination and that inactive editor nodes are disabled.
- UIA integration: open Run, set its field to `C:\tool.exe`; editor bytes stay unchanged. Repeat for Go To with `10:2`.
- Native journey: first Escape closes an empty Run or Compare Options; a composed prompt cancels preedit then closes on the next Escape.
- Pointer/key regression: click the dimmed editor and send text/Undo while Compare Options is open; the document and history do not change.
- UIA snapshot contains exactly one focused modal element, disabled background nodes, and invokable Done/submit/cancel controls.
- Focus returns to the invoking editor/control after every dismissal path.

## Risks and dependencies

Land this foundation before PR-U07 and PR-U08. Stable ids must not collide with existing semantics. Preserve current native confirmation dialogs, which have their own Windows focus model.

## Definition of done

Every visible modal owns all input and accessibility focus, background documents cannot change through modal interaction, and every advertised dismissal path works through mouse, keyboard, and UIA.

Status: Planned. Priority: P1. Finding: [UX-02](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
