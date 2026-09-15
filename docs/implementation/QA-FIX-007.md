# QA-FIX-007 — accessibility follows the active split view

Authority: UI-ED independent split selections and PR-024 active editor accessibility. Cause: native accessibility snapshot/text reader and selection/scroll actions always indexed the primary workspace editor. Use existing active_workspace_editor selectors for both publication and actions.

Verification: static only; builds/tests deferred. Pending split focus, secondary selection, geometry and rectangular selection native verification. Rectangle multi-range exposure is not changed by this selector fix.

Follow-up: active-pane geometry is clipped to actual pane bounds, global status footer stays global, and UIA GetSelection returns bounded existing selection rows (up to1024), preserving primary caret/preedit. UIA advertises Multiple and AddToSelection/RemoveFromSelection queue source-bound operations preserving the other ranges and a caret fallback when removing the last range. Existing fixture constructors updated for the added context field; no tests executed.

Multiple-selection manipulation follow-up: native provider now reports Multiple; Add/Remove queue ModifySelection actions. Active UI owner checks source identity/busy state, preserves other ranges and primary when possible, keeps a caret if removing the final range, and applies resident/paged selection APIs.

Golden review after parent batch: updated only 32 selection-range fields (equal to the existing primary selections) and Editor bounds in six compare snapshots from full window to the actual left pane [0,78,497,698]. Parsed both assertion sides to exclude decimal round-trip noise. Assertion unchanged; no tests run by implementation agent.
