# Menu, recovery status, Tab and split-view fixes

## Changes

- Native menus retain their last applied labels, enabled/checked/radio state and rendering type. Unchanged frames no longer rewrite menu items or request another menu repaint; rebuilding the menu invalidates the cache.
- Recovery checks request redraw only when their visible state changes. Routine snapshot preparation must stay pending for one second before replacing the footer hint; failures remain immediate and persistent. Timers are retired on completion or document close, and status follows the active split pane.
- Plain Tab with collapsed carets inserts a tab character at the caret. Both the effective keymap and legacy dispatcher use the same handler. Selected-line indentation, Shift+Tab, reassigned bindings and input-field routing retain their existing semantics. Split panes use the shared document input queue.

## Focused verification

| Check | Result | Receipt |
|---|---|---|
| Real Win32 menu state/cache regression | 3 passed | 02-menu-regression |
| Resident caret insertion and shared undo | Passed | 04-tab-regression |
| Recovery UI transitions, short cycles, delay, failure and cleanup | 15 passed | 05-recovery-ui |
| Split views, resident/paged Tab, shared history and global scroll synchronization | 18 passed | 06-split-regressions |
| Effective keymap, modifiers and focused fields | 4 passed | 11-effective-keymap |
| Corrected debug candidate compile | Passed | 12-debug-candidate |
| Exact Windows Clippy debt ratchet | Passed; baseline unchanged | 14-clippy |

Receipts, stdout/stderr and terminal results are retained under [evidence](evidence/manifest.json). Every final successful receipt reports stable source during its run. Earlier failed iterations are retained: the native-menu unit fixture lacked the executable activation manifest (01); initial compile fixes (03/08); a headless focus fixture needed proper field state (09/10); and an existing lint fingerprint changed with its surrounding lines (13), resolved without adding debt or changing the baseline.

The first optimized candidate (07) still indented the line on actual Tab input. Native testing found that the effective keymap runs before the ordinary editor key handler; the correction was then applied to both routes. This candidate is superseded and must not be installed.

## Native observations

The corrected debug executable SHA-256 is `0450d84d0017274f13e90da788d23be6bfbdad36aa96188761e442a6c2b75eac`. It ran with an isolated portable profile and generated scratch text through the computer-use plugin.

Observed passes: `hello world` became `hello \tworld` when Tab was pressed before the second word; selected text indented and Shift+Tab removed the indentation; vertical split opened; right-pane edits and Tab used its own caret; left-pane edits used its own caret; shared text updated in both panes; divider dragging resized both panes. The command hint stayed visible across these edits and menu/palette operations.

These are bounded observations, not high-frame-rate flicker measurements or full accessibility qualification. The tool reported a physical-Escape interruption while preparing the horizontal-split check. Horizontal layout and native split-close checks were left pending; the owner subsequently authorized resuming, pushing and installing. No interruption was inferred from document content.

## Delivery

The final optimized rebuild, installer and per-user installation are pending at this source checkpoint. Existing production-readiness counts are unchanged; this fix does not establish signed-release or full product acceptance.
