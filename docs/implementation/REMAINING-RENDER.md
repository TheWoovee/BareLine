# Remaining rendering follow-up — frozen source handoff

Date: 2026-09-08. Scope: native retest 019 and the additional gutter/Output observations in docs/qa/RETEST-NATIVE-20260908.md.

## Changes

- Compare pane tab strips retain all document tabs and keep the active source within the narrower pane's visible tab range. Previously the offset could show only earlier unrelated documents while the source itself was offscreen. The main strip and switching contract remain intact.
- Compare Options is sized and centered within the editor area above its reserved footer; the old center could place the bottom twelve pixels outside the Output-adjusted clip. Both compare-dark and compare-options reference images were inspected. Native raster parity remains pending.
- EditorSurface now derives a shared text inset from source line digits and font size. Source anchors account for six-digit paged viewports without full-document indexing. Painting, folding targets, pointer hits, rectangular hit testing, accessibility geometry, horizontal anchoring, and compare decoration use that same inset. Width-sensitive layouts invalidate when the inset changes.
- Coordinated shell integration in windows_app.rs (owned by the input agent): scrollbars now paint before compare/recovery overlays; global scrollbar hit geometry is preserved while draw operations join the translated editor layer. Output paints after editor operations and the editor-local footer is clipped when Output is open. Global status remains visible. This prevents Output's header being obscured by the duplicate internal footer.

## Evidence and remaining acceptance

Source tracing and scoped git diff --check completed. No build, test executable, or native UI was run by this delegated pass. No native pass is claimed. Existing local fixes were preserved; no commits made.

Native acceptance: open compare-left/right after many earlier tabs, confirm each pane shows its selected source, open Options with Workspace/toolbar and Output visible, inspect all panel edges and ensure no scrollbar overlays it. Scroll the large fixture into six-digit lines and verify gutter separation, caret hit position, folding and UIA bounds. Run a command and verify the Output header/results occupy the lower pane with exactly one global footer.

Files owned here: windows_app/views.rs, windows_app/compare.rs, editor-surface/lib.rs, editor-surface/view_geometry.rs, editor-surface/paged_view.rs, editor-surface/power/consumer.rs. No edits to macros.rs were needed. Shell integration reviewed in windows_app.rs after input-agent changes.

Combined-suite follow-up: the paged global-scroll regression used fixed x=74/89, now inside the widened gutter for its 40000-line fixture. Updated those two clicks to the view text inset plus the existing offsets; all selection/scroll/source assertions retained. No test was run by this owner.
