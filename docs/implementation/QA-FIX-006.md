# QA-FIX-006 — do not consume redraws in Extensions

Scope: ISSUE-006 Extensions dismissal; also affects ISSUE-014 visible state updates. Authority: UI keyboard dismissal and native shell lifecycle. Existing Escape handler closes Extensions, but its catch-all consumed RedrawRequested whenever pointer was inside the panel, preventing painting and accessibility publication. Unhandled lifecycle events now reach the shell.

Verification: static only; builds/tests deferred. Find/Replace and utility Escape handlers already exist; those observations remain unconfirmed until combined native verification. No redesign.

Native follow-up: a second legacy default-keymap dispatch still ran before focused Find handling. Escape resolved editor.selection.escape (Keep Primary Selection) and returned before FindClose. The fallback now yields while Find has focus; the earlier scoped keymap continues routing permitted field commands. No build/test by implementation agent.
