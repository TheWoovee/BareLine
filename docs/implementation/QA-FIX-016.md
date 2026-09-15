# QA-FIX-016 — show tabs at their actual hit-test origin

Scope: ISSUE-016 and ISSUE-018. Authority: inspected main-editor-dark.png for ISSUE-001; shared tab strip composition. Native composition clipped off view-owned tabs by beginning the outer clip 34px below editor origin, leaving fallback shell tabs painted at unrelated coordinates. Include the actual tab strip in the translated editor clip, matching tabs_event's editor-relative hit testing and toolbar/sidebar offsets.

Verification: static only; builds/tests deferred. Pending toolbar toggles and workspace sidebar tab clicks at native dimensions. No tab redesign.
