# QA-FIX-019 — compare labels and options clipping

Inspected compare-dark reference. Static tracing: compare_pair installs requested document IDs in pane controller, and draw_tab_strip resolves each tab.document_id through document_index to workspace.titles. Outer editor clip formerly removed those view-owned labels and showed unrelated shell fallback tabs. QA-FIX-016 exposes actual pane labels and options top area. QA-FIX-024 paints persistent Output beneath compare options, preventing lower overlap. No alternate label mapping invented.

Verification: static only. Compare source labels/options at native window dimensions remain pending combined verification; no exact raster parity claim.
