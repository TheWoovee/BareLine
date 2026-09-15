# QA-FIX-011 — language picker uses active theme

Authority: inspected main-editor-light.png, UI shared chrome and PR-012 theme tokens. Language picker directly used fixed dark constants and default dark list styling. Add a themed draw entry point and pass current settings theme from native composition. Layout and controls unchanged.

Verification: static only, builds/tests deferred. Other custom-panel theme gaps and UIA focus require follow-up; this fixes the concrete picker palette mismatch.

Theme follow-up: Column Editor text fields, Extensions controls/text fields (reference inspected), Document List, Outline, Workspace tree and Document Map now receive current settings/workspace tokens via themed draw entry points. Existing default draw APIs remain available to callers. No layout redesign; no builds/tests executed.
