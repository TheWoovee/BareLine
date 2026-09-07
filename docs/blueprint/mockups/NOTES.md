# Mockup Review Notes: v1.3

**Date:** 2026-09-05

Inspect generated outputs as visual references. Use image_gen.imagegen for the eight screen/hero changes; preserve three initial attempts before refinement. Retain the former compare-dark bytes as bareline-compare-merge.png. Restore the archived five-state composition through a menu insertion and a targeted red-line correction.

## Current images

| Image | Score / 10 | Reason |
|---|---|---|
| [bareline-compare-merge.png](bareline-compare-merge.png) | 8.5 | Directional copy controls and the dirty target make this useful as the merge-affordance reference; use compare-dark for the five semantic states. |
| [bareline-settings.png](bareline-settings.png) | 9 | Every setting row now shows its key, scope and pt units remain visible, and Reset section plus six neutral footer groups resolve the Settings-specific defects. |
| [bareline-workspace-split.png](bareline-workspace-split.png) | 8.5 | Both Rust panes now show over 30 numbered lines, their own labels and divider sync control, but generated tree spacing still does not establish an exact 28 px layout. |
| [bareline-extensions.png](bareline-extensions.png) | 9 | Installed/Discover/Updates/Disabled, the isolation banner, runtime removal and per-extension Permissions are visible with Bareline project publishers and readable capabilities. |
| [bareline-hero.png](logo-brand/bareline-hero.png) | 9 | Windows controls, all twelve menus, the standard six tabs and sequential 1-24 lines replace the macOS chrome while leaving only the tagline outside the window. |
| [bareline-recovery-center.png](bareline-recovery-center.png) | 8.5 | The title is Bareline and the unrelated greeting tab is replaced with notes.txt while Complete, Edits only and Corrupt tail actions remain legible. |
| [bareline-source-changed.png](bareline-source-changed.png) | 8.5 | The command-icon row is removed while missing-region hatching, recovery protection time and export-gap messaging remain intact; title normalization is outside this requested edit. |
| [bareline-replace-preview.png](bareline-replace-preview.png) | 9 | The command-icon row is removed while changed-since-preview exclusion and Apply to 2 unchanged files remain visible; the existing title is retained by the edit-only constraint. |
| [bareline-compare-dark.png](bareline-compare-dark.png) | 8.5 | The restored reference shows all five diff states and the complete menu; compare counts and alignment remain illustrative rather than verified fixture evidence. |
| [bareline-icon-16.png](logo-brand/bareline-icon-16.png) | 8 | The 16 px plain b retains an open bowl without fold, notch or text lines, but native-size human approval is still pending. |
| [bareline-icon-20.png](logo-brand/bareline-icon-20.png) | 8 | The 20 px plain b keeps the simplified silhouette for fractional Windows scaling without adding crowded interior details. |
| [bareline-icon-24.png](logo-brand/bareline-icon-24.png) | 7.5 | The 24 px mark includes the two interior lines and accent cut, although the small fold remains visually dense. |
| [bareline-icon-32.png](logo-brand/bareline-icon-32.png) | 8 | The 32 px raster separates the two text bars while preserving the full-height stem and diagonal accent. |
| [bareline-icon-40.png](logo-brand/bareline-icon-40.png) | 8 | The 40 px raster preserves the bowl opening and both text bars at the 250% small-icon scale. |
| [bareline-icon-48.png](logo-brand/bareline-icon-48.png) | 8.5 | The 48 px raster shows the fold and cyan corner distinctly while retaining the plain left stem. |
| [bareline-icon-64.png](logo-brand/bareline-icon-64.png) | 8.5 | The 64 px raster makes both interior lines readable with clean rounded tile edges. |
| [bareline-icon-128.png](logo-brand/bareline-icon-128.png) | 9 | The 128 px raster clearly separates the folded corner, two text lines and accent cut on the dark tile. |
| [bareline-icon-256.png](logo-brand/bareline-icon-256.png) | 9 | The 256 px raster exposes the adopted two-line construction cleanly enough to inspect the stroke and corner geometry. |

## Retained score corrections

Keep the reduction from 9: the mandatory menu bar is absent and 241 million long log lines are inconsistent with the displayed 3.8 GB fixture size.

Keep the reduction from 9: the window omits menu/status chrome and places competing whitespace checkboxes inside Colors instead of the FC-06 whitespace enum in General.

## Remaining image limits

Workspace: the regenerated tree/outline still has approximately 36 px raster row spacing at a declared 1600x1000 logical viewport; exact 28 px density remains unresolved after two targeted density refinements; preserve the rejected attempt in the generation log. Treat item 18 as partial for this one requirement. Both panes exceed 30 lines and show their own labels and sync control.

Source-changed and replace-preview: preserve existing title strings and placeholder icons because item 18 limits these two edits to removal of the command-icon row. Item 17 governs new compositions; do not silently broaden these two edits. Recovery retains its pre-existing illustrative footer values under the same narrow content-preservation approach.

## Human icon review

The SVGs are hand-authored on the 24-unit grid. resvg-py 0.5.0 renders the nine PNG sizes; Pillow 12.3.0 supplies PNG frame encoding for ICO assembly. Install with `python -m pip install resvg-py==0.5.0 Pillow==12.3.0`; run `python scripts/render_icons.py`. Inspect the actual [16 px raster](logo-brand/bareline-icon-16.png) at native size.

Human viewed 16 px: **PENDING**. Human approved 16 px: **PENDING**. ICO status: **DRAFT**, not final. Agent visual inspection does not satisfy human approval. Record reviewer, date and result here before finalizing. No prototype, application test or benchmark ran in this revision.
