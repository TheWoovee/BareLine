# Remaining native input issues — 2026-09-08

Scope: native retest 012, 008 and source audit of 028. Authority: AGENTS.md, decision log, PR-011 command/input precedence and prior QA-FIX-012/028/029 notes. Existing workspace changes preserved.

- 012: `Shell::dispatch` previously routed clipboard/edit actions to `views_action` before the visible palette. Split/compare therefore consumed Paste and Select All despite the keyboard/IME handlers already yielding. Palette clipboard/edit handling now runs before split dispatch, including Undo/Redo and Cut/Copy. Native validation must cover both panes, compare and regular clone, and confirm the document bytes/dirty state remain unchanged.
- 008: both dirty-close confirmations already supplied the live owner HWND and defaulted to No. Added `MB_SETFOREGROUND` to the tab and application discard dialogs. Source tracing found no held mutex or RefCell borrow across these MessageBox calls; the shell's Rust references alone do not establish a deadlock. This is explicit activation hardening, not proof that the prior inaccessible-modal observation is resolved. Native retest must locate the owned dialog, cancel without losing text, then separately confirm discard.
- 028: folder controls return false for CloseRequested, allowing the shell's existing close/tray route. Cancel Search closes folder configuration and signals both open-document/folder search jobs; Close Search Results closes configuration, hides results and clears search focus. Folder Escape cancels preedit first, otherwise dismisses configuration. No additional routing changes were required; cancellation and Alt+F4 remain native verification items.

Coordinated rendering integration requested by the rendering owner: scrollbars paint before compare/options while retaining global pointer geometry; Output paints after the editor, with the internal editor footer clipped when Output is visible. The rendering owner owns assessment of these changes.

Verification: source tracing only. No builds, tests or native UI were run by this task; frozen for the parent's coordinated verification pass. Graft retrieval reported approximately 76,426 tokens saved.

## Dirty-close follow-up after native retest

Foreground flags did not resolve 008 in the first corrected executable. Final tab/application close now queues intent and creates the native prompt from about_to_wait after the input WndProc unwinds, matching the existing native menu command phase. Tab intent captures the document identity, index and active view tab; any mismatch cancels the stale request. Busy/dirty checks run again at consumption. No/Cancel still leaves the document intact. A focused stale-target regression was added; execution deferred to the correction batch. The exact blocking native stack was not captured, so native visibility/cancellation remains the acceptance criterion.
