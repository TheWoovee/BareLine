# QA-FIX-025 — monitoring unlock confirmation

Authority: monitor interaction keeps the tab open and captures a fixed generation for editing. The unlock action reused a discard-and-close-document question despite neither discarding edits nor closing the tab. Add purpose-specific native confirmation copy and use it for unlocking monitoring.

Verification: static only; builds/tests deferred. Native confirmation pending.
