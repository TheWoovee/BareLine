# PR-U12 — Explain Find and Replace icon toggles

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

The Find bar uses compact `Aa`, `ab`, `Lit/Ext` controls without hover help. Their meaning and selected state are difficult to learn, especially because Extended is not a regular-expression mode.

## Scope

- `crates/app/src/find.rs`
- `apps/bareline/src/windows_app/search.rs` / find drawing and hit testing
- reusable `crates/ui` tooltip primitive
- accessibility names/descriptions and menu state

## Ordered implementation

1. Give each toggle a stable label, concise tooltip, selected/pressed state, and shortcut where applicable.
2. Thread pointer/hover time into Find drawing and use the existing delayed tooltip primitive with viewport clamping.
3. Use explicit names such as `Match case`, `Whole word`, and `Extended escape sequences`; state what Extended recognizes.
4. Keep UIA name/description/state identical to the visible tooltip contract.

## Tests and acceptance

- Hover each toggle long enough to show the correct tooltip; moving away or closing Find dismisses it.
- Keyboard/UIA users receive the same name and selected state without hover.
- Tooltips clamp at narrow widths and 100/150/200% DPI without obscuring the active query.

## Dependencies and risks

Can follow PR-U07. Avoid an idle redraw loop; schedule only the next tooltip deadline.

## Definition of done

Every icon toggle states its function and current state through mouse, keyboard focus, and accessibility APIs.

Status: Planned. Priority: P3 design gap. Finding: [UX-11](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
