# PR-U13 — Distinguish estimated line numbers while large files index

Common delivery rules: [IMPLEMENTOR_CONTRACT.md](../IMPLEMENTOR_CONTRACT.md).

## Problem and resulting behavior

Large-file gutters can show estimated global line numbers while indexing, but the current cue is only a faded color. Users can read an estimate as exact, and color alone is weak in high contrast or low vision conditions.

## Scope

- `crates/editor-surface/src/lib.rs` gutter painting
- `crates/editor-surface/src/paged_view.rs` exact/estimated state
- renderer font-face/style support or a tooltip/fallback marker
- status/accessibility description

## Ordered implementation

1. Carry an explicit exact/estimated flag with each painted gutter label.
2. Render estimates with italic face when supported plus a non-color cue; otherwise append a compact approximation marker and tooltip.
3. Describe the active range as estimated through accessibility while indexing, then announce the transition to exact once.
4. Invalidate only affected gutter/status regions as indexing advances.

## Tests and acceptance

- A deterministic incomplete index paints and describes estimates; completion switches the same rows to exact labels.
- Light/dark/high-contrast and 100/150/200% DPI remain legible and aligned.
- Font fallback lacking italic uses the documented marker/tooltip path.

## Dependencies and risks

Coordinate with renderer font-style support. Do not change the already-correct global number calculation or byte-based seek behavior.

## Definition of done

Users can distinguish estimated from exact gutter numbers without relying on color or source knowledge.

Status: Planned. Priority: P3 design gap. Finding: [UX-11](findings.md). [Native coverage](coverage.md) · [Full audit evidence](../TEST_REPORT.md).
