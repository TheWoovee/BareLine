# QA-FIX-004 — modified navigation

Scope: ISSUE-004. Authority: PR-003 editor input/selection and standard Ctrl navigation. Native key translation discarded Ctrl for arrows and Home/End. Add explicit document-edge and word navigation inputs and preserve Shift extension. Paged document edges use global coordinates; word movement uses bounded available text and keeps indexing limitations explicit.

Verification: static only; builds/tests deferred by user. Pending native modified navigation and paged boundary cases.

Limitation: word navigation remains bounded by available text; distant boundaries report unavailable rather than scanning the document on the UI thread. Macro recording preserves new navigation through existing edit.move_* IDs with optional control=true; older records retain their original behavior. Full paged word-boundary crossing remains pending verification.
