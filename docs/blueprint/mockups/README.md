# Bareline Mockup Index

**Version:** 1.3 · **Date:** 2026-09-05

Use compare-dark for five diff states and compare-merge for directional merge affordances. Keep both references. Follow [interaction rules](../11_UI_INTERACTION_SPEC.md) for behavior. Generated images are concepts, not application evidence.

| File | Score / 10 | File-specific reason or role |
|---|---|---|
| [bareline-command-palette.png](bareline-command-palette.png) | 8 | The palette shows command titles, menu paths and shortcut hints; disabled and no-match behavior still requires the interaction specification. |
| [bareline-compare-dark.png](bareline-compare-dark.png) | 8.5 | The restored reference shows all five diff states and the complete menu; compare counts and alignment remain illustrative rather than verified fixture evidence. |
| [bareline-compare-merge.png](bareline-compare-merge.png) | 8.5 | Directional copy controls and the dirty target make this useful as the merge-affordance reference; use compare-dark for the five semantic states. |
| [bareline-compare-options.png](bareline-compare-options.png) | 7 | Keep the reduction from 9: the window omits menu/status chrome and places competing whitespace checkboxes inside Colors instead of the FC-06 whitespace enum in General. |
| [bareline-extensions.png](bareline-extensions.png) | 9 | Installed/Discover/Updates/Disabled, the isolation banner, runtime removal and per-extension Permissions are visible with Bareline project publishers and readable capabilities. |
| [bareline-find-bar.png](bareline-find-bar.png) | 8.5 | The docked inline find row preserves editing context and visible match navigation; error and incomplete states remain specified separately. |
| [bareline-huge-log.png](bareline-huge-log.png) | 7.5 | Keep the reduction from 9: the mandatory menu bar is absent and 241 million long log lines are inconsistent with the displayed 3.8 GB fixture size. |
| [bareline-main-editor-dark.png](bareline-main-editor-dark.png) | 8.5 | The dark text-first composition retains six editor tabs and a dominant code surface; use the current chrome spec for implementation. |
| [bareline-main-editor-light.png](bareline-main-editor-light.png) | 8 | The warm light editor provides the paired theme composition; apply the corrected functional contrast tokens when implementing it. |
| [bareline-recovery-center.png](bareline-recovery-center.png) | 8.5 | The title is Bareline and the unrelated greeting tab is replaced with notes.txt while Complete, Edits only and Corrupt tail actions remain legible. |
| [bareline-replace-preview.png](bareline-replace-preview.png) | 9 | The command-icon row is removed while changed-since-preview exclusion and Apply to 2 unchanged files remain visible; the existing title is retained by the edit-only constraint. |
| [bareline-search-panel.png](bareline-search-panel.png) | 8 | The grouped results panel demonstrates file hierarchy and docked search placement; displayed counts remain illustrative. |
| [bareline-settings.png](bareline-settings.png) | 9 | Every setting row now shows its key, scope and pt units remain visible, and Reset section plus six neutral footer groups resolve the Settings-specific defects. |
| [bareline-source-changed.png](bareline-source-changed.png) | 8.5 | The command-icon row is removed while missing-region hatching, recovery protection time and export-gap messaging remain intact; title normalization is outside this requested edit. |
| [bareline-tail-mode.png](bareline-tail-mode.png) | 8 | The follow banner distinguishes scroll pause and unlock actions; source-generation continuity remains a contract to implement. |
| [bareline-workspace-split.png](bareline-workspace-split.png) | 8.5 | Both Rust panes now show over 30 numbered lines, their own labels and divider sync control, but generated tree spacing still does not establish an exact 28 px layout. |
| [design-tokens.json](design-tokens.json) | Not scored | Retain functional theme colors used by the contrast checks. |
| [GENERATION_LOG.json](GENERATION_LOG.json) | Not scored | Record actual tools, measured dimensions, prompts and preserved replacements. |
| [NOTES.md](NOTES.md) | Not scored | Record image-specific limits and the pending human icon review. |
| [README.md](README.md) | Not scored | Index every file in this directory, including metadata documents. |

## Brand assets

Use [hand-authored mark](logo-brand/bareline-mark.svg), [dark](logo-brand/bareline-mark-dark.svg), [light](logo-brand/bareline-mark-light.svg), [small-size source](logo-brand/bareline-mark-16.svg) and [draft ICO](logo-brand/bareline.ico). Keep earlier PNG brand explorations as historical direction. The new [Windows hero](logo-brand/bareline-hero.png) replaces the macOS composition. See [notes](NOTES.md) for optical review status.

Actual image generator: image_gen.imagegen. Vector renderer: resvg-py 0.5.0. Preserve previous images, documents and prompts under superseded with unused version suffixes. Measure raster dimensions from each PNG header in [GENERATION_LOG.json](GENERATION_LOG.json); logical density is not a pixel-exact guarantee.
