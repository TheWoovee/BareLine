# QA-FIX-012 — palette owns input above Compare

Authority: PR-011 layered focus and UI keyboard contract. Compare event handler did not yield while palette was open, allowing underlying compare controls to intercept palette input. Add the same palette guard used by other overlays.

Verification: static only; builds/tests deferred. Folder/open-document query mutation remains under diagnosis; existing handlers alone do not establish native delivery. This is a partial correction, not closure of ISSUE-012.

Concrete shortcut follow-up: settings_keymap_event ran before query handlers with the document keymap. Active text/modal layers now filter document commands while retaining palette and safely routed text actions. Native/menu SelectAll/Copy/Cut/Paste/Undo/Redo target folder/open-document fields, and folder controls without an active text field consume these actions safely. This addresses the clipboard-based typing mutation path; native validation remains pending.
