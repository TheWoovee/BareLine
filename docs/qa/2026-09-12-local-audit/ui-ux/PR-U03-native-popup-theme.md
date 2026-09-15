# PR-U03 — Apply the resolved theme to native popup menus

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

Bareline's dark title bar/editor is paired with light-gray File/Search/Settings popup menus. The sharp transition is persistent and reduces visual cohesion.

## Scope

- `crates/platform-windows/src/menu_bar.rs`
- Native menu construction code in `crates/platform-windows/src/native.rs`
- Platform-Windows menu tests and a small native screenshot journey

## Ordered implementation

1. During menu attachment, recursively enumerate every submenu and retain stable item/menu ownership metadata.
2. Apply background brushes and owner-draw state to popup items as well as top-level items, preserving native menu navigation and MSAA metadata.
3. Paint enabled, disabled, selected, checked, separator, submenu-arrow, shortcut, and high-contrast states from the resolved UI theme.
4. Refresh all menu brushes/fonts on theme and DPI changes, then invalidate the menu bar and open popup safely.
5. Release every GDI object and restore original menu item state on teardown.
6. If Windows high-contrast mode is active, defer colors to system metrics while retaining correct text/state contrast.

## State and error handling

- Failed styling of one submenu falls back to native system rendering for that submenu without breaking command invocation.
- Dynamic command labels and enable/check states must update without replacing accessibility names or ids.
- Nested popup ownership must not leave dangling item data after rebuild/drop.

## Tests and acceptance

- Unit-test recursive enumeration/state mapping with a synthetic three-level menu.
- Native journey captures a dark-themed popup and asserts its interior background is dark while disabled text and selection remain distinguishable.
- Repeat at 100%, 150%, and 200% DPI and in light/system/high-contrast modes.
- Keyboard arrows, access keys, accelerators, mouse hover, submenu expansion, and screen-reader names remain native-functional.

## Risks and dependencies

Owner-drawn Win32 menus are sensitive to DPI, GDI lifetime, and accessibility metadata. Prefer a contained recursive extension of the current `MenuBar` rather than a custom in-app menu replacement.

## Definition of done

Every popup follows the resolved theme or the system high-contrast theme, with no regression in native navigation, states, or accessibility.

Status: Planned. Priority: P2. Finding: [UX-05](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
