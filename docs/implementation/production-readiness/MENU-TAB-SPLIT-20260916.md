# Menu repaint, Tab insertion and split-view verification

The owner reports a rapidly blinking menu after launch and Tab indenting the start of a line while the caret is inside a sentence. They also request a split-view check. Address these sequentially with focused regressions and one final application build/native check.

## 1. Menu repaint

`render_frame` refreshes native command state every frame. `WindowsPlatform::sync_commands_localized` rewrites every native menu item and calls `DrawMenuBar` even when labels, checked/enabled/radio state and owner-draw type are unchanged. Retain the last successfully applied native projection and skip unchanged writes/repaints. Invalidate it on menu reconstruction, and preserve locale/keymap/state updates and native fallback rendering.

Interfaces: `apps/bareline/src/windows_app.rs::refresh_menus`, `crates/platform-windows/src/native.rs::sync_commands_localized`, and the owner-draw label/MSAA synchronization in `crates/platform-windows/src/menu_bar.rs`.

## 2. Tab behavior

Trace the effective shortcut dispatcher and both pane input paths. A plain Tab at an empty selection must use normal tab-character insertion at the caret; selection indentation and Shift+Tab must retain their intended behavior. Preserve completion/snippet and modal focus handling. Record resident and paged-path coverage.

## 3. Split view

Check the current split controller and native routing, then exercise independent pane focus, edits, shared-document synchronization, resizing and closing a split using isolated scratch documents. Record any limitation of the actual native observation. Do not count existing headless checks as a fresh native pass.

## 4. Recovery status stability

The owner also reports the footer alternating between the recovery-preparation message and the command hint during ordinary typing. Delay routine preparation notices until the operation stays pending for one second, reset the delay when it completes, and publish failures immediately. Keep long-running preparation visible and retain document-generation fencing. Verify quick repeated cycles, delayed preparation, failures, and cleanup with a controlled clock. The recovery pump also unconditionally requested redraw from the redraw handler; request a frame only when recovery UI changes, and schedule the delayed notice through the event-loop deadline.

## Delivery

Update implementation status and the installed Windows preview after verification. Preserve personal documents and the earlier immutable test evidence. Full suites are not required for each correction; use focused behavioral tests, then one integration build and targeted native checks.

## Results

Implemented the menu projection cache, recovery redraw/delay correction and caret Tab handling in both shortcut dispatchers. The first native check exposed the earlier effective-keymap route; the corrected debug candidate passed caret insertion and vertical split edits/resizing. Focused regressions and the exact Clippy ratchet passed. See [verification and delivery report](../../qa/2026-09-16-menu-tab-split/README.md). The owner explicitly requested pushing to the existing remote primary branch and installing the Windows update.
