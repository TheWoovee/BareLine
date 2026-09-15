# QA-FIX-003 — live global status footer

Authority: inspected main-editor-dark mock for ISSUE-001; UI-ED requires active document/caret status. Cause: shell paints placeholder status at the window bottom, while live editor status is drawn above the reduced Output area. Keep existing status labels and positioning, project the active editor's generated labels into the global footer after editor clipping.

Verification: static source review only. No builds/tests/native execution per user instruction. Pending: typing, navigation, Output visibility and split active pane status.
