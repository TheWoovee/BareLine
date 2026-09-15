# QA-FIX-024 — Output yields to visible overlays

Scope: ISSUE-024 print click interception and related ISSUE-028 folder controls. Cause: macros_event's underlying Output hit test ran before utilities/folder handlers and consumed clicks in the bottom200px even when those overlays were painted above Output. Yield Output interaction whenever a higher overlay owns input; preserve Macro Manager's separate modal handling.

Verification: static event-order review only; builds/tests deferred. Existing print/output artifact regression and lower button clicks pending native verification.

Paint-order follow-up: Output now paints immediately after base shell, before editor compare options, folder controls and Column Editor. Macro Manager remains in its existing overlay pass. This removes Output overpainting of lower controls; matches the new hit-test yielding.
