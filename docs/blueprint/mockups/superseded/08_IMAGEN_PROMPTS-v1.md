# Bareline Image Generation Briefs

**Version:** 1.2 • 2026-09-05. Filename retained for compatibility. These briefs may be used with ChatGPT image generation or Imagen; this revision used **ChatGPT image generation only**. [Actual prompts and provenance](mockups/GENERATION_LOG.json) identify the outputs.

## Common generation contract

Use-case: ui-mockup. Generate one flat Windows 11 screenshot at a logical 1600×900 viewport, 16:9; no device/perspective. Actual raster dimensions must be measured and recorded. Use the authoritative [brand tokens](00_BRAND_AND_LOGO.md) and [token JSON](mockups/design-tokens.json): dark editor #1F2328, chrome #181B1F, elevated #262B31, text #E6E8EA, muted/comment/gutter #9AA3AD, focus #2ED3C4; light editor #FAF8F5, chrome #F1EEE9, elevated #FFFFFF, text #23272B, muted/comment/gutter #5C6570 and focus/accent text #0B7A6F. Decorative light accent #14A89A is not the functional focus ring.

Segoe UI 13 px UI and Cascadia Mono 12 pt editor, with consistent spacing. Distinct Windows title bar, full twelve-menu row, optional explicitly enabled toolbar, then tabs. Pinned tabs first. Every split pane has its own file label. Six-part status reflects the actual active editor or neutral placeholders. Sample line numbers, content, counts and outline must agree. No terminal/source-control panels or unsupported product controls. User file text is not an application dependency declaration.

Render focus, dirty/read-only states and non-color status markers. Use small exact sample strings; prefer four correct code lines to dense fabricated code. Static PNGs do not replace component definitions or accessible interaction testing.

## Current screen briefs

| Reference | Required state and invariant |
|---|---|
| main-editor-dark / main-editor-light | Same editor layout and sample; pinned-first tabs, correct caret/status and no invented features |
| workspace-split | main.rs selected; Rust main outline; second pane config.rs explicitly labelled |
| find-bar | Mode, scope, case/word/wrap; include invalid regex, no-results and expanded replace variants when producing a full set |
| search-panel | Grouped results, clear counts/completeness, close/resize and cancel |
| settings | User/Workspace scope, 12 pt units, inline validation and saved/error feedback |
| extensions | Runtime absent/installed lifecycle, per-extension capabilities, explicit Remove runtime and verified offline install |
| huge-log | Coherent generated fixture counts, indexing unknown until known, complete menus |
| compare-dark | Two source labels, directional copy, dirty destination, current hunk and aligned unnumbered spacers |
| compare-options | Whitespace mode enum plus case/blank/EOL/BOM/tabs options; theme controls separate |
| recovery-center | Complete/Edits only/Corrupt tail, recovered-copy action, safe discard and original unchanged |
| command-palette | Relevant results, disabled reasons/no-match, focus restoration and full chrome |
| tail-mode | Following vs paused scroll vs unlocked fixed generation, rotation state |
| source-changed | Missing regions, last durable protection, normal-save disabled and export with gap report |
| replace-preview | Preview fingerprints, changed-file exclusion and exact eligible Apply count |

Full action/state definitions are in [UI interaction specification](11_UI_INTERACTION_SPEC.md). Exact executed prompts for the seven revised/new images are in mockups/prompts; refinement prompts are preserved with the initial brief.

## Brand briefs and production boundary

Retain the monoline b/document direction. Generate separate light/dark variants from a common chosen geometry, with the interior text-line motif and consistent fold/notch. Logo exploration and icon-size sheets are concepts; production requires SVG and an optical 16/20 px variant plus nine ICO sizes. A Windows hero must use approved Windows chrome and avoid numerical performance claims without measurement. Existing brand images are retained with their original scores; no vector or ICO is fabricated as completed work.

## Quality checklist

Review menus and Windows controls, context-consistent tabs/outline, numerical/line alignment, visible primary actions, source/destination labels, valid progress/completeness, focus/non-color indicators, font units/token contrast, scope fidelity and recorded provenance. Reject unsafe behavioral wording. Record residual raster defects rather than calling them ship-ready. Preserve originals before replacement under mockups/superseded with an unused version suffix.

[Historical v1.1 prompts](mockups/superseded/08_PROMPTS_v1.1.md) are retained solely for audit and may contain superseded instructions.
