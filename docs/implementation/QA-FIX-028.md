# QA-FIX-028 — search close/cancel targets active folder UI

Authority: search interaction close/cancel contract. Shared search.close_panel/search.cancel_panel dispatch previously affected only open-document results, leaving active folder controls open. Route those commands to close folder configuration first and cancel the workspace search. ISSUE-012 shortcut routing also prevents Tab/clipboard document edits while folder controls own input.

Verification: static only; builds/tests deferred. Lower-control clipping and native Alt+F4/modal attribution remain pending.

Output paint ordering corrected in QA-FIX-024 so folder lower controls remain visible above the persistent pane.
