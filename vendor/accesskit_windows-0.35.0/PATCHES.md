# Bareline local patches

Upstream: `accesskit_windows` 0.35.0, crates.io SHA-256
`a59e8d7160cbac991c1345c99153783ab96423644ccb1f20617cf80c97596aa1`.
Upstream git commit: `42e53b0d829e7a0b34dc8803bb06012db2e80cc6`,
`adapters/windows`. The root `Cargo.toml` substitutes this copy with `[patch.crates-io]`.

Apart from the two patches below, this file and the license files, every file has the
same content as the registry archive (line endings aside). These are application
patches, not a claim that upstream provides these APIs.

## 1. Editor pattern and property override

Files: `src/pattern_override.rs` (new), `src/lib.rs` (module and re-exports),
`src/node.rs` (`GetPatternProvider` and `GetPropertyValue` of `PlatformNode`).

- `register_pattern_override(hwnd, &Arc<dyn PatternOverride>)` stores a weak per-HWND
  factory and returns a `PatternRegistration` guard. Dropping the guard removes only its
  own entry, so a reused HWND keeps the newer registration, and no UIA client can keep
  the factory alive.
- `GetPatternProvider` and `GetPropertyValue` resolve the node's tree and ID under the
  consumer tree lock, release the lock, and only then ask the factory. `pattern` may
  return a replacement pattern object; `property` (default: none) may return a property
  value. When the factory declines, the unchanged AccessKit code runs.
- Bareline (`crates/platform-windows/src/accessibility/text_provider.rs`) answers only
  the editor node's Text, Text2 and TextEdit patterns and their `Is*PatternAvailable`
  properties. AccessKit keeps node identity, fragment navigation, all chrome patterns
  and event publication. No registry cache is edited.

## 2. Public focus reconciliation

File: `src/subclass.rs`.

`SubclassingAdapter::update_window_focus_state(is_focused)` is a public wrapper around
the existing private `SubclassImpl::update_window_focus_state`, which upstream calls only
from its focus, menu-loop and size-move message handling. It raises the resulting focus
events after the adapter state borrow ends. Bareline calls it from
`AccessibilityAdapter::update` (`crates/platform-windows/src/accessibility.rs`) with the
focus that `GetGUIThreadInfo` reports, outside `WM_GETOBJECT`, so UI Automation does not
stay focused on the window root when a focus message was missed.

## License files

`LICENSE-APACHE`, `LICENSE-MIT`, `LICENSE.chromium` and `AUTHORS` are the upstream files
at the commit above; the crates.io archive omits them. The upstream README's License
section points to the MIT/Apache files, to AUTHORS for copyright attribution and to
LICENSE.chromium for derived portions. Bytes are preserved without editing (retrieved
2026-09-06 from commit-pinned raw files):

- LICENSE-APACHE: https://raw.githubusercontent.com/AccessKit/accesskit/42e53b0d829e7a0b34dc8803bb06012db2e80cc6/LICENSE-APACHE ; SHA-256 62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a
- LICENSE-MIT: https://raw.githubusercontent.com/AccessKit/accesskit/42e53b0d829e7a0b34dc8803bb06012db2e80cc6/LICENSE-MIT ; SHA-256 23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3
- LICENSE.chromium: https://raw.githubusercontent.com/AccessKit/accesskit/42e53b0d829e7a0b34dc8803bb06012db2e80cc6/LICENSE.chromium ; SHA-256 845022e0c1db1abb41a6ba4cd3c4b674ec290f3359d9d3c78ae558d4c0ed9308
- AUTHORS: https://raw.githubusercontent.com/AccessKit/accesskit/42e53b0d829e7a0b34dc8803bb06012db2e80cc6/AUTHORS ; SHA-256 3bab8c36f6a85657504aaefb37c7ff34e29b8c917290f3d28a1f0920c992e502

`packaging/windows/generate-notices.ps1` reads all four from this directory.
